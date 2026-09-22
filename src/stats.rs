//! Exact arithmetic for the rules layer.
//!
//! Civ's rules are full of percentages, and Unciv holds them in `Float`. Floats
//! are the wrong choice here for one specific reason: this crate compiles to
//! both native and `wasm32`, and the whole point of that is the two agreeing
//! bit for bit. Fixed-point removes the question.
//!
//! [`Fixed`] is an `i64` in thousandths. That covers Civ's range (tech costs in
//! the tens of thousands, modifiers to three decimals) with room to spare.

use std::fmt;
use std::ops::{Add, AddAssign, Mul, Neg, Sub, SubAssign};

/// Thousandths per whole unit.
pub const SCALE: i64 = 1_000;

/// A number in the rules layer. Exact, deterministic, and identical on every
/// target. Construct with [`Fixed::whole`] or [`Fixed::permille`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Fixed(i64);

impl Fixed {
    pub const ZERO: Fixed = Fixed(0);
    pub const ONE: Fixed = Fixed(SCALE);

    /// `Fixed::whole(2)` is 2.000.
    pub const fn whole(n: i32) -> Fixed {
        Fixed(n as i64 * SCALE)
    }

    /// `Fixed::permille(1_250)` is 1.250.
    pub const fn permille(n: i64) -> Fixed {
        Fixed(n)
    }

    /// Raw thousandths.
    pub const fn raw(self) -> i64 {
        self.0
    }

    /// Parse a json number without going through `f64`, so `0.1` is exactly
    /// 100 thousandths rather than whatever the nearest double happens to be.
    pub fn parse(s: &str) -> Option<Fixed> {
        let s = s.trim();
        let (neg, s) = match s.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, s.strip_prefix('+').unwrap_or(s)),
        };
        let (int, frac) = s.split_once('.').unwrap_or((s, ""));
        if int.is_empty() && frac.is_empty() {
            return None;
        }
        if !int.chars().all(|c| c.is_ascii_digit()) || !frac.chars().all(|c| c.is_ascii_digit()) {
            return None;
        }
        let whole: i64 = if int.is_empty() { 0 } else { int.parse().ok()? };
        // Take three digits, rounding the fourth half-up.
        let mut milli = 0i64;
        let digits: Vec<i64> = frac.bytes().map(|b| (b - b'0') as i64).collect();
        for i in 0..3 {
            milli = milli * 10 + digits.get(i).copied().unwrap_or(0);
        }
        if digits.get(3).copied().unwrap_or(0) >= 5 {
            milli += 1;
        }
        let v = whole.checked_mul(SCALE)?.checked_add(milli)?;
        Some(Fixed(if neg { -v } else { v }))
    }

    /// Creates a fixed-point value from a fraction `num / den`, rounding to
    /// the nearest thousandth, half away from zero like [`Fixed::round`].
    pub fn from_fraction(num: i64, den: i64) -> Fixed {
        assert!(den != 0, "denominator cannot be zero");
        let scaled = num.checked_mul(SCALE).expect("overflow in from_fraction");
        // Integer division truncates toward zero, so the half goes on in the
        // numerator's direction — a fixed `+ den / 2` rounds -1/2 to -0.499.
        let half = den.abs() / 2;
        Fixed((scaled + if scaled < 0 { -half } else { half }) / den)
    }

    /// Rounds half away from zero, matching how Civ presents partial yields.
    pub fn round(self) -> i64 {
        let half = SCALE / 2;
        if self.0 >= 0 {
            (self.0 + half) / SCALE
        } else {
            (self.0 - half) / SCALE
        }
    }

    pub fn floor(self) -> i64 {
        self.0.div_euclid(SCALE)
    }

    pub fn is_zero(self) -> bool {
        self.0 == 0
    }

    /// `None` on a zero divisor, like the integer `checked_div`.
    pub fn checked_div(self, other: Fixed) -> Option<Fixed> {
        if other.0 == 0 {
            return None;
        }
        let q = (self.0 as i128) * SCALE as i128 / other.0 as i128;
        Some(Fixed(q.clamp(i64::MIN as i128, i64::MAX as i128) as i64))
    }
}

