//! The Bevy client for civ-sim matches (docs/CLIENT-BRIEF.md, docs/VISUAL-SPEC.md).

pub mod client;
mod harness;
pub mod link;
pub mod thread;
pub mod view_state;

use bevy::prelude::*;
use civ_sim::session::Sight;
use civ_sim::Settings;

pub fn run() {
    // A worker instantiates this module too, to run the engine thread; the app starts on the page only.
    if thread::in_worker() {
        return;
    }
    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "civ".into(),
                canvas: Some("#bevy".into()),
                fit_canvas_to_parent: true,
                ..default()
            }),
            ..default()
        }))
        .insert_resource(link::Link::watch(settings(), Sight::All))
        .add_plugins((client::plugin, harness::plugin))
        .run();
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
