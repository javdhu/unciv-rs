//! Demanding tribute from a city-state: gold, or a worker, taken by threat.
//!
//! A city-state pays if it is afraid enough (`CityStateFunctions.getTributeWillingness`):
//! a sum that starts at −110 and gains or loses for everything that makes the
//! demand safer or riskier — being the strongest civ in the world, having an
//! army at its gates, its having an ally or protector, having paid recently.
//! Anything at or above zero pays.
//!
//! Paying costs the demander influence — fifteen for gold, fifty for a
//! worker — marks the bully for twenty turns, angers the city-state's
//! protectors, and tells every city-state's quest book it happened
//! (`cityStateBullied`).
//!
//! Strength here is Unciv's *force evaluation* (`BaseUnit.evaluateForce`,
//! `MapUnit.getForceEvaluation`), a rough worth of a unit — strength to the
//! 1.5, speed to the 0.3 — rather than the combat strength a battle uses.

use crate::arena::Id;
use crate::ruleset::{BaseUnit, Ruleset};
use crate::stats::{Fixed, Precise};
use crate::unique_type::UniqueType;
use crate::world::diplomacy::{Flag, Sentiment};
use crate::world::entities::Unit;
use crate::world::grid::CivId;
use crate::world::world::World;

/// Turns a city-state remembers having paid (`CivFlags.RecentlyBullied`).
pub const RECENTLY_BULLIED_TURNS: u32 = 20;
/// Turns a city-state remembers its bully, for quests (`DiplomacyFlags.Bullied`).
pub const BULLIED_TURNS: u32 = 20;
/// Turns a protector remembers the bullying of its ward.
const PROTECTOR_MEMORY: u32 = 75;

fn pow(x: i64, e: i64) -> Precise {
    Precise::from_ratio(x, 1).pow(Fixed::permille(e)).unwrap_or(Precise::whole(0))
}

/// What one of these units is worth in a fight, before its health and
/// promotions (`BaseUnit.evaluateForce`).
pub fn base_force(r: &Ruleset, id: Id<BaseUnit>) -> i64 {
    let Some(b) = r.units.get(id) else { return 0 };
    let (strength, ranged) = (b.strength.unwrap_or(0) as i64, b.ranged_strength.unwrap_or(0) as i64);
    if strength == 0 && ranged == 0 {
        return 0;
    }
    let mut power = pow(strength, 1_500);
    let water = r.unit_types.by_name(&b.unit_type).is_some_and(|t| t.movement_type == "Water");
    if ranged > 0 {
        let mut ranged_power = pow(ranged, 1_450);
        if water {
            ranged_power = ranged_power * Precise::from_ratio(1, 2);
        }
        power = ranged_power;
    }
    power = power * pow(b.movement.max(0) as i64, 300);
    let has = |t: UniqueType| b.uniques.iter().any(|u| u.unique_type() == Some(t));
    if has(UniqueType::SelfDestructs) {
        power = power * Precise::from_ratio(1, 2);
    }
    if has(UniqueType::NuclearWeapon) {
        power = power + Precise::whole(4000);
    }

    // Its own uniques and those of the promotions it starts with.
    let starting = b.promotions.iter().filter_map(|p| r.promotions.id(p)).filter_map(|p| r.promotions.get(p));
    let uniques = b.uniques.iter().chain(starting.flat_map(|p| p.uniques.iter()));
    // Named "highest" in Unciv, but it is the last that counts.
    let mut conditional_bonus = Precise::whole(1);
    let percent = |n: i64, over: i64| Precise::from_ratio(100 * over + n, 100 * over);
    for u in uniques {
        let first = || u.params.first().and_then(|p| p.parse::<i64>().ok()).unwrap_or(0);
        match u.unique_type() {
            Some(UniqueType::Strength) => {
                let n = first();
                if n <= 0 {
                    continue;
                }
                let is = |t: UniqueType| u.conditionals.iter().any(|c| c.unique_type() == Some(t));
                conditional_bonus = if is(UniqueType::ConditionalVsUnits) {
                    percent(n, 4)
                } else if is(UniqueType::ConditionalVsCity)
                    || is(UniqueType::ConditionalAttacking)
                    || is(UniqueType::ConditionalDefending)
                    || is(UniqueType::ConditionalFightingInTiles)
                {
                    percent(n, 2)
                } else {
                    percent(n, 1)
                };
            }
            Some(UniqueType::StrengthNearCapital) if first() > 0 => power = power * percent(first(), 4),
            Some(UniqueType::MayParadrop) => power = power * Precise::from_ratio(5, 4),
            Some(UniqueType::MustSetUp) => power = power * Precise::from_ratio(5, 6),
            Some(UniqueType::AdditionalAttacks) => {
                let limit = (b.movement as i64 - 1).max(0);
                power = power * percent(20 * first().min(limit), 1);
            }
            _ => {}
        }
    }
    (power * conditional_bonus).floor()
}

