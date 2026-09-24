//! Wonders stand on a tile of their city, and some change the map
//! (docs/CONTENT.md, "Wonders and buildings change the map";
//! `world::placement`).
//!
//! The gods-and-kings fixture, with the placement and effect uniques
//! `rules/base` writes added to a few of its wonders in code.

use civ_core::arena::Id;
use civ_core::ruleset::{Building, Ruleset};
use civ_core::world::city::City;
use civ_core::world::commands::{execute, Command};
use civ_core::world::construction::{can_construct_in, Construction, Rejection};
use civ_core::world::entities::{Civilization, Unit};
use civ_core::world::grid::{CivId, TileGrid, TileId};
use civ_core::world::placement;
use civ_core::world::world::World;
use civ_core::Unique;

const ROME: CivId = CivId(0);

fn add(r: &mut Ruleset, building: &str, uniques: &[&str]) -> Id<Building> {
    let id = r.buildings.id(building).unwrap();
    let b = r.buildings.get_mut(id).unwrap();
    b.uniques.extend(uniques.iter().map(|u| Unique::parse(u)));
    id
}

/// A new wonder, copied from the Colossus so it has a cost and a tech.
fn new_wonder(r: &mut Ruleset, name: &str, uniques: &[&str]) -> Id<Building> {
    let mut b = r.buildings.by_name("Colossus").unwrap().clone();
    b.name = name.into();
    b.uniques = uniques.iter().map(|u| Unique::parse(u)).collect();
    r.buildings.insert(name, b)
}

fn rules() -> Ruleset {
    let mut r = Ruleset::load_dir("rules/gods-and-kings").unwrap().unwrap();
    add(&mut r, "The Pyramids", &["Stands on a [Desert] tile", "Stands on a [Flood plains] tile"]);
    add(&mut r, "Machu Picchu", &["Stands next to a [Mountain] tile"]);
    // Its empire-wide speed becomes a local one.
    let lighthouse = r.buildings.id("The Great Lighthouse").unwrap();
    r.buildings
        .get_mut(lighthouse)
        .unwrap()
        .uniques
        .retain(|u| u.unique_type() != Some(civ_core::UniqueType::Movement));
    add(
        &mut r,
        "The Great Lighthouse",
        &[
            "Stands next to a [Coast] tile",
            "[+1] Movement <for [Water] units> <within [2] tiles of a [The Great Lighthouse]>",
        ],
    );
    add(&mut r, "Great Wall", &["Stands on a [Land] tile"]);
    new_wonder(&mut r, "Suez Canal", &["Stands on a [Isthmus] tile"]);
    new_wonder(
        &mut r,
        "Grand Canal",
        &["Stands on a [Land] tile", "[+2 Gold] from each Trade Route <within [4] tiles of a [Grand Canal]>"],
    );
    r
}

fn terrain(w: &mut World, r: &Ruleset, x: u16, y: u16, name: &str) -> TileId {
    let t = w.grid.id_at(x, y).unwrap();
    w.grid.terrain[t.0 as usize] = r.terrain(name).unwrap();
    t
}

/// Rome at (6, 6) on grassland, holding every tile within three.
fn world(r: &Ruleset) -> (World, Id<City>) {
    let rome = Civilization::new(ROME, r.nations.id("Rome").unwrap(), false);
    let mut w = World::new(TileGrid::new(20, 14, r.terrain("Grassland").unwrap()), vec![rome]);
    let centre = w.grid.id_at(6, 6).unwrap();
    let city = w.found_city_in(r, City::found("Rome", ROME, centre));
    claim(&mut w, city, centre);
    (w, city)
}

fn claim(w: &mut World, city: Id<City>, centre: TileId) {
    for t in w.grid.within(centre, 3) {
        w.claim_tile(city, t);
    }
}

fn build(w: &mut World, r: &Ruleset, city: Id<City>, building: Id<Building>) {
    w.cities.get_mut(city).unwrap().built.insert(building);
    placement::settle(w, r);
}

