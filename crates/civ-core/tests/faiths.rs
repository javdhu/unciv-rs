//! Faiths that arise by era (docs/CONTENT.md, Religion; `world::faiths`).
//!
//! The gods-and-kings fixture with `tests/data/rebuild/Religions.json` and
//! `Beliefs.json` layered over it: Hinduism arises early in India, Judaism
//! a little later wherever faith is strongest, Christianity in the Medieval
//! era in Rome.

use civ_core::arena::Id;
use civ_core::ruleset::Ruleset;
use civ_core::stats::{Fixed, Stat};
use civ_core::world::assignment::CityFocus;
use civ_core::world::city::City;
use civ_core::world::commands::{execute, Command};
use civ_core::world::construction::{Construction, CostModifiers};
use civ_core::world::entities::Civilization;
use civ_core::world::faiths::{self, StateFaithRejection};
use civ_core::world::grid::{CivId, TileGrid};
use civ_core::world::religion;
use civ_core::world::religious_units::ReligiousAction;
use civ_core::world::turn::{advance_turn, TurnEvent, TurnModifiers};
use civ_core::world::world::World;
use civ_core::UniqueType;

mod common;

const INDIA: CivId = CivId(0);
const ROME: CivId = CivId(1);
const EGYPT: CivId = CivId(2);

fn rules() -> Ruleset {
    common::rebuild_rules(&["Religions.json", "Beliefs.json"])
}

/// Three majors with a capital each, eight tiles apart, all human so that
/// nobody adopts a faith unasked.
fn world(r: &Ruleset) -> (World, [Id<City>; 3]) {
    let civs = ["India", "Rome", "Egypt"]
        .iter()
        .enumerate()
        .map(|(i, n)| Civilization::new(CivId(i as u8), r.nations.id(n).unwrap(), false))
        .collect();
    let mut w = World::new(TileGrid::new(30, 12, r.terrain("Grassland").unwrap()), civs);
    let mut cities = Vec::new();
    for (i, x) in [4u16, 12, 20].into_iter().enumerate() {
        let civ = CivId(i as u8);
        let centre = w.grid.id_at(x, 6).unwrap();
        let city = w.found_city_in(r, City::found(format!("City {i}"), civ, centre));
        for t in w.grid.within(centre, 1) {
            w.claim_tile(city, t);
        }
        w.cities.get_mut(city).unwrap().population = 4;
        cities.push(city);
    }
    for i in 0..3 {
        civ_core::world::policies::rebuild_uniques(&mut w, r, CivId(i));
    }
    (w, [cities[0], cities[1], cities[2]])
}

fn turn(w: &mut World, r: &Ruleset) -> Vec<TurnEvent> {
    advance_turn(w, r, CityFocus::None, TurnModifiers::default())
}

fn arose(events: &[TurnEvent]) -> Vec<(CivId, Id<City>)> {
    events
        .iter()
        .filter_map(|e| match e {
            TurnEvent::FaithArose { religion, city } => Some((*religion, *city)),
            _ => None,
        })
        .collect()
}

fn give_shrines(w: &mut World, r: &Ruleset, city: Id<City>) {
    let shrine = r.building("Shrine").unwrap();
    w.cities.get_mut(city).unwrap().built.insert(shrine);
}

#[test]
fn religions_json_objects_switch_on_faiths_by_era() {
    let r = rules();
    assert!(r.faith_mode());
    assert_eq!(r.faiths.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(), ["Hinduism", "Judaism", "Christianity"]);
    assert_eq!(r.faiths[0].homeland, ["India"]);
    assert_eq!(r.faiths[2].homeland, ["Rome"], "a single homeland may be a string");
    assert_eq!(r.faiths[0].delay, 2);
    assert_eq!(r.religions, ["Hinduism", "Judaism", "Christianity"]);
    assert_eq!(faiths::index_of(faiths::key(2)), Some(2));
    assert_eq!(faiths::index_of(CivId(5)), None);

    let gk = Ruleset::load_dir("rules/gods-and-kings").unwrap().unwrap();
    assert!(!gk.faith_mode() && gk.faiths.is_empty() && !gk.religions.is_empty());
}

