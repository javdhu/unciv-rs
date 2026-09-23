//! How Civ's rules compose.
//!
//! A mature 4X accumulates hundreds of overlapping bonuses, and the bugs that
//! result are almost never "this modifier is wrong" — they are "these two
//! modifiers interacted, and the answer depended on which one the engine
//! happened to visit first". Unciv has 638 unique types; the number of pairs is
//! not something anyone tests exhaustively.
//!
//! So ordering is removed as a variable. Contributions go into one of four
//! ordered [`Layer`]s, and **within a layer the result cannot depend on
//! insertion order** — sums are integer sums, and products accumulate as exact
//! rationals with a single rounding at the end. That is a property of the
//! structure, not a convention to be maintained, and it is asserted directly in
//! this module's tests.
//!
//! Every contribution also carries its [`Source`], because "+2 Food" is not a
//! useful tooltip and "where did this 17 come from" is the question actually
//! asked when a number looks wrong.

use crate::stats::{Fixed, SCALE};

/// Which stage of the calculation a contribution belongs to.
///
/// The order here *is* the evaluation order; within one, order is irrelevant.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Layer {
    /// Flat amounts, summed. `+2 Food` from a building.
    Base,
    /// Percentages that stack additively: +25% and +25% give +50%.
    /// Unciv's own docs call this out per-unique; most bonuses are this kind.
    Additive,
    /// Percentages that stack multiplicatively: +50% and +50% give +125%.
    Multiplicative,
    /// Floors and ceilings, applied once everything else has resolved.
    Clamp,
}

/// Where a contribution came from. Kept on every modifier so a breakdown can
/// be shown without recomputing anything.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Source {
    pub kind: SourceKind,
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SourceKind {
    Terrain,
    Feature,
    Resource,
    Improvement,
    Building,
    Policy,
    Promotion,
    Belief,
    Difficulty,
    Nation,
    Era,
    Other,
}