#[test]
fn a_wonder_needs_a_tile_its_rule_allows() {
    let r = rules();
    let (mut w, city) = world(&r);
    let pyramids = r.building("The Pyramids").unwrap();
    w.civ_mut(ROME).unwrap().techs.insert(r.technologies.id("Masonry").unwrap());
    let c = w.cities.get(city).unwrap().clone();
    assert_eq!(
        can_construct_in(&w, &r, ROME, &c, Construction::Building(pyramids)),
        Err(Rejection::TerrainRequirement)
    );
    assert!(placement::valid_sites(&w, &r, city, pyramids).is_empty());

    let desert = terrain(&mut w, &r, 8, 6, "Desert");
    assert_eq!(placement::valid_sites(&w, &r, city, pyramids), [desert]);
    let c = w.cities.get(city).unwrap().clone();
    assert_eq!(can_construct_in(&w, &r, ROME, &c, Construction::Building(pyramids)), Ok(()));
    // Any one of its "Stands on" rules will do: a flood plain on grassland.
    let flood = w.grid.id_at(5, 5).unwrap();
    w.grid.features[flood.0 as usize].push(r.terrain("Flood plains").unwrap());
    assert!(placement::site_allowed(&w, &r, city, pyramids, flood));
    // Not the centre, not beyond the work radius, not unowned.
    let far = terrain(&mut w, &r, 12, 6, "Desert");
    assert!(!placement::site_allowed(&w, &r, city, pyramids, far));
    assert!(!placement::site_allowed(&w, &r, city, pyramids, w.cities.get(city).unwrap().centre));
}

#[test]
fn the_best_site_costs_the_least_and_the_wonder_takes_it() {
    let r = rules();
    let (mut w, city) = world(&r);
    let pyramids = r.building("The Pyramids").unwrap();
    let rich = terrain(&mut w, &r, 7, 6, "Desert");
    w.grid.resource[rich.0 as usize] = r.resources.id("Gold Ore");
    let plain = terrain(&mut w, &r, 8, 7, "Desert");
    w.grid.improvement[plain.0 as usize] = r.improvements.id("Trading post");
    let farm = w.grid.id_at(5, 6).unwrap();
    w.cities.get_mut(city).unwrap().worked = vec![plain, farm];
    assert_eq!(placement::best_site(&w, &r, city, pyramids), Some(plain), "the tile without a resource");

    build(&mut w, &r, city, pyramids);
    let placed = w.grid.placed_at(plain).expect("the pyramids stand on it");
    assert!(placed.building == pyramids && placed.takes_tile && !placed.canal);
    assert_eq!(placement::standing(&w, city, pyramids), Some(plain));
    assert_eq!(w.grid.improvement[plain.0 as usize], None, "its improvement is gone");
    let c = w.cities.get(city).unwrap();
    assert_eq!(c.worked, [farm], "nobody works it any more");
    assert!(!civ_core::world::city::workable_tiles(&w.grid, c).contains(&plain));
    // The building's name is the tile it stands on.
    assert_eq!(
        civ_core::world::conditionals::tile_matches_in(&w.grid, &r, plain, "The Pyramids", Some(ROME)),
        civ_core::world::conditionals::Verdict::True
    );
}

#[test]
fn a_player_chooses_the_site() {
    let r = rules();
    let (mut w, city) = world(&r);
    let pyramids = r.building("The Pyramids").unwrap();
    let a = terrain(&mut w, &r, 7, 6, "Desert");
    let b = terrain(&mut w, &r, 5, 7, "Desert");
    let grass = w.grid.id_at(6, 8).unwrap();
    let choose = |w: &mut World, tile| execute(w, &r, ROME, Command::ChooseSite { city, building: pyramids, tile });
    assert!(choose(&mut w, grass).is_err(), "not desert");
    choose(&mut w, b).unwrap();
    assert_eq!(placement::site_for(&w, &r, city, pyramids), Some(b));
    build(&mut w, &r, city, pyramids);
    assert_eq!(placement::standing(&w, city, pyramids), Some(b));
    assert!(w.grid.placed_at(a).is_none());
    assert!(w.cities.get(city).unwrap().sites.is_empty());
    assert!(choose(&mut w, a).is_err(), "already built");
}

