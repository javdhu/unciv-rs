use civ_core::{
    arena::Id,
    ruleset::Ruleset,
    stats::Fixed,
    world::{self, City, CivId, Civilization, TileGrid, TileId, Unit, World},
};
fn fixture() -> (Ruleset, World, Id<City>) {
    let mut r = Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings").unwrap().unwrap();
    for name in ["Atomic Bomb", "Nuclear Missile"] {
        let id = r.unit(name).unwrap();
        let unit = r.units.get_mut(id).unwrap();
        unit.required_resource = None;
        unit.uniques.retain(|u| u.unique_type() != Some(civ_core::UniqueType::ConsumesResources));
    }
    let nation = r.nations.id("Rome").unwrap();
    let mut w = World::new(
        TileGrid::new(25, 12, r.terrain("Grassland").unwrap()),
        vec![Civilization::new(CivId(0), nation, false), Civilization::new(CivId(1), nation, false)],
    );
    let city = w.found_city_in(&r, City::found("Target", CivId(1), w.grid.id_at(12, 4).unwrap()));
    w.cities.get_mut(city).unwrap().population = 10;
    w.cities.get_mut(city).unwrap().health = 200;
    w.diplomacy.meet(CivId(0), CivId(1));
    w.diplomacy.declare_war(CivId(0), CivId(1));
    for tile in w.grid.ids() {
        w.visibility.observe(CivId(0), tile);
    }
    (r, w, city)
}
fn unit(w: &mut World, r: &Ruleset, owner: u8, name: &str, x: u16, y: u16) -> Id<Unit> {
    let id = r.unit(name).unwrap();
    w.place_unit(Unit::new(id, CivId(owner), w.grid.id_at(x, y).unwrap(), r.units[id].movement))
}
fn tile(w: &World, x: u16, y: u16) -> TileId {
    w.grid.id_at(x, y).unwrap()
}
#[test]
fn atomic_damage_varies_and_shelter_protects_population_and_garrison() {
    let (r, mut original, city) = fixture();
    let bomb = unit(&mut original, &r, 0, "Atomic Bomb", 3, 4);
    let guard = unit(&mut original, &r, 1, "Infantry", 12, 4);
    let target = tile(&original, 12, 4);
    let mut protected = original.clone();
    protected.cities.get_mut(city).unwrap().built.insert(r.building("Bomb Shelter").unwrap());
    civ_core::world::city::rebuild_city_uniques(&r, protected.cities.get_mut(city).unwrap());
    world::combat::nuclear_strike(&mut original, &r, bomb, target).unwrap();
    world::combat::nuclear_strike(&mut protected, &r, bomb, target).unwrap();
    assert!(original.units.get(guard).is_none());
    assert!(protected.units.get(guard).is_some());
    assert!(protected.cities.get(city).unwrap().population > original.cities.get(city).unwrap().population);
    assert_eq!(protected.cities.get(city).unwrap().health, 100);
    assert!(!protected.grid.features[target.0 as usize].contains(&r.terrain("Fallout").unwrap()));
}
#[test]
fn missile_destroys_small_noncapital_but_preserves_original_capital() {
    let (r, mut w, capital) = fixture();
    let target = tile(&w, 12, 4);
    w.cities.get_mut(capital).unwrap().population = 3;
    let second = w.found_city_in(&r, City::found("Outpost", CivId(1), tile(&w, 14, 4)));
    let missile = unit(&mut w, &r, 0, "Nuclear Missile", 3, 4);
    world::combat::nuclear_strike(&mut w, &r, missile, target).unwrap();
    assert!(w.cities.get(second).is_none());
    assert!(w.cities.get(capital).is_some());
    assert!(w.units.get(missile).is_none());
    assert_eq!(
        w.diplomacy.attitude(CivId(1), CivId(0)).get(world::diplomacy::Sentiment::UsedNuclearWeapons),
        Fixed::whole(-50)
    );
}
#[test]
fn atomic_splash_is_not_guaranteed_lethal_and_replays_identically() {
    let (r, mut original, _) = fixture();
    let bomb = unit(&mut original, &r, 0, "Atomic Bomb", 3, 4);
    let guard = unit(&mut original, &r, 1, "Infantry", 13, 4);
    let target = tile(&original, 12, 4);
    let mut survived = 0;
    for turn in 0..32 {
        let mut w = original.clone();
        w.turn = turn;
        let mut replay = w.clone();
        assert_eq!(
            world::combat::nuclear_strike(&mut w, &r, bomb, target),
            world::combat::nuclear_strike(&mut replay, &r, bomb, target)
        );
        assert_eq!(w, replay);
        if let Some(u) = w.units.get(guard) {
            survived += 1;
            assert!(u.health < 100);
        }
    }
    assert!(survived > 0);
}
#[test]
fn fallout_never_covers_water_mountains_or_surviving_cities() {
    let (r, mut w, _) = fixture();
    let bomb = unit(&mut w, &r, 0, "Nuclear Missile", 3, 4);
    let target = tile(&w, 12, 4);
    let water = tile(&w, 13, 4);
    let mountain = tile(&w, 11, 4);
    w.grid.terrain[water.0 as usize] = r.terrain("Coast").unwrap();
    w.grid.terrain[mountain.0 as usize] = r.terrain("Mountain").unwrap();
    world::combat::nuclear_strike(&mut w, &r, bomb, target).unwrap();
    let fallout = r.terrain("Fallout").unwrap();
    for t in [water, mountain, target] {
        assert!(!w.grid.features[t.0 as usize].contains(&fallout));
    }
}
#[test]
fn rejected_launch_is_atomic_for_self_target_unknown_victim_and_active_truce() {
    use world::commands::{execute, Command};
    let (r, mut w, _) = fixture();
    let bomb = unit(&mut w, &r, 0, "Atomic Bomb", 3, 4);
    let target = tile(&w, 12, 4);
    let before = w.clone();
    let launch = tile(&w, 3, 4);
    assert!(execute(&mut w, &r, CivId(0), Command::NuclearStrike { unit: bomb, target: launch }).is_err());
    assert_eq!(w, before);
    w.diplomacy.make_peace(CivId(0), CivId(1));
    let before = w.clone();
    assert!(execute(&mut w, &r, CivId(0), Command::NuclearStrike { unit: bomb, target }).is_err());
    assert_eq!(w, before);
    let mut unknown = World::new(w.grid.clone(), w.civs.clone());
    unknown.units = w.units.clone();
    unknown.cities = w.cities.clone();
    unknown.visibility = w.visibility.clone();
    let before = unknown.clone();
    assert!(execute(&mut unknown, &r, CivId(0), Command::NuclearStrike { unit: bomb, target }).is_err());
    assert_eq!(unknown, before);
}
#[test]
fn fatal_interception_prevents_nuclear_blast() {
    let (r, mut w, city) = fixture();
    let bomb = unit(&mut w, &r, 0, "Atomic Bomb", 3, 4);
    w.units.get_mut(bomb).unwrap().health = 1;
    let fighter = unit(&mut w, &r, 1, "Fighter", 12, 4);
    let target = tile(&w, 12, 4);
    let result = world::combat::nuclear_strike(&mut w, &r, bomb, target).unwrap();
    assert!(result.tiles_affected.is_empty());
    assert_eq!(w.cities.get(city).unwrap().health, 200);
    assert_eq!(w.units.get(fighter).unwrap().attacks_this_turn, 1);
}
#[test]
fn ai_selects_nuclear_mission_at_war_and_respects_friendly_collateral() {
    let (r, mut w, _) = fixture();
    let bomb = unit(&mut w, &r, 0, "Atomic Bomb", 3, 4);
    let target = tile(&w, 12, 4);
    assert_eq!(world::ai::military_order(&w, &r, CivId(0), bomb), Some(world::ai::UnitOrder::NuclearStrike(target)));
    unit(&mut w, &r, 0, "Warrior", 13, 4);
    assert_eq!(world::nuclear::order(&w, &r, bomb), None);
}
