use civ_core::world::entities::Civilization;
use civ_core::{
    arena::Id,
    ruleset::Ruleset,
    stats::Fixed,
    world::{grid::TileGrid, movement as m, CivId, Unit, World},
};
fn fixture(nation: &str) -> (Ruleset, World) {
    let r = Ruleset::load_dir("rules/gods-and-kings").unwrap().unwrap();
    let mut w = World::new(
        TileGrid::new(8, 8, r.terrain("Grassland").unwrap()),
        vec![Civilization::new(CivId(0), r.nations.id(nation).unwrap(), false)],
    );
    civ_core::world::policies::rebuild_uniques(&mut w, &r, CivId(0));
    (r, w)
}
fn unit(w: &mut World, r: &Ruleset, name: &str) -> Id<Unit> {
    let b = r.units.id(name).unwrap();
    w.place_unit(Unit::new(b, CivId(0), w.grid.id_at(3, 3).unwrap(), r.units[b].movement))
}
fn tech(w: &mut World, r: &Ruleset, name: &str) {
    w.civ_mut(CivId(0)).unwrap().techs.insert(r.technologies.id(name).unwrap());
    civ_core::world::policies::rebuild_uniques(w, r, CivId(0));
}
#[test]
fn embarkation_obeys_coast_ocean_and_polynesian_privileges() {
    let (r, mut w) = fixture("Rome");
    let id = unit(&mut w, &r, "Warrior");
    let a = w.units.get(id).unwrap().position;
    let b = w.grid.neighbours(a).next().unwrap();
    w.grid.terrain[b.0 as usize] = r.terrain("Coast").unwrap();
    assert!(!m::can_traverse(&w, &r, w.units.get(id).unwrap(), a, b));
    tech(&mut w, &r, "Optics");
    assert!(m::can_traverse(&w, &r, w.units.get(id).unwrap(), a, b));
    w.grid.terrain[b.0 as usize] = r.terrain("Ocean").unwrap();
    assert!(!m::can_traverse(&w, &r, w.units.get(id).unwrap(), a, b));
    tech(&mut w, &r, "Astronomy");
    assert!(m::can_traverse(&w, &r, w.units.get(id).unwrap(), a, b));
    let (r, mut w) = fixture("Polynesia");
    let id = unit(&mut w, &r, "Warrior");
    let a = w.units.get(id).unwrap().position;
    let b = w.grid.neighbours(a).next().unwrap();
    w.grid.terrain[b.0 as usize] = r.terrain("Ocean").unwrap();
    assert!(m::can_traverse(&w, &r, w.units.get(id).unwrap(), a, b));
}
#[test]
fn road_upgrade_terrain_promotions_and_embarked_movement_are_live() {
    let (r, mut w) = fixture("Rome");
    let id = unit(&mut w, &r, "Warrior");
    let a = w.units.get(id).unwrap().position;
    let b = w.grid.neighbours(a).next().unwrap();
    w.grid.road[a.0 as usize] = civ_core::world::grid::RoadLevel::Road;
    w.grid.road[b.0 as usize] = civ_core::world::grid::RoadLevel::Road;
    assert_eq!(
        m::step_cost_for_unit(&w, &r, w.units.get(id).unwrap(), a, b, Fixed::whole(2)),
        Some(Fixed::from_fraction(1, 2))
    );
    tech(&mut w, &r, "Machinery");
    assert_eq!(
        m::step_cost_for_unit(&w, &r, w.units.get(id).unwrap(), a, b, Fixed::whole(2)),
        Some(Fixed::from_fraction(1, 3))
    );
    w.grid.road[b.0 as usize] = civ_core::world::grid::RoadLevel::None;
    w.grid.features[b.0 as usize].push(r.terrain("Hill").unwrap());
    w.units.get_mut(id).unwrap().promotions.insert(r.promotions.id("[Norwegian Ski Infantry] ability").unwrap());
    assert_eq!(m::step_cost_for_unit(&w, &r, w.units.get(id).unwrap(), a, b, Fixed::whole(2)), Some(Fixed::ONE));
    w.units.get_mut(id).unwrap().promotions.insert(r.promotions.id("Mobility I").unwrap());
    assert_eq!(m::max_movement(&w, &r, w.units.get(id).unwrap()), 3);
}
#[test]
fn carthage_mountain_unlock_survives_great_general_consumption() {
    let (r, mut w) = fixture("Carthage");
    let id = unit(&mut w, &r, "Warrior");
    let a = w.units.get(id).unwrap().position;
    let b = w.grid.neighbours(a).next().unwrap();
    w.grid.terrain[b.0 as usize] = r.terrain("Mountain").unwrap();
    assert!(!m::can_traverse(&w, &r, w.units.get(id).unwrap(), a, b));
    let general = unit(&mut w, &r, "Great General");
    w.remove_unit(general);
    assert!(m::can_traverse(&w, &r, w.units.get(id).unwrap(), a, b));
    m::move_unit(&mut w, &r, id, b).unwrap();
    assert_eq!(w.units.get(id).unwrap().position, b);
}