#[test]
fn machu_picchu_stands_next_to_a_mountain() {
    let r = rules();
    let (mut w, city) = world(&r);
    let machu = r.building("Machu Picchu").unwrap();
    assert!(placement::valid_sites(&w, &r, city, machu).is_empty());
    let mountain = terrain(&mut w, &r, 9, 6, "Mountain");
    let sites = placement::valid_sites(&w, &r, city, machu);
    assert!(!sites.is_empty());
    assert!(sites.iter().all(|t| w.grid.neighbours(*t).any(|n| n == mountain)));
}

/// A land strip one tile wide down column 10 splits two seas.
fn strait(r: &Ruleset) -> (World, Id<City>, TileId) {
    let rome = Civilization::new(ROME, r.nations.id("Rome").unwrap(), false);
    let mut w = World::new(TileGrid::new(20, 14, r.terrain("Coast").unwrap()), vec![rome]);
    for y in 0..14 {
        terrain(&mut w, r, 10, y, "Grassland");
    }
    let centre = w.grid.id_at(10, 4).unwrap();
    let city = w.found_city_in(r, City::found("Rome", ROME, centre));
    claim(&mut w, city, centre);
    let neck = w.grid.id_at(10, 6).unwrap();
    (w, city, neck)
}

#[test]
fn ships_pass_through_a_canal_on_an_isthmus() {
    let r = rules();
    let (mut w, city, neck) = strait(&r);
    assert!(w.grid.is_isthmus(&r, neck));
    assert!(!w.grid.is_isthmus(&r, w.grid.id_at(3, 3).unwrap()), "open sea is no isthmus");
    let suez = r.building("Suez Canal").unwrap();
    assert!(placement::valid_sites(&w, &r, city, suez).contains(&neck));

    let west = w.grid.id_at(9, 6).unwrap();
    let east = w.grid.id_at(11, 6).unwrap();
    let trireme = Unit::new(r.unit("Trireme").unwrap(), ROME, west, 2);
    assert!(!civ_core::world::movement::can_traverse(&w, &r, &trireme, west, neck), "no canal yet");

    w.cities.get_mut(city).unwrap().sites.insert(suez, neck);
    build(&mut w, &r, city, suez);
    assert!(w.grid.is_canal(neck));
    assert!(civ_core::world::movement::can_traverse(&w, &r, &trireme, west, neck));
    assert!(civ_core::world::movement::can_traverse(&w, &r, &trireme, neck, east));
}

#[test]
fn a_coastal_wonder_speeds_nearby_ships() {
    let r = rules();
    let (mut w, city, _) = strait(&r);
    let lighthouse = r.building("The Great Lighthouse").unwrap();
    let site = w.grid.id_at(10, 2).unwrap();
    w.cities.get_mut(city).unwrap().sites.insert(lighthouse, site);
    let near = w.units.insert(Unit::new(r.unit("Trireme").unwrap(), ROME, w.grid.id_at(9, 2).unwrap(), 2));
    let far = w.units.insert(Unit::new(r.unit("Trireme").unwrap(), ROME, w.grid.id_at(3, 12).unwrap(), 2));
    let speed = |w: &World, u| civ_core::world::movement::max_movement(w, &r, w.units.get(u).unwrap());
    let (near_before, far_before) = (speed(&w, near), speed(&w, far));
    build(&mut w, &r, city, lighthouse);
    civ_core::world::policies::rebuild_uniques(&mut w, &r, ROME);
    assert_eq!(placement::standing(&w, city, lighthouse), Some(site));
    assert_eq!(speed(&w, near), near_before + 1);
    assert_eq!(speed(&w, far), far_before, "only within two tiles");
}

