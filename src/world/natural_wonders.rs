//! Natural-wonder placement and permanent discovery.
//!
//! Placement follows `NaturalWonderGenerator`: weighted selection without
//! replacement, scarce sites first, five tiles clear of starts, and separate
//! wonders more than map-height / five apart. Terrain requirements, groups,
//! conversion and discovery rewards come from the ruleset. The random stream
//! and the rectangular map's equivalent hex radius use integers only.

use std::collections::{BTreeSet, VecDeque};

use crate::arena::Id;
use crate::rng::Rng;
use crate::ruleset::{Ruleset, Terrain};
use crate::stats::{Fixed, Stats};
use crate::world::conditionals::{applies, tile_matches_in, GameContext, Verdict};
use crate::world::grid::{CivId, TileGrid, TileId};
use crate::world::world::World;
use crate::{Unique, UniqueType};

const START_CLEARANCE: u32 = 5;

/// Default `ModConstants` count: round(radius * 0.124 + 0.1).
/// The equivalent radius solves area = 3*r*(r+1)+1. Testing the half-radius
/// boundary rounds that radius without taking a floating-point square root.
fn target_count(grid: &TileGrid) -> usize {
    let mut radius = 0usize;
    while 4 * grid.len() >= 12 * radius * radius + 24 * radius + 13 {
        radius += 1;
    }
    (radius * 124 + 600) / 1000
}

fn int_param(u: &Unique, n: usize) -> Option<i32> {
    u.params.get(n)?.parse().ok()
}

fn matches_wonder_filter(grid: &TileGrid, r: &Ruleset, tile: TileId, filter: &str) -> bool {
    let i = tile.0 as usize;
    let base = r.terrains.name_of(grid.terrain[i]);
    let top = grid.features[i].last().copied().unwrap_or(grid.terrain[i]);
    let hill = base == Some("Hill") || grid.features[i].iter().any(|t| r.terrains.name_of(*t) == Some("Hill"));
    match filter {
        "Elevated" => base == Some("Mountain") || hill,
        "Hill" => hill,
        "Water" => grid.is_water(r, tile),
        "Land" => !grid.is_water(r, tile),
        _ => {
            base == Some(filter)
                || r.terrains.name_of(top) == Some(filter)
                || grid.natural_wonder[i].and_then(|t| r.terrains.name_of(t)) == Some(filter)
        }
    }
}

/// Landmass rank per tile; zero is largest, water has no rank. Ties use the
/// first tile's id, so equal-sized islands are ordered reproducibly.
fn landmass_ranks(grid: &TileGrid, r: &Ruleset) -> Vec<Option<usize>> {
    let mut labels = vec![None; grid.len()];
    let mut sizes = Vec::new();
    for start in grid.ids() {
        if labels[start.0 as usize].is_some() || grid.is_water(r, start) {
            continue;
        }
        let id = sizes.len();
        let mut count = 0usize;
        let mut queue = VecDeque::from([start]);
        labels[start.0 as usize] = Some(id);
        while let Some(tile) = queue.pop_front() {
            count += 1;
            for n in grid.neighbours(tile) {
                if labels[n.0 as usize].is_none() && !grid.is_water(r, n) {
                    labels[n.0 as usize] = Some(id);
                    queue.push_back(n);
                }
            }
        }
        sizes.push((id, count));
    }
    sizes.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut ranks = vec![0; sizes.len()];
    for (rank, (id, _)) in sizes.into_iter().enumerate() {
        ranks[id] = rank;
    }
    labels.into_iter().map(|label| label.map(|id| ranks[id])).collect()
}

