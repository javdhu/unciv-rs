use civ_core::arena::Id;
use civ_core::ruleset::Ruleset;
use civ_core::stats::{Fixed, Stat};
use civ_core::world::commands::{execute, execute_with, AttackTarget, Command, CommandRejection};
use civ_core::world::construction::{production_cost_in, Construction, CostModifiers};
use civ_core::world::entities::Unit;
use civ_core::world::turn::{finish_round, plan_economy, update_visibility, TurnEvent, TurnModifiers};
use civ_core::world::{City, CityFocus, CivId, Civilization, TileGrid, World};

fn fixture() -> (Ruleset, World) {
    let r = Ruleset::load_dir("../android/assets/jsons/Civ V - Gods & Kings").unwrap().unwrap();
    let nation = r.nations.id("Rome").unwrap();
    let civs = vec![Civilization::new(CivId(0), nation, false), Civilization::new(CivId(1), nation, false)];
    let mut w = World::new(TileGrid::new(24, 18, r.terrain("Grassland").unwrap()), civs);
    for c in &mut w.civs {
        c.techs.insert(r.technologies.id("Agriculture").unwrap());
    }
    let a = w.grid.id_at(4, 8).unwrap();
    let b = w.grid.id_at(18, 8).unwrap();
    w.found_city_in(&r, City::found("Rome", CivId(0), a));
    w.found_city_in(&r, City::found("Antium", CivId(1), b));
    for city in w.cities.ids() {
        let centre = w.cities.get(city).unwrap().centre;
        for tile in w.grid.within(centre, 1) {
            w.claim_tile(city, tile);
        }
    }
    update_visibility(&mut w);
    civ_core::world::fog::capture_observations(&mut w, &r);
    (r, w)
}
fn unit(w: &mut World, r: &Ruleset, owner: u8, name: &str, x: u16, y: u16) -> Id<Unit> {
    let base = r.units.id(name).unwrap();
    w.place_unit(Unit::new(base, CivId(owner), w.grid.id_at(x, y).unwrap(), r.units.get(base).unwrap().movement))
}
fn observe(w: &mut World, r: &Ruleset) {
    update_visibility(w);
    civ_core::world::fog::capture_observations(w, r);
}

#[test]
fn purchases_reprice_after_expansion_and_cannot_use_a_supplied_city_count() {
    for (currency, old_price, current_price) in [(Stat::Gold, 490, 570), (Stat::Faith, 260, 320)] {
        let (mut r, mut w) = fixture();
        let city = w.civ(CivId(0)).unwrap().cities[0];
        let monument = r.building("Monument").unwrap();
        let building = r.buildings.get_mut(monument).unwrap();
        building.cost = Some(100);
        building.hurry_cost_modifier = Some(0);
        building.uniques.push(civ_core::Unique::parse("Cost increases by [30] per owned city"));
        let nation = w.civ(CivId(0)).unwrap().nation;
        r.nations.get_mut(nation).unwrap().uniques.push(civ_core::Unique::parse(
            "May buy [Monument] buildings with [Faith] for [2] times their normal Production cost",
        ));
        civ_core::world::policies::rebuild_uniques(&mut w, &r, CivId(0));
        let what = Construction::Building(monument);
        let mods = TurnModifiers { costs: CostModifiers { cities: 0, ..Default::default() }, ..Default::default() };
        assert_eq!(civ_core::world::purchase::cost(&w, &r, city, what, currency, mods.costs), Some(old_price));

        // An old quote and the host's default/cached count must not survive expansion.
        let centre = w.grid.id_at(9, 3).unwrap();
        w.found_city_in(&r, City::found("Expansion", CivId(0), centre));
        assert_eq!(production_cost_in(&w, &r, city, what, mods.costs), Some(160));
        assert_eq!(civ_core::world::purchase::cost(&w, &r, city, what, currency, mods.costs), Some(current_price));
        let excessive = CostModifiers { cities: 999, ..mods.costs };
        assert_eq!(civ_core::world::purchase::cost(&w, &r, city, what, currency, excessive), Some(current_price));

        let civ = w.civ_mut(CivId(0)).unwrap();
        civ.gold = old_price as i32;
        civ.faith.stored = old_price as i32;
        let command = Command::Purchase { city, what, currency };
        let before = w.clone();
        assert_eq!(execute_with(&mut w, &r, CivId(0), command.clone(), mods), Err(CommandRejection::Unavailable));
        assert_eq!(w, before, "the former price cannot buy the item or spend either balance");

        let civ = w.civ_mut(CivId(0)).unwrap();
        civ.gold = 1_000;
        civ.faith.stored = 1_000;
        let outcome = execute_with(&mut w, &r, CivId(0), command, mods).unwrap();
        let civ = w.civ(CivId(0)).unwrap();
        assert_eq!(civ.gold, if currency == Stat::Gold { 1_000 - current_price as i32 } else { 1_000 });
        assert_eq!(civ.faith.stored, if currency == Stat::Faith { 1_000 - current_price as i32 } else { 1_000 });
        assert!(outcome.events.iter().any(|event| match event {
            TurnEvent::Purchased { gold, .. } if currency == Stat::Gold => *gold == current_price,
            TurnEvent::FaithPurchased { faith, .. } if currency == Stat::Faith => *faith == current_price,
            _ => false,
        }));
    }
}

