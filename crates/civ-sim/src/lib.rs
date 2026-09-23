//! The one door between a client and civ-core.
//!
//! Builds games, derives one civ's view of the world, and advances turns. Compiled to wasm for the browser
//! (solo, inside a Web Worker) and natively for the server; both hand clients the same view types, so the
//! fog-of-war boundary is the same whether or not a server is involved.

mod city_states;
pub mod content;
mod hosted;
mod offers;
pub mod protocol;
pub mod queries;
pub mod session;
pub use hosted::HostedGame;
mod rules;
mod setup;
mod view;

pub use rules::{tables, RuleTables};
pub use setup::{new_game, Settings};
pub use view::{advance, advance_for, player_view, reachable, PlayerView, TurnReport};

use civ_core::ruleset::Ruleset;
use civ_core::world::{CivId, World};
use serde::Serialize;
use wasm_bindgen::prelude::*;

/// The bundled rule tables, for menus shown before any game exists.
#[wasm_bindgen(js_name = ruleTables)]
pub fn rule_tables() -> Result<JsValue, JsError> {
    js(&tables(&content::load().map_err(|e| JsError::new(&e))?))
}

/// A game hosted in the browser. Every method returns plain JS objects (camelCase, JSON-compatible).
#[wasm_bindgen]
pub struct Game {
    world: World,
    rules: Ruleset,
    me: CivId,
}

fn js<T: Serialize>(v: &T) -> Result<JsValue, JsError> {
    v.serialize(&serde_wasm_bindgen::Serializer::json_compatible())
        .map_err(|e| JsError::new(&e.to_string()))
}

#[wasm_bindgen]
impl Game {
    #[wasm_bindgen(constructor)]
    pub fn new(settings: JsValue) -> Result<Game, JsError> {
        console_error_panic_hook::set_once();
        let settings: Settings = if settings.is_undefined() || settings.is_null() {
            Settings::default()
        } else {
            serde_wasm_bindgen::from_value(settings).map_err(|e| JsError::new(&e.to_string()))?
        };
        let (world, rules, me) = new_game(&settings).map_err(|e| JsError::new(&e))?;
        Ok(Game { world, rules, me })
    }

    /// Static ruleset tables; send once, index by id.
    pub fn rules(&self) -> Result<JsValue, JsError> {
        js(&tables(&self.rules))
    }

    /// Everything the local civ may know right now.
    pub fn view(&self) -> Result<JsValue, JsError> {
        js(&player_view(&self.world, &self.rules, self.me))
    }

    /// Advance the world one turn and report what happened.
    #[wasm_bindgen(js_name = endTurn)]
    pub fn end_turn(&mut self) -> Result<JsValue, JsError> {
        js(&advance_for(&mut self.world, &self.rules, self.me))
    }

    /// Tiles one of the local civ's units can reach this turn, by the engine's own movement rule.
    pub fn reachable(&self, index: u32, generation: u32) -> Result<JsValue, JsError> {
        js(&reachable(
            &self.world,
            &self.rules,
            self.me,
            index,
            generation,
        ))
    }
}
