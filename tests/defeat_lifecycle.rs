use civ_core::ruleset::Ruleset;
use civ_core::world::city::City;
use civ_core::world::commands::{execute, AttackTarget, Command, CommandRejection};
use civ_core::world::entities::{Civilization, Unit};
use civ_core::world::grid::{CivId, TileGrid, TileId};
use civ_core::world::turn::{update_visibility, TurnEvent};
use civ_core::world::{conquest, World};

fn setup(count: usize) -> (Ruleset, World) {
    let r = Ruleset::load_dir("rules/gods-and-kings").unwrap().unwrap();
    let nation = r.nations.id("Rome").unwrap();
    let w = World::new(
        TileGrid::new(16, 16, r.terrain("Grassland").unwrap()),
        (0..count).map(|i| Civilization::new(CivId(i as u8), nation, false)).collect(),
    );
    (r, w)
}

#[test]
fn barbarian_raid_preserves_original_capital_and_consumes_raider() {
    for (treasury, remaining) in [(-20, 0), (0, 0), (75, 0), (500, 300)] {
        let (r, mut w) = setup(2);
        w.civ_mut(CivId(0)).unwrap().is_barbarian = true;
        w.diplomacy.mark_hostile(CivId(0));
        let centre = TileId(100);
        let city = w.found_city_in(&r, City::found("Capital", CivId(1), centre));
        w.cities.get_mut(city).unwrap().health = 1;
        w.civ_mut(CivId(1)).unwrap().gold = treasury;
        let outside = w.grid.neighbours(centre).next().unwrap();
        let attacker = w.place_unit(Unit::new(r.unit("Warrior").unwrap(), CivId(0), outside, 2));
        update_visibility(&mut w);
        let outcome =
            execute(&mut w, &r, CivId(0), Command::Attack { unit: attacker, target: AttackTarget::City(city) })
                .unwrap();
        assert!(w.units.get(attacker).is_none());
        let capital = w.cities.get(city).unwrap();
        assert_eq!(capital.owner, CivId(1));
        assert!(capital.original_capital);
        assert_eq!(capital.health, 2);
        assert_eq!(w.civ(CivId(1)).unwrap().gold, remaining);
        assert!(w.civ(CivId(1)).unwrap().is_alive());
        assert!(!outcome
            .events
            .iter()
            .any(|e| matches!(e, TurnEvent::CityCaptured { .. } | TurnEvent::CivDefeated { .. })));
    }
}

#[test]
fn losing_last_city_immediately_removes_remaining_units_and_rejects_actions() {
    let (r, mut w) = setup(2);
    w.found_city_in(&r, City::found("Home", CivId(0), TileId(20)));
    let city = w.found_city_in(&r, City::found("Target", CivId(1), TileId(150)));
    w.cities.get_mut(city).unwrap().health = 1;
    let worker = w.place_unit(Unit::new(r.unit("Worker").unwrap(), CivId(1), TileId(200), 2));
    let settler = w.place_unit(Unit::new(r.unit("Settler").unwrap(), CivId(1), TileId(201), 2));
    let outside = w.grid.neighbours(TileId(150)).next().unwrap();
    let attacker = w.place_unit(Unit::new(r.unit("Warrior").unwrap(), CivId(0), outside, 2));
    w.diplomacy.meet(CivId(0), CivId(1));
    w.diplomacy.declare_war(CivId(0), CivId(1));
    // Existing agreements must not keep charging a dead civilization.
    w.diplomacy.deals.push(civ_core::world::trade::Deal { a: CivId(0), b: CivId(1), a_gives: vec![], b_gives: vec![] });
    w.diplomacy.demands.insert((CivId(0), CivId(1), civ_core::world::promises::Demand::DontSpyOnUs));
    update_visibility(&mut w);
    let outcome =
        execute(&mut w, &r, CivId(0), Command::Attack { unit: attacker, target: AttackTarget::City(city) }).unwrap();
    let loser = w.civ(CivId(1)).unwrap();
    assert!(loser.defeated && !loser.is_alive());
    assert!(loser.units.is_empty());
    assert!(w.units.get(worker).is_none() && w.units.get(settler).is_none());
    assert!(w.diplomacy.deals.is_empty() && w.diplomacy.demands.is_empty());
    assert!(w.diplomacy.have_met(CivId(0), CivId(1)), "historical knowledge survives");
    assert_eq!(outcome.events.iter().filter(|e| matches!(e, TurnEvent::CivDefeated { civ: CivId(1) })).count(), 1);
    assert_eq!(execute(&mut w, &r, CivId(1), Command::SkipUnit { unit: worker }), Err(CommandRejection::Defeated));
    assert!(w.resolve_defeats().is_empty(), "cleanup and event are once only");
}

