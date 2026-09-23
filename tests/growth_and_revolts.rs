#![cfg(feature = "save")]
use civ_core::{
    ruleset::Ruleset,
    stats::Fixed,
    world::{
        construction::{self, Construction},
        entities::Civilization,
        grid::TileGrid,
        turn, City, CivId, World,
    },
};
fn fixture() -> (Ruleset, World, civ_core::arena::Id<City>) {
    let r = Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings").unwrap().unwrap();
    let mut barb = Civilization::new(CivId(1), r.nations.id("Barbarians").unwrap(), true);
    barb.is_barbarian = true;
    let mut w = World::new(
        TileGrid::new(12, 12, r.terrain("Grassland").unwrap()),
        vec![Civilization::new(CivId(0), r.nations.id("Rome").unwrap(), false), barb],
    );
    w.diplomacy.mark_hostile(CivId(1));
    w.civ_mut(CivId(0)).unwrap().techs.insert(r.technologies.id("Agriculture").unwrap());
    let centre = w.grid.id_at(6, 6).unwrap();
    let city = w.found_city_in(&r, City::found("Home", CivId(0), centre));
    let tiles: Vec<_> = w.grid.neighbours(centre).take(2).collect();
    for tile in &tiles {
        w.claim_tile(city, *tile);
    }
    let c = w.cities.get_mut(city).unwrap();
    c.population = 2;
    c.worked = tiles;
    c.food_stored = 5;
    (r, w, city)
}
#[test]
fn building_settler_converts_surplus_without_growing_and_starvation_stays_negative() {
    let (r, mut w, city) = fixture();
    let normal = turn::plan_economy(&w, &r, CivId(0), Default::default(), Default::default());
    let food = normal.food[0].1;
    assert!(food > 0);
    w.cities.get_mut(city).unwrap().queue = vec![Construction::Unit(r.units.id("Settler").unwrap())];
    let settler = turn::plan_economy(&w, &r, CivId(0), Default::default(), Default::default());
    assert_eq!(settler.food[0].1, 0);
    assert_eq!(settler.production[0].1, normal.production[0].1 + construction::production_from_food(food));
    w.cities.get_mut(city).unwrap().population = 40;
    let starve = turn::plan_economy(&w, &r, CivId(0), Default::default(), Default::default());
    assert!(starve.happiness < -10);
    assert!(starve.food[0].1 < 0);
    assert!(construction::can_construct_in(
        &w,
        &r,
        CivId(0),
        w.cities.get(city).unwrap(),
        Construction::Unit(r.units.id("Settler").unwrap())
    )
    .is_err());
}
#[test]
fn revolt_countdown_survives_save_and_recovery_cancels_it() {
    let (r, mut w, city) = fixture();
    w.cities.get_mut(city).unwrap().population = 40;
    assert!(civ_core::world::barbarians::revolt(&mut w, &r, CivId(0), -40, Fixed::ONE).is_empty());
    let left = w.civ(CivId(0)).unwrap().revolt_countdown.unwrap();
    assert!((4..=6).contains(&left));
    let mut restored = civ_core::save::decode(&civ_core::save::encode(&w, &r).unwrap(), &r).unwrap();
    for _ in 0..left {
        w.turn += 1;
        restored.turn += 1;
        let a = civ_core::world::barbarians::revolt(&mut w, &r, CivId(0), -40, Fixed::ONE);
        let b = civ_core::world::barbarians::revolt(&mut restored, &r, CivId(0), -40, Fixed::ONE);
        assert_eq!(a, b);
    }
    assert_eq!(w, restored);
    assert!(!w.civ(CivId(1)).unwrap().units.is_empty());
    assert!(w.civ(CivId(0)).unwrap().revolt_countdown.is_none());
    civ_core::world::barbarians::revolt(&mut w, &r, CivId(0), -40, Fixed::ONE);
    civ_core::world::barbarians::revolt(&mut w, &r, CivId(0), 0, Fixed::ONE);
    assert!(w.civ(CivId(0)).unwrap().revolt_countdown.is_none());
}
