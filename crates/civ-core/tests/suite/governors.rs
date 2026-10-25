// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What the engine keeps running for a human player between their own
//! choices: a puppet's governor, and the record of the eras they reached.
use civ_core::arena::Id;
use civ_core::ruleset::Ruleset;
use civ_core::world::construction::Construction;
use civ_core::world::turn::{finish_round, update_visibility, TurnModifiers};
use civ_core::world::{City, CityFocus, CivId, Civilization, TileGrid, World};

/// Two human civs, Rome and Antium, each with a city and its first ring.
fn fixture() -> (Ruleset, World, Id<City>, Id<City>) {
    let r = Ruleset::load_dir("rules/gods-and-kings").unwrap().unwrap();
    let nation = r.nations.id("Rome").unwrap();
    let civs = vec![Civilization::new(CivId(0), nation, false), Civilization::new(CivId(1), nation, false)];
    let mut w = World::new(TileGrid::new(24, 18, r.terrain("Grassland").unwrap()), civs);
    for c in &mut w.civs {
        c.techs.insert(r.technologies.id("Agriculture").unwrap());
    }
    let rome = w.found_city_in(&r, City::found("Rome", CivId(0), w.grid.id_at(4, 8).unwrap()));
    let antium = w.found_city_in(&r, City::found("Antium", CivId(1), w.grid.id_at(18, 8).unwrap()));
    for city in [rome, antium] {
        let centre = w.cities.get(city).unwrap().centre;
        for tile in w.grid.within(centre, 1) {
            w.claim_tile(city, tile);
        }
    }
    update_visibility(&mut w);
    civ_core::world::fog::capture_observations(&mut w, &r);
    (r, w, rome, antium)
}

fn round(w: &mut World, r: &Ruleset) {
    finish_round(w, r, CityFocus::None, TurnModifiers::for_world(w, r));
}

#[test]
fn a_humans_puppet_with_nothing_to_build_is_given_a_building() {
    let (r, mut w, _, _) = fixture();
    // Veii, a second Antium city, falls to Rome and is kept as a puppet.
    let veii = w.found_city_in(&r, City::found("Veii", CivId(1), w.grid.id_at(12, 8).unwrap()));
    w.claim_tile(veii, w.grid.id_at(12, 8).unwrap());
    w.cities.get_mut(veii).unwrap().population = 3;
    civ_core::world::combat::capture_city(&mut w, &r, veii, CivId(0));
    let city = w.cities.get(veii).unwrap();
    assert!(city.puppet && city.owner == CivId(0) && city.queue.is_empty());
    round(&mut w, &r);
    let queue = &w.cities.get(veii).unwrap().queue;
    assert!(matches!(queue.first(), Some(Construction::Building(_))), "the governor builds: {queue:?}");
    let Some(Construction::Building(b)) = queue.first() else { unreachable!() };
    assert!(r.buildings[*b].is_wonder != Some(true), "a puppet builds no wonders");

    // A human's own city with nothing to build stays the player's to choose.
    let rome = w.civ(CivId(0)).unwrap().cities[0];
    assert!(w.cities.get(rome).unwrap().queue.is_empty());
}

#[test]
fn the_eras_a_civ_reached_are_kept_by_the_turn_it_reached_them() {
    let (r, mut w, _, _) = fixture();
    assert_eq!(civ_core::world::policies::era_starts(&w, &r, CivId(0)), vec![0], "the first era, from the start");
    round(&mut w, &r);
    round(&mut w, &r);
    assert_eq!(w.civ(CivId(0)).unwrap().era_starts, vec![0]);
    // A classical technology moves the civ into the second era on turn 2.
    let classical = r
        .technologies
        .iter()
        .find(|(_, t)| t.era(&r).and_then(|e| r.eras.get(e)).is_some_and(|e| e.era_number == 1))
        .map(|(id, _)| id)
        .unwrap();
    w.civ_mut(CivId(0)).unwrap().techs.insert(classical);
    assert_eq!(civ_core::world::policies::era_starts(&w, &r, CivId(0)), vec![0, 2], "reached this turn");
    round(&mut w, &r);
    round(&mut w, &r);
    assert_eq!(w.civ(CivId(0)).unwrap().era_starts, vec![0, 2], "recorded, and never moved again");
    // Jumping ahead records every era passed on the same turn.
    let modern = r
        .technologies
        .iter()
        .find(|(_, t)| t.era(&r).and_then(|e| r.eras.get(e)).is_some_and(|e| e.era_number == 3))
        .map(|(id, _)| id)
        .unwrap();
    w.civ_mut(CivId(0)).unwrap().techs.insert(modern);
    round(&mut w, &r);
    assert_eq!(w.civ(CivId(0)).unwrap().era_starts, vec![0, 2, 4, 4]);
}
