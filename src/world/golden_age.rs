//! Golden ages (`GoldenAgeManager`).
//!
//! A happy empire banks its surplus happiness; when the bank reaches
//! `(500 + 250 × golden ages so far) × (1 + cities / 100) × speed`, it is
//! spent on ten turns of plenty — a fifth more production and culture in
//! every city, and a gold more on every tile that yields any. Unhappiness
//! draws the bank down, never below nothing, and nothing is banked while a
//! golden age runs.
//!
//! A Great Artist starts one outright, of eight turns. Either way the length
//! is scaled by the civ's `GoldenAgeLength` uniques and the speed, and a
//! second golden age begun inside the first adds its turns to what is left.

use crate::ruleset::Ruleset;
use crate::stats::Fixed;
use crate::world::grid::CivId;
use crate::world::world::World;
use crate::UniqueType;

/// Where a civ stands toward its next golden age.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GoldenAge {
    pub stored_happiness: i32,
    /// Golden ages begun from happiness. Each makes the next dearer.
    pub count: u32,
    pub turns_left: u32,
}

impl GoldenAge {
    pub fn active(&self) -> bool {
        self.turns_left > 0
    }
}

/// How long one begun by happiness lasts, before modifiers.
pub const TURNS: i32 = 10;
/// The share more that each city produces and makes of culture.
pub const PERCENT_BONUS: i32 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoldenAgeEvent {
    Started { civ: CivId, turns: u32 },
    Ended { civ: CivId },
}

pub fn is_golden(world: &World, civ: CivId) -> bool {
    world.civ(civ).is_some_and(|c| c.golden_age.active())
}

/// Happiness the next golden age needs (`happinessRequiredForNextGoldenAge`).
pub fn threshold(world: &World, civ: CivId, speed: Fixed) -> i32 {
    let Some(c) = world.civ(civ) else { return i32::MAX };
    let base = 500 + 250 * c.golden_age.count as i128;
    let cities = c.cities.len() as i128;
    (base * (100 + cities) * speed.raw() as i128 / (100 * Fixed::ONE.raw() as i128)) as i32
}

/// Turns a golden age of `unmodified` turns lasts for this civ
/// (`calculateGoldenAgeLength`): each `GoldenAgeLength` unique scales it by
/// its percentage, and the speed by its own.
pub fn length(world: &World, civ: CivId, unmodified: i32, speed_length: Fixed) -> u32 {
    let Some(c) = world.civ(civ) else { return 0 };
    let mut turns = Fixed::whole(unmodified);
    for u in c.uniques.matching(UniqueType::GoldenAgeLength) {
        if let Some(p) = u.params.first().and_then(|p| p.parse::<i32>().ok()) {
            turns = (turns * Fixed::whole(100 + p)).checked_div(Fixed::whole(100)).unwrap_or(turns);
        }
    }
    (turns * speed_length).floor().max(0) as u32
}

/// Begin a golden age of `unmodified` turns, or lengthen the one running
/// (`enterGoldenAge`).
pub fn enter(world: &mut World, civ: CivId, unmodified: i32, speed_length: Fixed) -> Option<GoldenAgeEvent> {
    let turns = length(world, civ, unmodified, speed_length);
    let c = world.civ_mut(civ)?;
    c.golden_age.turns_left += turns;
    Some(GoldenAgeEvent::Started { civ, turns })
}

/// A civ's turn ends with this much happiness (`GoldenAgeManager.endTurn`).
pub fn end_turn(
    world: &mut World,
    civ: CivId,
    happiness: i32,
    speed: Fixed,
    speed_length: Fixed,
) -> Option<GoldenAgeEvent> {
    let c = world.civ_mut(civ).filter(|c| c.is_major())?;
    let ga = &mut c.golden_age;
    if ga.active() {
        ga.turns_left -= 1;
        return (!ga.active()).then_some(GoldenAgeEvent::Ended { civ });
    }
    ga.stored_happiness = (ga.stored_happiness + happiness).max(0);
    let needed = threshold(world, civ, speed);
    let stored = world.civ(civ)?.golden_age.stored_happiness;
    if stored < needed {
        return None;
    }
    world.civ_mut(civ)?.golden_age.stored_happiness -= needed;
    let e = enter(world, civ, TURNS, speed_length);
    world.civ_mut(civ)?.golden_age.count += 1;
    e
}

