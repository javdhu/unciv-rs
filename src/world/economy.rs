//! What an empire pays each turn simply for existing.
//!
//! Buildings are charged to the city that holds them (`city_maintenance`), and
//! roads to the empire that owns the ground (`connections::transportation_upkeep`).
//! What is left is the army, and the army is the interesting one: its cost is
//! not linear in the number of soldiers, and it rises as the game goes on.
//!
//! # Why a curve rather than a price per unit
//!
//! A flat price per soldier makes a large empire's army free — income scales
//! with cities, the bill scales with units, and past a certain size the
//! constraint disappears. Civ V answers that by raising the exponent as the
//! game progresses: early on the bill is close to linear, and by the end it
//! grows faster than the army does. The first three units are free outright,
//! which is what keeps a one-city start playable.
//!
//! # Fixed point, and where it can disagree with Unciv
//!
//! `cost^(1 + progress/3)` is a fractional power, computed here as
//! `2^(y·log2 x)` in fixed point rather than by the platform's `powf`. That is
//! deliberate — `powf` differs between machines in the last bits, which is
//! exactly what makes a replay diverge. The trade is that this can land a
//! whole gold either side of Unciv's answer when the true value sits very near
//! an integer, because both truncate at the end. The differential tests are
//! the place that shows up, and the determinism fingerprint is what it buys.

use crate::ruleset::{Difficulty, Ruleset};
use crate::stats::{Fixed, Precise};
use crate::world::grid::CivId;
use crate::world::world::World;

/// Units a civ gets for nothing before the meter starts.
const FREE_UNITS: i32 = 3;
/// What one unit costs before the curve is applied.
const BASE_UNIT_COST: Fixed = Fixed::permille(500);

/// Gold this civ owes for its army this turn.
///
/// The free units are the *dearest* ones, not the first ones raised — a civ
/// with three expensive units and five cheap ones pays for the cheap ones.
/// Unciv sorts descending and drops from the front for exactly this reason,
/// and getting it backwards would make a large army cheaper than a small one.
pub fn unit_maintenance(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    game_length: i32,
    difficulty: Option<&Difficulty>,
) -> i32 {
    let Some(c) = world.civ(civ) else { return 0 };

    let mut free = FREE_UNITS;
    for u in c.uniques.matching(crate::UniqueType::FreeUnits) {
        free += u.params.first().and_then(|p| p.parse::<i32>().ok()).unwrap_or(0);
    }
    let garrisons_are_free = c.uniques.has(crate::UniqueType::UnitsInCitiesNoMaintenance);
    let city_centres: std::collections::BTreeSet<_> =
        world.cities.iter().filter(|(_, city)| city.owner == civ).map(|(_, city)| city.centre).collect();

    // One entry per unit we are actually billed for, each the fraction of full
    // price that unit pays.
    let mut costs: Vec<Fixed> = Vec::new();
    for unit_id in &c.units {
        let Some(unit) = world.units.get(*unit_id) else { continue };
        if garrisons_are_free && city_centres.contains(&unit.position) {
            continue;
        }
        let mut share = Fixed::ONE;
        let discounts = r
            .units
            .get(unit.base)
            .into_iter()
            .flat_map(|b| b.uniques.iter())
            .filter(|u| u.unique_type() == Some(crate::UniqueType::UnitMaintenanceDiscount))
            .chain(c.uniques.matching(crate::UniqueType::UnitMaintenanceDiscount));
        for u in discounts {
            // The parameter is a change, not a factor: -25 means three
            // quarters of the price.
            if let Some(pct) = u.params.first().and_then(|p| Fixed::parse(p)) {
                if let Some(next) = (share * (Fixed::whole(100) + pct)).checked_div(Fixed::whole(100)) {
                    share = next;
                }
            }
        }
        costs.push(share);
    }

    costs.sort_by(|a, b| b.cmp(a)); // dearest first, so the free ones come off the top
    let billed: Fixed = costs.into_iter().skip(free.max(0) as usize).sum();
    if billed.is_zero() {
        return 0;
    }

    // How far through the game we are, capped at the end — a game that runs
    // past its nominal length does not keep getting dearer.
    let progress = if game_length > 0 {
        Fixed::whole(world.turn as i32)
            .checked_div(Fixed::whole(game_length))
            .unwrap_or(Fixed::ONE)
            .min(Fixed::ONE)
    } else {
        Fixed::ONE
    };

    // 0.5 * units * (1 + progress), then raised to (1 + progress/3). Kept in
    // Precise so the chain rounds once at the end rather than at each step.
    let linear = ((Precise::from_fixed(BASE_UNIT_COST) * Precise::from_fixed(billed)) * Precise::from_fixed(Fixed::ONE + progress))
        .to_fixed();
    let exponent = Fixed::ONE
        + progress.checked_div(Fixed::whole(3)).unwrap_or(Fixed::ZERO);
    let mut cost = linear.pow(exponent).unwrap_or(linear);

    if c.is_ai {
        if let Some(d) = difficulty {
            if !d.ai_unit_maintenance_modifier.is_zero() {
                cost = (Precise::from_fixed(cost) * Precise::from_fixed(d.ai_unit_maintenance_modifier))
                    .to_fixed();
            }
        }
    }

    // Truncated, as Kotlin's `.toInt()` is. Rounding here would charge a gold
    // that Unciv does not.
    cost.floor().max(0) as i32
}