#[test]
fn production_and_purchase_discounts_recheck_growth_and_annexation() {
    let (mut r, mut w) = fixture();
    let city = w.civ(CivId(1)).unwrap().cities[0];
    let monument = r.building("Monument").unwrap();
    let building = r.buildings.get_mut(monument).unwrap();
    building.cost = Some(200);
    building.hurry_cost_modifier = Some(0);
    building.uniques.extend([
        civ_core::Unique::parse("[-50]% production cost <in cities with at least [2] [Population]>"),
        civ_core::Unique::parse("[-50]% production cost <in [Annexed] cities>"),
    ]);
    let what = Construction::Building(monument);
    let m = CostModifiers::default();
    assert_eq!(production_cost_in(&w, &r, city, what, m), Some(200));
    civ_core::world::city::apply_food(w.cities.get_mut(city).unwrap(), 15, Default::default());
    assert_eq!(w.cities.get(city).unwrap().population, 2);
    assert_eq!(production_cost_in(&w, &r, city, what, m), Some(100));

    // Conquest changes the owner; its current policy discounts apply after annexing.
    civ_core::world::conquest::conquer(&mut w, &r, city, CivId(0)).unwrap();
    let captured = w.cities.get_mut(city).unwrap();
    captured.population = 2;
    captured.resistance = 0;
    assert!(captured.puppet);
    assert_eq!(production_cost_in(&w, &r, city, what, m), Some(100));
    w.civ_mut(CivId(0))
        .unwrap()
        .uniques
        .insert(civ_core::Unique::parse("[Gold] cost of purchasing items in cities [-50]% <in [Annexed] cities>"));
    w.civ_mut(CivId(0)).unwrap().gold = 1_000;
    execute(&mut w, &r, CivId(0), Command::Annex { city }).unwrap();
    assert_eq!(production_cost_in(&w, &r, city, what, m), Some(50));
    assert_eq!(civ_core::world::purchase::can_purchase(&w, &r, city, what, Stat::Gold, m), Ok(120));
    let outcome = execute(&mut w, &r, CivId(0), Command::Purchase { city, what, currency: Stat::Gold }).unwrap();
    assert_eq!(w.civ(CivId(0)).unwrap().gold, 880);
    assert!(outcome.events.iter().any(|e| matches!(e, TurnEvent::Purchased { gold: 120, .. })));
}

