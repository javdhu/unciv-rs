use civ_core::arena::Id;
use civ_core::ruleset::Ruleset;
use civ_core::stats::Fixed;
use civ_core::world::air::{self, AirRejection, Target};
use civ_core::world::commands::{execute, Command};
use civ_core::world::{City, CityFocus, CivId, Civilization, TileGrid, TileId, Unit, World};

fn fixture() -> (Ruleset, World) {
    let mut r = Ruleset::load_dir("rules/gods-and-kings").unwrap().unwrap();
    // These tests isolate missions from the separate strategic-stock rules.
    for id in r.units.ids().collect::<Vec<_>>() {
        r.units.get_mut(id).unwrap().required_resource = None;
    }
    let nation = r.nations.id("Rome").unwrap();
    let mut w = World::new(
        TileGrid::new(30, 14, r.terrain("Grassland").unwrap()),
        vec![Civilization::new(CivId(0), nation, false), Civilization::new(CivId(1), nation, false)],
    );
    for (owner, x) in [(0, 3), (1, 12)] {
        let at = w.grid.id_at(x, 4).unwrap();
        w.found_city_in(&r, City::found("Airbase", CivId(owner), at));
    }
    w.diplomacy.declare_war(CivId(0), CivId(1));
    (r, w)
}

fn tile(w: &World, x: u16, y: u16) -> TileId {
    w.grid.id_at(x, y).unwrap()
}
fn unit(w: &mut World, r: &Ruleset, owner: u8, name: &str, x: u16, y: u16) -> Id<Unit> {
    let base = r.unit(name).unwrap();
    w.place_unit(Unit::new(base, CivId(owner), tile(w, x, y), r.units.get(base).unwrap().movement))
}
fn reveal(w: &mut World, r: &Ruleset) {
    for civ in [CivId(0), CivId(1)] {
        for tile in w.grid.ids() {
            w.visibility.observe(civ, tile);
        }
    }
    civ_core::world::fog::capture_observations(w, r);
}
fn sea(w: &mut World, r: &Ruleset, x: u16, y: u16) -> TileId {
    let tile = tile(w, x, y);
    w.grid.terrain[tile.0 as usize] = r.terrain("Coast").unwrap();
    tile
}

#[test]
fn archers_do_not_intercept_and_air_strikes_take_return_fire() {
    let (r, mut w) = fixture();
    let bomber = unit(&mut w, &r, 0, "Bomber", 3, 4);
    let archer = unit(&mut w, &r, 1, "Archer", 8, 4);
    let target = unit(&mut w, &r, 1, "Infantry", 9, 4);
    assert!(!air::can_intercept(&w, &r, archer, tile(&w, 9, 4)));
    let result = air::strike(&mut w, &r, bomber, Target::Unit(target)).unwrap();
    assert!(result.damage_to_attacker > 0 && result.damage_to_defender > 0);
    assert_eq!(w.units.get(archer).unwrap().attacks_this_turn, 0);
    assert!(w.units.get(bomber).unwrap().health < 100);
}

#[test]
fn interception_selects_highest_chance_and_never_the_defender() {
    let (r, mut w) = fixture();
    let bomber = unit(&mut w, &r, 0, "Bomber", 3, 4);
    let low = unit(&mut w, &r, 1, "Triplane", 9, 3);
    let high = unit(&mut w, &r, 1, "Fighter", 9, 4);
    let target = unit(&mut w, &r, 1, "Infantry", 8, 4);
    air::strike(&mut w, &r, bomber, Target::Unit(target)).unwrap();
    assert_eq!(w.units.get(high).unwrap().attacks_this_turn, 1);
    assert_eq!(w.units.get(low).unwrap().attacks_this_turn, 0);
    let (r, mut w) = fixture();
    let bomber = unit(&mut w, &r, 0, "Bomber", 3, 4);
    let fighter = unit(&mut w, &r, 1, "Fighter", 9, 4);
    air::strike(&mut w, &r, bomber, Target::Unit(fighter)).unwrap();
    assert_eq!(w.units.get(fighter).unwrap().attacks_this_turn, 0, "defending is not interception");
}

