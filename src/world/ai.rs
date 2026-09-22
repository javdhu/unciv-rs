//! Deciding what a civilization does.
//!
//! The only system here with no obligation to match Unciv. Rules are rules and
//! must agree; heuristics are opinions and may differ freely, so this is
//! written for clarity rather than fidelity — and a differential harness has
//! nothing to say about it.
//!
//! Everything is a pure function of `&World`. That is not incidental: the turn
//! loop's phase split means planning cannot mutate, which makes these testable
//! without a mutable world, swappable for a better opinion later, and runnable
//! for every civ at once. Unciv can do none of the three — it retrofitted
//! threading with 21 `synchronized` blocks.
//!
//! Every choice breaks ties on id. An AI that picked differently depending on
//! hash order would desync two machines running the same game.

use crate::arena::Id;
use crate::ruleset::{Policy, Ruleset, Technology};
use crate::stats::{Fixed, Stat};
use crate::world::assignment::ranking_weights;
use crate::world::city::City;
use crate::world::construction::{available, production_cost, Construction, CostModifiers};
use crate::world::grid::CivId;
use crate::world::policies::{is_adoptable, policy_cost, PolicyModifiers};
use crate::world::research::{available_techs, tech_cost, ScienceModifiers};
use crate::world::world::World;

/// What to research next.
///
/// Cheapest first. A crude opinion, but a defensible one: early techs unlock
/// the things later techs build on, so rushing an expensive one usually costs
/// more than it returns. Ties break on id.
pub fn choose_research(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    m: ScienceModifiers,
) -> Option<Id<Technology>> {
    let cities = world.civ(civ).map_or(0, |c| c.cities.len());
    available_techs(r, world.civ(civ)?)
        .into_iter()
        .filter_map(|t| tech_cost(r, t, cities, m).map(|c| (c, t)))
        .min_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)))
        .map(|(_, t)| t)
}

/// What a city should build.
///
/// Scored as yield-per-hammer: what the thing produces, weighted the same way a
/// citizen's tile is weighted, divided by what it costs. Reusing
/// [`ranking_weights`] keeps one definition of "valuable" in the engine rather
/// than two that drift.
///
/// Units score zero under this, which is correct for a builder and wrong for a
/// general — a military AI belongs on top of this, not inside it.
///
/// `net_gold` is what the civ makes per turn after existing upkeep. Anything
/// costing more than that to keep is refused. Self-play otherwise reaches -168
/// gold in a hundred turns, because the rules only *penalise* debt — out of
/// research — and never prevent it. Staying solvent is an opinion, so it lives
/// here rather than in the rules.
pub fn choose_construction(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    city: &City,
    m: CostModifiers,
    net_gold: i32,
    want_settler: bool,
) -> Option<Construction> {
    let civ_ref = world.civ(civ)?;
    let weights = ranking_weights();

    available(r, civ_ref, city)
        .into_iter()
        .filter_map(|what| {
            let cost = production_cost(r, what, m)?;
            let value = match what {
                // A settler is worth more than any building while the empire
                // is small — land compounds and a granary does not.
                Construction::Unit(id) if crate::world::settling::is_settler(r, id) && want_settler => {
                    Fixed::whole(400)
                }
                Construction::Building(id) => {
                    let b = r.buildings.get(id)?;
                    if b.maintenance.unwrap_or(0) > net_gold.max(0) {
                        return None; // cannot keep it
                    }
                    let stats = crate::ruleset::queries::HasStats::stats(b);
                    let flat: Fixed = Stat::ALL.into_iter().map(|s| stats[s].mul(weights[s])).sum();
                    // A percentage is worth something even with nothing to
                    // multiply yet, or nothing would ever build a Library.
                    let pct: i32 = b.percent_stat_bonus.values().sum();
                    flat + Fixed::whole(pct)
                }
                Construction::Unit(_) => Fixed::ZERO,
            };
            let score = value.div(Fixed::whole(cost.max(1)))?;
            Some((score, what))
        })
        .filter(|(score, _)| *score > Fixed::ZERO)
        .max_by(|a, b| a.0.cmp(&b.0).then(b.1.cmp(&a.1)))
        .map(|(_, what)| what)
}

