//! The Bevy side of a match: drain the `Link` into `ViewState`, announce what changed, ask for a resync when a
//! delta does not fit, and pace a watched match's rounds.

use crate::link::{Down, Link, Up};
use crate::view_state::{Applied, Changed, ViewState};
use bevy::prelude::*;
use civ_sim::protocol::{Action, ClientMessage, CommandReceipt, CommandRequest, QueryReply, ServerMessage};

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

/// A command's receipt, by the request id the Outbox chose.
#[derive(Message, Debug, Clone)]
pub struct ReceiptArrived {
    pub request_id: String,
    pub receipt: CommandReceipt,
}

/// A host error that answers no particular request.
#[derive(Message, Debug, Clone)]
pub struct HostError(pub String);

/// Commands waiting to go, sent one at a time in order (docs/CLIENT-PROTOCOL.md, Commands and retries): the next
/// leaves when the last one's receipt is in, so finishing the turn cannot overtake a move. Each carries a
/// command id stable across retries.
#[derive(Resource, Default)]
pub struct Outbox {
    queue: std::collections::VecDeque<(String, Action)>,
    in_flight: Option<String>,
    next: u64,
}

impl Outbox {
    /// Queue an action; returns its request id (also its command id).
    pub fn submit(&mut self, action: Action) -> String {
        self.next += 1;
        let id = format!("c{}", self.next);
        self.queue.push_back((id.clone(), action));
        id
    }

    pub fn busy(&self) -> bool {
        self.in_flight.is_some() || !self.queue.is_empty()
    }
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
        .add_message::<ReceiptArrived>()
        .add_message::<HostError>()
        .init_resource::<Outbox>()
        .add_systems(PreUpdate, (receive, pace, send).chain());
}

pub fn receive(
    link: Res<Link>,
    mut view: ResMut<ViewState>,
    mut state: ResMut<LinkState>,
    mut changed: MessageWriter<ViewChanged>,
    mut answered: MessageWriter<QueryAnswered>,
    mut rounds: MessageWriter<RoundEvents>,
    mut receipts: MessageWriter<ReceiptArrived>,
    mut errors: MessageWriter<HostError>,
    mut outbox: ResMut<Outbox>,
) {
    while let Some(down) = link.receive() {
        match down {
            Down::Message(ServerMessage::QueryReply { request_id, reply }) => {
                answered.write(QueryAnswered { request_id, reply });
            }
            Down::Message(ServerMessage::Receipt { request_id, receipt }) => {
                if outbox.in_flight.as_deref() == Some(request_id.as_str()) {
                    outbox.in_flight = None;
                }
                receipts.write(ReceiptArrived { request_id, receipt });
            }
            Down::Message(ServerMessage::Error { code }) => {
                // The host answers a failed command with an error, not a receipt: let the next one go.
                outbox.in_flight = None;
                errors.write(HostError(code));
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

/// Send the next queued command once the last one's receipt is in.
fn send(link: Res<Link>, view: Res<ViewState>, mut outbox: ResMut<Outbox>) {
    if outbox.in_flight.is_some() {
        return;
    }
    let Some(update) = view.update() else { return };
    let Some((id, action)) = outbox.queue.pop_front() else { return };
    let command = CommandRequest {
        command_id: id.clone(),
        epoch: update.cursor.epoch.clone(),
        turn_token: update.turn_token.clone(),
        action,
    };
    link.send(Up::Client(ClientMessage::Command { request_id: id.clone(), command }));
    outbox.in_flight = Some(id);
}
