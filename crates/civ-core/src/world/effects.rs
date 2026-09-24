//! Immediate ruleset rewards and their pending player choices.
use crate::arena::Id;
use crate::ruleset::{BaseUnit, Building, Policy, Ruleset, Technology};
use crate::stats::Fixed;
use crate::world::city::City;
use crate::world::conditionals::{applies, GameContext, Verdict};
use crate::world::construction::Construction;
use crate::world::entities::Unit;
use crate::world::grid::CivId;
use crate::world::world::World;
use crate::{Unique, UniqueType};

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PendingRewards {
    pub free_techs: u32,
    pub free_policies: u32,
    pub free_great_people: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EffectEvent {
    Technology { tech: Id<Technology> },
    Unit { unit: Id<Unit> },
    Building { city: Id<City>, building: Id<Building> },
    Choices,
    GoldenAge,
    Population { city: Id<City>, amount: u32 },
}

/// A civilization receives its replacement rather than the shared base unit.
pub fn equivalent_unit(world: &World, r: &Ruleset, civ: CivId, base: Id<BaseUnit>) -> Id<BaseUnit> {
    let Some(nation) = world.civ(civ).and_then(|c| r.nations.name_of(c.nation)) else { return base };
    let Some(b) = r.units.get(base) else { return base };
    let name = b.replaces.as_deref().unwrap_or(&b.name);
    r.units
        .iter()
        .find(|(_, u)| u.unique_to.as_deref() == Some(nation) && u.replaces.as_deref() == Some(name))
        .map_or_else(|| r.units.id(name).unwrap_or(base), |(id, _)| id)
}

pub fn equivalent_building(world: &World, r: &Ruleset, civ: CivId, base: Id<Building>) -> Id<Building> {
    let Some(nation) = world.civ(civ).and_then(|c| r.nations.name_of(c.nation)) else { return base };
    let Some(b) = r.buildings.get(base) else { return base };
    let name = b.replaces.as_deref().unwrap_or(&b.name);
    r.buildings
        .iter()
        .find(|(_, b)| b.unique_to.as_deref() == Some(nation) && b.replaces.as_deref() == Some(name))
        .map_or_else(|| r.buildings.id(name).unwrap_or(base), |(id, _)| id)
}

fn reward_city(world: &World, civ: CivId, city: Option<Id<City>>) -> Option<Id<City>> {
    city.filter(|id| world.cities.get(*id).is_some_and(|c| c.owner == civ)).or_else(|| {
        world.civ(civ)?.cities.iter().copied().find(|id| world.cities.get(*id).is_some_and(|c| c.owner == civ))
    })
}

fn spawn(world: &mut World, r: &Ruleset, civ: CivId, city: Option<Id<City>>, base: Id<BaseUnit>) -> Option<Id<Unit>> {
    let city = reward_city(world, civ, city)?;
    let origin = world.cities.get(city)?.centre;
    let base = equivalent_unit(world, r, civ, base);
    let unit = crate::world::construction::prepare_unit_near(world, r, civ, base, origin, Some(city))?;
    let id = world.place_unit(unit);
    crate::world::luminaries::name_unit(world, r, id);
    Some(id)
}

fn grant_building(world: &mut World, r: &Ruleset, civ: CivId, city: Id<City>, base: Id<Building>) -> bool {
    let base = equivalent_building(world, r, civ, base);
    let Some(c) = world.cities.get_mut(city).filter(|c| c.owner == civ) else { return false };
    c.free_buildings.insert(base);
    if !c.built.insert(base) {
        return false;
    }
    c.queue.retain(|q| *q != Construction::Building(base));
    c.production_progress.remove(&Construction::Building(base));
    crate::world::city::rebuild_city_uniques(r, c);
    true
}

/// Dispatch the immediate payload, only at the event that grants this source.
/// Trigger conditionals are dispatched separately; rebuilding indexes never repeats rewards.
pub fn apply_uniques(
    world: &mut World,
    r: &Ruleset,
    civ: CivId,
    city: Option<Id<City>>,
    uniques: &[Unique],
    speed: Fixed,
) -> Vec<EffectEvent> {
    let mut out = Vec::new();
    for u in uniques {
        let mut ctx = GameContext::new(r).with_world(world).for_civ(civ);
        if let Some(city) = city {
            ctx = ctx.in_city(city);
        }
        let mut conditional = u.clone();
        conditional.conditionals.retain(|c| c.unique_type() != Some(UniqueType::ModifiedByGameSpeed));
        if applies(&conditional, &ctx) != Verdict::True {
            continue;
        }
        let number = |i: usize| u.params.get(i).and_then(|p| p.parse::<u32>().ok()).unwrap_or(0);
        let kind = u.unique_type();
        match kind {
            Some(
                UniqueType::OneTimeFreeTech
                | UniqueType::OneTimeAmountFreeTechs
                | UniqueType::OneTimeFreePolicy
                | UniqueType::OneTimeAmountFreePolicies
                | UniqueType::OneTimeFreeGreatPerson,
            ) => {
                let Some(c) = world.civ_mut(civ) else { continue };
                let (slot, n) = match kind.unwrap() {
                    UniqueType::OneTimeFreeTech => (&mut c.pending_rewards.free_techs, 1),
                    UniqueType::OneTimeAmountFreeTechs => (&mut c.pending_rewards.free_techs, number(0)),
                    UniqueType::OneTimeFreePolicy => (&mut c.pending_rewards.free_policies, 1),
                    UniqueType::OneTimeAmountFreePolicies => (&mut c.pending_rewards.free_policies, number(0)),
                    _ => (&mut c.pending_rewards.free_great_people, 1),
                };
                *slot = slot.saturating_add(n);
                out.push(EffectEvent::Choices);
            }
            Some(UniqueType::OneTimeFreeUnit | UniqueType::OneTimeAmountFreeUnits) => {
                let amount = if kind == Some(UniqueType::OneTimeFreeUnit) { 1 } else { number(0) };
                let index = usize::from(kind == Some(UniqueType::OneTimeAmountFreeUnits));
                if let Some(base) = u.params.get(index).and_then(|p| r.units.id(p)) {
                    for _ in 0..amount {
                        if let Some(unit) = spawn(world, r, civ, city, base) {
                            out.push(EffectEvent::Unit { unit });
                        }
                    }
                }
            }
            Some(UniqueType::OneTimeEnterGoldenAge | UniqueType::OneTimeEnterGoldenAgeTurns) => {
                let turns = if kind == Some(UniqueType::OneTimeEnterGoldenAge) {
                    10
                } else {
                    number(0).min(i32::MAX as u32) as i32
                };
                if crate::world::golden_age::enter(
                    world,
                    civ,
                    turns,
                    r.speeds
                        .iter()
                        .find(|(_, s)| s.modifier == speed)
                        .map_or(speed, |(_, s)| s.golden_age_length_modifier),
                )
                .is_some()
                {
                    out.push(EffectEvent::GoldenAge);
                }
            }
            Some(UniqueType::OneTimeGainPopulation) => {
                let amount = number(0);
                let filter = u.params.get(1).map_or("in all cities", String::as_str);
                let cities: Vec<_> = world
                    .cities
                    .iter()
                    .filter(|(_, c)| {
                        c.owner == civ
                            && crate::world::filters::city_matches(
                                world,
                                r,
                                c,
                                city.and_then(|id| world.cities.get(id)),
                                filter,
                            ) == Verdict::True
                    })
                    .map(|(id, _)| id)
                    .collect();
                for id in cities {
                    if let Some(c) = world.cities.get_mut(id) {
                        c.population = c.population.saturating_add(amount);
                        c.faith.on_population_change(amount as i32, c.population);
                        out.push(EffectEvent::Population { city: id, amount });
                    }
                }
            }
            Some(UniqueType::OneTimeDiscoverTech) => {
                if let Some(tech) = u.params.first().and_then(|p| r.technologies.id(p)) {
                    if world.civ_mut(civ).is_some_and(|c| crate::world::research::grant_technology(c, r, tech)) {
                        on_technology_researched(world, r, civ, tech, speed);
                        out.push(EffectEvent::Technology { tech });
                    }
                }
            }
            Some(UniqueType::GainFreeBuildings) => {
                let Some(base) = u.params.first().and_then(|p| r.buildings.id(p)) else { continue };
                let filter = u.params.get(1).map_or("in this city", String::as_str);
                let cities: Vec<_> = world
                    .cities
                    .iter()
                    .filter(|(_, c)| {
                        c.owner == civ
                            && crate::world::filters::city_matches(
                                world,
                                r,
                                c,
                                city.and_then(|id| world.cities.get(id)),
                                filter,
                            ) == Verdict::True
                    })
                    .map(|(id, _)| id)
                    .collect();
                for id in cities {
                    if grant_building(world, r, civ, id, base) {
                        out.push(EffectEvent::Building { city: id, building: base });
                    }
                }
            }
            Some(UniqueType::OneTimeGainStat) => {
                let Some(stat) = u.params.get(1).and_then(|p| crate::stats::Stat::parse(p)) else { continue };
                let mut amount = u.params.first().and_then(|p| p.parse::<i32>().ok()).unwrap_or(0);
                if u.conditionals.iter().any(|c| c.unique_type() == Some(UniqueType::ModifiedByGameSpeed)) {
                    amount = (Fixed::whole(amount) * speed).floor() as i32;
                }
                if let Some(c) = world.civ_mut(civ) {
                    match stat {
                        crate::stats::Stat::Gold => c.gold = c.gold.saturating_add(amount),
                        crate::stats::Stat::Culture => c.add_culture(amount),
                        crate::stats::Stat::Faith => c.add_faith(amount),
                        crate::stats::Stat::Science => c.science_stored = c.science_stored.saturating_add(amount),
                        _ => {}
                    }
                }
            }
            Some(UniqueType::OneTimeGainSpy) => {
                let spy = crate::world::espionage::new_spy(world, r, civ);
                if let Some(c) = world.civ_mut(civ) {
                    c.espionage.spies.push(spy);
                }
            }
            Some(UniqueType::OneTimeSpiesLevelUp) => {
                if let Some(c) = world.civ_mut(civ) {
                    for spy in &mut c.espionage.spies {
                        spy.rank = spy.rank.saturating_add(number(0).min(3) as u8).min(3);
                    }
                }
            }
            Some(UniqueType::OneTimeRevealEntireMap) => {
                for tile in world.grid.ids() {
                    world.visibility.explore(civ, tile);
                }
                crate::world::fog::capture_observations(world, r);
                crate::world::natural_wonders::discover_visible(world, r);
            }
            Some(UniqueType::OneTimeTriggerVoting) => {
                crate::world::victory_actions::start_voting(world, speed);
            }
            _ => {}
        }
    }
    out
}

/// Persistent grants remember which cities already received a building. This
/// also allows a later city or newly unlocked culture building to receive it.
pub fn refresh_free_buildings(world: &mut World, r: &Ruleset, civ: CivId) {
    let Some(owner) = world.civ(civ) else { return };
    let uniques: Vec<_> = owner
        .uniques
        .all()
        .filter(|u| {
            matches!(
                u.unique_type(),
                Some(UniqueType::FreeStatBuildings | UniqueType::FreeSpecificBuildings | UniqueType::GainFreeBuildings)
            )
        })
        .cloned()
        .collect();
    let cities = owner.cities.clone();
    for u in uniques {
        if applies(&u, &GameContext::new(r).with_world(world).for_civ(civ)) != Verdict::True {
            continue;
        }
        if u.unique_type() == Some(UniqueType::GainFreeBuildings) {
            apply_uniques(world, r, civ, None, &[u], Fixed::ONE);
            continue;
        }
        let Some(value) = u.params.first() else { continue };
        let count = u.params.get(1).and_then(|n| n.parse::<usize>().ok()).unwrap_or(0);
        let stat = u.unique_type() == Some(UniqueType::FreeStatBuildings);
        let key = format!("{}:{}", if stat { "stat" } else { "building" }, value);
        for &city in cities.iter().take(count) {
            if world.civ(civ).is_some_and(|c| c.free_building_grants.get(&key).is_some_and(|ids| ids.contains(&city))) {
                continue;
            }
            let Some(c) = world.cities.get(city).filter(|c| c.owner == civ) else { continue };
            let base = if stat {
                r.buildings
                    .iter()
                    .filter(|(id, b)| {
                        b.is_wonder != Some(true)
                            && b.is_national_wonder != Some(true)
                            && crate::world::filters::building_matches(r, *id, value) == Verdict::True
                            && crate::world::construction::can_construct_in(
                                world,
                                r,
                                civ,
                                c,
                                Construction::Building(*id),
                            )
                            .is_ok()
                    })
                    .min_by_key(|(id, b)| (b.cost.unwrap_or(i32::MAX), *id))
                    .map(|(id, _)| id)
            } else {
                r.buildings.id(value)
            };
            let Some(base) = base else { continue };
            if !stat && c.built.contains(&equivalent_building(world, r, civ, base)) {
                continue;
            }
            world.civ_mut(civ).unwrap().free_building_grants.entry(key.clone()).or_default().insert(city);
            grant_building(world, r, civ, city, base);
        }
    }
    crate::world::policies::rebuild_uniques(world, r, civ);
}

pub fn on_policy_adopted(world: &mut World, r: &Ruleset, civ: CivId, policy: Id<Policy>, speed: Fixed) {
    if let Some(p) = r.policies.get(policy) {
        apply_uniques(world, r, civ, None, &p.uniques, speed);
    }
    trigger(world, r, civ, None, UniqueType::TriggerUponAdoptingPolicyOrBelief, &r.policies[policy].name, speed);
    refresh_free_buildings(world, r, civ);
}

pub fn on_construction_completed(world: &mut World, r: &Ruleset, city: Id<City>, what: Construction, speed: Fixed) {
    let science = crate::world::research::ScienceModifiers {
        speed: r.speeds.iter().find(|(_, s)| s.modifier == speed).map_or(speed, |(_, s)| s.science_cost_modifier),
        ..Default::default()
    };
    on_construction_completed_with_science(world, r, city, what, speed, science);
}
pub fn on_construction_completed_with_science(
    world: &mut World,
    r: &Ruleset,
    city: Id<City>,
    what: Construction,
    speed: Fixed,
    science: crate::world::research::ScienceModifiers,
) {
    let Some(civ) = world.cities.get(city).map(|c| c.owner) else { return };
    if let Construction::Building(id) = what {
        crate::world::policies::rebuild_uniques(world, r, civ);
        if let Some(b) = r.buildings.get(id) {
            apply_uniques(world, r, civ, Some(city), &b.uniques, speed);
        }
        trigger(world, r, civ, Some(city), UniqueType::TriggerUponConstructingBuilding, &r.buildings[id].name, speed);
        if world.civ(civ).is_some_and(|c| c.uniques.has(UniqueType::TechBoostWhenScientificBuildingsBuiltInCapital))
            && crate::world::connections::capital_of(world, r, civ) == Some(city)
            && (r.buildings[id].science.unwrap_or(0) > 0
                || r.buildings[id].percent_stat_bonus.get("science").copied().unwrap_or_default() > 0
                || r.buildings[id].uniques.iter().any(|u| {
                    matches!(
                        u.unique_type(),
                        Some(UniqueType::Stats | UniqueType::StatsPerPopulation | UniqueType::StatsFromTiles)
                    ) && u
                        .params
                        .first()
                        .and_then(|p| crate::stats::Stats::parse(p))
                        .is_some_and(|s| s.science > Fixed::ZERO)
                }))
        {
            let mut costs: Vec<_> = r
                .technologies
                .ids()
                .filter(|tech| {
                    world.civ(civ).is_some_and(|c| crate::world::research::can_research(r, c, *tech).is_ok())
                })
                .filter_map(|tech| crate::world::research::tech_cost_in(world, r, civ, tech, science))
                .collect();
            costs.sort_unstable();
            if !costs.is_empty() {
                let n = costs.len();
                let twice_median = if n % 2 == 0 {
                    i64::from(costs[n / 2 - 1]) + i64::from(costs[n / 2])
                } else {
                    2 * i64::from(costs[n / 2])
                };
                let amount = ((twice_median + 2) / 4).min(i64::from(i32::MAX)) as i32;
                let income = crate::world::turn::plan_economy(
                    world,
                    r,
                    civ,
                    crate::world::CityFocus::None,
                    crate::world::turn::TurnModifiers { science, ..Default::default() },
                )
                .science;
                if let crate::world::research::ResearchEvent::Completed { tech, .. } =
                    crate::world::research::apply_science_with_income_in(world, r, civ, amount, income, science)
                {
                    on_technology_researched(world, r, civ, tech, speed);
                }
            }
        }
        refresh_free_buildings(world, r, civ);
    }
}

pub fn on_technology_researched(world: &mut World, r: &Ruleset, civ: CivId, tech: Id<Technology>, speed: Fixed) {
    crate::world::policies::rebuild_uniques(world, r, civ);
    if let Some(t) = r.technologies.get(tech) {
        apply_uniques(world, r, civ, None, &t.uniques, speed);
    }
    trigger(world, r, civ, None, UniqueType::TriggerUponResearch, &r.technologies[tech].name, speed);
    if let Some(era) = r.technologies[tech].era(r).and_then(|id| r.eras.get(id)) {
        let other_same_era = world.civ(civ).is_some_and(|c| {
            c.techs.iter().any(|id| {
                *id != tech
                    && r.technologies
                        .get(*id)
                        .and_then(|t| t.era(r))
                        .and_then(|id| r.eras.get(id))
                        .is_some_and(|e| e.era_number >= era.era_number)
            })
        });
        if !other_same_era {
            trigger(world, r, civ, None, UniqueType::TriggerUponEnteringEra, &era.name, speed);
        }
    }
    refresh_free_buildings(world, r, civ);
}

/// Execute matching trigger sources once when their game event occurs.
pub fn trigger(
    world: &mut World,
    r: &Ruleset,
    civ: CivId,
    city: Option<Id<City>>,
    kind: UniqueType,
    subject: &str,
    speed: Fixed,
) {
    let sources: Vec<_> = world
        .civ(civ)
        .into_iter()
        .flat_map(|c| c.uniques.all())
        .chain(
            city.and_then(|id| world.cities.get(id)).into_iter().flat_map(|c| c.uniques.all().filter(|u| u.is_local())),
        )
        .cloned()
        .collect();
    let mut matches = Vec::new();
    for mut u in sources {
        let triggers: Vec<_> = u
            .conditionals
            .iter()
            .filter(|c| {
                c.unique_type()
                    .is_some_and(|t| t.targets().contains(&crate::unique_type::UniqueTarget::TriggerCondition))
            })
            .collect();
        if triggers.is_empty()
            || !triggers.iter().all(|c| {
                c.unique_type() == Some(kind)
                    && c.params.first().is_none_or(|filter| {
                        filter == subject
                            || filter == "All"
                            || (kind == UniqueType::TriggerUponExpendingUnit
                                && r.units.id(subject).is_some_and(|id| {
                                    crate::world::filters::base_unit_matches(r, id, filter) == Verdict::True
                                }))
                    })
            })
        {
            continue;
        }
        let first = if kind == UniqueType::TriggerUponResearch {
            r.technologies
                .id(subject)
                .is_some_and(|tech| world.civs.iter().all(|c| c.id == civ || !c.techs.contains(&tech)))
        } else {
            true
        };
        if !first && u.conditionals.iter().any(|c| c.unique_type() == Some(UniqueType::ConditionalFirstCivToResearch)) {
            continue;
        }
        u.conditionals.retain(|c| {
            !c.unique_type().is_some_and(|t| {
                t.targets().contains(&crate::unique_type::UniqueTarget::TriggerCondition)
                    || t == UniqueType::ConditionalFirstCivToResearch
            })
        });
        matches.push(u);
    }
    apply_uniques(world, r, civ, city, &matches, speed);
}

pub fn claim_free_technology(world: &mut World, r: &Ruleset, civ: CivId, tech: Id<Technology>, speed: Fixed) -> bool {
    let Some(c) = world.civ(civ) else { return false };
    if c.pending_rewards.free_techs == 0 || crate::world::research::can_research(r, c, tech).is_err() {
        return false;
    }
    let c = world.civ_mut(civ).unwrap();
    c.pending_rewards.free_techs -= 1;
    crate::world::research::grant_technology(c, r, tech);
    on_technology_researched(world, r, civ, tech, speed);
    true
}

pub fn great_person_types(world: &World, r: &Ruleset, civ: CivId) -> Vec<Id<BaseUnit>> {
    let mut units = std::collections::BTreeSet::new();
    for (id, _) in r
        .units
        .iter()
        .filter(|(id, _)| crate::world::filters::base_unit_matches(r, *id, "Great Person") == Verdict::True)
    {
        units.insert(equivalent_unit(world, r, civ, id));
    }
    units.into_iter().collect()
}
pub fn free_great_person_options(world: &World, r: &Ruleset, civ: CivId) -> Vec<Id<BaseUnit>> {
    let Some(c) = world.civ(civ).filter(|c| c.pending_rewards.free_great_people > 0 && !c.cities.is_empty()) else {
        return Vec::new();
    };
    great_person_types(world, r, civ)
        .into_iter()
        .filter(|id| c.great_people.maya_restricted == 0 || c.great_people.maya_available.contains(id))
        .collect()
}
pub fn claim_free_great_person(world: &mut World, r: &Ruleset, civ: CivId, base: Id<BaseUnit>) -> Option<Id<Unit>> {
    if !free_great_person_options(world, r, civ).contains(&base) {
        return None;
    }
    let unit = spawn(world, r, civ, None, base)?;
    let c = world.civ_mut(civ)?;
    c.pending_rewards.free_great_people -= 1;
    if c.great_people.maya_restricted > 0 {
        c.great_people.maya_restricted -= 1;
        c.great_people.maya_available.remove(&base);
    }
    Some(unit)
}

pub fn resolve_ai_rewards(world: &mut World, r: &Ruleset, civ: CivId, speed: Fixed) {
    // The caller selects automated seats (or trusted all-seat self-play).
    while world.civ(civ).is_some_and(|c| c.pending_rewards.free_techs > 0) {
        let choice = r
            .technologies
            .ids()
            .find(|tech| world.civ(civ).is_some_and(|c| crate::world::research::can_research(r, c, *tech).is_ok()));
        let Some(tech) = choice else { break };
        if !claim_free_technology(world, r, civ, tech, speed) {
            break;
        }
    }
    while world.civ(civ).is_some_and(|c| c.pending_rewards.free_great_people > 0) {
        let options = free_great_person_options(world, r, civ);
        if !options.into_iter().any(|base| claim_free_great_person(world, r, civ, base).is_some()) {
            break;
        }
    }
}
