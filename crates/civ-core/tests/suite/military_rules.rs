// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use civ_core::arena::Id;
use civ_core::ruleset::Ruleset;
use civ_core::stats::Fixed;
use civ_core::world::{ai, air, combat, military, City, CivId, Civilization, TileGrid, TileId, Unit, World};

fn fixture() -> (Ruleset, World) {
    let mut r = Ruleset::load_dir("rules/gods-and-kings").unwrap().unwrap();
    for id in r.units.ids().collect::<Vec<_>>() {
        r.units.get_mut(id).unwrap().required_resource = None;
    }
    let nation = r.nations.id("Rome").unwrap();
    let mut w = World::new(
        TileGrid::new(18, 12, r.terrain("Grassland").unwrap()),
        vec![Civilization::new(CivId(0), nation, false), Civilization::new(CivId(1), nation, false)],
    );
    w.diplomacy.declare_war(CivId(0), CivId(1));
    (r, w)
}
fn tile(w: &World, x: u16, y: u16) -> TileId {
    w.grid.id_at(x, y).unwrap()
}
fn unit(w: &mut World, r: &Ruleset, owner: u8, name: &str, x: u16, y: u16) -> Id<Unit> {
    let base = r.unit(name).unwrap();
    w.place_unit(Unit::new(base, CivId(owner), tile(w, x, y), r.units[base].movement))
}
fn promotion(w: &mut World, r: &Ruleset, id: Id<Unit>, name: &str) {
    w.units.get_mut(id).unwrap().promotions.insert(r.promotions.id(name).unwrap());
}

#[test]
fn free_promotions_do_not_raise_cost_and_prerequisites_are_alternatives() {
    let (r, mut w) = fixture();
    let id = unit(&mut w, &r, 0, "Warrior", 4, 4);
    promotion(&mut w, &r, id, "Shock II");
    promotion(&mut w, &r, id, "Morale");
    w.units.get_mut(id).unwrap().xp = 10;
    assert!(combat::can_promote(w.units.get(id).unwrap()));
    let march = r.promotions.id("March").unwrap();
    assert!(!military::available_promotions(&w, &r, id).contains(&march));
    promotion(&mut w, &r, id, "Shock III");
    assert!(military::available_promotions(&w, &r, id).contains(&march), "Shock III alone is sufficient");
    military::promote(&mut w, &r, id, march).unwrap();
    assert_eq!(w.units.get(id).unwrap().promotions_earned, 1);
    assert!(!combat::can_promote(w.units.get(id).unwrap()));
    w.units.get_mut(id).unwrap().xp = 30;
    assert!(combat::can_promote(w.units.get(id).unwrap()));
}

#[test]
fn instant_heal_consumes_each_earned_level_and_uses_healing_multiplier() {
    let (r, mut w) = fixture();
    let id = unit(&mut w, &r, 0, "Warrior", 4, 4);
    promotion(&mut w, &r, id, "Rejuvenation");
    let heal = r.promotions.id("Heal Instantly").unwrap();
    {
        let u = w.units.get_mut(id).unwrap();
        u.xp = 30;
        u.health = 1;
    }
    military::promote(&mut w, &r, id, heal).unwrap();
    assert_eq!(w.units.get(id).unwrap().health, 100);
    assert!(!w.units.get(id).unwrap().promotions.contains(&heal));
    w.units.get_mut(id).unwrap().health = 20;
    military::promote(&mut w, &r, id, heal).unwrap();
    assert_eq!(w.units.get(id).unwrap().promotions_earned, 2);
    assert!(military::promote(&mut w, &r, id, heal).is_err());
}

#[test]
fn city_training_stacks_buildings_and_does_not_charge_free_morale() {
    let (r, mut w) = fixture();
    let centre = tile(&w, 4, 4);
    let mut city = City::found("Training", CivId(0), centre);
    for name in ["Barracks", "Armory", "Heroic Epic"] {
        city.built.insert(r.building(name).unwrap());
    }
    let city = w.found_city_in(&r, city);
    let mut trained = civ_core::world::construction::prepare_unit_near(
        &w,
        &r,
        CivId(0),
        r.unit("Warrior").unwrap(),
        centre,
        Some(city),
    )
    .unwrap();
    assert_eq!(trained.xp, 0, "free placement is not city training");
    military::train(&w, &r, city, &mut trained);
    assert_eq!(trained.xp, 30);
    assert_eq!(trained.promotions_earned, 0);
    assert!(trained.promotions.contains(&r.promotions.id("Morale").unwrap()));
    let mut civilian = Unit::new(r.unit("Worker").unwrap(), CivId(0), centre, 2);
    military::train(&w, &r, city, &mut civilian);
    assert_eq!(civilian.xp, 0);
}

