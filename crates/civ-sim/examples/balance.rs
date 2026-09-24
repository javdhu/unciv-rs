//! Headless all-AI self-play for balance work (ENGINE-BRIEF B1).
//!
//! Plays whole games with every seat on the AI, through the same `finish_round`
//! the session uses, and reports what a designer tunes against: game length,
//! how and when games end, who wins, how fast eras arrive, wonders, expansion
//! and how often a game ends early by capture.
//!
//! ```sh
//! cargo run --release -p civ-sim --example balance -- \
//!     --speed Standard --map small --civs 4 --city-states 2 \
//!     --seeds 1..=200 --threads 4 --out balance.json
//! ```
//!
//! Options (defaults in brackets):
//!   --ruleset NAME     base | blitz [base]
//!   --speed NAME       game speed [the ruleset's: Standard, or Blitz]
//!   --map SIZE         tiny | small | medium | large | huge, or WxH [small]
//!   --civs N           major civilizations [4]
//!   --city-states N    [2]
//!   --max-turns N      turn limit; the speed's length when absent
//!   --seeds A..B       seed range, `A..B` or `A..=B` [1..=20]
//!   --threads N        worker threads [available cores]
//!   --slice NAME       play one Blitz slice (Cradles, Crossroads, Engines)
//!   --overlay DIR      also layer DIR's *.json over the ruleset, to try
//!                      changes before bundling them
//!   --out PATH         write every game as JSON
//!   --quiet            no per-game lines
//!   --ab               seat the old AI (docs/AI.md) against the new: in each
//!                      game half the seats play the old one, alternating
//!                      with the seed, and the summary compares the halves
//!   --legacy           every seat plays the old AI, for timing and baselines
//!   --trace N          every N turns, print each major civ's goal, cities,
//!                      strength and wars (use with one seed)
//!
//! Every game is deterministic in its seed, so any row can be replayed alone
//! with `--seeds S..=S`. A game that panics is reported with its seed and does
//! not stop the run; the process exits non-zero if any did.

use civ_core::ruleset::Ruleset;
use civ_core::world::construction::Construction;
use civ_core::world::policies::era_number;
use civ_core::world::{finish_round, CityFocus, CivId, TurnEvent, TurnModifiers, World};
use civ_sim::Settings;
use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Instant;

/// Turns at which each civ's city count is sampled.
const SAMPLES: [u32; 3] = [25, 50, 75];

#[derive(Clone)]
struct Options {
    ruleset: String,
    speed: Option<String>,
    width: u16,
    height: u16,
    civs: u8,
    city_states: u8,
    max_turns: Option<u32>,
    seeds: Vec<u64>,
    threads: usize,
    out: Option<String>,
    quiet: bool,
    overlay: Option<String>,
    slice: Option<String>,
    ab: bool,
    legacy: bool,
    trace: Option<u32>,
}

fn usage(error: &str) -> ! {
    eprintln!("balance: {error}\n\nsee the header of crates/civ-sim/examples/balance.rs for options");
    std::process::exit(2)
}

fn map_size(name: &str) -> (u16, u16) {
    // MapSize.kt's rectangular sizes.
    match name {
        "tiny" => (23, 15),
        "small" => (33, 21),
        "medium" => (44, 29),
        "large" => (66, 43),
        "huge" => (87, 57),
        _ => {
            let (w, h) = name.split_once('x').unwrap_or_else(|| usage("--map is a size name or WxH"));
            (w.parse().unwrap_or_else(|_| usage("bad map width")), h.parse().unwrap_or_else(|_| usage("bad map height")))
        }
    }
}

fn seeds(range: &str) -> Vec<u64> {
    let parse = |s: &str| s.trim().parse::<u64>().unwrap_or_else(|_| usage("--seeds is A..B or A..=B"));
    if let Some((a, b)) = range.split_once("..=") {
        (parse(a)..=parse(b)).collect()
    } else if let Some((a, b)) = range.split_once("..") {
        (parse(a)..parse(b)).collect()
    } else {
        vec![parse(range)]
    }
}

