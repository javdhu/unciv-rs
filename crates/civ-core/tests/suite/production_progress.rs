// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use civ_core::arena::Id;
use civ_core::ruleset::Ruleset;
use civ_core::stats::Stat;
use civ_core::world::commands::{execute, Command};
use civ_core::world::construction::{
    add_production_points, apply_production_in, production_cost_in, revalidate_production, Construction,
    ConstructionEvent, CostModifiers,
};
use civ_core::world::entities::Unit;
use civ_core::world::{City, CivId, Civilization, TileGrid, World};

fn fixture() -> (Ruleset, World, Id<City>) {
    let r = Ruleset::load_dir("rules/gods-and-kings").unwrap().unwrap();
    let nation = r.nations.id("Rome").unwrap();
    let mut w = World::new(
        TileGrid::new(12, 12, r.terrain("Grassland").unwrap()),
        vec![Civilization::new(CivId(0), nation, false), Civilization::new(CivId(1), nation, false)],
    );
    w.civ_mut(CivId(0)).unwrap().techs.insert(r.technologies.id("Agriculture").unwrap());
    let centre = w.grid.id_at(5, 5).unwrap();
    let city = w.found_city_in(&r, City::found("Home", CivId(0), centre));
    (r, w, city)
}

fn queue(w: &mut World, r: &Ruleset, city: Id<City>, items: Vec<Construction>) {
    execute(w, r, CivId(0), Command::SetProduction { city, queue: items }).unwrap();
}

fn produce(w: &mut World, r: &Ruleset, city: Id<City>, amount: i32) -> Vec<ConstructionEvent> {
    apply_production_in(w, r, city, amount, CostModifiers::default()).unwrap()
}

#[test]
fn switching_removing_and_restoring_items_keeps_separate_investments() {
    let (r, mut w, city) = fixture();
    let monument = Construction::Building(r.building("Monument").unwrap());
    let warrior = Construction::Unit(r.unit("Warrior").unwrap());
    queue(&mut w, &r, city, vec![monument]);
    produce(&mut w, &r, city, 11);
    queue(&mut w, &r, city, vec![warrior, monument]);
    produce(&mut w, &r, city, 7);
    queue(&mut w, &r, city, vec![]);
    produce(&mut w, &r, city, 100);
    queue(&mut w, &r, city, vec![monument]);
    let c = w.cities.get(city).unwrap();
    assert_eq!(c.current_production(), 11);
    assert_eq!(c.production_for(warrior), 7);
    assert_eq!(c.production_overflow, 0);
    produce(&mut w, &r, city, 3);
    assert_eq!(w.cities.get(city).unwrap().production_for(monument), 14);
    queue(&mut w, &r, city, vec![warrior]);
    assert_eq!(w.cities.get(city).unwrap().current_production(), 7);
}

#[test]
fn overflow_is_capped_and_repeated_units_start_a_fresh_investment() {
    let (r, mut w, city) = fixture();
    let warrior = Construction::Unit(r.unit("Warrior").unwrap());
    queue(&mut w, &r, city, vec![warrior, warrior]);
    add_production_points(w.cities.get_mut(city).unwrap(), 500);
    let events = produce(&mut w, &r, city, 10);
    assert!(events.contains(&ConstructionEvent::Completed { what: warrior, carried_over: 40 }));
    let c = w.cities.get(city).unwrap();
    assert_eq!(c.queue, [warrior]);
    assert_eq!(c.current_production(), 0);
    assert!(c.production_progress.is_empty());
    assert_eq!(w.units.len(), 1);
    // Overflow is spent on the next item on the next production pass only.
    produce(&mut w, &r, city, 0);
    assert_eq!(w.units.len(), 2);
    assert!(w.cities.get(city).unwrap().queue.is_empty());
    assert_eq!(w.cities.get(city).unwrap().production_overflow, 0);
    let positions: std::collections::BTreeSet<_> = w.units.iter().map(|(_, u)| u.position).collect();
    assert_eq!(positions.len(), 2, "new units must occupy separate military slots");
}

