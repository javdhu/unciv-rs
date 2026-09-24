//! Science and the tech tree.
//!
//! Techs gate almost everything else in the ruleset — 213 of 224 buildings and
//! 207 of 227 units name a `requiredTech` — so this is the spine the rest of
//! the game hangs from.
//!
//! The cost formula is Unciv's (`TechManager.kt:108`). Two of its inputs live
//! in Kotlin rather than json and are reproduced here as [`MapScaling`]:
//! larger maps make each tech dearer, but *reduce* the penalty per city, since
//! a bigger map means more of them. Everything ends in a truncation, as
//! everywhere else Kotlin calls `.toInt()`.

use crate::arena::Id;
use crate::ruleset::{Ruleset, Technology};
use crate::stats::Fixed;
use crate::world::conditionals::{applies, GameContext, Verdict};
use crate::world::entities::Civilization;
use crate::world::grid::CivId;
use crate::world::world::World;
use crate::UniqueType;

/// Cost scaling that depends on map size (`MapSize.kt:48`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapScaling {
    pub tech_cost_multiplier: Fixed,
    /// Added per city beyond the first. Smaller on larger maps.
    pub tech_cost_per_city: Fixed,
    /// The same idea for policies, and twice the size.
    pub policy_cost_per_city: Fixed,
}

impl MapScaling {
    pub const TINY: MapScaling = MapScaling::of(1_000, 50, 100);
    pub const SMALL: MapScaling = MapScaling::of(1_000, 50, 100);
    pub const MEDIUM: MapScaling = MapScaling::of(1_100, 50, 100);
    pub const LARGE: MapScaling = MapScaling::of(1_200, 37, 75);
    pub const HUGE: MapScaling = MapScaling::of(1_300, 25, 50);

    const fn of(multiplier: i64, per_city: i64, policy_per_city: i64) -> MapScaling {
        MapScaling {
            tech_cost_multiplier: Fixed::permille(multiplier),
            tech_cost_per_city: Fixed::permille(per_city),
            policy_cost_per_city: Fixed::permille(policy_per_city),
        }
    }
}

