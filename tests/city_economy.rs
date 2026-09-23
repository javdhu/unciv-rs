use civ_core::{
    arena::Id,
    ruleset::Ruleset,
    stats::Fixed,
    world::{
        city::{self, City, GrowthModifiers},
        city_effects, effects,
        entities::{Civilization, Unit},
        great_people,
        grid::{CivId, TileGrid},
        happiness, policies, religion,
        world::World,
    },
    Unique,
};
fn setup(nation: &str) -> (Ruleset, World, Id<City>) {
    let r = Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings").unwrap().unwrap();
    let mut w = World::new(
        TileGrid::new(20, 12, r.terrain("Grassland").unwrap()),
        vec![
            Civilization::new(CivId(0), r.nations.id(nation).unwrap(), false),
            Civilization::new(CivId(1), r.nations.id("Greece").unwrap(), false),
        ],
    );
    let id = w.found_city_in(&r, City::found("Home", CivId(0), w.grid.id_at(4, 5).unwrap()));
    (r, w, id)
}
fn policy(w: &mut World, r: &Ruleset, name: &str) {
    w.civ_mut(CivId(0)).unwrap().adopted_policies.insert(r.policies.id(name).unwrap());
    policies::rebuild_uniques(w, r, CivId(0));
}
fn building(w: &mut World, r: &Ruleset, city: Id<City>, name: &str) {
    let c = w.cities.get_mut(city).unwrap();
    c.built.insert(r.buildings.id(name).unwrap());
    city::rebuild_city_uniques(r, c);
    policies::rebuild_uniques(w, r, CivId(0));
}
#[test]
fn aqueduct_carries_food_only_on_growth_and_growth_nullification_keeps_bank() {
    let (r, mut w, id) = setup("Rome");
    building(&mut w, &r, id, "Aqueduct");
    let (carry, held) = city_effects::growth_rules(&w, &r, w.cities.get(id).unwrap(), 5);
    assert_eq!((carry, held), (40, false));
    let c = w.cities.get_mut(id).unwrap();
    let needed = city::food_to_next_population(1, GrowthModifiers::default());
    city::apply_food_with_rules(c, needed + 3, GrowthModifiers::default(), carry, held);
    assert_eq!(c.population, 2);
    assert_eq!(c.food_stored, 3 + needed * 40 / 100);
    let (_, held) = city_effects::growth_rules(&w, &r, w.cities.get(id).unwrap(), -20);
    assert!(held);
    let c = w.cities.get_mut(id).unwrap();
    city::apply_food_with_rules(c, 100, GrowthModifiers::default(), 95, held);
    assert_eq!(c.population, 2);
    assert!(c.food_stored > 100);
}
#[test]
fn civil_society_and_democracy_reduce_actual_specialist_costs() {
    let (r, mut w, id) = setup("Rome");
    let scientist = r.specialists.id("Scientist").unwrap();
    let c = w.cities.get_mut(id).unwrap();
    c.population = 6;
    c.specialists.insert(scientist, 2);
    let before = happiness::happiness(&w, &r, CivId(0), None);
    assert_eq!(city_effects::food_consumed(&w, &r, w.cities.get(id).unwrap()), Fixed::whole(12));
    policy(&mut w, &r, "Civil Society");
    policy(&mut w, &r, "Democracy");
    assert_eq!(city_effects::food_consumed(&w, &r, w.cities.get(id).unwrap()), Fixed::whole(10));
    assert_eq!(happiness::happiness(&w, &r, CivId(0), None) - before, 1);
}
#[test]
fn india_and_luxury_policy_apply_to_live_happiness() {
    let (r, mut w, id) = setup("India");
    w.cities.get_mut(id).unwrap().population = 8;
    assert_eq!(happiness::happiness(&w, &r, CivId(0), None), 9 - 6 - 4);
    let gold = r
        .resources
        .id("Gold Ore")
        .or_else(|| r.resources.iter().find(|(_, r)| r.resource_type == "Luxury").map(|(id, _)| id))
        .unwrap();
    w.civ_mut(CivId(0)).unwrap().resources.insert(gold, 1);
    let before = happiness::happiness(&w, &r, CivId(0), None);
    policy(&mut w, &r, "Protectionism");
    assert_eq!(happiness::happiness(&w, &r, CivId(0), None) - before, 2);
}
#[test]
fn eiffel_tower_counts_paid_and_free_policies_but_not_finishers() {
    let (r, mut w, id) = setup("Rome");
    building(&mut w, &r, id, "Eiffel Tower");
    for name in ["Tradition", "Legalism", "Tradition Complete"] {
        policy(&mut w, &r, name);
    }
    assert_eq!(city_effects::reserve_yields(&w, &r, CivId(0)).happiness, Fixed::ONE);
    w.civ_mut(CivId(0)).unwrap().uniques.insert(Unique::parse("[50]% of excess happiness converted to [Culture]"));
    assert_eq!(city_effects::happiness_conversion(&w, &r, CivId(0), 7).culture, Fixed::permille(3500));
}
#[test]
fn freedom_doubles_improvement_yield_without_doubling_underlying_grassland() {
    let (r, mut w, id) = setup("Rome");
    let tile = w.grid.id_at(5, 5).unwrap();
    w.claim_tile(id, tile);
    w.cities.get_mut(id).unwrap().worked.push(tile);
    w.grid.improvement[tile.0 as usize] = r.improvements.id("Academy");
    let before = city::city_yield_in(&w, &r, w.cities.get(id).unwrap(), false);
    policy(&mut w, &r, "Freedom Complete");
    let after = city::city_yield_in(&w, &r, w.cities.get(id).unwrap(), false);
    assert_eq!(after.food, before.food);
    assert_eq!(after.science - before.science, Fixed::whole(8));
}
#[test]
fn civilian_great_people_share_pool_and_combat_people_use_separate_threshold() {
    let (r, mut w, id) = setup("Rome");
    let c = w.civ_mut(CivId(0)).unwrap();
    c.great_people.points.insert("Great Scientist".into(), 100);
    c.great_people.points.insert("Great Merchant".into(), 100);
    assert_eq!(great_people::process_turn(&mut w, &r, CivId(0), Fixed::ONE).len(), 1);
    assert_eq!(w.civ(CivId(0)).unwrap().great_people.thresholds.get("Great Person"), Some(&200));
    let warrior =
        w.place_unit(Unit::new(r.units.id("Warrior").unwrap(), CivId(0), w.cities.get(id).unwrap().centre, 2));
    great_people::on_combat_xp(&mut w, &r, warrior, 200, false);
    assert_eq!(w.civ(CivId(0)).unwrap().great_people.combat_points.get("Great General"), Some(&200));
    assert!(great_people::process_turn(&mut w, &r, CivId(0), Fixed::whole(3)).contains(&"Great General".into()));
    assert_eq!(w.civ(CivId(0)).unwrap().great_people.combat_thresholds.get("Great General"), Some(&250));
    great_people::on_combat_xp(&mut w, &r, warrior, 100, true);
    assert_eq!(w.civ(CivId(0)).unwrap().great_people.combat_points.get("Great General"), Some(&0));
}
#[test]
fn local_gardens_and_national_scientist_bonus_apply_before_gp_rounding() {
    let (r, mut w, id) = setup("Babylon");
    w.cities.get_mut(id).unwrap().specialists.insert(r.specialists.id("Scientist").unwrap(), 2);
    building(&mut w, &r, id, "Garden");
    assert_eq!(great_people::points_per_turn(&w, &r, CivId(0)).get("Great Scientist"), Some(&11));
}
#[test]
fn first_conversion_reward_is_paid_once_and_history_survives_restore() {
    let (r, mut w, id) = setup("Rome");
    let c = w.civ_mut(CivId(1)).unwrap();
    c.faith.founded = true;
    c.faith.beliefs.insert(r.beliefs.id("Initiation Rites").unwrap());
    policies::rebuild_uniques(&mut w, &r, CivId(1));
    let c = w.cities.get_mut(id).unwrap();
    c.faith.add_pressure(Some(CivId(1)), 1000);
    c.faith.recount(c.population);
    let before = w.civ(CivId(1)).unwrap().gold;
    religion::settle_adoptions(&mut w, &r, Fixed::permille(670));
    assert_eq!(w.civ(CivId(1)).unwrap().gold - before, 67);
    religion::settle_adoptions(&mut w, &r, Fixed::permille(670));
    assert_eq!(w.civ(CivId(1)).unwrap().gold - before, 67);
    #[cfg(feature = "save")]
    {
        let saved = civ_core::save::encode(&w, &r).unwrap();
        let mut restored = civ_core::save::decode(&saved, &r).unwrap();
        religion::settle_adoptions(&mut restored, &r, Fixed::ONE);
        assert_eq!(restored, w);
    }
}
#[test]
fn spy_rewards_use_new_spy_level_then_raise_existing_spies() {
    let (r, mut w, _) = setup("Rome");
    w.civ_mut(CivId(0)).unwrap().uniques.insert(Unique::parse("New spies start with [1] level(s)"));
    effects::apply_uniques(&mut w, &r, CivId(0), None, &[Unique::parse("Gain an extra spy")], Fixed::ONE);
    assert_eq!(w.civ(CivId(0)).unwrap().espionage.spies[0].rank, 2);
    effects::apply_uniques(&mut w, &r, CivId(0), None, &[Unique::parse("Promotes all spies [2] time(s)")], Fixed::ONE);
    assert_eq!(w.civ(CivId(0)).unwrap().espionage.spies[0].rank, 3);
}
#[test]
fn maya_cycles_require_theology_and_restrict_each_choice_until_pool_empty() {
    let (r, mut w, _) = setup("The Maya");
    assert_eq!(great_people::calendar_year(&r, Fixed::ONE, 0), -4000);
    assert_eq!(great_people::calendar_year(&r, Fixed::ONE, 75), -1000);
    assert_eq!(great_people::calendar_year(&r, Fixed::ONE, 500), 2050);
    for turn in 1..=100 {
        w.turn = turn;
        great_people::begin_round(&mut w, &r, Fixed::ONE);
    }
    assert_eq!(w.civ(CivId(0)).unwrap().pending_rewards.free_great_people, 0);
    w.civ_mut(CivId(0)).unwrap().techs.insert(r.technologies.id("Theology").unwrap());
    for turn in 101..200 {
        w.turn = turn;
        great_people::begin_round(&mut w, &r, Fixed::ONE);
        if w.civ(CivId(0)).unwrap().pending_rewards.free_great_people > 0 {
            break;
        }
    }
    assert_eq!(w.civ(CivId(0)).unwrap().pending_rewards.free_great_people, 1);
    great_people::begin_round(&mut w, &r, Fixed::ONE);
    assert_eq!(w.civ(CivId(0)).unwrap().pending_rewards.free_great_people, 1);
    let scientist = r.units.id("Great Scientist").unwrap();
    effects::claim_free_great_person(&mut w, &r, CivId(0), scientist).unwrap();
    for turn in w.turn + 1..250 {
        w.turn = turn;
        great_people::begin_round(&mut w, &r, Fixed::ONE);
        if w.civ(CivId(0)).unwrap().pending_rewards.free_great_people > 0 {
            break;
        }
    }
    let before = w.clone();
    assert!(effects::claim_free_great_person(&mut w, &r, CivId(0), scientist).is_none());
    assert_eq!(w, before);
    assert!(!effects::free_great_person_options(&w, &r, CivId(0)).contains(&scientist));
    #[cfg(feature = "save")]
    {
        assert_eq!(civ_core::save::decode(&civ_core::save::encode(&w, &r).unwrap(), &r).unwrap(), w);
    }
}
#[test]
fn mausoleum_reward_uses_great_person_filter_and_is_not_replayed_on_rebuild() {
    let (r, mut w, id) = setup("Rome");
    building(&mut w, &r, id, "Mausoleum of Halicarnassus");
    let artist =
        w.place_unit(Unit::new(r.units.id("Great Artist").unwrap(), CivId(0), w.cities.get(id).unwrap().centre, 2));
    let before = w.civ(CivId(0)).unwrap().gold;
    civ_core::world::great_person_actions::perform(
        &mut w,
        &r,
        artist,
        civ_core::world::great_person_actions::Action::GoldenAge,
        Default::default(),
    )
    .unwrap();
    assert_eq!(w.civ(CivId(0)).unwrap().gold - before, 100);
    policies::rebuild_uniques(&mut w, &r, CivId(0));
    assert_eq!(w.civ(CivId(0)).unwrap().gold - before, 100);
}
#[test]
fn allied_city_state_luxuries_supply_stock_and_patronage_happiness() {
    let (r, mut w, _) = setup("Rome");
    w.civ_mut(CivId(1)).unwrap().city_state_type = r.city_state_types.ids().next();
    let foreign = w.found_city_in(&r, City::found("Ally", CivId(1), w.grid.id_at(15, 5).unwrap()));
    let gold = r
        .resources
        .iter()
        .find(|(_, res)| res.resource_type == "Luxury" && res.revealed_by.is_none())
        .map(|(id, _)| id)
        .unwrap();
    let tile = w.cities.get(foreign).unwrap().centre;
    w.grid.resource[tile.0 as usize] = Some(gold);
    w.civ_mut(CivId(1)).unwrap().techs.extend(r.technologies.ids());
    w.diplomacy.meet(CivId(0), CivId(1));
    w.diplomacy.add_influence(CivId(1), CivId(0), Fixed::whole(80));
    civ_core::world::resources::refresh_stocks(&mut w, &r);
    assert_eq!(w.civ(CivId(0)).unwrap().resources.get(&gold), Some(&1));
    let before = happiness::happiness(&w, &r, CivId(0), None);
    let policy_id = r
        .policies
        .iter()
        .find(|(_, p)| {
            p.uniques.iter().any(|u| u.unique_type() == Some(civ_core::UniqueType::CityStateLuxuryHappiness))
        })
        .map(|(id, _)| id)
        .unwrap();
    w.civ_mut(CivId(0)).unwrap().adopted_policies.insert(policy_id);
    policies::rebuild_uniques(&mut w, &r, CivId(0));
    assert_eq!(happiness::happiness(&w, &r, CivId(0), None) - before, 2);
}
#[test]
fn tithe_preserves_fractional_global_followers() {
    let (r, mut w, id) = setup("Rome");
    let c = w.civ_mut(CivId(1)).unwrap();
    c.faith.founded = true;
    c.faith.beliefs.insert(r.beliefs.id("Tithe").unwrap());
    policies::rebuild_uniques(&mut w, &r, CivId(1));
    let c = w.cities.get_mut(id).unwrap();
    c.population = 7;
    c.faith.add_pressure(Some(CivId(1)), 100000);
    c.faith.recount(7);
    assert_eq!(city_effects::founder_yields(&w, &r, CivId(1)).gold, Fixed::permille(1750));
}
#[test]
fn building_specific_percent_bonus_affects_city_stat_once() {
    let (r, mut w, id) = setup("Rome");
    building(&mut w, &r, id, "Temple");
    let before = city::city_yield_in(&w, &r, w.cities.get(id).unwrap(), false);
    w.civ_mut(CivId(0)).unwrap().uniques.insert(Unique::parse("[+10]% [Gold] from every [Temple]"));
    let after = city::city_yield_in(&w, &r, w.cities.get(id).unwrap(), false);
    assert_eq!(after.gold, before.gold * Fixed::permille(1100));
}
#[test]
fn korean_capital_library_rewards_science_at_supplied_research_costs() {
    use civ_core::world::{
        construction::{self, Construction, CostModifiers},
        research::{self, ScienceModifiers},
    };
    let (r, mut w, id) = setup("Korea");
    let c = w.civ_mut(CivId(0)).unwrap();
    c.techs.insert(r.technologies.id("Agriculture").unwrap());
    c.techs.insert(r.technologies.id("Pottery").unwrap());
    c.techs.insert(r.technologies.id("Writing").unwrap());
    let target = r.technologies.id("Archery").unwrap();
    c.researching = Some(target);
    let science = ScienceModifiers { speed: Fixed::whole(2), ..Default::default() };
    let mut prices: Vec<_> = r
        .technologies
        .ids()
        .filter(|t| research::can_research(&r, w.civ(CivId(0)).unwrap(), *t).is_ok())
        .filter_map(|t| research::tech_cost_in(&w, &r, CivId(0), t, science))
        .collect();
    prices.sort();
    let expected = if prices.len() % 2 == 0 {
        (prices[prices.len() / 2 - 1] + prices[prices.len() / 2] + 2) / 4
    } else {
        (prices[prices.len() / 2] + 1) / 2
    };
    w.cities.get_mut(id).unwrap().queue = vec![Construction::Building(r.buildings.id("Library").unwrap())];
    construction::apply_production_with_science(&mut w, &r, id, 10000, CostModifiers::default(), science).unwrap();
    assert_eq!(w.civ(CivId(0)).unwrap().science_invested.get(&target), Some(&expected));
}
#[test]
fn satellites_reveal_terrain_without_giving_live_units_or_foreign_city_details() {
    let (r, mut w, _) = setup("Rome");
    let far = w.grid.id_at(16, 6).unwrap();
    let foreign = w.found_city_in(&r, City::found("Secret", CivId(1), far));
    let unit = w.place_unit(Unit::new(r.units.id("Warrior").unwrap(), CivId(1), far, 2));
    let satellites = r.technologies.id("Satellites").unwrap();
    w.civ_mut(CivId(0)).unwrap().techs.insert(satellites);
    effects::on_technology_researched(&mut w, &r, CivId(0), satellites, Fixed::ONE);
    assert_eq!(w.visibility.explored_count(CivId(0)) as usize, w.grid.len());
    assert!(!w.visibility.can_see(CivId(0), far));
    assert!(w.visibility.known_tiles(CivId(0)).any(|(t, _)| t == far));
    assert!(!w.visibility.known_cities(CivId(0)).any(|c| c.id == foreign));
    assert!(w.units.get(unit).is_some());
}
#[test]
fn freedom_multiplies_koreas_added_great_improvement_science_too() {
    let (r, mut w, id) = setup("Korea");
    let tile = w.grid.id_at(5, 5).unwrap();
    w.claim_tile(id, tile);
    w.cities.get_mut(id).unwrap().worked.push(tile);
    w.grid.improvement[tile.0 as usize] = r.improvements.id("Academy");
    let before = city::city_yield_in(&w, &r, w.cities.get(id).unwrap(), false);
    policy(&mut w, &r, "Freedom Complete");
    let after = city::city_yield_in(&w, &r, w.cities.get(id).unwrap(), false);
    assert_eq!(after.science - before.science, Fixed::whole(10));
}
#[test]
fn russian_deposits_bazaars_and_recycling_supply_real_resource_stock() {
    let (r, mut w, id) = setup("Russia");
    w.civ_mut(CivId(0)).unwrap().techs.extend(r.technologies.ids());
    let tile = w.cities.get(id).unwrap().centre;
    let iron = r.resources.id("Iron").unwrap();
    w.grid.resource[tile.0 as usize] = Some(iron);
    w.grid.resource_amount[tile.0 as usize] = 2;
    assert_eq!(civ_core::world::resources::available_stock(&w, &r, CivId(0)).get(&iron), Some(&4));
    w.civ_mut(CivId(0)).unwrap().nation = r.nations.id("Arabia").unwrap();
    let luxury = r.resources.iter().find(|(_, res)| res.resource_type == "Luxury").map(|(id, _)| id).unwrap();
    w.grid.resource[tile.0 as usize] = Some(luxury);
    building(&mut w, &r, id, "Bazaar");
    assert_eq!(civ_core::world::resources::available_stock(&w, &r, CivId(0)).get(&luxury), Some(&2));
    building(&mut w, &r, id, "Recycling Center");
    assert_eq!(
        civ_core::world::resources::available_stock(&w, &r, CivId(0)).get(&r.resources.id("Aluminum").unwrap()),
        Some(&2)
    );
}
#[test]
fn factories_cannot_both_reserve_the_same_coal_and_free_buildings_do_not_consume() {
    use civ_core::stats::Stat;
    use civ_core::world::{
        construction::{Construction, CostModifiers},
        purchase, resources,
    };
    let (r, mut w, id) = setup("Rome");
    w.civ_mut(CivId(0)).unwrap().techs.extend(r.technologies.ids());
    w.civ_mut(CivId(0)).unwrap().gold = 100000;
    let tile = w.cities.get(id).unwrap().centre;
    let coal = r.resources.id("Coal").unwrap();
    w.grid.resource[tile.0 as usize] = Some(coal);
    w.grid.resource_amount[tile.0 as usize] = 1;
    let second = w.found_city_in(&r, City::found("Second", CivId(0), w.grid.id_at(12, 5).unwrap()));
    building(&mut w, &r, id, "Workshop");
    building(&mut w, &r, second, "Workshop");
    let factory = r.buildings.id("Factory").unwrap();
    purchase::purchase(&mut w, &r, id, Construction::Building(factory), Stat::Gold, CostModifiers::default()).unwrap();
    assert_eq!(resources::available_stock(&w, &r, CivId(0)).get(&coal), Some(&0));
    let before = w.clone();
    assert!(purchase::purchase(
        &mut w,
        &r,
        second,
        Construction::Building(factory),
        Stat::Gold,
        CostModifiers::default()
    )
    .is_err());
    assert_eq!(w, before);
    w.cities.get_mut(id).unwrap().free_buildings.insert(factory);
    assert_eq!(resources::available_stock(&w, &r, CivId(0)).get(&coal), Some(&1));
}
#[test]
fn fountain_of_youth_yields_without_taking_a_citizen_and_only_once_when_worked() {
    let (r, mut w, id) = setup("Rome");
    let tile = w.grid.id_at(5, 5).unwrap();
    w.claim_tile(id, tile);
    w.grid.natural_wonder[tile.0 as usize] = r.terrains.id("Fountain of Youth");
    let c = w.cities.get(id).unwrap();
    assert!(c.worked.is_empty());
    assert_eq!(city::city_yield_in(&w, &r, c, false).happiness, Fixed::whole(10));
    let before = happiness::happiness(&w, &r, CivId(0), None);
    w.cities.get_mut(id).unwrap().worked.push(tile);
    assert_eq!(happiness::happiness(&w, &r, CivId(0), None), before);
}
#[test]
fn production_conversion_uses_hammer_bonus_before_output_stat_bonus() {
    use civ_core::stats::Stat;
    let (r, mut w, id) = setup("Rome");
    let c = w.civ_mut(CivId(0)).unwrap();
    for text in [
        "[+100]% [Production] [in all cities]",
        "[+100]% [Gold] [in all cities]",
        "Production to [Gold] conversion in cities changed by [100]%",
    ] {
        c.uniques.insert(Unique::parse(text));
    }
    let before = city::city_yield_in(&w, &r, w.cities.get(id).unwrap(), false);
    w.cities.get_mut(id).unwrap().production_conversion = Some(Stat::Gold);
    let after = city::city_yield_in(&w, &r, w.cities.get(id).unwrap(), false);
    assert_eq!(after.production, before.production);
    assert_eq!(after.gold - before.gold, before.production);
}
#[test]
fn byzantium_explicit_beliefs_require_extra_slot_and_reject_duplicates_atomically() {
    use civ_core::world::{
        commands::{self, Command},
        religious_units,
    };
    let (r, mut w, _) = setup("Byzantium");
    w.civ_mut(CivId(0)).unwrap().faith.beliefs.insert(r.beliefs.id("Desert Folklore").unwrap());
    policies::rebuild_uniques(&mut w, &r, CivId(0));
    let unit = religious_units::spawn_prophet(&mut w, &r, CivId(0)).unwrap();
    let needs = religious_units::belief_requirements(&w, &r, CivId(0), false);
    assert_eq!(needs.get("Any"), Some(&1));
    let founder = r.beliefs.id("Tithe").unwrap();
    let follower = r.beliefs.id("Cathedrals").unwrap();
    let extra = r.beliefs.id("Pagodas").unwrap();
    let before = w.clone();
    assert!(commands::execute(
        &mut w,
        &r,
        CivId(0),
        Command::ChooseReligionBeliefs { unit, beliefs: vec![founder, follower, follower], enhancing: false }
    )
    .is_err());
    assert_eq!(w, before);
    commands::execute(
        &mut w,
        &r,
        CivId(0),
        Command::ChooseReligionBeliefs { unit, beliefs: vec![founder, follower, extra], enhancing: false },
    )
    .unwrap();
    assert!(w.civ(CivId(0)).unwrap().faith.founded);
    assert!(w.civ(CivId(0)).unwrap().faith.beliefs.contains(&extra));
    assert!(w.units.get(unit).is_none());
}
#[test]
fn messiah_reduces_the_faith_actually_spent_to_generate_a_prophet() {
    let (r, mut w, _) = setup("Rome");
    w.turn = 20; // Deterministic successful roll at the five-percent threshold.
    let c = w.civ_mut(CivId(0)).unwrap();
    c.faith.founded = true;
    c.faith.beliefs.insert(r.beliefs.id("Desert Folklore").unwrap());
    c.faith.beliefs.insert(r.beliefs.id("Messiah").unwrap());
    policies::rebuild_uniques(&mut w, &r, CivId(0));
    assert_eq!(religion::prophet_cost_in(&w, &r, CivId(0), Fixed::ONE), 150);
    assert!(matches!(
        religion::apply_faith(&mut w, &r, CivId(0), 149, Fixed::ONE),
        religion::FaithEvent::Stored { .. }
    ));
    assert!(matches!(
        religion::apply_faith(&mut w, &r, CivId(0), 1, Fixed::ONE),
        religion::FaithEvent::ProphetBorn { .. }
    ));
    assert_eq!(w.civ(CivId(0)).unwrap().faith.stored, 0);
}

