//! Which tile a citizen works.
//!
//! The first genuine *decision* in the engine, and the entry point to the
//! automation layer. Everything before this computed a value; this one chooses.
//!
//! It is a weighted sum, not a [`ModifierStack`](crate::modifiers). The stack
//! exists because game rules compose in ways where ordering must not matter;
//! ranking is a single dot product against a fixed weight vector, and dressing
//! it up as layered modifiers would add ceremony without adding a guarantee.
//!
//! The weights are Unciv's (`Stats.kt:157`), fractional parts included — they
//! are deliberate tie-breakers, with the reasoning written next to them in the
//! original: production is `12.01` to "tie break Production vs gold", science
//! `9.01` because "4 Science better than 3 Production".
//!
//! Food is not weighted like the others. It is split into the amount needed to
//! stop starving and the surplus beyond that, and only the first part gets the
//! emergency multiplier. Unciv explains why: scaling only what is needed lets a
//! starving city "pick a tile that gives exactly as much Food as we need to Not
//! Starve that also has other good yields instead of always picking the Highest
//! Food tile".

use crate::ruleset::Ruleset;
use crate::stats::{Fixed, Stat, Stats};
use crate::world::city::{food_consumed, workable_tiles, City};
use crate::world::grid::{TileGrid, TileId};
use crate::world::yields::tile_yield;

/// How much one point of each yield is worth, before focus.
pub fn ranking_weights() -> Stats {
    Stats {
        food: Fixed::whole(14),
        production: Fixed::permille(12_010),
        gold: Fixed::whole(6),
        science: Fixed::permille(9_010),
        culture: Fixed::whole(8),
        happiness: Fixed::whole(10),
        faith: Fixed::whole(7),
    }
}

/// Multiplier applied to the food a starving city needs to stop starving.
const STARVATION_URGENCY: i32 = 8;

/// What a city is being managed for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CityFocus {
    #[default]
    None,
    Single(Stat),
    /// Gold twice over, growth at one and a half.
    GoldGrowth,
    /// Production twice over, growth at one and a half.
    ProductionGrowth,
}

impl CityFocus {
    /// Unciv weights a focused stat at 3× (`CityFocus.kt`), leaving the rest at 1.
    pub fn multiplier(self, stat: Stat) -> Fixed {
        match self {
            CityFocus::None => Fixed::ONE,
            CityFocus::Single(s) if s == stat => Fixed::whole(3),
            CityFocus::Single(_) => Fixed::ONE,
            CityFocus::GoldGrowth => match stat {
                Stat::Gold => Fixed::whole(2),
                Stat::Food => Fixed::permille(1_500),
                _ => Fixed::ONE,
            },
            CityFocus::ProductionGrowth => match stat {
                Stat::Production => Fixed::whole(2),
                Stat::Food => Fixed::permille(1_500),
                _ => Fixed::ONE,
            },
        }
    }
}

/// How much a city values growth right now.
///
/// A small city wants to grow; a comfortable one would rather have production.
/// From the `NoFocus` branch of Unciv's food weighting
/// (`Automation.kt:63`).
fn growth_weight(city: &City, surplus_food: i32, focus: CityFocus) -> Fixed {
    if focus != CityFocus::None {
        return Fixed::ONE;
    }
    if city.population < 5 {
        return Fixed::whole(2);
    }
    if surplus_food > 0 {
        // Growth sits just under production once a city is established.
        return Fixed::permille(750);
    }
    Fixed::ONE
}

/// What one tile is worth to this city, and why.
#[derive(Debug, Clone, PartialEq)]
pub struct Ranking {
    pub score: Fixed,
    /// Per-stat contribution, so a choice can be explained rather than trusted.
    pub parts: Vec<(Stat, Fixed)>,
}

