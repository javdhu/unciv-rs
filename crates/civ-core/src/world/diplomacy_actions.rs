//! War and peace transitions shared by player commands, AI and nuclear combat.
use super::{city_states, grid::CivId, turn::TurnEvent, World};
use crate::{ruleset::Ruleset, stats::Fixed};

pub fn declare_war(world: &mut World, r: &Ruleset, aggressor: CivId, against: CivId) -> Vec<TurnEvent> {
    if !world.diplomacy.can_declare_war(aggressor, against) {
        return Vec::new();
    }
    // Defensive-pact allies cannot call their own pact partners into the war.
    let mut pending = std::collections::VecDeque::from([(aggressor, against, true, true)]);
    let mut events = Vec::new();
    while let Some((a, b, direct, call_defenders)) = pending.pop_front() {
        if a == b || world.diplomacy.at_war(a, b) {
            continue;
        }
        let allies = |major| {
            city_states::city_states(world)
                .into_iter()
                .filter(|cs| city_states::ally_of(world, *cs) == Some(major))
                .collect::<Vec<_>>()
        };
        let a_allies = allies(a);
        let b_allies = allies(b);
        let defenders = if call_defenders && world.civ(a).is_some_and(|c| c.is_major()) {
            world
                .civs
                .iter()
                .filter(|c| {
                    c.id != a && c.is_alive() && world.diplomacy.pact(b, c.id).is_some_and(|p| p.defensive_pact)
                })
                .map(|c| c.id)
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        if direct && world.civ(b).is_some_and(|c| c.is_city_state()) {
            if city_states::ally_of(world, b) == Some(a) {
                for witness in
                    world.civ_ids().filter(|id| *id != a && world.diplomacy.have_met(a, *id)).collect::<Vec<_>>()
                {
                    world.diplomacy.attitude_mut(witness, a).grow(
                        super::diplomacy::Sentiment::BetrayedFriendship,
                        Fixed::whole(-10),
                        None,
                    );
                }
            }
            world.diplomacy.set_influence(b, a, Fixed::whole(-60));
            super::quests::on_city_state_attacked(world, r, b, a);
        }
        // Closed borders displace visiting units before war makes enemy land traversable.
        world.diplomacy.set_open_borders(a, b, false);
        world.diplomacy.set_open_borders(b, a, false);
        super::movement::expel(world, r, a, b);
        super::movement::expel(world, r, b, a);
        if world.diplomacy.declare_war(a, b).is_none() {
            continue;
        }
        world.diplomacy.deals.retain(|d| !((d.a == a && d.b == b) || (d.a == b && d.b == a)));
        for other in world.civ_ids().collect::<Vec<_>>() {
            super::defensive_pacts::cancel(world, a, other);
        }
        super::opinion::on_war_declared(world, a, b);
        events.push(TurnEvent::WarDeclared { aggressor: a, against: b });
        for ally in defenders {
            pending.push_back((a, ally, false, false));
        }
        for ally in a_allies {
            if ally != b {
                pending.push_back((ally, b, false, false));
            }
        }
        for ally in b_allies {
            if ally != a {
                pending.push_back((ally, a, false, false));
            }
        }
    }
    super::defensive_pacts::refresh_opinions(world);
    super::city_state_actions::sync_alliances(world);
    events
}

pub fn make_peace(world: &mut World, r: &Ruleset, a: CivId, b: CivId) -> Vec<TurnEvent> {
    let mut pending = std::collections::VecDeque::from([(a, b)]);
    let mut events = Vec::new();
    while let Some((a, b)) = pending.pop_front() {
        if !world.diplomacy.at_war(a, b) || world.diplomacy.make_peace(a, b).is_none() {
            continue;
        }
        super::movement::expel(world, r, a, b);
        super::movement::expel(world, r, b, a);
        events.push(TurnEvent::PeaceMade { a, b });
        for (major, enemy) in [(a, b), (b, a)] {
            if !world.civ(major).is_some_and(|c| c.is_major()) {
                continue;
            }
            for cs in city_states::city_states(world) {
                if !world.diplomacy.have_met(major, cs) || !world.diplomacy.at_war(cs, enemy) {
                    continue;
                }
                if city_states::ally_of(world, cs) == Some(major) {
                    pending.push_back((cs, enemy));
                } else {
                    world.diplomacy.add_influence(cs, major, Fixed::whole(-10));
                }
            }
        }
    }
    events
}
