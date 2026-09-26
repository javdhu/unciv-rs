//! Who can see what.
//!
//! Unciv stores this the other way round: each `Tile` owns a
//! `HashSet<String>` of the civ ids that have explored it (`Tile.kt:54`). On a
//! 128×80 map that is 10,240 hash sets of strings. Inverted — one bitset per civ
//! over dense tile indices — the same information is **1.28 KB per civ**, and
//! "can this civ see that tile" becomes a bit test.
//!
//! Two masks, because Civ distinguishes three states, not two:
//!
//! * never explored — black, nothing is known
//! * explored but not currently observed — the remembered state is shown
//! * currently observed — live
//!
//! The remembered state is why [`TileMemory`] exists. Unciv keeps the
//! equivalent as `lastSeenImprovement` on each Civilization
//! (`Civilization.kt:228`). It stays sparse: most tiles a civ remembers are
//! unremarkable, and making it dense would cost more than the bitsets save.
//!
//! This is a security boundary, not a rendering concern. The server is
//! authoritative, so a player's view has to be *derived* from these masks
//! before anything is sent. A filter applied per query will eventually be
//! forgotten at one call site, and that call site leaks the map.

use super::grid::{CivId, RoadLevel, TileId};
use crate::arena::Id;
use crate::ruleset::{Nation, Ruleset, Terrain, TileImprovement, TileResource};
use crate::stats::Fixed;
use crate::world::city::City;
use crate::world::entities::Unit;
use crate::shared::Shared;
use crate::world::world::World;
use std::collections::BTreeMap;

#[cfg(any(test, feature = "sight-audit"))]
pub mod audit;

/// Sight is a pure function of map topology and the elevations and blocking terrain one ring beyond its range. The
/// cache keeps a whole map of those inputs, each tile's `(height, height with blocking)`, beside the answers worked
/// out from it, which sit in the slot of the tile seen from. A sweep ([`Sightlines::sweep`]) compares the whole map
/// with the grid once and drops every answer if one tile differs; a lone lookup compares its own reach. Either way
/// an answer is reused only while every input it came from still matches (not a hash, not a world's address), so
/// it cannot leak across games, edits or rulesets. Bounded per simulation thread.
#[derive(Default)]
struct SightCache {
    map: (u16, u16, bool),
    heights: Vec<(i32, i32)>,
    /// Bumped whenever the map above is replaced, so a sweep can tell that another grid used the cache meanwhile.
    generation: u64,
    slots: Vec<Vec<Sightline>>,
    entries: usize,
}
struct Sightline {
    range: u32,
    attack: bool,
    visible: Vec<TileId>,
}
thread_local! {
    static SIGHT: std::cell::RefCell<SightCache> = std::cell::RefCell::new(SightCache::default());
}
const SIGHTLINES: usize = 4096;

fn sight_heights(grid: &super::TileGrid, r: &Ruleset, tile: TileId) -> (i32, i32) {
    let (mut height, mut blocks) = (0, false);
    for id in grid.terrains(tile) {
        let (h, b) = r.terrain_sight(id);
        height += h;
        blocks |= b;
    }
    (height, height + i32::from(blocks))
}

impl SightCache {
    /// Make the cached map this grid's, dropping every answer if anything on it differs.
    fn sync(&mut self, grid: &super::TileGrid, r: &Ruleset) {
        let map = (grid.width(), grid.height(), grid.wraps_x());
        let same = self.map == map
            && self.heights.len() == grid.len()
            && grid.ids().zip(&self.heights).all(|(tile, &h)| sight_heights(grid, r, tile) == h);
        if !same {
            self.map = map;
            self.heights = grid.ids().map(|tile| sight_heights(grid, r, tile)).collect();
            self.generation += 1;
            self.slots.iter_mut().for_each(Vec::clear);
            self.entries = 0;
        }
    }

    /// The cached answer, worked out against the cached map if there is none yet. Callers have checked that the
    /// map holds for every tile the answer reads.
    fn with<T>(&mut self, grid: &super::TileGrid, from: TileId, range: u32, attack: bool, f: impl FnOnce(&[TileId]) -> T) -> T {
        let slot = from.0 as usize;
        if self.slots.len() <= slot {
            self.slots.resize_with(grid.len().max(slot + 1), Vec::new);
        }
        if let Some(i) = self.slots[slot].iter().position(|e| e.range == range && e.attack == attack) {
            return f(&self.slots[slot][i].visible);
        }
        if self.entries >= SIGHTLINES {
            self.slots.iter_mut().for_each(Vec::clear);
            self.entries = 0;
        }
        let heights = &self.heights;
        let visible = sightline(grid, from, range, attack, |tile| heights[tile.0 as usize]);
        self.entries += 1;
        self.slots[slot].push(Sightline { range, attack, visible });
        f(&self.slots[slot].last().expect("just pushed").visible)
    }
}

/// Line-of-sight lookups against one grid.
pub(crate) struct Sightlines<'g> {
    grid: &'g super::TileGrid,
    r: &'g Ruleset,
    /// The cache generation this sweep synced, or `None` for lone lookups.
    synced: Option<std::cell::Cell<u64>>,
}

impl<'g> Sightlines<'g> {
    /// Each lookup checks the terrain it reads: for a few lookups.
    pub(crate) fn lone(grid: &'g super::TileGrid, r: &'g Ruleset) -> Self {
        Sightlines { grid, r, synced: None }
    }

    /// The whole map is checked once, up front: for looking from many tiles. The grid is borrowed, so it cannot
    /// change underneath.
    pub(crate) fn sweep(grid: &'g super::TileGrid, r: &'g Ruleset) -> Self {
        let generation = SIGHT.with(|cache| {
            let mut cache = cache.borrow_mut();
            cache.sync(grid, r);
            cache.generation
        });
        Sightlines { grid, r, synced: Some(std::cell::Cell::new(generation)) }
    }

    /// [`viewable_tiles`], lent rather than copied.
    pub(crate) fn with<T>(&self, from: TileId, range: u32, attack: bool, f: impl FnOnce(&[TileId]) -> T) -> T {
        let (grid, r) = (self.grid, self.r);
        #[cfg(any(test, feature = "sight-audit"))]
        let f = |tiles: &[TileId]| {
            audit::check_sightline(tiles, || sightline(grid, from, range, attack, |t| sight_heights(grid, r, t)));
            f(tiles)
        };
        SIGHT.with(|cache| {
            let mut cache = cache.borrow_mut();
            let holds = match &self.synced {
                Some(synced) => {
                    if cache.generation != synced.get() {
                        cache.sync(grid, r);
                        synced.set(cache.generation);
                    }
                    true
                }
                None => {
                    cache.map == (grid.width(), grid.height(), grid.wraps_x())
                        && cache.heights.len() == grid.len()
                        && grid
                            .within(from, range.saturating_add(1))
                            .into_iter()
                            .all(|tile| sight_heights(grid, r, tile) == cache.heights[tile.0 as usize])
                }
            };
            if holds {
                cache.with(grid, from, range, attack, f)
            } else {
                drop(cache);
                f(&sightline(grid, from, range, attack, |tile| sight_heights(grid, r, tile)))
            }
        })
    }
}

/// Kotlin's elevation-based visibility, including tall terrain one ring beyond
/// sight. Paths are evaluated by rings so same-distance tiles cannot relay sight.
pub fn viewable_tiles(grid: &super::TileGrid, r: &Ruleset, from: TileId, range: u32, attack: bool) -> Vec<TileId> {
    Sightlines::lone(grid, r).with(from, range, attack, <[TileId]>::to_vec)
}

