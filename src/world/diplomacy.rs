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
pub use crate::world::promises::Demand;

/// The floor on city-state influence. Below this there is nothing further to
/// lose, which is what stops a long war from making a grudge permanent.
pub const MINIMUM_INFLUENCE: Fixed = Fixed::permille(-60_000);
use crate::world::grid::CivId;
use std::collections::BTreeMap;

/// An unordered pair of distinct civs, stored in a canonical order.
///
/// The constructor is the only way to build one, so `Pair::of(a, b)` and
/// `Pair::of(b, a)` are the same key and cannot drift apart.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
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

    /// Which slot a civ occupies, 0 for the lower id. Directed facts about a
    /// pair index by this, so they still live in the one pair record.
    pub fn slot(self, civ: CivId) -> Option<usize> {
        if civ == self.0 {
            Some(0)
        } else if civ == self.1 {
            Some(1)
        } else {
            None
        }
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

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Stance {
    /// Two civs who have met and are not fighting.
    #[default]
    Peace,
    War,
}

/// What is true of the pair.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Pact {
    pub stance: Stance,
    pub defensive_pact: bool,
    pub defensive_pact_turns: u32,
    pub embassies: [bool; 2],
    pub friendship_turns: u32,
    pub peace_treaty_turns: u32,
    pub research: Option<crate::world::research_agreements::ResearchAgreement>,
    /// Who lets whom walk through. Indexed by [`Pair::slot`] of the *grantor*:
    /// `open_borders[0]` is the lower-id civ opening its borders to the other.
    ///
    /// Open borders is the one genuinely one-sided thing about a pair — you
    /// can be let in without letting them in. Storing it as two bits in the
    /// one record keeps that asymmetry honest without splitting the pair into
    /// two records that could disagree about the war.
    pub open_borders: [bool; 2],
    /// Who has pledged to protect whom, indexed by [`Pair::slot`] of the
    /// *protector*. Only meaningful when one side is a city-state, but it
    /// costs nothing to carry and keeps every directed fact about a pair in
    /// the one record.
    pub protector: [bool; 2],
    /// Turns since the stance last changed — peace treaties have a duration.
    pub turns_in_stance: u32,
}

/// A named reason one civ feels a certain way about another
/// (`DiplomaticModifiers`). Each carries a signed amount; see [`Attitude`].
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Sentiment {
    // Grievances.
    DeclaredWarOnUs,
    WarMongerer,
    UsedNuclearWeapons,
    CapturedOurCities,
    Denunciation,
    DenouncedOurAllies,
    BulliedProtectedMinor,
    SpiedOnUs,
    /// Held by whoever was made to promise, against whoever made them.
    UnacceptableDemands,
    RefusedDemand(Demand),
    BetrayedPromise(Demand),
    /// Held by a bystander who watched a promise not to attack broken.
    BetrayedPromiseToOthers,
    BetrayedFriendship,
    LiberatedCity,
    GaveUsUnits,
    // Warmth.
    Friendship,
    DefensivePact,
    BetrayedDefensivePact,
    DefensivePactWithAllies,
    DefensivePactWithEnemies,
    Embassies,
    DenouncedOurEnemies,
    YearsOfPeace,
    SharedEnemy,
    OpenBorders,
    FulfilledPromise(Demand),
}

/// What one civ thinks of another. Not necessarily mutual.
///
/// Kept as the reasons rather than a running total, as Unciv keeps them: each
/// fades at its own pace — a declaration of war over a hundred and sixty
/// turns, warmongering in forty — and a total cannot remember which part of
/// itself is which.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Attitude {
    #[cfg_attr(feature = "serde", serde(with = "crate::serialization::map_as_entries"))]
    pub sentiments: BTreeMap<Sentiment, Fixed>,
    /// The opinion averaged over the last thirty turns or so, lagging one
    /// turn behind (`smoothedOpinionOfOtherCiv`). How far the opinion has
    /// fallen below it is what makes an AI denounce.
    pub smoothed: Fixed,
    /// Next turn's `smoothed`, already computed.
    pub smoothed_next: Fixed,
}

