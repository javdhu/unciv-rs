//! Standing orders for a human's units: work tiles on their own, or explore
//! (Unciv's `MapUnit.automated` and the Explore action, MapUnit.kt:890-928).
//!
//! The routines are the AI's ([`ai::worker_order`], [`ai::explore_order`]),
//! but they read only what the owner knows. A worker considers jobs on tiles
//! its civ has explored and half-dug tiles it can currently see, and paths
//! over [`fog::movement_knowledge`], the civ's own picture of the map. An
//! explorer chooses among [`fog::reachable_preview`] tiles, scored by its own
//! civ's explored set. Neither can use the map to learn what lies in the fog.
//!
//! Orders run when given and again as the round closes
//! ([`crate::world::turn::finish_round`]), for units with movement left.
//! Any manual order to the unit cancels automation, and so does a visible
//! enemy military unit next to it.

use crate::arena::Id;
use crate::ruleset::Ruleset;
use crate::world::ai::{self, UnitOrder, WorkerJob, WorkerSight};
use crate::world::commands::Command;
use crate::world::entities::Unit;
use crate::world::grid::{CivId, TileId};
use crate::world::turn::{TurnEvent, TurnModifiers};
use crate::world::{fog, World};

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Automation {
    /// Improve tiles and build roads, as the AI's workers do.
    Work,
    /// Walk toward whatever reveals the most unexplored land.
    Explore,
}

/// Orders one unit acts on in one call, so a worker can arrive and start
/// digging in the same turn, but a routine that keeps finding moves stops.
const MAX_ORDERS: usize = 3;

/// Whether this unit can take the standing order at all.
pub fn can_automate(world: &World, r: &Ruleset, unit: Id<Unit>, mode: Automation) -> bool {
    let Some(u) = world.units.get(unit) else { return false };
    let Some(base) = r.units.get(u.base) else { return false };
    let water_worker =
        base.uniques.iter().any(|unique| unique.unique_type() == Some(crate::UniqueType::CreateWaterImprovements));
    let worker = ai::is_worker(r, u.base) || water_worker;
    match mode {
        Automation::Work => worker,
        // explore_order's own filter: settlers and workers have jobs, and
        // aircraft do not walk. Great people and missionaries are not scouts.
        Automation::Explore => {
            !worker && !crate::world::settling::is_settler(r, u.base) && !u.is_air(r) && !u.is_civilian(r)
        }
    }
}

/// An enemy military unit its owner can see on an adjacent tile.
pub fn threatened(world: &World, r: &Ruleset, unit: Id<Unit>) -> bool {
    let Some(u) = world.units.get(unit) else { return false };
    let civ = u.owner;
    let near: Vec<TileId> = world.grid.neighbours(u.position).collect();
    world.units.iter().any(|(id, other)| {
        other.owner != civ
            && near.contains(&other.position)
            && other.is_military(r)
            && world.diplomacy.at_war(civ, other.owner)
            && fog::can_see_unit(world, r, civ, id)
    })
}

/// The order a human's automated unit takes now, from its civ's knowledge.
pub fn order(
    world: &World,
    r: &Ruleset,
    unit: Id<Unit>,
    jobs: &std::cell::OnceCell<Vec<WorkerJob>>,
) -> Option<UnitOrder> {
    let u = world.units.get(unit)?;
    let civ = u.owner;
    match u.automation? {
        Automation::Work => {
            let step = |target: TileId| {
                let (known, moving) = fog::movement_knowledge(world, r, civ, unit)?;
                crate::world::movement::step_toward(&known, r, moving, target)
            };
            let sight = WorkerSight {
                job: &|t| world.visibility.has_explored(civ, t),
                dig: &|t| world.visibility.can_see(civ, t),
                step: &step,
            };
            ai::worker_order_with(world, r, civ, unit, jobs, sight)
        }
        Automation::Explore => explore(world, r, civ, unit),
    }
}

/// [`ai::explore_order`] over the civ's own movement preview.
fn explore(world: &World, r: &Ruleset, civ: CivId, unit: Id<Unit>) -> Option<UnitOrder> {
    let u = world.units.get(unit)?;
    if !u.can_move() {
        return None;
    }
    let reachable: Vec<TileId> =
        fog::reachable_preview(world, r, civ, unit).into_iter().map(|p| p.tile).filter(|t| *t != u.position).collect();
    // Ruins it can see come first, lowest tile id (UnitAutomation.tryGoToRuinAndEncampment).
    let ruin = reachable.iter().copied().find(|t| {
        world.visibility.can_see(civ, *t)
            && world.grid.improvement[t.0 as usize].is_some_and(|i| crate::world::ruins::is_ruins(r, i))
    });
    if let Some(t) = ruin {
        return Some(UnitOrder::MoveTo(t));
    }
    let (best, gain) = reachable
        .into_iter()
        .map(|t| {
            let unseen =
                world.grid.within(t, 2).into_iter().filter(|n| !world.visibility.has_explored(civ, *n)).count();
            (t, unseen)
        })
        .max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0)))?;
    (gain > 0).then_some(UnitOrder::MoveTo(best))
}

fn command(unit: Id<Unit>, order: UnitOrder) -> Option<Command> {
    match order {
        UnitOrder::MoveTo(to) => Some(Command::MoveUnit { unit, to }),
        UnitOrder::Improve(improvement) => Some(Command::Improve { unit, improvement }),
        _ => None,
    }
}

/// Carry out one unit's standing order. Cancels it instead when an enemy is
/// next to the unit.
pub fn act(world: &mut World, r: &Ruleset, unit: Id<Unit>, mods: TurnModifiers) -> Vec<TurnEvent> {
    let mut events = Vec::new();
    let Some(civ) = world.units.get(unit).filter(|u| u.automation.is_some()).map(|u| u.owner) else {
        return events;
    };
    for _ in 0..MAX_ORDERS {
        if threatened(world, r, unit) {
            if let Some(u) = world.units.get_mut(unit) {
                u.automation = None;
            }
            break;
        }
        if !world.units.get(unit).is_some_and(|u| u.can_move()) {
            break;
        }
        let jobs = std::cell::OnceCell::new();
        let Some(next) = order(world, r, unit, &jobs).and_then(|o| command(unit, o)) else { break };
        let before = world.units.get(unit).map(|u| (u.position, u.movement_left));
        match crate::world::commands::execute_automated(world, r, civ, next, mods) {
            Ok(outcome) => events.extend(outcome.events),
            Err(_) => break,
        }
        if world.units.get(unit).map(|u| (u.position, u.movement_left)) == before {
            break;
        }
    }
    events
}

/// Every automated unit of this civ with movement left, in id order.
pub fn run(world: &mut World, r: &Ruleset, civ: CivId, mods: TurnModifiers) -> Vec<TurnEvent> {
    let units: Vec<Id<Unit>> =
        world.units.iter().filter(|(_, u)| u.owner == civ && u.automation.is_some()).map(|(id, _)| id).collect();
    let mut events = Vec::new();
    for unit in units {
        events.extend(act(world, r, unit, mods));
    }
    events
}
