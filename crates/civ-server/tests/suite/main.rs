// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! civ-server's integration tests, one binary: each file is compiled and linked once, not once per file.

mod network;
mod shutdown;
mod static_files;

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
