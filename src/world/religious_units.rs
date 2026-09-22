//! Physical prophets, missionaries and inquisitors. Unit action modifiers,
//! birth-city religion and inquisitor protection follow `UnitActionsReligion`,
//! `ReligionManager` and `CityReligionManager` in the Kotlin implementation.

use crate::arena::Id;
use crate::ruleset::{Belief, Ruleset, TileImprovement};
use crate::stats::{Fixed, Stat, Stats};
use crate::unique::Unique;
use crate::world::ai::UnitOrder;
use crate::world::city::City;
use crate::world::conditionals::{applies, GameContext, Verdict};
use crate::world::construction::{Construction, CostModifiers};
use crate::world::entities::Unit;
use crate::world::grid::{CivId, TileId};
use crate::world::religion::available_beliefs;
use crate::world::world::World;
use crate::UniqueType;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReligiousAction {
    FoundReligion,
    EnhanceReligion,
    SpreadReligion(Id<City>),
    RemoveHeresy(Id<City>),
    ConstructHolySite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReligiousRejection {
    NoSuchUnit,
    NoSuchCity,
    NotAble,
    NoMovement,
    NoReligion,
    NoUses,
    AlreadyUsed,
    AlreadyFounded,
    AlreadyEnhanced,
    NoBeliefs,
    NotInCity,
    NotOurCity,
    AlreadyHolyCity,
    AlreadyFollowing,
    ProtectedByInquisitor,
    NoHeresy,
    CannotBuildHere,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReligiousEvent {
    Founded {
        unit: Id<Unit>,
        city: Id<City>,
        beliefs: Vec<Id<Belief>>,
    },
    Enhanced {
        unit: Id<Unit>,
        beliefs: Vec<Id<Belief>>,
    },
    Spread {
        unit: Id<Unit>,
        city: Id<City>,
        religion: CivId,
        pressure: i32,
        uses_left: u8,
        from: Option<CivId>,
        to: Option<CivId>,
        rewards: Stats,
    },
    HeresyRemoved {
        unit: Id<Unit>,
        city: Id<City>,
        religion: CivId,
        from: Option<CivId>,
        to: Option<CivId>,
    },
    HolySiteConstructed {
        unit: Id<Unit>,
        tile: TileId,
        improvement: Id<TileImprovement>,
    },
    Attrition {
        unit: Id<Unit>,
        strength_left: i32,
        destroyed: bool,
    },
}

fn uniques<'a>(r: &'a Ruleset, unit: &'a Unit) -> impl Iterator<Item = &'a Unique> {
    r.units
        .get(unit.base)
        .into_iter()
        .flat_map(|b| &b.uniques)
        .chain(unit.promotions.iter().filter_map(|id| r.promotions.get(*id)).flat_map(|p| &p.uniques))
}

fn action_unique<'a>(r: &'a Ruleset, unit: &'a Unit, kind: UniqueType) -> Option<&'a Unique> {
    uniques(r, unit).find(|u| u.unique_type() == Some(kind))
}

/// Spread/removal charges are supplied by the ruleset's unit-action modifiers.
pub fn initial_uses(r: &Ruleset, unit: &Unit) -> u8 {
    let Some(unique) = action_unique(r, unit, UniqueType::CanSpreadReligion)
        .or_else(|| action_unique(r, unit, UniqueType::CanRemoveHeresy))
    else {
        return 0;
    };
    unique
        .conditionals
        .iter()
        .filter_map(|u| match u.unique_type() {
            Some(UniqueType::UnitActionOnce) => Some(1),
            Some(UniqueType::UnitActionLimitedTimes) => u.params.first()?.parse::<u8>().ok(),
            _ => None,
        })
        .max()
        .unwrap_or(1)
}

/// Initialize a freshly created unit; never call this to refresh an existing
/// unit, since charges and attrition must survive between turns.
pub fn initialize_unit(world: &World, r: &Ruleset, unit: &mut Unit, birth_city: Option<Id<City>>) {
    if !unit.has_unique(r, UniqueType::ReligiousUnit) {
        return;
    }
    unit.religious_strength = r.units.get(unit.base).and_then(|b| b.religious_strength).unwrap_or(0);
    unit.religious_uses_left = initial_uses(r, unit);
    unit.religion = if unit.has_unique(r, UniqueType::TakeReligionOverBirthCity) {
        world.civ(unit.owner).filter(|c| c.faith.founded).map(|c| c.id)
    } else {
        birth_city
            .and_then(|id| world.cities.get(id))
            .and_then(|c| c.faith.majority(c.population))
            .filter(|religion| world.civ(*religion).is_some_and(|c| c.faith.founded))
    };
}

/// Earning faith never consumes a prophet off-map. If its city has no legal
/// spawn tile, the caller leaves the faith in reserve and retries next turn.
pub fn spawn_prophet(world: &mut World, r: &Ruleset, civ: CivId) -> Option<Id<Unit>> {
    let base = r
        .units
        .iter()
        .find(|(_, b)| b.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::MayFoundReligion)))?
        .0;
    let home = world
        .civ(civ)?
        .cities
        .iter()
        .copied()
        .filter(|id| world.cities.get(*id).is_some_and(|c| c.owner == civ))
        .min_by_key(|id| (world.cities.get(*id).is_none_or(|c| c.faith.holy_city_of != Some(civ)), *id))?;
    let centre = world.cities.get(home)?.centre;
    let mut unit = Unit::new(base, civ, centre, r.units.get(base)?.movement);
    let position = world
        .grid
        .within(centre, 2)
        .into_iter()
        .filter(|t| {
            !world.grid.is_water(r, *t)
                && crate::world::movement::is_passable(&world.grid, r, *t)
                && world.grid.owner[t.0 as usize].is_none_or(|owner| owner == civ)
                && !crate::world::movement::is_tile_occupied_for(world, r, *t, &unit)
        })
        .min_by_key(|t| (world.grid.distance(centre, *t), *t))?;
    unit.position = position;
    initialize_unit(world, r, &mut unit, Some(home));
    Some(world.place_unit(unit))
}

