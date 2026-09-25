//! Persistent endgame actions and the shared diplomatic election clock.
use std::collections::{BTreeMap, BTreeSet};

use crate::arena::Id;
use crate::rng::Rng;
use crate::ruleset::{BaseUnit, Ruleset, Victory};
use crate::stats::Fixed;
use crate::unique_type::UniqueType;
use crate::world::entities::Unit;
use crate::world::grid::CivId;
use crate::world::world::World;

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CivVictory {
    #[cfg_attr(feature = "serde", serde(with = "crate::serialization::map_as_entries"))]
    pub spaceship_parts: BTreeMap<Id<BaseUnit>, u32>,
    pub diplomatic_win: bool,
    /// Includes the first completion of a repeatable technology, like Kotlin.
    pub repeated_techs: u32,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endgame {
    /// None enables every victory in the ruleset; an empty set enables none.
    pub enabled_victories: Option<BTreeSet<Id<Victory>>>,
    pub max_turns: u32,
    pub voting: Option<DiplomaticVoting>,
}
impl Default for Endgame {
    fn default() -> Self {
        Self { enabled_victories: None, max_turns: 500, voting: None }
    }
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiplomaticVoting {
    pub interval: u32,
    /// The action window in which ballots can be cast. None once a vote wins.
    pub opens_on: Option<u32>,
    #[cfg_attr(feature = "serde", serde(with = "crate::serialization::map_as_entries"))]
    pub ballots: BTreeMap<CivId, Option<CivId>>,
    /// Public only after the ballot window closes. Live ballots are private.
    pub last_result: Option<VoteResult>,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VoteResult {
    pub turn: u32,
    #[cfg_attr(feature = "serde", serde(with = "crate::serialization::map_as_entries"))]
    pub votes: BTreeMap<CivId, u32>,
    pub needed: u32,
    pub winner: Option<CivId>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    PartAssembled { civ: CivId, part: Id<BaseUnit>, total: u32 },
    VotingOpened { turn: u32 },
    VoteResolved(VoteResult),
}

pub fn assembled_parts(world: &World, civ: CivId, part: Id<BaseUnit>) -> u32 {
    world.civ(civ).and_then(|c| c.victory.spaceship_parts.get(&part)).copied().unwrap_or(0)
}

/// A needed ship part competes with military production; delivered, fielded
/// and queued parts all count so cities do not build redundant components.
pub fn part_priority(world: &World, r: &Ruleset, civ: CivId, part: Id<BaseUnit>) -> Option<Fixed> {
    if !r.units.get(part)?.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::AddInCapital)) {
        return None;
    }
    let needed = r
        .victories
        .iter()
        .filter(|(_, v)| super::victory::is_enabled(world, r, &v.name))
        .filter_map(|(_, v)| super::victory::required_parts(r, v).get(&part).copied())
        .max()?;
    let committed = assembled_parts(world, civ, part)
        + world.units.iter().filter(|(_, u)| u.owner == civ && u.base == part).count() as u32
        + world
            .cities
            .iter()
            .filter(|(_, c)| c.owner == civ)
            .filter(|(_, c)| c.queue.contains(&super::construction::Construction::Unit(part)))
            .count() as u32;
    (committed < needed).then_some(Fixed::whole(100_000))
}

pub fn can_assemble(world: &World, r: &Ruleset, unit: Id<Unit>) -> bool {
    let Some(u) = world.units.get(unit) else { return false };
    u.has_unique(r, UniqueType::AddInCapital)
        && world.civ(u.owner).is_some_and(|c| c.is_major() && c.is_alive())
        && crate::world::connections::capital_of(world, r, u.owner)
            .and_then(|id| world.cities.get(id))
            .is_some_and(|c| c.centre == u.position)
}

/// Adding a part consumes the physical unit and releases its resources.
pub fn assemble(world: &mut World, r: &Ruleset, unit: Id<Unit>) -> Option<Event> {
    if !can_assemble(world, r, unit) {
        return None;
    }
    let u = world.units.get(unit)?;
    let (civ, part) = (u.owner, u.base);
    world.remove_unit(unit);
    let total = world.civ_mut(civ)?.victory.spaceship_parts.entry(part).or_default();
    *total = total.saturating_add(1);
    Some(Event::PartAssembled { civ, part, total: *total })
}

/// Triggered by the UN completion effect. Kept even if the UN is later lost.
pub fn start_voting(world: &mut World, speed: Fixed) {
    let interval = (Fixed::whole(15) * speed).floor().max(1) as u32;
    world.endgame.voting = Some(DiplomaticVoting {
        interval,
        opens_on: Some(world.turn.saturating_add(interval)),
        ballots: BTreeMap::new(),
        last_result: None,
    });
}

pub fn voting_open(world: &World) -> bool {
    world.endgame.voting.as_ref().is_some_and(|v| v.opens_on == Some(world.turn))
}

pub fn may_vote(world: &World, civ: CivId) -> bool {
    voting_open(world)
        && world.civ(civ).is_some_and(|c| c.is_alive() && !c.is_barbarian)
        && world.civs.iter().any(|c| c.id != civ && c.is_major() && c.is_alive())
        && world.endgame.voting.as_ref().is_some_and(|v| !v.ballots.contains_key(&civ))
}

pub fn candidates(world: &World, voter: CivId) -> Vec<CivId> {
    world
        .civs
        .iter()
        .filter(|c| c.id != voter && c.is_major() && c.is_alive() && world.diplomacy.have_met(voter, c.id))
        .map(|c| c.id)
        .collect()
}

pub fn cast_vote(world: &mut World, voter: CivId, candidate: Option<CivId>) -> bool {
    if !may_vote(world, voter) || candidate.is_some_and(|c| !candidates(world, voter).contains(&c)) {
        return false;
    }
    world.endgame.voting.as_mut().unwrap().ballots.insert(voter, candidate);
    true
}

pub fn united_nations_owner(world: &World, r: &Ruleset) -> Option<CivId> {
    world.civs.iter().filter(|c| c.is_alive() && !c.is_barbarian).find_map(|c| {
        c.cities
            .iter()
            .filter_map(|id| world.cities.get(*id))
            .any(|city| {
                city.built.iter().any(|id| {
                    r.buildings.get(*id).is_some_and(|b| {
                        b.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::OneTimeTriggerVoting))
                    })
                })
            })
            .then_some(c.id)
    })
}

