//! Persistent city-state identity, timed gifts, and diplomatic marriage.
use super::{
    city_states::{self, Relationship},
    conditionals::{applies, GameContext},
    construction::{Construction, CostModifiers},
    grid::CivId,
    City, Unit, World,
};
use crate::{
    arena::Id,
    rng::Rng,
    ruleset::{BaseUnit, Ruleset, TileResource},
    stats::Fixed,
    UniqueType,
};
use std::collections::BTreeMap;

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Personality {
    Friendly,
    #[default]
    Neutral,
    Hostile,
    Irrational,
}

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CityStateData {
    pub initialized: bool,
    pub personality: Personality,
    pub unique_luxury: Option<Id<TileResource>>,
    pub unique_unit: Option<Id<BaseUnit>>,
    #[cfg_attr(feature = "serde", serde(with = "crate::serialization::map_as_entries"))]
    pub military_gifts: BTreeMap<CivId, u32>,
    pub tracked_ally: Option<CivId>,
    pub allied_turns: u32,
    /// Austria's marriage prevents subsequent liberation of this state.
    pub absorbed: bool,
    pub minor_attacks: u32,
    #[cfg_attr(feature = "serde", serde(with = "crate::serialization::map_as_entries"))]
    pub wary_until: BTreeMap<CivId, u32>,
    /// The Patronage finisher has one clock per recipient, independent of
    /// which of its allied city-states supplies the eventual great person.
    pub great_person_gift: Option<u32>,
}

pub fn initialize(world: &mut World, r: &Ruleset) {
    for cs in world.civ_ids().collect::<Vec<_>>() {
        let Some(c) = world.civ(cs) else { continue };
        let Some(kind) = c.city_state_type.and_then(|id| r.city_state_types.get(id)) else { continue };
        if c.city_state.initialized {
            continue;
        }
        let mut rng = Rng::from_parts(&[
            0xC57A7E,
            cs.0 as u64,
            c.nation.index() as u64,
            world.grid.len() as u64,
            c.cities.first().and_then(|id| world.cities.get(*id)).map_or(0, |c| c.centre.0 as u64),
        ]);
        let has = |t| {
            kind.friend_bonus_uniques
                .iter()
                .chain(&kind.ally_bonus_uniques)
                .any(|s| crate::Unique::parse(s).unique_type() == Some(t))
        };
        let personality = match r.nations[c.nation].personality.as_deref() {
            Some("Friendly") => Personality::Friendly,
            Some("Neutral") => Personality::Neutral,
            Some("Hostile") => Personality::Hostile,
            Some("Irrational") => Personality::Irrational,
            _ => [Personality::Friendly, Personality::Neutral, Personality::Hostile, Personality::Irrational]
                [rng.range(0, 3) as usize],
        };
        let luxury = if has(UniqueType::CityStateUniqueLuxury) {
            let choices: Vec<_> = r
                .resources
                .iter()
                .filter(|(_, res)| {
                    res.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::CityStateOnlyResource))
                })
                .map(|(id, _)| id)
                .collect();
            choose(&choices, &mut rng)
        } else {
            None
        };
        let unit = if has(UniqueType::CityStateMilitaryUnits) {
            let choices: Vec<_> = r
                .units
                .iter()
                .filter(|(_, u)| {
                    let Some(nation) = u.unique_to.as_deref().and_then(|n| r.nations.id(n)) else { return false };
                    !world.civs.iter().any(|c| c.nation == nation)
                        && (u.strength.unwrap_or(0) > 0 || u.ranged_strength.unwrap_or(0) > 0)
                        && r.unit_types.by_name(&u.unit_type).is_some_and(|t| t.movement_type == "Land")
                        && u.required_tech
                            .as_deref()
                            .and_then(|t| r.technologies.id(t))
                            .is_some_and(|t| !c.techs.contains(&t))
                })
                .map(|(id, _)| id)
                .collect();
            choose(&choices, &mut rng)
        } else {
            None
        };
        let data = &mut world.civ_mut(cs).unwrap().city_state;
        data.initialized = true;
        data.personality = personality;
        data.unique_luxury = luxury;
        data.unique_unit = unit;
    }
}
fn choose<T: Copy>(choices: &[T], rng: &mut Rng) -> Option<T> {
    if choices.is_empty() {
        None
    } else {
        Some(choices[rng.range(0, choices.len() as i64 - 1) as usize])
    }
}

