//! Specialists: citizens who work in a building instead of on a tile.
//!
//! Buildings open slots — a Market one Merchant, a University two Scientists —
//! and a citizen in a slot yields the specialist's stats (`Specialists.json`)
//! plus great-person points toward its kind, and eats like anyone else. The
//! automatic manager fills a free citizen into whichever is worth more, the
//! best open tile or the best open slot, by the same ranking tiles are judged
//! by with the great-person points added on top (`Automation.rankSpecialist`).
//!
//! Stats a specialist gets from uniques — Secularism's `[+2 Science] from
//! every specialist` — are the civ's, and a city's yield is computed from the
//! city alone, so they are added to the city's output by the turn loop after
//! its percentage bonuses rather than before. Unciv adds them before. At two
//! science a specialist the difference is under a point a turn.

use crate::arena::Id;
use crate::ruleset::{Ruleset, Specialist};
use crate::stats::{Fixed, Stats};
use crate::unique_type::UniqueType;
use crate::world::assignment::{rank_for_city_work, CityFocus};
use crate::world::city::City;
use crate::world::conditionals::{applies, GameContext, Verdict};
use crate::world::world::World;
use std::collections::BTreeMap;

/// Citizens in each kind of slot.
pub type Allocation = BTreeMap<Id<Specialist>, u32>;

/// Slots this city's buildings open, by kind (`getMaxSpecialists`).
pub fn slots(r: &Ruleset, city: &City) -> Allocation {
    let mut out = Allocation::new();
    for building in city.built.iter().filter_map(|id| r.buildings.get(*id)) {
        for (name, n) in &building.specialist_slots {
            if let Some(id) = r.specialists.id(name) {
                *out.entry(id).or_insert(0) += (*n).max(0) as u32;
            }
        }
    }
    out
}

/// Citizens in slots, all kinds together.
pub fn count(city: &City) -> u32 {
    city.specialists.values().sum()
}

/// What one specialist of this kind yields before any unique.
pub fn base_stats(r: &Ruleset, id: Id<Specialist>) -> Stats {
    let Some(s) = r.specialists.get(id) else { return Stats::ZERO };
    let v = |x: Option<i32>| Fixed::whole(x.unwrap_or(0));
    Stats { science: v(s.science), gold: v(s.gold), culture: v(s.culture), production: v(s.production), ..Stats::ZERO }
}

/// Stats every specialist in this city gets on top of its own, from
/// `[stats] from every specialist [cityFilter]` uniques.
pub fn bonus_per_specialist(world: &World, r: &Ruleset, city_id: Id<crate::world::city::City>) -> Stats {
    let Some(city) = world.cities.get(city_id) else { return Stats::ZERO };
    let ctx = GameContext::new(r).with_world(world).for_civ(city.owner).in_city(city_id);
    let civ_uniques =
        world.civ(city.owner).into_iter().flat_map(|c| c.uniques.matching(UniqueType::StatsFromSpecialist));
    let mut total = Stats::ZERO;
    for u in city.uniques.matching(UniqueType::StatsFromSpecialist).chain(civ_uniques) {
        if applies(u, &ctx) != Verdict::True {
            continue;
        }
        let (Some(stats), Some(filter)) = (u.params.first().and_then(|p| Stats::parse(p)), u.params.get(1)) else {
            continue;
        };
        if crate::world::filters::city_matches(world, r, city, Some(city), filter) == Verdict::True {
            total += stats;
        }
    }
    total
}

/// Great-person points one specialist of this kind produces, summed over kinds.
fn great_person_points(r: &Ruleset, id: Id<Specialist>) -> i32 {
    r.specialists.get(id).map_or(0, |s| s.great_person_points.values().sum())
}

/// How much a citizen in this slot is worth, on the scale tiles are ranked on.
pub fn rank(r: &Ruleset, city: &City, id: Id<Specialist>, bonus: Stats, focus: CityFocus, surplus_food: i32) -> Fixed {
    rank_for_city_work(base_stats(r, id) + bonus, city, focus, surplus_food).score
        + Fixed::whole(great_person_points(r, id))
}

/// The city's specialists' own yields, for its yield breakdown.
pub fn yields(r: &Ruleset, city: &City) -> Stats {
    let mut total = Stats::ZERO;
    for (id, n) in &city.specialists {
        for _ in 0..*n {
            total += base_stats(r, *id);
        }
    }
    total
}

