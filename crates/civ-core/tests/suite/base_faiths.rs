//! The faith data `rules/base` ships (docs/CONTENT.md, Religion) holds
//! together: the eleven faiths in their eras, homelands on the roster, and
//! one follower and one state trait each, every one a rule the engine knows.

use civ_core::ruleset::Ruleset;
use civ_core::UniqueKind;

const ERAS: [&str; 8] = [
    "Age of Bronze",
    "Age of Iron",
    "Age of Paper",
    "Age of Sail",
    "Age of Steam",
    "Age of Electricity",
    "Age of the Atom",
    "Age of Information",
];

/// docs/ROSTER.md wave 1, as Nations.json names them.
const ROSTER: [&str; 16] = [
    "Rome",
    "England",
    "United States",
    "Persia",
    "Ottomans",
    "Egypt",
    "Mali",
    "Ndongo",
    "Maurya",
    "Chola",
    "Tang China",
    "Japan",
    "Mongols",
    "Majapahit",
    "Triple Alliance",
    "Tawantinsuyu",
];

#[test]
fn base_faiths_arise_in_the_eras_content_gives_them() {
    let r = Ruleset::load_dir("rules/base").unwrap().unwrap();
    assert!(r.faith_mode());
    let by_era: Vec<(&str, &str)> = r.faiths.iter().map(|f| (f.name.as_str(), f.era.as_str())).collect();
    assert_eq!(
        by_era,
        [
            ("Hinduism", "Age of Bronze"),
            ("Zoroastrianism", "Age of Iron"),
            ("Judaism", "Age of Iron"),
            ("Jainism", "Age of Iron"),
            ("Buddhism", "Age of Iron"),
            ("Confucianism", "Age of Iron"),
            ("Taoism", "Age of Iron"),
            ("Christianity", "Age of Iron"),
            ("Islam", "Age of Paper"),
            ("Shinto", "Age of Paper"),
            ("Sikhism", "Age of Sail"),
        ]
    );
    for f in &r.faiths {
        assert!(ERAS.contains(&f.era.as_str()), "{}: {}", f.name, f.era);
        assert!(f.delay < 30, "{} arises within its era", f.name);
        for home in &f.homeland {
            assert!(ROSTER.contains(&home.as_str()), "{}: homeland {home} is not a wave 1 nation", f.name);
        }
    }
    // Christianity comes late in the Age of Iron.
    let iron: Vec<_> = r.faiths.iter().filter(|f| f.era == "Age of Iron").collect();
    assert_eq!(iron.iter().max_by_key(|f| f.delay).unwrap().name, "Christianity");
}

#[test]
fn every_faith_has_one_follower_and_one_state_trait_the_engine_knows() {
    let r = Ruleset::load_dir("rules/base").unwrap().unwrap();
    let mut used = std::collections::BTreeSet::new();
    for f in &r.faiths {
        let kinds: Vec<_> = f
            .beliefs
            .iter()
            .map(|name| {
                r.beliefs.by_name(name).unwrap_or_else(|| panic!("{}: no belief {name}", f.name)).r#type.as_str()
            })
            .collect();
        assert_eq!(kinds, ["Follower", "State"], "{}", f.name);
        for name in &f.beliefs {
            assert!(used.insert(name.clone()), "{name} belongs to two faiths");
            for u in &r.beliefs.by_name(name).unwrap().uniques {
                assert!(matches!(u.kind, UniqueKind::Known(_)), "{name}: {} is not a rule", u.text);
                assert!(u.conditionals.iter().all(|c| matches!(c.kind, UniqueKind::Known(_))), "{name}: {}", u.text);
            }
        }
    }
    assert_eq!(used.len(), r.beliefs.len(), "every belief is some faith's trait; nothing is left to pick");
}