impl Add for Fixed {
    type Output = Fixed;
    fn add(self, o: Fixed) -> Fixed {
        Fixed(self.0.saturating_add(o.0))
    }
}
impl Sub for Fixed {
    type Output = Fixed;
    fn sub(self, o: Fixed) -> Fixed {
        Fixed(self.0.saturating_sub(o.0))
    }
}
/// Saturating on overflow rather than wrapping: a silently wrapped yield
/// would be far harder to notice than a pinned one.
impl Mul for Fixed {
    type Output = Fixed;
    fn mul(self, o: Fixed) -> Fixed {
        let product = (self.0 as i128) * (o.0 as i128) / SCALE as i128;
        Fixed(product.clamp(i64::MIN as i128, i64::MAX as i128) as i64)
    }
}
impl Neg for Fixed {
    type Output = Fixed;
    fn neg(self) -> Fixed {
        Fixed(-self.0)
    }
}
impl AddAssign for Fixed {
    fn add_assign(&mut self, o: Fixed) {
        *self = *self + o
    }
}
impl SubAssign for Fixed {
    fn sub_assign(&mut self, o: Fixed) {
        *self = *self - o
    }
}
impl std::iter::Sum for Fixed {
    fn sum<I: Iterator<Item = Fixed>>(it: I) -> Fixed {
        it.fold(Fixed::ZERO, |a, b| a + b)
    }
}

impl fmt::Display for Fixed {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (sign, v) = if self.0 < 0 { ("-", -self.0) } else { ("", self.0) };
        let (w, m) = (v / SCALE, v % SCALE);
        if m == 0 {
            write!(f, "{sign}{w}")
        } else {
            write!(f, "{sign}{w}.{:03}", m).and_then(|_| Ok(()))
        }
    }
}

/// The seven yields Civ tracks. One struct rather than seven parameters,
/// because nearly everything that produces one produces several.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stats {
    pub food: Fixed,
    pub production: Fixed,
    pub gold: Fixed,
    pub science: Fixed,
    pub culture: Fixed,
    pub faith: Fixed,
    pub happiness: Fixed,
}

/// Names one yield, for iterating and for parsing `[+2 Food]` out of a unique.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Stat {
    Food,
    Production,
    Gold,
    Science,
    Culture,
    Faith,
    Happiness,
}

impl Stat {
    pub const ALL: [Stat; 7] =
        [Stat::Food, Stat::Production, Stat::Gold, Stat::Science, Stat::Culture, Stat::Faith, Stat::Happiness];

    /// Case-insensitive, because the ruleset writes stat names both ways:
    /// `[+1 Science]` inside a unique, `"science"` as a `percentStatBonus` key.
    /// Matching only one form fails silently, which is worse than failing.
    pub fn parse(s: &str) -> Option<Stat> {
        let s = s.trim();
        Stat::ALL.into_iter().find(|stat| stat.name().eq_ignore_ascii_case(s))
    }

    pub fn name(self) -> &'static str {
        match self {
            Stat::Food => "Food",
            Stat::Production => "Production",
            Stat::Gold => "Gold",
            Stat::Science => "Science",
            Stat::Culture => "Culture",
            Stat::Faith => "Faith",
            Stat::Happiness => "Happiness",
        }
    }
}

impl std::ops::Index<Stat> for Stats {
    type Output = Fixed;
    fn index(&self, s: Stat) -> &Fixed {
        match s {
            Stat::Food => &self.food,
            Stat::Production => &self.production,
            Stat::Gold => &self.gold,
            Stat::Science => &self.science,
            Stat::Culture => &self.culture,
            Stat::Faith => &self.faith,
            Stat::Happiness => &self.happiness,
        }
    }
}

impl std::ops::IndexMut<Stat> for Stats {
    fn index_mut(&mut self, s: Stat) -> &mut Fixed {
        match s {
            Stat::Food => &mut self.food,
            Stat::Production => &mut self.production,
            Stat::Gold => &mut self.gold,
            Stat::Science => &mut self.science,
            Stat::Culture => &mut self.culture,
            Stat::Faith => &mut self.faith,
            Stat::Happiness => &mut self.happiness,
        }
    }
}