fn options() -> Options {
    let mut o = Options {
        ruleset: civ_sim::content::DEFAULT_RULESET.into(),
        speed: None,
        width: 33,
        height: 21,
        civs: 4,
        city_states: 2,
        max_turns: None,
        seeds: seeds("1..=20"),
        threads: std::thread::available_parallelism().map_or(1, |n| n.get()),
        out: None,
        quiet: false,
        overlay: None,
        slice: None,
        ab: false,
        legacy: false,
        trace: None,
    };
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        if flag == "--quiet" {
            o.quiet = true;
            continue;
        }
        if flag == "--ab" {
            o.ab = true;
            continue;
        }
        if flag == "--legacy" {
            o.legacy = true;
            continue;
        }
        let value = args.next().unwrap_or_else(|| usage(&format!("{flag} needs a value")));
        let number = |v: &str| v.parse::<u32>().unwrap_or_else(|_| usage(&format!("{flag} needs a number")));
        match flag.as_str() {
            "--ruleset" => o.ruleset = value,
            "--speed" => o.speed = Some(value),
            "--map" => (o.width, o.height) = map_size(&value),
            "--civs" => o.civs = number(&value) as u8,
            "--city-states" => o.city_states = number(&value) as u8,
            "--max-turns" => o.max_turns = Some(number(&value)),
            "--seeds" => o.seeds = seeds(&value),
            "--threads" => o.threads = number(&value).max(1) as usize,
            "--out" => o.out = Some(value),
            "--overlay" => o.overlay = Some(value),
            "--slice" => o.slice = Some(value),
            "--trace" => o.trace = Some(number(&value).max(1)),
            _ => usage(&format!("unknown option {flag}")),
        }
    }
    if !civ_sim::content::RULESETS.contains(&o.ruleset.as_str()) {
        usage(&format!("unknown ruleset {}; bundled: {:?}", o.ruleset, civ_sim::content::RULESETS));
    }
    o
}

