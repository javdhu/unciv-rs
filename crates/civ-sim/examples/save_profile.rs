use civ_sim::session::Session;
use civ_sim::{content, Settings};
use std::hint::black_box;
use std::time::Instant;

fn measure<T>(name: &str, count: usize, mut action: impl FnMut() -> T) {
    let start = Instant::now();
    for _ in 0..count {
        black_box(action());
    }
    println!(
        "{name}: {:.3} ms",
        start.elapsed().as_secs_f64() * 1000.0 / count as f64
    );
}

fn main() {
    let mut session = Session::new(Settings::default(), "profile".into(), vec![0]).unwrap();
    for turn in 0..12 {
        let start = Instant::now();
        session = session.stage_timeout(session.turn()).unwrap().unwrap().next;
        println!(
            "round {}: {:.1} ms",
            turn + 1,
            start.elapsed().as_secs_f64() * 1000.0
        );
    }
    let world = session.world();
    let rules = session.rules();
    println!("save bytes: {}", session.save().unwrap().len());
    measure("validate", 20, || {
        black_box(civ_core::save::validate(world, rules).unwrap())
    });
    measure("ruleset identity", 20, || {
        black_box(civ_core::save::ruleset_identity(rules))
    });
    measure("world to value", 20, || {
        black_box(serde_json::to_value(world).unwrap())
    });
    let value = serde_json::to_value(world).unwrap();
    measure("value to vec", 20, || {
        black_box(serde_json::to_vec(&value).unwrap())
    });
    measure("core encode", 20, || {
        black_box(civ_core::save::encode(world, rules).unwrap())
    });
    let identity = civ_core::save::RulesetIdentity::new(rules);
    measure("core encode (cached identity)", 20, || {
        black_box(civ_core::save::encode_with_identity(world, rules, &identity).unwrap())
    });
    measure("content hash", 20, || black_box(content::hash()));
    measure("session save", 20, || black_box(session.save().unwrap()));
}
