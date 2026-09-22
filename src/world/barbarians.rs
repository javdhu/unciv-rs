//! Barbarians.
//!
//! A civilization in the data model and not one in the game: permanently
//! hostile, never at peace, no diplomacy, no cities of its own. Encampments on
//! the map produce hostile units on a timer, which is the early game's only
//! source of pressure on a civ that would otherwise expand unopposed.
//!
//! The camp is identified by its `Marks a barbarian camp` unique rather than
//! by the name "Barbarian encampment" — the same reasoning as the palace,
//! settlers and workers.

use crate::arena::Id;
use crate::rng::Rng;
use crate::ruleset::{Ruleset, TileImprovement};
use crate::world::entities::Unit;
use crate::world::grid::{CivId, TileId};
use crate::world::world::World;

/// Turns between spawns from one camp, before difficulty scales it.
pub const DEFAULT_SPAWN_DELAY: u32 = 8;

/// The improvement that marks a camp.
pub fn camp_improvement(r: &Ruleset) -> Option<Id<TileImprovement>> {
    r.improvements
        .iter()
        .find(|(_, i)| i.uniques.iter().any(|u| u.text.contains("Marks a barbarian camp")))
        .map(|(id, _)| id)
}

/// Every camp on the map, in tile order.
pub fn camps(world: &World, r: &Ruleset) -> Vec<TileId> {
    let Some(camp) = camp_improvement(r) else { return Vec::new() };
    world.grid.ids().filter(|t| world.grid.improvement[t.0 as usize] == Some(camp)).collect()
}

/// Place a camp. Used by map setup and by tests.
pub fn place_camp(world: &mut World, r: &Ruleset, tile: TileId) -> bool {
    match camp_improvement(r) {
        Some(camp) => {
            world.grid.improvement[tile.0 as usize] = Some(camp);
            true
        }
        None => false,
    }
}

/// What a camp can field.
///
/// The ruleset supplies purpose-built barbarian units — Brute and Galley, both
/// `uniqueTo: "Barbarians"` and neither requiring a technology. Preferring
/// those is why barbarians stay a nuisance rather than fielding whatever the
/// era allows. Anything tech-free falls back behind them.
pub fn spawnable_unit(r: &Ruleset) -> Option<Id<crate::ruleset::BaseUnit>> {
    let land_military = |u: &crate::ruleset::BaseUnit| {
        u.strength.unwrap_or(0) > 0
            && u.cost.unwrap_or(0) > 0
            && u.required_tech.is_none()
            && r.unit_types.by_name(&u.unit_type).is_some_and(|t| t.movement_type == "Land")
    };

    r.units
        .iter()
        .filter(|(_, u)| land_military(u) && u.unique_to.as_deref() == Some("Barbarians"))
        .min_by_key(|(id, u)| (u.cost.unwrap_or(i32::MAX), id.index()))
        .or_else(|| {
            r.units
                .iter()
                .filter(|(_, u)| land_military(u) && u.unique_to.is_none())
                .min_by_key(|(id, u)| (u.cost.unwrap_or(i32::MAX), id.index()))
        })
        .map(|(id, _)| id)
}