#[test]
fn ordinary_idle_income_is_discarded_but_explicit_grants_survive() {
    let (r, mut w, city) = fixture();
    add_production_points(w.cities.get_mut(city).unwrap(), 9);
    assert_eq!(produce(&mut w, &r, city, 50), [ConstructionEvent::Idle { stored: 9 }]);
    let monument = Construction::Building(r.building("Monument").unwrap());
    queue(&mut w, &r, city, vec![monument]);
    produce(&mut w, &r, city, 2);
    let c = w.cities.get(city).unwrap();
    assert_eq!(c.current_production(), 11);
    assert_eq!(c.production_overflow, 0);
}

#[test]
fn a_lost_world_wonder_refunds_paused_work_exactly_once() {
    let (r, mut w, city) = fixture();
    let wonder_id = r.building("The Pyramids").unwrap();
    let wonder = Construction::Building(wonder_id);
    let monument = Construction::Building(r.building("Monument").unwrap());
    let centre = w.grid.id_at(10, 10).unwrap();
    let foreign = w.found_city_in(&r, City::found("Other", CivId(1), centre));
    let c = w.cities.get_mut(city).unwrap();
    c.queue.push(monument);
    c.production_progress.insert(wonder, 23);
    c.production_progress.insert(monument, 8);
    c.production_overflow = 5;
    w.civ_mut(CivId(0)).unwrap().techs.insert(r.technologies.id("Masonry").unwrap());
    w.cities.get_mut(foreign).unwrap().built.insert(wonder_id);
    assert_eq!(revalidate_production(&mut w, &r, city), [ConstructionEvent::Cancelled { what: wonder, gold: 23 }]);
    assert_eq!(w.civ(CivId(0)).unwrap().gold, 23);
    assert!(revalidate_production(&mut w, &r, city).is_empty());
    let c = w.cities.get(city).unwrap();
    assert_eq!(c.production_for(wonder), 0);
    assert_eq!(c.current_production(), 8);
    assert_eq!(c.production_overflow, 5);
    assert_eq!(w.civ(CivId(0)).unwrap().gold, 23);
}

#[test]
fn temporary_resource_loss_preserves_work_for_a_later_order() {
    let (r, mut w, city) = fixture();
    let legion = Construction::Unit(r.unit("Legion").unwrap());
    for name in ["Iron Working", "Mining", "Bronze Working"] {
        w.civ_mut(CivId(0)).unwrap().techs.insert(r.technologies.id(name).unwrap());
    }
    let centre = w.cities.get(city).unwrap().centre;
    w.grid.resource[centre.0 as usize] = Some(r.resources.id("Iron").unwrap());
    w.grid.resource_amount[centre.0 as usize] = 2;
    queue(&mut w, &r, city, vec![legion]);
    produce(&mut w, &r, city, 17);
    w.grid.resource_amount[centre.0 as usize] = 0;
    assert_eq!(revalidate_production(&mut w, &r, city), [ConstructionEvent::Blocked { what: legion }]);
    let c = w.cities.get(city).unwrap();
    assert!(c.queue.is_empty());
    assert_eq!(c.production_for(legion), 17);
    w.grid.resource_amount[centre.0 as usize] = 2;
    queue(&mut w, &r, city, vec![legion]);
    assert_eq!(w.cities.get(city).unwrap().current_production(), 17);
}

#[test]
fn obsolete_work_merges_into_the_available_national_upgrade() {
    let (r, mut w, city) = fixture();
    let warrior = Construction::Unit(r.unit("Warrior").unwrap());
    let legion = Construction::Unit(r.unit("Legion").unwrap());
    queue(&mut w, &r, city, vec![warrior]);
    produce(&mut w, &r, city, 17);
    w.cities.get_mut(city).unwrap().production_progress.insert(legion, 9);
    for name in ["Iron Working", "Metal Casting", "Mining", "Bronze Working"] {
        w.civ_mut(CivId(0)).unwrap().techs.insert(r.technologies.id(name).unwrap());
    }
    let centre = w.cities.get(city).unwrap().centre;
    w.grid.resource[centre.0 as usize] = Some(r.resources.id("Iron").unwrap());
    w.grid.resource_amount[centre.0 as usize] = 2;
    assert_eq!(
        revalidate_production(&mut w, &r, city),
        [ConstructionEvent::Transferred { from: warrior, to: legion, production: 17 }]
    );
    let c = w.cities.get(city).unwrap();
    assert_eq!(c.queue, [legion]);
    assert_eq!(c.current_production(), 26);
    assert_eq!(c.production_for(warrior), 0);
}

