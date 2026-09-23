#![cfg(feature = "save")]
use civ_core::world::{
    commands::{execute, Command},
    entities::Civilization,
    grid::TileGrid,
};
use civ_core::{
    arena::Id,
    ruleset::Ruleset,
    stats::Fixed,
    world::{City, CivId, Unit, World},
};

fn fixture() -> (Ruleset, World, Id<City>) {
    let r = Ruleset::load_dir("rules/gods-and-kings").unwrap().unwrap();
    let grid = TileGrid::new(18, 8, r.terrain("Grassland").unwrap());
    let mut w = World::new(
        grid,
        vec![
            Civilization::new(CivId(0), r.nations.id("Rome").unwrap(), false),
            Civilization::new(CivId(1), r.nations.id("Greece").unwrap(), false),
        ],
    );
    let city = w.found_city_in(&r, City::found("Home", CivId(0), w.grid.id_at(3, 4).unwrap()));
    w.found_city_in(&r, City::found("Away", CivId(1), w.grid.id_at(14, 4).unwrap()));
    w.civ_mut(CivId(0)).unwrap().gold = 1000;
    for tile in w.grid.within(w.cities.get(city).unwrap().centre, 1) {
        w.claim_tile(city, tile);
    }
    civ_core::world::turn::update_visibility_in(&mut w, &r);
    (r, w, city)
}

#[test]
fn sales_are_owned_once_per_round_and_cannot_sell_free_buildings_or_wonders() {
    let (r, mut w, city) = fixture();
    let monument = r.buildings.id("Monument").unwrap();
    let granary = r.buildings.id("Granary").unwrap();
    w.cities.get_mut(city).unwrap().built.extend([monument, granary]);
    let before = w.clone();
    assert!(execute(&mut w, &r, CivId(1), Command::SellBuilding { city, building: monument }).is_err());
    assert_eq!(w, before);
    execute(&mut w, &r, CivId(0), Command::SellBuilding { city, building: monument }).unwrap();
    assert_eq!(w.civ(CivId(0)).unwrap().gold, 1000 + r.buildings[monument].cost.unwrap() / 10);
    assert!(!w.cities.get(city).unwrap().built.contains(&monument));
    assert!(execute(&mut w, &r, CivId(0), Command::SellBuilding { city, building: granary }).is_err());
    let save = civ_core::save::encode(&w, &r).unwrap();
    w = civ_core::save::decode(&save, &r).unwrap();
    assert!(execute(&mut w, &r, CivId(0), Command::SellBuilding { city, building: granary }).is_err());
    let c = w.cities.get_mut(city).unwrap();
    c.sold_building_this_turn = false;
    c.free_buildings.insert(granary);
    assert!(execute(&mut w, &r, CivId(0), Command::SellBuilding { city, building: granary }).is_err());
    let palace = r.buildings.id("Palace").unwrap();
    assert!(execute(&mut w, &r, CivId(0), Command::SellBuilding { city, building: palace }).is_err());
}

#[test]
fn buy_tile_uses_city_border_rules_and_disband_requires_movement() {
    let (r, mut w, city) = fixture();
    let centre = w.cities.get(city).unwrap().centre;
    let tile = w.grid.within(centre, 2).into_iter().find(|t| w.grid.distance(centre, *t) == 2).unwrap();
    w.visibility.observe(CivId(0), tile);
    execute(&mut w, &r, CivId(0), Command::BuyTile { city, tile }).unwrap();
    assert_eq!(w.grid.owner[tile.0 as usize], Some(CivId(0)));
    let gold = w.civ(CivId(0)).unwrap().gold;
    let unit = w.place_unit(Unit::new(r.units.id("Warrior").unwrap(), CivId(0), tile, 2));
    w.units.get_mut(unit).unwrap().movement_left = Fixed::ZERO;
    assert!(execute(&mut w, &r, CivId(0), Command::DisbandUnit { unit }).is_err());
    w.units.get_mut(unit).unwrap().movement_left = Fixed::ONE;
    execute(&mut w, &r, CivId(0), Command::DisbandUnit { unit }).unwrap();
    assert!(w.units.get(unit).is_none());
    assert!(w.civ(CivId(0)).unwrap().gold > gold);
}

