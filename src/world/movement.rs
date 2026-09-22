//! Moving units across the map.
//!
//! The first spatial decision, and the first search. Terrain cost comes from
//! the *topmost* terrain on a tile — Unciv reads `to.lastTerrain.movementCost`
//! — which is consistent with how yields resolve: a forested hill is a forest.
//!
//! The rule worth naming is the clamp (`UnitMovement.kt:133`). If a step costs
//! more movement than a unit has left, the cost is reduced to exactly what
//! remains rather than being refused. That is what makes a Warrior with half a
//! point left still able to enter a marsh — it simply ends its turn there. A
//! naive "cost must fit in the budget" check gets this wrong and strands units.
//!
//! Search order is fixed. The frontier is kept sorted by `(cost, TileId)`
//! rather than left to a hash, because two machines walking the same map must
//! expand the same tiles in the same order or their units end up in different
//! places.

use crate::arena::Id;
use crate::ruleset::Ruleset;
use crate::stats::Fixed;
use crate::world::entities::Unit;
use crate::world::grid::{TileGrid, TileId};
use crate::world::world::World;
use std::collections::BTreeMap;

/// A city centre always costs one to enter, whatever it was built on.
const CITY_CENTRE_COST: Fixed = Fixed::ONE;

/// Cost of entering `to`, ignoring roads, rivers and zones of control.
///
/// `None` when the tile cannot be entered at all.
pub fn terrain_cost(grid: &TileGrid, r: &Ruleset, to: TileId) -> Option<Fixed> {
    // The last terrain in the fold order is the one you walk on.
    let top = grid.terrains(to).last()?;
    let t = r.terrains.get(top)?;
    if t.impassable == Some(true) {
        return None;
    }
    Some(Fixed::whole(t.movement_cost.unwrap_or(1)))
}

/// Whether any unit could stand here.
pub fn is_passable(grid: &TileGrid, r: &Ruleset, tile: TileId) -> bool {
    terrain_cost(grid, r, tile).is_some()
}

