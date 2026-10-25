// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Shared unit training and battle aftermath rules.

use crate::arena::Id;
use crate::rng::Rng;
use crate::ruleset::Ruleset;
use crate::stats::Fixed;
use crate::world::conditionals::{applies, CombatRole, GameContext, Opponent};
use crate::world::{City, CivId, TileId, Unit, World};
use crate::{Unique, UniqueType};

fn ctx<'a>(
    world: &'a World,
    r: &'a Ruleset,
    id: Id<Unit>,
    opponent: Option<Id<Unit>>,
    defending: bool,
) -> GameContext<'a> {
    let unit = world.units.get(id).expect("existing combatant");
    let mut context = GameContext::new(r)
        .with_world(world)
        .for_civ(unit.owner)
        .for_unit(id)
        .on_tile(unit.position)
        .in_combat(if defending { CombatRole::Defending } else { CombatRole::Attacking });
    context.opponent = Some(opponent.map_or(Opponent::City, Opponent::Unit));
    context
}

fn uniques<'a>(world: &'a World, r: &'a Ruleset, id: Id<Unit>) -> impl Iterator<Item = &'a Unique> {
    world.units.get(id).into_iter().flat_map(move |unit| {
        unit.all_uniques(r).chain(world.civ(unit.owner).into_iter().flat_map(|c| c.uniques.all()))
    })
}

fn number(u: &Unique, n: usize) -> i32 {
    u.params.get(n).and_then(|p| p.parse().ok()).unwrap_or(0)
}

pub fn set_up(world: &mut World, r: &Ruleset, id: Id<Unit>) -> bool {
    let Some(unit) = world.units.get(id) else { return false };
    if unit.set_up || !unit.can_move() || !unit.has_unique(r, UniqueType::MustSetUp) || unit.embarked {
        return false;
    }
    let unit = world.units.get_mut(id).expect("validated setup");
    unit.set_up = true;
    unit.fortification_turns = 0;
    unit.moved_this_turn = true;
    unit.movement_left = (unit.movement_left - Fixed::ONE).max(Fixed::ZERO);
    true
}

pub fn may_attack(world: &World, r: &Ruleset, id: Id<Unit>, target: crate::world::air::Target) -> bool {
    let Some(unit) = world.units.get(id) else { return false };
    let (opponent, tile) = match target {
        crate::world::air::Target::Unit(other) => {
            let Some(u) = world.units.get(other) else { return false };
            (Some(other), u.position)
        }
        crate::world::air::Target::City(city) => {
            let Some(c) = world.cities.get(city) else { return false };
            (None, c.centre)
        }
    };
    let context = ctx(world, r, id, opponent, false).on_tile(tile);
    if !unit.is_military(r) || (unit.embarked && (unit.is_ranged(r) || world.grid.is_water(r, tile))) {
        return false;
    }
    for rule in uniques(world, r, id).filter(|u| applies(u, &context).holds()) {
        match rule.unique_type() {
            Some(UniqueType::CannotAttack) => return false,
            Some(UniqueType::CanOnlyAttackUnits) => {
                let compiled = rule.compiled(r);
                let allowed = rule.params.first().zip(compiled.filter(0)).is_some_and(|(filter, compiled)| {
                    if let Some(other) = opponent {
                        crate::world::filters::matches_unit_in(
                            world,
                            r,
                            world.units.get(other).expect("target"),
                            compiled,
                        )
                        .holds()
                    } else {
                        matches!(filter.as_str(), "City" | "All")
                    }
                });
                if !allowed {
                    return false;
                }
            }
            Some(UniqueType::CanOnlyAttackTiles)
                if !rule
                    .compiled(r)
                    .filter(0)
                    .is_some_and(|filter| crate::world::conditionals::matches_tile(&context, tile, filter).holds()) =>
            {
                return false
            }
            _ => {}
        }
    }
    true
}

pub fn available_promotions(world: &World, r: &Ruleset, id: Id<Unit>) -> Vec<Id<crate::ruleset::Promotion>> {
    let Some(unit) = world.units.get(id) else { return Vec::new() };
    let context = GameContext::new(r).with_world(world).for_civ(unit.owner).for_unit(id).on_tile(unit.position);
    crate::world::combat::available_promotions(r, unit)
        .into_iter()
        .filter(|id| {
            r.promotions[*id].uniques.iter().all(|rule| match rule.unique_type() {
                Some(UniqueType::Unavailable) => !applies(rule, &context).holds(),
                Some(UniqueType::OnlyAvailable) => applies(rule, &context).holds(),
                _ => true,
            })
        })
        .collect()
}

