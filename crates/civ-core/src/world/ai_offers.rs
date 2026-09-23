//! What an AI proposes to a human (`DiplomacyAutomation.kt`, `TradeAutomation.kt`).
//!
//! Between AIs, agreements both sides want are simply signed
//! ([`crate::world::research_agreements::automate`]). A human has to be asked,
//! and can say no, so this module only chooses the question: at most one
//! offer per AI per human per turn, the most pressing first. The session
//! delivers it to the human's inbox and records a refusal with
//! [`declined`], after which the AI waits before asking the same again.
//!
//! What the AI reads is its own relations and the public state of the pair.
//! Two places read the human's side as Unciv's AI does: military strength for
//! peace (`getStatForRanking(Force)` in `wants_peace`) and which luxuries the
//! human has spare (`TradeLogic.theirAvailableOffers` in
//! `TradeAutomation.potentialLuxuryTrades`). Neither reaches the human: the
//! offer shows only the human's own resources.

use crate::arena::Id;
use crate::rng::Rng;
use crate::ruleset::{Ruleset, TileResource};
use crate::world::agreements::Terms;
use crate::world::diplomacy::{Flag, Proposal};
use crate::world::grid::CivId;
use crate::world::opinion::{relationship, RelationshipLevel};
use crate::world::research_agreements as ra;
use crate::world::trade::Item;
use crate::world::turn::TurnModifiers;
use crate::world::World;

/// Turns the AI waits after a human says no (Trade.kt:60-77 and
/// AlertPopup.kt:268, at the default personality of 5).
pub fn refusal_turns(kind: Proposal) -> u32 {
    match kind {
        Proposal::Friendship => 20,
        Proposal::ResearchAgreement => 10,
        Proposal::DefensivePact | Proposal::OpenBorders | Proposal::Embassy => 10,
        Proposal::LuxuryExchange => 3,
        Proposal::Peace => 3,
    }
}

/// The kind of an offer, for its refusal wait.
pub fn kind(terms: &Terms) -> Proposal {
    match terms {
        Terms::Peace | Terms::PeaceTreaty { .. } => Proposal::Peace,
        Terms::Friendship => Proposal::Friendship,
        Terms::ResearchAgreement => Proposal::ResearchAgreement,
        Terms::DefensivePact => Proposal::DefensivePact,
        Terms::Trade { give, get } => {
            if give.iter().chain(get).any(|i| matches!(i, Item::OpenBorders)) {
                Proposal::OpenBorders
            } else if give.iter().chain(get).any(|i| matches!(i, Item::Embassy)) {
                Proposal::Embassy
            } else {
                Proposal::LuxuryExchange
            }
        }
    }
}

/// The human turned it down, or let it lapse: wait before asking again.
pub fn declined(world: &mut World, ai: CivId, human: CivId, terms: &Terms) {
    let kind = kind(terms);
    world.diplomacy.set_flag(ai, human, Flag::Declined(kind), refusal_turns(kind));
}

fn waiting(world: &World, ai: CivId, human: CivId, kind: Proposal) -> bool {
    world.diplomacy.has_flag(ai, human, Flag::Declined(kind))
}

/// Unciv rolls `(1..10).random()` against a threshold with a seed from the
/// turn and both civs (`DiplomacyAutomation.getRandom`).
fn roll(world: &World, ai: CivId, human: CivId, context: u64) -> i64 {
    Rng::from_parts(&[0xD1_9105, context, world.turn as u64, ai.0 as u64, human.0 as u64]).range(1, 10)
}