#[test]
fn production_completion_uses_current_empire_size_and_conditional_cost() {
    use civ_core::world::construction::{apply_production_in, ConstructionEvent};
    let (mut r, mut w) = fixture();
    let city = w.civ(CivId(0)).unwrap().cities[0];
    let college = r.building("National College").unwrap();
    w.civ_mut(CivId(0)).unwrap().techs.extend(r.technologies.ids());
    let library = r.building("Library").unwrap();
    w.cities.get_mut(city).unwrap().built.insert(library);
    let what = Construction::Building(college);
    let m = CostModifiers { cities: 0, ..Default::default() };
    let original_price = production_cost_in(&w, &r, city, what, m).unwrap();
    w.cities.get_mut(city).unwrap().queue.push(what);
    w.cities.get_mut(city).unwrap().production_progress.insert(what, original_price - 1);
    let centre = w.grid.id_at(9, 3).unwrap();
    let second = w.found_city_in(&r, City::found("Expansion", CivId(0), centre));
    w.cities.get_mut(second).unwrap().built.insert(library);
    assert_eq!(production_cost_in(&w, &r, city, what, m), Some(original_price + 30));
    assert_eq!(
        apply_production_in(&mut w, &r, city, 1, m),
        Some(vec![ConstructionEvent::Progressed { stored: original_price }])
    );
    assert!(!w.cities.get(city).unwrap().built.contains(&college));

    r.buildings
        .get_mut(college)
        .unwrap()
        .uniques
        .push(civ_core::Unique::parse("[-50]% production cost <in cities with at least [2] [Population]>"));
    civ_core::world::city::apply_food(w.cities.get_mut(city).unwrap(), 15, Default::default());
    let discounted = (original_price + 30) / 2;
    assert_eq!(production_cost_in(&w, &r, city, what, m), Some(discounted));
    assert_eq!(
        apply_production_in(&mut w, &r, city, 0, m),
        Some(vec![ConstructionEvent::Completed { what, carried_over: original_price - discounted }])
    );
}

#[test]
fn ownership_and_invalid_actions_never_mutate_state() {
    let (r, mut w) = fixture();
    let foreign = unit(&mut w, &r, 1, "Warrior", 6, 8);
    let city = w.civ(CivId(1)).unwrap().cities[0];
    let before = w.clone();
    assert_eq!(execute(&mut w, &r, CivId(0), Command::SkipUnit { unit: foreign }), Err(CommandRejection::NotOwner));
    assert_eq!(w, before);
    assert_eq!(
        execute(&mut w, &r, CivId(0), Command::SetProduction { city, queue: vec![] }),
        Err(CommandRejection::NotOwner)
    );
    assert_eq!(w, before);
    let own_city = w.civ(CivId(0)).unwrap().cities[0];
    assert!(execute(
        &mut w,
        &r,
        CivId(0),
        Command::Purchase {
            city: own_city,
            what: Construction::Unit(r.unit("Warrior").unwrap()),
            currency: Stat::Gold
        }
    )
    .is_err());
    assert_eq!(w, before);
}

#[test]
fn worker_progress_is_one_action_per_round() {
    let (r, mut w) = fixture();
    let worker = unit(&mut w, &r, 0, "Worker", 4, 9);
    let tile = w.units.get(worker).unwrap().position;
    w.grid.owner[tile.0 as usize] = Some(CivId(0));
    w.civ_mut(CivId(0)).unwrap().techs.insert(r.technologies.id("Agriculture").unwrap());
    let command = Command::Improve { unit: worker, improvement: r.improvements.id("Farm").unwrap() };
    execute(&mut w, &r, CivId(0), command.clone()).unwrap();
    let after = w.clone();
    assert_eq!(w.units.get(worker).unwrap().movement_left, Fixed::ZERO);
    assert_eq!(execute(&mut w, &r, CivId(0), command), Err(CommandRejection::NoMovement));
    assert_eq!(w, after);
}