#[test]
fn astronomy_lifts_work_boat_ocean_restriction() {
    let (r, mut w) = fixture("Rome");
    let id = unit(&mut w, &r, "Work Boats");
    let a = w.units.get(id).unwrap().position;
    let b = w.grid.neighbours(a).next().unwrap();
    w.grid.terrain[a.0 as usize] = r.terrain("Coast").unwrap();
    w.grid.terrain[b.0 as usize] = r.terrain("Ocean").unwrap();
    assert!(!m::can_traverse(&w, &r, w.units.get(id).unwrap(), a, b));
    tech(&mut w, &r, "Astronomy");
    assert!(m::can_traverse(&w, &r, w.units.get(id).unwrap(), a, b));
}
#[test]
fn founding_requires_a_capable_unit_on_land_and_conquistadors_need_another_continent() {
    use civ_core::world::{settling, City};
    let (r, mut w) = fixture("Spain");
    let warrior = unit(&mut w, &r, "Warrior");
    assert!(settling::can_found_with(&w, &r, warrior).is_err());
    let settler = unit(&mut w, &r, "Settler");
    assert!(settling::can_found_with(&w, &r, settler).is_ok());
    let a = w.units.get(settler).unwrap().position;
    w.grid.terrain[a.0 as usize] = r.terrain("Coast").unwrap();
    assert!(settling::can_found_with(&w, &r, settler).is_err());
    w.grid.terrain[a.0 as usize] = r.terrain("Grassland").unwrap();
    let cap = w.grid.id_at(0, 0).unwrap();
    w.found_city_in(&r, City::found("Madrid", CivId(0), cap));
    let conquistador = unit(&mut w, &r, "Conquistador");
    assert!(settling::can_found_with(&w, &r, conquistador).is_err());
    for t in w.grid.neighbours(a).collect::<Vec<_>>() {
        w.grid.terrain[t.0 as usize] = r.terrain("Coast").unwrap();
    }
    assert!(settling::can_found_with(&w, &r, conquistador).is_ok());
    settling::found_city_with(&mut w, &r, conquistador, "Island").unwrap();
    assert!(w.units.get(conquistador).is_none());
}

#[test]
fn border_expulsion_keeps_carried_aircraft_with_their_carrier() {
    let (r, mut w) = fixture("Rome");
    w.civs.push(Civilization::new(CivId(1), r.nations.id("Greece").unwrap(), false));
    w.grid.terrain.fill(r.terrain("Coast").unwrap());
    let tile = w.grid.id_at(3, 3).unwrap();
    w.grid.owner[tile.0 as usize] = Some(CivId(1));
    let carrier = unit(&mut w, &r, "Carrier");
    let fighter = unit(&mut w, &r, "Fighter");
    w.units.get_mut(fighter).unwrap().transport = Some(carrier);
    let u = w.units.get_mut(carrier).unwrap();
    u.fortified = true;
    u.fortification_turns = 2;
    u.set_up = true;
    m::expel(&mut w, &r, CivId(0), CivId(1));
    let u = w.units.get(carrier).unwrap();
    assert_ne!(u.position, tile);
    assert_eq!(w.units.get(fighter).unwrap().position, u.position);
    assert_eq!(w.units.get(fighter).unwrap().transport, Some(carrier));
    assert!(!u.fortified);
    assert_eq!(u.fortification_turns, 0);
    assert!(!u.set_up);
}

#[test]
fn traded_city_evicts_foreign_garrison_even_with_open_borders_but_preserves_condition() {
    use civ_core::world::City;
    let (r, mut w) = fixture("Rome");
    w.civs.push(Civilization::new(CivId(1), r.nations.id("Greece").unwrap(), false));
    let garrison = unit(&mut w, &r, "Warrior");
    let origin = w.units.get(garrison).unwrap().position;
    w.found_city_in(&r, City::found("Transferred", CivId(1), origin));
    w.diplomacy.set_open_borders(CivId(1), CivId(0), true);
    let u = w.units.get_mut(garrison).unwrap();
    u.health = 43;
    u.movement_left = Fixed::from_fraction(1, 2);
    u.moved_this_turn = true;
    u.attacks_this_turn = 1;
    u.fortified = true;
    u.fortification_turns = 2;
    assert!(m::displace_unit(&mut w, &r, garrison));
    let u = w.units.get(garrison).unwrap();
    assert_ne!(u.position, origin);
    assert_eq!(
        (u.health, u.movement_left, u.moved_this_turn, u.attacks_this_turn),
        (43, Fixed::from_fraction(1, 2), true, 1)
    );
    assert!(!u.fortified);
    assert_eq!(u.fortification_turns, 0);

    let base = r.units.id("Warrior").unwrap();
    let receiver = w.place_unit(Unit::new(base, CivId(1), origin, 2));
    let before = w.units.get(receiver).unwrap().clone();
    assert!(!m::displace_unit(&mut w, &r, receiver));
    assert_eq!(w.units.get(receiver), Some(&before));
}

