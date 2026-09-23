//! Ruleset contributions evaluated with the city and its actual religion.
use crate::ruleset::Ruleset;
use crate::stats::{Fixed, Stat, Stats};
use crate::world::conditionals::{applies, GameContext, Verdict};
use crate::world::{city::City, world::World};
use crate::UniqueType;

pub fn bonuses(world: &World, r: &Ruleset, city: &City, happiness: Option<i32>) -> (Stats, Stats) {
    let id = world.cities.iter().find(|(_, c)| c.centre == city.centre).map(|(id, _)| id);
    let mut ctx = GameContext::new(r).with_world(world).for_civ(city.owner);
    ctx.city = id;
    ctx.happiness = happiness;
    let mut flat = Stats::ZERO;
    let mut percent = Stats::ZERO;
    let tiles = crate::world::city::yield_tiles(&world.grid, r, city);
    let city_matches = |f: &str| {
        crate::world::filters::multi_filter(f, &|term| {
            crate::world::filters::city_matches(world, r, city, Some(city), term)
        }) == Verdict::True
    };
    for u in crate::world::religion::city_uniques(world, r, city).into_iter().chain(r.global_uniques.iter()) {
        if applies(u, &ctx) != Verdict::True {
            continue;
        }
        let p = |i: usize| u.params.get(i).map(String::as_str);
        let stats = p(0).and_then(Stats::parse).unwrap_or_default();
        let int = |i: usize| p(i).and_then(|p| p.parse::<i32>().ok()).unwrap_or(0);
        let tile_matches =
            |tile, filter: &str| crate::world::conditionals::tile_matches(&ctx, tile, filter) == Verdict::True;
        let count = match u.unique_type() {
            Some(UniqueType::Stats) if city.uniques.all().any(|local| std::ptr::eq(local, u)) => 1,
            Some(UniqueType::StatsPerCity) if p(1).is_some_and(city_matches) => 1,
            Some(UniqueType::StatsPerPopulation) if int(1) > 0 && p(2).is_some_and(city_matches) => {
                city.population as i32 / int(1)
            }
            Some(UniqueType::StatsFromCitiesOnSpecificTiles) if p(1).is_some_and(|f| tile_matches(city.centre, f)) => 1,
            Some(UniqueType::StatsFromTiles) if p(2).is_some_and(city_matches) => {
                tiles.iter().filter(|&&tile| p(1).is_some_and(|f| tile_matches(tile, f))).count() as i32
            }
            Some(UniqueType::StatsFromTilesWithout) if p(3).is_some_and(city_matches) => tiles
                .iter()
                .filter(|&&tile| {
                    p(1).is_some_and(|f| tile_matches(tile, f)) && p(2).is_some_and(|f| !tile_matches(tile, f))
                })
                .count()
                as i32,
            Some(UniqueType::StatsFromBuildings) => city
                .built
                .iter()
                .filter(|&&b| p(1).is_some_and(|f| crate::world::filters::building_matches(r, b, f) == Verdict::True))
                .count() as i32,
            Some(UniqueType::StatsFromObject) => {
                let Some(f) = p(1) else { continue };
                let buildings = city
                    .built
                    .iter()
                    .filter(|&&b| crate::world::filters::building_matches(r, b, f) == Verdict::True)
                    .count();
                let worked = tiles.iter().filter(|&&tile| tile_matches(tile, f)).count();
                let specialists = city
                    .specialists
                    .iter()
                    .filter(|(id, _)| r.specialists.name_of(**id) == Some(f) || f == "Specialist")
                    .map(|(_, n)| *n as usize)
                    .sum::<usize>();
                (buildings + worked + specialists) as i32
            }
            _ => 0,
        };
        for stat in Stat::ALL {
            flat[stat] += stats[stat] * Fixed::whole(count);
        }
        let value = Fixed::whole(int(0));
        match u.unique_type() {
            Some(UniqueType::StatPercentFromObject) => {
                if let (Some(stat), Some(filter)) = (p(1).and_then(Stat::parse), p(2)) {
                    let count = city
                        .built
                        .iter()
                        .filter(|b| crate::world::filters::building_matches(r, **b, filter) == Verdict::True)
                        .count();
                    percent[stat] += value * Fixed::whole(count as i32);
                }
            }
            Some(UniqueType::AllStatsPercentFromObject) => {
                if let Some(filter) = p(1) {
                    let count = city
                        .built
                        .iter()
                        .filter(|b| crate::world::filters::building_matches(r, **b, filter) == Verdict::True)
                        .count();
                    for stat in Stat::ALL {
                        percent[stat] += value * Fixed::whole(count as i32);
                    }
                }
            }
            Some(UniqueType::StatPercentBonus) => {
                if let Some(stat) = p(1).and_then(Stat::parse) {
                    percent[stat] += value;
                }
            }
            Some(UniqueType::StatPercentBonusCities) if p(2).is_some_and(city_matches) => {
                if let Some(stat) = p(1).and_then(Stat::parse) {
                    percent[stat] += value;
                }
            }
            Some(UniqueType::StatPercentFromReligionFollowers) => {
                if let Some(stat) = p(1).and_then(Stat::parse) {
                    let followers = city.faith.majority_followers(city.population);
                    percent[stat] += Fixed::whole((int(0) * followers as i32).min(int(2)));
                }
            }
            Some(
                UniqueType::PercentProductionBuildings
                | UniqueType::PercentProductionWonders
                | UniqueType::PercentProductionUnits,
            ) if p(2).is_some_and(city_matches) => {
                let matches = match city.queue.first() {
                    Some(crate::world::construction::Construction::Building(b))
                        if u.unique_type() != Some(UniqueType::PercentProductionUnits) =>
                    {
                        let wonder = r
                            .buildings
                            .get(*b)
                            .is_some_and(|b| b.is_wonder == Some(true) || b.is_national_wonder == Some(true));
                        (u.unique_type() != Some(UniqueType::PercentProductionWonders) || wonder)
                            && p(1).is_some_and(|f| crate::world::filters::building_matches(r, *b, f) == Verdict::True)
                    }
                    Some(crate::world::construction::Construction::Unit(b))
                        if u.unique_type() == Some(UniqueType::PercentProductionUnits) =>
                    {
                        p(1).is_some_and(|f| crate::world::filters::base_unit_matches(r, *b, f) == Verdict::True)
                    }
                    _ => false,
                };
                if matches {
                    percent.production += value;
                }
            }
            Some(UniqueType::PercentProductionBuildingsInCapital) => {
                if let Some(crate::world::construction::Construction::Building(b)) = city.queue.first() {
                    if crate::world::connections::capital_of(world, r, city.owner)
                        .and_then(|id| world.cities.get(id))
                        .is_some_and(|capital| capital.built.contains(b))
                    {
                        percent.production += value;
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(id) = id {
        let specialist_bonus = crate::world::specialists::bonus_per_specialist(world, r, id);
        for stat in Stat::ALL {
            flat[stat] += specialist_bonus[stat] * Fixed::whole(crate::world::specialists::count(city) as i32);
        }
    }
    for tile in tiles {
        flat += crate::world::improvements::conditional_yields(world, r, city.owner, tile);
    }
    (flat, percent)
}

pub fn founder_yields(world: &World, r: &Ruleset, civ: crate::world::grid::CivId) -> Stats {
    let Some(c) = world.civ(civ) else { return Stats::ZERO };
    let mut total = Stats::ZERO;
    for u in crate::world::religion::founder_uniques(r, &c.faith) {
        if applies(u, &GameContext::new(r).with_world(world).for_civ(civ)) != Verdict::True {
            continue;
        }
        let n = match u.unique_type() {
            Some(UniqueType::StatsFromGlobalCitiesFollowingReligion) => Fixed::whole(
                world.cities.iter().filter(|(_, city)| city.faith.majority(city.population) == Some(civ)).count()
                    as i32,
            ),
            Some(UniqueType::StatsFromGlobalFollowers) => {
                let per = u.params.get(1).and_then(|p| p.parse::<i32>().ok()).unwrap_or(0);
                if per <= 0 {
                    continue;
                }
                let filter = u.params.get(2).map_or("in all cities", String::as_str);
                let followers: u32 = world
                    .cities
                    .iter()
                    .filter(|(_, city)| match filter {
                        "in foreign cities" => city.owner != civ,
                        "in foreign cities that are not at war with you" | "in non-enemy foreign cities" => {
                            city.owner != civ && !world.diplomacy.at_war(civ, city.owner)
                        }
                        _ => true,
                    })
                    .map(|(_, city)| city.faith.followers_of(civ))
                    .sum();
                Fixed::from_fraction(i64::from(followers), i64::from(per))
            }
            _ => Fixed::ZERO,
        };
        if let Some(stats) = u.params.first().and_then(|p| Stats::parse(p)) {
            for stat in Stat::ALL {
                total[stat] += stats[stat] * n;
            }
        }
    }
    total
}

/// Population filters shared by food and unhappiness rules.
pub fn population_count(r: &Ruleset, city: &City, filter: &str) -> u32 {
    match filter {
        "Population" => city.population,
        "Specialists" => crate::world::specialists::count(city),
        "Followers of the Majority Religion" | "Followers of this Religion" => {
            city.faith.majority_followers(city.population)
        }
        "Unemployed" => {
            city.population.saturating_sub(city.worked.len() as u32 + crate::world::specialists::count(city))
        }
        name => r.specialists.id(name).and_then(|id| city.specialists.get(&id).copied()).unwrap_or(0),
    }
}

pub fn food_consumed(world: &World, r: &Ruleset, city: &City) -> Fixed {
    let id = world.cities.iter().find(|(_, c)| c.centre == city.centre).map(|(id, _)| id);
    let mut ctx = GameContext::new(r).with_world(world).for_civ(city.owner);
    ctx.city = id;
    let matches = |f: &str| crate::world::filters::city_matches(world, r, city, Some(city), f) == Verdict::True;
    let specialists = crate::world::specialists::count(city) as i32;
    let mut specialist_food = Fixed::whole(2 * specialists);
    let uniques = crate::world::religion::city_uniques(world, r, city);
    for u in &uniques {
        if u.unique_type() == Some(UniqueType::FoodConsumptionBySpecialists)
            && applies(u, &ctx) == Verdict::True
            && u.params.get(1).is_some_and(|f| matches(f))
        {
            if let Some(pct) = u.params.first().and_then(|p| Fixed::parse(p)) {
                specialist_food = specialist_food * (Fixed::ONE + pct.checked_div(Fixed::whole(100)).unwrap());
            }
        }
    }
    let mut food = Fixed::whole(2 * (city.population as i32 - specialists)) + specialist_food;
    for u in &uniques {
        if u.unique_type() == Some(UniqueType::FoodConsumptionByPopulation)
            && applies(u, &ctx) == Verdict::True
            && u.params.get(2).is_some_and(|f| matches(f))
        {
            if let (Some(pct), Some(filter)) = (u.params.first().and_then(|p| Fixed::parse(p)), u.params.get(1)) {
                food += (Fixed::whole(2 * population_count(r, city, filter) as i32) * pct)
                    .checked_div(Fixed::whole(100))
                    .unwrap();
            }
        }
    }
    food.max(Fixed::ZERO)
}

/// Carryover is capped at 95%; nullified growth retains the accumulated food.
pub fn growth_rules(world: &World, r: &Ruleset, city: &City, happiness: i32) -> (i32, bool) {
    let id = world.cities.iter().find(|(_, c)| c.centre == city.centre).map(|(id, _)| id);
    let mut ctx = GameContext::new(r).with_world(world).for_civ(city.owner).with_happiness(happiness);
    ctx.city = id;
    let mut carry = 0;
    let mut nullified = false;
    for u in crate::world::religion::city_uniques(world, r, city).into_iter().chain(r.global_uniques.iter()) {
        if applies(u, &ctx) != Verdict::True {
            continue;
        }
        let filter = match u.unique_type() {
            Some(UniqueType::CarryOverFood) => u.params.get(1),
            Some(UniqueType::NullifiesGrowth) => u.params.first(),
            _ => continue,
        };
        if !filter.is_some_and(|f| crate::world::filters::city_matches(world, r, city, Some(city), f) == Verdict::True)
        {
            continue;
        }
        if u.unique_type() == Some(UniqueType::CarryOverFood) {
            carry += u.params[0].parse::<i32>().unwrap_or(0);
        } else {
            nullified = true;
        }
    }
    (carry.clamp(0, 95), nullified)
}

pub fn maintenance(world: &World, r: &Ruleset, city: &City) -> i32 {
    let id = world.cities.iter().find(|(_, c)| c.centre == city.centre).map(|(id, _)| id);
    let mut ctx = GameContext::new(r).with_world(world).for_civ(city.owner);
    ctx.city = id;
    let uniques = crate::world::religion::city_uniques(world, r, city);
    let mut total = Fixed::ZERO;
    for id in city.built.iter().filter(|id| !city.free_buildings.contains(id)) {
        let mut cost = Fixed::whole(r.buildings.get(*id).and_then(|b| b.maintenance).unwrap_or(0));
        for u in &uniques {
            if u.unique_type() == Some(UniqueType::BuildingMaintenance)
                && applies(u, &ctx) == Verdict::True
                && u.params.get(1).is_some_and(|f| crate::world::filters::building_matches(r, *id, f) == Verdict::True)
                && u.params.get(2).is_some_and(|f| {
                    crate::world::filters::city_matches(world, r, city, Some(city), f) == Verdict::True
                })
            {
                cost = cost
                    * (Fixed::ONE
                        + u.params
                            .first()
                            .and_then(|p| Fixed::parse(p))
                            .unwrap_or_default()
                            .checked_div(Fixed::whole(100))
                            .unwrap());
            }
        }
        total += cost;
    }
    total.floor().max(0) as i32
}

pub fn reserve_yields(world: &World, r: &Ruleset, civ: crate::world::grid::CivId) -> Stats {
    let Some(c) = world.civ(civ) else { return Stats::ZERO };
    let mut out = Stats::ZERO;
    let ctx = GameContext::new(r).with_world(world).for_civ(civ);
    for u in c.uniques.all().filter(|u| applies(u, &ctx) == Verdict::True) {
        let per = u.params.get(1).and_then(|p| p.parse::<i32>().ok()).unwrap_or(0);
        if per <= 0 {
            continue;
        }
        let amount = match u.unique_type() {
            Some(UniqueType::StatsPerPolicies) => {
                c.adopted_policies
                    .iter()
                    .filter(|id| r.policies.get(**id).is_some_and(|p| !p.name.ends_with(" Complete")))
                    .count() as i32
                    / per
            }
            Some(UniqueType::StatsPerStat) => match u.params.get(2).and_then(|p| Stat::parse(p)) {
                Some(Stat::Gold) => c.gold / per,
                Some(Stat::Culture) => c.culture_stored / per,
                Some(Stat::Faith) => c.faith.stored / per,
                Some(Stat::Science) => c.science_stored / per,
                _ => 0,
            },
            _ => 0,
        };
        if let Some(stats) = u.params.first().and_then(|p| Stats::parse(p)) {
            for stat in Stat::ALL {
                out[stat] += stats[stat] * Fixed::whole(amount);
            }
        }
    }
    out
}

pub fn happiness_conversion(world: &World, r: &Ruleset, civ: crate::world::grid::CivId, happiness: i32) -> Stats {
    let mut out = Stats::ZERO;
    let Some(c) = world.civ(civ) else { return out };
    if happiness <= 0 {
        return out;
    }
    let ctx = GameContext::new(r).with_world(world).for_civ(civ).with_happiness(happiness);
    for u in c.uniques.matching(UniqueType::ExcessHappinessToGlobalStat).filter(|u| applies(u, &ctx) == Verdict::True) {
        if let (Some(pct), Some(stat)) =
            (u.params.first().and_then(|p| Fixed::parse(p)), u.params.get(1).and_then(|p| Stat::parse(p)))
        {
            out[stat] += (Fixed::whole(happiness) * pct).checked_div(Fixed::whole(100)).unwrap();
        }
    }
    out
}
