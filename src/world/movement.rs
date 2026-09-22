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
use crate::unique_type::UniqueType;
use crate::world::entities::Unit;
use crate::world::grid::{CivId, RoadLevel, TileGrid, TileId};
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
            let (total, can_continue) = if raw < budget { (raw, true) } else { (budget, false) };

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

/// Returns whether movement between adjacent tiles `from` and `to` is affected by Zone of Control.
pub fn is_movement_affected_by_zoc(world: &World, r: &Ruleset, unit: &Unit, from: TileId, to: TileId) -> bool {
    if unit.has_unique(r, UniqueType::IgnoresZOC) {
        return false;
    }
    for n in world.grid.neighbours(from) {
        if world.grid.distance(n, to) != 1 {
            continue;
        }
        if let Some(city) = world.cities.iter().find(|(_, c)| c.centre == n).map(|(_, c)| c) {
            if world.diplomacy.at_war(unit.owner, city.owner) {
                return true;
            }
        }
        for (_, other) in world.units.iter() {
            if other.position == n
                && other.owner != unit.owner
                && other.is_military(r)
                && world.diplomacy.at_war(unit.owner, other.owner)
                && (other.is_water_unit(r) || (unit.is_land_unit(r) && !other.embarked))
            {
                return true;
            }
        }
    }
    false
}

/// Whether `tile` is occupied such that `unit` cannot land (end turn) on it under 1UPT rules.
pub fn is_tile_occupied_for(world: &World, r: &Ruleset, tile: TileId, unit: &Unit) -> bool {
    let is_civ = unit.is_civilian(r);
    for (_, other) in world.units.iter() {
        if other.position != tile {
            continue;
        }
        if other.owner != unit.owner {
            return true;
        }
        if is_civ == other.is_civilian(r) {
            return true;
        }
    }
    for (_, city) in world.cities.iter() {
        if city.centre == tile && city.owner != unit.owner {
            return true;
        }
    }
    false
}

/// May a unit of `visitor` enter land `owner` holds (`canPassThroughTiles`)?
///
/// Its own, a barbarian's, an enemy's, or one whose borders are open to it.
/// A human player may also walk through a city-state's land; the AI may not.
/// Anyone else's is closed — the reason open borders are worth trading for.
pub fn may_enter_territory(world: &World, visitor: CivId, owner: CivId) -> bool {
    if visitor == owner {
        return true;
    }
    let barbarian = |c: CivId| world.civ(c).is_some_and(|x| x.is_barbarian);
    if barbarian(owner) || barbarian(visitor) {
        return true;
    }
    if world.diplomacy.at_war(visitor, owner) || world.diplomacy.may_enter_borders_of(visitor, owner) {
        return true;
    }
    world.civ(visitor).is_some_and(|c| !c.is_ai) && world.civ(owner).is_some_and(|c| c.is_city_state())
}

/// Whether `unit` can path through `tile`.
pub fn can_pass_through(world: &World, _r: &Ruleset, tile: TileId, unit: &Unit) -> bool {
    if let Some(owner) = world.grid.owner[tile.0 as usize] {
        if !may_enter_territory(world, unit.owner, owner) {
            return false;
        }
    }
    for (_, other) in world.units.iter() {
        if other.position == tile && other.owner != unit.owner && world.diplomacy.at_war(unit.owner, other.owner) {
            return false;
        }
    }
    for (_, city) in world.cities.iter() {
        if city.centre == tile && city.owner != unit.owner && world.diplomacy.at_war(unit.owner, city.owner) {
            return false;
        }
    }
    true
}

