//! What a city builds.
//!
//! The production half of the loop, and the first system gated by the tech
//! tree — a city cannot build a Library before Writing, which is the whole
//! reason research matters.
//!
//! Costs are not in the json for most buildings. Only 61 of 224 state one; the
//! rest inherit their tech column's `buildingCost`, or `wonderCost` if they are
//! a wonder. `Ruleset::derive` fills that in, following Unciv
//! (`Ruleset.kt:618`), whose comment explains the intent: editing a column's
//! costs should move every building in it, so a mod adding a building inherits
//! the original values rather than restating them.

use crate::arena::Id;
use crate::ruleset::{BaseUnit, Building, Ruleset};
use crate::stats::Fixed;
use crate::world::city::City;
use crate::world::entities::Civilization;
use crate::UniqueType;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Construction {
    Building(Id<Building>),
    Unit(Id<BaseUnit>),
}

/// Why a city cannot build something.
///
/// Named rather than boolean, because "you cannot build this" is a question
/// the interface has to answer in words, and because the AI picks differently
/// depending on whether a thing is impossible or merely not yet unlocked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    /// The unlocking tech has not been researched.
    NotUnlocked(String),
    /// This city already has it.
    AlreadyBuilt,
    /// A prerequisite building is missing from this city.
    MissingBuilding(String),
    /// Restricted to another civilization.
    UniqueToAnother(String),
    /// Carries no cost and cannot be produced — the Palace, or a
    /// faith-purchased religious building.
    NotProducible,
    /// The city is too small — a Settler costs two citizens, so a size-1 city
    /// cannot make one without erasing itself.
    NeedsPopulation(u32),
    /// The id does not resolve in this ruleset.
    Unknown,
}

impl std::fmt::Display for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Rejection::NotUnlocked(tech) => write!(f, "needs {tech}"),
            Rejection::AlreadyBuilt => write!(f, "already built in this city"),
            Rejection::MissingBuilding(b) => write!(f, "needs a {b} in this city"),
            Rejection::UniqueToAnother(civ) => write!(f, "unique to {civ}"),
            Rejection::NotProducible => write!(f, "cannot be produced"),
            Rejection::NeedsPopulation(n) => write!(f, "needs a city of size {n}"),
            Rejection::Unknown => write!(f, "not in this ruleset"),
        }
    }
}

impl std::error::Error for Rejection {}

/// Scaling applied on top of a base cost.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CostModifiers {
    pub speed: Fixed,
    /// Difficulty, and ×1.5 for city-states.
    pub civ: Fixed,
    /// How many cities the owner has. National wonders carry
    /// `Cost increases by [n] per owned city`, so a wide empire pays more for
    /// them — the mechanism Civ uses to keep them from being free wins for
    /// whoever expanded most.
    pub cities: usize,
    /// How many turns a game at this speed runs for. Unit upkeep needs it:
    /// Civ makes soldiers dearer as a fraction of the whole game elapsed.
    pub game_length: i32,
    /// How much influence a gift of gold to a city-state buys, scaled by
    /// speed — a quick game gives more per coin, so diplomacy still resolves
    /// inside it.
    pub gold_gift_modifier: Fixed,
    /// Speed's `goldCostModifier`, for anything bought with gold.
    pub gold_cost: Fixed,
    /// Speed's `faithCostModifier`, for anything bought with faith.
    pub faith_cost: Fixed,
    /// Base religious pressure one city puts on its neighbours each turn.
    /// Nine on Quick, six on Standard, two on Marathon — a shorter game
    /// converts faster so religion still resolves inside it.
    pub religious_pressure: i32,
}

// `game_length` and `religious_pressure` are not costs. They are here because
// they come from the same place as `speed` — the game speed object — and the
// turn loop needs all three together. The alternative was a ninth and tenth
// argument on every turn function, which buys tidiness in the type name and
// pays for it at every call site.

impl Default for CostModifiers {
    fn default() -> Self {
        CostModifiers {
            speed: Fixed::ONE,
            civ: Fixed::ONE,
            cities: 1,
            // Standard speed, so a default-constructed game ages and converts
            // at the rate most of the ruleset was balanced for.
            game_length: 500,
            religious_pressure: 6,
            gold_gift_modifier: Fixed::ONE,
            gold_cost: Fixed::ONE,
            faith_cost: Fixed::ONE,
        }
    }
}

