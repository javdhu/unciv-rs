//! Cross-check the shipped rulesets against the packed atlases.
//!
//! Unciv names sprites after ruleset objects, so an object with no reachable
//! icon is a missing texture waiting to happen. `civ_core::icons` holds the
//! resolution chains; this asserts they cover the real data completely.

mod common;
use civ_core::generated::assets::Sprite;
use civ_core::icons;
use common::read_json;
use std::path::Path;

const RULESET: &str = "../android/assets/jsons/Civ V - Gods & Kings";

/// Every object carrying a `name`, paired with its enclosing json object.
fn objects(file: &str) -> Vec<serde_json::Map<String, serde_json::Value>> {
    fn walk(v: &serde_json::Value, out: &mut Vec<serde_json::Map<String, serde_json::Value>>) {
        match v {
            serde_json::Value::Object(m) => {
                if m.contains_key("name") {
                    out.push(m.clone());
                }
                m.values().for_each(|x| walk(x, out));
            }
            serde_json::Value::Array(a) => a.iter().for_each(|x| walk(x, out)),
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(&read_json(&Path::new(RULESET).join(file)), &mut out);
    out
}

fn name_of(m: &serde_json::Map<String, serde_json::Value>) -> String {
    m["name"].as_str().unwrap_or_default().to_string()
}

#[test]
fn sprite_ids_round_trip() {
    for s in Sprite::ALL {
        assert_eq!(Sprite::from_region(s.region()), Some(s));
    }
    assert_eq!(Sprite::ALL.len(), 2441);
}

#[test]
fn every_ruleset_object_resolves_to_a_packed_icon() {
    type Resolver = fn(&serde_json::Map<String, serde_json::Value>) -> Option<Sprite>;

    let cases: Vec<(&str, Resolver)> = vec![
        ("Buildings.json",       |m| icons::building_icon(&name_of(m))),
        ("Units.json",           |m| icons::unit_icon(&name_of(m))),
        ("Techs.json",           |m| icons::tech_icon(&name_of(m))),
        ("TileResources.json",   |m| icons::resource_icon(&name_of(m))),
        ("TileImprovements.json",|m| icons::improvement_icon(&name_of(m))),
        ("UnitPromotions.json",  |m| icons::promotion_icon_or_fallback(&name_of(m)).map(|(s, _)| s)),
        ("Policies.json",        |m| icons::policy_icon(&name_of(m))),
        ("Nations.json",         |m| icons::nation_icon(&name_of(m),
                                        m.get("cityStateType").and_then(|v| v.as_str()))),
    ];

    let (mut total, mut missing) = (0usize, Vec::new());
    for (file, resolve) in cases {
        let objs = objects(file);
        // Techs.json nests eras that also carry `name`; those have no icon of their own.
        let objs: Vec<_> = objs.into_iter()
            .filter(|m| file != "Techs.json" || m.contains_key("row") || m.contains_key("prerequisites"))
            .collect();
        let miss: Vec<_> = objs.iter().filter(|m| resolve(m).is_none()).map(name_of).collect();
        println!("{:<24} {:>4} objects, {:>3} unresolved", file, objs.len(), miss.len());
        for m in miss.iter().take(8) { println!("        {m}") }
        total += objs.len();
        missing.extend(miss);
    }
    let on_fallback: Vec<_> = objects("UnitPromotions.json").iter()
        .filter(|m| icons::promotion_icon(&name_of(m)).is_none())
        .map(name_of).collect();
    println!("\nTOTAL {}/{} resolved  ({} of them via the group fallback: {:?})",
             total - missing.len(), total, on_fallback.len(), on_fallback);
    assert!(total > 600, "expected the full ruleset, got {total}");
    assert!(missing.is_empty(), "{} objects have no packed icon: {:?}", missing.len(), missing);
}
