//! Blitz slices (docs/CONTENT.md, Spine): a game that plays only some of the
//! eras. It starts in the slice's first era, as Unciv starts a game in a
//! later era (every earlier tech known, that era's starting units, gold and
//! culture, and new cities founded with its population and buildings); no
//! tech past its last era can be researched; victories that need something
//! past it are not offered; and it ends after the slice's turns, when The
//! Chronicle decides if nobody has won. The slice is chosen at setup
//! (`civ-sim` `Settings.slice`); without one nothing here applies.

use crate::arena::Id;
use crate::ruleset::{Building, Ruleset, Victory};
use crate::world::victory::Milestone;
use crate::world::world::World;

/// The slice a game is playing.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slice {
    pub name: String,
    /// Era numbers: where it starts and the last it may reach.
    pub first_era: i32,
    pub last_era: i32,
}

/// Resolve a slice of the ruleset by name.
pub fn resolve(r: &Ruleset, name: &str) -> Result<Slice, String> {
    let def = r.slices.iter().find(|s| s.name == name).ok_or_else(|| format!("unknown slice: {name}"))?;
    let eras: Vec<i32> = def
        .eras
        .iter()
        .map(|e| r.eras.by_name(e).map(|e| e.era_number).ok_or_else(|| format!("slice {name}: unknown era {e}")))
        .collect::<Result<_, _>>()?;
    let (Some(first), Some(last)) = (eras.first(), eras.last()) else {
        return Err(format!("slice {name} has no eras"));
    };
    Ok(Slice { name: name.to_string(), first_era: *first, last_era: *last })
}

fn tech_era(r: &Ruleset, tech: &str) -> Option<i32> {
    r.technologies.by_name(tech)?.era(r).and_then(|e| r.eras.get(e)).map(|e| e.era_number)
}

fn building_era(r: &Ruleset, building: &Building) -> i32 {
    building.required_tech.as_deref().and_then(|t| tech_era(r, t)).unwrap_or(0)
}

/// The earliest era by which a victory's requirements can all be met:
/// its buildings, its spaceship parts and whatever opens the diplomatic
/// vote. Victories that need nothing era-bound (conquest, influence, trade,
/// score) answer 0.
pub fn victory_era(r: &Ruleset, victory: &Victory) -> i32 {
    let mut era = 0;
    for text in &victory.milestones {
        let needed = match crate::world::victory::parse_milestone(text) {
            Milestone::BuiltBuilding(name) | Milestone::BuildingBuiltGlobally(name) => {
                r.buildings.by_name(&name).map(|b| building_era(r, b))
            }
            Milestone::SpaceshipParts => victory
                .required_spaceship_parts
                .iter()
                .filter_map(|p| r.units.by_name(p))
                .map(|u| u.required_tech.as_deref().and_then(|t| tech_era(r, t)).unwrap_or(0))
                .max(),
            Milestone::DiplomaticVote => r
                .buildings
                .values()
                .filter(|b| b.uniques.iter().any(|u| u.unique_type() == Some(crate::UniqueType::OneTimeTriggerVoting)))
                .map(|b| building_era(r, b))
                .min(),
            _ => None,
        };
        era = era.max(needed.unwrap_or(0));
    }
    era
}

/// The victories a slice can be won by.
pub fn victories_within(r: &Ruleset, slice: &Slice) -> Vec<Id<Victory>> {
    r.victories.iter().filter(|(_, v)| victory_era(r, v) <= slice.last_era).map(|(id, _)| id).collect()
}

/// A wonder the slice's starting era lists as already gone
/// (`startingObsoleteWonders`), which nobody can build.
pub fn is_gone(world: &World, r: &Ruleset, building: Id<Building>) -> bool {
    let Some(slice) = &world.slice else { return false };
    let Some(era) = r.eras.values().find(|e| e.era_number == slice.first_era) else { return false };
    r.buildings.name_of(building).is_some_and(|name| era.starting_obsolete_wonders.iter().any(|w| w == name))
}

/// A city founded in a slice starts as the first era says
/// (`settlerPopulation`, `settlerBuildings`), as in Unciv's CityFounder.
pub fn on_city_founded(world: &mut World, r: &Ruleset, city: Id<crate::world::city::City>) {
    let Some(slice) = &world.slice else { return };
    let Some(era) = r.eras.values().find(|e| e.era_number == slice.first_era) else { return };
    let population = era.settler_population.max(1) as u32;
    let buildings: Vec<_> = era.settler_buildings.iter().filter_map(|b| r.buildings.id(b)).collect();
    let Some(c) = world.cities.get_mut(city) else { return };
    if population > c.population {
        c.population = population;
    }
    for b in buildings {
        c.built.insert(b);
        c.free_buildings.insert(b);
    }
    crate::world::city::rebuild_city_uniques(r, c);
}

/// The date shown on `turn` of a slice with calendar years.
pub fn year(r: &Ruleset, slice: &Slice, turn: u32) -> Option<i32> {
    let def = r.slices.iter().find(|s| s.name == slice.name)?;
    let (start, end) = (def.start_year?, def.end_year?);
    let span = i64::from(def.turns.max(1));
    Some(start + ((i64::from(end - start) * i64::from(turn.min(def.turns))) / span) as i32)
}