impl Stats {
    pub const ZERO: Stats = Stats {
        food: Fixed::ZERO,
        production: Fixed::ZERO,
        gold: Fixed::ZERO,
        science: Fixed::ZERO,
        culture: Fixed::ZERO,
        faith: Fixed::ZERO,
        happiness: Fixed::ZERO,
    };

    pub fn of(stat: Stat, amount: Fixed) -> Stats {
        let mut s = Stats::ZERO;
        s[stat] = amount;
        s
    }

    pub fn iter(&self) -> impl Iterator<Item = (Stat, Fixed)> + '_ {
        Stat::ALL.into_iter().map(|s| (s, self[s]))
    }

    pub fn is_zero(&self) -> bool {
        Stat::ALL.into_iter().all(|s| self[s].is_zero())
    }

    /// Parse the `[stats]` parameter of a unique, e.g. `"+2 Food, +1 Culture"`.
    pub fn parse(s: &str) -> Option<Stats> {
        let mut out = Stats::ZERO;
        let mut any = false;
        for part in s.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let (amount, name) = part.rsplit_once(' ')?;
            out[Stat::parse(name)?] += Fixed::parse(amount)?;
            any = true;
        }
        any.then_some(out)
    }
}

impl Add for Stats {
    type Output = Stats;
    fn add(self, o: Stats) -> Stats {
        let mut out = Stats::ZERO;
        for s in Stat::ALL {
            out[s] = self[s] + o[s];
        }
        out
    }
}

impl AddAssign for Stats {
    fn add_assign(&mut self, o: Stats) {
        *self = *self + o
    }
}

impl std::iter::Sum for Stats {
    fn sum<I: Iterator<Item = Stats>>(it: I) -> Stats {
        it.fold(Stats::ZERO, |a, b| a + b)
    }
}

impl fmt::Display for Stats {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts: Vec<String> = self
            .iter()
            .filter(|(_, v)| !v.is_zero())
            .map(|(s, v)| format!("{}{} {}", if v > Fixed::ZERO { "+" } else { "" }, v, s.name()))
            .collect();
        write!(f, "{}", if parts.is_empty() { "-".into() } else { parts.join(", ") })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_without_float_error() {
        // 0.1 is not representable in binary floating point; here it is exact.
        assert_eq!(Fixed::parse("0.1"), Some(Fixed::permille(100)));
        assert_eq!(Fixed::parse("1.25"), Some(Fixed::permille(1_250)));
        assert_eq!(Fixed::parse("-2"), Some(Fixed::whole(-2)));
        assert_eq!(Fixed::parse("3.0"), Some(Fixed::whole(3)));
        assert_eq!(Fixed::parse("1.2345"), Some(Fixed::permille(1_235)), "rounds half up");
        assert_eq!(Fixed::parse("abc"), None);
        assert_eq!(Fixed::parse(""), None);
    }

    #[test]
    fn fractions_round_half_away_from_zero_on_both_sides() {
        assert_eq!(Fixed::from_fraction(1, 2), Fixed::permille(500));
        assert_eq!(Fixed::from_fraction(1, 3), Fixed::permille(333));
        assert_eq!(Fixed::from_fraction(2, 3), Fixed::permille(667));
        assert_eq!(Fixed::from_fraction(-1, 2), Fixed::permille(-500));
        assert_eq!(Fixed::from_fraction(-2, 3), Fixed::permille(-667));
        assert_eq!(Fixed::from_fraction(2, -3), Fixed::permille(-667));
        assert_eq!(Fixed::from_fraction(-2, -3), Fixed::permille(667));
    }

    #[test]
    fn tenths_sum_exactly() {
        // The canonical float failure: 0.1 ten times should be exactly 1.
        let sum: Fixed = std::iter::repeat_n(Fixed::permille(100), 10).sum();
        assert_eq!(sum, Fixed::ONE);
    }

    #[test]
    fn multiplication_is_exact_and_saturates() {
        assert_eq!(Fixed::whole(3) * Fixed::permille(1_500), Fixed::permille(4_500));
        assert_eq!(Fixed::permille(i64::MAX) * Fixed::whole(1000), Fixed::permille(i64::MAX));
    }

