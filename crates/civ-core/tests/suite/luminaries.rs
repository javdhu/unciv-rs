// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Luminaries' names, region first (docs/CONTENT.md, Great people).

use civ_core::ruleset::civgame::REGIONS;
use civ_core::ruleset::Ruleset;
use civ_core::world::city::City;
use civ_core::world::entities::Civilization;
use civ_core::world::grid::{CivId, TileGrid};
use civ_core::world::luminaries::candidates;
use civ_core::world::world::World;

use crate::common;

/// Religious founders never appear as units; legendary figures and a ruler
/// the old lists misfiled are gone.
const NEVER: [&str; 19] = [
    "Siddhartha Gautama",
    "Gautama Buddha",
    "Jesus",
    "Muhammad",
    "Guru Nanak",
    "Mahavira",
    "Zoroaster",
    "Zarathustra",
    "Confucius",
    "Laozi",
    "Moses",
    "Merit-Ptah",
    "Xi Ling Shi",
    "Homer",
    "Valmiki",
    "Sun Tzu",
    "Ling Lun",
    "Shah Jahan",
    "Genghis",
];

/// docs/ROSTER.md wave 1 leaders: a leader is never also a Luminary.
const LEADERS: [&str; 32] = [
    "Marcus Aurelius",
    "Galla Placidia",
    "Alfred the Great",
    "Æthelflæd",
    "Abraham Lincoln",
    "Dwight D. Eisenhower",
    "Khosrow I",
    "Purandokht",
    "Osman I",
    "Kösem Sultan",
    "Thutmose III",
    "Hatshepsut",
    "Sundiata Keita",
    "Mansa Musa",
    "Njinga Mbande",
    "Ngola Kiluanje",
    "Ashoka",
    "Chandragupta",
    "Rajaraja I",
    "Rajendra I",
    "Taizong",
    "Wu Zetian",
    "Empress Suiko",
    "Prince Shōtoku",
    "Genghis Khan",
    "Mandukhai",
    "Hayam Wuruk",
    "Tribhuwana",
    "Nezahualcoyotl",
    "Itzcoatl",
    "Túpac Inca Yupanqui",
    "Pachacuti",
];

#[test]
fn base_pools_cover_every_region_and_keep_to_real_people() {
    let r = Ruleset::load_dir("rules/base").unwrap().unwrap();
    let mut seen = std::collections::BTreeSet::new();
    for group in &r.name_groups {
        let names: Vec<&String> = group.unit_names.iter().chain(group.regions.values().flatten()).collect();
        if group.name != "Khan" {
            assert_eq!(group.regions.keys().map(String::as_str).collect::<Vec<_>>().len(), 8, "{}", group.name);
            for region in REGIONS {
                assert!(group.regions.contains_key(region), "{} has no {region} pool", group.name);
            }
        }
        for name in names {
            assert!(seen.insert(name.clone()), "{name} appears twice");
            assert!(!NEVER.iter().any(|n| name == n), "{name} may not be a Luminary");
            assert!(!LEADERS.iter().any(|n| name.contains(n)), "{name} leads a nation");
        }
    }
    // Every group a unit names resolves, so no Luminary goes unnamed by accident.
    for (_, unit) in r.units.iter() {
        for u in unit.uniques.iter().filter(|u| u.unique_type() == Some(civ_core::UniqueType::OneTimeUnitGetsName)) {
            let group = &u.params[1];
            assert!(r.name_groups.iter().any(|g| g.name == *group), "{}: no name group {group}", unit.name);
        }
    }
    // The misfiled economist moved from Magnates to Scholars.
    let scholars = r.name_groups.iter().find(|g| g.name == "Scientist").unwrap();
    assert!(scholars.regions["Europe"].iter().any(|n| n == "Adam Smith"));
}

#[test]
fn candidates_start_with_the_region_then_take_the_world_in_turn() {
    let r = Ruleset::load_dir("rules/base").unwrap().unwrap();
    let scholars = r.name_groups.iter().find(|g| g.name == "Scientist").unwrap();
    let asia = candidates(scholars, Some("SouthAsia"));
    assert_eq!(asia[0], "Aryabhata");
    let own = scholars.regions["SouthAsia"].len();
    assert_eq!(&asia[own..own + 3], ["Aristotle", "Al-Khwarizmi", "Euclid"], "then one from each other region");
    let total: usize = scholars.regions.values().map(Vec::len).sum();
    assert_eq!(asia.len(), total, "every name is offered once");
    assert_eq!(candidates(scholars, None)[0], "Aristotle");
}

#[test]
fn luminaries_take_the_next_unused_name_from_their_region() {
    let r = {
        let mut r = common::rebuild_rules(&["Nations.json", "../../../rules/base/UnitNameGroups.json"]);
        let egypt = r.nations.id("Egypt").unwrap();
        r.nations.get_mut(egypt).unwrap().region = Some("Africa".into());
        r
    };
    let civs = ["Rome", "Egypt"]
        .iter()
        .enumerate()
        .map(|(i, n)| Civilization::new(CivId(i as u8), r.nations.id(n).unwrap(), false))
        .collect();
    let mut w = World::new(TileGrid::new(20, 10, r.terrain("Grassland").unwrap()), civs);
    for (i, x) in [4u16, 14].into_iter().enumerate() {
        let centre = w.grid.id_at(x, 5).unwrap();
        w.found_city_in(&r, City::found(format!("City {i}"), CivId(i as u8), centre));
    }
    let scientist = r.units.id("Great Scientist").unwrap();
    let born = |w: &mut World, civ: u8| {
        w.civ_mut(CivId(civ)).unwrap().pending_rewards.free_great_people += 1;
        let id = civ_core::world::effects::claim_free_great_person(w, &r, CivId(civ), scientist).expect("a free one");
        w.units.get(id).unwrap().name.clone().unwrap()
    };
    assert_eq!(born(&mut w, 0), "Aristotle");
    assert_eq!(born(&mut w, 0), "Nicolaus Copernicus");
    assert_eq!(born(&mut w, 1), "Euclid");
    // Nobody is named twice in a game.
    assert!(w.given_names.contains("Aristotle") && w.given_names.len() == 3);
}