#[test]
fn ranged_attack_dispatches_at_distance_and_requires_visible_enemy() {
    let (r, mut w) = fixture();
    let attacker = unit(&mut w, &r, 0, "Archer", 8, 8);
    let defender = unit(&mut w, &r, 1, "Warrior", 10, 8);
    w.diplomacy.declare_war(CivId(0), CivId(1));
    observe(&mut w, &r);
    let from = w.units.get(attacker).unwrap().position;
    execute(&mut w, &r, CivId(0), Command::Attack { unit: attacker, target: AttackTarget::Unit(defender) }).unwrap();
    assert!(w.units.get(defender).is_none_or(|u| u.health < 100));
    assert_eq!(w.units.get(attacker).unwrap().position, from);
    assert_eq!(w.units.get(attacker).unwrap().health, 100);
    let before = w.clone();
    assert!(execute(&mut w, &r, CivId(0), Command::Attack { unit: attacker, target: AttackTarget::Unit(defender) })
        .is_err());
    assert_eq!(w, before);
}

#[test]
fn city_bombard_is_available_once_per_round() {
    let (r, mut w) = fixture();
    let city = w.civ(CivId(0)).unwrap().cities[0];
    let target = unit(&mut w, &r, 1, "Infantry", 6, 8);
    w.diplomacy.declare_war(CivId(0), CivId(1));
    observe(&mut w, &r);
    let command = Command::Bombard { city, target };
    execute(&mut w, &r, CivId(0), command.clone()).unwrap();
    assert_eq!(w.cities.get(city).unwrap().attacks_left, 0);
    let before = w.clone();
    assert!(execute(&mut w, &r, CivId(0), command).is_err());
    assert_eq!(w, before);
    finish_round(&mut w, &r, CityFocus::None, TurnModifiers::default());
    assert_eq!(w.cities.get(city).unwrap().attacks_left, 1);
}

#[test]
fn commands_do_not_bank_income_and_finish_round_banks_exactly_once() {
    let (r, mut w) = fixture();
    let city = w.civ(CivId(0)).unwrap().cities[0];
    let warrior = unit(&mut w, &r, 0, "Warrior", 5, 8);
    let agriculture = r.technologies.id("Pottery").unwrap();
    execute(&mut w, &r, CivId(0), Command::SetResearch { technology: agriculture }).unwrap();
    execute(
        &mut w,
        &r,
        CivId(0),
        Command::SetProduction { city, queue: vec![Construction::Unit(r.unit("Warrior").unwrap())] },
    )
    .unwrap();
    let (gold, science, production) = (
        w.civ(CivId(0)).unwrap().gold,
        w.civ(CivId(0)).unwrap().science_stored,
        w.cities.get(city).unwrap().current_production(),
    );
    execute(&mut w, &r, CivId(0), Command::Fortify { unit: warrior }).unwrap();
    assert_eq!(w.turn, 0);
    assert_eq!(w.civ(CivId(0)).unwrap().gold, gold);
    assert_eq!(w.civ(CivId(0)).unwrap().science_stored, science);
    assert_eq!(w.cities.get(city).unwrap().current_production(), production);
    let income = plan_economy(&w, &r, CivId(0), CityFocus::None, TurnModifiers::default());
    let events = finish_round(&mut w, &r, CityFocus::None, TurnModifiers::default());
    assert_eq!(w.turn, 1);
    assert_eq!(events.iter().filter(|e| matches!(e, TurnEvent::TurnEnded { .. })).count(), 1);
    assert_eq!(w.civ(CivId(0)).unwrap().gold, gold + income.gold);
    assert_eq!(
        w.civ(CivId(0)).unwrap().science_invested.get(&agriculture).copied().unwrap_or(0),
        science + income.science
    );
    assert_eq!(
        w.civ(CivId(0)).unwrap().science_stored,
        0,
        "uncompleted research is item work, not completion overflow"
    );
    assert_eq!(w.cities.get(city).unwrap().current_production(), production + income.production[0].1);
    assert!(w.units.get(warrior).unwrap().fortified);
}

