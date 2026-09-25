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
use crate::world::conditionals::{applies, CombatRole, GameContext};
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

impl std::fmt::Display for CombatRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            CombatRejection::NoSuchUnit => "no such unit",
            CombatRejection::NotAtWar => "not at war",
            CombatRejection::SameOwner => "both belong to the same nation",
            CombatRejection::NotAdjacent => "a melee attack needs an adjacent target",
            CombatRejection::OutOfRange => "the target is out of range",
            CombatRejection::NoMovement => "no movement or attacks left",
            CombatRejection::NoRangedAttack => "the unit has no ranged attack",
            CombatRejection::TargetBlockedByLOS => "line of sight to the target is blocked",
        })
    }
}

impl std::error::Error for CombatRejection {}

/// A unit's strength, and where each part of it came from against a specific target.
pub fn strength_breakdown_against(
    world: &World,
    r: &Ruleset,
    unit_id: Id<Unit>,
    target_tile: Option<TileId>,
    defending: bool,
) -> Option<(Fixed, ModifierStack)> {
    let opponent = target_tile.and_then(|tile| {
        if world.cities.iter().any(|(_, c)| c.centre == tile) {
            Some(crate::world::conditionals::Opponent::City)
        } else {
            world
                .units
                .iter()
                .find(|(_, u)| u.position == tile)
                .map(|(id, _)| crate::world::conditionals::Opponent::Unit(id))
        }
    });
    strength_breakdown_full(world, r, unit_id, target_tile, defending, opponent, false)
}

fn strength_breakdown_full(
    world: &World,
    r: &Ruleset,
    unit_id: Id<Unit>,
    target_tile: Option<TileId>,
    defending: bool,
    opponent: Option<crate::world::conditionals::Opponent>,
    ranged: bool,
) -> Option<(Fixed, ModifierStack)> {
    strength_parts(world, r, unit_id, target_tile, defending, opponent, ranged).map(|b| (b.base, b.stack))
}

/// A strength, where each part of it came from, and the units that lent theirs.
#[derive(Debug, Clone)]
pub(crate) struct Breakdown {
    pub base: Fixed,
    pub stack: ModifierStack,
    /// Friendly units beside the target that the flanking bonus counted.
    pub flankers: Vec<Id<Unit>>,
    /// The unit whose aura set the general's bonus, when it counted.
    pub general: Option<Id<Unit>>,
}

impl Breakdown {
    pub fn strength(&self) -> Fixed {
        self.stack.evaluate(self.base)
    }
}

