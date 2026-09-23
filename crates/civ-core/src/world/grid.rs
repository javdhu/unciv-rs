//! The map, as parallel arrays over a dense index.
//!
//! Tiles are the one thing in a Civ game that is never created and never
//! destroyed, so [`TileId`] is a plain index — no generation counter. That is
//! deliberate: `Arena`'s generations exist to catch use-after-free, and paying
//! for them where nothing can die is the mirror of the bug they prevent. Units
//! and cities, which do come and go, keep their arenas.
//!
//! Storage is structure-of-arrays rather than a `Vec<Tile>`, because the reads
//! that matter are columnar — "the owner of every tile in this region", "every
//! tile with this resource" — and because Unciv measured the alternative:
//! `TileMap` keeps a matrix beside its list with the note that it "works several
//! times faster than a hashmap, the performance difference is really
//! astounding".
//!
//! Adjacency is computed once at construction, again following Unciv, whose
//! `Tile.neighbors` is a lazy cached list because "the neighbors of a tile never
//! change".

use crate::arena::Id;
use crate::ruleset::{Ruleset, Terrain, TileImprovement, TileResource};
use crate::unique_type::UniqueType;

/// A dense index into [`TileGrid`]. `u16` holds 65,535 tiles; Civ's largest
/// standard map is 128×80 = 10,240.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TileId(pub u16);

/// A player. Fixed at game start, so also a dense index.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CivId(pub u8);

/// The six hex directions, in a fixed order so that anything iterating them
/// visits them identically on every machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum Direction {
    East = 0,
    NorthEast = 1,
    NorthWest = 2,
    West = 3,
    SouthWest = 4,
    SouthEast = 5,
}

impl Direction {
    pub const ALL: [Direction; 6] = [
        Direction::East,
        Direction::NorthEast,
        Direction::NorthWest,
        Direction::West,
        Direction::SouthWest,
        Direction::SouthEast,
    ];

    #[inline]
    pub fn bit(self) -> u8 {
        1 << (self as u8)
    }

    pub fn opposite(self) -> Direction {
        match self {
            Direction::East => Direction::West,
            Direction::West => Direction::East,
            Direction::NorthEast => Direction::SouthWest,
            Direction::SouthWest => Direction::NorthEast,
            Direction::NorthWest => Direction::SouthEast,
            Direction::SouthEast => Direction::NorthWest,
        }
    }

    /// Column and row deltas in an odd-r offset layout, where odd rows sit half
    /// a hex to the right.
    fn delta(self, odd_row: bool) -> (i32, i32) {
        if odd_row {
            match self {
                Direction::East => (1, 0),
                Direction::NorthEast => (1, -1),
                Direction::NorthWest => (0, -1),
                Direction::West => (-1, 0),
                Direction::SouthWest => (0, 1),
                Direction::SouthEast => (1, 1),
            }
        } else {
            match self {
                Direction::East => (1, 0),
                Direction::NorthEast => (0, -1),
                Direction::NorthWest => (-1, -1),
                Direction::West => (-1, 0),
                Direction::SouthWest => (-1, 1),
                Direction::SouthEast => (0, 1),
            }
        }
    }
}

/// How good a road a tile carries.
///
/// This is a separate axis from `improvement`, not a kind of improvement: a
/// tile can hold a Farm *and* a Road at once, and Unciv stores them in two
/// fields for exactly that reason. The ordering is meaningful — a railroad is
/// strictly better than a road — so `max` over a route gives its quality.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub enum RoadLevel {
    #[default]
    None,
    Road,
    Railroad,
}

impl RoadLevel {
    /// The ruleset improvement that builds this, by the name the JSON uses.
    /// `None` has no improvement, which is why this returns an `Option`.
    pub fn improvement_name(self) -> Option<&'static str> {
        match self {
            RoadLevel::None => None,
            RoadLevel::Road => Some("Road"),
            RoadLevel::Railroad => Some("Railroad"),
        }
    }

    pub fn from_improvement_name(name: &str) -> Option<RoadLevel> {
        match name {
            "Road" => Some(RoadLevel::Road),
            "Railroad" => Some(RoadLevel::Railroad),
            _ => None,
        }
    }
}

