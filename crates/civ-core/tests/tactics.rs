//! The live army tactics (docs/AI.md, level 1): each behaviour on a small map.

use civ_core::arena::Id;
use civ_core::ruleset::Ruleset;
use civ_core::world::commands::{execute_with, AttackTarget, Command};
use civ_core::world::tactics::Tactics;
use civ_core::world::turn::TurnModifiers;
use civ_core::world::{City, CivId, Civilization, TileGrid, TileId, Unit, World};

fn fixture() -> (Ruleset, World) {
    let mut r = Ruleset::load_dir("rules/gods-and-kings").unwrap().unwrap();
    for id in r.units.ids().collect::<Vec<_>>() {
        r.units.get_mut(id).unwrap().required_resource = None;
    }
    let nation = r.nations.id("Rome").unwrap();
    let mut w = World::new(
        TileGrid::new(18, 12, r.terrain("Grassland").unwrap()),
        vec![Civilization::new(CivId(0), nation, true), Civilization::new(CivId(1), nation, true)],
    );
    w.diplomacy.declare_war(CivId(0), CivId(1));
    (r, w)
}

fn tile(w: &World, x: u16, y: u16) -> TileId {
    w.grid.id_at(x, y).unwrap()
}

fn unit(w: &mut World, r: &Ruleset, owner: u8, name: &str, x: u16, y: u16) -> Id<Unit> {
    let base = r.unit(name).unwrap();
    let at = tile(w, x, y);
    w.visibility.observe(CivId(1 - owner), at);
    w.place_unit(Unit::new(base, CivId(owner), at, r.units[base].movement))
}

/// Run civ 0's army for one turn and return the commands it issued.
fn run(w: &mut World, r: &Ruleset) -> Vec<Command> {
    let mut tactics = Tactics::new(w, r, CivId(0));
    let mut issued = Vec::new();
    while let Some(command) = tactics.next(w, r) {
        issued.push(command.clone());
        let _ = execute_with(w, r, CivId(0), command, TurnModifiers::default());
        assert!(issued.len() < 50, "the army stops");
    }
    issued
}

fn attacks(issued: &[Command]) -> Vec<AttackTarget> {
    issued
        .iter()
        .filter_map(|c| match c {
            Command::Attack { target, .. } => Some(*target),
            _ => None,
        })
        .collect()
}

#[test]
fn a_melee_unit_steps_up_and_strikes_in_the_same_turn() {
    let (r, mut w) = fixture();
    let ours = unit(&mut w, &r, 0, "Swordsman", 4, 4);
    let theirs = unit(&mut w, &r, 1, "Warrior", 6, 4);
    w.units.get_mut(theirs).unwrap().health = 30;
    assert_eq!(w.grid.distance(tile(&w, 4, 4), tile(&w, 6, 4)), 2);
    let issued = run(&mut w, &r);
    assert!(matches!(issued.first(), Some(Command::MoveUnit { unit, .. }) if *unit == ours));
    assert_eq!(attacks(&issued), vec![AttackTarget::Unit(theirs)]);
    assert!(w.units.get(theirs).is_none(), "a 30-health warrior does not survive a swordsman");
}

#[test]
fn a_wounded_unit_falls_back_instead_of_fighting_on() {
    let (r, mut w) = fixture();
    w.found_city_in(&r, City::found("Home", CivId(0), tile(&w, 2, 4)));
    let ours = unit(&mut w, &r, 0, "Warrior", 5, 4);
    w.units.get_mut(ours).unwrap().health = 30;
    let theirs = unit(&mut w, &r, 1, "Warrior", 6, 4);
    let before = w.grid.distance(tile(&w, 5, 4), tile(&w, 6, 4));
    let issued = run(&mut w, &r);
    assert!(attacks(&issued).is_empty(), "no attack at 30 health on an even fight: {issued:?}");
    let now = w.units.get(ours).unwrap().position;
    let enemy = w.units.get(theirs).unwrap().position;
    assert!(w.grid.distance(now, enemy) > before, "it backs away from the enemy");
}

#[test]
fn a_hopeless_attack_is_not_made() {
    let (r, mut w) = fixture();
    unit(&mut w, &r, 0, "Warrior", 4, 4);
    unit(&mut w, &r, 1, "Rifleman", 5, 4);
    let issued = run(&mut w, &r);
    assert!(attacks(&issued).is_empty(), "a warrior does not charge a rifleman: {issued:?}");
}

#[test]
fn a_city_bombards_an_enemy_in_range() {
    let (r, mut w) = fixture();
    let city = w.found_city_in(&r, City::found("Home", CivId(0), tile(&w, 4, 4)));
    let theirs = unit(&mut w, &r, 1, "Warrior", 6, 4);
    w.visibility.observe(CivId(0), tile(&w, 6, 4));
    let issued = run(&mut w, &r);
    assert!(issued.contains(&Command::Bombard { city, target: theirs }), "{issued:?}");
}

#[test]
fn ranged_units_shoot_before_melee_units_close_in() {
    let (r, mut w) = fixture();
    let melee = unit(&mut w, &r, 0, "Swordsman", 4, 5);
    let archer = unit(&mut w, &r, 0, "Archer", 4, 3);
    unit(&mut w, &r, 1, "Spearman", 5, 4);
    let issued = run(&mut w, &r);
    let shooters: Vec<_> = issued
        .iter()
        .filter_map(|c| match c {
            Command::Attack { unit, .. } => Some(*unit),
            _ => None,
        })
        .collect();
    assert_eq!(shooters.first(), Some(&archer), "{issued:?}");
    assert!(melee != archer);
}

#[test]
fn a_lone_unit_waits_outside_a_city_it_cannot_take() {
    let (r, mut w) = fixture();
    w.found_city_in(&r, City::found("Home", CivId(0), tile(&w, 2, 4)));
    let target = tile(&w, 12, 4);
    w.found_city_in(&r, City::found("Walls", CivId(1), target));
    w.visibility.observe(CivId(0), target);
    let ours = unit(&mut w, &r, 0, "Warrior", 8, 4);
    assert_eq!(w.grid.distance(tile(&w, 8, 4), target), 4);
    run(&mut w, &r);
    let now = w.units.get(ours).unwrap().position;
    assert!(w.grid.distance(now, target) >= 3, "it holds outside the city's guns");
}

#[test]
fn a_threatened_city_calls_a_defender_home() {
    let (r, mut w) = fixture();
    let centre = tile(&w, 4, 4);
    w.found_city_in(&r, City::found("Home", CivId(0), centre));
    let ours = unit(&mut w, &r, 0, "Warrior", 4, 8);
    let enemy_at = tile(&w, 1, 4);
    unit(&mut w, &r, 1, "Warrior", 1, 4);
    let before = w.grid.distance(tile(&w, 4, 8), centre);
    assert!(w.grid.distance(tile(&w, 4, 8), enemy_at) > 3, "the enemy is out of our warrior's reach");
    run(&mut w, &r);
    let now = w.units.get(ours).unwrap().position;
    assert!(w.grid.distance(now, centre) < before, "it heads home");
}

#[test]
fn the_same_map_gives_the_same_orders() {
    let (r, mut w) = fixture();
    unit(&mut w, &r, 0, "Swordsman", 4, 4);
    unit(&mut w, &r, 0, "Archer", 3, 4);
    unit(&mut w, &r, 1, "Warrior", 6, 4);
    unit(&mut w, &r, 1, "Spearman", 6, 5);
    let mut again = w.clone();
    assert_eq!(run(&mut w, &r), run(&mut again, &r));
}