/// Called exactly on a new first-contact transition, after the pair is met.
pub fn on_met(world: &mut World, r: &Ruleset, cs: CivId, major: CivId, first_major: bool) -> crate::stats::Stats {
    let Some(c) = world.civ(cs).filter(|c| c.is_city_state()) else { return Default::default() };
    if !world.civ(major).is_some_and(|c| c.is_major() && c.city_state.minor_attacks < 2)
        || world.diplomacy.at_war(cs, major)
    {
        return Default::default();
    }
    let religious = c.city_state_type.and_then(|id| r.city_state_types.get(id)).is_some_and(|kind| {
        kind.friend_bonus_uniques.iter().chain(&kind.ally_bonus_uniques).map(|s| crate::Unique::parse(s)).any(|u| {
            u.params.first().and_then(|p| crate::stats::Stats::parse(p)).is_some_and(|s| s.faith > Fixed::ZERO)
        })
    });
    let gold = if first_major { 30 } else { 15 };
    let faith = if religious { 4 } else { 0 };
    let c = world.civ_mut(major).unwrap();
    c.gold = c.gold.saturating_add(gold);
    c.add_faith(faith);
    if let Some(capital) = super::connections::capital_of(world, r, cs) {
        super::fog::reveal_city_center(world, r, major, capital);
    }
    crate::stats::Stats { gold: Fixed::whole(gold), faith: Fixed::whole(faith), ..Default::default() }
}

pub fn unique_luxury(world: &World, cs: CivId) -> Option<Id<TileResource>> {
    world.civ(cs).filter(|c| c.is_city_state() && c.is_alive()).and_then(|c| c.city_state.unique_luxury)
}

/// Called after actions as well as round settlement, so briefly losing an
/// alliance resets Austria's waiting period even if it is bought back.
pub fn sync_alliances(world: &mut World) {
    for cs in city_states::city_states(world) {
        let ally = city_states::ally_of(world, cs);
        let data = &mut world.civ_mut(cs).unwrap().city_state;
        if data.tracked_ally != ally {
            data.tracked_ally = ally;
            data.allied_turns = 0;
        }
    }
}
/// Direct aggression, including the source's probability of alarming other
/// known city-states. The normal war transition owns alliance propagation.
pub fn register_attack(world: &mut World, r: &Ruleset, cs: CivId, attacker: CivId) {
    use super::quests::{Proximity, Target};
    let data = &mut world.civ_mut(attacker).unwrap().city_state;
    data.minor_attacks = data.minor_attacks.saturating_add(1);
    let count = data.minor_attacks;
    if count < 2 {
        return;
    }
    let mut rng = Rng::from_parts(&[0xC5A66, world.turn as u64, cs.0 as u64, attacker.0 as u64, count as u64]);
    let mut wary = Vec::new();
    if count >= 4 || rng.range(0, 1) == 1 {
        wary.push(cs);
    }
    for other in city_states::city_states(world).into_iter().filter(|c| *c != cs) {
        if city_states::ally_of(world, other) == Some(attacker) || !world.diplomacy.have_met(other, attacker) {
            continue;
        }
        if world.civ(other).unwrap().quests.active.iter().any(|q| {
            r.quests.get(q.quest).is_some_and(|q| q.name == "Conquer City State") && q.target == Target::Civ(cs)
        }) {
            continue;
        }
        let proximity = super::quests::proximity(world, other, attacker);
        let probability = if count >= 4 {
            match proximity {
                Proximity::Neighbors => 100,
                Proximity::Close => 75,
                Proximity::Far => 50,
                Proximity::Distant => 25,
                Proximity::None => 0,
            }
        } else {
            match proximity {
                Proximity::Neighbors => 50,
                Proximity::Close => 20,
                _ => 0,
            }
        } + if world.diplomacy.at_war(other, attacker) { 50 } else { 0 };
        if rng.range(0, 99) < probability {
            wary.push(other);
        }
    }
    let until = world.turn.saturating_add(10);
    for cs in wary {
        world.civ_mut(cs).unwrap().city_state.wary_until.entry(attacker).or_insert(until);
    }
}

