//! Where to put a city.
//!
//! The first decision that looks at a *region* rather than a tile, and the
//! first that looks ahead: a settler is choosing what its citizens will work
//! for the next two hundred turns.
//!
//! Unciv's ranker (`CityLocationTileRanker.kt:103`) sums the surrounding tiles
//! with a weight of `3/(ring+1)` out to ring two — so the centre counts triple,
//! its neighbours one and a half, the ring beyond them once — and then
//! subtracts 2.4 for every tile of that hexagon that is missing, which is what
//! makes a site on the map edge or a narrow peninsula unattractive.
//!
//! The site itself carries flat bonuses on top, and they are large relative to
//! the terrain: a hill is worth 14 and a river 20, against a grassland tile's
//! handful. Civ rewards defensible, well-watered ground far more than fertile
//! ground, and the numbers say so plainly.

use crate::arena::Id;
use crate::ruleset::Ruleset;
use crate::stats::{Fixed, Stat};
use crate::world::assignment::ranking_weights;
use crate::world::city::City;
use crate::world::entities::Unit;
use crate::world::grid::{CivId, TileId};
use crate::world::movement::is_passable;
use crate::world::world::World;
use crate::world::yields::tile_yield;

/// Whether this unit type founds cities.
///
/// By the `FoundCity` unique rather than the name "Settler" — a ruleset may
/// call it something else, and the same reasoning applies as to the palace.
pub fn is_settler(r: &Ruleset, base: Id<crate::ruleset::BaseUnit>) -> bool {
    r.units.get(base).is_some_and(|u| u.uniques.iter().any(|x| x.unique_type() == Some(crate::UniqueType::FoundCity)))
}

/// The ruleset's settler, if it has exactly one obvious candidate.
pub fn settler_unit(r: &Ruleset) -> Option<Id<crate::ruleset::BaseUnit>> {
    r.units
        .ids()
        .filter(|id| is_settler(r, *id))
        .filter(|id| r.units.get(*id).is_some_and(|u| u.cost.unwrap_or(0) > 0))
        .min()
}

/// Cities may not be founded within this many tiles of another
/// (`ModConstants.kt:49`).
pub const MINIMAL_CITY_DISTANCE: u32 = 3;

/// Tiles in a hexagon of radius two — the area a site is judged over.
const SITE_HEXAGON: usize = 19;

/// Subtracted for each tile of that hexagon the site does not have.
const MISSING_TILE_PENALTY: Fixed = Fixed::permille(2_400);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettleRejection {
    /// Water, or terrain nothing can stand on.
    Impassable,
    /// Inside another civilization's borders.
    ForeignTerritory(CivId),
    /// Another city is too near.
    TooClose {
        city: Id<City>,
        distance: u32,
    },
    /// The settler id no longer resolves.
    NoSuchUnit,
    CannotFound,
}

impl std::fmt::Display for SettleRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SettleRejection::Impassable => write!(f, "nothing can be founded on that terrain"),
            SettleRejection::ForeignTerritory(civ) => write!(f, "inside civ {}'s borders", civ.0),
            SettleRejection::TooClose { city, distance } => {
                write!(f, "{distance} tiles from city {city:?}, which is too close")
            }
            SettleRejection::NoSuchUnit => write!(f, "no such unit"),
            SettleRejection::CannotFound => write!(f, "this unit cannot found a city here"),
        }
    }
}

impl std::error::Error for SettleRejection {}

/// Whether a city may be founded here.
pub fn can_settle(world: &World, r: &Ruleset, civ: CivId, tile: TileId) -> Result<(), SettleRejection> {
    if !world.grid.contains(tile) || world.grid.is_water(r, tile) || !is_passable(&world.grid, r, tile) {
        return Err(SettleRejection::Impassable);
    }
    if let Some(owner) = world.grid.owner[tile.0 as usize] {
        if owner != civ {
            return Err(SettleRejection::ForeignTerritory(owner));
        }
    }
    for (id, city) in world.cities.iter() {
        let distance = world.grid.distance(tile, city.centre);
        if distance <= 2 || (distance <= MINIMAL_CITY_DISTANCE && same_continent(&world.grid, r, tile, city.centre)) {
            return Err(SettleRejection::TooClose { city: id, distance });
        }
    }
    Ok(())
}