#[derive(Serialize, Default, Clone)]
#[serde(rename_all = "camelCase")]
struct CivRecord {
    nation: String,
    /// Cities owned at turns 25, 50 and 75 (absent if the game ended first).
    cities_at: BTreeMap<u32, usize>,
    cities_final: usize,
    cities_founded: u32,
    cities_captured: u32,
    wonders: u32,
    wars_declared: u32,
    techs: usize,
    era: i32,
    defeated_turn: Option<u32>,
    /// Population of all its cities at the end.
    population: u32,
    /// Policies adopted, branch openers included.
    policies: usize,
    /// Luminaries born.
    luminaries: u32,
    /// Played the old AI (`--ab`).
    legacy: bool,
    /// Final score, the Chronicle's measure.
    score: i64,
    /// Original major capitals held at the end (Conquest).
    capitals: usize,
    /// Highest share of world trade held in any turn, in percent (Reserve Currency).
    trade_peak: u16,
    /// Rivals this civ was influential over at the end (Lingua Franca).
    influential_over: usize,
    /// The AI's goal (docs/AI.md) at turns 25, 50 and 75.
    goals_at: BTreeMap<u32, String>,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct WonderRecord {
    name: String,
    nation: String,
    turn: u32,
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct GameRecord {
    seed: u64,
    turns: u32,
    victory: Option<String>,
    victory_turn: Option<u32>,
    winner: Option<String>,
    /// The winner's seat, which `civs` is indexed by.
    winner_seat: Option<u8>,
    /// Won by conquest before the turn limit.
    ended_by_capture: bool,
    /// The first turn any major civ entered each era, by era name.
    era_first_reached: BTreeMap<String, u32>,
    wonders: Vec<WonderRecord>,
    civs: Vec<CivRecord>,
    engine_ms: f64,
    /// Engine time for the first 60 turns, the length PLATFORM.md §5 estimated.
    engine_ms_60: Option<f64>,
    /// Lingua Franca at the end: the best civ's influence over its least
    /// influenced rival, in tenths of a percent of the way (1000 wins).
    lingua_best: i64,
    /// Reserve Currency: the highest share of world trade any civ held on
    /// any turn, in tenths of a percent.
    trade_best: u16,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Report<'a> {
    ruleset: &'a str,
    speed: String,
    width: u16,
    height: u16,
    civs: u8,
    city_states: u8,
    max_turns: u32,
    games: &'a [GameRecord],
    panics: &'a [(u64, String)],
}

fn is_world_wonder(r: &Ruleset, what: Construction) -> Option<String> {
    let Construction::Building(id) = what else { return None };
    let b = r.buildings.get(id)?;
    (b.is_wonder == Some(true)).then(|| b.name.clone())
}

fn settings(o: &Options, seed: u64) -> Settings {
    Settings {
        seed,
        width: o.width,
        height: o.height,
        majors: o.civs,
        city_states: o.city_states,
        max_turns: o.max_turns,
        speed: o.speed.clone(),
        ruleset: Some(o.ruleset.clone()),
        slice: o.slice.clone(),
        ..Settings::default()
    }
}

/// The ruleset to play: the bundled one, plus any `--overlay` files.
fn ruleset(o: &Options) -> Result<Ruleset, String> {
    let Some(dir) = &o.overlay else { return civ_sim::content::load_ruleset(&o.ruleset) };
    let mut sources: Vec<(String, String)> = civ_sim::content::SOURCES
        .iter()
        .map(|(n, d)| (n.to_string(), d.to_string()))
        .collect();
    if o.ruleset == civ_sim::content::BLITZ {
        sources.extend(civ_sim::content::BLITZ_SOURCES.iter().map(|(n, d)| (n.to_string(), d.to_string())));
    }
    let mut files: Vec<_> = std::fs::read_dir(dir).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).collect();
    files.sort();
    for p in files.into_iter().filter(|p| p.extension().is_some_and(|x| x == "json")) {
        let name = p.file_name().unwrap_or_default().to_string_lossy().into_owned();
        sources.push((name, std::fs::read_to_string(&p).map_err(|e| e.to_string())?));
    }
    Ruleset::from_sources(&sources).map_err(|e| e.to_string())
}

fn play(o: &Options, seed: u64) -> Result<GameRecord, String> {
    let (mut world, rules, _) = civ_sim::new_game_in(&settings(o, seed), &[], ruleset(o)?)?;
    let r = &rules;
    let majors: Vec<CivId> = (0..o.civs).map(CivId).collect();
    // Alternate which half is old with the seed, so neither side always gets
    // the same starts.
    let legacy: Vec<CivId> =
        majors.iter().copied().filter(|c| o.legacy || (o.ab && (c.0 as u64 + seed) % 2 == 1)).collect();
    for c in &legacy {
        if let Some(civ) = world.civ_mut(*c) {
            civ.legacy_ai = true;
        }
    }
    let mut civs: Vec<CivRecord> = majors
        .iter()
        .map(|&c| CivRecord { nation: nation(&world, r, c), legacy: legacy.contains(&c), ..Default::default() })
        .collect();
    let mut eras = BTreeMap::new();
    let mut wonders = Vec::new();
    let era_name = |n: i32| r.eras.values().find(|e| e.era_number == n).map_or(n.to_string(), |e| e.name.clone());
    let limit = world.endgame.max_turns;
    let started = Instant::now();
    let mut engine_ms_60 = None;
    while world.winner.is_none() && world.turn <= limit + 1 {
        let mods = TurnModifiers::for_world(&world, r);
        let events = finish_round(&mut world, r, CityFocus::default(), mods);
        for e in &events {
            match e {
                TurnEvent::CityCompleted { city, what } => {
                    if let (Some(name), Some(c)) = (is_world_wonder(r, *what), world.cities.get(*city)) {
                        if let Some(rec) = civs.get_mut(c.owner.0 as usize) {
                            rec.wonders += 1;
                        }
                        wonders.push(WonderRecord { name, nation: nation(&world, r, c.owner), turn: world.turn });
                    }
                }
                TurnEvent::CityFounded { by, .. } => {
                    if let Some(rec) = civs.get_mut(by.0 as usize) {
                        rec.cities_founded += 1;
                    }
                }
                TurnEvent::WarDeclared { aggressor, .. } => {
                    if let Some(rec) = civs.get_mut(aggressor.0 as usize) {
                        rec.wars_declared += 1;
                    }
                }
                TurnEvent::CityCaptured { by, .. } => {
                    if let Some(rec) = civs.get_mut(by.0 as usize) {
                        rec.cities_captured += 1;
                    }
                }
                TurnEvent::GreatPersonBorn { civ, .. } => {
                    if let Some(rec) = civs.get_mut(civ.0 as usize) {
                        rec.luminaries += 1;
                    }
                }
                TurnEvent::CivDefeated { civ } => {
                    if let Some(rec) = civs.get_mut(civ.0 as usize) {
                        rec.defeated_turn.get_or_insert(world.turn);
                    }
                }
                _ => {}
            }
        }
        for (civ, share) in civ_core::world::influence::trade_shares(&world, r) {
            if let Some(rec) = civs.get_mut(civ.0 as usize) {
                rec.trade_peak = rec.trade_peak.max(share);
            }
        }
        if let Some(every) = o.trace {
            if world.turn % every == 0 {
                trace(&world, r, &majors, seed);
            }
        }
        if world.turn == 60 {
            engine_ms_60 = Some(started.elapsed().as_secs_f64() * 1000.0);
        }
        for &c in &majors {
            let era = era_number(&world, r, c);
            eras.entry(era_name(era)).or_insert(world.turn);
            if SAMPLES.contains(&world.turn) {
                civs[c.0 as usize].cities_at.insert(world.turn, cities(&world, c));
                let goal = civ_core::world::goals::goal(&world, r, c);
                civs[c.0 as usize].goals_at.insert(world.turn, format!("{goal:?}"));
            }
        }
    }
    let engine_ms = started.elapsed().as_secs_f64() * 1000.0;
    for &c in &majors {
        let rec = &mut civs[c.0 as usize];
        rec.cities_final = cities(&world, c);
        let civ = world.civ(c).expect("major civ");
        rec.techs = civ.techs.len();
        rec.era = era_number(&world, r, c);
        rec.policies = civ.adopted_policies.len();
        rec.population = civ.cities.iter().filter_map(|id| world.cities.get(*id)).map(|city| city.population).sum();
        rec.score = civ_core::world::victory::score(&world, r, c).round();
        rec.capitals = world.cities.iter().filter(|(_, city)| city.original_capital && city.owner == c).count();
        rec.influential_over = majors
            .iter()
            .filter(|&&o| o != c && civ_core::world::influence::is_influential(&world, r, c, o))
            .count();
    }
    let victory = world.winner.map(|(_, v)| r.victories[v].name.clone());
    let majors: Vec<_> = world.civs.iter().filter(|c| c.is_major() && c.is_alive()).map(|c| c.id).collect();
    let progress = |a, b| civ_core::world::influence::influence_progress(&world, r, a, b);
    let lingua_best = majors
        .iter()
        .map(|a| majors.iter().filter(|b| *b != a).map(|b| progress(*a, *b)).min().unwrap_or(0))
        .max()
        .unwrap_or(0);
    let trade_best = world.influence.trade_shares.values().flatten().copied().max().unwrap_or(0);
    Ok(GameRecord {
        lingua_best,
        trade_best,
        seed,
        turns: world.turn,
        ended_by_capture: victory.as_deref() == Some("Conquest") && world.victory_turn.is_some_and(|t| t < limit),
        victory,
        victory_turn: world.victory_turn,
        winner: world.winner.map(|(c, _)| nation(&world, r, c)),
        winner_seat: world.winner.map(|(c, _)| c.0),
        era_first_reached: eras,
        wonders,
        civs,
        engine_ms,
        engine_ms_60,
    })
}

fn speed_name(o: &Options, rules: &Ruleset) -> String {
    let id = settings(o, 0).speed_in(rules).unwrap_or_else(|e| usage(&e));
    rules.speeds[id].name.clone()
}

fn nation(world: &World, r: &Ruleset, civ: CivId) -> String {
    world.civ(civ).map_or_else(String::new, |c| r.nations[c.nation].name.clone())
}

fn cities(world: &World, civ: CivId) -> usize {
    world.civ(civ).map_or(0, |c| c.cities.len())
}

fn median(mut v: Vec<f64>) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len().max(1) as f64
}

