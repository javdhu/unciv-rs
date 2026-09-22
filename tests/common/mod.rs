//! Shared test helpers.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// Unciv ships libGDX-flavoured JSON: `//` and `/* */` comments, trailing commas.
fn relax(src: &str) -> String {
    let (mut out, mut in_str, mut esc) = (String::with_capacity(src.len()), false, false);
    let b: Vec<char> = src.chars().collect();
    let mut i = 0;
    while i < b.len() {
        let c = b[i];
        if in_str {
            out.push(c);
            if esc { esc = false } else if c == '\\' { esc = true } else if c == '"' { in_str = false }
            i += 1;
            continue;
        }
        match (c, b.get(i + 1)) {
            ('"', _) => { in_str = true; out.push(c); i += 1 }
            ('/', Some('/')) => { while i < b.len() && b[i] != '\n' { i += 1 } }
            ('/', Some('*')) => {
                i += 2;
                while i + 1 < b.len() && !(b[i] == '*' && b[i + 1] == '/') { i += 1 }
                i += 2;
            }
            _ => { out.push(c); i += 1 }
        }
    }
    // trailing commas before } or ]
    let mut cleaned = String::with_capacity(out.len());
    let o: Vec<char> = out.chars().collect();
    for (i, &c) in o.iter().enumerate() {
        if c == ',' {
            if let Some(&n) = o[i + 1..].iter().find(|c| !c.is_whitespace()) {
                if n == '}' || n == ']' { continue }
            }
        }
        cleaned.push(c);
    }
    cleaned
}


/// Parse one of Unciv's lenient ruleset files.
pub fn read_json(path: &Path) -> serde_json::Value {
    let raw = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&relax(raw.trim_start_matches('\u{feff}')))
        .unwrap_or_else(|e| panic!("{} did not parse: {e}", path.display()))
}

/// Every ruleset json except translations and the tutorial prose.
pub fn ruleset_files(root: &str) -> Vec<PathBuf> {
    fn walk(dir: &Path, acc: &mut Vec<PathBuf>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            if p.is_dir() {
                if p.file_name().is_some_and(|n| n == "translations") { continue }
                walk(&p, acc);
            } else if p.extension().is_some_and(|x| x == "json")
                && !p.file_name().is_some_and(|n| n == "Tutorials.json") {
                acc.push(p);
            }
        }
    }
    let mut v = Vec::new();
    walk(Path::new(root), &mut v);
    v.sort();
    v
}