/// Continents include mountain chains; passability is a unit rule, not a
/// boundary between two landmasses.
pub fn same_continent(grid: &crate::world::grid::TileGrid, r: &Ruleset, a: TileId, b: TileId) -> bool {
    if !grid.contains(a) || !grid.contains(b) || grid.is_water(r, a) || grid.is_water(r, b) {
        return false;
    }
    let mut seen = std::collections::BTreeSet::from([a]);
    let mut queue = std::collections::VecDeque::from([a]);
    while let Some(tile) = queue.pop_front() {
        if tile == b {
            return true;
        }
        for next in grid.neighbours(tile) {
            if !grid.is_water(r, next) && seen.insert(next) {
                queue.push_back(next);
            }
        }
    }
    false
}

pub fn can_found_with(world: &World, r: &Ruleset, unit: Id<Unit>) -> Result<(), SettleRejection> {
    let u = world.units.get(unit).ok_or(SettleRejection::NoSuchUnit)?;
    can_found_on(world, r, u, u.position)
}

pub fn can_found_on(world: &World, r: &Ruleset, u: &Unit, tile: TileId) -> Result<(), SettleRejection> {
    let ctx = crate::world::conditionals::GameContext::new(r)
        .with_world(world)
        .for_civ(u.owner)
        .for_unit_value(u)
        .on_tile(tile);
    let allowed = u.all_uniques(r).filter(|x| x.unique_type() == Some(crate::UniqueType::FoundCity)).any(|rule| {
        let mut rule = rule.clone();
        rule.conditionals.retain(|c| {
            !c.unique_type()
                .is_some_and(|t| t.targets().contains(&crate::unique_type::UniqueTarget::UnitActionModifier))
        });
        crate::world::conditionals::applies(&rule, &ctx).holds()
    });
    // Barbarians keep a captured settler only so it can be taken back
    // (BattleUnitCapture.kt:192); BarbarianAutomation never settles with it.
    let barbarian = world.civ(u.owner).is_some_and(|c| c.is_barbarian);
    if !u.can_move() || !allowed || barbarian {
        return Err(SettleRejection::CannotFound);
    }
    can_settle(world, r, u.owner, tile)
}

/// What one tile contributes, as its yields dotted with the work weights.
///
/// Unciv scores nearby tiles with its own function; reusing the weights that
/// already decide which tile a citizen works keeps one definition of "good
/// tile" in the engine rather than two that can drift apart.
pub fn tile_worth(world: &World, r: &Ruleset, tile: TileId) -> Fixed {
    tile_worth_for(world, r, tile, None)
}
fn tile_worth_for(
    world: &World,
    r: &Ruleset,
    tile: TileId,
    civ: Option<&crate::world::entities::Civilization>,
) -> Fixed {
    let y = civ.map_or_else(
        || tile_yield(&world.grid, r, tile),
        |c| crate::world::yields::tile_yield_for(&world.grid, r, tile, c),
    );
    let w = ranking_weights();
    Stat::ALL.into_iter().map(|s| y[s] * w[s]).sum::<Fixed>().checked_div(Fixed::whole(10)).unwrap_or(Fixed::ZERO)
}

/// Flat bonuses for the site tile itself (`CityLocationTileRanker.kt:107`).
fn site_bonuses(world: &World, r: &Ruleset, tile: TileId, civ: Option<&crate::world::entities::Civilization>) -> Fixed {
    let mut v = Fixed::ZERO;
    let named = |id| r.terrains.get(id).map(|t| t.name.clone());

    let on_hill = world.grid.terrains(tile).filter_map(named).any(|n| n == "Hill");
    if on_hill {
        v += Fixed::whole(14);
    }
    let next_to_mountain =
        world.grid.neighbours(tile).any(|n| world.grid.terrains(n).filter_map(named).any(|t| t == "Mountain"));
    if next_to_mountain {
        v += Fixed::whole(5);
    }
    // Building on a resource wastes it.
    if world.grid.resource[tile.0 as usize]
        .is_some_and(|res| civ.is_none_or(|c| crate::world::resources::is_visible(r, c, res)))
    {
        v -= Fixed::whole(4);
    }
    v
}