#[test]
fn religion_cap_preserves_faith_and_rejects_a_waiting_prophet_atomically() {
    use civ_core::world::{
        commands::{self, Command},
        religious_units::{self, ReligiousAction},
    };
    let (mut r, mut w, _) = setup("Rome");
    for (id, nation) in [(2, "Egypt"), (3, "India")] {
        w.civs.push(Civilization::new(CivId(id), r.nations.id(nation).unwrap(), false));
    }
    assert_eq!(r.religions.len(), 11);
    assert_eq!(religion::max_religions(&w, &r), 3);
    w.civ_mut(CivId(3)).unwrap().defeated = true;
    assert_eq!(religion::max_religions(&w, &r), 3);
    w.civ_mut(CivId(0)).unwrap().faith.beliefs.insert(r.beliefs.id("Desert Folklore").unwrap());
    let prophet = religious_units::spawn_prophet(&mut w, &r, CivId(0)).unwrap();
    for id in 1..4 {
        w.civ_mut(CivId(id)).unwrap().faith.founded = true;
    }
    assert_eq!(religion::remaining_foundable_religions(&w, &r), 0);
    let before = w.clone();
    assert!(commands::execute(
        &mut w,
        &r,
        CivId(0),
        Command::Religious { unit: prophet, action: ReligiousAction::FoundReligion }
    )
    .is_err());
    assert_eq!(w, before);
    w.remove_unit(prophet);
    assert_eq!(
        religion::apply_faith(&mut w, &r, CivId(0), 500, Fixed::ONE),
        religion::FaithEvent::Stored { faith: 500 }
    );
    assert!(w.civ(CivId(0)).unwrap().units.is_empty());

    // Available religion symbols can impose a stricter limit than player count.
    r.religions.truncate(1);
    assert_eq!(religion::max_religions(&w, &r), 1);
}