impl Default for MapScaling {
    fn default() -> Self {
        MapScaling::MEDIUM
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScienceModifiers {
    /// Difficulty's `researchCostModifier`.
    pub difficulty: Fixed,
    /// Speed's `scienceCostModifier`.
    pub speed: Fixed,
    pub map: MapScaling,
}

impl Default for ScienceModifiers {
    fn default() -> Self {
        ScienceModifiers { difficulty: Fixed::ONE, speed: Fixed::ONE, map: MapScaling::default() }
    }
}

/// Science needed to finish a tech.
///
/// `cities` counts the researching civ's cities; each one beyond the first
/// raises the price.
pub fn tech_cost(r: &Ruleset, tech: Id<Technology>, cities: usize, m: ScienceModifiers) -> Option<i32> {
    let base = r.technologies.get(tech)?.cost?;
    if base <= 0 {
        return None;
    }
    let city_modifier = Fixed::whole(cities.saturating_sub(1) as i32) * m.map.tech_cost_per_city;
    let cost =
        (((Fixed::whole(base) * m.difficulty) * m.speed) * m.map.tech_cost_multiplier) * (Fixed::ONE + city_modifier);
    Some(cost.floor().max(1) as i32)
}

/// Live price: puppet cities do not increase science cost, known major
/// discoveries grant diffusion, and civilization-wide cost uniques apply.
pub fn tech_cost_in(world: &World, r: &Ruleset, civ: CivId, tech: Id<Technology>, m: ScienceModifiers) -> Option<i32> {
    let c = world.civ(civ)?;
    let base = r.technologies.get(tech)?.cost?;
    if base <= 0 {
        return None;
    }
    let known = world
        .civs
        .iter()
        .filter(|other| {
            other.is_major() && other.id != civ && world.diplomacy.have_met(civ, other.id) && other.has_tech(tech)
        })
        .count() as i64;
    let remaining = world.civs.iter().filter(|other| other.is_major() && other.is_alive()).count().max(1) as i64;
    let difficulty = if c.is_ai { Fixed::ONE } else { m.difficulty };
    let mut cost = Fixed::whole(base) * difficulty * m.speed;
    cost = cost * m.map.tech_cost_multiplier;
    let cities = c.cities.iter().filter_map(|id| world.cities.get(*id)).filter(|city| !city.puppet).count();
    let mut city_modifier = Fixed::whole(cities.saturating_sub(1) as i32) * m.map.tech_cost_per_city;
    let ctx = GameContext::new(r).with_world(world).for_civ(civ);
    for u in c.uniques.matching(UniqueType::LessTechCostFromCities).filter(|u| applies(u, &ctx) == Verdict::True) {
        if let Some(n) = u.params.first().and_then(|n| n.parse::<i32>().ok()) {
            city_modifier = city_modifier * (Fixed::ONE - Fixed::from_fraction(i64::from(n), 100));
        }
    }
    for u in c.uniques.matching(UniqueType::LessTechCost).filter(|u| applies(u, &ctx) == Verdict::True) {
        if let Some(n) = u.params.first().and_then(|n| n.parse::<i32>().ok()) {
            cost = cost * (Fixed::ONE + Fixed::from_fraction(i64::from(n), 100));
        }
    }
    cost = cost * (Fixed::ONE + city_modifier);
    // Keep diffusion rational until the final truncation: rounding 1/1.1
    // to a thousandth before multiplying can turn an exact cost of 40 into 39.
    Some(
        (i128::from(cost.raw()) * i128::from(10 * remaining) / (1000 * i128::from(10 * remaining + 3 * known)))
            .clamp(1, i128::from(i32::MAX)) as i32,
    )
}

pub fn invested(civ: &Civilization, tech: Id<Technology>) -> i32 {
    civ.science_invested.get(&tech).copied().unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResearchRejection {
    AlreadyKnown,
    /// A prerequisite has not been researched.
    MissingPrerequisite(String),
    /// Past the last era of the game's Blitz slice.
    BeyondSlice,
    Unknown,
}

impl std::fmt::Display for ResearchRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResearchRejection::AlreadyKnown => write!(f, "already researched"),
            ResearchRejection::MissingPrerequisite(tech) => write!(f, "needs {tech} first"),
            ResearchRejection::BeyondSlice => write!(f, "beyond the last era of this game"),
            ResearchRejection::Unknown => write!(f, "not in this ruleset"),
        }
    }
}

impl std::error::Error for ResearchRejection {}

pub fn can_research(r: &Ruleset, civ: &Civilization, tech: Id<Technology>) -> Result<(), ResearchRejection> {
    let t = r.technologies.get(tech).ok_or(ResearchRejection::Unknown)?;
    if civ.has_tech(tech)
        && !t.uniques.iter().any(|u| u.unique_type() == Some(crate::UniqueType::ResearchableMultipleTimes))
    {
        return Err(ResearchRejection::AlreadyKnown);
    }
    if let Some(cap) = civ.tech_era_cap {
        if t.era(r).and_then(|e| r.eras.get(e)).is_some_and(|e| e.era_number > cap) {
            return Err(ResearchRejection::BeyondSlice);
        }
    }
    for prereq in &t.prerequisites {
        let known = r.technologies.id(prereq).is_some_and(|p| civ.has_tech(p));
        if !known {
            return Err(ResearchRejection::MissingPrerequisite(prereq.clone()));
        }
    }
    Ok(())
}

/// Grant a technology from research or a free reward. Repeated Future Tech
/// completions count toward score, including the first one. Returns false
/// for an unknown technology or an already-known non-repeatable technology.
pub fn grant_technology(civ: &mut Civilization, r: &Ruleset, tech: Id<Technology>) -> bool {
    let Some(t) = r.technologies.get(tech) else { return false };
    let repeatable = t.uniques.iter().any(|u| u.unique_type() == Some(crate::UniqueType::ResearchableMultipleTimes));
    if civ.has_tech(tech) && !repeatable {
        return false;
    }
    civ.techs.insert(tech);
    civ.science_invested.remove(&tech);
    if repeatable {
        civ.victory.repeated_techs = civ.victory.repeated_techs.saturating_add(1);
    }
    if civ.researching == Some(tech) {
        civ.researching = repeatable.then_some(tech);
    }
    true
}

