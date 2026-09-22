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
use crate::world::entities::Civilization;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResearchRejection {
    AlreadyKnown,
    /// A prerequisite has not been researched.
    MissingPrerequisite(String),
    Unknown,
}

impl std::fmt::Display for ResearchRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ResearchRejection::AlreadyKnown => write!(f, "already researched"),
            ResearchRejection::MissingPrerequisite(tech) => write!(f, "needs {tech} first"),
            ResearchRejection::Unknown => write!(f, "not in this ruleset"),
        }
    }
}

impl std::error::Error for ResearchRejection {}

pub fn can_research(r: &Ruleset, civ: &Civilization, tech: Id<Technology>) -> Result<(), ResearchRejection> {
    let t = r.technologies.get(tech).ok_or(ResearchRejection::Unknown)?;
    if civ.has_tech(tech) {
        return Err(ResearchRejection::AlreadyKnown);
    }
    for prereq in &t.prerequisites {
        let known = r.technologies.id(prereq).is_some_and(|p| civ.has_tech(p));
        if !known {
            return Err(ResearchRejection::MissingPrerequisite(prereq.clone()));
        }
    }
    Ok(())
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
    /// Nothing selected; the science is banked toward whatever comes next.
    Idle {
        stored: i32,
    },
}

/// Bank a turn's science and finish the current research if it is paid for.
pub fn apply_science(civ: &mut Civilization, r: &Ruleset, science: i32, m: ScienceModifiers) -> ResearchEvent {
    civ.science_stored += science.max(0);

    let Some(tech) = civ.researching else {
        return ResearchEvent::Idle { stored: civ.science_stored };
    };
    let Some(cost) = tech_cost(r, tech, civ.cities.len(), m) else {
        return ResearchEvent::Idle { stored: civ.science_stored };
    };
    if civ.science_stored < cost {
        return ResearchEvent::Progressed { stored: civ.science_stored };
    }

    civ.science_stored -= cost;
    civ.techs.insert(tech);
    civ.researching = None;
    ResearchEvent::Completed { tech, carried_over: civ.science_stored }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::grid::CivId;

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings").expect("readable").expect("loads")
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
    fn science_with_nothing_selected_is_banked() {
        let r = ruleset();
        let mut c = civ(&r);
        assert_eq!(apply_science(&mut c, &r, 12, ScienceModifiers::default()), ResearchEvent::Idle { stored: 12 });
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