#[test]
fn xp_modifiers_apply_and_barbarian_cap_uses_total_earned_experience() {
    let (r, mut w) = fixture();
    let a = unit(&mut w, &r, 0, "Warrior", 4, 4);
    let d = unit(&mut w, &r, 1, "Warrior", 5, 4);
    promotion(&mut w, &r, a, "Quick Study");
    assert_eq!(military::award_xp(&mut w, &r, a, 5, Some(d), CivId(1), false), 7);
    w.civ_mut(CivId(1)).unwrap().is_barbarian = true;
    w.units.get_mut(a).unwrap().xp = 29;
    assert_eq!(military::award_xp(&mut w, &r, a, 5, Some(d), CivId(1), false), 7);
    assert_eq!(military::award_xp(&mut w, &r, a, 5, Some(d), CivId(1), false), 0);
}

#[test]
fn siege_needs_setup_and_ai_selects_a_real_ranged_target() {
    let (r, mut w) = fixture();
    let a = unit(&mut w, &r, 0, "Catapult", 4, 4);
    let d = unit(&mut w, &r, 1, "Warrior", 6, 4);
    let target = w.units.get(d).unwrap().position;
    w.visibility.observe(CivId(0), target);
    assert_eq!(ai::military_order(&w, &r, CivId(0), a), Some(ai::UnitOrder::Attack(d)));
    assert!(combat::resolve_ranged_attack(&mut w, &r, a, target).is_err());
    assert!(military::set_up(&mut w, &r, a));
    assert_eq!(w.units.get(a).unwrap().movement_left, Fixed::ONE);
    assert!(combat::resolve_ranged_attack(&mut w, &r, a, target).is_ok());
    assert!(w.units.get(d).unwrap().health < 100);
}

#[test]
fn attack_filters_prevent_carriers_submarine_land_attacks_and_ram_unit_attacks() {
    let (r, mut w) = fixture();
    let d = unit(&mut w, &r, 1, "Warrior", 6, 4);
    for (name, x) in [("Carrier", 3), ("Submarine", 4), ("Battering Ram", 5)] {
        let id = unit(&mut w, &r, 0, name, x, 4);
        assert!(!military::may_attack(&w, &r, id, civ_core::world::air::Target::Unit(d)), "{name}");
    }
}

#[test]
fn ranged_civilians_take_forty_damage_and_melee_capture_advances_without_attack() {
    let (r, mut w) = fixture();
    let a = unit(&mut w, &r, 0, "Archer", 4, 4);
    let d = unit(&mut w, &r, 1, "Worker", 6, 4);
    let target = w.units.get(d).unwrap().position;
    let result = combat::resolve_ranged_attack(&mut w, &r, a, target).unwrap();
    assert_eq!(result.damage_to_defender, 40);
    assert_eq!(w.units.get(d).unwrap().health, 60);
    let melee = unit(&mut w, &r, 0, "Warrior", 5, 4);
    combat::resolve_attack(&mut w, &r, melee, d).unwrap();
    assert_eq!(w.units.get(melee).unwrap().position, target);
    assert_eq!(w.units.get(melee).unwrap().attacks_this_turn, 0);
    assert_eq!(w.units.get(d).unwrap().owner, CivId(0));
}

#[test]
fn fatal_melee_hit_stops_retaliation_and_survivor_advances() {
    let (r, mut original) = fixture();
    let a = unit(&mut original, &r, 0, "Warrior", 4, 4);
    let d = unit(&mut original, &r, 1, "Warrior", 5, 4);
    original.units.get_mut(a).unwrap().health = 1;
    original.units.get_mut(d).unwrap().health = 1;
    for turn in 0..32 {
        let mut w = original.clone();
        w.turn = turn;
        let result = combat::resolve_attack(&mut w, &r, a, d).unwrap();
        assert_ne!(result.attacker_died, result.defender_died);
        if result.defender_died {
            assert_eq!(w.units.get(a).unwrap().position, tile(&w, 5, 4));
        }
    }
}

#[test]
fn withdrawal_is_deterministic_spends_attacker_action_and_preserves_defender_health() {
    let (r, mut original) = fixture();
    let a = unit(&mut original, &r, 0, "Warrior", 4, 4);
    let d = unit(&mut original, &r, 1, "Slinger", 5, 4);
    let mut successes = 0;
    let mut failures = 0;
    for turn in 0..32 {
        let mut w = original.clone();
        w.turn = turn;
        let mut replay = w.clone();
        let result = combat::resolve_attack(&mut w, &r, a, d).unwrap();
        assert_eq!(combat::resolve_attack(&mut replay, &r, a, d).unwrap(), result);
        assert_eq!(w, replay);
        if result.damage_to_defender == 0 {
            successes += 1;
            assert_ne!(w.units.get(d).unwrap().position, tile(&w, 5, 4));
            assert_eq!(w.units.get(d).unwrap().health, 100);
            assert_eq!(w.units.get(a).unwrap().attacks_this_turn, 1);
        } else {
            failures += 1;
        }
    }
    assert!(successes > 0 && failures > 0);
}