fn summary(o: &Options, games: &[GameRecord], panics: &[(u64, String)], rules: &Ruleset, wall: f64) {
    let n = games.len();
    let pct = |k: usize| 100.0 * k as f64 / n.max(1) as f64;
    println!();
    println!(
        "{} games ({} panicked) · {} · {} · {}x{} · {} civs + {} city-states",
        n,
        panics.len(),
        o.ruleset,
        speed_name(o, rules),
        o.width,
        o.height,
        o.civs,
        o.city_states
    );
    if n == 0 {
        return;
    }
    let turns: Vec<f64> = games.iter().map(|g| g.turns as f64).collect();
    let ms: Vec<f64> = games.iter().map(|g| g.engine_ms).collect();
    let min = turns.iter().cloned().fold(f64::INFINITY, f64::min);
    let max = turns.iter().cloned().fold(0.0, f64::max);
    println!("\nGame length: median {:.0} turns, mean {:.1}, min {min:.0}, max {max:.0}", median(turns.clone()), mean(&turns));
    println!(
        "Engine time: mean {:.0} ms/game, median {:.0} ms/game, {:.2} ms/turn; wall {:.1} s on {} threads",
        mean(&ms),
        median(ms.clone()),
        ms.iter().sum::<f64>() / turns.iter().sum::<f64>(),
        wall,
        o.threads
    );

    let first_60: Vec<f64> = games.iter().filter_map(|g| g.engine_ms_60).collect();
    if !first_60.is_empty() {
        println!("First 60 turns: mean {:.0} ms, median {:.0} ms", mean(&first_60), median(first_60.clone()));
    }

    println!("\n{:<22} {:>6} {:>7} {:>12}", "Victory", "games", "share", "median turn");
    let mut by_victory: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for g in games {
        by_victory.entry(g.victory.clone().unwrap_or_else(|| "(none)".into())).or_default().push(g.turns as f64);
    }
    for (v, t) in &by_victory {
        println!("{:<22} {:>6} {:>6.1}% {:>12.0}", v, t.len(), pct(t.len()), median(t.clone()));
    }
    let captured = games.iter().filter(|g| g.ended_by_capture).count();
    println!("Ended early by capture: {captured} ({:.1}%)", pct(captured));
    let lingua: Vec<f64> = games.iter().map(|g| g.lingua_best as f64 / 10.0).collect();
    let trade: Vec<f64> = games.iter().map(|g| f64::from(g.trade_best) / 10.0).collect();
    println!(
        "Lingua Franca at the end: best civ {:.0}% of the way over its weakest rival (median; max {:.0}%)",
        median(lingua.clone()),
        lingua.iter().copied().fold(0.0, f64::max)
    );
    println!("Highest share of world trade held: median {:.0}%, max {:.0}%", median(trade.clone()), trade.iter().copied().fold(0.0, f64::max));

    println!("\n{:<20} {:>6} {:>5} {:>8} {:>7} {:>8} {:>8} {:>8}", "Nation", "played", "wins", "win rate", "× fair", "wars/g", "wonders", "founded");
    let mut nations: BTreeMap<String, (usize, usize, u32, u32, u32)> = BTreeMap::new();
    for g in games {
        for c in &g.civs {
            let e = nations.entry(c.nation.clone()).or_default();
            e.0 += 1;
            e.1 += usize::from(g.winner.as_ref() == Some(&c.nation));
            e.2 += c.wars_declared;
            e.3 += c.wonders;
            e.4 += c.cities_founded;
        }
    }
    let fair = 1.0 / f64::from(o.civs.max(1));
    for (name, (played, wins, wars, wonders, founded)) in &nations {
        let rate = *wins as f64 / *played as f64;
        let p = *played as f64;
        println!(
            "{:<20} {:>6} {:>5} {:>7.1}% {:>7.2} {:>8.2} {:>8.2} {:>8.2}",
            name,
            played,
            wins,
            100.0 * rate,
            rate / fair,
            *wars as f64 / p,
            *wonders as f64 / p,
            *founded as f64 / p
        );
    }

    println!("\n{:<20} {:>8} {:>12}", "Era (first civ)", "reached", "median turn");
    let mut eras: Vec<_> = rules.eras.values().map(|e| (e.era_number, e.name.clone())).collect();
    eras.sort();
    for (_, era) in eras {
        let t: Vec<f64> = games.iter().filter_map(|g| g.era_first_reached.get(&era).map(|&t| t as f64)).collect();
        if !t.is_empty() {
            println!("{:<20} {:>7.1}% {:>12.0}", era, pct(t.len()), median(t));
        }
    }

    // Growth, Luminaries and policies per surviving major civ at the end, the proportions Speeds.json tunes.
    let alive: Vec<&CivRecord> = games.iter().flat_map(|g| g.civs.iter()).filter(|c| c.defeated_turn.is_none()).collect();
    let per = |f: &dyn Fn(&CivRecord) -> f64| median(alive.iter().map(|c| f(c)).collect());
    println!(
        "\nPer civ at the end (median of {}): population {:.0}, per city {:.1}, policies {:.0}, Luminaries {:.0}, techs {:.0}",
        alive.len(),
        per(&|c| f64::from(c.population)),
        per(&|c| f64::from(c.population) / c.cities_final.max(1) as f64),
        per(&|c| c.policies as f64),
        per(&|c| f64::from(c.luminaries)),
        per(&|c| c.techs as f64)
    );

    let wonders: Vec<f64> = games.iter().map(|g| g.wonders.len() as f64).collect();
    println!("\nWorld wonders built: mean {:.1} per game, median {:.0}", mean(&wonders), median(wonders.clone()));
    let mut first: BTreeMap<&str, Vec<f64>> = BTreeMap::new();
    for g in games {
        for w in &g.wonders {
            first.entry(&w.name).or_default().push(w.turn as f64);
        }
    }
    let mut common: Vec<_> = first.iter().map(|(n, t)| (t.len(), *n, median(t.clone()))).collect();
    common.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
    for (count, name, turn) in common.iter().take(8) {
        println!("  {:<28} built in {:>5.1}% of games, median turn {:.0}", name, pct(*count), turn);
    }

    println!("\n{:<10} {:>12} {:>14} {:>10}", "Turn", "cities/civ", "median/civ", "games");
    for t in SAMPLES {
        let c: Vec<f64> = games.iter().flat_map(|g| g.civs.iter().filter_map(move |c| c.cities_at.get(&t).map(|&n| n as f64))).collect();
        let played = games.iter().filter(|g| g.civs.iter().any(|c| c.cities_at.contains_key(&t))).count();
        if !c.is_empty() {
            println!("{:<10} {:>12.2} {:>14.0} {:>10}", t, mean(&c), median(c.clone()), played);
        }
    }
    progress_summary(games);
    if o.ab {
        ab_summary(games);
    }
    for (seed, message) in panics {
        println!("\nPANIC seed {seed}: {message}");
    }
}