fn fits(grid: &TileGrid, r: &Ruleset, tile: TileId, wonder: &Terrain, ranks: &[Option<usize>]) -> bool {
    wonder.uniques.iter().all(|u| match u.unique_type() {
        Some(UniqueType::NaturalWonderNeighborCount) => {
            let (Some(count), Some(filter)) = (int_param(u, 0), u.params.get(1)) else { return false };
            grid.neighbours(tile).filter(|n| matches_wonder_filter(grid, r, *n, filter)).count() as i32 == count
        }
        Some(UniqueType::NaturalWonderNeighborsRange) => {
            let (Some(low), Some(high), Some(filter)) = (int_param(u, 0), int_param(u, 1), u.params.get(2)) else {
                return false;
            };
            let count = grid.neighbours(tile).filter(|n| matches_wonder_filter(grid, r, *n, filter)).count() as i32;
            (low..=high).contains(&count)
        }
        Some(UniqueType::NaturalWonderLargerLandmass) => {
            int_param(u, 0).is_some_and(|count| ranks[tile.0 as usize].is_some_and(|rank| (rank as i32) < count))
        }
        Some(UniqueType::NaturalWonderSmallerLandmass) => {
            int_param(u, 0).is_some_and(|count| ranks[tile.0 as usize].is_none_or(|rank| (rank as i32) >= count))
        }
        Some(UniqueType::NaturalWonderLatitude) => {
            let (Some(low), Some(high)) = (int_param(u, 0), int_param(u, 1)) else { return false };
            let pole = grid.height().saturating_sub(1) as i32;
            let latitude = (2 * grid.coords(tile).1 as i32 - pole).abs();
            latitude * 100 >= pole * low && latitude * 100 <= pole * high
        }
        _ => true,
    })
}

fn candidates(r: &Ruleset, grid: &TileGrid, wonder: Id<Terrain>, starts: &[TileId]) -> Vec<TileId> {
    let Some(wonder) = r.terrains.get(wonder) else { return Vec::new() };
    let ranks = landmass_ranks(grid, r);
    let occupied: Vec<_> = grid.ids().filter(|t| grid.natural_wonder[t.0 as usize].is_some()).collect();
    let spacing = grid.height() as u32 / 5;
    grid.ids()
        .filter(|tile| {
            let i = tile.0 as usize;
            let top = grid.features[i].last().copied().unwrap_or(grid.terrain[i]);
            grid.resource[i].is_none()
                && grid.natural_wonder[i].is_none()
                && grid.improvement[i].is_none()
                && grid.owner[i].is_none()
                && starts.iter().all(|s| grid.distance(*s, *tile) > START_CLEARANCE)
                && occupied.iter().all(|other| grid.distance(*other, *tile) > spacing)
                && r.terrains.name_of(top).is_some_and(|name| wonder.occurs_on.iter().any(|n| n == name))
                && fits(grid, r, *tile, wonder, &ranks)
        })
        .collect()
}

