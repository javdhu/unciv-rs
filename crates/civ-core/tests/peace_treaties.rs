use civ_core::ruleset::Ruleset;
use civ_core::stats::Fixed;
use civ_core::world::agreements::{accept, validate_proposal, Terms};
use civ_core::world::commands::{execute, Command};
use civ_core::world::construction::Construction;
use civ_core::world::trade::{self, Item};
use civ_core::world::{City, CivId, Civilization, TileGrid, TileId, TurnEvent, TurnModifiers, Unit, World};
const A: CivId = CivId(0);
const B: CivId = CivId(1);
fn setup() -> (Ruleset, World) {
    let r = Ruleset::load_dir("rules/gods-and-kings").unwrap().unwrap();
    let mut w = World::new(
        TileGrid::new(24, 12, r.terrain("Grassland").unwrap()),
        (0..2).map(|i| Civilization::new(CivId(i), r.nations.id("Rome").unwrap(), false)).collect(),
    );
    for (owner, tile) in [(A, TileId(25)), (B, TileId(260))] {
        w.found_city_in(&r, City::found("Capital", owner, tile));
        w.civ_mut(owner).unwrap().gold = 1000;
    }
    w.diplomacy.meet(A, B);
    (r, w)
}
fn war(w: &mut World, r: &Ruleset) {
    civ_core::world::diplomacy_actions::declare_war(w, r, A, B);
}

#[test]
fn compensated_peace_requires_consent_and_pays_only_once() {
    let (r, mut w) = setup();
    war(&mut w, &r);
    let terms = Terms::PeaceTreaty { give: vec![Item::Gold(250)], get: vec![] };
    let before = w.clone();
    assert!(execute(&mut w, &r, A, Command::OfferPeaceTreaty { target: B, give: vec![Item::Gold(250)], get: vec![] })
        .is_err());
    assert_eq!(w, before);
    validate_proposal(&w, &r, A, B, &terms, TurnModifiers::default()).unwrap();
    assert_eq!(w, before, "proposals do not end wars or transfer funds");
    let result = accept(&mut w, &r, A, B, &terms, TurnModifiers::default()).unwrap();
    assert!(!w.diplomacy.at_war(A, B));
    assert!(result.events.contains(&TurnEvent::PeaceMade { a: A, b: B }));
    assert_eq!((w.civ(A).unwrap().gold, w.civ(B).unwrap().gold), (750, 1250));
    assert_eq!(w.diplomacy.pact(A, B).unwrap().peace_treaty_turns, 10);
    let after = w.clone();
    assert!(accept(&mut w, &r, A, B, &terms, TurnModifiers::default()).is_err());
    assert_eq!(w, after);
}

#[test]
fn an_unaffordable_recipient_payment_does_not_end_war_or_move_troops() {
    let (r, mut w) = setup();
    war(&mut w, &r);
    let terms = Terms::PeaceTreaty { give: vec![Item::Gold(50)], get: vec![Item::Gold(800)] };
    let mut poor = w.clone();
    poor.civ_mut(B).unwrap().gold = 0;
    assert_eq!(
        validate_proposal(&w, &r, A, B, &terms, TurnModifiers::default()),
        validate_proposal(&poor, &r, A, B, &terms, TurnModifiers::default())
    );
    let centre = poor.cities.get(poor.civ(B).unwrap().cities[0]).unwrap().centre;
    poor.place_unit(Unit::new(r.unit("Warrior").unwrap(), A, centre, 2));
    let before = poor.clone();
    assert!(accept(&mut poor, &r, A, B, &terms, TurnModifiers::default()).is_err());
    assert_eq!(poor, before, "failed compensation rolls back peace callbacks and relocation");
}

#[test]
fn city_compensation_preserves_people_damage_work_and_owned_troops() {
    let (r, mut w) = setup();
    let city = w.found_city_in(&r, City::found("Border", B, TileId(150)));
    let work = Construction::Building(r.buildings.id("Granary").unwrap());
    let market = r.buildings.id("Market").unwrap();
    {
        let c = w.cities.get_mut(city).unwrap();
        c.population = 4;
        c.health = 87;
        c.food_stored = 13;
        c.queue.push(work);
        c.production_progress.insert(work, 17);
        c.built.insert(market);
    }
    civ_core::world::city::rebuild_city_uniques(&r, w.cities.get_mut(city).unwrap());
    civ_core::world::fog::reveal_city_site(&mut w, &r, A, city);
    let unit = w.place_unit(Unit::new(r.unit("Warrior").unwrap(), B, TileId(150), 2));
    w.units.get_mut(unit).unwrap().health = 61;
    w.units.get_mut(unit).unwrap().movement_left = Fixed::ONE;
    war(&mut w, &r);
    let terms = Terms::PeaceTreaty { give: vec![], get: vec![Item::City(city)] };
    accept(&mut w, &r, A, B, &terms, TurnModifiers::default()).unwrap();
    let c = w.cities.get(city).unwrap();
    assert_eq!((c.owner, c.founder, c.population, c.health, c.food_stored), (A, B, 4, 87, 13));
    assert_eq!(c.production_for(work), 17);
    assert!(c.built.contains(&market));
    assert!(!c.puppet && c.resistance == 0);
    let u = w.units.get(unit).unwrap();
    assert_eq!((u.owner, u.health, u.movement_left), (B, 61, Fixed::ONE));
    assert_ne!(u.position, TileId(150));
    assert_eq!((w.civ(A).unwrap().gold, w.civ(B).unwrap().gold), (1000, 1000));
    assert!(w.diplomacy.deals.is_empty(), "a city is not a recurring item");
    #[cfg(feature = "save")]
    assert_eq!(w, civ_core::save::decode(&civ_core::save::encode(&w, &r).unwrap(), &r).unwrap());
}

