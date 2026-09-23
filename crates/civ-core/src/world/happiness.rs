//! Empire happiness.
//!
//! Derived, never stored. It is a pure function of the world, and a cached copy
//! would be a `@Transient` by another name — the thing Unciv has 185 of across
//! 43 files, and the reason it needs a rebuild step after every load. The
//! arithmetic is integer over a few dozen cities; this is not the expensive
//! part of a turn.
//!
//! What happiness *does* is not written here, and that is the interesting part.
//! The penalties live in `GlobalUniques.json` as conditional uniques:
//!
//! ```text
//! [-75]% growth [in all cities]      <when between [-10] and [-1] [Happiness]>
//! Nullifies Growth [in all cities]   <when below [-10] [Happiness]>
//! [-50]% [Production] [in all cities] <when below [-10] [Happiness]>
//! [-33]% Strength <for [All] units>   <when below [-10] [Happiness]>
//! ```
//!
//! So this module computes a number, and
//! [`conditionals`](crate::world::conditionals) turns that number into
//! behaviour. Writing the thresholds in Rust would have duplicated data that
//! the ruleset already states — and a mod changing them would then be ignored.

use crate::modifiers::{Source, SourceKind};
use crate::ruleset::{Difficulty, Ruleset};
use crate::stats::Fixed;
use crate::world::grid::CivId;
use crate::world::world::World;

/// Happiness each distinct luxury provides, before difficulty.
const BASE_HAPPINESS_PER_LUXURY: i32 = 4;

/// Unhappiness each city causes (`CityStats.kt:398`).
const UNHAPPINESS_PER_CITY: i32 = -3;

/// Unhappiness each citizen causes.
const UNHAPPINESS_PER_CITIZEN: i32 = -1;

/// What an annexed city adds to its unhappiness as a city, before a Courthouse.
const ANNEXED_CITY_EXTRA: i32 = -2;

/// Unhappiness an annexed city adds for the occupation itself, unscaled.
const OCCUPIED_CITY: i32 = -2;

/// One line of the happiness tally.
pub type Contribution = (Source, i32);

