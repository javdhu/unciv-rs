//! Spies: the one system that is allowed to be unfair.
//!
//! Every other subsystem here answers the same question for everyone — the
//! same tile yields the same food whoever works it. Espionage does not. A spy
//! in someone else's city takes something they cannot get back, and the
//! victim may never learn who took it.
//!
//! # Why the randomness is bounded, and where it comes from
//!
//! A steal resolves against a roll in `0..300`, adjusted by the spy's rank and
//! by any counter-intelligence in the city. Three bands: under 100 nobody
//! knows who did it, 100 to 200 the victim learns whose spy it was, 200 and
//! above the spy is caught and killed. The bands mean a good spy is *safer*
//! rather than merely luckier, which is what makes rank worth having.
//!
//! The roll comes from [`Rng::from_parts`] seeded on the turn, the civ, the
//! city and the spy's index — never from an ambient generator. Two machines
//! replaying the same game must catch the same spy on the same turn, and a
//! generator whose state depends on how many other rolls happened first cannot
//! promise that.
//!
//! City-state election and coup resolution lives in `city_states`; this
//! module owns spy travel, work, death and recruitment.

use crate::arena::Id;
use crate::rng::Rng;
use crate::ruleset::{Ruleset, Technology};
use crate::stats::Fixed;
use crate::world::city::City;
use crate::world::grid::CivId;
use crate::world::world::World;

/// A spy's rank adds this much to its skill roll, per level.
pub const RANK_SKILL_PERCENT: i32 = 30;
/// And this much to how fast it steals, per level.
pub const RANK_STEAL_PERCENT: i32 = 25;
/// Stealing a tech costs this much more than researching it.
pub const STEAL_COST_MODIFIER: Fixed = Fixed::permille(1_250);
/// Turns to establish a network before any work can start.
pub const NETWORK_TURNS: u32 = 3;
/// Turns to travel to a new posting.
pub const TRAVEL_TURNS: u32 = 1;
/// Roll at or above which the spy is caught and killed.
pub const CAUGHT_THRESHOLD: i32 = 200;
/// Roll at or above which the victim learns whose spy it was.
pub const IDENTIFIED_THRESHOLD: i32 = 100;
/// The first era that grants a spy. Before this, nobody has one, which is why
/// an ancient-era game has no espionage without anything having to disable it.
pub const FIRST_SPY_ERA: i32 = 3;

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SpyAction {
    /// At home, unassigned.
    #[default]
    Hideout,
    Moving,
    Dead,
    Surveillance,
    EstablishingNetwork,
    /// In place in a foreign city, working.
    StealingTech,
    /// In place in one of our own cities, watching for others.
    CounterIntelligence,
    /// In place in a city-state, buying votes. A city-state has no technology
    /// worth the trip, so a spy sent to one does this instead.
    RiggingElections,
    /// A coup ordered for resolution at the round boundary.
    Coup,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spy {
    /// One upward. A higher rank steals faster and is caught less.
    pub rank: u8,
    pub station: Option<Id<City>>,
    pub action: SpyAction,
    pub turns_remaining: u32,
    /// Science banked toward the next theft.
    pub progress: i32,
}

impl Spy {
    pub fn new() -> Spy {
        Spy { rank: 1, station: None, action: SpyAction::Hideout, turns_remaining: 0, progress: 0 }
    }

    pub fn kill(&mut self) {
        *self = Self { action: SpyAction::Dead, turns_remaining: 5, ..Self::new() };
    }

    /// How much the roll is shifted in this spy's favour.
    pub fn skill(&self) -> i32 {
        self.rank as i32 * RANK_SKILL_PERCENT
    }

    /// Is the spy in place and working, rather than travelling or settling in?
    pub fn is_established(&self) -> bool {
        matches!(
            self.action,
            SpyAction::StealingTech
                | SpyAction::Surveillance
                | SpyAction::CounterIntelligence
                | SpyAction::RiggingElections
                | SpyAction::Coup
        )
    }
}

impl Default for Spy {
    fn default() -> Spy {
        Spy::new()
    }
}

/// A civ's whole intelligence service.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Espionage {
    pub spies: Vec<Spy>,
    /// Eras already paid out, so reaching the same era twice does not grant
    /// two spies. Stored as the era number rather than a count, because eras
    /// can be skipped and a count would then be wrong.
    pub eras_rewarded: std::collections::BTreeSet<i32>,
}

impl Espionage {
    /// Grant a spy for each new era reached from [`FIRST_SPY_ERA`] on.
    ///
    /// Returns how many were granted this call.
    pub fn grant_for_era(&mut self, era: i32) -> usize {
        let mut granted = 0;
        for e in FIRST_SPY_ERA..=era {
            if self.eras_rewarded.insert(e) {
                self.spies.push(Spy::new());
                granted += 1;
            }
        }
        granted
    }

    pub fn idle(&self) -> impl Iterator<Item = usize> + '_ {
        self.spies.iter().enumerate().filter(|(_, s)| s.action == SpyAction::Hideout).map(|(i, _)| i)
    }

    /// Whoever we have watching this city, if anyone.
    pub fn defender_of(&self, city: Id<City>) -> Option<&Spy> {
        self.spies.iter().find(|s| s.station == Some(city) && s.action == SpyAction::CounterIntelligence)
    }
}

/// Send a spy to a city. Travelling and settling in take turns, so a spy
/// posted this turn cannot steal this turn.
pub fn assign(world: &mut World, civ: CivId, index: usize, city: Id<City>) -> bool {
    // A remembered destination may have been razed out of sight. Accept the
    // trip and discover the missing posting on arrival, not as a hidden-state
    // oracle in a command rejection.
    if world.cities.get(city).is_none() && !world.visibility.known_cities(civ).any(|c| c.id == city) {
        return false;
    }
    let Some(c) = world.civ_mut(civ) else { return false };
    if c.espionage.spies.iter().enumerate().any(|(i, s)| i != index && s.station == Some(city)) {
        return false;
    }
    let Some(spy) = c.espionage.spies.get_mut(index) else { return false };
    if spy.action == SpyAction::Dead {
        return false;
    }
    if spy.station == Some(city) {
        return true;
    }
    spy.station = Some(city);
    spy.action = SpyAction::Moving;
    spy.turns_remaining = TRAVEL_TURNS;
    spy.progress = 0;
    true
}

