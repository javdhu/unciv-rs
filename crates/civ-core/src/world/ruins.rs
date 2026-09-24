//! Ancient ruins (`RuinsManager`, `MapGenerator.spreadAncientRuins`).
//!
//! About one land tile in forty holds ruins, spread out, and none within
//! three of where anyone starts. The first major civ's unit to stand on them
//! clears them and gets a reward from `Ruins.json`: the rewards it may have —
//! not excluded by difficulty, not one of the last two it got, and whose
//! `Only available`/`Unavailable` conditions allow — are shuffled, and the
//! first that actually does something is the one it gets. A free tech with
//! nothing left in the era, or an upgrade for a unit with none, falls
//! through to the next.
//!
//! What each reward does is read from its uniques, not its name, so a mod's
//! rewards work if they use the same ones.

use crate::arena::Id;
use crate::ruleset::{BaseUnit, Difficulty, RuinReward, Ruleset, TileImprovement};
use crate::stats::Fixed;
use crate::world::conditionals::{applies, GameContext, Verdict};
use crate::world::entities::Unit;
use crate::world::grid::{CivId, TileGrid, TileId};
use crate::world::world::World;
use crate::{rng::Rng, UniqueType};

/// `ancientRuinCountMultiplier`.
const COUNT_PER_THOUSAND: i64 = 25;
/// Ruins this close to a start are cleared before the game begins.
const START_CLEARANCE: u32 = 3;
/// How many recent rewards a civ will not be given again.
const NO_REPEAT: usize = 2;

/// The improvement that is ruins, in this ruleset.
pub fn ruins_improvement(r: &Ruleset) -> Option<Id<TileImprovement>> {
    r.improvements
        .iter()
        .find(|(_, i)| i.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::IsAncientRuinsEquivalent)))
        .map(|(id, _)| id)
}

pub fn is_ruins(r: &Ruleset, improvement: Id<TileImprovement>) -> bool {
    r.improvements
        .get(improvement)
        .is_some_and(|i| i.uniques.iter().any(|u| u.unique_type() == Some(UniqueType::IsAncientRuinsEquivalent)))
}

/// Scatter ruins over a new map.
pub fn place(r: &Ruleset, grid: &mut TileGrid, seed: u64, starts: &[TileId]) {
    let equivalents: Vec<_> = r.improvements.ids().filter(|id| is_ruins(r, *id)).collect();
    if equivalents.is_empty() {
        return;
    }
    let fits = |grid: &TileGrid, tile: TileId, improvement| {
        r.improvements.get(improvement).is_some_and(|i| {
            i.terrains_can_be_built_on
                .iter()
                .any(|filter| crate::world::conditionals::tile_matches_in(grid, r, tile, filter, None) == Verdict::True)
        })
    };
    let mut candidates: Vec<TileId> = grid
        .ids()
        .filter(|t| {
            let i = t.0 as usize;
            grid.improvement[i].is_none()
                && grid.natural_wonder[i].is_none()
                && !grid.is_water(r, *t)
                && crate::world::movement::is_passable(grid, r, *t)
                && equivalents.iter().any(|id| fits(grid, *t, *id))
        })
        .collect();
    let count = ((candidates.len() as i64 * COUNT_PER_THOUSAND + 500) / 1000) as usize;
    if count == 0 {
        return;
    }
    let mut rng = Rng::from_parts(&[seed, 0x2A_1B5]);
    shuffle(&mut candidates, &mut rng);

    // Spread out (`chooseSpreadOutLocations`): as far apart as the land
    // allows, closing in only when that runs out.
    let mut spacing = isqrt(candidates.len() / count).max(1) as u32;
    let mut chosen: Vec<TileId> = Vec::new();
    while chosen.len() < count && spacing > 0 {
        for t in &candidates {
            if chosen.len() >= count {
                break;
            }
            if chosen.iter().all(|c| grid.distance(*c, *t) >= spacing) {
                chosen.push(*t);
            }
        }
        spacing -= 1;
    }
    for t in chosen {
        if starts.iter().all(|s| grid.distance(*s, t) > START_CLEARANCE) {
            let options: Vec<_> = equivalents.iter().copied().filter(|id| fits(grid, t, *id)).collect();
            let improvement = options[rng.range(0, options.len() as i64 - 1) as usize];
            grid.set_improvement(t, Some(improvement));
        }
    }
}

