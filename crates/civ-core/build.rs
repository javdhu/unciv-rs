// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Detect the Unciv fixture rulesets.
//!
//! `rules/gods-and-kings` and `rules/vanilla` are not part of this repository:
//! `tools/fetch_unciv_rulesets.sh` downloads them from upstream Unciv. The
//! library builds without them; `fingerprint::embedded_sources` embeds the
//! Gods & Kings files only when they are present (`cfg(unciv_rulesets)`), and
//! panics with instructions otherwise. The test suite needs them.

use std::path::Path;

const EMBEDDED: &[&str] = &[
    "Buildings.json",
    "Units.json",
    "UnitTypes.json",
    "Terrains.json",
    "TileResources.json",
    "TileImprovements.json",
    "Techs.json",
    "Policies.json",
    "Eras.json",
    "Speeds.json",
    "Nations.json",
    "Ruins.json",
    "Beliefs.json",
    "Religions.json",
    "UnitPromotions.json",
];

fn main() {
    println!("cargo::rustc-check-cfg=cfg(unciv_rulesets)");
    println!("cargo::rerun-if-changed=rules/gods-and-kings");
    let dir = Path::new("rules/gods-and-kings");
    for f in EMBEDDED {
        println!("cargo::rerun-if-changed=rules/gods-and-kings/{f}");
    }
    if EMBEDDED.iter().all(|f| dir.join(f).is_file()) {
        println!("cargo::rustc-cfg=unciv_rulesets");
    } else {
        println!(
            "cargo::warning=rules/gods-and-kings is missing: run crates/civ-core/tools/fetch_unciv_rulesets.sh before running tests"
        );
    }
}