pub fn recall(world: &mut World, civ: CivId, index: usize) -> bool {
    let Some(spy) = world.civ_mut(civ).and_then(|c| c.espionage.spies.get_mut(index)) else { return false };
    if spy.action == SpyAction::Dead || spy.action == SpyAction::Hideout {
        return false;
    }
    *spy = Spy { rank: spy.rank, ..Spy::new() };
    true
}

pub fn new_spy(world: &World, r: &Ruleset, civ: CivId) -> Spy {
    use crate::world::conditionals::{applies, GameContext};
    let ctx = GameContext::new(r).with_world(world).for_civ(civ);
    let extra: i32 = world
        .civ(civ)
        .into_iter()
        .flat_map(|c| c.uniques.matching(crate::UniqueType::SpyStartingLevel))
        .filter(|u| applies(u, &ctx).holds())
        .filter_map(|u| u.params.first()?.parse::<i32>().ok())
        .sum();
    Spy { rank: (1 + extra).clamp(1, 3) as u8, ..Spy::new() }
}

/// Era spy rewards are global: the first civilization to enter an era grants
/// a spy to every living major, including those still researching older eras.
pub fn grant_global_era_spies(world: &mut World, r: &Ruleset) {
    let highest = world
        .civs
        .iter()
        .filter(|c| c.is_major() && c.is_alive())
        .map(|c| crate::world::policies::era_number(world, r, c.id))
        .max()
        .unwrap_or(0);
    let eras: Vec<_> = r
        .eras
        .values()
        .filter(|e| {
            e.era_number <= highest
                && e.uniques
                    .iter()
                    .any(|u| u.unique_type() == Some(crate::UniqueType::OneTimeGlobalSpiesWhenEnteringEra))
        })
        .map(|e| e.era_number)
        .collect();
    for civ in world.civ_ids().collect::<Vec<_>>() {
        if !world.civ(civ).is_some_and(|c| c.is_major() && c.is_alive()) {
            continue;
        }
        for &era in &eras {
            if world.civ(civ).unwrap().espionage.eras_rewarded.contains(&era) {
                continue;
            }
            let spy = new_spy(world, r, civ);
            let c = world.civ_mut(civ).unwrap();
            c.espionage.eras_rewarded.insert(era);
            c.espionage.spies.push(spy);
        }
    }
}

/// What a spy does once its network is up: watch at home, steal abroad.
fn settled_action(world: &World, civ: CivId, city: Id<City>) -> SpyAction {
    let Some(owner) = world.cities.get(city).map(|c| c.owner) else {
        return SpyAction::CounterIntelligence;
    };
    if owner == civ {
        return SpyAction::CounterIntelligence;
    }
    // A city-state has no technology worth crossing a map for, but it has an
    // election. Sending a spy to one to steal would waste it.
    if world.civ(owner).is_some_and(|c| c.is_city_state()) {
        return SpyAction::RiggingElections;
    }
    SpyAction::StealingTech
}

/// Technologies `target` knows and `thief` does not, in id order.
pub fn stealable(world: &World, r: &Ruleset, thief: CivId, target: CivId) -> Vec<Id<Technology>> {
    let (Some(a), Some(b)) = (world.civ(thief), world.civ(target)) else { return Vec::new() };
    b.techs.difference(&a.techs).copied().filter(|t| crate::world::research::can_research(r, a, *t).is_ok()).collect()
}

/// Building and national modifiers multiply after each side's bonuses add.
pub fn efficiency(world: &World, r: &Ruleset, civ: CivId, city_id: Id<City>) -> Fixed {
    use crate::world::conditionals::{applies, GameContext};
    let Some(city) = world.cities.get(city_id) else { return Fixed::ONE };
    let sum = |owner: CivId, kind: crate::UniqueType, uniques: Vec<&crate::Unique>| {
        let ctx = GameContext::new(r).with_world(world).for_civ(owner).in_city(city_id);
        uniques
            .into_iter()
            .filter(|u| u.unique_type() == Some(kind) && applies(u, &ctx).holds())
            .filter(|u| {
                u.params.get(1).is_some_and(|f| {
                    crate::world::filters::multi_filter(f, &|term| {
                        crate::world::filters::city_matches(world, r, city, Some(city), term)
                    })
                    .holds()
                })
            })
            .filter_map(|u| u.params.first()?.parse::<i32>().ok())
            .sum::<i32>()
    };
    let friendly = if city.owner == civ {
        crate::world::religion::city_uniques(world, r, city)
    } else {
        world.civ(civ).into_iter().flat_map(|c| c.uniques.all()).collect()
    };
    let a = sum(civ, crate::UniqueType::SpyEffectiveness, friendly);
    let b = if city.owner == civ {
        0
    } else {
        sum(city.owner, crate::UniqueType::EnemySpyEffectiveness, crate::world::religion::city_uniques(world, r, city))
    };
    (Fixed::from_fraction(i64::from(100 + a), 100) * Fixed::from_fraction(i64::from(100 + b), 100)).max(Fixed::ZERO)
}

/// Established spies observe the city and its immediate neighbours.
pub fn update_visibility(world: &mut World) {
    for (civ, centre) in watched_cities(world) {
        for tile in world.grid.within(centre, 1) {
            world.visibility.observe(civ, tile);
        }
    }
}

/// The centre of every city an established spy watches, with the spy's civ.
pub(crate) fn watched_cities(world: &World) -> Vec<(CivId, crate::world::grid::TileId)> {
    world
        .civs
        .iter()
        .filter(|c| c.is_alive())
        .flat_map(|c| {
            c.espionage
                .spies
                .iter()
                .filter(|s| s.is_established())
                .filter_map(|s| world.cities.get(s.station?).map(|city| (c.id, city.centre)))
        })
        .collect()
}

/// Queue one coup; the live turn boundary resolves its chance and consequences.
pub fn queue_coup(world: &mut World, r: &Ruleset, civ: CivId, target: CivId) -> bool {
    if !crate::world::city_states::can_coup(world, r, civ, target) {
        return false;
    }
    let Some(city) = crate::world::connections::capital_of(world, r, target)
        .or_else(|| world.civ(target).and_then(|c| c.cities.first().copied()))
    else {
        return false;
    };
    let Some(spy) = world.civ_mut(civ).and_then(|c| c.espionage.spies.iter_mut().find(|s| s.station == Some(city)))
    else {
        return false;
    };
    if spy.action == SpyAction::Coup {
        return false;
    }
    spy.action = SpyAction::Coup;
    spy.turns_remaining = 0;
    true
}