#[test]
fn conquered_capital_relocates_and_liberation_revives_without_second_damage() {
    use civ_core::world::conquest;
    let (r, mut w, home) = fixture();
    let away = w.civ(CivId(1)).unwrap().cities[0];
    let second = w.found_city_in(&r, City::found("Second", CivId(1), w.grid.id_at(14, 1).unwrap()));
    let palace = r.buildings.id("Palace").unwrap();
    conquest::conquer(&mut w, &r, away, CivId(0)).unwrap();
    assert!(w.cities.get(home).unwrap().built.contains(&palace));
    assert!(!w.cities.get(away).unwrap().built.contains(&palace));
    assert!(w.cities.get(second).unwrap().built.contains(&palace));
    assert!(!conquest::can_liberate(&w, away), "cannot liberate a city captured directly from its founder");

    // A third party liberates the occupied original city after the founder
    // has lost its last city. No second plunder, burn, or population loss.
    w.civs.push(Civilization::new(CivId(2), r.nations.id("Egypt").unwrap(), false));
    w.visibility = civ_core::world::Visibility::new(w.grid.len(), w.civs.len());
    w.found_city_in(&r, City::found("Third", CivId(2), w.grid.id_at(6, 1).unwrap()));
    conquest::conquer(&mut w, &r, second, CivId(0)).unwrap();
    w.resolve_defeats();
    assert!(w.civ(CivId(1)).unwrap().defeated);
    conquest::conquer(&mut w, &r, away, CivId(2)).unwrap();
    let population = w.cities.get(away).unwrap().population;
    let gold = w.civ(CivId(2)).unwrap().gold;
    assert!(conquest::can_liberate(&w, away));
    execute(&mut w, &r, CivId(2), Command::LiberateCity { city: away }).unwrap();
    let city = w.cities.get(away).unwrap();
    assert_eq!(city.owner, CivId(1));
    assert_eq!(city.population, population);
    assert!(!city.puppet);
    assert_eq!(city.resistance, 0);
    assert!(city.built.contains(&palace));
    assert!(!w.civ(CivId(1)).unwrap().defeated);
    assert!(w.civ(CivId(1)).unwrap().is_alive());
    assert_eq!(w.civ(CivId(2)).unwrap().gold, gold);
    assert!(execute(&mut w, &r, CivId(2), Command::LiberateCity { city: away }).is_err());
    civ_core::save::validate(&w, &r).unwrap();
}

#[test]
fn unit_gifts_and_protection_obey_city_state_restrictions() {
    use civ_core::world::diplomacy::Flag;
    let (r, mut w, _) = fixture();
    let cs = CivId(2);
    let nation = r.nations.iter().find(|(_, n)| n.city_state_type.is_some()).unwrap().0;
    let mut minor = Civilization::new(cs, nation, true);
    minor.city_state_type = r.city_state_types.id(r.nations[nation].city_state_type.as_deref().unwrap());
    w.civs.push(minor);
    w.visibility = civ_core::world::Visibility::new(w.grid.len(), w.civs.len());
    let city = w.found_city_in(&r, City::found("Minor", cs, w.grid.id_at(9, 4).unwrap()));
    w.diplomacy.meet(CivId(0), cs);
    execute(&mut w, &r, CivId(0), Command::PledgeProtection { target: cs }).unwrap();
    assert!(w.diplomacy.is_protector(CivId(0), cs));
    assert!(execute(&mut w, &r, CivId(0), Command::WithdrawProtection { target: cs }).is_err());
    w.diplomacy.set_flag(cs, CivId(0), Flag::RecentlyPledgedProtection, 0);
    execute(&mut w, &r, CivId(0), Command::WithdrawProtection { target: cs }).unwrap();
    assert!(!w.diplomacy.is_protector(CivId(0), cs));
    assert_eq!(w.diplomacy.influence(cs, CivId(0)), Fixed::whole(-20));
    assert!(execute(&mut w, &r, CivId(0), Command::PledgeProtection { target: cs }).is_err());
    let tile = w.cities.get(city).unwrap().centre;
    let worker = w.place_unit(Unit::new(r.units.id("Worker").unwrap(), CivId(0), tile, 2));
    assert!(execute(&mut w, &r, CivId(0), Command::GiftUnit { unit: worker }).is_err());
    w.remove_unit(worker);
    let warrior = w.place_unit(Unit::new(r.units.id("Warrior").unwrap(), CivId(0), tile, 2));
    execute(&mut w, &r, CivId(0), Command::GiftUnit { unit: warrior }).unwrap();
    assert_eq!(w.units.get(warrior).unwrap().owner, cs);
    assert_eq!(w.diplomacy.influence(cs, CivId(0)), Fixed::whole(-15));
    civ_core::save::validate(&w, &r).unwrap();
}

