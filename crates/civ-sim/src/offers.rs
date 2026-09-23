//! Private, immutable offers. This owns consent and expiry; core owns legality.
use crate::protocol::{Action, AgreementTerms, CommandRequest, DiplomaticOffer, RejectionCode, TradeItem};
use civ_core::ruleset::Ruleset;
use civ_core::world::{agreements, CivId, TurnEvent, TurnModifiers, World};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Offers(Vec<DiplomaticOffer>);

/// An AI's offer to a human. Its id names the AI and the turn, and only an AI
/// civilization can have made one; a human's offers are keyed by their
/// command and backed by its recorded consent.
pub(crate) fn is_ai_offer(world: &World, offer: &DiplomaticOffer) -> bool {
    offer.id == ai_offer_id(offer.from, offer.turn)
        && world.civ(CivId(offer.from)).is_some_and(|c| c.is_ai && c.is_major())
}

pub(crate) fn ai_offer_id(from: u8, turn: u32) -> String {
    format!("{from}:ai:{turn}")
}

/// Core terms as the protocol states them, for the kinds an AI offers.
fn protocol_terms(terms: &agreements::Terms) -> Option<AgreementTerms> {
    use civ_core::world::trade::Item;
    let item = |i: &Item| {
        Some(match i {
            Item::Luxury(id) => TradeItem::Luxury { resource: id.index() },
            Item::Embassy => TradeItem::Embassy,
            Item::OpenBorders => TradeItem::OpenBorders,
            _ => return None,
        })
    };
    Some(match terms {
        agreements::Terms::Peace => AgreementTerms::Peace,
        agreements::Terms::Friendship => AgreementTerms::Friendship,
        agreements::Terms::ResearchAgreement => AgreementTerms::ResearchAgreement,
        agreements::Terms::DefensivePact => AgreementTerms::DefensivePact,
        agreements::Terms::Trade { give, get } => AgreementTerms::Trade {
            give: give.iter().map(item).collect::<Option<_>>()?,
            get: get.iter().map(item).collect::<Option<_>>()?,
        },
        agreements::Terms::PeaceTreaty { .. } => return None,
    })
}

impl Offers {
    /// Each AI's one question to each human this turn (`ai_offers`). Runs
    /// after the round closes and expired offers are gone.
    pub fn propose_from_ais(&mut self, world: &World, r: &Ruleset, humans: &[u8]) {
        let mods = TurnModifiers::default();
        let ais: Vec<u8> = world
            .civs
            .iter()
            .filter(|c| c.is_ai && c.is_major() && c.is_alive())
            .map(|c| c.id.0)
            .collect();
        for &ai in &ais {
            for &human in humans {
                let Some(terms) =
                    civ_core::world::ai_offers::offer_to_human(world, r, CivId(ai), CivId(human), mods)
                else {
                    continue;
                };
                let Some(terms) = protocol_terms(&terms) else { continue };
                let offer = DiplomaticOffer { id: ai_offer_id(ai, world.turn), from: ai, to: human, turn: world.turn, terms };
                // Validation is the same as for a human's proposal; a refusal here just means no offer.
                let _ = self.propose(world, r, offer);
            }
        }
    }

    /// Close the turn's offers. An AI whose offer lapsed unanswered takes it
    /// as a refusal and waits before asking again.
    pub fn expire(&mut self, world: &mut World, r: &Ruleset) {
        let lapsed: Vec<_> =
            self.0.iter().filter(|o| o.turn != world.turn && is_ai_offer(world, o)).cloned().collect();
        for offer in lapsed {
            if let Ok(terms) = core_terms(r, &offer.terms) {
                civ_core::world::ai_offers::declined(world, CivId(offer.from), CivId(offer.to), &terms);
            }
        }
        self.prune(world);
    }

    /// A matching original proposal is insufficient: later successful input
    /// can withdraw, replace, decline or consume that consent.
    pub fn consent_is_active<'a>(
        &self,
        commands: impl Iterator<Item = (u8, &'a CommandRequest)>,
    ) -> bool {
        let mut active = std::collections::BTreeMap::new();
        for (seat, request) in commands {
            match &request.action {
                Action::OfferTrade { target, .. }
                | Action::OfferPeace { target }
                | Action::OfferPeaceTreaty { target, .. }
                | Action::OfferFriendship { target }
                | Action::OfferDefensivePact { target }
                | Action::OfferResearchAgreement { target } => {
                    active.insert((seat, *target), format!("{seat}:{}", request.command_id));
                }
                Action::WithdrawOffer { offer } => {
                    active.retain(|&(from, _), id| from != seat || id != offer);
                }
                Action::AnswerOffer { offer, accept } => {
                    if let Some(pair) = active
                        .iter()
                        .find(|(&(.., to), id)| to == seat && *id == offer)
                        .map(|(&pair, _)| pair)
                    {
                        active.remove(&pair);
                        if *accept {
                            active.remove(&(pair.1, pair.0));
                        }
                    }
                }
                Action::DeclareWar { target } => {
                    active.remove(&(seat, *target));
                    active.remove(&(*target, seat));
                }
                _ => {}
            }
        }
        self.0
            .iter()
            .filter(|offer| !offer.id.contains(":ai:"))
            .all(|offer| active.get(&(offer.from, offer.to)) == Some(&offer.id))
    }

    pub fn visible(&self, seat: u8) -> Vec<DiplomaticOffer> {
        self.0
            .iter()
            .filter(|o| o.from == seat || o.to == seat)
            .cloned()
            .collect()
    }

