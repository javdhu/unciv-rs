//! Fast lookup over a set of uniques.
//!
//! `hasUnique` is the hottest read in the engine — 122 call sites in Unciv's
//! automation alone, twice anything else — and it is a *presence* question.
//! Presence fits in a bitset: 638 unique types is 80 bytes, and the answer is a
//! bit test.
//!
//! Unciv pays more. Its `UniqueMap` keeps a `HashMap` keyed by placeholder text
//! *and* an `EnumMap` over the type enum, and its own source comments on the
//! cost: "there are *600 unique types*, 750 including deprecated, and EnumMap
//! creates a N-sized array" — roughly 5 KB of pointers per object that owns
//! one.
//!
//! # Rebuilt, never patched
//!
//! An index is built from scratch whenever its sources change: a policy
//! adopted, a building finished, a tech learned. Incremental update is where
//! staleness lives, and a stale index is indistinguishable from a rule that
//! does not exist. Rebuilding a civ's index is a walk over a few hundred
//! uniques; it is not worth being clever about.

use crate::{Unique, UniqueKind, UniqueType};

/// `ceil(640 / 64)`: Unciv's 638 unique types and civ.game's two.
const WORDS: usize = 10;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct UniqueIndex {
    present: [u64; WORDS],
    entries: Vec<Unique>,
}

impl UniqueIndex {
    /// Build from every unique in scope, in the order given.
    pub fn build<'a>(sources: impl IntoIterator<Item = &'a Unique>) -> UniqueIndex {
        let mut index = UniqueIndex::default();
        for u in sources {
            index.insert(u.clone());
        }
        index
    }

    pub fn insert(&mut self, unique: Unique) {
        if let UniqueKind::Known(t) = unique.kind {
            let bit = t as usize;
            self.present[bit / 64] |= 1u64 << (bit % 64);
        }
        self.entries.push(unique);
    }

    /// The hot path: is there any unique of this type?
    pub fn has(&self, t: UniqueType) -> bool {
        let bit = t as usize;
        self.present[bit / 64] & (1u64 << (bit % 64)) != 0
    }

    /// Every unique of this type. Iterates only when `has` already said yes.
    pub fn matching(&self, t: UniqueType) -> impl Iterator<Item = &Unique> {
        let present = self.has(t);
        self.entries.iter().filter(move |u| present && u.unique_type() == Some(t))
    }

    pub fn all(&self) -> impl Iterator<Item = &Unique> {
        self.entries.iter()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Bytes the presence bitset occupies, regardless of how many uniques the
    /// index holds.
    pub const fn bitset_bytes() -> usize {
        WORDS * 8
    }
}

// The bitset is a cache, reconstructed from the ordered entries on load.
#[cfg(feature = "serde")]
impl serde::Serialize for UniqueIndex {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serde::Serialize::serialize(&self.entries, serializer)
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for UniqueIndex {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let entries = <Vec<Unique> as serde::Deserialize>::deserialize(deserializer)?;
        Ok(Self::build(&entries))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presence_is_a_bit_test() {
        let uniques = vec![
            Unique::parse("[+2 Food] from [Forest] tiles [in all cities]"),
            Unique::parse("[+25]% Strength <when attacking>"),
        ];
        let index = UniqueIndex::build(&uniques);

        assert!(index.has(UniqueType::StatsFromTiles));
        assert!(!index.has(UniqueType::NullifyYields));
        assert_eq!(index.matching(UniqueType::StatsFromTiles).count(), 1);
        assert_eq!(index.matching(UniqueType::NullifyYields).count(), 0);
    }

    #[test]
    fn the_bitset_is_eighty_bytes_whatever_it_holds() {
        assert_eq!(UniqueIndex::bitset_bytes(), 80);
        let many: Vec<Unique> = (0..500).map(|_| Unique::parse("[+1 Food]")).collect();
        let index = UniqueIndex::build(&many);
        assert_eq!(index.len(), 500);
        assert_eq!(UniqueIndex::bitset_bytes(), 80, "presence does not grow with entries");
    }

    #[test]
    fn every_unique_type_gets_its_own_bit() {
        // A collision would make one rule answer for another, silently.
        let mut seen = std::collections::HashSet::new();
        for t in UniqueType::ALL {
            let index = UniqueIndex::build(&[Unique::parse(t.template())]);
            let bits: Vec<usize> =
                (0..UniqueType::ALL.len()).filter(|b| index.present[b / 64] & (1u64 << (b % 64)) != 0).collect();
            assert_eq!(bits.len(), 1, "{t:?} should set exactly one bit");
            assert!(seen.insert(bits[0]), "{t:?} collides with an earlier type");
        }
        assert_eq!(seen.len(), UniqueType::ALL.len());
    }

    #[test]
    fn tags_carry_no_bit_but_are_still_kept() {
        let index = UniqueIndex::build(&[Unique::parse("Aircraft")]);
        assert_eq!(index.len(), 1, "the tag is retained");
        assert!(index.present.iter().all(|w| *w == 0), "but sets no type bit");
    }
}