#[test]
fn peace_treaty_survives_reload_and_blocks_war_for_ten_rounds() {
    let (r, mut w, _) = fixture();
    let a = CivId(0);
    let b = CivId(1);
    w.diplomacy.meet(a, b);
    assert!(w.diplomacy.can_declare_war(a, b));
    execute(&mut w, &r, a, Command::DeclareWar { target: b }).unwrap();
    w.diplomacy.make_peace(a, b);
    let data = civ_core::save::encode(&w, &r).unwrap();
    w = civ_core::save::decode(&data, &r).unwrap();
    for _ in 0..10 {
        assert!(execute(&mut w, &r, a, Command::DeclareWar { target: b }).is_err());
        w.diplomacy.tick();
    }
    execute(&mut w, &r, a, Command::DeclareWar { target: b }).unwrap();
    assert!(w.diplomacy.at_war(a, b));
}

#[test]
fn autocracy_finisher_strength_expires_at_the_saved_turn() {
    use civ_core::{world::policies, UniqueType};
    let (r, mut w, _) = fixture();
    let me = CivId(0);
    let last = r.policies.id("Total War").unwrap();
    let branch = r.policies[last].branch;
    let civ = w.civ_mut(me).unwrap();
    civ.techs.insert(r.technologies.id("Steam Power").unwrap());
    civ.pending_rewards.free_policies = 1;
    civ.adopted_policies.extend(
        r.policies
            .iter()
            .filter(|(id, p)| p.branch == branch && *id != last && !policies::is_finisher(&r, *id))
            .map(|(id, _)| id),
    );
    policies::apply_culture(&mut w, &r, me, 0, Some(last), Default::default()).unwrap();
    let finisher = r.policies.id("Autocracy Complete").unwrap();
    let expiry = w.civ(me).unwrap().timed_policy_expirations[&finisher];
    assert_eq!(expiry, 50);
    let has_strength = |w: &World| {
        w.civ(me).unwrap().uniques.matching(UniqueType::Strength).any(|u| {
            u.params.first().is_some_and(|p| p == "+25")
                && u.conditionals.iter().any(|c| c.unique_type() == Some(UniqueType::ConditionalAttacking))
        })
    };
    assert!(has_strength(&w));
    w = civ_core::save::decode(&civ_core::save::encode(&w, &r).unwrap(), &r).unwrap();
    w.turn = 49;
    policies::rebuild_uniques(&mut w, &r, me);
    assert!(has_strength(&w));
    w.turn = 50;
    policies::rebuild_uniques(&mut w, &r, me);
    assert!(!has_strength(&w));
}

#[test]
fn city_state_allies_join_war_and_peace_without_losing_influence() {
    use civ_core::world::{city_states, diplomacy_actions};
    let (r, mut w, _) = fixture();
    let cs = CivId(2);
    let nation = r.nations.iter().find(|(_, n)| n.city_state_type.is_some()).unwrap().0;
    let mut state = Civilization::new(cs, nation, true);
    state.city_state_type = r.city_state_types.ids().next();
    w.civs.push(state);
    w.visibility = civ_core::world::Visibility::new(w.grid.len(), w.civs.len());
    w.found_city_in(&r, City::found("Ally", cs, w.grid.id_at(8, 1).unwrap()));
    w.diplomacy.meet(CivId(0), CivId(1));
    w.diplomacy.meet(CivId(0), cs);
    w.diplomacy.set_influence(cs, CivId(0), Fixed::whole(100));
    assert_eq!(city_states::ally_of(&w, cs), Some(CivId(0)));
    let events = diplomacy_actions::declare_war(&mut w, &r, CivId(0), CivId(1));
    assert_eq!(events.len(), 2);
    assert!(w.diplomacy.at_war(cs, CivId(1)));
    assert_eq!(w.diplomacy.influence(cs, CivId(0)), Fixed::whole(100));
    let events = diplomacy_actions::make_peace(&mut w, &r, CivId(0), CivId(1));
    assert_eq!(events.len(), 2);
    assert!(!w.diplomacy.at_war(cs, CivId(1)));
    assert_eq!(w.diplomacy.pact(cs, CivId(1)).unwrap().peace_treaty_turns, 10);
    diplomacy_actions::declare_war(&mut w, &r, CivId(0), cs);
    assert_eq!(w.diplomacy.influence(cs, CivId(0)), Fixed::whole(-60));
    assert_eq!(city_states::ally_of(&w, cs), None);
}

