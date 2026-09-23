//! Embassy, friendship and research-pact rules. Signing is available only
//! through the consent boundary in `agreements`.
use crate::ruleset::Ruleset;
use crate::stats::Fixed;
use crate::world::conditionals::{applies, GameContext, Verdict};
use crate::world::diplomacy::{Flag, Pair, Sentiment};
use crate::world::{CivId, TurnEvent, TurnModifiers, World};
use crate::UniqueType;

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResearchAgreement {
    pub turns_left: u32,
    /// Accumulated ordinary science, in canonical Pair slot order.
    pub science: [i32; 2],
}

pub fn can_friend(world: &World, a: CivId, b: CivId) -> bool {
    a != b
        && !world.diplomacy.at_war(a, b)
        && !world.diplomacy.has_flag(a, b, Flag::Denunciation)
        && !world.diplomacy.has_flag(b, a, Flag::Denunciation)
        && world.diplomacy.pact(a, b).is_some_and(|p| p.friendship_turns == 0)
}

pub(crate) fn sign_friendship(world: &mut World, a: CivId, b: CivId) {
    world.diplomacy.pact_mut(a, b).unwrap().friendship_turns = 30;
    for (holder, subject) in [(a, b), (b, a)] {
        world.diplomacy.attitude_mut(holder, subject).set(Sentiment::Friendship, Fixed::whole(35));
    }
}

pub fn can_give_embassy(world: &World, giver: CivId, receiver: CivId) -> bool {
    world
        .civ(giver)
        .is_some_and(|c| c.is_major() && c.uniques.has(UniqueType::EnablesEmbassies) && !c.cities.is_empty())
        && world.civ(receiver).is_some_and(|c| c.is_major())
        && !world.diplomacy.at_war(giver, receiver)
        && world.diplomacy.flag_turns(receiver, giver, Flag::Denunciation) != 30
        && world.diplomacy.flag_turns(giver, receiver, Flag::Denunciation) != 30
        && !world.diplomacy.has_embassy(receiver, giver)
}

pub(crate) fn establish_embassy(world: &mut World, giver: CivId, receiver: CivId) {
    let Some(pair) = Pair::of(giver, receiver) else { return };
    world.diplomacy.pact_mut(giver, receiver).unwrap().embassies[pair.slot(giver).unwrap()] = true;
    for (holder, subject) in [(giver, receiver), (receiver, giver)] {
        let value = i32::from(world.diplomacy.has_embassy(holder, subject)) * 2
            + i32::from(world.diplomacy.has_embassy(subject, holder));
        world.diplomacy.attitude_mut(holder, subject).set(Sentiment::Embassies, Fixed::whole(value));
    }
}

pub fn cost(world: &World, r: &Ruleset, a: CivId, b: CivId, gold_cost: Fixed) -> Option<i32> {
    world.civ(a)?;
    world.civ(b)?;
    let era = crate::world::policies::era_number(world, r, a).max(crate::world::policies::era_number(world, r, b));
    let base = r.eras.values().find(|e| e.era_number == era)?.research_agreement_cost;
    Some((Fixed::whole(base) * gold_cost).floor().max(0) as i32)
}

fn eligible(world: &World, r: &Ruleset, civ: CivId) -> bool {
    world.civ(civ).is_some_and(|c| {
        c.is_major()
            && c.is_alive()
            && c.uniques.has(UniqueType::EnablesResearchAgreements)
            && r.technologies.ids().any(|t| !c.has_tech(t))
    })
}

fn relation_ready(world: &World, r: &Ruleset, a: CivId, b: CivId) -> bool {
    a != b
        && world.diplomacy.have_met(a, b)
        && !world.diplomacy.at_war(a, b)
        && world.diplomacy.pact(a, b).is_some_and(|p| p.friendship_turns > 0 && p.research.is_none())
        && [a, b].into_iter().all(|c| {
            !(r.global_uniques.iter().any(|u| u.unique_type() == Some(UniqueType::RequiresEmbassiesForDiplomacy))
                || world.civ(c).is_some_and(|c| c.uniques.has(UniqueType::RequiresEmbassiesForDiplomacy)))
                || (world.diplomacy.has_embassy(a, b) && world.diplomacy.has_embassy(b, a))
        })
}

/// No recipient treasury, life or technology check during a proposal.
pub fn can_propose(world: &World, r: &Ruleset, a: CivId, b: CivId, mods: TurnModifiers) -> bool {
    relation_ready(world, r, a, b)
        && eligible(world, r, a)
        && mods.costs.deal_duration > 0
        && cost(world, r, a, a, mods.costs.gold_cost).is_some_and(|fee| world.civ(a).is_some_and(|c| c.gold >= fee))
}

pub fn can_sign(world: &World, r: &Ruleset, a: CivId, b: CivId, mods: TurnModifiers) -> bool {
    can_propose(world, r, a, b, mods)
        && eligible(world, r, b)
        && cost(world, r, a, b, mods.costs.gold_cost)
            .is_some_and(|fee| [a, b].into_iter().all(|id| world.civ(id).is_some_and(|c| c.gold >= fee)))
}