/// What this unit, as it stands, is worth (`MapUnit.getForceEvaluation`).
pub fn unit_force(r: &Ruleset, unit: &Unit) -> i64 {
    let promoted = pow(unit.promotions.len() as i64 + 1, 300);
    let power = (Precise::from_ratio(base_force(r, unit.base), 1) * promoted).floor();
    power * unit.health as i64 / 100
}

/// A civ's military might, for ranking (`Civilization.calculateMilitaryMight`):
/// its units' force, ships at half, raised by up to double for a full treasury.
pub fn might(world: &World, r: &Ruleset, civ: CivId) -> i64 {
    let Some(c) = world.civ(civ) else { return 0 };
    let mut sum = 1;
    for u in c.units.iter().filter_map(|u| world.units.get(*u)) {
        let f = unit_force(r, u);
        sum += if u.is_water_unit(r) { f / 2 } else { f };
    }
    // `sqrt(gold)` as a percentage, capped at doubling.
    let gold_bonus = Precise::whole(1) + pow(c.gold.max(0) as i64, 500) * Precise::from_ratio(1, 100);
    (Precise::from_ratio(sum, 1) * gold_bonus.min(Precise::whole(2))).floor()
}

/// The city-state's capital: its first standing city.
fn capital_of(world: &World, cs: CivId) -> Option<Id<crate::world::city::City>> {
    world.civ(cs)?.cities.first().copied()
}

/// Every consideration, by name, as Unciv lists them (`getTributeModifiers`).
pub fn modifiers(world: &World, r: &Ruleset, cs: CivId, demander: CivId, worker: bool) -> Vec<(&'static str, i64)> {
    let mut out = Vec::new();
    let Some(c) = world.civ(cs) else { return out };
    if !c.is_city_state() {
        out.push(("Major Civ", -999));
        return out;
    }
    let Some(capital) = capital_of(world, cs).and_then(|id| world.cities.get(id)) else {
        out.push(("No Cities", -999));
        return out;
    };
    out.push(("Base value", -110));
    if crate::world::city_states::ally_of(world, cs).is_some_and(|a| a != demander) {
        out.push(("Has Ally", -10));
    }
    if world.civ_ids().any(|m| m != demander && world.diplomacy.is_protector(m, cs)) {
        out.push(("Has Protector", -20));
    }
    if worker {
        out.push(("Demanding a Worker", -30));
        if capital.population < 4 {
            out.push(("Demanding a Worker from small City-State", -300));
        }
    }
    match c.recently_bullied {
        n if n > 10 => out.push(("Very recently paid tribute", -300)),
        n if n > 0 => out.push(("Recently paid tribute", -40)),
        _ => {}
    }
    if world.diplomacy.influence(cs, demander) < Fixed::whole(-30) {
        out.push(("Influence below -30", -300));
    }

    // Highest might is rank 0; the weakest gets some pity points.
    let mut majors: Vec<(i64, CivId)> = world
        .civ_ids()
        .filter(|m| world.civ(*m).is_some_and(|x| x.is_major() && x.is_alive()))
        .map(|m| (might(world, r, m), m))
        .collect();
    majors.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
    let alive = majors.len().max(1) as i64;
    let rank = majors.iter().position(|(_, m)| *m == demander).map_or(alive, |i| i as i64);
    out.push(("Military Rank", 100 * (alive - rank) / alive));

    // Force at its gates, against its own.
    let range = (world.grid.width() as u32 / 10).clamp(5, 10);
    let near: Vec<_> = world.grid.within(capital.centre, range).into_iter().filter(|t| *t != capital.centre).collect();
    let force_of = |owner: CivId| -> i64 {
        world
            .units
            .iter()
            .filter(|(_, u)| u.owner == owner && near.contains(&u.position) && u.is_military(r))
            .map(|(_, u)| unit_force(r, u))
            .sum()
    };
    let theirs = force_of(demander);
    let city_id = capital_of(world, cs);
    let defence = city_id.map_or(Fixed::ZERO, |id| crate::world::combat::city_defense_strength(world, r, id));
    let own = Precise::from_fixed(defence).pow(Fixed::permille(1_500)).map_or(0, |p| p.floor()) + force_of(cs);
    let local = if own <= 0 {
        if theirs > 0 {
            100
        } else {
            0
        }
    } else {
        // `forceRatio > 3 / 2 / 1.5 / 1 / 0.5`, compared without dividing.
        match theirs * 2 {
            x if x > own * 6 => 100,
            x if x > own * 4 => 80,
            x if x > own * 3 => 60,
            x if x > own * 2 => 40,
            x if x > own => 20,
            _ => 0,
        }
    };
    out.push(("Military near City-State", local));
    out
}

