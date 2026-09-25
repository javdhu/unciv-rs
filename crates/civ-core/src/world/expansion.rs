//! Borders: how a city's territory grows after it is founded.
//!
//! A city is founded with its centre and the ring around it, and everything
//! after that is bought — with culture, one tile at a time, or with gold. The
//! rules are `CityExpansionManager.kt`.
//!
//! The price of the next tile rises with the tiles already claimed beyond the
//! free ring, on the curve `6 × (n + 1.4813)^1.3`. The 1.4813 has more
//! decimals than a [`Fixed`] holds, and the result is rounded to a whole
//! number, so the curve is worked in [`Precise`] from the start: rounded to
//! thousandths first, the base would move a result across a half often
//! enough to matter.
//!
//! Which tile comes next is a ranking, lower is better, ported as written
//! (`Automation.rankTileForExpansion`). Unciv takes the first lowest-ranked
//! tile in its own ring order, and the original game a random one; here a tie
//! goes to the lower tile id, which is the only order two machines agree on.

use crate::arena::Id;
use crate::ruleset::{Ruleset, TileResource};
use crate::stats::{Fixed, Precise, Stat, SCALE};
use crate::unique::Unique;
use crate::unique_type::UniqueType;
use crate::world::city::{City, WORK_RADIUS};
use crate::world::conditionals::{applies, GameContext, Verdict};
use crate::world::entities::Civilization;
use crate::world::grid::{TileGrid, TileId};
use crate::world::world::World;

/// How far from its centre a city's borders can reach (`ModConstants.cityExpandRange`).
pub const EXPAND_RANGE: u32 = 5;

/// Gold per ring of distance beyond the first, for a bought tile.
const TILE_GOLD_PER_RING: i64 = 50;
/// Gold added per tile already claimed beyond the free ring.
const TILE_GOLD_PER_CLAIMED: i64 = 5;

/// Tiles this city has claimed beyond its centre and the ring it was founded
/// with (`tilesClaimed`). The founding ring is free, so it does not raise the
/// price of what comes after.
pub fn tiles_claimed(grid: &TileGrid, city: &City) -> i64 {
    let ring: Vec<TileId> = grid.neighbours(city.centre).collect();
    city.tiles.iter().filter(|t| **t != city.centre && !ring.contains(t)).count() as i64
}

/// `6 × (claimed + 1.4813)^1.3`, scaled, then rounded half up — the order
/// `getCultureToNextTile` applies them in.
pub fn border_cost(claimed: i64, speed: Fixed, city_state: bool, multipliers: &[Precise]) -> i64 {
    let base = Precise::from_ratio(claimed.max(0) * 10_000 + 14_813, 10_000);
    let mut cost = Precise::whole(6) * base.pow(Fixed::permille(1_300)).unwrap_or(Precise::whole(0));
    cost = cost * Precise::from_fixed(speed);
    if city_state {
        cost = cost * Precise::from_ratio(3, 2);
    }
    for m in multipliers {
        cost = cost * *m;
    }
    cost.round()
}

/// Culture this city needs for its next tile.
pub fn culture_to_next_tile(world: &World, r: &Ruleset, city: Id<City>, speed: Fixed) -> Option<i64> {
    let c = world.cities.get(city)?;
    let city_state = world.civ(c.owner).is_some_and(|civ| civ.is_city_state());
    let multipliers = percent_multipliers(world, r, city, UniqueType::BorderGrowthPercentage);
    Some(border_cost(tiles_claimed(&world.grid, c), speed, city_state, &multipliers))
}

/// Every `[n]% ... [cityFilter]` unique that reaches this city, as a factor.
///
/// A building's unique speaks for its own city ("in this city"); a policy's or
/// a nation's is the civ's and names the cities it covers. A filter the engine
/// cannot answer withholds its discount rather than guessing.
fn percent_multipliers(world: &World, r: &Ruleset, city_id: Id<City>, t: UniqueType) -> Vec<Precise> {
    let Some(city) = world.cities.get(city_id) else { return Vec::new() };
    let ctx = GameContext::new(r).with_world(world).for_civ(city.owner).in_city(city_id);
    let factor = |u: &Unique, origin: Option<&City>| -> Option<Precise> {
        if applies(u, &ctx) != Verdict::True {
            return None;
        }
        let pct = u.params.first().and_then(|p| Fixed::parse(p))?;
        let filter = u.params.get(1)?;
        if crate::world::filters::city_matches(world, r, city, origin, filter) != Verdict::True {
            return None;
        }
        Some(Precise::from_ratio(100 + pct.round(), 100))
    };
    let from_city = city.uniques.matching(t).filter_map(|u| factor(u, Some(city)));
    let from_civ =
        world.civ(city.owner).into_iter().flat_map(|c| c.uniques.matching(t)).filter_map(|u| factor(u, None));
    from_city.chain(from_civ).collect()
}