#[test]
fn requested_cities_use_remembered_ownership_and_recheck_live_state_at_acceptance() {
    let (r, mut w) = setup();
    let city = w.found_city_in(&r, City::found("Border", B, TileId(150)));
    let terms = Terms::Trade { give: vec![], get: vec![Item::City(city)] };
    assert!(validate_proposal(&w, &r, A, B, &terms, TurnModifiers::default()).is_err());
    civ_core::world::fog::reveal_city_site(&mut w, &r, A, city);
    validate_proposal(&w, &r, A, B, &terms, TurnModifiers::default()).unwrap();
    let mut changed = w.clone();
    civ_core::world::conquest::transfer_city(&mut changed, &r, city, A).unwrap();
    assert_eq!(
        validate_proposal(&w, &r, A, B, &terms, TurnModifiers::default()),
        validate_proposal(&changed, &r, A, B, &terms, TurnModifiers::default())
    );
    let before = changed.clone();
    assert!(accept(&mut changed, &r, A, B, &terms, TurnModifiers::default()).is_err());
    assert_eq!(changed, before);
}

#[test]
fn city_terms_reject_capitals_resistance_duplicates_and_last_city() {
    let (r, mut w) = setup();
    let capital = w.civ(B).unwrap().cities[0];
    let city = w.found_city_in(&r, City::found("Border", B, TileId(150)));
    let attempt =
        |w: &mut World, items: &[Item]| trade::accept_with(w, &r, B, A, (items, &[]), TurnModifiers::default());
    let before = w.clone();
    assert!(attempt(&mut w, &[Item::City(capital)]).is_err());
    assert!(attempt(&mut w, &[Item::City(city), Item::City(city)]).is_err());
    assert_eq!(w, before);
    w.cities.get_mut(city).unwrap().resistance = 2;
    assert!(attempt(&mut w, &[Item::City(city)]).is_err());
    w.cities.get_mut(city).unwrap().resistance = 0;
    civ_core::world::conquest::transfer_city(&mut w, &r, capital, A).unwrap();
    assert!(attempt(&mut w, &[Item::City(city)]).is_err(), "losing the old capital cannot make the last city tradable");
}

#[test]
fn voluntary_city_trade_moves_a_garrison_even_when_borders_remain_open() {
    let (r, mut w) = setup();
    let city = w.found_city_in(&r, City::found("Border", B, TileId(150)));
    w.diplomacy.set_open_borders(A, B, true);
    let unit = w.place_unit(Unit::new(r.unit("Warrior").unwrap(), B, TileId(150), 2));
    trade::accept_with(&mut w, &r, B, A, (&[Item::City(city)], &[]), TurnModifiers::default()).unwrap();
    assert_ne!(w.units.get(unit).unwrap().position, TileId(150));
    assert_eq!(w.units.get(unit).unwrap().owner, B);
}

#[test]
fn ai_peace_compensation_preserves_its_war_and_trade_decisions() {
    let (r, mut w) = setup();
    w.civ_mut(B).unwrap().is_ai = true;
    war(&mut w, &r);
    let offer = Command::OfferPeaceTreaty { target: B, give: vec![Item::Gold(250)], get: vec![] };
    let before = w.clone();
    assert!(execute(&mut w, &r, A, offer.clone()).is_err(), "fresh wars are still refused");
    assert_eq!(w, before);
    // Past even a conqueror's patience for a stalemate (ai::wants_peace).
    for _ in 0..91 {
        w.diplomacy.tick();
    }
    let before = w.clone();
    assert!(
        execute(&mut w, &r, A, Command::OfferPeaceTreaty { target: B, give: vec![], get: vec![Item::Gold(250)] })
            .is_err(),
        "wanting peace does not accept negative-value compensation"
    );
    assert_eq!(w, before);
    execute(&mut w, &r, A, offer).unwrap();
    assert!(!w.diplomacy.at_war(A, B));
    assert_eq!((w.civ(A).unwrap().gold, w.civ(B).unwrap().gold), (750, 1250));
}
