//! The Bevy side of a match: drain the `Link` into `ViewState`, announce what changed, ask for a resync when a
//! delta does not fit, and pace a watched match's rounds.

use crate::link::{Down, Link, Up};
use crate::view_state::{Applied, Changed, ViewState};
use bevy::prelude::*;
use civ_sim::protocol::ClientMessage;

/// One applied view message, for the projection (M1) and presentation (M2).
#[derive(Message, Debug, Clone)]
pub struct ViewChanged(pub Changed);

/// When a watched match closes its next round. Presentation (M2) will hold rounds while it plays them.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pace {
    /// The next round as soon as the last one has been applied.
    Continuous,
    Paused,
}

#[derive(Resource, Default, Debug)]
pub struct LinkState {
    pub applied: u32,
    pub stale: u32,
    pub resyncs: u32,
    /// A round asked for and not yet applied.
    pub awaiting_round: bool,
    pub stopped: Option<String>,
}

pub fn plugin(app: &mut App) {
    app.init_resource::<ViewState>()
        .init_resource::<LinkState>()
        .insert_resource(Pace::Continuous)
        .add_message::<ViewChanged>()
        .add_systems(PreUpdate, (receive, pace).chain());
}

fn receive(
    link: Res<Link>,
    mut view: ResMut<ViewState>,
    mut state: ResMut<LinkState>,
    mut changed: MessageWriter<ViewChanged>,
) {
    while let Some(down) = link.receive() {
        match down {
            Down::Message(message) => match view.apply(message) {
                Some(Applied::Changed(c)) => {
                    state.applied += 1;
                    state.awaiting_round = false;
                    changed.write(ViewChanged(c));
                }
                Some(Applied::Stale) => state.stale += 1,
                Some(Applied::NeedResync) => {
                    state.resyncs += 1;
                    link.send(Up::Client(ClientMessage::Resync));
                }
                // Receipts, query replies and errors belong to the Outbox (M3).
                None => {}
            },
            Down::Stopped(reason) => state.stopped = Some(reason),
        }
    }
}

fn pace(link: Res<Link>, pace: Res<Pace>, view: Res<ViewState>, mut state: ResMut<LinkState>) {
    let finished = view.view().is_some_and(|v| v.winner.is_some() || v.result.is_some());
    if *pace == Pace::Continuous && view.view().is_some() && !state.awaiting_round && state.stopped.is_none() && !finished {
        state.awaiting_round = true;
        link.send(Up::Advance);
    }
}