#[test]
fn triplanes_can_miss_and_attempts_are_consumed_on_misses() {
    let (r, mut original) = fixture();
    let bomber = unit(&mut original, &r, 0, "Bomber", 3, 4);
    let fighter = unit(&mut original, &r, 1, "Triplane", 9, 3);
    let target = unit(&mut original, &r, 1, "Infantry", 8, 4);
    let (mut hits, mut misses) = (0, 0);
    for turn in 0..48 {
        let mut w = original.clone();
        w.turn = turn;
        let result = air::strike(&mut w, &r, bomber, Target::Unit(target)).unwrap();
        let mut replay = original.clone();
        replay.turn = turn;
        assert_eq!(air::strike(&mut replay, &r, bomber, Target::Unit(target)).unwrap(), result);
        assert_eq!(w, replay);
        assert_eq!(w.units.get(fighter).unwrap().attacks_this_turn, 1);
        if w.units.get(fighter).unwrap().xp > 0 {
            hits += 1;
        } else {
            misses += 1;
        }
    }
    assert!(hits > 0 && misses > 0, "deterministic chance must produce both outcomes");
}

#[test]
fn interception_wounds_reduce_the_strike_and_stealth_reduces_interception_only() {
    let (r, mut original) = fixture();
    let bomber = unit(&mut original, &r, 0, "Bomber", 3, 4);
    let fighter = unit(&mut original, &r, 1, "Fighter", 9, 3);
    let target = unit(&mut original, &r, 1, "Infantry", 8, 4);
    let mut clear = original.clone();
    clear.remove_unit(fighter);
    let with_interception = air::strike(&mut original, &r, bomber, Target::Unit(target)).unwrap();
    let without = air::strike(&mut clear, &r, bomber, Target::Unit(target)).unwrap();
    assert!(with_interception.damage_to_defender < without.damage_to_defender);
    assert!(with_interception.damage_to_attacker > without.damage_to_attacker);

    let (r, mut w) = fixture();
    let stealth = unit(&mut w, &r, 0, "Stealth Bomber", 3, 4);
    let fighter = unit(&mut w, &r, 1, "Fighter", 9, 3);
    let target = unit(&mut w, &r, 1, "Infantry", 8, 4);
    let mut clear = w.clone();
    clear.remove_unit(fighter);
    let with = air::strike(&mut w, &r, stealth, Target::Unit(target)).unwrap();
    assert_eq!(with, air::strike(&mut clear, &r, stealth, Target::Unit(target)).unwrap());
    assert_eq!(w.units.get(fighter).unwrap().attacks_this_turn, 1);
    assert_eq!(w.units.get(fighter).unwrap().xp, 0);
    assert!(with.damage_to_attacker > 0, "stealth still takes the target's defensive fire");
}

#[test]
fn fatal_interception_leaves_target_unhurt_and_invalid_strikes_are_atomic() {
    let (r, mut w) = fixture();
    let bomber = unit(&mut w, &r, 0, "Bomber", 3, 4);
    unit(&mut w, &r, 1, "Fighter", 9, 3);
    let target = unit(&mut w, &r, 1, "Infantry", 8, 4);
    w.units.get_mut(bomber).unwrap().health = 1;
    let result = air::strike(&mut w, &r, bomber, Target::Unit(target)).unwrap();
    assert!(result.attacker_died);
    assert_eq!(w.units.get(target).unwrap().health, 100);
    let bomber = unit(&mut w, &r, 0, "Bomber", 3, 4);
    let invalid = Target::Unit(bomber);
    let before = w.clone();
    assert_eq!(air::strike(&mut w, &r, bomber, invalid), Err(AirRejection::NotAtWar));
    assert_eq!(w, before);
}

#[test]
fn sweeps_prioritize_aircraft_and_expend_ground_interceptors_without_damage() {
    let (r, mut w) = fixture();
    let fighter = unit(&mut w, &r, 0, "Fighter", 3, 4);
    let aa = unit(&mut w, &r, 1, "Anti-Aircraft Gun", 8, 4);
    let enemy = unit(&mut w, &r, 1, "Triplane", 9, 3);
    reveal(&mut w, &r);
    let to = tile(&w, 8, 4);
    let result = air::sweep(&mut w, &r, fighter, to).unwrap();
    assert!(result.damage_to_attacker > 0 && result.damage_to_defender > 0);
    assert_eq!(w.units.get(enemy).unwrap().attacks_this_turn, 1);
    assert_eq!(w.units.get(aa).unwrap().attacks_this_turn, 0);
    let fighter2 = unit(&mut w, &r, 0, "Fighter", 3, 4);
    let result = air::sweep(&mut w, &r, fighter2, to).unwrap();
    assert_eq!(result.damage_to_attacker, 0);
    assert_eq!(result.damage_to_defender, 0);
    assert_eq!(w.units.get(aa).unwrap().attacks_this_turn, 1);
    let bomber = unit(&mut w, &r, 0, "Bomber", 3, 4);
    assert!(!air::can_intercept(&w, &r, aa, to));
    assert!(air::strike(&mut w, &r, bomber, Target::Unit(aa)).is_ok());
}