#[test]
fn citadel_damage_uses_max_and_ignores_pillaged_and_friendly_improvements() {
    let (r, mut w) = fixture();
    let id = unit(&mut w, &r, 0, "Warrior", 4, 4);
    let citadel = r.improvements.id("Citadel").unwrap();
    let around: Vec<_> = w.grid.neighbours(tile(&w, 4, 4)).take(2).collect();
    for tile in &around {
        w.grid.owner[tile.0 as usize] = Some(CivId(1));
        w.grid.improvement[tile.0 as usize] = Some(citadel);
    }
    military::end_round(&mut w, &r);
    assert_eq!(w.units.get(id).unwrap().health, 70);
    for tile in &around {
        w.grid.pillage_improvement(*tile);
    }
    assert!(military::end_round(&mut w, &r).is_empty());
    for tile in &around {
        w.grid.repair_improvement(*tile);
        w.grid.owner[tile.0 as usize] = Some(CivId(0));
    }
    assert!(military::end_round(&mut w, &r).is_empty());
}

#[test]
fn adjacent_fountain_grants_free_rejuvenation_and_pillage_uses_real_loot() {
    let (r, mut w) = fixture();
    let id = unit(&mut w, &r, 0, "Warrior", 4, 4);
    let near = tile(&w, 5, 4);
    w.grid.natural_wonder[near.0 as usize] = Some(r.terrain("Fountain of Youth").unwrap());
    military::on_enter(&mut w, &r, id);
    assert!(w.units.get(id).unwrap().promotions.contains(&r.promotions.id("Rejuvenation").unwrap()));
    assert_eq!(w.units.get(id).unwrap().promotions_earned, 0);
    let pos = tile(&w, 4, 4);
    w.grid.improvement[pos.0 as usize] = Some(r.improvements.id("Farm").unwrap());
    promotion(&mut w, &r, id, "[Sipahi] ability");
    w.units.get_mut(id).unwrap().health = 10;
    let before = w.clone();
    let result = combat::pillage(&mut w, &r, id).unwrap();
    let mut replay = before.clone();
    assert_eq!(combat::pillage(&mut replay, &r, id).unwrap(), result);
    assert_eq!(w, replay);
    assert_eq!(w.units.get(id).unwrap().health, 60);
    assert_eq!(w.units.get(id).unwrap().movement_left, before.units.get(id).unwrap().movement_left);
    assert!(!w.units.get(id).unwrap().moved_this_turn);
    assert!((0..=34).contains(&result.gold_pillaged));
}

#[test]
fn fortification_builds_over_two_resting_rounds_and_movement_resets_it() {
    let (r, mut w) = fixture();
    let id = unit(&mut w, &r, 0, "Warrior", 4, 4);
    let base = combat::defending_strength(&w, &r, id).unwrap();
    civ_core::world::commands::execute(&mut w, &r, CivId(0), civ_core::world::commands::Command::Fortify { unit: id })
        .unwrap();
    assert_eq!(combat::defending_strength(&w, &r, id).unwrap(), base);
    civ_core::world::healing::recover(&mut w, &r);
    assert_eq!(w.units.get(id).unwrap().fortification_turns, 1);
    assert_eq!(combat::defending_strength(&w, &r, id).unwrap(), base * Fixed::permille(1200));
    civ_core::world::healing::recover(&mut w, &r);
    assert_eq!(w.units.get(id).unwrap().fortification_turns, 2);
    assert_eq!(combat::defending_strength(&w, &r, id).unwrap(), base * Fixed::permille(1400));
    w.units.get_mut(id).unwrap().refresh(2);
    let to = tile(&w, 5, 4);
    civ_core::world::movement::move_unit(&mut w, &r, id, to).unwrap();
    assert_eq!(w.units.get(id).unwrap().fortification_turns, 0);
}

#[test]
fn great_general_uses_strongest_aura_and_maori_malus_does_not_stack() {
    let (r, mut w) = fixture();
    let id = unit(&mut w, &r, 0, "Warrior", 4, 4);
    let target = unit(&mut w, &r, 1, "Warrior", 6, 4);
    let base = combat::combat_strength(&w, &r, id, civ_core::world::air::Target::Unit(target), true).unwrap();
    unit(&mut w, &r, 0, "Great General", 4, 4);
    unit(&mut w, &r, 0, "Great General", 4, 5);
    assert_eq!(
        combat::combat_strength(&w, &r, id, civ_core::world::air::Target::Unit(target), true).unwrap(),
        base * Fixed::permille(1150)
    );
    unit(&mut w, &r, 1, "Maori Warrior", 5, 4);
    unit(&mut w, &r, 1, "Maori Warrior", 3, 4);
    assert_eq!(
        combat::combat_strength(&w, &r, id, civ_core::world::air::Target::Unit(target), true).unwrap(),
        base * Fixed::permille(1050)
    );
}