#[test]
fn the_grand_canal_adds_to_trade_routes_near_it() {
    let r = rules();
    let (mut w, rome) = world(&r);
    for name in ["The Wheel", "Railroads"] {
        w.civ_mut(ROME).unwrap().techs.insert(r.technologies.id(name).unwrap());
    }
    let ostia_tile = w.grid.id_at(10, 6).unwrap();
    let ostia = w.found_city_in(&r, City::found("Ostia", ROME, ostia_tile));
    for x in 6..=10 {
        let t = w.grid.id_at(x, 6).unwrap();
        w.grid.set_road(t, civ_core::world::grid::RoadLevel::Road);
        w.grid.owner[t.0 as usize] = Some(ROME);
    }
    let gold = |w: &World| {
        let c = civ_core::world::connections::find(w, &r, ROME);
        civ_core::world::connections::trade_route_stats(w, &r, ROME, &c, ostia).gold
    };
    let before = gold(&w);
    let canal = r.building("Grand Canal").unwrap();
    let site = w.grid.id_at(8, 5).unwrap(); // two tiles from Ostia
    w.cities.get_mut(rome).unwrap().sites.insert(canal, site);
    build(&mut w, &r, rome, canal);
    civ_core::world::policies::rebuild_uniques(&mut w, &r, ROME);
    assert!(placement::standing(&w, rome, canal).is_some());
    assert_eq!(gold(&w), before + civ_core::stats::Fixed::whole(2));
}

#[test]
fn the_great_wall_runs_along_its_citys_land_border() {
    let r = rules();
    let (mut w, city) = world(&r);
    let wall = r.building("Great Wall").unwrap();
    assert!(placement::wall_segments(&w, &r, city).is_empty(), "not built");
    build(&mut w, &r, city, wall);
    let segments = placement::wall_segments(&w, &r, city);
    assert!(!segments.is_empty());
    let centre = w.cities.get(city).unwrap().centre;
    assert!(segments.iter().all(|t| w.grid.distance(centre, *t) == 3), "the outer ring of a round city");
}

#[test]
fn a_city_that_loses_its_wonder_loses_its_tile() {
    let r = rules();
    let (mut w, city) = world(&r);
    let pyramids = r.building("The Pyramids").unwrap();
    let desert = terrain(&mut w, &r, 7, 6, "Desert");
    build(&mut w, &r, city, pyramids);
    assert!(w.grid.is_taken(desert));
    w.cities.get_mut(city).unwrap().built.remove(&pyramids);
    placement::settle(&mut w, &r);
    assert!(w.grid.placed_at(desert).is_none());
}

#[test]
fn the_fixtures_place_nothing() {
    let r = Ruleset::load_dir("rules/gods-and-kings").unwrap().unwrap();
    assert!(r.buildings.ids().all(|b| !placement::is_placed(&r, b)));
    let (mut w, city) = world(&r);
    let pyramids = r.building("The Pyramids").unwrap();
    build(&mut w, &r, city, pyramids);
    assert!(w.grid.placed.is_empty());
}

#[cfg(feature = "save")]
#[test]
fn placements_and_chosen_sites_survive_a_save() {
    let r = rules();
    let (mut w, city) = world(&r);
    let pyramids = r.building("The Pyramids").unwrap();
    let wall = r.building("Great Wall").unwrap();
    terrain(&mut w, &r, 7, 6, "Desert");
    build(&mut w, &r, city, pyramids);
    let site = w.grid.id_at(5, 5).unwrap();
    w.cities.get_mut(city).unwrap().sites.insert(wall, site);
    let text = civ_core::save::encode(&w, &r).unwrap();
    assert_eq!(civ_core::save::decode(&text, &r).unwrap(), w);
}
