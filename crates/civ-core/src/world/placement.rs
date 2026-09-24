//! Wonders and buildings that stand on a tile of their city (docs/CONTENT.md,
//! "Wonders and buildings change the map").
//!
//! A building whose rules say `Stands on a [tileFilter] tile` or `Stands next
//! to a [tileFilter] tile` is built on one tile of its city's workable area:
//!
//! * every `Stands on` filter is an alternative (Desert *or* Flood plains);
//!   every `Stands next to` filter must hold (a Mountain next door);
//! * a **world wonder takes its tile**: the improvement and any removable
//!   feature (one a `Remove …` improvement would clear) go, and no citizen
//!   works it again. The tile's own yield is given up; the wonder's yields
//!   and effects apply city-wide as before. A wonder stands on land;
//! * other buildings keep their tile workable and may stand on water (a
//!   Harbor on the coast); their tile effects are ordinary tile uniques that
//!   name the building as a tile filter (`[+1 Food] from [Harbor] tiles`);
//! * a building standing on an `Isthmus` tile is a canal: ships may pass.
//!
//! The player chooses a site (`Command::ChooseSite`); otherwise, and for the
//! AI, the best site is used: for a wonder the tile whose loss costs least,
//! for a building the one nearest the centre. Placement happens when the
//! building is completed, and [`settle`] keeps placements true to the map
//! every turn: a building granted for free is placed, one whose city lost
//! it is removed. Rulesets without these uniques (the fixtures) never place
//! anything.

use crate::arena::Id;
use crate::ruleset::{Building, Ruleset};
use crate::world::city::City;
use crate::world::conditionals::{tile_matches_in, Verdict};
use crate::world::grid::{Placed, TileId};
use crate::world::world::World;
use crate::UniqueType;

/// A building's placement rule, if it has one.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Rule<'r> {
    /// Any one of these must hold on the tile.
    pub on: Vec<&'r str>,
    /// Each must hold on some neighbour of the tile.
    pub next_to: Vec<&'r str>,
}

pub fn rule(r: &Ruleset, building: Id<Building>) -> Option<Rule<'_>> {
    let b = r.buildings.get(building)?;
    let mut rule = Rule::default();
    for u in &b.uniques {
        match u.unique_type() {
            Some(UniqueType::StandsOnTile) => rule.on.extend(u.params.first().map(String::as_str)),
            Some(UniqueType::StandsNextToTile) => rule.next_to.extend(u.params.first().map(String::as_str)),
            _ => {}
        }
    }
    (!rule.on.is_empty() || !rule.next_to.is_empty()).then_some(rule)
}

/// Whether this building stands on a tile at all.
pub fn is_placed(r: &Ruleset, building: Id<Building>) -> bool {
    rule(r, building).is_some()
}

/// A world wonder takes its tile; any other building leaves it workable.
pub fn takes_tile(r: &Ruleset, building: Id<Building>) -> bool {
    r.buildings.get(building).is_some_and(|b| b.is_wonder == Some(true))
}

/// A building standing on an isthmus is a canal.
pub fn is_canal(r: &Ruleset, building: Id<Building>) -> bool {
    rule(r, building).is_some_and(|rule| rule.on.iter().any(|f| f.contains("Isthmus")))
}

/// Whether `building` may stand on `tile` of `city`.
pub fn site_allowed(world: &World, r: &Ruleset, city: Id<City>, building: Id<Building>, tile: TileId) -> bool {
    let (Some(c), Some(rule)) = (world.cities.get(city), rule(r, building)) else { return false };
    let grid = &world.grid;
    let i = tile.0 as usize;
    if !grid.contains(tile)
        || tile == c.centre
        || !c.tiles.contains(&tile)
        || grid.owner[i] != Some(c.owner)
        || grid.distance(c.centre, tile) > crate::world::city::WORK_RADIUS
        || grid.natural_wonder[i].is_some()
        || grid.placed_at(tile).is_some_and(|p| p.building != building)
        || (takes_tile(r, building) && grid.is_water(r, tile))
    {
        return false;
    }
    let holds = |t: TileId, filter: &str| tile_matches_in(grid, r, t, filter, Some(c.owner)) == Verdict::True;
    (rule.on.is_empty() || rule.on.iter().any(|f| holds(tile, f)))
        && rule.next_to.iter().all(|f| grid.neighbours(tile).any(|n| holds(n, f)))
}

/// Every tile `building` may stand on in `city`, in tile order.
pub fn valid_sites(world: &World, r: &Ruleset, city: Id<City>, building: Id<Building>) -> Vec<TileId> {
    let Some(c) = world.cities.get(city) else { return Vec::new() };
    let mut sites: Vec<TileId> =
        c.tiles.iter().copied().filter(|t| site_allowed(world, r, city, building, *t)).collect();
    sites.sort();
    sites.dedup();
    sites
}

/// What a tile is worth to the city that would give it up.
fn tile_value(world: &World, r: &Ruleset, tile: TileId) -> i64 {
    use crate::stats::Stat;
    let y = crate::world::yields::tile_yield(&world.grid, r, tile);
    let weight = |s: Stat| match s {
        Stat::Food => 3,
        Stat::Production => 3,
        _ => 2,
    };
    let yields: i64 = Stat::ALL.into_iter().map(|s| (y[s] * crate::stats::Fixed::whole(weight(s))).floor()).sum();
    yields + if world.grid.resource[tile.0 as usize].is_some() { 6 } else { 0 }
}