pub(crate) fn sign(world: &mut World, r: &Ruleset, a: CivId, b: CivId, mods: TurnModifiers) -> bool {
    if !can_sign(world, r, a, b, mods) {
        return false;
    }
    let fee = cost(world, r, a, b, mods.costs.gold_cost).unwrap();
    world.civ_mut(a).unwrap().gold -= fee;
    world.civ_mut(b).unwrap().gold -= fee;
    world.diplomacy.pact_mut(a, b).unwrap().research =
        Some(ResearchAgreement { turns_left: mods.costs.deal_duration, science: [0, 0] });
    true
}

/// End of the shared round: both parties' ordinary incomes are accumulated
/// once. Expiry pays the lower total to each pending research pool.
pub fn tick(world: &mut World) -> Vec<TurnEvent> {
    let ids: Vec<_> = world.civ_ids().collect();
    let mut events = Vec::new();
    for (i, &a) in ids.iter().enumerate() {
        for &b in &ids[i + 1..] {
            if world.diplomacy.pact(a, b).is_none_or(|p| p.research.is_none()) {
                continue;
            }
            if !world.civ(a).is_some_and(|c| c.is_alive())
                || !world.civ(b).is_some_and(|c| c.is_alive())
                || world.diplomacy.at_war(a, b)
            {
                world.diplomacy.pact_mut(a, b).unwrap().research = None;
                continue;
            }
            let incomes = [a, b].map(|c| world.civ(c).unwrap().recent_science[(world.turn % 8) as usize].max(0));
            let pact = world.diplomacy.pact_mut(a, b).unwrap();
            let research = pact.research.as_mut().unwrap();
            for (total, income) in research.science.iter_mut().zip(incomes) {
                *total = total.saturating_add(income);
            }
            research.turns_left = research.turns_left.saturating_sub(1);
            if research.turns_left == 0 {
                let total = research.science[0].min(research.science[1]);
                pact.research = None;
                for civ in [a, b] {
                    let c = world.civ_mut(civ).unwrap();
                    c.science_from_agreements = c.science_from_agreements.saturating_add(total);
                }
                events.push(TurnEvent::ResearchAgreementCompleted { a, b });
            }
        }
    }
    events
}

pub fn take_science(world: &mut World, r: &Ruleset, civ: CivId) -> i32 {
    let Some(c) = world.civ(civ) else { return 0 };
    if c.researching.is_none() || c.science_from_agreements == 0 {
        return 0;
    }
    let ctx = GameContext::new(r).with_world(world).for_civ(civ);
    let mut modifier = Fixed::permille(500);
    for u in c.uniques.matching(UniqueType::ScienceFromResearchAgreements).filter(|u| applies(u, &ctx) == Verdict::True)
    {
        if let Some(p) = u.params.first().and_then(|n| n.parse::<i64>().ok()) {
            modifier += Fixed::from_fraction(p, 200);
        }
    }
    let grant = (Fixed::whole(c.science_from_agreements / 3) * modifier).floor().max(0) as i32;
    world.civ_mut(civ).unwrap().science_from_agreements = 0;
    grant
}

/// Conservative deterministic diplomacy policy; strategic desirability is
/// separate from the rules and from the other party's consent.
pub fn wants_friendship(world: &World, holder: CivId, other: CivId) -> bool {
    world.diplomacy.attitude(holder, other).opinion() >= Fixed::whole(30)
}

pub fn automate(world: &mut World, r: &Ruleset, mods: TurnModifiers, all_ai: bool) -> Vec<TurnEvent> {
    use crate::world::agreements::{self, Terms};
    use crate::world::trade::Item;
    let ids: Vec<_> =
        world.civs.iter().filter(|c| c.is_major() && c.is_alive() && (all_ai || c.is_ai)).map(|c| c.id).collect();
    let mut events = Vec::new();
    for (i, &a) in ids.iter().enumerate() {
        for &b in &ids[i + 1..] {
            if !world.diplomacy.have_met(a, b) || world.diplomacy.at_war(a, b) {
                continue;
            }
            if can_friend(world, a, b) && wants_friendship(world, a, b) && wants_friendship(world, b, a) {
                if let Ok(out) = agreements::accept(world, r, a, b, &Terms::Friendship, mods) {
                    events.extend(out.events);
                }
            }
            if world.diplomacy.pact(a, b).is_some_and(|p| p.friendship_turns > 0) {
                let give = if can_give_embassy(world, a, b) { vec![Item::Embassy] } else { Vec::new() };
                let get = if can_give_embassy(world, b, a) { vec![Item::Embassy] } else { Vec::new() };
                if !give.is_empty() || !get.is_empty() {
                    if let Ok(out) = agreements::accept(world, r, a, b, &Terms::Trade { give, get }, mods) {
                        events.extend(out.events);
                    }
                }
                if crate::world::defensive_pacts::can_sign(world, r, a, b, mods)
                    && crate::world::defensive_pacts::wants_to_sign(world, r, a, b)
                    && crate::world::defensive_pacts::wants_to_sign(world, r, b, a)
                {
                    if let Ok(out) = agreements::accept(world, r, a, b, &Terms::DefensivePact, mods) {
                        events.extend(out.events);
                    }
                }
                if can_sign(world, r, a, b, mods) {
                    if let Ok(out) = agreements::accept(world, r, a, b, &Terms::ResearchAgreement, mods) {
                        events.extend(out.events);
                    }
                }
            }
        }
    }
    events
}
