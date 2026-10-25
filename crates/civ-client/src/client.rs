// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The Bevy side of a match: drain the `Link` into `ViewState`, announce what changed, ask for a resync when a
//! delta does not fit, and pace a watched match's rounds.

use crate::link::{Down, Link, Status, Up};
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

/// The solo host's save at a turn's end, for `saves.rs` to keep.
#[derive(Message, Debug, Clone)]
pub struct Autosaved(pub Vec<u8>);

/// A host error that answers no particular request.
#[derive(Message, Debug, Clone)]
pub struct HostError(pub String);

/// Commands waiting to go, sent one at a time in order (docs/CLIENT-PROTOCOL.md, Commands and retries): the next
/// leaves when the last one's receipt is in, so finishing the turn cannot overtake a move. Each carries a
/// command id stable across retries and unique to this Outbox: a resumed save or a rejoined room already holds
/// receipts for another client's ids, and refuses a reused one.
#[derive(Resource)]
pub struct Outbox {
    /// This Outbox's own id prefix: the clock in milliseconds, and a count for two made in the same one.
    prefix: String,
    queue: std::collections::VecDeque<(String, Action)>,
    /// Sent, its receipt not yet in: sent again exactly as it was after every welcome (a reconnect), so the
    /// server applies it once and answers with the receipt it stored.
    in_flight: Option<(String, CommandRequest)>,
    next: u64,
}

impl Default for Outbox {
    fn default() -> Self {
        static MADE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let made = MADE.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        #[cfg(not(target_arch = "wasm32"))]
        let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as u64);
        #[cfg(target_arch = "wasm32")]
        let ms = js_sys::Date::now() as u64;
        Outbox { prefix: format!("{ms:x}{made:x}"), queue: Default::default(), in_flight: None, next: 0 }
    }
}

impl Outbox {
    /// Queue an action; returns its request id (also its command id).
    pub fn submit(&mut self, action: Action) -> String {
        self.next += 1;
        let id = format!("c{}-{}", self.prefix, self.next);
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
    /// A remote host's connection; always live in-process.
    pub connection: Status,
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
        .add_message::<Autosaved>()
        .init_resource::<Outbox>()
        .add_systems(PreUpdate, (pump, receive, pace, send).chain().in_set(crate::app::InMatch))
        .add_systems(
            OnExit(crate::app::AppState::Match),
            (crate::app::reset::<ViewState>, crate::app::reset::<LinkState>, crate::app::reset::<Outbox>),
        );
}

/// A room's socket lives on the page's thread in the browser; natively it has its own.
fn pump(_link: Res<Link>) {
    #[cfg(target_arch = "wasm32")]
    _link.pump();
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
    mut saves: MessageWriter<Autosaved>,
    mut outbox: ResMut<Outbox>,
) {
    while let Some(down) = link.receive() {
        match down {
            Down::Message(ServerMessage::QueryReply { request_id, reply }) => {
                answered.write(QueryAnswered { request_id, reply });
            }
            Down::Message(ServerMessage::Receipt { request_id, receipt }) => {
                if outbox.in_flight.as_ref().is_some_and(|(id, _)| *id == request_id) {
                    outbox.in_flight = None;
                }
                receipts.write(ReceiptArrived { request_id, receipt });
            }
            Down::Message(ServerMessage::Error { code }) => {
                // The in-process host answers a failed command with an error, not a receipt: let the next one
                // go. A server's errors are about the connection, and the command goes again after the welcome.
                if !link.is_remote() {
                    outbox.in_flight = None;
                }
                errors.write(HostError(code));
            }
            Down::Message(message) => match (matches!(message, ServerMessage::Welcome { .. }), view.apply(message)) {
                (welcome, Some(Applied::Changed(c))) => {
                    if welcome {
                        if let Some((id, command)) = &outbox.in_flight {
                            link.send(Up::Client(ClientMessage::Command { request_id: id.clone(), command: command.clone() }));
                        }
                    }
                    state.applied += 1;
                    state.awaiting_round = false;
                    changed.write(ViewChanged(c));
                    if let Some(update) = view.update() {
                        let presented = presented(update);
                        if !presented.is_empty() {
                            rounds.write(RoundEvents(presented));
                        }
                    }
                }
                (_, Some(Applied::Stale)) => state.stale += 1,
                (_, Some(Applied::NeedResync)) => {
                    state.resyncs += 1;
                    link.send(Up::Client(ClientMessage::Resync));
                }
                (_, None) => {}
            },
            Down::Status(status) => state.connection = status,
            Down::Saved(save) => {
                saves.write(Autosaved(save));
            }
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
    link.send(Up::Client(ClientMessage::Command { request_id: id.clone(), command: command.clone() }));
    outbox.in_flight = Some((id, command));
}

/// What presentation plays for an update: the other civs' turns (replay batches, in order) and then this seat's
/// own events. Battles this seat fought are in both (docs/CLIENT-PROTOCOL.md), so with a replay they come from it.
fn presented(update: &civ_sim::protocol::GameUpdate) -> Vec<civ_sim::protocol::PresentedEvent> {
    use civ_sim::protocol::PresentedEvent;
    let mut out = Vec::new();
    for (b, batch) in update.replay.iter().flatten().enumerate() {
        for (i, event) in batch.events.iter().enumerate() {
            out.push(PresentedEvent { id: format!("replay:{}:{b}:{i}", update.cursor.revision), event: event.clone() });
        }
    }
    let replayed = !out.is_empty();
    out.extend(
        update
            .events
            .iter()
            .filter(|e| !(replayed && matches!(e.event.event, civ_sim::PresentationEvent::Attacked { .. })))
            .cloned(),
    );
    out
}