fn bonus(world: &World, r: &Ruleset, civ: CivId, kind: UniqueType) -> i32 {
    let ctx = GameContext::new(r).with_world(world).for_civ(civ);
    world
        .civ(civ)
        .into_iter()
        .flat_map(|c| c.uniques.matching(kind))
        .filter(|u| applies(u, &ctx).holds())
        .filter_map(|u| u.params.first()?.parse::<i32>().ok())
        .sum()
}
fn recipient_city(world: &World, r: &Ruleset, cs: CivId, major: CivId) -> Option<Id<City>> {
    let from = super::connections::capital_of(world, r, cs).and_then(|id| world.cities.get(id))?.centre;
    world
        .civ(major)?
        .cities
        .iter()
        .filter_map(|id| world.cities.get(*id).map(|c| (*id, c)))
        .min_by_key(|(id, c)| (world.grid.distance(from, c.centre), *id))
        .map(|(id, _)| id)
}
fn gift(world: &mut World, r: &Ruleset, cs: CivId, major: CivId, great: bool) -> Option<Id<Unit>> {
    let city = recipient_city(world, r, cs, major)?;
    let c = world.civ(major)?;
    let mut rng = Rng::from_parts(&[0xC5C1F7, world.turn as u64, cs.0 as u64, major.0 as u64, great as u64]);
    let unique = if great {
        None
    } else {
        world.civ(cs)?.city_state.unique_unit.filter(|id| {
            let u = &r.units[*id];
            u.required_tech.as_deref().is_none_or(|t| r.technologies.id(t).is_some_and(|t| c.techs.contains(&t)))
                && u.obsolete_tech.as_deref().is_none_or(|t| r.technologies.id(t).is_none_or(|t| !c.techs.contains(&t)))
        })
    };
    let base = unique.or_else(|| {
        let choices: Vec<_> = r
            .units
            .iter()
            .filter(|(id, u)| {
                if great {
                    return u.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::GreatPerson))
                        && !u.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::MayFoundReligion));
                }
                u.unique_to.is_none()
                    && (u.strength.unwrap_or(0) > 0 || u.ranged_strength.unwrap_or(0) > 0)
                    && r.unit_types.by_name(&u.unit_type).is_some_and(|t| t.movement_type == "Land")
                    && super::construction::can_construct_in(
                        world,
                        r,
                        major,
                        world.cities.get(city).unwrap(),
                        Construction::Unit(*id),
                    )
                    .is_ok()
            })
            .map(|(id, _)| id)
            .collect();
        choose(&choices, &mut rng)
    })?;
    let mut unit =
        super::construction::prepare_unit_near(world, r, major, base, world.cities.get(city)?.centre, Some(city))?;
    if !great {
        if let Some(capital) = super::connections::capital_of(world, r, cs) {
            super::military::train(world, r, capital, &mut unit);
        }
        unit.xp = (i32::from(unit.xp) + bonus(world, r, major, UniqueType::CityStateGiftedUnitsStartWithXp))
            .clamp(0, u16::MAX as i32) as u16;
    }
    Some(world.place_unit(unit))
}
fn gp_interval(world: &World, major: CivId, speed: Fixed) -> u32 {
    let mut rng = Rng::from_parts(&[0xC59F, world.turn as u64, major.0 as u64]);
    (Fixed::whole(rng.range(37, 43) as i32) * speed).floor().max(1) as u32
}