#[test]
fn remaining_city_preserves_established_civ_but_a_worker_cannot_replace_it() {
    let (r, mut w) = setup(2);
    w.found_city_in(&r, City::found("Home", CivId(0), TileId(20)));
    let capital = w.found_city_in(&r, City::found("Capital", CivId(1), TileId(150)));
    let second = w.found_city_in(&r, City::found("Second", CivId(1), TileId(220)));
    let worker = w.place_unit(Unit::new(r.unit("Worker").unwrap(), CivId(1), TileId(200), 2));
    conquest::conquer(&mut w, &r, capital, CivId(0)).unwrap();
    assert!(w.resolve_defeats().is_empty());
    assert!(w.civ(CivId(1)).unwrap().is_alive());
    assert!(w.units.get(worker).is_some());
    conquest::destroy(&mut w, second);
    assert_eq!(w.resolve_defeats(), vec![CivId(1)]);
    assert!(w.units.get(worker).is_none());
}

#[test]
fn unfounded_settler_survives_and_first_founding_does_not_trigger_transient_defeat() {
    let (r, mut w) = setup(1);
    let settler = w.place_unit(Unit::new(r.unit("Settler").unwrap(), CivId(0), TileId(100), 2));
    assert!(w.resolve_defeats().is_empty());
    assert!(w.civ(CivId(0)).unwrap().is_alive());
    execute(&mut w, &r, CivId(0), Command::FoundCity { unit: settler, name: "First".into() }).unwrap();
    let civ = w.civ(CivId(0)).unwrap();
    assert!(civ.is_alive() && civ.has_ever_owned_original_capital);
    assert!(!civ.defeated && civ.units.is_empty());
}

#[test]
fn acquiring_a_foreign_original_capital_preserves_establishment_history() {
    let (r, mut w) = setup(3);
    let town = w.found_city(City::found("Town", CivId(0), TileId(20)));
    // Model a city received before this civilization ever founded or held a
    // capital. The ordinary founding helper correctly marks real first cities.
    w.cities.get_mut(town).unwrap().original_capital = false;
    w.civ_mut(CivId(0)).unwrap().has_ever_owned_original_capital = false;
    assert!(!w.civ(CivId(0)).unwrap().is_alive(), "before establishment, cities cannot substitute for units");
    let capital = w.found_city_in(&r, City::found("Original", CivId(1), TileId(150)));
    w.found_city_in(&r, City::found("Third", CivId(2), TileId(220)));
    let worker = w.place_unit(Unit::new(r.unit("Worker").unwrap(), CivId(0), TileId(30), 2));
    conquest::conquer(&mut w, &r, capital, CivId(0)).unwrap();
    assert!(w.civ(CivId(0)).unwrap().has_ever_owned_original_capital);
    conquest::conquer(&mut w, &r, capital, CivId(2)).unwrap();
    assert!(w.civ(CivId(0)).unwrap().is_alive());
    conquest::destroy(&mut w, town);
    assert!(w.resolve_defeats().contains(&CivId(0)));
    assert!(w.units.get(worker).is_none());
}

#[cfg(feature = "save")]
#[test]
fn defeat_history_survives_snapshot_and_inconsistent_original_history_is_rejected() {
    let (r, mut w) = setup(2);
    w.found_city_in(&r, City::found("Home", CivId(0), TileId(20)));
    let capital = w.found_city_in(&r, City::found("Original", CivId(1), TileId(150)));
    conquest::conquer(&mut w, &r, capital, CivId(0)).unwrap();
    w.resolve_defeats();
    let restored = civ_core::save::decode(&civ_core::save::encode(&w, &r).unwrap(), &r).unwrap();
    assert_eq!(restored, w);
    assert!(restored.civ(CivId(1)).unwrap().has_ever_owned_original_capital);
    w.civ_mut(CivId(0)).unwrap().has_ever_owned_original_capital = false;
    assert!(civ_core::save::encode(&w, &r).is_err());
}
