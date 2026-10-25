// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The facade builds a game, keeps the fog boundary, and survives turns — natively, no browser.

use civ_sim::{advance, new_game, player_view, reachable, tables, Settings};

#[test]
fn a_game_starts_and_plays_twenty_turns() {
    let (mut w, r, me) = new_game(&Settings::default()).expect("game");
    let v0 = player_view(&w, &r, me);
    assert!(!v0.tiles.is_empty(), "the start should be explored");
    assert!(
        v0.units.iter().any(|u| u.owner == me.0),
        "the player should have units"
    );
    assert!(
        v0.tiles.len() < (v0.width as usize * v0.height as usize),
        "fog should hide part of the map"
    );

    for _ in 0..20 {
        advance(&mut w, &r);
    }
    let v = player_view(&w, &r, me);
    assert_eq!(v.turn, v0.turn + 20);
    assert!(
        v.cities.iter().any(|c| c.owner == me.0),
        "the AI should have founded a city for the player"
    );
}

#[test]
fn only_the_player_sees_its_own_economy() {
    let (w, r, me) = new_game(&Settings::default()).expect("game");
    let v = player_view(&w, &r, me);
    for c in &v.civs {
        assert_eq!(c.gold.is_some(), c.id == me.0);
    }
}

#[test]
fn reachable_answers_for_own_units_only() {
    let (w, r, me) = new_game(&Settings::default()).expect("game");
    let v = player_view(&w, &r, me);
    let mine = v.units.iter().find(|u| u.owner == me.0).unwrap();
    assert!(!reachable(&w, &r, me, mine.id, mine.gen).is_empty());
    let theirs = w
        .units
        .iter()
        .find(|(_, u)| u.owner != me)
        .map(|(id, _)| id)
        .unwrap();
    assert!(reachable(&w, &r, me, theirs.index(), theirs.generation()).is_empty());
}

#[test]
fn a_seed_always_deals_the_same_game() {
    let a = new_game(&Settings {
        seed: 42,
        ..Settings::default()
    })
    .unwrap();
    let b = new_game(&Settings {
        seed: 42,
        ..Settings::default()
    })
    .unwrap();
    let (va, vb) = (player_view(&a.0, &a.1, a.2), player_view(&b.0, &b.1, b.2));
    assert_eq!(format!("{va:?}"), format!("{vb:?}"));
    assert!(!tables(&a.1).terrains.is_empty());
}
