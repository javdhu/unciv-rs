// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A save as JSON, and back again after editing it:
//! `cargo run --release -p civ-sim --example save_dump -- SAVE > save.json`, then
//! `... --example save_dump -- --pack save.json > SAVE`.
use civ_sim::session::Session;
use std::io::Write;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let read = |path: &str| std::fs::read(path).unwrap_or_else(|e| panic!("{path}: {e}"));
    let out = match args.as_slice() {
        [flag, path] if flag == "--pack" => {
            let value = serde_json::from_slice(&read(path)).expect("a save's JSON");
            Session::undump(&value).unwrap_or_else(|e| panic!("{e}"))
        }
        [path] => {
            let value = Session::dump(&read(path)).unwrap_or_else(|e| panic!("{e}"));
            let mut text = serde_json::to_vec_pretty(&value).unwrap();
            text.push(b'\n');
            text
        }
        _ => panic!("usage: save_dump SAVE, or save_dump --pack JSON"),
    };
    std::io::stdout().write_all(&out).unwrap();
}