#[test]
fn technology_and_actual_garrison_health_raise_city_strength() {
    let (r, mut w) = fixture();
    let centre = tile(&w, 4, 4);
    let city = w.found_city_in(&r, City::found("Capital", CivId(0), centre));
    let ancient = combat::city_defense_strength(&w, &r, city);
    w.civ_mut(CivId(0)).unwrap().techs.extend(r.technologies.ids());
    let late = combat::city_defense_strength(&w, &r, city);
    assert!(late > ancient + Fixed::whole(110));
    let guard = unit(&mut w, &r, 0, "Infantry", 4, 4);
    let healthy = combat::city_defense_strength(&w, &r, city);
    w.units.get_mut(guard).unwrap().health = 50;
    let wounded = combat::city_defense_strength(&w, &r, city);
    assert_eq!(healthy - late, Fixed::whole(14));
    assert_eq!(wounded - late, Fixed::whole(7));
}

#[test]
fn upgrade_uses_national_replacement_discount_speed_and_five_gold_rounding() {
    let (r, mut w) = fixture();
    let id = unit(&mut w, &r, 0, "Warrior", 4, 4);
    let pos = tile(&w, 4, 4);
    w.grid.owner[pos.0 as usize] = Some(CivId(0));
    w.civ_mut(CivId(0)).unwrap().gold = 1000;
    w.civ_mut(CivId(0)).unwrap().techs.extend(r.technologies.ids());
    w.civ_mut(CivId(0)).unwrap().uniques.insert(civ_core::Unique::parse("[-33]% Gold cost of upgrading"));
    let (target, price) = combat::can_upgrade_with(&w, &r, id, Fixed::permille(1500)).unwrap();
    assert_eq!(r.units[target].name, "Legion");
    let normal = combat::upgrade_cost(&r, r.unit("Warrior").unwrap(), target).unwrap();
    assert_eq!(price, ((i64::from(normal) * 670 * 1500 / 1_000_000) / 5 * 5) as i32);
    let mut poor = w.clone();
    poor.civ_mut(CivId(0)).unwrap().gold = 0;
    poor.civ_mut(CivId(0)).unwrap().techs.clear();
    poor.units.get_mut(id).unwrap().movement_left = Fixed::ZERO;
    assert_eq!(combat::upgrade_quote(&poor, &r, id, Fixed::permille(1500)), Some((target, price)));
    assert!(combat::can_upgrade_with(&poor, &r, id, Fixed::permille(1500)).is_err());
    combat::upgrade_unit_with(&mut w, &r, id, Fixed::permille(1500)).unwrap();
    assert_eq!(w.civ(CivId(0)).unwrap().gold, 1000 - price);
    assert!(!w.units.get(id).unwrap().can_move());
}

#[test]
fn kill_plunder_uses_defeated_strength_and_prize_ships_restore_enemy_hull() {
    let (r, mut w) = fixture();
    let a = unit(&mut w, &r, 0, "Infantry", 4, 4);
    let d = unit(&mut w, &r, 1, "Warrior", 5, 4);
    w.units.get_mut(d).unwrap().health = 1;
    w.civ_mut(CivId(0))
        .unwrap()
        .uniques
        .insert(civ_core::Unique::parse("Earn [100]% of killed [Military] unit's [Strength] as [Culture]"));
    combat::resolve_attack(&mut w, &r, a, d).unwrap();
    assert_eq!(w.civ(CivId(0)).unwrap().culture_stored, 8);
    let (r, mut original) = fixture();
    for x in 3..8 {
        for y in 3..6 {
            let t = tile(&original, x, y);
            original.grid.terrain[t.0 as usize] = r.terrain("Coast").unwrap();
        }
    }
    let a = unit(&mut original, &r, 0, "Privateer", 4, 4);
    let d = unit(&mut original, &r, 1, "Trireme", 5, 4);
    original.units.get_mut(d).unwrap().health = 1;
    let mut captured = 0;
    for turn in 0..32 {
        let mut w = original.clone();
        w.turn = turn;
        combat::resolve_attack(&mut w, &r, a, d).unwrap();
        if let Some((_, hull)) = w.units.iter().find(|(id, u)| *id != a && u.owner == CivId(0)) {
            captured += 1;
            assert_eq!(hull.base, r.unit("Trireme").unwrap());
            assert_eq!(hull.health, 50);
            assert_eq!(hull.movement_left, Fixed::ZERO);
        };
    }
    assert!(captured > 0 && captured < 32);
}