fn strength_parts(
    world: &World,
    r: &Ruleset,
    unit_id: Id<Unit>,
    target_tile: Option<TileId>,
    defending: bool,
    opponent: Option<crate::world::conditionals::Opponent>,
    ranged: bool,
) -> Option<Breakdown> {
    let unit = world.units.get(unit_id)?;
    let mut flankers = Vec::new();
    let definition = r.units.get(unit.base)?;
    let mut base = if ranged { definition.ranged_strength.unwrap_or(0) } else { definition.strength.unwrap_or(0) };
    let mut stack = ModifierStack::new();

    // A unit whose resource has run out still stands; it just fights badly.
    if !world.civ(unit.owner).is_some_and(|c| c.is_barbarian)
        && !crate::world::resources::can_supply_unit(world, r, unit.owner, unit.base, Some(unit_id))
    {
        stack.add(Modifier::additive_percent(
            Fixed::whole(crate::world::resources::MISSING_RESOURCE_MALUS),
            Source::new(SourceKind::Other, "Missing resource"),
        ));
    }

    if defending && !unit.embarked {
        let context = GameContext::new(r)
            .with_world(world)
            .for_civ(unit.owner)
            .for_unit(unit_id)
            .on_tile(unit.position)
            .in_combat(CombatRole::Defending);
        let idx = unit.position.0 as usize;
        let bonus_of = |id| r.terrains.get(id).and_then(|t| t.defence_bonus).unwrap_or(Fixed::ZERO);
        let mut bonus = bonus_of(world.grid.terrain[idx]);
        if let Some(feature) = world.grid.features[idx].iter().map(|id| bonus_of(*id)).max().filter(|b| !b.is_zero()) {
            bonus = feature;
        }
        if let Some(wonder) = world.grid.natural_wonder[idx] {
            bonus += bonus_of(wonder);
        }
        if let Some(improvement) =
            world.grid.unpillaged_improvement(unit.position).and_then(|id| r.improvements.get(id))
        {
            for rule in &improvement.uniques {
                if rule.unique_type() == Some(UniqueType::DefensiveBonus) && applies(rule, &context).holds() {
                    bonus += Fixed::from_fraction(rule.params.first().and_then(|p| p.parse().ok()).unwrap_or(0), 100);
                }
            }
        }
        let ignores = |kind| {
            unit.all_uniques(r)
                .chain(world.civ(unit.owner).into_iter().flat_map(|c| c.uniques.all()))
                .any(|u| u.unique_type() == Some(kind) && applies(u, &context).holds())
        };
        if (bonus > Fixed::ZERO && !ignores(UniqueType::NoDefensiveTerrainBonus))
            || (bonus < Fixed::ZERO && !ignores(UniqueType::NoDefensiveTerrainPenalty))
        {
            stack.add(Modifier::additive_percent(bonus * Fixed::whole(100), Source::new(SourceKind::Terrain, "Tile")));
        }
        if unit.fortified {
            stack.add(Modifier::additive_percent(
                Fixed::whole(FORTIFICATION_BONUS * i32::from(unit.fortification_turns)),
                Source::new(SourceKind::Other, "Fortified"),
            ));
        }
    } else if let Some(target) = target_tile.filter(|_| !ranged) {
        // Across river penalty
        if world.grid.has_river_between(unit.position, target) {
            let has_bridge = world.civ(unit.owner).is_some_and(|c| {
                c.techs.iter().any(|t| {
                    r.technologies.get(*t).is_some_and(|tech| {
                        tech.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::RoadsConnectAcrossRivers))
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
        flankers = world
            .grid
            .neighbours(target)
            .filter(|&n| n != unit.position)
            .flat_map(|n| world.units_at(n))
            .filter(|&other_id| {
                world.units.get(other_id).is_some_and(|o| o.owner == unit.owner && o.is_military(r) && !o.is_ranged(r))
            })
            .collect();
        let flanking_units = flankers.len() as i32;

        if flanking_units > 0 {
            let context = GameContext::new(r)
                .with_world(world)
                .for_civ(unit.owner)
                .for_unit(unit_id)
                .in_combat(CombatRole::Attacking)
                .on_tile(target);
            let mut bonus = Fixed::whole(BASE_FLANKING_BONUS);
            for rule in unit
                .all_uniques(r)
                .chain(world.civ(unit.owner).into_iter().flat_map(|c| c.uniques.all()))
                .filter(|u| u.unique_type() == Some(UniqueType::FlankAttackBonus) && applies(u, &context).holds())
            {
                bonus = bonus
                    * (Fixed::ONE
                        + Fixed::from_fraction(rule.params.first().and_then(|p| p.parse().ok()).unwrap_or(0), 100));
            }
            stack.add(Modifier::additive_percent(
                Fixed::whole((bonus * Fixed::whole(flanking_units)).floor() as i32),
                Source::new(SourceKind::Other, "Flanking"),
            ));
        }
    }

    let mut context = GameContext::new(r)
        .with_world(world)
        .for_civ(unit.owner)
        .for_unit(unit_id)
        .on_tile(if defending { unit.position } else { target_tile.unwrap_or(unit.position) })
        .in_combat(if defending { CombatRole::Defending } else { CombatRole::Attacking });
    context.opponent = opponent;
    context.opponent_civ = match opponent {
        Some(crate::world::conditionals::Opponent::Unit(id)) => world.units.get(id).map(|u| u.owner),
        Some(crate::world::conditionals::Opponent::City) => {
            target_tile.and_then(|tile| world.cities.iter().find(|(_, c)| c.centre == tile).map(|(_, c)| c.owner))
        }
        None => None,
    };
    if context.opponent_civ.and_then(|id| world.civ(id)).is_some_and(|c| c.is_barbarian) {
        let bonus = world.game_difficulty(r).map_or(Fixed::ZERO, |d| d.barbarian_bonus);
        stack.add(Modifier::additive_percent(
            Fixed::whole((bonus * Fixed::whole(100)).floor() as i32),
            Source::new(SourceKind::Other, "Difficulty"),
        ));
    }
    if defending && unit.embarked && unit.is_military(r) {
        let era = crate::world::policies::era_number(world, r, unit.owner);
        base = r.eras.iter().find(|(_, e)| e.era_number == era).map_or(3, |(_, e)| e.embark_defense);
    }
    for unique in unit.all_uniques(r).chain(world.civ(unit.owner).into_iter().flat_map(|c| c.uniques.all())) {
        if !applies(unique, &context).holds() {
            continue;
        }
        let amount = unique.params.first().and_then(|p| p.parse::<i32>().ok()).unwrap_or(0);
        match unique.unique_type() {
            Some(UniqueType::Strength) => {
                stack.add(Modifier::additive_percent(
                    Fixed::whole(amount),
                    Source::new(SourceKind::Other, unique.text.clone()),
                ));
            }
            Some(UniqueType::StrengthAmount) => base += amount,
            Some(UniqueType::StrengthNearCapital) => {
                if let Some(capital) =
                    crate::world::connections::capital_of(world, r, unit.owner).and_then(|id| world.cities.get(id))
                {
                    let amount = (amount - 3 * world.grid.distance(unit.position, capital.centre) as i32).max(0);
                    stack.add(Modifier::additive_percent(
                        Fixed::whole(amount),
                        Source::new(SourceKind::Other, "Near capital"),
                    ));
                }
            }
            _ => {}
        }
    }
    // The strongest aura counts; the general is the unit that gave it.
    let (general, aura) = world
        .units
        .iter()
        .filter(|(_, u)| u.owner == unit.owner)
        .flat_map(|(id, source)| {
            source
                .all_uniques(r)
                .filter(|u| u.unique_type() == Some(UniqueType::StrengthBonusInRadius) && applies(u, &context).holds())
                .map(move |u| (id, source, u))
        })
        .filter(|(_, source, u)| {
            u.params.len() >= 3
                && world.grid.distance(source.position, unit.position) <= u.params[2].parse::<u32>().unwrap_or(0)
                && crate::world::filters::unit_matches_in(world, r, unit, &u.params[1]).holds()
        })
        .map(|(id, source, u)| {
            let double = source.all_uniques(r).any(|u| {
                u.unique_type() == Some(UniqueType::GreatPerson) && u.params.first().is_some_and(|p| p == "War")
            }) && unit
                .all_uniques(r)
                .chain(world.civ(unit.owner).into_iter().flat_map(|c| c.uniques.all()))
                .any(|u| {
                    u.unique_type() == Some(UniqueType::GreatGeneralProvidesDoubleCombatBonus)
                        && applies(u, &context).holds()
                });
            (Some(id), u.params[0].parse::<i32>().unwrap_or(0) * if double { 2 } else { 1 })
        })
        // Like `max`, the last of equals wins.
        .max_by_key(|(_, amount)| *amount)
        .unwrap_or((None, 0));
    let general = general.filter(|_| aura != 0);
    if aura != 0 {
        stack.add(Modifier::additive_percent(Fixed::whole(aura), Source::new(SourceKind::Other, "General aura")));
    }
    let adjacent = world
        .units
        .iter()
        .filter(|(_, u)| {
            world.diplomacy.at_war(unit.owner, u.owner) && world.grid.neighbours(unit.position).any(|t| t == u.position)
        })
        .flat_map(|(_, u)| u.all_uniques(r))
        .filter(|u| {
            u.unique_type() == Some(UniqueType::StrengthForAdjacentEnemies)
                && u.params.len() >= 3
                && applies(u, &context).holds()
                && crate::world::filters::unit_matches_in(world, r, unit, &u.params[1]).holds()
                && crate::world::conditionals::tile_matches(&context, unit.position, &u.params[2]).holds()
        })
        .filter_map(|u| u.params[0].parse::<i32>().ok())
        .min()
        .unwrap_or(0);
    if adjacent != 0 {
        stack.add(Modifier::additive_percent(Fixed::whole(adjacent), Source::new(SourceKind::Other, "Adjacent enemy")));
    }
    // Strength never drops below a token amount, or the ratio maths degenerates.
    stack.floor_at(Fixed::ONE);
    Some(Breakdown { base: Fixed::whole(base), stack, flankers, general })
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

pub fn combat_strength(
    world: &World,
    r: &Ruleset,
    id: Id<Unit>,
    target: crate::world::air::Target,
    defending: bool,
) -> Option<Fixed> {
    combat_strength_with_bonus(world, r, id, target, defending, 0)
}
pub(crate) fn combat_strength_with_bonus(
    world: &World,
    r: &Ruleset,
    id: Id<Unit>,
    target: crate::world::air::Target,
    defending: bool,
    bonus: i32,
) -> Option<Fixed> {
    let (base, stack) = combat_breakdown(world, r, id, target, defending, bonus)?;
    Some(stack.evaluate(base))
}

/// A unit's strength against a specific target, and where each part of it came from.
pub fn combat_breakdown(
    world: &World,
    r: &Ruleset,
    id: Id<Unit>,
    target: crate::world::air::Target,
    defending: bool,
    bonus: i32,
) -> Option<(Fixed, ModifierStack)> {
    combat_parts(world, r, id, target, defending, bonus).map(|b| (b.base, b.stack))
}

/// [`combat_breakdown`], with the units that lent their strength.
pub(crate) fn combat_parts(
    world: &World,
    r: &Ruleset,
    id: Id<Unit>,
    target: crate::world::air::Target,
    defending: bool,
    bonus: i32,
) -> Option<Breakdown> {
    let unit = world.units.get(id)?;
    let (opponent, tile, enemy_ranged) = match target {
        crate::world::air::Target::Unit(other) => {
            let other_unit = world.units.get(other)?;
            (crate::world::conditionals::Opponent::Unit(other), other_unit.position, other_unit.is_ranged(r))
        }
        crate::world::air::Target::City(city) => {
            (crate::world::conditionals::Opponent::City, world.cities.get(city)?.centre, true)
        }
    };
    let ranged = unit.is_ranged(r) && (!defending || enemy_ranged);
    let mut parts = strength_parts(world, r, id, Some(tile), defending, Some(opponent), ranged)?;
    parts.stack.add(Modifier::additive_percent(Fixed::whole(bonus), Source::new(SourceKind::Other, "Mission")));
    Some(parts)
}

/// One side of a previewed attack.
#[derive(Debug, Clone, PartialEq)]
pub struct PreviewSide {
    /// Strength after every modifier.
    pub strength: Fixed,
    /// Strength before percentage modifiers. Cities list no modifiers.
    pub base: Fixed,
    pub modifiers: Vec<Modifier>,
    pub health: i32,
    pub max_health: i32,
    /// Damage this side takes at the lowest and the highest attack roll.
    pub damage: (i32, i32),
}

/// What an attack would do, across the whole attack roll.
///
/// The roll is seeded by the turn and both tiles, so the exact outcome is knowable in advance. A preview
/// gives its range instead: players weigh the odds without learning which tile rolls best. The figures
/// use the same formulas as [`resolve_attack`], [`resolve_ranged_attack`], [`attack_city`] and
/// [`attack_city_ranged`]; a death ends a melee exchange early, so a fatal fight can deal less than the
/// lower figure.
#[derive(Debug, Clone, PartialEq)]
pub struct AttackPreview {
    pub ranged: bool,
    /// A melee attack on a civilian or a city at one health captures it outright.
    pub capture: bool,
    pub attacker: PreviewSide,
    pub defender: PreviewSide,
}

pub fn preview_attack(
    world: &World,
    r: &Ruleset,
    attacker: Id<Unit>,
    target: crate::world::air::Target,
) -> Option<AttackPreview> {
    use crate::world::air::Target;
    let a = world.units.get(attacker)?;
    let ranged = a.is_ranged(r);
    let a_health = i32::from(a.health);
    let (a_base, a_stack) = combat_breakdown(world, r, attacker, target, false, 0)?;
    let att = a_stack.evaluate(a_base);
    let wounded_attacker = Precise::from_fixed(unit_health_ratio(world, r, attacker));
    let span = |f: &dyn Fn(Fixed) -> i32| (f(Fixed::ZERO), f(Fixed::ONE));
    let cap = |(low, high): (i32, i32), limit: i32| (low.clamp(0, limit.max(0)), high.clamp(0, limit.max(0)));
    let mut capture = false;
    let (defender, to_attacker) = match target {
        Target::Unit(id) => {
            let d = world.units.get(id)?;
            let d_health = i32::from(d.health);
            if d.is_civilian(r) {
                // Ranged fire deals a flat 40 to civilians; melee captures them without a fight.
                let damage = if ranged { 40.min(d_health) } else { 0 };
                capture = !ranged;
                let side = PreviewSide {
                    strength: Fixed::ZERO,
                    base: Fixed::ZERO,
                    modifiers: Vec::new(),
                    health: d_health,
                    max_health: 100,
                    damage: (damage, damage),
                };
                (side, (0, 0))
            } else {
                let (d_base, d_stack) = combat_breakdown(world, r, id, Target::Unit(attacker), true, 0)?;
                let def = d_stack.evaluate(d_base);
                let ratio = strength_ratio(att, def);
                let wounded_defender = Precise::from_fixed(unit_health_ratio(world, r, id));
                let to_d = span(&|roll| (damage_modifier_precise(ratio, false, roll) * wounded_attacker).round().max(0) as i32);
                let to_a = if ranged {
                    (0, 0)
                } else {
                    span(&|roll| (damage_modifier_precise(ratio, true, roll) * wounded_defender).round().max(0) as i32)
                };
                let side = PreviewSide {
                    strength: def,
                    base: d_base,
                    modifiers: d_stack.modifiers().to_vec(),
                    health: d_health,
                    max_health: 100,
                    damage: cap(to_d, d_health),
                };
                (side, to_a)
            }
        }
        Target::City(id) => {
            let c = world.cities.get(id)?;
            let def = city_combat_strength(world, r, id, Some(attacker), false);
            let ratio = strength_ratio(att, def);
            let to_d = span(&|roll| (damage_modifier_precise(ratio, false, roll) * wounded_attacker).round().max(0) as i32);
            let to_a = if ranged { (0, 0) } else { span(&|roll| damage_modifier_precise(ratio, true, roll).round().max(0) as i32) };
            capture = !ranged && c.health <= 1;
            // Cities stop at one health; the melee blow that finds them there takes the city.
            let side = PreviewSide {
                strength: def,
                base: def,
                modifiers: Vec::new(),
                health: c.health,
                max_health: crate::world::city::max_health(r, c),
                damage: cap(to_d, c.health - 1),
            };
            (side, to_a)
        }
    };
    let attacker = PreviewSide {
        strength: att,
        base: a_base,
        modifiers: a_stack.modifiers().to_vec(),
        health: a_health,
        max_health: 100,
        damage: cap(to_attacker, a_health),
    };
    Some(AttackPreview { ranged, capture, attacker, defender })
}

/// What set each side's strength in a fight, and who stood by the attacker. Empty where no strength was
/// reckoned: a capture, a withdrawal, a city's side.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BattleTags {
    pub attacker_mods: Vec<Modifier>,
    pub defender_mods: Vec<Modifier>,
    /// Friendly units beside the target that the attacker's flanking bonus counted.
    pub flankers: Vec<Id<Unit>>,
    /// The unit whose aura counted toward the attacker's strength: a great general.
    pub general: Option<Id<Unit>>,
}

impl BattleTags {
    pub(crate) fn of(attacker: Option<&Breakdown>, defender: Option<&Breakdown>) -> BattleTags {
        let mods = |side: Option<&Breakdown>| side.map(|b| b.stack.modifiers().to_vec()).unwrap_or_default();
        BattleTags {
            attacker_mods: mods(attacker),
            defender_mods: mods(defender),
            flankers: attacker.map(|b| b.flankers.clone()).unwrap_or_default(),
            general: attacker.and_then(|b| b.general),
        }
    }
}

/// A civilian a melee attack took where it stood, `at`. It keeps its id; `becomes` is its new type, when
/// it changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capture {
    pub unit: Id<Unit>,
    pub at: TileId,
    pub from_owner: crate::world::grid::CivId,
    pub to_owner: crate::world::grid::CivId,
    pub becomes: Option<Id<BaseUnit>>,
}

/// Who fought whom, from where, and what the blow cost each side. Presentation shows it to the two sides.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BattleReport {
    pub attacker: Id<Unit>,
    pub attacker_base: Id<BaseUnit>,
    pub attacker_owner: crate::world::grid::CivId,
    pub target: crate::world::air::Target,
    /// The defending unit's type; none when a city defends.
    pub defender_base: Option<Id<BaseUnit>>,
    pub defender_owner: crate::world::grid::CivId,
    /// Where the attacker stood, and where the blow landed.
    pub from: TileId,
    pub at: TileId,
    pub ranged: bool,
    pub damage_to_attacker: i32,
    pub damage_to_defender: i32,
    pub tags: BattleTags,
}

/// Both sides as they stood before a blow. [`BattleWatch::report`] reads what changed, so every kind of
/// attack (melee, ranged, air, on a city) reports alike.
#[derive(Debug, Clone)]
pub struct BattleWatch {
    report: BattleReport,
    attacker_health: i32,
    defender_health: i32,
}

impl BattleWatch {
    pub fn before(world: &World, r: &Ruleset, attacker: Id<Unit>, target: crate::world::air::Target) -> Option<BattleWatch> {
        use crate::world::air::Target;
        let a = world.units.get(attacker)?;
        let (owner, at, base, health) = match target {
            Target::Unit(id) => {
                let d = world.units.get(id)?;
                (d.owner, d.position, Some(d.base), i32::from(d.health))
            }
            Target::City(id) => {
                let c = world.cities.get(id)?;
                (c.owner, c.centre, None, c.health)
            }
        };
        let report = BattleReport {
            attacker,
            attacker_base: a.base,
            attacker_owner: a.owner,
            target,
            defender_base: base,
            defender_owner: owner,
            from: a.position,
            at,
            ranged: a.is_ranged(r) || a.is_air(r),
            damage_to_attacker: 0,
            damage_to_defender: 0,
            tags: BattleTags::default(),
        };
        Some(BattleWatch { report, attacker_health: i32::from(a.health), defender_health: health })
    }

    /// A side that is gone lost all of its health; a captured civilian lost none. `tags` are the fight's own,
    /// from its result.
    pub fn report(self, world: &World, tags: BattleTags) -> BattleReport {
        use crate::world::air::Target;
        let attacker = world.units.get(self.report.attacker).map_or(0, |u| i32::from(u.health));
        let defender = match self.report.target {
            Target::Unit(id) => world.units.get(id).map_or(0, |u| i32::from(u.health)),
            Target::City(id) => world.cities.get(id).map_or(0, |c| c.health),
        };
        BattleReport {
            damage_to_attacker: self.attacker_health - attacker,
            damage_to_defender: self.defender_health - defender,
            tags,
            ..self.report
        }
    }
}

pub fn unit_health_ratio(world: &World, r: &Ruleset, id: Id<Unit>) -> Fixed {
    let Some(unit) = world.units.get(id) else { return Fixed::ONE };
    let context = GameContext::new(r).with_world(world).for_civ(unit.owner).for_unit(id).on_tile(unit.position);
    if unit
        .all_uniques(r)
        .chain(world.civ(unit.owner).into_iter().flat_map(|c| c.uniques.all()))
        .any(|u| u.unique_type() == Some(UniqueType::NoDamagePenaltyWoundedUnits) && applies(u, &context).holds())
    {
        Fixed::ONE
    } else {
        health_ratio(unit.health)
    }
}

/// How much less a wounded unit hits for.
pub fn health_ratio(health: u8) -> Fixed {
    let missing = Fixed::whole(100 - health.min(100) as i32);
    Fixed::ONE - missing.checked_div(Fixed::whole(WOUNDED_DAMAGE_DIVISOR)).unwrap_or(Fixed::ZERO)
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
    Precise::from_fixed(attack).checked_div(Precise::from_fixed(defend)).unwrap_or(Precise::whole(1))
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
    let inverse = one.checked_div(ratio).unwrap_or(one);
    let stronger = if ratio < one { inverse } else { ratio };

    let quarter = (stronger + Precise::whole(3)).checked_div(Precise::whole(4)).unwrap_or(one);
    let mut modifier = (quarter.powi(4) + one).checked_div(Precise::whole(2)).unwrap_or(one);

    // The weaker party's damage is the reciprocal.
    let invert = (to_attacker && ratio > one) || (!to_attacker && ratio < one);
    if invert {
        modifier = one.checked_div(modifier).unwrap_or(one);
    }

    let base = Precise::whole(BASE_DAMAGE_LOW) + (Precise::from_fixed(randomness) * Precise::whole(BASE_DAMAGE_SPREAD));
    base * modifier
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CombatResult {
    pub damage_to_defender: i32,
    pub damage_to_attacker: i32,
    pub defender_died: bool,
    pub attacker_died: bool,
    /// The civilian the attack took, if it took one.
    pub captured: Option<Capture>,
    pub tags: BattleTags,
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
    if !crate::world::military::may_attack(world, r, attacker, crate::world::air::Target::Unit(defender)) {
        return Err(CombatRejection::NoRangedAttack);
    }
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

    if crate::world::military::withdraw(world, r, attacker, defender) {
        return Ok(CombatResult::default());
    }
    if d_is_civilian {
        let uncapturable = world.units.get(defender).is_some_and(|u| u.has_unique(r, UniqueType::Uncapturable));
        let mut captured = None;
        if uncapturable {
            world.remove_unit(defender);
        } else {
            let barbarian_captor = world.civ(a_owner).is_some_and(|c| c.is_barbarian);
            let demoted_to =
                if !barbarian_captor && r.unit("Settler") == Some(d_base) { r.unit("Worker") } else { None };
            world.transfer_unit(defender, a_owner);
            if let Some(u) = world.units.get_mut(defender) {
                if let Some(worker) = demoted_to {
                    u.base = worker;
                }
                u.movement_left = Fixed::ZERO;
                captured =
                    Some(Capture { unit: defender, at: d_position, from_owner: d_owner, to_owner: a_owner, becomes: demoted_to });
            }
        }
        // Capturing a civilian costs the actual move, not a combat attack.
        crate::world::military::advance(world, r, attacker, d_position);
        return Ok(CombatResult { defender_died: uncapturable, captured, ..CombatResult::default() });
    }

    let a_parts = combat_parts(world, r, attacker, crate::world::air::Target::Unit(defender), false, 0)
        .ok_or(CombatRejection::NoSuchUnit)?;
    let d_parts = combat_parts(world, r, defender, crate::world::air::Target::Unit(attacker), true, 0)
        .ok_or(CombatRejection::NoSuchUnit)?;
    let (att, def) = (a_parts.strength(), d_parts.strength());
    let tags = BattleTags::of(Some(&a_parts), Some(&d_parts));
    let ratio = strength_ratio(att, def);
    let randomness = combat_seed(world, a_position, d_position).unit_interval();
    let to_defender =
        damage_modifier_precise(ratio, false, randomness) * Precise::from_fixed(unit_health_ratio(world, r, attacker));
    let to_attacker =
        damage_modifier_precise(ratio, true, randomness) * Precise::from_fixed(unit_health_ratio(world, r, defender));
    let (dmg_a, dmg_d) = crate::world::military::exchange(
        combat_seed(world, a_position, d_position),
        i32::from(a_health),
        i32::from(d_health),
        to_attacker.round().max(0) as i32,
        to_defender.round().max(0) as i32,
        false,
    );
    if world.civ(d_owner).is_some_and(|c| c.is_barbarian) {
        crate::world::barbarians::on_camp_attacked(world, d_position);
    }
    crate::world::military::plunder_damage(world, r, attacker, crate::world::air::Target::Unit(defender), dmg_d);
    let defender_died = dmg_d >= d_health as i32;
    let attacker_died = dmg_a >= a_health as i32;
    let capture = defender_died
        && !attacker_died
        && crate::world::military::captures_kill(world, r, attacker, defender, att, def);
    let defeated = world.units.get(defender).expect("defender").clone();
    crate::world::military::award_xp(world, r, defender, 4, Some(attacker), a_owner, true);
    crate::world::military::award_xp(world, r, attacker, 5, Some(defender), d_owner, false);
    if defender_died {
        crate::world::military::reward_kill(world, r, attacker, defender, false);
    }
    if attacker_died {
        crate::world::military::reward_kill(world, r, defender, attacker, true);
    }
    if defender_died {
        world.remove_unit(defender);
    } else if let Some(u) = world.units.get_mut(defender) {
        u.health = (i32::from(u.health) - dmg_d).max(1) as u8;
    }
    if attacker_died {
        world.remove_unit(attacker);
    } else {
        world.units.get_mut(attacker).expect("survivor").health = (i32::from(a_health) - dmg_a).max(1) as u8;
        let captured = capture && crate::world::military::spawn_capture(world, r, a_owner, &defeated).is_some();
        let advanced = defender_died && !captured && crate::world::military::advance(world, r, attacker, d_position);
        crate::world::military::spend_attack(world, r, attacker, advanced);
        if defender_died {
            crate::world::military::heal_on_kill(world, r, attacker);
        }
    }

    Ok(CombatResult {
        damage_to_defender: dmg_d,
        damage_to_attacker: dmg_a,
        defender_died,
        attacker_died,
        captured: None,
        tags,
    })
}

/// What an attack on a city did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CityAttackResult {
    pub damage: i32,
    pub city_health: i32,
    /// The city changed hands. A city is never destroyed — it is taken.
    pub captured: bool,
    pub attacker_died: bool,
    /// The attacker's side only: a city lists no modifiers.
    pub tags: BattleTags,
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
    if !crate::world::military::may_attack(world, r, attacker, crate::world::air::Target::City(city_id)) {
        return Err(CombatRejection::NoRangedAttack);
    }
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

    let a_parts = combat_parts(world, r, attacker, crate::world::air::Target::City(city_id), false, 0)
        .ok_or(CombatRejection::NoSuchUnit)?;
    let att = a_parts.strength();
    let tags = BattleTags::of(Some(&a_parts), None);
    let def = city_combat_strength(world, r, city_id, Some(attacker), false);
    let ratio = strength_ratio(att, def);
    let randomness = combat_seed(world, a_tile, centre).unit_interval();

    let to_city = (damage_modifier_precise(ratio, false, randomness)
        * Precise::from_fixed(unit_health_ratio(world, r, attacker)))
    .round()
    .max(0) as i32;
    let to_attacker = damage_modifier_precise(ratio, true, randomness).round().max(0) as i32;

    let (to_attacker, to_city) = crate::world::military::exchange(
        combat_seed(world, a_tile, centre),
        i32::from(a_health),
        city.health,
        to_attacker,
        to_city,
        true,
    );
    let city_owner = city.owner;
    crate::world::military::plunder_damage(world, r, attacker, crate::world::air::Target::City(city_id), to_city);
    crate::world::military::award_xp(world, r, attacker, 5, None, city_owner, false);
    // A city floors at one; it is captured, not levelled.
    let city = world.cities.get_mut(city_id).ok_or(CombatRejection::NoSuchUnit)?;
    city.health = (city.health - to_city).max(1);
    let at_bay = city.health <= 1;

    let mut attacker_died = to_attacker >= a_health as i32;
    if attacker_died {
        world.remove_unit(attacker);
    } else if let Some(u) = world.units.get_mut(attacker) {
        u.health = (u.health as i32 - to_attacker).max(1) as u8;
    }

    // Taking it needs a surviving attacker and a city already at bay.
    let mut captured = at_bay
        && !attacker_died
        && !world.units.get(attacker).is_some_and(|u| u.has_unique(r, UniqueType::CannotCaptureCities));
    if captured && world.civ(owner).is_some_and(|c| c.is_barbarian) {
        // Barbarians raid instead of conquering (`Battle.handleCityDefeated`).
        // The city and its original-capital identity survive the attack.
        let defender = world.cities.get(city_id).expect("attacked city").owner;
        world.cities.get_mut(city_id).expect("attacked city").health = 2;
        if let Some(civ) = world.civ_mut(defender) {
            civ.gold -= civ.gold.min(200);
        }
        world.remove_unit(attacker);
        captured = false;
        attacker_died = true;
    } else if captured {
        crate::world::military::capture_city_occupants(world, r, owner, centre);
        capture_city(world, r, city_id, owner);
    }

    if !attacker_died {
        let advanced = captured && crate::world::military::advance(world, r, attacker, centre);
        crate::world::military::spend_attack(world, r, attacker, advanced);
    }

    Ok(CityAttackResult {
        damage: to_city,
        city_health: world.cities.get(city_id).map_or(0, |c| c.health),
        captured,
        attacker_died,
        tags,
    })
}

/// Hand a city to its conqueror (`Battle.conquerCity`).
///
/// Barbarian attacks are handled as raids by [`attack_city`]. Other attackers
/// plunder the city and hold it as a resisting puppet — see [`conquest`](crate::world::conquest) — and an AI
/// then decides at once whether to keep or raze it, as Unciv's does. A human
/// player keeps the puppet until they choose.
pub fn capture_city(
    world: &mut World,
    r: &Ruleset,
    city_id: Id<crate::world::city::City>,
    new_owner: crate::world::grid::CivId,
) {
    if world.civ(new_owner).is_some_and(|c| c.is_barbarian) {
        return;
    }
    let previous_owner = world.cities.get(city_id).map(|c| c.owner);
    if crate::world::conquest::conquer(world, r, city_id, new_owner).is_none() {
        return;
    }
    if let Some(old) = previous_owner.filter(|c| world.civ(*c).is_some_and(|c| c.is_city_state() && !c.is_alive())) {
        crate::world::quests::on_city_state_conquered(world, r, old, new_owner);
    }
    if world.civ(new_owner).is_some_and(|c| c.is_ai) {
        if crate::world::ai::should_raze(world, r, new_owner, city_id) {
            let _ = crate::world::conquest::raze(world, city_id);
        }
    } else if let Some(c) = world.cities.get_mut(city_id).filter(|c| c.puppet) {
        // A human answers this turn (`CityConquestPopup`); see `conquest::capture_choices`.
        c.capture_pending = true;
    }
}

/// Breakdown of ranged strength for a unit.
pub fn ranged_strength_breakdown(world: &World, r: &Ruleset, unit_id: Id<Unit>) -> Option<(Fixed, ModifierStack)> {
    if !world.units.get(unit_id)?.is_ranged(r) {
        return None;
    }
    strength_breakdown_full(world, r, unit_id, None, false, None, true)
}

/// Evaluated ranged strength of a unit.
pub fn ranged_strength(world: &World, r: &Ruleset, unit: Id<Unit>) -> Option<Fixed> {
    let (base, stack) = ranged_strength_breakdown(world, r, unit)?;
    Some(stack.evaluate(base))
}

/// Line of sight check between `from` and `to`.
pub fn has_line_of_sight(grid: &TileGrid, r: &Ruleset, from: TileId, to: TileId, indirect_fire: bool) -> bool {
    indirect_fire || crate::world::fog::viewable_tiles(grid, r, from, grid.distance(from, to), true).contains(&to)
}

/// Execute a ranged attack on an enemy unit at `target_tile`.
pub fn resolve_ranged_attack(
    world: &mut World,
    r: &Ruleset,
    attacker: Id<Unit>,
    target_tile: TileId,
) -> Result<CombatResult, CombatRejection> {
    let a = world.units.get(attacker).ok_or(CombatRejection::NoSuchUnit)?;
    if a.has_unique(r, UniqueType::MustSetUp) && !a.set_up {
        return Err(CombatRejection::NoMovement);
    }
    if !a.can_move() || a.attacks_left == 0 {
        return Err(CombatRejection::NoMovement);
    }
    let base = r.units.get(a.base).ok_or(CombatRejection::NoSuchUnit)?;
    let range = crate::world::air::attack_range(world, r, attacker);
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

    let defender_id =
        world.units.iter().find(|(_, u)| u.position == target_tile && u.owner != a.owner).map(|(id, _)| id);
    let Some(defender) = defender_id else {
        return Err(CombatRejection::NoSuchUnit);
    };

    if !crate::world::military::may_attack(world, r, attacker, crate::world::air::Target::Unit(defender)) {
        return Err(CombatRejection::NoRangedAttack);
    }
    let d = world.units.get(defender).ok_or(CombatRejection::NoSuchUnit)?;
    if world.diplomacy.have_met(a.owner, d.owner) && !world.diplomacy.at_war(a.owner, d.owner) {
        return Err(CombatRejection::NotAtWar);
    }

    let a_parts = combat_parts(world, r, attacker, crate::world::air::Target::Unit(defender), false, 0)
        .ok_or(CombatRejection::NoSuchUnit)?;
    let d_parts = combat_parts(world, r, defender, crate::world::air::Target::Unit(attacker), true, 0)
        .ok_or(CombatRejection::NoSuchUnit)?;
    let (att, def) = (a_parts.strength(), d_parts.strength());
    let tags = BattleTags::of(Some(&a_parts), Some(&d_parts));
    let ratio = strength_ratio(att, def);

    let randomness = combat_seed(world, a.position, target_tile).unit_interval();
    let to_defender = (damage_modifier_precise(ratio, false, randomness)
        * Precise::from_fixed(unit_health_ratio(world, r, attacker)))
    .round()
    .max(0) as i32;

    let to_defender = if d.is_civilian(r) { 40 } else { to_defender }.min(i32::from(d.health));
    let defender_died = to_defender >= d.health as i32;
    let d_owner = d.owner;
    let a_owner = a.owner;
    let defeated = d.clone();
    if world.civ(d_owner).is_some_and(|c| c.is_barbarian) {
        crate::world::barbarians::on_camp_attacked(world, target_tile);
    }
    crate::world::military::plunder_damage(world, r, attacker, crate::world::air::Target::Unit(defender), to_defender);
    let capture = defender_died && crate::world::military::captures_kill(world, r, attacker, defender, att, def);
    crate::world::military::award_xp(world, r, defender, 2, Some(attacker), a_owner, true);
    crate::world::military::award_xp(world, r, attacker, 2, Some(defender), d_owner, false);
    if defender_died {
        crate::world::military::reward_kill(world, r, attacker, defender, false);
    }
    if defender_died {
        world.remove_unit(defender);
    } else if let Some(u) = world.units.get_mut(defender) {
        u.health = (i32::from(u.health) - to_defender).max(1) as u8;
    }
    if capture {
        crate::world::military::spawn_capture(world, r, a_owner, &defeated);
    }
    crate::world::military::spend_attack(world, r, attacker, false);
    if defender_died {
        crate::world::military::heal_on_kill(world, r, attacker);
    }

    Ok(CombatResult { damage_to_defender: to_defender, defender_died, tags, ..CombatResult::default() })
}

/// Execute a ranged attack on a city.
pub fn attack_city_ranged(
    world: &mut World,
    r: &Ruleset,
    attacker: Id<Unit>,
    city_id: Id<crate::world::city::City>,
) -> Result<CityAttackResult, CombatRejection> {
    if !crate::world::military::may_attack(world, r, attacker, crate::world::air::Target::City(city_id)) {
        return Err(CombatRejection::NoRangedAttack);
    }
    let a = world.units.get(attacker).ok_or(CombatRejection::NoSuchUnit)?;
    if a.has_unique(r, UniqueType::MustSetUp) && !a.set_up {
        return Err(CombatRejection::NoMovement);
    }
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
    let range = crate::world::air::attack_range(world, r, attacker);
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

    let a_parts = combat_parts(world, r, attacker, crate::world::air::Target::City(city_id), false, 0)
        .ok_or(CombatRejection::NoSuchUnit)?;
    let att = a_parts.strength();
    let tags = BattleTags::of(Some(&a_parts), None);
    let def = city_combat_strength(world, r, city_id, Some(attacker), false);
    let ratio = strength_ratio(att, def);
    let randomness = combat_seed(world, a.position, city.centre).unit_interval();

    let to_city = (damage_modifier_precise(ratio, false, randomness)
        * Precise::from_fixed(unit_health_ratio(world, r, attacker)))
    .round()
    .max(0) as i32;

    let city_owner = city.owner;
    let to_city = to_city.min(city.health.saturating_sub(1));
    crate::world::military::plunder_damage(world, r, attacker, crate::world::air::Target::City(city_id), to_city);
    crate::world::military::award_xp(world, r, attacker, 3, None, city_owner, false);
    let city = world.cities.get_mut(city_id).ok_or(CombatRejection::NoSuchUnit)?;
    city.health = (city.health - to_city).max(1);

    crate::world::military::spend_attack(world, r, attacker, false);

    Ok(CityAttackResult {
        damage: to_city,
        city_health: world.cities.get(city_id).map_or(0, |c| c.health),
        captured: false,
        attacker_died: false,
        tags,
    })
}

/// Effective defensive combat strength of a city including garrison bonuses.
pub fn city_defense_strength(world: &World, r: &Ruleset, city_id: Id<crate::world::city::City>) -> Fixed {
    city_combat_strength(world, r, city_id, None, false)
}
pub(crate) fn city_combat_strength(
    world: &World,
    r: &Ruleset,
    id: Id<crate::world::City>,
    enemy: Option<Id<Unit>>,
    attacking: bool,
) -> Fixed {
    let Some(city) = world.cities.get(id) else { return Fixed::ONE };
    if !attacking && city.health <= 1 {
        return Fixed::ONE;
    }
    let context = GameContext::new(r)
        .with_world(world)
        .for_civ(city.owner)
        .in_city(id)
        .on_tile(city.centre)
        .in_combat(if attacking { CombatRole::Attacking } else { CombatRole::Defending });
    let mut context = context;
    context.opponent = enemy.map(crate::world::conditionals::Opponent::Unit);
    let rules = crate::world::religion::city_uniques(world, r, city);
    let mut strength = Fixed::whole(8) + Fixed::permille(400) * Fixed::whole(city.population as i32);
    for terrain in world.grid.terrains(city.centre).filter_map(|id| r.terrains.get(id)) {
        for rule in terrain.uniques.iter().filter(|u| u.unique_type() == Some(UniqueType::GrantsCityStrength)) {
            strength += Fixed::whole(rule.params.first().and_then(|p| p.parse().ok()).unwrap_or(0));
        }
    }
    let ratio = if r.technologies.is_empty() {
        Fixed::permille(500)
    } else {
        Fixed::from_fraction(world.civ(city.owner).map_or(0, |c| c.techs.len()) as i64, r.technologies.len() as i64)
    };
    strength += (ratio * Fixed::permille(5500)).pow(Fixed::permille(2800)).unwrap_or(Fixed::ZERO);
    if let Some((_, garrison)) = world
        .units
        .iter()
        .find(|(_, u)| u.owner == city.owner && u.position == city.centre && u.is_military(r) && !u.is_air(r))
    {
        strength += Fixed::whole(r.units[garrison.base].strength.unwrap_or(0))
            * Fixed::from_fraction(i64::from(garrison.health), 500);
    }
    let mut buildings: Fixed = city.built.iter().filter_map(|id| r.buildings[*id].city_strength).sum();
    let mut percent =
        if enemy.and_then(|id| world.units.get(id)).and_then(|u| world.civ(u.owner)).is_some_and(|c| c.is_barbarian) {
            (world.game_difficulty(r).map_or(Fixed::ZERO, |d| d.barbarian_bonus) * Fixed::whole(100)).floor() as i32
        } else {
            0
        };
    for rule in rules.into_iter().filter(|u| applies(u, &context).holds()) {
        let n = rule.params.first().and_then(|p| p.parse::<i32>().ok()).unwrap_or(0);
        match rule.unique_type() {
            Some(UniqueType::BetterDefensiveBuildings) => {
                buildings = buildings * (Fixed::ONE + Fixed::from_fraction(i64::from(n), 100))
            }
            Some(UniqueType::StrengthAmount) => strength += Fixed::whole(n),
            Some(UniqueType::StrengthForCities) => percent += n,
            _ => {}
        }
    }
    let base = (strength + buildings).round() as i32;
    let base = if attacking { (Fixed::whole(base) * Fixed::permille(750)).round() as i32 } else { base };
    (Fixed::whole(base) * (Fixed::ONE + Fixed::from_fraction(i64::from(percent), 100))).max(Fixed::ONE)
}

/// A city bombards an enemy unit within range (range 2).
/// Enemy units a city could bombard now: at war, within two tiles, in its line of fire and in sight.
/// The city also needs an attack left and no resistance, which [`city_bombard`]'s callers check.
pub fn bombard_targets(world: &World, r: &Ruleset, city_id: Id<crate::world::city::City>) -> Vec<Id<Unit>> {
    let Some(city) = world.cities.get(city_id) else { return Vec::new() };
    world
        .units
        .iter()
        .filter(|(id, u)| {
            u.owner != city.owner
                && world.diplomacy.at_war(city.owner, u.owner)
                && world.grid.distance(city.centre, u.position) <= 2
                && has_line_of_sight(&world.grid, r, city.centre, u.position, false)
                && crate::world::fog::can_see_unit(world, r, city.owner, *id)
        })
        .map(|(id, _)| id)
        .collect()
}

/// A bombard's damage to its target at the lowest and the highest roll, as [`city_bombard`] computes it.
pub fn preview_bombard(
    world: &World,
    r: &Ruleset,
    city_id: Id<crate::world::city::City>,
    target: Id<Unit>,
) -> Option<(i32, i32)> {
    let t = world.units.get(target)?;
    let health = i32::from(t.health);
    if t.is_civilian(r) {
        return Some((40.min(health), 40.min(health)));
    }
    let att = city_combat_strength(world, r, city_id, Some(target), true);
    let def = combat_strength(world, r, target, crate::world::air::Target::City(city_id), true)?;
    let ratio = strength_ratio(att, def);
    let damage = |roll| (damage_modifier_precise(ratio, false, roll).round().max(0) as i32).min(health);
    Some((damage(Fixed::ZERO), damage(Fixed::ONE)))
}

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

    let att = city_combat_strength(world, r, city_id, Some(target_unit_id), true);
    let def = combat_strength(world, r, target_unit_id, crate::world::air::Target::City(city_id), true)
        .ok_or(CombatRejection::NoSuchUnit)?;
    let ratio = strength_ratio(att, def);
    let randomness = combat_seed(world, city.centre, target.position).unit_interval();

    let to_target = if target.is_civilian(r) {
        40
    } else {
        damage_modifier_precise(ratio, false, randomness).round().max(0) as i32
    }
    .min(i32::from(target.health));
    let city_owner = city.owner;
    let target_died = to_target >= target.health as i32;
    if world.civ(target.owner).is_some_and(|c| c.is_barbarian) {
        let position = target.position;
        crate::world::barbarians::on_camp_attacked(world, position);
    }
    crate::world::military::award_xp(world, r, target_unit_id, 2, None, city_owner, true);
    if target_died {
        crate::world::military::reward_city_kill(world, r, city_id, target_unit_id);
    }
    if target_died {
        world.remove_unit(target_unit_id);
    } else if let Some(u) = world.units.get_mut(target_unit_id) {
        u.health = (u.health as i32 - to_target).max(1) as u8;
    }

    Ok(CombatResult { damage_to_defender: to_target, defender_died: target_died, ..CombatResult::default() })
}

/// Result of deploying a nuclear weapon.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NuclearStrikeResult {
    pub tiles_affected: Vec<TileId>,
    pub units_destroyed: Vec<Id<Unit>>,
    pub cities_damaged: Vec<Id<crate::world::city::City>>,
}

/// Compatibility entry point; all air rules are owned by `air`.
pub fn rebase_air_unit(world: &mut World, r: &Ruleset, unit: Id<Unit>, target: TileId) -> Result<(), CombatRejection> {
    crate::world::air::rebase(world, r, unit, target).map_err(Into::into)
}

/// Strike the tile's primary defender. Commands use an explicit target.
pub fn air_strike(
    world: &mut World,
    r: &Ruleset,
    unit: Id<Unit>,
    tile: TileId,
) -> Result<CombatResult, CombatRejection> {
    let owner = world.units.get(unit).ok_or(CombatRejection::NoSuchUnit)?.owner;
    let target = crate::world::air::target_at(world, owner, tile).ok_or(CombatRejection::NoSuchUnit)?;
    crate::world::air::strike(world, r, unit, target).map_err(Into::into)
}

pub fn nuclear_strike(
    world: &mut World,
    r: &Ruleset,
    unit: Id<Unit>,
    target: TileId,
) -> Result<NuclearStrikeResult, CombatRejection> {
    crate::world::nuclear::strike(world, r, unit, target)
}

/// Experience required to reach the given promotion level.
/// Level 1: 10 XP, Level 2: 30 XP, Level 3: 60 XP, Level 4: 100 XP.
pub fn xp_for_level(level: usize) -> u16 {
    (10_u64.saturating_mul(level as u64).saturating_mul(level as u64 + 1) / 2).min(u64::from(u16::MAX)) as u16
}

/// Whether the unit has earned enough experience to select a new promotion.
pub fn can_promote(unit: &Unit) -> bool {
    let next_level = u64::from(unit.promotions_earned) + 1;
    u64::from(unit.xp) >= 5 * next_level * (next_level + 1)
}

/// List of promotions available to this unit given its type, XP, and prerequisites.
pub fn available_promotions(r: &Ruleset, unit: &Unit) -> Vec<Id<Promotion>> {
    let Some(base) = r.units.get(unit.base) else { return Vec::new() };
    r.promotions
        .iter()
        .filter_map(|(id, promo)| {
            if unit.promotions.contains(&id) || base.promotions.contains(&promo.name) {
                return None;
            }
            if !promo.unit_types.iter().any(|ut| ut == &base.unit_type) {
                return None;
            }
            let prereqs_met = promo.prerequisites.is_empty()
                || promo.prerequisites.iter().any(|prereq_name| {
                    base.promotions.contains(prereq_name)
                        || r.promotions.id(prereq_name).is_some_and(|req_id| unit.promotions.contains(&req_id))
                });
            if !prereqs_met {
                return None;
            }
            Some(id)
        })
        .collect()
}

/// Choose and apply a promotion to a unit.
pub fn choose_promotion(unit: &mut Unit, r: &Ruleset, promo_id: Id<Promotion>) -> Result<(), &'static str> {
    let promo = r.promotions.get(promo_id).ok_or("No such promotion")?;
    let free = promo.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::FreePromotion));
    if !free && !can_promote(unit) {
        return Err("Not enough XP for promotion");
    }
    let available = available_promotions(r, unit);
    if !available.contains(&promo_id) {
        return Err("Promotion not available");
    }
    if !free {
        unit.promotions_earned = unit.promotions_earned.saturating_add(1);
    }
    if !promo.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::SkipPromotion)) {
        unit.promotions.insert(promo_id);
    }
    for unique in &promo.uniques {
        if unique.unique_type() == Some(UniqueType::OneTimeUnitHeal)
            && unique.params.first().is_some_and(|p| p == "This Unit")
        {
            let amount = unique.params.get(1).and_then(|p| p.parse::<u16>().ok()).unwrap_or(0);
            unit.health = (u16::from(unit.health).saturating_add(amount)).min(100) as u8;
        }
    }
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
    if !unit.can_move() {
        return false;
    }
    let pos = unit.position;
    world.grid.unpillaged_improvement(pos).is_some() || (world.grid.unpillaged_road(pos) != RoadLevel::None)
}