/// AI uses the same public coup estimate and queues at most one attempt per spy.
pub fn automate_coups(world: &mut World, r: &Ruleset, civ: CivId) {
    let targets: Vec<_> = world
        .civ(civ)
        .into_iter()
        .flat_map(|c| c.espionage.spies.iter().enumerate())
        .filter(|(_, s)| s.action == SpyAction::RiggingElections)
        .filter_map(|(i, s)| world.cities.get(s.station?).map(|c| (i, c.owner)))
        .collect();
    for (index, cs) in targets {
        if crate::world::city_states::visible_coup_chance(world, r, civ, cs) < 70 {
            continue;
        }
        if crate::world::city_states::ally_of(world, cs)
            .is_some_and(|ally| world.diplomacy.attitude(civ, ally).opinion() >= Fixed::whole(30))
        {
            continue;
        }
        let mut rng = Rng::from_parts(&[0x5_9C0, world.turn as u64, u64::from(civ.0), index as u64]);
        if rng.range(0, 99) < 20 {
            queue_coup(world, r, civ, cs);
        }
    }
}

/// What stealing the dearest available tech costs in banked science.
pub fn steal_cost(r: &Ruleset, techs: &[Id<Technology>], science_cost_modifier: Fixed) -> i32 {
    let dearest = techs.iter().filter_map(|t| r.technologies.get(*t)).filter_map(|t| t.cost).max().unwrap_or(0);
    let modifier = if science_cost_modifier.is_zero() { Fixed::ONE } else { science_cost_modifier };
    ((Fixed::whole(dearest) * STEAL_COST_MODIFIER) * modifier).floor().max(1) as i32
}

/// What happened to a spy this turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpyEvent {
    Coup(crate::world::city_states::CoupResult),
    Established {
        civ: CivId,
        city: Id<City>,
    },
    /// A technology changed hands. `identified` is whether the victim learned
    /// who took it — which is what turns a theft into a grievance.
    TechStolen {
        thief: CivId,
        victim: CivId,
        tech: Id<Technology>,
        identified: bool,
    },
    Caught {
        civ: CivId,
        city: Id<City>,
    },
    /// Nothing left worth taking here.
    NothingToSteal {
        civ: CivId,
        city: Id<City>,
    },
}

fn city_science(world: &World, r: &Ruleset, city: &City, difficulty: Option<&crate::ruleset::Difficulty>) -> Fixed {
    let happiness = crate::world::happiness::happiness(world, r, city.owner, difficulty);
    let golden_age = world.civ(city.owner).is_some_and(|c| c.golden_age.turns_left > 0);
    crate::world::city::city_yield_with_happiness(world, r, city, golden_age, Some(happiness)).science
}

/// One turn for every spy on the map.
///
/// Planned against the world as it is and applied in civ order, like every
/// other turn phase — a spy that steals a tech must not change what the next
/// civ's spy finds worth stealing on the same turn.
pub fn tick(world: &mut World, r: &Ruleset, science_cost_modifier: Fixed) -> Vec<SpyEvent> {
    tick_in(world, r, science_cost_modifier, None)
}