#[test]
fn human_choices_stay_pending_while_ai_plays() {
    let (r, mut w) = fixture();
    w.civ_mut(CivId(1)).unwrap().is_ai = true;
    let human = w.civ(CivId(0)).unwrap().cities[0];
    let ai = w.civ(CivId(1)).unwrap().cities[0];
    let settler = unit(&mut w, &r, 0, "Settler", 10, 4);
    let position = w.units.get(settler).unwrap().position;
    w.civ_mut(CivId(0)).unwrap().faith.stored = 100;
    w.civ_mut(CivId(0)).unwrap().culture_stored = 100;
    let human_gold = w.civ(CivId(0)).unwrap().gold;
    observe(&mut w, &r);
    finish_round(&mut w, &r, CityFocus::None, TurnModifiers::default());
    assert_eq!(w.units.get(settler).unwrap().position, position);
    let c = w.civ(CivId(0)).unwrap();
    assert_eq!(c.cities.len(), 1);
    assert!(c.researching.is_none());
    assert!(c.adopted_policies.is_empty());
    assert!(c.faith.beliefs.is_empty());
    assert!(c.faith.stored >= 100);
    assert!(c.gold >= human_gold);
    assert!(w.cities.get(human).unwrap().queue.is_empty());
    // Like Unciv's governor, the round puts idle citizens to work; every other choice waits for the player.
    assert!(!w.cities.get(human).unwrap().worked.is_empty());
    assert!(w.civ(CivId(1)).unwrap().researching.is_some());
    assert!(!w.cities.get(ai).unwrap().queue.is_empty());
}

#[test]
fn visible_blocker_rejects_and_hidden_changed_terrain_stops_without_rollback() {
    let (r, mut w) = fixture();
    let scout = unit(&mut w, &r, 0, "Scout", 8, 8);
    let destination = w.grid.id_at(10, 8).unwrap();
    observe(&mut w, &r);
    // All terrain in the preview is ordinary grass. A new hidden mountain
    // on the last step is discovered only after the first actual step.
    w.visibility.clear_visible(CivId(0));
    w.grid.terrain[destination.0 as usize] = r.terrain("Mountain").unwrap();
    let result = execute(&mut w, &r, CivId(0), Command::MoveUnit { unit: scout, to: destination }).unwrap();
    assert!(result.events.iter().any(|e| matches!(e, TurnEvent::UnitMoved(_))));
    assert_ne!(w.units.get(scout).unwrap().position, destination);
    assert_ne!(w.units.get(scout).unwrap().position, w.grid.id_at(8, 8).unwrap());
    assert!(w.visibility.has_explored(CivId(0), destination));
    let before = w.clone();
    assert!(execute(&mut w, &r, CivId(0), Command::MoveUnit { unit: scout, to: destination }).is_err());
    assert_eq!(w, before);
}

#[test]
fn duplicate_trade_items_cannot_overdraw_an_ai() {
    let (r, mut w) = fixture();
    w.civ_mut(CivId(1)).unwrap().is_ai = true;
    w.civ_mut(CivId(0)).unwrap().gold = 100;
    w.diplomacy.meet(CivId(0), CivId(1));
    let before = w.clone();
    let result = execute(
        &mut w,
        &r,
        CivId(0),
        Command::OfferTrade {
            target: CivId(1),
            give: vec![civ_core::world::trade::Item::Gold(80), civ_core::world::trade::Item::Gold(70)],
            get: vec![],
        },
    );
    assert!(result.is_err());
    assert_eq!(w, before);
}

#[test]
fn newly_founded_city_enters_this_rounds_economy() {
    let (r, mut w) = fixture();
    let settler = unit(&mut w, &r, 0, "Settler", 10, 3);
    observe(&mut w, &r);
    execute(&mut w, &r, CivId(0), Command::FoundCity { unit: settler, name: "New City".into() }).unwrap();
    let city = *w.civ(CivId(0)).unwrap().cities.last().unwrap();
    execute(
        &mut w,
        &r,
        CivId(0),
        Command::SetProduction { city, queue: vec![Construction::Building(r.building("Monument").unwrap())] },
    )
    .unwrap();
    let income = plan_economy(&w, &r, CivId(0), CityFocus::None, TurnModifiers::default());
    let production = income.production.iter().find(|(id, _)| *id == city).unwrap().1;
    assert!(production > 0);
    let result = finish_round(&mut w, &r, CityFocus::None, TurnModifiers::default());
    assert!(result.iter().any(|e| matches!(e, TurnEvent::CityStored { city: id, .. } if *id == city)));
    assert_eq!(w.cities.get(city).unwrap().current_production(), production);
    assert!(w.units.get(settler).is_none());
}