impl Attitude {
    /// The sum of every reason (`opinionOfOtherCiv`). Positive is warmth.
    pub fn opinion(&self) -> Fixed {
        self.sentiments.values().copied().sum()
    }

    pub fn get(&self, s: Sentiment) -> Fixed {
        self.sentiments.get(&s).copied().unwrap_or(Fixed::ZERO)
    }

    pub fn has(&self, s: Sentiment) -> bool {
        self.sentiments.contains_key(&s)
    }

    pub fn set(&mut self, s: Sentiment, amount: Fixed) {
        self.sentiments.insert(s, amount);
    }

    /// Add to a reason, held within `limit` if one is given (`addModifier`).
    pub fn add(&mut self, s: Sentiment, amount: Fixed, limit: Option<Fixed>) {
        let mut v = self.get(s) + amount;
        if let Some(limit) = limit {
            v = if limit < Fixed::ZERO { v.max(limit) } else { v.min(limit) };
        }
        self.sentiments.insert(s, v);
    }

    /// Fade a reason toward nothing by `step`, dropping it once it would pass
    /// zero (`revertToZero`).
    pub fn fade(&mut self, s: Sentiment, step: Fixed) {
        let Some(v) = self.sentiments.get(&s).copied() else { return };
        if step >= if v < Fixed::ZERO { -v } else { v } {
            self.sentiments.remove(&s);
        } else if v > Fixed::ZERO {
            self.sentiments.insert(s, v - step);
        } else {
            self.sentiments.insert(s, v + step);
        }
    }

    /// Grow a reason by `step`, never past `max` (`accumulateToAtMost`).
    pub fn grow(&mut self, s: Sentiment, step: Fixed, max: Option<Fixed>) {
        let v = self.get(s);
        match max {
            Some(max) if v >= max => {}
            Some(max) if step >= max - v => self.set(s, max),
            _ => self.add(s, step, None),
        }
    }
}

// `Attitude` used to carry `grants_open_borders` and `influence` as well.
// Both were superseded — open borders by the directed slots on `Pact`, and
// influence by the map keyed `(city_state, major)` — and neither was ever read
// again. Two fields holding the same fact in two places is the one thing this
// module exists to prevent, so the dead ones are gone rather than left as a
// trap for whoever writes to them next.

/// Every relationship in the game.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Diplomacy {
    /// Symmetric facts. One entry per pair, never two.
    #[cfg_attr(feature = "serde", serde(with = "crate::serialization::map_as_entries"))]
    pacts: BTreeMap<Pair, Pact>,
    /// Directed facts, keyed `(holder, subject)`.
    #[cfg_attr(feature = "serde", serde(with = "crate::serialization::map_as_entries"))]
    attitudes: BTreeMap<(CivId, CivId), Attitude>,
    /// Who has met whom. Symmetric, so also keyed by pair.
    #[cfg_attr(feature = "serde", serde(with = "crate::serialization::map_as_entries"))]
    met: BTreeMap<Pair, bool>,
    /// Permanently at war with everyone.
    always_hostile: std::collections::BTreeSet<CivId>,
    /// How much a major civ has bought, fought or bribed its way into a
    /// city-state's favour. Directed and *not* symmetric — the city-state's
    /// opinion of you is not your opinion of it — and keyed
    /// `(city_state, major)`, which is the direction Unciv stores it in too.
    #[cfg_attr(feature = "serde", serde(with = "crate::serialization::map_as_entries"))]
    influence: BTreeMap<(CivId, CivId), Fixed>,
    /// Directed countdowns, keyed `(holder, subject)`.
    #[cfg_attr(feature = "serde", serde(with = "crate::serialization::nested_map_as_entries"))]
    flags: BTreeMap<(CivId, CivId), BTreeMap<Flag, u32>>,
    /// Trades in force; see `trade`.
    pub deals: Vec<crate::world::trade::Deal>,
    /// Demands made and not yet answered, keyed `(demander, asked)`; see
    /// `promises`.
    pub demands: std::collections::BTreeSet<(CivId, CivId, Demand)>,
}

