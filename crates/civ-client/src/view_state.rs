//! A seat's latest knowledge: the server's own `GameUpdate`, with deltas applied to it.
//!
//! Invariant: after any run of welcomes, updates and deltas, the view held here equals the host's full view
//! for this seat at the same cursor (`tests/view_state.rs` checks it every round of seeded bot games). A delta
//! whose base is not the cursor held here is never guessed at: the caller asks for a resync.

use bevy::prelude::Resource;
use civ_sim::protocol::{GameUpdate, ServerMessage, ViewCursor, ViewRemovals};
use civ_sim::PlayerView;

/// The keys one applied message touched, for the projection to redraw.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Changed {
    /// A new match, seat or map size: redraw everything.
    pub all: bool,
    pub tiles: Vec<u16>,
    pub units: Vec<(u32, u32)>,
    pub cities: Vec<(u32, u32)>,
    pub civs: Vec<u8>,
}

#[derive(Debug, PartialEq)]
pub enum Applied {
    Changed(Changed),
    /// Older than or equal to what is held; nothing changed.
    Stale,
    /// The delta's base is not the cursor held here, or nothing is held yet.
    NeedResync,
}

#[derive(Resource, Default)]
pub struct ViewState {
    update: Option<GameUpdate>,
}

impl ViewState {
    pub fn update(&self) -> Option<&GameUpdate> {
        self.update.as_ref()
    }

    pub fn view(&self) -> Option<&PlayerView> {
        self.update.as_ref().map(|u| &u.view)
    }

    pub fn cursor(&self) -> Option<&ViewCursor> {
        self.update.as_ref().map(|u| &u.cursor)
    }

    /// Apply a welcome, update or delta. Other messages carry no view and return `None`.
    pub fn apply(&mut self, message: ServerMessage) -> Option<Applied> {
        Some(match message {
            ServerMessage::Welcome { update, .. } | ServerMessage::Update { update } => self.replace(update),
            ServerMessage::Delta { base, update, removed } => self.patch(base, update, removed),
            ServerMessage::Receipt { .. } | ServerMessage::QueryReply { .. } | ServerMessage::Error { .. } => {
                return None;
            }
        })
    }

    fn replace(&mut self, mut update: GameUpdate) -> Applied {
        sort(&mut update.view);
        let Some(old) = self.update.as_ref() else {
            self.update = Some(update);
            return Applied::Changed(Changed { all: true, ..Changed::default() });
        };
        if old.cursor.epoch == update.cursor.epoch && revision(&update.cursor) < revision(&old.cursor) {
            return Applied::Stale;
        }
        let changed = if same_frame(&old.view, &update.view) && old.cursor.epoch == update.cursor.epoch {
            Changed {
                all: false,
                tiles: diff(&old.view.tiles, &update.view.tiles, |t| t.id),
                units: diff(&old.view.units, &update.view.units, |u| (u.id, u.r#gen)),
                cities: diff(&old.view.cities, &update.view.cities, |c| (c.id, c.r#gen)),
                civs: diff(&old.view.civs, &update.view.civs, |c| c.id),
            }
        } else {
            Changed { all: true, ..Changed::default() }
        };
        self.update = Some(update);
        Applied::Changed(changed)
    }

    fn patch(&mut self, base: ViewCursor, mut update: GameUpdate, removed: ViewRemovals) -> Applied {
        let Some(old) = self.update.as_mut() else { return Applied::NeedResync };
        if base.epoch != old.cursor.epoch || update.cursor.epoch != old.cursor.epoch {
            return Applied::NeedResync;
        }
        let held = revision(&old.cursor);
        if revision(&update.cursor) <= held {
            return Applied::Stale;
        }
        if revision(&base) != held {
            return Applied::NeedResync;
        }
        let view = &mut old.view;
        let changed = Changed {
            all: false,
            tiles: merge(&mut view.tiles, std::mem::take(&mut update.view.tiles), &removed.tiles, |t| t.id),
            units: merge(&mut view.units, std::mem::take(&mut update.view.units), &removed.units, |u| (u.id, u.r#gen)),
            cities: merge(
                &mut view.cities,
                std::mem::take(&mut update.view.cities),
                &removed.cities,
                |c| (c.id, c.r#gen),
            ),
            civs: merge(&mut view.civs, std::mem::take(&mut update.view.civs), &removed.civs, |c| c.id),
        };
        // Everything but the four keyed collections is absolute.
        update.view.tiles = std::mem::take(&mut view.tiles);
        update.view.units = std::mem::take(&mut view.units);
        update.view.cities = std::mem::take(&mut view.cities);
        update.view.civs = std::mem::take(&mut view.civs);
        *old = update;
        Applied::Changed(changed)
    }
}

/// Revisions are decimal strings; one that does not parse orders first, so it can never pass as newer.
fn revision(cursor: &ViewCursor) -> Option<u64> {
    cursor.revision.parse().ok()
}

fn same_frame(a: &PlayerView, b: &PlayerView) -> bool {
    (a.me, a.width, a.height, a.wrap_x) == (b.me, b.width, b.height, b.wrap_x)
}

/// Keyed collections are held sorted by key, so merges and comparisons need no index.
pub fn sort(view: &mut PlayerView) {
    view.tiles.sort_by_key(|t| t.id);
    view.units.sort_by_key(|u| (u.id, u.r#gen));
    view.cities.sort_by_key(|c| (c.id, c.r#gen));
    view.civs.sort_by_key(|c| c.id);
}

/// Upsert `next` and drop `removed` in a sorted collection; returns every key touched.
fn merge<T, K: Ord + Copy>(held: &mut Vec<T>, next: Vec<T>, removed: &[K], key: impl Fn(&T) -> K) -> Vec<K> {
    let mut touched = Vec::with_capacity(next.len() + removed.len());
    for k in removed {
        if let Ok(i) = held.binary_search_by_key(k, &key) {
            held.remove(i);
        }
        touched.push(*k);
    }
    for item in next {
        let k = key(&item);
        match held.binary_search_by_key(&k, &key) {
            Ok(i) => held[i] = item,
            Err(i) => held.insert(i, item),
        }
        touched.push(k);
    }
    touched
}

/// Keys present in only one side, or different between them; both sides sorted by key.
fn diff<T: PartialEq, K: Ord + Copy>(old: &[T], new: &[T], key: impl Fn(&T) -> K) -> Vec<K> {
    let (mut i, mut j, mut out) = (0, 0, Vec::new());
    while i < old.len() || j < new.len() {
        match (old.get(i), new.get(j)) {
            (Some(a), Some(b)) if key(a) == key(b) => {
                if a != b {
                    out.push(key(a));
                }
                i += 1;
                j += 1;
            }
            (Some(a), Some(b)) if key(a) < key(b) => {
                out.push(key(a));
                i += 1;
            }
            (Some(a), None) => {
                out.push(key(a));
                i += 1;
            }
            (_, Some(b)) => {
                out.push(key(b));
                j += 1;
            }
            (None, None) => unreachable!(),
        }
    }
    out
}
