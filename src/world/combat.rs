//! Fighting.
//!
//! The system with the most rule surface, and the one place in this engine
//! where [`ModifierStack`](crate::modifiers) is the right tool rather than a
//! forced one. Citizen assignment was a dot product against fixed weights;
//! defence genuinely composes — terrain, fortification, promotions and
//! situational bonuses all stack, and the answer must not depend on which the
//! engine happened to apply first.
//!
//! The damage curve is Unciv's (`BattleDamage.kt:335`), which cites the
//! CivFanatics reconstruction of Civ V's own maths:
//!
//! ```text
//! stronger = max(ratio, 1/ratio)
//! modifier = (((stronger + 3) / 4)^4 + 1) / 2
//! damage   = (24 + 12·random) · modifier · health_ratio
//! ```
//!
//! Two evenly matched units therefore deal about 30 damage to each other, and
//! the curve is steep: doubling your strength roughly triples the damage you
//! deal relative to what you take.
//!
//! Wounded units hit softer — one percent less for every three points of
//! health missing — which is what makes finishing a damaged unit worthwhile
//! rather than merely tidy.

use crate::arena::Id;
use crate::modifiers::{Modifier, ModifierStack, Source, SourceKind};
use crate::rng::Rng;
use crate::ruleset::{BaseUnit, Promotion, Ruleset};
use crate::stats::{Fixed, Precise};
use crate::unique_type::UniqueType;
use crate::world::conditionals::{applies, CombatRole, GameContext, Verdict};
use crate::world::entities::Unit;
use crate::world::grid::{RoadLevel, TileGrid, TileId};
use crate::world::world::World;

/// Percent added per level of fortification (`BattleConstants.kt:11`).
pub const FORTIFICATION_BONUS: i32 = 20;

/// Malus when attacking across a river with melee without bridge or unique.
pub const ATTACKING_ACROSS_RIVER_MALUS: i32 = -20;

/// Malus when land melee unit attacks water unit without amphibious unique.
pub const BOARDING_MALUS: i32 = -50;

/// Malus when water unit attacks land unit without amphibious unique.
pub const LANDING_MALUS: i32 = -50;

/// Flanking bonus percentage per additional adjacent friendly military unit.
pub const BASE_FLANKING_BONUS: i32 = 10;

/// Maximum combat XP that can be earned against barbarian units.
pub const MAX_XP_FROM_BARBARIANS: u16 = 30;

/// Health missing is divided by this to get the damage penalty, so three
/// points of health cost one percent (`BattleConstants.kt:12`).
pub const WOUNDED_DAMAGE_DIVISOR: i32 = 300;

/// Damage before modifiers is centred on 30 and spread ±6.
const BASE_DAMAGE_LOW: i32 = 24;
const BASE_DAMAGE_SPREAD: i32 = 12;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CombatRejection {
    NoSuchUnit,
    /// Known to each other, and at peace.
    NotAtWar,
    /// Units of the same civilization.
    SameOwner,
    /// Melee attacks require adjacency.
    NotAdjacent,
    /// Target is out of attack range.
    OutOfRange,
    /// No movement left to attack with.
    NoMovement,
    /// Unit cannot make ranged attacks.
    NoRangedAttack,
    /// Line of sight is blocked to target.
    TargetBlockedByLOS,
}

/// A unit's strength, and where each part of it came from against a specific target.
pub fn strength_breakdown_against(
    world: &World,
    r: &Ruleset,
    unit_id: Id<Unit>,
    target_tile: Option<TileId>,
    defending: bool,
) -> Option<(Fixed, ModifierStack)> {
    let unit = world.units.get(unit_id)?;
    let base = r.units.get(unit.base)?.strength.unwrap_or(0);
    let mut stack = ModifierStack::new();

    // A unit whose resource has run out still stands; it just fights badly.
    if world
        .civ(unit.owner)
        .is_some_and(|c| crate::world::resources::is_missing_resources(r, c, unit.base))
    {
        stack.add(Modifier::additive_percent(
            Fixed::whole(crate::world::resources::MISSING_RESOURCE_MALUS),
            Source::new(SourceKind::Other, "Missing resource"),
        ));
    }

    if defending {
        for id in world.grid.terrains(unit.position) {
            let Some(t) = r.terrains.get(id) else { continue };
            let Some(bonus) = t.defence_bonus else { continue };
            if bonus.is_zero() {
                continue;
            }
            stack.add(Modifier::additive_percent(
                bonus.mul(Fixed::whole(100)),
                Source::new(SourceKind::Terrain, t.name.clone()),
            ));
        }
        if unit.fortified {
            stack.add(Modifier::additive_percent(
                Fixed::whole(FORTIFICATION_BONUS),
                Source::new(SourceKind::Other, "Fortified"),
            ));
        }
    } else if let Some(target) = target_tile {
        // Across river penalty
        if world.grid.has_river_between(unit.position, target) {
            let has_bridge = world.civ(unit.owner).is_some_and(|c| {
                c.techs.iter().any(|t| {
                    r.technologies.get(*t).is_some_and(|tech| {
                        tech.uniques
                            .iter()
                            .any(|u| u.unique_type() == Some(UniqueType::RoadsConnectAcrossRivers))
                    })
                })
            }) && world.grid.unpillaged_road(unit.position) >= RoadLevel::Road
                && world.grid.unpillaged_road(target) >= RoadLevel::Road;

            if !has_bridge && !unit.has_unique(r, UniqueType::AttackAcrossRiver) {
                stack.add(Modifier::additive_percent(
                    Fixed::whole(ATTACKING_ACROSS_RIVER_MALUS),
                    Source::new(SourceKind::Terrain, "Across river"),
                ));
            }
        }

        // Landing / Boarding malus
        let from_water = world.grid.is_water(r, unit.position);
        let to_water = world.grid.is_water(r, target);
        if from_water && !to_water && !unit.has_unique(r, UniqueType::AttackAcrossCoast) {
            stack.add(Modifier::additive_percent(
                Fixed::whole(LANDING_MALUS),
                Source::new(SourceKind::Terrain, "Landing"),
            ));
        } else if !from_water && to_water && !unit.has_unique(r, UniqueType::AttackAcrossCoast) {
            stack.add(Modifier::additive_percent(
                Fixed::whole(BOARDING_MALUS),
                Source::new(SourceKind::Terrain, "Boarding"),
            ));
        }

        // Flanking bonus (+10% per other friendly military unit adjacent to target)
        let flanking_units = world
            .grid
            .neighbours(target)
            .filter(|&n| n != unit.position)
            .flat_map(|n| world.units_at(n))
            .filter(|&other_id| {
                world
                    .units
                    .get(other_id)
                    .is_some_and(|o| o.owner == unit.owner && o.is_military(r))
            })
            .count() as i32;

        if flanking_units > 0 {
            stack.add(Modifier::additive_percent(
                Fixed::whole(flanking_units * BASE_FLANKING_BONUS),
                Source::new(SourceKind::Other, "Flanking"),
            ));
        }
    }

    let ctx = GameContext::new(r)
        .with_world(world)
        .for_civ(unit.owner)
        .on_tile(unit.position)
        .in_combat(if defending { CombatRole::Defending } else { CombatRole::Attacking });

    let mut check_unique = |u: &crate::Unique, name: &str| {
        if u.unique_type() == Some(UniqueType::Strength) && applies(u, &ctx) == Verdict::True {
            if let Some(percent) = u.params.first().and_then(|p| p.parse::<i32>().ok()) {
                stack.add(Modifier::additive_percent(
                    Fixed::whole(percent),
                    Source::new(SourceKind::Other, name.to_string()),
                ));
            }
        }
    };

    if let Some(b) = r.units.get(unit.base) {
        for u in &b.uniques {
            check_unique(u, &b.name);
        }
    }
    for promo_id in &unit.promotions {
        if let Some(promo) = r.promotions.get(*promo_id) {
            for u in &promo.uniques {
                check_unique(u, &promo.name);
            }
        }
    }

    // Strength never drops below a token amount, or the ratio maths degenerates.
    stack.floor_at(Fixed::ONE);
    Some((Fixed::whole(base), stack))
}

/// A unit's strength, and where each part of it came from.
pub fn strength_breakdown(
    world: &World,
    r: &Ruleset,
    unit_id: Id<Unit>,
    defending: bool,
) -> Option<(Fixed, ModifierStack)> {
    strength_breakdown_against(world, r, unit_id, None, defending)
}

/// Strength a unit attacks with against a specific target tile.
pub fn attacking_strength_against(
    world: &World,
    r: &Ruleset,
    unit: Id<Unit>,
    target_tile: Option<TileId>,
) -> Option<Fixed> {
    let (base, stack) = strength_breakdown_against(world, r, unit, target_tile, false)?;
    Some(stack.evaluate(base))
}

/// Strength a unit attacks with.
pub fn attacking_strength(world: &World, r: &Ruleset, unit: Id<Unit>) -> Option<Fixed> {
    attacking_strength_against(world, r, unit, None)
}

/// Strength a unit defends with, including terrain and fortification.
pub fn defending_strength(world: &World, r: &Ruleset, unit: Id<Unit>) -> Option<Fixed> {
    let (base, stack) = strength_breakdown(world, r, unit, true)?;
    Some(stack.evaluate(base))
}

/// How much less a wounded unit hits for.
pub fn health_ratio(health: u8) -> Fixed {
    let missing = Fixed::whole(100 - health.min(100) as i32);
    Fixed::ONE - missing.div(Fixed::whole(WOUNDED_DAMAGE_DIVISOR)).unwrap_or(Fixed::ZERO)
}

/// The ratio curve. `to_attacker` asks for the damage flowing back.
pub fn damage_modifier(ratio: Fixed, to_attacker: bool, randomness: Fixed) -> Fixed {
    damage_modifier_precise(Precise::from_fixed(ratio), to_attacker, randomness).to_fixed()
}