pub fn tick_in(
    world: &mut World,
    r: &Ruleset,
    science_cost_modifier: Fixed,
    difficulty: Option<&crate::ruleset::Difficulty>,
) -> Vec<SpyEvent> {
    let mut events = Vec::new();
    let turn = world.turn;

    for civ in world.civ_ids().collect::<Vec<_>>() {
        if !world.civ(civ).is_some_and(|c| c.is_major() && c.is_alive()) {
            continue;
        }
        let count = world.civ(civ).map_or(0, |c| c.espionage.spies.len());
        for i in 0..count {
            let Some(spy) = world.civ(civ).and_then(|c| c.espionage.spies.get(i)).cloned() else {
                continue;
            };
            if spy.action == SpyAction::Dead {
                let remaining = spy.turns_remaining.saturating_sub(1);
                let next =
                    if remaining == 0 { new_spy(world, r, civ) } else { Spy { turns_remaining: remaining, ..spy } };
                world.civ_mut(civ).unwrap().espionage.spies[i] = next;
                continue;
            }
            let Some(city_id) = spy.station else { continue };
            let Some(city) = world.cities.get(city_id) else {
                // The city is gone — razed, or never existed. Come home.
                if let Some(s) = world.civ_mut(civ).and_then(|c| c.espionage.spies.get_mut(i)) {
                    *s = Spy { rank: spy.rank, ..Spy::new() };
                }
                continue;
            };
            let victim = city.owner;

            match spy.action {
                SpyAction::Moving | SpyAction::EstablishingNetwork => {
                    let remaining = spy.turns_remaining.saturating_sub(1);
                    let next = if remaining > 0 {
                        Spy { turns_remaining: remaining, ..spy.clone() }
                    } else if spy.action == SpyAction::Moving && victim == civ {
                        events.push(SpyEvent::Established { civ, city: city_id });
                        Spy { action: SpyAction::CounterIntelligence, turns_remaining: 10, ..spy.clone() }
                    } else if spy.action == SpyAction::Moving {
                        Spy { action: SpyAction::EstablishingNetwork, turns_remaining: NETWORK_TURNS, ..spy.clone() }
                    } else {
                        events.push(SpyEvent::Established { civ, city: city_id });
                        Spy { action: settled_action(world, civ, city_id), turns_remaining: 0, ..spy.clone() }
                    };
                    if let Some(s) = world.civ_mut(civ).and_then(|c| c.espionage.spies.get_mut(i)) {
                        *s = next;
                    }
                }
                SpyAction::Coup => {
                    let result = crate::world::city_states::attempt_coup(world, r, civ, victim);
                    if result == crate::world::city_states::CoupResult::Impossible {
                        world.civ_mut(civ).unwrap().espionage.spies[i].action = settled_action(world, civ, city_id);
                    }
                    events.push(SpyEvent::Coup(result));
                }
                SpyAction::RiggingElections => {
                    // The election itself is a city-state event, held on its
                    // own schedule; the spy only has to be standing there.
                    // But if the city stopped being a city-state's — captured
                    // by someone — there is nothing left to rig.
                    if !world.civ(victim).is_some_and(|c| c.is_city_state()) {
                        let next = settled_action(world, civ, city_id);
                        if let Some(s) = world.civ_mut(civ).and_then(|c| c.espionage.spies.get_mut(i)) {
                            s.action = next;
                            s.progress = 0;
                        }
                    }
                }
                SpyAction::CounterIntelligence => {
                    world.civ_mut(civ).unwrap().espionage.spies[i].turns_remaining =
                        spy.turns_remaining.saturating_sub(1);
                    // A city we were watching that is no longer ours is a
                    // city to rob, not to guard.
                    if victim != civ {
                        if let Some(s) = world.civ_mut(civ).and_then(|c| c.espionage.spies.get_mut(i)) {
                            s.action = SpyAction::StealingTech;
                            s.progress = 0;
                        }
                    }
                }
                SpyAction::Hideout | SpyAction::Dead => {}
                SpyAction::Surveillance => {
                    if victim == civ || world.civ(victim).is_some_and(|c| c.is_city_state()) {
                        let action = settled_action(world, civ, city_id);
                        world.civ_mut(civ).unwrap().espionage.spies[i].action = action;
                        continue;
                    }
                    if !stealable(world, r, civ, victim).is_empty()
                        && city_science(world, r, city, difficulty) > Fixed::ZERO
                    {
                        world.civ_mut(civ).unwrap().espionage.spies[i].action = SpyAction::StealingTech;
                    }
                }
                SpyAction::StealingTech => {
                    // Cities change hands. A spy posted to a rival's city that
                    // we have since captured was left standing in our own
                    // streets, stealing from itself: `stealable(civ, civ)` is
                    // empty, so it reported nothing to steal every turn
                    // forever and never went anywhere else. Found by a spy
                    // whose progress froze at 86 for two hundred and fifty
                    // turns.
                    if victim == civ || world.civ(victim).is_some_and(|c| c.is_city_state()) {
                        let next = settled_action(world, civ, city_id);
                        if let Some(s) = world.civ_mut(civ).and_then(|c| c.espionage.spies.get_mut(i)) {
                            s.action = next;
                            s.progress = 0;
                        }
                        continue;
                    }
                    let techs = stealable(world, r, civ, victim);
                    if techs.is_empty() || city_science(world, r, city, difficulty) <= Fixed::ZERO {
                        events.push(SpyEvent::NothingToSteal { civ, city: city_id });
                        if let Some(s) = world.civ_mut(civ).and_then(|c| c.espionage.spies.get_mut(i)) {
                            s.action = SpyAction::Surveillance;
                            s.turns_remaining = 0;
                        }
                        continue;
                    }
                    // Progress is the city's own science, turned against it.
                    let science = city_science(world, r, city, difficulty);
                    let rate = Fixed::whole(spy.rank as i32 * RANK_STEAL_PERCENT + 75);
                    let gained = ((science * rate).checked_div(Fixed::whole(100)).unwrap_or(Fixed::ZERO)
                        * efficiency(world, r, civ, city_id))
                    .floor()
                    .max(0) as i32;
                    let progress = spy.progress.saturating_add(gained);
                    let cost = steal_cost(r, &techs, science_cost_modifier);
                    if progress < cost {
                        if let Some(s) = world.civ_mut(civ).and_then(|c| c.espionage.spies.get_mut(i)) {
                            s.progress = progress;
                            s.turns_remaining = if gained > 0 {
                                ((i64::from(cost - progress) + i64::from(gained) - 1) / i64::from(gained)) as u32
                            } else {
                                0
                            };
                        }
                        continue;
                    }

                    // Seeded from the game's own state, so a replay catches the
                    // same spy on the same turn.
                    let mut rng = Rng::from_parts(&[turn as u64, civ.0 as u64, city_id.index() as u64, i as u64]);
                    let defence =
                        world.civ(victim).and_then(|c| c.espionage.defender_of(city_id)).map_or(0, |d| d.skill());
                    let roll = rng.range(0, 299) as i32 - spy.skill() + defence;

                    // Ties broken by id so the same techs are taken in the
                    // same order everywhere; Unciv picks at random.
                    let tech = techs[rng.range(0, techs.len() as i64 - 1) as usize];

                    if roll >= CAUGHT_THRESHOLD {
                        events.push(SpyEvent::Caught { civ, city: city_id });
                        if let Some(s) = world.civ_mut(civ).and_then(|c| c.espionage.spies.get_mut(i)) {
                            s.kill(); // replacement recruited after five turns
                        }
                        // The catcher learns from it.
                        if let Some(d) = world.civ_mut(victim).and_then(|c| {
                            c.espionage
                                .spies
                                .iter_mut()
                                .find(|s| s.station == Some(city_id) && s.action == SpyAction::CounterIntelligence)
                        }) {
                            d.rank = d.rank.saturating_add(1).min(3);
                        }
                        continue;
                    }

                    if let Some(c) = world.civ_mut(civ) {
                        crate::world::research::grant_technology(c, r, tech);
                        if let Some(s) = c.espionage.spies.get_mut(i) {
                            s.progress = 0;
                            s.turns_remaining = 0;
                            s.rank = s.rank.saturating_add(1).min(3);
                        }
                    }
                    crate::world::effects::on_technology_researched(world, r, civ, tech, science_cost_modifier);
                    events.push(SpyEvent::TechStolen {
                        thief: civ,
                        victim,
                        tech,
                        identified: roll >= IDENTIFIED_THRESHOLD,
                    });
                }
            }
        }
    }
    events
}

/// A bilateral promise remains binding until its known expiry. Hidden research
/// levels must not change whether a human command is accepted.
pub fn keeps_promise_not_to_spy(world: &World, _r: &Ruleset, civ: CivId, other: CivId) -> bool {
    use crate::world::diplomacy::{Demand, Flag};
    world.diplomacy.has_flag(other, civ, Flag::Agreed(Demand::DontSpyOnUs))
}

