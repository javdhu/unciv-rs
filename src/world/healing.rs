//! Recovery at the end of a round, before action history is reset.

use crate::arena::Id;
use crate::ruleset::Ruleset;
use crate::world::conditionals::{applies, GameContext};
use crate::world::{City, CivId, Unit, World};
use crate::{Unique, UniqueType};

fn matching<'a>(world: &'a World, r: &'a Ruleset, id: Id<Unit>, kind: UniqueType, civ: bool) -> Vec<&'a Unique> {
    let Some(unit) = world.units.get(id) else { return Vec::new() };
    let ctx = GameContext::new(r).with_world(world).for_civ(unit.owner).for_unit(id).on_tile(unit.position);
    let base = r.units.get(unit.base);
    base.into_iter()
        .flat_map(|b| &b.uniques)
        .chain(base.and_then(|b| r.unit_types.by_name(&b.unit_type)).into_iter().flat_map(|t| &t.uniques))
        .chain(unit.promotions.iter().filter_map(|p| r.promotions.get(*p)).flat_map(|p| &p.uniques))
        .chain(world.civ(unit.owner).filter(|_| civ).into_iter().flat_map(|c| c.uniques.all()))
        .filter(|u| u.unique_type() == Some(kind) && applies(u, &ctx).holds())
        .collect()
}

fn amount(unique: &Unique, index: usize) -> i32 {
    unique.params.get(index).and_then(|p| p.parse().ok()).unwrap_or(0)
}

fn allies(world: &World, a: CivId, b: CivId) -> bool {
    a == b
        || (world.civ(a).is_some_and(|c| c.is_city_state()) && crate::world::city_states::ally_of(world, a) == Some(b))
        || (world.civ(b).is_some_and(|c| c.is_city_state()) && crate::world::city_states::ally_of(world, b) == Some(a))
}

fn city_bonus(world: &World, r: &Ruleset, id: Id<Unit>, city_id: Id<City>, city: &City) -> Option<i32> {
    let unit = world.units.get(id).expect("healing unit exists");
    let ctx =
        GameContext::new(r).with_world(world).for_civ(city.owner).in_city(city_id).for_unit(id).on_tile(unit.position);
    let uniques: Vec<_> = crate::world::religion::city_uniques(world, r, city)
        .into_iter()
        .filter(|u| u.unique_type() == Some(UniqueType::CityHealingUnits) && applies(u, &ctx).holds())
        .collect();
    if uniques.is_empty() {
        return None;
    }
    // Kotlin selects the first city with an aura before checking alliance or
    // the recipient filter. An inapplicable aura still excludes later cities.
    if !allies(world, unit.owner, city.owner) {
        return Some(0);
    }
    Some(
        uniques
            .into_iter()
            .filter(|u| {
                u.params.first().is_some_and(|filter| crate::world::filters::unit_matches(r, unit, filter).holds())
            })
            .map(|u| amount(u, 1))
            .sum(),
    )
}

/// Health this unit can recover by resting now, including healing modifiers.
/// A Skip/Fortify command spends available movement but does not cancel rest.
pub fn unit_healing(world: &World, r: &Ruleset, id: Id<Unit>) -> i32 {
    let Some(unit) = world.units.get(id) else { return 0 };
    if !unit.is_hurt()
        || unit.embarked
        || !matching(world, r, id, UniqueType::HealOnlyByPillaging, true).is_empty()
        || ((unit.moved_this_turn || unit.attacked_this_turn)
            && matching(world, r, id, UniqueType::HealsEvenAfterAction, false).is_empty())
    {
        return 0;
    }
    let friendly = crate::world::movement::is_friendly_territory(world, unit.owner, unit.position);
    let water = world.grid.is_water(r, unit.position);
    let city = world.cities.iter().any(|(_, c)| c.centre == unit.position);
    let mut healing = if city {
        25
    } else if water {
        if friendly
            && (unit.is_water_unit(r)
                || unit.transport.is_some()
                || !matching(world, r, id, UniqueType::CanMoveOnWater, false).is_empty())
        {
            20
        } else {
            0
        }
    } else if friendly {
        20
    } else {
        10
    };
    if healing == 0 && (!water || matching(world, r, id, UniqueType::HealsOutsideFriendlyTerritory, true).is_empty()) {
        return 0;
    }
    healing += matching(world, r, id, UniqueType::Heal, true).into_iter().map(|u| amount(u, 0)).sum::<i32>();
    // City auras do not stack. Use stable tile order for adjacent cities.
    for tile in world.grid.within(unit.position, 1) {
        if let Some((city_id, city)) = world.cities.iter().find(|(_, c)| c.centre == tile) {
            if let Some(bonus) = city_bonus(world, r, id, city_id, city) {
                healing += bonus;
                break;
            }
        }
    }
    // Multiple promotions on one medic add; multiple medics do not stack.
    healing += world
        .grid
        .neighbours(unit.position)
        .flat_map(|t| world.units_at(t))
        .filter(|other| world.units.get(*other).is_some_and(|u| u.owner == unit.owner))
        .map(|other| {
            matching(world, r, other, UniqueType::HealAdjacentUnits, false)
                .into_iter()
                .map(|u| amount(u, 0))
                .sum::<i32>()
        })
        .max()
        .unwrap_or(0);
    modified_amount(world, r, id, healing)
}