/// Place each selected wonder at most once; a group such as the reef is one
/// wonder on several connected tiles. Impossible candidates are skipped.
pub fn place(r: &Ruleset, grid: &mut TileGrid, seed: u64, starts: &[TileId]) {
    let existing: BTreeSet<_> = grid.natural_wonder.iter().flatten().copied().collect();
    let target = target_count(grid).saturating_sub(existing.len());
    let mut remaining: Vec<_> = r
        .terrains
        .iter()
        .filter(|(id, t)| t.r#type == "NaturalWonder" && t.weight.unwrap_or(10) > 0 && !existing.contains(id))
        .map(|(id, _)| id)
        .collect();
    let mut rng = Rng::from_parts(&[seed, 0x4E41_5457]);
    let mut chosen = Vec::new();
    while chosen.len() < target && !remaining.is_empty() {
        let weight = |id| r.terrains.get(id).unwrap().weight.unwrap_or(10) as i64;
        let total: i64 = remaining.iter().map(|id| weight(*id)).sum();
        let mut roll = rng.range(1, total);
        let index = remaining
            .iter()
            .position(|id| {
                roll -= weight(*id);
                roll <= 0
            })
            .unwrap();
        chosen.push(remaining.remove(index));
    }
    let mut spawned = 0usize;
    for batch in [&mut chosen, &mut remaining] {
        batch.sort_by_key(|id| (candidates(r, grid, *id, starts).len(), *id));
        for &wonder in batch.iter() {
            if spawned == target {
                return;
            }
            if try_place(r, grid, wonder, starts, &mut rng) {
                spawned += 1;
            }
        }
    }
}

fn try_place(r: &Ruleset, grid: &mut TileGrid, wonder: Id<Terrain>, starts: &[TileId], rng: &mut Rng) -> bool {
    let definition = r.terrains.get(wonder).unwrap();
    let (min, max) = match definition.uniques.iter().find(|u| u.unique_type() == Some(UniqueType::NaturalWonderGroups))
    {
        Some(u) => match (int_param(u, 0), int_param(u, 1)) {
            (Some(min), Some(max)) if min > 0 && max >= min => (min as usize, max as usize),
            _ => return false,
        },
        None => (1, 1),
    };
    let mut sites = candidates(r, grid, wonder, starts);
    if sites.len() < min {
        return false;
    }
    let wanted = rng.range(min as i64, max.min(sites.len()) as i64) as usize;
    // Retry another seed site when an isolated site cannot form the required
    // group. Every attempt uses the same immutable candidate map.
    while !sites.is_empty() {
        let first = sites.remove(rng.range(0, sites.len() as i64 - 1) as usize);
        let mut group = vec![first];
        while group.len() < wanted {
            let next: Vec<_> = sites
                .iter()
                .copied()
                .filter(|n| !group.contains(n) && group.iter().any(|t| grid.neighbours(*t).any(|adj| adj == *n)))
                .collect();
            if next.is_empty() {
                break;
            }
            group.push(next[rng.range(0, next.len() as i64 - 1) as usize]);
        }
        if group.len() >= min {
            for tile in group {
                convert_wonder(r, grid, tile, wonder);
            }
            return true;
        }
    }
    false
}

fn clear_tile(grid: &mut TileGrid, tile: TileId) {
    let i = tile.0 as usize;
    grid.features[i].clear();
    grid.resource[i] = None;
    grid.resource_amount[i] = 0;
    grid.set_improvement(tile, None);
    grid.improvement_progress[i] = None;
}

fn conversion_applies(grid: &TileGrid, r: &Ruleset, tile: TileId, u: &Unique) -> bool {
    u.conditionals.iter().all(|c| {
        let Some(filter) = c.params.first() else { return false };
        let matched = tile_matches_in(grid, r, tile, filter, None);
        match c.unique_type() {
            Some(UniqueType::ConditionalInTiles) => matched == Verdict::True,
            Some(UniqueType::ConditionalInTilesNot) => matched == Verdict::False,
            _ => false, // An unsupported condition must not convert terrain.
        }
    })
}

fn has_unique(t: &Terrain, kind: UniqueType) -> bool {
    t.uniques.iter().any(|u| u.unique_type() == Some(kind))
}

fn convert_wonder(r: &Ruleset, grid: &mut TileGrid, tile: TileId, wonder: Id<Terrain>) {
    let definition = r.terrains.get(wonder).unwrap();
    let base = definition.turns_into.as_deref().and_then(|name| r.terrain(name));
    let preserved: Vec<_> = grid.features[tile.0 as usize]
        .iter()
        .copied()
        .filter(|id| {
            base.is_none() && r.terrains.name_of(*id).is_some_and(|name| definition.occurs_on.iter().any(|n| n == name))
        })
        .collect();
    clear_tile(grid, tile);
    grid.features[tile.0 as usize] = preserved;
    if let Some(base) = base {
        grid.terrain[tile.0 as usize] = base;
    }
    grid.natural_wonder[tile.0 as usize] = Some(wonder);
    for neighbour in grid.neighbours(tile).collect::<Vec<_>>() {
        if grid.natural_wonder[neighbour.0 as usize].is_some() {
            continue;
        }
        for u in
            definition.uniques.iter().filter(|u| u.unique_type() == Some(UniqueType::NaturalWonderConvertNeighbors))
        {
            if !conversion_applies(grid, r, neighbour, u) {
                continue;
            }
            let Some(to) = u.params.first().and_then(|name| r.terrain(name)) else { continue };
            if grid.terrains(neighbour).any(|id| id == to) {
                continue;
            }
            let terrain = r.terrains.get(to).unwrap();
            if has_unique(terrain, UniqueType::FreshWater)
                && grid.neighbours(neighbour).any(|n| {
                    r.terrains.get(grid.terrain[n.0 as usize]).is_some_and(|t| has_unique(t, UniqueType::CoastalWater))
                })
            {
                continue;
            }
            match terrain.r#type.as_str() {
                "Land" | "Water" => {
                    if has_unique(terrain, UniqueType::CoastalWater) {
                        convert_adjacent_lakes(r, grid, neighbour, to);
                    }
                    clear_tile(grid, neighbour);
                    grid.terrain[neighbour.0 as usize] = to;
                }
                "TerrainFeature"
                    if r.terrains
                        .name_of(grid.terrain[neighbour.0 as usize])
                        .is_some_and(|name| terrain.occurs_on.iter().any(|n| n == name)) =>
                {
                    let features = grid.features[neighbour.0 as usize].clone();
                    clear_tile(grid, neighbour);
                    grid.features[neighbour.0 as usize] = features;
                    grid.features[neighbour.0 as usize].push(to);
                }
                _ => {}
            }
        }
    }
}