#[test]
fn enhanced_religion_closes_late_pantheons_for_ai_and_player_actions() {
    use civ_core::world::commands::{self, Command};
    let (r, mut w, _) = setup("Rome");
    for (id, nation) in [(2, "Egypt"), (3, "India")] {
        w.civs.push(Civilization::new(CivId(id), r.nations.id(nation).unwrap(), false));
    }
    let pantheons = religion::available_beliefs(&w, &r, "Pantheon");
    for id in 1..4 {
        w.civ_mut(CivId(id)).unwrap().faith.beliefs.insert(pantheons[id as usize]);
    }
    assert!(religion::can_found_pantheon(&w, &r, CivId(0)));
    let c = w.civ_mut(CivId(1)).unwrap();
    c.faith.founded = true;
    c.faith.enhanced = true;
    assert!(!religion::can_found_pantheon(&w, &r, CivId(0)));
    assert_eq!(
        religion::apply_faith(&mut w, &r, CivId(0), 500, Fixed::ONE),
        religion::FaithEvent::Stored { faith: 500 }
    );
    let before = w.clone();
    assert!(commands::execute(&mut w, &r, CivId(0), Command::ChoosePantheon { belief: pantheons[0] }).is_err());
    assert_eq!(w, before);
    // Existing religions may still earn prophets after founding is closed.
    w.found_city_in(&r, City::found("Holy", CivId(1), w.grid.id_at(14, 5).unwrap()));
    assert!(matches!(
        religion::apply_faith(&mut w, &r, CivId(1), 295, Fixed::ONE),
        religion::FaithEvent::ProphetBorn { .. }
    ));
}