/// Whether the civ knows what is under a tile — a resource hidden behind an
/// unresearched tech does not count for or against it (`canSeeResource`).
fn can_see(r: &Ruleset, civ: &Civilization, resource: Id<TileResource>) -> bool {
    crate::world::resources::is_visible(r, civ, resource)
}

fn is_bonus(r: &Ruleset, resource: Id<TileResource>) -> bool {
    r.resources.get(resource).is_some_and(|res| res.resource_type == "Bonus")
}

/// How much this city wants `tile` next. Lower is better.
pub fn rank_tile(world: &World, r: &Ruleset, city: &City, tile: TileId) -> i64 {
    let grid = &world.grid;
    let Some(civ) = world.civ(city.owner) else { return i64::MAX };
    let distance = grid.distance(tile, city.centre);
    let mut score = distance as i64 * 100;

    // Resources are good; a bonus resource only if it can be worked.
    match grid.resource[tile.0 as usize].filter(|res| can_see(r, civ, *res)) {
        Some(res) if !is_bonus(r, res) => score -= 105,
        Some(_) if distance <= WORK_RADIUS => score -= 104,
        Some(_) => {}
        None => {
            // Open water is poor, and out of reach is worse.
            if grid.is_water(r, tile) {
                score += 3;
            }
            if distance > WORK_RADIUS {
                score += 100;
            }
        }
    }
    if grid.natural_wonder[tile.0 as usize].is_some() {
        score -= 105;
    }

    // The sum of every yield, truncated as Kotlin's `toInt` does.
    let y = crate::world::yields::tile_yield_for(grid, r, tile, civ);
    let total: Fixed = Stat::ALL.into_iter().map(|s| y[s]).sum();
    score -= total.raw() / SCALE;

    // What the tile opens up beyond it.
    let (mut near_wonder, mut contested) = (false, false);
    for n in grid.neighbours(tile) {
        if !world.visibility.has_explored(city.owner, n) || grid.owner[n.0 as usize] == Some(city.owner) {
            continue;
        }
        if grid.owner[n.0 as usize].is_some() {
            contested = true;
            continue;
        }
        let d = grid.distance(city.centre, n);
        if grid.resource[n.0 as usize]
            .is_some_and(|res| can_see(r, civ, res) && (d <= WORK_RADIUS || !is_bonus(r, res)))
        {
            score -= 1;
        }
        if grid.natural_wonder[n.0 as usize].is_some() {
            near_wonder |= d <= WORK_RADIUS;
            score -= 1;
        }
    }
    if near_wonder {
        score -= 1;
    }
    if contested {
        score -= 1;
    }

    // Borders grow outward; a tile touching none of this city's is all but
    // unreachable.
    if !grid.neighbours(tile).any(|n| city.tiles.binary_search(&n).is_ok()) {
        score += 1000;
    }
    score
}

/// The tile this city's borders take next, if any is left in range.
pub fn choose_tile(world: &World, r: &Ruleset, city: Id<City>) -> Option<TileId> {
    let c = world.cities.get(city)?;
    world
        .grid
        .within(c.centre, EXPAND_RANGE)
        .into_iter()
        .filter(|t| world.grid.owner[t.0 as usize].is_none())
        .min_by_key(|t| (rank_tile(world, r, c, *t), *t))
}