/// Where a civ's happiness comes from, itemised.
///
/// Itemised rather than summed because "why is my empire unhappy" is the
/// question players actually ask, and answering it after the fact means
/// recomputing.
pub fn breakdown(world: &World, r: &Ruleset, civ_id: CivId, difficulty: Option<&Difficulty>) -> Vec<Contribution> {
    let Some(civ) = world.civ(civ_id) else { return Vec::new() };
    let mut out = Vec::new();

    let base = difficulty.map_or(9, |d| d.base_happiness);
    out.push((Source::new(SourceKind::Difficulty, "Base happiness"), base));

    // Each *distinct* luxury counts once, however many copies you hold — which
    // is why Civ players trade for variety rather than quantity.
    let ctx = crate::world::conditionals::GameContext::new(r).with_world(world).for_civ(civ_id);
    let bonus = |kind| {
        civ.uniques
            .matching(kind)
            .filter(|u| crate::world::conditionals::applies(u, &ctx) == crate::world::conditionals::Verdict::True)
            .filter_map(|u| u.params.first()?.parse::<i32>().ok())
            .sum::<i32>()
    };
    let per_luxury = BASE_HAPPINESS_PER_LUXURY
        + difficulty.map_or(0, |d| d.extra_happiness_per_luxury)
        + bonus(crate::UniqueType::BonusHappinessFromLuxury);
    let luxuries = civ
        .resources
        .iter()
        .filter(|(_, amount)| **amount > 0)
        .filter(|(id, _)| r.resources.get(**id).is_some_and(|x| x.resource_type == "Luxury"))
        .count() as i32;
    if luxuries > 0 {
        out.push((Source::new(SourceKind::Resource, "Luxury resources"), luxuries * per_luxury));
    }

    let supplied: std::collections::BTreeSet<_> = crate::world::city_states::city_states(world)
        .into_iter()
        .filter(|cs| crate::world::city_states::ally_of(world, *cs) == Some(civ_id))
        .flat_map(|cs| crate::world::resources::recompute_stock(world, r, cs))
        .filter(|(id, n)| {
            *n > 0
                && civ.resources.get(id).copied().unwrap_or(0) > 0
                && r.resources.get(*id).is_some_and(|res| res.resource_type == "Luxury")
        })
        .map(|(id, _)| id)
        .collect();
    let extra = (Fixed::whole(per_luxury * supplied.len() as i32)
        * Fixed::from_fraction(i64::from(bonus(crate::UniqueType::CityStateLuxuryHappiness)), 100))
    .floor() as i32;
    if extra != 0 {
        out.push((Source::new(SourceKind::Resource, "City-State luxuries"), extra));
    }
    let exported: std::collections::BTreeSet<_> = world
        .diplomacy
        .deals
        .iter()
        .flat_map(|d| [(d.a, &d.a_gives), (d.b, &d.b_gives)])
        .filter(|(giver, _)| *giver == civ_id)
        .flat_map(|(_, items)| items)
        .filter(|item| item.turns_left > 0)
        .filter_map(|item| match item.item {
            crate::world::trade::Item::Luxury(id) if civ.resources.get(&id).copied().unwrap_or(0) <= 0 => Some(id),
            _ => None,
        })
        .collect();
    let retained = (Fixed::whole(per_luxury * exported.len() as i32)
        * Fixed::from_fraction(i64::from(bonus(crate::UniqueType::RetainHappinessFromLuxury)), 100))
    .floor() as i32;
    if retained != 0 {
        out.push((Source::new(SourceKind::Resource, "Traded luxuries"), retained));
    }

    let wonders = crate::world::natural_wonders::known_stats(world, r, civ_id).happiness.floor() as i32;
    if wonders != 0 {
        out.push((Source::new(SourceKind::Other, "Natural Wonders"), wonders));
    }

    let from_cities: i32 = world
        .cities
        .iter()
        .filter(|(_, city)| city.owner == civ_id)
        .map(|(_, city)| crate::world::city::city_yield_in(world, r, city, false).happiness.floor() as i32)
        .sum();
    if from_cities != 0 {
        out.push((Source::new(SourceKind::Other, "City happiness"), from_cities));
    }
    let global = crate::world::city_states::civ_wide_yields(world, r, civ_id).happiness.floor() as i32;
    if global != 0 {
        out.push((Source::new(SourceKind::Other, "Empire happiness"), global));
    }
    let modifier = difficulty.map_or(Fixed::ONE, |d| d.unhappiness_modifier);
    let city_modifier =
        Fixed::from_fraction(i64::from(100 + bonus(crate::UniqueType::UnhappinessFromCitiesPercentage)), 100);
    let mut city_unhappy = Fixed::ZERO;
    let mut citizen_unhappy = Fixed::ZERO;
    let mut occupied = 0;
    for id in &civ.cities {
        let Some(city) = world.cities.get(*id) else { continue };
        let annexed = crate::world::conquest::annexed_unhappiness(city);
        city_unhappy += Fixed::whole(UNHAPPINESS_PER_CITY + if annexed { ANNEXED_CITY_EXTRA } else { 0 })
            * city_modifier
            * modifier;
        let mut people = Fixed::whole(city.population as i32);
        let context = crate::world::conditionals::GameContext::new(r).with_world(world).for_civ(civ_id).in_city(*id);
        for u in crate::world::religion::city_uniques(world, r, city) {
            if u.unique_type() != Some(crate::UniqueType::UnhappinessFromPopulationTypePercentageChange)
                || crate::world::conditionals::applies(u, &context) != crate::world::conditionals::Verdict::True
                || !u.params.get(2).is_some_and(|f| {
                    crate::world::filters::city_matches(world, r, city, Some(city), f)
                        == crate::world::conditionals::Verdict::True
                })
            {
                continue;
            }
            if let (Some(pct), Some(filter)) = (u.params.first().and_then(|p| p.parse::<i64>().ok()), u.params.get(1)) {
                people += Fixed::whole(crate::world::city_effects::population_count(r, city, filter) as i32)
                    * Fixed::from_fraction(pct, 100);
            }
        }
        citizen_unhappy += people.max(Fixed::ZERO)
            * Fixed::whole(if annexed { 2 * UNHAPPINESS_PER_CITIZEN } else { UNHAPPINESS_PER_CITIZEN })
            * modifier;
        if annexed {
            occupied += 1;
        }
    }
    if !city_unhappy.is_zero() {
        out.push((Source::new(SourceKind::Other, "Cities"), city_unhappy.floor() as i32));
    }
    if !citizen_unhappy.is_zero() {
        out.push((Source::new(SourceKind::Other, "Population"), citizen_unhappy.floor() as i32));
    }
    if occupied > 0 {
        out.push((Source::new(SourceKind::Other, "Occupied cities"), occupied * OCCUPIED_CITY));
    }

    out
}

