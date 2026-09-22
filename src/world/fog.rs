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

use super::grid::{CivId, TileId};
use crate::arena::Id;
use crate::ruleset::{TileImprovement, TileResource};
use std::collections::BTreeMap;

/// What a civ remembers of a tile it can no longer see.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TileMemory {
    pub improvement: Option<Id<TileImprovement>>,
    pub resource: Option<Id<TileResource>>,
    pub owner: Option<CivId>,
}

fn words_for(tiles: usize) -> usize {
    tiles.div_ceil(64)
}

/// Per-civ exploration and visibility masks.
#[derive(Debug, Clone, PartialEq)]
pub struct Visibility {
    tiles: usize,
    words: usize,
    /// Civ-major: civ `c` occupies `[c*words, (c+1)*words)`.
    explored: Vec<u64>,
    visible: Vec<u64>,
    /// Sparse, and ordered so iteration is reproducible.
    memory: BTreeMap<(CivId, TileId), TileMemory>,
}

impl Visibility {
    pub fn new(tiles: usize, civs: usize) -> Visibility {
        let words = words_for(tiles);
        Visibility {
            tiles,
            words,
            explored: vec![0; words * civs],
            visible: vec![0; words * civs],
            memory: BTreeMap::new(),
        }
    }

    fn at(&self, civ: CivId, tile: TileId) -> (usize, u64) {
        let bit = tile.0 as usize;
        (civ.0 as usize * self.words + bit / 64, 1u64 << (bit % 64))
    }

    pub fn has_explored(&self, civ: CivId, tile: TileId) -> bool {
        let (w, m) = self.at(civ, tile);
        self.explored.get(w).is_some_and(|x| x & m != 0)
    }

    pub fn can_see(&self, civ: CivId, tile: TileId) -> bool {
        let (w, m) = self.at(civ, tile);
        self.visible.get(w).is_some_and(|x| x & m != 0)
    }

    /// Mark a tile observed. Observation implies exploration, permanently.
    pub fn observe(&mut self, civ: CivId, tile: TileId) {
        let (w, m) = self.at(civ, tile);
        if let Some(x) = self.visible.get_mut(w) {
            *x |= m;
        }
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
        self.explored[base..base + self.words].iter().map(|w| w.count_ones()).sum()
    }

    pub fn remember(&mut self, civ: CivId, tile: TileId, what: TileMemory) {
        self.memory.insert((civ, tile), what);
    }

    pub fn recalls(&self, civ: CivId, tile: TileId) -> Option<&TileMemory> {
        self.memory.get(&(civ, tile))
    }

    /// Bytes held by the masks, excluding the sparse memory.
    pub fn mask_bytes(&self) -> usize {
        (self.explored.len() + self.visible.len()) * 8
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CIV: CivId = CivId(0);
    const OTHER: CivId = CivId(1);

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
}
