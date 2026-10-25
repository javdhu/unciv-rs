// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The client's applied view equals the host's full view for its seat, every round of seeded bot games, with
//! all-seeing and fogged sight (docs/CLIENT-BRIEF.md, component 2). Run optimised: `cargo test -p civ-client
//! --profile client`.

use civ_client::view_state::{sort, Applied, ViewState};
use civ_sim::protocol::{ServerMessage, PROTOCOL_VERSION};
use civ_sim::session::{Session, Sight};
use civ_sim::Settings;

const ROUNDS: u32 = 120;

fn welcome(session: &Session) -> ServerMessage {
    ServerMessage::Welcome {
        version: PROTOCOL_VERSION,
        content_hash: session.content_hash(),
        update: session.current_update(0).unwrap(),
    }
}

/// Close rounds and push each round's messages through `ViewState`, comparing with the host's full view.
fn play(seed: u64, sight: Sight) -> (u32, u32) {
    let settings = Settings { seed, ..Settings::default() };
    let mut session = Session::watch(settings, format!("test-{seed}"), sight).unwrap();
    let mut state = ViewState::default();
    assert!(matches!(state.apply(welcome(&session)), Some(Applied::Changed(c)) if c.all));
    let (mut rounds, mut deltas) = (0, 0);
    while rounds < ROUNDS {
        let Some(transition) = session.stage_round().unwrap() else { break };
        for (seat, update) in transition.updates {
            if seat != 0 {
                continue;
            }
            let message = session.update_message(0, update).unwrap();
            deltas += matches!(message, ServerMessage::Delta { .. }) as u32;
            let applied = state.apply(message);
            assert!(matches!(applied, Some(Applied::Changed(_))), "seed {seed}: {applied:?}");
        }
        session = transition.next;
        rounds += 1;
        let mut full = session.current_update(0).unwrap();
        sort(&mut full.view);
        assert_eq!(state.cursor(), Some(&full.cursor), "seed {seed} round {rounds}");
        assert!(state.view() == Some(&full.view), "seed {seed} round {rounds}: views differ");
    }
    (rounds, deltas)
}

#[test]
fn applied_views_match_the_host_all_seeing() {
    for seed in 1..=3 {
        let (rounds, deltas) = play(seed, Sight::All);
        assert!(rounds >= 60 && deltas >= rounds / 2, "seed {seed}: {rounds} rounds, {deltas} deltas");
    }
}

#[test]
fn applied_views_match_the_host_fogged() {
    for seed in 1..=3 {
        let (rounds, deltas) = play(seed, Sight::Seat);
        assert!(rounds >= 60 && deltas >= rounds / 2, "seed {seed}: {rounds} rounds, {deltas} deltas");
    }
}

/// A missed delta asks for a resync, a full update recovers, and a repeated delta changes nothing.
#[test]
fn gaps_resync_and_repeats_are_stale() {
    let mut session = Session::watch(Settings::default(), "gaps".into(), Sight::Seat).unwrap();
    let mut state = ViewState::default();
    state.apply(welcome(&session));
    let next_round = |session: &mut Session| {
        let transition = session.stage_round().unwrap().unwrap();
        let (_, update) = transition.updates.into_iter().find(|(seat, _)| *seat == 0).unwrap();
        let message = session.update_message(0, update).unwrap();
        *session = transition.next;
        message
    };
    let first = next_round(&mut session);
    let repeat = first.clone();
    assert!(matches!(state.apply(first), Some(Applied::Changed(_))));
    assert_eq!(state.apply(repeat), Some(Applied::Stale));
    let _missed = next_round(&mut session);
    let after_gap = next_round(&mut session);
    assert!(matches!(after_gap, ServerMessage::Delta { .. }));
    assert_eq!(state.apply(after_gap), Some(Applied::NeedResync));
    let resync = ServerMessage::Update { update: session.current_update(0).unwrap() };
    assert!(matches!(state.apply(resync), Some(Applied::Changed(_))));
    let mut full = session.current_update(0).unwrap();
    sort(&mut full.view);
    assert!(state.view() == Some(&full.view));
}