pub fn promote(
    world: &mut World,
    r: &Ruleset,
    id: Id<Unit>,
    promotion: Id<crate::ruleset::Promotion>,
) -> Result<(), &'static str> {
    if !available_promotions(world, r, id).contains(&promotion) {
        return Err("Promotion unavailable");
    }
    let old_health = world.units.get(id).ok_or("No such unit")?.health;
    crate::world::combat::choose_promotion(world.units.get_mut(id).expect("existing unit"), r, promotion)?;
    // The compatibility helper has no world context. Reapply the immediate
    // healing effect here through the same modifiers as other healing sources.
    let healing: i32 = r.promotions[promotion]
        .uniques
        .iter()
        .filter(|u| {
            u.unique_type() == Some(UniqueType::OneTimeUnitHeal) && u.params.first().is_some_and(|p| p == "This Unit")
        })
        .map(|u| number(u, 1))
        .sum();
    if healing > 0 {
        world.units.get_mut(id).expect("existing unit").health = old_health;
        crate::world::healing::heal(world, r, id, healing);
    }
    Ok(())
}

pub fn promotion_choice(world: &World, r: &Ruleset, id: Id<Unit>) -> Option<Id<crate::ruleset::Promotion>> {
    let unit = world.units.get(id)?;
    if !crate::world::combat::can_promote(unit) {
        return None;
    }
    available_promotions(world, r, id)
        .into_iter()
        .filter(|id| {
            unit.health < 60
                || !r.promotions[*id].uniques.iter().any(|u| u.unique_type() == Some(UniqueType::OneTimeUnitHeal))
        })
        .max_by_key(|id| {
            let score: i32 = r.promotions[*id]
                .uniques
                .iter()
                .map(|rule| match rule.unique_type() {
                    Some(UniqueType::OneTimeUnitHeal) => 100,
                    Some(UniqueType::AdditionalAttacks) => 80,
                    Some(UniqueType::Strength) => number(rule, 0).max(0),
                    Some(UniqueType::Range) => 40,
                    _ => 0,
                })
                .sum();
            (score, std::cmp::Reverse(*id))
        })
}

/// Free base and civilization promotions apply to every newly created unit.
pub fn initialize_promotions(world: &World, r: &Ruleset, unit: &mut Unit) {
    let initial = unit.clone();
    let context =
        GameContext::new(r).with_world(world).for_civ(unit.owner).for_unit_value(&initial).on_tile(unit.position);
    for name in r.units.get(unit.base).into_iter().flat_map(|b| &b.promotions) {
        if let Some(p) = r.promotions.id(name) {
            unit.promotions.insert(p);
        }
    }
    for rule in world.civ(unit.owner).into_iter().flat_map(|c| c.uniques.matching(UniqueType::UnitsGainPromotion)) {
        if rule.params.len() < 2
            || !applies(rule, &context).holds()
            || !rule
                .compiled(r)
                .filter(0)
                .is_some_and(|f| crate::world::filters::matches_unit_in(world, r, &initial, f).holds())
        {
            continue;
        }
        if let Some(p) = r.promotions.id(&rule.params[1]) {
            unit.promotions.insert(p);
        }
    }
}