/// Production needed to finish something.
///
/// Truncated, not rounded — Unciv's `getProductionCost` ends in `.toInt()`,
/// and Kotlin truncates toward zero.
pub fn production_cost(r: &Ruleset, what: Construction, m: CostModifiers) -> Option<i32> {
    let (base, uniques) = match what {
        Construction::Building(id) => {
            let b = r.buildings.get(id)?;
            (b.cost?, &b.uniques)
        }
        Construction::Unit(id) => {
            let u = r.units.get(id)?;
            (u.cost?, &u.uniques)
        }
    };
    if base <= 0 {
        return None;
    }

    // Flat additions land before any scaling, as they do in Unciv's
    // `getProductionCost`.
    let per_city: i32 = uniques
        .iter()
        .filter(|u| u.unique_type() == Some(UniqueType::CostIncreasesPerCity))
        .filter_map(|u| u.params.first())
        .filter_map(|p| p.parse::<i32>().ok())
        .sum();
    let base = base + per_city * m.cities as i32;

    Some(((Fixed::whole(base) * m.speed) * m.civ).floor().max(1) as i32)
}

/// Whether this city may start on something now.
pub fn can_construct(r: &Ruleset, civ: &Civilization, city: &City, what: Construction) -> Result<(), Rejection> {
    let (required_tech, unique_to) = match what {
        Construction::Building(id) => {
            let b = r.buildings.get(id).ok_or(Rejection::Unknown)?;
            if city.built.contains(&id) {
                return Err(Rejection::AlreadyBuilt);
            }
            if b.cost.unwrap_or(0) <= 0 {
                return Err(Rejection::NotProducible);
            }
            if let Some(req) = b.required_building.as_deref() {
                let have = r.buildings.id(req).is_some_and(|rid| city.built.contains(&rid));
                if !have {
                    return Err(Rejection::MissingBuilding(req.to_string()));
                }
            }
            (b.required_tech.clone(), b.unique_to.clone())
        }
        Construction::Unit(id) => {
            let u = r.units.get(id).ok_or(Rejection::Unknown)?;
            if u.cost.unwrap_or(0) <= 0 {
                return Err(Rejection::NotProducible);
            }
            for unique in u.uniques.iter().filter(|x| x.unique_type() == Some(UniqueType::RequiresPopulation)) {
                let needed = unique.params.first().and_then(|p| p.parse::<u32>().ok()).unwrap_or(0);
                if city.population < needed {
                    return Err(Rejection::NeedsPopulation(needed));
                }
            }
            (u.required_tech.clone(), u.unique_to.clone())
        }
    };

    if let Some(tech) = required_tech {
        let known = r.technologies.id(&tech).is_some_and(|t| civ.has_tech(t));
        if !known {
            return Err(Rejection::NotUnlocked(tech));
        }
    }
    if let Some(nation) = unique_to {
        let theirs = r.nations.name_of(civ.nation) == Some(nation.as_str());
        if !theirs {
            return Err(Rejection::UniqueToAnother(nation));
        }
    }
    Ok(())
}

