//! Nuclear attack legality and deterministic blast effects (`Nuke.kt`).
use crate::world::combat::{CombatRejection, NuclearStrikeResult};
use crate::world::conditionals::{applies, GameContext};
use crate::world::{City, CivId, TileId, Unit, World};
use crate::{arena::Id, rng::Rng, ruleset::Ruleset, stats::Fixed, UniqueType};
use std::collections::BTreeSet;

fn amount(rule: &crate::Unique) -> i32 {
    rule.params.first().and_then(|p| p.parse().ok()).unwrap_or(0)
}
pub fn blast_radius(world: &World, r: &Ruleset, id: Id<Unit>) -> u32 {
    world
        .units
        .get(id)
        .into_iter()
        .flat_map(|u| u.all_uniques(r))
        .find(|u| u.unique_type() == Some(UniqueType::BlastRadius))
        .map_or(2, |u| amount(u).max(0) as u32)
}
pub fn victims(world: &World, r: &Ruleset, id: Id<Unit>, target: TileId) -> BTreeSet<CivId> {
    let tiles: BTreeSet<_> = world.grid.within(target, blast_radius(world, r, id)).into_iter().collect();
    world
        .grid
        .ids()
        .filter(|t| tiles.contains(t))
        .filter_map(|t| world.grid.owner[t.0 as usize])
        .chain(world.units.iter().filter(|(_, u)| tiles.contains(&u.position)).map(|(_, u)| u.owner))
        .collect()
}
pub fn can_strike(world: &World, r: &Ruleset, id: Id<Unit>, target: TileId) -> Result<(), CombatRejection> {
    let unit = world.units.get(id).ok_or(CombatRejection::NoSuchUnit)?;
    if !unit.can_move() || unit.attacks_left == 0 {
        return Err(CombatRejection::NoMovement);
    }
    if !unit.has_unique(r, UniqueType::NuclearWeapon)
        || target == unit.position
        || !world.grid.contains(target)
        || !world.visibility.has_explored(unit.owner, target)
    {
        return Err(CombatRejection::NoRangedAttack);
    }
    if world.grid.distance(unit.position, target) > crate::world::air::attack_range(world, r, id).max(0) as u32 {
        return Err(CombatRejection::OutOfRange);
    }
    for other in victims(world, r, id, target).into_iter().filter(|c| *c != unit.owner) {
        let Some(civ) = world.civ(other) else { return Err(CombatRejection::NoSuchUnit) };
        if civ.is_alive()
            && !civ.is_barbarian
            && (!world.diplomacy.have_met(unit.owner, other)
                || (!world.diplomacy.at_war(unit.owner, other) && !world.diplomacy.can_declare_war(unit.owner, other)))
        {
            return Err(CombatRejection::NotAtWar);
        }
    }
    Ok(())
}
fn city_modifier(world: &World, r: &Ruleset, id: Id<City>, kind: UniqueType) -> Fixed {
    let Some(city) = world.cities.get(id) else { return Fixed::ONE };
    let context = GameContext::new(r).with_world(world).for_civ(city.owner).in_city(id).on_tile(city.centre);
    crate::world::religion::city_uniques(world, r, city)
        .into_iter()
        .filter(|u| {
            u.unique_type() == Some(kind)
                && applies(u, &context).holds()
                && u.params
                    .get(1)
                    .is_some_and(|f| crate::world::filters::city_matches(world, r, city, Some(city), f).holds())
        })
        .fold(Fixed::ONE, |factor, u| factor * (Fixed::ONE + Fixed::from_fraction(i64::from(amount(u)), 100)))
}
fn random(world: &World, id: Id<Unit>, tile: TileId, phase: u64) -> Rng {
    Rng::from_parts(&[
        0xB0_0B00 + phase,
        u64::from(world.turn),
        id.index() as u64,
        id.generation() as u64,
        u64::from(tile.0),
    ])
}
pub fn strike(
    world: &mut World,
    r: &Ruleset,
    id: Id<Unit>,
    target: TileId,
) -> Result<NuclearStrikeResult, CombatRejection> {
    can_strike(world, r, id, target)?;
    let unit = world.units.get(id).expect("validated");
    let owner = unit.owner;
    let strength = unit.all_uniques(r).find(|u| u.unique_type() == Some(UniqueType::NuclearWeapon)).map_or(0, amount);
    let air = unit.is_air(r);
    let self_destruct = unit.has_unique(r, UniqueType::SelfDestructs);
    let stock = crate::world::resources::available_stock(world, r, owner);
    let missing = if world.civ(owner).is_some_and(|c| c.is_barbarian) {
        0
    } else {
        crate::world::resources::required_resources(r, unit.base)
            .iter()
            .filter(|res| stock.get(res).copied().unwrap_or(0) < 0)
            .count()
    };
    let mut resource_modifier = Fixed::ONE;
    for _ in 0..missing {
        resource_modifier = resource_modifier * Fixed::permille(500);
    }
    let tiles = world.grid.within(target, blast_radius(world, r, id));
    let hit_unit_civs: BTreeSet<_> = world
        .units
        .iter()
        .filter(|(_, u)| tiles.contains(&u.position) && u.owner != owner)
        .map(|(_, u)| u.owner)
        .collect();
    for other in victims(world, r, id, target).into_iter().filter(|c| *c != owner) {
        crate::world::diplomacy_actions::declare_war(world, r, owner, other);
    }
    let mut result =
        NuclearStrikeResult { tiles_affected: Vec::new(), units_destroyed: Vec::new(), cities_damaged: Vec::new() };
    if air {
        for other in hit_unit_civs {
            crate::world::air::intercept_nuclear(world, r, id, target, other);
            if world.units.get(id).is_none() {
                return Ok(result);
            }
        }
    }
    for tile in tiles {
        result.tiles_affected.push(tile);
        let mut rng = random(world, id, tile, 1);
        let city = world.cities.iter().find(|(_, c)| c.centre == tile).map(|(id, _)| id);
        let garrison =
            city.map_or(Fixed::ONE, |city| city_modifier(world, r, city, UniqueType::GarrisonDamageFromNukes));
        if let Some(city_id) = city {
            result.cities_damaged.push(city_id);
            let city = world.cities.get(city_id).expect("city");
            let destroy = (strength > 2 || (strength > 1 && city.population < 5))
                && !city.original_capital
                && city.faith.holy_city_of.is_none();
            if destroy {
                crate::world::conquest::destroy_in(world, r, city_id);
            } else {
                let modifier = city_modifier(world, r, city_id, UniqueType::PopulationLossFromNukes);
                let fraction = match strength {
                    0 => 0,
                    1 => 30 + rng.range(0, 19) as i32 + rng.range(0, 19) as i32,
                    2 => 60 + rng.range(0, 9) as i32 + rng.range(0, 9) as i32,
                    _ => 100,
                };
                let city = world.cities.get_mut(city_id).expect("surviving city");
                city.health = (city.health
                    - (Fixed::whole(city.health) * Fixed::permille(500) * resource_modifier).floor() as i32)
                    .max(1);
                let lost =
                    (Fixed::whole(city.population as i32) * modifier * Fixed::from_fraction(i64::from(fraction), 100))
                        .floor()
                        .max(0) as u32;
                let old = city.population;
                city.population = city.population.saturating_sub(lost).max(1);
                city.faith.on_population_change(city.population as i32 - old as i32, city.population);
            }
        }
        for defender in world.units_at(tile) {
            let Some(unit) = world.units.get(defender) else { continue };
            let base = if tile == target || strength >= 2 {
                100
            } else if strength == 1 {
                30 + rng.range(0, 39) as i32 + rng.range(0, 39) as i32
            } else {
                20 + rng.range(0, 29) as i32
            };
            let damage = (Fixed::whole(base) * garrison * resource_modifier).floor().max(0) as i32;
            let died = if unit.is_civilian(r) {
                i32::from(unit.health) - damage <= 40
            } else {
                damage >= i32::from(unit.health)
            };
            if died {
                world.remove_unit(defender);
                result.units_destroyed.push(defender);
            } else if !unit.is_civilian(r) {
                world.units.get_mut(defender).expect("survivor").health -= damage as u8;
            }
        }
        if world.cities.iter().any(|(_, c)| c.centre == tile) {
            continue;
        }
        let features = world.grid.features[tile.0 as usize].clone();
        let mut destructible = false;
        let mut fallout = false;
        for feature in features {
            for rule in r.terrains[feature]
                .uniques
                .iter()
                .filter(|u| u.unique_type() == Some(UniqueType::DestroyableByNukesChance))
            {
                destructible = true;
                let chance = amount(rule);
                if (tile == target && chance > 0) || rng.range(0, 99) < i64::from(chance) {
                    world.grid.features[tile.0 as usize].retain(|t| *t != feature);
                    fallout = true;
                }
            }
        }
        if !destructible {
            fallout = tile == target || rng.range(0, 99) < 50;
        }
        if fallout {
            apply_fallout(world, r, tile);
        }
    }
    if world.units.get(id).is_some() {
        if self_destruct {
            world.remove_unit(id);
        } else {
            crate::world::military::spend_attack(world, r, id, false);
        }
    }
    let known: Vec<_> =
        world.civs.iter().filter(|c| c.id != owner && world.diplomacy.have_met(owner, c.id)).map(|c| c.id).collect();
    for civ in known {
        world
            .diplomacy
            .attitude_mut(civ, owner)
            .set(crate::world::diplomacy::Sentiment::UsedNuclearWeapons, Fixed::whole(-50));
    }
    Ok(result)
}
fn apply_fallout(world: &mut World, r: &Ruleset, tile: TileId) {
    if let Some(improvement) = world.grid.unpillaged_improvement(tile) {
        let rules = &r.improvements[improvement].uniques;
        if !rules.iter().any(|u| u.unique_type() == Some(UniqueType::Irremovable)) {
            if rules.iter().any(|u| u.unique_type() == Some(UniqueType::Unpillagable)) {
                world.grid.improvement[tile.0 as usize] = None;
            } else {
                world.grid.pillage_improvement(tile);
            }
        }
    }
    world.grid.pillage_road(tile);
    if world.grid.is_water(r, tile) || !crate::world::movement::is_passable(&world.grid, r, tile) {
        return;
    }
    if let Some(fallout) = r.terrain("Fallout") {
        if !world.grid.features[tile.0 as usize].contains(&fallout) {
            world.grid.features[tile.0 as usize].push(fallout);
        }
    }
}

/// Prefer enemy cities and avoid knowingly harming friendly or neutral forces.
pub fn order(world: &World, r: &Ruleset, id: Id<Unit>) -> Option<TileId> {
    let unit = world.units.get(id)?;
    world
        .cities
        .iter()
        .filter(|(_, c)| world.diplomacy.at_war(unit.owner, c.owner) && world.visibility.can_see(unit.owner, c.centre))
        .filter(|(_, c)| {
            can_strike(world, r, id, c.centre).is_ok()
                && victims(world, r, id, c.centre)
                    .into_iter()
                    .all(|other| other != unit.owner && world.diplomacy.at_war(unit.owner, other))
        })
        .max_by_key(|(id, c)| (c.population, std::cmp::Reverse(*id)))
        .map(|(_, c)| c.centre)
}
