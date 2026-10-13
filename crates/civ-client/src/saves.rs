//! Solo saves (docs/ACTIONS-SPEC.md, A3, A4): the solo host saves at every turn's end (`Down::Saved`), one slot
//! per match, kept as files natively (`<config>/saves/<id>.civsave` and `<id>.json`) and in IndexedDB in the
//! browser (stores `saves` and `meta` of the `civ` database). Home lists them, newest first, with Continue.
//!
//! A save restores only on a build with the same engine version and rules (`Session::restore`); another build's
//! save fails when opened, and the player is told why on Home.

use crate::client::{Autosaved, LinkState};
use crate::view_state::ViewState;
use bevy::prelude::*;
use concurrent_queue::ConcurrentQueue;
use std::sync::Arc;

/// What Home shows about a save.
#[derive(Clone, Debug, PartialEq)]
pub struct SaveMeta {
    pub id: String,
    pub turn: u32,
    pub year: i32,
    pub nation: String,
    pub width: u16,
    pub height: u16,
    /// Unix seconds.
    pub saved_at: f64,
}

impl SaveMeta {
    fn to_json(&self) -> String {
        serde_json::json!({
            "id": self.id, "turn": self.turn, "year": self.year, "nation": self.nation,
            "width": self.width, "height": self.height, "savedAt": self.saved_at,
        })
        .to_string()
    }

    fn from_json(text: &str) -> Option<SaveMeta> {
        let v: serde_json::Value = serde_json::from_str(text).ok()?;
        Some(SaveMeta {
            id: v.get("id")?.as_str()?.to_string(),
            turn: v.get("turn")?.as_u64()? as u32,
            year: v.get("year")?.as_i64()? as i32,
            nation: v.get("nation")?.as_str()?.to_string(),
            width: v.get("width")?.as_u64()? as u16,
            height: v.get("height")?.as_u64()? as u16,
            saved_at: v.get("savedAt")?.as_f64()?,
        })
    }
}

/// An answer from the store.
pub enum Reply {
    Listed(Vec<SaveMeta>),
    Loaded(String, Vec<u8>),
    Failed(String),
}

#[derive(Resource)]
pub struct Saves {
    /// The solo match being played, whose autosaves land under this id.
    pub current: Option<String>,
    /// Saved matches, newest first, once listed.
    pub list: Vec<SaveMeta>,
    /// Something Home should say: a save that would not open, a match that stopped before it began.
    pub notice: Option<String>,
    replies: Arc<ConcurrentQueue<Reply>>,
}

impl Default for Saves {
    fn default() -> Self {
        Saves { current: None, list: Vec::new(), notice: None, replies: Arc::new(ConcurrentQueue::unbounded()) }
    }
}

/// A new solo match's id: unique on this machine.
pub fn new_id(seed: u64) -> String {
    format!("solo-{:x}-{seed}", (now_secs() * 1000.0) as u64)
}

fn now_secs() -> f64 {
    #[cfg(not(target_arch = "wasm32"))]
    return std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    #[cfg(target_arch = "wasm32")]
    return js_sys::Date::now() / 1000.0;
}

impl Saves {
    pub fn request_list(&self) {
        store::list(&self.replies);
    }

    pub fn request_load(&self, id: &str) {
        store::load(&self.replies, id);
    }

    pub fn delete(&mut self, id: &str) {
        store::delete(id);
        self.list.retain(|m| m.id != id);
    }

    fn write(&mut self, meta: SaveMeta, save: Vec<u8>) {
        store::write(&self.replies, &meta, save);
        self.list.retain(|m| m.id != meta.id);
        self.list.insert(0, meta);
    }

    /// Answers that arrived since the last call; listings and failures are applied here.
    pub fn receive(&mut self) -> Vec<(String, Vec<u8>)> {
        let mut loaded = Vec::new();
        while let Ok(reply) = self.replies.pop() {
            match reply {
                Reply::Listed(mut list) => {
                    list.sort_by(|a, b| b.saved_at.total_cmp(&a.saved_at));
                    self.list = list;
                }
                Reply::Loaded(id, save) => loaded.push((id, save)),
                Reply::Failed(why) => self.notice = Some(why),
            }
        }
        loaded
    }
}

pub fn plugin(app: &mut App) {
    app.init_resource::<Saves>()
        .add_systems(OnEnter(crate::app::AppState::Home), |saves: Res<Saves>| saves.request_list())
        .add_systems(Update, (autosave, stopped_early).in_set(crate::app::InMatch));
}