/// The uncached answer, given each tile's `(height, height with blocking)`.
fn sightline(
    grid: &super::TileGrid,
    from: TileId,
    range: u32,
    attack: bool,
    heights: impl Fn(TileId) -> (i32, i32),
) -> Vec<TileId> {
    let source = heights(from).0;
    let reach = range.saturating_add(1);
    // `within` gives the reach in ascending id order, so a tile's slot is found by a binary search; each slot holds
    // the highest ground seen on the way to that tile, once its ring is done
    let tiles = grid.within(from, reach);
    let terrain: Vec<_> = tiles.iter().map(|&tile| heights(tile)).collect();
    let slot = |tile: TileId| tiles.binary_search(&tile).ok();
    let distance: Vec<u32> = tiles.iter().map(|&t| grid.distance(from, t)).collect();
    let mut highest: Vec<Option<i32>> = vec![None; tiles.len()];
    if let Some(i) = slot(from) {
        highest[i] = Some(source);
    }
    let mut visible = vec![from];
    let mut ring: Vec<(usize, i32)> = Vec::new();
    for d in 1..=reach {
        ring.clear();
        for (i, &tile) in tiles.iter().enumerate() {
            if distance[i] != d {
                continue;
            }
            let (unit_height, tile_height) = terrain[i];
            if d == reach && (attack || tile_height <= source) {
                continue;
            }
            let Some(previous) = grid.neighbours(tile).filter_map(|n| slot(n).and_then(|j| highest[j])).min() else { continue };
            if source >= previous || (if attack { unit_height } else { tile_height }) > previous {
                visible.push(tile);
            }
            ring.push((i, tile_height.max(previous)));
        }
        for &(i, h) in &ring {
            highest[i] = Some(h);
        }
    }
    visible.sort();
    visible
}

pub fn unit_visible_tiles(world: &World, r: &Ruleset, id: Id<Unit>) -> Vec<TileId> {
    with_unit_sight(world, r, id, &Sightlines::lone(&world.grid, r), <[TileId]>::to_vec)
}

/// [`unit_visible_tiles`], lent rather than copied.
pub(crate) fn with_unit_sight<T>(
    world: &World,
    r: &Ruleset,
    id: Id<Unit>,
    lines: &Sightlines,
    f: impl FnOnce(&[TileId]) -> T,
) -> T {
    use crate::world::conditionals::{applies, GameContext};
    use crate::UniqueType;
    let Some(unit) = world.units.get(id) else { return f(&[]) };
    let ctx = GameContext::new(r).with_world(world).for_civ(unit.owner).for_unit(id).on_tile(unit.position);
    // One walk over the unit's own uniques (it looks its type and promotions up by name) finds all three kinds.
    let (mut blind, mut sight, mut over) = (Vec::new(), Vec::new(), Vec::new());
    for u in unit.all_uniques(r) {
        match u.unique_type() {
            Some(UniqueType::NoSight) => blind.push(u),
            Some(UniqueType::Sight) => sight.push(u),
            Some(UniqueType::CanSeeOverObstacles) => over.push(u),
            _ => {}
        }
    }
    if blind.into_iter().any(|u| applies(u, &ctx).holds()) {
        return f(&[unit.position]);
    }
    let sight: i32 = sight
        .into_iter()
        .chain(world.civ(unit.owner).into_iter().flat_map(|c| c.uniques.matching(UniqueType::Sight)))
        .chain(
            world
                .grid
                .terrains(unit.position)
                .filter_map(|id| r.terrains.get(id))
                .flat_map(|t| &t.uniques)
                .filter(|u| u.unique_type() == Some(UniqueType::Sight)),
        )
        .filter(|u| applies(u, &ctx).holds())
        .filter_map(|u| u.params.first()?.parse::<i32>().ok())
        .sum();
    let range = (2 + sight).max(1) as u32;
    if over.into_iter().any(|u| applies(u, &ctx).holds()) {
        f(&world.grid.within(unit.position, range))
    } else {
        lines.with(unit.position, range, false, f)
    }
}

/// Unit visibility includes ownership, fog, active invisibility and matching
/// detector sight. Tile visibility alone must never authorize a unit query.
pub fn can_see_unit(world: &World, r: &Ruleset, viewer: CivId, id: Id<Unit>) -> bool {
    use crate::world::conditionals::{applies, GameContext};
    use crate::UniqueType;
    let Some(unit) = world.units.get(id) else { return false };
    if unit.owner == viewer {
        return true;
    }
    if !world.visibility.can_see(viewer, unit.position) {
        return false;
    }
    let ctx = GameContext::new(r).with_world(world).for_civ(unit.owner).for_unit(id).on_tile(unit.position);
    let has = |kind| unit.all_uniques(r).any(|u| u.unique_type() == Some(kind) && applies(u, &ctx).holds());
    let invisible = has(UniqueType::Invisible)
        || (has(UniqueType::InvisibleToNonAdjacent)
            && !world
                .units
                .iter()
                .any(|(_, u)| u.owner == viewer && world.grid.distance(u.position, unit.position) <= 1));
    if !invisible {
        return true;
    }
    world.units.iter().filter(|(_, u)| u.owner == viewer).any(|(detector, u)| {
        let ctx = GameContext::new(r).with_world(world).for_civ(viewer).for_unit(detector).on_tile(u.position);
        u.all_uniques(r).any(|rule| {
            rule.unique_type() == Some(UniqueType::CanSeeInvisibleUnits)
                && applies(rule, &ctx).holds()
                && rule
                    .params
                    .first()
                    .is_some_and(|filter| crate::world::filters::unit_matches(r, unit, filter).holds())
        }) && unit_visible_tiles(world, r, detector).contains(&unit.position)
    })
}

/// What a civ remembers of a tile it can no longer see.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TileMemory {
    pub improvement: Option<Id<TileImprovement>>,
    pub resource: Option<Id<TileResource>>,
    pub owner: Option<CivId>,
}

/// A historical observation, not a reference to today's tile. Resources are
/// recorded internally and revealed only through `revealed_resource`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct TileObservation {
    pub terrain: Id<Terrain>,
    pub features: Vec<Id<Terrain>>,
    pub natural_wonder: Option<Id<Terrain>>,
    resource: Option<Id<TileResource>>,
    pub improvement: Option<Id<TileImprovement>>,
    pub improvement_pillaged: bool,
    pub road: RoadLevel,
    pub road_pillaged: bool,
    pub river_edges: u8,
    pub owner: Option<CivId>,
    pub observed_turn: u32,
    /// The wonder or building seen standing here (`world::placement`).
    /// Absent from older saves.
    #[cfg_attr(feature = "serde", serde(default))]
    pub placed: Option<crate::world::grid::Placed>,
}

impl TileObservation {
    /// Host-only deterministic state auditing. Player projections must use
    /// `revealed_resource`, which applies the viewer's technology gate.
    pub(crate) fn recorded_resource(&self) -> Option<Id<TileResource>> {
        self.resource
    }