/// How good a city here would be. Higher is better; sites may score negative.
pub fn rank_site(world: &World, r: &Ruleset, tile: TileId) -> Fixed {
    rank_site_for(world, r, tile, None)
}
fn rank_site_for(
    world: &World,
    r: &Ruleset,
    tile: TileId,
    civ: Option<&crate::world::entities::Civilization>,
) -> Fixed {
    let mut value = site_bonuses(world, r, tile, civ);
    let mut counted = 0usize;

    for ring in 0..=2u32 {
        // 3, then 1.5, then 1 — the centre matters most.
        let weight = Fixed::whole(3).checked_div(Fixed::whole(ring as i32 + 1)).unwrap_or(Fixed::ONE);
        for t in world.grid.ring(tile, ring) {
            counted += 1;
            value += tile_worth_for(world, r, t, civ) * weight;
        }
    }

    // A site missing part of its hexagon — coastline, map edge — is worth less.
    let missing = SITE_HEXAGON.saturating_sub(counted);
    value - (Fixed::whole(missing as i32) * MISSING_TILE_PENALTY)
}

/// The best legal site within `radius` of `from`, best first.
pub fn best_site(world: &World, r: &Ruleset, civ: CivId, from: TileId, radius: u32) -> Option<(TileId, Fixed)> {
    ranked_sites(world, r, civ, from, radius).into_iter().next()
}

/// Every legal site within `radius`, best first, ties by id.
pub fn ranked_sites(world: &World, r: &Ruleset, civ: CivId, from: TileId, radius: u32) -> Vec<(TileId, Fixed)> {
    let mut sites: Vec<(TileId, Fixed)> = world
        .grid
        .within(from, radius)
        .into_iter()
        .filter(|t| can_settle(world, r, civ, *t).is_ok())
        .filter(|t| !crate::world::promises::promised_not_to_settle(world, civ, *t))
        .map(|t| (t, rank_site_for(world, r, t, world.civ(civ))))
        .collect();
    // Ties break on id, so the choice is the same on every machine.
    sites.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    sites
}

/// Whether anywhere within `radius` of `from` could take a city at all.
///
/// [`best_site`] answers this too, but by ranking every candidate first — and
/// the question "is there anywhere left?" is asked of every city, every turn,
/// most often when the answer is no and every candidate must be looked at.
pub fn has_site(world: &World, r: &Ruleset, civ: CivId, from: TileId, radius: u32) -> bool {
    world
        .grid
        .within(from, radius)
        .into_iter()
        .any(|t| can_settle(world, r, civ, t).is_ok() && !crate::world::promises::promised_not_to_settle(world, civ, t))
}

