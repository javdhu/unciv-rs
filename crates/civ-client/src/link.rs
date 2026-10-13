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

fn watch(settings: Settings, sight: Sight, up: &ConcurrentQueue<Up>, down: &ConcurrentQueue<Down>) -> Result<(), String> {
    let epoch = format!("watch-{}", settings.seed);
    let mut session = Session::watch(settings, epoch, sight).map_err(|e| e.to_string())?;
    let welcome = |session: &Session| -> Result<ServerMessage, String> {
        Ok(ServerMessage::Welcome {
            version: PROTOCOL_VERSION,
            content_hash: session.content_hash(),
            update: session.current_update(0).map_err(|e| e.to_string())?,
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
                for (seat, update) in transition.updates {
                    if seat == 0 {
                        send(session.update_message(0, update).map_err(|e| e.to_string())?);
                    }
                }
                session = transition.next;
            }
            Up::Client(ClientMessage::Hello { .. }) => send(welcome(&session)?),
            Up::Client(ClientMessage::Resync) => send(ServerMessage::Update {
                update: session.current_update(0).map_err(|e| e.to_string())?,
            }),
            Up::Client(ClientMessage::Query { request_id, query }) => match session.query(0, query) {
                Ok(reply) => send(ServerMessage::QueryReply { request_id, reply }),
                Err(_) => send(ServerMessage::Error { code: "query".into() }),
            },
            // A watched match takes no commands.
            Up::Client(ClientMessage::Command { .. }) => send(ServerMessage::Error { code: "watching".into() }),
        }
    }
}
