// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! A watched match shows its watcher exactly what the observer `Game` showed the pages that watch bots: seat 0's
//! view, or every tile in sight, and seat 0's turn report, turn after turn.

use civ_core::world::CivId;
use civ_sim::protocol::{Action, CommandRequest, CommandStatus, GameUpdate, RejectionCode, ServerMessage};
use civ_sim::session::{Session, SessionError, Sight};
use civ_sim::{advance_for, new_game, player_view, PlayerView, Settings};

/// The landing page's match, smaller.
fn settings(seed: u64) -> Settings {
    Settings { seed, width: 36, height: 24, majors: 5, city_states: 2, ..Settings::default() }
}

/// `Game::spectatorView` as it was: seat 0's view built on a copy of the world with every tile observed.
fn old_spectator_view(world: &civ_core::world::World, rules: &civ_core::ruleset::Ruleset) -> PlayerView {
    let mut world = world.clone();
    let tiles: Vec<_> = world.grid.ids().collect();
    for tile in tiles {
        world.visibility.observe(CivId(0), tile);
    }
    civ_core::world::fog::capture_observations(&mut world, rules);
    player_view(&world, rules, CivId(0))
}

fn only_update(updates: Vec<(u8, GameUpdate)>, watcher: u8) -> GameUpdate {
    let mut updates = updates.into_iter();
    let (seat, update) = updates.next().expect("the watcher is shown the round");
    assert_eq!(seat, watcher);
    assert!(updates.next().is_none(), "only the watcher is shown anything");
    update
}

fn compare(sight: Sight, seed: u64, turns: u32) {
    // `new Game(settings)`, then `endTurn()` and `view()` or `spectatorView()` turn by turn.
    let (mut world, rules, player) = new_game(&settings(seed)).unwrap();
    assert_eq!(player, CivId(0));
    let me = match sight {
        Sight::Civ(civ) => CivId(civ),
        Sight::Seat | Sight::All => player,
    };
    let old_view = |world: &_| match sight {
        Sight::Seat | Sight::Civ(_) => player_view(world, &rules, me),
        Sight::All => old_spectator_view(world, &rules),
    };
    let mut session = Session::watch(settings(seed), "watch".into(), sight).unwrap();
    assert_eq!(session.seats().collect::<Vec<_>>(), vec![me.0]);
    assert_eq!(session.current_update(me.0).unwrap().view, old_view(&world));
    let mut reported = 0;
    for _ in 0..turns {
        let report = advance_for(&mut world, &rules, me);
        let transition = session.stage_round().unwrap().expect("the match is not over");
        let update = only_update(transition.updates, me.0);
        session = transition.next;
        assert_eq!(update.view, old_view(&world), "turn {}", report.turn);
        let events: Vec<_> = update.events.into_iter().map(|e| e.event).collect();
        assert_eq!(events, report.events, "turn {}", report.turn);
        assert!(!update.controlled && update.replay.is_none());
        reported += events.len();
        if world.winner.is_some() {
            break;
        }
    }
    assert!(reported > 0, "the watched civ saw something happen");
}

#[test]
fn a_watched_match_shows_seat_zero_as_the_observer_game_did() {
    compare(Sight::Seat, 11, 40);
}

#[test]
fn a_watcher_who_sees_all_sees_what_the_observer_game_showed() {
    compare(Sight::All, 12, 30);
}

#[test]
fn a_watcher_can_watch_through_any_civilization() {
    compare(Sight::Civ(2), 13, 30);
    assert!(matches!(
        Session::watch(settings(13), "watch".into(), Sight::Civ(200)),
        Err(SessionError::InvalidSettings(_))
    ));
}

#[test]
fn a_watched_match_takes_no_commands_and_is_not_saved() {
    let session = Session::watch(settings(3), "watch".into(), Sight::Seat).unwrap();
    let update = session.current_update(0).unwrap();
    let request = CommandRequest {
        command_id: "finish".into(),
        epoch: "watch".into(),
        turn_token: update.turn_token,
        action: Action::FinishTurn,
    };
    let transition = session.stage_command(0, request).unwrap();
    assert_eq!(transition.receipt.result, CommandStatus::Rejected { code: RejectionCode::NotControlled });
    assert!(transition.updates.is_empty());
    assert!(matches!(session.save(), Err(SessionError::InvalidSave(_))));
    // A played match does not close its own rounds.
    let played = Session::new(settings(3), "played".into(), vec![0]).unwrap();
    assert!(played.stage_round().is_err());
}

#[test]
fn a_watchers_updates_are_deltas_against_its_last_view() {
    let mut session = Session::watch(settings(5), "watch".into(), Sight::All).unwrap();
    for _ in 0..3 {
        let transition = session.stage_round().unwrap().unwrap();
        let update = only_update(transition.updates, 0);
        let ServerMessage::Delta { removed, .. } = session.update_message(0, update).unwrap() else {
            panic!("a watcher's round is a delta");
        };
        // Every tile is in sight from the start, so none is ever dropped.
        assert!(removed.tiles.is_empty());
        session = transition.next;
    }
}