/// The map. One array per attribute.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub struct TileGrid {
    width: u16,
    height: u16,
    wrap_x: bool,
    pub terrain: Vec<Id<Terrain>>,
    /// Between the base terrain and the features in the yield fold order.
    pub natural_wonder: Vec<Option<Id<Terrain>>>,
    /// Hills, forest, rivers — a handful at most, so a `Vec` per tile is fine.
    pub features: Vec<Vec<Id<Terrain>>>,
    pub resource: Vec<Option<Id<TileResource>>>,
    /// Size of the deposit. Strategic resources supply this many, not one.
    pub resource_amount: Vec<i32>,
    pub improvement: Vec<Option<Id<TileImprovement>>>,
    /// Work underway: what is being built here and how many turns are done.
    /// Kept on the tile rather than the worker, as Unciv does, so progress
    /// survives the worker being killed.
    pub improvement_progress: Vec<Option<(Id<TileImprovement>, u32)>>,
    pub owner: Vec<Option<CivId>>,
    /// Roads, on their own axis so a tile can carry a road and an improvement.
    pub road: Vec<RoadLevel>,
    /// A pillaged road is still drawn but carries nothing, so connection and
    /// movement ask [`TileGrid::unpillaged_road`] rather than reading `road`.
    pub road_pillaged: Vec<bool>,
    /// Bitmask of river edges for each tile (indexed by Direction::bit).
    pub river_edges: Vec<u8>,
    /// Whether the improvement on this tile is pillaged.
    pub improvement_pillaged: Vec<bool>,
    /// Precomputed once. `None` at the map edge.
    neighbours: Vec<[Option<TileId>; 6]>,
}

impl TileGrid {
    /// A grid of `width × height`, every tile the given terrain.
    pub fn new(width: u16, height: u16, default_terrain: Id<Terrain>) -> TileGrid {
        Self::new_with_wrap(width, height, default_terrain, false)
    }

    /// Cylindrical maps join the east and west edges; the poles remain bounded.
    pub fn new_with_wrap(width: u16, height: u16, default_terrain: Id<Terrain>, wrap_x: bool) -> TileGrid {
        let n = width as usize * height as usize;
        assert!(n <= u16::MAX as usize, "{width}x{height} exceeds what TileId can index");
        let mut grid = TileGrid {
            width,
            height,
            wrap_x,
            terrain: vec![default_terrain; n],
            natural_wonder: vec![None; n],
            features: vec![Vec::new(); n],
            resource: vec![None; n],
            resource_amount: vec![1; n],
            improvement: vec![None; n],
            improvement_progress: vec![None; n],
            owner: vec![None; n],
            road: vec![RoadLevel::None; n],
            road_pillaged: vec![false; n],
            river_edges: vec![0; n],
            improvement_pillaged: vec![false; n],
            neighbours: Vec::new(),
        };
        grid.neighbours = (0..n).map(|i| grid.compute_neighbours(TileId(i as u16))).collect();
        grid
    }

    /// The road a tile carries for any purpose that cares — movement,
    /// connection, upkeep. A pillaged road carries nothing.
    pub fn unpillaged_road(&self, t: TileId) -> RoadLevel {
        let i = t.0 as usize;
        if self.road_pillaged[i] {
            RoadLevel::None
        } else {
            self.road[i]
        }
    }

    /// Lay a road. Building over a pillaged road repairs it, which is why this
    /// clears the flag rather than leaving the caller to remember.
    pub fn set_road(&mut self, t: TileId, level: RoadLevel) {
        let i = t.0 as usize;
        self.road[i] = level;
        self.road_pillaged[i] = false;
    }

    /// Pillage the road on this tile.
    pub fn pillage_road(&mut self, t: TileId) {
        self.road_pillaged[t.0 as usize] = true;
    }

    /// Repair the road on this tile.
    pub fn repair_road(&mut self, t: TileId) {
        self.road_pillaged[t.0 as usize] = false;
    }

    /// The direction from `from` to adjacent tile `to`, if they are neighbours.
    pub fn direction_to(&self, from: TileId, to: TileId) -> Option<Direction> {
        let i = from.0 as usize;
        Direction::ALL.into_iter().find(|&d| self.neighbours[i][d as usize] == Some(to))
    }

    /// Whether there is a river along the shared edge between two adjacent tiles.
    pub fn has_river_between(&self, a: TileId, b: TileId) -> bool {
        if let Some(d) = self.direction_to(a, b) {
            (self.river_edges[a.0 as usize] & d.bit()) != 0
        } else {
            false
        }
    }

    /// Mark a river on the shared edge between two adjacent tiles. Symmetrical.
    pub fn set_river_between(&mut self, a: TileId, b: TileId) {
        if let Some(d) = self.direction_to(a, b) {
            self.river_edges[a.0 as usize] |= d.bit();
            self.river_edges[b.0 as usize] |= d.opposite().bit();
        }
    }