pub fn end_round(world: &mut World, r: &Ruleset, costs: CostModifiers, all_ai: bool) -> Vec<super::turn::TurnEvent> {
    use super::turn::TurnEvent;
    initialize(world, r);
    sync_alliances(world);
    for c in &mut world.civs {
        c.city_state.wary_until.retain(|_, until| *until > world.turn);
    }
    let states = city_states::city_states(world);
    let majors: Vec<_> = world.civs.iter().filter(|c| c.is_major() && c.is_alive()).map(|c| c.id).collect();
    let mut events = Vec::new();
    for &cs in &states {
        if world.civ(cs).unwrap().city_state.tracked_ally.is_some() {
            let data = &mut world.civ_mut(cs).unwrap().city_state;
            data.allied_turns = data.allied_turns.saturating_add(1);
        }
        for &major in &majors {
            let level = city_states::relationship(world, cs, major);
            let kind = &r.city_state_types[world.civ(cs).unwrap().city_state_type.unwrap()];
            let rules = match level {
                Relationship::Ally => &kind.ally_bonus_uniques,
                Relationship::Friend => &kind.friend_bonus_uniques,
                _ => {
                    world.civ_mut(cs).unwrap().city_state.military_gifts.remove(&major);
                    continue;
                }
            };
            let ctx = GameContext::new(r).with_world(world).for_civ(major);
            let interval = rules
                .iter()
                .map(|s| crate::Unique::parse(s))
                .filter(|u| u.unique_type() == Some(UniqueType::CityStateMilitaryUnits) && applies(u, &ctx).holds())
                .filter_map(|u| u.params.first()?.parse::<u32>().ok())
                .min();
            let Some(interval) = interval else {
                world.civ_mut(cs).unwrap().city_state.military_gifts.remove(&major);
                continue;
            };
            let previous = world.civ(cs).unwrap().city_state.military_gifts.get(&major).copied();
            let mut left = previous.unwrap_or(0);
            if previous.is_some() {
                let common =
                    world.civs.iter().any(|c| world.diplomacy.at_war(cs, c.id) && world.diplomacy.at_war(major, c.id));
                let decrement =
                    if common { bonus(world, r, major, UniqueType::CityStateMoreGiftedUnits).max(1) as u32 } else { 1 };
                left = left.saturating_sub(decrement);
                if left == 0 {
                    if let Some(unit) = gift(world, r, cs, major, false) {
                        events.push(TurnEvent::CityStateGift { city_state: cs, to: major, unit });
                    }
                }
            }
            if left == 0 || left > interval {
                let mut rng = Rng::from_parts(&[0xC51F7, world.turn as u64, cs.0 as u64, major.0 as u64]);
                left = (i64::from(interval) + rng.range(-1, 1)).max(1) as u32;
            }
            world.civ_mut(cs).unwrap().city_state.military_gifts.insert(major, left);
        }
    }
    for major in majors {
        let ctx = GameContext::new(r).with_world(world).for_civ(major);
        let enabled = world
            .civ(major)
            .unwrap()
            .uniques
            .matching(UniqueType::CityStateCanGiftGreatPeople)
            .any(|u| applies(u, &ctx).holds());
        if enabled {
            let allies: Vec<_> =
                states.iter().copied().filter(|cs| city_states::ally_of(world, *cs) == Some(major)).collect();
            let initial = world
                .civ(major)
                .unwrap()
                .city_state
                .great_person_gift
                .unwrap_or_else(|| gp_interval(world, major, costs.speed));
            let mut left = if allies.is_empty() { initial } else { initial.saturating_sub(1) };
            if left < (allies.len().min(10) as u32) && !world.civ(major).unwrap().cities.is_empty() {
                let choices: Vec<_> =
                    allies.into_iter().filter(|cs| !world.civ(*cs).unwrap().cities.is_empty()).collect();
                let mut rng = Rng::from_parts(&[0xC5F1, world.turn as u64, major.0 as u64]);
                if let Some(cs) = choose(&choices, &mut rng) {
                    if let Some(unit) = gift(world, r, cs, major, true) {
                        events.push(TurnEvent::CityStateGift { city_state: cs, to: major, unit });
                    }
                    left = gp_interval(world, major, costs.speed);
                }
            }
            world.civ_mut(major).unwrap().city_state.great_person_gift = Some(left);
        }
        if all_ai || world.civ(major).unwrap().is_ai {
            for &cs in &states {
                if super::happiness::happiness(world, r, major, None) <= 0 {
                    break;
                }
                if can_marry(world, r, major, cs, costs) {
                    if let Some(gold) = marry(world, r, major, cs, costs) {
                        events.push(TurnEvent::CityStateMarried { city_state: cs, by: major, gold });
                    }
                }
            }
        }
    }
    events
}