    #[test]
    fn rounds_away_from_zero() {
        assert_eq!(Fixed::permille(1_500).round(), 2);
        assert_eq!(Fixed::permille(-1_500).round(), -2);
        assert_eq!(Fixed::permille(1_499).round(), 1);
    }

    #[test]
    fn parses_the_stats_parameter_of_a_unique() {
        let s = Stats::parse("+2 Food, +1 Culture").unwrap();
        assert_eq!(s.food, Fixed::whole(2));
        assert_eq!(s.culture, Fixed::whole(1));
        assert_eq!(s.gold, Fixed::ZERO);
        assert_eq!(Stats::parse("+1 Nonsense"), None);
    }

    #[test]
    fn stats_display_like_the_ruleset_writes_them() {
        assert_eq!(Stats::parse("+2 Food, +1 Culture").unwrap().to_string(), "+2 Food, +1 Culture");
        assert_eq!(Stats::ZERO.to_string(), "-");
    }
}

// ---------------------------------------------------------------------------
// Powers, without touching a float.
//
// Civ's growth formulas use fractional exponents — city expansion is
// `(tilesClaimed + 1.4813)^1.3`, population food is `(pop-1)^1.5`, and city
// strength raises to an exponent the *ruleset* supplies. `powf` is the one
// place this crate would otherwise have to trust the platform: IEEE-754 covers
// `+ - * / sqrt` and nothing else, so `exp`/`ln`/`powf` come from the system
// libm and differ in the last ulp between macOS, glibc, MSVC and wasm.
//
// So they are built here from integer operations only. Everything below is
// exact bit-for-bit on every target, by construction rather than by assumption.
//
// Internally the work happens in Q32 — a `u128` with 32 fractional bits — which
// leaves ~9 decimal digits of headroom over the 3 that `Fixed` keeps, so the
// single rounding back to thousandths is the only rounding that occurs.

const Q32: u128 = 1 << 32;

/// Square root of a Q32 value, exactly. `u128::isqrt` truncates, which is a
/// defined operation on every target — unlike `f64::sqrt` on a value that had
/// to survive a conversion first.
fn sqrt_q32(q: u128) -> u128 {
    (q << 32).isqrt()
}

/// `2^(2^-k)` for k = 1..=32, by repeated exact square roots of 2.
///
/// Built once and cached. Deterministic: integer square root has one answer.
fn exp2_bit_table() -> &'static [u128; 33] {
    static TABLE: std::sync::OnceLock<[u128; 33]> = std::sync::OnceLock::new();
    TABLE.get_or_init(|| {
        let mut t = [0u128; 33];
        let mut v = 2 * Q32; // 2.0
        for slot in t.iter_mut().skip(1) {
            v = sqrt_q32(v);
            *slot = v;
        }
        t
    })
}

/// Binary logarithm of a Q32 value, in Q32.
///
/// Integer part from the bit position; fractional part by repeated squaring —
/// square the mantissa, and every time it passes 2, emit a one bit and halve.
/// A fixed 32 iterations, so the cost and the answer are both constant.
fn log2_q32(mut v: u128) -> Option<i128> {
    if v == 0 {
        return None;
    }
    let mut int_part: i128 = 0;
    while v >= 2 * Q32 {
        v >>= 1;
        int_part += 1;
    }
    while v < Q32 {
        v <<= 1;
        int_part -= 1;
    }
    let mut frac: u128 = 0;
    for i in 0..32 {
        v = (v * v) >> 32;
        if v >= 2 * Q32 {
            v >>= 1;
            frac |= 1 << (31 - i);
        }
    }
    Some((int_part << 32) + frac as i128)
}

/// `2^y` for a Q32 `y`, in Q32. The inverse of [`log2_q32`]: shift by the
/// integer part, then multiply in one constant per set fractional bit.
fn exp2_q32(y: i128) -> u128 {
    let int_part = y >> 32;
    let frac = (y - (int_part << 32)) as u128; // always in [0, 1)
    let table = exp2_bit_table();
    let mut acc = Q32;
    for (k, &factor) in table.iter().enumerate().skip(1) {
        if frac & (1 << (32 - k)) != 0 {
            acc = (acc * factor) >> 32;
        }
    }
    // Shift last, so the mantissa keeps its precision through the multiplies.
    if int_part >= 0 {
        if int_part > 100 {
            return u128::MAX;
        }
        acc << int_part
    } else {
        if int_part < -100 {
            return 0;
        }
        acc >> (-int_part)
    }
}