#[test]
fn a_faith_arises_in_its_homeland_after_its_delay_and_then_spreads() {
    let r = rules();
    let (mut w, [delhi, rome, _]) = world(&r);
    give_shrines(&mut w, &r, rome); // Rome has more faith; the homeland still wins.
    let hinduism = faiths::key(0);
    let mut when = None;
    for t in 0..6 {
        let events = turn(&mut w, &r);
        if arose(&events).contains(&(hinduism, delhi)) {
            when = Some(t);
            break;
        }
    }
    assert_eq!(when, Some(2), "it waits out its two-turn delay");
    let city = w.cities.get(delhi).unwrap();
    assert_eq!(city.faith.holy_city_of, Some(hinduism));
    assert_eq!(city.faith.majority(city.population), Some(hinduism));
    assert!(religion::religion_exists(&w, hinduism));
    assert!(w.faiths.arisen.contains_key(&0));

    // It spreads by pressure like any religion.
    let mut converted = false;
    for _ in 0..60 {
        for e in turn(&mut w, &r) {
            if matches!(e, TurnEvent::CityConverted { to, .. } if to == hinduism) {
                converted = true;
            }
        }
    }
    assert!(converted, "a neighbouring city came to follow it");
}

#[test]
fn without_a_homeland_a_faith_arises_where_faith_is_strongest() {
    let r = rules();
    let (mut w, [delhi, _, memphis]) = world(&r);
    give_shrines(&mut w, &r, memphis);
    let mut seen = Vec::new();
    for _ in 0..8 {
        seen.extend(arose(&turn(&mut w, &r)));
    }
    assert_eq!(seen, [(faiths::key(0), delhi), (faiths::key(1), memphis)], "Judaism needs no homeland");
}

#[test]
fn a_later_eras_faith_waits_for_the_world_to_reach_it() {
    let r = rules();
    let (mut w, [_, rome, memphis]) = world(&r);
    give_shrines(&mut w, &r, memphis); // Judaism arises there, leaving Rome free.
    for _ in 0..8 {
        turn(&mut w, &r);
    }
    assert!(!faiths::has_arisen(&w, faiths::key(2)), "no civ is in the Medieval era yet");
    // A civ that skips straight past the era raises it at once.
    let tech = r.technologies.id("Navigation").unwrap();
    w.civ_mut(EGYPT).unwrap().techs.insert(tech);
    let events = turn(&mut w, &r);
    assert_eq!(arose(&events), [(faiths::key(2), rome)]);
}

#[test]
fn nobody_founds_a_pantheon_or_religion_when_faiths_arise_by_era() {
    let r = rules();
    let (mut w, _) = world(&r);
    w.civ_mut(INDIA).unwrap().faith.stored = 5000;
    assert!(!religion::can_found_pantheon(&w, &r, INDIA));
    assert_eq!(religion::remaining_foundable_religions(&w, &r), 0);
    let event = religion::apply_faith(&mut w, &r, INDIA, 10, Fixed::ONE);
    assert!(matches!(event, religion::FaithEvent::Stored { faith: 5010 }));
    let pantheon = religion::available_beliefs(&w, &r, "Pantheon")[0];
    assert!(execute(&mut w, &r, INDIA, Command::ChoosePantheon { belief: pantheon }).is_err());
    assert!(w.civ(INDIA).unwrap().faith.beliefs.is_empty());
}