/// Whether the city-state would pay: the sum of its [`modifiers`].
pub fn willingness(world: &World, r: &Ruleset, cs: CivId, demander: CivId, worker: bool) -> i64 {
    modifiers(world, r, cs, demander, worker).iter().map(|(_, v)| v).sum()
}

/// Gold a tribute yields (`goldGainedByTribute`): fifty at standard speed,
/// rounded down to a five, and five more every `interval` turns.
pub fn gold_amount(world: &World, gold_gift_modifier: Fixed, interval: Fixed) -> i32 {
    let base = (Fixed::whole(10) * gold_gift_modifier).floor() as i32 * 5;
    let steps = Fixed::whole(world.turn as i32).checked_div(interval).map_or(0, |s| s.floor() as i32);
    base + 5 * steps
}

/// Why tribute was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TributeRejection {
    NotACityState,
    AtWar,
    /// Not afraid enough: the willingness it would need to be at least zero.
    Unwilling(i64),
    /// No worker this city-state could build.
    NoWorker,
}

impl std::fmt::Display for TributeRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TributeRejection::NotACityState => write!(f, "only a city-state pays tribute"),
            TributeRejection::AtWar => write!(f, "not while at war"),
            TributeRejection::Unwilling(w) => write!(f, "not afraid enough ({w})"),
            TributeRejection::NoWorker => write!(f, "it has no worker to give"),
        }
    }
}

impl std::error::Error for TributeRejection {}

fn check(world: &World, r: &Ruleset, cs: CivId, demander: CivId, worker: bool) -> Result<(), TributeRejection> {
    if !world.civ(cs).is_some_and(|c| c.is_city_state()) {
        return Err(TributeRejection::NotACityState);
    }
    if world.diplomacy.at_war(cs, demander) {
        return Err(TributeRejection::AtWar);
    }
    let w = willingness(world, r, cs, demander, worker);
    if w < 0 {
        return Err(TributeRejection::Unwilling(w));
    }
    Ok(())
}

/// Whether the city-state would pay tribute now, gold or a worker.
pub fn may_demand(world: &World, r: &Ruleset, cs: CivId, demander: CivId, worker: bool) -> bool {
    check(world, r, cs, demander, worker).is_ok()
}

/// Take gold (`tributeGold`). Returns the gold taken.
pub fn demand_gold(
    world: &mut World,
    r: &Ruleset,
    cs: CivId,
    demander: CivId,
    gold_gift_modifier: Fixed,
    interval: Fixed,
) -> Result<i32, TributeRejection> {
    check(world, r, cs, demander, false)?;
    let gold = gold_amount(world, gold_gift_modifier, interval);
    if let Some(c) = world.civ_mut(demander) {
        c.gold += gold;
    }
    world.diplomacy.add_influence(cs, demander, Fixed::whole(-15));
    bullied(world, cs, demander);
    Ok(gold)
}