#[test]
fn extra_interceptions_and_movement_follow_separate_limits() {
    let (r, mut w) = fixture();
    let fighter = unit(&mut w, &r, 1, "Fighter", 9, 3);
    let promotion = r
        .promotions
        .iter()
        .find(|(_, p)| {
            p.uniques.iter().any(|u| u.unique_type() == Some(civ_core::UniqueType::ExtraInterceptionsPerTurn))
        })
        .unwrap()
        .0;
    w.units.get_mut(fighter).unwrap().promotions.insert(promotion);
    let to = tile(&w, 8, 4);
    w.units.get_mut(fighter).unwrap().spend_attack();
    assert!(air::can_intercept(&w, &r, fighter, to));
    w.units.get_mut(fighter).unwrap().spend_attack();
    assert!(!air::can_intercept(&w, &r, fighter, to));
    w.units.get_mut(fighter).unwrap().refresh(1);
    w.units.get_mut(fighter).unwrap().movement_left = Fixed::ZERO;
    assert!(!air::can_intercept(&w, &r, fighter, to));
    let aa = unit(&mut w, &r, 1, "Anti-Aircraft Gun", 8, 4);
    w.units.get_mut(aa).unwrap().movement_left = Fixed::ZERO;
    assert!(air::can_intercept(&w, &r, aa, to));
}

#[test]
fn carrier_capacity_filters_and_payload_lifecycle_are_explicit() {
    let (r, mut w) = fixture();
    let at = sea(&mut w, &r, 7, 4);
    let next = sea(&mut w, &r, 7, 5);
    let carrier = unit(&mut w, &r, 0, "Carrier", 7, 4);
    let a = unit(&mut w, &r, 0, "Fighter", 3, 4);
    let b = unit(&mut w, &r, 0, "Bomber", 3, 4);
    air::rebase(&mut w, &r, a, at).unwrap();
    air::rebase(&mut w, &r, b, at).unwrap();
    let third = unit(&mut w, &r, 0, "Fighter", 3, 4);
    assert_eq!(air::can_rebase(&w, &r, third, at), Err(AirRejection::NoBase));
    let stealth = unit(&mut w, &r, 0, "Stealth Bomber", 3, 4);
    assert!(!air::can_carry(&w, &r, carrier, w.units.get(stealth).unwrap(), Some(stealth)));
    civ_core::world::movement::move_unit(&mut w, &r, carrier, next).unwrap();
    for id in [a, b] {
        assert_eq!(w.units.get(id).unwrap().position, next);
        assert_eq!(w.units.get(id).unwrap().transport, Some(carrier));
    }
    air::validate_transports(&w, &r).unwrap();
    w.transfer_unit(carrier, CivId(1));
    assert_eq!(w.units.get(a).unwrap().owner, CivId(1));
    air::validate_transports(&w, &r).unwrap();
    w.remove_unit(carrier);
    assert!(w.units.get(a).is_none() && w.units.get(b).is_none());
    assert!(w.units.get(third).is_some());
}

#[test]
fn missiles_use_missile_carriers_and_leave_transports_when_rebased_to_a_city() {
    let (r, mut w) = fixture();
    let at = sea(&mut w, &r, 6, 4);
    let sub = unit(&mut w, &r, 0, "Nuclear Submarine", 6, 4);
    let missile = unit(&mut w, &r, 0, "Guided Missile", 3, 4);
    air::rebase(&mut w, &r, missile, at).unwrap();
    assert_eq!(w.units.get(missile).unwrap().transport, Some(sub));
    let fighter = unit(&mut w, &r, 0, "Fighter", 3, 4);
    assert_eq!(air::can_rebase(&w, &r, fighter, at), Err(AirRejection::NoBase));
    w.units.get_mut(missile).unwrap().refresh(2);
    let home = tile(&w, 3, 4);
    air::rebase(&mut w, &r, missile, home).unwrap();
    assert_eq!(w.units.get(missile).unwrap().transport, None);
    w.remove_unit(sub);
    assert!(w.units.get(missile).is_some());
}