#[test]
fn a_state_faith_brings_its_traits_and_holds_for_a_while() {
    let r = rules();
    let (mut w, [delhi, _, memphis]) = world(&r);
    give_shrines(&mut w, &r, memphis);
    for _ in 0..8 {
        turn(&mut w, &r);
    }
    let (hinduism, judaism) = (faiths::key(0), faiths::key(1));
    assert_eq!(faiths::adoptable(&w, &r, INDIA), [hinduism]);
    assert_eq!(faiths::can_adopt(&w, &r, INDIA, judaism), Err(StateFaithRejection::NotFollowed));
    assert!(execute(&mut w, &r, INDIA, Command::AdoptStateFaith { religion: judaism }).is_err());

    let outcome = execute(&mut w, &r, INDIA, Command::AdoptStateFaith { religion: hinduism }).unwrap();
    assert_eq!(outcome.events, [TurnEvent::StateFaithAdopted { civ: INDIA, religion: hinduism }]);
    let india = w.civ(INDIA).unwrap();
    assert_eq!(india.faith.state, Some(hinduism));
    assert_eq!(religion::civ_religion(&w, INDIA), Some(hinduism));
    assert!(india.uniques.matching(UniqueType::StatsFromGlobalCitiesFollowingReligion).next().is_some());
    let gold = civ_core::world::city_effects::founder_yields(&w, &r, INDIA)[Stat::Gold];
    assert!(gold >= Fixed::whole(2), "+2 Gold for each city following it: {gold:?}");

    // Delhi follows Hinduism, so its follower trait applies there, and only there.
    let traits = |city| {
        let c = w.cities.get(city).unwrap();
        religion::city_belief_uniques(&w, &r, c).into_iter().map(|u| u.text.clone()).collect::<Vec<_>>()
    };
    assert_eq!(traits(delhi), ["[+3 Culture] from every [Monument]"]);
    assert_eq!(traits(memphis), ["[+1 Happiness] [in cities following this religion]"]);

    // Changing takes STATE_FAITH_LOCK turns.
    let c = w.cities.get_mut(delhi).unwrap();
    c.faith.add_pressure(Some(judaism), 100_000);
    c.faith.recount(c.population);
    assert!(matches!(faiths::can_adopt(&w, &r, INDIA, judaism), Err(StateFaithRejection::TooSoon { .. })));
    w.turn += faiths::STATE_FAITH_LOCK;
    assert_eq!(faiths::can_adopt(&w, &r, INDIA, judaism), Ok(()));
}

#[test]
fn computer_players_take_up_the_faith_their_people_follow() {
    let r = rules();
    let (mut w, _) = world(&r);
    w.civ_mut(INDIA).unwrap().is_ai = true;
    let mut adopted = None;
    for _ in 0..6 {
        for e in turn(&mut w, &r) {
            if let TurnEvent::StateFaithAdopted { civ, religion } = e {
                adopted = Some((civ, religion));
            }
        }
    }
    assert_eq!(adopted, Some((INDIA, faiths::key(0))));
    assert_eq!(w.civ(ROME).unwrap().faith.state, None, "a human is never chosen for");
}

#[test]
fn missionaries_carry_an_arisen_faith() {
    let r = rules();
    let (mut w, [delhi, rome, _]) = world(&r);
    for _ in 0..4 {
        turn(&mut w, &r);
    }
    let hinduism = faiths::key(0);
    w.civ_mut(INDIA).unwrap().faith.stored = 5000;
    let missionary = r.unit("Missionary").unwrap();
    let costs = CostModifiers::default();
    civ_core::world::purchase::purchase(&mut w, &r, delhi, Construction::Unit(missionary), Stat::Faith, costs)
        .expect("a city following an arisen faith buys missionaries");
    let (id, unit) = w.units.iter().find(|(_, u)| u.base == missionary).unwrap();
    assert_eq!(unit.religion, Some(hinduism));

    let target = w.cities.get(rome).unwrap().centre;
    let unit = w.units.get_mut(id).unwrap();
    unit.position = target;
    unit.movement_left = Fixed::whole(4);
    let event =
        civ_core::world::religious_units::perform(&mut w, &r, id, ReligiousAction::SpreadReligion(rome)).unwrap();
    assert!(
        matches!(event, civ_core::world::religious_units::ReligiousEvent::Spread { religion, .. } if religion == hinduism)
    );
    assert!(w.cities.get(rome).unwrap().faith.pressure_of(Some(hinduism)) > 0);
}