/// Great-person points this city's specialists produce, by kind.
pub fn points(r: &Ruleset, city: &City) -> BTreeMap<String, i32> {
    let mut out = BTreeMap::new();
    for (id, n) in &city.specialists {
        let Some(s) = r.specialists.get(*id) else { continue };
        for (kind, amount) in &s.great_person_points {
            *out.entry(kind.clone()).or_insert(0) += amount * *n as i32;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::assignment::{assign_idle_citizens, release_extra_citizens};
    use crate::world::entities::Civilization;
    use crate::world::grid::{CivId, TileGrid};

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings").expect("readable").expect("loads")
    }

    /// A city on open grassland holding its founding ring, with a Market.
    fn world(r: &Ruleset) -> (World, Id<City>) {
        let g = TileGrid::new(16, 16, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        let mut w = World::new(g, vec![Civilization::new(CivId(0), nation, false)]);
        let centre = w.grid.id_at(8, 8).unwrap();
        let city = w.found_city(City::found("Home", CivId(0), centre));
        for t in w.grid.within(centre, 1) {
            w.claim_tile(city, t);
        }
        let market = r.buildings.id("Market").unwrap();
        w.cities.get_mut(city).unwrap().built.insert(market);
        (w, city)
    }

    #[test]
    fn a_market_opens_a_merchant_slot() {
        let r = ruleset();
        let (w, city) = world(&r);
        let merchant = r.specialists.id("Merchant").unwrap();
        assert_eq!(slots(&r, w.cities.get(city).unwrap()).get(&merchant), Some(&1));
        assert_eq!(base_stats(&r, merchant).gold, Fixed::whole(3));
    }

    #[test]
    fn when_the_tiles_run_out_the_next_citizen_takes_the_slot() {
        let r = ruleset();
        let (mut w, city) = world(&r);
        let merchant = r.specialists.id("Merchant").unwrap();
        let c = w.cities.get_mut(city).unwrap();
        // Six ring tiles and one slot: the eighth citizen has nowhere to go.
        c.population = 8;
        assign_idle_citizens(&w.grid, &r, c, CityFocus::None, Stats::ZERO);
        assert_eq!(c.worked.len(), 6);
        assert_eq!(c.specialists.get(&merchant), Some(&1), "the seventh took the market");
        assert_eq!(c.idle_population(), 1, "and the eighth is unemployed");
    }

    #[test]
    fn a_merchant_beats_a_poor_tile() {
        let r = ruleset();
        let (mut w, city) = world(&r);
        let merchant = r.specialists.id("Merchant").unwrap();
        // Make every ring tile worthless, as a mountain range would be.
        let mountain = r.terrain("Mountain").unwrap();
        let centre = w.cities.get(city).unwrap().centre;
        for t in w.grid.neighbours(centre).collect::<Vec<_>>() {
            w.grid.terrain[t.0 as usize] = mountain;
        }
        let c = w.cities.get_mut(city).unwrap();
        assign_idle_citizens(&w.grid, &r, c, CityFocus::None, Stats::ZERO);
        assert_eq!(c.specialists.get(&merchant), Some(&1));
        assert!(c.worked.is_empty());
    }

    #[test]
    fn specialists_yield_and_make_great_people() {
        let r = ruleset();
        let (mut w, city) = world(&r);
        let merchant = r.specialists.id("Merchant").unwrap();
        w.cities.get_mut(city).unwrap().specialists.insert(merchant, 1);
        let c = w.cities.get(city).unwrap();
        assert_eq!(yields(&r, c).gold, Fixed::whole(3));
        assert_eq!(points(&r, c).get("Great Merchant"), Some(&3));

        // And they show up in what the civ banks toward a great person.
        let gained = crate::world::great_people::points_per_turn(&w, &r, CivId(0));
        assert!(gained.get("Great Merchant").is_some_and(|p| *p >= 3), "{gained:?}");
    }

    #[test]
    fn secularism_adds_science_to_every_specialist() {
        let r = ruleset();
        let (mut w, city) = world(&r);
        w.civ_mut(CivId(0))
            .unwrap()
            .uniques
            .insert(crate::unique::Unique::parse("[+2 Science] from every specialist [in all cities]"));
        assert_eq!(bonus_per_specialist(&w, &r, city).science, Fixed::whole(2));
    }

    #[test]
    fn a_starving_city_lets_go_of_its_worst_citizen() {
        let r = ruleset();
        let (mut w, city) = world(&r);
        let c = w.cities.get_mut(city).unwrap();
        c.population = 7;
        assign_idle_citizens(&w.grid, &r, c, CityFocus::None, Stats::ZERO);
        assert_eq!(c.worked.len() as u32 + count(c), 7);

        c.population = 5;
        release_extra_citizens(&w.grid, &r, c, CityFocus::None, Stats::ZERO);
        assert_eq!(c.worked.len() as u32 + count(c), 5, "no citizen works without existing");
        assert_eq!(c.idle_population(), 0);
    }

    #[test]
    fn a_slot_that_is_gone_is_given_up() {
        let r = ruleset();
        let (mut w, city) = world(&r);
        let merchant = r.specialists.id("Merchant").unwrap();
        let c = w.cities.get_mut(city).unwrap();
        c.specialists.insert(merchant, 1);
        c.built.clear(); // the market burned with the city
        release_extra_citizens(&w.grid, &r, c, CityFocus::None, Stats::ZERO);
        assert!(c.specialists.values().all(|n| *n == 0));
    }
}
