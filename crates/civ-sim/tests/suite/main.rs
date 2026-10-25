// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! civ-sim's integration tests, one binary: each file is compiled and linked once, not once per file.

mod base_rules;
mod battle_owners;
mod compiled_filters;
mod great_person_progress;
mod leaders;
mod normal_match;
mod player_events;
mod player_knowledge;
mod protocol_rebuild;
mod query_options;
mod renamed_content;
mod sight_audit;
mod slices;
mod smoke;
mod tradable;
mod watched_match;
mod yield_breakdown;

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
