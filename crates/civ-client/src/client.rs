//! The Bevy side of a match: drain the `Link` into `ViewState`, announce what changed, ask for a resync when a
//! delta does not fit, and pace a watched match's rounds.

use crate::link::{Down, Link, Up};
use crate::view_state::{Applied, Changed, ViewState};
use bevy::prelude::*;
use civ_sim::protocol::{ClientMessage, QueryReply, ServerMessage};

/// One applied view message, for the projection (M1) and presentation (M2).
#[derive(Message, Debug, Clone)]
pub struct ViewChanged(pub Changed);

/// The presentation events of one applied update, in order (a watched round's moves, battles, foundings).
#[derive(Message, Debug, Clone)]
pub struct RoundEvents(pub Vec<civ_sim::protocol::PresentedEvent>);

/// Closed while presentation plays a round: the next round waits. Absent, rounds run back to back.
#[derive(Resource, Debug, Clone, Copy)]
pub struct Gate {
    pub open: bool,
}

/// A reply to a query this client sent, by the request id it chose.
#[derive(Message, Debug, Clone)]
pub struct QueryAnswered {
    pub request_id: String,
    pub reply: QueryReply,
}

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
        .add_message::<QueryAnswered>()
        .add_message::<RoundEvents>()
        .add_systems(PreUpdate, (receive, pace).chain());
}

pub fn receive(
    link: Res<Link>,
    mut view: ResMut<ViewState>,
    mut state: ResMut<LinkState>,
    mut changed: MessageWriter<ViewChanged>,
    mut answered: MessageWriter<QueryAnswered>,
    mut rounds: MessageWriter<RoundEvents>,
) {
    while let Some(down) = link.receive() {
        match down {
            Down::Message(ServerMessage::QueryReply { request_id, reply }) => {
                answered.write(QueryAnswered { request_id, reply });
            }
            Down::Message(message) => match view.apply(message) {
                Some(Applied::Changed(c)) => {
                    state.applied += 1;
                    state.awaiting_round = false;
                    changed.write(ViewChanged(c));
                    if let Some(update) = view.update().filter(|u| !u.events.is_empty()) {
                        rounds.write(RoundEvents(update.events.clone()));
                    }
                }
                Some(Applied::Stale) => state.stale += 1,
                Some(Applied::NeedResync) => {
                    state.resyncs += 1;
                    link.send(Up::Client(ClientMessage::Resync));
                }
                // Receipts and errors belong to the Outbox (M3).
                None => {}
            },
            Down::Stopped(reason) => state.stopped = Some(reason),
        }
    }
}

fn pace(
    link: Res<Link>,
    pace: Res<Pace>,
    gate: Option<Res<Gate>>,
    view: Res<ViewState>,
    mut state: ResMut<LinkState>,
) {
    let finished = view.view().is_some_and(|v| v.winner.is_some() || v.result.is_some());
    let open = gate.is_none_or(|g| g.open);
    if *pace == Pace::Continuous && open && view.view().is_some() && !state.awaiting_round && state.stopped.is_none() && !finished {
        state.awaiting_round = true;
        link.send(Up::Advance);
    }
}
