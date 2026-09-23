//! Rules for agreements whose two parties have explicitly consented.
//!
//! The session owns offers and consent. This trusted entry point rechecks the
//! current world before atomically committing the agreed terms; it must never
//! be exposed as an unauthenticated unilateral player command.
use crate::ruleset::Ruleset;
use crate::world::commands::{ActionOutcome, CommandRejection};
use crate::world::trade::{self, Item};
use crate::world::{CivId, TurnEvent, TurnModifiers, World};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Terms {
    Peace,
    PeaceTreaty { give: Vec<Item>, get: Vec<Item> },
    Friendship,
    ResearchAgreement,
    DefensivePact,
    Trade { give: Vec<Item>, get: Vec<Item> },
}

/// Checks the proposer and public terms only. A proposal must not reveal the
/// recipient's private treasury, resource balance or recurring income.
pub fn validate_proposal(
    world: &World,
    r: &Ruleset,
    proposer: CivId,
    recipient: CivId,
    terms: &Terms,
    mods: TurnModifiers,
) -> Result<(), CommandRejection> {
    use CommandRejection::NotAllowed;
    if world.winner.is_some()
        || proposer == recipient
        || !world.civ(proposer).is_some_and(|c| c.is_major() && c.is_alive())
        || !world.civ(recipient).is_some_and(|c| c.is_major())
        || !world.diplomacy.have_met(proposer, recipient)
    {
        return Err(NotAllowed);
    }
    match terms {
        Terms::Friendship => {
            if crate::world::research_agreements::can_friend(world, proposer, recipient) {
                Ok(())
            } else {
                Err(NotAllowed)
            }
        }
        Terms::DefensivePact => {
            if crate::world::defensive_pacts::can_propose(world, r, proposer, recipient, mods) {
                Ok(())
            } else {
                Err(NotAllowed)
            }
        }
        Terms::ResearchAgreement => {
            if crate::world::research_agreements::can_propose(world, r, proposer, recipient, mods) {
                Ok(())
            } else {
                Err(NotAllowed)
            }
        }
        Terms::PeaceTreaty { give, get } => {
            let mut peaceful = world.clone();
            if peaceful.diplomacy.make_peace(proposer, recipient).is_none() {
                return Err(NotAllowed);
            }
            trade::validate_proposal(&peaceful, r, proposer, recipient, (give, get), mods).map_err(|_| NotAllowed)
        }
        Terms::Peace if world.diplomacy.at_war(proposer, recipient) => Ok(()),
        Terms::Peace => Err(NotAllowed),
        Terms::Trade { give, get } => {
            trade::validate_proposal(world, r, proposer, recipient, (give, get), mods).map_err(|_| NotAllowed)
        }
    }
}

pub fn accept(
    world: &mut World,
    r: &Ruleset,
    proposer: CivId,
    recipient: CivId,
    terms: &Terms,
    mods: TurnModifiers,
) -> Result<ActionOutcome, CommandRejection> {
    use CommandRejection::NotAllowed;
    if !world.civ(recipient).is_some_and(|c| c.is_alive()) {
        return Err(NotAllowed);
    }
    validate_proposal(world, r, proposer, recipient, terms, mods)?;
    let mut next = world.clone();
    let mut events = match terms {
        Terms::Friendship => {
            crate::world::research_agreements::sign_friendship(&mut next, proposer, recipient);
            vec![TurnEvent::FriendshipSigned { a: proposer, b: recipient }]
        }
        Terms::ResearchAgreement => {
            if !crate::world::research_agreements::sign(&mut next, r, proposer, recipient, mods) {
                return Err(NotAllowed);
            }
            vec![TurnEvent::ResearchAgreementSigned { a: proposer, b: recipient }]
        }
        Terms::DefensivePact => {
            if !crate::world::defensive_pacts::sign(&mut next, r, proposer, recipient, mods) {
                return Err(NotAllowed);
            }
            vec![TurnEvent::DefensivePactSigned { a: proposer, b: recipient }]
        }
        Terms::Peace => {
            let events = crate::world::diplomacy_actions::make_peace(&mut next, r, proposer, recipient);
            if events.is_empty() {
                return Err(NotAllowed);
            }
            events
        }
        Terms::PeaceTreaty { give, get } => {
            let mut events = crate::world::diplomacy_actions::make_peace(&mut next, r, proposer, recipient);
            if events.is_empty() {
                return Err(NotAllowed);
            }
            trade::accept_with(&mut next, r, proposer, recipient, (give, get), mods).map_err(|_| NotAllowed)?;
            events.push(TurnEvent::TradeMade { a: proposer, b: recipient });
            events
        }
        Terms::Trade { give, get } => {
            trade::accept_with(&mut next, r, proposer, recipient, (give, get), mods).map_err(|_| NotAllowed)?;
            vec![TurnEvent::TradeMade { a: proposer, b: recipient }]
        }
    };
    crate::world::commands::finish_action(&mut next, r, &mut events);
    *world = next;
    Ok(ActionOutcome { events })
}