pub fn votes_needed(world: &World, r: &Ruleset) -> u32 {
    let n = world.civs.iter().filter(|c| c.is_alive() && !c.is_barbarian).count() as u32
        + u32::from(united_nations_owner(world, r).is_some());
    if n > 28 {
        n * 35 / 100
    } else {
        n * (67 - 11 * n / 10) / 100 + 1
    }
}

fn ai_vote(world: &World, voter: CivId) -> Option<CivId> {
    if world.civ(voter)?.is_city_state() {
        return crate::world::city_states::ally_of(world, voter).filter(|c| candidates(world, voter).contains(c));
    }
    let choices = candidates(world, voter);
    let best = choices.iter().map(|c| world.diplomacy.attitude(voter, *c).opinion()).max()?;
    let mut rng = Rng::from_parts(&[0xD1_910, world.turn as u64, voter.0 as u64]);
    if best < Fixed::whole(-80)
        || (best < Fixed::whole(-40) && best + Fixed::whole(rng.range(0, 39) as i32) < Fixed::whole(-40))
    {
        return None;
    }
    let ties: Vec<_> = choices.into_iter().filter(|c| world.diplomacy.attitude(voter, *c).opinion() == best).collect();
    Some(ties[rng.range(0, ties.len() as i64 - 1) as usize])
}

/// Called before incrementing the round, after players had a full action
/// window. Controller changes and defeated seats are read at this boundary.
pub fn finish_voting(world: &mut World, r: &Ruleset, all_ai: bool) -> Vec<Event> {
    if !voting_open(world) {
        return Vec::new();
    }
    let voters: Vec<_> = world
        .civs
        .iter()
        .filter(|c| (all_ai || c.is_ai || c.is_city_state()) && may_vote(world, c.id))
        .map(|c| c.id)
        .collect();
    for voter in voters {
        let choice = ai_vote(world, voter);
        cast_vote(world, voter, choice);
    }
    let un = united_nations_owner(world, r);
    let mut votes = BTreeMap::new();
    for (voter, candidate) in &world.endgame.voting.as_ref().unwrap().ballots {
        if let Some(candidate) = candidate {
            *votes.entry(*candidate).or_default() += if Some(*voter) == un { 2 } else { 1 };
        }
    }
    let needed = votes_needed(world, r);
    let highest = votes.values().copied().max().unwrap_or(0);
    let leaders: Vec<_> = votes.iter().filter(|(_, n)| **n == highest).map(|(c, _)| *c).collect();
    let winner = (highest >= needed && leaders.len() == 1)
        .then(|| leaders[0])
        .filter(|id| world.civ(*id).is_some_and(|c| c.is_major() && c.is_alive()));
    if let Some(civ) = winner {
        world.civ_mut(civ).unwrap().victory.diplomatic_win = true;
    }
    let turn = world.turn.saturating_add(1);
    let result = VoteResult { turn, votes, needed, winner };
    let voting = world.endgame.voting.as_mut().unwrap();
    voting.last_result = Some(result.clone());
    voting.ballots.clear();
    voting.opens_on = winner.is_none().then(|| turn.saturating_add(voting.interval));
    vec![Event::VoteResolved(result)]
}

/// Called after incrementing the round. Detection also covers scenario setup
/// that starts with a built UN and no completion effect.
pub fn begin_round(world: &mut World, r: &Ruleset, speed: Fixed) -> Vec<Event> {
    if world.endgame.voting.is_none() && united_nations_owner(world, r).is_some() {
        start_voting(world, speed);
    }
    if voting_open(world) {
        vec![Event::VotingOpened { turn: world.turn }]
    } else {
        Vec::new()
    }
}

