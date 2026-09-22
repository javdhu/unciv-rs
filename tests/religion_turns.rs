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
    let r = Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings").unwrap().unwrap();
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
                TurnEvent::ReligionFounded { .. } => founded = true,
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
