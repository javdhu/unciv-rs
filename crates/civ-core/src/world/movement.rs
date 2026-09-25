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
//! Search order is fixed. The frontier takes the least `(cost, TileId)`
//! from a priority queue, because two machines walking the same map must
//! expand the same tiles in the same order or their units end up in different
//! places.

use crate::arena::Id;
use crate::ruleset::Ruleset;
use crate::stats::Fixed;
use crate::unique_type::UniqueType;
use crate::world::entities::Unit;
use crate::world::grid::{CivId, RoadLevel, TileGrid, TileId};
use crate::world::world::World;
use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap};

/// A city centre always costs one to enter, whatever it was built on.
const CITY_CENTRE_COST: Fixed = Fixed::ONE;

type CityCentres = BTreeMap<TileId, CivId>;

fn city_centres(world: &World) -> CityCentres {
    world.cities.iter().map(|(_, city)| (city.centre, city.owner)).collect()
}

fn city_owner(world: &World, centres: Option<&CityCentres>, tile: TileId) -> Option<CivId> {
    match centres {
        Some(centres) => centres.get(&tile).copied(),
        None => world.cities.iter().find(|(_, city)| city.centre == tile).map(|(_, city)| city.owner),
    }
}

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

fn active_uniques<'a>(
    world: &'a World,
    r: &'a Ruleset,
    unit: &'a Unit,
    tile: TileId,
) -> impl Iterator<Item = &'a crate::Unique> {
    let ctx = crate::world::conditionals::GameContext::new(r)
        .with_world(world)
        .for_civ(unit.owner)
        .for_unit_value(unit)
        .on_tile(tile);
    unit.all_uniques(r)
        .chain(world.civ(unit.owner).into_iter().flat_map(|c| c.uniques.all()))
        .filter(move |u| crate::world::conditionals::applies(u, &ctx).holds())
}

fn civ_has(world: &World, r: &Ruleset, unit: &Unit, kind: UniqueType) -> bool {
    let ctx = crate::world::conditionals::GameContext::new(r)
        .with_world(world)
        .for_civ(unit.owner)
        .for_unit_value(unit)
        .on_tile(unit.position);
    world.civ(unit.owner).is_some_and(|c| {
        c.uniques
            .matching(kind)
            .chain(c.leader_uniques(r).iter().filter(|u| u.unique_type() == Some(kind)))
            .chain(c.techs.iter().flat_map(|t| &r.technologies[*t].uniques).filter(|u| u.unique_type() == Some(kind)))
            .any(|u| crate::world::conditionals::applies(u, &ctx).holds())
    })
}

fn unit_terrain_passable(world: &World, r: &Ruleset, unit: &Unit, tile: TileId) -> bool {
    if is_passable(&world.grid, r, tile) || unit.has_unique(r, UniqueType::CanPassImpassable) {
        return true;
    }
    if unit.has_unique(r, UniqueType::CanEnterIceTiles)
        && world.grid.features[tile.0 as usize].iter().any(|id| r.terrains[*id].name == "Ice")
    {
        return true;
    }
    if !unit.is_land_unit(r) {
        return false;
    }
    let Some(civ) = world.civ(unit.owner) else {
        return false;
    };
    active_uniques(world, r, unit, tile)
        .filter(|u| u.unique_type() == Some(UniqueType::LandUnitsCrossTerrainAfterUnitGained))
        .any(|u| {
            let (Some(terrain), Some(filter)) = (u.params.first(), u.params.get(1)) else {
                return false;
            };
            world.grid.terrains(tile).any(|id| r.terrains[id].name == *terrain)
                && civ.units_gained.iter().any(|base| {
                    crate::world::filters::unit_matches(r, &Unit::new(*base, unit.owner, tile, 0), filter).holds()
                })
        })
}