/// Take a worker (`tributeWorker`), placed beside the city-state's capital.
pub fn demand_worker(world: &mut World, r: &Ruleset, cs: CivId, demander: CivId) -> Result<Id<Unit>, TributeRejection> {
    check(world, r, cs, demander, true)?;
    let civ = world.civ(cs).ok_or(TributeRejection::NotACityState)?;
    let mut workers: Vec<Id<BaseUnit>> = r
        .units
        .iter()
        .filter(|(_, u)| u.uniques.iter().any(|x| x.unique_type() == Some(UniqueType::BuildImprovements)))
        .filter(|(_, u)| u.strength.unwrap_or(0) == 0)
        .filter(|(_, u)| {
            u.required_tech.as_deref().is_none_or(|t| r.technologies.id(t).is_some_and(|t| civ.has_tech(t)))
        })
        .map(|(id, _)| id)
        .collect();
    workers.sort_by_key(|i| i.index());
    let mut rng = crate::rng::Rng::from_parts(&[0x7_21B, cs.0 as u64, demander.0 as u64, world.turn as u64]);
    let base = *workers.get(rng.range(0, workers.len() as i64 - 1) as usize).ok_or(TributeRejection::NoWorker)?;
    let centre = capital_of(world, cs)
        .and_then(|id| world.cities.get(id))
        .map(|c| c.centre)
        .ok_or(TributeRejection::NotACityState)?;
    // The nearest open ground beside the capital, by id.
    let spot = world
        .grid
        .neighbours(centre)
        .filter(|t| crate::world::movement::is_passable(&world.grid, r, *t) && !world.grid.is_water(r, *t))
        .find(|t| world.units_at(*t).is_empty())
        .unwrap_or(centre);
    let movement = r.units.get(base).map_or(2, |u| u.movement);
    let unit = world.place_unit(Unit::new(base, demander, spot, movement));
    world.diplomacy.add_influence(cs, demander, Fixed::whole(-50));
    bullied(world, cs, demander);
    Ok(unit)
}

/// The consequences of being bullied (`cityStateBullied`), beyond the tribute.
fn bullied(world: &mut World, cs: CivId, bully: CivId) {
    // Its protectors take it personally — less so a second time.
    let protectors: Vec<CivId> =
        world.civ_ids().filter(|m| *m != bully && world.diplomacy.is_protector(*m, cs)).collect();
    for p in protectors {
        if !world.diplomacy.have_met(p, bully) {
            continue;
        }
        let repeat = world.diplomacy.attitude(p, bully).has(Sentiment::BulliedProtectedMinor)
            && world.diplomacy.flag_turns(p, bully, Flag::RememberBulliedProtectedMinor) > 50;
        let hit = if repeat { -10 } else { -15 };
        world.diplomacy.attitude_mut(p, bully).add(
            Sentiment::BulliedProtectedMinor,
            Fixed::whole(hit),
            Some(Fixed::whole(-100)),
        );
        world.diplomacy.set_flag(p, bully, Flag::RememberBulliedProtectedMinor, PROTECTOR_MEMORY);
    }
    world.diplomacy.set_flag(cs, bully, Flag::Bullied, BULLIED_TURNS);
    if let Some(c) = world.civ_mut(cs) {
        c.recently_bullied = RECENTLY_BULLIED_TURNS;
    }
}

/// Who bullied this city-state most recently, while it still remembers
/// (`getMostRecentBully`).
pub fn most_recent_bully(world: &World, cs: CivId) -> Option<CivId> {
    world
        .civ_ids()
        .filter(|m| world.diplomacy.has_flag(cs, *m, Flag::Bullied))
        .max_by_key(|m| (world.diplomacy.flag_turns(cs, *m, Flag::Bullied), std::cmp::Reverse(m.0)))
}