fn isqrt(n: usize) -> usize {
    let mut x = 0;
    while (x + 1) * (x + 1) <= n {
        x += 1;
    }
    x
}

fn shuffle<T>(v: &mut [T], rng: &mut Rng) {
    for i in (1..v.len()).rev() {
        let j = rng.range(0, i as i64) as usize;
        v.swap(i, j);
    }
}

/// What a civ found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RuinsFound {
    pub civ: CivId,
    pub tile: TileId,
    pub reward: Id<RuinReward>,
}

/// What a game's settings say, that ruins need.
#[derive(Debug, Clone, Copy)]
pub struct Setting<'a> {
    pub difficulty: Option<&'a Difficulty>,
    /// Speed's `modifier`, for rewards "(modified by game speed)".
    pub speed: Fixed,
    pub gold_cost: Fixed,
    pub culture_cost: Fixed,
    pub science_cost: Fixed,
    /// Speed's `faithCostModifier`.
    pub faith_cost: Fixed,
}

/// A unit stands on a tile; if ruins are there and it is a major civ's, it
/// clears them and is rewarded (`MapUnit.getAncientRuinBonus`).
pub fn on_enter(world: &mut World, r: &Ruleset, unit: Id<Unit>, s: Setting) -> Option<RuinsFound> {
    let u = world.units.get(unit)?;
    let (civ, tile) = (u.owner, u.position);
    if !world.grid.improvement[tile.0 as usize].is_some_and(|i| is_ruins(r, i))
        || !world.civ(civ).is_some_and(|c| c.is_major())
    {
        return None;
    }
    world.grid.set_improvement(tile, None);

    let recent = world.civ(civ).map(|c| c.recent_ruins.clone()).unwrap_or_default();
    let mut options: Vec<Id<RuinReward>> = r
        .ruin_rewards
        .iter()
        .filter(|(_, rw)| !recent.contains(&rw.name))
        .filter(|(_, rw)| s.difficulty.is_none_or(|d| !rw.excluded_difficulties.contains(&d.name)))
        .filter(|(_, rw)| available(world, r, civ, unit, rw, s))
        .flat_map(|(id, rw)| std::iter::repeat_n(id, rw.weight.unwrap_or(1).max(0) as usize))
        .collect();
    // Stable for this tile: delaying a visit must not reroll its rewards.
    let mut rng = Rng::from_parts(&[0x2A_1B5, tile.0 as u64]);
    shuffle(&mut options, &mut rng);
    for id in options {
        let Some(rw) = r.ruin_rewards.get(id) else { continue };
        let mut any = false;
        for u in &rw.uniques {
            let ctx = context(world, r, civ, unit, s);
            if effect_applies(u, &ctx) {
                any |= grant(world, r, civ, unit, tile, u, s, &mut rng);
            }
        }
        if any {
            if let Some(c) = world.civ_mut(civ) {
                c.recent_ruins.push(rw.name.clone());
                if c.recent_ruins.len() > NO_REPEAT {
                    c.recent_ruins.remove(0);
                }
            }
            return Some(RuinsFound { civ, tile, reward: id });
        }
    }
    None
}

fn context<'a>(world: &'a World, r: &'a Ruleset, civ: CivId, unit: Id<Unit>, s: Setting<'a>) -> GameContext<'a> {
    let mut ctx = GameContext::new(r).with_world(world).for_civ(civ).for_unit(unit);
    ctx.tile = world.units.get(unit).map(|u| u.position);
    ctx.difficulty = s.difficulty;
    ctx
}