#[test]
fn obsolescence_transfers_only_if_the_original_item_has_no_other_rejection() {
    let (r, mut w, city) = fixture();
    let old = Construction::Unit(r.unit("Longswordsman").unwrap());
    let newer = Construction::Unit(r.unit("Musketman").unwrap());
    for name in ["Steel", "Gunpowder"] {
        w.civ_mut(CivId(0)).unwrap().techs.insert(r.technologies.id(name).unwrap());
    }
    w.cities.get_mut(city).unwrap().production_progress.insert(old, 30);
    // Musketmen need no Iron, but lost Iron is an independent rejection of
    // the old construction. Unciv transfers only pure obsolescence.
    assert_eq!(revalidate_production(&mut w, &r, city), [ConstructionEvent::Cancelled { what: old, gold: 0 }]);
    assert_eq!(w.cities.get(city).unwrap().production_for(newer), 0);
    assert!(w.cities.get(city).unwrap().production_progress.is_empty());
}

#[test]
fn permanent_wonder_loss_is_not_hidden_by_a_missing_prerequisite() {
    let (r, mut w, city) = fixture();
    let wonder = r.building("The Pyramids").unwrap();
    let centre = w.grid.id_at(10, 10).unwrap();
    let foreign = w.found_city_in(&r, City::found("Winner", CivId(1), centre));
    w.cities.get_mut(foreign).unwrap().built.insert(wonder);
    let what = Construction::Building(wonder);
    w.cities.get_mut(city).unwrap().production_progress.insert(what, 25);
    // The recipient of a captured city can lack Masonry and still be due
    // compensation for a wonder that permanently became unavailable.
    assert!(!w.civ(CivId(0)).unwrap().has_tech(r.technologies.id("Masonry").unwrap()));
    assert_eq!(revalidate_production(&mut w, &r, city), [ConstructionEvent::Cancelled { what, gold: 25 }]);
    assert_eq!(w.civ(CivId(0)).unwrap().gold, 25);
}

#[test]
fn obsolete_buildings_discard_only_their_own_work() {
    let (mut r, mut w, city) = fixture();
    let monument = r.building("Monument").unwrap();
    r.buildings.get_mut(monument).unwrap().uniques.push(civ_core::Unique::parse("Obsolete with [Writing]"));
    let what = Construction::Building(monument);
    queue(&mut w, &r, city, vec![what]);
    produce(&mut w, &r, city, 12);
    w.cities.get_mut(city).unwrap().production_overflow = 3;
    w.civ_mut(CivId(0)).unwrap().techs.insert(r.technologies.id("Writing").unwrap());
    assert_eq!(revalidate_production(&mut w, &r, city), [ConstructionEvent::Cancelled { what, gold: 0 }]);
    let c = w.cities.get(city).unwrap();
    assert!(c.queue.is_empty());
    assert!(c.production_progress.is_empty());
    assert_eq!(c.production_overflow, 3);
}

#[test]
fn purchasing_a_partial_unit_preserves_its_work_and_overflow() {
    let (r, mut w, city) = fixture();
    let warrior = Construction::Unit(r.unit("Warrior").unwrap());
    queue(&mut w, &r, city, vec![warrior]);
    produce(&mut w, &r, city, 17);
    w.cities.get_mut(city).unwrap().production_overflow = 3;
    w.civ_mut(CivId(0)).unwrap().gold = 10_000;
    execute(&mut w, &r, CivId(0), Command::Purchase { city, what: warrior, currency: Stat::Gold }).unwrap();
    let c = w.cities.get(city).unwrap();
    assert!(c.queue.is_empty());
    assert_eq!(c.production_for(warrior), 17);
    assert_eq!(c.production_overflow, 3);
    assert_eq!(w.units.len(), 1);
    queue(&mut w, &r, city, vec![warrior]);
    produce(&mut w, &r, city, 20);
    assert_eq!(w.units.len(), 2);
    assert!(w.cities.get(city).unwrap().production_progress.is_empty());
}