/// Everything this city could start on right now.
pub fn available(r: &Ruleset, civ: &Civilization, city: &City) -> Vec<Construction> {
    let mut out: Vec<Construction> = r
        .buildings
        .ids()
        .map(Construction::Building)
        .chain(r.units.ids().map(Construction::Unit))
        .filter(|c| can_construct(r, civ, city, *c).is_ok())
        .collect();
    out.sort();
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConstructionEvent {
    Progressed {
        stored: i32,
    },
    Completed {
        what: Construction,
        carried_over: i32,
    },
    /// Nothing queued; the production is banked anyway.
    Idle {
        stored: i32,
    },
}

/// Bank a turn's production and finish the head of the queue if it is paid for.
pub fn apply_production(city: &mut City, r: &Ruleset, production: i32, m: CostModifiers) -> ConstructionEvent {
    city.production_stored += production.max(0);

    let Some(&head) = city.queue.first() else {
        return ConstructionEvent::Idle { stored: city.production_stored };
    };
    let Some(cost) = production_cost(r, head, m) else {
        return ConstructionEvent::Idle { stored: city.production_stored };
    };
    if city.production_stored < cost {
        return ConstructionEvent::Progressed { stored: city.production_stored };
    }

    city.production_stored -= cost;
    city.queue.remove(0);
    if let Construction::Building(id) = head {
        city.built.insert(id);
        // Without this the building stands and does nothing.
        crate::world::city::rebuild_city_uniques(r, city);
    }
    ConstructionEvent::Completed { what: head, carried_over: city.production_stored }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::grid::{CivId, TileId};

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings").expect("readable").expect("loads")
    }

    fn civ(r: &Ruleset) -> Civilization {
        Civilization::new(CivId(0), r.nations.ids().next().unwrap(), false)
    }

    #[test]
    fn a_building_is_gated_on_its_tech() {
        let r = ruleset();
        let mut c = civ(&r);
        let city = City::found("Rome", CivId(0), TileId(0));
        let library = Construction::Building(r.building("Library").unwrap());

        assert_eq!(can_construct(&r, &c, &city, library), Err(Rejection::NotUnlocked("Writing".into())));

        c.techs.insert(r.technologies.id("Writing").unwrap());
        assert_eq!(can_construct(&r, &c, &city, library), Ok(()));
    }

    #[test]
    fn what_cannot_be_produced_is_rejected_as_such() {
        let r = ruleset();
        let c = civ(&r);
        let city = City::found("Rome", CivId(0), TileId(0));
        // The Palace is free in the capital and carries no cost.
        let palace = Construction::Building(r.building("Palace").unwrap());
        assert_eq!(can_construct(&r, &c, &city, palace), Err(Rejection::NotProducible));
    }

    #[test]
    fn a_building_already_here_is_not_offered_again() {
        let r = ruleset();
        let mut c = civ(&r);
        c.techs.insert(r.technologies.id("Writing").unwrap());
        let mut city = City::found("Rome", CivId(0), TileId(0));
        let id = r.building("Library").unwrap();
        city.built.insert(id);
        assert_eq!(can_construct(&r, &c, &city, Construction::Building(id)), Err(Rejection::AlreadyBuilt));
    }

    #[test]
    fn unique_units_belong_to_their_nation() {
        let r = ruleset();
        let c = civ(&r);
        let city = City::found("Rome", CivId(0), TileId(0));
        let owner = r.nations.name_of(c.nation).unwrap().to_string();

        // Rejections are reported in check order, and tech comes first — so
        // give this civ every tech, leaving the nation restriction as the only
        // thing that can refuse.
        let mut c = c;
        for t in r.technologies.ids() {
            c.techs.insert(t);
        }

        let foreign = r
            .units
            .iter()
            .find(|(_, u)| u.unique_to.as_ref().is_some_and(|n| *n != owner))
            .map(|(id, _)| id)
            .expect("the ruleset has unique units");
        assert!(matches!(
            can_construct(&r, &c, &city, Construction::Unit(foreign)),
            Err(Rejection::UniqueToAnother(_))
        ));
    }

    #[test]
    fn costs_scale_and_truncate() {
        let r = ruleset();
        let library = Construction::Building(r.building("Library").unwrap());
        let base = production_cost(&r, library, CostModifiers::default()).unwrap();

        let slow =
            production_cost(&r, library, CostModifiers { speed: Fixed::whole(3), ..Default::default() }).unwrap();
        assert_eq!(slow, base * 3);

        // 1.5× truncates rather than rounds, as everywhere else.
        let city_state =
            production_cost(&r, library, CostModifiers { civ: Fixed::permille(1_500), ..Default::default() }).unwrap();
        assert_eq!(city_state, (base * 3) / 2);
    }

    #[test]
    fn production_accumulates_until_the_thing_is_paid_for() {
        let r = ruleset();
        let m = CostModifiers::default();
        let library = Construction::Building(r.building("Library").unwrap());
        let cost = production_cost(&r, library, m).unwrap();

        let mut city = City::found("Rome", CivId(0), TileId(0));
        city.queue.push(library);

        assert_eq!(apply_production(&mut city, &r, cost - 1, m), ConstructionEvent::Progressed { stored: cost - 1 });
        assert!(city.built.is_empty());

        assert_eq!(
            apply_production(&mut city, &r, 5, m),
            ConstructionEvent::Completed { what: library, carried_over: 4 }
        );
        assert!(city.queue.is_empty());
        assert!(city.built.contains(&r.building("Library").unwrap()));
        assert_eq!(city.production_stored, 4, "the excess carries over");
    }

    #[test]
    fn an_empty_queue_still_banks_production() {
        let r = ruleset();
        let mut city = City::found("Rome", CivId(0), TileId(0));
        assert_eq!(apply_production(&mut city, &r, 7, CostModifiers::default()), ConstructionEvent::Idle { stored: 7 });
    }

    #[test]
    fn a_fresh_civ_can_build_something_and_not_everything() {
        let r = ruleset();
        let c = civ(&r);
        let city = City::found("Rome", CivId(0), TileId(0));
        let options = available(&r, &c, &city);
        println!("with no techs: {} options", options.len());
        assert!(!options.is_empty(), "a new city must be able to build something");
        assert!(options.len() < r.buildings.len() + r.units.len(), "but not everything");
    }
}