/// Found a city where a settler stands, consuming it.
pub fn found_city_with(
    world: &mut World,
    r: &Ruleset,
    settler: Id<Unit>,
    name: impl Into<String>,
) -> Result<Id<City>, SettleRejection> {
    can_found_with(world, r, settler)?;
    let unit = world.units.get(settler).ok_or(SettleRejection::NoSuchUnit)?;
    let (civ, tile) = (unit.owner, unit.position);
    can_settle(world, r, civ, tile)?;

    world.remove_unit(settler);
    let name = crate::world::city_names::founding_name(world, r, civ, &name.into());
    // `found_city_in`, not `found_city`: a civ's first city gets its palace.
    // Founding through the ruleset-free path left every capital founded by a
    // settler without one, so no civ in a real game had a capital at all.
    let id = world.found_city_in(r, City::found(name, civ, tile));

    // A new city claims its immediate surroundings.
    for t in world.grid.within(tile, 1) {
        if world.grid.owner[t.0 as usize].is_none() {
            world.claim_tile(id, t);
        }
    }
    if world.civ(civ).is_some_and(|c| c.is_city_state()) {
        crate::world::city_state_actions::initialize(world, r);
    }
    Ok(id)
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
        let g = TileGrid::new(20, 20, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        World::new(g, vec![Civilization::new(CivId(0), nation, false), Civilization::new(CivId(1), nation, true)])
    }

    fn settler(w: &mut World, r: &Ruleset, at: TileId, civ: CivId) -> Id<Unit> {
        let base = r.unit("Settler").or_else(|| r.units.ids().next()).unwrap();
        w.place_unit(Unit::new(base, civ, at, 2))
    }

    #[test]
    fn cities_cannot_crowd_each_other() {
        let r = ruleset();
        let mut w = world(&r);
        let centre = w.grid.id_at(10, 10).unwrap();
        w.found_city(City::found("Rome", CivId(0), centre));

        for distance in 0..=MINIMAL_CITY_DISTANCE {
            let near = w.grid.ring(centre, distance)[0];
            assert!(
                matches!(can_settle(&w, &r, CivId(0), near), Err(SettleRejection::TooClose { .. })),
                "distance {distance} should be refused"
            );
        }
        let far = w.grid.ring(centre, MINIMAL_CITY_DISTANCE + 1)[0];
        assert_eq!(can_settle(&w, &r, CivId(0), far), Ok(()), "one step further is fine");
    }

    #[test]
    fn barbarians_keep_a_captured_settler_but_never_settle() {
        let r = ruleset();
        let mut w = world(&r);
        w.civ_mut(CivId(1)).unwrap().is_barbarian = true;
        let at = w.grid.id_at(10, 10).unwrap();
        let captive = settler(&mut w, &r, at, CivId(1));
        assert_eq!(can_found_with(&w, &r, captive), Err(SettleRejection::CannotFound));
        let own = settler(&mut w, &r, at, CivId(0));
        assert_eq!(can_found_with(&w, &r, own), Ok(()));
    }

    #[test]
    fn foreign_borders_and_impassable_ground_are_refused() {
        let r = ruleset();
        let mut w = world(&r);
        let theirs = w.grid.id_at(3, 3).unwrap();
        w.grid.owner[theirs.0 as usize] = Some(CivId(1));
        assert_eq!(can_settle(&w, &r, CivId(0), theirs), Err(SettleRejection::ForeignTerritory(CivId(1))));

        let mountain = r.terrains.iter().find(|(_, t)| t.impassable == Some(true)).unwrap().0;
        let rock = w.grid.id_at(5, 5).unwrap();
        w.grid.terrain[rock.0 as usize] = mountain;
        w.grid.features[rock.0 as usize].clear();
        assert_eq!(can_settle(&w, &r, CivId(0), rock), Err(SettleRejection::Impassable));
    }

    /// Hills are worth 14 — far more than the terrain under them.
    #[test]
    fn a_hill_site_beats_flat_ground() {
        let r = ruleset();
        let mut w = world(&r);
        let flat = w.grid.id_at(10, 10).unwrap();
        let hill_site = w.grid.id_at(4, 4).unwrap();
        w.grid.features[hill_site.0 as usize].push(r.terrain("Hill").unwrap());

        let (a, b) = (rank_site(&w, &r, flat), rank_site(&w, &r, hill_site));
        println!("flat {a}, hill {b}");
        assert!(b > a, "Civ rewards defensible ground");
    }

    #[test]
    fn the_map_edge_costs_a_site_its_missing_tiles() {
        let r = ruleset();
        let w = world(&r);
        let middle = rank_site(&w, &r, w.grid.id_at(10, 10).unwrap());
        let corner = rank_site(&w, &r, w.grid.id_at(0, 0).unwrap());
        println!("middle {middle}, corner {corner}");
        assert!(corner < middle, "a corner is missing most of its hexagon");
    }

    /// Every resource in the ruleset yields something, so a resource on the
    /// site is worth its yield *minus* four for the waste of building over it.
    /// Checking the exact delta pins the penalty rather than assuming which
    /// side of it wins.
    #[test]
    fn building_on_a_resource_costs_four() {
        let r = ruleset();
        let mut w = world(&r);
        let site = w.grid.id_at(10, 10).unwrap();

        let before_score = rank_site(&w, &r, site);
        let before_worth = tile_worth(&w, &r, site);

        let res = r
            .resources
            .iter()
            .find(|(_, x)| !crate::ruleset::queries::HasStats::stats(*x).is_zero())
            .map(|(id, _)| id)
            .expect("resources carry yields");
        w.grid.resource[site.0 as usize] = Some(res);

        let after_score = rank_site(&w, &r, site);
        let after_worth = tile_worth(&w, &r, site);

        // The centre sits in ring zero, weighted three.
        let yield_gain = (after_worth - before_worth) * Fixed::whole(3);
        assert_eq!(
            after_score - before_score,
            yield_gain - Fixed::whole(4),
            "the resource is worth its yield less the four-point waste"
        );
    }

    #[test]
    fn a_settler_becomes_a_city_and_claims_ground() {
        let r = ruleset();
        let mut w = world(&r);
        let site = w.grid.id_at(10, 10).unwrap();
        let s = settler(&mut w, &r, site, CivId(0));

        let id = found_city_with(&mut w, &r, s, "Rome").expect("a legal site");
        assert!(w.units.get(s).is_none(), "the settler is consumed");
        assert_eq!(w.cities.get(id).unwrap().centre, site);
        assert_eq!(w.civ(CivId(0)).unwrap().cities, vec![id]);

        // The centre and its neighbours are now Roman.
        for t in w.grid.within(site, 1) {
            assert_eq!(w.grid.owner[t.0 as usize], Some(CivId(0)));
        }
    }

    #[test]
    fn a_settlers_first_city_is_a_capital() {
        let r = ruleset();
        let mut w = world(&r);
        let site = w.grid.id_at(10, 10).unwrap();
        let s = settler(&mut w, &r, site, CivId(0));
        let rome = found_city_with(&mut w, &r, s, "Rome").expect("a legal site");
        assert_eq!(
            crate::world::connections::capital_of(&w, &r, CivId(0)),
            Some(rome),
            "the first city has the palace"
        );
        assert!(w.cities.get(rome).unwrap().original_capital);

        let far = w.grid.id_at(2, 2).unwrap();
        let s2 = settler(&mut w, &r, far, CivId(0));
        let antium = found_city_with(&mut w, &r, s2, "Antium").expect("a legal site");
        assert_eq!(crate::world::connections::capital_of(&w, &r, CivId(0)), Some(rome), "and the second does not");
        assert!(!w.cities.get(antium).unwrap().original_capital);
    }

    #[test]
    fn a_settler_cannot_found_on_top_of_a_neighbour() {
        let r = ruleset();
        let mut w = world(&r);
        let centre = w.grid.id_at(10, 10).unwrap();
        w.found_city(City::found("Rome", CivId(0), centre));

        let crowded = w.grid.ring(centre, 2)[0];
        let s = settler(&mut w, &r, crowded, CivId(0));
        assert!(matches!(found_city_with(&mut w, &r, s, "Antium"), Err(SettleRejection::TooClose { .. })));
        assert!(w.units.get(s).is_some(), "and the settler survives the refusal");
    }

    #[test]
    fn the_best_site_is_found_and_is_the_same_every_time() {
        let r = ruleset();
        let mut w = world(&r);
        let from = w.grid.id_at(10, 10).unwrap();
        // One obviously better spot within reach.
        let prize = w.grid.id_at(12, 11).unwrap();
        w.grid.features[prize.0 as usize].push(r.terrain("Hill").unwrap());

        let (best, score) = best_site(&w, &r, CivId(0), from, 4).expect("somewhere to settle");
        println!("best {best:?} at {score}");
        assert_eq!(best, prize);
        assert_eq!(best_site(&w, &r, CivId(0), from, 4), Some((best, score)));
    }
}