/// How many cities to aim for before stopping expansion.
///
/// A flat number is a crude opinion — a real one would weigh happiness, land
/// and neighbours — but expansion has to stop somewhere, and an empire that
/// never stops is worse than one that stops too early.
const TARGET_CITIES: usize = 5;

/// Whether the civ should be making a settler.
///
/// Only if it is below its target *and* somewhere legal exists to put one:
/// building a settler with nowhere to go wastes 106 production and two
/// citizens.
pub fn wants_settler(world: &World, r: &Ruleset, civ: CivId) -> bool {
    let Some(c) = world.civ(civ) else { return false };
    if c.cities.len() >= TARGET_CITIES {
        return false;
    }
    // One in flight at a time.
    let settlers = c
        .units
        .iter()
        .filter_map(|u| world.units.get(*u))
        .filter(|u| crate::world::settling::is_settler(r, u.base))
        .count();
    if settlers > 0 {
        return false;
    }
    c.cities
        .iter()
        .filter_map(|id| world.cities.get(*id))
        .any(|city| crate::world::settling::best_site(world, r, civ, city.centre, 6).is_some())
}

/// What a settler should do this turn: walk toward the best site, or found.
pub fn settler_order(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    unit: Id<crate::world::entities::Unit>,
) -> Option<UnitOrder> {
    let u = world.units.get(unit)?;
    if !crate::world::settling::is_settler(r, u.base) {
        return None;
    }
    // Standing somewhere legal? Then stop walking.
    if crate::world::settling::can_settle(world, r, civ, u.position).is_ok() {
        return Some(UnitOrder::FoundCity);
    }
    let (target, _) = crate::world::settling::best_site(world, r, civ, u.position, 8)?;
    // One step of the way, as far as this turn's movement allows.
    let reachable = crate::world::movement::reachable(&world.grid, r, u.position, u.movement_left, &[]);
    let step = reachable
        .keys()
        .copied()
        .filter(|t| *t != u.position)
        .min_by_key(|t| (world.grid.distance(*t, target), t.0))?;
    Some(UnitOrder::MoveTo(step))
}

/// What a unit is told to do. Deliberately small: the turn loop applies these,
/// so anything here has to be expressible as a single reversible step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitOrder {
    MoveTo(crate::world::grid::TileId),
    FoundCity,
}

/// Which policy to adopt, if one is affordable.
///
/// Cheapest adoptable, which in practice means opening branches before
/// deepening them — a reasonable default, and the place a real opinion would go.
pub fn choose_policy(
    world: &World,
    r: &Ruleset,
    civ: CivId,
    m: PolicyModifiers,
) -> Option<Id<Policy>> {
    let c = world.civ(civ)?;
    let cost = policy_cost(c.adopted_policies.len(), c.cities.len(), m);
    if c.culture_stored < cost {
        return None;
    }
    r.policies
        .ids()
        .filter(|p| is_adoptable(world, r, civ, *p).is_ok())
        .min()
}