/// The strength ratio, kept wide.
///
/// Quantising this to thousandths is enough on its own to move a damage figure
/// by a point: 8 against 10.5 is 0.761904…, which becomes 0.762, and the `^4`
/// in the curve magnifies the difference. The ratio has to stay wide from the
/// division onward.
pub fn strength_ratio(attack: Fixed, defend: Fixed) -> Precise {
    Precise::from_fixed(attack)
        .div(Precise::from_fixed(defend))
        .unwrap_or(Precise::whole(1))
}

/// The curve, kept in [`Precise`] so the six steps round only at the end.
///
/// Rounding at each step costs about a point of damage on one matchup in forty
/// — small enough to look like noise and large enough to matter, since damage
/// is compared against health as a whole number.
pub fn damage_modifier_precise(ratio: Precise, to_attacker: bool, randomness: Fixed) -> Precise {
    let one = Precise::whole(1);
    if ratio == Precise::whole(0) {
        return Precise::whole(0);
    }
    let inverse = one.div(ratio).unwrap_or(one);
    let stronger = if ratio < one { inverse } else { ratio };

    let quarter = stronger.add(Precise::whole(3)).div(Precise::whole(4)).unwrap_or(one);
    let mut modifier = quarter
        .powi(4)
        .add(one)
        .div(Precise::whole(2))
        .unwrap_or(one);

    // The weaker party's damage is the reciprocal.
    let invert = (to_attacker && ratio > one) || (!to_attacker && ratio < one);
    if invert {
        modifier = one.div(modifier).unwrap_or(one);
    }

    let base = Precise::whole(BASE_DAMAGE_LOW)
        .add(Precise::from_fixed(randomness).mul(Precise::whole(BASE_DAMAGE_SPREAD)));
    base.mul(modifier)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CombatResult {
    pub damage_to_defender: i32,
    pub damage_to_attacker: i32,
    pub defender_died: bool,
    pub attacker_died: bool,
}

/// Seed built from the state that produced the fight, so a replay of the same
/// turn fights it the same way.
fn combat_seed(world: &World, attacker: TileId, defender: TileId) -> Rng {
    Rng::from_parts(&[world.turn as u64, attacker.0 as u64, defender.0 as u64])
}

/// Resolve one melee attack. Both sides take damage; either may die.
pub fn resolve_attack(
    world: &mut World,
    r: &Ruleset,
    attacker: Id<Unit>,
    defender: Id<Unit>,
) -> Result<CombatResult, CombatRejection> {
    let (a_owner, a_can_move, a_attacks_left, a_position, a_health) = {
        let a = world.units.get(attacker).ok_or(CombatRejection::NoSuchUnit)?;
        (a.owner, a.can_move(), a.attacks_left, a.position, a.health)
    };
    let (d_owner, d_is_civilian, d_base, d_position, d_health) = {
        let d = world.units.get(defender).ok_or(CombatRejection::NoSuchUnit)?;
        (d.owner, d.is_civilian(r), d.base, d.position, d.health)
    };
    if a_owner == d_owner {
        return Err(CombatRejection::SameOwner);
    }
    // Civs who have met and are at peace do not fight. Two who have never met
    // are treated as free to, which is how a first contact goes badly.
    if world.diplomacy.have_met(a_owner, d_owner) && !world.diplomacy.at_war(a_owner, d_owner) {
        return Err(CombatRejection::NotAtWar);
    }
    if !a_can_move || a_attacks_left == 0 {
        return Err(CombatRejection::NoMovement);
    }
    if !world.grid.neighbours(a_position).any(|t| t == d_position) {
        return Err(CombatRejection::NotAdjacent);
    }

    if d_is_civilian {
        // A captured Settler becomes a Worker, except in barbarian hands, where
        // it stays whole so it can be ransomed back (`Battle.captureCivilianUnit`).
        let barbarian_captor = world.civ(a_owner).is_some_and(|c| c.is_barbarian);
        let demoted_to = if !barbarian_captor && r.unit("Settler") == Some(d_base) { r.unit("Worker") } else { None };
        world.transfer_unit(defender, a_owner);
        if let Some(u) = world.units.get_mut(defender) {
            if let Some(worker) = demoted_to {
                u.base = worker;
            }
            u.movement_left = Fixed::ZERO;
        }
        if let Some(a_unit) = world.units.get_mut(attacker) {
            a_unit.movement_left = Fixed::ZERO;
            a_unit.attacks_left = a_unit.attacks_left.saturating_sub(1);
        }
        return Ok(CombatResult {
            damage_to_defender: 0,
            damage_to_attacker: 0,
            defender_died: false,
            attacker_died: false,
        });
    }

    let att = attacking_strength(world, r, attacker).ok_or(CombatRejection::NoSuchUnit)?;
    let def = defending_strength(world, r, defender).ok_or(CombatRejection::NoSuchUnit)?;
    let ratio = strength_ratio(att, def);

    // One roll drives both sides, as in Unciv.
    let randomness = combat_seed(world, a_position, d_position).unit_interval();

    // One rounding each, at the very end.
    let to_defender = damage_modifier_precise(ratio, false, randomness)
        .mul(Precise::from_fixed(health_ratio(a_health)));
    let to_attacker = damage_modifier_precise(ratio, true, randomness)
        .mul(Precise::from_fixed(health_ratio(d_health)));

    let (dmg_d, dmg_a) = (to_defender.round().max(0) as i32, to_attacker.round().max(0) as i32);

    let defender_died = dmg_d >= d_health as i32;
    let attacker_died = dmg_a >= a_health as i32;

    let is_barbarian_fight = world.civ(d_owner).is_some_and(|c| c.is_barbarian)
        || world.civ(a_owner).is_some_and(|c| c.is_barbarian);

    if defender_died {
        world.remove_unit(defender);
    } else if let Some(u) = world.units.get_mut(defender) {
        u.health = (u.health as i32 - dmg_d).max(1) as u8;
        if !is_barbarian_fight || u.xp < MAX_XP_FROM_BARBARIANS {
            u.xp = (u.xp + 5).min(if is_barbarian_fight { MAX_XP_FROM_BARBARIANS } else { 1000 });
        }
    }

    if attacker_died {
        world.remove_unit(attacker);
    } else if let Some(u) = world.units.get_mut(attacker) {
        u.health = (u.health as i32 - dmg_a).max(1) as u8;
        u.movement_left = Fixed::ZERO; // attacking ends the turn
        u.attacks_left = u.attacks_left.saturating_sub(1);
        u.fortified = false;
        if !is_barbarian_fight || u.xp < MAX_XP_FROM_BARBARIANS {
            u.xp = (u.xp + 5).min(if is_barbarian_fight { MAX_XP_FROM_BARBARIANS } else { 1000 });
        }
    }

    Ok(CombatResult {
        damage_to_defender: dmg_d,
        damage_to_attacker: dmg_a,
        defender_died,
        attacker_died,
    })
}

/// What an attack on a city did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CityAttackResult {
    pub damage: i32,
    pub city_health: i32,
    /// The city changed hands. A city is never destroyed — it is taken.
    pub captured: bool,
    pub attacker_died: bool,
}

/// Attack a city with a melee unit.
///
/// Cities floor at one health rather than being destroyed, and a melee unit
/// that strikes a city already at one takes it. That is why war is worth
/// fighting: killing an army achieves nothing on its own.
pub fn attack_city(
    world: &mut World,
    r: &Ruleset,
    attacker: Id<Unit>,
    city_id: Id<crate::world::city::City>,
) -> Result<CityAttackResult, CombatRejection> {
    let a = world.units.get(attacker).ok_or(CombatRejection::NoSuchUnit)?;
    let city = world.cities.get(city_id).ok_or(CombatRejection::NoSuchUnit)?;
    if a.owner == city.owner {
        return Err(CombatRejection::SameOwner);
    }
    if !world.diplomacy.at_war(a.owner, city.owner) {
        return Err(CombatRejection::NotAtWar);
    }
    if !a.can_move() || a.attacks_left == 0 {
        return Err(CombatRejection::NoMovement);
    }
    let (a_tile, centre, a_health, owner) = (a.position, city.centre, a.health, a.owner);
    if !world.grid.neighbours(a_tile).any(|t| t == centre) {
        return Err(CombatRejection::NotAdjacent);
    }

    let att = attacking_strength_against(world, r, attacker, Some(centre)).ok_or(CombatRejection::NoSuchUnit)?;
    let def = city_defense_strength(world, r, city_id);
    let ratio = strength_ratio(att, def);
    let randomness = combat_seed(world, a_tile, centre).unit_interval();

    let to_city = damage_modifier_precise(ratio, false, randomness)
        .mul(Precise::from_fixed(health_ratio(a_health)))
        .round()
        .max(0) as i32;
    let to_attacker = damage_modifier_precise(ratio, true, randomness).round().max(0) as i32;

    // A city floors at one; it is captured, not levelled.
    let city = world.cities.get_mut(city_id).ok_or(CombatRejection::NoSuchUnit)?;
    city.health = (city.health - to_city).max(1);
    let at_bay = city.health <= 1;

    let attacker_died = to_attacker >= a_health as i32;
    if attacker_died {
        world.remove_unit(attacker);
    } else if let Some(u) = world.units.get_mut(attacker) {
        u.health = (u.health as i32 - to_attacker).max(1) as u8;
        u.movement_left = Fixed::ZERO;
        u.attacks_left = u.attacks_left.saturating_sub(1);
        u.fortified = false;
        u.xp += 5;
    }

    // Taking it needs a surviving attacker and a city already at bay.
    let captured = at_bay && !attacker_died;
    if captured {
        capture_city(world, r, city_id, owner);
    }

    Ok(CityAttackResult {
        damage: to_city,
        city_health: world.cities.get(city_id).map_or(0, |c| c.health),
        captured,
        attacker_died,
    })
}