fn belief_pair(world: &World, r: &Ruleset, enhancing: bool) -> Option<Vec<Id<Belief>>> {
    let kind = if enhancing { "Enhancer" } else { "Founder" };
    Some(vec![*available_beliefs(world, r, kind).first()?, *available_beliefs(world, r, "Follower").first()?])
}

fn city_centre(world: &World, position: TileId) -> Option<Id<City>> {
    world.cities.iter().find(|(_, city)| city.centre == position).map(|(id, _)| id)
}

fn in_city(world: &World, unit: &Unit, city: &City) -> bool {
    unit.position == city.centre
        || (city.tiles.contains(&unit.position) && world.grid.owner[unit.position.0 as usize] == Some(city.owner))
}

/// Inquisitors protect the city centre and its immediate ring. The religion
/// carried by the defender, rather than its owner, determines what it blocks.
pub fn protected_by_inquisitor(world: &World, r: &Ruleset, city: Id<City>, religion: CivId) -> bool {
    let Some(city) = world.cities.get(city) else { return false };
    world.units.iter().any(|(_, unit)| {
        unit.religion.is_some_and(|creed| creed != religion)
            && unit.has_unique(r, UniqueType::PreventSpreadingReligion)
            && world.grid.distance(unit.position, city.centre) <= 1
    })
}

fn holy_site(r: &Ruleset, unit: &Unit) -> Option<Id<TileImprovement>> {
    // Use the prophet's own construction unique (including mod replacements).
    if !unit.has_unique(r, UniqueType::MayFoundReligion) {
        return None;
    }
    action_unique(r, unit, UniqueType::ConstructImprovementInstantly)
        .and_then(|u| u.params.first())
        .and_then(|name| r.improvements.id(name))
}

/// Validate before changing either the unit or the destination city.
pub fn can(world: &World, r: &Ruleset, id: Id<Unit>, action: ReligiousAction) -> Result<(), ReligiousRejection> {
    use ReligiousRejection as E;
    let unit = world.units.get(id).ok_or(E::NoSuchUnit)?;
    if !world.civ(unit.owner).is_some_and(|c| c.is_major()) {
        return Err(E::NotAble);
    }
    if !unit.can_move() {
        return Err(E::NoMovement);
    }
    let kind = match action {
        ReligiousAction::FoundReligion => UniqueType::MayFoundReligion,
        ReligiousAction::EnhanceReligion => UniqueType::MayEnhanceReligion,
        ReligiousAction::SpreadReligion(_) => UniqueType::CanSpreadReligion,
        ReligiousAction::RemoveHeresy(_) => UniqueType::CanRemoveHeresy,
        ReligiousAction::ConstructHolySite => UniqueType::ConstructImprovementInstantly,
    };
    if !unit.has_unique(r, kind) {
        return Err(E::NotAble);
    }
    let fresh = unit.religious_uses_left == initial_uses(r, unit);
    match action {
        ReligiousAction::FoundReligion | ReligiousAction::EnhanceReligion => {
            if !fresh {
                return Err(E::AlreadyUsed);
            }
            let faith = &world.civ(unit.owner).ok_or(E::NotAble)?.faith;
            let enhancing = matches!(action, ReligiousAction::EnhanceReligion);
            if enhancing {
                if !faith.founded {
                    return Err(E::NoReligion);
                }
                if faith.enhanced {
                    return Err(E::AlreadyEnhanced);
                }
            } else if faith.founded {
                return Err(E::AlreadyFounded);
            }
            if belief_pair(world, r, enhancing).is_none() {
                return Err(E::NoBeliefs);
            }
            let city = city_centre(world, unit.position).and_then(|id| world.cities.get(id)).ok_or(E::NotInCity)?;
            if city.owner != unit.owner {
                return Err(E::NotOurCity);
            }
            if !enhancing && city.faith.holy_city_of.is_some() {
                return Err(E::AlreadyHolyCity);
            }
        }
        ReligiousAction::SpreadReligion(city) | ReligiousAction::RemoveHeresy(city) => {
            if unit.religious_uses_left == 0 {
                return Err(E::NoUses);
            }
            let religion =
                unit.religion.filter(|c| world.civ(*c).is_some_and(|c| c.faith.founded)).ok_or(E::NoReligion)?;
            let target = world.cities.get(city).ok_or(E::NoSuchCity)?;
            if !in_city(world, unit, target) {
                return Err(E::NotInCity);
            }
            if matches!(action, ReligiousAction::SpreadReligion(_)) {
                if unit.religious_strength <= 0 {
                    return Err(E::NoUses);
                }
                if target.faith.majority(target.population) == Some(religion) {
                    return Err(E::AlreadyFollowing);
                }
                if protected_by_inquisitor(world, r, city, religion) {
                    return Err(E::ProtectedByInquisitor);
                }
            } else {
                if target.owner != unit.owner {
                    return Err(E::NotOurCity);
                }
                if !(target.faith.pressures().any(|(c, p)| c.is_some_and(|c| c != religion) && p > 0)
                    || target.faith.holy_city_of == Some(religion) && target.faith.holy_city_blocked)
                {
                    return Err(E::NoHeresy);
                }
            }
        }
        ReligiousAction::ConstructHolySite => {
            if !fresh {
                return Err(E::AlreadyUsed);
            }
            let improvement = holy_site(r, unit).ok_or(E::NotAble)?;
            crate::world::great_person_actions::can_place(world, r, unit.owner, unit.position, improvement)
                .map_err(|_| E::CannotBuildHere)?;
        }
    }
    Ok(())
}