#[test]
fn shared_round_does_not_choose_human_upgrades_or_promotions() {
    let (r, mut w) = fixture();
    let centre = tile(&w, 4, 4);
    w.found_city_in(&r, City::found("Human", CivId(0), centre));
    let id = unit(&mut w, &r, 0, "Warrior", 4, 4);
    w.units.get_mut(id).unwrap().xp = 30;
    w.civ_mut(CivId(0)).unwrap().gold = 1000;
    w.civ_mut(CivId(0)).unwrap().techs.extend(r.technologies.ids());
    civ_core::world::turn::finish_round(&mut w, &r, civ_core::world::CityFocus::None, Default::default());
    assert_eq!(w.units.get(id).unwrap().base, r.unit("Warrior").unwrap());
    assert_eq!(w.units.get(id).unwrap().promotions_earned, 0);
}

#[test]
fn tile_defense_uses_feature_override_and_unpillaged_fort_or_citadel() {
    let (r, mut w) = fixture();
    let id = unit(&mut w, &r, 0, "Warrior", 4, 4);
    let pos = tile(&w, 4, 4);
    assert_eq!(combat::defending_strength(&w, &r, id), Some(Fixed::whole(8)));
    w.grid.features[pos.0 as usize] = vec![r.terrain("Hill").unwrap(), r.terrain("Forest").unwrap()];
    assert_eq!(
        combat::defending_strength(&w, &r, id),
        Some(Fixed::whole(10)),
        "feature defense does not stack hill and forest"
    );
    w.grid.set_improvement(pos, r.improvements.id("Fort"));
    assert_eq!(combat::defending_strength(&w, &r, id), Some(Fixed::whole(14)));
    w.grid.set_improvement(pos, r.improvements.id("Citadel"));
    assert_eq!(combat::defending_strength(&w, &r, id), Some(Fixed::whole(18)));
    w.grid.improvement_pillaged[pos.0 as usize] = true;
    assert_eq!(combat::defending_strength(&w, &r, id), Some(Fixed::whole(10)));
    w.grid.improvement_pillaged[pos.0 as usize] = false;
    w.units.get_mut(id).unwrap().base = r.unit("Horseman").unwrap();
    assert_eq!(
        combat::defending_strength(&w, &r, id),
        Some(Fixed::whole(12)),
        "no defensive terrain bonus includes fort/citadel defense"
    );
}

#[test]
fn civilization_free_promotions_apply_to_prepared_units_without_earned_xp() {
    let (r, mut w) = fixture();
    w.civ_mut(CivId(0)).unwrap().nation = r.nations.id("Songhai").unwrap();
    civ_core::world::policies::rebuild_uniques(&mut w, &r, CivId(0));
    let pos = tile(&w, 4, 4);
    let soldier =
        civ_core::world::construction::prepare_unit_near(&w, &r, CivId(0), r.unit("Warrior").unwrap(), pos, None)
            .unwrap();
    assert!(soldier.promotions.contains(&r.promotions.id("Amphibious").unwrap()));
    assert_eq!(soldier.promotions_earned, 0);
    assert_eq!(soldier.xp, 0);
    let worker =
        civ_core::world::construction::prepare_unit_near(&w, &r, CivId(0), r.unit("Worker").unwrap(), pos, None)
            .unwrap();
    assert!(!worker.promotions.contains(&r.promotions.id("Amphibious").unwrap()));
    w.civ_mut(CivId(0)).unwrap().nation = r.nations.id("Barbarians").unwrap();
    civ_core::world::policies::rebuild_uniques(&mut w, &r, CivId(0));
    w.grid.terrain[pos.0 as usize] = r.terrain("Coast").unwrap();
    let ship =
        civ_core::world::construction::prepare_unit_near(&w, &r, CivId(0), r.unit("Trireme").unwrap(), pos, None)
            .unwrap();
    assert!(ship.has_unique(&r, civ_core::UniqueType::CannotPillage));
}

#[test]
fn difficulty_bonus_uses_match_setting_for_human_ai_and_city_combat() {
    use civ_core::world::air::Target;
    let (r, mut w) = fixture();
    let a = unit(&mut w, &r, 0, "Warrior", 4, 4);
    let d = unit(&mut w, &r, 1, "Warrior", 5, 4);
    w.civ_mut(CivId(1)).unwrap().is_barbarian = true;
    assert_eq!(w.game_difficulty(&r).unwrap().name, "Prince");
    let strength = combat::combat_strength(&w, &r, a, Target::Unit(d), false).unwrap();
    assert_eq!(strength, Fixed::whole(8) * Fixed::permille(1330));
    w.difficulty = r.difficulties.id("Settler");
    assert_eq!(combat::combat_strength(&w, &r, a, Target::Unit(d), false), Some(Fixed::whole(14)));
    w.civ_mut(CivId(0)).unwrap().is_ai = true;
    assert_eq!(
        combat::combat_strength(&w, &r, a, Target::Unit(d), false),
        Some(Fixed::whole(14)),
        "battle uses match difficulty even for AI"
    );
    w.difficulty = r.difficulties.id("Deity");
    assert_eq!(combat::combat_strength(&w, &r, a, Target::Unit(d), false), Some(Fixed::whole(8)));
    let centre = tile(&w, 4, 4);
    let city = w.found_city_in(&r, City::found("Capital", CivId(0), centre));
    let mut easy = w.clone();
    easy.difficulty = r.difficulties.id("Settler");
    let normal = combat::city_bombard(&mut w, &r, city, d).unwrap();
    let boosted = combat::city_bombard(&mut easy, &r, city, d).unwrap();
    assert!(boosted.damage_to_defender > normal.damage_to_defender);
}