#[cfg(feature = "save")]
#[test]
fn faiths_survive_a_save() {
    let r = rules();
    let (mut w, _) = world(&r);
    w.civ_mut(INDIA).unwrap().is_ai = true;
    for _ in 0..6 {
        turn(&mut w, &r);
    }
    assert!(!w.faiths.arisen.is_empty() && w.civ(INDIA).unwrap().faith.state.is_some());
    let text = civ_core::save::encode(&w, &r).unwrap();
    assert_eq!(civ_core::save::decode(&text, &r).unwrap(), w);
}

#[test]
fn a_state_faith_presses_gently_in_its_adopters_cities() {
    let r = rules();
    let (mut w, [_, rome, memphis]) = world(&r);
    give_shrines(&mut w, &r, memphis);
    for _ in 0..8 {
        turn(&mut w, &r);
    }
    let judaism = faiths::key(1);
    execute(&mut w, &r, EGYPT, Command::AdoptStateFaith { religion: judaism }).unwrap();
    // Rome is Egypt's neighbour but not Egypt's: only Egypt's own cities get the state push.
    let before = |w: &World, c| w.cities.get(c).unwrap().faith.pressure_of(Some(judaism));
    let (rome_before, memphis_before) = (before(&w, rome), before(&w, memphis));
    religion::spread(&mut w, &r, 10);
    let memphis_gain = before(&w, memphis) - memphis_before;
    let rome_gain = before(&w, rome) - rome_before;
    assert!(memphis_gain > 5 * 10, "holy city push plus the state's: {memphis_gain}");
    assert!(rome_gain < memphis_gain);
    assert_eq!(
        religion::STATE_FAITH_PRESSURE * 10,
        10,
        "a fifth of a holy city's {}",
        religion::HOLY_CITY_MULTIPLIER * 10
    );
}

#[test]
fn trade_routes_carry_faith_between_a_city_and_its_capital() {
    let r = rules();
    const ME: CivId = CivId(0);
    let rome = Civilization::new(ME, r.nations.id("Rome").unwrap(), false);
    let mut w = World::new(TileGrid::new(40, 8, r.terrain("Grassland").unwrap()), vec![rome]);
    w.civ_mut(ME).unwrap().techs.insert(r.technologies.id("The Wheel").unwrap());
    let capital = w.found_city_in(&r, City::found("Rome", ME, w.grid.id_at(2, 4).unwrap()));
    let far = w.found_city_in(&r, City::found("Far", ME, w.grid.id_at(30, 4).unwrap()));
    let hinduism = faiths::key(0);
    w.faiths.arisen.insert(0, faiths::Arisen { turn: 0, city: capital });
    let c = w.cities.get_mut(capital).unwrap();
    c.faith.add_pressure(Some(hinduism), 1000);
    c.faith.recount(c.population);
    let pressure = |w: &World| w.cities.get(far).unwrap().faith.pressure_of(Some(hinduism));

    religion::spread(&mut w, &r, 10);
    assert_eq!(pressure(&w), 0, "28 tiles away and no road");
    for x in 2..=30 {
        let t = w.grid.id_at(x, 4).unwrap();
        w.grid.set_road(t, civ_core::world::grid::RoadLevel::Road);
        w.grid.owner[t.0 as usize] = Some(ME);
    }
    religion::spread(&mut w, &r, 10);
    assert_eq!(pressure(&w), 10 * religion::TRADE_ROUTE_PRESSURE_PERCENT / 100, "half a city's push along the road");

    // The fixtures' founded religions do not travel this way.
    let gk = Ruleset::load_dir("rules/gods-and-kings").unwrap().unwrap();
    assert!(!gk.faith_mode());
}