/// Barracks/Armory/etc and trained free promotions apply to production and
/// purchases; general free-unit placement does not silently gain city training.
pub fn train(world: &World, r: &Ruleset, city: Id<City>, unit: &mut Unit) {
    initialize_promotions(world, r, unit);
    let Some(c) = world.cities.get(city) else { return };
    let religious_uses_before = crate::world::religious_units::initial_uses(r, unit);
    let initial = unit.clone();
    let context = GameContext::new(r)
        .with_world(world)
        .for_civ(unit.owner)
        .for_unit_value(&initial)
        .in_city(city)
        .on_tile(c.centre);
    for rule in crate::world::religion::city_uniques(world, r, c) {
        if !applies(rule, &context).holds() {
            continue;
        }
        match rule.unique_type() {
            Some(UniqueType::UnitStartingExperience) if rule.params.len() >= 3 => {
                if rule.compiled(r).filter(0).is_some_and(|f| crate::world::filters::matches_unit(r, unit, f).holds())
                    && crate::world::filters::city_matches(world, r, c, Some(c), &rule.params[2]).holds()
                {
                    unit.xp = (i32::from(unit.xp) + number(rule, 1)).clamp(0, i32::from(u16::MAX)) as u16;
                }
            }
            Some(UniqueType::UnitStartingPromotions) if rule.params.len() >= 3 => {
                let Some(promotion) = r.promotions.id(&rule.params[2]) else { continue };
                let relevant = rule.params[0] == "relevant"
                    && r.promotions[promotion]
                        .unit_types
                        .iter()
                        .any(|kind| r.units.get(unit.base).is_some_and(|base| base.unit_type == *kind));
                if (relevant
                    || rule
                        .compiled(r)
                        .filter(0)
                        .is_some_and(|f| crate::world::filters::matches_unit(r, unit, f).holds()))
                    && crate::world::filters::city_matches(world, r, c, Some(c), &rule.params[1]).holds()
                {
                    unit.promotions.insert(promotion);
                }
            }
            _ => {}
        }
    }
    let extra_uses = crate::world::religious_units::initial_uses(r, unit).saturating_sub(religious_uses_before);
    unit.religious_uses_left = unit.religious_uses_left.saturating_add(extra_uses);
}

pub fn award_xp(
    world: &mut World,
    r: &Ruleset,
    id: Id<Unit>,
    base: i32,
    opponent: Option<Id<Unit>>,
    other: CivId,
    defending: bool,
) -> u16 {
    let Some(unit) = world.units.get(id) else { return 0 };
    let barbarian = world.civ(other).is_some_and(|c| c.is_barbarian);
    if barbarian && unit.xp >= crate::world::combat::MAX_XP_FROM_BARBARIANS {
        return 0;
    }
    let context = ctx(world, r, id, opponent, defending);
    let mut flat = base;
    let mut percent = 100;
    for rule in uniques(world, r, id).filter(|u| applies(u, &context).holds()) {
        match rule.unique_type() {
            Some(UniqueType::FlatXPGain) => flat += number(rule, 0),
            Some(UniqueType::PercentageXPGain) => percent += number(rule, 0),
            _ => {}
        }
    }
    let amount = (i64::from(flat) * i64::from(percent) / 100).clamp(0, i64::from(u16::MAX)) as u16;
    let unit = world.units.get_mut(id).expect("existing combatant");
    let old = unit.xp;
    unit.xp = unit.xp.saturating_add(amount);
    let gained = unit.xp - old;
    crate::world::great_people::on_combat_xp(world, r, id, gained, barbarian);
    gained
}

pub fn spend_attack(world: &mut World, r: &Ruleset, id: Id<Unit>, advanced: bool) {
    let Some(unit) = world.units.get(id) else { return };
    let context = ctx(world, r, id, None, false);
    let may_move = uniques(world, r, id)
        .any(|u| u.unique_type() == Some(UniqueType::CanMoveAfterAttacking) && applies(u, &context).holds());
    let further = unit.attacks_this_turn.saturating_add(1) < crate::world::air::max_attacks(world, r, id);
    let air = unit.is_air(r);
    let unit = world.units.get_mut(id).expect("existing combatant");
    unit.spend_attack();
    if !may_move && !further {
        unit.movement_left = Fixed::ZERO;
    } else if !air && !advanced {
        unit.movement_left = (unit.movement_left - Fixed::ONE).max(Fixed::ZERO);
    }
    unit.fortified = false;
    unit.fortification_turns = 0;
}

fn random(world: &World, attacker: Id<Unit>, defender: Id<Unit>, phase: u64) -> Rng {
    Rng::from_parts(&[
        0xC0_AB00 + phase,
        u64::from(world.turn),
        attacker.index() as u64,
        attacker.generation() as u64,
        defender.index() as u64,
        defender.generation() as u64,
        world.units.get(attacker).map_or(0, |u| u64::from(u.attacks_this_turn)),
        world.units.get(defender).map_or(0, |u| u64::from(u.position.0)),
    ])
}

