//! Barbarians.
//!
//! A civilization in the data model and not one in the game: permanently
//! hostile, never at peace, no diplomacy, no cities of its own. Encampments on
//! the map produce hostile units on a timer, which is the early game's only
//! source of pressure on a civ that would otherwise expand unopposed.
//!
//! The camp is identified by its `Marks a barbarian camp` unique rather than
//! by the name "Barbarian encampment" — the same reasoning as the palace,
//! settlers and workers.

use crate::arena::Id;
use crate::rng::Rng;
use crate::ruleset::{Ruleset, TileImprovement};
use crate::world::entities::Unit;
use crate::world::grid::{CivId, TileId};
use crate::world::world::World;

/// Normal-difficulty delay added to each camp's randomized 8–12 turn clock.
pub const DEFAULT_SPAWN_DELAY: u32 = 0;

#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Camp {
    pub countdown: u32,
    pub spawned_units: i32,
    pub destroyed: bool,
}
impl Default for Camp {
    fn default() -> Self {
        Self { countdown: 0, spawned_units: -1, destroyed: false }
    }
}
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CampModifiers {
    pub difficulty_delay: i32,
    pub speed: crate::stats::Fixed,
    pub raging: bool,
}
impl Default for CampModifiers {
    fn default() -> Self {
        Self { difficulty_delay: 0, speed: crate::stats::Fixed::ONE, raging: false }
    }
}

pub fn on_camp_attacked(world: &mut World, tile: TileId) {
    if let Some(camp) = world.barbarian_camps.get_mut(&tile).filter(|c| !c.destroyed) {
        camp.countdown /= 2;
    }
}
fn mark_destroyed(world: &mut World, tile: TileId) {
    let camp = world.barbarian_camps.entry(tile).or_default();
    if !camp.destroyed {
        camp.destroyed = true;
        camp.countdown = 15;
    }
}
pub(crate) fn synchronize(world: &mut World, r: &Ruleset) {
    let live: std::collections::BTreeSet<_> = camps(world, r).into_iter().collect();
    for tile in &live {
        world.barbarian_camps.entry(*tile).or_default();
    }
    for tile in world.barbarian_camps.keys().copied().collect::<Vec<_>>() {
        if !live.contains(&tile) {
            mark_destroyed(world, tile);
        }
    }
    world.barbarian_camps.retain(|_, c| !c.destroyed || c.countdown > 0);
}

/// The improvement that marks a camp.
pub fn camp_improvement(r: &Ruleset) -> Option<Id<TileImprovement>> {
    r.improvements
        .iter()
        .find(|(_, i)| i.uniques.iter().any(|u| u.text.contains("Marks a barbarian camp")))
        .map(|(id, _)| id)
}

/// Every camp on the map, in tile order.
pub fn camps(world: &World, r: &Ruleset) -> Vec<TileId> {
    let Some(camp) = camp_improvement(r) else { return Vec::new() };
    world.grid.ids().filter(|t| world.grid.improvement[t.0 as usize] == Some(camp)).collect()
}

