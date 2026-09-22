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
use crate::ruleset::Ruleset;
use crate::stats::{Fixed, Precise};
use crate::world::entities::Unit;
use crate::world::grid::TileId;
use crate::world::world::World;

/// Percent added per level of fortification (`BattleConstants.kt:11`).
pub const FORTIFICATION_BONUS: i32 = 20;

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
    /// No movement left to attack with.
    NoMovement,
}

/// A unit's strength, and where each part of it came from.
pub fn strength_breakdown(
    world: &World,
    r: &Ruleset,
    unit_id: Id<Unit>,
    defending: bool,
) -> Option<(Fixed, ModifierStack)> {
    let unit = world.units.get(unit_id)?;
    let base = r.units.get(unit.base)?.strength.unwrap_or(0);
    let mut stack = ModifierStack::new();

    // A unit whose resource has run out still stands; it just fights badly.
    // Applies to attack and defence alike — Unciv puts it in the modifiers both
    // sides share (`BattleDamage.kt:52`).
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
        // Terrain defence is a fraction in the json — Hill is 0.25 — so it
        // becomes a percentage here.
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
    }

    // Strength never drops below a token amount, or the ratio maths degenerates.
    stack.floor_at(Fixed::ONE);
    Some((Fixed::whole(base), stack))
}

/// Strength a unit attacks with.
pub fn attacking_strength(world: &World, r: &Ruleset, unit: Id<Unit>) -> Option<Fixed> {
    let (base, stack) = strength_breakdown(world, r, unit, false)?;
    Some(stack.evaluate(base))
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
    let (a, d) = (
        world.units.get(attacker).ok_or(CombatRejection::NoSuchUnit)?,
        world.units.get(defender).ok_or(CombatRejection::NoSuchUnit)?,
    );
    if a.owner == d.owner {
        return Err(CombatRejection::SameOwner);
    }
    // Civs who have met and are at peace do not fight. Two who have never met
    // are treated as free to, which is how a first contact goes badly.
    if world.diplomacy.have_met(a.owner, d.owner) && !world.diplomacy.at_war(a.owner, d.owner) {
        return Err(CombatRejection::NotAtWar);
    }
    if !a.can_move() {
        return Err(CombatRejection::NoMovement);
    }
    let (a_tile, d_tile, a_health, d_health) = (a.position, d.position, a.health, d.health);
    if !world.grid.neighbours(a_tile).any(|t| t == d_tile) {
        return Err(CombatRejection::NotAdjacent);
    }

    let att = attacking_strength(world, r, attacker).ok_or(CombatRejection::NoSuchUnit)?;
    let def = defending_strength(world, r, defender).ok_or(CombatRejection::NoSuchUnit)?;
    let ratio = strength_ratio(att, def);

    // One roll drives both sides, as in Unciv.
    let randomness = combat_seed(world, a_tile, d_tile).unit_interval();

    // One rounding each, at the very end.
    let to_defender = damage_modifier_precise(ratio, false, randomness)
        .mul(Precise::from_fixed(health_ratio(a_health)));
    let to_attacker = damage_modifier_precise(ratio, true, randomness)
        .mul(Precise::from_fixed(health_ratio(d_health)));

    let (dmg_d, dmg_a) = (to_defender.round().max(0) as i32, to_attacker.round().max(0) as i32);

    let defender_died = dmg_d >= d_health as i32;
    let attacker_died = dmg_a >= a_health as i32;

    if defender_died {
        world.remove_unit(defender);
    } else if let Some(u) = world.units.get_mut(defender) {
        u.health = (u.health as i32 - dmg_d).max(1) as u8;
    }
    if attacker_died {
        world.remove_unit(attacker);
    } else if let Some(u) = world.units.get_mut(attacker) {
        u.health = (u.health as i32 - dmg_a).max(1) as u8;
        u.movement_left = Fixed::ZERO; // attacking ends the turn
        u.fortified = false;
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
    if !a.can_move() {
        return Err(CombatRejection::NoMovement);
    }
    let (a_tile, centre, a_health, owner) = (a.position, city.centre, a.health, a.owner);
    if !world.grid.neighbours(a_tile).any(|t| t == centre) {
        return Err(CombatRejection::NotAdjacent);
    }

    let att = attacking_strength(world, r, attacker).ok_or(CombatRejection::NoSuchUnit)?;
    let def = crate::world::city::city_strength(r, world.cities.get(city_id).unwrap());
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
        w.units.get_mut(a).unwrap().movement_left = Fixed::whole(movement);
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
            w.units.get_mut(unit).unwrap().movement_left = Fixed::whole(2);
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
}