fn trace(world: &World, r: &Ruleset, majors: &[CivId], seed: u64) {
    for &c in majors {
        let Some(civ) = world.civ(c) else { continue };
        let wars: Vec<String> = majors
            .iter()
            .filter(|o| **o != c && world.diplomacy.at_war(c, **o))
            .map(|o| o.0.to_string())
            .collect();
        let objective = civ_core::world::tactics::choose_objective(world, r, c).and_then(|id| world.cities.get(id)).map(|city| {
            let nearest = civ
                .units
                .iter()
                .filter_map(|u| world.units.get(*u))
                .filter(|u| u.is_military(r))
                .map(|u| world.grid.distance(u.position, city.centre))
                .min();
            let near: Vec<_> = civ
                .units
                .iter()
                .filter_map(|u| world.units.get(*u))
                .filter(|u| u.is_military(r) && world.grid.distance(u.position, city.centre) <= 4)
                .collect();
            let names: Vec<String> =
                near.iter().map(|u| format!("{}@{}hp", r.units[u.base].name, u.health)).collect();
            let id = world.cities.iter().find(|(_, c)| c.centre == city.centre).map(|(id, _)| id).unwrap();
            if std::env::var("TRACE_UNITS").is_ok() {
                for u in civ.units.iter().filter_map(|u| world.units.get(*u)).filter(|u| u.is_military(r)) {
                    let home = world.cities.iter().any(|(_, x)| x.owner == c && x.centre == u.position);
                    println!(
                        "    {} {}hp d{} {}",
                        r.units[u.base].name,
                        u.health,
                        world.grid.distance(u.position, city.centre),
                        if home { "in city" } else { "" }
                    );
                }
            }
            format!(
                " -> {} of civ {} (nearest {:?}, hp {}, defence {}, near: {})",
                city.name,
                city.owner.0,
                nearest,
                city.health,
                civ_core::world::combat::city_defense_strength(world, r, id).round(),
                names.join(" ")
            )
        });
        println!(
            "seed {seed} t{:<3} civ {} {:<12} {:<9} cities {:>2} capitals {} units {:>2} strength {:>4} techs {:>3} war [{}]{}{}",
            world.turn,
            c.0,
            nation(world, r, c),
            format!("{:?}", civ_core::world::goals::goal(world, r, c)),
            cities(world, c),
            civ_core::world::goals::capitals_held(world, c),
            civ.units.len(),
            civ_core::world::ai::military_strength(world, r, c).round(),
            civ.techs.len(),
            wars.join(","),
            objective.unwrap_or_default(),
            if civ.legacy_ai { " (old)" } else { "" },
        );
    }
}

