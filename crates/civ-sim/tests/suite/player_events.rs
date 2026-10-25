// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Detailed new events must not bypass the player view's fog boundary.

use civ_core::arena::Registry;
use civ_core::fingerprint::embedded_sources;
use civ_core::ruleset::{RuinReward, Ruleset};
use civ_core::world::city::City;
use civ_core::world::entities::Civilization;
use civ_core::world::grid::TileGrid;
use civ_core::world::religious_units::initialize_unit;
use civ_core::world::turn::update_visibility;
use civ_core::world::{CivId, Unit, World};
use civ_core::Unique;
use civ_sim::{advance, advance_for};

fn separated_empires() -> (World, Ruleset) {
    let mut r = Ruleset::from_sources(&embedded_sources()).unwrap();
    r.ruin_rewards = Registry::new();
    r.ruin_rewards.insert(
        "Inscription",
        RuinReward {
            name: "Inscription".into(),
            uniques: vec![Unique::parse("Gain [17] [Culture]")],
            ..Default::default()
        },
    );
    let civs = ["Rome", "Greece"]
        .into_iter()
        .enumerate()
        .map(|(i, name)| Civilization::new(CivId(i as u8), r.nations.id(name).unwrap(), true))
        .collect();
    let mut w = World::new(TileGrid::new(30, 10, r.terrain("Grassland").unwrap()), civs);
    let pantheons: Vec<_> = r
        .beliefs
        .iter()
        .filter(|(_, b)| b.r#type == "Pantheon")
        .map(|(id, _)| id)
        .collect();
    for (i, x) in [3, 23].into_iter().enumerate() {
        let civ = CivId(i as u8);
        let centre = w.grid.id_at(x, 4).unwrap();
        let city = w.found_city_in(&r, City::found(format!("City {i}"), civ, centre));
        w.civ_mut(civ).unwrap().faith.beliefs.insert(pantheons[i]);
        w.place_unit(Unit::new(r.units.id("Scout").unwrap(), civ, centre, 2));
        let ruin = w.grid.id_at(x + 1, 4).unwrap();
        w.grid
            .set_improvement(ruin, civ_core::world::ruins::ruins_improvement(&r));
        let mut prophet = Unit::new(r.units.id("Great Prophet").unwrap(), civ, centre, 2);
        initialize_unit(&w, &r, &mut prophet, Some(city));
        w.place_unit(prophet);
    }
    update_visibility(&mut w);
    (w, r)
}

#[test]
fn player_report_keeps_own_events_and_omits_unseen_foreign_details() {
    let (mut w, r) = separated_empires();
    let me = CivId(0);
    let foreign_tile = w.civ(CivId(1)).unwrap().cities[0];
    assert!(!w
        .visibility
        .has_explored(me, w.cities.get(foreign_tile).unwrap().centre));
    let mut trusted = w.clone();
    let full = advance(&mut trusted, &r);
    let player = advance_for(&mut w, &r, me);
    assert_eq!(
        w, trusted,
        "filtering reports must not alter the simulation"
    );

    for kind in ["UnitMoved", "RuinsExplored", "ReligiousUnitActed"] {
        assert_eq!(
            full.events.iter().filter(|e| e.kind == kind).count(),
            2,
            "the fixture must exercise both civs: {kind}"
        );
        assert_eq!(
            player.events.iter().filter(|e| e.kind == kind).count(),
            1,
            "the player only receives its own {kind}"
        );
    }
    assert!(
        player
            .events
            .iter()
            .all(|event| !event.text.contains("CivId(")),
        "wire reports are presentation text, not Debug output"
    );
    let own_scout = w.units.iter().find(|(_, u)| u.owner == me).unwrap().0;
    let moved = player
        .events
        .iter()
        .find(|e| e.kind == "UnitMoved")
        .unwrap();
    let payload = serde_json::to_value(&moved.event).unwrap();
    assert_eq!(payload["unit"]["id"], own_scout.index());
    assert_eq!(payload["unit"]["gen"], own_scout.generation());
}