fn convert_adjacent_lakes(r: &Ruleset, grid: &mut TileGrid, tile: TileId, coast: Id<Terrain>) {
    let mut future_coast = BTreeSet::from([tile]);
    let mut queue = VecDeque::from([tile]);
    while let Some(next) = queue.pop_front() {
        for n in grid.neighbours(next) {
            if grid.natural_wonder[n.0 as usize].is_none()
                && r.terrains.get(grid.terrain[n.0 as usize]).is_some_and(|t| has_unique(t, UniqueType::FreshWater))
                && future_coast.insert(n)
            {
                queue.push_back(n);
            }
        }
    }
    for next in future_coast {
        if next != tile {
            clear_tile(grid, next);
            grid.terrain[next.0 as usize] = coast;
        }
        for n in grid.neighbours(next).collect::<Vec<_>>() {
            grid.clear_river_between(next, n);
        }
    }
}

/// A newly discovered wonder; grouped tiles produce just one event per civ.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Discovery {
    pub civ: CivId,
    pub wonder: Id<Terrain>,
    pub tile: TileId,
    pub first: bool,
    pub stats: Stats,
}

/// Wonder tiles in tile-id order. A caller processing several movement steps
/// can reuse this list while the map's natural-wonder sites remain unchanged.
pub fn sites(grid: &TileGrid) -> Vec<(TileId, Id<Terrain>)> {
    grid.ids().filter_map(|t| grid.natural_wonder[t.0 as usize].map(|w| (t, w))).collect()
}

/// Register explored wonders and grant their one-time bonuses. Exploration is
/// used so a reveal from ruins, or sight gained while passing a tile, is not
/// lost when the live visibility mask is rebuilt at turn end. Simultaneous
/// discoveries break ties by civ order, then tile id.
pub fn discover_visible(world: &mut World, r: &Ruleset) -> Vec<Discovery> {
    discover_at(world, r, &sites(&world.grid))
}

/// Discovery against a current, ordered list returned by [`sites`]. Keeping
/// that list outside this function avoids rescanning the grid at every step.
pub fn discover_at(world: &mut World, r: &Ruleset, wonders: &[(TileId, Id<Terrain>)]) -> Vec<Discovery> {
    let mut events = Vec::new();
    if wonders.is_empty() {
        return events;
    }
    for civ in world.civ_ids().collect::<Vec<_>>() {
        if world.civ(civ).is_none_or(|c| c.is_barbarian || c.defeated) {
            continue;
        }
        for &(tile, wonder) in wonders {
            if !world.visibility.has_explored(civ, tile)
                || world.civ(civ).unwrap().known_natural_wonders.contains(&wonder)
            {
                continue;
            }
            let first = !world.civ_ids().filter(|other| *other != civ).any(|other| {
                world.civ(other).is_some_and(|c| c.is_major() && c.known_natural_wonders.contains(&wonder))
            });
            let ctx = GameContext::new(r).with_world(world).for_civ(civ).on_tile(tile);
            let mut stats = Stats::ZERO;
            if first {
                for u in world.grid.terrains(tile).filter_map(|id| r.terrains.get(id)).flat_map(|t| &t.uniques) {
                    if u.unique_type() == Some(UniqueType::GrantsStatsToFirstToDiscover)
                        && applies(u, &ctx) == Verdict::True
                    {
                        stats += u.params.first().and_then(|p| Stats::parse(p)).unwrap_or_default();
                    }
                }
            }
            for u in world.civ(civ).unwrap().uniques.matching(UniqueType::StatBonusWhenDiscoveringNaturalWonder) {
                if applies(u, &ctx) == Verdict::True {
                    stats += u.params.get(usize::from(first)).and_then(|p| Stats::parse(p)).unwrap_or_default();
                }
            }
            let c = world.civ_mut(civ).unwrap();
            c.known_natural_wonders.insert(wonder);
            // Civilization.addStats truncates and only accepts empire reserves.
            c.gold = c.gold.saturating_add((stats.gold.raw() / 1000) as i32);
            c.science_stored = c.science_stored.saturating_add((stats.science.raw() / 1000) as i32);
            c.culture_stored = c.culture_stored.saturating_add((stats.culture.raw() / 1000) as i32);
            c.faith.stored = c.faith.stored.saturating_add((stats.faith.raw() / 1000) as i32);
            events.push(Discovery { civ, tile, wonder, first, stats });
        }
    }
    events
}