/// Which tiles gain a golden age's extra gold: any that yield gold at all.
pub fn gains_gold(r: &Ruleset, grid: &crate::world::grid::TileGrid, tile: crate::world::grid::TileId) -> bool {
    !crate::world::yields::tile_yield(grid, r, tile).gold.is_zero()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::city::City;
    use crate::world::entities::Civilization;
    use crate::world::grid::TileGrid;

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings").expect("readable").expect("loads")
    }

    fn world(r: &Ruleset, cities: u16) -> World {
        let g = TileGrid::new(20, 12, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        let mut w = World::new(g, vec![Civilization::new(CivId(0), nation, true)]);
        for i in 0..cities {
            let t = w.grid.id_at(i * 4, 3).unwrap();
            w.found_city(City::found(format!("C{i}"), CivId(0), t));
        }
        w
    }

    #[test]
    fn the_bar_is_five_hundred_raised_by_past_ages_and_cities() {
        let r = ruleset();
        let mut w = world(&r, 1);
        assert_eq!(threshold(&w, CivId(0), Fixed::ONE), 505);
        let w4 = world(&r, 4);
        assert_eq!(threshold(&w4, CivId(0), Fixed::ONE), 520);
        w.civ_mut(CivId(0)).unwrap().golden_age.count = 2;
        assert_eq!(threshold(&w, CivId(0), Fixed::ONE), 1010);
        assert_eq!(threshold(&w, CivId(0), Fixed::permille(1500)), 1515, "Epic");
    }

    #[test]
    fn happiness_banks_until_it_buys_ten_turns() {
        let r = ruleset();
        let mut w = world(&r, 1);
        let mut started = None;
        for turn in 1..=60 {
            if let Some(e) = end_turn(&mut w, CivId(0), 10, Fixed::ONE, Fixed::ONE) {
                started = Some((turn, e));
                break;
            }
        }
        // 505 needed at ten a turn: the fifty-first turn.
        assert_eq!(started, Some((51, GoldenAgeEvent::Started { civ: CivId(0), turns: 10 })));
        let ga = w.civ(CivId(0)).unwrap().golden_age;
        assert_eq!((ga.stored_happiness, ga.count, ga.turns_left), (5, 1, 10));

        // Nothing is banked while it runs, and it ends after ten.
        let mut ended = 0;
        for turn in 1..=10 {
            if end_turn(&mut w, CivId(0), 10, Fixed::ONE, Fixed::ONE).is_some() {
                ended = turn;
            }
        }
        assert_eq!(ended, 10);
        assert_eq!(w.civ(CivId(0)).unwrap().golden_age.stored_happiness, 5);
    }

    #[test]
    fn unhappiness_drains_the_bank_but_not_below_nothing() {
        let r = ruleset();
        let mut w = world(&r, 1);
        end_turn(&mut w, CivId(0), 30, Fixed::ONE, Fixed::ONE);
        end_turn(&mut w, CivId(0), -50, Fixed::ONE, Fixed::ONE);
        assert_eq!(w.civ(CivId(0)).unwrap().golden_age.stored_happiness, 0);
    }

    #[test]
    fn a_second_age_begun_inside_the_first_adds_to_it() {
        let r = ruleset();
        let mut w = world(&r, 1);
        enter(&mut w, CivId(0), 10, Fixed::ONE);
        enter(&mut w, CivId(0), 8, Fixed::permille(1500));
        assert_eq!(w.civ(CivId(0)).unwrap().golden_age.turns_left, 22);
    }

    #[test]
    fn a_golden_city_makes_a_fifth_more_and_gold_only_where_there_was_gold() {
        let r = ruleset();
        let mut w = world(&r, 1);
        let id = w.civ(CivId(0)).unwrap().cities[0];
        let city = w.cities.get(id).unwrap().clone();
        let plain = crate::world::city::city_yield_with(&w.grid, &r, &city, false);
        let golden = crate::world::city::city_yield_with(&w.grid, &r, &city, true);
        assert_eq!(golden.production, plain.production * Fixed::permille(1200));
        assert_eq!(golden.culture, plain.culture * Fixed::permille(1200));
        // Grassland yields no gold, so the centre gains none.
        assert_eq!(golden.gold, plain.gold);
        let _ = &mut w;
    }
}