#[test]
fn pantheon_waits_for_the_humans_explicit_choice() {
    let (r, mut w) = fixture();
    w.civ_mut(CivId(0)).unwrap().faith.stored = 100;
    let belief = civ_core::world::religion::available_beliefs(&w, &r, "Pantheon").into_iter().last().unwrap();
    finish_round(&mut w, &r, CityFocus::None, TurnModifiers::default());
    assert!(w.civ(CivId(0)).unwrap().faith.beliefs.is_empty());
    let cost = civ_core::world::religion::pantheon_cost(&w, Fixed::ONE);
    let before = w.civ(CivId(0)).unwrap().faith.stored;
    execute(&mut w, &r, CivId(0), Command::ChoosePantheon { belief }).unwrap();
    assert_eq!(w.civ(CivId(0)).unwrap().faith.stored, before - cost);
    assert_eq!(w.civ(CivId(0)).unwrap().faith.beliefs.iter().copied().collect::<Vec<_>>(), vec![belief]);
    let before = w.clone();
    assert!(execute(&mut w, &r, CivId(0), Command::ChoosePantheon { belief }).is_err());
    assert_eq!(w, before);
}

#[test]
fn ordinary_unit_cannot_be_used_as_a_nuclear_weapon() {
    let (r, mut w) = fixture();
    let warrior = unit(&mut w, &r, 0, "Warrior", 8, 8);
    observe(&mut w, &r);
    let target = w.grid.id_at(10, 8).unwrap();
    let before = w.clone();
    assert!(execute(&mut w, &r, CivId(0), Command::NuclearStrike { unit: warrior, target }).is_err());
    assert_eq!(w, before);
}

#[test]
fn competing_world_wonders_resolve_once_and_refund_losing_work() {
    let (r, mut w) = fixture();
    let wonder = r.buildings.id("Stonehenge").unwrap();
    let what = Construction::Building(wonder);
    let cost = civ_core::world::construction::production_cost(&r, what, Default::default()).unwrap();
    let a = w.civ(CivId(0)).unwrap().cities[0];
    let b = w.civ(CivId(1)).unwrap().cities[0];
    for civ in &mut w.civs {
        civ.techs.extend(r.technologies.ids());
    }
    execute(&mut w, &r, CivId(0), Command::SetProduction { city: a, queue: vec![what] }).unwrap();
    execute(&mut w, &r, CivId(1), Command::SetProduction { city: b, queue: vec![what] }).unwrap();
    w.cities.get_mut(a).unwrap().production_progress.insert(what, cost - 1);
    w.cities.get_mut(b).unwrap().production_progress.insert(what, cost - 1);
    let gold = w.civ(CivId(1)).unwrap().gold;
    let first = civ_core::world::construction::apply_production_in(&mut w, &r, a, 1, Default::default()).unwrap();
    let second = civ_core::world::construction::apply_production_in(&mut w, &r, b, 1, Default::default()).unwrap();
    assert!(first.iter().any(|e| matches!(e, civ_core::world::construction::ConstructionEvent::Completed { .. })));
    assert!(second.contains(&civ_core::world::construction::ConstructionEvent::Cancelled { what, gold: cost - 1 }));
    assert_eq!(w.civ(CivId(1)).unwrap().gold, gold + cost - 1);
    assert_eq!(w.cities.iter().filter(|(_, c)| c.built.contains(&wonder)).count(), 1);
    assert!(w.cities.get(b).unwrap().queue.is_empty());
    let before = w.clone();
    assert!(execute(&mut w, &r, CivId(1), Command::SetProduction { city: b, queue: vec![what] }).is_err());
    assert_eq!(w, before);
}