/// One turn of border growth: bank the culture, and claim a tile if it is
/// enough (`CityExpansionManager.nextTurn`).
///
/// At most one tile a turn, however much is banked, and the surplus carries.
pub fn grow(world: &mut World, r: &Ruleset, city: Id<City>, culture: i32, speed: Fixed) -> Option<TileId> {
    world.cities.get_mut(city)?.culture_stored += culture.max(0);
    let cost = culture_to_next_tile(world, r, city, speed)?;
    if (world.cities.get(city)?.culture_stored as i64) < cost {
        return None;
    }
    let tile = choose_tile(world, r, city)?;
    world.cities.get_mut(city)?.culture_stored -= cost as i32;
    world.claim_tile(city, tile);
    Some(tile)
}

/// Gold to buy `tile` for this city (`getGoldCostOfTile`).
pub fn gold_cost_of_tile(world: &World, r: &Ruleset, city: Id<City>, tile: TileId, gold_speed: Fixed) -> Option<i64> {
    let c = world.cities.get(city)?;
    let distance = world.grid.distance(tile, c.centre) as i64;
    let base = TILE_GOLD_PER_RING * (distance - 1) + tiles_claimed(&world.grid, c) * TILE_GOLD_PER_CLAIMED;
    let mut cost = Precise::from_ratio(base, 1) * Precise::from_fixed(gold_speed);
    for m in percent_multipliers(world, r, city, UniqueType::TileCostPercentage) {
        cost = cost * m;
    }
    Some(cost.round())
}

/// Why a tile could not be bought.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuyTileRejection {
    NoSuchCity,
    /// Someone already holds it.
    AlreadyOwned,
    /// Beyond the city's work range.
    OutOfRange,
    /// None of its neighbours is this city's.
    NotContiguous,
    /// A puppet, a city being razed, or one still resisting buys no land.
    Unavailable,
    NotEnoughGold {
        cost: i64,
        gold: i32,
    },
}

impl std::fmt::Display for BuyTileRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuyTileRejection::NoSuchCity => write!(f, "no such city"),
            BuyTileRejection::AlreadyOwned => write!(f, "the tile is already owned"),
            BuyTileRejection::OutOfRange => write!(f, "beyond the city's work range"),
            BuyTileRejection::NotContiguous => write!(f, "the tile does not touch the city's borders"),
            BuyTileRejection::Unavailable => write!(f, "the city cannot buy land now"),
            BuyTileRejection::NotEnoughGold { cost, gold } => write!(f, "costs {cost} gold, has {gold}"),
        }
    }
}

impl std::error::Error for BuyTileRejection {}

/// What buying this tile would cost, if the city may buy it at all — every
/// check [`buy_tile`] makes except whether the treasury holds enough.
pub fn tile_price(
    world: &World,
    r: &Ruleset,
    city: Id<City>,
    tile: TileId,
    gold_speed: Fixed,
) -> Result<i64, BuyTileRejection> {
    let c = world.cities.get(city).ok_or(BuyTileRejection::NoSuchCity)?;
    if c.puppet || c.razing || c.resistance > 0 {
        return Err(BuyTileRejection::Unavailable);
    }
    if world.grid.owner[tile.0 as usize].is_some() {
        return Err(BuyTileRejection::AlreadyOwned);
    }
    if world.grid.distance(tile, c.centre) > WORK_RADIUS {
        return Err(BuyTileRejection::OutOfRange);
    }
    if !world.grid.neighbours(tile).any(|n| c.tiles.binary_search(&n).is_ok()) {
        return Err(BuyTileRejection::NotContiguous);
    }
    gold_cost_of_tile(world, r, city, tile, gold_speed).ok_or(BuyTileRejection::NoSuchCity)
}