fn spread_strength(world: &World, r: &Ruleset, id: Id<Unit>, city: Id<City>) -> i32 {
    let Some(unit) = world.units.get(id) else { return 0 };
    let ctx =
        GameContext::new(r).with_world(world).for_civ(unit.owner).for_unit(id).on_tile(unit.position).in_city(city);
    // Kotlin's spread action uses base strength; foreign attrition determines
    // how long the missionary survives, not pressure from an individual use.
    let mut strength = Fixed::whole(r.units.get(unit.base).and_then(|b| b.religious_strength).unwrap_or(0));
    for unique in uniques(r, unit).chain(world.civ(unit.owner).into_iter().flat_map(|c| c.uniques.all())) {
        if unique.unique_type() != Some(UniqueType::SpreadReligionStrength) || applies(unique, &ctx) != Verdict::True {
            continue;
        }
        if let Some(percent) = unique.params.first().and_then(|p| Fixed::parse(p)) {
            strength = (strength * (Fixed::whole(100) + percent)).checked_div(Fixed::whole(100)).unwrap_or(strength);
        }
    }
    strength.floor().max(0) as i32
}

fn spreading_rewards(world: &World, r: &Ruleset, id: Id<Unit>, city: Id<City>) -> Stats {
    let mut rewards = Stats::ZERO;
    let Some(unit) = world.units.get(id) else { return rewards };
    let Some(target) = world.cities.get(city) else { return rewards };
    let others: u32 = target.faith.followers().filter(|(creed, _)| Some(*creed) != unit.religion).map(|(_, n)| n).sum();
    let ctx =
        GameContext::new(r).with_world(world).for_civ(unit.owner).for_unit(id).on_tile(unit.position).in_city(city);
    for unique in uniques(r, unit).chain(world.civ(unit.owner).into_iter().flat_map(|c| c.uniques.all())) {
        if unique.unique_type() != Some(UniqueType::StatsWhenSpreading) || applies(unique, &ctx) != Verdict::True {
            continue;
        }
        let Some(per_follower) = unique.params.first().and_then(|p| p.parse::<i32>().ok()) else { continue };
        let Some(stat) = unique.params.get(1).and_then(|p| Stat::parse(p)) else { continue };
        if matches!(stat, Stat::Gold | Stat::Science | Stat::Culture | Stat::Faith) {
            rewards[stat] += Fixed::whole(per_follower.saturating_mul(others as i32));
        }
    }
    rewards
}

fn spend_use(world: &mut World, r: &Ruleset, id: Id<Unit>, kind: UniqueType) -> u8 {
    let Some(unit) = world.units.get(id) else { return 0 };
    let movement = action_unique(r, unit, kind)
        .into_iter()
        .flat_map(|u| &u.conditionals)
        .find(|u| u.unique_type() == Some(UniqueType::UnitActionMovementCost))
        .and_then(|u| u.params.first())
        .and_then(|p| p.parse::<i32>().ok())
        .unwrap_or(1);
    let Some(unit) = world.units.get_mut(id) else { return 0 };
    unit.religious_uses_left = unit.religious_uses_left.saturating_sub(1);
    unit.movement_left = (unit.movement_left - Fixed::whole(movement)).max(Fixed::ZERO);
    let left = unit.religious_uses_left;
    if left == 0 {
        world.remove_unit(id);
    }
    left
}