/// The AI's physical delivery path, including the source game's adjacent
/// civilian swap when another worker or part occupies the capital.
pub fn automate_parts(world: &mut World, r: &Ruleset, civ: CivId) -> Vec<crate::world::turn::TurnEvent> {
    use crate::world::turn::TurnEvent;
    let mut events = Vec::new();
    let Some(capital) =
        crate::world::connections::capital_of(world, r, civ).and_then(|id| world.cities.get(id)).map(|c| c.centre)
    else {
        return events;
    };
    let parts: Vec<_> = world
        .units
        .iter()
        .filter(|(_, u)| u.owner == civ && u.has_unique(r, UniqueType::AddInCapital))
        .map(|(id, _)| id)
        .collect();
    for part in parts {
        if let Some(target) = crate::world::movement::step_toward(world, r, part, capital) {
            if let Ok(event) = crate::world::movement::move_unit(world, r, part, target) {
                events.push(TurnEvent::UnitMoved(event));
            }
        }
        if let Some(u) = world.units.get(part).cloned() {
            if world.grid.distance(u.position, capital) == 1 && u.can_move() {
                let blocker = world.units_at(capital).into_iter().find(|id| {
                    world.units.get(*id).is_some_and(|b| b.owner == civ && b.is_civilian(r) && b.can_move())
                });
                if let Some(blocker) = blocker {
                    let b = world.units.get(blocker).unwrap().clone();
                    // Test both legal endpoints with the two swappers removed
                    // from occupancy, without relaxing terrain or border rules.
                    let mut staged = world.clone();
                    staged.remove_unit(part);
                    staged.remove_unit(blocker);
                    if crate::world::movement::can_traverse(&staged, r, &u, u.position, capital)
                        && crate::world::movement::can_traverse(&staged, r, &b, capital, u.position)
                        && !crate::world::movement::is_tile_occupied_for(&staged, r, capital, &u)
                        && !crate::world::movement::is_tile_occupied_for(&staged, r, u.position, &b)
                    {
                        for (id, from, to, spent) in [
                            (part, u.position, capital, u.movement_left),
                            (blocker, capital, u.position, b.movement_left),
                        ] {
                            let unit = world.units.get_mut(id).unwrap();
                            unit.position = to;
                            unit.movement_left = Fixed::ZERO;
                            events.push(TurnEvent::UnitMoved(crate::world::movement::MoveEvent::plain(
                                id, from, to, spent,
                            )));
                        }
                    }
                }
            }
        }
        if let Some(event) = assemble(world, r, part) {
            events.push(TurnEvent::Endgame(event));
        }
    }
    events
}

#[cfg(feature = "save")]
pub(crate) fn validate_snapshot(world: &World, r: &Ruleset) -> Result<(), String> {
    if !(1..=1_000_000_000).contains(&world.endgame.max_turns)
        || world.endgame.enabled_victories.as_ref().is_some_and(|v| v.iter().any(|id| r.victories.get(*id).is_none()))
    {
        return Err("invalid endgame settings".into());
    }
    for civ in &world.civs {
        if civ.victory.repeated_techs > 1_000_000_000
            || civ.victory.spaceship_parts.iter().any(|(id, n)| {
                *n == 0
                    || *n > 1_000_000
                    || r.units
                        .get(*id)
                        .is_none_or(|u| !u.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::AddInCapital)))
            })
        {
            return Err("invalid spaceship or repeat-research state".into());
        }
    }
    if let Some(vote) = &world.endgame.voting {
        let valid_civ = |id: CivId| world.civ(id).is_some_and(|c| !c.is_barbarian);
        let valid_candidate = |id: CivId| world.civ(id).is_some_and(|c| c.is_major());
        if vote.interval == 0
            || vote.interval > 1_000_000
            || vote.opens_on.is_some_and(|turn| turn < world.turn || turn > 1_001_000_000)
            || (!vote.ballots.is_empty() && vote.opens_on != Some(world.turn))
            || vote
                .ballots
                .iter()
                .any(|(voter, target)| !valid_civ(*voter) || target.is_some_and(|c| c == *voter || !valid_candidate(c)))
        {
            return Err("invalid diplomatic voting clock or ballot".into());
        }
        if let Some(result) = &vote.last_result {
            if result.turn > world.turn
                || result.needed == 0
                || result.needed > 256
                || result.votes.iter().any(|(civ, n)| !valid_candidate(*civ) || *n == 0 || *n > 257)
                || result.votes.values().map(|n| u64::from(*n)).sum::<u64>() > 257
                || result.winner.is_some_and(|id| {
                    !valid_candidate(id)
                        || !world.civ(id).is_some_and(|c| c.victory.diplomatic_win)
                        || result.votes.get(&id).is_none_or(|n| *n < result.needed)
                })
                || (result.winner.is_some() && vote.opens_on.is_some())
            {
                return Err("invalid diplomatic vote result".into());
            }
        } else if vote.opens_on.is_none() {
            return Err("diplomatic voting stopped without a result".into());
        }
    }
    Ok(())
}
