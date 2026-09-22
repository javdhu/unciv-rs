//! Randomness that is a function of game state.
//!
//! Combat is the first system here that needs a die roll, and a die roll is
//! where determinism usually dies: a thread-local generator, or one seeded from
//! the clock, makes the same action produce different results on the server and
//! on the client. So there is no ambient generator in this crate. Every roll is
//! seeded explicitly from the state that produced it — turn number, tile,
//! participants — which means a replayed log rolls the same numbers.
//!
//! Unciv does the same thing (`BattleDamage.kt:289` seeds from the turn and the
//! tile position), though it feeds `Vector2.hashCode()` into the seed, and a
//! JVM hash code is not a portable value. Here the seed is built from integers
//! whose meaning is fixed.
//!
//! The generator is splitmix64: four wrapping integer operations, no tables, no
//! floats, identical on every target.

use crate::stats::Fixed;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rng(u64);

impl Rng {
    pub fn seeded(seed: u64) -> Rng {
        Rng(seed)
    }

    /// Build a seed from the facts that identify an event.
    ///
    /// Order matters and is part of the contract: the same parts in the same
    /// order always give the same stream.
    pub fn from_parts(parts: &[u64]) -> Rng {
        let mut h: u64 = 0x243F_6A88_85A3_08D3;
        for p in parts {
            h = h.wrapping_add(*p).wrapping_mul(0x9E37_79B9_7F4A_7C15);
            h ^= h >> 29;
        }
        Rng(h)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// A value in `[0, 1)`, to three decimals — the precision everything else
    /// in this crate works to.
    pub fn unit_interval(&mut self) -> Fixed {
        Fixed::permille((self.next_u64() % 1_000) as i64)
    }

    /// A value in `[low, high]`, inclusive.
    pub fn range(&mut self, low: i64, high: i64) -> i64 {
        if high <= low {
            return low;
        }
        low + (self.next_u64() % (high - low + 1) as u64) as i64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_same_seed_gives_the_same_stream() {
        let mut a = Rng::seeded(12345);
        let mut b = Rng::seeded(12345);
        let one: Vec<u64> = (0..50).map(|_| a.next_u64()).collect();
        let two: Vec<u64> = (0..50).map(|_| b.next_u64()).collect();
        assert_eq!(one, two);
    }

    #[test]
    fn different_seeds_diverge() {
        let mut a = Rng::seeded(1);
        let mut b = Rng::seeded(2);
        assert_ne!(a.next_u64(), b.next_u64());
    }

    #[test]
    fn seeds_built_from_parts_depend_on_order() {
        assert_ne!(Rng::from_parts(&[1, 2]), Rng::from_parts(&[2, 1]));
        assert_eq!(Rng::from_parts(&[7, 8, 9]), Rng::from_parts(&[7, 8, 9]));
    }

    #[test]
    fn the_unit_interval_stays_in_range_and_spreads() {
        let mut r = Rng::seeded(99);
        let mut buckets = [0usize; 10];
        for _ in 0..10_000 {
            let v = r.unit_interval();
            assert!(v >= Fixed::ZERO && v < Fixed::ONE, "{v}");
            buckets[(v.raw() / 100) as usize] += 1;
        }
        // Roughly a thousand per bucket; this is a smoke test for gross bias,
        // not a statistical claim.
        for (i, count) in buckets.iter().enumerate() {
            assert!((700..1_300).contains(count), "bucket {i} had {count}");
        }
    }

    #[test]
    fn range_is_inclusive_at_both_ends() {
        let mut r = Rng::seeded(4);
        let mut seen_low = false;
        let mut seen_high = false;
        for _ in 0..500 {
            let v = r.range(1, 6);
            assert!((1..=6).contains(&v));
            seen_low |= v == 1;
            seen_high |= v == 6;
        }
        assert!(seen_low && seen_high, "a d6 should roll both ends in 500 tries");
        assert_eq!(r.range(5, 5), 5, "a degenerate range is its own answer");
    }

    /// The property the module exists for, stated as a test.
    #[test]
    fn a_replayed_sequence_rolls_the_same_numbers() {
        let roll = |turn: u64, tile: u64| {
            let mut r = Rng::from_parts(&[turn, tile]);
            (0..5).map(|_| r.range(24, 36)).collect::<Vec<_>>()
        };
        assert_eq!(roll(17, 4_000), roll(17, 4_000));
        assert_ne!(roll(17, 4_000), roll(18, 4_000), "a later turn rolls differently");
    }
}