#[test]
fn city_capacity_is_independent_of_ground_garrison_and_carrier_payload() {
    let (r, mut w) = fixture();
    let home = tile(&w, 3, 4);
    unit(&mut w, &r, 0, "Warrior", 3, 4);
    let template = Unit::new(r.unit("Fighter").unwrap(), CivId(0), home, 1);
    for _ in 0..6 {
        assert!(air::base_for(&w, &r, &template, home, None).is_some());
        w.place_unit(template.clone());
    }
    assert!(air::base_for(&w, &r, &template, home, None).is_none());
    let carrier = unit(&mut w, &r, 0, "Carrier", 3, 4);
    let base = air::base_for(&w, &r, &template, home, None).unwrap();
    assert_eq!(base.transport(), Some(carrier));
    let born = civ_core::world::construction::prepare_unit_near(&w, &r, CivId(0), template.base, home, None).unwrap();
    assert_eq!(born.transport, Some(carrier));
}

#[test]
fn paradrop_requires_unspent_action_visible_legal_destination_and_source_condition() {
    let (r, mut w) = fixture();
    let id = unit(&mut w, &r, 0, "Paratrooper", 3, 4);
    let to = tile(&w, 7, 4);
    let before = w.clone();
    assert!(execute(&mut w, &r, CivId(0), Command::Paradrop { unit: id, to }).is_err());
    assert_eq!(w, before);
    reveal(&mut w, &r);
    execute(&mut w, &r, CivId(0), Command::Paradrop { unit: id, to }).unwrap();
    let u = w.units.get(id).unwrap();
    assert_eq!(u.position, to);
    assert_eq!(u.movement_left, Fixed::ONE);
    assert_eq!(u.attacks_this_turn, 1);
    assert_eq!(u.attacks_left, 0);
    let home = tile(&w, 3, 4);
    assert!(air::can_paradrop(&w, &r, id, home).is_err());
    w.units.get_mut(id).unwrap().refresh(2);
    assert!(air::can_paradrop(&w, &r, id, home).is_err(), "Paratroopers require a friendly launch tile");
}

#[test]
fn hosted_ai_uses_air_range_and_rebases_instead_of_ground_movement() {
    let (r, mut w) = fixture();
    let bomber = unit(&mut w, &r, 0, "Bomber", 3, 4);
    let target = unit(&mut w, &r, 1, "Infantry", 8, 4);
    unit(&mut w, &r, 0, "Scout", 6, 4);
    reveal(&mut w, &r);
    assert!(matches!(air::order(&w, &r, bomber), Some(civ_core::world::ai::UnitOrder::Attack(id)) if id == target));
    w.civ_mut(CivId(0)).unwrap().is_ai = true;
    civ_core::world::turn::finish_round(&mut w, &r, CityFocus::None, Default::default());
    assert!(w.units.get(target).unwrap().health < 100);

    let (r, mut w) = fixture();
    let bomber = unit(&mut w, &r, 0, "Bomber", 3, 4);
    let base = tile(&w, 16, 4);
    w.found_city_in(&r, City::found("Forward", CivId(0), base));
    let enemy = tile(&w, 24, 4);
    w.found_city_in(&r, City::found("Enemy", CivId(1), enemy));
    w.diplomacy.make_peace(CivId(0), CivId(1));
    // Remove the nearby enemy city so the distant base is the useful route.
    let old = w.civ(CivId(1)).unwrap().cities[0];
    civ_core::world::conquest::destroy(&mut w, old);
    w.diplomacy.declare_war(CivId(0), CivId(1));
    reveal(&mut w, &r);
    assert_eq!(air::order(&w, &r, bomber), Some(civ_core::world::ai::UnitOrder::Rebase(base)));
    w.civ_mut(CivId(0)).unwrap().is_ai = true;
    civ_core::world::turn::finish_round(&mut w, &r, CityFocus::None, Default::default());
    assert_eq!(w.units.get(bomber).unwrap().position, base);
}