/// Score a set of yields for city work.
///
/// `surplus_food` is the city's current food balance: negative means starving,
/// which changes how food is valued.
pub fn rank_for_city_work(yields: Stats, city: &City, focus: CityFocus, surplus_food: i32) -> Ranking {
    let weights = ranking_weights();
    let mut parts = Vec::new();
    let mut score = Fixed::ZERO;

    for stat in Stat::ALL {
        let amount = yields[stat];
        if stat == Stat::Food {
            continue; // handled below
        }
        let v = (amount * weights[stat]) * focus.multiplier(stat);
        if !v.is_zero() {
            parts.push((stat, v));
        }
        score += v;
    }

    // Food splits: what stops the starving, and what is left over.
    let starving = surplus_food < 0;
    let feed = if starving { yields.food.min(Fixed::whole(-surplus_food)).max(Fixed::ZERO) } else { Fixed::ZERO };
    let growth = if city.avoid_growth { Fixed::ZERO } else { yields.food - feed };

    let food_value = (feed * weights.food) * Fixed::whole(STARVATION_URGENCY)
        + (((growth * weights.food) * growth_weight(city, surplus_food, focus)) * focus.multiplier(Stat::Food));

    if !food_value.is_zero() {
        parts.push((Stat::Food, food_value));
    }
    score += food_value;

    parts.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    Ranking { score, parts }
}

/// Rank every tile a city could work but is not working, best first.
///
/// Ties break on [`TileId`], so two machines reach the same answer.
pub fn rank_available_tiles(grid: &TileGrid, r: &Ruleset, city: &City, focus: CityFocus) -> Vec<(TileId, Ranking)> {
    rank_tiles(grid, r, city, focus, None)
}

fn observed_yield(
    grid: &TileGrid,
    r: &Ruleset,
    tile: TileId,
    observer: Option<&crate::world::entities::Civilization>,
) -> Stats {
    observer.map_or_else(|| tile_yield(grid, r, tile), |civ| crate::world::yields::tile_yield_for(grid, r, tile, civ))
}

fn rank_tiles(
    grid: &TileGrid,
    r: &Ruleset,
    city: &City,
    focus: CityFocus,
    observer: Option<&crate::world::entities::Civilization>,
) -> Vec<(TileId, Ranking)> {
    let surplus = crate::world::city::food_surplus(grid, r, city);
    let mut ranked: Vec<(TileId, Ranking)> = workable_tiles(grid, city)
        .into_iter()
        .filter(|t| !city.worked.contains(t))
        .map(|t| (t, rank_for_city_work(observed_yield(grid, r, t, observer), city, focus, surplus)))
        .collect();
    ranked.sort_by(|a, b| b.1.score.cmp(&a.1.score).then(a.0.cmp(&b.0)));
    ranked
}

/// The tile a new citizen should take.
pub fn best_tile_for_new_citizen(grid: &TileGrid, r: &Ruleset, city: &City, focus: CityFocus) -> Option<TileId> {
    rank_available_tiles(grid, r, city, focus).into_iter().next().map(|(t, _)| t)
}

/// Assign every idle citizen, greedily and one at a time.
///
/// Re-ranking after each assignment matters: taking a food tile can stop a city
/// starving, which changes what the next citizen should want.
///
/// Each citizen goes to the best open tile or the best open specialist slot,
/// whichever ranks higher; a tile must rank *above* the slot to win, and with
/// no slot open it must rank above nothing (`autoAssignPopulation`). One that
/// fits neither stays unemployed. `bonus` is what every specialist here gets
/// from uniques, which a city alone cannot see.
pub fn assign_idle_citizens(grid: &TileGrid, r: &Ruleset, city: &mut City, focus: CityFocus, bonus: Stats) {
    assign_idle(grid, r, city, focus, bonus, None)
}

pub fn assign_idle_citizens_for(
    grid: &TileGrid,
    r: &Ruleset,
    city: &mut City,
    focus: CityFocus,
    bonus: Stats,
    civ: &crate::world::entities::Civilization,
) {
    assign_idle(grid, r, city, focus, bonus, Some(civ))
}