pub fn max_movement(world: &World, r: &Ruleset, unit: &Unit) -> i32 {
    let own = |u: &Unit| {
        let base = if u.embarked { 2 } else { r.units.get(u.base).map_or(2, |b| b.movement) };
        (base
            + active_uniques(world, r, u, u.position)
                .filter(|u| u.unique_type() == Some(UniqueType::Movement))
                .filter_map(|u| u.params.first()?.parse::<i32>().ok())
                .sum::<i32>())
        .max(1)
    };
    let mut maximum = own(unit);
    for (_, other) in world.units.iter().filter(|(_, u)| u.owner == unit.owner && u.position == unit.position) {
        if active_uniques(world, r, other, other.position).any(|u| {
            u.unique_type() == Some(UniqueType::TransferMovement)
                && u.params.first().is_some_and(|f| crate::world::filters::unit_matches(r, unit, f).holds())
        }) {
            maximum = maximum.max(own(other));
        }
    }
    maximum
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
    let mut frontier = BinaryHeap::from([Reverse((Fixed::ZERO, from))]);

    while let Some(Reverse((cost_here, tile))) = frontier.pop() {
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
                    frontier.push(Reverse((total, next)));
                }
            }
        }
    }
    best
}

/// Returns whether movement between adjacent tiles `from` and `to` is affected by Zone of Control.
pub fn is_movement_affected_by_zoc(world: &World, r: &Ruleset, unit: &Unit, from: TileId, to: TileId) -> bool {
    is_movement_affected_by_zoc_with_centres(world, r, unit, from, to, None)
}

