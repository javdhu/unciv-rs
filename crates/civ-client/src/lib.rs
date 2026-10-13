//! The Bevy client for civ-sim matches (docs/CLIENT-BRIEF.md, docs/VISUAL-SPEC.md).

pub mod api;
pub mod app;
pub mod camera;
pub mod client;
mod decisions;
pub mod empire;
mod harness;
mod hud;
pub mod hex;
mod inspect;
pub mod log;
pub mod link;
mod map;
pub mod notes;
pub mod net;
mod paint;
mod pieces;
pub mod play;
mod playui;
mod screens;
mod present;
mod relief;
mod rooms;
pub mod saves;
mod roomui;
pub mod rules;
mod shots;
pub mod thread;
pub mod view_state;

use bevy::prelude::*;
use bevy::window::WindowResolution;
use civ_sim::session::Sight;
use civ_sim::Settings;

pub fn run() {
    // A worker instantiates this module too, to run the engine thread; the app starts on the page only.
    if thread::in_worker() {
        return;
    }
    let shots = shots::Shots::requested();
    // Shots and recordings go straight to their match, as do CIV_SOLO, CIV_WATCH and CIV_MATCH (civSolo,
    // civWatch and ?match= on the page); otherwise the app opens on the rooms screens.
    let launch = match &shots {
        Some(s) if s.scene.room => None,
        Some(s) if s.scene.play => Some(app::Start::Solo(s.scene.settings.clone())),
        Some(s) => Some(app::Start::Watch(s.scene.settings.clone(), s.scene.sight)),
        None if flag("CIV_WATCH", "civWatch") => Some(app::Start::Watch(settings(), sight())),
        None if flag("CIV_SOLO", "civSolo") => Some(app::Start::Solo(settings())),
        None => match_requested().map(|match_id| app::Start::Room { server: net::Server::from_env(), match_id }),
    };
    let mut window =
        Window { title: "civ".into(), canvas: Some("#bevy".into()), fit_canvas_to_parent: true, ..default() };
    if shots.is_some() {
        // Review shots are the same size everywhere.
        window.resolution = WindowResolution::new(1600, 1000).with_scale_factor_override(1.0);
    }
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin { primary_window: Some(window), ..default() }))
        .add_plugins(app::plugin)
        .insert_resource(app::Launch(launch))
        .add_plugins((
            client::plugin,
            rules::plugin,
            map::plugin,
            paint::plugin,
            pieces::plugin,
            present::plugin,
            hud::plugin,
            play::plugin,
            playui::plugin,
            screens::plugin,
            camera::plugin,
            inspect::plugin,
            harness::plugin,
            shots::plugin,
        ))
        .add_plugins((roomui::plugin, rooms::plugin, decisions::plugin, log::plugin, saves::plugin, empire::plugin));
    if let Some(shots) = shots {
        // A recording steps the clock by exactly one frame's time, so it plays at true speed however long
        // each capture takes.
        if shots.scene.record.is_some() {
            app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(std::time::Duration::from_secs_f32(
                1.0 / shots::RECORD_FPS,
            )));
        }
        app.insert_resource(shots);
    }
    app.run();
}

/// An environment variable natively, a page global in the browser.
fn flag(env: &str, global: &str) -> bool {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = global;
        std::env::var(env).is_ok()
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = env;
        js_sys::Reflect::get(&js_sys::global(), &global.into()).is_ok_and(|v| v.is_truthy())
    }
}

/// A room's match to open at once: `CIV_MATCH` natively, `?match=` on the page (kept there while a match is
/// open, so a reload returns to it).
fn match_requested() -> Option<String> {
    #[cfg(not(target_arch = "wasm32"))]
    return std::env::var("CIV_MATCH").ok().filter(|m| !m.is_empty());
    #[cfg(target_arch = "wasm32")]
    return web_sys::window()
        .and_then(|w| w.location().search().ok())
        .and_then(|q| q.trim_start_matches('?').split('&').find_map(|p| p.strip_prefix("match=").map(str::to_string)))
        .filter(|m| !m.is_empty() && m.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_'));
}

/// Whose eyes a watched match is seen through: `CIV_SIGHT=seat` or a civ id natively, `civSight` on the page;
/// all-seeing otherwise.
fn sight() -> Sight {
    #[cfg(not(target_arch = "wasm32"))]
    let wanted = std::env::var("CIV_SIGHT").ok();
    #[cfg(target_arch = "wasm32")]
    let wanted = js_sys::Reflect::get(&js_sys::global(), &"civSight".into()).ok().and_then(|v| v.as_string());
    match wanted.as_deref() {
        Some("seat") => Sight::Seat,
        Some(civ) => civ.parse().map_or(Sight::All, Sight::Civ),
        None => Sight::All,
    }
}

/// Match settings as civ-sim's JSON (`Settings`, camelCase): `CIV_SETTINGS` natively, `civSettings` on the
/// page. Absent or unreadable gives civ-sim's default 32x20 `base` match.
fn settings() -> Settings {
    #[cfg(not(target_arch = "wasm32"))]
    let json = std::env::var("CIV_SETTINGS").ok();
    #[cfg(target_arch = "wasm32")]
    let json = js_sys::Reflect::get(&js_sys::global(), &"civSettings".into()).ok().and_then(|v| v.as_string());
    json.and_then(|json| serde_json::from_str(&json).map_err(|e| warn!("CIV_SETTINGS: {e}")).ok())
        .unwrap_or_default()
}