#[test]
fn national_wonder_is_limited_per_civilization_instead_of_globally() {
    let (r, mut w) = fixture();
    let wonder = r.buildings.id("National Epic").unwrap();
    let a = w.civ(CivId(0)).unwrap().cities[0];
    let b = w.civ(CivId(1)).unwrap().cities[0];
    let centre = w.grid.id_at(10, 3).unwrap();
    let another = w.found_city_in(&r, City::found("Another", CivId(0), centre));
    for civ in &mut w.civs {
        civ.techs.extend(r.technologies.ids());
    }
    for (_, c) in w.cities.iter_mut() {
        c.built.insert(r.buildings.id("Monument").unwrap());
    }
    w.cities.get_mut(a).unwrap().built.insert(wonder);
    let before = w.clone();
    assert!(execute(
        &mut w,
        &r,
        CivId(0),
        Command::SetProduction { city: another, queue: vec![Construction::Building(wonder)] }
    )
    .is_err());
    assert_eq!(w, before);
    execute(&mut w, &r, CivId(1), Command::SetProduction { city: b, queue: vec![Construction::Building(wonder)] })
        .unwrap();
}

#[test]
fn sequential_trades_cannot_sell_the_same_last_resource_twice() {
    let (r, mut w) = fixture();
    w.civ_mut(CivId(1)).unwrap().is_ai = true;
    w.diplomacy.meet(CivId(0), CivId(1));
    w.civ_mut(CivId(0)).unwrap().techs.extend(r.technologies.ids());
    let city = w.civ(CivId(0)).unwrap().cities[0];
    let tile = w.cities.get(city).unwrap().centre;
    let iron = r.resources.id("Iron").unwrap();
    w.grid.resource[tile.0 as usize] = Some(iron);
    w.grid.resource_amount[tile.0 as usize] = 1;
    // The cache deliberately disagrees with the map. Commit reads the map.
    w.civ_mut(CivId(0)).unwrap().resources.insert(iron, 50);
    let command = Command::OfferTrade {
        target: CivId(1),
        give: vec![civ_core::world::trade::Item::Strategic(iron, 1)],
        get: vec![],
    };
    execute(&mut w, &r, CivId(0), command.clone()).unwrap();
    assert_eq!(w.civ(CivId(0)).unwrap().resources.get(&iron), Some(&0));
    assert_eq!(w.civ(CivId(1)).unwrap().resources.get(&iron), Some(&1));
    let before = w.clone();
    assert!(execute(&mut w, &r, CivId(0), command).is_err());
    assert_eq!(w, before);
    assert_eq!(w.diplomacy.deals.len(), 1);
}

#[test]
fn shared_trade_commit_validates_aggregate_spending_before_any_transfer() {
    let (r, mut w) = fixture();
    w.diplomacy.meet(CivId(0), CivId(1));
    w.civ_mut(CivId(0)).unwrap().gold = 100;
    let before = w.clone();
    let result = civ_core::world::trade::accept_in(
        &mut w,
        &r,
        CivId(0),
        CivId(1),
        &[civ_core::world::trade::Item::Gold(70), civ_core::world::trade::Item::Gold(80)],
        &[],
        30,
    );
    assert!(result.is_err());
    assert_eq!(w, before);
}