#[cfg(feature = "save")]
#[test]
fn difficulty_is_persistent_and_unknown_ruleset_ids_are_rejected() {
    let (r, mut w) = fixture();
    w.difficulty = r.difficulties.id("King");
    let encoded = civ_core::save::encode(&w, &r).unwrap();
    let copy = civ_core::save::decode(&encoded, &r).unwrap();
    assert_eq!(copy, w);
    let mut value = serde_json::to_value(&w).unwrap();
    value["difficulty"] = serde_json::json!([99999, 1]);
    let invalid: World = serde_json::from_value(value).unwrap();
    assert!(civ_core::save::encode(&invalid, &r).is_err());
}

#[test]
fn ranged_and_city_attacks_accelerate_live_barbarian_camps_even_without_kills() {
    use civ_core::world::barbarians::{camp_improvement, Camp};
    let (r, mut w) = fixture();
    let a = unit(&mut w, &r, 0, "Archer", 4, 4);
    let d = unit(&mut w, &r, 1, "Infantry", 5, 4);
    let camp = tile(&w, 5, 4);
    w.civ_mut(CivId(1)).unwrap().is_barbarian = true;
    w.grid.set_improvement(camp, camp_improvement(&r));
    w.barbarian_camps.insert(camp, Camp { countdown: 10, spawned_units: 0, destroyed: false });
    combat::resolve_ranged_attack(&mut w, &r, a, camp).unwrap();
    assert!(w.units.get(d).is_some());
    assert_eq!(w.barbarian_camps[&camp].countdown, 5);
    let centre = tile(&w, 4, 4);
    let city = w.found_city_in(&r, City::found("Capital", CivId(0), centre));
    combat::city_bombard(&mut w, &r, city, d).unwrap();
    assert_eq!(w.barbarian_camps[&camp].countdown, 2);
}

#[test]
fn withdrawal_never_teleports_onto_impassable_mountains() {
    let (r, mut original) = fixture();
    let a = unit(&mut original, &r, 0, "Warrior", 4, 4);
    let d = unit(&mut original, &r, 1, "Slinger", 5, 4);
    let from = original.units.get(d).unwrap().position;
    let attacker = original.units.get(a).unwrap().position;
    for next in original.grid.neighbours(from).collect::<Vec<_>>() {
        if next != attacker {
            original.grid.terrain[next.0 as usize] = r.terrain("Mountain").unwrap();
        }
    }
    for turn in 0..32 {
        let mut w = original.clone();
        w.turn = turn;
        assert!(!military::withdraw(&mut w, &r, a, d));
        assert_eq!(w.units.get(d).unwrap().position, from);
    }
}

#[test]
fn mounted_victory_advance_ignores_zoc_but_ordinary_movement_does_not() {
    let (r, mut w) = fixture();
    let a = unit(&mut w, &r, 0, "Horseman", 4, 4);
    let d = unit(&mut w, &r, 1, "Warrior", 5, 4);
    let from = w.units.get(a).unwrap().position;
    let to = w.units.get(d).unwrap().position;
    let zoc = w.grid.neighbours(from).find(|t| w.grid.distance(*t, to) == 1).unwrap();
    w.place_unit(Unit::new(r.unit("Warrior").unwrap(), CivId(1), zoc, 2));
    assert!(civ_core::world::movement::is_movement_affected_by_zoc(&w, &r, w.units.get(a).unwrap(), from, to));
    w.units.get_mut(d).unwrap().health = 1;
    let result = combat::resolve_attack(&mut w, &r, a, d).unwrap();
    assert!(result.defender_died);
    let horse = w.units.get(a).unwrap();
    assert_eq!(horse.position, to);
    assert_eq!(horse.movement_left, Fixed::whole(3));
}

