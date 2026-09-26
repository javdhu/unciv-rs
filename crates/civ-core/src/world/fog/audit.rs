//! The shadow check for live sight: the plain recomputation that [`update_visibility_in`] and
//! [`capture_observations`] replace, run beside them on a copy of the world, which must come out the same.
//!
//! Live sight is cached and written sparingly — line of sight is looked up in a cache keyed by the terrain around
//! it, masks and records are written only where they change, and stores are shared with staged worlds until then.
//! Each shortcut is exact by construction; this proves it over whole games. Switched on per thread, so a test can
//! audit every step of a game while other tests on other threads run unaudited.
//!
//! [`update_visibility_in`]: crate::world::turn::update_visibility_in
//! [`capture_observations`]: super::capture_observations

use super::{sight_heights, sightline, CityObservation, CivObservation, TileMemory, TileObservation, Visibility};
use crate::arena::Id;
use crate::ruleset::Ruleset;
use crate::shared::Shared;
use crate::world::entities::Unit;
use crate::world::grid::{RoadLevel, TileId};
use crate::world::world::World;
use std::cell::Cell;

thread_local! {
    static ON: Cell<bool> = const { Cell::new(false) };
    static STEPS: Cell<u64> = const { Cell::new(0) };
}

/// Audit every sight update and capture on this thread from now on, or stop.
pub fn enable(on: bool) {
    ON.with(|c| c.set(on));
}

/// How many updates and captures this thread has audited.
pub fn steps() -> u64 {
    STEPS.with(Cell::get)
}

/// When auditing, what `reference` makes of a copy of the world as it stands.
pub(crate) fn expect(world: &World, r: &Ruleset, reference: fn(&mut World, &Ruleset)) -> Option<Shared<Visibility>> {
    ON.with(Cell::get).then(|| {
        let mut copy = world.clone();
        reference(&mut copy, r);
        copy.visibility
    })
}

/// Panic unless the world's visibility is what the reference made.
pub(crate) fn check(expected: Option<Shared<Visibility>>, world: &World, what: &str) {
    if let Some(expected) = expected {
        STEPS.with(|c| c.set(c.get() + 1));
        assert!(*expected == *world.visibility, "{what} disagrees with the plain recomputation on turn {}", world.turn);
    }
}

/// Panic unless a cached line of sight is the one worked out afresh.
pub(crate) fn check_sightline(cached: &[TileId], fresh: impl FnOnce() -> Vec<TileId>) {
    if ON.with(Cell::get) {
        assert_eq!(cached, fresh(), "a cached line of sight disagrees with the one worked out afresh");
    }
}

/// One unit's sight, each rule looked for afresh and line of sight worked out uncached.
fn unit_visible_tiles(world: &World, r: &Ruleset, id: Id<Unit>) -> Vec<TileId> {
    use crate::world::conditionals::{applies, GameContext};
    use crate::UniqueType;
    let Some(unit) = world.units.get(id) else { return Vec::new() };
    let ctx = GameContext::new(r).with_world(world).for_civ(unit.owner).for_unit(id).on_tile(unit.position);
    if unit.all_uniques(r).any(|u| u.unique_type() == Some(UniqueType::NoSight) && applies(u, &ctx).holds()) {
        return vec![unit.position];
    }
    let sight: i32 = unit
        .all_uniques(r)
        .chain(world.civ(unit.owner).into_iter().flat_map(|c| c.uniques.all()))
        .chain(world.grid.terrains(unit.position).filter_map(|id| r.terrains.get(id)).flat_map(|t| &t.uniques))
        .filter(|u| u.unique_type() == Some(UniqueType::Sight) && applies(u, &ctx).holds())
        .filter_map(|u| u.params.first()?.parse::<i32>().ok())
        .sum();
    let range = (2 + sight).max(1) as u32;
    if unit.all_uniques(r).any(|u| u.unique_type() == Some(UniqueType::CanSeeOverObstacles) && applies(u, &ctx).holds())
    {
        world.grid.within(unit.position, range)
    } else {
        let grid = &world.grid;
        sightline(grid, unit.position, range, false, |tile| sight_heights(grid, r, tile))
    }
}

/// Every civ's live sight cleared and observed again from every source.
pub(crate) fn update_visibility(world: &mut World, r: &Ruleset) {
    let seen: Vec<_> = world.units.iter().map(|(id, u)| (u.owner, unit_visible_tiles(world, r, id))).collect();
    for civ in world.civ_ids().collect::<Vec<_>>() {
        world.visibility.clear_visible(civ);
    }
    for (civ, tiles) in seen {
        for tile in tiles {
            world.visibility.observe(civ, tile);
        }
    }
    let spied: Vec<_> = world
        .civs
        .iter()
        .filter(|c| c.is_alive())
        .flat_map(|c| {
            c.espionage
                .spies
                .iter()
                .filter(|s| s.is_established())
                .filter_map(|s| world.cities.get(s.station?).map(|city| (c.id, city.centre)))
        })
        .collect();
    for (civ, centre) in spied {
        for tile in world.grid.within(centre, 1) {
            world.visibility.observe(civ, tile);
        }
    }
    for tile in world.grid.ids() {
        if let Some(owner) = world.grid.owner[tile.0 as usize] {
            world.visibility.observe(owner, tile);
            for next in world.grid.neighbours(tile) {
                world.visibility.observe(owner, next);
            }
        }
    }
    for (_, city) in world.cities.iter() {
        let ally = if world.civ(city.owner).is_some_and(|c| c.is_city_state()) {
            crate::world::city_states::ally_of(world, city.owner)
        } else {
            None
        };
        if let Some(ally) = ally {
            for tile in &city.tiles {
                world.visibility.observe(ally, *tile);
            }
        }
    }
}