/// Every tech this civ could start on now, in id order.
pub fn available_techs(r: &Ruleset, civ: &Civilization) -> Vec<Id<Technology>> {
    r.technologies.ids().filter(|t| can_research(r, civ, *t).is_ok()).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResearchEvent {
    Progressed {
        stored: i32,
    },
    Completed {
        tech: Id<Technology>,
        carried_over: i32,
    },
    /// Nothing selected. Passive income is lost; existing overflow survives.
    Idle {
        stored: i32,
    },
}

/// A detached civilization has no diplomacy or puppet-city information;
/// live callers use `apply_science_in` instead.
pub fn apply_science(civ: &mut Civilization, r: &Ruleset, science: i32, m: ScienceModifiers) -> ResearchEvent {
    let cost = civ.researching.and_then(|tech| tech_cost(r, tech, civ.cities.len(), m));
    add_science(civ, r, science, science, cost)
}

pub fn apply_science_in(
    world: &mut World,
    r: &Ruleset,
    civ: CivId,
    science: i32,
    m: ScienceModifiers,
) -> ResearchEvent {
    apply_science_with_income_in(world, r, civ, science, science, m)
}

/// Great Scientists add a lump sum, but the overflow limit still uses ordinary
/// turn income, not five times the Scientist's entire grant.
pub fn apply_science_with_income_in(
    world: &mut World,
    r: &Ruleset,
    civ: CivId,
    science: i32,
    income: i32,
    m: ScienceModifiers,
) -> ResearchEvent {
    let cost = world.civ(civ).and_then(|c| c.researching).and_then(|tech| tech_cost_in(world, r, civ, tech, m));
    let Some(c) = world.civ_mut(civ) else { return ResearchEvent::Idle { stored: 0 } };
    add_science(c, r, science, income, cost)
}

/// Selection and new-turn price changes can finish already-paid research
/// immediately, without consuming another turn's income.
pub fn update_research_progress(
    world: &mut World,
    r: &Ruleset,
    civ: CivId,
    income: i32,
    m: ScienceModifiers,
) -> Option<ResearchEvent> {
    let c = world.civ(civ)?;
    let tech = c.researching?;
    let cost = tech_cost_in(world, r, civ, tech, m)?;
    if invested(c, tech).saturating_add(c.science_stored) < cost {
        return None;
    }
    Some(apply_science_with_income_in(world, r, civ, 0, income, m))
}

fn add_science(civ: &mut Civilization, r: &Ruleset, science: i32, income: i32, cost: Option<i32>) -> ResearchEvent {
    let Some((tech, cost)) = civ.researching.zip(cost) else {
        return ResearchEvent::Idle { stored: civ.science_stored };
    };
    let stored = invested(civ, tech).saturating_add(science.max(0)).saturating_add(civ.science_stored.max(0));
    civ.science_stored = 0;
    if stored < cost {
        civ.science_invested.insert(tech, stored);
        return ResearchEvent::Progressed { stored };
    }
    let base = r.technologies.get(tech).and_then(|t| t.cost).unwrap_or(cost);
    civ.science_stored = (stored - cost).min(base.max(income.max(0).saturating_mul(5)));
    grant_technology(civ, r, tech);
    ResearchEvent::Completed { tech, carried_over: civ.science_stored }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::grid::CivId;

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("rules/gods-and-kings").expect("readable").expect("loads")
    }

    fn civ(r: &Ruleset) -> Civilization {
        Civilization::new(CivId(0), r.nations.ids().next().unwrap(), false)
    }

    #[test]
    fn the_tree_opens_one_layer_at_a_time() {
        let r = ruleset();
        let mut c = civ(&r);

        // Agriculture has no prerequisites; it is where everyone starts.
        let start = available_techs(&r, &c);
        println!("initially researchable: {}", start.len());
        assert!(!start.is_empty());
        let agriculture = r.technologies.id("Agriculture").unwrap();
        assert!(start.contains(&agriculture));

        // Pottery needs Agriculture and is not offered before it.
        let pottery = r.technologies.id("Pottery").unwrap();
        assert_eq!(can_research(&r, &c, pottery), Err(ResearchRejection::MissingPrerequisite("Agriculture".into())));

        c.techs.insert(agriculture);
        assert_eq!(can_research(&r, &c, pottery), Ok(()));
        assert_eq!(can_research(&r, &c, agriculture), Err(ResearchRejection::AlreadyKnown));
        assert!(available_techs(&r, &c).len() > start.len(), "the tree widened");
    }

    #[test]
    fn later_techs_cost_more() {
        let r = ruleset();
        let m = ScienceModifiers::default();
        let cost = |n: &str| tech_cost(&r, r.technologies.id(n).unwrap(), 1, m).unwrap();
        let (early, mid, late) = (cost("Agriculture"), cost("Currency"), cost("Computers"));
        println!("Agriculture {early}, Currency {mid}, Computers {late}");
        assert!(early < mid && mid < late);
    }

    #[test]
    fn every_tech_is_priced() {
        let r = ruleset();
        let m = ScienceModifiers::default();
        for t in r.technologies.ids() {
            assert!(
                tech_cost(&r, t, 1, m).is_some_and(|c| c > 0),
                "{} has no cost",
                r.technologies.name_of(t).unwrap()
            );
        }
    }

    /// More cities means dearer techs, and larger maps soften the penalty.
    #[test]
    fn empire_size_raises_the_price_less_on_bigger_maps() {
        let r = ruleset();
        let currency = r.technologies.id("Currency").unwrap();

        let on = |map: MapScaling, cities: usize| {
            tech_cost(&r, currency, cities, ScienceModifiers { map, ..Default::default() }).unwrap()
        };

        let tiny_1 = on(MapScaling::TINY, 1);
        let tiny_9 = on(MapScaling::TINY, 9);
        assert!(tiny_9 > tiny_1, "each city past the first costs");

        // Same eight extra cities, cheaper penalty on a huge map (0.025/city
        // against 0.05), even though the base multiplier is higher.
        let tiny_growth = (tiny_9 - tiny_1) as f64 / tiny_1 as f64;
        let huge_growth = {
            let (a, b) = (on(MapScaling::HUGE, 1), on(MapScaling::HUGE, 9));
            (b - a) as f64 / a as f64
        };
        println!("penalty for 9 cities: tiny {tiny_growth:.3}, huge {huge_growth:.3}");
        assert!(huge_growth < tiny_growth);
    }

    #[test]
    fn science_accumulates_until_the_tech_lands() {
        let r = ruleset();
        let m = ScienceModifiers::default();
        let mut c = civ(&r);
        let agriculture = r.technologies.id("Agriculture").unwrap();
        c.researching = Some(agriculture);
        let cost = tech_cost(&r, agriculture, 0, m).unwrap();

        assert_eq!(apply_science(&mut c, &r, cost - 1, m), ResearchEvent::Progressed { stored: cost - 1 });
        assert!(!c.has_tech(agriculture));

        assert_eq!(apply_science(&mut c, &r, 3, m), ResearchEvent::Completed { tech: agriculture, carried_over: 2 });
        assert!(c.has_tech(agriculture));
        assert_eq!(c.researching, None, "and needs a new selection");
        assert_eq!(c.science_stored, 2, "the excess carries over");
    }

    #[test]
    fn science_with_nothing_selected_is_lost_and_existing_overflow_survives() {
        let r = ruleset();
        let mut c = civ(&r);
        c.science_stored = 3;
        assert_eq!(apply_science(&mut c, &r, 12, ScienceModifiers::default()), ResearchEvent::Idle { stored: 3 });
    }

    /// The whole tree is reachable from a standing start.
    #[test]
    fn every_tech_can_eventually_be_researched() {
        let r = ruleset();
        let mut c = civ(&r);
        let mut rounds = 0;
        while c.techs.len() < r.technologies.len() {
            let next = available_techs(&r, &c);
            assert!(!next.is_empty(), "the tree stalled with {} of {} known", c.techs.len(), r.technologies.len());
            for t in next {
                c.techs.insert(t);
            }
            rounds += 1;
            assert!(rounds < 40, "too many layers");
        }
        println!("the tree is {rounds} layers deep");
        assert_eq!(c.techs.len(), r.technologies.len());
    }
}