#[test]
fn attack_previews_bracket_every_rolled_outcome() {
    let (r, original) = fixture();
    for (attacker, defender, ranged) in [("Warrior", "Warrior", false), ("Spearman", "Horseman", false), ("Archer", "Spearman", true)] {
        let mut w = original.clone();
        let a = unit(&mut w, &r, 0, attacker, 4, 4);
        let d = unit(&mut w, &r, 1, defender, 5, 4);
        let p = combat::preview_attack(&w, &r, a, air::Target::Unit(d)).unwrap();
        assert_eq!(p.ranged, ranged, "{attacker}");
        assert!(!p.capture);
        assert!(p.defender.damage.0 <= p.defender.damage.1 && p.defender.damage.1 > 0, "{attacker}: {p:?}");
        if ranged {
            assert_eq!(p.attacker.damage, (0, 0));
        }
        for turn in 0..24 {
            let mut t = w.clone();
            t.turn = turn;
            let at = tile(&t, 5, 4);
            let result = if ranged {
                combat::resolve_ranged_attack(&mut t, &r, a, at)
            } else {
                combat::resolve_attack(&mut t, &r, a, d)
            }
            .unwrap();
            // Healthy units survive one exchange, so each rolled figure sits inside the previewed range.
            let (low, high) = p.defender.damage;
            assert!((low..=high).contains(&result.damage_to_defender), "{attacker} turn {turn}: {result:?} vs {p:?}");
            let (low, high) = p.attacker.damage;
            assert!((low..=high).contains(&result.damage_to_attacker), "{attacker} turn {turn}: {result:?} vs {p:?}");
        }
    }
}

#[test]
fn attack_previews_show_captures_and_city_limits() {
    let (r, mut w) = fixture();
    let a = unit(&mut w, &r, 0, "Warrior", 4, 4);
    let worker = unit(&mut w, &r, 1, "Worker", 5, 4);
    let p = combat::preview_attack(&w, &r, a, air::Target::Unit(worker)).unwrap();
    assert!(p.capture);
    assert_eq!(p.defender.damage, (0, 0));

    let city = w.found_city_in(&r, City::found("Walls", CivId(1), tile(&w, 4, 5)));
    let p = combat::preview_attack(&w, &r, a, air::Target::City(city)).unwrap();
    let c = w.cities.get(city).unwrap();
    assert_eq!((p.defender.health, p.defender.max_health), (c.health, civ_core::world::city::max_health(&r, c)));
    assert!(p.defender.damage.1 < c.health, "a city never drops below one health: {p:?}");
    assert!(p.defender.modifiers.is_empty() && !p.capture);
}

#[test]
fn attack_commands_report_who_fought_and_what_it_cost() {
    use civ_core::world::commands::{self, AttackTarget, Command};
    use civ_core::world::turn::TurnEvent;
    let (r, mut w) = fixture();
    let a = unit(&mut w, &r, 0, "Warrior", 4, 4);
    let d = unit(&mut w, &r, 1, "Warrior", 5, 4);
    let seen = tile(&w, 5, 4);
    w.visibility.observe(CivId(0), seen);
    let outcome = commands::execute(&mut w, &r, CivId(0), Command::Attack { unit: a, target: AttackTarget::Unit(d) }).unwrap();
    let report = outcome
        .events
        .iter()
        .find_map(|e| match e {
            TurnEvent::Battle { report: Some(b), .. } => Some(b.clone()),
            _ => None,
        })
        .expect("the attack reports its participants");
    assert_eq!((report.attacker, report.attacker_owner, report.defender_owner), (air::Target::Unit(a), CivId(0), CivId(1)));
    assert_eq!((report.target, report.from, report.at, report.ranged), (air::Target::Unit(d), tile(&w, 4, 4), tile(&w, 5, 4), false));
    assert_eq!(report.damage_to_attacker, 100 - i32::from(w.units.get(a).map_or(0, |u| u.health)));
    assert_eq!(report.damage_to_defender, 100 - i32::from(w.units.get(d).map_or(0, |u| u.health)));
    assert!(report.damage_to_defender > 0);
}

#[test]
fn human_cities_put_their_first_citizen_to_work() {
    use civ_core::world::commands::{self, Command};
    let (r, mut w) = fixture();
    let settler = unit(&mut w, &r, 0, "Settler", 8, 6);
    commands::execute(&mut w, &r, CivId(0), Command::FoundCity { unit: settler, name: "Pasargadae".into() }).unwrap();
    let city = w.cities.iter().find(|(_, c)| c.owner == CivId(0)).unwrap().1;
    assert!(!w.civ(CivId(0)).unwrap().is_ai);
    assert_eq!((city.population, city.worked.len()), (1, 1), "the founding citizen works a tile at once");
}

#[test]
fn bombard_targets_and_previews_match_the_bombard() {
    let (r, mut w) = fixture();
    let city = w.found_city_in(&r, City::found("Walls", CivId(0), tile(&w, 4, 4)));
    let near = unit(&mut w, &r, 1, "Warrior", 6, 4);
    let far = unit(&mut w, &r, 1, "Warrior", 8, 4);
    for t in [tile(&w, 6, 4), tile(&w, 8, 4)] {
        w.visibility.observe(CivId(0), t);
    }
    let targets = combat::bombard_targets(&w, &r, city);
    assert!(targets.contains(&near) && !targets.contains(&far), "{targets:?}");
    let (low, high) = combat::preview_bombard(&w, &r, city, near).unwrap();
    for turn in 0..16 {
        let mut t = w.clone();
        t.turn = turn;
        let before = t.units.get(near).unwrap().health;
        combat::city_bombard(&mut t, &r, city, near).unwrap();
        let dealt = i32::from(before) - t.units.get(near).map_or(0, |u| i32::from(u.health));
        assert!((low..=high).contains(&dealt), "turn {turn}: {dealt} not in {low}..={high}");
    }
}