#[test]
fn religion_slots_also_require_unclaimed_founder_and_follower_beliefs() {
    let (r, mut w, _) = setup("Rome");
    assert_eq!(religion::remaining_foundable_religions(&w, &r), 2);
    for belief in religion::available_beliefs(&w, &r, "Founder") {
        w.civ_mut(CivId(1)).unwrap().faith.beliefs.insert(belief);
    }
    assert_eq!(religion::remaining_foundable_religions(&w, &r), 0);
}

#[test]
fn prophet_roll_repeats_by_turn_and_preserves_unspent_faith() {
    let (r, mut w, _) = setup("Rome");
    w.civ_mut(CivId(0)).unwrap().faith.beliefs.insert(r.beliefs.id("Desert Folklore").unwrap());
    assert_eq!(
        religion::apply_faith(&mut w, &r, CivId(0), 200, Fixed::ONE),
        religion::FaithEvent::Stored { faith: 200 }
    );
    assert_eq!(w.civ(CivId(0)).unwrap().faith.prophets_earned, 0);
    w.turn = 20;
    let mut replay = w.clone();
    assert!(matches!(
        religion::apply_faith(&mut w, &r, CivId(0), 0, Fixed::ONE),
        religion::FaithEvent::ProphetBorn { .. }
    ));
    religion::apply_faith(&mut replay, &r, CivId(0), 0, Fixed::ONE);
    assert_eq!(w, replay);
    assert_eq!(w.civ(CivId(0)).unwrap().faith.stored, 0);
    // Ninety-five excess faith makes the next roll certain, and remains stored.
    assert!(matches!(
        religion::apply_faith(&mut w, &r, CivId(0), 395, Fixed::ONE),
        religion::FaithEvent::ProphetBorn { .. }
    ));
    assert_eq!(w.civ(CivId(0)).unwrap().faith.stored, 95);
    assert_eq!(w.civ(CivId(0)).unwrap().faith.prophets_earned, 2);
}