fn pillage_target(
    world: &World,
    r: &Ruleset,
    unit_id: Id<Unit>,
) -> Option<(Id<crate::ruleset::TileImprovement>, bool)> {
    let unit = world.units.get(unit_id)?;
    if !unit.can_move()
        || unit.is_civilian(r)
        || unit.is_air(r)
        || unit.transport.is_some()
        || world.cities.iter().any(|(_, c)| c.centre == unit.position)
        || world.grid.owner[unit.position.0 as usize].is_some_and(|owner| !world.diplomacy.at_war(owner, unit.owner))
    {
        return None;
    }
    let context = GameContext::new(r).with_world(world).for_civ(unit.owner).for_unit(unit_id).on_tile(unit.position);
    if unit
        .all_uniques(r)
        .chain(world.civ(unit.owner).into_iter().flat_map(|c| c.uniques.all()))
        .any(|u| u.unique_type() == Some(UniqueType::CannotPillage) && applies(u, &context).holds())
    {
        return None;
    }
    if let Some(id) = world.grid.unpillaged_improvement(unit.position) {
        if !r.improvements[id]
            .uniques
            .iter()
            .any(|u| u.unique_type() == Some(UniqueType::Unpillagable) && applies(u, &context).holds())
        {
            return Some((id, true));
        }
    }
    let name = match world.grid.unpillaged_road(unit.position) {
        RoadLevel::Road => "Road",
        RoadLevel::Railroad => "Railroad",
        RoadLevel::None => return None,
    };
    r.improvements.id(name).map(|id| (id, false))
}