/// Permanent per-turn contribution, including the default +1 Happiness and
/// `StatsFromNaturalWonders` (Spain adds another +1 Happiness).
pub fn known_stats(world: &World, r: &Ruleset, civ: CivId) -> Stats {
    let Some(c) = world.civ(civ) else { return Stats::ZERO };
    if c.known_natural_wonders.is_empty() {
        return Stats::ZERO;
    }
    let mut per_wonder = Stats { happiness: Fixed::ONE, ..Stats::ZERO };
    let ctx = GameContext::new(r).with_world(world).for_civ(civ);
    for u in c.uniques.matching(UniqueType::StatsFromNaturalWonders) {
        if applies(u, &ctx) == Verdict::True {
            per_wonder += u.params.first().and_then(|p| Stats::parse(p)).unwrap_or_default();
        }
    }
    let count = Fixed::whole(c.known_natural_wonders.len() as i32);
    for stat in crate::stats::Stat::ALL {
        per_wonder[stat] = per_wonder[stat] * count;
    }
    per_wonder
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::entities::{Civilization, Unit};
    use crate::world::happiness::happiness;
    use crate::world::mapgen::{generate, MapParameters};

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("rules/gods-and-kings").unwrap().unwrap()
    }

    fn world(r: &Ruleset, nations: &[&str]) -> World {
        let civs = nations
            .iter()
            .enumerate()
            .map(|(i, nation)| Civilization::new(CivId(i as u8), r.nations.id(nation).unwrap(), false))
            .collect();
        let mut w = World::new(TileGrid::new(12, 10, r.terrain("Grassland").unwrap()), civs);
        for civ in w.civ_ids().collect::<Vec<_>>() {
            crate::world::policies::rebuild_uniques(&mut w, r, civ);
        }
        w
    }

    #[test]
    fn count_uses_equivalent_hex_radius_and_default_constants() {
        let r = ruleset();
        let land = r.terrain("Grassland").unwrap();
        assert_eq!(target_count(&TileGrid::new(1, 1, land)), 0);
        assert_eq!(target_count(&TileGrid::new(23, 15, land)), 1);
        assert_eq!(target_count(&TileGrid::new(44, 29, land)), 3);
        assert_eq!(target_count(&TileGrid::new(87, 57, land)), 5);
    }

    #[test]
    fn generated_wonders_are_deterministic_separate_and_clear_of_starts() {
        let r = ruleset();
        let mut found = 0;
        for seed in 0..8 {
            let p = MapParameters { seed, ..Default::default() };
            let m = generate(&r, p, 4);
            assert_eq!(m, generate(&r, p, 4));
            let wonders: Vec<_> =
                m.grid.ids().filter_map(|t| m.grid.natural_wonder[t.0 as usize].map(|w| (t, w))).collect();
            let kinds: BTreeSet<_> = wonders.iter().map(|(_, w)| *w).collect();
            assert!(kinds.len() <= target_count(&m.grid));
            found += kinds.len();
            for (tile, wonder) in &wonders {
                assert_eq!(r.terrains.get(*wonder).unwrap().r#type, "NaturalWonder");
                assert!(m.starts.iter().all(|s| m.grid.distance(*s, *tile) > START_CLEARANCE));
                assert!(m.grid.resource[tile.0 as usize].is_none());
                assert!(m.grid.improvement[tile.0 as usize].is_none(), "ruins and camps must skip wonders");
                assert!(!crate::world::movement::is_passable(&m.grid, &r, *tile));
                for (other_tile, other_wonder) in &wonders {
                    if wonder != other_wonder {
                        assert!(m.grid.distance(*tile, *other_tile) > m.grid.height() as u32 / 5);
                    }
                }
            }
            for wonder in kinds {
                let group: Vec<_> = wonders.iter().filter(|(_, w)| *w == wonder).map(|(t, _)| *t).collect();
                let definition = r.terrains.get(wonder).unwrap();
                if let Some(u) =
                    definition.uniques.iter().find(|u| u.unique_type() == Some(UniqueType::NaturalWonderGroups))
                {
                    assert!((int_param(u, 0).unwrap()..=int_param(u, 1).unwrap()).contains(&(group.len() as i32)));
                    assert!(group
                        .iter()
                        .all(|t| group.len() == 1 || group.iter().any(|n| m.grid.distance(*t, *n) == 1)));
                } else {
                    assert_eq!(group.len(), 1);
                }
            }
        }
        assert!(found >= 8, "the generation path should place wonders, found {found}");
    }

    #[test]
    fn placement_honors_last_terrain_resources_and_exact_neighbour_rules() {
        let r = ruleset();
        let mut g = TileGrid::new(12, 10, r.terrain("Plains").unwrap());
        let centre = g.id_at(6, 5).unwrap();
        let cerro = r.terrain("Cerro de Potosi").unwrap();
        assert!(!candidates(&r, &g, cerro, &[]).contains(&centre), "Cerro needs an adjacent hill");
        let neighbour = g.neighbours(centre).next().unwrap();
        g.features[neighbour.0 as usize].push(r.terrain("Hill").unwrap());
        assert!(candidates(&r, &g, cerro, &[]).contains(&centre));
        g.features[centre.0 as usize].push(r.terrain("Forest").unwrap());
        assert!(!candidates(&r, &g, cerro, &[]).contains(&centre), "occursOn matches the last terrain");
        g.features[centre.0 as usize].clear();
        g.resource[centre.0 as usize] = r.resources.ids().next();
        assert!(!candidates(&r, &g, cerro, &[]).contains(&centre));
        g.resource[centre.0 as usize] = None;
        g.terrain[neighbour.0 as usize] = r.terrain("Coast").unwrap();
        assert!(!candidates(&r, &g, cerro, &[]).contains(&centre), "Cerro forbids adjacent coast");
    }

    #[test]
    fn landmass_and_latitude_requirements_reject_unsuitable_sites() {
        let r = ruleset();
        let mut g = TileGrid::new(12, 10, r.terrain("Ocean").unwrap());
        for row in 2..8 {
            for col in 1..5 {
                let t = g.id_at(col, row).unwrap();
                g.terrain[t.0 as usize] = r.terrain("Grassland").unwrap();
            }
        }
        let island = g.id_at(9, 5).unwrap();
        g.terrain[island.0 as usize] = r.terrain("Grassland").unwrap();
        let ranks = landmass_ranks(&g, &r);
        let sri = r.terrains.by_name("Sri Pada").unwrap();
        assert!(fits(&g, &r, island, sri, &ranks));
        assert!(!fits(&g, &r, g.id_at(3, 5).unwrap(), sri, &ranks));
        let latitude = Terrain {
            uniques: vec![Unique::parse("Occurs on latitudes from [10] to [70] percent of distance equator to pole")],
            ..Terrain::default()
        };
        assert!(!fits(&g, &r, g.id_at(6, 0).unwrap(), &latitude, &ranks));
        assert!(fits(&g, &r, g.id_at(6, 3).unwrap(), &latitude, &ranks));
    }

    #[test]
    fn reef_requires_two_adjacent_eligible_tiles() {
        let r = ruleset();
        let mut g = TileGrid::new(12, 10, r.terrain("Ocean").unwrap());
        let coast = g.id_at(6, 5).unwrap();
        g.terrain[coast.0 as usize] = r.terrain("Coast").unwrap();
        let reef = r.terrain("Great Barrier Reef").unwrap();
        assert!(try_place(&r, &mut g, reef, &[], &mut Rng::seeded(7)));
        let group: Vec<_> = g.ids().filter(|t| g.natural_wonder[t.0 as usize] == Some(reef)).collect();
        assert_eq!(group.len(), 2);
        assert_eq!(g.distance(group[0], group[1]), 1);
        assert!(group.iter().all(|t| g.terrain[t.0 as usize] == r.terrain("Coast").unwrap()));

        let mut isolated = TileGrid::new(12, 10, r.terrain("Coast").unwrap());
        isolated.terrain[coast.0 as usize] = r.terrain("Ocean").unwrap();
        assert!(!try_place(&r, &mut isolated, reef, &[], &mut Rng::seeded(7)));
        assert!(isolated.natural_wonder.iter().all(Option::is_none));
    }

    #[test]
    fn gibraltar_converts_neighbours_but_keeps_its_mountain() {
        let r = ruleset();
        let mut g = TileGrid::new(12, 10, r.terrain("Grassland").unwrap());
        let centre = g.id_at(6, 5).unwrap();
        let neighbours: Vec<_> = g.neighbours(centre).collect();
        let mountain = neighbours[0];
        g.terrain[mountain.0 as usize] = r.terrain("Mountain").unwrap();
        for n in &neighbours[1..] {
            g.features[n.0 as usize].push(r.terrain("Forest").unwrap());
            g.resource[n.0 as usize] = r.resources.ids().next();
            g.resource_amount[n.0 as usize] = 4;
            g.set_river_between(centre, *n);
        }
        convert_wonder(&r, &mut g, centre, r.terrain("Rock of Gibraltar").unwrap());
        assert_eq!(g.terrain[mountain.0 as usize], r.terrain("Mountain").unwrap());
        for n in &neighbours[1..] {
            assert_eq!(g.terrain[n.0 as usize], r.terrain("Coast").unwrap());
            assert!(g.features[n.0 as usize].is_empty());
            assert!(g.resource[n.0 as usize].is_none());
            assert_eq!(g.resource_amount[n.0 as usize], 0);
            assert!(!g.has_river_between(centre, *n));
        }
    }

    #[test]
    fn discovery_is_once_per_civ_even_for_a_group_and_survives_lost_sight() {
        let r = ruleset();
        let mut w = world(&r, &["Rome", "Spain"]);
        let reef = r.terrain("Great Barrier Reef").unwrap();
        for t in [TileId(21), TileId(22)] {
            w.grid.natural_wonder[t.0 as usize] = Some(reef);
        }
        let before = happiness(&w, &r, CivId(0), None);
        assert!(discover_visible(&mut w, &r).is_empty());
        w.visibility.observe(CivId(0), TileId(21));
        w.visibility.observe(CivId(0), TileId(22));
        assert_eq!(discover_visible(&mut w, &r).len(), 1);
        assert_eq!(happiness(&w, &r, CivId(0), None), before + 1);
        assert_eq!(w.civ(CivId(1)).unwrap().known_natural_wonders.len(), 0);
        w.visibility.clear_visible(CivId(0));
        assert!(discover_visible(&mut w, &r).is_empty());
        assert_eq!(happiness(&w, &r, CivId(0), None), before + 1);
        w.visibility.observe(CivId(1), TileId(22));
        let late = discover_visible(&mut w, &r);
        assert_eq!(late.len(), 1);
        assert!(!late[0].first);
        assert_eq!(w.civ(CivId(1)).unwrap().gold, 100);
        assert_eq!(known_stats(&w, &r, CivId(1)).happiness, Fixed::whole(2));
    }

    #[test]
    fn spains_first_discovery_stacks_with_el_dorado_once() {
        let r = ruleset();
        let mut w = world(&r, &["Spain", "Rome"]);
        let dorado = r.terrain("El Dorado").unwrap();
        w.grid.natural_wonder[21] = Some(dorado);
        w.visibility.observe(CivId(0), TileId(21));
        let first = discover_visible(&mut w, &r);
        assert_eq!(first.len(), 1);
        assert!(first[0].first);
        assert_eq!(first[0].stats.gold, Fixed::whole(1000));
        assert_eq!(w.civ(CivId(0)).unwrap().gold, 1000);
        assert!(discover_visible(&mut w, &r).is_empty());
        assert_eq!(w.civ(CivId(0)).unwrap().gold, 1000);
        w.visibility.observe(CivId(1), TileId(21));
        assert!(!discover_visible(&mut w, &r)[0].first);
        assert_eq!(w.civ(CivId(1)).unwrap().gold, 0);
    }

    #[test]
    fn reused_sites_preserve_discovery_order_rewards_and_state() {
        let r = ruleset();
        let mut initial = world(&r, &["Spain", "Rome"]);
        initial.grid.natural_wonder[21] = r.terrain("El Dorado");
        for tile in [22, 23] {
            initial.grid.natural_wonder[tile] = r.terrain("Great Barrier Reef");
        }
        let locations = sites(&initial.grid);
        assert_eq!(locations.iter().map(|(t, _)| *t).collect::<Vec<_>>(), [TileId(21), TileId(22), TileId(23)]);
        let mut scanned = initial.clone();
        let mut reused = initial;
        for tile in [21, 22, 23, 21] {
            for civ in [CivId(0), CivId(1)] {
                scanned.visibility.observe(civ, TileId(tile));
                reused.visibility.observe(civ, TileId(tile));
            }
            assert_eq!(discover_at(&mut reused, &r, &locations), discover_visible(&mut scanned, &r));
            assert_eq!(reused, scanned);
        }
    }

    #[test]
    fn city_state_discovery_does_not_claim_the_major_civ_first_bonus() {
        let r = ruleset();
        let mut w = world(&r, &["Rome", "Spain"]);
        w.civ_mut(CivId(0)).unwrap().city_state_type = r.city_state_types.ids().next();
        w.grid.natural_wonder[21] = r.terrain("El Dorado");
        for civ in [CivId(0), CivId(1)] {
            w.visibility.observe(civ, TileId(21));
        }
        let discoveries = discover_visible(&mut w, &r);
        assert_eq!(discoveries.len(), 2);
        assert!(discoveries.iter().all(|d| d.first));
        assert_eq!(w.civ(CivId(1)).unwrap().gold, 1000);
    }

    #[test]
    fn spain_doubles_worked_reef_yields_before_city_percentages() {
        use crate::world::city::{city_yield_in, city_yield_with, City};
        let r = ruleset();
        let mut w = world(&r, &["Spain"]);
        let centre = w.grid.id_at(6, 5).unwrap();
        let adjacent: Vec<_> = w.grid.neighbours(centre).collect();
        let reef = adjacent[0];
        w.grid.natural_wonder[reef.0 as usize] = r.terrain("Great Barrier Reef");
        w.grid.terrain[reef.0 as usize] = r.terrain("Coast").unwrap();
        let mut city = City::found("Madrid", CivId(0), centre);
        city.population = 2;
        city.worked = vec![reef, adjacent[1]];
        let city_id = w.found_city(city);
        let city = w.cities.get(city_id).unwrap();
        // Coast + reef is 3 Food, 1 Production, 2 Gold and 2 Science.
        // The other grassland and centre retain their ordinary yields.
        let ordinary = city_yield_with(&w.grid, &r, city, false);
        assert_eq!(
            (ordinary.food, ordinary.production, ordinary.gold, ordinary.science),
            (Fixed::whole(7), Fixed::whole(2), Fixed::whole(2), Fixed::whole(4))
        );
        let spanish = city_yield_in(&w, &r, city, false);
        assert_eq!(
            (spanish.food, spanish.production, spanish.gold, spanish.science),
            (Fixed::whole(10), Fixed::whole(3), Fixed::whole(4), Fixed::whole(6))
        );
        assert_eq!(
            city_yield_in(&w, &r, city, true).gold,
            Fixed::whole(6),
            "Spain also doubles the reef's golden-age gold"
        );
        assert_eq!(tile_matches_in(&w.grid, &r, reef, "Natural Wonder", None), Verdict::True);
        assert_eq!(tile_matches_in(&w.grid, &r, adjacent[1], "Natural Wonder", None), Verdict::False);
        w.cities.get_mut(city_id).unwrap().built.insert(r.buildings.id("Observatory").unwrap());
        assert_eq!(city_yield_in(&w, &r, w.cities.get(city_id).unwrap(), false).science, Fixed::whole(9));
    }

    #[test]
    fn a_generated_wonder_is_discovered_through_real_unit_visibility() {
        let r = ruleset();
        let m = generate(&r, MapParameters { seed: 3, ..Default::default() }, 4);
        let tile = m.grid.ids().find(|t| m.grid.natural_wonder[t.0 as usize].is_some()).expect("generated wonder");
        let unit_tile = m
            .grid
            .within(tile, 2)
            .into_iter()
            .find(|t| !m.grid.is_water(&r, *t) && crate::world::movement::is_passable(&m.grid, &r, *t))
            .expect("reachable sight");
        let mut w = World::new(m.grid, vec![Civilization::new(CivId(0), r.nations.id("Rome").unwrap(), false)]);
        w.place_unit(Unit::new(r.units.id("Scout").unwrap(), CivId(0), unit_tile, 2));
        crate::world::turn::update_visibility(&mut w);
        assert!(!discover_visible(&mut w, &r).is_empty());
        assert!(w
            .civ(CivId(0))
            .unwrap()
            .known_natural_wonders
            .contains(&w.grid.natural_wonder[tile.0 as usize].unwrap()));
    }
}