fn effect_applies(u: &crate::Unique, ctx: &GameContext) -> bool {
    u.conditionals.iter().all(|c| {
        c.unique_type() == Some(UniqueType::ModifiedByGameSpeed)
            || crate::world::conditionals::evaluate(c, ctx) == Verdict::True
    })
}

fn available(world: &World, r: &Ruleset, civ: CivId, unit: Id<Unit>, rw: &RuinReward, s: Setting) -> bool {
    let ctx = context(world, r, civ, unit, s);
    rw.uniques.iter().all(|u| match u.unique_type() {
        Some(UniqueType::OnlyAvailable) => applies(u, &ctx) == Verdict::True,
        Some(UniqueType::Unavailable) => applies(u, &ctx) != Verdict::True,
        _ => true,
    })
}

fn param(u: &crate::Unique, i: usize) -> Option<&str> {
    u.params.get(i).map(String::as_str)
}

fn by_speed(u: &crate::Unique, amount: i64, stat: &str, s: Setting) -> i64 {
    let scaled = u.conditionals.iter().any(|c| c.unique_type() == Some(UniqueType::ModifiedByGameSpeed));
    if scaled {
        let speed = match stat {
            "Gold" => s.gold_cost,
            "Culture" => s.culture_cost,
            "Science" => s.science_cost,
            "Faith" => s.faith_cost,
            _ => s.speed,
        };
        (Fixed::whole(amount as i32) * speed).round()
    } else {
        amount
    }
}

fn add_stat(world: &mut World, civ: CivId, stat: &str, amount: i64) -> bool {
    let Some(c) = world.civ_mut(civ) else { return false };
    let n = amount as i32;
    match stat {
        "Gold" => c.gold += n,
        "Culture" => c.add_culture(n),
        "Science" => c.science_stored += n,
        "Faith" => c.add_faith(n),
        _ => return false,
    }
    n != 0
}

/// Resolve a nation's replacement before granting or upgrading a unit.
fn equivalent_unit(world: &World, r: &Ruleset, civ: CivId, base: Id<BaseUnit>) -> Id<BaseUnit> {
    let nation = world.civ(civ).and_then(|c| r.nations.name_of(c.nation));
    let name = r.units.name_of(base);
    r.units
        .iter()
        .find(|(_, u)| {
            u.unique_to.as_deref() == nation && nation.is_some() && u.replaces.as_deref() == name && name.is_some()
        })
        .map_or(base, |(id, _)| id)
}

fn valid_terrain(world: &World, r: &Ruleset, unit: &Unit, tile: TileId) -> bool {
    crate::world::movement::is_passable(&world.grid, r, tile)
        && if unit.is_water_unit(r) {
            world.grid.is_water(r, tile) || world.cities.iter().any(|(_, c)| c.centre == tile && c.owner == unit.owner)
        } else if unit.is_air(r) {
            world.cities.iter().any(|(_, c)| c.centre == tile && c.owner == unit.owner)
        } else {
            !world.grid.is_water(r, tile)
        }
}

/// Search connected legal tiles, as `TileMap.placeUnitNearTile` does. Occupied
/// tiles can be crossed, but never used as an invalid fallback destination.
fn placement_near(world: &World, r: &Ruleset, unit: &Unit, centre: TileId) -> Option<TileId> {
    let mut seen = std::collections::BTreeSet::from([centre]);
    let mut frontier = vec![centre];
    for _ in 0..=10 {
        for t in &frontier {
            if valid_terrain(world, r, unit, *t)
                && crate::world::movement::can_pass_through(world, r, *t, unit)
                && !crate::world::movement::is_tile_occupied_for(world, r, *t, unit)
            {
                return Some(*t);
            }
        }
        let mut next = Vec::new();
        for t in frontier {
            for n in world.grid.neighbours(t) {
                if seen.insert(n)
                    && valid_terrain(world, r, unit, n)
                    && crate::world::movement::can_pass_through(world, r, n, unit)
                {
                    next.push(n);
                }
            }
        }
        next.sort();
        frontier = next;
        if frontier.is_empty() {
            break;
        }
    }
    None
}