/// Conditional chance is rolled once per attempted battle, from stable state.
pub fn withdraw(world: &mut World, r: &Ruleset, attacker: Id<Unit>, defender: Id<Unit>) -> bool {
    let (Some(a), Some(d)) = (world.units.get(attacker), world.units.get(defender)) else { return false };
    if a.is_ranged(r) || d.embarked || d.has_unique(r, UniqueType::CannotMove) || d.is_civilian(r) {
        return false;
    }
    let context = ctx(world, r, defender, Some(attacker), true);
    let mut rng = random(world, attacker, defender, 1);
    let has_withdraw = uniques(world, r, defender)
        .filter(|u| u.unique_type() == Some(UniqueType::WithdrawsBeforeMeleeCombat))
        .any(|rule| {
            rule.conditionals.iter().all(|condition| {
                if condition.unique_type() == Some(UniqueType::ConditionalChance) {
                    rng.range(0, 99) < i64::from(number(condition, 0))
                } else {
                    crate::world::conditionals::evaluate(condition, &context).holds()
                }
            })
        });
    if !has_withdraw {
        return false;
    }
    let from = d.position;
    let adjacent: Vec<_> = world.grid.neighbours(a.position).collect();
    let mut legal: Vec<_> = world
        .grid
        .neighbours(from)
        .filter(|tile| *tile != a.position)
        .filter(|tile| {
            (!d.is_land_unit(r) || !world.grid.is_water(r, *tile))
                && crate::world::movement::can_traverse(world, r, d, from, *tile)
                && crate::world::movement::can_pass_through(world, r, *tile, d)
                && !crate::world::movement::is_tile_occupied_for(world, r, *tile, d)
                && !world.cities.iter().any(|(_, c)| c.centre == *tile && c.owner != d.owner)
        })
        .collect();
    if legal.iter().any(|tile| !adjacent.contains(tile)) {
        legal.retain(|tile| !adjacent.contains(tile));
    }
    if legal.is_empty() {
        return false;
    }
    let to = legal[rng.range(0, legal.len() as i64 - 1) as usize];
    let defender_unit = world.units.get_mut(defender).expect("withdrawal defender");
    defender_unit.position = to;
    defender_unit.set_up = false;
    crate::world::air::move_payload(world, defender, to);
    on_enter(world, r, defender);
    spend_attack(world, r, attacker, false);
    true
}

/// Decide before destroying the defender, while its combat context is intact.
pub fn captures_kill(
    world: &mut World,
    r: &Ruleset,
    attacker: Id<Unit>,
    defender: Id<Unit>,
    attack: Fixed,
    defense: Fixed,
) -> bool {
    let (Some(a), Some(d)) = (world.units.get(attacker), world.units.get(defender)) else { return false };
    if d.is_civilian(r) || d.has_unique(r, UniqueType::Uncapturable) {
        return false;
    }
    let context = ctx(world, r, attacker, Some(defender), false);
    let applicable = uniques(world, r, attacker).any(|rule| {
        rule.unique_type() == Some(UniqueType::KillUnitCapture)
            && applies(rule, &context).holds()
            && rule
                .compiled(r)
                .filter(0)
                .is_some_and(|f| crate::world::filters::matches_unit_in(world, r, d, f).holds())
    });
    let mut rng = random(world, attacker, defender, 2);
    let recruited_gold: Option<i32> = (!a.is_ranged(r))
        .then(|| {
            let mut rewards = Vec::new();
            for rule in
                uniques(world, r, attacker).filter(|u| u.unique_type() == Some(UniqueType::GainFromDefeatingUnit))
            {
                if rule
                    .compiled(r)
                    .filter(0)
                    .is_none_or(|f| !crate::world::filters::matches_unit_in(world, r, d, f).holds())
                {
                    continue;
                }
                if rule.conditionals.iter().all(|condition| {
                    if condition.unique_type() == Some(UniqueType::ConditionalChance) {
                        rng.range(0, 99) < i64::from(number(condition, 0))
                    } else {
                        crate::world::conditionals::evaluate(condition, &context).holds()
                    }
                }) {
                    rewards.push(number(rule, 1));
                }
            }
            (!rewards.is_empty()).then(|| rewards.into_iter().sum())
        })
        .flatten();
    if let Some(gold) = recruited_gold {
        let owner = a.owner;
        if let Some(civ) = world.civ_mut(owner) {
            civ.gold = civ.gold.saturating_add(gold);
        }
        return true;
    }
    if !applicable {
        return false;
    }
    let chance = (Fixed::permille(100) + attack.checked_div(defense).unwrap_or(Fixed::ONE) * Fixed::permille(400))
        .min(Fixed::permille(800));
    rng.unit_interval() <= chance
}