/// Cost for `unit` to step between adjacent tiles `from` and `to`.
pub fn step_cost_for_unit(
    world: &World,
    r: &Ruleset,
    unit: &Unit,
    from: TileId,
    to: TileId,
    budget: Fixed,
) -> Option<Fixed> {
    let _ = budget;
    let terrain = terrain_cost(&world.grid, r, to)?;

    let from_water = world.grid.is_water(r, from);
    let to_water = world.grid.is_water(r, to);

    if unit.is_water_unit(r) {
        if !to_water && !world.cities.iter().any(|(_, c)| c.centre == to) {
            return None;
        }
    } else if unit.is_land_unit(r) {
        if !from_water && to_water {
            let can_embark = unit.has_unique(r, UniqueType::LandUnitEmbarkation)
                || world.civ(unit.owner).is_some_and(|c| {
                    c.techs.iter().any(|t| {
                        r.technologies.get(*t).is_some_and(|tech| {
                            tech.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::LandUnitEmbarkation))
                        })
                    })
                });
            if !can_embark {
                return None;
            }
            return Some(Fixed::whole(100));
        } else if from_water && !to_water {
            return Some(Fixed::whole(100));
        }
    }

    if is_movement_affected_by_zoc(world, r, unit, from, to) {
        return Some(Fixed::whole(100));
    }

    if world.cities.iter().any(|(_, c)| c.centre == to) {
        return Some(CITY_CENTRE_COST);
    }

    let from_road = world.grid.unpillaged_road(from);
    let to_road = world.grid.unpillaged_road(to);
    let has_river = world.grid.has_river_between(from, to);

    let bridges = world.civ(unit.owner).is_some_and(|c| {
        c.techs.iter().any(|t| {
            r.technologies.get(*t).is_some_and(|tech| {
                tech.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::RoadsConnectAcrossRivers))
            })
        })
    });

    if from_road == RoadLevel::Railroad && to_road == RoadLevel::Railroad && (!has_river || bridges) {
        return Some(Fixed::from_fraction(1, 5));
    }

    if from_road >= RoadLevel::Road && to_road >= RoadLevel::Road && (!has_river || bridges) {
        return Some(Fixed::from_fraction(1, 2));
    }

    if has_river && !unit.has_unique(r, UniqueType::IgnoresTerrainCost) {
        return Some(Fixed::whole(100));
    }

    if unit.has_unique(r, UniqueType::IgnoresTerrainCost) {
        return Some(Fixed::ONE);
    }

    let is_hill = world
        .grid
        .terrains(to)
        .any(|t_id| r.terrains.get(t_id).is_some_and(|t| t.r#type == "Hill" || t.name == "Hill"));
    if is_hill
        && world.civ(unit.owner).is_some_and(|c| {
            r.nations
                .get(c.nation)
                .is_some_and(|n| n.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::IgnoreHillMovementCost)))
        })
    {
        return Some(Fixed::ONE);
    }

    Some(terrain)
}

/// Every tile reachable for `unit_id` with its current movement and state.
pub fn reachable_for_unit(world: &World, r: &Ruleset, unit_id: Id<Unit>) -> BTreeMap<TileId, Fixed> {
    let Some(unit) = world.units.get(unit_id) else {
        return BTreeMap::new();
    };
    let from = unit.position;
    let budget = unit.movement_left;
    let mut best: BTreeMap<TileId, Fixed> = BTreeMap::new();
    best.insert(from, Fixed::ZERO);
    if budget <= Fixed::ZERO {
        return best;
    }

    let mut frontier: Vec<(Fixed, TileId)> = vec![(Fixed::ZERO, from)];

    while !frontier.is_empty() {
        frontier.sort_by(|a, b| b.cmp(a));
        let (cost_here, tile) = frontier.pop().expect("non-empty");
        if best.get(&tile).is_some_and(|c| *c < cost_here) {
            continue;
        }

        for next in world.grid.neighbours(tile) {
            if !can_pass_through(world, r, next, unit) {
                continue;
            }

            let Some(step) = step_cost_for_unit(world, r, unit, tile, next, budget - cost_here) else {
                continue;
            };

            let raw = cost_here + step;
            let (total, can_continue) = if raw < budget { (raw, true) } else { (budget, false) };

            if best.get(&next).is_none_or(|c| *c > total) {
                best.insert(next, total);
                if can_continue {
                    frontier.push((total, next));
                }
            }
        }
    }

    best.retain(|&tile, _| tile == from || !is_tile_occupied_for(world, r, tile, unit));
    best
}

/// Whether this unit's civ lets land units take to the water (`Optics`, or
/// the unit's own unique).
fn can_embark(world: &World, r: &Ruleset, unit: &Unit) -> bool {
    unit.has_unique(r, UniqueType::LandUnitEmbarkation)
        || world.civ(unit.owner).is_some_and(|c| {
            c.techs.iter().any(|t| {
                r.technologies.get(*t).is_some_and(|tech| {
                    tech.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::LandUnitEmbarkation))
                })
            })
        })
}