#[test]
fn great_mosque_trains_earned_prophets_and_purchased_missionaries() {
    use civ_core::{
        stats::Stat,
        world::{
            construction::{Construction, CostModifiers},
            purchase, religious_units,
        },
    };
    let (r, mut w, id) = setup("Rome");
    let c = w.civ_mut(CivId(0)).unwrap();
    c.faith.founded = true;
    c.faith.stored = 1000;
    c.faith.beliefs.insert(r.beliefs.id("Desert Folklore").unwrap());
    let city = w.cities.get_mut(id).unwrap();
    city.faith.holy_city_of = Some(CivId(0));
    city.faith.add_pressure(Some(CivId(0)), 1000);
    city.faith.recount(city.population);
    building(&mut w, &r, id, "Great Mosque of Djenne");
    let prophet = religious_units::spawn_prophet(&mut w, &r, CivId(0)).unwrap();
    let unit = w.units.get(prophet).unwrap();
    assert!(unit.promotions.contains(&r.promotions.id("Devout").unwrap()));
    assert_eq!(unit.religious_uses_left, 5);
    assert!(religious_units::can(&w, &r, prophet, religious_units::ReligiousAction::EnhanceReligion).is_ok());
    w.remove_unit(prophet);
    let missionary = r.units.id("Missionary").unwrap();
    purchase::purchase(&mut w, &r, id, Construction::Unit(missionary), Stat::Faith, CostModifiers::default()).unwrap();
    let unit = w.units.iter().find(|(_, u)| u.base == missionary).unwrap().1;
    assert!(unit.promotions.contains(&r.promotions.id("Devout").unwrap()));
    assert_eq!(unit.religious_uses_left, 3);
}

