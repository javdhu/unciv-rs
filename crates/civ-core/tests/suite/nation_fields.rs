// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! civ.game's nation fields: regions, voices, second leaders and era renames.
//!
//! Tested on the gods-and-kings fixture with `tests/data/rebuild/Nations.json`
//! layered over it, so the fixture itself stays untouched.

use civ_core::ruleset::Ruleset;
use civ_core::world::assignment::CityFocus;
use civ_core::world::city::City;
use civ_core::world::entities::{Civilization, Unit};
use civ_core::world::grid::{CivId, TileGrid};
use civ_core::world::turn::{advance_turn, TurnEvent, TurnModifiers};
use civ_core::world::world::World;
use civ_core::UniqueType;

use crate::common;

fn rules() -> Ruleset {
    common::rebuild_rules(&["Nations.json"])
}

fn rome_world(r: &Ruleset, alt_leader: bool) -> World {
    let mut rome = Civilization::new(CivId(0), r.nations.id("Rome").unwrap(), true);
    rome.alt_leader = alt_leader;
    let mut w = World::new(TileGrid::new(30, 12, r.terrain("Grassland").unwrap()), vec![rome]);
    civ_core::world::policies::rebuild_uniques(&mut w, r, CivId(0));
    w
}

fn learn(w: &mut World, r: &Ruleset, tech: &str) {
    let id = r.technologies.id(tech).unwrap();
    w.civ_mut(CivId(0)).unwrap().techs.insert(id);
}

fn settle(w: &mut World, r: &Ruleset, x: u16, name: &str) -> civ_core::arena::Id<City> {
    let tile = w.grid.id_at(x, 6).unwrap();
    let settler = Unit::new(r.unit("Settler").unwrap(), CivId(0), tile, 2);
    let settler = w.place_unit(settler);
    civ_core::world::settling::found_city_with(w, r, settler, name).expect("a legal site")
}

#[test]
fn nations_read_region_voice_second_leader_and_renames() {
    let r = rules();
    let rome = r.nations.by_name("Rome").unwrap();
    assert_eq!(rome.region.as_deref(), Some("Europe"));
    assert_eq!(rome.voice.as_deref(), Some("Italian"));
    let alt = rome.alt_leader.as_ref().unwrap();
    assert_eq!(alt.leader_name, "Galla Placidia");
    assert_eq!(alt.uniques[0].unique_type(), Some(UniqueType::StartsWithTech));
    assert_eq!(rome.leaders().collect::<Vec<_>>(), ["Marcus Aurelius", "Galla Placidia"]);
    assert_eq!(rome.city_renames.len(), 4);
    assert!(civ_core::ruleset::civgame::REGIONS.contains(&rome.region.as_deref().unwrap()));

    // The fixtures carry none of it.
    let gk = Ruleset::load_dir("rules/gods-and-kings").unwrap().unwrap();
    assert!(gk.nations.iter().all(|(_, n)| n.region.is_none() && n.alt_leader.is_none() && n.city_renames.is_empty()));
    assert_eq!(gk.nations.iter().filter(|(_, n)| n.is_barbarian).count(), 1);
}

#[test]
fn the_second_leader_replaces_the_nations_ability() {
    let r = rules();
    let production = UniqueType::PercentProductionBuildingsInCapital;
    let default = rome_world(&r, false);
    let civ = default.civ(CivId(0)).unwrap();
    assert!(civ.uniques.matching(production).next().is_some());
    assert!(civ.uniques.matching(UniqueType::StartsWithTech).next().is_none());

    let alt = rome_world(&r, true);
    let civ = alt.civ(CivId(0)).unwrap();
    assert!(civ.uniques.matching(production).next().is_none(), "the default leader's ability is gone");
    assert!(civ.uniques.matching(UniqueType::StartsWithTech).next().is_some());
    assert_eq!(r.nations[civ.nation].leader(civ.alt_leader), Some("Galla Placidia"));
}

#[test]
fn cities_rename_when_their_owner_enters_the_era() {
    let r = rules();
    let mut w = rome_world(&r, false);
    let roma = settle(&mut w, &r, 3, "Roma");
    let neapolis = settle(&mut w, &r, 10, "Neapolis");
    let byzantion = settle(&mut w, &r, 17, "Byzantion");
    assert_eq!(w.cities.get(neapolis).unwrap().name, "Neapolis", "founded under its period name");

    learn(&mut w, &r, "Theology"); // Medieval: Byzantion passes Constantinople on the way.
    let events = advance_turn(&mut w, &r, CityFocus::None, TurnModifiers::default());
    let renamed: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            TurnEvent::CityRenamed { city, from, to } => Some((*city, from.as_str(), to.as_str())),
            _ => None,
        })
        .collect();
    assert_eq!(renamed, [(neapolis, "Neapolis", "Napoli"), (byzantion, "Byzantion", "Constantinople")]);
    assert_eq!(w.cities.get(roma).unwrap().name, "Roma");

    learn(&mut w, &r, "Navigation"); // Renaissance
    advance_turn(&mut w, &r, CityFocus::None, TurnModifiers::default());
    assert_eq!(w.cities.get(byzantion).unwrap().name, "Istanbul");
    let quiet = advance_turn(&mut w, &r, CityFocus::None, TurnModifiers::default());
    assert!(!quiet.iter().any(|e| matches!(e, TurnEvent::CityRenamed { .. })), "a rename happens once");
}

#[test]
fn a_city_founded_in_a_later_era_takes_that_eras_name() {
    let r = rules();
    let mut w = rome_world(&r, false);
    learn(&mut w, &r, "Theology");
    let city = settle(&mut w, &r, 5, "Mediolanum");
    assert_eq!(w.cities.get(city).unwrap().name, "Milano");
}

#[test]
fn the_ai_names_cities_from_its_list_in_its_eras_form() {
    let r = rules();
    let mut w = rome_world(&r, false);
    settle(&mut w, &r, 3, "Roma");
    learn(&mut w, &r, "Theology");
    let next = civ_core::world::city_names::next_city_name(&w, &r, CivId(0), &[]);
    assert_eq!(next.as_deref(), Some("Napoli"));
    let after = civ_core::world::city_names::next_city_name(&w, &r, CivId(0), &["Napoli".into()]);
    assert_eq!(after.as_deref(), Some("Milano"));
}
