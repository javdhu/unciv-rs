//! The one path between the Bevy world and a match host (docs/CLIENT-BRIEF.md, component 1).
//!
//! Messages are civ-sim's own wire types, so a remote host (M5) slots in behind the same queues. The solo host
//! is a `Session` on its own thread (`thread::spawn`). Both directions are lock-free queues: the browser's
//! main thread must never wait on a lock, so the Bevy side only pushes, pops and unparks; the host thread
//! parks when it has nothing to do.

use crate::thread;
use civ_sim::protocol::{ClientMessage, ServerMessage, PROTOCOL_VERSION};
use civ_sim::session::{Session, Sight};
use civ_sim::Settings;
use concurrent_queue::ConcurrentQueue;
use std::sync::{Arc, OnceLock};

/// To the host.
pub enum Up {
    Client(ClientMessage),
    /// A watched match closes its next round. Watched rounds are the host's to close; the client paces them.
    Advance,
    /// Watch the same match through other eyes: the host replays it to the current turn under the new sight
    /// (the engine is deterministic) and welcomes the client again under a new epoch.
    Watch(Sight),
}

/// From the host.
pub enum Down {
    Message(ServerMessage),
    /// The host stopped and sends nothing more: the match is over or the host failed.
    Stopped(String),
}

#[derive(bevy::prelude::Resource)]
pub struct Link {
    up: Arc<ConcurrentQueue<Up>>,
    down: Arc<ConcurrentQueue<Down>>,
    host: Arc<OnceLock<std::thread::Thread>>,
}

impl Link {
    /// Watch bots play one match in-process. The host sends its welcome unasked.
    pub fn watch(settings: Settings, sight: Sight) -> Link {
        let link = Link {
            up: Arc::new(ConcurrentQueue::unbounded()),
            down: Arc::new(ConcurrentQueue::unbounded()),
            host: Arc::new(OnceLock::new()),
        };
        let (up, down, host) = (link.up.clone(), link.down.clone(), link.host.clone());
        thread::spawn("host", move || {
            let _ = host.set(std::thread::current());
            let reason = match watch(settings, sight, &up, &down) {
                Ok(()) => "finished".to_string(),
                Err(e) => e,
            };
            let _ = down.push(Down::Stopped(reason));
        });
        link
    }

    pub fn send(&self, message: Up) {
        let _ = self.up.push(message);
        if let Some(host) = self.host.get() {
            host.unpark();
        }
    }

    pub fn receive(&self) -> Option<Down> {
        self.down.pop().ok()
    }
}

fn epoch(settings: &Settings, sight: Sight) -> String {
    let eyes = match sight {
        Sight::All => "all".to_string(),
        Sight::Seat => "seat".to_string(),
        Sight::Civ(civ) => format!("civ{civ}"),
    };
    format!("watch-{}-{eyes}", settings.seed)
}

fn watch(settings: Settings, sight: Sight, up: &ConcurrentQueue<Up>, down: &ConcurrentQueue<Down>) -> Result<(), String> {
    let mut session = Session::watch(settings.clone(), epoch(&settings, sight), sight).map_err(|e| e.to_string())?;
    // The watcher's seat: seat 0, or the civ watched through.
    let seat = |session: &Session| session.seats().next().unwrap_or(0);
    let welcome = |session: &Session| -> Result<ServerMessage, String> {
        Ok(ServerMessage::Welcome {
            version: PROTOCOL_VERSION,
            content_hash: session.content_hash(),
            update: session.current_update(seat(session)).map_err(|e| e.to_string())?,
        })
    };
    let send = |message| {
        let _ = down.push(Down::Message(message));
    };
    send(welcome(&session)?);
    loop {
        let Ok(message) = up.pop() else {
            if session.finished() {
                return Ok(());
            }
            std::thread::park();
            continue;
        };
        match message {
            Up::Advance => {
                let Some(transition) = session.stage_round().map_err(|e| e.to_string())? else { continue };
                let watcher = seat(&session);
                for (s, update) in transition.updates {
                    if s == watcher {
                        send(session.update_message(watcher, update).map_err(|e| e.to_string())?);
                    }
                }
                session = transition.next;
            }
            Up::Watch(sight) => {
                let turn = session.turn();
                let mut again =
                    Session::watch(settings.clone(), epoch(&settings, sight), sight).map_err(|e| e.to_string())?;
                while again.turn() < turn {
                    let Some(t) = again.stage_round().map_err(|e| e.to_string())? else { break };
                    again = t.next;
                }
                session = again;
                send(welcome(&session)?);
            }
            Up::Client(ClientMessage::Hello { .. }) => send(welcome(&session)?),
            Up::Client(ClientMessage::Resync) => send(ServerMessage::Update {
                update: session.current_update(seat(&session)).map_err(|e| e.to_string())?,
            }),
            Up::Client(ClientMessage::Query { request_id, query }) => match session.query(seat(&session), query) {
                Ok(reply) => send(ServerMessage::QueryReply { request_id, reply }),
                Err(_) => send(ServerMessage::Error { code: "query".into() }),
            },
            // A watched match takes no commands.
            Up::Client(ClientMessage::Command { .. }) => send(ServerMessage::Error { code: "watching".into() }),
        }
    }
}
