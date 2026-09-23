use civ_core::{
    arena::Id,
    ruleset::Ruleset,
    stats::{Fixed, Stat},
    world::{
        city::{self, City},
        commands::{self, Command},
        construction::{self, Construction, CostModifiers},
        effects,
        entities::{Civilization, Unit},
        grid::{CivId, TileGrid},
        policies::{self, PolicyModifiers},
        purchase,
        world::World,
    },
    Unique,
};
fn setup() -> (Ruleset, World, Id<City>) {
    let r = Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings").unwrap().unwrap();
    let mut w = World::new(
        TileGrid::new(20, 12, r.terrain("Grassland").unwrap()),
        vec![
            Civilization::new(CivId(0), r.nations.id("Rome").unwrap(), false),
            Civilization::new(CivId(1), r.nations.id("Babylon").unwrap(), false),
        ],
    );
    let city = w.found_city_in(&r, City::found("Rome", CivId(0), w.grid.id_at(5, 5).unwrap()));
    (r, w, city)
}
#[test]
fn great_library_grants_one_choice_and_free_library_after_completion() {
    let (r, mut w, city) = setup();
    w.civ_mut(CivId(0)).unwrap().techs.insert(r.technologies.id("Writing").unwrap());
    let what = Construction::Building(r.buildings.id("The Great Library").unwrap());
    w.cities.get_mut(city).unwrap().queue = vec![what];
    construction::apply_production_in(&mut w, &r, city, 10000, CostModifiers::default()).unwrap();
    assert_eq!(w.civ(CivId(0)).unwrap().pending_rewards.free_techs, 1);
    let library = r.buildings.id("Library").unwrap();
    assert!(w.cities.get(city).unwrap().built.contains(&library));
    assert!(w.cities.get(city).unwrap().free_buildings.contains(&library));
    assert_eq!(city::city_maintenance(&r, w.cities.get(city).unwrap()), 0);
    effects::refresh_free_buildings(&mut w, &r, CivId(0));
    construction::apply_production_in(&mut w, &r, city, 10000, CostModifiers::default()).unwrap();
    assert_eq!(w.civ(CivId(0)).unwrap().pending_rewards.free_techs, 1);
}
#[test]
fn free_technology_choice_validates_atomically_and_preserves_other_work() {
    let (r, mut w, _) = setup();
    w.civ_mut(CivId(0)).unwrap().pending_rewards.free_techs = 1;
    let writing = r.technologies.id("Writing").unwrap();
    let before = w.clone();
    assert!(commands::execute(&mut w, &r, CivId(0), Command::ClaimFreeTechnology { technology: writing }).is_err());
    assert_eq!(w, before);
    let pottery = r.technologies.id("Agriculture").unwrap();
    w.civ_mut(CivId(0)).unwrap().science_invested.insert(writing, 17);
    commands::execute(&mut w, &r, CivId(0), Command::ClaimFreeTechnology { technology: pottery }).unwrap();
    assert!(w.civ(CivId(0)).unwrap().techs.contains(&pottery));
    assert_eq!(w.civ(CivId(0)).unwrap().science_invested.get(&writing), Some(&17));
    assert_eq!(w.civ(CivId(0)).unwrap().pending_rewards.free_techs, 0);
}
#[test]
fn free_policies_do_not_raise_cost_and_finishers_are_automatic() {
    let (r, mut w, _) = setup();
    let civ = CivId(0);
    let finisher = r.policies.id("Liberty Complete").unwrap();
    assert!(policies::is_adoptable(&w, &r, civ, finisher).is_err());
    w.civ_mut(civ).unwrap().pending_rewards.free_policies = 6;
    loop {
        let choice = r.policies.ids().find(|id| {
            r.policies[*id].branch == r.policies[finisher].branch && policies::is_adoptable(&w, &r, civ, *id).is_ok()
        });
        let Some(choice) = choice else { break };
        policies::apply_culture(&mut w, &r, civ, 0, Some(choice), PolicyModifiers::default()).unwrap();
    }
    let c = w.civ(civ).unwrap();
    assert!(c.adopted_policies.contains(&finisher));
    assert_eq!(c.paid_policies, 0);
    assert_eq!(c.pending_rewards.free_policies, 0);
    assert_eq!(c.pending_rewards.free_great_people, 1);
    assert_eq!(policies::policy_cost_in(&w, &r, civ, PolicyModifiers::default()), 25);
    let units = w.units.len();
    commands::execute(&mut w, &r, civ, Command::ClaimFreeGreatPerson { unit: r.units.id("Great Scientist").unwrap() })
        .unwrap();
    assert_eq!(w.units.len(), units + 1);
    assert_eq!(w.civ(civ).unwrap().pending_rewards.free_great_people, 0);
}
#[test]
fn legalism_grants_first_four_cities_once_and_survives_save() {
    let (r, mut w, first) = setup();
    let civ = CivId(0);
    for name in ["Tradition", "Legalism"] {
        w.civ_mut(civ).unwrap().pending_rewards.free_policies = 1;
        policies::apply_culture(&mut w, &r, civ, 0, Some(r.policies.id(name).unwrap()), PolicyModifiers::default())
            .unwrap();
    }
    let monument = r.buildings.id("Monument").unwrap();
    assert!(w.cities.get(first).unwrap().free_buildings.contains(&monument));
    for i in 0..4 {
        let city = w.found_city_in(&r, City::found(format!("New{i}"), civ, w.grid.id_at(1 + i * 4, 10).unwrap()));
        assert_eq!(w.cities.get(city).unwrap().built.contains(&monument), i < 3);
    }
    effects::refresh_free_buildings(&mut w, &r, civ);
    assert_eq!(w.civ(civ).unwrap().free_building_grants["stat:Culture"].len(), 4);
    #[cfg(feature = "save")]
    {
        let restored = civ_core::save::decode(&civ_core::save::encode(&w, &r).unwrap(), &r).unwrap();
        assert_eq!(w, restored);
    }
}
#[test]
fn babylon_research_trigger_spawns_scientist_in_a_free_slot() {
    let (r, mut w, _) = setup();
    let civ = CivId(1);
    let city = w.found_city_in(&r, City::found("Babylon", civ, w.grid.id_at(14, 5).unwrap()));
    let tile = w.cities.get(city).unwrap().centre;
    w.place_unit(Unit::new(r.units.id("Worker").unwrap(), civ, tile, 2));
    let writing = r.technologies.id("Writing").unwrap();
    w.civ_mut(civ).unwrap().techs.insert(writing);
    effects::on_technology_researched(&mut w, &r, civ, writing, Fixed::ONE);
    let scientist =
        w.units.iter().find(|(_, u)| u.owner == civ && r.units[u.base].name == "Great Scientist").unwrap().1;
    assert_ne!(scientist.position, tile);
}
#[test]
fn follower_bonuses_follow_foreign_majority_and_founder_tithe_stays_global() {
    let (r, mut w, city) = setup();
    let foreign = CivId(1);
    for name in ["Desert Folklore", "Tithe", "Cathedrals"] {
        w.civ_mut(foreign).unwrap().faith.beliefs.insert(r.beliefs.id(name).unwrap());
    }
    w.civ_mut(foreign).unwrap().faith.founded = true;
    policies::rebuild_uniques(&mut w, &r, foreign);
    let tile = w.cities.get(city).unwrap().centre;
    w.grid.terrain[tile.0 as usize] = r.terrain("Desert").unwrap();
    w.cities.get_mut(city).unwrap().population = 8;
    let before = city::city_yield_in(&w, &r, w.cities.get(city).unwrap(), false);
    let c = w.cities.get_mut(city).unwrap();
    c.faith.add_pressure(Some(foreign), 100000);
    c.faith.recount(c.population);
    let after = city::city_yield_in(&w, &r, w.cities.get(city).unwrap(), false);
    assert_eq!(after.faith - before.faith, Fixed::ONE);
    assert_eq!(civ_core::world::city_effects::founder_yields(&w, &r, foreign).gold, Fixed::whole(2));
    let cathedral = Construction::Building(r.buildings.id("Cathedral").unwrap());
    w.civ_mut(CivId(0)).unwrap().faith.stored = 1000;
    assert!(purchase::can_purchase(&w, &r, city, cathedral, Stat::Faith, CostModifiers::default()).is_ok());
    assert!(construction::can_construct_in(&w, &r, CivId(0), w.cities.get(city).unwrap(), cathedral).is_err());
    purchase::purchase(&mut w, &r, city, cathedral, Stat::Faith, CostModifiers::default()).unwrap();
    let c = w.cities.get_mut(city).unwrap();
    c.faith.remove_all_pressures_except(CivId(0));
    c.faith.add_pressure(Some(CivId(0)), 100000);
    c.faith.recount(c.population);
    assert_eq!(
        city::city_yield_in(&w, &r, w.cities.get(city).unwrap(), false).faith,
        after.faith - Fixed::ONE + Fixed::ONE
    );
}
#[test]
fn national_wonders_require_prerequisites_in_each_nonpuppet_city() {
    let (r, mut w, city) = setup();
    let civ = CivId(0);
    w.civ_mut(civ).unwrap().techs.insert(r.technologies.id("Philosophy").unwrap());
    let second = w.found_city_in(&r, City::found("Second", civ, w.grid.id_at(14, 7).unwrap()));
    let college = Construction::Building(r.buildings.id("National College").unwrap());
    w.cities.get_mut(city).unwrap().built.insert(r.buildings.id("Library").unwrap());
    assert_eq!(
        construction::can_construct_in(&w, &r, civ, w.cities.get(city).unwrap(), college),
        Err(construction::Rejection::NotAvailable)
    );
    w.cities.get_mut(second).unwrap().puppet = true;
    assert!(construction::can_construct_in(&w, &r, civ, w.cities.get(city).unwrap(), college).is_ok());
}
#[test]
fn quick_speed_golden_age_uses_its_distinct_duration_modifier() {
    let (r, mut w, _) = setup();
    effects::apply_uniques(
        &mut w,
        &r,
        CivId(0),
        None,
        &[Unique::parse("Empire enters golden age")],
        Fixed::permille(670),
    );
    assert_eq!(w.civ(CivId(0)).unwrap().golden_age.turns_left, 8);
}