/// A countdown on a directed relation (`DiplomacyFlags`): in force while
/// turns remain, gone when they run out.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Flag {
    RecentlyPledgedProtection,
    RecentlyWithdrewProtection,
    /// The holder has publicly denounced the subject.
    Denunciation,
    /// The holder, a city-state, paid the subject tribute — remembered for
    /// quests, and to hand the subject none.
    Bullied,
    /// The holder protects a city-state the subject bullied.
    RememberBulliedProtectedMinor,
    /// The subject promised the holder this. Kept to the end, it earns warmth.
    Agreed(Demand),
    /// The holder caught the subject doing what this demand forbids.
    Violated(Demand),
    /// The holder has given up asking — refused once, or already betrayed.
    IgnoreViolation(Demand),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiplomacyEvent {
    Met { pair: Pair },
    WarDeclared { aggressor: CivId, against: CivId },
    PeaceMade { pair: Pair },
}

impl Diplomacy {
    /// Resurrection starts both sides of the restored civ's relations afresh.
    pub fn revive(&mut self, civ: CivId) {
        self.end_agreements_for(civ);
        self.flags.retain(|(a, b), _| *a != civ && *b != civ);
        self.attitudes.retain(|(a, b), _| *a != civ && *b != civ);
        for (pair, pact) in &mut self.pacts {
            if pair.slot(civ).is_some() {
                pact.stance = Stance::Peace;
                pact.turns_in_stance = 0;
            }
        }
    }
    /// A defeated civilization cannot keep exchanging resources, granting
    /// passage or making unanswered demands. Knowledge and opinions survive.
    pub fn end_agreements_for(&mut self, civ: CivId) {
        self.deals.retain(|deal| deal.a != civ && deal.b != civ);
        self.demands.retain(|(a, b, _)| *a != civ && *b != civ);
        for (pair, pact) in &mut self.pacts {
            if pair.slot(civ).is_some() {
                pact.open_borders = [false, false];
                pact.defensive_pact = false;
                pact.defensive_pact_turns = 0;
                pact.protector = [false, false];
                pact.embassies = [false, false];
                pact.friendship_turns = 0;
                pact.peace_treaty_turns = 0;
                pact.research = None;
            }
        }
    }

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

    /// Civs who are permanently hostile — barbarians. Held here rather than
    /// checked through the world, so `at_war` stays a pure lookup.
    pub fn mark_hostile(&mut self, civ: CivId) {
        self.always_hostile.insert(civ);
    }

    pub fn is_hostile(&self, civ: CivId) -> bool {
        self.always_hostile.contains(&civ)
    }

    /// The stance between two civs. The same answer whichever way it is asked,
    /// because there is only one place to read it from.
    pub fn stance(&self, a: CivId, b: CivId) -> Stance {
        // Barbarians are at war with everyone and cannot be reasoned with, so
        // there is no pact to consult.
        if self.always_hostile.contains(&a) || self.always_hostile.contains(&b) {
            return Stance::War;
        }
        Pair::of(a, b).and_then(|p| self.pacts.get(&p)).map_or(Stance::Peace, |p| p.stance)
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
        let betrayed = pact.friendship_turns > 0;
        let betrayed_defensive = pact.defensive_pact;
        pact.friendship_turns = 0;
        pact.embassies = [false, false];
        pact.research = None;
        pact.stance = Stance::War;
        pact.peace_treaty_turns = 0;
        pact.defensive_pact = false; // war ends any pact between the two
        pact.defensive_pact_turns = 0;
        pact.open_borders = [false, false]; // and closes both sets of borders
        pact.protector = [false, false]; // you do not protect what you invade
        pact.turns_in_stance = 0;
        for (holder, subject) in [(aggressor, against), (against, aggressor)] {
            self.attitude_mut(holder, subject).sentiments.remove(&Sentiment::Friendship);
            self.attitude_mut(holder, subject).sentiments.remove(&Sentiment::Embassies);
            self.attitude_mut(holder, subject).sentiments.remove(&Sentiment::DefensivePact);
        }
        if betrayed {
            let witnesses: Vec<_> = self.met.keys().filter_map(|p| p.other(aggressor)).collect();
            for witness in witnesses {
                self.attitude_mut(witness, aggressor).grow(
                    Sentiment::BetrayedFriendship,
                    Fixed::whole(if witness == against { -40 } else { -20 }),
                    None,
                );
            }
        }
        if betrayed_defensive {
            let witnesses: Vec<_> = self.met.keys().filter_map(|p| p.other(aggressor)).collect();
            for witness in witnesses {
                self.attitude_mut(witness, aggressor).grow(
                    Sentiment::BetrayedDefensivePact,
                    Fixed::whole(if witness == against { -20 } else { -10 }),
                    None,
                );
            }
        }
        self.met.insert(pair, true); // you have certainly met now
        Some(DiplomacyEvent::WarDeclared { aggressor, against })
    }