pub fn can_pillage_in(world: &World, r: &Ruleset, unit: Id<Unit>) -> bool {
    pillage_target(world, r, unit).is_some()
}

/// Improvement-defined loot is deterministic. Roads yield no healing, and
/// free-pillage abilities preserve the unit's movement and resting status.
pub fn pillage(world: &mut World, r: &Ruleset, unit_id: Id<Unit>) -> Option<PillageEvent> {
    let (target, improvement) = pillage_target(world, r, unit_id)?;
    let unit = world.units.get(unit_id)?;
    let (pos, owner) = (unit.position, unit.owner);
    let context = GameContext::new(r).with_world(world).for_civ(owner).for_unit(unit_id).on_tile(pos);
    let mut random = Rng::from_parts(&[0xF1_11A6E, u64::from(world.turn), u64::from(pos.0), unit_id.index() as u64]);
    let mut loot = crate::stats::Stats::ZERO;
    for unique in &r.improvements[target].uniques {
        if !applies(unique, &context).holds() {
            continue;
        }
        let stats = unique.params.first().and_then(|p| crate::stats::Stats::parse(p)).unwrap_or_default();
        match unique.unique_type() {
            Some(UniqueType::PillageYieldFixed) => loot += stats,
            Some(UniqueType::PillageYieldRandom) => {
                for stat in crate::stats::Stat::ALL {
                    let max = stats[stat].floor().max(0);
                    if max > 0 {
                        loot[stat] += Fixed::whole((random.range(0, max - 1) + random.range(0, max - 1)) as i32);
                    }
                }
            }
            _ => {}
        }
    }
    let mut free = false;
    let mut heal = Fixed::whole(25);
    for unique in unit
        .all_uniques(r)
        .chain(world.civ(owner).into_iter().flat_map(|c| c.uniques.all()))
        .filter(|u| applies(u, &context).holds())
    {
        let factor = Fixed::ONE
            + unique
                .params
                .first()
                .and_then(|p| Fixed::parse(p))
                .unwrap_or(Fixed::ZERO)
                .checked_div(Fixed::whole(100))
                .unwrap_or(Fixed::ZERO);
        match unique.unique_type() {
            Some(UniqueType::NoMovementToPillage) => free = true,
            Some(UniqueType::PercentHealthFromPillaging) => heal = heal * factor,
            Some(UniqueType::PercentYieldFromPillaging) => {
                for stat in crate::stats::Stat::ALL {
                    loot[stat] = loot[stat] * factor;
                }
            }
            _ => {}
        }
    }
    let destroyed = r.improvements[target]
        .uniques
        .iter()
        .any(|u| u.unique_type() == Some(UniqueType::DestroyedWhenPillaged) && applies(u, &context).holds());
    if improvement {
        if destroyed {
            world.grid.improvement[pos.0 as usize] = None;
            world.grid.improvement_pillaged[pos.0 as usize] = false;
        } else {
            world.grid.pillage_improvement(pos);
        }
    } else {
        world.grid.pillage_road(pos);
    }
    if !free {
        let unit = world.units.get_mut(unit_id)?;
        unit.movement_left = (unit.movement_left - Fixed::ONE).max(Fixed::ZERO);
        unit.fortification_turns = 0;
        unit.moved_this_turn = true;
    }
    let healed = if improvement { crate::world::healing::heal(world, r, unit_id, heal.floor() as i32) } else { 0 };
    let gold_pillaged = (loot.gold.raw() / 1000) as i32;
    crate::world::military::credit_yields(world, owner, pos, loot);
    Some(PillageEvent { unit: unit_id, tile: pos, healed, gold_pillaged })
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

impl std::fmt::Display for UpgradeRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            UpgradeRejection::NoSuchUnit => "no such unit",
            UpgradeRejection::CannotUpgrade => "the unit has nothing to upgrade to",
            UpgradeRejection::TechNotResearched => "the upgrade's tech is not researched",
            UpgradeRejection::MissingResources => "the upgrade needs resources the civ lacks",
            UpgradeRejection::NotInFriendlyTerritory => "units upgrade only in friendly territory",
            UpgradeRejection::NotEnoughGold => "not enough gold",
        })
    }
}

