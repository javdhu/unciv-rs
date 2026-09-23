//! Demands, and the promises that answer them (`Demand`).
//!
//! One civ catches another doing something it minds — settling near it,
//! spying on it, preaching to it, massing troops on its border — and asks it
//! to stop. The one asked agrees or refuses. Either costs it: being told what
//! to do is resented (−10 for giving in, −20 for refusing, both held against
//! the one who asked), and a refusal is itself held against the refuser (−15),
//! except a refusal to promise not to attack, which is war.
//!
//! A promise runs a hundred turns. Kept to the end it earns +10. Broken — the
//! same thing caught again while it runs — it costs −20, which fades at an
//! eighth of a point a turn, and the victim stops asking. Breaking a promise
//! not to attack costs −10 more with everyone who knows both.
//!
//! Everything is stored on the one who asked, about the one asked, as Unciv
//! stores it ("set on the promisee, not the promiser"): the [`Flag`]s
//! `Agreed`, `Violated` and `IgnoreViolation`, each for one [`Demand`].
//!
//! Who asks, and how the asked answer, are AI choices and live in `ai`. What
//! follows from them is here.

use crate::ruleset::Ruleset;
use crate::stats::Fixed;
use crate::world::diplomacy::{Flag, Sentiment};
use crate::world::grid::CivId;
use crate::world::world::World;
use crate::world::TurnEvent;

/// Something one civ can ask another to stop doing.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Demand {
    DontSpyOnUs,
    DoNotSpreadReligion,
    DoNotSettleNearUs,
    /// Only ever asked by an AI, of a human whose army sits on its border.
    DoNotAttackUs,
}

impl Demand {
    pub const ALL: [Demand; 4] =
        [Demand::DontSpyOnUs, Demand::DoNotSpreadReligion, Demand::DoNotSettleNearUs, Demand::DoNotAttackUs];
}

/// How long a promise runs, and how long a refusal or a betrayal is
/// remembered before the victim would ask again.
pub const PROMISE_TURNS: u32 = 100;
/// How long a violation waits to be noticed.
pub const VIOLATION_TURNS: u32 = 30;
/// How near a new city may come to another civ's before it is "near us".
pub const NEAR_US: u32 = 6;

const AGREED: Fixed = Fixed::whole(-10);
const REFUSED: Fixed = Fixed::whole(-20);
const REFUSAL_RESENTED: Fixed = Fixed::whole(-15);
const BETRAYED: Fixed = Fixed::whole(-20);
const BETRAYED_OTHERS: Fixed = Fixed::whole(-10);
const FULFILLED: Fixed = Fixed::whole(10);
const SPIED_ON: Fixed = Fixed::whole(-15);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DemandRejection {
    NotMajor,
    NotMet,
    AtWar,
    /// Already asked and not yet answered, or already promised.
    AlreadyAsked,
    /// Nobody may ask this; see [`Demand::DoNotAttackUs`].
    NotOffered,
    /// There is no such demand waiting to be answered.
    NotAsked,
}

impl std::fmt::Display for DemandRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            DemandRejection::NotMajor => "only major civs make and answer demands",
            DemandRejection::NotMet => "the two civs have not met",
            DemandRejection::AtWar => "the two civs are at war",
            DemandRejection::AlreadyAsked => "already asked, or already promised",
            DemandRejection::NotOffered => "this demand is not offered",
            DemandRejection::NotAsked => "no such demand is waiting",
        })
    }
}

impl std::error::Error for DemandRejection {}

/// What came of a demand or a promise, for the turn log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromiseEvent {
    Demanded { by: CivId, of: CivId, demand: Demand },
    Agreed { by: CivId, to: CivId, demand: Demand },
    Refused { by: CivId, to: CivId, demand: Demand },
    Broken { by: CivId, to: CivId, demand: Demand },
    Kept { by: CivId, to: CivId, demand: Demand },
}

fn is_major(world: &World, civ: CivId) -> bool {
    world.civ(civ).is_some_and(|c| c.is_major() && c.is_alive())
}

