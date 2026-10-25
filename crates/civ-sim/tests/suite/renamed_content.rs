// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The engine keys nothing on content names that `rules/base` renames.
//!
//! The Gods & Kings rules, with eras, barbarians, difficulties, great people
//! and the science and diplomacy projects renamed
//! the way `rules/base` renames them, must play exactly the same game.

use civ_core::ruleset::Ruleset;
use civ_sim::{advance, content, new_game_in, Settings};

const ERAS: [(&str, &str); 9] = [
    ("Ancient era", "Age of Bronze"),
    ("Classical era", "Age of Iron"),
    ("Medieval era", "Age of Paper"),
    ("Renaissance era", "Age of Sail"),
    ("Industrial era", "Age of Steam"),
    ("Modern era", "Age of Electricity"),
    ("Atomic era", "Age of the Atom"),
    ("Information era", "Age of Information"),
    ("Future era", "Age of Tomorrow"),
];

const DIFFICULTIES: [(&str, &str); 8] = [
    ("Settler", "Pupil"),
    ("Chieftain", "Student"),
    ("Warlord", "Graduate"),
    ("Prince", "Tutor"),
    ("King", "Lecturer"),
    ("Emperor", "Professor"),
    ("Immortal", "Dean"),
    ("Deity", "Chancellor"),
];

/// Unit and building names, renamed wherever they stand as a name: quoted or
/// in a unique's brackets. Unique templates keep their words ("Great General
/// provides double combat bonus" is an identifier, not a reference).
const NAMES: [(&str, &str); 13] = [
    ("Great Prophet", "Sage"),
    ("Great Scientist", "Scholar"),
    ("Great Engineer", "Master Builder"),
    ("Great Merchant", "Magnate"),
    ("Great Artist", "Virtuoso"),
    ("Great General", "Marshal"),
    ("Great Admiral", "Admiral"),
    ("Apollo Program", "The Turing Project"),
    ("United Nations", "World Charter"),
    ("SS Booster", "Compute"),
    ("SS Cockpit", "Data"),
    ("SS Engine", "Models"),
    ("SS Stasis Chamber", "Alignment"),
];

/// The frozen Gods & Kings fixture, in the order `content::SOURCES` lists
/// its files. `rules/base` itself is no longer G&K with new names.
fn fixture_sources() -> Vec<(String, String)> {
    let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/../civ-core/rules/gods-and-kings/");
    content::SOURCES
        .iter()
        .map(|(file, _)| (file.to_string(), std::fs::read_to_string(format!("{dir}{file}")).unwrap()))
        .collect()
}

