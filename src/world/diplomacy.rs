//! Relations between civilizations.
//!
//! A relationship is a fact about a *pair*, and that is the whole design
//! problem. Unciv gives each `Civilization` its own `DiplomacyManager` for every
//! other civ, so every symmetric fact is stored twice and kept in step by hand.
//! Declaring war reads:
//!
//! ```kotlin
//! diplomacyManager.diplomaticStatus = DiplomaticStatus.War
//! val theirDiplomacy = diplomacyManager.otherCiv.getDiplomacyManager(...)
//! ```
//!
//! — one side set, the other fetched to be set separately. Which means "A at
//! war with B, B at peace with A" is *representable*, and a representable
//! inconsistency eventually happens.
//!
//! # The split
//!
//! Facts about the pair are stored **once**, under a canonically ordered key.
//! Facts about a direction are stored **per direction**.
//!
//! | symmetric — stored once | directed — stored per side |
//! |---|---|
//! | war or peace | opinion |
//! | having met | open borders granted |
//! | defensive pact | influence (toward a city-state) |
//!
//! The asymmetry is real: B's resentment need not match A's, and A may grant
//! passage without receiving it. The symmetry is then true *by construction*
//! rather than by maintenance — there is no second copy to disagree with.

use crate::stats::Fixed;
use crate::world::grid::CivId;
use std::collections::BTreeMap;

/// An unordered pair of distinct civs, stored in a canonical order.
///
/// The constructor is the only way to build one, so `Pair::of(a, b)` and
/// `Pair::of(b, a)` are the same key and cannot drift apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Pair(CivId, CivId);

impl Pair {
    /// `None` for a civ paired with itself, which has no meaning here.
    pub fn of(a: CivId, b: CivId) -> Option<Pair> {
        match a.0.cmp(&b.0) {
            std::cmp::Ordering::Less => Some(Pair(a, b)),
            std::cmp::Ordering::Greater => Some(Pair(b, a)),
            std::cmp::Ordering::Equal => None,
        }
    }

    pub fn civs(self) -> (CivId, CivId) {
        (self.0, self.1)
    }

    pub fn other(self, civ: CivId) -> Option<CivId> {
        if civ == self.0 {
            Some(self.1)
        } else if civ == self.1 {
            Some(self.0)
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Stance {
    /// Two civs who have met and are not fighting.
    #[default]
    Peace,
    War,
}

/// What is true of the pair.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Pact {
    pub stance: Stance,
    pub defensive_pact: bool,
    /// Turns since the stance last changed — peace treaties have a duration.
    pub turns_in_stance: u32,
}

/// What one civ thinks of another. Not necessarily mutual.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Attitude {
    /// Positive is warmth. Unciv sums named modifiers into the same figure.
    pub opinion: i32,
    /// Whether the *holder* of this attitude lets the other civ pass.
    pub grants_open_borders: bool,
    /// Toward a city-state. Directed by nature.
    pub influence: Fixed,
}

/// Every relationship in the game.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Diplomacy {
    /// Symmetric facts. One entry per pair, never two.
    pacts: BTreeMap<Pair, Pact>,
    /// Directed facts, keyed `(holder, subject)`.
    attitudes: BTreeMap<(CivId, CivId), Attitude>,
    /// Who has met whom. Symmetric, so also keyed by pair.
    met: BTreeMap<Pair, bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiplomacyEvent {
    Met { pair: Pair },
    WarDeclared { aggressor: CivId, against: CivId },
    PeaceMade { pair: Pair },
}

impl Diplomacy {
    pub fn new() -> Diplomacy {
        Diplomacy::default()
    }

    /// Record that two civs have met. Symmetric by construction.
    pub fn meet(&mut self, a: CivId, b: CivId) -> Option<DiplomacyEvent> {
        let pair = Pair::of(a, b)?;
        if self.met.get(&pair).copied().unwrap_or(false) {
            return None;
        }
        self.met.insert(pair, true);
        self.pacts.entry(pair).or_default();
        Some(DiplomacyEvent::Met { pair })
    }

    pub fn have_met(&self, a: CivId, b: CivId) -> bool {
        Pair::of(a, b).is_some_and(|p| self.met.get(&p).copied().unwrap_or(false))
    }

    /// The stance between two civs. The same answer whichever way it is asked,
    /// because there is only one place to read it from.
    pub fn stance(&self, a: CivId, b: CivId) -> Stance {
        Pair::of(a, b)
            .and_then(|p| self.pacts.get(&p))
            .map_or(Stance::Peace, |p| p.stance)
    }

    pub fn at_war(&self, a: CivId, b: CivId) -> bool {
        self.stance(a, b) == Stance::War
    }

    /// Declare war. `aggressor` is recorded in the event, but the *state* is a
    /// single symmetric fact — there is no second copy to keep in step.
    pub fn declare_war(&mut self, aggressor: CivId, against: CivId) -> Option<DiplomacyEvent> {
        let pair = Pair::of(aggressor, against)?;
        let pact = self.pacts.entry(pair).or_default();
        if pact.stance == Stance::War {
            return None;
        }
        pact.stance = Stance::War;
        pact.defensive_pact = false; // war ends any pact between the two
        pact.turns_in_stance = 0;
        self.met.insert(pair, true); // you have certainly met now
        Some(DiplomacyEvent::WarDeclared { aggressor, against })
    }