pub fn perform(
    world: &mut World,
    r: &Ruleset,
    id: Id<Unit>,
    action: ReligiousAction,
) -> Result<ReligiousEvent, ReligiousRejection> {
    can(world, r, id, action)?;
    let unit = world.units.get(id).ok_or(ReligiousRejection::NoSuchUnit)?;
    let (owner, position, religion) = (unit.owner, unit.position, unit.religion);
    let event = match action {
        ReligiousAction::FoundReligion | ReligiousAction::EnhanceReligion => {
            let enhancing = matches!(action, ReligiousAction::EnhanceReligion);
            let beliefs = belief_pair(world, r, enhancing).ok_or(ReligiousRejection::NoBeliefs)?;
            let civ = world.civ_mut(owner).ok_or(ReligiousRejection::NotAble)?;
            civ.faith.beliefs.extend(beliefs.iter().copied());
            if enhancing {
                civ.faith.enhanced = true;
            } else {
                civ.faith.founded = true;
            }
            crate::world::policies::rebuild_uniques(world, r, owner);
            world.remove_unit(id);
            if enhancing {
                ReligiousEvent::Enhanced { unit: id, beliefs }
            } else {
                let city_id = city_centre(world, position).ok_or(ReligiousRejection::NotInCity)?;
                let city = world.cities.get_mut(city_id).ok_or(ReligiousRejection::NoSuchCity)?;
                city.faith.holy_city_of = Some(owner);
                city.faith.holy_city_blocked = false;
                city.faith.add_pressure(Some(owner), city.population.max(1) as i32 * 500);
                city.faith.recount(city.population);
                let units: Vec<_> = world
                    .units
                    .iter()
                    .filter(|(_, u)| u.owner == owner && u.has_unique(r, UniqueType::TakeReligionOverBirthCity))
                    .map(|(id, _)| id)
                    .collect();
                for id in units {
                    if let Some(unit) = world.units.get_mut(id) {
                        unit.religion = Some(owner);
                    }
                }
                ReligiousEvent::Founded { unit: id, city: city_id, beliefs }
            }
        }
        ReligiousAction::SpreadReligion(city_id) => {
            let religion = religion.ok_or(ReligiousRejection::NoReligion)?;
            let pressure = spread_strength(world, r, id, city_id);
            let rewards = spreading_rewards(world, r, id, city_id);
            let removes_others = unit.has_unique(r, UniqueType::RemoveOtherReligions);
            let city = world.cities.get_mut(city_id).ok_or(ReligiousRejection::NoSuchCity)?;
            let (city_owner, from) = (city.owner, city.faith.majority(city.population));
            city.faith.add_pressure(Some(religion), pressure);
            if removes_others {
                city.faith.remove_all_pressures_except(religion);
            }
            city.faith.recount(city.population);
            let to = city.faith.majority(city.population);
            let uses_left = spend_use(world, r, id, UniqueType::CanSpreadReligion);
            if let Some(civ) = world.civ_mut(owner) {
                civ.gold += rewards.gold.floor() as i32;
                civ.science_stored += rewards.science.floor() as i32;
                civ.culture_stored += rewards.culture.floor() as i32;
                civ.faith.stored += rewards.faith.floor() as i32;
            }
            crate::world::promises::on_religion_spread(world, city_owner, owner, true);
            ReligiousEvent::Spread { unit: id, city: city_id, religion, pressure, uses_left, from, to, rewards }
        }
        ReligiousAction::RemoveHeresy(city_id) => {
            let religion = religion.ok_or(ReligiousRejection::NoReligion)?;
            let city = world.cities.get_mut(city_id).ok_or(ReligiousRejection::NoSuchCity)?;
            let from = city.faith.majority(city.population);
            city.faith.remove_all_pressures_except(religion);
            if let Some(holy) = city.faith.holy_city_of {
                city.faith.holy_city_blocked = holy != religion;
            }
            city.faith.recount(city.population);
            let to = city.faith.majority(city.population);
            spend_use(world, r, id, UniqueType::CanRemoveHeresy);
            ReligiousEvent::HeresyRemoved { unit: id, city: city_id, religion, from, to }
        }
        ReligiousAction::ConstructHolySite => {
            let improvement = holy_site(r, unit).ok_or(ReligiousRejection::NotAble)?;
            crate::world::great_person_actions::perform(
                world,
                r,
                id,
                crate::world::great_person_actions::Action::Construct(improvement),
                Default::default(),
            )
            .map_err(|_| ReligiousRejection::CannotBuildHere)?;
            ReligiousEvent::HolySiteConstructed { unit: id, tile: position, improvement }
        }
    };
    Ok(event)
}

/// Tick after movement/actions, before movement is refreshed. Open borders,
/// war, unowned tiles and city-state territory prevent missionary attrition.
pub fn end_turn(world: &mut World, r: &Ruleset, civ: CivId) -> Vec<ReligiousEvent> {
    let losses: Vec<_> = world
        .units
        .iter()
        .filter(|(_, u)| u.owner == civ && u.has_unique(r, UniqueType::ReligiousUnit))
        .filter_map(|(id, unit)| {
            let host = world.grid.owner[unit.position.0 as usize]?;
            if world.civ(host).is_some_and(|c| c.is_city_state())
                || crate::world::movement::may_enter_territory(world, civ, host)
            {
                return None;
            }
            let loss = uniques(r, unit)
                .filter(|u| u.unique_type() == Some(UniqueType::CanEnterForeignTilesButLosesReligiousStrength))
                .filter_map(|u| u.params.first()?.parse::<i32>().ok())
                .min()?;
            Some((id, (unit.religious_strength - loss.max(0)).max(0)))
        })
        .collect();
    losses
        .into_iter()
        .map(|(id, strength_left)| {
            if strength_left == 0 {
                world.remove_unit(id);
            } else if let Some(unit) = world.units.get_mut(id) {
                unit.religious_strength = strength_left;
            }
            ReligiousEvent::Attrition { unit: id, strength_left, destroyed: strength_left == 0 }
        })
        .collect()
}

fn move_toward(world: &World, r: &Ruleset, unit: Id<Unit>, target: TileId) -> Option<UnitOrder> {
    crate::world::movement::step_toward(world, r, unit, target).map(UnitOrder::MoveTo)
}