fn is_ai(world: &World, civ: CivId) -> bool {
    world.civ(civ).is_some_and(|c| c.is_ai)
}

/// Whether `by` may ask `of` for this now.
pub fn may_demand(world: &World, by: CivId, of: CivId, demand: Demand) -> Result<(), DemandRejection> {
    if by == of || !is_major(world, by) || !is_major(world, of) {
        return Err(DemandRejection::NotMajor);
    }
    if !world.diplomacy.have_met(by, of) {
        return Err(DemandRejection::NotMet);
    }
    if world.diplomacy.at_war(by, of) {
        return Err(DemandRejection::AtWar);
    }
    // Unciv offers this one to no player, "to avoid abuse": only an AI asks
    // it, and only of a human.
    if demand == Demand::DoNotAttackUs && (!is_ai(world, by) || is_ai(world, of)) {
        return Err(DemandRejection::NotOffered);
    }
    if world.diplomacy.demands.contains(&(by, of, demand)) || world.diplomacy.has_flag(by, of, Flag::Agreed(demand)) {
        return Err(DemandRejection::AlreadyAsked);
    }
    Ok(())
}

/// Ask. The answer comes on the asked civ's turn.
pub fn demand(world: &mut World, by: CivId, of: CivId, demand: Demand) -> Result<PromiseEvent, DemandRejection> {
    may_demand(world, by, of, demand)?;
    world.diplomacy.demands.insert((by, of, demand));
    Ok(PromiseEvent::Demanded { by, of, demand })
}

/// Demands waiting on `civ`'s answer, in key order.
pub fn asked_of(world: &World, civ: CivId) -> Vec<(CivId, Demand)> {
    world.diplomacy.demands.iter().filter(|(_, of, _)| *of == civ).map(|(by, _, d)| (*by, *d)).collect()
}

/// Answer a demand (`agreeToDemand`, `refuseDemand`).
///
/// Refusing to promise not to attack is a declaration of war, and the war is
/// returned so the caller can report it like any other.
pub fn answer(
    world: &mut World,
    r: &Ruleset,
    of: CivId,
    by: CivId,
    demand: Demand,
    agree: bool,
) -> Result<(PromiseEvent, Vec<TurnEvent>), DemandRejection> {
    if !world.diplomacy.demands.contains(&(by, of, demand)) {
        return Err(DemandRejection::NotAsked);
    }
    if !is_major(world, by) {
        // Asked by a civ since destroyed. Nobody is left to answer to.
        return Err(DemandRejection::NotMajor);
    }
    if !agree && demand == Demand::DoNotAttackUs && !world.diplomacy.can_declare_war(of, by) {
        return Err(DemandRejection::NotOffered);
    }
    world.diplomacy.demands.remove(&(by, of, demand));
    if agree {
        world.diplomacy.set_flag(by, of, Flag::Agreed(demand), PROMISE_TURNS);
        world.diplomacy.attitude_mut(of, by).add(Sentiment::UnacceptableDemands, AGREED, None);
        return Ok((PromiseEvent::Agreed { by: of, to: by, demand }, Vec::new()));
    }
    world.diplomacy.attitude_mut(of, by).add(Sentiment::UnacceptableDemands, REFUSED, None);
    world.diplomacy.set_flag(by, of, Flag::IgnoreViolation(demand), PROMISE_TURNS);
    let war = if demand == Demand::DoNotAttackUs {
        // No grudge for the refusal itself: "they respect our honesty".
        crate::world::diplomacy_actions::declare_war(world, r, of, by)
    } else {
        world.diplomacy.attitude_mut(by, of).add(Sentiment::RefusedDemand(demand), REFUSAL_RESENTED, None);
        Vec::new()
    };
    Ok((PromiseEvent::Refused { by: of, to: by, demand }, war))
}

/// `victim` caught `offender` at it.
fn violated(world: &mut World, victim: CivId, offender: CivId, demand: Demand) {
    world.diplomacy.set_flag(victim, offender, Flag::Violated(demand), VIOLATION_TURNS);
}