fn assign_idle(
    grid: &TileGrid,
    r: &Ruleset,
    city: &mut City,
    focus: CityFocus,
    bonus: Stats,
    observer: Option<&crate::world::entities::Civilization>,
) {
    let slots = crate::world::specialists::slots(r, city);
    while city.idle_population() > 0 {
        let surplus = crate::world::city::food_surplus(grid, r, city);
        let tile = rank_tiles(grid, r, city, focus, observer).into_iter().next();
        let job = slots
            .iter()
            .filter(|(id, max)| city.specialists.get(id).copied().unwrap_or(0) < **max)
            .map(|(id, _)| (crate::world::specialists::rank(r, city, *id, bonus, focus, surplus), *id))
            // Best rank, then the lower id.
            .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
        let tile_value = tile.as_ref().map_or(Fixed::ZERO, |(_, rk)| rk.score);
        let job_value = job.map_or(Fixed::ZERO, |(v, _)| v);
        match (tile, job) {
            (Some((t, _)), _) if tile_value > job_value => city.worked.push(t),
            (_, Some((_, id))) => *city.specialists.entry(id).or_insert(0) += 1,
            _ => break,
        }
    }
}

/// Let go of citizens the city no longer has, worst first
/// (`unassignExtraPopulation`).
///
/// A tile outside the city's reach is dropped outright, and so is a slot whose
/// building is gone. Then, while more are assigned than exist — after a
/// famine, say — the worst tile and the worst specialist are compared and the
/// weaker let go. Without this a city that starved kept working the tile of
/// the citizen it lost.
/// Put a city's idle citizens to work, leaving the ones already placed where they are. Human cities call
/// this when they are founded and when they grow, as Unciv's governor does.
pub fn place_idle_citizens(world: &mut crate::world::World, r: &Ruleset, id: crate::arena::Id<City>) {
    let Some(mut city) = world.cities.get(id).cloned() else { return };
    let Some(civ) = world.civ(city.owner) else { return };
    let bonus = crate::world::specialists::bonus_per_specialist(world, r, id);
    assign_idle_citizens_for(&world.grid, r, &mut city, CityFocus::None, bonus, civ);
    if let Some(c) = world.cities.get_mut(id) {
        c.worked = city.worked;
        c.specialists = city.specialists;
    }
}

pub fn release_extra_citizens(grid: &TileGrid, r: &Ruleset, city: &mut City, focus: CityFocus, bonus: Stats) {
    release_extra(grid, r, city, focus, bonus, None)
}

pub fn release_extra_citizens_for(
    grid: &TileGrid,
    r: &Ruleset,
    city: &mut City,
    focus: CityFocus,
    bonus: Stats,
    civ: &crate::world::entities::Civilization,
) {
    release_extra(grid, r, city, focus, bonus, Some(civ))
}

fn release_extra(
    grid: &TileGrid,
    r: &Ruleset,
    city: &mut City,
    focus: CityFocus,
    bonus: Stats,
    observer: Option<&crate::world::entities::Civilization>,
) {
    let workable = workable_tiles(grid, city);
    city.worked.retain(|t| workable.contains(t));

    let slots = crate::world::specialists::slots(r, city);
    for (id, n) in city.specialists.iter_mut() {
        *n = (*n).min(slots.get(id).copied().unwrap_or(0));
    }
    city.specialists.retain(|_, n| *n > 0);

    while city.worked.len() as u32 + crate::world::specialists::count(city) > city.population {
        let surplus = crate::world::city::food_surplus(grid, r, city);
        let worst_tile = city
            .worked
            .iter()
            .map(|t| (rank_for_city_work(observed_yield(grid, r, *t, observer), city, focus, surplus).score, *t))
            .min_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
        let worst_job = city
            .specialists
            .keys()
            .map(|id| (crate::world::specialists::rank(r, city, *id, bonus, focus, surplus), *id))
            .min_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)));
        match (worst_tile, worst_job) {
            (Some((tv, t)), Some((jv, _))) if tv < jv => city.worked.retain(|w| *w != t),
            (_, Some((_, id))) => {
                if let Some(n) = city.specialists.get_mut(&id) {
                    *n -= 1;
                }
                city.specialists.retain(|_, n| *n > 0);
            }
            (Some((_, t)), None) => city.worked.retain(|w| *w != t),
            (None, None) => break,
        }
    }
}