/// One turn of city-states forgetting that they paid.
pub fn tick(world: &mut World) {
    for c in world.civs.iter_mut() {
        c.recently_bullied = c.recently_bullied.saturating_sub(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::city::City;
    use crate::world::entities::Civilization;
    use crate::world::grid::TileGrid;

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("rules/gods-and-kings").expect("readable").expect("loads")
    }

    /// Two majors and a city-state at (10, 10); civ 0 has met the city-state.
    fn world(r: &Ruleset) -> World {
        let g = TileGrid::new(24, 24, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        let mut civs = vec![Civilization::new(CivId(0), nation, true), Civilization::new(CivId(1), nation, true)];
        let mut cs = Civilization::new(CivId(2), nation, true);
        cs.city_state_type = Some(r.city_state_types.ids().next().unwrap());
        civs.push(cs);
        let mut w = World::new(g, civs);
        w.found_city_in(r, City::found("Home", CivId(0), w.grid.id_at(2, 2).unwrap()));
        w.found_city_in(r, City::found("Rival", CivId(1), w.grid.id_at(20, 20).unwrap()));
        w.found_city_in(r, City::found("Minor", CivId(2), w.grid.id_at(10, 10).unwrap()));
        w.diplomacy.meet(CivId(0), CivId(2));
        w.diplomacy.meet(CivId(1), CivId(2));
        w
    }

    fn army(w: &mut World, r: &Ruleset, civ: CivId, name: &str, n: usize, near: (u16, u16)) {
        let base = r.units.id(name).unwrap();
        let centre = w.grid.id_at(near.0, near.1).unwrap();
        for t in w.grid.ring(centre, 2).into_iter().take(n) {
            w.place_unit(Unit::new(base, civ, t, 2));
        }
    }

    #[test]
    fn a_units_force_is_strength_to_the_one_and_a_half_times_speed_to_the_point_three() {
        let r = ruleset();
        let warrior = r.units.id("Warrior").unwrap();
        let s = r.units.get(warrior).unwrap().strength.unwrap() as f64;
        let m = r.units.get(warrior).unwrap().movement as f64;
        assert_eq!(base_force(&r, warrior), (s.powf(1.5) * m.powf(0.3)) as i64);
    }

    #[test]
    fn a_lone_civ_far_away_is_refused() {
        let r = ruleset();
        let w = world(&r);
        let will = willingness(&w, &r, CivId(2), CivId(0), false);
        assert!(will < 0, "{will}: {:?}", modifiers(&w, &r, CivId(2), CivId(0), false));
    }

    #[test]
    fn the_strongest_civ_with_an_army_at_the_gates_is_paid() {
        let r = ruleset();
        let mut w = world(&r);
        army(&mut w, &r, CivId(0), "Swordsman", 8, (10, 10));
        let will = willingness(&w, &r, CivId(2), CivId(0), false);
        assert!(will >= 0, "{will}: {:?}", modifiers(&w, &r, CivId(2), CivId(0), false));

        let before = w.civ(CivId(0)).unwrap().gold;
        let gold = demand_gold(&mut w, &r, CivId(2), CivId(0), Fixed::ONE, Fixed::permille(6_500)).expect("pays");
        assert_eq!(gold, 50, "fifty on the first turn at standard speed");
        assert_eq!(w.civ(CivId(0)).unwrap().gold, before + 50);
        assert_eq!(w.diplomacy.influence(CivId(2), CivId(0)), Fixed::whole(-15));
        assert_eq!(most_recent_bully(&w, CivId(2)), Some(CivId(0)));

        // And it will not pay again so soon.
        assert!(matches!(
            demand_gold(&mut w, &r, CivId(2), CivId(0), Fixed::ONE, Fixed::permille(6_500)),
            Err(TributeRejection::Unwilling(_))
        ));
    }

    #[test]
    fn tribute_grows_with_the_game() {
        let r = ruleset();
        let mut w = world(&r);
        w.turn = 130;
        // 50 + 5 · (130 / 6.5) = 150.
        assert_eq!(gold_amount(&w, Fixed::ONE, Fixed::permille(6_500)), 150);
    }

    #[test]
    fn a_protector_resents_the_bully() {
        let r = ruleset();
        let mut w = world(&r);
        w.diplomacy.meet(CivId(0), CivId(1));
        w.diplomacy.set_protector(CivId(1), CivId(2), true);
        army(&mut w, &r, CivId(0), "Swordsman", 12, (10, 10));
        demand_gold(&mut w, &r, CivId(2), CivId(0), Fixed::ONE, Fixed::permille(6_500)).expect("pays even so");
        assert_eq!(w.diplomacy.attitude(CivId(1), CivId(0)).get(Sentiment::BulliedProtectedMinor), Fixed::whole(-15));
    }

    #[test]
    fn a_worker_costs_fifty_influence_and_appears_by_the_capital() {
        let r = ruleset();
        let mut w = world(&r);
        army(&mut w, &r, CivId(0), "Swordsman", 12, (10, 10));
        w.cities.iter_mut().for_each(|(_, c)| c.population = 6);
        // It can only hand over a worker it could build itself.
        assert!(matches!(demand_worker(&mut w, &r, CivId(2), CivId(0)), Err(TributeRejection::NoWorker)));
        let agriculture = r.technologies.id("Agriculture").unwrap();
        w.civ_mut(CivId(2)).unwrap().techs.insert(agriculture);
        let before = w.civ(CivId(0)).unwrap().units.len();
        let worker = demand_worker(&mut w, &r, CivId(2), CivId(0)).expect("a big army and a big city");
        assert_eq!(w.civ(CivId(0)).unwrap().units.len(), before + 1);
        assert_eq!(w.units.get(worker).unwrap().owner, CivId(0));
        assert_eq!(w.diplomacy.influence(CivId(2), CivId(0)), Fixed::whole(-50));
    }
}