#[cfg(feature = "save")]
#[test]
fn transported_aircraft_survive_save_roundtrip_and_invalid_carrier_references_do_not() {
    let (r, mut w) = fixture();
    let at = sea(&mut w, &r, 7, 4);
    let carrier = unit(&mut w, &r, 0, "Carrier", 7, 4);
    let fighter = unit(&mut w, &r, 0, "Fighter", 3, 4);
    air::rebase(&mut w, &r, fighter, at).unwrap();
    let encoded = civ_core::save::encode(&w, &r).unwrap();
    let loaded = civ_core::save::decode(&encoded, &r).unwrap();
    assert_eq!(loaded, w);
    w.units.get_mut(fighter).unwrap().transport = Some(fighter);
    assert!(civ_core::save::encode(&w, &r).is_err());
    w.units.get_mut(fighter).unwrap().transport = Some(carrier);
    w.units.get_mut(carrier).unwrap().owner = CivId(1);
    assert!(air::validate_transports(&w, &r).is_err());
}

#[test]
fn great_admirals_can_relocate_only_between_owned_coastal_cities() {
    let (r, mut w) = fixture();
    sea(&mut w, &r, 4, 4);
    sea(&mut w, &r, 17, 4);
    let destination = tile(&w, 16, 4);
    w.found_city_in(&r, City::found("Port", CivId(0), destination));
    let inland = tile(&w, 20, 8);
    w.found_city_in(&r, City::found("Inland", CivId(0), inland));
    let admiral = unit(&mut w, &r, 0, "Great Admiral", 3, 4);
    reveal(&mut w, &r);
    assert_eq!(air::can_paradrop(&w, &r, admiral, inland), Err(AirRejection::NotAllowed));
    assert_eq!(air::can_paradrop(&w, &r, admiral, tile(&w, 12, 4)), Err(AirRejection::NotAllowed));
    execute(&mut w, &r, CivId(0), Command::Paradrop { unit: admiral, to: destination }).unwrap();
    assert_eq!(w.units.get(admiral).unwrap().position, destination);
}

#[test]
fn passive_carrier_movement_preserves_air_actions_and_friendly_water_recovery() {
    let (r, mut w) = fixture();
    let at = sea(&mut w, &r, 7, 4);
    let next = sea(&mut w, &r, 7, 5);
    let carrier = unit(&mut w, &r, 0, "Carrier", 7, 4);
    let fighter = unit(&mut w, &r, 0, "Fighter", 3, 4);
    air::rebase(&mut w, &r, fighter, at).unwrap();
    w.units.get_mut(fighter).unwrap().refresh(1);
    w.units.get_mut(fighter).unwrap().health = 60;
    w.grid.owner[next.0 as usize] = Some(CivId(0));
    reveal(&mut w, &r);
    execute(&mut w, &r, CivId(0), Command::MoveUnit { unit: carrier, to: next }).unwrap();
    let aircraft = w.units.get(fighter).unwrap();
    assert_eq!(aircraft.position, next);
    assert_eq!(aircraft.movement_left, Fixed::ONE);
    assert!(!aircraft.moved_this_turn);
    assert_eq!(civ_core::world::healing::unit_healing(&w, &r, fighter), 20);
}

#[test]
fn capture_and_destruction_remove_aircraft_from_the_lost_city() {
    let (r, mut w) = fixture();
    let foreign = w.civ(CivId(1)).unwrap().cities[0];
    let defender = unit(&mut w, &r, 1, "Fighter", 12, 4);
    let elsewhere = unit(&mut w, &r, 1, "Fighter", 20, 4);
    civ_core::world::conquest::conquer(&mut w, &r, foreign, CivId(0)).unwrap();
    assert!(w.units.get(defender).is_none());
    assert!(w.units.get(elsewhere).is_some());
    let based = unit(&mut w, &r, 0, "Bomber", 12, 4);
    civ_core::world::conquest::destroy_in(&mut w, &r, foreign);
    assert!(w.units.get(based).is_none());
}