/// Current food balance including consumption, for callers that have a city
/// but not a grid.
pub fn surplus_of(grid: &TileGrid, r: &Ruleset, city: &City) -> i32 {
    crate::world::city::city_yield(grid, r, city).food.round() as i32 - food_consumed(city)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::grid::CivId;

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("rules/gods-and-kings").expect("readable").expect("loads")
    }

    fn grid_of(r: &Ruleset, terrain: &str) -> TileGrid {
        let mut g = TileGrid::new(16, 16, r.terrain(terrain).unwrap());
        g.owner.iter_mut().for_each(|o| *o = Some(CivId(0)));
        g
    }

    fn city(g: &TileGrid) -> City {
        City::found("Rome", CivId(0), g.id_at(8, 8).unwrap())
    }

    /// The tie-breaks are the point of the fractional weights.
    #[test]
    fn the_weights_order_yields_as_unciv_intends() {
        let w = ranking_weights();
        assert!(w.food > w.production, "food leads");
        assert!(w.production > w.gold, "production over gold");
        assert!(w.science > w.culture && w.culture > w.faith);
        // "2 gold worth about 1 production" — and the `.01` is what makes
        // production win that tie rather than draw it. 2 gold is 12.000
        // against production's 12.010, so the two are worth the same to within
        // a hundredth and production takes it.
        let two_gold = w.gold * Fixed::whole(2);
        assert!(two_gold < w.production, "production wins the tie");
        assert_eq!(w.production - two_gold, Fixed::permille(10), "by exactly 0.01");

        // "4 Science better than 3 Production" — 36.04 against 36.03, again by
        // a hundredth, and again on purpose.
        let four_science = w.science * Fixed::whole(4);
        let three_production = w.production * Fixed::whole(3);
        assert!(four_science > three_production, "science takes it");
        assert_eq!(four_science - three_production, Fixed::permille(10));
    }

    #[test]
    fn a_focus_triples_its_stat() {
        let c = city(&grid_of(&ruleset(), "Grassland"));
        let yields = Stats::parse("+3 Production").unwrap();
        let plain = rank_for_city_work(yields, &c, CityFocus::None, 5).score;
        let focused = rank_for_city_work(yields, &c, CityFocus::Single(Stat::Production), 5).score;
        assert_eq!(focused, plain * Fixed::whole(3));
    }

    /// The behaviour the food split exists for: a starving city should not
    /// simply grab the most food.
    #[test]
    fn a_starving_city_values_exactly_the_food_it_needs() {
        let c = city(&grid_of(&ruleset(), "Grassland"));
        let starving = -2;

        // Two food and nothing else, versus one food plus strong production.
        let pure_food = Stats::parse("+2 Food").unwrap();
        let mixed = Stats::parse("+2 Food, +3 Production").unwrap();

        let a = rank_for_city_work(pure_food, &c, CityFocus::None, starving).score;
        let b = rank_for_city_work(mixed, &c, CityFocus::None, starving).score;
        assert!(b > a, "both end the starvation, so the extra production decides");

        // And food beyond what is needed is worth much less than food that is.
        let plenty = Stats::parse("+6 Food").unwrap();
        let c_score = rank_for_city_work(plenty, &c, CityFocus::None, starving).score;
        let surplus_part = c_score - a;
        assert!(surplus_part < a, "the 4 surplus food should be worth less than the 2 that stop starvation");
    }

    #[test]
    fn avoid_growth_discards_surplus_food_entirely() {
        let mut c = city(&grid_of(&ruleset(), "Grassland"));
        let yields = Stats::parse("+4 Food").unwrap();
        let normal = rank_for_city_work(yields, &c, CityFocus::None, 3).score;
        c.avoid_growth = true;
        let held = rank_for_city_work(yields, &c, CityFocus::None, 3).score;
        assert!(held < normal);
        assert_eq!(held, Fixed::ZERO, "food is all that tile offers, and it is unwanted");
    }

    #[test]
    fn small_cities_want_to_grow_more_than_large_ones() {
        let g = grid_of(&ruleset(), "Grassland");
        let mut small = city(&g);
        small.population = 2;
        let mut large = city(&g);
        large.population = 8;

        let yields = Stats::parse("+3 Food").unwrap();
        let s = rank_for_city_work(yields, &small, CityFocus::None, 4).score;
        let l = rank_for_city_work(yields, &large, CityFocus::None, 4).score;
        assert!(s > l, "under size 5 growth is weighted 2x, above it 0.75x");
    }

    #[test]
    fn ranking_is_explainable() {
        let c = city(&grid_of(&ruleset(), "Grassland"));
        let yields = Stats::parse("+2 Food, +1 Production").unwrap();
        let r = rank_for_city_work(yields, &c, CityFocus::None, 5);
        assert_eq!(r.parts.len(), 2);
        assert_eq!(r.parts.iter().map(|(_, v)| *v).sum::<Fixed>(), r.score);
        assert_eq!(r.parts[0].0, Stat::Food, "sorted by contribution");
    }

    #[test]
    fn citizens_take_the_best_tile_available() {
        let r = ruleset();
        let mut g = grid_of(&r, "Plains");
        let mut c = city(&g);

        // One clearly better tile nearby.
        let prize = g.neighbours(c.centre).next().unwrap();
        g.terrain[prize.0 as usize] = r.terrain("Grassland").unwrap();
        g.resource[prize.0 as usize] = r
            .resources
            .iter()
            .find(|(_, res)| !crate::ruleset::queries::HasStats::stats(*res).is_zero())
            .map(|(id, _)| id);

        assert_eq!(best_tile_for_new_citizen(&g, &r, &c, CityFocus::None), Some(prize));

        c.population = 3;
        assign_idle_citizens(&g, &r, &mut c, CityFocus::None, Stats::ZERO);
        assert_eq!(c.worked.len(), 3);
        assert_eq!(c.worked[0], prize, "the best tile went first");
        assert_eq!(c.idle_population(), 0);
    }

    #[test]
    fn assignment_is_deterministic_across_identical_tiles() {
        let r = ruleset();
        let g = grid_of(&r, "Grassland");
        let run = || {
            let mut c = city(&g);
            c.population = 5;
            assign_idle_citizens(&g, &r, &mut c, CityFocus::None, Stats::ZERO);
            c.worked
        };
        // Every tile is identical, so only the tie-break decides. It must not
        // vary between runs.
        assert_eq!(run(), run());
        assert_eq!(run().len(), 5);
    }

    #[test]
    fn a_city_cannot_work_more_tiles_than_it_owns() {
        let r = ruleset();
        let mut g = TileGrid::new(16, 16, r.terrain("Grassland").unwrap());
        let mut c = city(&g);
        // Own only three tiles around the centre.
        let owned: Vec<_> = g.neighbours(c.centre).take(3).collect();
        for t in &owned {
            g.owner[t.0 as usize] = Some(CivId(0));
        }
        c.population = 10;
        assign_idle_citizens(&g, &r, &mut c, CityFocus::None, Stats::ZERO);
        assert_eq!(c.worked.len(), 3, "the rest become specialists");
        assert_eq!(c.idle_population(), 7);
    }
}