    /// An initial peaceful relationship does not imply a signed truce.
    pub fn can_declare_war(&self, a: CivId, b: CivId) -> bool {
        a != b && !self.at_war(a, b) && self.pact(a, b).is_none_or(|p| p.peace_treaty_turns == 0)
    }

    pub fn make_peace(&mut self, a: CivId, b: CivId) -> Option<DiplomacyEvent> {
        // No treaty with barbarians.
        if self.always_hostile.contains(&a) || self.always_hostile.contains(&b) {
            return None;
        }
        let pair = Pair::of(a, b)?;
        let pact = self.pacts.entry(pair).or_default();
        if pact.stance == Stance::Peace {
            return None;
        }
        pact.stance = Stance::Peace;
        pact.peace_treaty_turns = 10;
        pact.turns_in_stance = 0;
        Some(DiplomacyEvent::PeaceMade { pair })
    }

    /// May `visitor` pass through `grantor`'s territory?
    ///
    /// Own territory always, war never, and otherwise only if it was granted.
    pub fn may_enter_borders_of(&self, visitor: CivId, grantor: CivId) -> bool {
        if visitor == grantor {
            return true;
        }
        if self.always_hostile.contains(&visitor) || self.always_hostile.contains(&grantor) {
            return false;
        }
        let Some(pair) = Pair::of(visitor, grantor) else { return true };
        if !self.met.get(&pair).copied().unwrap_or(false) {
            return false;
        }
        let Some(pact) = self.pacts.get(&pair) else { return false };
        if pact.stance == Stance::War {
            return false;
        }
        pair.slot(grantor).is_some_and(|i| pact.open_borders[i])
    }

    /// Open (or close) `grantor`'s borders to `visitor`. One-sided: the
    /// reverse direction is a separate call.
    pub fn set_open_borders(&mut self, grantor: CivId, visitor: CivId, open: bool) {
        let Some(pair) = Pair::of(grantor, visitor) else { return };
        let Some(slot) = pair.slot(grantor) else { return };
        self.pacts.entry(pair).or_default().open_borders[slot] = open;
    }

    pub fn pact(&self, a: CivId, b: CivId) -> Option<&Pact> {
        Pair::of(a, b).and_then(|p| self.pacts.get(&p))
    }

    pub(crate) fn pact_mut(&mut self, a: CivId, b: CivId) -> Option<&mut Pact> {
        Some(self.pacts.entry(Pair::of(a, b)?).or_default())
    }

    /// The host granted this visitor an embassy in the host's capital.
    pub fn has_embassy(&self, visitor: CivId, host: CivId) -> bool {
        Pair::of(visitor, host)
            .is_some_and(|pair| self.pacts.get(&pair).is_some_and(|p| p.embassies[pair.slot(host).unwrap()]))
    }

    /// Has `major` pledged to protect `city_state`?
    ///
    /// Directed, and stored on the pair record's second directed slot rather
    /// than in a set of its own: protection is exactly as one-sided as open
    /// borders and belongs beside it.
    pub fn is_protector(&self, major: CivId, city_state: CivId) -> bool {
        let Some(pair) = Pair::of(major, city_state) else { return false };
        self.pacts.get(&pair).zip(pair.slot(major)).is_some_and(|(pact, slot)| pact.protector[slot])
    }