impl Source {
    pub fn new(kind: SourceKind, name: impl Into<String>) -> Source {
        Source { kind, name: name.into() }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Modifier {
    pub layer: Layer,
    /// For [`Layer::Base`] and [`Layer::Clamp`], an absolute amount.
    /// For the percentage layers, the percentage itself: `25` means +25%.
    pub amount: Fixed,
    pub source: Source,
}

impl Modifier {
    pub fn base(amount: Fixed, source: Source) -> Modifier {
        Modifier { layer: Layer::Base, amount, source }
    }
    pub fn additive_percent(percent: Fixed, source: Source) -> Modifier {
        Modifier { layer: Layer::Additive, amount: percent, source }
    }
    pub fn multiplicative_percent(percent: Fixed, source: Source) -> Modifier {
        Modifier { layer: Layer::Multiplicative, amount: percent, source }
    }
}

/// Collects contributions and resolves them. Add in any order.
#[derive(Debug, Clone, Default)]
pub struct ModifierStack {
    modifiers: Vec<Modifier>,
    floor: Option<Fixed>,
    ceiling: Option<Fixed>,
}

fn gcd(a: i128, b: i128) -> i128 {
    let (mut a, mut b) = (a.abs(), b.abs());
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a.max(1)
}

impl ModifierStack {
    pub fn new() -> ModifierStack {
        ModifierStack::default()
    }

    pub fn add(&mut self, m: Modifier) -> &mut Self {
        self.modifiers.push(m);
        self
    }

    /// Every contribution, in the order added.
    pub fn modifiers(&self) -> &[Modifier] {
        &self.modifiers
    }

    /// The result will not go below `v`. Applied in [`Layer::Clamp`].
    pub fn floor_at(&mut self, v: Fixed) -> &mut Self {
        self.floor = Some(match self.floor {
            Some(existing) => existing.max(v),
            None => v,
        });
        self
    }

    pub fn ceil_at(&mut self, v: Fixed) -> &mut Self {
        self.ceiling = Some(match self.ceiling {
            Some(existing) => existing.min(v),
            None => v,
        });
        self
    }

    pub fn is_empty(&self) -> bool {
        self.modifiers.is_empty() && self.floor.is_none() && self.ceiling.is_none()
    }

    /// Resolve against a starting value.
    pub fn evaluate(&self, start: Fixed) -> Fixed {
        let base = start + self.modifiers.iter().filter(|m| m.layer == Layer::Base).map(|m| m.amount).sum::<Fixed>();

        // Additive percentages sum first, then apply once — so +25% +25% is
        // exactly +50%, not two roundings of +25%.
        let additive: Fixed = self.modifiers.iter().filter(|m| m.layer == Layer::Additive).map(|m| m.amount).sum();
        let after_additive =
            base + (base * Fixed::permille(additive.raw()).checked_div(Fixed::whole(100)).unwrap_or(Fixed::ZERO));

        // Multiplicative factors accumulate as one exact rational and round
        // once, which is what makes them order-independent.
        let (mut num, mut den) = (1i128, 1i128);
        for m in self.modifiers.iter().filter(|m| m.layer == Layer::Multiplicative) {
            let factor = SCALE as i128 + (m.amount.raw() as i128 * SCALE as i128) / (100 * SCALE) as i128;
            num = num.saturating_mul(factor);
            den = den.saturating_mul(SCALE as i128);
            let g = gcd(num, den);
            num /= g;
            den /= g;
        }
        let scaled = (after_additive.raw() as i128).saturating_mul(num) / den.max(1);
        let mut out = Fixed::permille(scaled.clamp(i64::MIN as i128, i64::MAX as i128) as i64);

        if let Some(f) = self.floor {
            out = out.max(f);
        }
        if let Some(c) = self.ceiling {
            out = out.min(c);
        }
        out
    }

    /// Per-source contributions for a tooltip, in a stable order.
    ///
    /// Each entry is what the total would lose if that one source were removed,
    /// which is the number a player expects to see next to its name.
    pub fn breakdown(&self, start: Fixed) -> Vec<(Source, Fixed)> {
        let total = self.evaluate(start);
        let mut out: Vec<(Source, Fixed)> = Vec::new();
        for m in &self.modifiers {
            let without = ModifierStack {
                modifiers: self.modifiers.iter().filter(|o| o.source != m.source).cloned().collect(),
                floor: self.floor,
                ceiling: self.ceiling,
            };
            if out.iter().any(|(s, _)| *s == m.source) {
                continue;
            }
            out.push((m.source.clone(), total - without.evaluate(start)));
        }
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(n: &str) -> Source {
        Source::new(SourceKind::Building, n)
    }

    /// Deterministic shuffle, so the property is tested without a dev-dependency.
    fn permutations<T: Clone>(items: &[T]) -> Vec<Vec<T>> {
        if items.len() <= 1 {
            return vec![items.to_vec()];
        }
        let mut out = Vec::new();
        for i in 0..items.len() {
            let mut rest = items.to_vec();
            let head = rest.remove(i);
            for mut p in permutations(&rest) {
                p.insert(0, head.clone());
                out.push(p);
            }
        }
        out
    }

    #[test]
    fn additive_percentages_sum_rather_than_compound() {
        let mut s = ModifierStack::new();
        s.add(Modifier::additive_percent(Fixed::whole(25), src("a")));
        s.add(Modifier::additive_percent(Fixed::whole(25), src("b")));
        // +25% +25% on 100 is 150, not 156.25
        assert_eq!(s.evaluate(Fixed::whole(100)), Fixed::whole(150));
    }

    #[test]
    fn multiplicative_percentages_compound() {
        let mut s = ModifierStack::new();
        s.add(Modifier::multiplicative_percent(Fixed::whole(50), src("a")));
        s.add(Modifier::multiplicative_percent(Fixed::whole(50), src("b")));
        // x1.5 * x1.5 = x2.25
        assert_eq!(s.evaluate(Fixed::whole(100)), Fixed::whole(225));
    }

    #[test]
    fn layers_apply_in_order() {
        let mut s = ModifierStack::new();
        s.add(Modifier::base(Fixed::whole(10), src("flat")));
        s.add(Modifier::additive_percent(Fixed::whole(100), src("pct")));
        // (5 + 10) doubled
        assert_eq!(s.evaluate(Fixed::whole(5)), Fixed::whole(30));
    }

    /// The property the whole module exists for.
    #[test]
    fn result_never_depends_on_insertion_order() {
        let mods = vec![
            Modifier::base(Fixed::whole(3), src("a")),
            Modifier::base(Fixed::permille(1_500), src("b")),
            Modifier::additive_percent(Fixed::whole(33), src("c")),
            Modifier::additive_percent(Fixed::whole(17), src("d")),
            Modifier::multiplicative_percent(Fixed::whole(50), src("e")),
            Modifier::multiplicative_percent(Fixed::whole(25), src("f")),
        ];
        let perms = permutations(&mods);
        assert_eq!(perms.len(), 720);
        let expected = {
            let mut s = ModifierStack::new();
            for m in &mods {
                s.add(m.clone());
            }
            s.evaluate(Fixed::whole(7))
        };
        for p in perms {
            let mut s = ModifierStack::new();
            for m in p {
                s.add(m);
            }
            assert_eq!(s.evaluate(Fixed::whole(7)), expected);
        }
    }

    #[test]
    fn clamps_apply_last_and_tighten() {
        let mut s = ModifierStack::new();
        s.add(Modifier::base(-Fixed::whole(50), src("penalty")));
        s.floor_at(Fixed::ZERO);
        assert_eq!(s.evaluate(Fixed::whole(10)), Fixed::ZERO);

        let mut t = ModifierStack::new();
        t.ceil_at(Fixed::whole(10)).ceil_at(Fixed::whole(5));
        assert_eq!(t.evaluate(Fixed::whole(100)), Fixed::whole(5), "tightest ceiling wins");
    }

    #[test]
    fn breakdown_attributes_each_source() {
        let mut s = ModifierStack::new();
        s.add(Modifier::base(Fixed::whole(2), Source::new(SourceKind::Terrain, "Grassland")));
        s.add(Modifier::base(Fixed::whole(1), Source::new(SourceKind::Improvement, "Farm")));
        let b = s.breakdown(Fixed::ZERO);
        assert_eq!(b.len(), 2);
        assert_eq!(b.iter().map(|(_, v)| *v).sum::<Fixed>(), Fixed::whole(3));
    }

    #[test]
    fn an_empty_stack_is_the_identity() {
        assert_eq!(ModifierStack::new().evaluate(Fixed::whole(4)), Fixed::whole(4));
    }
}