    pub fn revealed_resource(&self, world: &World, r: &Ruleset, viewer: CivId) -> Option<Id<TileResource>> {
        let resource = self.resource?;
        let definition = r.resources.get(resource)?;
        let revealed = definition.revealed_by.as_ref().is_none_or(|name| {
            r.technologies.id(name).is_some_and(|tech| world.civ(viewer).is_some_and(|c| c.techs.contains(&tech)))
        });
        revealed.then_some(resource)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CityObservation {
    pub id: Id<City>,
    pub name: String,
    pub owner: CivId,
    pub centre: TileId,
    pub population: u32,
    pub tiles: Vec<TileId>,
    pub observed_turn: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CivObservation {
    pub id: CivId,
    pub nation: Id<Nation>,
    pub city_state: bool,
    pub barbarian: bool,
    pub alive: bool,
    pub observed_turn: u32,
}

fn words_for(tiles: usize) -> usize {
    tiles.div_ceil(64)
}

/// Per-civ exploration and visibility masks.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Visibility {
    tiles: usize,
    words: usize,
    /// Civ-major: civ `c` occupies `[c*words, (c+1)*words)`.
    explored: Vec<u64>,
    visible: Vec<u64>,
    /// A row a civ, iterated in (civ, tile) order so iteration is reproducible; saved in columns (`CivTiles`).
    memory: CivTiles<TileMemory>,
    tile_observations: CivTiles<TileObservation>,
    /// Shared, like the rows above, so that copying the masks for a staged world does not copy the records.
    #[cfg_attr(feature = "serde", serde(with = "crate::serialization::shared_map_as_entries"))]
    city_observations: Shared<BTreeMap<(CivId, TileId), CityObservation>>,
    #[cfg_attr(feature = "serde", serde(with = "crate::serialization::shared_map_as_entries"))]
    civ_observations: Shared<BTreeMap<(CivId, CivId), CivObservation>>,
}

impl Visibility {
    pub fn new(tiles: usize, civs: usize) -> Visibility {
        let words = words_for(tiles);
        Visibility {
            tiles,
            words,
            explored: vec![0; words * civs],
            visible: vec![0; words * civs],
            memory: CivTiles::default(),
            tile_observations: CivTiles::default(),
            city_observations: Default::default(),
            civ_observations: Default::default(),
        }
    }

    fn at(&self, civ: CivId, tile: TileId) -> (usize, u64) {
        let bit = tile.0 as usize;
        (civ.0 as usize * self.words + bit / 64, 1u64 << (bit % 64))
    }

    pub fn has_explored(&self, civ: CivId, tile: TileId) -> bool {
        if tile.0 as usize >= self.tiles {
            return false;
        }
        let (w, m) = self.at(civ, tile);
        self.explored.get(w).is_some_and(|x| x & m != 0)
    }

    pub fn can_see(&self, civ: CivId, tile: TileId) -> bool {
        if tile.0 as usize >= self.tiles {
            return false;
        }
        let (w, m) = self.at(civ, tile);
        self.visible.get(w).is_some_and(|x| x & m != 0)
    }

    /// Mark a tile observed. Observation implies exploration, permanently.
    pub fn observe(&mut self, civ: CivId, tile: TileId) {
        if tile.0 as usize >= self.tiles {
            return;
        }
        let (w, m) = self.at(civ, tile);
        if let Some(x) = self.visible.get_mut(w) {
            *x |= m;
        }
        if let Some(x) = self.explored.get_mut(w) {
            *x |= m;
        }
    }

    /// Learn the map without gaining live sight of its units or ownership.
    pub fn explore(&mut self, civ: CivId, tile: TileId) {
        if tile.0 as usize >= self.tiles {
            return;
        }
        let (w, m) = self.at(civ, tile);
        if let Some(x) = self.explored.get_mut(w) {
            *x |= m;
        }
    }

    /// Clear the live mask for one civ. Called at the start of recomputing
    /// sight; exploration is untouched, because it is permanent.
    pub fn clear_visible(&mut self, civ: CivId) {
        let base = civ.0 as usize * self.words;
        if let Some(slice) = self.visible.get_mut(base..base + self.words) {
            slice.fill(0);
        }
    }

    /// Empty masks shaped like these, for a recomputation of live sight to gather into.
    pub(crate) fn seen(&self) -> Seen {
        Seen { tiles: self.tiles, words: self.words, bits: vec![0; self.visible.len()] }
    }

    /// The masks `clear_visible` on each of `civs` and then `observe` of everything in `seen` would leave.
    fn with_sight<'a>(&'a self, civs: &[CivId], seen: &'a Seen) -> impl Iterator<Item = (usize, u64, u64)> + 'a {
        let mut cleared = vec![false; self.visible.len() / self.words.max(1)];
        for civ in civs {
            if let Some(c) = cleared.get_mut(civ.0 as usize) {
                *c = true;
            }
        }
        seen.bits.iter().enumerate().map(move |(w, &bits)| {
            let visible = if cleared[w / self.words] { 0 } else { self.visible[w] };
            (w, visible | bits, self.explored[w] | bits)
        })
    }

    /// Whether replacing `civs`' live sight with `seen` changes anything. Checked first, so that a step which
    /// changes nobody's sight does not copy a store a staged world shares.
    pub(crate) fn sight_changes(&self, civs: &[CivId], seen: &Seen) -> bool {
        self.with_sight(civs, seen).any(|(w, visible, explored)| visible != self.visible[w] || explored != self.explored[w])
    }

    /// Clear `civs`' live sight and observe everything in `seen`.
    pub(crate) fn replace_sight(&mut self, civs: &[CivId], seen: &Seen) {
        let masks: Vec<_> = self.with_sight(civs, seen).collect();
        for (w, visible, explored) in masks {
            self.visible[w] = visible;
            self.explored[w] = explored;
        }
    }

    /// Everything this civ can currently see, in ascending id order.
    pub fn visible_tiles(&self, civ: CivId) -> Vec<TileId> {
        let base = civ.0 as usize * self.words;
        let mut out = Vec::new();
        for w in 0..self.words {
            let mut bits = self.visible.get(base + w).copied().unwrap_or(0);
            while bits != 0 {
                let b = bits.trailing_zeros() as usize;
                let id = w * 64 + b;
                if id < self.tiles {
                    out.push(TileId(id as u16));
                }
                bits &= bits - 1; // clear the lowest set bit
            }
        }
        out
    }

    pub fn explored_count(&self, civ: CivId) -> u32 {
        let base = civ.0 as usize * self.words;
        self.explored.get(base..base + self.words).unwrap_or(&[]).iter().map(|w| w.count_ones()).sum()
    }

    pub fn remember(&mut self, civ: CivId, tile: TileId, what: TileMemory) {
        self.memory.insert((civ, tile), what);
    }

    pub fn recalls(&self, civ: CivId, tile: TileId) -> Option<&TileMemory> {
        self.memory.get(&(civ, tile))
    }

    pub fn known_tiles(&self, civ: CivId) -> impl Iterator<Item = (TileId, &TileObservation)> {
        self.tile_observations.row(civ)
    }

    pub fn known_cities(&self, civ: CivId) -> impl Iterator<Item = &CityObservation> {
        self.city_observations.iter().filter(move |((viewer, _), _)| *viewer == civ).map(|(_, seen)| seen)
    }

    pub fn known_civs(&self, civ: CivId) -> impl Iterator<Item = &CivObservation> {
        self.civ_observations.iter().filter(move |((viewer, _), _)| *viewer == civ).map(|(_, seen)| seen)
    }

    #[cfg(feature = "save")]
    pub(crate) fn validate_snapshot(&self, tiles: usize, civs: usize) -> Result<(), String> {
        let words = words_for(tiles);
        if self.tiles != tiles
            || self.words != words
            || self.explored.len() != words * civs
            || self.visible.len() != words * civs
        {
            return Err("visibility dimensions do not match the world".into());
        }
        for (explored, visible) in self.explored.iter().zip(&self.visible) {
            if visible & !explored != 0 {
                return Err("visible tile has not been explored".into());
            }
        }
        if !tiles.is_multiple_of(64) && words != 0 {
            let mask = (1u64 << (tiles % 64)) - 1;
            for civ in 0..civs {
                if (self.explored[(civ + 1) * words - 1] | self.visible[(civ + 1) * words - 1]) & !mask != 0 {
                    return Err("visibility has bits outside the map".into());
                }
            }
        }
        let tile_key_valid = |(civ, tile): &(CivId, TileId)| (civ.0 as usize) < civs && (tile.0 as usize) < tiles;
        if !self.memory.keys().all(|k| tile_key_valid(&k))
            || !self.tile_observations.keys().all(|k| tile_key_valid(&k))
            || !self.city_observations.keys().all(tile_key_valid)
            || self.civ_observations.keys().any(|(a, b)| a.0 as usize >= civs || b.0 as usize >= civs)
            || self.memory.values().any(|m| m.owner.is_some_and(|owner| owner.0 as usize >= civs))
            || self.tile_observations.values().any(|m| m.owner.is_some_and(|owner| owner.0 as usize >= civs))
            || self.civ_observations.iter().any(|((_, id), c)| c.id != *id)
            || self.city_observations.iter().any(|((_, tile), c)| {
                c.centre != *tile || c.owner.0 as usize >= civs || c.tiles.iter().any(|t| t.0 as usize >= tiles)
            })
        {
            return Err("observation references an invalid civilization or tile".into());
        }
        Ok(())
    }

    #[cfg(feature = "save")]
    pub(crate) fn validate_snapshot_rules(&self, r: &Ruleset) -> Result<(), String> {
        let tile_valid = |m: &TileObservation| {
            r.terrains.get(m.terrain).is_some()
                && m.features.iter().all(|id| r.terrains.get(*id).is_some())
                && m.natural_wonder.is_none_or(|id| r.terrains.get(id).is_some())
                && m.resource.is_none_or(|id| r.resources.get(id).is_some())
                && m.improvement.is_none_or(|id| r.improvements.get(id).is_some())
        };
        if !self.tile_observations.values().all(tile_valid)
            || self.memory.values().any(|m| {
                m.resource.is_some_and(|id| r.resources.get(id).is_none())
                    || m.improvement.is_some_and(|id| r.improvements.get(id).is_none())
            })
            || self.civ_observations.values().any(|c| r.nations.get(c.nation).is_none())
        {
            return Err("observation contains an unknown ruleset item".into());
        }
        Ok(())
    }

    /// Bytes held by the masks, excluding the sparse memory.
    pub fn mask_bytes(&self) -> usize {
        (self.explored.len() + self.visible.len()) * 8
    }
}

/// Live sight being recomputed: [`Visibility::observe`] into masks of the same shape, which replace the live ones
/// only if they differ.
pub(crate) struct Seen {
    tiles: usize,
    words: usize,
    bits: Vec<u64>,
}

impl Seen {
    pub(crate) fn observe(&mut self, civ: CivId, tile: TileId) {
        let bit = tile.0 as usize;
        if bit >= self.tiles {
            return;
        }
        if let Some(x) = self.bits.get_mut(civ.0 as usize * self.words + bit / 64) {
            *x |= 1u64 << (bit % 64);
        }
    }
}

/// An embassy reveals its host's capital and surrounding map once. It
/// records a historical city observation without granting live unit sight.
pub fn reveal_city_site(world: &mut World, r: &Ruleset, viewer: CivId, city: Id<City>) {
    reveal_city_radius(world, r, viewer, city, 2);
}
/// City-state contact reveals the capital center, without live unit sight.
pub fn reveal_city_center(world: &mut World, r: &Ruleset, viewer: CivId, city: Id<City>) {
    reveal_city_radius(world, r, viewer, city, 0);
}
fn reveal_city_radius(world: &mut World, r: &Ruleset, viewer: CivId, city: Id<City>, radius: u32) {
    let Some(c) = world.cities.get(city).cloned() else { return };
    if world.civ(viewer).is_none() {
        return;
    }
    let revealed = world.grid.within(c.centre, radius);
    for &tile in &revealed {
        world.visibility.explore(viewer, tile);
    }
    capture_observations(world, r);
    let mut tiles: Vec<_> = c.tiles.iter().copied().filter(|t| revealed.contains(t)).collect();
    if let Some(previous) = world.visibility.city_observations.get(&(viewer, c.centre)) {
        tiles.extend(previous.tiles.iter().copied().filter(|t| !revealed.contains(t)));
    }
    tiles.sort();
    tiles.dedup();
    for &tile in &tiles {
        if revealed.contains(&tile) {
            if let Some(observation) = world.visibility.tile_observations.get_mut(&(viewer, tile)) {
                observation.owner = Some(c.owner);
            }
        }
    }
    world.visibility.city_observations.insert(
        (viewer, c.centre),
        CityObservation {
            id: city,
            name: c.name,
            owner: c.owner,
            centre: c.centre,
            population: c.population,
            tiles,
            observed_turn: world.turn,
        },
    );
}

/// Honor reveals a new camp's improvement on an already explored tile only.
pub fn remember_improvement(world: &mut World, r: &Ruleset, viewer: CivId, tile: TileId) {
    if !world.grid.contains(tile) || !world.visibility.has_explored(viewer, tile) {
        return;
    }
    capture_observations(world, r);
    if let Some(seen) = world.visibility.tile_observations.get_mut(&(viewer, tile)) {
        seen.improvement = world.grid.improvement[tile.0 as usize];
        seen.improvement_pillaged = false;
    }
    if let Some(seen) = world.visibility.memory.get_mut(&(viewer, tile)) {
        seen.improvement = world.grid.improvement[tile.0 as usize];
    }
}

/// Capture permitted observations at the instant visibility is updated. Call
/// this during movement as well as at the end of a round: an end-only scan
/// cannot reconstruct a city seen briefly while a scout passed it.
pub fn capture_observations(world: &mut World, r: &Ruleset) {
    #[cfg(any(test, feature = "sight-audit"))]
    let expected = audit::expect(world, r, audit::capture_observations);
    let viewers: Vec<_> = world.civ_ids().collect();
    let mut centres: Vec<TileId> = world.cities.iter().map(|(_, c)| c.centre).collect();
    centres.sort_unstable();
    for viewer in viewers {
        // Every record depends on its own entry and the world alone, so they can all be worked out before any is
        // written. Only records that differ are, and a capture that learns nothing leaves a store the staged world
        // shares alone (fog runs after every step of every move).
        let learned = observations(world, r, viewer, &centres);
        if !learned.is_empty() {
            learned.record(&mut world.visibility, viewer);
        }
    }
    #[cfg(any(test, feature = "sight-audit"))]
    audit::check(expected, world, "capture_observations");
}

/// What one capture changes in one viewer's records.
#[derive(Default)]
struct Learned {
    tiles: Vec<(TileId, TileObservation)>,
    memories: Vec<(TileId, TileMemory)>,
    gone: Vec<(CivId, TileId)>,
    cities: Vec<CityObservation>,
    civs: Vec<CivObservation>,
}

impl Learned {
    fn is_empty(&self) -> bool {
        self.tiles.is_empty()
            && self.memories.is_empty()
            && self.gone.is_empty()
            && self.cities.is_empty()
            && self.civs.is_empty()
    }

    fn record(self, visibility: &mut Visibility, viewer: CivId) {
        for (tile, seen) in self.tiles {
            visibility.tile_observations.insert((viewer, tile), seen);
        }
        for (tile, what) in self.memories {
            visibility.remember(viewer, tile, what);
        }
        for key in self.gone {
            visibility.city_observations.remove(&key);
        }
        for seen in self.cities {
            visibility.city_observations.insert((viewer, seen.centre), seen);
        }
        for seen in self.civs {
            visibility.civ_observations.insert((viewer, seen.id), seen);
        }
    }
}

fn observations(world: &World, r: &Ruleset, viewer: CivId, centres: &[TileId]) -> Learned {
    let mut learned = Learned::default();
    observe_tiles(world, viewer, &mut learned);
    let visible_tiles = world.visibility.visible_tiles(viewer);
    observe_cities(world, viewer, centres, &visible_tiles, &mut learned);
    observe_civs(world, r, viewer, &visible_tiles, &mut learned);
    learned
}

fn observe_tiles(world: &World, viewer: CivId, learned: &mut Learned) {
    let v = &*world.visibility;
    // Only a tile seen now or explored without a record can need one: walk those bits of the masks, and the placed
    // buildings (in tile order too) alongside them.
    let base = viewer.0 as usize * v.words;
    let mut standing = world.grid.placed.iter().peekable();
    for w in 0..v.words {
        let (visible_bits, explored_bits) =
            (v.visible.get(base + w).copied().unwrap_or(0), v.explored.get(base + w).copied().unwrap_or(0));
        let mut bits = visible_bits | (explored_bits & !v.tile_observations.present(viewer, w));
        while bits != 0 {
            let b = bits.trailing_zeros() as usize;
            bits &= bits - 1;
            let i = w * 64 + b;
            if i >= v.tiles {
                break;
            }
            let tile = TileId(i as u16);
            let visible = visible_bits & (1 << b) != 0;
            let recorded = v.tile_observations.get(&(viewer, tile));
            if !visible && recorded.is_some() {
                continue;
            }
            while standing.next_if(|(at, _)| **at < tile).is_some() {}
            let prior = v.recalls(viewer, tile);
            let placed = if visible {
                standing.peek().filter(|(at, _)| **at == tile).map(|(_, p)| **p)
            } else {
                recorded.and_then(|o| o.placed)
            };
            let improvement = if visible { world.grid.improvement[i] } else { prior.and_then(|m| m.improvement) };
            let improvement_pillaged = visible && world.grid.improvement_pillaged[i];
            let road = if visible { world.grid.road[i] } else { RoadLevel::None };
            let road_pillaged = visible && world.grid.road_pillaged[i];
            let owner = if visible { world.grid.owner[i] } else { None };
            // A tile seen as it was already remembered needs no new record. Every field is named, so a field added
            // to the record must be added here too.
            let unchanged = recorded.is_some_and(|o| {
                let TileObservation {
                    placed: was_placed,
                    terrain,
                    features,
                    natural_wonder,
                    resource,
                    improvement: was_improvement,
                    improvement_pillaged: was_pillaged,
                    road: was_road,
                    road_pillaged: was_road_pillaged,
                    river_edges,
                    owner: was_owner,
                    observed_turn,
                } = o;
                *was_placed == placed
                    && *terrain == world.grid.terrain[i]
                    && *features == world.grid.features[i]
                    && *natural_wonder == world.grid.natural_wonder[i]
                    && *resource == world.grid.resource[i]
                    && *was_improvement == improvement
                    && *was_pillaged == improvement_pillaged
                    && *was_road == road
                    && *was_road_pillaged == road_pillaged
                    && *river_edges == world.grid.river_edges[i]
                    && *was_owner == owner
                    && *observed_turn == world.turn
            });
            if !unchanged {
                learned.tiles.push((
                    tile,
                    TileObservation {
                        placed,
                        terrain: world.grid.terrain[i],
                        features: world.grid.features[i].clone(),
                        natural_wonder: world.grid.natural_wonder[i],
                        resource: world.grid.resource[i],
                        improvement,
                        improvement_pillaged,
                        road,
                        road_pillaged,
                        river_edges: world.grid.river_edges[i],
                        owner,
                        observed_turn: world.turn,
                    },
                ));
            }
            if visible {
                let what = TileMemory {
                    improvement: world.grid.improvement[i],
                    resource: world.grid.resource[i],
                    owner: world.grid.owner[i],
                };
                if prior != Some(&what) {
                    learned.memories.push((tile, what));
                }
            }
        }
    }
}

fn observe_cities(world: &World, viewer: CivId, centres: &[TileId], visible_tiles: &[TileId], learned: &mut Learned) {
    let v = &*world.visibility;
    // Seeing empty ground clears a remembered city. Losing sight does not.
    learned.gone = v
        .city_observations
        .range((viewer, TileId(0))..=(viewer, TileId(u16::MAX)))
        .map(|(key, _)| *key)
        .filter(|(_, tile)| visible_tiles.binary_search(tile).is_ok() && centres.binary_search(tile).is_err())
        .collect();
    for (id, city) in world.cities.iter() {
        if city.owner != viewer && !v.can_see(viewer, city.centre) {
            continue;
        }
        let previous = v.city_observations.get(&(viewer, city.centre));
        // The tiles are what was recorded, less those now seen outside the city, plus those of it seen now.
        let keep = |t: &TileId| !v.can_see(viewer, *t) || city.tiles.contains(t);
        let add = |t: &&TileId| city.owner == viewer || v.can_see(viewer, **t);
        let unchanged = previous.is_some_and(|c| {
            let CityObservation { id: was_id, name, owner, centre, population, tiles, observed_turn } = c;
            // Sorted and deduplicated already, keeping every tile and adding none it lacks: the same tiles again.
            *was_id == id
                && *name == city.name
                && *owner == city.owner
                && *centre == city.centre
                && *population == city.population
                && *observed_turn == world.turn
                && tiles.windows(2).all(|w| w[0] < w[1])
                && tiles.iter().all(keep)
                && city.tiles.iter().filter(add).all(|t| tiles.binary_search(t).is_ok())
        });
        if !unchanged {
            let mut tiles = previous.map(|c| c.tiles.clone()).unwrap_or_default();
            tiles.retain(keep);
            tiles.extend(city.tiles.iter().filter(add).copied());
            tiles.sort();
            tiles.dedup();
            learned.cities.push(CityObservation {
                id,
                name: city.name.clone(),
                owner: city.owner,
                centre: city.centre,
                population: city.population,
                tiles,
                observed_turn: world.turn,
            });
        }
    }
}

fn observe_civs(world: &World, r: &Ruleset, viewer: CivId, visible_tiles: &[TileId], learned: &mut Learned) {
    let v = &*world.visibility;
    // the civs this viewer sees a city or a tile of, gathered once; then the rest, by whether a unit of theirs is
    // seen, the costly test, in one pass over the units
    let mut shown = vec![false; world.civs.len()];
    let mut show = |civ: CivId| {
        if let Some(seen) = shown.get_mut(civ.0 as usize) {
            *seen = true;
        }
    };
    visible_tiles.iter().filter_map(|t| world.grid.owner[t.0 as usize]).for_each(&mut show);
    world.cities.iter().filter(|(_, c)| v.can_see(viewer, c.centre)).for_each(|(_, c)| show(c.owner));
    for (id, u) in world.units.iter() {
        if shown.get(u.owner.0 as usize) == Some(&false) && u.owner != viewer && can_see_unit(world, r, viewer, id) {
            shown[u.owner.0 as usize] = true;
        }
    }
    for civ in &world.civs {
        let observed = civ.id == viewer || shown.get(civ.id.0 as usize) == Some(&true);
        let previous = v.civ_observations.get(&(viewer, civ.id));
        let first_contact = world.diplomacy.have_met(viewer, civ.id) && previous.is_none();
        if observed || first_contact {
            let seen = CivObservation {
                id: civ.id,
                nation: civ.nation,
                city_state: civ.is_city_state(),
                barbarian: civ.is_barbarian,
                // Contact establishes existence; defeat later needs observation
                // or a separately authorized public notification.
                alive: if observed { civ.is_alive() } else { true },
                observed_turn: world.turn,
            };
            if previous != Some(&seen) {
                learned.civs.push(seen);
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PreviewReach {
    pub tile: TileId,
    /// An estimate from remembered terrain, never a promise of execution.
    pub moves_left: Fixed,
    /// The selected route touches fog or assumed unexplored terrain.
    pub uncertain: bool,
}

/// A deliberately reconstructed movement model. It never copies the hidden
/// grid, cities, units, or other civilizations' private state. Unknown tiles
/// use a stated land/water assumption, so different hidden maps answer alike.
pub(crate) fn movement_knowledge(world: &World, r: &Ruleset, viewer: CivId, id: Id<Unit>) -> Option<(World, Id<Unit>)> {
    use crate::world::entities::Civilization;
    use crate::world::grid::TileGrid;
    let unit = world.units.get(id).filter(|u| u.owner == viewer)?;
    let own = world.civ(viewer)?;
    let terrain = if unit.is_water_unit(r) {
        r.terrain("Ocean").or_else(|| r.terrain("Coast"))
    } else {
        r.terrain("Grassland").or_else(|| r.terrain("Plains"))
    }
    .or_else(|| {
        r.terrains
            .iter()
            .find(|(_, t)| {
                t.impassable != Some(true) && t.r#type == if unit.is_water_unit(r) { "Water" } else { "Land" }
            })
            .map(|(id, _)| id)
    })?;
    let mut grid = TileGrid::new_with_wrap(world.grid.width(), world.grid.height(), terrain, world.grid.wraps_x());
    for (tile, seen) in world.visibility.known_tiles(viewer) {
        let i = tile.0 as usize;
        grid.terrain[i] = seen.terrain;
        grid.features[i] = seen.features.clone();
        grid.natural_wonder[i] = seen.natural_wonder;
        grid.road[i] = seen.road;
        grid.road_pillaged[i] = seen.road_pillaged;
        grid.river_edges[i] = seen.river_edges;
        grid.owner[i] = seen.owner;
        if let Some(placed) = seen.placed {
            grid.placed.insert(tile, placed);
        }
    }
    let last = world.visibility.known_civs(viewer).map(|c| c.id.0).max().unwrap_or(viewer.0).max(viewer.0);
    let mut civs: Vec<_> = (0..=last).map(|id| Civilization::new(CivId(id), own.nation, false)).collect();
    civs[viewer.0 as usize] = own.clone();
    for seen in world.visibility.known_civs(viewer).filter(|c| c.id != viewer) {
        let c = &mut civs[seen.id.0 as usize];
        c.nation = seen.nation;
        c.is_barbarian = seen.barbarian;
        if seen.city_state {
            c.city_state_type = r.city_state_types.ids().next();
        }
    }
    let mut known = World::new(grid, civs);
    for seen in world.visibility.known_civs(viewer).filter(|c| c.id != viewer) {
        known.diplomacy.meet(viewer, seen.id);
        if world.diplomacy.at_war(viewer, seen.id) {
            known.diplomacy.declare_war(viewer, seen.id);
        }
        if world.diplomacy.may_enter_borders_of(viewer, seen.id) {
            known.diplomacy.set_open_borders(seen.id, viewer, true);
        }
    }
    for city in world.visibility.known_cities(viewer) {
        // No registration/claiming: that would overwrite separately observed borders.
        known.cities.insert(City::found(city.name.clone(), city.owner, city.centre));
    }
    let moving = known.units.insert(unit.clone());
    for (other_id, other) in world.units.iter() {
        if other_id == id || !can_see_unit(world, r, viewer, other_id) {
            continue;
        }
        let mut blocker = Unit::new(other.base, other.owner, other.position, 0);
        blocker.embarked = other.embarked;
        known.units.insert(blocker);
    }
    Some((known, moving))
}

type PreviewCosts = BTreeMap<TileId, (Fixed, bool)>;
type PreviewParents = BTreeMap<TileId, TileId>;

fn preview_search(world: &World, r: &Ruleset, viewer: CivId, id: Id<Unit>) -> Option<(PreviewCosts, PreviewParents)> {
    use crate::world::movement::{can_pass_through, is_tile_occupied_for, step_cost_for_unit};
    let (known, moving) = movement_knowledge(world, r, viewer, id)?;
    let unit = known.units.get(moving)?;
    let budget = unit.movement_left;
    let mut best = BTreeMap::from([(unit.position, (Fixed::ZERO, false))]);
    let mut parents = BTreeMap::new();
    let mut frontier = vec![(Fixed::ZERO, false, unit.position)];
    while budget > Fixed::ZERO && !frontier.is_empty() {
        frontier.sort_by(|a, b| b.cmp(a));
        let (cost, uncertain, tile) = frontier.pop()?;
        if best.get(&tile).is_some_and(|b| *b < (cost, uncertain)) {
            continue;
        }
        for next in known.grid.neighbours(tile) {
            if !can_pass_through(&known, r, next, unit) {
                continue;
            }
            let Some(step) = step_cost_for_unit(&known, r, unit, tile, next, budget - cost) else { continue };
            let raw = cost + step;
            // Unseen adjacent units could exert a zone of control even when
            // the destination itself is visible.
            let hidden_adjacent = known
                .grid
                .neighbours(tile)
                .chain(known.grid.neighbours(next))
                .any(|t| !world.visibility.can_see(viewer, t));
            let candidate = (raw.min(budget), uncertain || !world.visibility.can_see(viewer, next) || hidden_adjacent);
            if best.get(&next).is_none_or(|b| candidate < *b) {
                best.insert(next, candidate);
                parents.insert(next, tile);
                if raw < budget {
                    frontier.push((candidate.0, candidate.1, next));
                }
            }
        }
    }
    best.retain(|tile, _| *tile == unit.position || !is_tile_occupied_for(&known, r, *tile, unit));
    Some((best, parents))
}

pub fn reachable_preview(world: &World, r: &Ruleset, viewer: CivId, unit: Id<Unit>) -> Vec<PreviewReach> {
    let Some((best, _)) = preview_search(world, r, viewer, unit) else { return Vec::new() };
    let budget = world.units.get(unit).map_or(Fixed::ZERO, |u| u.movement_left);
    best.into_iter()
        .map(|(tile, (cost, uncertain))| PreviewReach { tile, moves_left: budget - cost, uncertain })
        .collect()
}

/// Planned path excluding the start; authoritative execution checks each step
/// and can stop early when a previously unknown obstacle becomes observed.
pub fn preview_path(world: &World, r: &Ruleset, viewer: CivId, unit: Id<Unit>, target: TileId) -> Option<Vec<TileId>> {
    let (best, parents) = preview_search(world, r, viewer, unit)?;
    if !best.contains_key(&target) {
        return None;
    }
    let start = world.units.get(unit)?.position;
    let mut path = Vec::new();
    let mut tile = target;
    while tile != start {
        if path.len() >= world.grid.len() {
            return None;
        }
        path.push(tile);
        tile = *parents.get(&tile)?;
    }
    path.reverse();
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const CIV: CivId = CivId(0);
    const OTHER: CivId = CivId(1);

    #[test]
    fn sight_cache_tracks_terrain_edits_and_distinct_maps() {
        let r = Ruleset::from_sources(&crate::fingerprint::embedded_sources()).unwrap();
        let grass = r.terrain("Grassland").unwrap();
        let mut grid = super::super::TileGrid::new(9, 7, grass);
        let from = grid.id_at(3, 3).unwrap();
        let open = viewable_tiles(&grid, &r, from, 3, false);
        for tile in grid.within(from, 1) {
            if tile != from { grid.terrain[tile.0 as usize] = r.terrain("Mountain").unwrap(); }
        }
        let blocked = viewable_tiles(&grid, &r, from, 3, false);
        assert_ne!(open, blocked);
        let other = super::super::TileGrid::new(9, 7, grass);
        assert_eq!(open, viewable_tiles(&other, &r, from, 3, false));
        assert_eq!(blocked, viewable_tiles(&grid, &r, from, 3, false));
        grid.terrain.fill(grass);
        assert_eq!(open, viewable_tiles(&grid, &r, from, 3, false));
        assert_ne!(open, viewable_tiles(&grid, &r, from, 1, false));
    }

    #[cfg(feature = "save")]
    #[test]
    fn memories_save_in_columns_with_one_encoding() {
        let r = Ruleset::from_sources(&crate::fingerprint::embedded_sources()).unwrap();
        let (grass, forest) = (r.terrain("Grassland").unwrap(), r.terrain("Forest").unwrap());
        let seen = |t: u16, turn: u32| TileObservation {
            terrain: grass,
            features: if t % 50 < 20 { vec![forest] } else { Vec::new() },
            natural_wonder: None,
            resource: None,
            improvement: None,
            improvement_pillaged: false,
            road: RoadLevel::None,
            road_pillaged: false,
            river_edges: 0,
            owner: (t > 40).then_some(OTHER),
            observed_turn: turn,
            placed: None,
        };
        let mut v = Visibility::new(200, 4);
        for t in (0..60).chain([130, 199]) {
            v.tile_observations.insert((CIV, TileId(t)), seen(t, 3 + u32::from(t) / 20));
            v.memory.insert((CIV, TileId(t)), TileMemory { owner: (t > 40).then_some(OTHER), ..Default::default() });
        }
        // Civ 1 remembers nothing; civ 2 one tile.
        v.tile_observations.insert((CivId(2), TileId(7)), seen(7, 9));
        let bytes = postcard::to_allocvec(&v).unwrap();
        assert_eq!(postcard::from_bytes::<Visibility>(&bytes).unwrap(), v);
        let entries = postcard::to_allocvec(&(
            v.tile_observations.iter().collect::<Vec<_>>(),
            v.memory.iter().collect::<Vec<_>>(),
        ))
        .unwrap();
        assert!(bytes.len() * 4 < entries.len(), "{} bytes against {} as entries", bytes.len(), entries.len());

        let mut regrown = v.clone();
        regrown.tile_observations.rows.resize_with(4, Default::default);
        regrown.tile_observations.rows[0].resize_with(200, || None);
        assert_eq!(regrown, v);
        assert_eq!(postcard::to_allocvec(&regrown).unwrap(), bytes, "however the rows grew");

        let json = serde_json::to_value(&v).unwrap();
        assert_eq!(json["tile_observations"][0][0], serde_json::json!([0, 0]), "JSON keeps the entry list");
        assert_eq!(serde_json::from_value::<Visibility>(json).unwrap(), v);
    }

    #[cfg(feature = "save")]
    #[test]
    fn a_column_must_cover_its_tiles_exactly() {
        assert_eq!(runs([1, 1, 2, 2, 2, 1].into_iter()), vec![(2, 1), (3, 2), (1, 1)]);
        assert_eq!(expand(vec![(2, 'a'), (1, 'b')], 3).unwrap().collect::<String>(), "aab");
        for (runs, len) in [(vec![(2, 'a')], 3), (vec![(2, 'a'), (2, 'b')], 3), (vec![(0, 'a'), (3, 'b')], 3)] {
            assert!(expand(runs, len).is_err());
        }
        assert!(expand(vec![(u32::MAX, 'a')], 3).is_err());
    }

    #[test]
    fn observing_explores_permanently() {
        let mut v = Visibility::new(10_240, 8);
        let t = TileId(4_000);
        assert!(!v.has_explored(CIV, t));

        v.observe(CIV, t);
        assert!(v.can_see(CIV, t));
        assert!(v.has_explored(CIV, t));

        // A unit moves away: no longer visible, still explored.
        v.clear_visible(CIV);
        assert!(!v.can_see(CIV, t));
        assert!(v.has_explored(CIV, t), "exploration does not decay");
    }

    #[test]
    fn civs_do_not_share_sight() {
        let mut v = Visibility::new(10_240, 8);
        let t = TileId(123);
        v.observe(CIV, t);
        assert!(!v.can_see(OTHER, t));
        assert!(!v.has_explored(OTHER, t));
        v.clear_visible(OTHER);
        assert!(v.can_see(CIV, t), "clearing one civ must not touch another");
    }

    #[test]
    fn the_masks_are_small() {
        // The number the inversion is for: 128x80, eight players.
        let v = Visibility::new(10_240, 8);
        assert_eq!(v.mask_bytes(), 2 * 8 * 1_280, "1.28 KB per civ per mask");
        assert!(v.mask_bytes() < 21_000);
    }

    #[test]
    fn visible_tiles_are_ascending_and_complete() {
        let mut v = Visibility::new(10_240, 4);
        let marked = [TileId(0), TileId(63), TileId(64), TileId(4_097), TileId(10_239)];
        for t in marked {
            v.observe(CIV, t);
        }
        assert_eq!(v.visible_tiles(CIV), marked.to_vec());
        assert_eq!(v.explored_count(CIV), 5);
    }

    #[test]
    fn memory_survives_losing_sight() {
        let mut v = Visibility::new(100, 2);
        let t = TileId(7);
        v.observe(CIV, t);
        v.remember(CIV, t, TileMemory { owner: Some(OTHER), ..Default::default() });
        v.clear_visible(CIV);
        assert!(!v.can_see(CIV, t));
        assert_eq!(v.recalls(CIV, t).and_then(|m| m.owner), Some(OTHER));
        assert_eq!(v.recalls(OTHER, t), None);
    }

    #[test]
    fn a_partial_last_word_does_not_leak_phantom_tiles() {
        // 100 tiles is not a multiple of 64; the tail bits must stay unset.
        let mut v = Visibility::new(100, 1);
        for t in 0..100u16 {
            v.observe(CIV, TileId(t));
        }
        assert_eq!(v.visible_tiles(CIV).len(), 100);
        assert_eq!(v.explored_count(CIV), 100);
    }

    fn transit_fixture() -> (World, Ruleset, Id<Unit>) {
        use crate::world::entities::Civilization;
        let r = Ruleset::from_sources(&crate::fingerprint::embedded_sources()).unwrap();
        let mut w = World::new(
            super::super::grid::TileGrid::new(6, 1, r.terrain("Grassland").unwrap()),
            vec![Civilization::new(CIV, r.nations.id("Rome").unwrap(), false)],
        );
        let moving = w.place_unit(Unit::new(r.units.id("Warrior").unwrap(), CIV, TileId(0), 2));
        w.place_unit(Unit::new(r.units.id("Warrior").unwrap(), CIV, TileId(1), 2));
        for tile in [TileId(0), TileId(1), TileId(2)] {
            w.visibility.observe(CIV, tile);
        }
        capture_observations(&mut w, &r);
        (w, r, moving)
    }

    #[test]
    fn a_known_path_can_cross_a_friendly_unit_to_a_free_destination() {
        let (mut w, r, moving) = transit_fixture();
        assert_eq!(preview_path(&w, &r, CIV, moving, TileId(2)), Some(vec![TileId(1), TileId(2)]));
        crate::world::commands::execute(
            &mut w,
            &r,
            CIV,
            crate::world::commands::Command::MoveUnit { unit: moving, to: TileId(2) },
        )
        .unwrap();
        assert_eq!(w.units.get(moving).unwrap().position, TileId(2));
        assert_eq!(w.units_at(TileId(1)).len(), 1);
    }

    #[test]
    fn an_unseen_blocker_cannot_leave_a_transiting_unit_stacked_with_a_friend() {
        let (mut w, r, moving) = transit_fixture();
        w.visibility.clear_visible(CIV);
        w.visibility.observe(CIV, TileId(0));
        w.grid.terrain[2] = r.terrain("Mountain").unwrap();
        let outcome = crate::world::commands::execute(
            &mut w,
            &r,
            CIV,
            crate::world::commands::Command::MoveUnit { unit: moving, to: TileId(2) },
        )
        .unwrap();
        assert_eq!(w.units.get(moving).unwrap().position, TileId(0));
        assert_eq!(w.units.get(moving).unwrap().movement_left, Fixed::whole(2));
        assert!(!outcome.events.iter().any(|e| matches!(e, crate::world::TurnEvent::UnitMoved(_))));
    }

    #[test]
    fn movement_ending_entry_effect_cannot_commit_an_occupied_stop_or_free_reward() {
        let (mut w, mut r, moving) = transit_fixture();
        r.ruin_rewards = crate::arena::Registry::new();
        r.ruin_rewards.insert(
            "Upgrade",
            crate::ruleset::RuinReward {
                name: "Upgrade".into(),
                uniques: vec![crate::Unique::parse("[This Unit] upgrades for free including special upgrades")],
                ..Default::default()
            },
        );
        let scout = r.units.id("Scout").unwrap();
        w.units.get_mut(moving).unwrap().base = scout;
        w.grid.set_improvement(TileId(1), crate::world::ruins::ruins_improvement(&r));
        capture_observations(&mut w, &r);
        let outcome = crate::world::commands::execute(
            &mut w,
            &r,
            CIV,
            crate::world::commands::Command::MoveUnit { unit: moving, to: TileId(2) },
        )
        .unwrap();
        assert_eq!(w.units.get(moving).unwrap().position, TileId(0));
        assert_eq!(w.units.get(moving).unwrap().base, scout);
        assert!(w.grid.improvement[1].is_some());
        assert!(outcome.events.is_empty());
    }
}

/// A value for each (civ, tile), kept densely, a row a civ. It reads and writes like the
/// `BTreeMap<(CivId, TileId), T>` it replaced and iterates in the same (civ, tile) order; its JSON is that map's list
/// of entries (`serialization::map_as_entries`), its binary snapshot a denser form of its own. Fog reads and writes
/// it for every civ and every tile it sees, on every step of every move, where the map's lookups were most of the
/// cost.
#[derive(Debug, Clone)]
pub(crate) struct CivTiles<T> {
    /// Shared row by row, so a staged world that records one civ's view copies that civ's row alone.
    rows: Vec<Shared<Vec<Option<T>>>>,
    /// Which slots of each row hold an entry, 64 tiles to a word. Entries are only ever added, by `insert`.
    present: Vec<Vec<u64>>,
    len: usize,
}

impl<T> Default for CivTiles<T> {
    fn default() -> Self {
        CivTiles { rows: Vec::new(), present: Vec::new(), len: 0 }
    }
}

impl<T> CivTiles<T> {
    pub(crate) fn get(&self, &(civ, tile): &(CivId, TileId)) -> Option<&T> {
        self.rows.get(civ.0 as usize)?.get(tile.0 as usize)?.as_ref()
    }

    /// Which of tiles `64 * word ..` this civ has an entry for, as bits.
    pub(crate) fn present(&self, civ: CivId, word: usize) -> u64 {
        self.present.get(civ.0 as usize).and_then(|row| row.get(word)).copied().unwrap_or(0)
    }

    /// Every entry, in (civ, tile) order.
    pub(crate) fn iter(&self) -> impl Iterator<Item = ((CivId, TileId), &T)> {
        self.rows.iter().enumerate().flat_map(|(c, row)| {
            row.iter().enumerate().filter_map(move |(t, v)| v.as_ref().map(|v| ((CivId(c as u8), TileId(t as u16)), v)))
        })
    }

    #[cfg_attr(not(feature = "serde"), allow(dead_code))] // save validation reads it
    pub(crate) fn keys(&self) -> impl Iterator<Item = (CivId, TileId)> + '_ {
        self.iter().map(|(k, _)| k)
    }

    #[cfg_attr(not(feature = "serde"), allow(dead_code))] // save validation reads it
    pub(crate) fn values(&self) -> impl Iterator<Item = &T> {
        self.iter().map(|(_, v)| v)
    }

    /// One civ's entries, in tile order.
    pub(crate) fn row(&self, civ: CivId) -> impl Iterator<Item = (TileId, &T)> {
        self.rows
            .get(civ.0 as usize)
            .into_iter()
            .flat_map(|row| row.iter().enumerate().filter_map(|(t, v)| v.as_ref().map(|v| (TileId(t as u16), v))))
    }
}

impl<T: Clone> CivTiles<T> {
    pub(crate) fn get_mut(&mut self, &(civ, tile): &(CivId, TileId)) -> Option<&mut T> {
        self.rows.get_mut(civ.0 as usize)?.get_mut(tile.0 as usize)?.as_mut()
    }

    pub(crate) fn insert(&mut self, (civ, tile): (CivId, TileId), value: T) -> Option<T> {
        let (c, t) = (civ.0 as usize, tile.0 as usize);
        if self.rows.len() <= c {
            self.rows.resize_with(c + 1, Shared::default);
        }
        let row = &mut *self.rows[c];
        if row.len() <= t {
            row.resize_with(t + 1, || None);
        }
        let old = row[t].replace(value);
        if old.is_none() {
            self.len += 1;
            if self.present.len() <= c {
                self.present.resize_with(c + 1, Vec::new);
            }
            let words = &mut self.present[c];
            if words.len() <= t / 64 {
                words.resize(t / 64 + 1, 0);
            }
            words[t / 64] |= 1 << (t % 64);
        }
        old
    }
}

/// Equal when they hold the same entries, however their rows grew.
impl<T: PartialEq> PartialEq for CivTiles<T> {
    fn eq(&self, other: &Self) -> bool {
        self.len == other.len && self.iter().eq(other.iter())
    }
}

/// The two forms a `CivTiles` saves in. JSON (`save::to_json`) keeps the entry list. A binary snapshot keeps a row
/// a civ, up to the last civ with any entry: the tiles it holds as a bitset, then its values field by field
/// ([`Columns`]). The keys cost nothing, and a civ's memories of neighbouring tiles mostly agree a field at a time —
/// the same terrain, the same owner, seen on the same turn — where whole values rarely do.
#[cfg(feature = "serde")]
impl<T: serde::Serialize + Columns> serde::Serialize for CivTiles<T> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeSeq;
        if !serializer.is_human_readable() {
            let civs = self.rows.iter().rposition(|row| row.iter().any(Option::is_some)).map_or(0, |c| c + 1);
            let mut seq = serializer.serialize_seq(Some(civs))?;
            for row in &self.rows[..civs] {
                let end = row.iter().rposition(Option::is_some).map_or(0, |t| t + 1);
                let mut tiles = vec![0u8; end.div_ceil(8)];
                let mut values = Vec::new();
                for (t, value) in row[..end].iter().enumerate() {
                    if let Some(value) = value {
                        tiles[t / 8] |= 1 << (t % 8);
                        values.push(value);
                    }
                }
                seq.serialize_element(&(tiles, T::store(&values)))?;
            }
            return seq.end();
        }
        let mut seq = serializer.serialize_seq(Some(self.len))?;
        for (key, value) in self.iter() {
            seq.serialize_element(&(&key, value))?;
        }
        seq.end()
    }
}

#[cfg(feature = "serde")]
impl<'de, T: serde::Deserialize<'de> + Columns + Clone> serde::Deserialize<'de> for CivTiles<T> {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let mut map = CivTiles::default();
        if !deserializer.is_human_readable() {
            let rows = Vec::<(Vec<u8>, T::Stored)>::deserialize(deserializer)?;
            for (c, (tiles, stored)) in rows.into_iter().enumerate() {
                let civ = u8::try_from(c).map_err(|_| D::Error::custom("too many civilizations in fog snapshot"))?;
                if tiles.len() * 8 > usize::from(u16::MAX) + 1 {
                    return Err(D::Error::custom("fog snapshot row exceeds the tile range"));
                }
                let keys: Vec<_> = (0..tiles.len() * 8).filter(|t| tiles[t / 8] & (1 << (t % 8)) != 0).collect();
                let values = T::load(stored, keys.len()).map_err(D::Error::custom)?;
                for (t, value) in keys.into_iter().zip(values) {
                    map.insert((CivId(civ), TileId(t as u16)), value);
                }
            }
            return Ok(map);
        }
        let entries = Vec::<((CivId, TileId), T)>::deserialize(deserializer)?;
        for (key, value) in entries {
            if map.insert(key, value).is_some() {
                return Err(D::Error::custom("duplicate snapshot map key"));
            }
        }
        Ok(map)
    }
}

/// A type kept in `CivTiles`, as binary snapshots store it: each field a column of runs of equal values, in tile
/// order.
#[cfg(feature = "serde")]
pub(crate) trait Columns: Sized {
    type Stored: serde::Serialize + serde::de::DeserializeOwned;
    fn store(values: &[&Self]) -> Self::Stored;
    fn load(stored: Self::Stored, len: usize) -> Result<Vec<Self>, String>;
}

/// `(count, value)` runs.
#[cfg(feature = "serde")]
fn runs<T: PartialEq>(values: impl Iterator<Item = T>) -> Vec<(u32, T)> {
    let mut out: Vec<(u32, T)> = Vec::new();
    for value in values {
        match out.last_mut() {
            Some((n, last)) if *last == value => *n += 1,
            _ => out.push((1, value)),
        }
    }
    out
}

/// The `len` values the runs stand for; they must cover exactly that many.
#[cfg(feature = "serde")]
fn expand<T: Clone>(runs: Vec<(u32, T)>, len: usize) -> Result<std::vec::IntoIter<T>, String> {
    let mut out = Vec::with_capacity(len);
    for (n, value) in runs {
        let n = n as usize;
        if n == 0 || n > len - out.len() {
            return Err("fog snapshot column does not match its tiles".into());
        }
        out.extend(std::iter::repeat_n(value, n));
    }
    if out.len() != len {
        return Err("fog snapshot column does not match its tiles".into());
    }
    Ok(out.into_iter())
}

/// Implements [`Columns`]; the struct literal in `load` fails to compile if a field is left out.
#[cfg(feature = "serde")]
macro_rules! columns {
    ($ty:ident as $stored:ident { $($field:ident: $t:ty),+ $(,)? }) => {
        #[derive(serde::Serialize, serde::Deserialize)]
        pub(crate) struct $stored {
            $($field: Vec<(u32, $t)>,)+
        }
        impl Columns for $ty {
            type Stored = $stored;
            fn store(values: &[&Self]) -> $stored {
                $stored { $($field: runs(values.iter().map(|v| v.$field.clone())),)+ }
            }
            fn load(stored: $stored, len: usize) -> Result<Vec<Self>, String> {
                $(let mut $field = expand(stored.$field, len)?;)+
                Ok((0..len).map(|_| $ty { $($field: $field.next().unwrap(),)+ }).collect())
            }
        }
    };
}

#[cfg(feature = "serde")]
columns!(TileMemory as MemoryColumns {
    improvement: Option<Id<TileImprovement>>,
    resource: Option<Id<TileResource>>,
    owner: Option<CivId>,
});

#[cfg(feature = "serde")]
columns!(TileObservation as ObservationColumns {
    terrain: Id<Terrain>,
    features: Vec<Id<Terrain>>,
    natural_wonder: Option<Id<Terrain>>,
    resource: Option<Id<TileResource>>,
    improvement: Option<Id<TileImprovement>>,
    improvement_pillaged: bool,
    road: RoadLevel,
    road_pillaged: bool,
    river_edges: u8,
    owner: Option<CivId>,
    observed_turn: u32,
    placed: Option<crate::world::grid::Placed>,
});