#[test]
fn blocked_placement_keeps_completed_work_until_a_legal_slot_opens() {
    let (r, mut w, city) = fixture();
    let base = r.unit("Warrior").unwrap();
    let what = Construction::Unit(base);
    let centre = w.cities.get(city).unwrap().centre;
    let existing = w.place_unit(Unit::new(base, CivId(0), centre, 2));
    let neighbors: Vec<_> = w.grid.neighbours(centre).collect();
    for &tile in &neighbors {
        w.grid.terrain[tile.0 as usize] = r.terrain("Mountain").unwrap();
    }
    queue(&mut w, &r, city, vec![what]);
    assert_eq!(produce(&mut w, &r, city, 40), [ConstructionEvent::Blocked { what }]);
    assert_eq!(w.units.len(), 1);
    assert_eq!(w.cities.get(city).unwrap().current_production(), 40);
    assert_eq!(w.cities.get(city).unwrap().queue, [what]);
    w.grid.terrain[neighbors[0].0 as usize] = r.terrain("Grassland").unwrap();
    produce(&mut w, &r, city, 0);
    assert_eq!(w.units.len(), 2);
    assert_eq!(w.units.get(existing).unwrap().position, centre);
    assert_eq!(w.units.iter().find(|(id, _)| *id != existing).unwrap().1.position, neighbors[0]);
    assert!(w.cities.get(city).unwrap().production_progress.is_empty());
}

#[test]
fn naval_production_uses_an_adjacent_water_slot_when_the_city_is_occupied() {
    let (r, mut w, city) = fixture();
    let centre = w.cities.get(city).unwrap().centre;
    let coast = w.grid.neighbours(centre).next().unwrap();
    w.grid.terrain[coast.0 as usize] = r.terrain("Coast").unwrap();
    w.cities.get_mut(city).unwrap().coastal = true;
    w.civ_mut(CivId(0)).unwrap().techs.insert(r.technologies.id("Sailing").unwrap());
    w.place_unit(Unit::new(r.unit("Warrior").unwrap(), CivId(0), centre, 2));
    let trireme = r.unit("Trireme").unwrap();
    let what = Construction::Unit(trireme);
    queue(&mut w, &r, city, vec![what]);
    let cost = production_cost_in(&w, &r, city, what, CostModifiers::default()).unwrap();
    produce(&mut w, &r, city, cost);
    assert_eq!(w.units.iter().find(|(_, u)| u.base == trireme).unwrap().1.position, coast);
}

#[test]
fn aircraft_wait_for_capacity_or_use_a_nearby_friendly_city() {
    let (r, mut w, city) = fixture();
    let base = r.unit("Triplane").unwrap();
    let what = Construction::Unit(base);
    let centre = w.cities.get(city).unwrap().centre;
    for name in ["Flight", "Biology"] {
        w.civ_mut(CivId(0)).unwrap().techs.insert(r.technologies.id(name).unwrap());
    }
    w.grid.resource[centre.0 as usize] = Some(r.resources.id("Oil").unwrap());
    w.grid.resource_amount[centre.0 as usize] = 10;
    for _ in 0..6 {
        w.place_unit(Unit::new(base, CivId(0), centre, r.units.get(base).unwrap().movement));
    }
    queue(&mut w, &r, city, vec![what]);
    let cost = production_cost_in(&w, &r, city, what, CostModifiers::default()).unwrap();
    assert_eq!(produce(&mut w, &r, city, cost), [ConstructionEvent::Blocked { what }]);
    assert_eq!(w.units.len(), 6);
    assert_eq!(w.cities.get(city).unwrap().current_production(), cost);
    let second = w.grid.id_at(9, 5).unwrap();
    w.found_city_in(&r, City::found("Airfield", CivId(0), second));
    assert!(produce(&mut w, &r, city, 0).iter().any(|e| matches!(e, ConstructionEvent::Completed { .. })));
    assert_eq!(w.units.len(), 7);
    assert_eq!(w.units.iter().filter(|(_, u)| u.position == second).count(), 1);
    assert!(w.cities.get(city).unwrap().production_progress.is_empty());
}