pub fn marriage_cost(world: &World, r: &Ruleset, cs: CivId, costs: CostModifiers) -> Option<i32> {
    let c = world.civ(cs).filter(|c| c.is_city_state() && c.is_alive() && !c.cities.is_empty())?;
    let mut cost = (Fixed::whole(500) * costs.gold_cost).floor() as i32;
    for id in &c.units {
        let u = world.units.get(*id)?;
        let b = &r.units[u.base];
        let production = super::construction::production_cost(
            r,
            Construction::Unit(u.base),
            CostModifiers { civ: Fixed::permille(1500), ..costs },
        )
        .unwrap_or(0);
        cost = cost.saturating_add(
            (super::purchase::gold_price(i64::from(production), b.hurry_cost_modifier.unwrap_or(0)).floor() / 20).max(0)
                as i32,
        );
    }
    Some(cost / 5 * 5)
}
pub fn can_marry(world: &World, r: &Ruleset, major: CivId, cs: CivId, costs: CostModifiers) -> bool {
    let Some(c) = world.civ(major).filter(|c| c.is_major() && c.is_alive()) else { return false };
    let Some(state) = world.civ(cs) else { return false };
    let ctx = GameContext::new(r).with_world(world).for_civ(major);
    let required = c
        .uniques
        .matching(UniqueType::CityStateCanBeBoughtForGold)
        .filter(|u| applies(u, &ctx).holds())
        .filter_map(|u| u.params.first()?.parse::<u32>().ok())
        .min();
    required.is_some_and(|n| state.city_state.tracked_ally == Some(major) && state.city_state.allied_turns >= n)
        && city_states::ally_of(world, cs) == Some(major)
        && world.diplomacy.have_met(major, cs)
        && marriage_cost(world, r, cs, costs).is_some_and(|cost| c.gold >= cost)
}
pub fn marry(world: &mut World, r: &Ruleset, major: CivId, cs: CivId, costs: CostModifiers) -> Option<i32> {
    if !can_marry(world, r, major, cs, costs) {
        return None;
    }
    let cost = marriage_cost(world, r, cs, costs)?;
    world.civ_mut(major)?.gold -= cost;
    let units = world.civ(cs)?.units.clone();
    for id in units {
        world.transfer_unit(id, major);
    }
    let cities = world.civ(cs)?.cities.clone();
    for (_, city) in world.cities.iter_mut() {
        if city.founder == cs {
            city.original_capital = false;
        }
    }
    for city in cities {
        super::conquest::transfer_city(world, r, city, major)?;
        let c = world.cities.get_mut(city)?;
        c.puppet = true;
        c.queue.clear();
        c.production_conversion = None;
    }
    let state = &mut world.civ_mut(cs)?.city_state;
    state.absorbed = true;
    state.military_gifts.clear();
    state.tracked_ally = None;
    state.allied_turns = 0;
    world.resolve_defeats();
    Some(cost)
}

#[cfg(feature = "save")]
pub fn validate(world: &World, r: &Ruleset) -> Result<(), String> {
    for c in &world.civs {
        let s = &c.city_state;
        if s.unique_luxury.is_some_and(|id| id.generation() != 1 || r.resources.get(id).is_none())
            || s.unique_unit.is_some_and(|id| id.generation() != 1 || r.units.get(id).is_none())
        {
            return Err("unknown city-state gift identity".into());
        }
        if s.wary_until.keys().any(|id| !world.civ(*id).is_some_and(|c| c.is_major())) {
            return Err("invalid city-state wary target".into());
        }
        if s.tracked_ally.is_some_and(|id| !world.civ(id).is_some_and(|c| c.is_major()))
            || s.military_gifts.iter().any(|(id, n)| !world.civ(*id).is_some_and(|c| c.is_major()) || *n > 10000)
        {
            return Err("invalid city-state gift clock".into());
        }
        if s.minor_attacks > 1_000_000
            || s.great_person_gift.is_some_and(|n| n > 100000)
            || s.tracked_ally.is_none() && s.allied_turns != 0
        {
            return Err("invalid city-state alliance clock".into());
        }
        if s.absorbed
            && (!c.is_city_state()
                || !c.defeated
                || !c.cities.is_empty()
                || !c.units.is_empty()
                || world.cities.iter().any(|(_, city)| city.founder == c.id && city.original_capital))
        {
            return Err("invalid absorbed city-state".into());
        }
        if !c.is_city_state()
            && (s.initialized
                || s.unique_luxury.is_some()
                || s.unique_unit.is_some()
                || !s.military_gifts.is_empty()
                || !s.wary_until.is_empty()
                || s.tracked_ally.is_some())
        {
            return Err("city-state identity on a major".into());
        }
    }
    Ok(())
}