    /// Pledge, or withdraw, protection.
    pub fn set_protector(&mut self, major: CivId, city_state: CivId, on: bool) {
        let Some(pair) = Pair::of(major, city_state) else { return };
        let Some(slot) = pair.slot(major) else { return };
        self.pacts.entry(pair).or_default().protector[slot] = on;
    }

    /// How much sway `major` holds over `city_state`.
    pub fn influence(&self, city_state: CivId, major: CivId) -> Fixed {
        self.influence.get(&(city_state, major)).copied().unwrap_or(Fixed::ZERO)
    }

    /// Set it, floored — influence has a bottom, so a city-state you have
    /// wronged beyond measure cannot be wronged further.
    pub fn set_influence(&mut self, city_state: CivId, major: CivId, amount: Fixed) {
        self.influence.insert((city_state, major), amount.max(MINIMUM_INFLUENCE));
    }

    pub fn add_influence(&mut self, city_state: CivId, major: CivId, amount: Fixed) {
        let now = self.influence(city_state, major);
        self.set_influence(city_state, major, now + amount);
    }

    /// Everyone who holds any opinion of this city-state, in id order.
    pub fn influencers(&self, city_state: CivId) -> Vec<(CivId, Fixed)> {
        self.influence.iter().filter(|((cs, _), _)| *cs == city_state).map(|((_, major), v)| (*major, *v)).collect()
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
    ///
    /// Returns the countdowns that ran out, `(holder, subject, flag)`, in key
    /// order — some of them mean something when they end rather than while
    /// they run. One cleared early is not reported.
    pub fn tick(&mut self) -> Vec<(CivId, CivId, Flag)> {
        for pact in self.pacts.values_mut() {
            pact.turns_in_stance = pact.turns_in_stance.saturating_add(1);
            pact.friendship_turns = pact.friendship_turns.saturating_sub(1);
            pact.peace_treaty_turns = pact.peace_treaty_turns.saturating_sub(1);
            pact.defensive_pact_turns = pact.defensive_pact_turns.saturating_sub(1);
            if pact.defensive_pact_turns == 0 {
                pact.defensive_pact = false;
            }
        }
        let mut expired = Vec::new();
        for (&(holder, subject), flags) in self.flags.iter_mut() {
            for (&flag, turns) in flags.iter_mut() {
                *turns = turns.saturating_sub(1);
                if *turns == 0 {
                    expired.push((holder, subject, flag));
                }
            }
            flags.retain(|_, turns| *turns > 0);
        }
        self.flags.retain(|_, flags| !flags.is_empty());
        expired
    }

    /// Start (or restart) a countdown on `holder`'s relation to `subject`.
    pub fn set_flag(&mut self, holder: CivId, subject: CivId, flag: Flag, turns: u32) {
        if turns == 0 {
            self.clear_flag(holder, subject, flag);
        } else {
            self.flags.entry((holder, subject)).or_default().insert(flag, turns);
        }
    }

    pub fn clear_flag(&mut self, holder: CivId, subject: CivId, flag: Flag) {
        if let Some(f) = self.flags.get_mut(&(holder, subject)) {
            f.remove(&flag);
        }
    }

    pub fn has_flag(&self, holder: CivId, subject: CivId, flag: Flag) -> bool {
        self.flag_turns(holder, subject, flag) > 0
    }

    /// Turns left on a countdown, zero if it is not running.
    pub fn flag_turns(&self, holder: CivId, subject: CivId, flag: Flag) -> u32 {
        self.flags.get(&(holder, subject)).and_then(|f| f.get(&flag)).copied().unwrap_or(0)
    }

    /// Every civ this one is at war with, in id order.
    pub fn enemies_of(&self, civ: CivId) -> Vec<CivId> {
        self.pacts.iter().filter(|(_, p)| p.stance == Stance::War).filter_map(|(pair, _)| pair.other(civ)).collect()
    }

    pub fn pair_count(&self) -> usize {
        self.pacts.len()
    }
}

#[cfg(feature = "save")]
impl Diplomacy {
    pub(crate) fn validate_snapshot(&self, civs: usize) -> Result<(), String> {
        let valid = |c: CivId| (c.0 as usize) < civs;
        let directed = |(a, b): &(CivId, CivId)| valid(*a) && valid(*b) && a != b;
        if self.pacts.keys().chain(self.met.keys()).any(|Pair(a, b)| !valid(*a) || !valid(*b) || a >= b)
            || self.attitudes.keys().chain(self.influence.keys()).chain(self.flags.keys()).any(|p| !directed(p))
            || self.always_hostile.iter().any(|c| !valid(*c))
            || self.demands.iter().any(|(a, b, _)| !directed(&(*a, *b)))
        {
            return Err("diplomacy references an invalid civilization pair".into());
        }
        if self.pacts.values().any(|p| {
            p.friendship_turns > 30
                || p.defensive_pact_turns > 1_000_000
                || p.defensive_pact != (p.defensive_pact_turns > 0)
                || (p.stance == Stance::War && p.defensive_pact)
                || p.peace_treaty_turns > 1_000_000
                || (p.stance == Stance::War && p.peace_treaty_turns > 0)
                || (p.stance == Stance::War
                    && (p.friendship_turns > 0 || p.embassies != [false, false] || p.research.is_some()))
                || p.research.as_ref().is_some_and(|ra| {
                    ra.turns_left == 0
                        || ra.turns_left > 1_000_000
                        || ra.science.iter().any(|n| !(0..=1_000_000_000).contains(n))
                })
        }) {
            return Err("invalid embassy, friendship or research agreement".into());
        }
        const MAX: i64 = 1_000_000_000_000;
        if self.influence.values().any(|v| !(-MAX..=MAX).contains(&v.raw()))
            || self.attitudes.values().any(|a| {
                a.sentiments.values().chain([&a.smoothed, &a.smoothed_next]).any(|v| !(-MAX..=MAX).contains(&v.raw()))
            })
        {
            return Err("diplomacy value is outside snapshot bounds".into());
        }
        Ok(())
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
        d.attitude_mut(A, B).set(Sentiment::YearsOfPeace, Fixed::whole(40));
        d.attitude_mut(B, A).set(Sentiment::DeclaredWarOnUs, Fixed::whole(-30));

        assert_eq!(d.attitude(A, B).opinion(), Fixed::whole(40));
        assert_eq!(d.attitude(B, A).opinion(), Fixed::whole(-30), "resentment need not be mutual");
    }