/// Hand a city to its conqueror.
///
/// Population is halved, as Civ does — a sacked city is not the city it was.
/// Buildings survive here; Unciv destroys some of them on capture, which
/// needs a per-building rule this crate does not carry yet.
pub fn capture_city(
    world: &mut World,
    r: &Ruleset,
    city_id: Id<crate::world::city::City>,
    new_owner: crate::world::grid::CivId,
) {
    let Some(city) = world.cities.get_mut(city_id) else { return };
    let old_owner = city.owner;
    city.owner = new_owner;
    city.population = (city.population / 2).max(1);
    city.health = crate::world::city::max_health(r, city) / 2;
    city.worked.clear();
    city.queue.clear();
    city.production_stored = 0;
    // And the granary with it. Halving the population while keeping the food
    // banked for the larger city means a captured city grows back on the turn
    // after it falls, which is the opposite of what sacking one should do.
    city.food_stored = 0;
    let centre = city.centre;

    if let Some(old) = world.civ_mut(old_owner) {
        old.cities.retain(|c| *c != city_id);
    }
    if let Some(new) = world.civ_mut(new_owner) {
        new.cities.push(city_id);
    }
    // The land around it changes hands with it.
    for t in world.grid.within(centre, 1) {
        if world.grid.owner[t.0 as usize] == Some(old_owner) {
            world.grid.owner[t.0 as usize] = Some(new_owner);
        }
    }
}

/// Breakdown of ranged strength for a unit.
pub fn ranged_strength_breakdown(
    world: &World,
    r: &Ruleset,
    unit_id: Id<Unit>,
) -> Option<(Fixed, ModifierStack)> {
    let unit = world.units.get(unit_id)?;
    let base = r.units.get(unit.base)?.ranged_strength.unwrap_or(0);
    if base == 0 {
        return None;
    }
    let mut stack = ModifierStack::new();

    if world
        .civ(unit.owner)
        .is_some_and(|c| crate::world::resources::is_missing_resources(r, c, unit.base))
    {
        stack.add(Modifier::additive_percent(
            Fixed::whole(crate::world::resources::MISSING_RESOURCE_MALUS),
            Source::new(SourceKind::Other, "Missing resource"),
        ));
    }

    let ctx = GameContext::new(r)
        .with_world(world)
        .for_civ(unit.owner)
        .on_tile(unit.position)
        .in_combat(CombatRole::Attacking);

    let mut check_unique = |u: &crate::Unique, name: &str| {
        if u.unique_type() == Some(UniqueType::Strength) && applies(u, &ctx) == Verdict::True {
            if let Some(percent) = u.params.first().and_then(|p| p.parse::<i32>().ok()) {
                stack.add(Modifier::additive_percent(
                    Fixed::whole(percent),
                    Source::new(SourceKind::Other, name.to_string()),
                ));
            }
        }
    };

    if let Some(b) = r.units.get(unit.base) {
        for u in &b.uniques {
            check_unique(u, &b.name);
        }
    }
    for promo_id in &unit.promotions {
        if let Some(promo) = r.promotions.get(*promo_id) {
            for u in &promo.uniques {
                check_unique(u, &promo.name);
            }
        }
    }

    stack.floor_at(Fixed::ONE);
    Some((Fixed::whole(base), stack))
}

/// Evaluated ranged strength of a unit.
pub fn ranged_strength(world: &World, r: &Ruleset, unit: Id<Unit>) -> Option<Fixed> {
    let (base, stack) = ranged_strength_breakdown(world, r, unit)?;
    Some(stack.evaluate(base))
}

/// Line of sight check between `from` and `to`.
pub fn has_line_of_sight(
    grid: &TileGrid,
    r: &Ruleset,
    from: TileId,
    to: TileId,
    indirect_fire: bool,
) -> bool {
    if indirect_fire {
        return true;
    }
    let dist = grid.distance(from, to);
    if dist <= 1 {
        return true;
    }
    let shooter_on_hill = grid.terrains(from).any(|t| {
        r.terrains.get(t).is_some_and(|terrain| terrain.r#type == "Hill" || terrain.name == "Hill")
    });

    for n in grid.neighbours(from) {
        if grid.distance(n, to) < dist {
            let blocks = grid.terrains(n).any(|t| {
                r.terrains.get(t).is_some_and(|terrain| {
                    terrain.impassable == Some(true)
                        || (!shooter_on_hill && (terrain.name == "Hill" || terrain.name == "Forest" || terrain.name == "Jungle"))
                })
            });
            if blocks {
                return false;
            }
        }
    }
    true
}

/// Execute a ranged attack on an enemy unit at `target_tile`.
pub fn resolve_ranged_attack(
    world: &mut World,
    r: &Ruleset,
    attacker: Id<Unit>,
    target_tile: TileId,
) -> Result<CombatResult, CombatRejection> {
    let a = world.units.get(attacker).ok_or(CombatRejection::NoSuchUnit)?;
    if !a.can_move() || a.attacks_left == 0 {
        return Err(CombatRejection::NoMovement);
    }
    let base = r.units.get(a.base).ok_or(CombatRejection::NoSuchUnit)?;
    let range = base.range.unwrap_or(2);
    if base.ranged_strength.unwrap_or(0) == 0 {
        return Err(CombatRejection::NoRangedAttack);
    }
    let dist = world.grid.distance(a.position, target_tile) as i32;
    if dist > range {
        return Err(CombatRejection::OutOfRange);
    }
    if !has_line_of_sight(&world.grid, r, a.position, target_tile, a.has_unique(r, UniqueType::IndirectFire)) {
        return Err(CombatRejection::TargetBlockedByLOS);
    }

    let defender_id = world.units.iter().find(|(_, u)| u.position == target_tile && u.owner != a.owner).map(|(id, _)| id);
    let Some(defender) = defender_id else {
        return Err(CombatRejection::NoSuchUnit);
    };

    let d = world.units.get(defender).ok_or(CombatRejection::NoSuchUnit)?;
    if world.diplomacy.have_met(a.owner, d.owner) && !world.diplomacy.at_war(a.owner, d.owner) {
        return Err(CombatRejection::NotAtWar);
    }

    let att = ranged_strength(world, r, attacker).ok_or(CombatRejection::NoSuchUnit)?;
    let def = defending_strength(world, r, defender).ok_or(CombatRejection::NoSuchUnit)?;
    let ratio = strength_ratio(att, def);

    let randomness = combat_seed(world, a.position, target_tile).unit_interval();
    let to_defender = damage_modifier_precise(ratio, false, randomness)
        .mul(Precise::from_fixed(health_ratio(a.health)))
        .round()
        .max(0) as i32;

    let defender_died = to_defender >= d.health as i32;
    let d_owner = d.owner;
    let is_barbarian = world.civ(d_owner).is_some_and(|c| c.is_barbarian);

    if defender_died {
        world.remove_unit(defender);
    } else if let Some(u) = world.units.get_mut(defender) {
        u.health = (u.health as i32 - to_defender).max(1) as u8;
        if !is_barbarian || u.xp < MAX_XP_FROM_BARBARIANS {
            u.xp = (u.xp + 4).min(if is_barbarian { MAX_XP_FROM_BARBARIANS } else { 1000 });
        }
    }

    if let Some(u) = world.units.get_mut(attacker) {
        u.movement_left = Fixed::ZERO;
        u.attacks_left = u.attacks_left.saturating_sub(1);
        u.fortified = false;
        if !is_barbarian || u.xp < MAX_XP_FROM_BARBARIANS {
            u.xp = (u.xp + 2).min(if is_barbarian { MAX_XP_FROM_BARBARIANS } else { 1000 });
        }
    }

    Ok(CombatResult {
        damage_to_defender: to_defender,
        damage_to_attacker: 0,
        defender_died,
        attacker_died: false,
    })
}

/// Execute a ranged attack on a city.
pub fn attack_city_ranged(
    world: &mut World,
    r: &Ruleset,
    attacker: Id<Unit>,
    city_id: Id<crate::world::city::City>,
) -> Result<CityAttackResult, CombatRejection> {
    let a = world.units.get(attacker).ok_or(CombatRejection::NoSuchUnit)?;
    let city = world.cities.get(city_id).ok_or(CombatRejection::NoSuchUnit)?;
    if a.owner == city.owner {
        return Err(CombatRejection::SameOwner);
    }
    if !world.diplomacy.at_war(a.owner, city.owner) {
        return Err(CombatRejection::NotAtWar);
    }
    if !a.can_move() || a.attacks_left == 0 {
        return Err(CombatRejection::NoMovement);
    }
    let base = r.units.get(a.base).ok_or(CombatRejection::NoSuchUnit)?;
    let range = base.range.unwrap_or(2);
    if base.ranged_strength.unwrap_or(0) == 0 {
        return Err(CombatRejection::NoRangedAttack);
    }
    let dist = world.grid.distance(a.position, city.centre) as i32;
    if dist > range {
        return Err(CombatRejection::OutOfRange);
    }
    if !has_line_of_sight(&world.grid, r, a.position, city.centre, a.has_unique(r, UniqueType::IndirectFire)) {
        return Err(CombatRejection::TargetBlockedByLOS);
    }

    let att = ranged_strength(world, r, attacker).ok_or(CombatRejection::NoSuchUnit)?;
    let def = city_defense_strength(world, r, city_id);
    let ratio = strength_ratio(att, def);
    let randomness = combat_seed(world, a.position, city.centre).unit_interval();

    let to_city = damage_modifier_precise(ratio, false, randomness)
        .mul(Precise::from_fixed(health_ratio(a.health)))
        .round()
        .max(0) as i32;

    let city = world.cities.get_mut(city_id).ok_or(CombatRejection::NoSuchUnit)?;
    city.health = (city.health - to_city).max(1);

    if let Some(u) = world.units.get_mut(attacker) {
        u.movement_left = Fixed::ZERO;
        u.attacks_left = u.attacks_left.saturating_sub(1);
        u.fortified = false;
        u.xp += 2;
    }

    Ok(CityAttackResult {
        damage: to_city,
        city_health: world.cities.get(city_id).map_or(0, |c| c.health),
        captured: false,
        attacker_died: false,
    })
}

/// Effective defensive combat strength of a city including garrison bonuses.
pub fn city_defense_strength(
    world: &World,
    r: &Ruleset,
    city_id: Id<crate::world::city::City>,
) -> Fixed {
    let Some(city) = world.cities.get(city_id) else { return Fixed::ONE };
    let base = crate::world::city::city_strength(r, city);
    let has_garrison = world
        .units
        .iter()
        .any(|(_, u)| u.position == city.centre && u.owner == city.owner && u.is_military(r));
    if has_garrison {
        base.mul(Fixed::from_fraction(6, 5))
    } else {
        base
    }
}

/// A city bombards an enemy unit within range (range 2).
pub fn city_bombard(
    world: &mut World,
    r: &Ruleset,
    city_id: Id<crate::world::city::City>,
    target_unit_id: Id<Unit>,
) -> Result<CombatResult, CombatRejection> {
    let city = world.cities.get(city_id).ok_or(CombatRejection::NoSuchUnit)?;
    let target = world.units.get(target_unit_id).ok_or(CombatRejection::NoSuchUnit)?;
    if city.owner == target.owner {
        return Err(CombatRejection::SameOwner);
    }
    if !world.diplomacy.at_war(city.owner, target.owner) {
        return Err(CombatRejection::NotAtWar);
    }
    let dist = world.grid.distance(city.centre, target.position);
    if dist > 2 {
        return Err(CombatRejection::OutOfRange);
    }
    if !has_line_of_sight(&world.grid, r, city.centre, target.position, false) {
        return Err(CombatRejection::TargetBlockedByLOS);
    }

    let att = city_defense_strength(world, r, city_id);
    let def = defending_strength(world, r, target_unit_id).ok_or(CombatRejection::NoSuchUnit)?;
    let ratio = strength_ratio(att, def);
    let randomness = combat_seed(world, city.centre, target.position).unit_interval();

    let to_target = damage_modifier_precise(ratio, false, randomness)
        .round()
        .max(0) as i32;

    let target_died = to_target >= target.health as i32;
    if target_died {
        world.remove_unit(target_unit_id);
    } else if let Some(u) = world.units.get_mut(target_unit_id) {
        u.health = (u.health as i32 - to_target).max(1) as u8;
    }

    Ok(CombatResult {
        damage_to_defender: to_target,
        damage_to_attacker: 0,
        defender_died: target_died,
        attacker_died: false,
    })
}

/// Result of deploying a nuclear weapon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NuclearStrikeResult {
    pub tiles_affected: Vec<TileId>,
    pub units_destroyed: Vec<Id<Unit>>,
    pub cities_damaged: Vec<Id<crate::world::city::City>>,
}