    /// Clear a river on the shared edge between two adjacent tiles. Symmetrical.
    pub fn clear_river_between(&mut self, a: TileId, b: TileId) {
        if let Some(d) = self.direction_to(a, b) {
            self.river_edges[a.0 as usize] &= !d.bit();
            self.river_edges[b.0 as usize] &= !d.opposite().bit();
        }
    }

    /// Whether this tile borders any river.
    pub fn has_river(&self, t: TileId) -> bool {
        self.river_edges[t.0 as usize] != 0
    }

    /// The improvement this tile carries if unpillaged.
    pub fn unpillaged_improvement(&self, t: TileId) -> Option<Id<TileImprovement>> {
        let i = t.0 as usize;
        if self.improvement_pillaged[i] {
            None
        } else {
            self.improvement[i]
        }
    }

    /// Set or change the improvement on a tile, clearing its pillaged status.
    pub fn set_improvement(&mut self, t: TileId, imp: Option<Id<TileImprovement>>) {
        let i = t.0 as usize;
        self.improvement[i] = imp;
        self.improvement_pillaged[i] = false;
    }

    /// Pillage the improvement on this tile.
    pub fn pillage_improvement(&mut self, t: TileId) {
        self.improvement_pillaged[t.0 as usize] = true;
    }

    /// Repair the improvement on this tile.
    pub fn repair_improvement(&mut self, t: TileId) {
        self.improvement_pillaged[t.0 as usize] = false;
    }

    /// Whether this tile is open water.
    pub fn is_water(&self, r: &Ruleset, tile: TileId) -> bool {
        r.terrains.get(self.terrain[tile.0 as usize]).is_some_and(|t| t.r#type == "Water")
    }

    /// Whether this tile has access to fresh water (river border, or adjacent fresh water lake/oasis).
    pub fn is_fresh_water(&self, r: &Ruleset, tile: TileId) -> bool {
        if self.has_river(tile) {
            return true;
        }
        for n in self.neighbours(tile) {
            for t_id in self.terrains(n) {
                if let Some(t) = r.terrains.get(t_id) {
                    if t.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::FreshWater)) {
                        return true;
                    }
                }
            }
        }
        false
    }

    pub fn width(&self) -> u16 {
        self.width
    }
    pub fn height(&self) -> u16 {
        self.height
    }
    pub fn wraps_x(&self) -> bool {
        self.wrap_x
    }
    pub fn len(&self) -> usize {
        self.terrain.len()
    }
    pub fn is_empty(&self) -> bool {
        self.terrain.is_empty()
    }

    pub fn id_at(&self, col: u16, row: u16) -> Option<TileId> {
        (col < self.width && row < self.height).then(|| TileId(row * self.width + col))
    }

    pub fn coords(&self, t: TileId) -> (u16, u16) {
        (t.0 % self.width, t.0 / self.width)
    }

    pub fn contains(&self, t: TileId) -> bool {
        (t.0 as usize) < self.len()
    }

    fn compute_neighbours(&self, t: TileId) -> [Option<TileId>; 6] {
        let (col, row) = self.coords(t);
        let odd = row % 2 == 1;
        let mut out = [None; 6];
        for (i, d) in Direction::ALL.into_iter().enumerate() {
            let (dc, dr) = d.delta(odd);
            let (mut c, r) = (col as i32 + dc, row as i32 + dr);
            if self.wrap_x && self.width > 0 {
                c = c.rem_euclid(self.width as i32);
            }
            if c >= 0 && r >= 0 && c < self.width as i32 && r < self.height as i32 {
                out[i] = self.id_at(c as u16, r as u16);
            }
        }
        out
    }

    pub fn neighbour(&self, t: TileId, d: Direction) -> Option<TileId> {
        self.neighbours[t.0 as usize][Direction::ALL.iter().position(|x| *x == d).unwrap()]
    }

    /// The up-to-six adjacent tiles, always in [`Direction::ALL`] order.
    pub fn neighbours(&self, t: TileId) -> impl Iterator<Item = TileId> + '_ {
        self.neighbours[t.0 as usize].into_iter().flatten()
    }

    /// Base terrain, then natural wonder, then features — the order Unciv's
    /// `allTerrains` yields, which matters because `overrideStats` replaces
    /// whatever came before it.
    pub fn terrains(&self, t: TileId) -> impl Iterator<Item = Id<Terrain>> + '_ {
        let i = t.0 as usize;
        std::iter::once(self.terrain[i]).chain(self.natural_wonder[i]).chain(self.features[i].iter().copied())
    }

    pub fn ids(&self) -> impl Iterator<Item = TileId> {
        (0..self.len() as u16).map(TileId)
    }

    /// Every tile within `radius` steps, including `from`, in ascending id
    /// order so callers that fold over it get the same answer everywhere.
    pub fn within(&self, from: TileId, radius: u32) -> Vec<TileId> {
        let mut seen = vec![false; self.len()];
        let mut frontier = vec![from];
        seen[from.0 as usize] = true;
        for _ in 0..radius {
            let mut next = Vec::new();
            for t in frontier.drain(..) {
                for n in self.neighbours(t) {
                    if !seen[n.0 as usize] {
                        seen[n.0 as usize] = true;
                        next.push(n);
                    }
                }
            }
            frontier = next;
            if frontier.is_empty() {
                break;
            }
        }
        self.ids().filter(|t| seen[t.0 as usize]).collect()
    }

    /// Tiles at exactly `distance` steps, in ascending id order.
    pub fn ring(&self, from: TileId, distance: u32) -> Vec<TileId> {
        if distance == 0 {
            return vec![from];
        }
        let inner = self.within(from, distance - 1);
        self.within(from, distance).into_iter().filter(|t| !inner.contains(t)).collect()
    }

    /// Steps between two tiles, ignoring terrain.
    pub fn distance(&self, a: TileId, b: TileId) -> u32 {
        let (ac, ar) = self.coords(a);
        let (bc, br) = self.coords(b);
        // Offset coordinates do not subtract meaningfully; convert to cube first.
        let to_cube = |c: u16, r: u16| {
            let x = c as i32 - ((r as i32 - (r as i32 & 1)) / 2);
            let z = r as i32;
            (x, -x - z, z)
        };
        let (ax, ay, az) = to_cube(ac, ar);
        let (bx, by, bz) = to_cube(bc, br);
        let distance = |shift: i32| (((ax - bx - shift).abs() + (ay - by + shift).abs() + (az - bz).abs()) / 2) as u32;
        if self.wrap_x {
            distance(0).min(distance(self.width as i32)).min(distance(-(self.width as i32)))
        } else {
            distance(0)
        }
    }
}