/// Disband soldiers a bankrupt empire cannot pay for.
///
/// Without this, gold is a number that only ever falls: a civ deep in debt
/// keeps its army, keeps paying for it, and drifts to negative infinity while
/// its science is throttled for the rest of the game. Debt has to *do*
/// something or it is not a constraint. Measured over 300 turns of self-play,
/// three civs ended at −1425, −1176 and −474 gold with nothing stopping them.
///
/// Unciv's threshold is −200 with a negative income, both conditions, so a
/// civ with reserves may run a deficit and a civ in debt that has balanced its
/// books is left alone. Units in our own territory go first, because they are
/// the ones we can replace, and the least experienced of those before the
/// veterans.
///
/// Returns the units disbanded, in the order they went.
pub fn disband_for_debt(
    world: &mut World,
    r: &Ruleset,
    civ: CivId,
    game_length: i32,
    difficulty: Option<&Difficulty>,
    gold_per_turn: i32,
) -> Vec<crate::arena::Id<crate::world::entities::Unit>> {
    let mut gone = Vec::new();
    let Some(c) = world.civ(civ) else { return gone };
    if c.is_barbarian {
        return gone; // barbarians have no treasury to run dry
    }

    // Only unit upkeep changes as we disband, so the rest of the income is
    // held fixed rather than replanning the whole turn after each loss.
    let before = unit_maintenance(world, r, civ, game_length, difficulty);
    while let Some(c) = world.civ(civ) {
        if c.gold > DEBT_LIMIT {
            break;
        }
        let now = unit_maintenance(world, r, civ, game_length, difficulty);
        if gold_per_turn + (before - now) >= 0 {
            break; // the books balance again; keep what is left
        }
        let Some(worst) = cheapest_soldier(world, r, civ) else { break };
        world.remove_unit(worst);
        gone.push(worst);
    }
    gone
}

/// Debt this deep forces a disband, if income is also negative.
const DEBT_LIMIT: i32 = -200;