pub fn spawn_capture(world: &mut World, r: &Ruleset, owner: CivId, defeated: &Unit) -> Option<Id<Unit>> {
    let mut captured =
        crate::world::construction::prepare_unit_near(world, r, owner, defeated.base, defeated.position, None)?;
    captured.health = 50;
    captured.movement_left = Fixed::ZERO;
    Some(world.place_unit(captured))
}

pub fn heal_on_kill(world: &mut World, r: &Ruleset, id: Id<Unit>) {
    if world.units.get(id).is_none() {
        return;
    }
    let context = ctx(world, r, id, None, false);
    let amount = uniques(world, r, id)
        .filter(|u| u.unique_type() == Some(UniqueType::HealsAfterKilling) && applies(u, &context).holds())
        .map(|u| number(u, 0))
        .sum();
    crate::world::healing::heal(world, r, id, amount);
}

pub fn adjacent_promotions(
    world: &World,
    r: &Ruleset,
    unit: &Unit,
) -> std::collections::BTreeSet<Id<crate::ruleset::Promotion>> {
    let context = GameContext::new(r).with_world(world).for_civ(unit.owner).on_tile(unit.position);
    world
        .grid
        .neighbours(unit.position)
        .flat_map(|tile| world.grid.terrains(tile))
        .filter_map(|terrain| r.terrains.get(terrain))
        .flat_map(|t| &t.uniques)
        .filter(|u| {
            u.unique_type() == Some(UniqueType::TerrainGrantsPromotion)
                && applies(u, &context).holds()
                && u.compiled(r)
                    .filter(2)
                    .is_some_and(|f| crate::world::filters::matches_unit_in(world, r, unit, f).holds())
        })
        .filter_map(|u| r.promotions.id(u.params.first()?))
        .collect()
}

pub fn on_enter(world: &mut World, r: &Ruleset, id: Id<Unit>) {
    let Some(unit) = world.units.get(id) else { return };
    let promotions = adjacent_promotions(world, r, unit);
    world.units.get_mut(id).expect("existing unit").promotions.extend(promotions);
}

/// End-round hazards occur after recovery. Citadels take only the strongest
/// adjacent damage source; terrain damage then stacks across terrain layers.
pub fn end_round(world: &mut World, r: &Ruleset) -> Vec<(Id<Unit>, CivId, u8, bool)> {
    let damage: Vec<_> = world
        .units
        .iter()
        .map(|(id, unit)| {
            let context = GameContext::new(r).with_world(world).for_civ(unit.owner).for_unit(id).on_tile(unit.position);
            let terrain: i32 = world
                .grid
                .terrains(unit.position)
                .filter_map(|id| r.terrains.get(id))
                .flat_map(|t| &t.uniques)
                .filter(|u| u.unique_type() == Some(UniqueType::DamagesContainingUnits) && applies(u, &context).holds())
                .map(|u| number(u, 0))
                .sum();
            let citadel = world
                .grid
                .neighbours(unit.position)
                .filter(|tile| {
                    world.grid.owner[tile.0 as usize].is_some_and(|owner| world.diplomacy.at_war(unit.owner, owner))
                })
                .filter_map(|tile| Some((tile, r.improvements.get(world.grid.unpillaged_improvement(tile)?)?)))
                .map(|(tile, improvement)| {
                    let context = GameContext::new(r)
                        .with_world(world)
                        .on_tile(tile)
                        .for_civ(world.grid.owner[tile.0 as usize].expect("owned improvement"));
                    improvement
                        .uniques
                        .iter()
                        .filter(|u| {
                            u.unique_type() == Some(UniqueType::DamagesAdjacentEnemyUnits)
                                && applies(u, &context).holds()
                        })
                        .map(|u| number(u, 0))
                        .sum::<i32>()
                })
                .max()
                .unwrap_or(0);
            (id, (terrain + citadel).max(0))
        })
        .collect();
    let mut events = Vec::new();
    for (id, damage) in damage {
        let Some(unit) = world.units.get(id) else { continue };
        if damage == 0 {
            continue;
        }
        let died = damage >= i32::from(unit.health);
        events.push((id, unit.owner, damage.min(i32::from(unit.health)) as u8, died));
        if died {
            world.remove_unit(id);
        } else if let Some(unit) = world.units.get_mut(id) {
            unit.health -= damage as u8;
        }
    }
    events
}

