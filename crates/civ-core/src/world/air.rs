//! Aircraft basing, transport, combat missions and paradrops.
//!
//! Missions validate completely before spending an action. Randomness comes
//! from stable game state; carrier ownership is explicit rather than inferred
//! from whichever units happen to share a tile.

use crate::arena::Id;
use crate::rng::Rng;
use crate::ruleset::Ruleset;
use crate::stats::{Fixed, Precise};
use crate::world::combat::{self, CombatRejection, CombatResult};
use crate::world::conditionals::{applies, matches_tile, CombatRole, GameContext, Opponent};
use crate::world::{City, CivId, TileId, Unit, World};
use crate::{Unique, UniqueType};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AirRejection {
    NoSuchUnit,
    InvalidTarget,
    NoMovement,
    NotAllowed,
    NotAtWar,
    OutOfRange,
    NoBase,
    NotVisible,
}

impl From<AirRejection> for CombatRejection {
    fn from(reason: AirRejection) -> Self {
        match reason {
            AirRejection::NoSuchUnit | AirRejection::InvalidTarget => Self::NoSuchUnit,
            AirRejection::NoMovement => Self::NoMovement,
            AirRejection::NotAtWar => Self::NotAtWar,
            AirRejection::OutOfRange => Self::OutOfRange,
            AirRejection::NoBase => Self::NotAdjacent,
            AirRejection::NotAllowed | AirRejection::NotVisible => Self::NoRangedAttack,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Unit(Id<Unit>),
    City(Id<City>),
}

impl Target {
    fn facts(self, world: &World) -> Option<(CivId, TileId, i32)> {
        match self {
            Self::Unit(id) => world.units.get(id).map(|u| (u.owner, u.position, i32::from(u.health))),
            Self::City(id) => world.cities.get(id).map(|c| (c.owner, c.centre, c.health)),
        }
    }
    fn opponent(self) -> Opponent {
        match self {
            Self::Unit(id) => Opponent::Unit(id),
            Self::City(_) => Opponent::City,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AirBase {
    City(Id<City>),
    Carrier(Id<Unit>),
}

impl AirBase {
    pub fn transport(self) -> Option<Id<Unit>> {
        match self {
            Self::City(_) => None,
            Self::Carrier(id) => Some(id),
        }
    }
}

fn uniques<'a>(world: &'a World, r: &'a Ruleset, unit: &'a Unit, civ: bool) -> impl Iterator<Item = &'a Unique> {
    unit.all_uniques(r).chain(world.civ(unit.owner).filter(|_| civ).into_iter().flat_map(|c| c.uniques.all()))
}

fn context<'a>(world: &'a World, r: &'a Ruleset, id: Id<Unit>) -> GameContext<'a> {
    let unit = world.units.get(id).expect("validated unit");
    GameContext::new(r).with_world(world).for_civ(unit.owner).for_unit(id).on_tile(unit.position)
}

fn matching<'a>(
    world: &'a World,
    r: &'a Ruleset,
    id: Id<Unit>,
    kind: UniqueType,
    ctx: &GameContext<'_>,
    civ: bool,
) -> Vec<&'a Unique> {
    let Some(unit) = world.units.get(id) else { return Vec::new() };
    uniques(world, r, unit, civ).filter(|u| u.unique_type() == Some(kind) && applies(u, ctx).holds()).collect()
}

fn number(u: &Unique, index: usize) -> i32 {
    u.params.get(index).and_then(|p| p.parse().ok()).unwrap_or(0)
}

fn sum(world: &World, r: &Ruleset, id: Id<Unit>, kind: UniqueType, ctx: &GameContext<'_>, civ: bool) -> i32 {
    matching(world, r, id, kind, ctx, civ).into_iter().map(|u| number(u, 0)).sum()
}

fn has(world: &World, r: &Ruleset, id: Id<Unit>, kind: UniqueType, ctx: &GameContext<'_>, civ: bool) -> bool {
    !matching(world, r, id, kind, ctx, civ).is_empty()
}

pub fn attack_range(world: &World, r: &Ruleset, id: Id<Unit>) -> i32 {
    let Some(unit) = world.units.get(id) else { return 0 };
    (r.units.get(unit.base).and_then(|b| b.range).unwrap_or(if unit.is_ranged(r) { 2 } else { 1 })
        + sum(world, r, id, UniqueType::Range, &context(world, r, id), true))
    .max(0)
}

pub fn max_attacks(world: &World, r: &Ruleset, id: Id<Unit>) -> u8 {
    if world.units.get(id).is_none() {
        return 0;
    }
    (1 + sum(world, r, id, UniqueType::AdditionalAttacks, &context(world, r, id), true)).clamp(0, 255) as u8
}

/// G&K's default six slots, plus matching local/empire capacity modifiers.
pub fn city_capacity(world: &World, r: &Ruleset, city: Id<City>) -> Option<usize> {
    let c = world.cities.get(city)?;
    let owner = world.civ(c.owner)?;
    let ctx = GameContext::new(r).with_world(world).for_civ(c.owner).in_city(city).on_tile(c.centre);
    let extra: i64 = c
        .uniques
        .matching(UniqueType::CarryExtraAirUnits)
        .chain(owner.uniques.matching(UniqueType::CarryExtraAirUnits))
        .filter(|u| u.params.get(1).is_some_and(|f| f == "Air") && applies(u, &ctx).holds())
        .map(|u| i64::from(number(u, 0)))
        .sum();
    Some((6 + extra).clamp(0, i64::from(u32::MAX)) as usize)
}

/// Match passengers to capacity slots, including overlapping cargo filters.
/// Existing passengers count against their explicit carrier only.
pub fn can_carry(world: &World, r: &Ruleset, carrier: Id<Unit>, passenger: &Unit, exclude: Option<Id<Unit>>) -> bool {
    let Some(ship) = world.units.get(carrier) else { return false };
    if !passenger.is_air(r) || ship.is_air(r) || ship.owner != passenger.owner {
        return false;
    }
    let ctx = context(world, r, carrier);
    let rules = matching(world, r, carrier, UniqueType::CarryAirUnits, &ctx, false);
    if !rules
        .iter()
        .any(|u| u.compiled(r).filter(1).is_some_and(|f| crate::world::filters::matches_unit(r, passenger, f).holds()))
    {
        return false;
    }
    let mut passenger_ctx = GameContext::new(r).with_world(world).for_civ(passenger.owner).on_tile(passenger.position);
    if let Some(id) = exclude {
        passenger_ctx = passenger_ctx.for_unit(id);
    }
    if uniques(world, r, passenger, false).any(|u| {
        u.unique_type() == Some(UniqueType::CannotBeCarriedBy)
            && applies(u, &passenger_ctx).holds()
            && u.compiled(r).filter(0).is_some_and(|f| crate::world::filters::matches_unit(r, ship, f).holds())
    }) {
        return false;
    }
    let mut passengers: Vec<_> = world
        .units
        .iter()
        .filter(|(id, u)| Some(*id) != exclude && u.transport == Some(carrier))
        .map(|(_, u)| u)
        .collect();
    passengers.push(passenger);
    let extra = matching(world, r, carrier, UniqueType::CarryExtraAirUnits, &ctx, false);
    let slots: Vec<_> = rules
        .into_iter()
        .chain(extra)
        .flat_map(|u| {
            std::iter::repeat_n(
                u.params.get(1).map(String::as_str).unwrap_or(""),
                (number(u, 0).max(0) as usize).min(passengers.len()),
            )
        })
        .collect();
    if slots.len() < passengers.len() {
        return false;
    }
    fn assign(
        n: usize,
        passengers: &[&Unit],
        slots: &[&str],
        assigned: &mut [Option<usize>],
        visited: &mut [bool],
        r: &Ruleset,
    ) -> bool {
        for (slot, filter) in slots.iter().enumerate() {
            if visited[slot] || !crate::world::filters::unit_matches(r, passengers[n], filter).holds() {
                continue;
            }
            visited[slot] = true;
            if assigned[slot].is_none_or(|old| assign(old, passengers, slots, assigned, visited, r)) {
                assigned[slot] = Some(n);
                return true;
            }
        }
        false
    }
    let mut assigned = vec![None; slots.len()];
    (0..passengers.len()).all(|n| assign(n, &passengers, &slots, &mut assigned, &mut vec![false; slots.len()], r))
}

pub fn base_for(world: &World, r: &Ruleset, unit: &Unit, tile: TileId, exclude: Option<Id<Unit>>) -> Option<AirBase> {
    if !unit.is_air(r) || !world.grid.contains(tile) {
        return None;
    }
    if let Some((id, _)) = world.cities.iter().find(|(_, c)| c.owner == unit.owner && c.centre == tile) {
        let used = world
            .units
            .iter()
            .filter(|(id, u)| Some(*id) != exclude && u.position == tile && u.is_air(r) && u.transport.is_none())
            .count();
        if used < city_capacity(world, r, id)? {
            return Some(AirBase::City(id));
        }
    }
    world
        .units
        .iter()
        .filter(|(_, u)| u.position == tile)
        .find(|(id, _)| Some(*id) != exclude && can_carry(world, r, *id, unit, exclude))
        .map(|(id, _)| AirBase::Carrier(id))
}

pub fn can_rebase(world: &World, r: &Ruleset, id: Id<Unit>, tile: TileId) -> Result<AirBase, AirRejection> {
    let unit = world.units.get(id).ok_or(AirRejection::NoSuchUnit)?;
    if !unit.is_air(r) {
        return Err(AirRejection::NotAllowed);
    }
    if !unit.can_move() {
        return Err(AirRejection::NoMovement);
    }
    if !world.grid.contains(tile) || tile == unit.position {
        return Err(AirRejection::InvalidTarget);
    }
    if world.grid.distance(unit.position, tile) > attack_range(world, r, id).saturating_mul(2) as u32 {
        return Err(AirRejection::OutOfRange);
    }
    base_for(world, r, unit, tile, Some(id)).ok_or(AirRejection::NoBase)
}

pub fn rebase(world: &mut World, r: &Ruleset, id: Id<Unit>, tile: TileId) -> Result<(), AirRejection> {
    let base = can_rebase(world, r, id, tile)?;
    let unit = world.units.get_mut(id).expect("validated");
    unit.position = tile;
    unit.transport = base.transport();
    unit.movement_left = Fixed::ZERO;
    unit.moved_this_turn = true;
    Ok(())
}

/// Passive carriage does not spend the passenger's own action or healing.
pub fn move_payload(world: &mut World, carrier: Id<Unit>, tile: TileId) {
    for (_, unit) in world.units.iter_mut() {
        if unit.transport == Some(carrier) {
            unit.position = tile;
        }
    }
}

/// Evacuate a disbanded carrier. Returned passengers could not escape and
/// remain aboard, allowing the caller to account for their disband refunds
/// before `World::remove_unit` removes the carrier and remaining payload.
pub fn evacuate_payload_before_disband(world: &mut World, r: &Ruleset, carrier: Id<Unit>) -> Vec<Id<Unit>> {
    let passengers: Vec<_> =
        world.units.iter().filter(|(_, u)| u.transport == Some(carrier)).map(|(id, _)| id).collect();
    let mut stranded = Vec::new();
    for id in passengers {
        let unit = world.units.get(id).expect("listed passenger");
        let position = unit.position;
        if matches!(base_for(world, r, unit, position, Some(id)), Some(AirBase::City(_))) {
            world.units.get_mut(id).expect("listed passenger").transport = None;
            continue;
        }
        let mut candidates: Vec<_> = world.grid.within(position, attack_range(world, r, id).saturating_mul(2) as u32);
        candidates.sort_by_key(|tile| (world.grid.distance(position, *tile), *tile));
        let destination =
            candidates.into_iter().find(|tile| *tile != position && can_rebase(world, r, id, *tile).is_ok());
        if let Some(tile) = destination {
            rebase(world, r, id, tile).expect("validated evacuation");
        } else {
            stranded.push(id);
        }
    }
    stranded
}

pub fn validate_transports(world: &World, r: &Ruleset) -> Result<(), String> {
    for (id, unit) in world.units.iter() {
        let Some(carrier) = unit.transport else { continue };
        let ship = world.units.get(carrier).ok_or("aircraft references a missing carrier")?;
        if id == carrier || ship.position != unit.position || !can_carry(world, r, carrier, unit, Some(id)) {
            return Err("invalid aircraft carrier ownership, position or capacity".into());
        }
    }
    Ok(())
}

pub fn target_at(world: &World, owner: CivId, tile: TileId) -> Option<Target> {
    world.cities.iter().find(|(_, c)| c.centre == tile && c.owner != owner).map(|(id, _)| Target::City(id)).or_else(
        || world.units.iter().find(|(_, u)| u.position == tile && u.owner != owner).map(|(id, _)| Target::Unit(id)),
    )
}

pub fn can_strike(world: &World, r: &Ruleset, id: Id<Unit>, target: Target) -> Result<(), AirRejection> {
    let unit = world.units.get(id).ok_or(AirRejection::NoSuchUnit)?;
    let (owner, tile, _) = target.facts(world).ok_or(AirRejection::InvalidTarget)?;
    if !unit.is_air(r) || r.units.get(unit.base).and_then(|b| b.ranged_strength).unwrap_or(0) <= 0 {
        return Err(AirRejection::NotAllowed);
    }
    if !unit.can_move() || unit.attacks_this_turn >= max_attacks(world, r, id) || unit.attacks_left == 0 {
        return Err(AirRejection::NoMovement);
    }
    if !world.diplomacy.at_war(unit.owner, owner) {
        return Err(AirRejection::NotAtWar);
    }
    if world.grid.distance(unit.position, tile) > attack_range(world, r, id) as u32 {
        return Err(AirRejection::OutOfRange);
    }
    let ctx = context(world, r, id).in_combat(CombatRole::Attacking).against(target.opponent()).on_tile(tile);
    if has(world, r, id, UniqueType::CannotAttack, &ctx, true)
        || has(world, r, id, UniqueType::NuclearWeapon, &ctx, false)
    {
        return Err(AirRejection::NotAllowed);
    }
    for unique in matching(world, r, id, UniqueType::CanOnlyAttackTiles, &ctx, false) {
        if !unique.compiled(r).filter(0).is_some_and(|f| matches_tile(&ctx, tile, f).holds()) {
            return Err(AirRejection::NotAllowed);
        }
    }
    for unique in matching(world, r, id, UniqueType::CanOnlyAttackUnits, &ctx, false) {
        let allowed = match target {
            Target::Unit(other) => unique.compiled(r).filter(0).is_some_and(|f| {
                crate::world::filters::matches_unit(r, world.units.get(other).expect("validated"), f).holds()
            }),
            Target::City(_) => unique.params.first().is_some_and(|f| matches!(f.as_str(), "All" | "City")),
        };
        if !allowed {
            return Err(AirRejection::NotAllowed);
        }
    }
    Ok(())
}

/// Chance is an explicit unique, never inferred from a unit's ranged weapon.
pub fn intercept_chance(world: &World, r: &Ruleset, id: Id<Unit>) -> i32 {
    if world.units.get(id).is_none() {
        return 0;
    }
    sum(world, r, id, UniqueType::ChanceInterceptAirAttacks, &context(world, r, id), false).clamp(0, 100)
}

pub fn can_intercept(world: &World, r: &Ruleset, id: Id<Unit>, tile: TileId) -> bool {
    let Some(unit) = world.units.get(id) else { return false };
    if intercept_chance(world, r, id) == 0 || (unit.is_air(r) && !unit.can_move()) {
        return false;
    }
    let ctx = context(world, r, id);
    let limit = 1 + sum(world, r, id, UniqueType::ExtraInterceptionsPerTurn, &ctx, false);
    let range = r.units.get(unit.base).and_then(|b| b.intercept_range).unwrap_or(0)
        + sum(world, r, id, UniqueType::AirInterceptionRange, &ctx, true);
    i32::from(unit.attacks_this_turn) < limit
        && range >= 0
        && world.grid.contains(tile)
        && world.grid.distance(unit.position, tile) <= range as u32
}

/// Percent chance that the interceptor a strike on `tile` would meet stops
/// it, choosing only among units `seen` admits (a player previews against the
/// defenders it can see). `defender` is the struck target, which never
/// intercepts for itself; `owner` stands in for the defender's owner when the
/// strike has no single target, as a nuclear one. Zero when nothing seen can
/// intercept, or the attacker cannot be intercepted.
pub fn visible_intercept_chance(
    world: &World,
    r: &Ruleset,
    attacker: Id<Unit>,
    tile: TileId,
    defender: Option<Target>,
    owner: Option<CivId>,
    seen: &dyn Fn(Id<Unit>) -> bool,
) -> i32 {
    if world.units.get(attacker).is_none() {
        return 0;
    }
    let mut ctx = context(world, r, attacker).on_tile(tile);
    if let Some(target) = defender {
        ctx = ctx.against(target.opponent());
    }
    if has(world, r, attacker, UniqueType::CannotBeIntercepted, &ctx, false) {
        return 0;
    }
    interceptor_among(world, r, attacker, tile, defender, false, owner, seen)
        .map_or(0, |id| intercept_chance(world, r, id))
}

/// Where this aircraft may rebase now: its owner's cities and carriers within
/// reach that have room for it.
pub fn rebase_targets(world: &World, r: &Ruleset, id: Id<Unit>) -> Vec<TileId> {
    let Some(unit) = world.units.get(id) else { return Vec::new() };
    let mut tiles: std::collections::BTreeSet<TileId> =
        world.cities.iter().filter(|(_, c)| c.owner == unit.owner).map(|(_, c)| c.centre).collect();
    tiles.extend(world.units.iter().filter(|(other, u)| *other != id && u.owner == unit.owner).map(|(_, u)| u.position));
    tiles.into_iter().filter(|t| can_rebase(world, r, id, *t).is_ok()).collect()
}

/// Tiles in range, and in sight, this aircraft may sweep now.
pub fn sweep_targets(world: &World, r: &Ruleset, id: Id<Unit>) -> Vec<TileId> {
    let Some(unit) = world.units.get(id) else { return Vec::new() };
    let range = attack_range(world, r, id).max(0) as u32;
    world.grid.within(unit.position, range).into_iter().filter(|t| can_sweep(world, r, id, *t).is_ok()).collect()
}

/// Tiles in sight this unit may drop onto now, within its paradrop range.
pub fn paradrop_targets(world: &World, r: &Ruleset, id: Id<Unit>) -> Vec<TileId> {
    let Some(unit) = world.units.get(id) else { return Vec::new() };
    let range = matching(world, r, id, UniqueType::MayParadrop, &context(world, r, id), false)
        .into_iter()
        .map(|u| number(u, 1))
        .max()
        .unwrap_or(0)
        .max(0) as u32;
    world.grid.within(unit.position, range).into_iter().filter(|t| can_paradrop(world, r, id, *t).is_ok()).collect()
}

/// The unit that meets a sweep over `tile`, as [`sweep`] chooses it.
pub fn sweep_interceptor(world: &World, r: &Ruleset, attacker: Id<Unit>, tile: TileId) -> Option<Id<Unit>> {
    interceptor(world, r, attacker, tile, None, true, None)
}

fn interceptor(
    world: &World,
    r: &Ruleset,
    attacker: Id<Unit>,
    tile: TileId,
    defender: Option<Target>,
    sweep: bool,
    owner_override: Option<CivId>,
) -> Option<Id<Unit>> {
    interceptor_among(world, r, attacker, tile, defender, sweep, owner_override, &|_| true)
}

/// The interceptor a mission would meet, choosing only among units `seen`
/// admits: a player's preview of a strike reads only the defenders it can see.
#[allow(clippy::too_many_arguments)]
fn interceptor_among(
    world: &World,
    r: &Ruleset,
    attacker: Id<Unit>,
    tile: TileId,
    defender: Option<Target>,
    sweep: bool,
    owner_override: Option<CivId>,
    seen: &dyn Fn(Id<Unit>) -> bool,
) -> Option<Id<Unit>> {
    let a = world.units.get(attacker)?;
    let owner = owner_override.or_else(|| defender.and_then(|d| d.facts(world)).map(|d| d.0));
    let candidates = world.units.iter().filter(|(id, u)| {
        seen(*id) && u.owner != a.owner
            && world.diplomacy.at_war(a.owner, u.owner)
            && (sweep || owner == Some(u.owner))
            && defender != Some(Target::Unit(*id))
            && can_intercept(world, r, *id, tile)
            && !matching(
                world,
                r,
                *id,
                UniqueType::CannotInterceptUnits,
                &context(world, r, *id).against(Opponent::Unit(attacker)).on_tile(tile),
                false,
            )
            .iter()
            .any(|u| u.compiled(r).filter(0).is_some_and(|f| crate::world::filters::matches_unit(r, a, f).holds()))
    });
    candidates
        .min_by_key(|(id, u)| {
            (if sweep && u.is_air(r) { 0 } else { 1 }, std::cmp::Reverse(intercept_chance(world, r, *id)), *id)
        })
        .map(|(id, _)| id)
}

fn random(world: &World, attacker: Id<Unit>, tile: TileId, phase: u64) -> Rng {
    let a = world.units.get(attacker).expect("validated");
    Rng::from_parts(&[
        0xA1_0000 + phase,
        u64::from(world.turn),
        attacker.index() as u64,
        attacker.generation() as u64,
        u64::from(a.position.0),
        u64::from(tile.0),
        u64::from(a.attacks_this_turn),
    ])
}

fn health_factor(world: &World, r: &Ruleset, id: Id<Unit>) -> Fixed {
    let unit = world.units.get(id).expect("validated");
    if has(world, r, id, UniqueType::NoDamagePenaltyWoundedUnits, &context(world, r, id), true) {
        Fixed::ONE
    } else {
        combat::health_ratio(unit.health)
    }
}

fn strength(world: &World, r: &Ruleset, id: Id<Unit>, opponent: Target, defending: bool, sweep: bool) -> Fixed {
    parts(world, r, id, opponent, defending, sweep).map_or(Fixed::ONE, |b| b.strength())
}

fn parts(
    world: &World,
    r: &Ruleset,
    id: Id<Unit>,
    opponent: Target,
    defending: bool,
    sweep: bool,
) -> Option<combat::Breakdown> {
    let bonus = if sweep && !defending {
        sum(world, r, id, UniqueType::StrengthWhenAirsweep, &context(world, r, id), false)
    } else {
        0
    };
    combat::combat_parts(world, r, id, opponent, defending, bonus)
}

fn apply_damage(world: &mut World, id: Id<Unit>, damage: i32) -> bool {
    let Some(unit) = world.units.get_mut(id) else { return true };
    if damage >= i32::from(unit.health) {
        world.remove_unit(id);
        true
    } else {
        unit.health = (i32::from(unit.health) - damage.max(0)) as u8;
        false
    }
}

fn take_interception(world: &mut World, r: &Ruleset, attacker: Id<Unit>, target: Target) -> Option<combat::Interception> {
    let (_, tile, _) = target.facts(world).expect("validated target");
    let ctx = context(world, r, attacker).against(target.opponent()).on_tile(tile);
    if has(world, r, attacker, UniqueType::CannotBeIntercepted, &ctx, false) {
        return None;
    }
    let selected = interceptor(world, r, attacker, tile, Some(target), false, None)?;
    let from = world.units.get(selected).expect("selected interceptor").position;
    let damage = apply_interception(world, r, attacker, tile, selected, target.opponent());
    Some(combat::Interception { unit: selected, from, damage })
}

pub(crate) fn intercept_nuclear(world: &mut World, r: &Ruleset, attacker: Id<Unit>, tile: TileId, owner: CivId) {
    if has(world, r, attacker, UniqueType::CannotBeIntercepted, &context(world, r, attacker), false) {
        return;
    }
    if let Some(interceptor) = interceptor(world, r, attacker, tile, None, false, Some(owner)) {
        apply_interception(world, r, attacker, tile, interceptor, Opponent::City);
    }
}
fn apply_interception(
    world: &mut World,
    r: &Ruleset,
    attacker: Id<Unit>,
    tile: TileId,
    interceptor: Id<Unit>,
    opponent: Opponent,
) -> i32 {
    let chance = intercept_chance(world, r, interceptor);
    let mut rng = random(world, attacker, tile, 1);
    world.units.get_mut(interceptor).expect("selected").spend_attack();
    if rng.range(0, 99) >= i64::from(chance) {
        return 0;
    }
    let att = strength(world, r, interceptor, Target::Unit(attacker), false, false);
    let def = strength(world, r, attacker, Target::Unit(interceptor), true, false);
    let mut damage = combat::damage_modifier_precise(combat::strength_ratio(att, def), false, rng.unit_interval())
        * Precise::from_fixed(health_factor(world, r, interceptor));
    let bonus = sum(world, r, interceptor, UniqueType::DamageWhenIntercepting, &context(world, r, interceptor), false);
    damage = damage * Precise::from_fixed(Fixed::permille(i64::from(100 + bonus) * 10));
    let ctx = context(world, r, attacker).against(opponent).on_tile(tile);
    for unique in matching(world, r, attacker, UniqueType::DamageFromInterceptionReduced, &ctx, false) {
        damage = damage * Precise::from_fixed(Fixed::permille(i64::from(100 - number(unique, 0)) * 10));
    }
    let damage = (damage.floor().max(0) as i32).min(i32::from(world.units.get(attacker).expect("validated").health));
    if damage > 0 {
        let owner = world.units.get(attacker).expect("validated").owner;
        crate::world::military::award_xp(world, r, interceptor, 2, Some(attacker), owner, false);
    }
    if damage >= i32::from(world.units.get(attacker).expect("attacker").health) {
        crate::world::military::reward_kill(world, r, interceptor, attacker, false);
    }
    apply_damage(world, attacker, damage);
    damage
}

fn spend_mission(world: &mut World, r: &Ruleset, id: Id<Unit>) {
    crate::world::military::spend_attack(world, r, id, false);
}

pub fn strike(
    world: &mut World,
    r: &Ruleset,
    attacker: Id<Unit>,
    target: Target,
) -> Result<CombatResult, AirRejection> {
    can_strike(world, r, attacker, target)?;
    let (_, tile, d_health) = target.facts(world).expect("validated");
    let interception = take_interception(world, r, attacker, target);
    let intercepted = interception.map_or(0, |i| i.damage);
    if world.units.get(attacker).is_none() {
        return Ok(CombatResult { damage_to_attacker: intercepted, attacker_died: true, interception, ..CombatResult::default() });
    }
    let a_parts = parts(world, r, attacker, target, false, false);
    let att = a_parts.as_ref().map_or(Fixed::ONE, |b| b.strength());
    let (def, d_factor, civilian, d_parts) = match target {
        Target::City(id) => {
            (combat::city_combat_strength(world, r, id, Some(attacker), false), Fixed::ONE, false, None)
        }
        Target::Unit(id) => {
            let d_parts = parts(world, r, id, Target::Unit(attacker), true, false);
            (
                d_parts.as_ref().map_or(Fixed::ONE, |b| b.strength()),
                health_factor(world, r, id),
                world.units.get(id).expect("validated").is_civilian(r),
                d_parts,
            )
        }
    };
    let tags = combat::BattleTags::of(a_parts.as_ref(), d_parts.as_ref());
    let mut rng = random(world, attacker, tile, 2);
    let roll = rng.unit_interval();
    let ratio = combat::strength_ratio(att, def);
    let to_d = if civilian {
        40
    } else {
        (combat::damage_modifier_precise(ratio, false, roll) * Precise::from_fixed(health_factor(world, r, attacker)))
            .round()
            .max(0) as i32
    };
    let to_a = if civilian {
        0
    } else {
        (combat::damage_modifier_precise(ratio, true, roll) * Precise::from_fixed(d_factor)).round().max(0) as i32
    };
    let (to_a, to_d) = crate::world::military::exchange(
        rng,
        i32::from(world.units.get(attacker).expect("validated").health),
        d_health,
        to_a,
        to_d,
        matches!(target, Target::City(_)),
    );
    crate::world::military::plunder_damage(world, r, attacker, target, to_d);
    let owner = world.units.get(attacker).expect("validated").owner;
    let target_owner = target.facts(world).expect("target").0;
    if world.civ(target_owner).is_some_and(|c| c.is_barbarian) {
        crate::world::barbarians::on_camp_attacked(world, tile);
    }
    if let Target::Unit(defender) = target {
        crate::world::military::award_xp(world, r, defender, 2, Some(attacker), owner, true);
    }
    crate::world::military::award_xp(
        world,
        r,
        attacker,
        4,
        match target {
            Target::Unit(id) => Some(id),
            _ => None,
        },
        target_owner,
        false,
    );
    if let Target::Unit(id) = target {
        if to_d >= d_health {
            crate::world::military::reward_kill(world, r, attacker, id, false);
        }
        if to_a >= i32::from(world.units.get(attacker).expect("attacker").health) {
            crate::world::military::reward_kill(world, r, id, attacker, true);
        }
    }
    let defender_died = match target {
        Target::Unit(id) => apply_damage(world, id, to_d),
        Target::City(id) => {
            world.cities.get_mut(id).expect("validated").health -= to_d;
            false
        }
    };
    let mut attacker_died = apply_damage(world, attacker, to_a);
    if !attacker_died {
        let self_destruct = has(world, r, attacker, UniqueType::SelfDestructs, &context(world, r, attacker), false);
        spend_mission(world, r, attacker);
        if self_destruct {
            world.remove_unit(attacker);
            attacker_died = true;
        }
    }
    if defender_died && !attacker_died {
        crate::world::military::heal_on_kill(world, r, attacker);
    }
    Ok(CombatResult {
        damage_to_attacker: intercepted + to_a,
        damage_to_defender: to_d,
        attacker_died,
        defender_died,
        interception,
        captured: None,
        tags,
    })
}

pub fn can_sweep(world: &World, r: &Ruleset, id: Id<Unit>, tile: TileId) -> Result<(), AirRejection> {
    let unit = world.units.get(id).ok_or(AirRejection::NoSuchUnit)?;
    if !unit.is_air(r) || !has(world, r, id, UniqueType::CanAirsweep, &context(world, r, id), false) {
        return Err(AirRejection::NotAllowed);
    }
    if !unit.can_move() || unit.attacks_left == 0 || unit.attacks_this_turn >= max_attacks(world, r, id) {
        return Err(AirRejection::NoMovement);
    }
    if !world.grid.contains(tile) || !world.visibility.can_see(unit.owner, tile) {
        return Err(AirRejection::NotVisible);
    }
    if world.grid.distance(unit.position, tile) > attack_range(world, r, id) as u32 {
        return Err(AirRejection::OutOfRange);
    }
    Ok(())
}

pub fn sweep(world: &mut World, r: &Ruleset, attacker: Id<Unit>, tile: TileId) -> Result<CombatResult, AirRejection> {
    can_sweep(world, r, attacker, tile)?;
    let selected = interceptor(world, r, attacker, tile, None, true, None);
    let rng = random(world, attacker, tile, 3);
    spend_mission(world, r, attacker);
    let mut result = CombatResult::default();
    let Some(defender) = selected else { return Ok(result) };
    let is_air = world.units.get(defender).expect("selected").is_air(r);
    world.units.get_mut(defender).expect("selected").spend_attack();
    if !is_air {
        return Ok(result);
    }
    let a_parts = parts(world, r, attacker, Target::Unit(defender), false, true);
    let d_parts = parts(world, r, defender, Target::Unit(attacker), true, false);
    let a = a_parts.as_ref().map_or(Fixed::ONE, |b| b.strength());
    let d = d_parts.as_ref().map_or(Fixed::ONE, |b| b.strength());
    result.tags = combat::BattleTags::of(a_parts.as_ref(), d_parts.as_ref());
    let ratio = combat::strength_ratio(a, d);
    let mut damage_rng = rng.clone();
    let roll = damage_rng.unit_interval();
    let to_d = (combat::damage_modifier_precise(ratio, false, roll)
        * Precise::from_fixed(health_factor(world, r, attacker)))
    .round()
    .max(0) as i32;
    let to_a = (combat::damage_modifier_precise(ratio, true, roll)
        * Precise::from_fixed(health_factor(world, r, defender)))
    .round()
    .max(0) as i32;
    let (to_a, to_d) = crate::world::military::exchange(
        rng,
        i32::from(world.units.get(attacker).expect("validated").health),
        i32::from(world.units.get(defender).expect("selected").health),
        to_a,
        to_d,
        false,
    );
    result.damage_to_attacker = to_a;
    result.damage_to_defender = to_d;
    let attacker_owner = world.units.get(attacker).expect("attacker").owner;
    let defender_owner = world.units.get(defender).expect("defender").owner;
    crate::world::military::award_xp(world, r, attacker, 5, Some(defender), defender_owner, false);
    crate::world::military::award_xp(world, r, defender, 5, Some(attacker), attacker_owner, true);
    if to_a >= i32::from(world.units.get(attacker).expect("attacker").health) {
        crate::world::military::reward_kill(world, r, defender, attacker, true);
    }
    if to_d >= i32::from(world.units.get(defender).expect("defender").health) {
        crate::world::military::reward_kill(world, r, attacker, defender, false);
    }
    result.attacker_died = apply_damage(world, attacker, to_a);
    result.defender_died = apply_damage(world, defender, to_d);
    Ok(result)
}

pub fn can_paradrop(world: &World, r: &Ruleset, id: Id<Unit>, to: TileId) -> Result<(), AirRejection> {
    let unit = world.units.get(id).ok_or(AirRejection::NoSuchUnit)?;
    if !unit.can_move() || unit.moved_this_turn || unit.attacked_this_turn {
        return Err(AirRejection::NoMovement);
    }
    if !world.grid.contains(to) || to == unit.position || !world.visibility.can_see(unit.owner, to) {
        return Err(AirRejection::NotVisible);
    }
    let ctx = context(world, r, id);
    let allowed = matching(world, r, id, UniqueType::MayParadrop, &ctx, false).into_iter().any(|u| {
        world.grid.distance(unit.position, to) <= number(u, 1).max(0) as u32
            && u.compiled(r).filter(0).is_some_and(|filter| matches_tile(&ctx, to, filter).holds())
    });
    if !allowed {
        return Err(AirRejection::NotAllowed);
    }
    if !crate::world::movement::can_pass_through(world, r, to, unit)
        || crate::world::movement::is_tile_occupied_for(world, r, to, unit)
    {
        return Err(AirRejection::InvalidTarget);
    }
    Ok(())
}

pub fn paradrop(
    world: &mut World,
    r: &Ruleset,
    id: Id<Unit>,
    to: TileId,
) -> Result<crate::world::movement::MoveEvent, AirRejection> {
    can_paradrop(world, r, id, to)?;
    let unit = world.units.get_mut(id).expect("validated");
    let from = unit.position;
    unit.position = to;
    unit.movement_left = (unit.movement_left - Fixed::ONE).max(Fixed::ZERO);
    unit.moved_this_turn = true;
    unit.spend_attack();
    unit.fortified = false;
    unit.fortification_turns = 0;
    unit.set_up = false;
    unit.embarked = false;
    move_payload(world, id, to);
    Ok(crate::world::movement::MoveEvent::plain(id, from, to, Fixed::ONE))
}

/// Choose useful air missions only from observed targets and owned bases.
pub fn order(world: &World, r: &Ruleset, id: Id<Unit>) -> Option<crate::world::ai::UnitOrder> {
    use crate::world::ai::UnitOrder;
    let unit = world.units.get(id)?;
    if !unit.is_air(r) || !unit.can_move() || unit.health < 75 {
        return None;
    }
    let visible: Vec<_> = world
        .cities
        .iter()
        .filter(|(_, c)| world.diplomacy.at_war(unit.owner, c.owner) && world.visibility.can_see(unit.owner, c.centre))
        .map(|(id, _)| Target::City(id))
        .chain(
            world
                .units
                .iter()
                .filter(|(other, u)| {
                    world.diplomacy.at_war(unit.owner, u.owner)
                        && crate::world::fog::can_see_unit(world, r, unit.owner, *other)
                })
                .map(|(id, _)| Target::Unit(id)),
        )
        .collect();
    let fighter = has(world, r, id, UniqueType::CanAirsweep, &context(world, r, id), false);
    if fighter {
        let enemy_air = visible
            .iter()
            .filter(|target| match target {
                Target::Unit(other) => world.units.get(*other).is_some_and(|u| {
                    u.is_air(r) && world.grid.distance(unit.position, u.position) <= attack_range(world, r, id) as u32
                }),
                _ => false,
            })
            .count();
        let unused = world
            .units
            .iter()
            .filter(|(other, u)| {
                u.owner == unit.owner
                    && u.is_air(r)
                    && u.can_move()
                    && u.attacks_this_turn < max_attacks(world, r, *other)
                    && has(world, r, *other, UniqueType::CanAirsweep, &context(world, r, *other), false)
                    && world.grid.distance(unit.position, u.position) <= attack_range(world, r, id) as u32
            })
            .count();
        if unused < enemy_air / 2 {
            return None;
        }
        // Sweep a visible air-defense position before friendly bombers strike.
        let bomber_support = world.units.iter().any(|(_, u)| {
            u.owner == unit.owner
                && u.is_air(r)
                && r.units.get(u.base).is_some_and(|b| b.unit_type == "Bomber")
                && u.can_move()
        });
        if enemy_air > 0 || bomber_support {
            if let Some(tile) = visible
                .iter()
                .filter_map(|target| target.facts(world).map(|x| x.1))
                .filter(|tile| can_sweep(world, r, id, *tile).is_ok())
                .filter(|tile| {
                    world.units.iter().any(|(other, u)| {
                        crate::world::fog::can_see_unit(world, r, unit.owner, other)
                            && world.diplomacy.at_war(unit.owner, u.owner)
                            && can_intercept(world, r, other, *tile)
                    })
                })
                .min_by_key(|tile| (world.grid.distance(unit.position, *tile), *tile))
            {
                return Some(UnitOrder::AirSweep(tile));
            }
        }
    }
    let mut targets: Vec<_> =
        visible.iter().copied().filter(|target| can_strike(world, r, id, *target).is_ok()).collect();
    targets.sort_by_key(|target| {
        let (_, tile, health) = target.facts(world).expect("known target");
        (health, world.grid.distance(unit.position, tile), tile)
    });
    if let Some(target) = targets.first() {
        return Some(match *target {
            Target::City(id) => UnitOrder::Assault(id),
            Target::Unit(id) => UnitOrder::Attack(id),
        });
    }
    if unit.health <= 90 {
        return None;
    }
    let mut objectives: Vec<_> = visible
        .iter()
        .filter_map(|target| target.facts(world).map(|x| x.1))
        .chain(
            world
                .visibility
                .known_cities(unit.owner)
                .filter(|c| world.diplomacy.at_war(unit.owner, c.owner))
                .map(|c| c.centre),
        )
        .collect();
    objectives.sort();
    objectives.dedup();
    if objectives.is_empty() {
        return None;
    }
    let mut bases: Vec<_> = world
        .cities
        .iter()
        .filter(|(_, c)| c.owner == unit.owner)
        .map(|(_, c)| c.centre)
        .chain(world.units.iter().filter(|(_, u)| u.owner == unit.owner).map(|(_, u)| u.position))
        .filter(|tile| base_for(world, r, unit, *tile, Some(id)).is_some())
        .collect();
    bases.sort();
    bases.dedup();
    let range = attack_range(world, r, id).saturating_mul(2) as u32;
    let mut reached = std::collections::BTreeMap::new();
    let mut queue = std::collections::VecDeque::from([(unit.position, unit.position, 0u32)]);
    reached.insert(unit.position, (unit.position, 0));
    while let Some((from, first, hops)) = queue.pop_front() {
        for &to in &bases {
            if reached.contains_key(&to) || world.grid.distance(from, to) > range {
                continue;
            }
            let first = if hops == 0 { to } else { first };
            reached.insert(to, (first, hops + 1));
            queue.push_back((to, first, hops + 1));
        }
    }
    let distance = |base| objectives.iter().map(|&target| world.grid.distance(base, target)).min().unwrap_or(u32::MAX);
    let current_distance = distance(unit.position);
    reached
        .into_iter()
        .filter(|(tile, _)| *tile != unit.position && distance(*tile) < current_distance)
        .min_by_key(|(tile, (_, hops))| {
            (distance(*tile) > attack_range(world, r, id) as u32, *hops, distance(*tile), *tile)
        })
        .map(|(_, (first, _))| UnitOrder::Rebase(first))
}

pub fn paradrop_order(world: &World, r: &Ruleset, id: Id<Unit>) -> Option<TileId> {
    let unit = world.units.get(id)?;
    if matching(world, r, id, UniqueType::MayParadrop, &context(world, r, id), false).is_empty() {
        return None;
    }
    let targets: Vec<_> = world
        .visibility
        .known_cities(unit.owner)
        .filter(|c| world.diplomacy.at_war(unit.owner, c.owner))
        .map(|c| c.centre)
        .collect();
    let current = targets.iter().map(|&t| world.grid.distance(unit.position, t)).min()?;
    world
        .visibility
        .visible_tiles(unit.owner)
        .into_iter()
        .filter(|&tile| can_paradrop(world, r, id, tile).is_ok())
        .filter_map(|tile| {
            let distance = targets.iter().map(|&t| world.grid.distance(tile, t)).min()?;
            (distance + 1 < current).then_some((distance, tile))
        })
        .min()
        .map(|(_, tile)| tile)
}
