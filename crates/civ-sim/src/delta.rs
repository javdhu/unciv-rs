//! Deltas are computed only from seat-filtered views, never from the hidden world.
use crate::protocol::{ViewCursor, GameUpdate, ServerMessage, ViewRemovals};
use crate::view::PlayerView;
use std::collections::BTreeMap;

fn changed<T: PartialEq, K: Ord + Copy>(old: &[T], next: &mut Vec<T>, key: impl Fn(&T) -> K) -> Vec<K> {
    let mut prior: BTreeMap<_, _> = old.iter().map(|value| (key(value), value)).collect();
    next.retain(|value| prior.remove(&key(value)) != Some(value));
    prior.into_keys().collect()
}

pub fn message(cursor: &ViewCursor, before: &PlayerView, mut update: GameUpdate) -> ServerMessage {
    if cursor.epoch != update.cursor.epoch || before.me != update.view.me
        || (before.width, before.height, before.wrap_x) != (update.view.width, update.view.height, update.view.wrap_x)
    {
        return ServerMessage::Update { update };
    }
    let removed = ViewRemovals {
        tiles: changed(&before.tiles, &mut update.view.tiles, |t| t.id),
        units: changed(&before.units, &mut update.view.units, |u| (u.id, u.gen)),
        cities: changed(&before.cities, &mut update.view.cities, |c| (c.id, c.gen)),
        civs: changed(&before.civs, &mut update.view.civs, |c| c.id),
    };
    ServerMessage::Delta { base: cursor.clone(), update, removed }
}