    #[test]
    fn a_reason_fades_to_nothing_and_is_forgotten() {
        let mut a = Attitude::default();
        a.set(Sentiment::WarMongerer, Fixed::whole(-1));
        a.fade(Sentiment::WarMongerer, Fixed::permille(400));
        assert_eq!(a.get(Sentiment::WarMongerer), Fixed::permille(-600));
        a.fade(Sentiment::WarMongerer, Fixed::permille(700));
        assert!(!a.has(Sentiment::WarMongerer), "past zero is gone, not flipped");

        a.grow(Sentiment::YearsOfPeace, Fixed::whole(25), Some(Fixed::whole(30)));
        a.grow(Sentiment::YearsOfPeace, Fixed::whole(25), Some(Fixed::whole(30)));
        assert_eq!(a.get(Sentiment::YearsOfPeace), Fixed::whole(30), "and warmth stops at its cap");
    }

    #[test]
    fn open_borders_are_granted_not_shared() {
        let mut d = Diplomacy::new();
        d.meet(A, B); // you cannot open your borders to someone you have not met
        d.set_open_borders(A, B, true);
        assert!(d.may_enter_borders_of(B, A));
        assert!(!d.may_enter_borders_of(A, B), "a one-way grant is normal");
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
        if let Some(p) = Pair::of(A, B) {
            d.pacts.get_mut(&p).unwrap().defensive_pact = true;
        }
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