/// Rebase an air unit to another friendly city or carrier.
pub fn rebase_air_unit(
    world: &mut World,
    r: &Ruleset,
    unit_id: Id<Unit>,
    target_tile: TileId,
) -> Result<(), CombatRejection> {
    let unit = world.units.get(unit_id).ok_or(CombatRejection::NoSuchUnit)?;
    if !unit.can_move() {
        return Err(CombatRejection::NoMovement);
    }
    if !unit.is_air(r) {
        return Err(CombatRejection::NoMovement);
    }
    let base = r.units.get(unit.base).ok_or(CombatRejection::NoSuchUnit)?;
    let rebase_range = base.range.unwrap_or(8) * 2;
    if world.grid.distance(unit.position, target_tile) as i32 > rebase_range {
        return Err(CombatRejection::OutOfRange);
    }
    let is_friendly_city = world.cities.iter().any(|(_, c)| c.centre == target_tile && c.owner == unit.owner);
    let is_friendly_carrier = world.units.iter().any(|(_, u)| {
        u.position == target_tile && u.owner == unit.owner && r.units.get(u.base).is_some_and(|b| b.unit_type == "Aircraft Carrier")
    });
    if !is_friendly_city && !is_friendly_carrier {
        return Err(CombatRejection::NotAdjacent);
    }
    if let Some(u) = world.units.get_mut(unit_id) {
        u.position = target_tile;
        u.movement_left = Fixed::ZERO;
        u.attacks_left = 0;
    }
    Ok(())
}

/// Execute an air strike against a unit or city.
pub fn air_strike(
    world: &mut World,
    r: &Ruleset,
    attacker_id: Id<Unit>,
    target_tile: TileId,
) -> Result<CombatResult, CombatRejection> {
    let (a_owner, a_can_move, a_attacks_left, a_is_air, a_base, a_position, a_health) = {
        let a = world.units.get(attacker_id).ok_or(CombatRejection::NoSuchUnit)?;
        (a.owner, a.can_move(), a.attacks_left, a.is_air(r), a.base, a.position, a.health)
    };
    if !a_can_move || a_attacks_left == 0 {
        return Err(CombatRejection::NoMovement);
    }
    if !a_is_air {
        return Err(CombatRejection::NoMovement);
    }
    let base = r.units.get(a_base).ok_or(CombatRejection::NoSuchUnit)?;
    let range = base.range.unwrap_or(8);
    if world.grid.distance(a_position, target_tile) as i32 > range {
        return Err(CombatRejection::OutOfRange);
    }

    // Interception check: find enemy fighter in range that can intercept
    let mut interception_damage = 0;
    let mut interceptor_id = None;
    for (enemy_id, enemy_u) in world.units.iter() {
        if enemy_u.owner != a_owner && enemy_u.attacks_left > 0 && world.diplomacy.at_war(a_owner, enemy_u.owner) {
            if let Some(enemy_base) = r.units.get(enemy_u.base) {
                let int_range = enemy_base.intercept_range.unwrap_or(enemy_base.range.unwrap_or(0));
                if int_range > 0 && world.grid.distance(enemy_u.position, target_tile) as i32 <= int_range {
                    let ratio = strength_ratio(Fixed::whole(enemy_base.strength.unwrap_or(45)), Fixed::whole(base.strength.unwrap_or(45)));
                    let roll = combat_seed(world, enemy_u.position, a_position).unit_interval();
                    interception_damage = damage_modifier_precise(ratio, false, roll).round().max(0) as i32;
                    interceptor_id = Some(enemy_id);
                    break;
                }
            }
        }
    }
    if let Some(id) = interceptor_id {
        if let Some(u) = world.units.get_mut(id) {
            u.attacks_left = u.attacks_left.saturating_sub(1);
        }
    }

    let attacker_died = interception_damage >= a_health as i32;
    if attacker_died {
        world.remove_unit(attacker_id);
        return Ok(CombatResult {
            damage_to_defender: 0,
            damage_to_attacker: interception_damage,
            defender_died: false,
            attacker_died: true,
        });
    }

    let target_city = world.cities.iter().find(|(_, c)| c.centre == target_tile && c.owner != a_owner).map(|(id, _)| id);
    let target_unit = world.units.iter().find(|(_, u)| u.position == target_tile && u.owner != a_owner).map(|(id, _)| id);

    let (dmg_def, def_died) = if let Some(city_id) = target_city {
        let city = world.cities.get(city_id).ok_or(CombatRejection::NoSuchUnit)?;
        if !world.diplomacy.at_war(a_owner, city.owner) {
            return Err(CombatRejection::NotAtWar);
        }
        let att = Fixed::whole(base.ranged_strength.or(base.strength).unwrap_or(45));
        let def = city_defense_strength(world, r, city_id);
        let ratio = strength_ratio(att, def);
        let roll = combat_seed(world, a_position, target_tile).unit_interval();
        let dmg = damage_modifier_precise(ratio, false, roll)
            .mul(Precise::from_fixed(health_ratio(a_health)))
            .round().max(0) as i32;
        if let Some(c) = world.cities.get_mut(city_id) {
            c.health = (c.health - dmg).max(1);
        }
        (dmg, false)
    } else if let Some(def_id) = target_unit {
        let d = world.units.get(def_id).ok_or(CombatRejection::NoSuchUnit)?;
        if world.diplomacy.have_met(a_owner, d.owner) && !world.diplomacy.at_war(a_owner, d.owner) {
            return Err(CombatRejection::NotAtWar);
        }
        let att = Fixed::whole(base.ranged_strength.or(base.strength).unwrap_or(45));
        let def = defending_strength(world, r, def_id).ok_or(CombatRejection::NoSuchUnit)?;
        let ratio = strength_ratio(att, def);
        let roll = combat_seed(world, a_position, target_tile).unit_interval();
        let dmg = damage_modifier_precise(ratio, false, roll)
            .mul(Precise::from_fixed(health_ratio(a_health)))
            .round().max(0) as i32;
        let died = dmg >= d.health as i32;
        if died {
            world.remove_unit(def_id);
        } else if let Some(u) = world.units.get_mut(def_id) {
            u.health = (u.health as i32 - dmg).max(1) as u8;
        }
        (dmg, died)
    } else {
        return Err(CombatRejection::NoSuchUnit);
    };

    if let Some(u) = world.units.get_mut(attacker_id) {
        u.health = (u.health as i32 - interception_damage).max(1) as u8;
        u.movement_left = Fixed::ZERO;
        u.attacks_left = u.attacks_left.saturating_sub(1);
        u.xp += 2;
    }

    Ok(CombatResult {
        damage_to_defender: dmg_def,
        damage_to_attacker: interception_damage,
        defender_died: def_died,
        attacker_died: false,
    })
}