/// Whether `unit` could ever step from `from` to the adjacent `to` — the
/// map's standing rules only: terrain, closed borders, land and water.
///
/// Who stands where and how far it can go this turn are left out; this is
/// for planning a route over many turns, which [`reachable_for_unit`] cannot.
pub fn can_traverse(world: &World, r: &Ruleset, unit: &Unit, from: TileId, to: TileId) -> bool {
    if !is_passable(&world.grid, r, to) {
        return false;
    }
    if let Some(owner) = world.grid.owner[to.0 as usize] {
        if !may_enter_territory(world, unit.owner, owner) {
            return false;
        }
    }
    let to_water = world.grid.is_water(r, to);
    if unit.is_water_unit(r) {
        return to_water || world.cities.iter().any(|(_, c)| c.centre == to);
    }
    if unit.is_land_unit(r) && to_water && !world.grid.is_water(r, from) {
        return can_embark(world, r, unit);
    }
    true
}

/// Steps from each tile within `radius` of `target` to `target`, for this
/// unit, by the map's standing rules. Searched outward from the goal, so one
/// search answers "how far is it from here" for every tile the unit could
/// reach this turn.
pub fn distances_to(world: &World, r: &Ruleset, unit: &Unit, target: TileId, radius: u32) -> BTreeMap<TileId, u32> {
    let mut dist: BTreeMap<TileId, u32> = BTreeMap::new();
    dist.insert(target, 0);
    let mut frontier = vec![target];
    let mut steps = 0;
    while !frontier.is_empty() && steps < radius * 2 {
        steps += 1;
        let mut next = Vec::new();
        for tile in frontier {
            for from in world.grid.neighbours(tile) {
                if dist.contains_key(&from) || world.grid.distance(from, target) > radius {
                    continue;
                }
                // Could the unit step *from* there *to* here?
                if can_traverse(world, r, unit, from, tile) {
                    dist.insert(from, steps);
                    next.push(from);
                }
            }
        }
        next.sort();
        frontier = next;
    }
    dist
}

/// The tile to move to this turn to get closer to `target` by a route that
/// exists — fewest steps left, then lowest tile id. `None` when there is no
/// route within reach, or nothing this turn gets any closer.
pub fn step_toward(world: &World, r: &Ruleset, unit_id: Id<Unit>, target: TileId) -> Option<TileId> {
    let unit = world.units.get(unit_id)?;
    let radius = world.grid.distance(unit.position, target) + 12;
    let dist = distances_to(world, r, unit, target, radius);
    let here = *dist.get(&unit.position)?;
    reachable_for_unit(world, r, unit_id)
        .into_keys()
        .filter(|t| *t != unit.position)
        .filter_map(|t| dist.get(&t).map(|d| (*d, t)))
        .filter(|(d, _)| *d < here)
        .min()
        .map(|(_, t)| t)
}

/// Send `visitor`'s units out of `host`'s land, each to the nearest tile it
/// may stand on (`teleportToClosestMoveableTile`) — what a peace treaty, or
/// an open-borders agreement running out, does to an army left inside.
pub fn expel(world: &mut World, r: &Ruleset, visitor: CivId, host: CivId) {
    let inside: Vec<Id<Unit>> = world
        .units
        .iter()
        .filter(|(_, u)| u.owner == visitor && world.grid.owner[u.position.0 as usize] == Some(host))
        .map(|(id, _)| id)
        .collect();
    for id in inside {
        let Some(unit) = world.units.get(id).cloned() else { continue };
        let fits = |t: TileId| {
            is_passable(&world.grid, r, t)
                && world.grid.is_water(r, t) == unit.is_water_unit(r)
                && world.grid.owner[t.0 as usize].is_none_or(|o| may_enter_territory(world, visitor, o))
                && !is_tile_occupied_for(world, r, t, &unit)
        };
        let dest = (1..=30u32).find_map(|d| {
            let mut ring = world.grid.ring(unit.position, d);
            ring.sort();
            ring.into_iter().find(|t| fits(*t))
        });
        if let (Some(t), Some(u)) = (dest, world.units.get_mut(id)) {
            u.position = t;
            u.embarked = false;
        }
    }
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
    /// A tile already occupied by a friendly unit of the same type or an enemy.
    TileOccupied,
}

