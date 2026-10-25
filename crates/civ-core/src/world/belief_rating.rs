// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! How much an AI wants a belief (`ReligionAutomation.rateBelief`,
//! ReligionAutomation.kt:210-413).
//!
//! A belief is scored by what it would give this civ as it stands: its tiles
//! (worked ones count fully, the city's others 0.7, the rest in reach 0.5),
//! its cities, and the civ as a whole. Pantheons count 0.9. Unciv also
//! jitters each term by a few percent with a seeded RNG; this keeps the
//! scores exact and breaks ties by belief order instead. Uniques this engine
//! does not rate contribute nothing, as in Unciv's `else -> 0f`.

use crate::arena::Id;
use crate::ruleset::{Belief, Ruleset};
use crate::stats::{Fixed, Stat, Stats};
use crate::world::conditionals::{tile_matches, GameContext, Verdict};
use crate::world::grid::CivId;
use crate::world::World;
use crate::UniqueType;

/// City work range, as `CITY_WORK_RADIUS` in the AI.
const WORK_RANGE: u32 = 3;

fn stats_sum(param: Option<&String>) -> Fixed {
    param.and_then(|p| Stats::parse(p)).map_or(Fixed::ZERO, |s| Stat::ALL.into_iter().map(|stat| s[stat]).sum())
}

fn number(param: Option<&String>) -> Fixed {
    param.and_then(|p| p.trim_start_matches('+').parse::<i32>().ok()).map_or(Fixed::ZERO, Fixed::whole)
}

fn fraction(value: Fixed, over: i32) -> Fixed {
    value.checked_div(Fixed::whole(over)).unwrap_or(Fixed::ZERO)
}

/// The civ's score for a belief.
pub fn rate(world: &World, r: &Ruleset, civ: CivId, belief: Id<Belief>) -> Fixed {
    let Some(b) = r.beliefs.get(belief) else { return Fixed::ZERO };
    let Some(c) = world.civ(civ) else { return Fixed::ZERO };
    let ctx = GameContext::new(r).with_world(world).for_civ(civ);
    let mut score = Fixed::ZERO;
    for city_id in &c.cities {
        let Some(city) = world.cities.get(*city_id) else { continue };
        // beliefBonusForTile, weighted by how much the city uses the tile.
        for tile in world.grid.within(city.centre, WORK_RANGE) {
            let weight = if city.worked.contains(&tile) || tile == city.centre {
                Fixed::ONE
            } else if city.tiles.contains(&tile) {
                Fixed::permille(700)
            } else {
                Fixed::permille(500)
            };
            let matches = |filter: Option<&String>| {
                filter.is_some_and(|f| tile_matches(&ctx, tile, f) == Verdict::True)
            };
            for u in &b.uniques {
                let bonus = match u.unique_type() {
                    Some(UniqueType::StatsFromObject) if matches(u.params.get(1)) => stats_sum(u.params.first()),
                    Some(UniqueType::StatsFromTilesWithout)
                        if matches(u.params.get(1)) && !matches(u.params.get(2)) =>
                    {
                        stats_sum(u.params.first())
                    }
                    _ => Fixed::ZERO,
                };
                score += bonus * weight;
            }
        }
        // beliefBonusForCity; each conditional halves the value.
        for u in &b.uniques {
            let p = |i: usize| u.params.get(i);
            let value = match u.unique_type() {
                Some(UniqueType::GrowthPercentBonus) => fraction(number(p(0)), 3),
                Some(UniqueType::BorderGrowthPercentage) => -fraction(number(p(0)), 10),
                Some(UniqueType::StrengthForCities) => fraction(number(p(0)), 10),
                Some(UniqueType::CityHealingUnits) => fraction(number(p(1)), 10),
                Some(
                    UniqueType::PercentProductionBuildings
                    | UniqueType::PercentProductionWonders
                    | UniqueType::PercentProductionUnits,
                ) => fraction(number(p(0)), 3),
                Some(UniqueType::StatsFromCitiesOnSpecificTiles)
                    if p(1).is_some_and(|f| tile_matches(&ctx, city.centre, f) == Verdict::True) =>
                {
                    stats_sum(p(0))
                }
                Some(UniqueType::StatsFromObject) => match p(1).and_then(|f| r.buildings.by_name(f)) {
                    Some(building) if building.is_national_wonder == Some(true) => {
                        stats_sum(p(0)) * Fixed::permille(250)
                    }
                    Some(_) => stats_sum(p(0)),
                    None if p(1).is_some_and(|f| r.specialists.by_name(f).is_some()) => {
                        stats_sum(p(0)) * if city.population > 8 { Fixed::whole(2) } else { Fixed::ONE }
                    }
                    None => Fixed::ZERO,
                },
                Some(UniqueType::StatPercentFromReligionFollowers) => {
                    (number(p(0)) * Fixed::whole(city.population as i32)).min(number(p(2)))
                }
                Some(UniqueType::StatsPerCity) => stats_sum(p(0)),
                _ => Fixed::ZERO,
            };
            let halved = (0..u.conditionals.len()).fold(value, |v, _| v * Fixed::permille(500));
            score += halved;
        }
    }
    // beliefBonusForPlayer. Unciv scales early and late value by how many
    // religions are founded; with no religion yet this is the "early" case.
    let founded = world.civs.iter().filter(|c| c.faith.founded).count();
    let (early, late) = if founded == 0 { (Fixed::ONE, Fixed::whole(2)) } else { (Fixed::whole(2), Fixed::ONE) };
    for u in &b.uniques {
        let p = |i: usize| u.params.get(i);
        score += match u.unique_type() {
            Some(UniqueType::StatsWhenSpreading) => fraction(number(p(0)), 15),
            Some(UniqueType::StatsWhenAdoptingReligion) => fraction(stats_sum(p(0)), 50),
            Some(UniqueType::StatsFromGlobalCitiesFollowingReligion) => stats_sum(p(0)) * Fixed::whole(2),
            Some(UniqueType::StatsFromGlobalFollowers) => {
                fraction(Fixed::whole(10) * stats_sum(p(0)), number(p(1)).floor().max(1) as i32)
            }
            Some(UniqueType::Strength) => number(p(0)) * Fixed::whole(3),
            Some(UniqueType::ReligionSpreadDistance) => (Fixed::whole(10) + number(p(0))) * early,
            Some(UniqueType::NaturalReligionSpreadStrength) => fraction(number(p(0)) * early, 10),
            Some(UniqueType::SpreadReligionStrength) => fraction(number(p(0)) * late, 10),
            Some(UniqueType::FaithCostOfGreatProphetChange) => -fraction(number(p(0)) * late, 10),
            _ => Fixed::ZERO,
        };
    }
    if b.r#type == "Pantheon" {
        score = score * Fixed::permille(900);
    }
    score
}

/// The best of these, ties to the earliest.
pub fn best(world: &World, r: &Ruleset, civ: CivId, choices: &[Id<Belief>]) -> Option<Id<Belief>> {
    choices.iter().copied().max_by(|a, b| rate(world, r, civ, *a).cmp(&rate(world, r, civ, *b)).then(b.cmp(a)))
}