#[test]
fn an_engineer_uses_the_active_item_and_live_empire_cost() {
    use civ_core::world::great_person_actions::{perform, Action, Outcome, Scaling};
    let (mut r, mut w, city) = fixture();
    let monument = r.building("Monument").unwrap();
    let building = r.buildings.get_mut(monument).unwrap();
    building.cost = Some(100);
    building.uniques.push(civ_core::Unique::parse("Cost increases by [30] per owned city"));
    let what = Construction::Building(monument);
    let paused = Construction::Unit(r.unit("Warrior").unwrap());
    queue(&mut w, &r, city, vec![what]);
    produce(&mut w, &r, city, 10);
    w.cities.get_mut(city).unwrap().production_progress.insert(paused, 22);
    let second = w.grid.id_at(9, 5).unwrap();
    w.found_city_in(&r, City::found("Expansion", CivId(0), second));
    let centre = w.cities.get(city).unwrap().centre;
    let engineer = w.place_unit(Unit::new(r.unit("Great Engineer").unwrap(), CivId(0), centre, 2));
    assert_eq!(
        perform(&mut w, &r, engineer, Action::HurryProduction, Scaling::default()).unwrap(),
        Outcome::Production { city, hammers: 149 }
    );
    let c = w.cities.get(city).unwrap();
    assert_eq!(c.current_production(), 159);
    assert_eq!(c.production_for(paused), 22);
    assert_eq!(c.production_overflow, 0);
}

#[test]
fn conquest_retains_the_citys_investment_but_clears_orders() {
    let (r, mut w, city) = fixture();
    let what = Construction::Building(r.building("Monument").unwrap());
    queue(&mut w, &r, city, vec![what]);
    produce(&mut w, &r, city, 12);
    w.cities.get_mut(city).unwrap().production_overflow = 4;
    civ_core::world::conquest::conquer(&mut w, &r, city, CivId(1)).unwrap();
    let c = w.cities.get(city).unwrap();
    assert!(c.queue.is_empty());
    assert_eq!(c.production_for(what), 12);
    assert_eq!(c.production_overflow, 4);
}

#[cfg(feature = "save")]
#[test]
fn paused_items_and_overflow_survive_a_complete_save_roundtrip() {
    let (r, mut w, city) = fixture();
    let monument = Construction::Building(r.building("Monument").unwrap());
    let warrior = Construction::Unit(r.unit("Warrior").unwrap());
    queue(&mut w, &r, city, vec![monument]);
    produce(&mut w, &r, city, 11);
    queue(&mut w, &r, city, vec![warrior]);
    produce(&mut w, &r, city, 7);
    w.cities.get_mut(city).unwrap().production_overflow = 3;
    let data = civ_core::save::encode(&w, &r).unwrap();
    let mut restored = civ_core::save::decode(&data, &r).unwrap();
    assert_eq!(restored, w);
    queue(&mut w, &r, city, vec![monument]);
    queue(&mut restored, &r, city, vec![monument]);
    assert_eq!(produce(&mut w, &r, city, 4), produce(&mut restored, &r, city, 4));
    assert_eq!(restored, w);
    assert_eq!(restored.cities.get(city).unwrap().current_production(), 18);
}

#[cfg(feature = "save")]
#[test]
fn invalid_investment_counters_cannot_be_saved() {
    let (r, w, city) = fixture();
    let what = Construction::Unit(r.unit("Warrior").unwrap());
    for amount in [-1, 0, 1_000_000_001] {
        let mut malformed = w.clone();
        malformed.cities.get_mut(city).unwrap().production_progress.insert(what, amount);
        assert!(matches!(civ_core::save::encode(&malformed, &r), Err(civ_core::save::SaveError::InvalidWorld(_))));
    }
    let mut malformed = w;
    malformed.cities.get_mut(city).unwrap().production_overflow = -1;
    assert!(matches!(civ_core::save::encode(&malformed, &r), Err(civ_core::save::SaveError::InvalidWorld(_))));
}