/// What a city should work toward, given what its owner lacks.
///
/// Food while small, production once established — the same shape as the
/// citizen-assignment focus, so the two do not pull against each other.
pub fn choose_focus(city: &City) -> crate::world::assignment::CityFocus {
    use crate::world::assignment::CityFocus;
    if city.population < 5 {
        CityFocus::None // the default already favours growth below size 5
    } else {
        CityFocus::ProductionGrowth
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::entities::Civilization;
    use crate::world::grid::TileGrid;

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings")
            .expect("readable")
            .expect("loads")
    }

    fn world(r: &Ruleset) -> World {
        let mut g = TileGrid::new(16, 16, r.terrain("Grassland").unwrap());
        g.owner.iter_mut().for_each(|o| *o = Some(CivId(0)));
        let nation = r.nations.ids().next().unwrap();
        World::new(g, vec![Civilization::new(CivId(0), nation, false)])
    }

    #[test]
    fn research_starts_at_the_cheapest_available_tech() {
        let r = ruleset();
        let mut w = world(&r);
        let m = ScienceModifiers::default();

        let first = choose_research(&w, &r, CivId(0), m).expect("something is researchable");
        println!("first choice: {}", r.technologies.name_of(first).unwrap());
        // Only Agriculture has no prerequisites at the start.
        assert_eq!(r.technologies.name_of(first), Some("Agriculture"));

        w.civ_mut(CivId(0)).unwrap().techs.insert(first);
        let second = choose_research(&w, &r, CivId(0), m).expect("the tree widened");
        assert_ne!(second, first, "it does not pick what it already knows");
    }

    #[test]
    fn a_city_builds_something_worth_building() {
        let r = ruleset();
        let w = world(&r);
        let city = City::found("Rome", CivId(0), w.grid.id_at(8, 8).unwrap());
        let choice = choose_construction(&w, &r, CivId(0), &city, CostModifiers::default(), 10, false)
            .expect("a new city can build something");
        match choice {
            Construction::Building(id) => println!("chose {}", r.buildings[id].name),
            Construction::Unit(id) => panic!("chose a unit: {}", r.units[id].name),
        }
        assert!(crate::world::construction::can_construct(&r, w.civ(CivId(0)).unwrap(), &city, choice).is_ok());
    }

    #[test]
    fn nothing_is_built_that_cannot_be_kept() {
        let r = ruleset();
        let w = world(&r);
        let city = City::found("Rome", CivId(0), w.grid.id_at(8, 8).unwrap());
        let m = CostModifiers::default();

        // With no income, only free-to-keep buildings are considered.
        let broke = choose_construction(&w, &r, CivId(0), &city, m, 0, false);
        if let Some(Construction::Building(id)) = broke {
            assert_eq!(r.buildings[id].maintenance.unwrap_or(0), 0, "{}", r.buildings[id].name);
        }

        // With income, upkeep-bearing buildings come into range.
        let rich = choose_construction(&w, &r, CivId(0), &city, m, 20, false);
        assert!(rich.is_some());
    }

    #[test]
    fn choices_are_the_same_every_time() {
        let r = ruleset();
        let w = world(&r);
        let city = City::found("Rome", CivId(0), w.grid.id_at(8, 8).unwrap());
        let m = CostModifiers::default();
        let s = ScienceModifiers::default();

        for _ in 0..5 {
            assert_eq!(
                choose_construction(&w, &r, CivId(0), &city, m, 10, false),
                choose_construction(&w, &r, CivId(0), &city, m, 10, false)
            );
            assert_eq!(
                choose_research(&w, &r, CivId(0), s),
                choose_research(&w, &r, CivId(0), s)
            );
        }
    }

    #[test]
    fn a_policy_is_only_chosen_when_it_can_be_paid_for() {
        let r = ruleset();
        let mut w = world(&r);
        let m = PolicyModifiers::default();
        assert_eq!(choose_policy(&w, &r, CivId(0), m), None, "no culture yet");

        w.civ_mut(CivId(0)).unwrap().culture_stored = 1_000;
        let choice = choose_policy(&w, &r, CivId(0), m).expect("something is adoptable");
        assert!(is_adoptable(&w, &r, CivId(0), choice).is_ok());
        println!("policy: {}", r.policies.name_of(choice).unwrap());
    }

    #[test]
    fn planning_never_touches_the_world() {
        let r = ruleset();
        let w = world(&r);
        let before = w.clone();
        let city = City::found("Rome", CivId(0), w.grid.id_at(8, 8).unwrap());
        choose_research(&w, &r, CivId(0), ScienceModifiers::default());
        choose_construction(&w, &r, CivId(0), &city, CostModifiers::default(), 10, false);
        choose_policy(&w, &r, CivId(0), PolicyModifiers::default());
        assert_eq!(w, before, "every decision is a pure read");
    }
}
