//! The frame-time harness (S0, docs/CLIENT-BRIEF.md): a record of frame times, round times and link health while
//! the host thread closes a watched match's rounds, reported every five seconds (stdout natively,
//! `globalThis.civReport` on the page).
//! F3 shows the readout. The record serves the budgets in docs/VISUAL-SPEC.md.

use crate::client::{LinkState, ViewChanged};
use crate::view_state::ViewState;
use bevy::prelude::*;

/// Seconds per reported window, and windows per summary.
const WINDOW: f64 = 5.0;
const SUMMARY_WINDOWS: u32 = 12;

pub fn plugin(app: &mut App) {
    app.init_resource::<Record>()
        .add_systems(Startup, setup)
        .add_systems(Update, (count, toggle, measure).chain());
}

#[derive(Component)]
struct Hud;

#[derive(Default)]
struct Window {
    frames: Vec<f32>,
    busy_frames: Vec<f32>,
    rounds: Vec<f32>,
}

#[derive(Resource, Default)]
struct Record {
    window: Window,
    total: Window,
    window_start: f64,
    windows: u32,
    turn: u32,
    /// Wall time between applied rounds, as the main thread sees them.
    last_round: Option<f64>,
    changed_keys: usize,
    resyncs: u32,
    stale: u32,
    stopped: Option<String>,
    chunk_ms: Vec<f32>,
}

/// The readout, hidden until F3.
fn setup(mut commands: Commands) {
    commands.spawn((
        Hud,
        Text::new(""),
        TextFont { font_size: FontSize::Px(13.0), ..default() },
        Node { position_type: PositionType::Absolute, top: Val::Px(10.0), right: Val::Px(12.0), ..default() },
        Visibility::Hidden,
    ));
}

fn toggle(keys: Res<ButtonInput<KeyCode>>, mut hud: Query<&mut Visibility, With<Hud>>) {
    if keys.just_pressed(KeyCode::F3) {
        for mut v in &mut hud {
            *v = if *v == Visibility::Hidden { Visibility::Inherited } else { Visibility::Hidden };
        }
    }
}

fn count(
    real: Res<Time<Real>>,
    view: Res<ViewState>,
    link: Res<LinkState>,
    mut dirty: ResMut<crate::map::Dirty>,
    mut changes: MessageReader<ViewChanged>,
    mut record: ResMut<Record>,
) {
    let now = real.elapsed_secs_f64();
    for ViewChanged(changed) in changes.read() {
        record.changed_keys += changed.tiles.len() + changed.units.len() + changed.cities.len() + changed.civs.len();
        if let Some(last) = record.last_round.replace(now) {
            let ms = ((now - last) * 1000.0) as f32;
            record.window.rounds.push(ms);
            record.total.rounds.push(ms);
        }
    }
    let built = std::mem::take(&mut dirty.built_ms);
    record.chunk_ms.extend(built);
    record.turn = view.view().map_or(0, |v| v.turn);
    (record.resyncs, record.stale, record.stopped) = (link.resyncs, link.stale, link.stopped.clone());
}

fn measure(
    real: Res<Time<Real>>,
    link: Res<LinkState>,
    mut record: ResMut<Record>,
    mut hud: Query<&mut Text, With<Hud>>,
) {
    let dt = real.delta_secs() * 1000.0;
    // A round asked for and not yet applied: the host thread is working on it.
    let busy = link.awaiting_round;
    let now = real.elapsed_secs_f64();
    if dt > 0.0 {
        record.window.frames.push(dt);
        if busy {
            record.window.busy_frames.push(dt);
        }
    }
    if now - record.window_start < WINDOW {
        return;
    }
    record.window_start = now;
    record.windows += 1;
    let window = std::mem::take(&mut record.window);
    // The first window holds pipeline compilation; the summary leaves it out.
    if record.windows > 1 {
        record.total.frames.extend_from_slice(&window.frames);
        record.total.busy_frames.extend_from_slice(&window.busy_frames);
    }
    let line = report(&record, &window, record.windows, false);
    if let Ok(mut text) = hud.single_mut() {
        text.0 = hud_text(&record, &window);
    }
    emit(&line);
    if record.windows % SUMMARY_WINDOWS == 0 {
        let line = report(&record, &record.total, record.windows, true);
        emit(&line);
    }
}