    pub fn propose(
        &mut self,
        world: &World,
        r: &Ruleset,
        offer: DiplomaticOffer,
    ) -> Result<(), RejectionCode> {
        let terms = core_terms(r, &offer.terms)?;
        agreements::validate_proposal(
            world,
            r,
            CivId(offer.from),
            CivId(offer.to),
            &terms,
            TurnModifiers::for_world(world, r),
        )
        .map_err(|_| RejectionCode::IllegalAction)?;
        // Replacing one's outgoing offer never changes the terms of an old ID.
        self.0.retain(|o| o.from != offer.from || o.to != offer.to);
        self.0.push(offer);
        Ok(())
    }

    pub fn answer(
        &mut self,
        world: &mut World,
        r: &Ruleset,
        seat: u8,
        id: &str,
        accept: bool,
    ) -> Result<Vec<TurnEvent>, RejectionCode> {
        let offer = self
            .0
            .iter()
            .find(|o| o.id == id && o.to == seat)
            .ok_or(RejectionCode::IllegalAction)?
            .clone();
        let events = if accept {
            let terms = core_terms(r, &offer.terms)?;
            agreements::accept(
                world,
                r,
                CivId(offer.from),
                CivId(offer.to),
                &terms,
                TurnModifiers::for_world(world, r),
            )
            .map_err(|_| RejectionCode::IllegalAction)?
            .events
        } else {
            if is_ai_offer(world, &offer) {
                let terms = core_terms(r, &offer.terms)?;
                civ_core::world::ai_offers::declined(world, CivId(offer.from), CivId(offer.to), &terms);
            }
            Vec::new()
        };
        self.0
            .retain(|o| o.id != id && !(accept && o.from == offer.to && o.to == offer.from));
        Ok(events)
    }

    pub fn withdraw(&mut self, seat: u8, id: &str) -> Result<(), RejectionCode> {
        let index = self
            .0
            .iter()
            .position(|o| o.id == id && o.from == seat)
            .ok_or(RejectionCode::IllegalAction)?;
        self.0.remove(index);
        Ok(())
    }

    pub fn prune(&mut self, world: &World) {
        self.0.retain(|offer| {
            offer.turn == world.turn
                // Unseen elimination is private. Keep consent until its public
                // expiry/stance change; acceptance rechecks both live parties.
                && matches!(offer.terms, AgreementTerms::Peace | AgreementTerms::PeaceTreaty { .. }) == world.diplomacy.at_war(CivId(offer.from), CivId(offer.to))
        });
    }

    pub fn validate_saved(
        &self,
        world: &World,
        r: &Ruleset,
        seats: &std::collections::BTreeSet<u8>,
    ) -> bool {
        let mut pairs = std::collections::BTreeSet::new();
        let mut ids = std::collections::BTreeSet::new();
        let mut pruned = self.clone();
        pruned.prune(world);
        if pruned != *self {
            return false;
        }
        self.0.iter().all(|o| {
            let prefix = format!("{}:", o.from);
            let from_ai = is_ai_offer(world, o);
            (seats.contains(&o.from) || from_ai)
                && seats.contains(&o.to)
                && o.from != o.to
                && (!o.id.contains(":ai:") || from_ai)
                && o.id
                    .strip_prefix(&prefix)
                    .is_some_and(crate::session::valid_identifier)
                && ids.insert(&o.id)
                && pairs.insert((o.from, o.to))
                && world.diplomacy.have_met(CivId(o.from), CivId(o.to))
                && match &o.terms {
                    AgreementTerms::Peace
                    | AgreementTerms::Friendship
                    | AgreementTerms::DefensivePact
                    | AgreementTerms::ResearchAgreement => true,
                    AgreementTerms::PeaceTreaty { give, get }
                    | AgreementTerms::Trade { give, get } => {
                        give.len() <= 32 && get.len() <= 32 && (!give.is_empty() || !get.is_empty())
                    }
                }
                && core_terms(r, &o.terms).is_ok_and(|terms| match terms {
                    agreements::Terms::Peace
                    | agreements::Terms::Friendship
                    | agreements::Terms::DefensivePact
                    | agreements::Terms::ResearchAgreement => true,
                    agreements::Terms::PeaceTreaty { give, get }
                    | agreements::Terms::Trade { give, get } => {
                        civ_core::world::trade::valid_proposal_terms(r, &give, &get)
                    }
                })
        })
    }
}

fn core_terms(r: &Ruleset, terms: &AgreementTerms) -> Result<agreements::Terms, RejectionCode> {
    Ok(match terms {
        AgreementTerms::Peace => agreements::Terms::Peace,
        AgreementTerms::PeaceTreaty { give, get } => agreements::Terms::PeaceTreaty {
            give: give
                .iter()
                .map(|item| crate::session::trade_item(r, item))
                .collect::<Result<_, _>>()?,
            get: get
                .iter()
                .map(|item| crate::session::trade_item(r, item))
                .collect::<Result<_, _>>()?,
        },
        AgreementTerms::DefensivePact => agreements::Terms::DefensivePact,
        AgreementTerms::Friendship => agreements::Terms::Friendship,
        AgreementTerms::ResearchAgreement => agreements::Terms::ResearchAgreement,
        AgreementTerms::Trade { give, get } => agreements::Terms::Trade {
            give: give
                .iter()
                .map(|item| crate::session::trade_item(r, item))
                .collect::<Result<_, _>>()?,
            get: get
                .iter()
                .map(|item| crate::session::trade_item(r, item))
                .collect::<Result<_, _>>()?,
        },
    })
}