#[test]
fn guided_missiles_self_destruct_and_logistics_retains_a_second_air_attack() {
    let (r, mut w) = fixture();
    let missile = unit(&mut w, &r, 0, "Guided Missile", 3, 4);
    let fighter = unit(&mut w, &r, 1, "Fighter", 7, 3);
    let target = unit(&mut w, &r, 1, "Infantry", 6, 4);
    let result = air::strike(&mut w, &r, missile, Target::Unit(target)).unwrap();
    assert!(result.attacker_died && w.units.get(missile).is_none());
    assert_eq!(w.units.get(fighter).unwrap().attacks_this_turn, 0);

    w.remove_unit(fighter);
    let bomber = unit(&mut w, &r, 0, "Bomber", 3, 4);
    w.units.get_mut(bomber).unwrap().promotions.insert(r.promotions.id("Logistics").unwrap());
    civ_core::world::movement::refresh_movement(&mut w, &r);
    let city = w.civ(CivId(1)).unwrap().cities[0];
    w.units.get_mut(bomber).unwrap().position = tile(&w, 5, 4);
    air::strike(&mut w, &r, bomber, Target::City(city)).unwrap();
    assert_eq!(w.units.get(bomber).unwrap().attacks_left, 1);
    assert_eq!(w.units.get(bomber).unwrap().movement_left, Fixed::ONE);
    air::strike(&mut w, &r, bomber, Target::City(city)).unwrap();
    assert_eq!(w.units.get(bomber).unwrap().attacks_this_turn, 2);
    assert_eq!(air::can_strike(&w, &r, bomber, Target::City(city)), Err(AirRejection::NoMovement));
}

#[test]
fn disbanding_carriers_evacuate_ready_cargo_and_return_stranded_passengers() {
    let (r, mut w) = fixture();
    let at = sea(&mut w, &r, 7, 4);
    let carrier = unit(&mut w, &r, 0, "Carrier", 7, 4);
    let ready = unit(&mut w, &r, 0, "Fighter", 3, 4);
    let stranded = unit(&mut w, &r, 0, "Bomber", 3, 4);
    air::rebase(&mut w, &r, ready, at).unwrap();
    air::rebase(&mut w, &r, stranded, at).unwrap();
    w.units.get_mut(ready).unwrap().refresh(1);
    assert_eq!(air::evacuate_payload_before_disband(&mut w, &r, carrier), vec![stranded]);
    assert_eq!(w.units.get(ready).unwrap().position, tile(&w, 3, 4));
    assert_eq!(w.units.get(ready).unwrap().transport, None);
    w.remove_unit(carrier);
    assert!(w.units.get(ready).is_some() && w.units.get(stranded).is_none());
}

#[test]
fn submarine_visibility_requires_adjacency_or_the_right_detectors_own_sight() {
    use civ_core::world::fog::can_see_unit;
    let (r, mut w) = fixture();
    for x in 14..=21 {
        sea(&mut w, &r, x, 8);
    }
    let submarine = unit(&mut w, &r, 1, "Submarine", 18, 8);
    reveal(&mut w, &r);
    assert!(!can_see_unit(&w, &r, CivId(0), submarine));
    assert!(can_see_unit(&w, &r, CivId(1), submarine));
    let ordinary = unit(&mut w, &r, 0, "Battleship", 16, 8);
    assert!(!can_see_unit(&w, &r, CivId(0), submarine), "ordinary ranged sight is insufficient");
    w.units.get_mut(ordinary).unwrap().position = tile(&w, 17, 8);
    assert!(can_see_unit(&w, &r, CivId(0), submarine));
    w.remove_unit(ordinary);
    let detector = unit(&mut w, &r, 0, "Destroyer", 16, 8);
    assert!(can_see_unit(&w, &r, CivId(0), submarine));
    w.units.get_mut(detector).unwrap().position = tile(&w, 14, 8);
    assert!(!can_see_unit(&w, &r, CivId(0), submarine), "another unit's sight cannot extend a detector");
}

#[test]
fn unseen_submarines_cannot_be_attacked_or_leak_into_path_previews_and_contacts() {
    use civ_core::world::commands::AttackTarget;
    let (r, mut w) = fixture();
    for x in 14..=21 {
        sea(&mut w, &r, x, 8);
    }
    let attacker = unit(&mut w, &r, 0, "Battleship", 16, 8);
    let submarine = unit(&mut w, &r, 1, "Submarine", 18, 8);
    reveal(&mut w, &r);
    let before = w.clone();
    assert!(execute(&mut w, &r, CivId(0), Command::Attack { unit: attacker, target: AttackTarget::Unit(submarine) })
        .is_err());
    assert_eq!(w, before);
    let preview = civ_core::world::fog::reachable_preview(&w, &r, CivId(0), attacker);
    w.remove_unit(submarine);
    assert_eq!(preview, civ_core::world::fog::reachable_preview(&w, &r, CivId(0), attacker));
}