/// How close the leading civ of each game came to each victory.
fn progress_summary(games: &[GameRecord]) {
    let best = |f: &dyn Fn(&CivRecord) -> f64| -> Vec<f64> {
        games.iter().map(|g| g.civs.iter().map(f).fold(0.0, f64::max)).collect()
    };
    let capitals = best(&|c| c.capitals as f64);
    let trade = best(&|c| f64::from(c.trade_peak) / 10.0);
    let influence = best(&|c| c.influential_over as f64);
    let mut goals: BTreeMap<(u32, &str), usize> = BTreeMap::new();
    for c in games.iter().flat_map(|g| g.civs.iter()).filter(|c| !c.legacy) {
        for (t, g) in &c.goals_at {
            *goals.entry((*t, g.as_str())).or_default() += 1;
        }
    }
    if !goals.is_empty() {
        println!("\nGoals of the new AI's seats:");
        for t in SAMPLES {
            let row: Vec<String> =
                goals.iter().filter(|((at, _), _)| *at == t).map(|((_, g), n)| format!("{g} {n}")).collect();
            if !row.is_empty() {
                println!("  turn {t:<3} {}", row.join(", "));
            }
        }
    }
    println!("\nClosest approach per game (median, max):");
    println!("  capitals held by one civ   {:>5.1} {:>5.0}", median(capitals.clone()), capitals.iter().cloned().fold(0.0, f64::max));
    println!("  peak world trade share, %  {:>5.1} {:>5.0}", median(trade.clone()), trade.iter().cloned().fold(0.0, f64::max));
    println!("  rivals influenced          {:>5.1} {:>5.0}", median(influence.clone()), influence.iter().cloned().fold(0.0, f64::max));
}