#[test]
fn ground_unit_can_enter_own_city_with_stationed_aircraft() {
    let (r, mut w, home) = fixture();
    let tile = w.cities.get(home).unwrap().centre;
    let from = w.grid.neighbours(tile).next().unwrap();
    w.place_unit(Unit::new(r.units.id("Fighter").unwrap(), CivId(0), tile, 2));
    let warrior = w.place_unit(Unit::new(r.units.id("Warrior").unwrap(), CivId(0), from, 2));
    execute(&mut w, &r, CivId(0), Command::MoveUnit { unit: warrior, to: tile }).unwrap();
    assert_eq!(w.units.get(warrior).unwrap().position, tile);
}

#[test]
fn commanded_withdrawal_resolves_the_defenders_new_tile() {
    use civ_core::world::{commands::AttackTarget, turn};
    let (r, mut original, _) = fixture();
    original.diplomacy.declare_war(CivId(0), CivId(1));
    let a =
        original.place_unit(Unit::new(r.units.id("Warrior").unwrap(), CivId(0), original.grid.id_at(6, 4).unwrap(), 2));
    let from = original.grid.id_at(7, 4).unwrap();
    let d = original.place_unit(Unit::new(r.units.id("Slinger").unwrap(), CivId(1), from, 2));
    let camp = civ_core::world::barbarians::camp_improvement(&r).unwrap();
    for tile in original.grid.neighbours(from).collect::<Vec<_>>() {
        original.grid.set_improvement(tile, Some(camp));
    }
    turn::update_visibility_in(&mut original, &r);
    let mut withdrew = false;
    for round in 0..32 {
        let mut w = original.clone();
        w.turn = round;
        let outcome =
            execute(&mut w, &r, CivId(0), Command::Attack { unit: a, target: AttackTarget::Unit(d) }).unwrap();
        if let Some(unit) = w.units.get(d).filter(|u| u.position != from) {
            assert_ne!(w.grid.improvement[unit.position.0 as usize], Some(camp));
            assert!(outcome.events.iter().any(|e| matches!(e,turn::TurnEvent::UnitMoved(m) if m.unit==d)));
            assert!(civ_core::save::decode(&civ_core::save::encode(&w, &r).unwrap(), &r).is_ok());
            withdrew = true;
            break;
        }
    }
    assert!(withdrew, "a seeded withdrawal must exercise entry settlement");
}

#[test]
fn founding_inside_own_borders_transfers_the_city_claim_and_survives_save() {
    let (r, mut w, old_city) = fixture();
    let tile = w.grid.id_at(7, 4).unwrap();
    w.claim_tile(old_city, tile);
    w.cities.get_mut(old_city).unwrap().worked.push(tile);
    let settler = w.place_unit(Unit::new(r.units.id("Settler").unwrap(), CivId(0), tile, 2));
    let city = civ_core::world::settling::found_city_with(&mut w, &r, settler, "New city").unwrap();
    assert!(w.cities.get(city).unwrap().tiles.contains(&tile));
    assert!(!w.cities.get(old_city).unwrap().tiles.contains(&tile));
    assert!(!w.cities.get(old_city).unwrap().worked.contains(&tile));
    assert_eq!(w.grid.owner[tile.0 as usize], Some(CivId(0)));
    assert_eq!(civ_core::save::decode(&civ_core::save::encode(&w, &r).unwrap(), &r).unwrap(), w);
}