/// Place a camp. Used by map setup and by tests.
pub fn place_camp(world: &mut World, r: &Ruleset, tile: TileId) -> bool {
    match camp_improvement(r) {
        Some(camp) => {
            if !world.grid.contains(tile) {
                return false;
            }
            world.grid.set_improvement(tile, Some(camp));
            world.barbarian_camps.insert(tile, Camp::default());
            true
        }
        None => false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClearedCamp {
    pub civ: CivId,
    pub tile: TileId,
    pub gold: i32,
    pub recruited: Option<Id<Unit>>,
    pub quests: Vec<crate::world::quests::QuestEvent>,
}

/// Unit entry clears a camp before later quest observation, once per site.
pub fn on_enter(
    world: &mut World,
    r: &Ruleset,
    id: Id<Unit>,
    mods: crate::world::turn::TurnModifiers,
) -> Option<ClearedCamp> {
    use crate::{
        stats::Fixed,
        world::conditionals::{applies, evaluate, GameContext},
        UniqueType,
    };
    let unit = world.units.get(id)?;
    let (civ, tile) = (unit.owner, unit.position);
    if world.civ(civ)?.is_barbarian
        || world.grid.improvement[tile.0 as usize] != camp_improvement(r)
        || camp_improvement(r).is_none()
    {
        return None;
    }
    let ctx = GameContext::new(r).with_world(world).for_civ(civ).for_unit(id).on_tile(tile);
    let mut rng = Rng::from_parts(&[0xCA_CC, world.turn as u64, u64::from(tile.0), u64::from(civ.0)]);
    let extra: Vec<i32> = world
        .civ(civ)?
        .uniques
        .matching(UniqueType::GainFromEncampment)
        .filter(|u| {
            u.conditionals.iter().all(|c| {
                if c.unique_type() == Some(UniqueType::ConditionalChance) {
                    rng.range(0, 99) < c.params.first().and_then(|p| p.parse::<i64>().ok()).unwrap_or(0)
                } else {
                    evaluate(c, &ctx).holds()
                }
            })
        })
        .filter_map(|u| u.params.first()?.parse().ok())
        .collect();
    let multiplier = world
        .civ(civ)?
        .uniques
        .matching(UniqueType::GoldFromEncampmentsAndCities)
        .filter(|u| applies(u, &ctx).holds())
        .filter_map(|u| u.params.first()?.parse::<i64>().ok())
        .fold(Fixed::ONE, |acc, n| acc * Fixed::from_fraction(100 + n, 100));
    let difficulty = if world.civ(civ)?.is_ai { r.default_difficulty() } else { mods.difficulty };
    let reward = difficulty.map_or(25, |d| d.clear_barbarian_camp_reward) + extra.iter().sum::<i32>();
    let gold = (Fixed::whole(reward) * mods.costs.gold_cost * multiplier).floor().max(0) as i32;
    let quests = crate::world::quests::on_camp_cleared(world, r, civ, tile);
    world.grid.set_improvement(tile, None);
    mark_destroyed(world, tile);
    let mut recruited = None;
    if let Some(barbarians) = world.civs.iter().find(|c| c.is_barbarian).map(|c| c.id) {
        update_techs(world, r, barbarians);
        for _ in extra {
            if let Some(id) = spawn_one(world, r, barbarians, tile, civ) {
                world.units.get_mut(id).unwrap().movement_left = Fixed::ZERO;
                recruited = Some(id);
            }
        }
    }
    world.civ_mut(civ)?.gold = world.civ(civ)?.gold.saturating_add(gold);
    Some(ClearedCamp { civ, tile, gold, recruited, quests })
}

/// What a camp can field.
///
/// The ruleset supplies purpose-built barbarian units — Brute and Galley, both
/// unique to the barbarian nation and neither requiring a technology. Preferring
/// those is why barbarians stay a nuisance rather than fielding whatever the
/// era allows. Anything tech-free falls back behind them.
pub fn spawnable_unit(r: &Ruleset) -> Option<Id<crate::ruleset::BaseUnit>> {
    let land_military = |u: &crate::ruleset::BaseUnit| {
        u.strength.unwrap_or(0) > 0
            && u.cost.unwrap_or(0) > 0
            && u.required_tech.is_none()
            && r.unit_types.by_name(&u.unit_type).is_some_and(|t| t.movement_type == "Land")
    };

    r.units
        .iter()
        .filter(|(_, u)| land_military(u) && u.unique_to.as_deref().is_some_and(|n| r.is_barbarian_name(n)))
        .min_by_key(|(id, u)| (u.cost.unwrap_or(i32::MAX), id.index()))
        .or_else(|| {
            r.units
                .iter()
                .filter(|(_, u)| land_military(u) && u.unique_to.is_none())
                .min_by_key(|(id, u)| (u.cost.unwrap_or(i32::MAX), id.index()))
        })
        .map(|(id, _)| id)
}

/// Shared researched technologies determine the barbarian unit pool. Resources
/// are intentionally irrelevant for this civilization (BaseUnit.kt).
fn update_techs(world: &mut World, r: &Ruleset, barbarians: CivId) {
    let living: Vec<_> = world.civs.iter().filter(|c| !c.is_barbarian && c.is_alive()).collect();
    let known = if living.is_empty() {
        Default::default()
    } else {
        r.technologies.ids().filter(|id| living.iter().all(|c| c.techs.contains(id))).collect()
    };
    if let Some(c) = world.civ_mut(barbarians) {
        c.techs = known;
        // This civilization inherits the common tree; it has no independent
        // research project. Clear any selection/investment from generic AI
        // planning before newly common technologies become known.
        c.researching = None;
        c.science_invested.clear();
        c.science_stored = 0;
    }
    crate::world::policies::rebuild_uniques(world, r, barbarians);
}
fn spawn_choice(
    world: &World,
    r: &Ruleset,
    barbarians: CivId,
    tile: TileId,
    rng: &mut Rng,
) -> Option<Id<crate::ruleset::BaseUnit>> {
    let owner = world.civ(barbarians)?;
    let water = world.grid.is_water(r, tile);
    let ctx = crate::world::conditionals::GameContext::new(r).with_world(world).for_civ(barbarians).on_tile(tile);
    let choices: Vec<_> = r
        .units
        .iter()
        .filter(|(_, u)| {
            let strength = u.strength.unwrap_or(0).max(u.ranged_strength.unwrap_or(0));
            strength > 0
                && r.unit_types
                    .by_name(&u.unit_type)
                    .is_some_and(|t| t.movement_type == if water { "Water" } else { "Land" })
                && u.unique_to.as_deref().is_none_or(|n| r.is_barbarian_name(n))
                && u.required_tech
                    .as_deref()
                    .is_none_or(|name| r.technologies.id(name).is_some_and(|id| owner.techs.contains(&id)))
                && u.obsolete_tech
                    .as_deref()
                    .is_none_or(|name| r.technologies.id(name).is_none_or(|id| !owner.techs.contains(&id)))
                && !u.uniques.iter().any(|u| {
                    matches!(
                        u.unique_type(),
                        Some(
                            crate::UniqueType::CannotBeBarbarian
                                | crate::UniqueType::CannotAttack
                                | crate::UniqueType::Unbuildable
                                | crate::UniqueType::Unavailable
                        )
                    ) && crate::world::conditionals::applies(u, &ctx).holds()
                })
                && u.uniques
                    .iter()
                    .filter(|u| {
                        matches!(
                            u.unique_type(),
                            Some(crate::UniqueType::OnlyAvailable | crate::UniqueType::CanOnlyBeBuiltWhen)
                        )
                    })
                    .all(|u| crate::world::conditionals::applies(u, &ctx).holds())
        })
        .map(|(id, u)| {
            (
                id,
                i64::from(u.strength.unwrap_or(0).max(u.ranged_strength.unwrap_or(0))).max(1).pow(2)
                    * i64::from(u.movement.max(1)),
            )
        })
        .collect();
    let total: i64 = choices.iter().map(|(_, weight)| weight).sum();
    if total == 0 {
        return None;
    }
    let mut pick = rng.range(0, total - 1);
    for (id, weight) in choices {
        if pick < weight {
            return Some(id);
        }
        pick -= weight;
    }
    None
}

/// Compatibility entry point for scenario callers; live matches use `tick`.
pub fn spawn(world: &mut World, r: &Ruleset, barbarians: CivId, delay: u32) -> Vec<TileId> {
    synchronize(world, r);
    spawn_due(world, r, barbarians, CampModifiers { difficulty_delay: delay as i32, ..Default::default() })
}
fn spawn_one(world: &mut World, r: &Ruleset, barbarians: CivId, camp: TileId, allegiance: CivId) -> Option<Id<Unit>> {
    let turn = world.turn;
    let mut rng = Rng::from_parts(&[0xBA8B, turn as u64, camp.0 as u64, allegiance.0 as u64]);
    let military = world.units_at(camp).into_iter().find(|id| world.units.get(*id).is_some_and(|u| !u.is_civilian(r)));
    let target = if military.is_none() && allegiance == barbarians {
        camp
    } else {
        if allegiance == barbarians
            && (turn < 10
                || military.is_some_and(|id| world.units.get(id).is_some_and(|u| u.owner != barbarians))
                || world
                    .units
                    .iter()
                    .filter(|(_, u)| {
                        u.owner == barbarians && !u.is_civilian(r) && world.grid.distance(u.position, camp) <= 4
                    })
                    .count()
                    > 2)
        {
            return None;
        }
        let tiles: Vec<_> = world
            .grid
            .neighbours(camp)
            .filter(|t| {
                world.units_at(*t).is_empty()
                    && !world.cities.iter().any(|(_, c)| c.centre == *t)
                    && crate::world::movement::is_passable(&world.grid, r, *t)
                    && (!world.grid.is_water(r, *t)
                        || turn > 30
                            && !std::iter::once(&world.grid.terrain[t.0 as usize])
                                .chain(world.grid.features[t.0 as usize].iter())
                                .any(|id| {
                                    r.terrains[*id]
                                        .uniques
                                        .iter()
                                        .any(|u| u.unique_type() == Some(crate::UniqueType::FreshWater))
                                }))
            })
            .collect();
        if tiles.is_empty() {
            return None;
        }
        tiles[rng.range(0, tiles.len() as i64 - 1) as usize]
    };
    let base = spawn_choice(world, r, barbarians, target, &mut rng)?;
    let unit = crate::world::construction::prepare_unit_near(world, r, allegiance, base, target, None)?;
    Some(world.place_unit(unit))
}
fn spawn_due(world: &mut World, r: &Ruleset, barbarians: CivId, mods: CampModifiers) -> Vec<TileId> {
    update_techs(world, r, barbarians);
    let mut spawned = Vec::new();
    for tile in world.barbarian_camps.keys().copied().collect::<Vec<_>>() {
        let camp = &mut world.barbarian_camps.get_mut(&tile).unwrap();
        if camp.countdown > 0 {
            camp.countdown -= 1;
            continue;
        }
        if camp.destroyed {
            continue;
        }
        if let Some(unit) = spawn_one(world, r, barbarians, tile, barbarians) {
            spawned.push(world.units.get(unit).unwrap().position);
            let camp = world.barbarian_camps.get_mut(&tile).unwrap();
            camp.spawned_units = camp.spawned_units.saturating_add(1);
            let mut rng = Rng::from_parts(&[0xBA8C, world.turn as u64, tile.0 as u64, camp.spawned_units as u64]);
            let base = rng.range(8, 12) as i32;
            let base = if mods.raging { base / 2 } else { base };
            camp.countdown = (crate::stats::Fixed::whole(base + mods.difficulty_delay - camp.spawned_units.min(3))
                * mods.speed)
                .floor()
                .max(0) as u32;
        }
    }
    spawned
}
/// Target density is floor(mapSize^0.4), computed as an exact integer root.
fn fog_tiles_per_camp(size: usize) -> usize {
    let target = (size as u128).pow(2);
    let (mut lo, mut hi) = (1u128, size.max(1) as u128 + 1);
    while hi - lo > 1 {
        let mid = (lo + hi) / 2;
        if mid.pow(5) <= target {
            lo = mid
        } else {
            hi = mid
        }
    }
    lo as usize
}
fn replenish(world: &mut World, r: &Ruleset, barbarians: CivId) -> Vec<(CivId, TileId)> {
    let mut rng =
        Rng::from_parts(&[0xCA4F, world.turn as u64, barbarians.0 as u64, world.barbarian_camps.len() as u64]);
    // World.turn counts already-completed rounds; the first resolved round is zero.
    if world.turn > 0 && rng.range(0, 1) == 0 {
        return Vec::new();
    }
    let fog: Vec<_> = world
        .grid
        .ids()
        .filter(|t| {
            !world.grid.is_water(r, *t)
                && !world.civs.iter().any(|c| !c.is_barbarian && world.visibility.can_see(c.id, *t))
        })
        .collect();
    let existing = world.barbarian_camps.values().filter(|c| !c.destroyed).count();
    let desired = (fog.len() / fog_tiles_per_camp(world.grid.len())).saturating_sub(existing);
    let add = if world.turn == 0 { (desired / 3).max(1) } else { desired.min(1) };
    if add == 0 {
        return Vec::new();
    }
    let capitals: Vec<_> = world
        .civs
        .iter()
        .filter(|c| c.is_major() && c.is_alive())
        .filter_map(|c| crate::world::connections::capital_of(world, r, c.id))
        .filter_map(|id| world.cities.get(id))
        .map(|c| c.centre)
        .collect();
    let mut viable: Vec<_> = fog
        .into_iter()
        .filter(|t| {
            crate::world::movement::is_passable(&world.grid, r, *t)
                && world.grid.resource[t.0 as usize].is_none()
                && world.grid.features[t.0 as usize].iter().all(|id| {
                    r.terrains[*id]
                        .uniques
                        .iter()
                        .all(|u| u.unique_type() != Some(crate::UniqueType::RestrictedBuildableImprovements))
                })
                && world.grid.neighbours(*t).any(|n| !world.grid.is_water(r, n))
                && capitals.iter().all(|capital| world.grid.distance(*capital, *t) > 4)
                && world
                    .barbarian_camps
                    .iter()
                    .all(|(camp, state)| world.grid.distance(*camp, *t) > if state.destroyed { 4 } else { 7 })
        })
        .collect();
    let mut notifications = Vec::new();
    for _ in 0..add {
        if viable.is_empty() {
            break;
        }
        let coast = rng.range(0, 5) == 0;
        let coastal: Vec<_> = if coast {
            viable.iter().copied().filter(|t| world.grid.neighbours(*t).any(|n| world.grid.is_water(r, n))).collect()
        } else {
            Vec::new()
        };
        let choices = if coastal.is_empty() { &viable } else { &coastal };
        let tile = choices[rng.range(0, choices.len() as i64 - 1) as usize];
        if !place_camp(world, r, tile) {
            break;
        }
        let viewers: Vec<_> = world
            .civs
            .iter()
            .filter(|c| {
                world.visibility.has_explored(c.id, tile)
                    && c.uniques.has(crate::UniqueType::NotifiedOfBarbarianEncampments)
            })
            .map(|c| c.id)
            .collect();
        for civ in viewers {
            crate::world::fog::remember_improvement(world, r, civ, tile);
            notifications.push((civ, tile));
        }
        viable.retain(|t| world.grid.distance(tile, *t) > 7);
    }
    notifications
}
pub fn tick(world: &mut World, r: &Ruleset, barbarians: CivId, mods: CampModifiers) -> Vec<crate::world::TurnEvent> {
    synchronize(world, r);
    let mut events: Vec<_> = replenish(world, r, barbarians)
        .into_iter()
        .map(|(civ, tile)| crate::world::TurnEvent::BarbarianCampAppeared { civ, tile })
        .collect();
    events.extend(
        spawn_due(world, r, barbarians, mods)
            .into_iter()
            .map(|camp| crate::world::TurnEvent::BarbarianSpawned { camp }),
    );
    events
}
#[cfg(feature = "save")]
pub fn validate(world: &World, r: &Ruleset) -> Result<(), String> {
    for (tile, camp) in &world.barbarian_camps {
        if !world.grid.contains(*tile)
            || camp.countdown > 100_000
            || !(-1..=1_000_000).contains(&camp.spawned_units)
            || camp.destroyed && camp.countdown > 15
        {
            return Err("invalid barbarian camp clock".into());
        }
        if !camp.destroyed && world.grid.improvement[tile.0 as usize] != camp_improvement(r) {
            return Err("live barbarian camp lacks its improvement".into());
        }
    }
    Ok(())
}

/// Sustained severe unhappiness starts a saved countdown. Recovery cancels
/// it; a revolt consumes it before a later unhappy turn can start another.
pub fn revolt(world: &mut World, r: &Ruleset, civ: CivId, happiness: i32, speed: crate::stats::Fixed) -> Vec<Id<Unit>> {
    use crate::{
        stats::Fixed,
        world::{
            conditionals::{applies, GameContext},
            construction::{can_construct_in, Construction},
        },
        UniqueType,
    };
    let Some(owner) = world.civ(civ) else {
        return Vec::new();
    };
    let barb = world.civs.iter().find(|c| c.is_barbarian).map(|c| c.id);
    let ctx = GameContext::new(r).with_world(world).for_civ(civ).with_happiness(happiness);
    let eligible = owner.is_major()
        && owner.is_alive()
        && barb.is_some()
        && r.global_uniques
            .iter()
            .chain(owner.uniques.all())
            .any(|u| u.unique_type() == Some(UniqueType::SpawnRebels) && applies(u, &ctx).holds());
    if !eligible {
        world.civ_mut(civ).unwrap().revolt_countdown = None;
        return Vec::new();
    }
    let mut rng = Rng::from_parts(&[0x5245_564F, world.turn as u64, civ.0 as u64]);
    match owner.revolt_countdown {
        None => {
            world.civ_mut(civ).unwrap().revolt_countdown =
                Some((Fixed::whole(4 + rng.range(0, 2) as i32) * speed.max(Fixed::ONE)).floor().max(1) as u32);
            return Vec::new();
        }
        Some(n) if n > 1 => {
            world.civ_mut(civ).unwrap().revolt_countdown = Some(n - 1);
            return Vec::new();
        }
        _ => {}
    }
    world.civ_mut(civ).unwrap().revolt_countdown = None;
    let Some((_, city)) = world
        .cities
        .iter()
        .filter(|(_, c)| c.owner == civ)
        .max_by_key(|(_, c)| rng.range(0, i64::from(c.population) + 9))
    else {
        return Vec::new();
    };
    let origin = city
        .tiles
        .iter()
        .copied()
        .filter(|t| {
            *t != city.centre
                && !world.grid.is_water(r, *t)
                && crate::world::movement::is_passable(&world.grid, r, *t)
                && world.units_at(*t).is_empty()
        })
        .max_by_key(|t| {
            let index = t.0 as usize;
            10 + if world.grid.improvement[index].is_none() {
                4 + if world.grid.resource[index].is_some() { 3 } else { 0 }
            } else {
                0
            } + if world.grid.terrains(*t).any(|id| r.terrains[id].defence_bonus.unwrap_or(Fixed::ZERO) > Fixed::ZERO) {
                4
            } else {
                0
            }
        });
    let Some(origin) = origin else {
        return Vec::new();
    };
    let base = r
        .units
        .iter()
        .filter(|(id, b)| {
            b.unique_to.is_none()
                && b.strength.unwrap_or(0) > 0
                && b.ranged_strength.unwrap_or(0) == 0
                && r.unit_types.by_name(&b.unit_type).is_some_and(|t| t.movement_type == "Land")
                && !b.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::CannotAttack))
                && can_construct_in(world, r, civ, city, Construction::Unit(*id)).is_ok()
        })
        .max_by_key(|_| rng.range(0, 999))
        .map(|(id, _)| id);
    let Some(base) = base else {
        return Vec::new();
    };
    let count = 1 + rng.range(0, 99 + 20 * (world.civ(civ).unwrap().cities.len().saturating_sub(1)) as i64) / 100;
    let mut spawned = Vec::new();
    for _ in 0..count {
        if let Some(unit) = crate::world::construction::prepare_unit_near(world, r, barb.unwrap(), base, origin, None) {
            spawned.push(world.place_unit(unit));
        }
    }
    spawned
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::entities::Civilization;
    use crate::world::grid::TileGrid;

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("rules/gods-and-kings").expect("readable").expect("loads")
    }

    fn world(r: &Ruleset) -> World {
        let g = TileGrid::new(12, 12, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        let mut civs = vec![Civilization::new(CivId(0), nation, true), Civilization::new(CivId(1), nation, true)];
        civs[1].is_barbarian = true;
        let mut w = World::new(g, civs);
        w.diplomacy.mark_hostile(CivId(1));
        w
    }

    #[cfg(feature = "save")]
    #[test]
    fn inherited_technologies_clear_independent_barbarian_investment_before_save() {
        let r = ruleset();
        let mut w = world(&r);
        w.place_unit(Unit::new(r.unit("Warrior").unwrap(), CivId(0), TileId(20), 2));
        let agriculture = r.technologies.id("Agriculture").unwrap();
        let pottery = r.technologies.id("Pottery").unwrap();
        crate::world::research::grant_technology(w.civ_mut(CivId(0)).unwrap(), &r, agriculture);
        let barb = w.civ_mut(CivId(1)).unwrap();
        barb.researching = Some(agriculture);
        barb.science_invested.insert(agriculture, 4);
        barb.science_invested.insert(pottery, 2);
        barb.science_stored = 3;
        update_techs(&mut w, &r, CivId(1));
        let barb = w.civ(CivId(1)).unwrap();
        assert!(barb.has_tech(agriculture));
        assert!(!barb.has_tech(pottery));
        assert!(barb.science_invested.is_empty());
        assert_eq!(barb.researching, None);
        assert_eq!(barb.science_stored, 0);
        assert_eq!(w, crate::save::decode(&crate::save::encode(&w, &r).unwrap(), &r).unwrap());
    }

    #[test]
    fn the_camp_is_found_by_its_unique() {
        let r = ruleset();
        let camp = camp_improvement(&r).expect("the ruleset marks camps");
        assert_eq!(r.improvements[camp].name, "Barbarian encampment");
    }

    #[test]
    fn barbarians_are_at_war_with_everyone_and_cannot_be_reasoned_with() {
        let w = world(&ruleset());
        assert!(w.diplomacy.at_war(CivId(0), CivId(1)));
        assert!(w.diplomacy.at_war(CivId(1), CivId(0)), "both ways, as ever");

        let mut w = w;
        assert_eq!(w.diplomacy.make_peace(CivId(0), CivId(1)), None, "no treaty");
        assert!(w.diplomacy.at_war(CivId(0), CivId(1)), "still at war");
    }

    #[test]
    fn the_ruleset_supplies_a_barbarian_unit() {
        let r = ruleset();
        let unit = spawnable_unit(&r).expect("something can be spawned");
        println!("barbarians field: {}", r.units[unit].name);
        assert_eq!(r.units[unit].unique_to.as_deref(), Some("Barbarians"));
        assert!(r.units[unit].required_tech.is_none(), "and needs no research");
    }

    #[test]
    fn camps_produce_units_over_time() {
        let r = ruleset();
        let mut w = world(&r);
        place_camp(&mut w, &r, TileId(20));
        place_camp(&mut w, &r, TileId(50));
        assert_eq!(camps(&w, &r).len(), 2);

        let mut total = 0;
        for turn in 0..40 {
            w.turn = turn;
            // Clear the field so the camps are never blocked.
            for id in w.units.ids() {
                w.remove_unit(id);
            }
            total += spawn(&mut w, &r, CivId(1), DEFAULT_SPAWN_DELAY).len();
        }
        println!("{total} barbarians over 40 turns from 2 camps");
        assert!(total > 0, "camps should produce something");
        assert!(total < 40, "but not every turn");
    }

    #[test]
    fn a_camp_will_not_stack_units_on_itself() {
        let r = ruleset();
        let mut w = world(&r);
        place_camp(&mut w, &r, TileId(20));
        // Force a spawn turn, then try again with the tile occupied.
        let mut spawned_once = false;
        for turn in 0..40 {
            w.turn = turn;
            if !spawn(&mut w, &r, CivId(1), DEFAULT_SPAWN_DELAY).is_empty() {
                spawned_once = true;
                break;
            }
        }
        assert!(spawned_once, "it should have spawned at least once");
        let before = w.units.len();
        for turn in 0..40 {
            w.turn = turn;
            spawn(&mut w, &r, CivId(1), DEFAULT_SPAWN_DELAY);
        }
        assert!(
            w.units.len() >= before && w.units.len() <= 3,
            "nearby roaming units stay under the camp population limit"
        );
        assert_eq!(w.units_at(TileId(20)).len(), 1, "the camp tile never stacks units");
    }

    #[test]
    fn spawning_replays_identically() {
        let r = ruleset();
        let run = || {
            let mut w = world(&r);
            place_camp(&mut w, &r, TileId(20));
            let mut log = Vec::new();
            for turn in 0..30 {
                w.turn = turn;
                for id in w.units.ids() {
                    w.remove_unit(id);
                }
                log.push(spawn(&mut w, &r, CivId(1), DEFAULT_SPAWN_DELAY));
            }
            log
        };
        assert_eq!(run(), run());
    }
    #[test]
    fn a_camp_uses_its_saved_clock_and_scales_delay_after_raging() {
        use crate::stats::Fixed;
        let r = ruleset();
        let mut base = world(&r);
        let tile = TileId(20);
        place_camp(&mut base, &r, tile);
        let mut fast = base.clone();
        assert_eq!(spawn_due(&mut base, &r, CivId(1), CampModifiers::default()), vec![tile]);
        let ordinary = base.barbarian_camps[&tile].countdown;
        assert!((8..=12).contains(&ordinary));
        assert_eq!(base.barbarian_camps[&tile].spawned_units, 0);
        spawn_due(&mut fast, &r, CivId(1), CampModifiers { difficulty_delay: 2, speed: Fixed::whole(2), raging: true });
        assert_eq!(fast.barbarian_camps[&tile].countdown, (ordinary / 2 + 2) * 2);
        for id in base.units.ids() {
            base.remove_unit(id);
        }
        for _ in 0..ordinary {
            assert!(spawn_due(&mut base, &r, CivId(1), CampModifiers::default()).is_empty());
        }
        assert_eq!(base.barbarian_camps[&tile].countdown, 0);
        assert_eq!(spawn_due(&mut base, &r, CivId(1), CampModifiers::default()), vec![tile]);
        assert_eq!(base.barbarian_camps[&tile].spawned_units, 1);
        assert!((7..=11).contains(&base.barbarian_camps[&tile].countdown));
    }

    #[test]
    fn a_blocked_camp_retries_and_an_attack_halves_only_a_live_clock() {
        let r = ruleset();
        let mut w = world(&r);
        let tile = TileId(20);
        place_camp(&mut w, &r, tile);
        let blocker = w.place_unit(Unit::new(spawnable_unit(&r).unwrap(), CivId(1), tile, 2));
        assert!(spawn_due(&mut w, &r, CivId(1), CampModifiers::default()).is_empty());
        assert_eq!(w.barbarian_camps[&tile], Camp::default());
        w.remove_unit(blocker);
        assert_eq!(spawn_due(&mut w, &r, CivId(1), CampModifiers::default()), vec![tile]);
        w.barbarian_camps.get_mut(&tile).unwrap().countdown = 11;
        on_camp_attacked(&mut w, tile);
        assert_eq!(w.barbarian_camps[&tile].countdown, 5);
        mark_destroyed(&mut w, tile);
        on_camp_attacked(&mut w, tile);
        assert_eq!(w.barbarian_camps[&tile].countdown, 15);
    }

    #[test]
    fn destruction_leaves_a_fifteen_turn_ghost_without_restarting_it() {
        let r = ruleset();
        let mut w = world(&r);
        let tile = TileId(20);
        place_camp(&mut w, &r, tile);
        w.grid.set_improvement(tile, None);
        crate::world::commands::finish_action(&mut w, &r, &mut Vec::new());
        assert_eq!(w.barbarian_camps[&tile].countdown, 15);
        assert!(w.barbarian_camps[&tile].destroyed);
        #[cfg(feature = "save")]
        crate::save::encode(&w, &r).expect("action settlement leaves a valid snapshot");
        for left in (0..15).rev() {
            synchronize(&mut w, &r);
            assert!(spawn_due(&mut w, &r, CivId(1), CampModifiers::default()).is_empty());
            assert_eq!(w.barbarian_camps[&tile].countdown, left);
        }
        synchronize(&mut w, &r);
        assert!(!w.barbarian_camps.contains_key(&tile));
    }

    #[test]
    fn camps_replenish_out_of_sight_and_honor_only_updates_known_improvements() {
        let r = ruleset();
        let mut w = world(&r);
        let tile = TileId(70);
        w.turn = 0;
        let honor = r
            .policies
            .iter()
            .flat_map(|(_, p)| &p.uniques)
            .find(|u| u.unique_type() == Some(crate::UniqueType::NotifiedOfBarbarianEncampments))
            .unwrap()
            .clone();
        w.civ_mut(CivId(0)).unwrap().uniques.insert(honor);
        for id in w.grid.ids() {
            if id != tile {
                w.visibility.observe(CivId(0), id);
            }
        }
        w.visibility.explore(CivId(0), tile);
        crate::world::fog::capture_observations(&mut w, &r);
        w.grid.owner[tile.0 as usize] = Some(CivId(1));
        let events = tick(&mut w, &r, CivId(1), CampModifiers::default());
        assert_eq!(camps(&w, &r), vec![tile]);
        assert!(events.contains(&crate::world::TurnEvent::BarbarianCampAppeared { civ: CivId(0), tile }));
        assert!(!w.visibility.can_see(CivId(0), tile));
        let known = w.visibility.known_tiles(CivId(0)).find(|(id, _)| *id == tile).unwrap().1;
        assert_eq!(known.improvement, camp_improvement(&r));
        assert_eq!(known.owner, None, "an Honor notice does not refresh hidden ownership");
        assert_eq!(w.units_at(tile).len(), 1, "a new camp gets its first defender immediately");
    }

    #[test]
    fn replenishment_respects_live_camps_ghosts_resources_and_capitals() {
        use crate::world::City;
        let r = ruleset();
        let mut w = World::new(TileGrid::new(40, 20, r.terrain("Grassland").unwrap()), world(&r).civs);
        let centre = TileId(45);
        w.found_city_in(&r, City::found("Capital", CivId(0), centre));
        w.turn = 0;
        let camp = TileId(39);
        place_camp(&mut w, &r, camp);
        let ghost = TileId(760);
        w.barbarian_camps.insert(ghost, Camp { countdown: 15, destroyed: true, ..Default::default() });
        let resources: Vec<_> = w.grid.ids().filter(|t| t.0 % 5 == 0).collect();
        for tile in &resources {
            w.grid.resource[tile.0 as usize] = r.resources.id("Iron");
        }
        replenish(&mut w, &r, CivId(1));
        assert!(camps(&w, &r).len() > 1, "there is room for new camps outside every exclusion zone");
        for new in camps(&w, &r).into_iter().filter(|t| *t != camp) {
            assert!(w.grid.distance(new, centre) > 4);
            assert!(w.grid.distance(new, camp) > 7);
            assert!(w.grid.distance(new, ghost) > 4);
            assert!(!resources.contains(&new));
        }
        assert_eq!(fog_tiles_per_camp(1000), 15);
    }

    #[cfg(feature = "save")]
    #[test]
    fn camp_clocks_roundtrip_and_continue_identically() {
        let r = ruleset();
        let mut w = world(&r);
        let tile = TileId(20);
        place_camp(&mut w, &r, tile);
        spawn_due(&mut w, &r, CivId(1), CampModifiers::default());
        w.barbarian_camps.insert(TileId(100), Camp { countdown: 7, destroyed: true, ..Default::default() });
        let text = crate::save::encode(&w, &r).unwrap();
        let mut restored = crate::save::decode(&text, &r).unwrap();
        assert_eq!(restored, w);
        for turn in 10..35 {
            w.turn = turn;
            restored.turn = turn;
            assert_eq!(
                tick(&mut w, &r, CivId(1), CampModifiers::default()),
                tick(&mut restored, &r, CivId(1), CampModifiers::default())
            );
            assert_eq!(restored, w);
        }
        restored.barbarian_camps.get_mut(&tile).unwrap().countdown = 100_001;
        assert!(crate::save::validate(&restored, &r).is_err());
    }

    #[test]
    fn moving_into_a_camp_clears_it_and_pays_once() {
        let r = ruleset();
        let mut w = world(&r);
        let from = TileId(20);
        let to = w.grid.neighbours(from).next().unwrap();
        let unit = w.place_unit(Unit::new(r.unit("Warrior").unwrap(), CivId(0), from, 2));
        place_camp(&mut w, &r, to);
        let events = crate::world::commands::execute(
            &mut w,
            &r,
            CivId(0),
            crate::world::commands::Command::MoveUnit { unit, to },
        )
        .unwrap();
        assert!(events.events.iter().any(|e| matches!(e, crate::world::TurnEvent::CampCleared { gold: 25, .. })));
        assert_eq!(w.grid.improvement[to.0 as usize], None);
        assert_eq!(w.civ(CivId(0)).unwrap().gold, 25);
        assert!(on_enter(&mut w, &r, unit, crate::world::TurnModifiers::default()).is_none());
        assert_eq!(w.civ(CivId(0)).unwrap().gold, 25);
    }

    #[test]
    fn barbarian_occupants_keep_camps_and_songhai_gets_triple_gold() {
        let r = ruleset();
        let mut w = world(&r);
        let tile = TileId(20);
        let barb = w.place_unit(Unit::new(spawnable_unit(&r).unwrap(), CivId(1), tile, 2));
        place_camp(&mut w, &r, tile);
        assert!(on_enter(&mut w, &r, barb, crate::world::TurnModifiers::default()).is_none());
        w.remove_unit(barb);
        w.civ_mut(CivId(0)).unwrap().nation = r.nations.id("Songhai").unwrap();
        crate::world::policies::rebuild_uniques(&mut w, &r, CivId(0));
        let unit = w.place_unit(Unit::new(r.unit("Warrior").unwrap(), CivId(0), tile, 2));
        assert_eq!(on_enter(&mut w, &r, unit, crate::world::TurnModifiers::default()).unwrap().gold, 75);
    }

    #[test]
    fn german_recruitment_is_deterministic_and_new_recruit_cannot_move() {
        let r = ruleset();
        let mut w = world(&r);
        let tile = TileId(20);
        w.civ_mut(CivId(0)).unwrap().nation = r.nations.id("Germany").unwrap();
        crate::world::policies::rebuild_uniques(&mut w, &r, CivId(0));
        let unit = w.place_unit(Unit::new(r.unit("Warrior").unwrap(), CivId(0), tile, 2));
        place_camp(&mut w, &r, tile);
        let mut success = false;
        let mut failure = false;
        for turn in 0..40 {
            let mut a = w.clone();
            a.turn = turn;
            let mut b = a.clone();
            let outcome = on_enter(&mut a, &r, unit, crate::world::TurnModifiers::default()).unwrap();
            assert_eq!(Some(outcome.clone()), on_enter(&mut b, &r, unit, crate::world::TurnModifiers::default()));
            assert_eq!(a, b);
            if let Some(id) = outcome.recruited {
                success = true;
                assert_eq!(outcome.gold, 50);
                let u = a.units.get(id).unwrap();
                assert_eq!(u.health, 100);
                assert_eq!(u.movement_left, crate::stats::Fixed::ZERO);
            } else {
                failure = true;
                assert_eq!(outcome.gold, 25);
            }
        }
        assert!(success && failure);
    }
}
