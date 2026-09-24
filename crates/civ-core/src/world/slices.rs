//! Blitz slices (docs/CONTENT.md, Spine): a game that plays only some of the
//! eras. It starts in the slice's first era, as Unciv starts a game in a
//! later era (every earlier tech known, that era's starting units, gold and
//! culture, and new cities founded with its population and buildings); no
//! tech past its last era can be researched; victories that need something
//! past it are not offered; and it ends after the slice's turns, when The
//! Chronicle decides if nobody has won. The slice is chosen at setup
//! (`civ-sim` `Settings.slice`); without one nothing here applies.

use crate::arena::Id;
use crate::ruleset::civgame::{SliceDef, VictoryKnobs};
use crate::ruleset::{Building, Ruleset, Technology, Victory};
use crate::stats::Fixed;
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

/// The definition of the slice a world is playing, if any.
pub fn definition<'r>(world: &World, r: &'r Ruleset) -> Option<&'r SliceDef> {
    let slice = world.slice.as_ref()?;
    r.slices.iter().find(|d| d.name == slice.name)
}

/// The influence and trade knobs in force: a slice's own while one plays,
/// else the ruleset's.
pub fn victory_knobs<'r>(world: &World, r: &'r Ruleset) -> &'r VictoryKnobs {
    definition(world, r).map_or(&r.victory_knobs, |d| &d.victory_knobs)
}

/// The science cost modifier in force: the speed's, scaled by the slice's
/// `scienceCostPercent` while one plays.
pub fn science_cost(world: &World, r: &Ruleset, speed: Fixed) -> Fixed {
    match definition(world, r) {
        Some(d) if d.science_cost_percent != 100 => speed * Fixed::from_fraction(d.science_cost_percent, 100),
        _ => speed,
    }
}

fn tech_era(r: &Ruleset, tech: &str) -> Option<i32> {
    r.technologies.by_name(tech)?.era(r).and_then(|e| r.eras.get(e)).map(|e| e.era_number)
}

fn building_era(r: &Ruleset, building: &Building) -> i32 {
    building.required_tech.as_deref().and_then(|t| tech_era(r, t)).unwrap_or(0)
}

/// The techs that open a slice's first era, which its start grants besides
/// the earlier eras' techs: those in the era's first tech column (civ.game
/// puts only the namesake tech there, and discovering it is what moves a
/// nation into the era). A slice starting in the ruleset's first era needs none.
pub fn opening_techs(r: &Ruleset, slice: &Slice) -> Vec<Id<Technology>> {
    let first = r.eras.values().map(|e| e.era_number).min();
    if first.is_none_or(|f| slice.first_era <= f) {
        return Vec::new();
    }
    let Some(era) = r.eras.values().find(|e| e.era_number == slice.first_era) else { return Vec::new() };
    let Some(column) = r
        .tech_columns
        .iter()
        .filter(|(_, c)| c.era == era.name)
        .min_by_key(|(_, c)| c.column_number)
        .map(|(id, _)| id)
    else {
        return Vec::new();
    };
    r.technologies.iter().filter(|(_, t)| t.column == Some(column)).map(|(id, _)| id).collect()
}

/// The earliest era by which a victory's requirements can all be met:
/// its buildings, its spaceship parts, whatever opens the diplomatic vote,
/// and for a trade-share victory the era trade starts to count from
/// (`tradeShareFromEra`). Victories that need nothing era-bound (conquest,
/// influence, score) answer 0.
pub fn victory_era(r: &Ruleset, victory: &Victory) -> i32 {
    victory_era_with(r, &r.victory_knobs, victory)
}

fn victory_era_with(r: &Ruleset, knobs: &VictoryKnobs, victory: &Victory) -> i32 {
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
            Milestone::WorldTradeShare { .. } => knobs
                .trade_share_from_era
                .as_deref()
                .and_then(|e| r.eras.by_name(e))
                .map(|e| e.era_number),
            _ => None,
        };
        era = era.max(needed.unwrap_or(0));
    }
    era
}

/// The victories a slice can be won by, under its own knobs.
pub fn victories_within(r: &Ruleset, slice: &Slice) -> Vec<Id<Victory>> {
    let knobs = r.slices.iter().find(|d| d.name == slice.name).map_or(&r.victory_knobs, |d| &d.victory_knobs);
    r.victories.iter().filter(|(_, v)| victory_era_with(r, knobs, v) <= slice.last_era).map(|(id, _)| id).collect()
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