fn special_upgrade(world: &mut World, r: &Ruleset, unit: Id<Unit>, s: Setting) -> bool {
    let Some(old) = world.units.get(unit) else { return false };
    let Some(base) = r.units.get(old.base) else { return false };
    let ctx = context(world, r, old.owner, unit, s);
    let special =
        base.uniques.iter().find(|u| u.unique_type() == Some(UniqueType::RuinsUpgrade) && effect_applies(u, &ctx));
    let names: Vec<&str> = if let Some(special) = special {
        special.params.first().map(String::as_str).into_iter().collect()
    } else {
        base.upgrades_to
            .as_deref()
            .into_iter()
            .chain(
                base.uniques
                    .iter()
                    .filter(|u| u.unique_type() == Some(UniqueType::CanUpgrade) && effect_applies(u, &ctx))
                    .filter_map(|u| u.params.first().map(String::as_str)),
            )
            .collect()
    };
    let to = names
        .into_iter()
        .filter_map(|name| r.units.id(name))
        .map(|id| equivalent_unit(world, r, old.owner, id))
        .filter(|id| *id != old.base)
        .min_by_key(|id| (r.units.get(*id).and_then(|b| b.cost).unwrap_or(0), *id));
    let Some(to) = to else { return false };
    let mut upgraded = old.clone();
    upgraded.base = to;
    let position = if valid_terrain(world, r, &upgraded, old.position) && upgraded.is_civilian(r) == old.is_civilian(r)
    {
        Some(old.position)
    } else {
        placement_near(world, r, &upgraded, old.position)
    };
    let Some(position) = position else { return false };
    upgraded.position = position;
    upgraded.movement_left = Fixed::ZERO;
    // Base promotions remain with the old unit when it upgrades. In
    // particular a Scout keeps Ignore terrain cost after becoming an Archer.
    for name in base.promotions.iter().chain(r.units.get(to).into_iter().flat_map(|b| b.promotions.iter())) {
        if let Some(id) = r.promotions.id(name) {
            upgraded.promotions.insert(id);
        }
    }
    if let Some(x) = world.units.get_mut(unit) {
        *x = upgraded;
    }
    true
}