/// Deploy a nuclear weapon at `target_tile`.
pub fn nuclear_strike(
    world: &mut World,
    r: &Ruleset,
    nuke_unit_id: Id<Unit>,
    target_tile: TileId,
) -> Result<NuclearStrikeResult, CombatRejection> {
    let unit = world.units.get(nuke_unit_id).ok_or(CombatRejection::NoSuchUnit)?;
    if !unit.can_move() {
        return Err(CombatRejection::NoMovement);
    }
    let base = r.units.get(unit.base).ok_or(CombatRejection::NoSuchUnit)?;
    let range = base.range.unwrap_or(10);
    if world.grid.distance(unit.position, target_tile) as i32 > range {
        return Err(CombatRejection::OutOfRange);
    }

    let blast_radius = base.uniques.iter().find_map(|u| {
        if u.text.starts_with("Blast radius [") {
            u.params.first().and_then(|p| p.parse::<u32>().ok())
        } else {
            None
        }
    }).unwrap_or(2);

    let fallout_terrain = r.terrain("Fallout");

    let mut tiles_affected = Vec::new();
    let mut units_destroyed = Vec::new();
    let mut cities_damaged = Vec::new();

    let candidates: Vec<TileId> = world.grid.ids().filter(|&t| world.grid.distance(target_tile, t) <= blast_radius).collect();

    for t in candidates {
        tiles_affected.push(t);
        let tile_units: Vec<Id<Unit>> = world.units.iter().filter(|(id, u)| *id != nuke_unit_id && u.position == t).map(|(id, _)| id).collect();
        for u_id in tile_units {
            units_destroyed.push(u_id);
            world.remove_unit(u_id);
        }

        let target_city = world.cities.iter().find(|(_, c)| c.centre == t).map(|(id, _)| id);
        if let Some(city_id) = target_city {
            cities_damaged.push(city_id);
            if let Some(c) = world.cities.get_mut(city_id) {
                c.population = (c.population / 2).max(1);
                c.health = (c.health / 2).max(1);
            }
        }

        if let Some(f_id) = fallout_terrain {
            let features = &mut world.grid.features[t.0 as usize];
            if !features.contains(&f_id) {
                features.push(f_id);
            }
        }
        if world.grid.improvement[t.0 as usize].is_some() {
            world.grid.pillage_improvement(t);
        }
        if world.grid.road[t.0 as usize] != RoadLevel::None {
            world.grid.pillage_road(t);
        }
    }

    world.remove_unit(nuke_unit_id);

    Ok(NuclearStrikeResult {
        tiles_affected,
        units_destroyed,
        cities_damaged,
    })
}

/// Experience required to reach the given promotion level.
/// Level 1: 10 XP, Level 2: 30 XP, Level 3: 60 XP, Level 4: 100 XP.
pub fn xp_for_level(level: usize) -> u16 {
    (10 * level * (level + 1) / 2) as u16
}

/// Whether the unit has earned enough experience to select a new promotion.
pub fn can_promote(unit: &Unit) -> bool {
    let next_level = unit.promotions.len() + 1;
    unit.xp >= xp_for_level(next_level)
}

/// List of promotions available to this unit given its type, XP, and prerequisites.
pub fn available_promotions(r: &Ruleset, unit: &Unit) -> Vec<Id<Promotion>> {
    let Some(base) = r.units.get(unit.base) else { return Vec::new() };
    r.promotions
        .iter()
        .filter_map(|(id, promo)| {
            if unit.promotions.contains(&id) {
                return None;
            }
            if !promo.unit_types.is_empty() && !promo.unit_types.iter().any(|ut| ut == &base.unit_type) {
                return None;
            }
            let prereqs_met = promo.prerequisites.iter().all(|prereq_name| {
                r.promotions
                    .id(prereq_name)
                    .is_some_and(|req_id| unit.promotions.contains(&req_id))
            });
            if !prereqs_met {
                return None;
            }
            Some(id)
        })
        .collect()
}

/// Choose and apply a promotion to a unit.
pub fn choose_promotion(
    unit: &mut Unit,
    r: &Ruleset,
    promo_id: Id<Promotion>,
) -> Result<(), &'static str> {
    if !can_promote(unit) {
        return Err("Not enough XP for promotion");
    }
    let available = available_promotions(r, unit);
    if !available.contains(&promo_id) {
        return Err("Promotion not available");
    }
    unit.promotions.insert(promo_id);
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PillageEvent {
    pub unit: Id<Unit>,
    pub tile: TileId,
    pub healed: u8,
    pub gold_pillaged: i32,
}

/// Whether the unit can pillage the tile it currently stands on.
pub fn can_pillage(world: &World, unit_id: Id<Unit>) -> bool {
    let Some(unit) = world.units.get(unit_id) else { return false };
    if !unit.can_move() { return false; }
    let pos = unit.position;
    world.grid.unpillaged_improvement(pos).is_some() || (world.grid.unpillaged_road(pos) != RoadLevel::None)
}