/// Pure, deterministic unit planning. Prophets first found/enhance, then make
/// holy sites. Missionaries visit unconverted cities; inquisitors remove
/// rival pressure at home and otherwise guard a city centre.
pub fn plan_action(world: &World, r: &Ruleset, civ: CivId, id: Id<Unit>) -> Option<UnitOrder> {
    let unit = world.units.get(id)?;
    if unit.owner != civ || !unit.has_unique(r, UniqueType::ReligiousUnit) {
        return None;
    }
    let faith = &world.civ(civ)?.faith;
    let fresh = unit.religious_uses_left == initial_uses(r, unit);
    if unit.has_unique(r, UniqueType::MayFoundReligion) && fresh {
        let action = if !faith.founded {
            Some(ReligiousAction::FoundReligion)
        } else if !faith.enhanced {
            Some(ReligiousAction::EnhanceReligion)
        } else {
            None
        };
        if let Some(action) = action {
            if can(world, r, id, action).is_ok() {
                return Some(UnitOrder::Religious(action));
            }
            if belief_pair(world, r, faith.founded).is_some() {
                let mut targets: Vec<_> = world
                    .cities
                    .iter()
                    .filter(|(_, c)| c.owner == civ && (faith.founded || c.faith.holy_city_of.is_none()))
                    .map(|(_, c)| c.centre)
                    .collect();
                targets.sort_by_key(|t| (world.grid.distance(unit.position, *t), *t));
                for target in targets {
                    if let Some(order) = move_toward(world, r, id, target) {
                        return Some(order);
                    }
                }
                return None;
            }
        }
        if let Some(improvement) = holy_site(r, unit) {
            if can(world, r, id, ReligiousAction::ConstructHolySite).is_ok() {
                return Some(UnitOrder::Religious(ReligiousAction::ConstructHolySite));
            }
            if let Some(tile) = crate::world::ai::landmark_site(world, r, civ, improvement) {
                return move_toward(world, r, id, tile);
            }
        }
    }
    let religion = unit.religion?;
    let inquisitor = unit.has_unique(r, UniqueType::CanRemoveHeresy);
    let spreading = unit.has_unique(r, UniqueType::CanSpreadReligion);
    if !inquisitor && !spreading {
        return None;
    }
    let mut targets: Vec<_> = world
        .cities
        .iter()
        .filter(|(city_id, city)| {
            if inquisitor {
                city.owner == civ
                    && (city.faith.pressures().any(|(c, p)| c.is_some_and(|c| c != religion) && p > 0)
                        || (city.faith.holy_city_blocked && city.faith.holy_city_of == Some(religion)))
            } else {
                city.faith.majority(city.population) != Some(religion)
                    && !world.diplomacy.at_war(civ, city.owner)
                    && !protected_by_inquisitor(world, r, *city_id, religion)
            }
        })
        .collect();
    targets.sort_by_key(|(id, city)| (city.owner != civ, world.grid.distance(unit.position, city.centre), *id));
    for (city_id, city) in targets {
        let action =
            if inquisitor { ReligiousAction::RemoveHeresy(city_id) } else { ReligiousAction::SpreadReligion(city_id) };
        if can(world, r, id, action).is_ok() {
            return Some(UnitOrder::Religious(action));
        }
        let mut tiles: Vec<_> = city.tiles.iter().copied().chain(std::iter::once(city.centre)).collect();
        tiles.sort_by_key(|t| (world.grid.distance(unit.position, *t), *t));
        for tile in tiles {
            if let Some(order) = move_toward(world, r, id, tile) {
                return Some(order);
            }
        }
    }
    if inquisitor {
        let city = world
            .cities
            .iter()
            .filter(|(_, c)| c.owner == civ)
            .min_by_key(|(id, c)| {
                (c.faith.holy_city_of != Some(religion), world.grid.distance(unit.position, c.centre), *id)
            })?
            .1;
        if world.grid.distance(unit.position, city.centre) > 1 {
            return move_toward(world, r, id, city.centre);
        }
    }
    None
}