/// Carry out one of a reward's uniques. Whether it did anything.
#[allow(clippy::too_many_arguments)]
fn grant(
    world: &mut World,
    r: &Ruleset,
    civ: CivId,
    unit: Id<Unit>,
    tile: TileId,
    u: &crate::Unique,
    s: Setting,
    rng: &mut Rng,
) -> bool {
    let num = |i: usize| param(u, i).and_then(|p| p.trim_start_matches('+').parse::<i64>().ok());
    match u.unique_type() {
        Some(UniqueType::OneTimeGainStat) => {
            let (Some(n), Some(stat)) = (num(0), param(u, 1)) else { return false };
            add_stat(world, civ, stat, by_speed(u, n, stat, s))
        }
        Some(UniqueType::OneTimeGainStatRange) => {
            let (Some(lo), Some(hi), Some(stat)) = (num(0), num(1), param(u, 2)) else { return false };
            let n = rng.range(lo.min(hi), hi.max(lo));
            add_stat(world, civ, stat, by_speed(u, n, stat, s))
        }
        Some(UniqueType::OneTimeFreeUnitRuins) => {
            let Some(base) = param(u, 0).and_then(|n| r.units.id(n)) else { return false };
            let base = equivalent_unit(world, r, civ, base);
            let movement = r.units.get(base).map_or(2, |b| b.movement);
            let mut new_unit = Unit::new(base, civ, tile, movement);
            let Some(spot) = placement_near(world, r, &new_unit, tile) else { return false };
            new_unit.position = spot;
            crate::world::military::initialize_promotions(world, r, &mut new_unit);
            let id = world.place_unit(new_unit);
            crate::world::luminaries::name_unit(world, r, id);
            true
        }
        Some(UniqueType::OneTimeUnitGainXP) => {
            let Some(n) = num(1) else { return false };
            let Some(x) = world.units.get_mut(unit) else { return false };
            x.xp = x.xp.saturating_add(n as _);
            true
        }
        Some(UniqueType::OneTimeGainPopulationRandomCity) => {
            let Some(n) = num(0) else { return false };
            let cities = world.civ(civ).map(|c| c.cities.clone()).unwrap_or_default();
            if cities.is_empty() {
                return false;
            }
            let city = cities[rng.range(0, cities.len() as i64 - 1) as usize];
            let Some(c) = world.cities.get_mut(city) else { return false };
            let previous = c.population;
            c.population = (i64::from(c.population) + n).clamp(1, i64::from(u32::MAX)) as u32;
            let change = c.population as i64 - previous as i64;
            c.faith.on_population_change(change as i32, c.population);
            change != 0
        }
        Some(UniqueType::OneTimeFreeTechRuins) => {
            let (Some(n), Some(era)) = (num(0), param(u, 1)) else { return false };
            let Some(c) = world.civ(civ) else { return false };
            let mut techs: Vec<_> = crate::world::research::available_techs(r, c)
                .into_iter()
                .filter(|t| r.technologies.get(*t).is_some_and(|x| era == "All" || x.era_name(r) == Some(era)))
                .collect();
            shuffle(&mut techs, rng);
            techs.truncate(n.max(0) as usize);
            for t in &techs {
                if let Some(c) = world.civ_mut(civ) {
                    crate::world::research::grant_technology(c, r, *t);
                }
                crate::world::effects::on_technology_researched(world, r, civ, *t, s.speed);
            }
            !techs.is_empty()
        }
        Some(UniqueType::OneTimeUnitSpecialUpgrade) => special_upgrade(world, r, unit, s),
        Some(UniqueType::OneTimeRevealSpecificMapTiles) => {
            let (Some(filter), Some(radius)) = (param(u, 1), num(2)) else { return false };
            let mut found: Vec<TileId> = world
                .grid
                .within(tile, radius as u32)
                .into_iter()
                .filter(|t| {
                    world.grid.improvement[t.0 as usize].and_then(|id| r.improvements.name_of(id)) == Some(filter)
                        || crate::world::conditionals::tile_matches_in(&world.grid, r, *t, filter, Some(civ))
                            == Verdict::True
                })
                .filter(|t| !world.visibility.has_explored(civ, *t))
                .collect();
            if !param(u, 0).is_some_and(|n| n.eq_ignore_ascii_case("All")) {
                let Some(amount) = num(0) else { return false };
                shuffle(&mut found, rng);
                found.truncate(amount.max(0) as usize);
            }
            for t in &found {
                world.visibility.explore(civ, *t);
                world.visibility.remember(
                    civ,
                    *t,
                    crate::world::fog::TileMemory {
                        improvement: world.grid.improvement[t.0 as usize],
                        ..Default::default()
                    },
                );
            }
            !found.is_empty()
        }
        Some(UniqueType::OneTimeRevealCrudeMap) => {
            let (Some(away), Some(radius), Some(chance)) = (num(0), num(1), num(2)) else { return false };
            let ring: Vec<TileId> = world
                .grid
                .within(tile, away as u32)
                .into_iter()
                .filter(|t| world.grid.distance(tile, *t) == away as u32 && !world.visibility.has_explored(civ, *t))
                .collect();
            if ring.is_empty() {
                return false;
            }
            let centre = ring[rng.range(0, ring.len() as i64 - 1) as usize];
            for t in world.grid.within(centre, radius as u32) {
                if rng.range(1, 100) <= chance {
                    world.visibility.explore(civ, t);
                }
            }
            true
        }
        Some(UniqueType::OneTimeGainPantheon) => {
            let Some(c) = world.civ(civ) else { return false };
            if c.faith.has_pantheon() {
                return false;
            }
            let with = world.civs.iter().filter(|c| c.is_major() && c.faith.has_pantheon()).count() as i32;
            let cost = crate::world::religion::PANTHEON_BASE + (2 + with) * crate::world::religion::PANTHEON_GROWTH;
            add_stat(world, civ, "Faith", (Fixed::whole(cost) * s.faith_cost).round())
        }
        Some(UniqueType::OneTimeGainProphet) => {
            let Some(pct) = num(0) else { return false };
            let earned = world.civ(civ).map_or(0, |c| c.faith.prophets_earned);
            let cost = crate::world::religion::prophet_cost(earned, s.faith_cost) as i64;
            add_stat(world, civ, "Faith", cost * pct / 100)
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::city::City;
    use crate::world::entities::Civilization;

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("rules/gods-and-kings").expect("readable").expect("loads")
    }

    fn setting() -> Setting<'static> {
        Setting {
            difficulty: None,
            speed: Fixed::ONE,
            faith_cost: Fixed::ONE,
            gold_cost: Fixed::ONE,
            culture_cost: Fixed::ONE,
            science_cost: Fixed::ONE,
        }
    }

    fn world(r: &Ruleset) -> (World, Id<Unit>, TileId) {
        let g = TileGrid::new(20, 12, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        let mut w = World::new(g, vec![Civilization::new(CivId(0), nation, true)]);
        w.found_city(City::found("Home", CivId(0), w.grid.id_at(2, 2).unwrap()));
        let t = w.grid.id_at(10, 6).unwrap();
        w.grid.improvement[t.0 as usize] = ruins_improvement(r);
        let scout = r.units.id("Scout").unwrap();
        let u = w.place_unit(Unit::new(scout, CivId(0), t, 2));
        (w, u, t)
    }

    #[test]
    fn the_ruleset_carries_its_rewards() {
        let r = ruleset();
        assert_eq!(r.ruin_rewards.len(), 12);
        assert!(ruins_improvement(&r).is_some());
    }

    #[test]
    fn entering_ruins_clears_them_and_rewards_the_finder() {
        let r = ruleset();
        let (mut w, u, t) = world(&r);
        let found = on_enter(&mut w, &r, u, setting()).expect("a reward");
        assert_eq!(w.grid.improvement[t.0 as usize], None);
        assert_eq!(w.civ(CivId(0)).unwrap().recent_ruins, vec![r.ruin_rewards.get(found.reward).unwrap().name.clone()]);
        assert_eq!(on_enter(&mut w, &r, u, setting()), None, "cleared, so nothing twice");
    }

    #[test]
    fn a_civ_is_not_given_the_same_reward_three_times_running() {
        let r = ruleset();
        let (mut w, u, t) = world(&r);
        let mut names = Vec::new();
        for turn in 0..12 {
            w.turn = turn;
            w.grid.improvement[t.0 as usize] = ruins_improvement(&r);
            let f = on_enter(&mut w, &r, u, setting()).expect("something is always left");
            names.push(r.ruin_rewards.get(f.reward).unwrap().name.clone());
        }
        for win in names.windows(3) {
            assert!(win[0] != win[1] && win[1] != win[2] && win[0] != win[2], "{names:?}");
        }
    }

    #[test]
    fn early_rewards_wait_on_their_turn_and_difficulty_excludes_squatters() {
        let r = ruleset();
        let (w, u, _) = world(&r);
        let training = r.ruin_rewards.iter().find(|(_, x)| x.name.contains("training")).unwrap().1;
        assert!(!available(&w, &r, CivId(0), u, training, setting()), "not before turn ten");
        let mut w = w;
        w.turn = 10;
        assert!(available(&w, &r, CivId(0), u, training, setting()));
        let symbols = r.ruin_rewards.iter().find(|(_, x)| x.name.contains("holy symbols")).unwrap().1;
        w.turn = 25;
        assert!(available(&w, &r, CivId(0), u, symbols, setting()));
    }

    #[test]
    fn a_new_map_has_ruins_spread_out_and_none_by_a_start() {
        use crate::world::mapgen::{generate, MapParameters};
        let r = ruleset();
        let map = generate(&r, MapParameters { width: 40, height: 26, seed: 7, ..Default::default() }, 4);
        let ruins = ruins_improvement(&r);
        let at: Vec<TileId> = map.grid.ids().filter(|t| map.grid.improvement[t.0 as usize] == ruins).collect();
        assert!(at.len() >= 5, "{}", at.len());
        for s in &map.starts {
            assert!(at.iter().all(|t| map.grid.distance(*s, *t) > START_CLEARANCE));
        }
    }

    fn one_reward(r: &mut Ruleset, uniques: &[&str]) {
        r.ruin_rewards = Default::default();
        r.ruin_rewards.insert(
            "Reward",
            RuinReward {
                name: "Reward".into(),
                uniques: uniques.iter().map(|s| crate::Unique::parse(s)).collect(),
                ..Default::default()
            },
        );
    }

    fn give(w: &mut World, r: &Ruleset, unit: Id<Unit>, text: &str) -> bool {
        let tile = w.units.get(unit).unwrap().position;
        grant(w, r, CivId(0), unit, tile, &crate::Unique::parse(text), setting(), &mut Rng::from_parts(&[7]))
    }

    #[test]
    fn disabled_and_unknown_conditional_rewards_are_not_granted() {
        let mut r = ruleset();
        one_reward(&mut r, &["Gain [99] [Gold] <after turn number [20]>"]);
        let (mut w, u, _) = world(&r);
        assert!(on_enter(&mut w, &r, u, setting()).is_none());
        assert_eq!(w.civ(CivId(0)).unwrap().gold, 0);
        one_reward(&mut r, &["Gain [99] [Gold]", "Only available <an unsupported condition>"]);
        let (mut w, u, _) = world(&r);
        assert!(on_enter(&mut w, &r, u, setting()).is_none());
    }

    #[test]
    fn reward_weights_are_loaded_and_zero_weight_is_never_selected() {
        let reward = RuinReward::from_json(
            &crate::json::Json::parse(
                r#"{"name":"Disabled", "weight":0,
            "uniques":["Gain [99] [Gold]"]}"#,
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(reward.weight, Some(0));
        let mut r = ruleset();
        r.ruin_rewards = Default::default();
        r.ruin_rewards.insert("Disabled", reward);
        let (mut w, u, _) = world(&r);
        assert!(on_enter(&mut w, &r, u, setting()).is_none());
        assert_eq!(w.civ(CivId(0)).unwrap().gold, 0);
    }

    #[test]
    fn heavier_rewards_are_more_common_across_ruin_tiles() {
        let mut r = ruleset();
        one_reward(&mut r, &["Gain [1] [Culture]"]);
        r.ruin_rewards.insert(
            "Gold",
            RuinReward {
                name: "Gold".into(),
                weight: Some(20),
                uniques: vec![crate::Unique::parse("Gain [1] [Gold]")],
                ..Default::default()
            },
        );
        let (mut w, unit, _) = world(&r);
        for tile in w.grid.ids().collect::<Vec<_>>() {
            w.units.get_mut(unit).unwrap().position = tile;
            w.civ_mut(CivId(0)).unwrap().recent_ruins.clear();
            w.grid.set_improvement(tile, ruins_improvement(&r));
            assert!(on_enter(&mut w, &r, unit, setting()).is_some());
        }
        let civ = w.civ(CivId(0)).unwrap();
        assert!(civ.gold > civ.culture_stored * 5, "a weight of twenty must change the actual rewards");
    }

    #[test]
    fn scout_uses_its_special_upgrade_and_keeps_terrain_promotion() {
        let r = ruleset();
        let (mut w, u, _) = world(&r);
        w.units.get_mut(u).unwrap().xp = 13;
        assert!(give(&mut w, &r, u, "[This Unit] upgrades for free including special upgrades"));
        let upgraded = w.units.get(u).unwrap();
        assert_eq!(upgraded.base, r.units.id("Archer").unwrap());
        assert_eq!(upgraded.xp, 13);
        assert_eq!(upgraded.movement_left, Fixed::ZERO);
        assert!(upgraded.has_unique(&r, UniqueType::IgnoresTerrainCost));
    }

    #[test]
    fn free_worker_shares_a_military_tile_but_fails_if_every_civilian_slot_is_blocked() {
        let r = ruleset();
        let (mut w, u, tile) = world(&r);
        assert!(give(&mut w, &r, u, "Free [Worker] found in the ruins"));
        assert!(w.units.iter().any(|(_, x)| x.position == tile && x.is_civilian(&r)));
        let mountain = r.terrain("Mountain").unwrap();
        for t in w.grid.ids().collect::<Vec<_>>() {
            if t != tile {
                w.grid.terrain[t.0 as usize] = mountain;
            }
        }
        let before = w.units.len();
        assert!(!give(&mut w, &r, u, "Free [Worker] found in the ruins"));
        assert_eq!(w.units.len(), before);
    }

    #[test]
    fn population_reward_updates_religious_pressure() {
        let r = ruleset();
        let (mut w, u, _) = world(&r);
        let city = w.civ(CivId(0)).unwrap().cities[0];
        let before = w.cities.get(city).unwrap().faith.total_pressure();
        assert!(give(&mut w, &r, u, "[+1] population in a random city"));
        let city = w.cities.get(city).unwrap();
        assert_eq!(city.population, 2);
        assert_eq!(city.faith.total_pressure(), before + crate::world::religion::PRESSURE_PER_CITIZEN);
    }

    #[test]
    fn reveals_explore_without_giving_live_sight_and_useless_maps_fall_through() {
        let r = ruleset();
        let (mut w, u, tile) = world(&r);
        let camp_tile = w.grid.within(tile, 3).into_iter().find(|t| *t != tile).unwrap();
        w.grid.set_improvement(camp_tile, crate::world::barbarians::camp_improvement(&r));
        assert!(give(&mut w, &r, u, "Reveal up to [All] [Barbarian encampment] within a [10] tile radius"));
        assert!(w.visibility.has_explored(CivId(0), camp_tile));
        assert!(!w.visibility.can_see(CivId(0), camp_tile));
        assert_eq!(
            w.visibility.recalls(CivId(0), camp_tile).unwrap().improvement,
            crate::world::barbarians::camp_improvement(&r)
        );
        for t in w.grid.ids() {
            w.visibility.explore(CivId(0), t);
        }
        assert!(!give(&mut w, &r, u,
            "From a randomly chosen tile [4] tiles away from the ruins, reveal tiles up to [4] tiles away with [80]% chance"));
    }

    #[test]
    fn modified_rewards_round_using_their_stat_speed() {
        let u = crate::Unique::parse("Gain [5] [Culture] <(modified by game speed)>");
        let s = Setting { culture_cost: Fixed::permille(1500), speed: Fixed::whole(3), ..setting() };
        assert_eq!(by_speed(&u, 5, "Culture", s), 8);
    }

    #[test]
    fn multiple_free_techs_only_draw_from_initially_researchable_options() {
        let r = ruleset();
        let (mut w, u, _) = world(&r);
        let available = crate::world::research::available_techs(&r, w.civ(CivId(0)).unwrap());
        assert!(give(&mut w, &r, u, "[100] free random researchable Tech(s) from the [Ancient era]"));
        let learned = &w.civ(CivId(0)).unwrap().techs;
        assert!(
            learned.iter().all(|id| available.contains(id)),
            "newly unlocked successors wait for the next discovery"
        );
    }
}