#[cfg(feature = "save")]
impl TileGrid {
    pub(crate) fn validate_snapshot(&self) -> Result<(), String> {
        let len = self.width as usize * self.height as usize;
        if self.width == 0 || self.height == 0 || len > u16::MAX as usize {
            return Err("invalid grid dimensions".into());
        }
        let lengths = [
            self.terrain.len(),
            self.natural_wonder.len(),
            self.features.len(),
            self.resource.len(),
            self.resource_amount.len(),
            self.improvement.len(),
            self.improvement_progress.len(),
            self.owner.len(),
            self.road.len(),
            self.road_pillaged.len(),
            self.river_edges.len(),
            self.improvement_pillaged.len(),
            self.neighbours.len(),
        ];
        if lengths.iter().any(|n| *n != len) {
            return Err("grid columns have inconsistent lengths".into());
        }
        for i in 0..len {
            if self.neighbours[i] != self.compute_neighbours(TileId(i as u16)) {
                return Err("grid neighbour table is inconsistent".into());
            }
            if self.river_edges[i] & !0b11_1111 != 0 {
                return Err("grid river mask has invalid bits".into());
            }
            for direction in Direction::ALL {
                if self.river_edges[i] & direction.bit() == 0 {
                    continue;
                }
                if self.neighbours[i][direction as usize]
                    .is_none_or(|next| self.river_edges[next.0 as usize] & direction.opposite().bit() == 0)
                {
                    return Err("grid river edges are not symmetric".into());
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::Registry;
    use crate::ruleset::Terrain;

    fn grid(w: u16, h: u16) -> TileGrid {
        let mut r: Registry<Terrain> = Registry::new();
        let t = r.insert("Grassland", Terrain::default());
        TileGrid::new(w, h, t)
    }

    #[test]
    fn cylindrical_distance_matches_shortest_paths_and_edges_are_symmetric() {
        for width in [5, 6, 11, 12] {
            let terrain = grid(1, 1).terrain[0];
            let mut g = TileGrid::new_with_wrap(width, 7, terrain, true);
            for start in g.ids().collect::<Vec<_>>() {
                let mut distances = vec![u32::MAX; g.len()];
                distances[start.0 as usize] = 0;
                let mut queue = std::collections::VecDeque::from([start]);
                while let Some(tile) = queue.pop_front() {
                    for next in g.neighbours(tile) {
                        assert_eq!(g.direction_to(next, tile), g.direction_to(tile, next).map(Direction::opposite));
                        if distances[next.0 as usize] == u32::MAX {
                            distances[next.0 as usize] = distances[tile.0 as usize] + 1;
                            queue.push_back(next);
                        }
                    }
                }
                for to in g.ids() {
                    assert_eq!(g.distance(start, to), distances[to.0 as usize], "{width}: {start:?} -> {to:?}");
                }
            }
            let a = g.id_at(0, 3).unwrap();
            let b = g.id_at(width - 1, 3).unwrap();
            g.set_river_between(a, b);
            assert!(g.has_river_between(b, a));
            #[cfg(feature = "save")]
            assert!(g.validate_snapshot().is_ok());
        }
    }

    #[test]
    fn interior_tiles_have_six_neighbours() {
        let g = grid(10, 10);
        let centre = g.id_at(5, 5).unwrap();
        assert_eq!(g.neighbours(centre).count(), 6);
    }

    #[test]
    fn corners_have_fewer() {
        let g = grid(10, 10);
        assert!(g.neighbours(g.id_at(0, 0).unwrap()).count() < 6);
        assert!(g.neighbours(g.id_at(9, 9).unwrap()).count() < 6);
    }

    /// The property that catches an offset-layout mistake: if A is B's
    /// neighbour, B must be A's.
    #[test]
    fn adjacency_is_symmetric() {
        let g = grid(12, 12);
        for a in g.ids() {
            for b in g.neighbours(a) {
                assert!(g.neighbours(b).any(|x| x == a), "{a:?} -> {b:?} but not back; row parity is wrong");
            }
        }
    }

    #[test]
    fn opposite_directions_come_back() {
        let g = grid(12, 12);
        let a = g.id_at(6, 6).unwrap();
        for d in Direction::ALL {
            let b = g.neighbour(a, d).unwrap();
            assert_eq!(g.neighbour(b, d.opposite()), Some(a), "{d:?}");
        }
    }

    #[test]
    fn distance_agrees_with_stepping() {
        let g = grid(16, 16);
        let from = g.id_at(8, 8).unwrap();
        for radius in 0..4 {
            for t in g.within(from, radius) {
                assert!(g.distance(from, t) <= radius, "{t:?} at radius {radius}");
            }
        }
        // A ring of radius r on a hex grid holds 6r tiles, so the disc holds
        // 1 + 3r(r+1) — the standard centered-hexagonal number.
        for radius in 0u32..4 {
            let expected = 1 + 3 * radius * (radius + 1);
            assert_eq!(g.within(from, radius).len(), expected as usize, "radius {radius}");
        }
    }

    #[test]
    fn neighbours_are_in_a_fixed_order() {
        let g = grid(8, 8);
        let a = g.id_at(4, 4).unwrap();
        let once: Vec<_> = g.neighbours(a).collect();
        let twice: Vec<_> = g.neighbours(a).collect();
        assert_eq!(once, twice);
        assert_eq!(once.len(), 6);
    }

    #[test]
    fn within_returns_ascending_ids() {
        let g = grid(16, 16);
        let v = g.within(g.id_at(8, 8).unwrap(), 3);
        let mut sorted = v.clone();
        sorted.sort();
        assert_eq!(v, sorted, "callers fold over this; order must not vary");
    }

    #[test]
    fn river_edges_are_symmetric() {
        let mut g = grid(6, 6);
        let a = g.id_at(2, 2).unwrap();
        let b = g.neighbour(a, Direction::East).unwrap();
        assert!(!g.has_river_between(a, b));
        assert!(!g.has_river_between(b, a));

        g.set_river_between(a, b);
        assert!(g.has_river_between(a, b));
        assert!(g.has_river_between(b, a));
        assert!(g.has_river(a));
        assert!(g.has_river(b));

        g.clear_river_between(a, b);
        assert!(!g.has_river_between(a, b));
        assert!(!g.has_river_between(b, a));
        assert!(!g.has_river(a));
        assert!(!g.has_river(b));
    }

    #[test]
    fn improvement_pillaging_round_trips() {
        let mut g = grid(4, 4);
        let t = TileId(3);
        let mut reg: Registry<TileImprovement> = Registry::new();
        let farm = reg.insert("Farm", TileImprovement::default());

        g.set_improvement(t, Some(farm));
        assert_eq!(g.unpillaged_improvement(t), Some(farm));

        g.pillage_improvement(t);
        assert_eq!(g.unpillaged_improvement(t), None);

        g.repair_improvement(t);
        assert_eq!(g.unpillaged_improvement(t), Some(farm));
    }
}