impl std::error::Error for UpgradeRejection {}

/// Gold cost to upgrade from `current` to `target`.
pub fn upgrade_cost(r: &Ruleset, current: Id<BaseUnit>, target: Id<BaseUnit>) -> Option<i32> {
    let old_cost = r.units.get(current)?.cost.unwrap_or(0);
    let new_cost = r.units.get(target)?.cost.unwrap_or(0);
    Some((new_cost - old_cost).max(0) * 2 + 10)
}

/// Check if a unit is eligible for an upgrade.
pub fn can_upgrade(world: &World, r: &Ruleset, unit_id: Id<Unit>) -> Result<(Id<BaseUnit>, i32), UpgradeRejection> {
    can_upgrade_with(world, r, unit_id, Fixed::ONE)
}
/// Own-unit upgrade target and exact price, independent of current action
/// legality, technology, resource supply and affordability.
pub fn upgrade_quote(world: &World, r: &Ruleset, unit_id: Id<Unit>, speed: Fixed) -> Option<(Id<BaseUnit>, i32)> {
    let unit = world.units.get(unit_id)?;
    let base = r.units.get(unit.base)?;
    let target_id = crate::world::effects::equivalent_unit(world, r, unit.owner, r.unit(base.upgrades_to.as_ref()?)?);
    let civ = world.civ(unit.owner)?;
    let context = GameContext::new(r).with_world(world).for_civ(unit.owner).for_unit(unit_id).on_tile(unit.position);
    let mut price = Fixed::whole(upgrade_cost(r, unit.base, target_id).unwrap_or(10));
    for rule in civ.uniques.matching(UniqueType::UnitUpgradeCost).filter(|u| applies(u, &context).holds()) {
        price = price
            * (Fixed::ONE + Fixed::from_fraction(rule.params.first().and_then(|p| p.parse().ok()).unwrap_or(0), 100));
    }
    let cost = (((price * speed).floor() / 5) * 5).max(0) as i32;
    Some((target_id, cost))
}