/// Buy a tile with gold, returning what it cost (`canBuyTile`, `buyTile`).
pub fn buy_tile(
    world: &mut World,
    r: &Ruleset,
    city: Id<City>,
    tile: TileId,
    gold_speed: Fixed,
) -> Result<i64, BuyTileRejection> {
    let cost = tile_price(world, r, city, tile, gold_speed)?;
    let owner = world.cities.get(city).ok_or(BuyTileRejection::NoSuchCity)?.owner;
    let gold = world.civ(owner).map_or(0, |c| c.gold);
    if (gold as i64) < cost {
        return Err(BuyTileRejection::NotEnoughGold { cost, gold });
    }
    if let Some(c) = world.civ_mut(owner) {
        c.gold -= cost as i32;
    }
    world.claim_tile(city, tile);
    Ok(cost)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::grid::CivId;

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("rules/gods-and-kings").expect("readable").expect("loads")
    }

    /// One civ, one city at (8, 8) holding its founding ring, the whole map
    /// explored.
    fn world(r: &Ruleset) -> (World, Id<City>) {
        let g = TileGrid::new(16, 16, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        let mut w = World::new(g, vec![Civilization::new(CivId(0), nation, false)]);
        let centre = w.grid.id_at(8, 8).unwrap();
        let city = w.found_city(City::found("Home", CivId(0), centre));
        for t in w.grid.within(centre, 1) {
            w.claim_tile(city, t);
        }
        for i in 0..w.grid.len() {
            w.visibility.observe(CivId(0), TileId(i as u16));
        }
        (w, city)
    }

    #[test]
    fn the_founding_ring_is_free() {
        let r = ruleset();
        let (w, city) = world(&r);
        assert_eq!(w.cities.get(city).unwrap().tiles.len(), 7);
        assert_eq!(tiles_claimed(&w.grid, w.cities.get(city).unwrap()), 0);
    }

    #[test]
    fn the_culture_curve_matches_the_floating_point_original() {
        // Unciv computes this in doubles; the reference is the same formula
        // in f64, checked tile by tile across the whole range a game reaches.
        for claimed in 0..=60i64 {
            for (speed, speed_f) in [(Fixed::ONE, 1.0), (Fixed::permille(670), 0.67), (Fixed::whole(3), 3.0)] {
                for city_state in [false, true] {
                    let mut want = 6.0 * (claimed as f64 + 1.4813).powf(1.3) * speed_f;
                    if city_state {
                        want *= 1.5;
                    }
                    let got = border_cost(claimed, speed, city_state, &[]);
                    assert_eq!(got, want.round() as i64, "claimed {claimed}, speed {speed_f}, city-state {city_state}");
                }
            }
        }
    }

    #[test]
    fn a_policy_discount_makes_border_growth_cheaper() {
        let r = ruleset();
        let (mut w, city) = world(&r);
        let before = culture_to_next_tile(&w, &r, city, Fixed::ONE).unwrap();
        w.civ_mut(CivId(0))
            .unwrap()
            .uniques
            .insert(Unique::parse("[-25]% Culture cost of natural border growth [in all cities]"));
        let after = culture_to_next_tile(&w, &r, city, Fixed::ONE).unwrap();
        assert_eq!(after, border_cost(0, Fixed::ONE, false, &[Precise::from_ratio(3, 4)]));
        assert!(after < before, "{after} should be under {before}");
    }

    #[test]
    fn culture_buys_one_tile_a_turn_and_the_rest_carries() {
        let r = ruleset();
        let (mut w, city) = world(&r);
        let cost = culture_to_next_tile(&w, &r, city, Fixed::ONE).unwrap() as i32;

        assert_eq!(grow(&mut w, &r, city, cost - 1, Fixed::ONE), None, "one short buys nothing");
        let tile = grow(&mut w, &r, city, 1, Fixed::ONE).expect("exactly enough buys a tile");
        let c = w.cities.get(city).unwrap();
        assert_eq!(c.culture_stored, 0);
        assert!(c.tiles.contains(&tile));
        assert_eq!(w.grid.owner[tile.0 as usize], Some(CivId(0)));
        assert_eq!(w.grid.distance(tile, c.centre), 2, "the next ring out, not a leap");

        // A windfall still buys only one tile, and keeps the change.
        let next = culture_to_next_tile(&w, &r, city, Fixed::ONE).unwrap() as i32;
        grow(&mut w, &r, city, next * 5, Fixed::ONE).expect("a tile");
        assert_eq!(w.cities.get(city).unwrap().culture_stored, next * 4);
    }

    #[test]
    fn a_resource_draws_the_border_toward_it() {
        let r = ruleset();
        let (mut w, city) = world(&r);
        // A luxury two rings out, on the far side from the lowest ids.
        let target = w.grid.id_at(10, 8).unwrap();
        assert_eq!(w.grid.distance(target, w.cities.get(city).unwrap().centre), 2);
        let silk = r.resources.id("Silk").expect("Silk is a luxury");
        w.grid.resource[target.0 as usize] = Some(silk);

        assert_eq!(choose_tile(&w, &r, city), Some(target));
    }

    #[test]
    fn a_tile_that_touches_nothing_of_ours_is_last() {
        let r = ruleset();
        let (w, city) = world(&r);
        let c = w.cities.get(city).unwrap();
        let near = w.grid.id_at(10, 8).unwrap();
        let far = w.grid.id_at(11, 8).unwrap();
        assert!(rank_tile(&w, &r, c, far) - rank_tile(&w, &r, c, near) > 1000);
    }

    #[test]
    fn a_bought_tile_costs_by_distance_and_claims() {
        let r = ruleset();
        let (mut w, city) = world(&r);
        w.civ_mut(CivId(0)).unwrap().gold = 1_000;
        let ring2 = w.grid.id_at(10, 8).unwrap();
        let ring3 = w.grid.id_at(11, 8).unwrap();

        // Fifty for the second ring, nothing yet claimed beyond the first.
        assert_eq!(gold_cost_of_tile(&w, &r, city, ring2, Fixed::ONE), Some(50));
        assert_eq!(buy_tile(&mut w, &r, city, ring3, Fixed::ONE), Err(BuyTileRejection::NotContiguous));
        assert_eq!(buy_tile(&mut w, &r, city, ring2, Fixed::ONE), Ok(50));
        assert_eq!(w.civ(CivId(0)).unwrap().gold, 950);
        assert_eq!(buy_tile(&mut w, &r, city, ring2, Fixed::ONE), Err(BuyTileRejection::AlreadyOwned));

        // Now contiguous: a hundred for the third ring, plus five for the one
        // tile already claimed beyond the first.
        assert_eq!(gold_cost_of_tile(&w, &r, city, ring3, Fixed::ONE), Some(105));
        let beyond = w.grid.id_at(12, 8).unwrap();
        assert_eq!(buy_tile(&mut w, &r, city, beyond, Fixed::ONE), Err(BuyTileRejection::OutOfRange));

        w.civ_mut(CivId(0)).unwrap().gold = 10;
        assert_eq!(
            buy_tile(&mut w, &r, city, ring3, Fixed::ONE),
            Err(BuyTileRejection::NotEnoughGold { cost: 105, gold: 10 })
        );
    }

    #[test]
    fn a_capital_grows_its_borders_turn_by_turn() {
        use crate::world::assignment::CityFocus;
        use crate::world::turn::{advance_turn, TurnEvent, TurnModifiers};
        let r = ruleset();
        let g = TileGrid::new(16, 16, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        let mut w = World::new(g, vec![Civilization::new(CivId(0), nation, false)]);
        let centre = w.grid.id_at(8, 8).unwrap();
        let city = w.found_city_in(&r, City::found("Home", CivId(0), centre));

        let mut grew = Vec::new();
        for turn in 1..=40 {
            for e in advance_turn(&mut w, &r, CityFocus::None, TurnModifiers::default()) {
                if let TurnEvent::BordersExpanded { city: c, tile } = e {
                    assert_eq!(c, city);
                    grew.push((turn, tile));
                }
            }
        }
        println!("border growth: {grew:?}");
        assert!(grew.len() >= 2, "forty turns of a capital's culture buys more than one tile: {grew:?}");
        let c = w.cities.get(city).unwrap();
        for (_, tile) in &grew {
            assert!(c.tiles.contains(tile));
            assert_eq!(w.grid.owner[tile.0 as usize], Some(CivId(0)));
        }
    }

    #[test]
    fn a_conqueror_takes_the_whole_territory() {
        let r = ruleset();
        let (mut w, city) = world(&r);
        w.civs.push(Civilization::new(CivId(1), r.nations.ids().nth(1).unwrap(), false));
        let far = w.grid.id_at(10, 8).unwrap();
        w.claim_tile(city, far);

        crate::world::combat::capture_city(&mut w, &r, city, CivId(1));
        for t in w.cities.get(city).unwrap().tiles.clone() {
            assert_eq!(w.grid.owner[t.0 as usize], Some(CivId(1)), "tile {t:?} stayed behind");
        }
    }
}