#[test]
fn spaceship_limit_counts_other_queues_fielded_and_assembled_parts() {
    let (r, mut w, capital) = setup();
    let civ = CivId(0);
    for tech in r.technologies.ids() {
        w.civ_mut(civ).unwrap().techs.insert(tech);
    }
    w.cities.get_mut(capital).unwrap().built.insert(r.buildings.id("Apollo Program").unwrap());
    let second = w.found_city_in(&r, City::found("Other", civ, w.grid.id_at(14, 8).unwrap()));
    let tile = w.cities.get(second).unwrap().centre;
    let aluminum = r.resources.id("Aluminum").unwrap();
    w.grid.resource[tile.0 as usize] = Some(aluminum);
    w.grid.resource_amount[tile.0 as usize] = 6;
    let base = r.units.id("SS Cockpit").unwrap();
    let what = Construction::Unit(base);
    assert!(
        construction::can_construct_in(&w, &r, civ, w.cities.get(second).unwrap(), what).is_ok(),
        "Apollo in another city unlocks this city"
    );
    w.cities.get_mut(capital).unwrap().queue = vec![what];
    assert_eq!(
        construction::can_construct_in(&w, &r, civ, w.cities.get(second).unwrap(), what),
        Err(construction::Rejection::LimitReached)
    );
    w.cities.get_mut(capital).unwrap().queue.clear();
    let unit = w.place_unit(Unit::new(base, civ, tile, 2));
    assert_eq!(
        construction::can_construct_in(&w, &r, civ, w.cities.get(second).unwrap(), what),
        Err(construction::Rejection::LimitReached)
    );
    w.remove_unit(unit);
    w.civ_mut(civ).unwrap().victory.spaceship_parts.insert(base, 1);
    assert_eq!(
        construction::can_construct_in(&w, &r, civ, w.cities.get(second).unwrap(), what),
        Err(construction::Rejection::LimitReached)
    );
}

#[test]
fn nation_replacement_hides_original_and_rejected_production_is_atomic() {
    let (r, mut w, city) = setup();
    let civ = CivId(0);
    w.civ_mut(civ).unwrap().techs.insert(r.technologies.id("Iron Working").unwrap());
    let what = Construction::Unit(r.units.id("Swordsman").unwrap());
    assert_eq!(
        construction::can_construct_in(&w, &r, civ, w.cities.get(city).unwrap(), what),
        Err(construction::Rejection::NotAvailable)
    );
    let before = w.clone();
    assert!(commands::execute(&mut w, &r, civ, Command::SetProduction { city, queue: vec![what] }).is_err());
    assert_eq!(w, before);
}