impl std::fmt::Display for MoveRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            MoveRejection::NoSuchUnit => "no such unit",
            MoveRejection::Impassable => "the unit cannot enter that terrain",
            MoveRejection::OutOfRange => "further than this turn's movement allows",
            MoveRejection::NoMovement => "no movement to spend",
            MoveRejection::TileOccupied => "the tile is occupied",
        })
    }
}

impl std::error::Error for MoveRejection {}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MoveEvent {
    pub unit: Id<Unit>,
    pub from: TileId,
    pub to: TileId,
    pub spent: Fixed,
}

/// Move a unit, spending movement. Fails rather than half-moving.
pub fn move_unit(world: &mut World, r: &Ruleset, unit_id: Id<Unit>, to: TileId) -> Result<MoveEvent, MoveRejection> {
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

    // Every rule of the map — roads, rivers, embarking, zones of control,
    // closed borders, one unit to a tile — not the terrain alone.
    let cost = *reachable_for_unit(world, r, unit_id).get(&to).ok_or(MoveRejection::OutOfRange)?;

    let unit = world.units.get_mut(unit_id).ok_or(MoveRejection::NoSuchUnit)?;
    let from_water = world.grid.is_water(r, from);
    let to_water = world.grid.is_water(r, to);
    let embark_transition = (!from_water && to_water) || (from_water && !to_water);
    if !from_water && to_water {
        unit.embarked = true;
    } else if from_water && !to_water {
        unit.embarked = false;
    }
    let spent = if embark_transition && unit.is_land_unit(r) { budget } else { cost };
    unit.position = to;
    unit.movement_left = (budget - spent).max(Fixed::ZERO);
    unit.fortified = false;
    Ok(MoveEvent { unit: unit_id, from, to, spent })
}

