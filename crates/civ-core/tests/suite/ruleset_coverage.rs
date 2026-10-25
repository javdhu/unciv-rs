// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The gate that makes the port mechanical rather than hopeful:
//! every unique string in the shipped rulesets must resolve against the
//! generated algebra. A regression here means a rule silently stopped existing.

use civ_core::{Unique, UniqueKind};
use std::collections::BTreeMap;
use crate::common;
use common::{read_json, ruleset_files};

const JSONS: &str = "rules";
const UNIQUE_KEYS: [&str; 3] = ["uniques", "conditionals", "startingUniques"];

fn collect(v: &serde_json::Value, out: &mut Vec<String>) {
    match v {
        serde_json::Value::Object(m) => {
            for (k, val) in m {
                if UNIQUE_KEYS.contains(&k.as_str()) {
                    if let Some(arr) = val.as_array() {
                        out.extend(arr.iter().filter_map(|s| s.as_str()).map(String::from));
                    }
                }
                collect(val, out);
            }
        }
        serde_json::Value::Array(a) => a.iter().for_each(|x| collect(x, out)),
        _ => {}
    }
}

#[test]
fn shipped_rulesets_close_over_the_algebra() {
    let files = ruleset_files(JSONS);
    assert!(files.len() > 40, "expected the full ruleset tree, found {}", files.len());

    let (mut total, mut known, mut cond_total, mut cond_known) = (0usize, 0usize, 0usize, 0usize);
    let mut tags: BTreeMap<String, usize> = BTreeMap::new();

    for f in &files {
        let v = read_json(f);
        let mut strings = Vec::new();
        collect(&v, &mut strings);

        for s in strings {
            let u = Unique::parse(&s);
            total += 1;
            match u.kind {
                UniqueKind::Known(_) => known += 1,
                UniqueKind::Tag => *tags.entry(u.text.clone()).or_default() += 1,
            }
            for c in &u.conditionals {
                cond_total += 1;
                if matches!(c.kind, UniqueKind::Known(_)) {
                    cond_known += 1
                }
            }
        }
    }

    println!("files            {}", files.len());
    println!("uniques          {known}/{total}  ({:.2}%)", known as f64 / total as f64 * 100.0);
    println!("conditionals     {cond_known}/{cond_total}  ({:.2}%)", cond_known as f64 / cond_total as f64 * 100.0);
    println!("unresolved tags  {:?}", tags);

    assert_eq!(cond_known, cond_total, "every conditional must be a declared UniqueType");
    // The only strings allowed to fall through are deliberate filter flags.
    let allowed = ["Aircraft"];
    let unexpected: Vec<_> = tags.keys().filter(|t| !allowed.contains(&t.as_str())).collect();
    assert!(unexpected.is_empty(), "undeclared uniques appeared: {unexpected:?}");
    assert!(known as f64 / total as f64 > 0.995);
}
