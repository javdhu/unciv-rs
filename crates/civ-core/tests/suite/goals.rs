// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! What each AI plays to win (docs/AI.md, level 2).

use civ_core::ruleset::Ruleset;
use civ_core::world::goals::{beeline, goal, Goal};
use civ_core::world::{CivId, Civilization, TileGrid, Unit, World};

fn fixture() -> (Ruleset, World) {
    let r = Ruleset::load_dir("rules/gods-and-kings").unwrap().unwrap();
    let nation = r.nations.id("Rome").unwrap();
    let mut w = World::new(
        TileGrid::new(18, 12, r.terrain("Grassland").unwrap()),
        vec![Civilization::new(CivId(0), nation, true), Civilization::new(CivId(1), nation, true)],
    );
    w.endgame.max_turns = 200;
    w.turn = 20;
    (r, w)
}

fn army(w: &mut World, r: &Ruleset, owner: u8, name: &str, n: u16) {
    let base = r.unit(name).unwrap();
    for i in 0..n {
        let at = w.grid.id_at(2 + i, 2 + 4 * u16::from(owner)).unwrap();
        w.place_unit(Unit::new(base, CivId(owner), at, 2));
    }
}

#[test]
fn an_even_start_plays_for_science() {
    let (r, mut w) = fixture();
    army(&mut w, &r, 0, "Warrior", 2);
    army(&mut w, &r, 1, "Warrior", 2);
    assert_eq!(goal(&w, &r, CivId(0)), Goal::Science);
}

#[test]
fn a_much_stronger_army_plays_for_conquest() {
    let (r, mut w) = fixture();
    army(&mut w, &r, 0, "Swordsman", 5);
    army(&mut w, &r, 1, "Warrior", 2);
    assert_eq!(goal(&w, &r, CivId(0)), Goal::Conquest);
    assert_eq!(goal(&w, &r, CivId(1)), Goal::Science, "the weaker side builds");
}

#[test]
fn the_last_stretch_plays_for_score() {
    let (r, mut w) = fixture();
    army(&mut w, &r, 0, "Swordsman", 5);
    army(&mut w, &r, 1, "Warrior", 2);
    w.turn = 185;
    assert_eq!(goal(&w, &r, CivId(0)), Goal::Score);
}

#[test]
fn a_conqueror_researches_toward_a_stronger_soldier() {
    let (r, mut w) = fixture();
    army(&mut w, &r, 0, "Warrior", 1);
    let route = beeline(&w, &r, CivId(0), Goal::Conquest);
    assert!(!route.is_empty());
    let warrior = r.units[r.unit("Warrior").unwrap()].strength.unwrap();
    let unlocks_stronger = r.units.values().any(|b| {
        b.strength.unwrap_or(0).max(b.ranged_strength.unwrap_or(0)) * 4 > warrior * 5
            && b.required_tech.as_ref().and_then(|t| r.technologies.id(t)).is_some_and(|t| route.contains(&t))
    });
    assert!(unlocks_stronger, "the route ends at a clearly stronger unit");
    assert!(beeline(&w, &r, CivId(0), Goal::Score).is_empty());
}