#[test]
fn blocked_prophet_birth_keeps_faith_and_does_not_raise_the_next_price() {
    let (r, mut w, id) = setup("Rome");
    w.civ_mut(CivId(0)).unwrap().faith.beliefs.insert(r.beliefs.id("Desert Folklore").unwrap());
    let centre = w.cities.get(id).unwrap().centre;
    w.grid.terrain.fill(r.terrains.id("Mountain").unwrap());
    w.grid.terrain[centre.0 as usize] = r.terrains.id("Grassland").unwrap();
    w.place_unit(Unit::new(r.units.id("Worker").unwrap(), CivId(0), centre, 2));
    assert_eq!(
        religion::apply_faith(&mut w, &r, CivId(0), 295, Fixed::ONE),
        religion::FaithEvent::Stored { faith: 295 }
    );
    assert_eq!(w.civ(CivId(0)).unwrap().faith.prophets_earned, 0);
    assert_eq!(religion::prophet_cost_in(&w, &r, CivId(0), Fixed::ONE), 200);
}

#[test]
fn siam_scales_only_city_state_food_culture_and_faith_at_first_rebuild() {
    use civ_core::world::city_states;
    let (r, mut w, home) = setup("Siam");
    building(&mut w, &r, home, "Shrine");
    let before = city::city_yield_in(&w, &r, w.cities.get(home).unwrap(), false);
    for (id, kind, row) in [(1, "Maritime", 2), (2, "Cultured", 6), (3, "Religious", 10)] {
        if id > 1 {
            w.civs.push(Civilization::new(CivId(id), r.nations.id("Greece").unwrap(), true));
        }
        w.civ_mut(CivId(id)).unwrap().city_state_type = r.city_state_types.id(kind);
        w.found_city_in(&r, City::found(kind, CivId(id), w.grid.id_at(15, row).unwrap()));
        w.diplomacy.meet(CivId(0), CivId(id));
        w.diplomacy.add_influence(CivId(id), CivId(0), Fixed::whole(40));
    }
    w.civ_mut(CivId(0)).unwrap().uniques = Default::default();
    policies::rebuild_uniques(&mut w, &r, CivId(0));
    let friend = city_states::civ_wide_yields(&w, &r, CivId(0));
    assert_eq!((friend.culture, friend.faith), (Fixed::permille(4500), Fixed::permille(4500)));
    assert_eq!(city::city_yield_in(&w, &r, w.cities.get(home).unwrap(), false).food - before.food, Fixed::whole(3));
    assert_eq!(city::city_yield_in(&w, &r, w.cities.get(home).unwrap(), false).faith, before.faith);
    assert_eq!(civ_core::world::turn::plan_economy(&w, &r, CivId(0), Default::default(), Default::default()).faith, 5);
    for id in 1..4 {
        w.diplomacy.add_influence(CivId(id), CivId(0), Fixed::whole(40));
    }
    policies::rebuild_uniques(&mut w, &r, CivId(0));
    let ally = city_states::civ_wide_yields(&w, &r, CivId(0));
    assert_eq!((ally.culture, ally.faith), (Fixed::whole(9), Fixed::whole(9)));
    assert_eq!(
        city::city_yield_in(&w, &r, w.cities.get(home).unwrap(), false).food - before.food,
        Fixed::permille(4500)
    );
    // Rebuilding cannot compound the multiplier onto already-scaled rewards.
    policies::rebuild_uniques(&mut w, &r, CivId(0));
    assert_eq!(city_states::civ_wide_yields(&w, &r, CivId(0)), ally);
}