/// A city was founded (`CityFounder.triggerCitiesSettledNearOtherCiv`).
///
/// Every other major with a city within six tiles, that knows the founder and
/// has seen the spot, takes note.
pub fn on_city_founded(world: &mut World, city: crate::arena::Id<crate::world::city::City>) {
    let Some(c) = world.cities.get(city) else { return };
    let (founder, centre) = (c.owner, c.centre);
    if !is_major(world, founder) {
        return;
    }
    let mut near: Vec<CivId> = world
        .cities
        .iter()
        .filter(|(_, other)| other.owner != founder && world.grid.distance(other.centre, centre) <= NEAR_US)
        .map(|(_, other)| other.owner)
        .filter(|o| is_major(world, *o) && world.diplomacy.have_met(*o, founder))
        .filter(|o| world.visibility.has_explored(*o, centre))
        .collect();
    near.sort();
    near.dedup();
    for o in near {
        violated(world, o, founder, Demand::DoNotSettleNearUs);
    }
}

/// `victim` learned whose spy was in its city (`Spy.demandToNotBeSpiedOn`).
///
/// A spy caught and killed is learned of twice over in Unciv — once for the
/// killing, once for being identified — and so costs twice.
pub fn on_spy_discovered(world: &mut World, victim: CivId, thief: CivId, killed: bool) {
    if !is_major(world, victim) || !is_major(world, thief) || victim == thief {
        return;
    }
    let times = if killed { 2 } else { 1 };
    world.diplomacy.attitude_mut(victim, thief).add(Sentiment::SpiedOnUs, SPIED_ON * Fixed::whole(times), None);
    violated(world, victim, thief, Demand::DontSpyOnUs);
}

/// A missionary of `preacher`'s spread its faith in `owner`'s city
/// (`UnitActionsReligion`) — noticed only while `owner` could still have a
/// religion of its own to prefer.
pub fn on_religion_spread(world: &mut World, owner: CivId, preacher: CivId, owner_may_have_own: bool) {
    if owner == preacher || !owner_may_have_own || !is_major(world, owner) || !is_major(world, preacher) {
        return;
    }
    violated(world, owner, preacher, Demand::DoNotSpreadReligion);
}

/// A war was declared (`DeclareWar`): if the aggressor had promised not to,
/// that is a violation like any other, and will be judged as one.
pub fn on_war_declared(world: &mut World, aggressor: CivId, victim: CivId) {
    if world.diplomacy.has_flag(victim, aggressor, Flag::Agreed(Demand::DoNotAttackUs)) {
        violated(world, victim, aggressor, Demand::DoNotAttackUs);
    }
}

/// An AI goes over what it has caught others doing (`onDemandViolation`).
///
/// A violation of a promise is a betrayal. One already ignored is let go.
/// Anything else waits on the AI's choice to ask — which is `ai`'s, and comes
/// through [`demand`]. Either way the violation is then forgotten; one made
/// during a war is kept until the war is over or it runs out, except an
/// attack, which is what the war is.
pub fn review(world: &mut World, civ: CivId) -> Vec<PromiseEvent> {
    let mut events = Vec::new();
    if !is_ai(world, civ) || !is_major(world, civ) {
        return events;
    }
    let others: Vec<CivId> = world.civ_ids().filter(|o| *o != civ && is_major(world, *o)).collect();
    for other in others {
        for d in Demand::ALL {
            // One caught this turn, after the plans were made, waits for the
            // next: the choice to ask about it has not been made yet.
            let turns = world.diplomacy.flag_turns(civ, other, Flag::Violated(d));
            if turns == 0 || turns == VIOLATION_TURNS {
                continue;
            }
            if world.diplomacy.at_war(civ, other) && d != Demand::DoNotAttackUs {
                continue;
            }
            if world.diplomacy.has_flag(civ, other, Flag::IgnoreViolation(d)) {
                // Let go.
            } else if world.diplomacy.has_flag(civ, other, Flag::Agreed(d)) {
                world.diplomacy.set_flag(civ, other, Flag::IgnoreViolation(d), PROMISE_TURNS);
                world.diplomacy.attitude_mut(civ, other).set(Sentiment::BetrayedPromise(d), BETRAYED);
                if d == Demand::DoNotAttackUs {
                    let common: Vec<CivId> = world
                        .civ_ids()
                        .filter(|t| *t != civ && *t != other && is_major(world, *t))
                        .filter(|t| world.diplomacy.have_met(*t, civ) && world.diplomacy.have_met(*t, other))
                        .collect();
                    for t in common {
                        world.diplomacy.attitude_mut(t, other).set(Sentiment::BetrayedPromiseToOthers, BETRAYED_OTHERS);
                    }
                }
                world.diplomacy.clear_flag(civ, other, Flag::Agreed(d));
                events.push(PromiseEvent::Broken { by: other, to: civ, demand: d });
            }
            world.diplomacy.clear_flag(civ, other, Flag::Violated(d));
        }
    }
    events
}