/// The site the AI takes and a player is offered first: for a wonder the tile
/// whose loss costs least, for a building the one nearest the centre; ties
/// to the lower tile id.
pub fn best_site(world: &World, r: &Ruleset, city: Id<City>, building: Id<Building>) -> Option<TileId> {
    let centre = world.cities.get(city)?.centre;
    let sites = valid_sites(world, r, city, building);
    if takes_tile(r, building) {
        sites.into_iter().min_by_key(|t| (tile_value(world, r, *t), world.grid.distance(centre, *t), *t))
    } else {
        sites.into_iter().min_by_key(|t| (world.grid.distance(centre, *t), *t))
    }
}

/// The player's chosen site while it stays valid, else the best.
pub fn site_for(world: &World, r: &Ruleset, city: Id<City>, building: Id<Building>) -> Option<TileId> {
    let chosen = world.cities.get(city)?.sites.get(&building).copied();
    chosen.filter(|t| site_allowed(world, r, city, building, *t)).or_else(|| best_site(world, r, city, building))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SiteRejection {
    NotPlaced,
    NotAllowed,
    AlreadyBuilt,
}

/// Choose where a building will stand once built.
pub fn choose(
    world: &mut World,
    r: &Ruleset,
    city: Id<City>,
    building: Id<Building>,
    tile: TileId,
) -> Result<(), SiteRejection> {
    if !is_placed(r, building) {
        return Err(SiteRejection::NotPlaced);
    }
    if world.cities.get(city).is_some_and(|c| c.built.contains(&building)) {
        return Err(SiteRejection::AlreadyBuilt);
    }
    if !site_allowed(world, r, city, building, tile) {
        return Err(SiteRejection::NotAllowed);
    }
    world.cities.get_mut(city).expect("validated city").sites.insert(building, tile);
    Ok(())
}

/// Stand `building` on `tile`. A wonder clears the tile and takes it.
pub fn place(world: &mut World, r: &Ruleset, city: Id<City>, building: Id<Building>, tile: TileId) {
    let takes = takes_tile(r, building);
    let i = tile.0 as usize;
    if takes {
        world.grid.improvement[i] = None;
        world.grid.improvement_pillaged[i] = false;
        world.grid.improvement_progress[i] = None;
        world.grid.features[i].retain(|f| {
            r.terrains.name_of(*f).is_none_or(|name| r.improvements.id(&format!("Remove {name}")).is_none())
        });
        for (_, c) in world.cities.iter_mut() {
            c.worked.retain(|t| *t != tile);
        }
    }
    world.grid.placed.insert(tile, Placed { building, takes_tile: takes, canal: is_canal(r, building) });
    if let Some(c) = world.cities.get_mut(city) {
        c.sites.remove(&building);
    }
}

/// Where `building` stands for `city`, if it does.
pub fn standing(world: &World, city: Id<City>, building: Id<Building>) -> Option<TileId> {
    let c = world.cities.get(city)?;
    c.tiles.iter().copied().find(|t| world.grid.placed_at(*t).is_some_and(|p| p.building == building))
}

/// Place every built building of `city` that should stand on a tile and
/// does not yet, where a site exists.
pub fn settle_city(world: &mut World, r: &Ruleset, city: Id<City>) {
    let Some(c) = world.cities.get(city) else { return };
    let pending: Vec<Id<Building>> =
        c.built.iter().copied().filter(|b| is_placed(r, *b) && standing(world, city, *b).is_none()).collect();
    for building in pending {
        if let Some(tile) = site_for(world, r, city, building) {
            place(world, r, city, building, tile);
        }
    }
}

/// Keep placements true to the map: remove any whose tile no longer belongs
/// to a city that has the building, then place what is still unplaced.
pub fn settle(world: &mut World, r: &Ruleset) {
    if world.grid.placed.is_empty() && !r.buildings.ids().any(|b| is_placed(r, b)) {
        return;
    }
    let stale: Vec<TileId> = world
        .grid
        .placed
        .iter()
        .filter(|(tile, p)| !world.cities.iter().any(|(_, c)| c.tiles.contains(tile) && c.built.contains(&p.building)))
        .map(|(tile, _)| *tile)
        .collect();
    for tile in stale {
        world.grid.placed.remove(&tile);
    }
    let cities: Vec<_> = world.cities.iter().map(|(id, _)| id).collect();
    for city in cities {
        settle_city(world, r, city);
    }
}

/// The border tiles a wall-building wonder runs along: land tiles of its
/// city that touch a tile its owner does not hold. A wonder is a wall when
/// its effect makes enemies pay extra movement in your territory (the Great
/// Wall); its segments are for drawing, the effect is the unique's own.
pub fn wall_segments(world: &World, r: &Ruleset, city: Id<City>) -> Vec<TileId> {
    let Some(c) = world.cities.get(city) else { return Vec::new() };
    let walls = c.built.iter().any(|b| {
        is_placed(r, *b)
            && r.buildings[*b].uniques.iter().any(|u| u.unique_type() == Some(UniqueType::EnemyUnitsSpendExtraMovement))
    });
    if !walls {
        return Vec::new();
    }
    let grid = &world.grid;
    let mut out: Vec<TileId> = c
        .tiles
        .iter()
        .copied()
        .filter(|t| {
            grid.owner[t.0 as usize] == Some(c.owner)
                && !grid.is_water(r, *t)
                && grid.neighbours(*t).any(|n| grid.owner[n.0 as usize] != Some(c.owner) && !grid.is_water(r, n))
        })
        .collect();
    out.sort();
    out
}