#[test]
fn inherited_fighter_sight_and_elevation_occlusion_match_the_shared_visibility_rules() {
    let (r, mut w) = fixture();
    let fighter = unit(&mut w, &r, 0, "Fighter", 3, 4);
    let bomber = unit(&mut w, &r, 0, "Bomber", 3, 4);
    let far = tile(&w, 9, 4);
    assert!(civ_core::world::fog::unit_visible_tiles(&w, &r, fighter).contains(&far));
    assert!(!civ_core::world::fog::unit_visible_tiles(&w, &r, bomber).contains(&far));
    let at = tile(&w, 17, 8);
    let next = tile(&w, 18, 8);
    let behind = tile(&w, 19, 8);
    w.grid.features[next.0 as usize].push(r.terrain("Forest").unwrap());
    let visible = civ_core::world::fog::viewable_tiles(&w.grid, &r, at, 2, false);
    assert!(visible.contains(&next));
    assert!(!visible.contains(&behind));
}

#[test]
fn a_sweep_that_meets_an_aircraft_reports_the_dogfight_and_one_that_meets_nothing_does_not() {
    use civ_core::world::turn::TurnEvent;
    let (r, mut w) = fixture();
    let fighter = unit(&mut w, &r, 0, "Fighter", 3, 4);
    let enemy = unit(&mut w, &r, 1, "Triplane", 9, 3);
    reveal(&mut w, &r);
    let to = tile(&w, 8, 4);
    let reports = |events: &[TurnEvent]| {
        events
            .iter()
            .filter_map(|e| match e {
                TurnEvent::Battle { report: Some(b), .. } => Some(*b),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let events = execute(&mut w, &r, CivId(0), Command::AirSweep { unit: fighter, target: to }).unwrap().events;
    let [report] = reports(&events)[..] else { panic!("one dogfight: {events:?}") };
    assert_eq!((report.attacker, report.target), (fighter, Target::Unit(enemy)));
    assert!(report.damage_to_attacker > 0 && report.damage_to_defender > 0);
    let second = unit(&mut w, &r, 0, "Fighter", 3, 4);
    let far = tile(&w, 5, 10);
    let events = execute(&mut w, &r, CivId(0), Command::AirSweep { unit: second, target: far }).unwrap().events;
    assert!(reports(&events).is_empty(), "nothing met, nothing fought");
}

#[test]
fn target_lists_hold_exactly_the_tiles_each_mission_accepts() {
    let (r, mut w) = fixture();
    reveal(&mut w, &r);
    let fighter = unit(&mut w, &r, 0, "Fighter", 3, 4);
    let range = air::attack_range(&w, &r, fighter) as u32;
    let sweeps = air::sweep_targets(&w, &r, fighter);
    let expected: Vec<_> =
        w.grid.within(tile(&w, 3, 4), range).into_iter().filter(|t| air::can_sweep(&w, &r, fighter, *t).is_ok()).collect();
    assert!(!sweeps.is_empty());
    assert_eq!(sweeps, expected);
    assert!(air::rebase_targets(&w, &r, fighter).is_empty(), "no other base");
    let field = tile(&w, 6, 4);
    w.found_city_in(&r, City::found("Field", CivId(0), field));
    assert_eq!(air::rebase_targets(&w, &r, fighter), vec![field]);
    let para = unit(&mut w, &r, 0, "Paratrooper", 3, 4);
    let drops = air::paradrop_targets(&w, &r, para);
    assert!(drops.contains(&tile(&w, 7, 4)));
    assert!(drops.iter().all(|t| air::can_paradrop(&w, &r, para, *t).is_ok()));
    assert!(drops.iter().all(|t| w.grid.distance(tile(&w, 3, 4), *t) <= 5));
    assert!(air::paradrop_targets(&w, &r, fighter).is_empty());
}

#[test]
fn a_previewed_interception_reads_only_the_defenders_in_sight() {
    let (r, mut w) = fixture();
    let bomber = unit(&mut w, &r, 0, "Bomber", 3, 4);
    let fighter = unit(&mut w, &r, 1, "Fighter", 10, 4);
    let target = unit(&mut w, &r, 1, "Infantry", 9, 4);
    let at = tile(&w, 9, 4);
    let everything = |_| true;
    let chance =
        air::visible_intercept_chance(&w, &r, bomber, at, Some(Target::Unit(target)), Some(CivId(1)), &everything);
    assert_eq!(chance, air::intercept_chance(&w, &r, fighter));
    assert!(chance > 0);
    let blind = |id| id != fighter;
    assert_eq!(air::visible_intercept_chance(&w, &r, bomber, at, Some(Target::Unit(target)), Some(CivId(1)), &blind), 0);
}