#[test]
fn recurring_trade_uses_income_and_existing_obligations_not_treasury() {
    let (r, mut w) = fixture();
    w.civ_mut(CivId(1)).unwrap().is_ai = true;
    w.diplomacy.meet(CivId(0), CivId(1));
    let mods = TurnModifiers::default();
    let income = civ_core::world::trade::available_gold_per_turn(&w, &r, CivId(0), mods).unwrap();
    assert!(income > 0);
    assert_eq!(w.civ(CivId(0)).unwrap().gold, 0);
    let gift = Command::OfferTrade {
        target: CivId(1),
        give: vec![civ_core::world::trade::Item::GoldPerTurn(income as i32)],
        get: vec![],
    };
    execute(&mut w, &r, CivId(0), gift).unwrap();
    assert_eq!(civ_core::world::trade::available_gold_per_turn(&w, &r, CivId(0), mods), Ok(0));
    w.civ_mut(CivId(0)).unwrap().gold = 1_000_000;
    let before = w.clone();
    let extra =
        Command::OfferTrade { target: CivId(1), give: vec![civ_core::world::trade::Item::GoldPerTurn(1)], get: vec![] };
    assert!(execute(&mut w, &r, CivId(0), extra).is_err());
    assert_eq!(w, before);
    let absurd = Command::OfferTrade {
        target: CivId(1),
        give: vec![civ_core::world::trade::Item::GoldPerTurn(i32::MAX)],
        get: vec![],
    };
    assert!(execute(&mut w, &r, CivId(0), absurd).is_err());
    assert_eq!(w, before);
}

#[test]
fn two_human_seats_keep_their_choices_while_a_bot_plays_shared_rounds() {
    let (r, _) = fixture();
    let nation = r.nations.id("Rome").unwrap();
    let civs = vec![
        Civilization::new(CivId(0), nation, false),
        Civilization::new(CivId(1), nation, false),
        Civilization::new(CivId(2), nation, true),
    ];
    let mut w = World::new(TileGrid::new(24, 18, r.terrain("Grassland").unwrap()), civs);
    for c in &mut w.civs {
        c.techs.insert(r.technologies.id("Agriculture").unwrap());
    }
    let mut cities = Vec::new();
    for (owner, x, y) in [(0, 4, 8), (1, 18, 8), (2, 10, 2)] {
        let centre = w.grid.id_at(x, y).unwrap();
        let city = w.found_city_in(&r, City::found(format!("Seat {owner}"), CivId(owner), centre));
        for tile in w.grid.within(centre, 1) {
            w.claim_tile(city, tile);
        }
        cities.push(city);
    }
    let units = [unit(&mut w, &r, 0, "Warrior", 5, 8), unit(&mut w, &r, 1, "Warrior", 17, 8)];
    unit(&mut w, &r, 2, "Warrior", 10, 3);
    observe(&mut w, &r);
    let positions: Vec<_> = units.iter().map(|id| w.units.get(*id).unwrap().position).collect();
    let pottery = r.technologies.id("Pottery").unwrap();
    let build = Construction::Unit(r.unit("Worker").unwrap());
    for seat in 0..2 {
        let civ = CivId(seat as u8);
        execute(&mut w, &r, civ, Command::SetResearch { technology: pottery }).unwrap();
        execute(&mut w, &r, civ, Command::SetProduction { city: cities[seat], queue: vec![build] }).unwrap();
        execute(&mut w, &r, civ, Command::Fortify { unit: units[seat] }).unwrap();
    }
    for turn in 1..=3 {
        let events = finish_round(&mut w, &r, CityFocus::None, TurnModifiers::default());
        assert_eq!(w.turn, turn);
        assert_eq!(events.iter().filter(|e| matches!(e, TurnEvent::TurnEnded { .. })).count(), 1);
        for seat in 0..2 {
            let civ = CivId(seat as u8);
            assert_eq!(
                events.iter().filter(|e| matches!(e, TurnEvent::Happiness { civ: owner, .. } if *owner == civ)).count(),
                1
            );
            assert_eq!(w.units.get(units[seat]).unwrap().position, positions[seat]);
            assert!(w.units.get(units[seat]).unwrap().fortified);
            assert_eq!(w.civ(civ).unwrap().researching, Some(pottery));
            assert_eq!(w.cities.get(cities[seat]).unwrap().queue, vec![build]);
            assert!(!w.cities.get(cities[seat]).unwrap().worked.is_empty(), "idle citizens are placed");
        }
    }
    assert!(w.civ(CivId(2)).unwrap().researching.is_some());
    assert!(!w.cities.get(cities[2]).unwrap().queue.is_empty());
    assert!(!w.cities.get(cities[2]).unwrap().worked.is_empty());
}