/// Spawn from every camp whose turn has come.
///
/// The timer is per camp and derived from the turn number, so it needs no
/// stored state and replays identically. Randomness is seeded from the camp
/// and the turn, never from an ambient generator.
pub fn spawn(world: &mut World, r: &Ruleset, barbarians: CivId, delay: u32) -> Vec<TileId> {
    let delay = delay.max(1);
    let Some(base) = spawnable_unit(r) else { return Vec::new() };
    let turn = world.turn;
    let movement = r.units.get(base).map_or(2, |u| u.movement);

    let mut spawned = Vec::new();
    for camp in camps(world, r) {
        // Stagger camps against each other so they do not all fire together.
        let phase = (camp.0 as u32) % delay;
        if turn % delay != phase {
            continue;
        }
        // Not on top of an existing unit.
        if !world.units_at(camp).is_empty() {
            continue;
        }
        let mut rng = Rng::from_parts(&[turn as u64, camp.0 as u64]);
        // Two in three, so camps are irregular rather than metronomic.
        if rng.range(0, 2) == 0 {
            continue;
        }
        world.place_unit(Unit::new(base, barbarians, camp, movement));
        spawned.push(camp);
    }
    spawned
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::entities::Civilization;
    use crate::world::grid::TileGrid;

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings").expect("readable").expect("loads")
    }

    fn world(r: &Ruleset) -> World {
        let g = TileGrid::new(12, 12, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        let mut civs = vec![Civilization::new(CivId(0), nation, true), Civilization::new(CivId(1), nation, true)];
        civs[1].is_barbarian = true;
        let mut w = World::new(g, civs);
        w.diplomacy.mark_hostile(CivId(1));
        w
    }

    #[test]
    fn the_camp_is_found_by_its_unique() {
        let r = ruleset();
        let camp = camp_improvement(&r).expect("the ruleset marks camps");
        assert_eq!(r.improvements[camp].name, "Barbarian encampment");
    }

    #[test]
    fn barbarians_are_at_war_with_everyone_and_cannot_be_reasoned_with() {
        let w = world(&ruleset());
        assert!(w.diplomacy.at_war(CivId(0), CivId(1)));
        assert!(w.diplomacy.at_war(CivId(1), CivId(0)), "both ways, as ever");

        let mut w = w;
        assert_eq!(w.diplomacy.make_peace(CivId(0), CivId(1)), None, "no treaty");
        assert!(w.diplomacy.at_war(CivId(0), CivId(1)), "still at war");
    }

    #[test]
    fn the_ruleset_supplies_a_barbarian_unit() {
        let r = ruleset();
        let unit = spawnable_unit(&r).expect("something can be spawned");
        println!("barbarians field: {}", r.units[unit].name);
        assert_eq!(r.units[unit].unique_to.as_deref(), Some("Barbarians"));
        assert!(r.units[unit].required_tech.is_none(), "and needs no research");
    }

    #[test]
    fn camps_produce_units_over_time() {
        let r = ruleset();
        let mut w = world(&r);
        place_camp(&mut w, &r, TileId(20));
        place_camp(&mut w, &r, TileId(50));
        assert_eq!(camps(&w, &r).len(), 2);

        let mut total = 0;
        for turn in 0..40 {
            w.turn = turn;
            // Clear the field so the camps are never blocked.
            for id in w.units.ids() {
                w.remove_unit(id);
            }
            total += spawn(&mut w, &r, CivId(1), DEFAULT_SPAWN_DELAY).len();
        }
        println!("{total} barbarians over 40 turns from 2 camps");
        assert!(total > 0, "camps should produce something");
        assert!(total < 40, "but not every turn");
    }

    #[test]
    fn a_camp_will_not_stack_units_on_itself() {
        let r = ruleset();
        let mut w = world(&r);
        place_camp(&mut w, &r, TileId(20));
        // Force a spawn turn, then try again with the tile occupied.
        let mut spawned_once = false;
        for turn in 0..40 {
            w.turn = turn;
            if !spawn(&mut w, &r, CivId(1), DEFAULT_SPAWN_DELAY).is_empty() {
                spawned_once = true;
                break;
            }
        }
        assert!(spawned_once, "it should have spawned at least once");
        let before = w.units.len();
        for turn in 0..40 {
            w.turn = turn;
            spawn(&mut w, &r, CivId(1), DEFAULT_SPAWN_DELAY);
        }
        assert_eq!(w.units.len(), before, "the camp tile is occupied");
    }

    #[test]
    fn spawning_replays_identically() {
        let r = ruleset();
        let run = || {
            let mut w = world(&r);
            place_camp(&mut w, &r, TileId(20));
            let mut log = Vec::new();
            for turn in 0..30 {
                w.turn = turn;
                for id in w.units.ids() {
                    w.remove_unit(id);
                }
                log.push(spawn(&mut w, &r, CivId(1), DEFAULT_SPAWN_DELAY));
            }
            log
        };
        assert_eq!(run(), run());
    }
}