/// The one offer `ai` makes `human` this turn, if any.
pub fn offer_to_human(world: &World, r: &Ruleset, ai: CivId, human: CivId, mods: TurnModifiers) -> Option<Terms> {
    let (a, h) = (world.civ(ai)?, world.civ(human)?);
    if !a.is_ai || !a.is_major() || !a.is_alive() || a.cities.is_empty() || h.is_ai || !h.is_major() || !h.is_alive()
    {
        return None;
    }
    if !world.diplomacy.have_met(ai, human) {
        return None;
    }
    // Peace first (`offerPeaceTreaty`): only from an AI that wants it.
    if world.diplomacy.at_war(ai, human) {
        let wants = crate::world::ai::wants_peace(world, r, ai, human);
        return (wants && !waiting(world, ai, human, Proposal::Peace)).then_some(Terms::Peace);
    }
    let level = relationship(world, ai, human);
    // Declaration of friendship: two in ten turns for a civ that wants one.
    if ra::can_friend(world, ai, human)
        && !waiting(world, ai, human, Proposal::Friendship)
        && ra::wants_friendship(world, ai, human)
        && level >= RelationshipLevel::Favorable
        && roll(world, ai, human, 1) <= 2
    {
        return Some(Terms::Friendship);
    }
    // Embassies, mutual (`offerToEstablishEmbassy`): three in ten.
    if !waiting(world, ai, human, Proposal::Embassy)
        && level >= RelationshipLevel::Neutral
        && ra::can_give_embassy(world, ai, human)
        && ra::can_give_embassy(world, human, ai)
        && roll(world, ai, human, 2) >= 7
    {
        return Some(Terms::Trade { give: vec![Item::Embassy], get: vec![Item::Embassy] });
    }
    // Open borders both ways (`offerOpenBorders`): three in ten, from a civ
    // that thinks well of the human.
    let both_may = a.uniques.has(crate::UniqueType::EnablesOpenBorders)
        && h.uniques.has(crate::UniqueType::EnablesOpenBorders);
    let open = world.diplomacy.may_enter_borders_of(ai, human) || world.diplomacy.may_enter_borders_of(human, ai);
    if both_may
        && !open
        && !waiting(world, ai, human, Proposal::OpenBorders)
        && level >= RelationshipLevel::Favorable
        && roll(world, ai, human, 3) >= 7
    {
        return Some(Terms::Trade { give: vec![Item::OpenBorders], get: vec![Item::OpenBorders] });
    }
    // Research agreements are always offered when the AI can sign one
    // (`offerResearchAgreement`). Only the AI's own treasury is checked here;
    // the human's is checked when they accept.
    if !waiting(world, ai, human, Proposal::ResearchAgreement) && ra::can_propose(world, r, ai, human, mods) {
        return Some(Terms::ResearchAgreement);
    }
    if !waiting(world, ai, human, Proposal::DefensivePact)
        && crate::world::defensive_pacts::can_propose(world, r, ai, human, mods)
        && crate::world::defensive_pacts::wants_to_sign(world, r, ai, human)
        && roll(world, ai, human, 4) > 7
    {
        return Some(Terms::DefensivePact);
    }
    // A spare luxury for one the AI lacks (`exchangeLuxuries`), with anyone not an enemy.
    if level > RelationshipLevel::Enemy && !waiting(world, ai, human, Proposal::LuxuryExchange) {
        if let Some((give, get)) = luxury_swap(world, r, ai, human) {
            return Some(Terms::Trade { give: vec![Item::Luxury(give)], get: vec![Item::Luxury(get)] });
        }
    }
    None
}

fn luxury_swap(world: &World, r: &Ruleset, ai: CivId, human: CivId) -> Option<(Id<TileResource>, Id<TileResource>)> {
    let luxury = |id: &Id<TileResource>| r.resources.get(*id).is_some_and(|x| x.resource_type == "Luxury");
    let held = |civ: CivId, res: Id<TileResource>| {
        world.civ(civ).and_then(|c| c.resources.get(&res)).copied().unwrap_or(0)
    };
    let (a, h) = (world.civ(ai)?, world.civ(human)?);
    let give = a.resources.keys().copied().find(|res| luxury(res) && held(ai, *res) > 1 && held(human, *res) == 0)?;
    let get = h.resources.keys().copied().find(|res| luxury(res) && held(human, *res) > 1 && held(ai, *res) == 0)?;
    Some((give, get))
}