fn renamed_sources() -> Vec<(String, String)> {
    fixture_sources()
        .into_iter()
        .map(|(file, text)| {
            let mut text = text;
            for (old, new) in ERAS {
                text = text.replace(old, new);
            }
            for (old, new) in NAMES {
                text = text.replace(&format!("\"{old}\""), &format!("\"{new}\""));
                text = text.replace(&format!("[{old}]"), &format!("[{new}]"));
            }
            match file.as_str() {
                "Nations.json" => {
                    text = text.replace(
                        r#""name": "Barbarians","#,
                        r#""name": "Raiders", "isBarbarian": true,"#,
                    );
                    assert!(text.contains("Raiders"), "the barbarian entry is renamed");
                }
                "Units.json" => text = text.replace(r#""Barbarians""#, r#""Raiders""#),
                "Ruins.json" => {
                    for (old, new) in DIFFICULTIES {
                        let on = |n: &str| format!("<on [{n}] difficulty");
                        text = text.replace(&on(old), &on(new));
                        if old != "Settler" {
                            text = text.replace(&format!("\"{old}\""), &format!("\"{new}\""));
                        }
                    }
                }
                "Difficulties.json" => {
                    // Names only: the bonus unit lists also say "Settler".
                    for (old, new) in DIFFICULTIES {
                        for key in ["name", "aiDifficultyLevel"] {
                            let quoted = |n: &str| format!("\"{key}\": \"{n}\"");
                            text = text.replace(&quoted(old), &quoted(new));
                        }
                    }
                }
                _ => {}
            }
            (file.to_string(), text)
        })
        .collect()
}

fn digest(settings: &Settings, rules: Ruleset, turns: u32) -> Vec<String> {
    let (mut w, r, _) = new_game_in(settings, &[], rules).expect("game");
    for _ in 0..turns {
        advance(&mut w, &r);
    }
    let mut out: Vec<String> = w
        .civs
        .iter()
        .map(|c| {
            format!(
                "civ {} techs {} cities {} gold {} culture {} faith {} policies {} religion {}",
                c.id.0,
                c.techs.len(),
                c.cities.len(),
                c.gold,
                c.culture_stored,
                c.faith.stored,
                c.adopted_policies.len(),
                c.faith.founded
            )
        })
        .collect();
    out.push(format!("units {}", w.units.iter().count()));
    out.push(format!("turn {}", w.turn));
    out
}

#[test]
fn renamed_content_plays_the_same_game() {
    let original = Ruleset::from_sources(&fixture_sources()).unwrap();
    let renamed = Ruleset::from_sources(&renamed_sources()).unwrap();
    assert_eq!(renamed.first_era().unwrap().name, "Age of Bronze");
    assert_eq!(
        renamed.nations[renamed.barbarian_nation().unwrap()].name,
        "Raiders"
    );
    assert_eq!(renamed.default_difficulty().unwrap().name, "Tutor");
    assert_eq!(original.default_difficulty().unwrap().name, "Prince");
    assert!(renamed.nations.by_name("Barbarians").is_none());
    assert!(
        renamed.units.by_name("Sage").is_some() && renamed.units.by_name("Great Prophet").is_none()
    );

    let settings = Settings {
        seed: 7,
        ..Settings::default()
    };
    assert_eq!(
        digest(&settings, original, 100),
        digest(&settings, renamed, 100)
    );
}

#[test]
fn mod_options_can_name_the_default_difficulty() {
    let mut sources = renamed_sources();
    sources.push((
        "ModOptions.json".into(),
        r#"{ "defaultDifficulty": "Dean" }"#.into(),
    ));
    let r = Ruleset::from_sources(&sources).unwrap();
    assert_eq!(r.default_difficulty().unwrap().name, "Dean");
}

/// `rules/base`'s faiths and traits on the renamed Gods & Kings rules: they
/// arise, spread, are adopted, and the game stays deterministic.
#[test]
fn base_faiths_arise_and_are_adopted_in_a_full_game() {
    let mut sources = renamed_sources();
    sources.push((
        "Religions.json".into(),
        include_str!("../../../civ-core/rules/base/Religions.json").into(),
    ));
    sources.push((
        "Beliefs.json".into(),
        include_str!("../../../civ-core/rules/base/Beliefs.json").into(),
    ));
    let play = || {
        let rules = Ruleset::from_sources(&sources).unwrap();
        assert!(rules.faith_mode());
        let settings = Settings {
            seed: 11,
            majors: 6,
            city_states: 3,
            ..Settings::default()
        };
        let (mut w, r, _) = new_game_in(&settings, &[], rules).expect("game");
        for _ in 0..140 {
            advance(&mut w, &r);
        }
        (w, r)
    };
    let (w, r) = play();
    let arisen: Vec<_> = w
        .faiths
        .arisen
        .keys()
        .map(|i| r.faiths[usize::from(*i)].name.clone())
        .collect();
    assert!(
        arisen.len() >= 2,
        "Hinduism and the first faiths of the Age of Iron arose: {arisen:?}"
    );
    assert!(
        w.civs.iter().any(|c| c.faith.state.is_some()),
        "someone adopted a state faith"
    );
    assert!(
        w.civs
            .iter()
            .all(|c| !c.faith.founded && c.faith.beliefs.is_empty()),
        "nobody founded or picked"
    );
    let followed = w
        .cities
        .iter()
        .filter(|(_, c)| c.faith.majority(c.population).is_some())
        .count();
    let adopters = w.civs.iter().filter(|c| c.faith.state.is_some()).count();
    println!(
        "arisen {arisen:?}; {adopters} state faiths; {followed}/{} cities follow one",
        w.cities.iter().count()
    );
    assert!(
        followed > arisen.len(),
        "faiths spread beyond their holy cities"
    );
    assert_eq!(play().0, w, "the same seed plays the same game");
}
