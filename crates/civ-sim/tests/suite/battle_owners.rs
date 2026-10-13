//! `attacked` names the defender's owner as the engine recorded the fight, also when the defender died, and the
//! attacker's only where this player could know it.

use civ_core::world::air::Target;
use civ_core::world::combat::BattleReport;
use civ_core::world::{CityFocus, TurnEvent, TurnModifiers};
use civ_sim::protocol::AttackTarget;
use civ_sim::session::Session;
use civ_sim::{PresentationEvent, Settings};

fn same(target: &AttackTarget, engine: Target) -> bool {
    let (id, gen) = match engine {
        Target::Unit(id) => (id.index(), id.generation()),
        Target::City(id) => (id.index(), id.generation()),
    };
    match (target, engine) {
        (AttackTarget::Unit { unit }, Target::Unit(_)) => (unit.id, unit.r#gen) == (id, gen),
        (AttackTarget::City { city }, Target::City(_)) => (city.id, city.r#gen) == (id, gen),
        _ => false,
    }
}

#[derive(Default, Debug)]
struct Seen {
    fights: usize,
    replayed: usize,
    defender_died: usize,
    unnamed: usize,
}

/// Plays `rounds` rounds of a seeded match with seat 0 idle or handed to a bot, and checks every `attacked`
/// seat 0 is shown, as an event or in the replay, against the engine's own battle report. Each round is first
/// played on a copy of the world with the round driver the session uses, to read those reports.
fn check(seed: u64, bot: bool, rounds: usize) -> Seen {
    let settings = Settings { seed, width: 24, height: 16, majors: 2, city_states: 1, ..Settings::default() };
    let mut session = Session::new(settings, "fights".into(), vec![0]).unwrap();
    if bot {
        session = session.stage_controller(0, false).unwrap().next;
    }
    let mut seen = Seen::default();
    for _ in 0..rounds {
        let rules = session.rules().clone();
        let mut world = session.world().clone();
        let mut steps = Vec::new();
        let mods = TurnModifiers::for_world(&world, &rules);
        let events = civ_core::world::finish_round_recorded(&mut world, &rules, CityFocus::default(), mods, &mut steps);
        let Some(transition) = session.stage_timeout(session.turn()).unwrap() else { break };
        let fights: Vec<(&BattleReport, bool)> = events
            .iter()
            .chain(steps.iter().flat_map(|s| &s.events))
            .filter_map(|e| match e {
                TurnEvent::Battle { report: Some(b), defender_died, .. } => Some((b, *defender_died)),
                _ => None,
            })
            .collect();
        let Some((_, update)) = transition.updates.iter().find(|(seat, _)| *seat == 0) else {
            session = transition.next;
            continue;
        };
        // Each event with the units its viewer could place: the round's end view for its own events, and each
        // replay batch's units, seen as they acted, for the replay.
        let in_view: Vec<_> = update.view.units.iter().map(|u| (u.id, u.r#gen)).collect();
        let mut shown: Vec<_> = update.events.iter().map(|e| (&e.event, in_view.clone())).collect();
        for batch in update.replay.iter().flatten() {
            let units: Vec<_> = batch.units.iter().map(|u| (u.unit.id, u.unit.r#gen)).collect();
            shown.extend(batch.events.iter().map(|e| (e, units.clone())));
            seen.replayed += batch.events.iter().filter(|e| e.kind == "Attacked").count();
        }
        for (event, placed) in shown {
            let PresentationEvent::Attacked {
                viewer_attacked, attacker, target, from, at, defender_died, defender_owner, attacker_owner, ..
            } = &event.event
            else {
                continue;
            };
            // A copy that played a different round would fail here: the fight must be one the engine recorded.
            let (b, died) = fights
                .iter()
                .find(|(b, _)| same(attacker, b.attacker) && same(target, b.target) && b.from.0 == *from && b.at.0 == *at)
                .unwrap_or_else(|| panic!("no battle the engine recorded: {event:?}"));
            assert_eq!(defender_died, died, "{event:?}");
            assert_eq!(*defender_owner, Some(b.defender_owner.0), "{event:?}");
            // The attacker's owner, when given, is the engine's; always given for this player's own blows, and
            // withheld only for an attacker it could not place.
            if *viewer_attacked {
                assert_eq!(*attacker_owner, Some(0), "{event:?}");
            }
            match (attacker_owner, attacker) {
                (Some(owner), _) => assert_eq!(*owner, b.attacker_owner.0, "{event:?}"),
                (None, AttackTarget::Unit { unit }) => {
                    assert!(!placed.contains(&(unit.id, unit.r#gen)), "a placed attacker went unnamed: {event:?}");
                    seen.unnamed += 1;
                }
                (None, AttackTarget::City { .. }) => seen.unnamed += 1,
            }
            seen.fights += 1;
            seen.defender_died += usize::from(*died);
        }
        session = transition.next;
    }
    seen
}

/// An idle seat is raided: its own fights come as events, and the fights it watched as the replay.
#[test]
fn an_idle_seat_is_told_whose_fights_it_saw() {
    let seen = check(11, false, 100);
    assert!(seen.fights >= 10 && seen.replayed > 0, "{seen:?}");
    assert!(seen.defender_died > 0, "no fight shown where the defender died: {seen:?}");
    assert!(seen.unnamed < seen.fights, "{seen:?}");
}

/// A bot seat fights hundreds of times, killing many defenders whose units leave the view.
#[test]
fn a_bot_seat_is_told_the_owner_of_every_defender_it_fought() {
    let seen = check(7, true, 60);
    assert!(seen.fights >= 100, "{seen:?}");
    assert!(seen.defender_died >= 20, "{seen:?}");
    assert!(seen.unnamed * 10 < seen.fights, "most attackers are in sight: {seen:?}");
}
