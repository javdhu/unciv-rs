use civ_core::ruleset::Ruleset;
use civ_core::world::assignment::CityFocus;
use civ_core::world::city::City;
use civ_core::world::entities::Civilization;
use civ_core::world::grid::{CivId, TileGrid};
use civ_core::world::religious_units::ReligiousEvent;
use civ_core::world::turn::{advance_turn, update_visibility, TurnEvent, TurnModifiers};
use civ_core::world::world::World;

#[test]
fn stored_faith_reaches_prophets_enhancement_and_missionaries_in_actual_turns() {
    let r = Ruleset::load_dir("rules/gods-and-kings").unwrap().unwrap();
    let mut w = World::new(
        TileGrid::new(22, 10, r.terrain("Grassland").unwrap()),
        vec![Civilization::new(CivId(0), r.nations.id("Rome").unwrap(), true)],
    );
    let home = w.grid.id_at(3, 5).unwrap();
    let away = w.grid.id_at(15, 5).unwrap();
    let capital = w.found_city_in(&r, City::found("Home", CivId(0), home));
    let distant = w.found_city_in(&r, City::found("Distant", CivId(0), away));
    for (city, tile) in [(capital, home), (distant, away)] {
        for t in w.grid.within(tile, 2) {
            w.claim_tile(city, t);
        }
    }
    w.civ_mut(CivId(0)).unwrap().faith.stored = 1500;
    update_visibility(&mut w);
    let mut births = 0;
    let mut founded = false;
    let mut enhanced = false;
    let mut bought_missionary = false;
    let mut missionary_spread = false;
    for _ in 0..25 {
        for event in advance_turn(&mut w, &r, CityFocus::None, TurnModifiers::default()) {
            match event {
                TurnEvent::ProphetBorn { .. } => births += 1,
                TurnEvent::ReligionFounded { .. } => {
                    founded = true;
                    let city = w.cities.get(distant).unwrap();
                    assert_eq!(
                        city.faith.majority(city.population),
                        None,
                        "founding does not convert the distant pantheon"
                    );
                    assert_eq!(city.faith.majority_pantheon(city.population), Some(CivId(0)));
                }
                TurnEvent::ReligionEnhanced { .. } => enhanced = true,
                TurnEvent::FaithPurchased { what: civ_core::world::construction::Construction::Unit(base), .. }
                    if r.units.get(base).unwrap().name == "Missionary" =>
                {
                    bought_missionary = true
                }
                TurnEvent::ReligiousUnitActed { event: ReligiousEvent::Spread { city, pressure, .. }, .. }
                    if city == distant && pressure > 0 =>
                {
                    missionary_spread = true
                }
                _ => {}
            }
        }
        if missionary_spread {
            break;
        }
    }
    assert!(births >= 2, "two physical prophets are needed, got {births}");
    assert!(founded, "the first prophet must found a religion");
    assert!(enhanced, "the second prophet must enhance it");
    assert!(bought_missionary, "the AI must spend faith on an actual missionary");
    assert!(missionary_spread, "a purchased missionary must reach and preach in the distant city");
    assert_eq!(
        w.cities.get(distant).unwrap().faith.majority(w.cities.get(distant).unwrap().population),
        Some(CivId(0))
    );
}

#[test]
fn pantheon_cities_keep_the_original_benefits_until_the_full_religion_arrives() {
    use civ_core::stats::Fixed;
    use civ_core::world::{city, religion};
    let r = Ruleset::load_dir("rules/gods-and-kings").unwrap().unwrap();
    let mut w = World::new(
        TileGrid::new(24, 8, r.terrain("Desert").unwrap()),
        vec![Civilization::new(CivId(0), r.nations.id("Rome").unwrap(), true)],
    );
    let home = w.grid.id_at(3, 4).unwrap();
    let away = w.grid.id_at(19, 4).unwrap();
    let capital = w.found_city_in(&r, City::found("Home", CivId(0), home));
    let distant = w.found_city_in(&r, City::found("Distant", CivId(0), away));
    let pantheon = r.beliefs.id("Desert Folklore").unwrap();
    w.civ_mut(CivId(0)).unwrap().faith.beliefs.insert(pantheon);
    religion::on_pantheon_founded(&mut w, CivId(0));
    let before = city::city_yield_in(&w, &r, w.cities.get(distant).unwrap(), false);
    let follower = r.beliefs.id("Feed the World").unwrap();
    let faith = &mut w.civ_mut(CivId(0)).unwrap().faith;
    faith.beliefs.insert(follower);
    faith.founded = true;
    let city = w.cities.get_mut(capital).unwrap();
    city.faith.holy_city_of = Some(CivId(0));
    city.faith.add_pressure(Some(CivId(0)), 500);
    city.faith.recount(city.population);
    let city = w.cities.get(distant).unwrap();
    assert_eq!(city.faith.majority(city.population), None);
    assert_eq!(city.faith.majority_pantheon(city.population), Some(CivId(0)));
    assert_eq!(city::city_yield_in(&w, &r, city, false), before);
    assert!(before.faith >= Fixed::ONE, "the original Desert Folklore still applies");
    let old_effects = religion::city_belief_uniques(&w, &r, city);
    assert_eq!(old_effects, r.beliefs.get(pantheon).unwrap().uniques.iter().collect::<Vec<_>>());
    religion::spread(&mut w, &r, 6);
    assert_eq!(
        w.cities.get(distant).unwrap().faith.pressure_of(Some(CivId(0))),
        0,
        "a remote pantheon does not become a religion or emit full-religion pressure"
    );
    let city = w.cities.get_mut(distant).unwrap();
    city.faith.add_pressure(Some(CivId(0)), 1000);
    city.faith.recount(city.population);
    let effects = religion::city_belief_uniques(&w, &r, w.cities.get(distant).unwrap());
    assert!(r.beliefs.get(follower).unwrap().uniques.iter().all(|u| effects.contains(&u)));
}