pub fn can_upgrade_with(
    world: &World,
    r: &Ruleset,
    unit_id: Id<Unit>,
    speed: Fixed,
) -> Result<(Id<BaseUnit>, i32), UpgradeRejection> {
    let unit = world.units.get(unit_id).ok_or(UpgradeRejection::NoSuchUnit)?;
    let (target_id, cost) = upgrade_quote(world, r, unit_id, speed).ok_or(UpgradeRejection::CannotUpgrade)?;
    if !unit.can_move() || unit.embarked {
        return Err(UpgradeRejection::CannotUpgrade);
    }
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

    if civ.gold < cost {
        return Err(UpgradeRejection::NotEnoughGold);
    }
    if !crate::world::resources::can_supply_unit(world, r, unit.owner, target_id, Some(unit_id)) {
        return Err(UpgradeRejection::MissingResources);
    }

    Ok((target_id, cost))
}

/// Upgrade a unit in-place to its modern equivalent.
pub fn upgrade_unit(world: &mut World, r: &Ruleset, unit_id: Id<Unit>) -> Result<Id<BaseUnit>, UpgradeRejection> {
    upgrade_unit_with(world, r, unit_id, Fixed::ONE)
}
pub fn upgrade_unit_with(
    world: &mut World,
    r: &Ruleset,
    unit_id: Id<Unit>,
    speed: Fixed,
) -> Result<Id<BaseUnit>, UpgradeRejection> {
    let (target_id, cost) = can_upgrade_with(world, r, unit_id, speed)?;
    let unit = world.units.get(unit_id).ok_or(UpgradeRejection::NoSuchUnit)?;
    let owner = unit.owner;

    if let Some(c) = world.civ_mut(owner) {
        c.gold -= cost;
    }
    if let Some(u) = world.units.get_mut(unit_id) {
        for name in &r.units[u.base].promotions {
            if let Some(p) = r.promotions.id(name) {
                u.promotions.insert(p);
            }
        }
        for name in &r.units[target_id].promotions {
            if let Some(p) = r.promotions.id(name) {
                u.promotions.insert(p);
            }
        }
        u.base = target_id;
        u.set_up = false;
        u.fortification_turns = 0;
        u.moved_this_turn = true;
        u.movement_left = Fixed::ZERO;
    }
    let stock = crate::world::resources::available_stock(world, r, owner);
    if let Some(c) = world.civ_mut(owner) {
        c.resources = stock;
    }
    Ok(target_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::entities::Civilization;
    use crate::world::grid::{CivId, TileGrid};

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("rules/gods-and-kings").expect("readable").expect("loads")
    }

    fn world(r: &Ruleset) -> World {
        let g = TileGrid::new(12, 12, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        World::new(g, vec![Civilization::new(CivId(0), nation, false), Civilization::new(CivId(1), nation, true)])
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
            assert!(d >= Fixed::whole(24) && d <= Fixed::whole(36), "even fight rolled {d}, expected 24..36");
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
        assert!(dealt.checked_div(taken).unwrap() > Fixed::whole(2), "the curve is steeper than linear");
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
        assert_eq!(on_hill, open * Fixed::permille(1_250));

        // Fortifying adds another 20, additively — 45% total, not 50%.
        w.units.get_mut(id).unwrap().fortified = true;
        crate::world::healing::recover(&mut w, &r);
        let dug_in = defending_strength(&w, &r, id).unwrap();
        assert_eq!(dug_in, open * Fixed::permille(1_450), "additive, not compounding");
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
        assert_eq!(defending_strength(&w, &r, id).unwrap(), base * Fixed::permille(850));
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
        Ruleset::load_dir("rules/gods-and-kings").expect("readable").expect("loads")
    }

    /// Meeting someone makes peace the default, and peace forbids attacking.
    /// Two civs who have never met may still fight — which is how a first
    /// contact goes badly.
    #[test]
    fn peace_forbids_an_attack_and_war_permits_it() {
        let r = ruleset();
        let g = TileGrid::new(8, 8, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        let mut w =
            World::new(g, vec![Civilization::new(CivId(0), nation, false), Civilization::new(CivId(1), nation, false)]);
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
        Ruleset::load_dir("rules/gods-and-kings").expect("readable").expect("loads")
    }

    fn besieged(r: &Ruleset) -> (World, Id<Unit>, Id<City>) {
        let g = TileGrid::new(12, 12, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        let mut w =
            World::new(g, vec![Civilization::new(CivId(0), nation, true), Civilization::new(CivId(1), nation, true)]);
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
        while w.cities.get(city).is_some_and(|c| c.health > 1) && rounds < 100 {
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
        assert_eq!(taken.population, 4, "six loses one, and one more per four (-1 - pop / 4)");
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
        assert!(
            crate::world::victory::evaluate(&crate::world::victory::Milestone::CaptureAllCapitals, &w, &r, CivId(0))
                == crate::world::conditionals::Verdict::False,
            "the attacker has never founded its own original capital"
        );
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
        assert_eq!(c.food_stored, 0, "cut to size five and still holding a size-eight granary");
        assert_eq!(c.population, 5);
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
        Ruleset::load_dir("rules/gods-and-kings").expect("readable").expect("loads")
    }

    fn world(r: &Ruleset) -> World {
        let g = TileGrid::new(12, 12, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        World::new(g, vec![Civilization::new(CivId(0), nation, false), Civilization::new(CivId(1), nation, false)])
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
        assert_eq!(resolve_ranged_attack(&mut w, &r, archer, far_tile), Err(CombatRejection::OutOfRange));
    }

    #[test]
    fn line_of_sight_blocking() {
        let r = ruleset();
        let mut w = world(&r);
        w.diplomacy.declare_war(CivId(0), CivId(1));

        let a_tile = w.grid.id_at(4, 4).unwrap();
        let mid_tile = w.grid.id_at(5, 4).unwrap();
        let target_tile = w.grid.id_at(6, 4).unwrap();

        let archer = unit(&mut w, &r, "Archer", CivId(0), a_tile);
        let _defender = unit(&mut w, &r, "Warrior", CivId(1), target_tile);

        // Put a mountain in the middle
        let mountain = r.terrain("Mountain").unwrap();
        w.grid.terrain[mid_tile.0 as usize] = mountain;

        assert_eq!(resolve_ranged_attack(&mut w, &r, archer, target_tile), Err(CombatRejection::TargetBlockedByLOS));

        // Shooter on hill can fire over a hill/forest, but not over impassable mountain
        let hill = r.terrain("Hill").unwrap();
        w.grid.terrain[mid_tile.0 as usize] = r.terrain("Grassland").unwrap();
        w.grid.features[mid_tile.0 as usize].push(hill);

        // Archer on flat land cannot fire over hill
        assert_eq!(resolve_ranged_attack(&mut w, &r, archer, target_tile), Err(CombatRejection::TargetBlockedByLOS));

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
        assert_eq!(
            garr_def,
            base_def + Fixed::whole(2),
            "Warrior contributes 20% of its 8 strength, rounded with the city base"
        );

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

    /// The capture is a fact of the result: who was taken, where, from whom, and what it became.
    #[test]
    fn a_capture_names_the_civilian_and_what_it_becomes() {
        let r = ruleset();
        let mut w = world(&r);
        let a_tile = w.grid.id_at(5, 5).unwrap();
        let (worker_tile, settler_tile) =
            (w.grid.neighbours(a_tile).next().unwrap(), w.grid.neighbours(a_tile).nth(1).unwrap());
        w.diplomacy.declare_war(CivId(0), CivId(1));
        let warrior = unit(&mut w, &r, "Warrior", CivId(0), a_tile);
        let worker = unit(&mut w, &r, "Worker", CivId(1), worker_tile);
        let settler = unit(&mut w, &r, "Settler", CivId(1), settler_tile);
        let taken = |unit, at, becomes| Some(Capture { unit, at, from_owner: CivId(1), to_owner: CivId(0), becomes });
        let res = resolve_attack(&mut w, &r, warrior, worker).unwrap();
        assert_eq!(res.captured, taken(worker, worker_tile, None));
        assert_eq!(res.tags, BattleTags::default(), "no strength was reckoned");
        w.units.get_mut(warrior).unwrap().refresh(2);
        let res = resolve_attack(&mut w, &r, warrior, settler).unwrap();
        assert_eq!(res.captured, taken(settler, settler_tile, r.unit("Worker")));
    }

    /// The fight carries what set each side's strength, without changing it.
    #[test]
    fn a_fight_lists_each_sides_modifiers() {
        let r = ruleset();
        let mut w = world(&r);
        w.diplomacy.declare_war(CivId(0), CivId(1));
        let a_tile = w.grid.id_at(5, 5).unwrap();
        let d_tile = w.grid.neighbours(a_tile).next().unwrap();
        let a = unit(&mut w, &r, "Warrior", CivId(0), a_tile);
        let d = unit(&mut w, &r, "Warrior", CivId(1), d_tile);
        let defender = w.units.get_mut(d).unwrap();
        (defender.fortified, defender.fortification_turns) = (true, 1);
        let expected = (
            combat_breakdown(&w, &r, a, crate::world::air::Target::Unit(d), false, 0).unwrap().1.modifiers().to_vec(),
            combat_breakdown(&w, &r, d, crate::world::air::Target::Unit(a), true, 0).unwrap().1.modifiers().to_vec(),
        );
        let res = resolve_attack(&mut w, &r, a, d).unwrap();
        assert!(res.tags.defender_mods.iter().any(|m| m.source.name == "Fortified" && m.amount == Fixed::whole(20)));
        assert_eq!((res.tags.attacker_mods, res.tags.defender_mods), expected);
    }

    /// Flankers are the units the flanking bonus counted: beside the target, not the attacker itself.
    #[test]
    fn a_fight_names_its_flankers() {
        let r = ruleset();
        let mut w = world(&r);
        w.diplomacy.declare_war(CivId(0), CivId(1));
        let a_tile = w.grid.id_at(5, 5).unwrap();
        let d_tile = w.grid.neighbours(a_tile).next().unwrap();
        let beside: Vec<_> =
            w.grid.neighbours(d_tile).filter(|t| *t != a_tile && w.grid.distance(*t, a_tile) > 1).take(2).collect();
        let a = unit(&mut w, &r, "Warrior", CivId(0), a_tile);
        let d = unit(&mut w, &r, "Warrior", CivId(1), d_tile);
        let flankers: Vec<_> = beside.iter().map(|t| unit(&mut w, &r, "Spearman", CivId(0), *t)).collect();
        // An archer beside the target does not flank.
        let archer_tile = w.grid.neighbours(d_tile).find(|t| *t != a_tile && !beside.contains(t)).unwrap();
        unit(&mut w, &r, "Archer", CivId(0), archer_tile);
        let res = resolve_attack(&mut w, &r, a, d).unwrap();
        let mut named = res.tags.flankers.clone();
        named.sort();
        let mut expected = flankers.clone();
        expected.sort();
        assert_eq!(named, expected);
        assert!(res.tags.attacker_mods.iter().any(|m| m.source.name == "Flanking" && m.amount == Fixed::whole(20)));
        assert_eq!(res.tags.general, None);
    }

    /// The general is named only when its aura counted, and it does not change the strength it lends.
    #[test]
    fn a_fight_names_the_general_whose_aura_counted() {
        let r = ruleset();
        let mut w = world(&r);
        w.diplomacy.declare_war(CivId(0), CivId(1));
        let a_tile = w.grid.id_at(5, 5).unwrap();
        let d_tile = w.grid.neighbours(a_tile).next().unwrap();
        let a = unit(&mut w, &r, "Warrior", CivId(0), a_tile);
        let d = unit(&mut w, &r, "Warrior", CivId(1), d_tile);
        let far = w.grid.id_at(11, 11).unwrap();
        let general = unit(&mut w, &r, "Great General", CivId(0), far);
        let target = crate::world::air::Target::Unit(d);
        assert_eq!(combat_parts(&w, &r, a, target, false, 0).unwrap().general, None, "out of range");
        let behind = w.grid.neighbours(a_tile).find(|t| w.grid.distance(*t, d_tile) == 2).unwrap();
        w.units.get_mut(general).unwrap().position = behind;
        let parts = combat_parts(&w, &r, a, target, false, 0).unwrap();
        assert_eq!(parts.general, Some(general));
        assert_eq!(parts.strength(), combat_strength(&w, &r, a, target, false).unwrap());
        let res = resolve_attack(&mut w, &r, a, d).unwrap();
        assert_eq!(res.tags.general, Some(general));
        assert!(res.tags.attacker_mods.iter().any(|m| m.source.name == "General aura"));
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
        assert!((0..=34).contains(&pillage_res.gold_pillaged), "Farm yields two rolls below 18");
        assert_eq!(w.civ(CivId(0)).unwrap().gold, initial_gold + pillage_res.gold_pillaged);
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
        assert_eq!(can_upgrade(&w, &r, warrior), Err(UpgradeRejection::MissingResources));
        let iron = r.resources.id("Iron").unwrap();
        w.civ_mut(CivId(0)).unwrap().techs.extend(r.technologies.ids());
        w.grid.resource[tile.0 as usize] = Some(iron);
        w.grid.resource_amount[tile.0 as usize] = 1;
        w.grid.improvement[tile.0 as usize] = r.improvements.id("Mine");
        let (up_base, cost) = can_upgrade(&w, &r, warrior).expect("can upgrade");
        assert_eq!(up_base, swordsman);
        assert_eq!(cost, 80);

        let res = upgrade_unit(&mut w, &r, warrior).expect("upgrade completes");
        assert_eq!(res, swordsman);
        assert_eq!(w.units.get(warrior).unwrap().base, swordsman);
        assert_eq!(w.civ(CivId(0)).unwrap().gold, 20);
    }

    #[test]
    fn upgrades_credit_the_old_units_resources_but_cannot_overcommit() {
        let mut r = ruleset();
        let mut w = world(&r);
        let tile = w.grid.id_at(5, 5).unwrap();
        let iron = r.resources.id("Iron").unwrap();
        w.grid.owner[tile.0 as usize] = Some(CivId(0));
        w.grid.resource[tile.0 as usize] = Some(iron);
        w.grid.resource_amount[tile.0 as usize] = 1;
        w.grid.improvement[tile.0 as usize] = r.improvements.id("Mine");
        w.civ_mut(CivId(0)).unwrap().techs.extend(r.technologies.ids());
        w.civ_mut(CivId(0)).unwrap().gold = 10_000;
        let old = unit(&mut w, &r, "Swordsman", CivId(0), tile);
        let (target, _) = can_upgrade(&w, &r, old).expect("same Iron is reused");
        r.units.get_mut(target).unwrap().uniques.push(crate::Unique::parse("Consumes [1] [Iron]"));
        let before = w.clone();
        assert_eq!(upgrade_unit(&mut w, &r, old), Err(UpgradeRejection::MissingResources));
        assert_eq!(w, before);
        w.grid.resource_amount[tile.0 as usize] = 2;
        assert!(upgrade_unit(&mut w, &r, old).is_ok());
        assert_eq!(w.civ(CivId(0)).unwrap().resources.get(&iron), Some(&0));
    }

    #[test]
    fn nuclear_strike_mechanics() {
        let mut r = ruleset();
        let atomic = r.unit("Atomic Bomb").unwrap();
        r.units.get_mut(atomic).unwrap().required_resource = None;
        let mut w = world(&r);
        let nuke_tile = w.grid.id_at(5, 5).unwrap();
        let target_tile = w.grid.id_at(5, 8).unwrap();
        assert_eq!(w.grid.distance(nuke_tile, target_tile), 3);

        let abomb = unit(&mut w, &r, "Atomic Bomb", CivId(0), nuke_tile);

        let victim = unit(&mut w, &r, "Warrior", CivId(1), target_tile);
        let city = w.found_city_in(&r, City::found("Kyoto", CivId(1), target_tile));
        w.cities.get_mut(city).unwrap().population = 10;
        w.cities.get_mut(city).unwrap().health = 200;

        w.visibility.explore(CivId(0), target_tile);
        w.diplomacy.meet(CivId(0), CivId(1));
        let res = nuclear_strike(&mut w, &r, abomb, target_tile).expect("nuclear strike succeeds");
        assert!(res.units_destroyed.contains(&victim));
        assert!(res.cities_damaged.contains(&city));
        assert!(w.units.get(victim).is_none(), "victim unit destroyed");
        assert!(w.units.get(abomb).is_none(), "atomic bomb self-destructed");
        assert!((4..=7).contains(&w.cities.get(city).unwrap().population), "atomic population loss is a 30–68% roll");
        assert_eq!(w.cities.get(city).unwrap().health, 100, "city health halved");
        let fallout = r.terrain("Fallout").unwrap();
        assert!(
            !w.grid.features[target_tile.0 as usize].contains(&fallout),
            "surviving city centers never receive fallout"
        );
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
