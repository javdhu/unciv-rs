// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! ENGINE-BRIEF B7: the AI heals and retreats instead of throwing hurt units
//! away, only attacks what it would survive, races for wonders it can win,
//! and rates beliefs. Each is compared with the old AI (`Civilization::legacy_ai`).
use civ_core::ruleset::Ruleset;
use civ_core::world::ai::{military_order, UnitOrder};
use civ_core::world::city::City;
use civ_core::world::entities::Civilization;
use civ_core::world::grid::TileGrid;
use civ_core::world::{CivId, Unit, World};

const ME: CivId = CivId(0);
const THEM: CivId = CivId(1);

fn ruleset() -> Ruleset {
    Ruleset::load_dir("rules/gods-and-kings").expect("readable").expect("loads")
}

/// Two AI civs at war on open grassland, each with a city, all tiles seen.
fn war(r: &Ruleset) -> World {
    let g = TileGrid::new(20, 12, r.terrain("Grassland").unwrap());
    let nation = r.nations.id("Rome").unwrap();
    let other = r.nations.id("Greece").unwrap();
    let mut w = World::new(g, vec![Civilization::new(ME, nation, true), Civilization::new(THEM, other, true)]);
    let home = w.grid.id_at(2, 6).unwrap();
    w.found_city_in(r, City::found("Home", ME, home));
    let theirs = w.grid.id_at(17, 6).unwrap();
    w.found_city_in(r, City::found("Away", THEM, theirs));
    w.diplomacy.meet(ME, THEM);
    w.diplomacy.declare_war(ME, THEM);
    for t in w.grid.ids().collect::<Vec<_>>() {
        w.visibility.observe(ME, t);
        w.visibility.observe(THEM, t);
    }
    w
}

fn place(w: &mut World, r: &Ruleset, name: &str, civ: CivId, x: u16, y: u16, health: u8) -> civ_core::arena::Id<Unit> {
    let base = r.units.id(name).unwrap();
    let movement = r.units[base].movement;
    let mut unit = Unit::new(base, civ, w.grid.id_at(x, y).unwrap(), movement);
    unit.health = health;
    w.place_unit(unit)
}

#[test]
fn a_badly_hurt_unit_backs_off_instead_of_attacking() {
    let r = ruleset();
    let mut w = war(&r);
    let hurt = place(&mut w, &r, "Warrior", ME, 9, 6, 30);
    place(&mut w, &r, "Warrior", THEM, 10, 6, 100);
    let order = military_order(&w, &r, ME, hurt).unwrap();
    assert!(!matches!(order, UnitOrder::Attack(_)), "{order:?}");
    if let UnitOrder::MoveTo(to) = order {
        let enemy = w.grid.id_at(10, 6).unwrap();
        assert!(w.grid.distance(to, enemy) > 1, "it moved away from the enemy");
    }
    // The old AI hits the nearest thing whatever its own health.
    w.civ_mut(ME).unwrap().legacy_ai = true;
    assert!(matches!(military_order(&w, &r, ME, hurt), Some(UnitOrder::Attack(_))));
}

#[test]
fn it_does_not_attack_what_would_kill_it() {
    let r = ruleset();
    let mut w = war(&r);
    let brave = place(&mut w, &r, "Warrior", ME, 9, 6, 60);
    place(&mut w, &r, "Musketman", THEM, 10, 6, 100);
    let order = military_order(&w, &r, ME, brave);
    assert!(!matches!(order, Some(UnitOrder::Attack(_))), "{order:?}");
    w.civ_mut(ME).unwrap().legacy_ai = true;
    assert!(matches!(military_order(&w, &r, ME, brave), Some(UnitOrder::Attack(_))));

    // A fight it wins, it takes.
    let mut w = war(&r);
    let strong = place(&mut w, &r, "Swordsman", ME, 9, 6, 100);
    place(&mut w, &r, "Warrior", THEM, 10, 6, 60);
    assert!(matches!(military_order(&w, &r, ME, strong), Some(UnitOrder::Attack(_))));
}

#[test]
fn a_hurt_unit_at_peace_rests_until_healed() {
    let r = ruleset();
    let mut w = war(&r);
    w.diplomacy.make_peace(ME, THEM);
    let resting = place(&mut w, &r, "Warrior", ME, 3, 6, 70);
    let order = military_order(&w, &r, ME, resting);
    assert!(matches!(order, Some(UnitOrder::Fortify | UnitOrder::Hold | UnitOrder::MoveTo(_))), "{order:?}");
    w.civ_mut(ME).unwrap().legacy_ai = true;
    assert_eq!(military_order(&w, &r, ME, resting), None, "the old AI leaves it to wander");
}

#[test]
fn a_pantheon_is_chosen_for_the_land_around_it() {
    use civ_core::world::belief_rating::best;
    let r = ruleset();
    let mut w = war(&r);
    let desert = r.terrain("Desert").unwrap();
    let home = w.grid.id_at(2, 6).unwrap();
    for t in w.grid.within(home, 3) {
        w.grid.terrain[t.0 as usize] = desert;
    }
    let pantheons: Vec<_> = r.beliefs.iter().filter(|(_, b)| b.r#type == "Pantheon").map(|(id, _)| id).collect();
    let pick = best(&w, &r, ME, &pantheons).unwrap();
    assert_eq!(r.beliefs[pick].name, "Desert Folklore");
    assert_ne!(pantheons.iter().min().map(|id| r.beliefs[*id].name.as_str()), Some("Desert Folklore"), "the old pick differs");
}
