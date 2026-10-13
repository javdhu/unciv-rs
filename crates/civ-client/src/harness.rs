//! The frame-time harness (S0, docs/CLIENT-BRIEF.md): a stand-in scene that keeps the main thread busy every
//! frame while the host thread closes a watched match's rounds back to back, and a record of frame times, round
//! times and link health, reported every five seconds (stdout natively, `globalThis.civReport` on the page).
//! The stand-in scene goes when M1 draws the map; the record stays for the budgets in docs/VISUAL-SPEC.md.

use crate::client::{LinkState, ViewChanged};
use crate::view_state::ViewState;
use bevy::prelude::*;
use std::f32::consts::PI;

/// Seconds per reported window, and windows per summary.
const WINDOW: f64 = 5.0;
const SUMMARY_WINDOWS: u32 = 12;

pub fn plugin(app: &mut App) {
    app.init_resource::<Record>()
        .add_systems(Startup, setup)
        .add_systems(Update, (count, bob, measure).chain());
}

#[derive(Component)]
struct Bob(f32);

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
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let pitch = 52f32.to_radians();
    let distance = 24.0;
    commands.spawn((
        Camera3d::default(),
        Transform::from_xyz(0.0, distance * pitch.sin(), distance * pitch.cos()).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        DirectionalLight { illuminance: 12_000.0, shadow_maps_enabled: true, ..default() },
        Transform::from_rotation(Quat::from_euler(EulerRot::YXZ, (315f32 - 180.0).to_radians(), -42f32.to_radians(), 0.0)),
    ));
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(60.0, 36.0))),
        MeshMaterial3d(materials.add(Color::srgb_u8(0x6F, 0x8F, 0x3E))),
    ));
    let piece = meshes.add(Cuboid::new(0.6, 0.6, 0.6));
    let colours = [0xA6A05A, 0xD9C48E, 0x7C766D, 0x3C8FA9].map(|c: u32| {
        materials.add(Color::srgb_u8((c >> 16) as u8, (c >> 8) as u8, c as u8))
    });
    // One piece per tile of a 32x20 map, on odd-r hex centres.
    for row in 0..20 {
        for col in 0..32 {
            let x = 3f32.sqrt() * (col as f32 + 0.5 * (row % 2) as f32) - 28.0;
            let z = 1.5 * row as f32 - 14.0;
            commands.spawn((
                Mesh3d(piece.clone()),
                MeshMaterial3d(colours[(row * 32 + col) % 4].clone()),
                Transform::from_xyz(x, 0.3, z),
                Bob((row * 32 + col) as f32 * 0.37),
            ));
        }
    }
    commands.spawn((
        Hud,
        Text::new("S0: starting"),
        TextFont { font_size: FontSize::Px(15.0), ..default() },
        Node { position_type: PositionType::Absolute, top: Val::Px(10.0), left: Val::Px(12.0), ..default() },
    ));
}

fn bob(time: Res<Time>, mut pieces: Query<(&Bob, &mut Transform)>) {
    let t = time.elapsed_secs();
    for (bob, mut transform) in &mut pieces {
        transform.translation.y = 0.3 + 0.25 * (t * 1.7 + bob.0).sin();
        transform.rotation = Quat::from_rotation_y(t * 0.8 + bob.0 * PI);
    }
}

fn count(
    real: Res<Time<Real>>,
    view: Res<ViewState>,
    link: Res<LinkState>,
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
            "\"stopped\":{:?}}}"
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
