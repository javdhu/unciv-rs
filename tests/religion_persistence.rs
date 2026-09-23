#![cfg(feature = "save")]
use civ_core::arena::Id;
use civ_core::ruleset::Ruleset;
use civ_core::stats::Fixed;
use civ_core::world::religion::{self, FaithEvent};
use civ_core::world::religious_units::{self, ReligiousAction};
use civ_core::world::{City, CivId, Civilization, TileGrid, TileId, Unit, World};

fn setup() -> (Ruleset, World, Id<City>, Id<City>) {
    let r = Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings").unwrap().unwrap();
    let mut w = World::new(
        TileGrid::new(30, 12, r.terrain("Grassland").unwrap()),
        vec![Civilization::new(CivId(0), r.nations.id("Rome").unwrap(), false)],
    );
    let home = w.found_city_in(&r, City::found("Home", CivId(0), TileId(62)));
    let distant = w.found_city_in(&r, City::found("Distant", CivId(0), TileId(327)));
    for id in [home, distant] {
        w.cities.get_mut(id).unwrap().population = 5;
    }
    assert!(matches!(religion::apply_faith(&mut w, &r, CivId(0), 10, Fixed::ONE), FaithEvent::PantheonFounded { .. }));
    (r, w, home, distant)
}
fn restored(w: &World, r: &Ruleset) -> World {
    let copy = civ_core::save::decode(&civ_core::save::encode(w, r).unwrap(), r).unwrap();
    assert_eq!(w, &copy);
    copy
}

#[test]
fn pantheon_identity_survives_founding_reload_and_later_missionary_spread() {
    let (r, w, home, distant) = setup();
    let mut w = restored(&w, &r);
    let original_beliefs = w.civ(CivId(0)).unwrap().faith.pantheon_beliefs.clone();
    assert!(!original_beliefs.is_empty());
    for id in [home, distant] {
        let c = w.cities.get(id).unwrap();
        assert_eq!(c.faith.majority(c.population), None);
        assert_eq!(c.faith.majority_pantheon(c.population), Some(CivId(0)));
    }
    let tile = w.cities.get(home).unwrap().centre;
    let mut prophet = Unit::new(r.unit("Great Prophet").unwrap(), CivId(0), tile, 2);
    religious_units::initialize_unit(&w, &r, &mut prophet, Some(home));
    let id = w.place_unit(prophet);
    religious_units::perform(&mut w, &r, id, ReligiousAction::FoundReligion).unwrap();
    let target = w.cities.get(distant).unwrap();
    assert_eq!(target.faith.majority(target.population), None);
    assert_eq!(target.faith.majority_pantheon(target.population), Some(CivId(0)));
    assert_eq!(w.civ(CivId(0)).unwrap().faith.pantheon_beliefs, original_beliefs);
    let mut missionary = Unit::new(r.unit("Missionary").unwrap(), CivId(0), target.centre, 2);
    religious_units::initialize_unit(&w, &r, &mut missionary, Some(home));
    let unit = w.place_unit(missionary);
    let mut copy = restored(&w, &r);
    assert_eq!(
        religious_units::perform(&mut w, &r, unit, ReligiousAction::SpreadReligion(distant)),
        religious_units::perform(&mut copy, &r, unit, ReligiousAction::SpreadReligion(distant))
    );
    assert!(w.cities.get(distant).unwrap().faith.pressure_of(Some(CivId(0))) > 0);
    for _ in 0..8 {
        w.turn += 1;
        copy.turn += 1;
        assert_eq!(religion::spread(&mut w, &r, 6), religion::spread(&mut copy, &r, 6));
        assert_eq!(w, copy);
        copy = restored(&copy, &r);
    }
}

#[test]
fn malformed_pantheon_references_pressures_and_combined_counts_are_rejected() {
    let (r, w, city, _) = setup();
    let base = serde_json::to_value(&w.cities.get(city).unwrap().faith).unwrap();
    for (field, value) in [
        ("pantheon_pressure", serde_json::json!([[8, 100]])),
        ("pantheon_pressure", serde_json::json!([[0, -1]])),
        ("pantheon_pressure", serde_json::json!([[0, 2147483647]])),
        ("pantheon_followers", serde_json::json!([[0, 6]])),
    ] {
        let mut invalid = w.clone();
        let mut faith = base.clone();
        faith[field] = value;
        invalid.cities.get_mut(city).unwrap().faith = serde_json::from_value(faith).unwrap();
        assert!(civ_core::save::validate(&invalid, &r).is_err(), "{field}");
    }
    let mut invalid = w.clone();
    let mut faith = base;
    faith["followers"] = serde_json::json!([[0, 3]]);
    faith["pantheon_followers"] = serde_json::json!([[0, 3]]);
    invalid.cities.get_mut(city).unwrap().faith = serde_json::from_value(faith).unwrap();
    assert!(
        civ_core::save::validate(&invalid, &r).is_err(),
        "each count fits individually but their sum exceeds population"
    );
    let follower = r.beliefs.iter().find(|(_, b)| b.r#type == "Follower").unwrap().0;
    let mut invalid = w;
    invalid.civ_mut(CivId(0)).unwrap().faith.beliefs.insert(follower);
    invalid.civ_mut(CivId(0)).unwrap().faith.pantheon_beliefs.insert(follower);
    assert!(civ_core::save::validate(&invalid, &r).is_err());
}