pub fn credit_yields(world: &mut World, owner: CivId, tile: TileId, yields: crate::stats::Stats) {
    let city = world
        .cities
        .iter()
        .filter(|(_, c)| c.owner == owner)
        .min_by_key(|(id, c)| (world.grid.distance(tile, c.centre), *id))
        .map(|(id, _)| id);
    if let Some(civ) = world.civ_mut(owner) {
        civ.gold = civ.gold.saturating_add((yields.gold.raw() / 1000) as i32);
        civ.science_stored = civ.science_stored.saturating_add((yields.science.raw() / 1000) as i32);
        civ.culture_stored = civ.culture_stored.saturating_add((yields.culture.raw() / 1000) as i32);
        civ.faith.stored = civ.faith.stored.saturating_add((yields.faith.raw() / 1000) as i32);
    }
    if let Some(city) = city.and_then(|id| world.cities.get_mut(id)) {
        city.food_stored = city.food_stored.saturating_add((yields.food.raw() / 1000) as i32);
        crate::world::construction::add_production_points(city, (yields.production.raw() / 1000) as i32);
    }
}

/// Enter defeated ground using its normal movement cost, without requiring
/// movement left after the attack has already consumed the unit's action.
pub fn advance(world: &mut World, r: &Ruleset, id: Id<Unit>, to: TileId) -> bool {
    let Some(unit) = world.units.get(id) else { return false };
    if !crate::world::movement::can_pass_through(world, r, to, unit)
        || crate::world::movement::is_tile_occupied_for(world, r, to, unit)
    {
        return false;
    }
    let Some(cost) = crate::world::movement::step_cost_for_unit_with_zoc(
        world,
        r,
        unit,
        unit.position,
        to,
        unit.movement_left,
        !unit.has_unique(r, UniqueType::CanMoveAfterAttacking),
    ) else {
        return false;
    };
    let embarked =
        unit.is_land_unit(r) && world.grid.is_water(r, to) && !unit.has_unique(r, UniqueType::CanMoveOnWater);
    let unit = world.units.get_mut(id).expect("advancing survivor");
    unit.position = to;
    unit.movement_left = (unit.movement_left - cost).max(Fixed::ZERO);
    unit.moved_this_turn = true;
    unit.fortified = false;
    unit.fortification_turns = 0;
    unit.set_up = false;
    unit.embarked = embarked;
    crate::world::air::move_payload(world, id, to);
    on_enter(world, r, id);
    true
}

pub fn capture_city_occupants(world: &mut World, r: &Ruleset, owner: CivId, tile: TileId) {
    for id in world.units_at(tile) {
        let Some(unit) = world.units.get(id) else { continue };
        if unit.owner == owner {
            continue;
        }
        if unit.is_military(r) || unit.has_unique(r, UniqueType::Uncapturable) {
            world.remove_unit(id);
        } else {
            let demote = r.units.get(unit.base).is_some_and(|b| b.name == "Settler");
            world.transfer_unit(id, owner);
            let unit = world.units.get_mut(id).expect("captured civilian");
            if demote {
                if let Some(worker) = r.unit("Worker") {
                    unit.base = worker;
                }
            }
            unit.movement_left = Fixed::ZERO;
        }
    }
}

/// Source battle exchange: a fatal hit ends retaliation immediately.
pub(crate) fn exchange(
    mut rng: Rng,
    a_health: i32,
    d_health: i32,
    mut to_a: i32,
    mut to_d: i32,
    city: bool,
) -> (i32, i32) {
    let (mut a, mut d) = (a_health, d_health);
    while to_a + to_d > 0 && a > 0 && d > if city { 1 } else { 0 } {
        if rng.range(0, i64::from(to_a) + i64::from(to_d) - 1) < i64::from(to_d) {
            to_d -= 1;
            d -= 1;
        } else {
            to_a -= 1;
            a -= 1;
        }
    }
    (a_health - a, d_health - d)
}

/// Apply combat rewards while both combatants still exist for conditionals.
pub fn reward_kill(world: &mut World, r: &Ruleset, winner: Id<Unit>, defeated: Id<Unit>, defending: bool) {
    let (Some(unit), Some(enemy)) = (world.units.get(winner), world.units.get(defeated)) else { return };
    let (owner, position) = (unit.owner, unit.position);
    let (loser, lost_at, military) = (enemy.owner, enemy.position, enemy.is_military(r));
    let context = ctx(world, r, winner, Some(defeated), defending);
    let mut rules: Vec<_> = uniques(world, r, winner)
        .filter(|u| u.unique_type() == Some(UniqueType::KillUnitPlunder) && applies(u, &context).holds())
        .collect();
    let reward = kill_reward(world, r, position, &context, enemy, &mut rules);
    credit_yields(world, owner, position, reward);
    crate::world::quests::on_unit_killed(world, r, owner, loser, lost_at, military);
}

