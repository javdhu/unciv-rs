// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The Bevy side end to end, without a window: an in-process watched match streams through the `Link` into
//! `ViewState`, round after round, with no resync and nothing stale.

use bevy::prelude::*;
use civ_client::client::{self, LinkState};
use civ_client::link::Link;
use civ_client::view_state::ViewState;
use civ_sim::session::Sight;
use civ_sim::Settings;
use std::time::{Duration, Instant};

#[test]
fn a_watched_match_streams_into_the_view() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(client::plugin)
        .insert_resource(Link::watch(Settings { seed: 5, ..Settings::default() }, Sight::Seat));
    let started = Instant::now();
    let turn = |app: &App| app.world().resource::<ViewState>().view().map_or(0, |v| v.turn);
    while turn(&app) < 100 {
        app.update();
        let state = app.world().resource::<LinkState>();
        assert!(state.stopped.is_none(), "host stopped: {:?}", state.stopped);
        assert!(started.elapsed() < Duration::from_secs(300), "100 rounds took over five minutes");
        std::thread::sleep(Duration::from_millis(1));
    }
    let state = app.world().resource::<LinkState>();
    assert_eq!((state.resyncs, state.stale), (0, 0));
    assert!(state.applied >= 100);
}

/// Watching through a chosen civ shows that civ's view, and switching eyes mid-match lands on the same turn.
#[test]
fn eyes_follow_the_chosen_civ_and_switch_at_the_same_turn() {
    use civ_client::link::Up;
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(client::plugin)
        .insert_resource(Link::watch(Settings { seed: 5, ..Settings::default() }, Sight::Civ(2)));
    let view = |app: &App| app.world().resource::<ViewState>().view().map(|v| (v.me, v.turn));
    let started = Instant::now();
    while view(&app).map_or(0, |v| v.1) < 12 {
        app.update();
        assert!(started.elapsed() < Duration::from_secs(120));
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(view(&app).map(|v| v.0), Some(2));
    app.insert_resource(client::Pace::Paused);
    // Let any round in flight land, then switch.
    for _ in 0..50 {
        app.update();
        std::thread::sleep(Duration::from_millis(2));
    }
    let turn = view(&app).unwrap().1;
    app.world().resource::<Link>().send(Up::Watch(Sight::Civ(1)));
    while view(&app).map(|v| v.0) != Some(1) {
        app.update();
        assert!(started.elapsed() < Duration::from_secs(120), "no welcome through civ 1");
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(view(&app).unwrap().1, turn);
    let state = app.world().resource::<LinkState>();
    assert_eq!((state.resyncs, state.stale), (0, 0));
}