/// New AI against old, from games where each played half the seats.
fn ab_summary(games: &[GameRecord]) {
    let side = |legacy: bool| {
        let seats: Vec<&CivRecord> = games.iter().flat_map(|g| g.civs.iter().filter(move |c| c.legacy == legacy)).collect();
        let wins = games
            .iter()
            .filter(|g| g.winner_seat.and_then(|s| g.civs.get(s as usize)).is_some_and(|c| c.legacy == legacy))
            .count();
        let share: Vec<f64> = games
            .iter()
            .filter_map(|g| {
                let total: i64 = g.civs.iter().map(|c| c.score).sum();
                let mine: i64 = g.civs.iter().filter(|c| c.legacy == legacy).map(|c| c.score).sum();
                (total > 0).then(|| 100.0 * mine as f64 / total as f64)
            })
            .collect();
        let per = |f: &dyn Fn(&CivRecord) -> f64| mean(&seats.iter().map(|c| f(c)).collect::<Vec<_>>());
        (
            wins,
            mean(&share),
            per(&|c| c.cities_final as f64),
            per(&|c| c.wonders as f64),
            per(&|c| c.cities_captured as f64),
            per(&|c| c.wars_declared as f64),
            per(&|c| c.defeated_turn.is_some() as u8 as f64),
        )
    };
    let decided = games.iter().filter(|g| g.winner_seat.is_some()).count();
    println!("\nNew AI vs old ({} games, {decided} with a winner)", games.len());
    println!(
        "{:<6} {:>6} {:>7} {:>11} {:>10} {:>9} {:>9} {:>6} {:>9}",
        "Side", "wins", "win %", "score share", "cities", "wonders", "captures", "wars", "defeated"
    );
    for (name, legacy) in [("new", false), ("old", true)] {
        let (wins, share, cities, wonders, captured, wars, defeated) = side(legacy);
        println!(
            "{:<6} {:>6} {:>6.1}% {:>10.1}% {:>10.2} {:>9.2} {:>9.2} {:>6.2} {:>8.0}%",
            name,
            wins,
            100.0 * wins as f64 / decided.max(1) as f64,
            share,
            cities,
            wonders,
            captured,
            wars,
            100.0 * defeated
        );
    }
}