fn percentile(sorted: &[f32], p: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    sorted[((sorted.len() - 1) as f32 * p).round() as usize]
}

fn sorted(values: &[f32]) -> Vec<f32> {
    let mut v = values.to_vec();
    v.sort_by(f32::total_cmp);
    v
}

fn report(record: &Record, window: &Window, index: u32, summary: bool) -> String {
    let frames = sorted(&window.frames);
    let busy = sorted(&window.busy_frames);
    let seconds: f32 = window.frames.iter().sum::<f32>() / 1000.0;
    let rounds = sorted(&window.rounds);
    let mean = |v: &[f32]| if v.is_empty() { 0.0 } else { v.iter().sum::<f32>() / v.len() as f32 };
    format!(
        concat!(
            "{{\"summary\":{},\"window\":{},\"frames\":{},\"fps\":{:.1},\"p50\":{:.2},\"p95\":{:.2},",
            "\"p99\":{:.2},\"max\":{:.2},\"busyFrames\":{},\"busyP99\":{:.2},\"busyMax\":{:.2},\"rounds\":{},",
            "\"roundMsMean\":{:.1},\"roundMsMax\":{:.1},\"turn\":{},\"changedKeys\":{},\"resyncs\":{},\"stale\":{},",
            "\"chunks\":{},\"chunkMsP50\":{:.2},\"chunkMsMax\":{:.2},\"stopped\":{:?}}}"
        ),
        summary,
        index,
        frames.len(),
        if seconds > 0.0 { frames.len() as f32 / seconds } else { 0.0 },
        percentile(&frames, 0.50),
        percentile(&frames, 0.95),
        percentile(&frames, 0.99),
        frames.last().copied().unwrap_or(0.0),
        busy.len(),
        percentile(&busy, 0.99),
        busy.last().copied().unwrap_or(0.0),
        rounds.len(),
        mean(&rounds),
        rounds.last().copied().unwrap_or(0.0),
        record.turn,
        record.changed_keys,
        record.resyncs,
        record.stale,
        record.chunk_ms.len(),
        percentile(&sorted(&record.chunk_ms), 0.5),
        sorted(&record.chunk_ms).last().copied().unwrap_or(0.0),
        record.stopped.as_deref().unwrap_or(""),
    )
}

fn hud_text(record: &Record, window: &Window) -> String {
    let frames = sorted(&window.frames);
    let busy = sorted(&window.busy_frames);
    let seconds: f32 = window.frames.iter().sum::<f32>() / 1000.0;
    format!(
        "turn {}  rounds {} in {:.0} s  resyncs {}\nfps {:.0}  p99 {:.1} ms  max {:.1} ms  max during a round {:.1} ms{}",
        record.turn,
        window.rounds.len(),
        seconds,
        record.resyncs,
        if seconds > 0.0 { frames.len() as f32 / seconds } else { 0.0 },
        percentile(&frames, 0.99),
        frames.last().copied().unwrap_or(0.0),
        busy.last().copied().unwrap_or(0.0),
        record.stopped.as_ref().map(|e| format!("\nhost stopped: {e}")).unwrap_or_default(),
    )
}

#[cfg(target_arch = "wasm32")]
#[wasm_bindgen::prelude::wasm_bindgen(
    inline_js = "export function civ_report(line) { const f = globalThis.civReport; if (f) f(line); }"
)]
extern "C" {
    fn civ_report(line: &str);
}

fn emit(line: &str) {
    #[cfg(target_arch = "wasm32")]
    civ_report(line);
    #[cfg(not(target_arch = "wasm32"))]
    println!("{line}");
}