/// Pillage the tile the unit stands on, restoring HP and pillaging gold.
pub fn pillage(
    world: &mut World,
    _r: &Ruleset,
    unit_id: Id<Unit>,
) -> Option<PillageEvent> {
    let unit = world.units.get(unit_id)?;
    if !unit.can_move() { return None; }
    let pos = unit.position;
    let owner = unit.owner;

    let mut pillaged = false;
    if world.grid.unpillaged_improvement(pos).is_some() {
        world.grid.pillage_improvement(pos);
        pillaged = true;
    } else if world.grid.unpillaged_road(pos) != RoadLevel::None {
        world.grid.pillage_road(pos);
        pillaged = true;
    }

    if !pillaged {
        return None;
    }

    let u = world.units.get_mut(unit_id)?;
    u.movement_left = (u.movement_left - Fixed::ONE).max(Fixed::ZERO);
    let old_health = u.health;
    u.health = (u.health + 25).min(100);
    let healed = u.health - old_health;

    let gold_pillaged = 20;
    if let Some(c) = world.civ_mut(owner) {
        c.gold += gold_pillaged;
    }

    Some(PillageEvent {
        unit: unit_id,
        tile: pos,
        healed,
        gold_pillaged,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpgradeRejection {
    NoSuchUnit,
    CannotUpgrade,
    TechNotResearched,
    MissingResources,
    NotInFriendlyTerritory,
    NotEnoughGold,
}

/// Gold cost to upgrade from `current` to `target`.
pub fn upgrade_cost(r: &Ruleset, current: Id<BaseUnit>, target: Id<BaseUnit>) -> Option<i32> {
    let old_cost = r.units.get(current)?.cost.unwrap_or(0);
    let new_cost = r.units.get(target)?.cost.unwrap_or(0);
    Some((new_cost - old_cost).max(0) * 2 + 10)
}

/// Check if a unit is eligible for an upgrade.
pub fn can_upgrade(
    world: &World,
    r: &Ruleset,
    unit_id: Id<Unit>,
) -> Result<(Id<BaseUnit>, i32), UpgradeRejection> {
    let unit = world.units.get(unit_id).ok_or(UpgradeRejection::NoSuchUnit)?;
    let base = r.units.get(unit.base).ok_or(UpgradeRejection::NoSuchUnit)?;
    let target_name = base.upgrades_to.as_ref().ok_or(UpgradeRejection::CannotUpgrade)?;
    let target_id = r.unit(target_name).ok_or(UpgradeRejection::CannotUpgrade)?;
    let target_base = r.units.get(target_id).ok_or(UpgradeRejection::CannotUpgrade)?;

    let civ = world.civ(unit.owner).ok_or(UpgradeRejection::NoSuchUnit)?;

    if let Some(ref tech_name) = target_base.required_tech {
        let tech_id = r.technologies.id(tech_name).ok_or(UpgradeRejection::TechNotResearched)?;
        if !civ.has_tech(tech_id) {
            return Err(UpgradeRejection::TechNotResearched);
        }
    }

    let in_friendly = world.grid.owner.get(unit.position.0 as usize).copied().flatten() == Some(unit.owner)
        || world.cities.iter().any(|(_, c)| c.centre == unit.position && c.owner == unit.owner);
    if !in_friendly {
        return Err(UpgradeRejection::NotInFriendlyTerritory);
    }

    let cost = upgrade_cost(r, unit.base, target_id).unwrap_or(10);
    if civ.gold < cost {
        return Err(UpgradeRejection::NotEnoughGold);
    }

    Ok((target_id, cost))
}

/// Upgrade a unit in-place to its modern equivalent.
pub fn upgrade_unit(
    world: &mut World,
    r: &Ruleset,
    unit_id: Id<Unit>,
) -> Result<Id<BaseUnit>, UpgradeRejection> {
    let (target_id, cost) = can_upgrade(world, r, unit_id)?;
    let unit = world.units.get(unit_id).ok_or(UpgradeRejection::NoSuchUnit)?;
    let owner = unit.owner;

    if let Some(c) = world.civ_mut(owner) {
        c.gold -= cost;
    }
    if let Some(u) = world.units.get_mut(unit_id) {
        u.base = target_id;
        u.movement_left = Fixed::ZERO;
    }
    Ok(target_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::entities::Civilization;
    use crate::world::grid::{CivId, TileGrid};

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings")
            .expect("readable")
            .expect("loads")
    }

    fn world(r: &Ruleset) -> World {
        let g = TileGrid::new(12, 12, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        World::new(
            g,
            vec![
                Civilization::new(CivId(0), nation, false),
                Civilization::new(CivId(1), nation, true),
            ],
        )
    }

    fn unit(w: &mut World, r: &Ruleset, name: &str, civ: CivId, at: TileId) -> Id<Unit> {
        let base = r.unit(name).unwrap_or_else(|| panic!("{name} exists"));
        let movement = r.units.get(base).unwrap().movement;
        w.place_unit(Unit::new(base, civ, at, movement))
    }

    /// Evenly matched units trade about thirty damage. This is the anchor the
    /// rest of the curve hangs from.
    #[test]
    fn equal_strength_trades_about_thirty() {
        for roll in [Fixed::ZERO, Fixed::permille(500), Fixed::permille(999)] {
            let d = damage_modifier(Fixed::ONE, false, roll);
            assert!(
                d >= Fixed::whole(24) && d <= Fixed::whole(36),
                "even fight rolled {d}, expected 24..36"
            );
        }
        assert_eq!(damage_modifier(Fixed::ONE, false, Fixed::permille(500)), Fixed::whole(30));
    }

    #[test]
    fn strength_advantage_is_worth_more_than_it_looks() {
        let mid = Fixed::permille(500);
        let dealt = damage_modifier(Fixed::whole(2), false, mid);
        let taken = damage_modifier(Fixed::whole(2), true, mid);
        println!("twice as strong: deals {dealt}, takes {taken}");
        // ((2+3)/4)^4 = 2.4414; (2.4414+1)/2 = 1.7207 → about 51.6 dealt, 17.4 taken.
        assert!(dealt > Fixed::whole(51) && dealt < Fixed::whole(52));
        assert!(taken > Fixed::whole(17) && taken < Fixed::whole(18));
        assert!(dealt.div(taken).unwrap() > Fixed::whole(2), "the curve is steeper than linear");
    }

    #[test]
    fn the_curve_is_symmetric_about_an_even_fight() {
        let mid = Fixed::permille(500);
        let strong_deals = damage_modifier(Fixed::whole(2), false, mid);
        let weak_takes = damage_modifier(Fixed::permille(500), true, mid);
        assert_eq!(strong_deals, weak_takes, "being twice as strong is the mirror of half");
    }

    #[test]
    fn wounded_units_hit_softer() {
        assert_eq!(health_ratio(100), Fixed::ONE);
        // 40 health is 60 missing, so 20% less damage.
        assert_eq!(health_ratio(40), Fixed::permille(800));
        assert_eq!(health_ratio(1), Fixed::permille(670));
        assert!(health_ratio(50) < health_ratio(80));
    }

    #[test]
    fn terrain_and_fortification_add_to_defence() {
        let r = ruleset();
        let mut w = world(&r);
        let flat = w.grid.id_at(5, 5).unwrap();
        let id = unit(&mut w, &r, "Warrior", CivId(0), flat);

        let open = defending_strength(&w, &r, id).unwrap();
        assert_eq!(open, attacking_strength(&w, &r, id).unwrap(), "no bonus in the open");

        // Hills give 25%.
        w.grid.features[flat.0 as usize].push(r.terrain("Hill").unwrap());
        let on_hill = defending_strength(&w, &r, id).unwrap();
        assert_eq!(on_hill, open.mul(Fixed::permille(1_250)));

        // Fortifying adds another 20, additively — 45% total, not 50%.
        w.units.get_mut(id).unwrap().fortified = true;
        let dug_in = defending_strength(&w, &r, id).unwrap();
        assert_eq!(dug_in, open.mul(Fixed::permille(1_450)), "additive, not compounding");
    }

    #[test]
    fn a_defence_penalty_works_the_other_way() {
        let r = ruleset();
        let mut w = world(&r);
        let tile = w.grid.id_at(5, 5).unwrap();
        // Marsh is -0.15.
        w.grid.features[tile.0 as usize].push(r.terrain("Marsh").unwrap());
        let id = unit(&mut w, &r, "Warrior", CivId(0), tile);
        let base = attacking_strength(&w, &r, id).unwrap();
        assert_eq!(defending_strength(&w, &r, id).unwrap(), base.mul(Fixed::permille(850)));
    }

    #[test]
    fn an_attack_hurts_both_sides_and_ends_the_attacker_turn() {
        let r = ruleset();
        let mut w = world(&r);
        let a_tile = w.grid.id_at(5, 5).unwrap();
        let d_tile = w.grid.neighbours(a_tile).next().unwrap();
        let a = unit(&mut w, &r, "Warrior", CivId(0), a_tile);
        let d = unit(&mut w, &r, "Warrior", CivId(1), d_tile);

        let result = resolve_attack(&mut w, &r, a, d).expect("a legal attack");
        println!("{result:?}");
        assert!(result.damage_to_defender > 0 && result.damage_to_attacker > 0);
        assert!(!result.attacker_died);
        assert!(w.units.get(d).unwrap().health < 100);
        assert_eq!(w.units.get(a).unwrap().movement_left, Fixed::ZERO, "attacking ends the turn");
    }

    #[test]
    fn attacks_are_refused_for_the_obvious_reasons() {
        let r = ruleset();
        let mut w = world(&r);
        let a_tile = w.grid.id_at(5, 5).unwrap();
        let beside = w.grid.neighbours(a_tile).next().unwrap();
        let corner = w.grid.id_at(0, 0).unwrap();
        let a = unit(&mut w, &r, "Warrior", CivId(0), a_tile);
        let friend = unit(&mut w, &r, "Warrior", CivId(0), beside);
        let far = unit(&mut w, &r, "Warrior", CivId(1), corner);

        assert_eq!(resolve_attack(&mut w, &r, a, friend), Err(CombatRejection::SameOwner));
        assert_eq!(resolve_attack(&mut w, &r, a, far), Err(CombatRejection::NotAdjacent));

        w.units.get_mut(a).unwrap().movement_left = Fixed::ZERO;
        let enemy = unit(&mut w, &r, "Warrior", CivId(1), beside);
        assert_eq!(resolve_attack(&mut w, &r, a, enemy), Err(CombatRejection::NoMovement));
    }

    #[test]
    fn a_losing_unit_dies_and_its_id_stops_resolving() {
        let r = ruleset();
        let mut w = world(&r);
        let a_tile = w.grid.id_at(5, 5).unwrap();
        let d_tile = w.grid.neighbours(a_tile).next().unwrap();
        let a = unit(&mut w, &r, "Warrior", CivId(0), a_tile);
        let d = unit(&mut w, &r, "Warrior", CivId(1), d_tile);
        w.units.get_mut(d).unwrap().health = 5;

        let result = resolve_attack(&mut w, &r, a, d).expect("legal");
        assert!(result.defender_died);
        assert!(w.units.get(d).is_none());
        assert!(w.civ(CivId(1)).unwrap().units.is_empty(), "and it is unregistered");
    }

    /// Same turn, same tiles, same outcome — the property that lets a client
    /// replay a battle without being told the result.
    #[test]
    fn a_battle_replays_identically() {
        let r = ruleset();
        let fight = || {
            let mut w = world(&r);
            let a_tile = w.grid.id_at(5, 5).unwrap();
            let d_tile = w.grid.neighbours(a_tile).next().unwrap();
            let a = unit(&mut w, &r, "Warrior", CivId(0), a_tile);
            let d = unit(&mut w, &r, "Warrior", CivId(1), d_tile);
            resolve_attack(&mut w, &r, a, d).unwrap()
        };
        assert_eq!(fight(), fight());
    }
}

#[cfg(test)]
mod peace_tests {
    use super::*;
    use crate::world::entities::Civilization;
    use crate::world::grid::{CivId, TileGrid};

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings")
            .expect("readable")
            .expect("loads")
    }

    /// Meeting someone makes peace the default, and peace forbids attacking.
    /// Two civs who have never met may still fight — which is how a first
    /// contact goes badly.
    #[test]
    fn peace_forbids_an_attack_and_war_permits_it() {
        let r = ruleset();
        let g = TileGrid::new(8, 8, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        let mut w = World::new(
            g,
            vec![
                Civilization::new(CivId(0), nation, false),
                Civilization::new(CivId(1), nation, false),
            ],
        );
        let base = r.unit("Warrior").unwrap();
        let movement = r.units.get(base).unwrap().movement;
        let here = w.grid.id_at(4, 4).unwrap();
        let there = w.grid.neighbours(here).next().unwrap();
        let a = w.place_unit(Unit::new(base, CivId(0), here, movement));
        let d = w.place_unit(Unit::new(base, CivId(1), there, movement));

        // Strangers: nothing stops them.
        assert!(resolve_attack(&mut w, &r, a, d).is_ok());

        // Once acquainted and at peace, they will not.
        w.units.get_mut(a).unwrap().refresh(movement);
        w.diplomacy.meet(CivId(0), CivId(1));
        assert_eq!(resolve_attack(&mut w, &r, a, d), Err(CombatRejection::NotAtWar));

        // War re-permits it, from either side's declaration.
        w.diplomacy.declare_war(CivId(1), CivId(0));
        assert!(resolve_attack(&mut w, &r, a, d).is_ok());
    }
}

#[cfg(test)]
mod siege {
    use super::*;
    use crate::world::city::{max_health, City};
    use crate::world::entities::Civilization;
    use crate::world::grid::{CivId, TileGrid};

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings")
            .expect("readable")
            .expect("loads")
    }

    fn besieged(r: &Ruleset) -> (World, Id<Unit>, Id<City>) {
        let g = TileGrid::new(12, 12, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        let mut w = World::new(
            g,
            vec![
                Civilization::new(CivId(0), nation, true),
                Civilization::new(CivId(1), nation, true),
            ],
        );
        let centre = w.grid.id_at(6, 6).unwrap();
        let city = w.found_city_in(r, City::found("Carthage", CivId(1), centre));
        let outside = w.grid.neighbours(centre).next().unwrap();
        let warrior = r.unit("Warrior").unwrap();
        let movement = r.units.get(warrior).unwrap().movement;
        let unit = w.place_unit(Unit::new(warrior, CivId(0), outside, movement));
        w.diplomacy.meet(CivId(0), CivId(1));
        w.diplomacy.declare_war(CivId(0), CivId(1));
        (w, unit, city)
    }

    #[test]
    fn a_city_is_worn_down_but_never_destroyed() {
        let r = ruleset();
        let (mut w, unit, city) = besieged(&r);
        let full = max_health(&r, w.cities.get(city).unwrap());
        assert_eq!(w.cities.get(city).unwrap().health, 200, "the base before walls");
        assert!(full >= 200);

        // Hammer it, refreshing the attacker each turn.
        let mut rounds = 0;
        while w.cities.get(city).map_or(false, |c| c.health > 1) && rounds < 100 {
            w.units.get_mut(unit).unwrap().refresh(2);
            w.units.get_mut(unit).unwrap().health = 100;
            let before = w.cities.get(city).unwrap().health;
            let result = attack_city(&mut w, &r, unit, city).expect("a legal assault");
            assert!(result.damage > 0, "an assault should hurt");
            if !result.captured {
                assert!(w.cities.get(city).unwrap().health < before || before == 1);
            } else {
                break;
            }
            rounds += 1;
        }
        println!("{rounds} assaults to take the city");
        assert!(rounds > 0 && rounds < 100, "it should fall, but not at once");
    }

    #[test]
    fn taking_a_city_moves_it_and_its_land() {
        let r = ruleset();
        let (mut w, unit, city) = besieged(&r);
        w.cities.get_mut(city).unwrap().population = 6;
        w.cities.get_mut(city).unwrap().health = 1; // already at bay
        let centre = w.cities.get(city).unwrap().centre;

        assert_eq!(w.civ(CivId(1)).unwrap().cities, vec![city]);
        let result = attack_city(&mut w, &r, unit, city).expect("legal");
        assert!(result.captured);

        let taken = w.cities.get(city).unwrap();
        assert_eq!(taken.owner, CivId(0));
        assert_eq!(taken.population, 3, "a sacked city is halved");
        assert_eq!(w.civ(CivId(0)).unwrap().cities, vec![city]);
        assert!(w.civ(CivId(1)).unwrap().cities.is_empty(), "and the loser loses it");
        assert_eq!(w.grid.owner[centre.0 as usize], Some(CivId(0)), "the land goes with it");
    }

    #[test]
    fn a_city_at_peace_cannot_be_stormed() {
        let r = ruleset();
        let (mut w, unit, city) = besieged(&r);
        w.diplomacy.make_peace(CivId(0), CivId(1));
        assert_eq!(attack_city(&mut w, &r, unit, city), Err(CombatRejection::NotAtWar));
    }

    /// Losing every city ends a civilization, which is what makes Domination
    /// reachable at all.
    #[test]
    fn losing_the_last_city_ends_a_civ() {
        let r = ruleset();
        let (mut w, unit, city) = besieged(&r);
        w.cities.get_mut(city).unwrap().health = 1;
        assert!(w.civ(CivId(1)).unwrap().is_alive());

        attack_city(&mut w, &r, unit, city).expect("legal");
        assert!(!w.civ(CivId(1)).unwrap().is_alive(), "no cities and no units left");
        assert!(crate::world::victory::evaluate(
            &crate::world::victory::Milestone::CaptureAllCapitals,
            &w,
            &r,
            CivId(0)
        ) == crate::world::conditionals::Verdict::True);
    }

    /// A sacked city does not keep the granary of the city it was.
    #[test]
    fn capture_empties_the_granary() {
        let r = ruleset();
        let (mut w, unit, city) = besieged(&r);
        {
            let c = w.cities.get_mut(city).unwrap();
            c.population = 8;
            c.food_stored = 900;
            c.health = 1;
        }
        let result = attack_city(&mut w, &r, unit, city).expect("legal assault");
        assert!(result.captured);
        let c = w.cities.get(city).unwrap();
        assert_eq!(c.food_stored, 0, "halved to size four and still holding a size-eight granary");
        assert_eq!(c.population, 4);
    }

    /// The holy city of a religion stays its holy city whoever holds it.
    ///
    /// An audit suggested clearing this on capture. That is wrong for Civ V:
    /// taking Rome does not make you the pope, and a captured holy city
    /// remaining the religion's seat is exactly why taking one is worth doing.
    /// The population does not change its faith because the walls changed
    /// hands either, so the pressure stays too.
    #[test]
    fn capture_does_not_convert_the_conquered() {
        let r = ruleset();
        let (mut w, unit, city) = besieged(&r);
        {
            let c = w.cities.get_mut(city).unwrap();
            c.population = 6;
            c.faith.holy_city_of = Some(CivId(1));
            c.faith.add_pressure(Some(CivId(1)), 5000);
            c.faith.recount(6);
            c.health = 1;
        }
        assert!(attack_city(&mut w, &r, unit, city).expect("legal assault").captured);
        let c = w.cities.get(city).unwrap();
        assert_eq!(c.faith.holy_city_of, Some(CivId(1)), "the seat does not move with the flag");
        assert!(c.faith.pressure_of(Some(CivId(1))) > 0, "nor does the congregation");
        assert_eq!(c.owner, CivId(0), "but the city is ours");
    }
}

#[cfg(test)]
mod military_tests {
    use super::*;
    use crate::world::city::City;
    use crate::world::entities::Civilization;
    use crate::world::grid::{CivId, TileGrid};

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings")
            .expect("readable")
            .expect("loads")
    }

    fn world(r: &Ruleset) -> World {
        let g = TileGrid::new(12, 12, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        World::new(
            g,
            vec![
                Civilization::new(CivId(0), nation, false),
                Civilization::new(CivId(1), nation, false),
            ],
        )
    }

    fn unit(w: &mut World, r: &Ruleset, name: &str, civ: CivId, at: TileId) -> Id<Unit> {
        let base = r.unit(name).unwrap_or_else(|| panic!("{name} exists"));
        let movement = r.units.get(base).unwrap().movement;
        w.place_unit(Unit::new(base, civ, at, movement))
    }

    #[test]
    fn ranged_attack_deals_damage_without_return_damage() {
        let r = ruleset();
        let mut w = world(&r);
        w.diplomacy.declare_war(CivId(0), CivId(1));

        let a_tile = w.grid.id_at(4, 4).unwrap();
        let target_tile = w.grid.id_at(4, 6).unwrap();
        assert_eq!(w.grid.distance(a_tile, target_tile), 2);

        let archer = unit(&mut w, &r, "Archer", CivId(0), a_tile);
        let defender = unit(&mut w, &r, "Warrior", CivId(1), target_tile);

        let res = resolve_ranged_attack(&mut w, &r, archer, target_tile).expect("ranged attack succeeds");
        assert!(res.damage_to_defender > 0);
        assert_eq!(res.damage_to_attacker, 0);
        assert!(!res.attacker_died);
        assert_eq!(w.units.get(archer).unwrap().health, 100);
        assert!(w.units.get(defender).unwrap().health < 100);
        assert_eq!(w.units.get(archer).unwrap().position, a_tile, "attacker does not move");
        assert_eq!(w.units.get(archer).unwrap().movement_left, Fixed::ZERO);
        assert_eq!(w.units.get(archer).unwrap().xp, 2, "gains 2 XP for ranged attack");

        // Distance 3 should fail
        w.units.get_mut(archer).unwrap().refresh(2);
        let far_tile = w.grid.id_at(4, 7).unwrap();
        w.units.get_mut(defender).unwrap().position = far_tile;
        assert_eq!(
            resolve_ranged_attack(&mut w, &r, archer, far_tile),
            Err(CombatRejection::OutOfRange)
        );
    }

    #[test]
    fn line_of_sight_blocking() {
        let r = ruleset();
        let mut w = world(&r);
        w.diplomacy.declare_war(CivId(0), CivId(1));

        let a_tile = w.grid.id_at(4, 4).unwrap();
        let mid_tile = w.grid.id_at(4, 5).unwrap();
        let target_tile = w.grid.id_at(4, 6).unwrap();

        let archer = unit(&mut w, &r, "Archer", CivId(0), a_tile);
        let _defender = unit(&mut w, &r, "Warrior", CivId(1), target_tile);

        // Put a mountain in the middle
        let mountain = r.terrain("Mountain").unwrap();
        w.grid.terrain[mid_tile.0 as usize] = mountain;

        assert_eq!(
            resolve_ranged_attack(&mut w, &r, archer, target_tile),
            Err(CombatRejection::TargetBlockedByLOS)
        );

        // Shooter on hill can fire over a hill/forest, but not over impassable mountain
        let hill = r.terrain("Hill").unwrap();
        w.grid.terrain[mid_tile.0 as usize] = r.terrain("Grassland").unwrap();
        w.grid.features[mid_tile.0 as usize].push(hill);

        // Archer on flat land cannot fire over hill
        assert_eq!(
            resolve_ranged_attack(&mut w, &r, archer, target_tile),
            Err(CombatRejection::TargetBlockedByLOS)
        );

        // Place archer on hill -> now has LOS!
        w.grid.features[a_tile.0 as usize].push(hill);
        assert!(resolve_ranged_attack(&mut w, &r, archer, target_tile).is_ok());
    }

    #[test]
    fn city_bombardment_and_garrison_defense() {
        let r = ruleset();
        let mut w = world(&r);
        let centre = w.grid.id_at(6, 6).unwrap();
        let city = w.found_city_in(&r, City::found("Rome", CivId(0), centre));
        let enemy_tile = w.grid.id_at(6, 8).unwrap();
        assert_eq!(w.grid.distance(centre, enemy_tile), 2);
        w.diplomacy.declare_war(CivId(0), CivId(1));

        let enemy_unit = unit(&mut w, &r, "Warrior", CivId(1), enemy_tile);

        let base_def = city_defense_strength(&w, &r, city);

        let _warrior = unit(&mut w, &r, "Warrior", CivId(0), centre);
        let garr_def = city_defense_strength(&w, &r, city);
        assert_eq!(garr_def, base_def.mul(Fixed::from_fraction(6, 5)), "garrison adds +20%");

        let before_hp = w.units.get(enemy_unit).unwrap().health;
        let bombard_res = city_bombard(&mut w, &r, city, enemy_unit).expect("bombard succeeds");
        assert!(bombard_res.damage_to_defender > 0);
        assert_eq!(bombard_res.damage_to_attacker, 0);
        assert_eq!(w.units.get(enemy_unit).unwrap().health, before_hp - bombard_res.damage_to_defender as u8);
    }

    #[test]
    fn unit_xp_and_promotions() {
        let r = ruleset();
        let mut w = world(&r);
        let a_tile = w.grid.id_at(5, 5).unwrap();
        let warrior_id = unit(&mut w, &r, "Warrior", CivId(0), a_tile);
        let u = w.units.get_mut(warrior_id).unwrap();

        assert!(!can_promote(u));
        u.xp = 10;
        assert!(can_promote(u));

        let shock1 = r.promotions.id("Shock I").expect("Shock I exists");
        let shock2 = r.promotions.id("Shock II").expect("Shock II exists");

        let avail = available_promotions(&r, u);
        assert!(avail.contains(&shock1));
        assert!(!avail.contains(&shock2), "Shock II needs prerequisite");

        assert!(choose_promotion(u, &r, shock1).is_ok());
        assert!(u.promotions.contains(&shock1));
        assert!(!can_promote(u), "now need 30 XP for level 2");

        u.xp = 30;
        assert!(can_promote(u));
        let avail2 = available_promotions(&r, u);
        assert!(!avail2.contains(&shock1), "already has Shock I");
        assert!(avail2.contains(&shock2), "prerequisite now satisfied");

        assert!(choose_promotion(u, &r, shock2).is_ok());
        assert!(u.promotions.contains(&shock2));
    }

    #[test]
    fn civilian_capture() {
        let r = ruleset();
        let mut w = world(&r);
        let a_tile = w.grid.id_at(5, 5).unwrap();
        let d_tile = w.grid.neighbours(a_tile).next().unwrap();
        let settler_tile = w.grid.neighbours(a_tile).nth(1).unwrap();

        w.diplomacy.declare_war(CivId(0), CivId(1));

        let warrior = unit(&mut w, &r, "Warrior", CivId(0), a_tile);
        let worker = unit(&mut w, &r, "Worker", CivId(1), d_tile);
        let settler = unit(&mut w, &r, "Settler", CivId(1), settler_tile);

        let res = resolve_attack(&mut w, &r, warrior, worker).expect("capture succeeds");
        assert_eq!(res.damage_to_defender, 0);
        assert_eq!(res.damage_to_attacker, 0);
        assert_eq!(w.units.get(worker).unwrap().owner, CivId(0));
        assert!(w.civ(CivId(0)).unwrap().units.contains(&worker));
        assert!(!w.civ(CivId(1)).unwrap().units.contains(&worker));

        w.units.get_mut(warrior).unwrap().refresh(2);
        let res2 = resolve_attack(&mut w, &r, warrior, settler).expect("capture succeeds");
        assert_eq!(res2.damage_to_defender, 0);
        let captured_settler = w.units.get(settler).unwrap();
        assert_eq!(captured_settler.owner, CivId(0));
        assert_eq!(captured_settler.base, r.unit("Worker").unwrap(), "settler converted to worker");
    }

    #[test]
    fn barbarians_keep_a_captured_settler_whole_and_on_their_books() {
        let r = ruleset();
        let mut w = world(&r);
        let a_tile = w.grid.id_at(5, 5).unwrap();
        let d_tile = w.grid.neighbours(a_tile).next().unwrap();
        w.civ_mut(CivId(0)).unwrap().is_barbarian = true;
        w.diplomacy.declare_war(CivId(0), CivId(1));

        let brute = unit(&mut w, &r, "Warrior", CivId(0), a_tile);
        let settler = unit(&mut w, &r, "Settler", CivId(1), d_tile);
        resolve_attack(&mut w, &r, brute, settler).expect("capture succeeds");

        let captured = w.units.get(settler).unwrap();
        assert_eq!(captured.owner, CivId(0));
        assert_eq!(captured.base, r.unit("Settler").unwrap(), "barbarians hold settlers for ransom");
        assert!(w.civ(CivId(0)).unwrap().units.contains(&settler));
        assert!(
            !w.civ(CivId(1)).unwrap().units.contains(&settler),
            "the victim must stop counting a unit it no longer has"
        );
    }

    #[test]
    fn pillage_and_repair() {
        let r = ruleset();
        let mut w = world(&r);
        let tile = w.grid.id_at(5, 5).unwrap();
        let farm = r.improvements.id("Farm").unwrap();
        w.grid.set_improvement(tile, Some(farm));
        assert_eq!(w.grid.unpillaged_improvement(tile), Some(farm));

        let unit_id = unit(&mut w, &r, "Warrior", CivId(0), tile);
        w.units.get_mut(unit_id).unwrap().health = 70;
        let initial_gold = w.civ(CivId(0)).unwrap().gold;

        assert!(can_pillage(&w, unit_id));
        let pillage_res = pillage(&mut w, &r, unit_id).expect("pillage succeeds");
        assert_eq!(pillage_res.healed, 25);
        assert_eq!(w.units.get(unit_id).unwrap().health, 95);
        assert_eq!(w.civ(CivId(0)).unwrap().gold, initial_gold + 20);
        assert_eq!(w.grid.unpillaged_improvement(tile), None, "now pillaged");

        assert!(!can_pillage(&w, unit_id));

        w.grid.repair_improvement(tile);
        assert_eq!(w.grid.unpillaged_improvement(tile), Some(farm));
    }

    #[test]
    fn unit_upgrades() {
        let r = ruleset();
        let mut w = world(&r);
        let tile = w.grid.id_at(5, 5).unwrap();
        w.grid.owner[tile.0 as usize] = Some(CivId(0));

        let warrior = unit(&mut w, &r, "Warrior", CivId(0), tile);
        let swordsman = r.unit("Swordsman").unwrap();

        assert_eq!(can_upgrade(&w, &r, warrior), Err(UpgradeRejection::TechNotResearched));

        let tech = r.technologies.id("Iron Working").unwrap();
        w.civ_mut(CivId(0)).unwrap().techs.insert(tech);

        w.civ_mut(CivId(0)).unwrap().gold = 50;
        assert_eq!(can_upgrade(&w, &r, warrior), Err(UpgradeRejection::NotEnoughGold));

        w.civ_mut(CivId(0)).unwrap().gold = 100;
        let (up_base, cost) = can_upgrade(&w, &r, warrior).expect("can upgrade");
        assert_eq!(up_base, swordsman);
        assert_eq!(cost, 80);

        let res = upgrade_unit(&mut w, &r, warrior).expect("upgrade completes");
        assert_eq!(res, swordsman);
        assert_eq!(w.units.get(warrior).unwrap().base, swordsman);
        assert_eq!(w.civ(CivId(0)).unwrap().gold, 20);
    }

    #[test]
    fn nuclear_strike_mechanics() {
        let r = ruleset();
        let mut w = world(&r);
        let nuke_tile = w.grid.id_at(5, 5).unwrap();
        let target_tile = w.grid.id_at(5, 8).unwrap();
        assert_eq!(w.grid.distance(nuke_tile, target_tile), 3);

        let abomb = unit(&mut w, &r, "Atomic Bomb", CivId(0), nuke_tile);

        let victim = unit(&mut w, &r, "Warrior", CivId(1), target_tile);
        let city = w.found_city_in(&r, City::found("Kyoto", CivId(1), target_tile));
        w.cities.get_mut(city).unwrap().population = 10;
        w.cities.get_mut(city).unwrap().health = 200;

        let res = nuclear_strike(&mut w, &r, abomb, target_tile).expect("nuclear strike succeeds");
        assert!(res.units_destroyed.contains(&victim));
        assert!(res.cities_damaged.contains(&city));
        assert!(w.units.get(victim).is_none(), "victim unit destroyed");
        assert!(w.units.get(abomb).is_none(), "atomic bomb self-destructed");
        assert_eq!(w.cities.get(city).unwrap().population, 5, "population halved");
        assert_eq!(w.cities.get(city).unwrap().health, 100, "city health halved");
        let fallout = r.terrain("Fallout").unwrap();
        assert!(w.grid.features[target_tile.0 as usize].contains(&fallout));
    }

    #[test]
    fn air_strike_and_interception() {
        let r = ruleset();
        let mut w = world(&r);
        let base_tile = w.grid.id_at(4, 4).unwrap();
        let target_tile = w.grid.id_at(4, 7).unwrap();
        w.diplomacy.declare_war(CivId(0), CivId(1));

        let bomber = unit(&mut w, &r, "Bomber", CivId(0), base_tile);
        let defender = unit(&mut w, &r, "Warrior", CivId(1), target_tile);
        let fighter = unit(&mut w, &r, "Fighter", CivId(1), target_tile);

        let res = air_strike(&mut w, &r, bomber, target_tile).expect("air strike succeeds");
        assert!(res.damage_to_attacker > 0, "fighter intercepted and hurt bomber");
        assert!(res.damage_to_defender > 0, "bomber still dealt damage");
        assert_eq!(w.units.get(fighter).unwrap().attacks_left, 0, "interceptor spent attack");
        let _ = defender;
    }
}

