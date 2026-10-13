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