/// Restore every unit's movement. Called when its owner's turn begins.
pub fn refresh_movement(world: &mut World, r: &Ruleset) {
    let refreshed: Vec<(Id<Unit>, i32)> =
        world.units.iter().map(|(id, u)| (id, r.units.get(u.base).map_or(2, |b| b.movement))).collect();
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
        Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings").expect("readable").expect("loads")
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

    #[test]
    fn road_and_railroad_movement_discounts() {
        let r = ruleset();
        let mut w = world(&r, "Grassland");
        let a = w.grid.id_at(4, 4).unwrap();
        let b = w.grid.id_at(4, 5).unwrap();
        let id = warrior(&mut w, &r, a);
        let u = w.units.get(id).unwrap();

        // No roads: step cost is 1.0
        assert_eq!(step_cost_for_unit(&w, &r, u, a, b, Fixed::whole(2)), Some(Fixed::ONE));

        // Both Road: step cost is 0.5
        w.grid.road[a.0 as usize] = RoadLevel::Road;
        w.grid.road[b.0 as usize] = RoadLevel::Road;
        assert_eq!(step_cost_for_unit(&w, &r, u, a, b, Fixed::whole(2)), Some(Fixed::from_fraction(1, 2)));

        // Both Railroad: step cost is 0.2
        w.grid.road[a.0 as usize] = RoadLevel::Railroad;
        w.grid.road[b.0 as usize] = RoadLevel::Railroad;
        assert_eq!(step_cost_for_unit(&w, &r, u, a, b, Fixed::whole(2)), Some(Fixed::from_fraction(1, 5)));
    }

    #[test]
    fn river_crossing_expends_movement_unless_bridged() {
        let r = ruleset();
        let mut w = world(&r, "Grassland");
        let a = w.grid.id_at(4, 4).unwrap();
        let b = w.grid.id_at(4, 5).unwrap();
        let id = warrior(&mut w, &r, a);

        w.grid.set_river_between(a, b);
        // Crossing river without roads/bridges expends full movement (cost 100)
        assert_eq!(
            step_cost_for_unit(&w, &r, w.units.get(id).unwrap(), a, b, Fixed::whole(2)),
            Some(Fixed::whole(100))
        );

        // Add roads but no bridge tech -> still full cost
        w.grid.road[a.0 as usize] = RoadLevel::Road;
        w.grid.road[b.0 as usize] = RoadLevel::Road;
        assert_eq!(
            step_cost_for_unit(&w, &r, w.units.get(id).unwrap(), a, b, Fixed::whole(2)),
            Some(Fixed::whole(100))
        );

        // Give civ Engineering (bridge tech)
        if let Some(tech_id) = r.technologies.id("Engineering") {
            w.civ_mut(CivId(0)).unwrap().techs.insert(tech_id);
            assert_eq!(
                step_cost_for_unit(&w, &r, w.units.get(id).unwrap(), a, b, Fixed::whole(2)),
                Some(Fixed::from_fraction(1, 2))
            );
        }
    }

    #[test]
    fn embarkation_requires_optics_and_toggles_state() {
        let r = ruleset();
        let mut w = world(&r, "Grassland");
        let land_tile = w.grid.id_at(4, 4).unwrap();
        let water_tile = w.grid.id_at(4, 5).unwrap();
        let coast_id = r.terrain("Coast").unwrap();
        w.grid.terrain[water_tile.0 as usize] = coast_id;
        assert!(w.grid.is_water(&r, water_tile));

        let id = warrior(&mut w, &r, land_tile);

        // Without Optics: cannot embark onto water
        assert_eq!(step_cost_for_unit(&w, &r, w.units.get(id).unwrap(), land_tile, water_tile, Fixed::whole(2)), None);

        // Give Optics (enables embarkation)
        let optics = r.technologies.id("Optics").expect("Optics exists");
        w.civ_mut(CivId(0)).unwrap().techs.insert(optics);

        // Can embark, costs 100 (whole budget)
        assert_eq!(
            step_cost_for_unit(&w, &r, w.units.get(id).unwrap(), land_tile, water_tile, Fixed::whole(2)),
            Some(Fixed::whole(100))
        );

        // Move onto water
        let event = move_unit(&mut w, &r, id, water_tile).expect("can embark");
        assert_eq!(event.spent, Fixed::whole(2)); // clamped
        assert!(w.units.get(id).unwrap().embarked);

        // Refresh and disembark onto land
        w.units.get_mut(id).unwrap().refresh(2);
        let disembark_event = move_unit(&mut w, &r, id, land_tile).expect("can disembark");
        assert_eq!(disembark_event.spent, Fixed::whole(2));
        assert!(!w.units.get(id).unwrap().embarked);
    }

    #[test]
    fn zone_of_control_halts_movement() {
        let r = ruleset();
        let mut w = world(&r, "Grassland");
        let a = w.grid.id_at(4, 4).unwrap();
        let b = w.grid.id_at(4, 5).unwrap();
        let id_a = warrior(&mut w, &r, a);

        let shared = w.grid.neighbours(a).find(|&n| n != b && w.grid.distance(n, b) == 1).unwrap();

        let nation = r.nations.ids().next().unwrap();
        w.civs.push(Civilization::new(CivId(1), nation, true));
        let enemy_warrior = r.unit("Warrior").unwrap();
        w.place_unit(Unit::new(enemy_warrior, CivId(1), shared, 2));

        w.diplomacy.meet(CivId(0), CivId(1));
        w.diplomacy.declare_war(CivId(0), CivId(1));

        assert!(is_movement_affected_by_zoc(&w, &r, w.units.get(id_a).unwrap(), a, b));
        assert_eq!(
            step_cost_for_unit(&w, &r, w.units.get(id_a).unwrap(), a, b, Fixed::whole(2)),
            Some(Fixed::whole(100))
        );
    }

    #[test]
    fn stacking_and_1upt_rules() {
        let r = ruleset();
        let mut w = world(&r, "Grassland");
        let t1 = w.grid.id_at(4, 4).unwrap();
        let t2 = w.grid.id_at(4, 5).unwrap();

        let _mil1 = warrior(&mut w, &r, t1);
        let mil2 = warrior(&mut w, &r, t2);

        // Another military unit cannot end turn on t1
        let u2 = w.units.get(mil2).unwrap();
        assert!(is_tile_occupied_for(&w, &r, t1, u2));

        // But a civilian worker CAN share tile with a military unit
        let worker_base = r.unit("Worker").unwrap();
        let civ_worker = w.place_unit(Unit::new(worker_base, CivId(0), t2, 2));
        let worker_unit = w.units.get(civ_worker).unwrap();
        // Military on t1 does not occupy t1 for worker
        assert!(!is_tile_occupied_for(&w, &r, t1, worker_unit));
    }
}