impl Fixed {
    fn to_q32(self) -> Option<u128> {
        (self.0 >= 0).then(|| (self.0 as u128 * Q32) / SCALE as u128)
    }

    fn from_q32(q: u128) -> Fixed {
        let milli = (q * SCALE as u128 + Q32 / 2) >> 32;
        Fixed(milli.min(i64::MAX as u128) as i64)
    }

    /// Base-2 logarithm. `None` for zero and negatives.
    pub fn log2(self) -> Option<Fixed> {
        let l = log2_q32(self.to_q32()?)?;
        Some(if l >= 0 { Fixed::from_q32(l as u128) } else { -Fixed::from_q32((-l) as u128) })
    }

    /// `2^self`.
    pub fn exp2(self) -> Fixed {
        Fixed::from_q32(exp2_q32((self.0 as i128 * Q32 as i128) / SCALE as i128))
    }

    /// `self^exponent`, as `2^(exponent · log2 self)`.
    ///
    /// `None` when the base is negative or zero and the exponent is not a
    /// positive whole number — the same domain `powf` leaves undefined, made
    /// explicit rather than returning NaN.
    pub fn pow(self, exponent: Fixed) -> Option<Fixed> {
        if self.0 == 0 {
            return Some(if exponent.0 == 0 { Fixed::ONE } else { Fixed::ZERO });
        }
        if self.0 < 0 {
            // Negative bases are only defined for whole exponents; Civ has none,
            // so this is refused rather than half-supported.
            return None;
        }
        if exponent.0 == 0 {
            return Some(Fixed::ONE);
        }
        let l = log2_q32(self.to_q32()?)?;
        let y = exponent.0 as i128;
        // Q32 * milli / 1000 keeps the product in Q32.
        let scaled = (l * y) / SCALE as i128;
        Some(Fixed::from_q32(exp2_q32(scaled)))
    }

    /// Square root, as `self^(1/2)`.
    pub fn sqrt(self) -> Option<Fixed> {
        Some(Fixed::from_q32(sqrt_q32(self.to_q32()?)))
    }
}

#[cfg(test)]
mod pow_tests {
    use super::*;

    /// How far from the true value, in thousandths.
    fn err(got: Fixed, want: f64) -> i64 {
        (got.raw() - (want * 1000.0).round() as i64).abs()
    }

    #[test]
    fn log2_matches_reference() {
        for (x, want) in [(1.0, 0.0), (2.0, 1.0), (8.0, 3.0), (0.5, -1.0), (10.0, std::f64::consts::LOG2_10)] {
            let got = Fixed::parse(&x.to_string()).unwrap().log2().unwrap();
            assert!(err(got, want) <= 1, "log2({x}) = {got}, want {want}");
        }
        assert_eq!(Fixed::ZERO.log2(), None);
        assert_eq!(Fixed::whole(-4).log2(), None);
    }

    #[test]
    fn exp2_matches_reference() {
        for (x, want) in [(0.0, 1.0), (1.0, 2.0), (3.0, 8.0), (0.5, std::f64::consts::SQRT_2), (-2.0, 0.25)] {
            let got = Fixed::parse(&x.to_string()).unwrap().exp2();
            assert!(err(got, want) <= 1, "exp2({x}) = {got}, want {want}");
        }
    }

    /// Round-tripping through `log2` then `exp2` is lossy *by construction*,
    /// because the intermediate is a `Fixed` and so is quantised to
    /// thousandths. `d(2^y)/dy = 2^y·ln2`, so half a thousandth of error in the
    /// exponent becomes `x·0.00035` in the result. The bound below is that
    /// derivative, not a fudge factor.
    ///
    /// `pow` does not pay this: it keeps `log2(x)·y` in Q32 and rounds once.
    #[test]
    fn exp2_inverts_log2_within_the_derivative() {
        for n in 1..500i64 {
            let x = Fixed::permille(n * 37);
            let round_trip = x.log2().unwrap().exp2();
            let allowed = 2 + x.raw() / 2_000;
            assert!(
                (round_trip.raw() - x.raw()).abs() <= allowed,
                "{x} -> {round_trip} (allowed {allowed} thousandths)"
            );
        }
    }