/// Seeded rounds until a Slinger withdraws from a Warrior, through `attack`, which returns the events.
fn first_withdrawal(
    mut attack: impl FnMut(&mut World, Id<Unit>, Id<Unit>) -> Vec<civ_core::world::turn::TurnEvent>,
) -> (World, Id<Unit>, Id<Unit>, Vec<civ_core::world::turn::TurnEvent>) {
    let (r, mut original) = fixture();
    let a = unit(&mut original, &r, 0, "Warrior", 4, 4);
    let d = unit(&mut original, &r, 1, "Slinger", 5, 4);
    civ_core::world::turn::update_visibility_in(&mut original, &r);
    for turn in 0..32 {
        let mut w = original.clone();
        w.turn = turn;
        let events = attack(&mut w, a, d);
        if w.units.get(d).is_some_and(|u| u.position != tile(&w, 5, 4)) {
            return (w, a, d, events);
        }
    }
    panic!("no seeded round withdrew");
}

/// A withdrawal is its own fact, just before the defender's zero-cost step, from a player's attack and
/// from an AI's alike.
#[test]
fn a_withdrawal_is_reported_before_its_step_on_both_paths() {
    use civ_core::world::commands::{execute, AttackTarget, Command};
    use civ_core::world::turn::{apply_plan, plan_economy, TurnEvent, TurnModifiers};
    let (r, _) = fixture();
    let human = first_withdrawal(|w, a, d| {
        execute(w, &r, CivId(0), Command::Attack { unit: a, target: AttackTarget::Unit(d) }).unwrap().events
    });
    let ai = first_withdrawal(|w, a, d| {
        let mut plan = plan_economy(w, &r, CivId(0), civ_core::world::CityFocus::None, TurnModifiers::default());
        plan.orders = vec![(a, ai::UnitOrder::Attack(d))];
        apply_plan(w, &r, &plan, TurnModifiers::default())
    });
    for (w, a, d, events) in [human, ai] {
        let (from, to) = (tile(&w, 5, 4), w.units.get(d).unwrap().position);
        let at = events.iter().position(|e| matches!(e, TurnEvent::UnitWithdrew { .. })).expect("reported");
        assert_eq!(events[at], TurnEvent::UnitWithdrew { unit: d, from, to, attacker: a });
        assert!(
            matches!(events[at + 1], TurnEvent::UnitMoved(m) if m.unit == d && m.from == from && m.to == to && m.spent == Fixed::ZERO),
            "{events:?}"
        );
    }
}

/// An AI's capture is reported like a player's: after the battle, with what the Settler became.
#[test]
fn an_ai_capture_is_reported_after_its_battle() {
    use civ_core::world::turn::{apply_plan, plan_economy, TurnEvent, TurnModifiers};
    let (r, mut w) = fixture();
    let a = unit(&mut w, &r, 0, "Warrior", 4, 4);
    let settler = unit(&mut w, &r, 1, "Settler", 5, 4);
    let mut plan = plan_economy(&w, &r, CivId(0), civ_core::world::CityFocus::None, TurnModifiers::default());
    plan.orders = vec![(a, ai::UnitOrder::Attack(settler))];
    let events = apply_plan(&mut w, &r, &plan, TurnModifiers::default());
    let battle = events.iter().position(|e| matches!(e, TurnEvent::Battle { .. })).expect("a battle");
    assert_eq!(
        events[battle + 1],
        TurnEvent::UnitCaptured {
            unit: settler,
            at: tile(&w, 5, 4),
            from_owner: CivId(1),
            to_owner: CivId(0),
            by: a,
            becomes: r.unit("Worker"),
        }
    );
}

/// Pillaging reports what the unit took and healed, where it stood.
#[test]
fn pillaging_is_reported() {
    use civ_core::world::commands::{execute, Command};
    use civ_core::world::turn::TurnEvent;
    let (r, mut w) = fixture();
    let id = unit(&mut w, &r, 0, "Warrior", 4, 4);
    let pos = tile(&w, 4, 4);
    w.grid.improvement[pos.0 as usize] = Some(r.improvements.id("Farm").unwrap());
    w.grid.owner[pos.0 as usize] = Some(CivId(1));
    w.units.get_mut(id).unwrap().health = 50;
    let events = execute(&mut w, &r, CivId(0), Command::Pillage { unit: id }).unwrap().events;
    let Some(TurnEvent::Pillaged(p)) = events.first() else { panic!("{events:?}") };
    assert_eq!((p.unit, p.tile), (id, pos));
    assert_eq!(i32::from(p.healed), i32::from(w.units.get(id).unwrap().health) - 50);
    assert!(p.healed > 0);
}