/// Assign available spies to research, elections, or home defence.
pub fn postings(world: &World, r: &Ruleset, civ: CivId) -> Vec<(usize, Id<City>)> {
    let Some(c) = world.civ(civ) else { return Vec::new() };
    let idle: Vec<usize> = c
        .espionage
        .spies
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            matches!(s.action, SpyAction::Hideout | SpyAction::Surveillance)
                || (s.action == SpyAction::CounterIntelligence && s.turns_remaining == 0)
                || (s.action == SpyAction::RiggingElections
                    && s.station
                        .and_then(|id| world.cities.get(id))
                        .is_some_and(|city| world.diplomacy.at_war(civ, city.owner)))
        })
        .map(|(i, _)| i)
        .collect();
    if idle.is_empty() {
        return Vec::new();
    }

    let mut targets: Vec<(i32, Id<City>)> = Vec::new();
    for other in world.civ_ids() {
        if other == civ || !world.civ(other).is_some_and(|x| x.is_major() && x.is_alive()) {
            continue;
        }
        if stealable(world, r, civ, other).is_empty() || keeps_promise_not_to_spy(world, r, civ, other) {
            continue;
        }
        for (id, city) in world.cities.iter() {
            if city.owner != other || !world.visibility.has_explored(civ, city.centre) {
                continue;
            }
            let science = crate::world::city::city_yield(&world.grid, r, city).science;
            targets.push((science.floor() as i32, id));
        }
    }
    // Richest first, ties by city id, so two civs with identical maps pick the
    // same city.
    targets.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.index().cmp(&b.1.index())));

    let mut out = Vec::new();
    let mut taken: std::collections::BTreeSet<Id<City>> = c.espionage.spies.iter().filter_map(|s| s.station).collect();

    let mut idle = idle.into_iter();
    // The second spy onward guards home, once there is something to guard.
    let home = crate::world::connections::capital_of(world, r, civ);
    if c.espionage.spies.len() > 1 {
        if let Some(home) = home {
            if c.espionage.defender_of(home).is_none() && !taken.contains(&home) {
                if let Some(i) = idle.next() {
                    taken.insert(home);
                    out.push((i, home));
                }
            }
        }
    }
    let mut minor_targets: Vec<_> = world
        .cities
        .iter()
        .filter(|(_, city)| {
            world.civ(city.owner).is_some_and(|c| c.is_city_state() && c.is_alive())
                && world.diplomacy.have_met(civ, city.owner)
                && !world.diplomacy.at_war(civ, city.owner)
                && world.visibility.has_explored(civ, city.centre)
                && !city.razing
        })
        .map(|(id, city)| (world.diplomacy.influence(city.owner, civ), id))
        .collect();
    minor_targets.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    for i in idle {
        if let Some(station) = c.espionage.spies[i].station {
            taken.remove(&station);
        }
        let next = targets
            .iter()
            .find(|(_, id)| !taken.contains(id))
            .map(|(_, id)| *id)
            .or_else(|| minor_targets.iter().find(|(_, id)| !taken.contains(id)).map(|(_, id)| *id))
            .or_else(|| home.filter(|id| !taken.contains(id)));
        let Some(city) = next else { break };
        taken.insert(city);
        out.push((i, city));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::entities::Civilization;
    use crate::world::grid::TileGrid;

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("rules/gods-and-kings").expect("readable").expect("loads")
    }

    fn world(r: &Ruleset) -> (World, Id<City>, Id<City>) {
        let g = TileGrid::new(16, 16, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        let mut w =
            World::new(g, vec![Civilization::new(CivId(0), nation, true), Civilization::new(CivId(1), nation, true)]);
        let ours = w.found_city_in(r, City::found("Rome", CivId(0), w.grid.id_at(3, 3).unwrap()));
        let theirs = w.found_city_in(r, City::found("Babylon", CivId(1), w.grid.id_at(12, 12).unwrap()));
        w.cities.get_mut(theirs).unwrap().population = 12;
        w.visibility.explore(CivId(0), w.cities.get(theirs).unwrap().centre);
        (w, ours, theirs)
    }

    /// Give civ 1 techs civ 0 does not have.
    fn teach(w: &mut World, r: &Ruleset, civ: CivId, n: usize) -> Vec<Id<Technology>> {
        let ids: Vec<_> = r.technologies.ids().take(n).collect();
        for t in &ids {
            w.civ_mut(civ).unwrap().techs.insert(*t);
        }
        ids
    }

    #[test]
    fn spies_arrive_with_the_eras_and_only_once_each() {
        let mut e = Espionage::default();
        assert_eq!(e.grant_for_era(FIRST_SPY_ERA - 1), 0, "too early for spies");
        assert_eq!(e.grant_for_era(FIRST_SPY_ERA), 1);
        assert_eq!(e.grant_for_era(FIRST_SPY_ERA), 0, "the same era pays once");
        assert_eq!(e.grant_for_era(FIRST_SPY_ERA + 2), 2, "and skipped eras still pay");
        assert_eq!(e.spies.len(), 3);
    }

    #[test]
    fn a_spy_travels_and_settles_in_before_it_can_work() {
        let r = ruleset();
        let (mut w, _, theirs) = world(&r);
        teach(&mut w, &r, CivId(1), 4);
        w.civ_mut(CivId(0)).unwrap().espionage.spies.push(Spy::new());
        assert!(assign(&mut w, CivId(0), 0, theirs));

        let state = |w: &World| {
            let s = &w.civ(CivId(0)).unwrap().espionage.spies[0];
            (s.action, s.turns_remaining)
        };
        assert_eq!(state(&w), (SpyAction::Moving, TRAVEL_TURNS));

        tick(&mut w, &r, Fixed::ONE);
        assert_eq!(state(&w), (SpyAction::EstablishingNetwork, NETWORK_TURNS));
        for _ in 0..NETWORK_TURNS {
            tick(&mut w, &r, Fixed::ONE);
        }
        assert_eq!(state(&w).0, SpyAction::StealingTech, "in place at last");
        assert!(w.civ(CivId(0)).unwrap().espionage.spies[0].is_established());
    }

    #[test]
    fn a_spy_in_our_own_city_watches_instead_of_stealing() {
        let r = ruleset();
        let (mut w, ours, _) = world(&r);
        w.civ_mut(CivId(0)).unwrap().espionage.spies.push(Spy::new());
        assign(&mut w, CivId(0), 0, ours);
        for _ in 0..(TRAVEL_TURNS + NETWORK_TURNS + 1) {
            tick(&mut w, &r, Fixed::ONE);
        }
        assert_eq!(w.civ(CivId(0)).unwrap().espionage.spies[0].action, SpyAction::CounterIntelligence);
        assert!(w.civ(CivId(0)).unwrap().espionage.defender_of(ours).is_some());
    }

    #[test]
    fn only_what_they_know_and_we_do_not_can_be_taken() {
        let r = ruleset();
        let (mut w, _, _) = world(&r);
        let theirs = teach(&mut w, &r, CivId(1), 5);
        assert_eq!(stealable(&w, &r, CivId(0), CivId(1)).len(), 1, "only Agriculture has met prerequisites");

        // Learn three of them ourselves and only two are left worth taking.
        for t in theirs.iter().take(3) {
            w.civ_mut(CivId(0)).unwrap().techs.insert(*t);
        }
        assert_eq!(stealable(&w, &r, CivId(0), CivId(1)).len(), 2);
        assert!(stealable(&w, &r, CivId(1), CivId(0)).is_empty(), "we know nothing they do not");
    }

    #[test]
    fn a_theft_eventually_lands_and_the_tech_is_ours() {
        let r = ruleset();
        let (mut w, _, theirs) = world(&r);
        teach(&mut w, &r, CivId(1), 6);
        w.civ_mut(CivId(0)).unwrap().espionage.spies.push(Spy::new());
        assign(&mut w, CivId(0), 0, theirs);

        let mut stolen = None;
        for turn in 1..=400 {
            w.turn = turn;
            for e in tick(&mut w, &r, Fixed::ONE) {
                if let SpyEvent::TechStolen { tech, thief, victim, .. } = e {
                    assert_eq!((thief, victim), (CivId(0), CivId(1)));
                    stolen.get_or_insert((turn, tech));
                }
            }
            if stolen.is_some() {
                break;
            }
        }
        let (turn, tech) = stolen.expect("a spy in a size-twelve city should get something");
        println!("stole on turn {turn}");
        assert!(w.civ(CivId(0)).unwrap().techs.contains(&tech), "and we kept it");
    }

    #[test]
    fn nothing_is_taken_from_a_civ_that_knows_nothing_new() {
        let r = ruleset();
        let (mut w, _, theirs) = world(&r);
        w.civ_mut(CivId(0)).unwrap().espionage.spies.push(Spy::new());
        assign(&mut w, CivId(0), 0, theirs);
        for _ in 0..20 {
            for e in tick(&mut w, &r, Fixed::ONE) {
                assert!(!matches!(e, SpyEvent::TechStolen { .. }), "there was nothing there to take");
            }
        }
    }

    #[test]
    fn the_same_game_catches_the_same_spy_on_the_same_turn() {
        let r = ruleset();
        let (mut a, _, theirs) = world(&r);
        teach(&mut a, &r, CivId(1), 12);
        a.civ_mut(CivId(0)).unwrap().espionage.spies.push(Spy::new());
        assign(&mut a, CivId(0), 0, theirs);
        let mut b = a.clone();

        let run = |w: &mut World| {
            let mut log = Vec::new();
            for turn in 1..=300 {
                w.turn = turn;
                for e in tick(w, &r, Fixed::ONE) {
                    log.push((turn, format!("{e:?}")));
                }
            }
            log
        };
        let first = run(&mut a);
        let second = run(&mut b);
        assert_eq!(first, second, "espionage must replay exactly, or a saved game diverges");
        assert!(!first.is_empty(), "and something should actually have happened");
        assert_eq!(a, b);
    }

    #[test]
    fn a_caught_spy_dies_and_loses_its_rank() {
        let r = ruleset();
        let (mut w, _, theirs) = world(&r);
        teach(&mut w, &r, CivId(1), 12);
        w.civ_mut(CivId(0)).unwrap().espionage.spies.push(Spy {
            rank: 2,
            station: Some(theirs),
            action: SpyAction::StealingTech,
            progress: 100_000,
            turns_remaining: 0,
        });
        let mut caught = false;
        for turn in 1..=300 {
            let mut attempt = w.clone();
            attempt.turn = turn;
            if tick(&mut attempt, &r, Fixed::ONE).iter().any(|e| matches!(e, SpyEvent::Caught { .. })) {
                let spy = &attempt.civ(CivId(0)).unwrap().espionage.spies[0];
                assert_eq!(spy.rank, 1);
                assert_eq!(spy.station, None);
                assert_eq!(spy.action, SpyAction::Dead);
                assert_eq!(spy.turns_remaining, 5);
                caught = true;
                break;
            }
        }
        assert!(caught, "deterministic theft outcomes include death");
    }

    #[test]
    fn rank_makes_a_spy_safer_not_merely_luckier() {
        // The bands are what make rank worth having: the same roll that gets
        // a raw spy killed leaves a veteran merely identified.
        let raw = Spy { rank: 1, ..Spy::new() };
        let veteran = Spy { rank: 3, ..Spy::new() };
        assert!(veteran.skill() > raw.skill());
        // Rank one shifts the roll by 30, rank three by 90, so any roll from
        // 230 to 289 kills the novice and spares the veteran.
        let roll = 250;
        assert!(roll - raw.skill() >= CAUGHT_THRESHOLD, "the raw spy is caught");
        assert!(roll - veteran.skill() < CAUGHT_THRESHOLD, "the veteran is not");
        assert!(roll - veteran.skill() >= IDENTIFIED_THRESHOLD, "though it is seen, which is the price of the attempt");
    }

    #[test]
    fn counter_intelligence_makes_a_city_harder_to_rob() {
        let r = ruleset();
        let (mut w, _, theirs) = world(&r);
        // A defender in the target city shifts every roll against the thief.
        let mut defender = Spy::new();
        defender.rank = 3;
        defender.station = Some(theirs);
        defender.action = SpyAction::CounterIntelligence;
        w.civ_mut(CivId(1)).unwrap().espionage.spies.push(defender);

        let d = w.civ(CivId(1)).unwrap().espionage.defender_of(theirs).expect("on watch");
        assert_eq!(d.skill(), 3 * RANK_SKILL_PERCENT);
        assert!(d.skill() > Spy::new().skill(), "a veteran on watch should hurt more than a raw one");
    }

    /// Cities change hands, and a spy has to notice.
    ///
    /// A spy posted to a rival's city that we then captured was left standing
    /// in our own streets in `StealingTech`: `stealable(civ, civ)` is empty,
    /// so it reported nothing to steal every turn and never moved again.
    /// Found in a 300-turn game by a spy whose progress froze at 86.
    #[test]
    fn a_spy_reacts_to_the_city_changing_hands() {
        let r = ruleset();
        let (mut w, ours, theirs) = world(&r);
        teach(&mut w, &r, CivId(1), 6);
        w.civ_mut(CivId(0)).unwrap().espionage.spies.push(Spy::new());
        assign(&mut w, CivId(0), 0, theirs);
        for _ in 0..(TRAVEL_TURNS + NETWORK_TURNS + 1) {
            tick(&mut w, &r, Fixed::ONE);
        }
        assert_eq!(w.civ(CivId(0)).unwrap().espionage.spies[0].action, SpyAction::StealingTech);

        // We take the city. The spy is now at home and should be guarding it.
        w.cities.get_mut(theirs).unwrap().owner = CivId(0);
        tick(&mut w, &r, Fixed::ONE);
        assert_eq!(w.civ(CivId(0)).unwrap().espionage.spies[0].action, SpyAction::CounterIntelligence);

        // And the reverse: a city we were guarding that we then lost.
        let mut guard = Spy::new();
        guard.station = Some(ours);
        guard.action = SpyAction::CounterIntelligence;
        w.civ_mut(CivId(0)).unwrap().espionage.spies.push(guard);
        w.cities.get_mut(ours).unwrap().owner = CivId(1);
        tick(&mut w, &r, Fixed::ONE);
        assert_eq!(
            w.civ(CivId(0)).unwrap().espionage.spies[1].action,
            SpyAction::StealingTech,
            "a city that is no longer ours is one to rob, not to guard"
        );
    }

    #[test]
    fn a_spy_with_nothing_left_to_steal_keeps_watching_for_new_technology() {
        let r = ruleset();
        let (mut w, _, theirs) = world(&r);
        teach(&mut w, &r, CivId(1), 4);
        w.civ_mut(CivId(0)).unwrap().espionage.spies.push(Spy::new());
        assign(&mut w, CivId(0), 0, theirs);
        for _ in 0..(TRAVEL_TURNS + NETWORK_TURNS + 1) {
            tick(&mut w, &r, Fixed::ONE);
        }
        assert_eq!(w.civ(CivId(0)).unwrap().espionage.spies[0].action, SpyAction::StealingTech);

        // Learn everything they know and there is nothing left there.
        let theirs_techs: Vec<_> = w.civ(CivId(1)).unwrap().techs.iter().copied().collect();
        for t in theirs_techs {
            w.civ_mut(CivId(0)).unwrap().techs.insert(t);
        }
        tick(&mut w, &r, Fixed::ONE);
        let spy = &w.civ(CivId(0)).unwrap().espionage.spies[0];
        assert_eq!(spy.station, Some(theirs));
        assert_eq!(spy.action, SpyAction::Surveillance);
    }

    #[test]
    fn a_spy_whose_city_vanishes_comes_home() {
        let r = ruleset();
        let (mut w, _, theirs) = world(&r);
        teach(&mut w, &r, CivId(1), 4);
        w.civ_mut(CivId(0)).unwrap().espionage.spies.push(Spy::new());
        assign(&mut w, CivId(0), 0, theirs);
        w.cities.remove(theirs);

        tick(&mut w, &r, Fixed::ONE);
        let spy = &w.civ(CivId(0)).unwrap().espionage.spies[0];
        assert_eq!(spy.station, None);
        assert_eq!(spy.action, SpyAction::Hideout);
    }

    #[test]
    fn spies_are_posted_abroad_before_they_are_kept_at_home() {
        let r = ruleset();
        let (mut w, _, theirs) = world(&r);
        teach(&mut w, &r, CivId(1), 6);
        w.civ_mut(CivId(0)).unwrap().espionage.spies.push(Spy::new());
        let posted = postings(&w, &r, CivId(0));
        assert_eq!(posted, vec![(0, theirs)], "one spy goes where the techs are");
    }

    #[test]
    fn a_civ_with_nothing_to_steal_guards_home() {
        let r = ruleset();
        let (mut w, _, _) = world(&r);
        w.civ_mut(CivId(0)).unwrap().espionage.spies.push(Spy::new());
        assert_eq!(
            postings(&w, &r, CivId(0)).first().map(|(_, city)| *city),
            w.civ(CivId(0)).unwrap().cities.first().copied(),
            "without a foreign task, guard home"
        );
    }
    #[test]
    fn death_waits_five_turns_and_replacement_uses_starting_rank() {
        let r = ruleset();
        let (mut w, ours, _) = world(&r);
        w.civ_mut(CivId(0)).unwrap().uniques.insert(crate::Unique::parse("New spies start with [1] level(s)"));
        let mut spy = Spy::new();
        spy.rank = 3;
        spy.kill();
        w.civ_mut(CivId(0)).unwrap().espionage.spies.push(spy);
        for remaining in (1..=5).rev() {
            assert_eq!(w.civ(CivId(0)).unwrap().espionage.spies[0].turns_remaining, remaining);
            assert!(!assign(&mut w, CivId(0), 0, ours));
            assert!(!recall(&mut w, CivId(0), 0));
            assert!(w.civ(CivId(0)).unwrap().espionage.idle().next().is_none());
            tick(&mut w, &r, Fixed::ONE);
        }
        let spy = &w.civ(CivId(0)).unwrap().espionage.spies[0];
        assert_eq!(spy.action, SpyAction::Hideout);
        assert_eq!(spy.rank, 2);
    }

    #[test]
    fn global_era_rewards_reach_behind_civs_once() {
        let r = ruleset();
        let (mut w, _, _) = world(&r);
        w.civ_mut(CivId(1)).unwrap().techs.insert(r.technologies.id("Acoustics").unwrap());
        grant_global_era_spies(&mut w, &r);
        assert_eq!(w.civ(CivId(0)).unwrap().espionage.spies.len(), 1);
        assert_eq!(w.civ(CivId(1)).unwrap().espionage.spies.len(), 1);
        grant_global_era_spies(&mut w, &r);
        assert_eq!(w.civ(CivId(0)).unwrap().espionage.spies.len(), 1);
    }

    #[test]
    fn duplicate_postings_and_repeat_assignment_preserve_travel() {
        let r = ruleset();
        let (mut w, ours, theirs) = world(&r);
        w.civ_mut(CivId(0)).unwrap().espionage.spies.extend([Spy::new(), Spy::new()]);
        assert!(assign(&mut w, CivId(0), 0, theirs));
        tick(&mut w, &r, Fixed::ONE);
        assert!(assign(&mut w, CivId(0), 0, theirs));
        assert_eq!(w.civ(CivId(0)).unwrap().espionage.spies[0].turns_remaining, 3);
        assert!(!assign(&mut w, CivId(0), 1, theirs));
        assert!(assign(&mut w, CivId(0), 1, ours));
        tick(&mut w, &r, Fixed::ONE);
        assert_eq!(w.civ(CivId(0)).unwrap().espionage.spies[1].action, SpyAction::CounterIntelligence);
        assert!(recall(&mut w, CivId(0), 0));
        assert!(assign(&mut w, CivId(0), 1, theirs));
    }

    #[test]
    fn established_spies_reveal_one_ring_and_recall_removes_live_sight() {
        let r = ruleset();
        let (mut w, _, theirs) = world(&r);
        w.civ_mut(CivId(0)).unwrap().espionage.spies.push(Spy::new());
        assign(&mut w, CivId(0), 0, theirs);
        crate::world::turn::update_visibility_in(&mut w, &r);
        let centre = w.cities.get(theirs).unwrap().centre;
        assert!(!w.visibility.can_see(CivId(0), centre));
        for _ in 0..4 {
            tick(&mut w, &r, Fixed::ONE);
        }
        crate::world::turn::update_visibility_in(&mut w, &r);
        for tile in w.grid.within(centre, 1) {
            assert!(w.visibility.can_see(CivId(0), tile));
        }
        recall(&mut w, CivId(0), 0);
        crate::world::turn::update_visibility_in(&mut w, &r);
        assert!(!w.visibility.can_see(CivId(0), centre));
        assert!(w.visibility.has_explored(CivId(0), centre));
    }

    #[test]
    fn anti_spy_buildings_stack_locally_and_globally() {
        let r = ruleset();
        let (mut w, ours, theirs) = world(&r);
        for name in ["Constabulary", "Police Station"] {
            w.cities.get_mut(theirs).unwrap().built.insert(r.buildings.id(name).unwrap());
        }
        crate::world::city::rebuild_city_uniques(&r, w.cities.get_mut(theirs).unwrap());
        crate::world::policies::rebuild_uniques(&mut w, &r, CivId(1));
        assert_eq!(efficiency(&w, &r, CivId(0), theirs), Fixed::permille(500));
        w.cities.get_mut(theirs).unwrap().built.insert(r.buildings.id("Great Firewall").unwrap());
        crate::world::city::rebuild_city_uniques(&r, w.cities.get_mut(theirs).unwrap());
        crate::world::policies::rebuild_uniques(&mut w, &r, CivId(1));
        assert_eq!(efficiency(&w, &r, CivId(0), theirs), Fixed::ZERO);
        assert_eq!(efficiency(&w, &r, CivId(0), ours), Fixed::ONE);
        let other = w.found_city(City::found("Other", CivId(1), w.grid.id_at(14, 2).unwrap()));
        assert_eq!(efficiency(&w, &r, CivId(0), other), Fixed::permille(750));
    }

    #[cfg(feature = "save")]
    #[test]
    fn saved_death_countdown_resumes_and_duplicate_station_is_rejected() {
        let r = ruleset();
        let (mut w, ours, _) = world(&r);
        let mut spy = Spy::new();
        spy.kill();
        w.civ_mut(CivId(0)).unwrap().espionage.spies.push(spy);
        let mut restored = crate::save::decode(&crate::save::encode(&w, &r).unwrap(), &r).unwrap();
        for _ in 0..5 {
            assert_eq!(tick(&mut w, &r, Fixed::ONE), tick(&mut restored, &r, Fixed::ONE));
        }
        assert_eq!(w, restored);
        assert!(assign(&mut w, CivId(0), 0, ours));
        let spy = w.civ(CivId(0)).unwrap().espionage.spies[0].clone();
        w.civ_mut(CivId(0)).unwrap().espionage.spies.push(spy);
        assert!(crate::save::validate(&w, &r).is_err());
    }
    #[test]
    fn commands_use_remembered_city_and_cannot_duplicate_or_revive_dead_spies() {
        use crate::world::commands::{execute, Command};
        let r = ruleset();
        let (mut w, _, theirs) = world(&r);
        w.visibility.observe(CivId(0), w.cities.get(theirs).unwrap().centre);
        crate::world::fog::capture_observations(&mut w, &r);
        w.visibility.clear_visible(CivId(0));
        w.civ_mut(CivId(0)).unwrap().espionage.spies.extend([Spy::new(), Spy::new()]);
        execute(&mut w, &r, CivId(0), Command::AssignSpy { spy: 0, city: theirs }).unwrap();
        let before = w.clone();
        assert!(execute(&mut w, &r, CivId(0), Command::AssignSpy { spy: 1, city: theirs }).is_err());
        assert_eq!(w, before);
        execute(&mut w, &r, CivId(0), Command::WithdrawSpy { spy: 0 }).unwrap();
        w.civ_mut(CivId(0)).unwrap().espionage.spies[0].kill();
        let before = w.clone();
        assert!(execute(&mut w, &r, CivId(0), Command::WithdrawSpy { spy: 0 }).is_err());
        assert!(execute(&mut w, &r, CivId(0), Command::AssignSpy { spy: 0, city: theirs }).is_err());
        assert_eq!(w, before);
    }

    #[test]
    fn surveillance_resumes_when_a_researchable_technology_appears() {
        let r = ruleset();
        let (mut w, _, theirs) = world(&r);
        w.civ_mut(CivId(0)).unwrap().espionage.spies.push(Spy {
            station: Some(theirs),
            action: SpyAction::Surveillance,
            ..Spy::new()
        });
        tick(&mut w, &r, Fixed::ONE);
        assert_eq!(w.civ(CivId(0)).unwrap().espionage.spies[0].action, SpyAction::Surveillance);
        w.civ_mut(CivId(1)).unwrap().techs.insert(r.technologies.id("Agriculture").unwrap());
        tick(&mut w, &r, Fixed::ONE);
        assert_eq!(w.civ(CivId(0)).unwrap().espionage.spies[0].action, SpyAction::StealingTech);
        assert_eq!(w.civ(CivId(0)).unwrap().espionage.spies[0].progress, 0);
    }
    #[test]
    fn spying_promise_rejects_identically_with_unseen_technology_changes() {
        use crate::world::commands::{execute, Command};
        use crate::world::diplomacy::{Demand, Flag};
        let r = ruleset();
        let (mut w, _, theirs) = world(&r);
        w.visibility.observe(CivId(0), w.cities.get(theirs).unwrap().centre);
        crate::world::fog::capture_observations(&mut w, &r);
        w.visibility.clear_visible(CivId(0));
        w.civ_mut(CivId(0)).unwrap().espionage.spies.push(Spy::new());
        w.diplomacy.set_flag(CivId(1), CivId(0), Flag::Agreed(Demand::DontSpyOnUs), 20);
        let mut advanced = w.clone();
        advanced.civ_mut(CivId(1)).unwrap().techs.extend(r.technologies.ids());
        for world in [&mut w, &mut advanced] {
            let before = world.clone();
            assert!(execute(world, &r, CivId(0), Command::AssignSpy { spy: 0, city: theirs }).is_err());
            assert_eq!(*world, before);
        }
        for _ in 0..20 {
            w.diplomacy.tick();
        }
        assert!(execute(&mut w, &r, CivId(0), Command::AssignSpy { spy: 0, city: theirs }).is_ok());
    }
}