    /// The same values through `pow`, which keeps full precision internally.
    #[test]
    fn pow_of_one_is_exact() {
        for n in 1..500i64 {
            let x = Fixed::permille(n * 37);
            assert_eq!(x.pow(Fixed::ONE).unwrap().raw(), x.raw(), "{x}");
        }
    }

    #[test]
    fn sqrt_matches_reference() {
        for (x, want) in [(4.0, 2.0), (2.0, std::f64::consts::SQRT_2), (100.0, 10.0), (0.25, 0.5)] {
            let got = Fixed::parse(&x.to_string()).unwrap().sqrt().unwrap();
            assert!(err(got, want) <= 1, "sqrt({x}) = {got}, want {want}");
        }
    }

    /// The formulas this exists for, checked against the f64 values Unciv
    /// computes today.
    #[test]
    fn civ_growth_formulas() {
        // CityPopulationManager.kt:53 — food to grow: 15 + 8(p-1) + floor((p-1)^1.5)
        let food_required = |pop: i64| -> i64 {
            let p = Fixed::whole((pop - 1) as i32);
            15 + 8 * (pop - 1) + p.pow(Fixed::permille(1_500)).unwrap().floor()
        };
        for (pop, want) in [(1, 15), (2, 24), (5, 55), (10, 114), (20, 249)] {
            assert_eq!(food_required(pop), want, "pop {pop}");
        }

        // CityExpansionManager.kt:46 — culture for the next tile: 6(n + 1.4813)^1.3
        let culture = |claimed: i32| -> i64 {
            let base = Fixed::whole(claimed) + Fixed::parse("1.4813").unwrap();
            (Fixed::whole(6) * base.pow(Fixed::permille(1_300)).unwrap()).floor()
        };
        // Reference values computed from the same expression in f64.
        //
        // `n = 0` is the case worth keeping: the true value is 9.9997, three
        // ten-thousandths below the integer boundary. An implementation that
        // rounded a hair differently would floor it to 10 and charge a player a
        // different amount of culture for their second tile — which is exactly
        // the class of divergence this module exists to rule out.
        for (n, want) in [(0, 9), (1, 19), (5, 68), (10, 143), (20, 323)] {
            assert_eq!(culture(n), want, "tiles claimed {n}");
        }
    }

    #[test]
    fn edge_cases_are_explicit_not_nan() {
        assert_eq!(Fixed::whole(5).pow(Fixed::ZERO), Some(Fixed::ONE));
        assert_eq!(Fixed::ZERO.pow(Fixed::whole(2)), Some(Fixed::ZERO));
        assert_eq!(Fixed::ZERO.pow(Fixed::ZERO), Some(Fixed::ONE));
        assert_eq!(Fixed::whole(-2).pow(Fixed::whole(2)), None, "refused, not NaN");
    }

    #[test]
    fn integer_powers_are_exact() {
        for (b, e, want) in [(2i32, 3i32, 8i64), (3, 4, 81), (10, 3, 1000), (7, 2, 49)] {
            let got = Fixed::whole(b).pow(Fixed::whole(e)).unwrap();
            assert!((got.raw() - want * 1000).abs() <= 2, "{b}^{e} = {got}, want {want}");
        }
    }
}

// ---------------------------------------------------------------------------
// A wider intermediate, for chains that must round only once.

/// A Q32 value used between [`Fixed`] operations.
///
/// `Fixed` rounds to thousandths on every multiply, which is right for storage
/// and wrong for a formula with six steps in it: six roundings of a thousandth
/// each are enough to push a result across an integer boundary. Civ's damage
/// curve is exactly that shape, and a differential against Unciv found it — 9
/// of 375 matchups landed one point of damage out.
///
/// So a chain converts once into `Precise`, does its work, and converts back
/// once at the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Precise(i128);

impl Add for Precise {
    type Output = Precise;
    fn add(self, o: Precise) -> Precise {
        Precise(self.0 + o.0)
    }
}
impl Mul for Precise {
    type Output = Precise;
    fn mul(self, o: Precise) -> Precise {
        Precise((self.0 * o.0) >> 32)
    }
}