/// The soldier to let go of first: one standing on our own ground, and the
/// least experienced of those. Ties break on unit id so two identical armies
/// disband identically.
fn cheapest_soldier(
    world: &World,
    r: &Ruleset,
    civ: CivId,
) -> Option<crate::arena::Id<crate::world::entities::Unit>> {
    let c = world.civ(civ)?;
    c.units
        .iter()
        .copied()
        .filter_map(|id| world.units.get(id).map(|u| (id, u)))
        .filter(|(_, u)| {
            r.units.get(u.base).is_some_and(|b| {
                b.strength.unwrap_or(0) > 0 || b.ranged_strength.unwrap_or(0) > 0
            })
        })
        .min_by_key(|(id, u)| {
            let at_home = world.grid.owner[u.position.0 as usize] == Some(civ);
            // At home first, then the weakest, then by id.
            let strength = r
                .units
                .get(u.base)
                .map_or(0, |b| b.strength.unwrap_or(0).max(b.ranged_strength.unwrap_or(0)));
            (!at_home, strength, id.index())
        })
        .map(|(id, _)| id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::city::City;
    use crate::world::entities::{Civilization, Unit};
    use crate::world::grid::{TileGrid, TileId};

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings")
            .expect("readable")
            .expect("loads")
    }

    fn world(r: &Ruleset) -> World {
        let g = TileGrid::new(8, 8, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        World::new(g, vec![Civilization::new(CivId(0), nation, false)])
    }

    fn raise(w: &mut World, r: &Ruleset, n: usize) {
        let warrior = r.units.id("Warrior").expect("the ruleset has a warrior");
        for i in 0..n {
            w.place_unit(Unit::new(warrior, CivId(0), TileId(i as u16), 2));
        }
    }

    #[test]
    fn the_first_three_units_cost_nothing() {
        let r = ruleset();
        let mut w = world(&r);
        for n in 0..=3 {
            let mut w2 = w.clone();
            raise(&mut w2, &r, n);
            assert_eq!(unit_maintenance(&w2, &r, CivId(0), 500, None), 0, "{n} units");
        }
        raise(&mut w, &r, 10);
        // 0.5 * (10 - 3) = 3.5, raised to the power 1 at turn zero, truncated.
        assert_eq!(unit_maintenance(&w, &r, CivId(0), 500, None), 3);
    }

    #[test]
    fn an_army_gets_dearer_as_the_game_wears_on() {
        let r = ruleset();
        let mut w = world(&r);
        raise(&mut w, &r, 10);

        let early = unit_maintenance(&w, &r, CivId(0), 500, None);
        w.turn = 250;
        let middle = unit_maintenance(&w, &r, CivId(0), 500, None);
        w.turn = 500;
        let late = unit_maintenance(&w, &r, CivId(0), 500, None);
        assert!(early < middle && middle < late, "{early} then {middle} then {late}");

        // Past the nominal end it stops climbing — a game that runs long does
        // not bankrupt everyone for running long.
        w.turn = 900;
        assert_eq!(unit_maintenance(&w, &r, CivId(0), 500, None), late);
    }

    #[test]
    fn the_bill_grows_faster_than_the_army_late_on() {
        let r = ruleset();
        let mut w = world(&r);
        w.turn = 500; // fully progressed, so the exponent is at its highest
        raise(&mut w, &r, 13); // ten billed
        let ten = unit_maintenance(&w, &r, CivId(0), 500, None);
        let mut w2 = world(&r);
        w2.turn = 500;
        raise(&mut w2, &r, 23); // twenty billed
        let twenty = unit_maintenance(&w2, &r, CivId(0), 500, None);
        assert!(
            twenty > ten * 2,
            "doubling the army should more than double the bill: {ten} then {twenty}"
        );
    }

    #[test]
    fn a_shorter_game_ages_faster() {
        let r = ruleset();
        let mut w = world(&r);
        raise(&mut w, &r, 12);
        w.turn = 165;
        let quick = unit_maintenance(&w, &r, CivId(0), 330, None);
        let standard = unit_maintenance(&w, &r, CivId(0), 500, None);
        assert!(quick > standard, "halfway through a quick game beats a third of a long one");
    }

    #[test]
    fn free_units_are_the_dearest_ones() {
        // A civ whose free slots went to its cheapest units would pay more for
        // a bigger army than a smaller one. Unciv sorts descending and drops
        // from the front; this checks we do the same by making some units
        // cheaper than others.
        let r = ruleset();
        let discounted = r
            .units
            .iter()
            .find(|(_, u)| {
                u.uniques
                    .iter()
                    .any(|x| x.unique_type() == Some(crate::UniqueType::UnitMaintenanceDiscount))
            })
            .map(|(id, _)| id);
        let Some(discounted) = discounted else {
            return; // this ruleset has no discounted unit; nothing to check
        };

        let mut w = world(&r);
        let warrior = r.units.id("Warrior").unwrap();
        // Four full-price and four discounted. Three free slots must go to the
        // full-price ones, leaving one full-price and four discounted billed.
        for i in 0..4u16 {
            w.place_unit(Unit::new(warrior, CivId(0), TileId(i), 2));
            w.place_unit(Unit::new(discounted, CivId(0), TileId(i + 10), 2));
        }
        let mixed = unit_maintenance(&w, &r, CivId(0), 500, None);

        let mut all_full = world(&r);
        raise(&mut all_full, &r, 8);
        let full = unit_maintenance(&all_full, &r, CivId(0), 500, None);
        assert!(mixed <= full, "discounts should never cost more: {mixed} against {full}");
    }

    #[test]
    fn a_bankrupt_empire_lets_its_soldiers_go() {
        let r = ruleset();
        let mut w = world(&r);
        raise(&mut w, &r, 12);
        w.turn = 200;
        w.civ_mut(CivId(0)).unwrap().gold = -500;

        // A three-gold deficit that the army can cover: it lets go of enough
        // and stops, rather than disbanding the lot.
        let gone = disband_for_debt(&mut w, &r, CivId(0), 500, None, -3);
        assert!(!gone.is_empty(), "something has to give");
        let left = w.civ(CivId(0)).unwrap().units.len();
        assert_eq!(left, 12 - gone.len(), "the civ's roster matches what was taken");
        assert!(left > 0, "it stopped once the books balanced");
    }

    /// The other end of the same rule, and not a bug: if the whole army is
    /// worth less than the shortfall, the whole army goes and the civ is still
    /// in deficit. Unciv does the same — the loop runs out of units, not out
    /// of debt. The money is being lost somewhere the army cannot fix.
    #[test]
    fn a_deficit_the_army_cannot_cover_takes_the_whole_army() {
        let r = ruleset();
        let mut w = world(&r);
        raise(&mut w, &r, 12);
        w.turn = 200;
        w.civ_mut(CivId(0)).unwrap().gold = -500;

        let gone = disband_for_debt(&mut w, &r, CivId(0), 500, None, -200);
        assert_eq!(gone.len(), 12);
        assert_eq!(w.civ(CivId(0)).unwrap().units.len(), 0);
    }

    #[test]
    fn a_civ_with_reserves_keeps_its_army() {
        let r = ruleset();
        let mut w = world(&r);
        raise(&mut w, &r, 12);
        w.turn = 200;

        // Deep deficit but money in the bank: Civ's rule needs both.
        w.civ_mut(CivId(0)).unwrap().gold = 1000;
        assert!(disband_for_debt(&mut w, &r, CivId(0), 500, None, -50).is_empty());

        // In debt but earning: also left alone.
        w.civ_mut(CivId(0)).unwrap().gold = -500;
        assert!(disband_for_debt(&mut w, &r, CivId(0), 500, None, 5).is_empty());
        assert_eq!(w.civ(CivId(0)).unwrap().units.len(), 12);
    }

    #[test]
    fn barbarians_have_no_treasury_to_run_dry() {
        let r = ruleset();
        let mut w = world(&r);
        raise(&mut w, &r, 12);
        {
            let c = w.civ_mut(CivId(0)).unwrap();
            c.is_barbarian = true;
            c.gold = -5000;
        }
        assert!(disband_for_debt(&mut w, &r, CivId(0), 500, None, -99).is_empty());
    }

    #[test]
    fn disbanding_is_the_same_twice() {
        let r = ruleset();
        let mut a = world(&r);
        raise(&mut a, &r, 12);
        a.turn = 200;
        a.civ_mut(CivId(0)).unwrap().gold = -500;
        let mut b = a.clone();

        let first = disband_for_debt(&mut a, &r, CivId(0), 500, None, -20);
        let second = disband_for_debt(&mut b, &r, CivId(0), 500, None, -20);
        assert_eq!(first, second, "which soldiers go must not depend on iteration order");
        assert_eq!(a, b);
    }

    #[test]
    fn an_empire_without_units_pays_nothing() {
        let r = ruleset();
        let mut w = world(&r);
        w.found_city(City::found("Rome", CivId(0), TileId(20)));
        w.turn = 400;
        assert_eq!(unit_maintenance(&w, &r, CivId(0), 500, None), 0);
    }

    #[test]
    fn the_difficulty_bill_only_lands_on_the_ai() {
        let r = ruleset();
        let d = r.difficulties.by_name("Prince").or_else(|| r.difficulties.values().next());
        let Some(d) = d else { return };
        let mut human = world(&r);
        raise(&mut human, &r, 12);
        human.turn = 300;

        let mut ai = human.clone();
        ai.civ_mut(CivId(0)).unwrap().is_ai = true;

        let charged_human = unit_maintenance(&human, &r, CivId(0), 500, Some(d));
        let charged_ai = unit_maintenance(&ai, &r, CivId(0), 500, Some(d));
        let plain = unit_maintenance(&human, &r, CivId(0), 500, None);
        assert_eq!(charged_human, plain, "a human pays the list price whatever the difficulty");
        if d.ai_unit_maintenance_modifier < Fixed::ONE {
            assert!(charged_ai <= charged_human, "the ai gets the handicap, not the human");
        }
    }
}
