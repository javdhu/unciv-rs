//! The Bevy client for civ-sim matches (docs/CLIENT-BRIEF.md, docs/VISUAL-SPEC.md).

pub mod camera;
pub mod client;
mod harness;
pub mod hex;
mod inspect;
pub mod link;
mod map;
mod relief;
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
    let (settings, sight) = match &shots {
        Some(s) => (s.scene.settings.clone(), s.scene.sight),
        None => (settings(), sight()),
    };
    let mut window =
        Window { title: "civ".into(), canvas: Some("#bevy".into()), fit_canvas_to_parent: true, ..default() };
    if shots.is_some() {
        // Review shots are the same size everywhere.
        window.resolution = WindowResolution::new(1600, 1000).with_scale_factor_override(1.0);
    }
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin { primary_window: Some(window), ..default() }))
        .insert_resource(link::Link::watch(settings, sight))
        .add_plugins((
            client::plugin,
            rules::plugin,
            map::plugin,
            camera::plugin,
            inspect::plugin,
            harness::plugin,
            shots::plugin,
        ));
    if let Some(shots) = shots {
        app.insert_resource(shots);
    }
    app.run();
}

/// Whose eyes a watched match is seen through: `CIV_SIGHT=seat` natively, `civSight` on the page; all-seeing
/// otherwise.
fn sight() -> Sight {
    #[cfg(not(target_arch = "wasm32"))]
    let wanted = std::env::var("CIV_SIGHT").ok();
    #[cfg(target_arch = "wasm32")]
    let wanted = js_sys::Reflect::get(&js_sys::global(), &"civSight".into()).ok().and_then(|v| v.as_string());
    if wanted.as_deref() == Some("seat") { Sight::Seat } else { Sight::All }
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