#[test]
fn celts_faith_counts_only_adjacent_unimproved_forests() {
    let (r, mut w, home) = setup("Celts");
    let centre = w.cities.get(home).unwrap().centre;
    let neighbours: Vec<_> = w.grid.neighbours(centre).collect();
    let output = |w: &World| city::city_yield_in(w, &r, w.cities.get(home).unwrap(), false).faith;
    assert_eq!(output(&w), Fixed::ZERO);
    for (i, expected) in [(0, 1), (1, 1), (2, 2)] {
        w.grid.features[neighbours[i].0 as usize].push(r.terrains.id("Forest").unwrap());
        assert_eq!(output(&w), Fixed::whole(expected));
    }
    w.grid.improvement[neighbours[0].0 as usize] = r.improvements.id("Lumber mill");
    assert!(w.grid.improvement[neighbours[0].0 as usize].is_some());
    assert_eq!(output(&w), Fixed::ONE);
    w.grid.improvement_pillaged[neighbours[0].0 as usize] = true;
    assert_eq!(output(&w), Fixed::whole(2));
}

#[test]
fn city_center_and_improvements_read_the_ruleset_minimum_yield() {
    let (mut r, mut w, home) = setup("Rome");
    let centre = w.cities.get(home).unwrap().centre;
    w.grid.terrain[centre.0 as usize] = r.terrains.id("Snow").unwrap();
    let before = city::city_yield_in(&w, &r, w.cities.get(home).unwrap(), false);
    assert_eq!(before.food, Fixed::whole(2));
    let centre_improvement = r.improvements.id("City center").unwrap();
    r.improvements.get_mut(centre_improvement).unwrap().uniques =
        vec![Unique::parse("Ensures a minimum tile yield of [+4 Food, +3 Production]")];
    let after = city::city_yield_in(&w, &r, w.cities.get(home).unwrap(), false);
    assert_eq!(after.food - before.food, Fixed::whole(2));
    assert_eq!(after.production - before.production, Fixed::whole(2));
    let farm = r.improvements.id("Farm").unwrap();
    r.improvements.get_mut(farm).unwrap().uniques.push(Unique::parse("Ensures a minimum tile yield of [+6 Food]"));
    let tile = w.grid.neighbours(centre).next().unwrap();
    w.grid.improvement[tile.0 as usize] = Some(farm);
    assert_eq!(civ_core::world::yields::tile_yield(&w.grid, &r, tile).food, Fixed::whole(6));
}

