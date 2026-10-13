//! The one door between a client and civ-core.
//!
//! Builds games, derives one civ's view of the world, and advances turns. Compiled to wasm for the browser
//! (solo, inside a Web Worker) and natively for the server; both hand clients the same view types, so the
//! fog-of-war boundary is the same whether or not a server is involved.

mod city_states;
pub mod content;
#[cfg(feature = "js")]
mod hosted;
mod offers;
pub mod protocol;
pub mod queries;
pub mod rooms;
pub mod session;
#[cfg(feature = "js")]
pub use hosted::HostedGame;
mod rules;
mod setup;
mod view;

pub use rules::{tables, PolicyRow, RuleTables};
pub use setup::{new_game, new_game_for, new_game_in, SeatChoice, Settings};
pub use view::{advance, advance_for, player_view, reachable, PlayerView, TurnReport};
// The view and event types a Rust client matches on; they already cross the wire inside `GameUpdate`.
pub use view::{
    CaptureChoice, CityStateView, CityView, CivView, EventView, GreatPersonOutcome, GreatPersonProgressView, NoticeKind,
    PresentationEvent, Reach, RelationView, ReligiousEventKind, SpyActionView, TileView, TradableView, UnitView,
};

#[cfg(feature = "js")]
use serde::Serialize;
#[cfg(feature = "js")]
use wasm_bindgen::prelude::*;

/// The bundled rule tables, for menus shown before any game exists.
#[cfg(feature = "js")]
#[wasm_bindgen(js_name = ruleTables)]
pub fn rule_tables() -> Result<JsValue, JsError> {
    js(&tables(&content::load().map_err(|e| JsError::new(&e))?))
}

/// The rule tables of a bundled ruleset: `base` or `blitz`.
#[cfg(feature = "js")]
#[wasm_bindgen(js_name = ruleTablesFor)]
pub fn rule_tables_for(ruleset: &str) -> Result<JsValue, JsError> {
    js(&tables(&content::load_ruleset(ruleset).map_err(|e| JsError::new(&e))?))
}

#[cfg(feature = "js")]
fn js<T: Serialize>(v: &T) -> Result<JsValue, JsError> {
    v.serialize(&serde_wasm_bindgen::Serializer::json_compatible())
        .map_err(|e| JsError::new(&e.to_string()))
}

pub mod delta;