fn modified_amount(world: &World, r: &Ruleset, id: Id<Unit>, amount: i32) -> i32 {
    let double = !matching(world, r, id, UniqueType::HealingEffectsDoubled, true).is_empty();
    amount.max(0).saturating_mul(if double { 2 } else { 1 })
}

/// Restore health for immediate effects such as pillaging, honoring the shared
/// healing multiplier and cap. Returns the actual health restored.
pub fn heal(world: &mut World, r: &Ruleset, id: Id<Unit>, amount: i32) -> u8 {
    let amount = modified_amount(world, r, id, amount);
    let Some(unit) = world.units.get_mut(id) else { return 0 };
    let old = unit.health;
    unit.health = (i32::from(old).saturating_add(amount)).min(100) as u8;
    unit.health - old
}

/// Compute all recovery before mutating health, so arena order cannot change
/// conditional medic bonuses. Action history is reset by movement refresh.
pub fn recover(world: &mut World, r: &Ruleset) {
    for (_, unit) in world.units.iter_mut() {
        if !unit.fortified {
            unit.fortification_turns = 0;
        } else if !unit.moved_this_turn {
            unit.fortification_turns = unit.fortification_turns.saturating_add(1).min(2);
        }
    }
    let healing: Vec<_> = world.units.ids().into_iter().map(|id| (id, unit_healing(world, r, id))).collect();
    for (id, amount) in healing {
        if let Some(unit) = world.units.get_mut(id) {
            unit.health = (i32::from(unit.health).saturating_add(amount)).min(100) as u8;
        }
    }
    for (_, city) in world.cities.iter_mut() {
        city.health = city.health.saturating_add(20).min(crate::world::city::max_health(r, city));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::Fixed;
    use crate::world::commands::{execute, AttackTarget, Command};
    use crate::world::turn::{finish_round, update_visibility, TurnModifiers};
    use crate::world::{CityFocus, Civilization, TileGrid};

    fn fixture() -> (Ruleset, World) {
        let r = Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings").unwrap().unwrap();
        let nation = r.nations.id("Rome").unwrap();
        let civs = (0..3).map(|i| Civilization::new(CivId(i), nation, false)).collect();
        let mut world = World::new(TileGrid::new(16, 10, r.terrain("Grassland").unwrap()), civs);
        for (owner, x) in [(0, 2), (1, 8), (2, 13)] {
            let city = City::found("Rest", CivId(owner), world.grid.id_at(x, 5).unwrap());
            world.found_city_in(&r, city);
        }
        world.civ_mut(CivId(2)).unwrap().city_state_type = r.city_state_types.ids().next();
        (r, world)
    }

    fn unit(world: &mut World, r: &Ruleset, owner: u8, name: &str, x: u16, y: u16) -> Id<Unit> {
        let base = r.unit(name).unwrap();
        let mut unit =
            Unit::new(base, CivId(owner), world.grid.id_at(x, y).unwrap(), r.units.get(base).unwrap().movement);
        unit.health = 40;
        world.place_unit(unit)
    }

    fn promotion(world: &mut World, r: &Ruleset, id: Id<Unit>, name: &str) {
        world.units.get_mut(id).unwrap().promotions.insert(r.promotions.id(name).unwrap());
    }

    fn observe(world: &mut World, r: &Ruleset) {
        update_visibility(world);
        crate::world::fog::capture_observations(world, r);
    }

    #[test]
    fn recovery_uses_territory_water_and_embarkation_rules() {
        let (r, mut w) = fixture();
        let id = unit(&mut w, &r, 0, "Warrior", 4, 5);
        let tile = w.units.get(id).unwrap().position;
        assert_eq!(unit_healing(&w, &r, id), 10);
        w.grid.owner[tile.0 as usize] = Some(CivId(0));
        assert_eq!(unit_healing(&w, &r, id), 20);
        w.grid.owner[tile.0 as usize] = Some(CivId(1));
        assert_eq!(unit_healing(&w, &r, id), 10);
        w.units.get_mut(id).unwrap().position = w.grid.id_at(2, 5).unwrap();
        assert_eq!(unit_healing(&w, &r, id), 25);
        w.units.get_mut(id).unwrap().position = tile;
        w.grid.terrain[tile.0 as usize] = r.terrain("Coast").unwrap();
        w.units.get_mut(id).unwrap().embarked = true;
        assert_eq!(unit_healing(&w, &r, id), 0);
        let ship = unit(&mut w, &r, 0, "Trireme", 5, 5);
        let sea = w.units.get(ship).unwrap().position;
        w.grid.terrain[sea.0 as usize] = r.terrain("Coast").unwrap();
        assert_eq!(unit_healing(&w, &r, ship), 0);
        promotion(&mut w, &r, ship, "Supply");
        assert_eq!(unit_healing(&w, &r, ship), 15);
        w.grid.owner[sea.0 as usize] = Some(CivId(0));
        assert_eq!(unit_healing(&w, &r, ship), 20, "Supply's foreign bonus stops in friendly water");
    }

    #[test]
    fn friendly_treaties_and_city_states_match_healing_conditionals() {
        let (r, mut w) = fixture();
        let id = unit(&mut w, &r, 0, "Warrior", 4, 5);
        promotion(&mut w, &r, id, "Medic II");
        let tile = w.units.get(id).unwrap().position;
        w.grid.owner[tile.0 as usize] = Some(CivId(1));
        w.diplomacy.meet(CivId(0), CivId(1));
        assert_eq!(unit_healing(&w, &r, id), 15);
        w.diplomacy.set_open_borders(CivId(0), CivId(1), true);
        assert_eq!(unit_healing(&w, &r, id), 15, "granting passage is directional");
        w.diplomacy.set_open_borders(CivId(1), CivId(0), true);
        assert_eq!(unit_healing(&w, &r, id), 20);
        w.diplomacy.declare_war(CivId(0), CivId(1));
        assert_eq!(unit_healing(&w, &r, id), 15);
        w.grid.owner[tile.0 as usize] = Some(CivId(2));
        w.diplomacy.meet(CivId(0), CivId(2));
        assert_eq!(unit_healing(&w, &r, id), 15);
        w.diplomacy.add_influence(CivId(2), CivId(0), Fixed::whole(30));
        assert_eq!(unit_healing(&w, &r, id), 20);
        w.diplomacy.add_influence(CivId(2), CivId(0), Fixed::whole(-30));
        w.civ_mut(CivId(0))
            .unwrap()
            .uniques
            .insert(Unique::parse("City-State territory always counts as friendly territory"));
        assert_eq!(unit_healing(&w, &r, id), 20);
    }

    #[test]
    fn adjacent_medics_do_not_stack_but_their_promotions_do() {
        let (r, mut w) = fixture();
        let id = unit(&mut w, &r, 0, "Warrior", 5, 5);
        let adjacent: Vec<_> = w.grid.neighbours(w.units.get(id).unwrap().position).collect();
        let a = unit(&mut w, &r, 0, "Warrior", 0, 0);
        w.units.get_mut(a).unwrap().position = adjacent[0];
        promotion(&mut w, &r, a, "Medic");
        promotion(&mut w, &r, a, "Medic II");
        let b = unit(&mut w, &r, 0, "Warrior", 0, 1);
        w.units.get_mut(b).unwrap().position = adjacent[1];
        promotion(&mut w, &r, b, "Medic");
        assert_eq!(unit_healing(&w, &r, id), 20, "10 base plus the strongest medic's 10");
        w.transfer_unit(a, CivId(1));
        assert_eq!(unit_healing(&w, &r, id), 15, "foreign medics provide no bonus");
        promotion(&mut w, &r, id, "Rejuvenation");
        assert_eq!(unit_healing(&w, &r, id), 30);
        w.units.get_mut(id).unwrap().health = 90;
        recover(&mut w, &r);
        assert_eq!(w.units.get(id).unwrap().health, 100);
    }

    #[test]
    fn city_aura_follows_local_faith_and_stacks_with_one_medic() {
        let (r, mut w) = fixture();
        let city_id = w.civ(CivId(0)).unwrap().cities[0];
        let tile = w.cities.get(city_id).unwrap().centre;
        let id = unit(&mut w, &r, 0, "Warrior", 2, 5);
        let healer = r.beliefs.id("Faith Healers").unwrap();
        w.civ_mut(CivId(0)).unwrap().faith.beliefs.insert(healer);
        crate::world::religion::on_pantheon_founded(&mut w, CivId(0));
        crate::world::policies::rebuild_uniques(&mut w, &r, CivId(0));
        assert_eq!(unit_healing(&w, &r, id), 55);
        let medic = unit(&mut w, &r, 0, "Warrior", 0, 0);
        w.units.get_mut(medic).unwrap().position = w.grid.neighbours(tile).next().unwrap();
        promotion(&mut w, &r, medic, "Medic");
        assert_eq!(unit_healing(&w, &r, id), 60);
        let city = w.cities.get_mut(city_id).unwrap();
        city.faith.add_pressure(Some(CivId(1)), 1000);
        city.faith.recount(city.population);
        assert_eq!(unit_healing(&w, &r, id), 30, "a converted city no longer uses its founder's pantheon aura");
    }

    #[test]
    fn round_heals_skip_fortify_and_cities_then_resets_history() {
        let (r, mut w) = fixture();
        let skipped = unit(&mut w, &r, 0, "Warrior", 4, 4);
        let fortified = unit(&mut w, &r, 0, "Warrior", 5, 4);
        let moved = unit(&mut w, &r, 0, "Warrior", 4, 6);
        observe(&mut w, &r);
        execute(&mut w, &r, CivId(0), Command::SkipUnit { unit: skipped }).unwrap();
        execute(&mut w, &r, CivId(0), Command::Fortify { unit: fortified }).unwrap();
        let to = w.grid.id_at(5, 6).unwrap();
        execute(&mut w, &r, CivId(0), Command::MoveUnit { unit: moved, to }).unwrap();
        assert!(w.units.get(moved).unwrap().moved_this_turn);
        assert!(!w.units.get(skipped).unwrap().moved_this_turn);
        let city_id = w.civ(CivId(0)).unwrap().cities[0];
        w.cities.get_mut(city_id).unwrap().health = 190;
        finish_round(&mut w, &r, CityFocus::None, TurnModifiers::default());
        assert_eq!(w.units.get(skipped).unwrap().health, 50);
        assert_eq!(w.units.get(fortified).unwrap().health, 50);
        assert_eq!(w.units.get(moved).unwrap().health, 40);
        assert!(!w.units.get(moved).unwrap().moved_this_turn);
        assert_eq!(w.cities.get(city_id).unwrap().health, 200);
        w.cities.get_mut(city_id).unwrap().built.insert(r.buildings.id("Walls").unwrap());
        recover(&mut w, &r);
        assert_eq!(w.cities.get(city_id).unwrap().health, 220, "building health affects the cap");
    }

    #[test]
    fn attacks_prevent_rest_but_defending_and_rejected_actions_do_not() {
        let (r, mut w) = fixture();
        let a = unit(&mut w, &r, 0, "Warrior", 5, 5);
        let d = unit(&mut w, &r, 1, "Warrior", 6, 5);
        w.units.get_mut(a).unwrap().health = 90;
        w.units.get_mut(d).unwrap().health = 90;
        w.diplomacy.declare_war(CivId(0), CivId(1));
        observe(&mut w, &r);
        let before = w.units.get(a).unwrap().clone();
        assert!(execute(&mut w, &r, CivId(1), Command::Attack { unit: a, target: AttackTarget::Unit(d) }).is_err());
        assert_eq!(w.units.get(a).unwrap(), &before);
        execute(&mut w, &r, CivId(0), Command::Attack { unit: a, target: AttackTarget::Unit(d) }).unwrap();
        assert!(w.units.get(a).unwrap().attacked_this_turn);
        assert!(!w.units.get(d).unwrap().attacked_this_turn);
        assert_eq!(unit_healing(&w, &r, a), 0);
        assert_eq!(unit_healing(&w, &r, d), 10);
        let hurt = w.units.get(a).unwrap().health;
        promotion(&mut w, &r, a, "Air Repair");
        assert_eq!(unit_healing(&w, &r, a), 10, "repair overrides action history");
        recover(&mut w, &r);
        assert_eq!(w.units.get(a).unwrap().health, hurt + 10);
        w.civ_mut(CivId(0)).unwrap().uniques.insert(Unique::parse("Can only heal by pillaging"));
        assert_eq!(unit_healing(&w, &r, a), 0, "pillaging-only wins over repair");
    }

    #[test]
    fn low_level_moves_rebase_and_interception_record_action_history() {
        let (r, mut w) = fixture();
        let walker = unit(&mut w, &r, 0, "Warrior", 4, 4);
        let destination = w.grid.id_at(5, 4).unwrap();
        crate::world::movement::move_unit(&mut w, &r, walker, destination).unwrap();
        assert_eq!(unit_healing(&w, &r, walker), 0);
        let bomber = unit(&mut w, &r, 0, "Bomber", 2, 5);
        w.units.get_mut(bomber).unwrap().health = 100;
        let rebase = w.found_city_in(&r, City::found("Base", CivId(0), w.grid.id_at(5, 2).unwrap()));
        let base = w.cities.get(rebase).unwrap().centre;
        crate::world::combat::rebase_air_unit(&mut w, &r, bomber, base).unwrap();
        assert!(w.units.get(bomber).unwrap().moved_this_turn);
        w.units.get_mut(bomber).unwrap().refresh(2);
        let fighter = unit(&mut w, &r, 1, "Fighter", 8, 5);
        w.diplomacy.declare_war(CivId(0), CivId(1));
        let target = w.grid.id_at(8, 5).unwrap();
        crate::world::combat::air_strike(&mut w, &r, bomber, target).unwrap();
        assert!(w.units.get(fighter).unwrap().attacked_this_turn);
        assert_eq!(unit_healing(&w, &r, fighter), 0);
    }

    #[test]
    fn pillage_uses_shared_healing_multiplier_and_spends_rest() {
        let (r, mut w) = fixture();
        let id = unit(&mut w, &r, 0, "Warrior", 4, 5);
        promotion(&mut w, &r, id, "Rejuvenation");
        let tile = w.units.get(id).unwrap().position;
        w.grid.improvement[tile.0 as usize] = r.improvements.id("Farm");
        assert_eq!(crate::world::combat::pillage(&mut w, &r, id).unwrap().healed, 50);
        assert_eq!(w.units.get(id).unwrap().health, 90);
        assert_eq!(unit_healing(&w, &r, id), 0);
    }

    #[test]
    fn working_in_place_allows_recovery_but_upgrading_does_not() {
        let (r, mut w) = fixture();
        let worker = unit(&mut w, &r, 0, "Worker", 3, 5);
        let city = w.civ(CivId(0)).unwrap().cities[0];
        w.claim_tile(city, w.units.get(worker).unwrap().position);
        w.civ_mut(CivId(0)).unwrap().techs.insert(r.technologies.id("Agriculture").unwrap());
        observe(&mut w, &r);
        execute(
            &mut w,
            &r,
            CivId(0),
            Command::Improve { unit: worker, improvement: r.improvements.id("Farm").unwrap() },
        )
        .unwrap();
        assert_eq!(unit_healing(&w, &r, worker), 20);

        let archer = unit(&mut w, &r, 0, "Archer", 3, 4);
        w.claim_tile(city, w.units.get(archer).unwrap().position);
        w.civ_mut(CivId(0)).unwrap().techs.insert(r.technologies.id("Construction").unwrap());
        assert!(crate::world::combat::upgrade_unit(&mut w, &r, archer).is_err());
        assert_eq!(unit_healing(&w, &r, archer), 20, "an unaffordable upgrade spends nothing");
        w.civ_mut(CivId(0)).unwrap().gold = 1000;
        crate::world::combat::upgrade_unit(&mut w, &r, archer).unwrap();
        assert_eq!(unit_healing(&w, &r, archer), 0);
    }

    #[cfg(feature = "save")]
    #[test]
    fn save_mid_round_preserves_healing_eligibility() {
        let (r, mut w) = fixture();
        let id = unit(&mut w, &r, 0, "Warrior", 4, 5);
        let u = w.units.get_mut(id).unwrap();
        u.moved_this_turn = true;
        u.attacked_this_turn = true;
        let data = crate::save::encode(&w, &r).unwrap();
        let mut restored = crate::save::decode(&data, &r).unwrap();
        assert_eq!(restored.units.get(id), w.units.get(id));
        finish_round(&mut w, &r, CityFocus::None, TurnModifiers::default());
        finish_round(&mut restored, &r, CityFocus::None, TurnModifiers::default());
        assert_eq!(w.units.get(id).unwrap().health, 40);
        assert_eq!(crate::save::encode(&w, &r).unwrap(), crate::save::encode(&restored, &r).unwrap());
    }
}