/// The civ's net happiness.
pub fn happiness(world: &World, r: &Ruleset, civ: CivId, difficulty: Option<&Difficulty>) -> i32 {
    breakdown(world, r, civ, difficulty).into_iter().map(|(_, v)| v).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::city::City;
    use crate::world::conditionals::{applies, GameContext, Verdict};
    use crate::world::entities::Civilization;
    use crate::world::grid::{TileGrid, TileId};
    use crate::Unique;

    fn ruleset() -> Ruleset {
        Ruleset::load_dir("rules/gods-and-kings").expect("readable").expect("loads")
    }

    fn world(r: &Ruleset) -> World {
        let g = TileGrid::new(16, 16, r.terrain("Grassland").unwrap());
        let nation = r.nations.ids().next().unwrap();
        World::new(g, vec![Civilization::new(CivId(0), nation, false)])
    }

    fn prince(r: &Ruleset) -> Option<&Difficulty> {
        r.difficulties.by_name("Prince")
    }

    #[test]
    fn an_empty_civ_starts_content() {
        let r = ruleset();
        let w = world(&r);
        assert_eq!(happiness(&w, &r, CivId(0), prince(&r)), 9, "Prince's base happiness");
    }

    #[test]
    fn cities_and_citizens_both_cost() {
        let r = ruleset();
        let mut w = world(&r);
        let centre = w.grid.id_at(8, 8).unwrap();
        let id = w.found_city(City::found("Rome", CivId(0), centre));

        // One city of one citizen: 9 - 3 - 1.
        assert_eq!(happiness(&w, &r, CivId(0), prince(&r)), 5);

        w.cities.get_mut(id).unwrap().population = 6;
        assert_eq!(happiness(&w, &r, CivId(0), prince(&r)), 0, "9 - 3 - 6");

        let items = breakdown(&w, &r, CivId(0), prince(&r));
        println!("{:?}", items.iter().map(|(s, v)| (&s.name, v)).collect::<Vec<_>>());
        assert_eq!(items.iter().map(|(_, v)| *v).sum::<i32>(), 0);
    }

    #[test]
    fn variety_of_luxuries_is_what_counts_not_quantity() {
        let r = ruleset();
        let mut w = world(&r);
        let luxuries: Vec<_> =
            r.resources.iter().filter(|(_, x)| x.resource_type == "Luxury").map(|(id, _)| id).take(2).collect();
        assert_eq!(luxuries.len(), 2, "the ruleset has luxuries");

        let base = happiness(&w, &r, CivId(0), prince(&r));

        // Ten copies of one luxury.
        w.civ_mut(CivId(0)).unwrap().resources.insert(luxuries[0], 10);
        let one_kind = happiness(&w, &r, CivId(0), prince(&r));
        assert_eq!(one_kind - base, 4, "four per distinct luxury, however many copies");

        // One copy of a second.
        w.civ_mut(CivId(0)).unwrap().resources.insert(luxuries[1], 1);
        assert_eq!(happiness(&w, &r, CivId(0), prince(&r)) - one_kind, 4);
    }

    /// The point of the whole layer: a number computed here becomes behaviour
    /// declared in the ruleset.
    #[test]
    fn an_unhappy_empire_triggers_the_rulesets_own_penalties() {
        let r = ruleset();
        let mut w = world(&r);

        // A sprawling, miserable empire.
        for i in 0..6u16 {
            let centre = w.grid.id_at(2 + i * 2, 8).unwrap();
            let id = w.found_city(City::found(format!("City{i}"), CivId(0), centre));
            w.cities.get_mut(id).unwrap().population = 4;
        }
        let h = happiness(&w, &r, CivId(0), prince(&r));
        println!("six cities of four: {h} happiness");
        assert!(h < -10, "should be well into revolt territory, got {h}");

        let ctx = GameContext::new(&r).with_world(&w).for_civ(CivId(0)).with_happiness(h);
        for text in [
            "Nullifies Growth [in all cities] <when below [-10] [Happiness]>",
            "[-50]% [Production] [in all cities] <when below [-10] [Happiness]>",
        ] {
            assert_eq!(applies(&Unique::parse(text), &ctx), Verdict::True, "{text}");
        }

        // And a contented empire triggers none of them.
        let calm = GameContext::new(&r).with_world(&w).for_civ(CivId(0)).with_happiness(5);
        assert_eq!(
            applies(&Unique::parse("Nullifies Growth [in all cities] <when below [-10] [Happiness]>"), &calm),
            Verdict::False
        );
    }

    #[test]
    fn computing_it_twice_gives_the_same_answer() {
        let r = ruleset();
        let mut w = world(&r);
        w.found_city(City::found("Rome", CivId(0), TileId(30)));
        let once = breakdown(&w, &r, CivId(0), prince(&r));
        let twice = breakdown(&w, &r, CivId(0), prince(&r));
        assert_eq!(once, twice, "it is derived, so it cannot drift");
    }
}