fn is_movement_affected_by_zoc_with_centres(
    world: &World, r: &Ruleset, unit: &Unit, from: TileId, to: TileId, centres: Option<&CityCentres>,
) -> bool {
    if unit.has_unique(r, UniqueType::IgnoresZOC) {
        return false;
    }
    for n in world.grid.neighbours(from) {
        if world.grid.distance(n, to) != 1 {
            continue;
        }
        if let Some(owner) = city_owner(world, centres, n) {
            if world.diplomacy.at_war(unit.owner, owner) {
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
    if unit.is_air(r) {
        return crate::world::air::base_for(world, r, unit, tile, None).is_none();
    }
    let is_civ = unit.is_civilian(r);
    for (_, other) in world.units.iter() {
        if other.position != tile {
            continue;
        }
        if other.owner != unit.owner {
            return true;
        }
        if other.is_air(r) {
            continue;
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

/// Territory friendly for healing and tile conditionals, including granted
/// open borders and friendly city-states (`Tile.isFriendlyTerritory`).
pub fn is_friendly_territory(world: &World, civ: CivId, tile: TileId) -> bool {
    let Some(owner) = world.grid.owner[tile.0 as usize] else { return false };
    if owner == civ {
        return true;
    }
    if !world.diplomacy.have_met(civ, owner) {
        return false;
    }
    if world.civ(owner).is_some_and(|c| c.is_city_state()) {
        use crate::world::city_states::{relationship, Relationship};
        if matches!(relationship(world, owner, civ), Relationship::Friend | Relationship::Ally)
            || world.civ(civ).is_some_and(|c| c.uniques.has(UniqueType::CityStateTerritoryAlwaysFriendly))
        {
            return true;
        }
    }
    world.diplomacy.may_enter_borders_of(civ, owner)
        || (world.civ(civ).is_some_and(|c| c.is_city_state())
            && crate::world::city_states::ally_of(world, civ) == Some(owner))
}

/// Whether `unit` can path through `tile`.
pub fn may_unit_enter_territory(world: &World, r: &Ruleset, unit: &Unit, owner: CivId) -> bool {
    may_enter_territory(world, unit.owner, owner)
        || unit.has_unique(r, UniqueType::CanEnterForeignTiles)
        || unit.has_unique(r, UniqueType::CanEnterForeignTilesButLosesReligiousStrength)
}

/// Whether this unit may pass through a tile, including its border privileges.
pub fn can_pass_through(world: &World, r: &Ruleset, tile: TileId, unit: &Unit) -> bool {
    can_pass_through_with_centres(world, r, tile, unit, None)
}

fn can_pass_through_with_centres(
    world: &World, r: &Ruleset, tile: TileId, unit: &Unit, centres: Option<&CityCentres>,
) -> bool {
    if let Some(owner) = world.grid.owner[tile.0 as usize] {
        if !may_unit_enter_territory(world, r, unit, owner) {
            return false;
        }
    }
    for (_, other) in world.units.iter() {
        if other.position == tile && other.owner != unit.owner && world.diplomacy.at_war(unit.owner, other.owner) {
            return false;
        }
    }
    if let Some(owner) = city_owner(world, centres, tile) {
        if owner != unit.owner && world.diplomacy.at_war(unit.owner, owner) {
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
    step_cost_for_unit_with_zoc(world, r, unit, from, to, budget, true)
}

/// Combat advances may ignore zones of control for units allowed to keep moving.
pub fn step_cost_for_unit_with_zoc(
    world: &World,
    r: &Ruleset,
    unit: &Unit,
    from: TileId,
    to: TileId,
    budget: Fixed,
    consider_zoc: bool,
) -> Option<Fixed> {
    step_cost_for_unit_with_centres(world, r, unit, from, to, budget, consider_zoc, None)
}

fn step_cost_for_unit_with_centres(
    world: &World,
    r: &Ruleset,
    unit: &Unit,
    from: TileId,
    to: TileId,
    budget: Fixed,
    consider_zoc: bool,
    centres: Option<&CityCentres>,
) -> Option<Fixed> {
    let _ = budget;
    step_cost_parts(world, r, unit, from, to, consider_zoc, centres).map(|(cost, _)| cost)
}

/// [`step_cost_for_unit`], and whether a zone of control set it: such a step costs all the movement left.
pub fn step_cost_and_zoc(world: &World, r: &Ruleset, unit: &Unit, from: TileId, to: TileId) -> Option<(Fixed, bool)> {
    step_cost_parts(world, r, unit, from, to, true, None)
}

fn step_cost_parts(
    world: &World,
    r: &Ruleset,
    unit: &Unit,
    from: TileId,
    to: TileId,
    consider_zoc: bool,
    centres: Option<&CityCentres>,
) -> Option<(Fixed, bool)> {
    if !can_traverse_with_borders(world, r, unit, from, to, true, centres) {
        return None;
    }
    let from_water = world.grid.is_water(r, from);
    let to_water = world.grid.is_water(r, to);
    if unit.is_land_unit(r) && from_water != to_water && !unit.has_unique(r, UniqueType::CanMoveOnWater) {
        let kind = if from_water { UniqueType::ReducedDisembarkCost } else { UniqueType::ReducedEmbarkCost };
        let cost = active_uniques(world, r, unit, to)
            .filter(|u| u.unique_type() == Some(kind))
            .filter_map(|u| u.params.first().and_then(|n| Fixed::parse(n)))
            .min()
            .unwrap_or(Fixed::whole(100));
        return Some((cost, false));
    }
    if consider_zoc && is_movement_affected_by_zoc_with_centres(world, r, unit, from, to, centres) {
        return Some((Fixed::whole(100), true));
    }
    open_step_cost(world, r, unit, from, to, centres).map(|cost| (cost, false))
}

/// A step that neither crosses the shore nor meets a zone of control: roads, rivers, terrain and walls.
fn open_step_cost(world: &World, r: &Ruleset, unit: &Unit, from: TileId, to: TileId, centres: Option<&CityCentres>) -> Option<Fixed> {
    if unit.has_unique(r, UniqueType::AllTilesCost1Move) {
        return Some(Fixed::ONE);
    }
    let extra = world.grid.owner[to.0 as usize]
        .filter(|owner| world.diplomacy.at_war(unit.owner, *owner))
        .and_then(|owner| world.civ(owner))
        .map_or(Fixed::ZERO, |owner| {
            let ctx = crate::world::conditionals::GameContext::new(r)
                .with_world(world)
                .for_civ(owner.id)
                .for_unit_value(unit)
                .on_tile(to);
            owner
                .uniques
                .matching(UniqueType::EnemyUnitsSpendExtraMovement)
                .filter(|u| crate::world::conditionals::applies(u, &ctx).holds())
                .filter(|u| u.params.first().is_some_and(|f| crate::world::filters::unit_matches(r, unit, f).holds()))
                .filter_map(|u| u.params.get(1).and_then(|n| Fixed::parse(n)))
                .fold(Fixed::ZERO, |a, b| a + b)
        });
    let road = |tile| {
        let placed = world.grid.unpillaged_road(tile);
        if placed != RoadLevel::None {
            return placed;
        }
        let forest = civ_has(world, r, unit, UniqueType::ForestsAndJunglesAreRoads)
            && is_friendly_territory(world, unit.owner, tile)
            && world.grid.features[tile.0 as usize]
                .iter()
                .any(|f| matches!(r.terrains[*f].name.as_str(), "Forest" | "Jungle"));
        if forest || city_owner(world, centres, tile).is_some() {
            RoadLevel::Road
        } else {
            RoadLevel::None
        }
    };
    let a = road(from);
    let b = road(to);
    let river = world.grid.has_river_between(from, to);
    if a == RoadLevel::Railroad && b == RoadLevel::Railroad {
        return Some(Fixed::from_fraction(1, 10) + extra);
    }
    if a >= RoadLevel::Road
        && b >= RoadLevel::Road
        && (!river || civ_has(world, r, unit, UniqueType::RoadsConnectAcrossRivers))
    {
        return Some(
            if civ_has(world, r, unit, UniqueType::RoadMovementSpeed) {
                Fixed::from_fraction(1, 3)
            } else {
                Fixed::from_fraction(1, 2)
            } + extra,
        );
    }
    if unit.has_unique(r, UniqueType::IgnoresTerrainCost) {
        return Some(Fixed::ONE + extra);
    }
    if river {
        return Some(Fixed::whole(100));
    }
    let terrain = if city_owner(world, centres, to).is_some() {
        CITY_CENTRE_COST
    } else {
        Fixed::whole(r.terrains[world.grid.terrains(to).last()?].movement_cost.unwrap_or(1))
    };
    let ctx = crate::world::conditionals::GameContext::new(r)
        .with_world(world)
        .for_civ(unit.owner)
        .for_unit_value(unit)
        .on_tile(to);
    let double = |features_only| {
        active_uniques(world, r, unit, to).filter(|u| u.unique_type() == Some(UniqueType::DoubleMovementOnTerrain)).any(
            |u| {
                u.params.first().is_some_and(|f| {
                    if features_only {
                        world.grid.features[to.0 as usize].iter().any(|id| r.terrains[*id].name == *f)
                    } else {
                        crate::world::conditionals::tile_matches(&ctx, to, f).holds()
                    }
                })
            },
        )
    };
    if double(true) {
        return Some(terrain * Fixed::from_fraction(1, 2) + extra);
    }
    if unit.has_unique(r, UniqueType::RoughTerrainPenalty)
        && crate::world::conditionals::tile_matches(&ctx, to, "Rough terrain").holds()
    {
        return Some(Fixed::whole(100));
    }
    if civ_has(world, r, unit, UniqueType::IgnoreHillMovementCost)
        && crate::world::conditionals::tile_matches(&ctx, to, "Hill").holds()
    {
        return Some(Fixed::ONE + extra);
    }
    Some(if double(false) { terrain * Fixed::from_fraction(1, 2) } else { terrain } + extra)
}

/// Every tile reachable for `unit_id` with its current movement and state.
pub fn reachable_for_unit(world: &World, r: &Ruleset, unit_id: Id<Unit>) -> BTreeMap<TileId, Fixed> {
    search_for_unit(world, r, unit_id, None)
}

fn search_for_unit(
    world: &World,
    r: &Ruleset,
    unit_id: Id<Unit>,
    mut parents: Option<&mut BTreeMap<TileId, TileId>>,
) -> BTreeMap<TileId, Fixed> {
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

    let centres = city_centres(world);
    let mut frontier = BinaryHeap::from([Reverse((Fixed::ZERO, from))]);

    while let Some(Reverse((cost_here, tile))) = frontier.pop() {
        if best.get(&tile).is_some_and(|c| *c < cost_here) {
            continue;
        }

        for next in world.grid.neighbours(tile) {
            if !can_pass_through_with_centres(world, r, next, unit, Some(&centres)) {
                continue;
            }

            let Some(step) = step_cost_for_unit_with_centres(world, r, unit, tile, next, budget - cost_here, true, Some(&centres)) else {
                continue;
            };

            let raw = cost_here + step;
            let (total, can_continue) = if raw < budget { (raw, true) } else { (budget, false) };

            if best.get(&next).is_none_or(|c| *c > total) {
                best.insert(next, total);
                if let Some(parents) = parents.as_mut() {
                    parents.insert(next, tile);
                }
                if can_continue {
                    frontier.push(Reverse((total, next)));
                }
            }
        }
    }

    best.retain(|&tile, _| tile == from || !is_tile_occupied_for(world, r, tile, unit));
    best
}

/// Whether this unit's civ lets land units take to the water (`Optics`, or
/// the unit's own unique).
pub(crate) fn can_embark(world: &World, r: &Ruleset, unit: &Unit) -> bool {
    !unit.has_unique(r, UniqueType::CannotEmbark)
        && (unit.has_unique(r, UniqueType::LandUnitEmbarkation)
            || civ_has(world, r, unit, UniqueType::LandUnitEmbarkation))
}

/// Whether `unit` could ever step from `from` to the adjacent `to` — the
/// map's standing rules only: terrain, closed borders, land and water.
///
/// Who stands where and how far it can go this turn are left out; this is
/// for planning a route over many turns, which [`reachable_for_unit`] cannot.
pub fn can_traverse(world: &World, r: &Ruleset, unit: &Unit, from: TileId, to: TileId) -> bool {
    can_traverse_with_borders(world, r, unit, from, to, true, None)
}

fn can_traverse_with_borders(
    world: &World,
    r: &Ruleset,
    unit: &Unit,
    from: TileId,
    to: TileId,
    respect_borders: bool,
    centres: Option<&CityCentres>,
) -> bool {
    let _ = from;
    if !world.grid.contains(to) || !unit_terrain_passable(world, r, unit, to) {
        return false;
    }
    if let Some(owner) = world.grid.owner[to.0 as usize] {
        if respect_borders && !may_unit_enter_territory(world, r, unit, owner) {
            return false;
        }
    }
    let water = world.grid.is_water(r, to);
    // Ships enter land only at a city or through a canal (world::placement).
    if unit.is_water_unit(r) && !water && !world.grid.is_canal(to) && city_owner(world, centres, to).is_none()
    {
        return false;
    }
    let floating = unit.has_unique(r, UniqueType::CanMoveOnWater);
    if unit.is_land_unit(r) && water && !floating && !can_embark(world, r, unit) {
        return false;
    }
    if r.terrains[world.grid.terrain[to.0 as usize]].name == "Ocean" {
        let ocean_allowed = active_uniques(world, r, unit, to).any(|u| {
            u.unique_type() == Some(UniqueType::UnitsMayEnterOcean)
                && u.params.first().is_some_and(|f| {
                    f == "Embarked" && unit.is_land_unit(r) && !floating
                        || crate::world::filters::unit_matches(r, unit, f).holds()
                })
        });
        if !ocean_allowed
            && (active_uniques(world, r, unit, to).any(|u| u.unique_type() == Some(UniqueType::CannotEnterOcean))
                || unit.is_land_unit(r) && !floating)
        {
            return false;
        }
    }
    true
}

/// Steps from each tile within `radius` of `target` to `target`, for this
/// unit, by the map's standing rules. Searched outward from the goal, so one
/// search answers "how far is it from here" for every tile the unit could
/// reach this turn.
pub fn distances_to(world: &World, r: &Ruleset, unit: &Unit, target: TileId, radius: u32) -> BTreeMap<TileId, u32> {
    let centres = city_centres(world);
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
                if can_traverse_with_borders(world, r, unit, from, tile, true, Some(&centres)) {
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
        .filter(|(_, u)| {
            u.owner == visitor && u.transport.is_none() && world.grid.owner[u.position.0 as usize] == Some(host)
        })
        .filter(|(_, u)| !may_unit_enter_territory(world, r, u, host))
        .map(|(id, _)| id)
        .collect();
    for id in inside {
        displace_unit(world, r, id);
    }
}

/// Evacuate a unit that cannot stay after a border or city ownership change.
/// Legal occupants remain, and transported aircraft follow their carrier.
/// Search nearby terrain-connected tiles ignoring closed borders along the
/// route, then owned city tiles. Destroy a stranded unit and its payload.
/// Returns true only when the unit survives at a different position.
pub fn displace_unit(world: &mut World, r: &Ruleset, id: Id<Unit>) -> bool {
    let Some(unit) = world.units.get(id).cloned() else { return false };
    if unit.transport.is_some() {
        return false;
    }
    let origin = unit.position;
    let is_air = unit.is_air(r);
    let foreign_city = world.cities.iter().any(|(_, c)| c.centre == origin && c.owner != unit.owner);
    if !foreign_city
        && if is_air {
            crate::world::air::base_for(world, r, &unit, origin, Some(id)).is_some()
        } else {
            can_traverse(world, r, &unit, origin, origin) && can_pass_through(world, r, origin, &unit)
        }
    {
        return false;
    }

    let fits = |tile: TileId| {
        if is_air {
            crate::world::air::base_for(world, r, &unit, tile, Some(id)).is_some()
        } else {
            can_traverse(world, r, &unit, origin, tile) && !is_tile_occupied_for(world, r, tile, &unit)
        }
    };
    // Source getPathBetweenTiles ignores diplomatic borders, but still
    // respects terrain, embarkation and hostile occupants along the path.
    let mut reachable = std::collections::BTreeSet::from([origin]);
    if !is_air {
        let mut frontier = std::collections::VecDeque::from([origin]);
        while let Some(from) = frontier.pop_front() {
            for tile in world.grid.neighbours(from) {
                if reachable.contains(&tile)
                    || !can_traverse_with_borders(world, r, &unit, from, tile, false, None)
                    || world
                        .units
                        .iter()
                        .any(|(_, other)| other.position == tile && world.diplomacy.at_war(unit.owner, other.owner))
                    || world
                        .cities
                        .iter()
                        .any(|(_, city)| city.centre == tile && world.diplomacy.at_war(unit.owner, city.owner))
                {
                    continue;
                }
                reachable.insert(tile);
                frontier.push_back(tile);
            }
        }
    }
    let mut nearby = world.grid.within(origin, 5);
    nearby.sort_by_key(|tile| (world.grid.distance(origin, *tile), *tile));
    let destination = nearby
        .into_iter()
        .find(|tile| {
            *tile != origin
                && (is_air || reachable.contains(tile))
                && world.grid.owner[tile.0 as usize].is_none_or(|owner| !world.diplomacy.at_war(unit.owner, owner))
                && fits(*tile)
        })
        .or_else(|| {
            let mut owned: Vec<_> = world
                .cities
                .iter()
                .filter(|(_, c)| c.owner == unit.owner)
                .flat_map(|(_, city)| city.tiles.iter().copied())
                .collect();
            owned.sort_by_key(|tile| (world.grid.distance(origin, *tile), *tile));
            owned.into_iter().find(|tile| *tile != origin && fits(*tile))
        });
    let Some(tile) = destination else {
        world.remove_unit(id);
        return false;
    };
    let transport = is_air
        .then(|| crate::world::air::base_for(world, r, &unit, tile, Some(id)))
        .flatten()
        .and_then(|base| base.transport());
    let embarked = !is_air
        && unit.is_land_unit(r)
        && world.grid.is_water(r, tile)
        && !unit.has_unique(r, UniqueType::CanMoveOnWater);
    let moved = world.units.get_mut(id).expect("unit to displace");
    moved.position = tile;
    moved.transport = transport;
    moved.embarked = embarked;
    moved.fortified = false;
    moved.fortification_turns = 0;
    moved.set_up = false;
    crate::world::air::move_payload(world, id, tile);
    let payload: Vec<_> = world.units.iter().filter(|(_, u)| u.transport == Some(id)).map(|(uid, _)| uid).collect();
    let sites = crate::world::natural_wonders::sites(&world.grid);
    let mods = crate::world::turn::TurnModifiers { difficulty: world.game_difficulty(r), ..Default::default() };
    let mut events = Vec::new();
    for entered in std::iter::once(id).chain(payload) {
        crate::world::turn::explore_on_entry(world, r, entered, mods, &sites, &mut events);
    }
    true
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

/// A step across the shore: a land unit boarding, or stepping back onto land.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Embark {
    Embark,
    Land,
}

impl Embark {
    /// The crossing between a unit's embarked flag before and after a step, if it changed.
    pub fn between(was: bool, now: bool) -> Option<Embark> {
        match (was, now) {
            (false, true) => Some(Embark::Embark),
            (true, false) => Some(Embark::Land),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MoveEvent {
    pub unit: Id<Unit>,
    pub from: TileId,
    pub to: TileId,
    pub spent: Fixed,
    /// Set on the step where the unit boarded or landed.
    pub embark: Option<Embark>,
    /// The step entered an enemy's zone of control, which cost all the movement left.
    pub zoc: bool,
}

impl MoveEvent {
    /// A move that neither crossed the shore nor met a zone of control.
    pub fn plain(unit: Id<Unit>, from: TileId, to: TileId, spent: Fixed) -> MoveEvent {
        MoveEvent { unit, from, to, spent, embark: None, zoc: false }
    }
}

/// Move a unit, spending movement. Fails rather than half-moving.
pub fn move_unit(world: &mut World, r: &Ruleset, unit_id: Id<Unit>, to: TileId) -> Result<MoveEvent, MoveRejection> {
    move_unit_with(world, r, unit_id, to, |_, _| {})
}

/// Follow a validated route, resolving tile-entry effects after each step.
/// A callback that consumes movement (for example a ruins upgrade) can stop
/// the route early; the returned event reports where the unit actually ended.
pub fn move_unit_with(
    world: &mut World,
    r: &Ruleset,
    unit_id: Id<Unit>,
    to: TileId,
    mut entered: impl FnMut(&mut World, MoveEvent),
) -> Result<MoveEvent, MoveRejection> {
    let unit = world.units.get(unit_id).ok_or(MoveRejection::NoSuchUnit)?;
    let (from, budget) = (unit.position, unit.movement_left);
    if from == to {
        return Err(MoveRejection::NoMovement);
    }
    if budget <= Fixed::ZERO {
        return Err(MoveRejection::NoMovement);
    }
    if !unit_terrain_passable(world, r, unit, to) {
        return Err(MoveRejection::Impassable);
    }

    // Every rule of the map — roads, rivers, embarking, zones of control,
    // closed borders, one unit to a tile — not the terrain alone.
    let mut parents = BTreeMap::new();
    let reachable = search_for_unit(world, r, unit_id, Some(&mut parents));
    if !reachable.contains_key(&to) {
        return Err(MoveRejection::OutOfRange);
    }
    let mut path = Vec::new();
    let mut cursor = to;
    while cursor != from {
        path.push(cursor);
        cursor = *parents.get(&cursor).ok_or(MoveRejection::OutOfRange)?;
    }
    let mut last = from;
    let mut spent = Fixed::ZERO;
    for next in path.into_iter().rev() {
        let Some(unit) = world.units.get(unit_id) else { break };
        if unit.movement_left <= Fixed::ZERO || unit.position != last {
            break;
        }
        // Entry effects can spawn a unit or change territory after the route
        // was found. Validate the next step against that updated world.
        if !can_pass_through(world, r, next, unit) || (next == to && is_tile_occupied_for(world, r, next, unit)) {
            break;
        }
        let Some((cost, zoc)) = step_cost_and_zoc(world, r, unit, last, next) else { break };
        let cost = cost.min(unit.movement_left);
        let embarked =
            unit.is_land_unit(r) && !unit.has_unique(r, UniqueType::CanMoveOnWater) && world.grid.is_water(r, next);
        let embark = Embark::between(unit.embarked, embarked);
        let unit = world.units.get_mut(unit_id).expect("unit checked above");
        unit.position = next;
        unit.embarked = embarked;
        unit.movement_left -= cost;
        unit.moved_this_turn = true;
        unit.fortified = false;
        unit.fortification_turns = 0;
        unit.set_up = false;
        crate::world::air::move_payload(world, unit_id, next);
        crate::world::military::on_enter(world, r, unit_id);
        spent += cost;
        entered(world, MoveEvent { unit: unit_id, from: last, to: next, spent: cost, embark, zoc });
        last = next;
    }
    let ended = world.units.get(unit_id).map_or(last, |u| u.position);
    Ok(MoveEvent::plain(unit_id, from, ended, spent))
}

/// Restore every unit's movement. Called when its owner's turn begins.
pub fn refresh_movement(world: &mut World, r: &Ruleset) {
    let refreshed: Vec<_> = world
        .units
        .iter()
        .map(|(id, u)| (id, max_movement(world, r, u), crate::world::air::max_attacks(world, r, id)))
        .collect();
    for (id, movement, attacks) in refreshed {
        if let Some(u) = world.units.get_mut(id) {
            u.refresh(movement);
            u.attacks_left = attacks;
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
        Ruleset::load_dir("rules/gods-and-kings").expect("readable").expect("loads")
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

        // Both Railroad: step cost is 0.1
        w.grid.road[a.0 as usize] = RoadLevel::Railroad;
        w.grid.road[b.0 as usize] = RoadLevel::Railroad;
        assert_eq!(step_cost_for_unit(&w, &r, u, a, b, Fixed::whole(2)), Some(Fixed::from_fraction(1, 10)));
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

    /// Each step says whether it boarded or landed, and only the step that did.
    #[test]
    fn steps_name_the_shore_they_cross() {
        let r = ruleset();
        let mut w = world(&r, "Grassland");
        let (land, near, far) = (w.grid.id_at(4, 3).unwrap(), w.grid.id_at(4, 4).unwrap(), w.grid.id_at(4, 5).unwrap());
        for water in [near, far] {
            w.grid.terrain[water.0 as usize] = r.terrain("Coast").unwrap();
        }
        w.civ_mut(CivId(0)).unwrap().techs.insert(r.technologies.id("Optics").unwrap());
        let id = warrior(&mut w, &r, land);
        let mut steps = Vec::new();
        move_unit_with(&mut w, &r, id, near, |_, m| steps.push(m)).unwrap();
        w.units.get_mut(id).unwrap().refresh(2);
        move_unit_with(&mut w, &r, id, far, |_, m| steps.push(m)).unwrap();
        w.units.get_mut(id).unwrap().refresh(2);
        move_unit_with(&mut w, &r, id, near, |_, m| steps.push(m)).unwrap();
        w.units.get_mut(id).unwrap().refresh(2);
        move_unit_with(&mut w, &r, id, land, |_, m| steps.push(m)).unwrap();
        let crossings: Vec<_> = steps.iter().map(|m| (m.from, m.to, m.embark)).collect();
        assert_eq!(
            crossings,
            [(land, near, Some(Embark::Embark)), (near, far, None), (far, near, None), (near, land, Some(Embark::Land))]
        );
        assert!(steps.iter().all(|m| !m.zoc), "no enemy anywhere");
    }

    /// A step into an enemy's zone of control is marked, and costs the rest of the unit's movement.
    #[test]
    fn a_step_into_a_zone_of_control_is_marked() {
        let r = ruleset();
        let mut w = world(&r, "Grassland");
        let (a, b) = (w.grid.id_at(4, 4).unwrap(), w.grid.id_at(4, 5).unwrap());
        let id = warrior(&mut w, &r, a);
        let shared = w.grid.neighbours(a).find(|&n| n != b && w.grid.distance(n, b) == 1).unwrap();
        let nation = r.nations.ids().next().unwrap();
        w.civs.push(Civilization::new(CivId(1), nation, true));
        w.place_unit(Unit::new(r.unit("Warrior").unwrap(), CivId(1), shared, 2));
        w.diplomacy.meet(CivId(0), CivId(1));
        w.diplomacy.declare_war(CivId(0), CivId(1));
        let mut steps = Vec::new();
        move_unit_with(&mut w, &r, id, b, |_, m| steps.push(m)).unwrap();
        assert_eq!(steps.len(), 1);
        assert!(steps[0].zoc && steps[0].embark.is_none());
        assert_eq!((steps[0].spent, w.units.get(id).unwrap().movement_left), (Fixed::whole(2), Fixed::ZERO));
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
