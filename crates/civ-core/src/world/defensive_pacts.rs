//! Defensive pacts: bilateral consent, treaty lifetime and diplomatic value.
use super::{
    diplomacy::Sentiment,
    opinion::{self, RelationshipLevel},
    CivId, TurnModifiers, World,
};
use crate::{ruleset::Ruleset, stats::Fixed, UniqueType};

fn eligible(world: &World, civ: CivId) -> bool {
    world.civ(civ).is_some_and(|c| c.is_major() && c.is_alive() && c.uniques.has(UniqueType::EnablesDefensivePacts))
}

/// Check only the proposing player's capabilities and public relationship.
/// The recipient's private technology is checked after both parties consent.
pub fn can_propose(world: &World, r: &Ruleset, a: CivId, b: CivId, mods: TurnModifiers) -> bool {
    a != b
        && eligible(world, a)
        && world.civ(b).is_some_and(|c| c.is_major())
        && world.diplomacy.have_met(a, b)
        && !world.diplomacy.at_war(a, b)
        && world.diplomacy.pact(a, b).is_some_and(|p| p.friendship_turns > 0 && !p.defensive_pact)
        && mods.costs.deal_duration > 0
        && (!(r.global_uniques.iter().any(|u| u.unique_type() == Some(UniqueType::RequiresEmbassiesForDiplomacy))
            || world.civ(a).is_some_and(|c| c.uniques.has(UniqueType::RequiresEmbassiesForDiplomacy)))
            || (world.diplomacy.has_embassy(a, b) && world.diplomacy.has_embassy(b, a)))
}

pub fn can_sign(world: &World, r: &Ruleset, a: CivId, b: CivId, mods: TurnModifiers) -> bool {
    can_propose(world, r, a, b, mods) && can_propose(world, r, b, a, mods)
}

pub(crate) fn sign(world: &mut World, r: &Ruleset, a: CivId, b: CivId, mods: TurnModifiers) -> bool {
    if !can_sign(world, r, a, b, mods) {
        return false;
    }
    let pact = world.diplomacy.pact_mut(a, b).unwrap();
    pact.defensive_pact = true;
    // TradeOffer uses Speed.dealDuration, which is not always 30 * speed.
    pact.defensive_pact_turns = mods.costs.deal_duration;
    for (holder, subject) in [(a, b), (b, a)] {
        world.diplomacy.attitude_mut(holder, subject).set(Sentiment::DefensivePact, Fixed::whole(10));
    }
    refresh_opinions(world);
    true
}

/// Expiry and cancellation stop military obligations. The relationship bonus
/// fades naturally, just as the Kotlin modifier outlives its treaty flag.
pub fn cancel(world: &mut World, a: CivId, b: CivId) {
    if let Some(pact) = world.diplomacy.pact_mut(a, b) {
        pact.defensive_pact = false;
        pact.defensive_pact_turns = 0;
    }
}

/// Recompute third-party opinions from publicly known current pacts. Compute
/// from one snapshot so update order cannot affect relationship thresholds.
pub fn refresh_opinions(world: &mut World) {
    let majors: Vec<_> = world.civs.iter().filter(|c| c.is_major()).map(|c| c.id).collect();
    let mut values = Vec::new();
    for &holder in &majors {
        for &subject in &majors {
            if holder == subject || !world.diplomacy.have_met(holder, subject) {
                continue;
            }
            let (mut allies, mut enemies) = (0, 0);
            for &third in &majors {
                if third == holder
                    || third == subject
                    || !world.diplomacy.have_met(holder, third)
                    || !world.diplomacy.have_met(subject, third)
                    || !world.diplomacy.pact(subject, third).is_some_and(|p| p.defensive_pact)
                {
                    continue;
                }
                match opinion::relationship(world, holder, third) {
                    RelationshipLevel::Unforgivable => enemies -= 15,
                    RelationshipLevel::Enemy => enemies -= 10,
                    RelationshipLevel::Friend => allies += 2,
                    RelationshipLevel::Ally => allies += 5,
                    _ => {}
                }
            }
            values.push((holder, subject, allies, enemies));
        }
    }
    for (holder, subject, allies, enemies) in values {
        let attitude = world.diplomacy.attitude_mut(holder, subject);
        for (reason, value) in
            [(Sentiment::DefensivePactWithAllies, allies), (Sentiment::DefensivePactWithEnemies, enemies)]
        {
            if value == 0 {
                attitude.sentiments.remove(&reason);
            } else {
                attitude.set(reason, Fixed::whole(value));
            }
        }
    }
}

/// Deterministic counterpart of DiplomacyAutomation's default-personality
/// willingness: trust, common friends, strength and existing obligations.
pub fn wants_to_sign(world: &World, r: &Ruleset, holder: CivId, other: CivId) -> bool {
    use super::ai::{self, ThreatLevel};
    let attitude = world.diplomacy.attitude(holder, other);
    // DiplomacyAutomation.kt:307: `65 * inverseScaledFocus(Aggressive)`.
    let leader = super::personality::of(world, r, holder);
    if attitude.opinion() < Fixed::whole(65) * leader.inverse(super::personality::Trait::Aggressive) {
        return false;
    }
    let majors: Vec<_> = world.civs.iter().filter(|c| c.is_major() && c.is_alive()).map(|c| c.id).collect();
    let mut pacts = 0;
    let mut extra_pacts = 0;
    let mut wars = 0;
    for &third in &majors {
        if third != holder && world.diplomacy.at_war(holder, third) {
            wars += 1;
        }
        let ours = world.diplomacy.pact(holder, third).is_some_and(|p| p.defensive_pact);
        if ours {
            pacts += 1;
        }
        if world.diplomacy.pact(other, third).is_some_and(|p| p.defensive_pact) && !ours {
            extra_pacts += 1;
        }
        if third == holder
            || third == other
            || !world.diplomacy.have_met(holder, third)
            || !world.diplomacy.have_met(other, third)
        {
            continue;
        }
        if world.diplomacy.pact(holder, third).is_some_and(|p| p.friendship_turns > 0)
            && opinion::relationship(world, third, other) < RelationshipLevel::Favorable
        {
            return false;
        }
        if world.diplomacy.pact(other, third).is_some_and(|p| p.friendship_turns > 0)
            && opinion::relationship(world, third, holder) < RelationshipLevel::Neutral
        {
            return false;
        }
    }
    let threat = match ai::threat(world, r, holder, other) {
        ThreatLevel::VeryHigh => 10,
        ThreatLevel::High => 5,
        ThreatLevel::Low => -3,
        ThreatLevel::VeryLow => -7,
        ThreatLevel::Medium => 0,
    };
    let others = majors.len().saturating_sub(1) as i32;
    let ideal = Fixed::whole(others) * Fixed::permille(200);
    let saturation = if others > 0 {
        (Fixed::from_fraction(40 * (Fixed::whole(pacts) - ideal).raw(), (Fixed::whole(others) - ideal).raw()))
            .max(Fixed::ZERO)
    } else {
        Fixed::ZERO
    };
    attitude.opinion() - Fixed::whole(80)
        + attitude.get(Sentiment::WarMongerer)
        + Fixed::whole(threat + 5 * wars - 15 * extra_pacts)
        - saturation
        > Fixed::ZERO
}