/// Keep the host's save under the match's id, with what Home shows about it.
fn autosave(
    mut saved: MessageReader<Autosaved>,
    mut saves: ResMut<Saves>,
    view: Res<ViewState>,
    rules: Option<Res<crate::rules::Rules>>,
) {
    for Autosaved(save) in saved.read() {
        let (Some(id), Some(v)) = (saves.current.clone(), view.view()) else { continue };
        let nation = v
            .civs
            .iter()
            .find(|c| c.id == v.me)
            .and_then(|c| rules.as_ref().and_then(|r| r.nation(c.nation)))
            .map_or("Your nation".to_string(), |n| n.0.to_string());
        let meta = SaveMeta { id, turn: v.turn, year: v.year, nation, width: v.width, height: v.height, saved_at: now_secs() };
        saves.write(meta, save.clone());
    }
}

/// A match whose host stopped before its first view goes back where it came from, saying why.
fn stopped_early(
    link: Res<LinkState>,
    view: Res<ViewState>,
    remote: Option<Res<crate::link::Link>>,
    mut saves: ResMut<Saves>,
    mut lobby: Option<ResMut<crate::rooms::Lobby>>,
    mut next: ResMut<NextState<crate::app::AppState>>,
) {
    let (Some(why), None) = (link.stopped.clone(), view.view()) else { return };
    if remote.is_some_and(|l| l.is_remote()) {
        if let Some(lobby) = lobby.as_mut() {
            lobby.set_error(why);
        }
        crate::app::leave(&mut next, crate::app::AppState::Rooms);
    } else {
        saves.notice = Some(why);
        crate::app::leave(&mut next, crate::app::AppState::Home);
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod store {
    use super::*;
    use std::path::PathBuf;

    fn dir() -> PathBuf {
        crate::net::config_dir().join("saves")
    }

    pub fn write(replies: &ConcurrentQueue<Reply>, meta: &SaveMeta, save: Vec<u8>) {
        let dir = dir();
        let wrote = std::fs::create_dir_all(&dir)
            .and_then(|_| std::fs::write(dir.join(format!("{}.civsave", meta.id)), save))
            .and_then(|_| std::fs::write(dir.join(format!("{}.json", meta.id)), meta.to_json()));
        if let Err(e) = wrote {
            let _ = replies.push(Reply::Failed(format!("the match could not be saved: {e}")));
        }
    }

    pub fn list(replies: &ConcurrentQueue<Reply>) {
        let list = std::fs::read_dir(dir())
            .map(|entries| {
                entries
                    .flatten()
                    .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
                    .filter_map(|e| std::fs::read_to_string(e.path()).ok())
                    .filter_map(|t| SaveMeta::from_json(&t))
                    .collect()
            })
            .unwrap_or_default();
        let _ = replies.push(Reply::Listed(list));
    }

    pub fn load(replies: &ConcurrentQueue<Reply>, id: &str) {
        let _ = replies.push(match std::fs::read(dir().join(format!("{id}.civsave"))) {
            Ok(save) => Reply::Loaded(id.to_string(), save),
            Err(e) => Reply::Failed(format!("the save could not be read: {e}")),
        });
    }

    pub fn delete(id: &str) {
        let _ = std::fs::remove_file(dir().join(format!("{id}.civsave")));
        let _ = std::fs::remove_file(dir().join(format!("{id}.json")));
    }
}

#[cfg(target_arch = "wasm32")]
mod store {
    use super::*;
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::{JsCast, JsValue};
    use web_sys::{IdbDatabase, IdbOpenDbRequest, IdbRequest, IdbTransactionMode};

    /// Open the `civ` database (making its stores the first time) and hand it to `then`.
    fn with_db(replies: Arc<ConcurrentQueue<Reply>>, then: impl FnOnce(IdbDatabase) + 'static) {
        let fail = |replies: &ConcurrentQueue<Reply>| {
            let _ = replies.push(Reply::Failed("saves are not available in this browser".into()));
        };
        let Some(open) = web_sys::window()
            .and_then(|w| w.indexed_db().ok().flatten())
            .and_then(|f| f.open_with_u32("civ", 1).ok())
        else {
            return fail(&replies);
        };
        let upgrade = Closure::once(move |e: web_sys::Event| {
            let Some(req) = e.target().and_then(|t| t.dyn_into::<IdbOpenDbRequest>().ok()) else { return };
            if let Ok(db) = req.result().and_then(|r| r.dyn_into::<IdbDatabase>()) {
                let _ = db.create_object_store("saves");
                let _ = db.create_object_store("meta");
            }
        });
        let success = Closure::once(move |e: web_sys::Event| {
            let db = e.target().and_then(|t| t.dyn_into::<IdbOpenDbRequest>().ok()).and_then(|r| r.result().ok());
            match db.and_then(|d| d.dyn_into::<IdbDatabase>().ok()) {
                Some(db) => then(db),
                None => fail(&replies),
            }
        });
        open.set_onupgradeneeded(Some(upgrade.as_ref().unchecked_ref()));
        open.set_onsuccess(Some(success.as_ref().unchecked_ref()));
        upgrade.forget();
        success.forget();
    }

    fn on_success(request: &IdbRequest, then: impl FnOnce(JsValue) + 'static) {
        let done = Closure::once(move |e: web_sys::Event| {
            let value = e.target().and_then(|t| t.dyn_into::<IdbRequest>().ok()).and_then(|r| r.result().ok());
            then(value.unwrap_or(JsValue::UNDEFINED));
        });
        request.set_onsuccess(Some(done.as_ref().unchecked_ref()));
        done.forget();
    }

    pub fn write(replies: &Arc<ConcurrentQueue<Reply>>, meta: &SaveMeta, save: Vec<u8>) {
        let (meta, key) = (meta.to_json(), meta.id.clone());
        let failed = replies.clone();
        with_db(replies.clone(), move |db| {
            let stores = js_sys::Array::of2(&"saves".into(), &"meta".into());
            let Ok(tx) = db.transaction_with_str_sequence_and_mode(&stores, IdbTransactionMode::Readwrite) else {
                let _ = failed.push(Reply::Failed("the match could not be saved".into()));
                return;
            };
            let bytes = js_sys::Uint8Array::from(&save[..]);
            let key = JsValue::from_str(&key);
            if let (Ok(saves), Ok(metas)) = (tx.object_store("saves"), tx.object_store("meta")) {
                let _ = saves.put_with_key(&bytes, &key);
                let _ = metas.put_with_key(&JsValue::from_str(&meta), &key);
            }
        });
    }

    pub fn list(replies: &Arc<ConcurrentQueue<Reply>>) {
        let out = replies.clone();
        with_db(replies.clone(), move |db| {
            let Ok(store) = db.transaction_with_str("meta").and_then(|t| t.object_store("meta")) else { return };
            let Ok(request) = store.get_all() else { return };
            on_success(&request, move |all| {
                let list = js_sys::Array::from(&all)
                    .iter()
                    .filter_map(|m| m.as_string())
                    .filter_map(|t| SaveMeta::from_json(&t))
                    .collect();
                let _ = out.push(Reply::Listed(list));
            });
        });
    }

    pub fn load(replies: &Arc<ConcurrentQueue<Reply>>, id: &str) {
        let (out, id) = (replies.clone(), id.to_string());
        with_db(replies.clone(), move |db| {
            let Ok(store) = db.transaction_with_str("saves").and_then(|t| t.object_store("saves")) else { return };
            let Ok(request) = store.get(&JsValue::from_str(&id)) else { return };
            on_success(&request, move |value| {
                let _ = out.push(match value.dyn_into::<js_sys::Uint8Array>() {
                    Ok(bytes) => Reply::Loaded(id, bytes.to_vec()),
                    Err(_) => Reply::Failed("the save could not be read".into()),
                });
            });
        });
    }

    pub fn delete(id: &str) {
        let id = id.to_string();
        with_db(Arc::new(ConcurrentQueue::unbounded()), move |db| {
            let stores = js_sys::Array::of2(&"saves".into(), &"meta".into());
            if let Ok(tx) = db.transaction_with_str_sequence_and_mode(&stores, IdbTransactionMode::Readwrite) {
                for name in ["saves", "meta"] {
                    if let Ok(store) = tx.object_store(name) {
                        let _ = store.delete(&JsValue::from_str(&id));
                    }
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meta_reads_back_as_written() {
        let meta = SaveMeta { id: "solo-1-3".into(), turn: 42, year: -1200, nation: "Rome".into(), width: 32, height: 20, saved_at: 1.5e9 };
        assert_eq!(SaveMeta::from_json(&meta.to_json()), Some(meta));
        assert_eq!(SaveMeta::from_json("{}"), None);
    }
}