fn main() {
    let o = options();
    let rules = ruleset(&o).unwrap_or_else(|e| usage(&e));
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<GameRecord>> = Mutex::new(Vec::new());
    let panics: Mutex<Vec<(u64, String)>> = Mutex::new(Vec::new());
    // Panics are caught and reported per seed; keep the default hook quiet.
    std::panic::set_hook(Box::new(|_| {}));
    let wall = Instant::now();
    std::thread::scope(|scope| {
        for _ in 0..o.threads.min(o.seeds.len()).max(1) {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(&seed) = o.seeds.get(i) else { break };
                match std::panic::catch_unwind(|| play(&o, seed)) {
                    Ok(Ok(game)) => {
                        if !o.quiet {
                            eprintln!(
                                "seed {:>5}: {:>3} turns, {:<12} {:<16} {:>6.0} ms",
                                seed,
                                game.turns,
                                game.victory.as_deref().unwrap_or("-"),
                                game.winner.as_deref().unwrap_or("-"),
                                game.engine_ms
                            );
                        }
                        results.lock().unwrap().push(game);
                    }
                    Ok(Err(e)) => usage(&format!("seed {seed}: {e}")),
                    Err(payload) => {
                        let message = payload
                            .downcast_ref::<String>()
                            .cloned()
                            .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                            .unwrap_or_else(|| "non-string panic".into());
                        eprintln!("seed {seed:>5}: PANIC {message}");
                        panics.lock().unwrap().push((seed, message));
                    }
                }
            });
        }
    });
    let wall = wall.elapsed().as_secs_f64();
    let mut games = results.into_inner().unwrap();
    games.sort_by_key(|g| g.seed);
    let mut panics = panics.into_inner().unwrap();
    panics.sort();
    summary(&o, &games, &panics, &rules, wall);
    if let Some(path) = &o.out {
        let report = Report {
            ruleset: &o.ruleset,
            speed: speed_name(&o, &rules),
            width: o.width,
            height: o.height,
            civs: o.civs,
            city_states: o.city_states,
            max_turns: settings(&o, 0).turn_limit(&rules).unwrap_or_else(|e| usage(&e)),
            games: &games,
            panics: &panics,
        };
        std::fs::write(path, serde_json::to_vec_pretty(&report).expect("serializable")).expect("writable --out");
        eprintln!("wrote {path}");
    }
    if !panics.is_empty() {
        std::process::exit(1);
    }
}