/// Countdowns that ran out this turn: a promise that lasted is a promise kept
/// (`DiplomacyTurnManager`, the `agreedToDemand` flags).
pub fn on_flags_expired(world: &mut World, expired: &[(CivId, CivId, Flag)]) -> Vec<PromiseEvent> {
    let mut events = Vec::new();
    for &(holder, subject, flag) in expired {
        if let Flag::Agreed(d) = flag {
            world.diplomacy.attitude_mut(holder, subject).add(Sentiment::FulfilledPromise(d), FULFILLED, None);
            events.push(PromiseEvent::Kept { by: subject, to: holder, demand: d });
        }
    }
    events
}

/// Whether `civ` has promised to keep new cities away from `tile`
/// (`CityLocationTileRanker.canSettleTile`): a city of a civ it promised,
/// within six, not at war.
pub fn promised_not_to_settle(world: &World, civ: CivId, tile: crate::world::grid::TileId) -> bool {
    let promised: Vec<CivId> = world
        .civ_ids()
        .filter(|o| world.diplomacy.has_flag(*o, civ, Flag::Agreed(Demand::DoNotSettleNearUs)))
        .filter(|o| !world.diplomacy.at_war(civ, *o))
        .collect();
    !promised.is_empty()
        && world
            .cities
            .iter()
            .any(|(_, c)| promised.contains(&c.owner) && world.grid.distance(c.centre, tile) <= NEAR_US)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::city::City;
    use crate::world::entities::Civilization;
    use crate::world::grid::{TileGrid, TileId};

    const A: CivId = CivId(0);
    const B: CivId = CivId(1);
    const C: CivId = CivId(2);

    fn ruleset() -> crate::ruleset::Ruleset {
        crate::ruleset::Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings")
            .expect("readable")
            .expect("loads")
    }

    /// Three AI majors who have all met, each with a city so they are alive.
    fn world(r: &crate::ruleset::Ruleset) -> World {
        let g = TileGrid::new(20, 12, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        let mut w = World::new(g, (0..3).map(|i| Civilization::new(CivId(i), nation, true)).collect());
        for (a, b) in [(0, 1), (0, 2), (1, 2)] {
            w.diplomacy.meet(CivId(a), CivId(b));
        }
        for (i, x) in [(0u8, 1u16), (1, 9), (2, 17)] {
            let t = w.grid.id_at(x, 6).unwrap();
            w.found_city(City::found(format!("C{i}"), CivId(i), t));
        }
        w
    }

    fn opinion(w: &World, holder: CivId, subject: CivId, s: Sentiment) -> Fixed {
        w.diplomacy.attitude(holder, subject).get(s)
    }

    #[test]
    fn agreeing_is_resented_by_the_one_who_gave_in_and_starts_a_hundred_turn_promise() {
        let r = ruleset();
        let mut w = world(&r);
        demand(&mut w, A, B, Demand::DoNotSettleNearUs).unwrap();
        assert_eq!(asked_of(&w, B), vec![(A, Demand::DoNotSettleNearUs)]);
        assert_eq!(demand(&mut w, A, B, Demand::DoNotSettleNearUs), Err(DemandRejection::AlreadyAsked));

        answer(&mut w, &r, B, A, Demand::DoNotSettleNearUs, true).unwrap();
        assert!(asked_of(&w, B).is_empty());
        assert_eq!(w.diplomacy.flag_turns(A, B, Flag::Agreed(Demand::DoNotSettleNearUs)), PROMISE_TURNS);
        assert_eq!(opinion(&w, B, A, Sentiment::UnacceptableDemands), Fixed::whole(-10));
        assert_eq!(demand(&mut w, A, B, Demand::DoNotSettleNearUs), Err(DemandRejection::AlreadyAsked), "promised");
    }

    #[test]
    fn a_refusal_costs_both_sides_and_the_asker_stops_asking() {
        let r = ruleset();
        let mut w = world(&r);
        demand(&mut w, A, B, Demand::DontSpyOnUs).unwrap();
        let (e, war) = answer(&mut w, &r, B, A, Demand::DontSpyOnUs, false).unwrap();
        assert_eq!(e, PromiseEvent::Refused { by: B, to: A, demand: Demand::DontSpyOnUs });
        assert!(war.is_empty());
        assert_eq!(opinion(&w, B, A, Sentiment::UnacceptableDemands), Fixed::whole(-20));
        assert_eq!(opinion(&w, A, B, Sentiment::RefusedDemand(Demand::DontSpyOnUs)), Fixed::whole(-15));
        assert!(w.diplomacy.has_flag(A, B, Flag::IgnoreViolation(Demand::DontSpyOnUs)));
    }

    #[test]
    fn refusing_to_promise_peace_is_war() {
        let r = ruleset();
        let mut w = world(&r);
        w.civ_mut(B).unwrap().is_ai = false;
        assert_eq!(
            may_demand(&w, B, A, Demand::DoNotAttackUs),
            Err(DemandRejection::NotOffered),
            "a human may not ask"
        );
        demand(&mut w, A, B, Demand::DoNotAttackUs).unwrap();
        let (_, war) = answer(&mut w, &r, B, A, Demand::DoNotAttackUs, false).unwrap();
        assert!(!war.is_empty());
        assert!(w.diplomacy.at_war(A, B));
        assert!(!w.diplomacy.attitude(A, B).has(Sentiment::RefusedDemand(Demand::DoNotAttackUs)), "honesty respected");
        assert_eq!(opinion(&w, A, B, Sentiment::DeclaredWarOnUs), Fixed::whole(-20), "but it is still a war");
    }

    #[test]
    fn a_city_founded_near_a_neighbour_who_has_seen_the_spot_is_noticed() {
        let r = ruleset();
        let mut w = world(&r);
        let spot = w.grid.id_at(5, 6).unwrap(); // 4 from A's city, 4 from B's
        w.visibility.observe(A, spot);
        let city = w.found_city(City::found("Near", C, spot));
        on_city_founded(&mut w, city);
        assert!(w.diplomacy.has_flag(A, C, Flag::Violated(Demand::DoNotSettleNearUs)));
        assert!(!w.diplomacy.has_flag(B, C, Flag::Violated(Demand::DoNotSettleNearUs)), "B never saw it");
    }

    #[test]
    fn a_broken_promise_is_a_betrayal_and_a_kept_one_earns_warmth() {
        let r = ruleset();
        let mut w = world(&r);
        demand(&mut w, A, B, Demand::DoNotSettleNearUs).unwrap();
        answer(&mut w, &r, B, A, Demand::DoNotSettleNearUs, true).unwrap();
        violated(&mut w, A, B, Demand::DoNotSettleNearUs);
        assert!(review(&mut w, A).is_empty(), "not judged the turn it happens");
        w.diplomacy.tick();
        let events = review(&mut w, A);
        assert_eq!(events, vec![PromiseEvent::Broken { by: B, to: A, demand: Demand::DoNotSettleNearUs }]);
        assert_eq!(opinion(&w, A, B, Sentiment::BetrayedPromise(Demand::DoNotSettleNearUs)), Fixed::whole(-20));
        assert!(!w.diplomacy.has_flag(A, B, Flag::Agreed(Demand::DoNotSettleNearUs)));
        assert!(!w.diplomacy.has_flag(A, B, Flag::Violated(Demand::DoNotSettleNearUs)), "judged, so forgotten");
        // Caught again: let go, not a second betrayal.
        violated(&mut w, A, B, Demand::DoNotSettleNearUs);
        w.diplomacy.tick();
        assert!(review(&mut w, A).is_empty());
        assert!(!w.diplomacy.has_flag(A, B, Flag::Violated(Demand::DoNotSettleNearUs)));

        let mut w = world(&r);
        demand(&mut w, A, C, Demand::DontSpyOnUs).unwrap();
        answer(&mut w, &r, C, A, Demand::DontSpyOnUs, true).unwrap();
        let mut kept = Vec::new();
        for _ in 0..PROMISE_TURNS {
            let expired = w.diplomacy.tick();
            kept.extend(on_flags_expired(&mut w, &expired));
        }
        assert_eq!(kept, vec![PromiseEvent::Kept { by: C, to: A, demand: Demand::DontSpyOnUs }]);
        assert_eq!(opinion(&w, A, C, Sentiment::FulfilledPromise(Demand::DontSpyOnUs)), Fixed::whole(10));
    }

    #[test]
    fn attacking_after_promising_not_to_turns_everyone_who_knows_against_the_aggressor() {
        let r = ruleset();
        let mut w = world(&r);
        w.civ_mut(B).unwrap().is_ai = false;
        demand(&mut w, A, B, Demand::DoNotAttackUs).unwrap();
        answer(&mut w, &r, B, A, Demand::DoNotAttackUs, true).unwrap();
        w.diplomacy.declare_war(B, A);
        crate::world::opinion::on_war_declared(&mut w, B, A);
        assert!(w.diplomacy.has_flag(A, B, Flag::Violated(Demand::DoNotAttackUs)));
        w.diplomacy.tick();
        assert_eq!(review(&mut w, A).len(), 1, "judged even in the war it started");
        assert_eq!(opinion(&w, C, B, Sentiment::BetrayedPromiseToOthers), Fixed::whole(-10));
    }

    #[test]
    fn a_caught_spy_costs_twice_what_an_identified_one_does() {
        let r = ruleset();
        let mut w = world(&r);
        on_spy_discovered(&mut w, A, B, false);
        on_spy_discovered(&mut w, A, C, true);
        assert_eq!(opinion(&w, A, B, Sentiment::SpiedOnUs), Fixed::whole(-15));
        assert_eq!(opinion(&w, A, C, Sentiment::SpiedOnUs), Fixed::whole(-30));
        assert!(w.diplomacy.has_flag(A, B, Flag::Violated(Demand::DontSpyOnUs)));
    }

    /// Through the turn itself: caught, asked on the victim's turn, answered
    /// on the next — by an equal, who has nothing to fear, with a refusal.
    #[test]
    fn a_turn_asks_and_the_next_answers() {
        use crate::world::turn::{advance_turn, TurnEvent, TurnModifiers};
        let r = ruleset();
        let mut w = world(&r);
        let spot = w.grid.id_at(5, 6).unwrap();
        w.visibility.observe(A, spot);
        let city = w.found_city(City::found("Near", B, spot));
        on_city_founded(&mut w, city);

        let mut log = Vec::new();
        for _ in 0..3 {
            log.extend(advance_turn(&mut w, &r, crate::world::CityFocus::None, TurnModifiers::default()));
        }
        let d = Demand::DoNotSettleNearUs;
        let asked = log.iter().position(|e| *e == TurnEvent::DemandMade { by: A, of: B, demand: d });
        let answered =
            log.iter().position(|e| *e == TurnEvent::DemandAnswered { by: B, to: A, demand: d, agreed: false });
        assert!(asked.is_some() && answered.is_some() && asked < answered, "{log:?}");
        assert!(w.diplomacy.has_flag(A, B, Flag::IgnoreViolation(d)), "refused, so no longer asked");
        assert!(!w.diplomacy.has_flag(A, B, Flag::Violated(d)), "and the violation was judged");
    }

    fn warriors(w: &mut World, r: &crate::ruleset::Ruleset, civ: CivId, at: &[TileId]) {
        let warrior = r.units.iter().find(|(_, u)| u.name == "Warrior").map(|(id, _)| id).unwrap();
        for t in at {
            w.place_unit(crate::world::entities::Unit::new(warrior, civ, *t, 2));
        }
    }

    #[test]
    fn an_ai_gives_in_to_the_strong_and_no_one_else() {
        use crate::world::ai::{answer_demands, threat, ThreatLevel};
        let r = ruleset();
        let mut w = world(&r);
        let barracks: Vec<TileId> = (0..3).map(|x| w.grid.id_at(x, 0).unwrap()).collect();
        warriors(&mut w, &r, A, &barracks);
        assert_eq!(threat(&w, &r, B, A), ThreatLevel::VeryHigh);
        assert_eq!(threat(&w, &r, A, B), ThreatLevel::VeryLow);
        demand(&mut w, A, B, Demand::DontSpyOnUs).unwrap();
        demand(&mut w, C, B, Demand::DontSpyOnUs).unwrap();
        assert_eq!(answer_demands(&w, &r, B), vec![(A, Demand::DontSpyOnUs, true), (C, Demand::DontSpyOnUs, false)]);
    }

    #[test]
    fn a_human_army_on_the_border_is_asked_what_it_means() {
        use crate::world::ai::{border_alarm, choose_demands};
        let r = ruleset();
        let mut w = world(&r);
        w.civ_mut(B).unwrap().is_ai = false;
        let home = w.grid.id_at(1, 6).unwrap();
        let border: Vec<TileId> = w.grid.within(home, 2).into_iter().filter(|t| *t != home).take(8).collect();
        for t in &border {
            w.visibility.observe(A, *t);
        }
        // An army of A's own, so B is a match rather than a menace.
        let barracks: Vec<TileId> = (0..8).map(|x| w.grid.id_at(x, 11).unwrap()).collect();
        warriors(&mut w, &r, A, &barracks);
        warriors(&mut w, &r, B, &border[..7]);
        assert!(border_alarm(&w, &r, A).is_empty(), "seven is not an army");
        warriors(&mut w, &r, B, &border[7..]);
        assert_eq!(border_alarm(&w, &r, A), vec![B]);
        assert_eq!(choose_demands(&w, &r, A), vec![(B, Demand::DoNotAttackUs)]);
        w.diplomacy.set_open_borders(A, B, true);
        assert!(border_alarm(&w, &r, A).is_empty(), "guests are not doubted");
    }

    #[test]
    fn a_promise_not_to_settle_keeps_the_promiser_six_tiles_off() {
        let r = ruleset();
        let mut w = world(&r);
        demand(&mut w, A, B, Demand::DoNotSettleNearUs).unwrap();
        answer(&mut w, &r, B, A, Demand::DoNotSettleNearUs, true).unwrap();
        let a_city = w.grid.id_at(1, 6).unwrap();
        let near = w.grid.id_at(6, 6).unwrap();
        assert!(w.grid.distance(a_city, near) <= NEAR_US);
        assert!(promised_not_to_settle(&w, B, near));
        assert!(!promised_not_to_settle(&w, C, near), "only the one who promised");
        let far: TileId = w.grid.id_at(9, 1).unwrap();
        assert!(!promised_not_to_settle(&w, B, far));
    }
}