pub fn reward_city_kill(world: &mut World, r: &Ruleset, city: Id<City>, defeated: Id<Unit>) {
    let (Some(city), Some(enemy)) = (world.cities.get(city), world.units.get(defeated)) else { return };
    let (owner, position) = (city.owner, city.centre);
    let (loser, lost_at, military) = (enemy.owner, enemy.position, enemy.is_military(r));
    let context = GameContext::new(r)
        .with_world(world)
        .for_civ(owner)
        .on_tile(position)
        .against(Opponent::Unit(defeated))
        .in_combat(CombatRole::Attacking);
    let mut rules: Vec<_> = world
        .civ(owner)
        .into_iter()
        .flat_map(|c| c.uniques.matching(UniqueType::KillUnitPlunder))
        .filter(|u| applies(u, &context).holds())
        .collect();
    let reward = kill_reward(world, r, position, &context, enemy, &mut rules);
    credit_yields(world, owner, position, reward);
    crate::world::quests::on_unit_killed(world, r, owner, loser, lost_at, military);
}
fn kill_reward<'a>(
    world: &'a World,
    r: &'a Ruleset,
    position: TileId,
    context: &GameContext<'a>,
    enemy: &Unit,
    rules: &mut Vec<&'a Unique>,
) -> crate::stats::Stats {
    let Some(base) = r.units.get(enemy.base) else { return crate::stats::Stats::ZERO };
    if let Some((_, city)) =
        world.cities.iter().filter(|(_, c)| world.grid.distance(position, c.centre) <= 4).find(|(_, c)| {
            crate::world::religion::city_uniques(world, r, c)
                .iter()
                .any(|u| u.unique_type() == Some(UniqueType::KillUnitPlunderNearCity) && applies(u, context).holds())
        })
    {
        rules.extend(
            crate::world::religion::city_uniques(world, r, city).into_iter().filter(|u| {
                u.unique_type() == Some(UniqueType::KillUnitPlunderNearCity) && applies(u, context).holds()
            }),
        );
    }
    let mut reward = crate::stats::Stats::ZERO;
    for rule in rules.iter() {
        if rule.params.len() < 4
            || !rule
                .compiled(r)
                .filter(1)
                .is_some_and(|f| crate::world::filters::matches_unit_in(world, r, enemy, f).holds())
        {
            continue;
        }
        let source = if rule.params[2] == "Cost" {
            base.cost.unwrap_or(0)
        } else {
            base.strength.unwrap_or(0).max(base.ranged_strength.unwrap_or(0))
        };
        if let Some(stat) = crate::stats::Stat::parse(&rule.params[3]) {
            reward[stat] += Fixed::whole(source * number(rule, 0) / 100);
        }
    }
    reward
}

pub fn plunder_damage(
    world: &mut World,
    r: &Ruleset,
    attacker: Id<Unit>,
    target: crate::world::air::Target,
    damage: i32,
) {
    let Some(unit) = world.units.get(attacker) else { return };
    let (owner, position) = (unit.owner, unit.position);
    let context = ctx(
        world,
        r,
        attacker,
        match target {
            crate::world::air::Target::Unit(id) => Some(id),
            _ => None,
        },
        false,
    );
    let mut goods = crate::stats::Stats::ZERO;
    for rule in uniques(world, r, attacker).filter(|u| {
        u.unique_type() == Some(UniqueType::DamageUnitsPlunder) && u.params.len() >= 3 && applies(u, &context).holds()
    }) {
        let matched = match target {
            crate::world::air::Target::City(_) => matches!(rule.params[1].as_str(), "All" | "City"),
            crate::world::air::Target::Unit(id) => world.units.get(id).is_some_and(|u| {
                rule.compiled(r)
                    .filter(1)
                    .is_some_and(|f| crate::world::filters::matches_unit_in(world, r, u, f).holds())
            }),
        };
        if matched {
            if let Some(stat) = crate::stats::Stat::parse(&rule.params[2]) {
                goods[stat] += Fixed::whole(damage) * Fixed::from_fraction(i64::from(number(rule, 0)), 100);
            }
        }
    }
    credit_yields(world, owner, position, goods);
}