/// Buy from a city following the intended religion, after saving for the
/// second prophet. Keep at most two missionaries and one defensive inquisitor.
pub fn choose_purchase(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    costs: CostModifiers,
) -> Option<(Id<City>, Construction)> {
    let c = world.civ(civ)?;
    if !c.is_ai || !c.faith.enhanced {
        return None;
    }
    let needs_conversion = world.cities.iter().any(|(id, city)| {
        city.faith.majority(city.population) != Some(civ)
            && !world.diplomacy.at_war(civ, city.owner)
            && !protected_by_inquisitor(world, r, id, civ)
    });
    let heresy = world.cities.iter().any(|(_, city)| {
        city.owner == civ
            && city.faith.pressures().any(|(creed, p)| creed.is_some_and(|religion| religion != civ) && p > 0)
    });
    let count = |kind| c.units.iter().filter_map(|id| world.units.get(*id)).filter(|u| u.has_unique(r, kind)).count();
    let kind = if heresy && count(UniqueType::CanRemoveHeresy) == 0 {
        UniqueType::CanRemoveHeresy
    } else if needs_conversion && count(UniqueType::CanSpreadReligion) < 2 {
        UniqueType::CanSpreadReligion
    } else {
        return None;
    };
    for &city in &c.cities {
        let home = world.cities.get(city)?;
        if home.faith.majority(home.population) != Some(civ) {
            continue;
        }
        for (id, base) in r.units.iter() {
            if !base.uniques.iter().any(|u| u.unique_type() == Some(kind)) {
                continue;
            }
            let item = Construction::Unit(id);
            if crate::world::purchase::can_purchase(world, r, city, item, Stat::Faith, costs).is_ok() {
                return Some((city, item));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::entities::Civilization;
    use crate::world::grid::TileGrid;
    use crate::world::religion::{apply_faith, FaithEvent};

    fn setup() -> (Ruleset, World, [Id<City>; 3]) {
        let r = Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings").unwrap().unwrap();
        let nation = r.nations.ids().next().unwrap();
        let grid = TileGrid::new(20, 12, r.terrain("Grassland").unwrap());
        let mut world = World::new(
            grid,
            vec![Civilization::new(CivId(0), nation, true), Civilization::new(CivId(1), nation, true)],
        );
        let cities = [(CivId(0), 3, 3), (CivId(1), 10, 3), (CivId(1), 16, 3)].map(|(owner, x, y)| {
            let tile = world.grid.id_at(x, y).unwrap();
            let mut city = City::found("City", owner, tile);
            city.population = 5;
            let id = world.found_city_in(&r, city);
            for tile in world.grid.within(tile, 1) {
                world.claim_tile(id, tile);
            }
            id
        });
        world.diplomacy.meet(CivId(0), CivId(1));
        (r, world, cities)
    }

    fn creed(world: &mut World, city: Id<City>, religion: CivId, pressure: i32) {
        world.civ_mut(religion).unwrap().faith.founded = true;
        let city = world.cities.get_mut(city).unwrap();
        city.faith.add_pressure(Some(religion), pressure);
        city.faith.recount(city.population);
    }

    fn born(world: &mut World, r: &Ruleset, name: &str, owner: CivId, city: Id<City>) -> Id<Unit> {
        let base = r.unit(name).unwrap();
        let position = world.cities.get(city).unwrap().centre;
        let mut unit = Unit::new(base, owner, position, r.units.get(base).unwrap().movement);
        initialize_unit(world, r, &mut unit, Some(city));
        world.place_unit(unit)
    }

    #[test]
    fn faith_spawns_prophets_and_the_second_enhances_only_when_used() {
        let (r, mut world, [home, _, _]) = setup();
        assert!(matches!(apply_faith(&mut world, &r, CivId(0), 10, Fixed::ONE), FaithEvent::PantheonFounded { .. }));
        let FaithEvent::ProphetBorn { unit: first } = apply_faith(&mut world, &r, CivId(0), 200, Fixed::ONE) else {
            panic!("prophet")
        };
        assert_eq!(world.units.get(first).unwrap().religious_uses_left, 4);
        assert!(!world.civ(CivId(0)).unwrap().faith.founded);
        let snapshot = world.clone();
        assert_eq!(
            plan_action(&world, &r, CivId(0), first),
            Some(UnitOrder::Religious(ReligiousAction::FoundReligion))
        );
        assert_eq!(world, snapshot, "planning must remain pure");
        assert!(
            matches!(perform(&mut world, &r, first, ReligiousAction::FoundReligion), Ok(ReligiousEvent::Founded { city, .. }) if city == home)
        );
        assert!(world.units.get(first).is_none());
        assert_eq!(world.cities.get(home).unwrap().faith.holy_city_of, Some(CivId(0)));
        assert_eq!(world.cities.get(home).unwrap().faith.pressure_of(Some(CivId(0))), 2500);
        let FaithEvent::ProphetBorn { unit: second } = apply_faith(&mut world, &r, CivId(0), 300, Fixed::ONE) else {
            panic!("second prophet")
        };
        assert!(!world.civ(CivId(0)).unwrap().faith.enhanced);
        assert_eq!(
            plan_action(&world, &r, CivId(0), second),
            Some(UnitOrder::Religious(ReligiousAction::EnhanceReligion))
        );
        let before = world.civ(CivId(0)).unwrap().faith.beliefs.clone();
        perform(&mut world, &r, second, ReligiousAction::EnhanceReligion).unwrap();
        let faith = &world.civ(CivId(0)).unwrap().faith;
        assert!(faith.enhanced);
        assert_eq!(faith.prophets_earned, 2);
        let mut added: Vec<_> =
            faith.beliefs.difference(&before).map(|id| r.beliefs.get(*id).unwrap().r#type.as_str()).collect();
        added.sort();
        assert_eq!(added, ["Enhancer", "Follower"]);
        assert!(world.units.get(second).is_none());
        assert_eq!(faith.beliefs.len(), 5);
    }

    #[test]
    fn waiting_prophet_does_not_spend_faith_on_duplicate_units() {
        let (r, mut world, _) = setup();
        apply_faith(&mut world, &r, CivId(0), 10, Fixed::ONE);
        apply_faith(&mut world, &r, CivId(0), 200, Fixed::ONE);
        let count = world.units.len();
        assert_eq!(apply_faith(&mut world, &r, CivId(0), 500, Fixed::ONE), FaithEvent::Stored { faith: 500 });
        assert_eq!(world.units.len(), count);
        assert_eq!(world.civ(CivId(0)).unwrap().faith.prophets_earned, 1);
    }

    #[test]
    fn missionary_spreads_twice_and_is_consumed_with_its_last_charge() {
        let (r, mut world, [home, foreign, another]) = setup();
        creed(&mut world, home, CivId(0), 2500);
        let unit = born(&mut world, &r, "Missionary", CivId(0), home);
        assert_eq!(world.units.get(unit).unwrap().religious_uses_left, 2);
        for (target, left) in [(foreign, 1), (another, 0)] {
            let target_position = world.cities.get(target).unwrap().centre;
            world.units.get_mut(unit).unwrap().position = target_position;
            world.units.get_mut(unit).unwrap().refresh(4);
            let result = perform(&mut world, &r, unit, ReligiousAction::SpreadReligion(target)).unwrap();
            assert!(
                matches!(result, ReligiousEvent::Spread { pressure: 1000, uses_left, to: Some(CivId(0)), .. } if uses_left == left)
            );
            assert_eq!(world.cities.get(target).unwrap().faith.pressure_of(Some(CivId(0))), 1000);
            if left > 0 {
                assert_eq!(world.units.get(unit).unwrap().movement_left, Fixed::ZERO);
            }
        }
        assert!(world.units.get(unit).is_none());
        assert!(!world.civ(CivId(0)).unwrap().units.contains(&unit));
    }

    #[test]
    fn inquisitor_protects_the_adjacent_city_and_removes_heresy_only_at_home() {
        let (r, mut world, [home, foreign, _]) = setup();
        creed(&mut world, home, CivId(0), 2500);
        creed(&mut world, foreign, CivId(1), 2500);
        let missionary = born(&mut world, &r, "Missionary", CivId(0), home);
        let inquisitor = born(&mut world, &r, "Inquisitor", CivId(1), foreign);
        let centre = world.cities.get(foreign).unwrap().centre;
        world.units.get_mut(missionary).unwrap().position = centre;
        world.units.get_mut(inquisitor).unwrap().position = world.grid.neighbours(centre).next().unwrap();
        let snapshot = world.clone();
        assert_eq!(
            perform(&mut world, &r, missionary, ReligiousAction::SpreadReligion(foreign)),
            Err(ReligiousRejection::ProtectedByInquisitor)
        );
        assert_eq!(world, snapshot);
        let home_tile = world.cities.get(home).unwrap().centre;
        world.units.get_mut(inquisitor).unwrap().position = home_tile;
        assert_eq!(
            can(&world, &r, inquisitor, ReligiousAction::RemoveHeresy(home)),
            Err(ReligiousRejection::NotOurCity)
        );
        world.units.get_mut(inquisitor).unwrap().position = centre;
        creed(&mut world, foreign, CivId(0), 4000);
        perform(&mut world, &r, inquisitor, ReligiousAction::RemoveHeresy(foreign)).unwrap();
        let city = world.cities.get(foreign).unwrap();
        assert_eq!(city.faith.pressure_of(Some(CivId(0))), 0);
        assert_eq!(city.faith.pressure_of(Some(CivId(1))), 2500);
        assert_eq!(city.faith.majority(city.population), Some(CivId(1)));
        assert!(world.units.get(inquisitor).is_none());
    }

    #[test]
    fn heresy_removal_suppresses_and_restores_a_holy_city() {
        let (r, mut world, [home, _, _]) = setup();
        creed(&mut world, home, CivId(0), 500);
        creed(&mut world, home, CivId(1), 2500);
        world.cities.get_mut(home).unwrap().faith.holy_city_of = Some(CivId(1));
        let inquisitor = born(&mut world, &r, "Inquisitor", CivId(0), home);
        world.units.get_mut(inquisitor).unwrap().religion = Some(CivId(0));
        perform(&mut world, &r, inquisitor, ReligiousAction::RemoveHeresy(home)).unwrap();
        assert!(world.cities.get(home).unwrap().faith.holy_city_blocked);
        crate::world::religion::spread(&mut world, &r, 6);
        assert_eq!(world.cities.get(home).unwrap().faith.pressure_of(Some(CivId(1))), 0);
        creed(&mut world, home, CivId(1), 2500);
        let restorer = born(&mut world, &r, "Inquisitor", CivId(0), home);
        assert_eq!(world.units.get(restorer).unwrap().religion, Some(CivId(1)));
        perform(&mut world, &r, restorer, ReligiousAction::RemoveHeresy(home)).unwrap();
        assert!(!world.cities.get(home).unwrap().faith.holy_city_blocked);
    }

    #[test]
    fn missionary_border_attrition_stops_with_open_borders_and_eventually_destroys() {
        let (r, mut world, [home, foreign, _]) = setup();
        creed(&mut world, home, CivId(0), 2500);
        let missionary = born(&mut world, &r, "Missionary", CivId(0), home);
        world.units.get_mut(missionary).unwrap().position = world.cities.get(foreign).unwrap().centre;
        assert_eq!(
            end_turn(&mut world, &r, CivId(0)),
            vec![ReligiousEvent::Attrition { unit: missionary, strength_left: 750, destroyed: false }]
        );
        world.diplomacy.set_open_borders(CivId(1), CivId(0), true);
        assert!(end_turn(&mut world, &r, CivId(0)).is_empty());
        assert_eq!(world.units.get(missionary).unwrap().religious_strength, 750);
        world.diplomacy.set_open_borders(CivId(1), CivId(0), false);
        for strength in [500, 250, 0] {
            assert_eq!(
                end_turn(&mut world, &r, CivId(0)),
                vec![ReligiousEvent::Attrition { unit: missionary, strength_left: strength, destroyed: strength == 0 }]
            );
        }
        assert!(world.units.get(missionary).is_none());
    }

    #[test]
    fn a_prophet_that_spread_cannot_found_enhance_or_construct_a_holy_site() {
        let (r, mut world, [home, foreign, _]) = setup();
        creed(&mut world, home, CivId(0), 2500);
        let prophet = born(&mut world, &r, "Great Prophet", CivId(0), home);
        world.units.get_mut(prophet).unwrap().position = world.cities.get(foreign).unwrap().centre;
        perform(&mut world, &r, prophet, ReligiousAction::SpreadReligion(foreign)).unwrap();
        let unit = world.units.get_mut(prophet).unwrap();
        unit.position = world.cities.get(home).unwrap().centre;
        unit.refresh(2);
        for action in
            [ReligiousAction::FoundReligion, ReligiousAction::EnhanceReligion, ReligiousAction::ConstructHolySite]
        {
            assert_eq!(can(&world, &r, prophet, action), Err(ReligiousRejection::AlreadyUsed));
        }
        assert!(
            crate::world::great_person_actions::can(
                &world,
                &r,
                prophet,
                crate::world::great_person_actions::Action::Construct(r.improvements.id("Holy site").unwrap()),
                Default::default(),
            )
            .is_err(),
            "the generic great-person action cannot bypass the unused-prophet restriction"
        );
    }

    #[test]
    fn an_unused_prophet_builds_the_rulesets_holy_site() {
        let (r, mut world, [home, _, _]) = setup();
        let unit = born(&mut world, &r, "Great Prophet", CivId(0), home);
        let centre = world.cities.get(home).unwrap().centre;
        let tile = world.grid.neighbours(centre).next().unwrap();
        world.units.get_mut(unit).unwrap().position = tile;
        let event = perform(&mut world, &r, unit, ReligiousAction::ConstructHolySite).unwrap();
        assert!(matches!(event, ReligiousEvent::HolySiteConstructed { .. }));
        assert_eq!(world.grid.improvement[tile.0 as usize], r.improvements.id("Holy site"));
        assert!(world.units.get(unit).is_none());
    }

    #[test]
    fn faith_purchase_initializes_the_birth_city_creed_and_ai_can_buy_then_spread() {
        let (r, mut world, [home, _, _]) = setup();
        let item = Construction::Unit(r.unit("Missionary").unwrap());
        world.civ_mut(CivId(0)).unwrap().faith.stored = 1000;
        assert!(crate::world::purchase::can_purchase(&world, &r, home, item, Stat::Faith, Default::default()).is_err());
        creed(&mut world, home, CivId(0), 2500);
        world.civ_mut(CivId(0)).unwrap().faith.enhanced = true;
        let chosen = choose_purchase(&world, &r, CivId(0), Default::default());
        assert_eq!(chosen, Some((home, item)));
        let paid =
            crate::world::purchase::purchase(&mut world, &r, home, item, Stat::Faith, Default::default()).unwrap();
        assert!(paid > 0);
        let id = world.civ(CivId(0)).unwrap().units[0];
        let unit = world.units.get(id).unwrap();
        assert_eq!((unit.religion, unit.religious_strength, unit.religious_uses_left), (Some(CivId(0)), 1000, 2));
        assert_eq!(unit.movement_left, Fixed::ZERO);
        assert_eq!(world.civ(CivId(0)).unwrap().faith.stored, 1000 - paid as i32);
        assert_eq!(
            crate::world::purchase::can_purchase(&world, &r, home, item, Stat::Faith, Default::default()),
            Err(crate::world::purchase::PurchaseRejection::TileOccupied)
        );
        world.units.get_mut(id).unwrap().refresh(4);
        assert!(matches!(plan_action(&world, &r, CivId(0), id), Some(UnitOrder::MoveTo(_))));
    }

    #[test]
    fn spreading_belief_rewards_use_the_previous_followers_and_strength_conditionals_apply() {
        let (r, mut world, [home, foreign, _]) = setup();
        creed(&mut world, home, CivId(0), 2500);
        creed(&mut world, foreign, CivId(1), 2500);
        let rewards = r.beliefs.id("Interfaith Dialogue").unwrap();
        let strength = r.beliefs.id("Missionary Zeal").unwrap();
        world.civ_mut(CivId(0)).unwrap().faith.beliefs.extend([rewards, strength]);
        crate::world::policies::rebuild_uniques(&mut world, &r, CivId(0));
        let followers_before = world.cities.get(foreign).unwrap().faith.followers_of(CivId(1));
        let missionary = born(&mut world, &r, "Missionary", CivId(0), home);
        world.units.get_mut(missionary).unwrap().position = world.cities.get(foreign).unwrap().centre;
        let result = perform(&mut world, &r, missionary, ReligiousAction::SpreadReligion(foreign)).unwrap();
        let ReligiousEvent::Spread { pressure, rewards, .. } = result else { panic!("spread event") };
        assert_eq!(pressure, 1250, "Missionary Zeal applies to missionaries");
        assert_eq!(rewards.science, Fixed::whole(followers_before as i32 * 10));
        assert_eq!(world.civ(CivId(0)).unwrap().science_stored, followers_before as i32 * 10);
        let prophet = born(&mut world, &r, "Great Prophet", CivId(0), home);
        world.units.get_mut(prophet).unwrap().position = world.cities.get(foreign).unwrap().centre;
        assert_eq!(
            spread_strength(&world, &r, prophet, foreign),
            1000,
            "the missionary bonus must not affect prophets"
        );
    }

    #[test]
    fn a_purchased_inquisitor_carries_the_citys_religion_even_when_its_owner_founded_another() {
        let (r, mut world, [home, _, _]) = setup();
        world.civ_mut(CivId(0)).unwrap().faith.founded = true;
        world.civ_mut(CivId(0)).unwrap().faith.stored = 1000;
        creed(&mut world, home, CivId(1), 2500);
        let item = Construction::Unit(r.unit("Inquisitor").unwrap());
        crate::world::purchase::purchase(&mut world, &r, home, item, Stat::Faith, Default::default()).unwrap();
        let unit = world.units.get(world.civ(CivId(0)).unwrap().units[0]).unwrap();
        assert_eq!(unit.religion, Some(CivId(1)));
        assert_eq!(unit.religious_uses_left, 1);
    }

    #[test]
    fn enhancement_requires_both_belief_types_and_failed_actions_do_not_consume_the_prophet() {
        let (r, mut world, [home, _, _]) = setup();
        creed(&mut world, home, CivId(0), 2500);
        let unavailable: Vec<_> =
            r.beliefs.iter().filter(|(_, belief)| belief.r#type == "Enhancer").map(|(id, _)| id).collect();
        world.civ_mut(CivId(1)).unwrap().faith.beliefs.extend(unavailable);
        let prophet = born(&mut world, &r, "Great Prophet", CivId(0), home);
        let snapshot = world.clone();
        assert_eq!(
            perform(&mut world, &r, prophet, ReligiousAction::EnhanceReligion),
            Err(ReligiousRejection::NoBeliefs)
        );
        assert_eq!(world, snapshot);
    }
}