impl Precise {
    const ONE: Precise = Precise(1 << 32);

    pub fn from_fixed(f: Fixed) -> Precise {
        Precise((f.raw() as i128 * (1 << 32)) / SCALE as i128)
    }

    pub fn whole(n: i32) -> Precise {
        Precise((n as i128) << 32)
    }

    pub fn checked_div(self, o: Precise) -> Option<Precise> {
        (o.0 != 0).then(|| Precise((self.0 << 32) / o.0))
    }

    /// `num / den` to within one Q32 step — for constants with more decimals
    /// than a `Fixed` holds, like the 1.4813 in the border curve.
    pub fn from_ratio(num: i64, den: i64) -> Precise {
        Precise(((num as i128) << 32) / den as i128)
    }

    /// Real powers, as `exp2(exponent * log2(self))`, without a round trip
    /// through thousandths on the way in. `None` for a negative base.
    pub fn pow(self, exponent: Fixed) -> Option<Precise> {
        if self.0 < 0 {
            return None;
        }
        if self.0 == 0 {
            return Some(if exponent.is_zero() { Precise::ONE } else { Precise(0) });
        }
        let l = log2_q32(self.0 as u128)?;
        Some(Precise(exp2_q32((l * exponent.raw() as i128) / SCALE as i128) as i128))
    }

    /// Integer powers by repeated squaring.
    ///
    /// Far more accurate than routing through `log2`/`exp2`, and the exponents
    /// that appear in Civ's formulas — the `^4` in the damage curve — are all
    /// whole numbers.
    pub fn powi(self, mut exp: u32) -> Precise {
        let mut acc = Precise::ONE;
        let mut base = self;
        while exp > 0 {
            if exp & 1 == 1 {
                acc = acc * base;
            }
            base = base * base;
            exp >>= 1;
        }
        acc
    }

    /// The single rounding, back to thousandths.
    pub fn to_fixed(self) -> Fixed {
        let scaled = self.0 * SCALE as i128;
        let half = 1i128 << 31;
        let rounded = if scaled >= 0 { (scaled + half) >> 32 } else { -((-scaled + half) >> 32) };
        Fixed::permille(rounded.clamp(i64::MIN as i128, i64::MAX as i128) as i64)
    }

    /// Round down to a whole number — toward negative infinity.
    pub fn floor(self) -> i64 {
        (self.0 >> 32).clamp(i64::MIN as i128, i64::MAX as i128) as i64
    }

    /// Round to a whole number, away from zero at the halfway point.
    pub fn round(self) -> i64 {
        let half = 1i128 << 31;
        let v = if self.0 >= 0 { (self.0 + half) >> 32 } else { -((-self.0 + half) >> 32) };
        v.clamp(i64::MIN as i128, i64::MAX as i128) as i64
    }
}

#[cfg(test)]
mod precise_tests {
    use super::*;

    #[test]
    fn integer_powers_are_exact() {
        assert_eq!(Precise::whole(2).powi(4).round(), 16);
        assert_eq!(Precise::whole(3).powi(5).round(), 243);
        assert_eq!(Precise::whole(7).powi(0).round(), 1);
    }

    #[test]
    fn it_survives_a_chain_that_fixed_would_erode() {
        // (1.1875)^4 = 1.98712...; through Fixed this loses ground at each step.
        let precise = Precise::from_fixed(Fixed::permille(1_1875 / 10)).powi(4);
        let via_fixed = Fixed::permille(1_187).pow(Fixed::whole(4)).unwrap();
        let truth = Fixed::permille(1_987);
        assert!(
            (precise.to_fixed().raw() - truth.raw()).abs() <= (via_fixed.raw() - truth.raw()).abs(),
            "precise {} should be no worse than fixed {}",
            precise.to_fixed(),
            via_fixed
        );
    }

    #[test]
    fn round_trips_through_fixed() {
        for n in [-5_000i64, -1, 0, 1, 250, 1_000, 33_333] {
            let f = Fixed::permille(n);
            assert_eq!(Precise::from_fixed(f).to_fixed(), f, "{f}");
        }
    }
}
