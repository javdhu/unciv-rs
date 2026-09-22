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
use crate::ruleset::{Terrain, TileImprovement, TileResource};

/// A dense index into [`TileGrid`]. `u16` holds 65,535 tiles; Civ's largest
/// standard map is 128×80 = 10,240.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TileId(pub u16);

/// A player. Fixed at game start, so also a dense index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CivId(pub u8);

/// The six hex directions, in a fixed order so that anything iterating them
/// visits them identically on every machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Direction {
    East,
    NorthEast,
    NorthWest,
    West,
    SouthWest,
    SouthEast,
}

impl Direction {
    pub const ALL: [Direction; 6] = [
        Direction::East, Direction::NorthEast, Direction::NorthWest,
        Direction::West, Direction::SouthWest, Direction::SouthEast,
    ];

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

/// The map. One array per attribute.
#[derive(Debug, Clone, PartialEq)]
pub struct TileGrid {
    width: u16,
    height: u16,
    pub terrain: Vec<Id<Terrain>>,
    /// Between the base terrain and the features in the yield fold order.
    pub natural_wonder: Vec<Option<Id<Terrain>>>,
    /// Hills, forest, rivers — a handful at most, so a `Vec` per tile is fine.
    pub features: Vec<Vec<Id<Terrain>>>,
    pub resource: Vec<Option<Id<TileResource>>>,
    /// Size of the deposit. Strategic resources supply this many, not one.
    pub resource_amount: Vec<i32>,
    pub improvement: Vec<Option<Id<TileImprovement>>>,
    pub owner: Vec<Option<CivId>>,
    /// Precomputed once. `None` at the map edge.
    neighbours: Vec<[Option<TileId>; 6]>,
}

impl TileGrid {
    /// A grid of `width × height`, every tile the given terrain.
    pub fn new(width: u16, height: u16, default_terrain: Id<Terrain>) -> TileGrid {
        let n = width as usize * height as usize;
        assert!(n <= u16::MAX as usize, "{width}x{height} exceeds what TileId can index");
        let mut grid = TileGrid {
            width,
            height,
            terrain: vec![default_terrain; n],
            natural_wonder: vec![None; n],
            features: vec![Vec::new(); n],
            resource: vec![None; n],
            resource_amount: vec![1; n],
            improvement: vec![None; n],
            owner: vec![None; n],
            neighbours: Vec::new(),
        };
        grid.neighbours = (0..n).map(|i| grid.compute_neighbours(TileId(i as u16))).collect();
        grid
    }

    pub fn width(&self) -> u16 { self.width }
    pub fn height(&self) -> u16 { self.height }
    pub fn len(&self) -> usize { self.terrain.len() }
    pub fn is_empty(&self) -> bool { self.terrain.is_empty() }

    pub fn id_at(&self, col: u16, row: u16) -> Option<TileId> {
        (col < self.width && row < self.height)
            .then(|| TileId(row * self.width + col))
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
            let (c, r) = (col as i32 + dc, row as i32 + dr);
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
        std::iter::once(self.terrain[i])
            .chain(self.natural_wonder[i])
            .chain(self.features[i].iter().copied())
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
        (((ax - bx).abs() + (ay - by).abs() + (az - bz).abs()) / 2) as u32
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
                assert!(
                    g.neighbours(b).any(|x| x == a),
                    "{a:?} -> {b:?} but not back; row parity is wrong"
                );
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
}