#[test]
fn displacement_can_cross_closed_borders_but_not_impassable_terrain() {
    use civ_core::world::City;
    let (r, mut w) = fixture("Rome");
    w.civs.push(Civilization::new(CivId(1), r.nations.id("Greece").unwrap(), false));
    let id = unit(&mut w, &r, "Warrior");
    let origin = w.units.get(id).unwrap().position;
    w.grid.owner.fill(Some(CivId(1)));
    let safe = w.grid.id_at(3, 0).unwrap();
    w.grid.owner[safe.0 as usize] = None;
    assert!(m::displace_unit(&mut w, &r, id));
    assert_eq!(w.units.get(id).unwrap().position, safe);

    w.units.get_mut(id).unwrap().position = origin;
    for tile in w.grid.neighbours(origin).collect::<Vec<_>>() {
        w.grid.terrain[tile.0 as usize] = r.terrain("Mountain").unwrap();
    }
    // The only close safe tile is now unreachable. An owned city still
    // provides a source-compatible fallback even on another landmass.
    let home = w.grid.id_at(0, 0).unwrap();
    w.found_city_in(&r, City::found("Home", CivId(0), home));
    assert!(m::displace_unit(&mut w, &r, id));
    assert!(w.cities.iter().any(|(_, city)| city.tiles.contains(&w.units.get(id).unwrap().position)));
}

#[test]
fn displaced_aircraft_need_owned_base_capacity_and_stranded_units_are_removed() {
    use civ_core::world::City;
    let (r, mut w) = fixture("Rome");
    w.civs.push(Civilization::new(CivId(1), r.nations.id("Greece").unwrap(), false));
    let air = unit(&mut w, &r, "Fighter");
    let origin = w.units.get(air).unwrap().position;
    w.found_city_in(&r, City::found("Transferred", CivId(1), origin));
    let home = w.grid.id_at(0, 0).unwrap();
    w.found_city_in(&r, City::found("Home", CivId(0), home));
    let base = r.units.id("Fighter").unwrap();
    for _ in 0..5 {
        w.place_unit(Unit::new(base, CivId(0), home, 2));
    }
    w.units.get_mut(air).unwrap().movement_left = Fixed::ZERO;
    assert!(m::displace_unit(&mut w, &r, air));
    assert_eq!(w.units.get(air).unwrap().position, home);
    assert_eq!(w.units.get(air).unwrap().movement_left, Fixed::ZERO);
    let excess = w.place_unit(Unit::new(base, CivId(0), origin, 2));
    assert!(!m::displace_unit(&mut w, &r, excess));
    assert!(w.units.get(excess).is_none());
    assert!(!w.civ(CivId(0)).unwrap().units.contains(&excess));
}

#[test]
fn stranded_carrier_removal_also_removes_payload() {
    let (r, mut w) = fixture("Rome");
    w.civs.push(Civilization::new(CivId(1), r.nations.id("Greece").unwrap(), false));
    w.grid.terrain.fill(r.terrain("Coast").unwrap());
    w.grid.owner.fill(Some(CivId(1)));
    let carrier = unit(&mut w, &r, "Carrier");
    let fighter = unit(&mut w, &r, "Fighter");
    w.units.get_mut(fighter).unwrap().transport = Some(carrier);
    assert!(!m::displace_unit(&mut w, &r, fighter));
    assert!(w.units.get(fighter).is_some());
    assert!(!m::displace_unit(&mut w, &r, carrier));
    assert!(w.units.get(carrier).is_none());
    assert!(w.units.get(fighter).is_none());
}

#[test]
fn displacement_resolves_destination_camp_and_observes_the_new_site() {
    let (r, mut w) = fixture("Rome");
    w.civs.push(Civilization::new(CivId(1), r.nations.id("Greece").unwrap(), false));
    let id = unit(&mut w, &r, "Warrior");
    w.grid.owner.fill(Some(CivId(1)));
    let safe = w.grid.id_at(3, 0).unwrap();
    w.grid.owner[safe.0 as usize] = None;
    w.grid.improvement[safe.0 as usize] = r.improvements.id("Barbarian encampment");
    assert!(w.grid.improvement[safe.0 as usize].is_some());
    let before = w.civ(CivId(0)).unwrap().gold;
    assert!(m::displace_unit(&mut w, &r, id));
    assert_eq!(w.units.get(id).unwrap().position, safe);
    assert!(w.grid.improvement[safe.0 as usize].is_none());
    assert!(w.civ(CivId(0)).unwrap().gold > before);
    assert!(w.visibility.can_see(CivId(0), safe));
}

#[test]
fn oasis_restricts_worker_and_great_person_improvements_to_roads() {
    use civ_core::world::improvements;
    let (r, mut w) = fixture("Rome");
    let tile = w.grid.id_at(3, 3).unwrap();
    w.grid.terrain[tile.0 as usize] = r.terrain("Desert").unwrap();
    w.grid.features[tile.0 as usize].push(r.terrain("Oasis").unwrap());
    assert!(improvements::terrain_allows(&w, &r, CivId(0), tile, r.improvements.id("Road").unwrap()));
    assert!(!improvements::terrain_allows(&w, &r, CivId(0), tile, r.improvements.id("Farm").unwrap()));
    assert!(!improvements::terrain_allows(&w, &r, CivId(0), tile, r.improvements.id("Academy").unwrap()));
}