/// Every record written afresh, tile by tile, whether or not it changed.
pub(crate) fn capture_observations(world: &mut World, r: &Ruleset) {
    let viewers: Vec<_> = world.civ_ids().collect();
    let mut centres: Vec<TileId> = world.cities.iter().map(|(_, c)| c.centre).collect();
    centres.sort_unstable();
    for viewer in viewers {
        for tile in world.grid.ids() {
            let visible = world.visibility.can_see(viewer, tile);
            let first_map = world.visibility.has_explored(viewer, tile)
                && world.visibility.tile_observations.get(&(viewer, tile)).is_none();
            if !visible && !first_map {
                continue;
            }
            let i = tile.0 as usize;
            let prior = world.visibility.recalls(viewer, tile);
            let placed = if visible {
                world.grid.placed.get(&tile).copied()
            } else {
                world.visibility.tile_observations.get(&(viewer, tile)).and_then(|o| o.placed)
            };
            let seen = TileObservation {
                placed,
                terrain: world.grid.terrain[i],
                features: world.grid.features[i].clone(),
                natural_wonder: world.grid.natural_wonder[i],
                resource: world.grid.resource[i],
                improvement: if visible { world.grid.improvement[i] } else { prior.and_then(|m| m.improvement) },
                improvement_pillaged: visible && world.grid.improvement_pillaged[i],
                road: if visible { world.grid.road[i] } else { RoadLevel::None },
                road_pillaged: visible && world.grid.road_pillaged[i],
                river_edges: world.grid.river_edges[i],
                owner: if visible { world.grid.owner[i] } else { None },
                observed_turn: world.turn,
            };
            world.visibility.tile_observations.insert((viewer, tile), seen);
            if visible {
                world.visibility.remember(
                    viewer,
                    tile,
                    TileMemory {
                        improvement: world.grid.improvement[i],
                        resource: world.grid.resource[i],
                        owner: world.grid.owner[i],
                    },
                );
            }
        }
        let visible_tiles = world.visibility.visible_tiles(viewer);
        let gone: Vec<_> = world
            .visibility
            .city_observations
            .range((viewer, TileId(0))..=(viewer, TileId(u16::MAX)))
            .map(|(key, _)| *key)
            .filter(|(_, tile)| visible_tiles.binary_search(tile).is_ok() && centres.binary_search(tile).is_err())
            .collect();
        for key in gone {
            world.visibility.city_observations.remove(&key);
        }
        for (id, city) in world.cities.iter() {
            if city.owner != viewer && !world.visibility.can_see(viewer, city.centre) {
                continue;
            }
            let mut tiles = world
                .visibility
                .city_observations
                .get(&(viewer, city.centre))
                .map(|c| c.tiles.clone())
                .unwrap_or_default();
            tiles.retain(|t| !world.visibility.can_see(viewer, *t) || city.tiles.contains(t));
            tiles.extend(
                city.tiles.iter().copied().filter(|t| city.owner == viewer || world.visibility.can_see(viewer, *t)),
            );
            tiles.sort();
            tiles.dedup();
            world.visibility.city_observations.insert(
                (viewer, city.centre),
                CityObservation {
                    id,
                    name: city.name.clone(),
                    owner: city.owner,
                    centre: city.centre,
                    population: city.population,
                    tiles,
                    observed_turn: world.turn,
                },
            );
        }
        for civ in &world.civs {
            let observed = civ.id == viewer
                || visible_tiles.iter().any(|t| world.grid.owner[t.0 as usize] == Some(civ.id))
                || world.cities.iter().any(|(_, c)| c.owner == civ.id && world.visibility.can_see(viewer, c.centre))
                || world.units.iter().any(|(id, u)| u.owner == civ.id && super::can_see_unit(world, r, viewer, id));
            let first_contact = world.diplomacy.have_met(viewer, civ.id)
                && !world.visibility.civ_observations.contains_key(&(viewer, civ.id));
            if observed || first_contact {
                world.visibility.civ_observations.insert(
                    (viewer, civ.id),
                    CivObservation {
                        id: civ.id,
                        nation: civ.nation,
                        city_state: civ.is_city_state(),
                        barbarian: civ.is_barbarian,
                        alive: if observed { civ.is_alive() } else { true },
                        observed_turn: world.turn,
                    },
                );
            }
        }
    }
}