    pub fn make_peace(&mut self, a: CivId, b: CivId) -> Option<DiplomacyEvent> {
        let pair = Pair::of(a, b)?;
        let pact = self.pacts.entry(pair).or_default();
        if pact.stance == Stance::Peace {
            return None;
        }
        pact.stance = Stance::Peace;
        pact.turns_in_stance = 0;
        Some(DiplomacyEvent::PeaceMade { pair })
    }

    pub fn pact(&self, a: CivId, b: CivId) -> Option<&Pact> {
        Pair::of(a, b).and_then(|p| self.pacts.get(&p))
    }

    /// `holder`'s view of `subject`.
    pub fn attitude(&self, holder: CivId, subject: CivId) -> Attitude {
        self.attitudes.get(&(holder, subject)).cloned().unwrap_or_default()
    }

    pub fn attitude_mut(&mut self, holder: CivId, subject: CivId) -> &mut Attitude {
        self.attitudes.entry((holder, subject)).or_default()
    }

    /// Advance the clock on every relationship. Ordered iteration, so the
    /// result does not depend on map layout.
    pub fn tick(&mut self) {
        for pact in self.pacts.values_mut() {
            pact.turns_in_stance = pact.turns_in_stance.saturating_add(1);
        }
    }

    /// Every civ this one is at war with, in id order.
    pub fn enemies_of(&self, civ: CivId) -> Vec<CivId> {
        self.pacts
            .iter()
            .filter(|(_, p)| p.stance == Stance::War)
            .filter_map(|(pair, _)| pair.other(civ))
            .collect()
    }

    pub fn pair_count(&self) -> usize {
        self.pacts.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: CivId = CivId(0);
    const B: CivId = CivId(1);
    const C: CivId = CivId(2);

    #[test]
    fn a_pair_is_the_same_key_whichever_way_it_is_built() {
        assert_eq!(Pair::of(A, B), Pair::of(B, A));
        assert_eq!(Pair::of(A, A), None, "a civ has no relationship with itself");
        assert_eq!(Pair::of(A, B).unwrap().other(A), Some(B));
        assert_eq!(Pair::of(A, B).unwrap().other(C), None);
    }

    /// The property the whole module exists for.
    #[test]
    fn a_one_sided_war_cannot_be_expressed() {
        let mut d = Diplomacy::new();
        d.declare_war(A, B);

        assert!(d.at_war(A, B));
        assert!(d.at_war(B, A), "asking the other way gives the same answer");
        assert_eq!(d.pair_count(), 1, "one fact, not two");

        // And peace made by either side ends it for both.
        d.make_peace(B, A);
        assert!(!d.at_war(A, B));
        assert!(!d.at_war(B, A));
    }

    #[test]
    fn opinion_is_directed_and_may_disagree() {
        let mut d = Diplomacy::new();
        d.attitude_mut(A, B).opinion = 40;
        d.attitude_mut(B, A).opinion = -30;

        assert_eq!(d.attitude(A, B).opinion, 40);
        assert_eq!(d.attitude(B, A).opinion, -30, "resentment need not be mutual");
    }

    #[test]
    fn open_borders_are_granted_not_shared() {
        let mut d = Diplomacy::new();
        d.attitude_mut(A, B).grants_open_borders = true;
        assert!(d.attitude(A, B).grants_open_borders);
        assert!(!d.attitude(B, A).grants_open_borders, "a one-way grant is normal");
    }

    #[test]
    fn meeting_is_symmetric_and_happens_once() {
        let mut d = Diplomacy::new();
        assert!(!d.have_met(A, B));
        assert!(matches!(d.meet(A, B), Some(DiplomacyEvent::Met { .. })));
        assert!(d.have_met(A, B) && d.have_met(B, A));
        assert_eq!(d.meet(B, A), None, "meeting again is not an event");
    }

    #[test]
    fn declaring_war_reports_who_started_it() {
        let mut d = Diplomacy::new();
        let e = d.declare_war(B, A).unwrap();
        assert_eq!(e, DiplomacyEvent::WarDeclared { aggressor: B, against: A });
        // The blame is in the event; the state is just "these two are at war".
        assert!(d.at_war(A, B));
        assert_eq!(d.declare_war(A, B), None, "already at war");
    }

    #[test]
    fn war_dissolves_a_defensive_pact_between_the_two() {
        let mut d = Diplomacy::new();
        d.meet(A, B);
        Pair::of(A, B).map(|p| d.pacts.get_mut(&p).unwrap().defensive_pact = true);
        assert!(d.pact(A, B).unwrap().defensive_pact);

        d.declare_war(A, B);
        assert!(!d.pact(A, B).unwrap().defensive_pact);
    }

    #[test]
    fn enemies_are_listed_in_id_order() {
        let mut d = Diplomacy::new();
        d.declare_war(C, A);
        d.declare_war(B, A);
        assert_eq!(d.enemies_of(A), vec![B, C]);
        assert_eq!(d.enemies_of(B), vec![A]);
    }

    #[test]
    fn the_clock_advances_every_relationship() {
        let mut d = Diplomacy::new();
        d.meet(A, B);
        d.declare_war(A, C);
        d.tick();
        d.tick();
        assert_eq!(d.pact(A, B).unwrap().turns_in_stance, 2);
        assert_eq!(d.pact(A, C).unwrap().turns_in_stance, 2);
    }
}