#[test]
fn empire_reserves_truncate_after_combining_fractional_cities_and_upkeep() {
    let (r, mut w, _) = setup("Rome");
    w.found_city_in(&r, City::found("Second", CivId(0), w.grid.id_at(10, 5).unwrap()));
    let output =
        |w: &World| civ_core::world::turn::plan_economy(w, &r, CivId(0), Default::default(), Default::default());
    let before = output(&w);
    w.civ_mut(CivId(0))
        .unwrap()
        .uniques
        .insert(Unique::parse("[+0.6 Science, +0.6 Culture, +0.6 Faith, +0.6 Gold] [in all cities]"));
    let after = output(&w);
    assert_eq!(
        (
            after.science - before.science,
            after.culture - before.culture,
            after.faith - before.faith,
            after.gold - before.gold
        ),
        (1, 1, 1, 1)
    );

    let (r, mut w, _) = setup("Rome");
    let before = civ_core::world::turn::plan_economy(&w, &r, CivId(0), Default::default(), Default::default());
    w.civ_mut(CivId(0)).unwrap().uniques.insert(Unique::parse(&format!("[-{}.6 Gold]", before.gold)));
    let after = civ_core::world::turn::plan_economy(&w, &r, CivId(0), Default::default(), Default::default());
    assert_eq!(after.gold, 0, "negative fractions truncate toward zero too");
}

#[test]
fn coal_base_and_mine_resource_bonus_wait_for_industrialization() {
    use civ_core::world::yields;
    let (r, mut w, home) = setup("Rome");
    let tile = w.grid.neighbours(w.cities.get(home).unwrap().centre).next().unwrap();
    w.claim_tile(home, tile);
    w.cities.get_mut(home).unwrap().worked.push(tile);
    w.grid.improvement[tile.0 as usize] = r.improvements.id("Mine");
    let output = |w: &World| city::city_yield_in(w, &r, w.cities.get(home).unwrap(), false);
    let without = output(&w);
    w.grid.resource[tile.0 as usize] = r.resources.id("Coal");
    let hidden = output(&w);
    assert_eq!(hidden, without, "unrevealed coal cannot affect the city's output");
    assert!(!yields::tile_yield_breakdown_for(&w.grid, &r, tile, w.civ(CivId(0)).unwrap())
        .iter()
        .any(|(source, _)| source.name == "Coal"));
    let old_plan = civ_core::world::turn::plan_economy(&w, &r, CivId(0), Default::default(), Default::default());
    w.civ_mut(CivId(0)).unwrap().techs.insert(r.technologies.id("Industrialization").unwrap());
    let revealed = output(&w);
    assert_eq!(revealed.production - hidden.production, Fixed::whole(2));
    let plan = civ_core::world::turn::plan_economy(&w, &r, CivId(0), Default::default(), Default::default());
    assert_eq!(plan.production[0].1 - old_plan.production[0].1, 2);
    w.grid.improvement_pillaged[tile.0 as usize] = true;
    assert_eq!(yields::tile_yield_for(&w.grid, &r, tile, w.civ(CivId(0)).unwrap()).production, Fixed::ONE);
}

#[test]
fn hidden_iron_in_city_center_does_not_grant_base_or_forge_bonus() {
    use civ_core::world::conditionals::{tile_matches, GameContext, Verdict};
    let (r, mut w, home) = setup("Rome");
    let centre = w.cities.get(home).unwrap().centre;
    w.grid.features[centre.0 as usize].push(r.terrains.id("Hill").unwrap());
    building(&mut w, &r, home, "Forge");
    let before = city::city_yield_in(&w, &r, w.cities.get(home).unwrap(), false);
    w.grid.resource[centre.0 as usize] = r.resources.id("Iron");
    assert_eq!(city::city_yield_in(&w, &r, w.cities.get(home).unwrap(), false), before);
    assert_eq!(tile_matches(&GameContext::new(&r).with_world(&w).for_civ(CivId(0)), centre, "Iron"), Verdict::False);
    w.civ_mut(CivId(0)).unwrap().techs.insert(r.technologies.id("Iron Working").unwrap());
    assert_eq!(tile_matches(&GameContext::new(&r).with_world(&w).for_civ(CivId(0)), centre, "Iron"), Verdict::True);
    let after = city::city_yield_in(&w, &r, w.cities.get(home).unwrap(), false);
    assert_eq!(after.production - before.production, Fixed::whole(2));
    w.civ_mut(CivId(0)).unwrap().uniques.insert(Unique::parse("[+100]% [Production] from every [Iron]"));
    let boosted = city::city_yield_in(&w, &r, w.cities.get(home).unwrap(), false);
    assert_eq!(boosted.production - after.production, Fixed::whole(4));
    w.civ_mut(CivId(0)).unwrap().techs.remove(&r.technologies.id("Iron Working").unwrap());
    assert_eq!(city::city_yield_in(&w, &r, w.cities.get(home).unwrap(), false), before);
}

#[test]
fn automatic_citizen_assignment_cannot_prefer_unrevealed_deposit() {
    use civ_core::world::assignment::{assign_idle_citizens_for, CityFocus};
    let (r, mut w, home) = setup("Rome");
    let centre = w.cities.get(home).unwrap().centre;
    let mut tiles: Vec<_> = w.grid.neighbours(centre).collect();
    tiles.sort();
    // Restrict the comparison to equal known terrain with a deterministic tie.
    w.grid.owner.fill(None);
    w.claim_tile(home, centre);
    for &tile in &tiles[..2] {
        w.claim_tile(home, tile);
    }
    w.grid.resource[tiles[1].0 as usize] = r.resources.id("Coal");
    let mut city = w.cities.get(home).unwrap().clone();
    assign_idle_citizens_for(&w.grid, &r, &mut city, CityFocus::None, Default::default(), w.civ(CivId(0)).unwrap());
    assert_eq!(city.worked, vec![tiles[0]]);
    w.civ_mut(CivId(0)).unwrap().techs.insert(r.technologies.id("Industrialization").unwrap());
    city.worked.clear();
    assign_idle_citizens_for(&w.grid, &r, &mut city, CityFocus::None, Default::default(), w.civ(CivId(0)).unwrap());
    assert_eq!(city.worked, vec![tiles[1]]);
}
