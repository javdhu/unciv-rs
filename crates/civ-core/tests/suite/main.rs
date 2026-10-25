// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! civ-core's integration tests, one binary: each file is compiled and linked once, not once per file.
//! The release budget stays a binary of its own (tests/budget.rs), since other tests running beside it would
//! skew its timings.

#[path = "../common/mod.rs"]
mod common;
mod action_turns;
mod administration;
mod ai_tactics;
mod air_rules;
mod asset_coverage;
mod base_faiths;
mod citizen_focus;
mod city_economy;
mod city_state_rules;
mod defeat_lifecycle;
mod defensive_pacts;
mod differential;
mod endgame;
mod exploration_turns;
mod faiths;
mod goals;
mod governors;
mod growth_and_revolts;
mod luminaries;
mod military_rules;
mod movement_rules;
mod nation_fields;
mod nuclear_rules;
mod peace_treaties;
mod placement;
mod production_progress;
mod religion_persistence;
mod religion_turns;
mod research_agreements;
mod rules_effects;
mod ruleset_coverage;
mod ruleset_load;
mod tactics;
mod tile_filters;
mod victories;
mod world_integration;

/// A file added to this directory runs only once it is declared above.
#[test]
fn every_file_here_is_declared() {
    let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/suite");
    let declared = include_str!("main.rs");
    for entry in std::fs::read_dir(here).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        if let Some(module) = name.strip_suffix(".rs").filter(|m| *m != "main") {
            assert!(declared.contains(&format!("\nmod {module};\n")), "tests/suite/{name} is not declared in tests/suite/main.rs");
        }
    }
}