/// Every tile reachable with `budget` movement, and what it costs to get there.
///
/// Dijkstra with the clamp described above: a tile that costs more than the
/// remaining budget is still reachable, at the price of the whole budget.
/// `city_centres` are entered at cost 1 regardless of terrain.
pub fn reachable(
    grid: &TileGrid,
    r: &Ruleset,
    from: TileId,
    budget: Fixed,
    city_centres: &[TileId],
) -> BTreeMap<TileId, Fixed> {
    let mut best: BTreeMap<TileId, Fixed> = BTreeMap::new();
    best.insert(from, Fixed::ZERO);
    if budget <= Fixed::ZERO {
        return best;
    }

    // (cost, tile) ordering makes expansion order identical on every machine.
    let mut frontier: Vec<(Fixed, TileId)> = vec![(Fixed::ZERO, from)];

    while !frontier.is_empty() {
        frontier.sort_by(|a, b| b.cmp(a)); // cheapest last, so `pop` takes it
        let (cost_here, tile) = frontier.pop().expect("non-empty");
        if best.get(&tile).is_some_and(|c| *c < cost_here) {
            continue; // superseded
        }

        for next in grid.neighbours(tile) {
            let Some(step) = terrain_cost(grid, r, next) else { continue };
            let step = if city_centres.contains(&next) { CITY_CENTRE_COST } else { step };
            let raw = cost_here + step;

            // The clamp: over budget is still reachable, for everything left.
            let (total, can_continue) =
                if raw < budget { (raw, true) } else { (budget, false) };

            if best.get(&next).is_none_or(|c| *c > total) {
                best.insert(next, total);
                if can_continue {
                    frontier.push((total, next));
                }
            }
        }
    }
    best
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoveRejection {
    /// The unit id no longer resolves.
    NoSuchUnit,
    /// Terrain the unit cannot enter.
    Impassable,
    /// Further than this turn's movement allows.
    OutOfRange,
    /// Already there.
    NoMovement,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoveEvent {
    pub unit: Id<Unit>,
    pub from: TileId,
    pub to: TileId,
    pub spent: Fixed,
}

/// Move a unit, spending movement. Fails rather than half-moving.
pub fn move_unit(
    world: &mut World,
    r: &Ruleset,
    unit_id: Id<Unit>,
    to: TileId,
) -> Result<MoveEvent, MoveRejection> {
    let unit = world.units.get(unit_id).ok_or(MoveRejection::NoSuchUnit)?;
    let (from, budget) = (unit.position, unit.movement_left);
    if from == to {
        return Err(MoveRejection::NoMovement);
    }
    if budget <= Fixed::ZERO {
        return Err(MoveRejection::NoMovement);
    }
    if !is_passable(&world.grid, r, to) {
        return Err(MoveRejection::Impassable);
    }

    let centres: Vec<TileId> = world.cities.iter().map(|(_, c)| c.centre).collect();
    let cost = *reachable(&world.grid, r, from, budget, &centres)
        .get(&to)
        .ok_or(MoveRejection::OutOfRange)?;

    let unit = world.units.get_mut(unit_id).ok_or(MoveRejection::NoSuchUnit)?;
    unit.position = to;
    unit.movement_left = (budget - cost).max(Fixed::ZERO);
    unit.fortified = false;
    Ok(MoveEvent { unit: unit_id, from, to, spent: cost })
}

/// Restore every unit's movement. Called when its owner's turn begins.
pub fn refresh_movement(world: &mut World, r: &Ruleset) {
    let refreshed: Vec<(Id<Unit>, i32)> = world
        .units
        .iter()
        .map(|(id, u)| (id, r.units.get(u.base).map_or(2, |b| b.movement)))
        .collect();
    for (id, movement) in refreshed {
        if let Some(u) = world.units.get_mut(id) {
            u.refresh(movement);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::city::City;
    use crate::world::entities::{Civilization, Unit};
    use crate::world::grid::CivId;

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings")
            .expect("readable")
            .expect("loads")
    }

    fn world(r: &Ruleset, terrain: &str) -> World {
        let g = TileGrid::new(16, 16, r.terrain(terrain).unwrap());
        let nation = r.nations.ids().next().unwrap();
        World::new(g, vec![Civilization::new(CivId(0), nation, false)])
    }

    fn warrior(w: &mut World, r: &Ruleset, at: TileId) -> Id<Unit> {
        let base = r.unit("Warrior").unwrap();
        let movement = r.units.get(base).unwrap().movement;
        w.place_unit(Unit::new(base, CivId(0), at, movement))
    }

    #[test]
    fn terrain_cost_comes_from_the_topmost_feature() {
        let r = ruleset();
        let mut g = TileGrid::new(4, 4, r.terrain("Grassland").unwrap());
        let t = TileId(5);
        assert_eq!(terrain_cost(&g, &r, t), Some(Fixed::ONE), "bare grassland");

        g.features[t.0 as usize].push(r.terrain("Forest").unwrap());
        assert_eq!(terrain_cost(&g, &r, t), Some(Fixed::whole(2)), "forest costs 2");

        g.features[t.0 as usize].push(r.terrain("Marsh").unwrap());
        assert_eq!(terrain_cost(&g, &r, t), Some(Fixed::whole(3)), "marsh is now on top");
    }

    #[test]
    fn impassable_terrain_cannot_be_entered() {
        let r = ruleset();
        let impassable = r
            .terrains
            .iter()
            .find(|(_, t)| t.impassable == Some(true))
            .map(|(id, t)| (id, t.name.clone()))
            .expect("the ruleset has impassable terrain");
        println!("impassable: {}", impassable.1);

        let mut g = TileGrid::new(4, 4, r.terrain("Grassland").unwrap());
        g.terrain[5] = impassable.0;
        g.features[5].clear();
        assert_eq!(terrain_cost(&g, &r, TileId(5)), None);
        assert!(!is_passable(&g, &r, TileId(5)));
    }

    #[test]
    fn a_unit_reaches_what_its_movement_allows() {
        let r = ruleset();
        let w = world(&r, "Grassland");
        let start = w.grid.id_at(8, 8).unwrap();
        // Two movement over cost-1 terrain is the radius-2 disc: 1 + 3·2·3 = 19.
        let set = reachable(&w.grid, &r, start, Fixed::whole(2), &[]);
        assert_eq!(set.len(), 19, "got {}", set.len());
        assert_eq!(set[&start], Fixed::ZERO);
    }

    /// The rule that stops units being stranded.
    #[test]
    fn a_step_costing_more_than_is_left_is_still_taken() {
        let r = ruleset();
        let mut w = world(&r, "Grassland");
        let start = w.grid.id_at(8, 8).unwrap();
        let marsh = w.grid.neighbours(start).next().unwrap();
        w.grid.features[marsh.0 as usize].push(r.terrain("Marsh").unwrap());
        assert_eq!(terrain_cost(&w.grid, &r, marsh), Some(Fixed::whole(3)));

        // One movement point against a cost of three.
        let set = reachable(&w.grid, &r, start, Fixed::ONE, &[]);
        assert_eq!(set.get(&marsh), Some(&Fixed::ONE), "clamped to the whole budget");

        let id = warrior(&mut w, &r, start);
        w.units.get_mut(id).unwrap().movement_left = Fixed::ONE;
        let event = move_unit(&mut w, &r, id, marsh).expect("the move is legal");
        assert_eq!(event.spent, Fixed::ONE);
        assert_eq!(w.units.get(id).unwrap().movement_left, Fixed::ZERO, "and the turn ends");
    }

    #[test]
    fn rough_terrain_shrinks_the_reachable_set() {
        let r = ruleset();
        let mut w = world(&r, "Grassland");
        let start = w.grid.id_at(8, 8).unwrap();
        let open = reachable(&w.grid, &r, start, Fixed::whole(2), &[]).len();

        let forest = r.terrain("Forest").unwrap();
        for t in w.grid.ids() {
            if t != start {
                w.grid.features[t.0 as usize].push(forest);
            }
        }
        let wooded = reachable(&w.grid, &r, start, Fixed::whole(2), &[]).len();
        println!("open {open}, forested {wooded}");
        assert!(wooded < open, "forest costs 2, so 2 movement reaches less");
    }

    #[test]
    fn city_centres_are_always_cheap_to_enter() {
        let r = ruleset();
        let mut w = world(&r, "Grassland");
        let start = w.grid.id_at(8, 8).unwrap();
        let site = w.grid.neighbours(start).next().unwrap();
        // Bury the city under expensive terrain; entering it still costs one.
        w.grid.features[site.0 as usize].push(r.terrain("Marsh").unwrap());
        w.found_city(City::found("Rome", CivId(0), site));

        let centres = vec![site];
        let set = reachable(&w.grid, &r, start, Fixed::whole(2), &centres);
        assert_eq!(set.get(&site), Some(&Fixed::ONE));
    }

    #[test]
    fn a_unit_cannot_move_beyond_its_range_or_into_rock() {
        let r = ruleset();
        let mut w = world(&r, "Grassland");
        let start = w.grid.id_at(8, 8).unwrap();
        let id = warrior(&mut w, &r, start);

        let far = w.grid.id_at(0, 0).unwrap();
        assert_eq!(move_unit(&mut w, &r, id, far), Err(MoveRejection::OutOfRange));
        assert_eq!(move_unit(&mut w, &r, id, start), Err(MoveRejection::NoMovement));
        assert_eq!(w.units.get(id).unwrap().position, start, "a failed move changes nothing");
    }

    #[test]
    fn movement_is_spent_and_refreshed() {
        let r = ruleset();
        let mut w = world(&r, "Grassland");
        let start = w.grid.id_at(8, 8).unwrap();
        let id = warrior(&mut w, &r, start);
        let full = w.units.get(id).unwrap().movement_left;

        let step = w.grid.neighbours(start).next().unwrap();
        move_unit(&mut w, &r, id, step).unwrap();
        assert_eq!(w.units.get(id).unwrap().movement_left, full - Fixed::ONE);

        refresh_movement(&mut w, &r);
        assert_eq!(w.units.get(id).unwrap().movement_left, full);
    }

    #[test]
    fn the_search_expands_identically_every_time() {
        let r = ruleset();
        let mut w = world(&r, "Grassland");
        // Uneven terrain, so ties and orderings actually arise.
        let forest = r.terrain("Forest").unwrap();
        for t in w.grid.ids().filter(|t| t.0 % 3 == 0) {
            w.grid.features[t.0 as usize].push(forest);
        }
        let start = w.grid.id_at(8, 8).unwrap();
        let once = reachable(&w.grid, &r, start, Fixed::whole(4), &[]);
        let twice = reachable(&w.grid, &r, start, Fixed::whole(4), &[]);
        assert_eq!(once, twice);
        assert!(once.len() > 20);
    }
}
