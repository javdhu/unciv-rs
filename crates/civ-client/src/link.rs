// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The one path between the Bevy world and a match host (docs/CLIENT-BRIEF.md, component 1).
//!
//! Messages are civ-sim's own wire types, so a room's remote host (`Link::remote`, `net.rs`) sits behind the
//! same queues. The solo host
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
    /// The client left the match: the host stops. Sent when the `Link` is dropped.
    Close,
}

/// From the host.
pub enum Down {
    Message(ServerMessage),
    /// A remote host's connection came up or was lost.
    Status(Status),
    /// The solo host saved the match at a turn's end (`Session::save`).
    Saved(Vec<u8>),
    /// The host stopped and sends nothing more: the match is over, the host failed, or the server will not
    /// take this client back.
    Stopped(String),
}

/// A remote host's connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    Live,
    /// Lost, about to try again for the nth time in a row.
    Retrying(u32),
}

#[derive(bevy::prelude::Resource)]
pub struct Link {
    up: Arc<ConcurrentQueue<Up>>,
    down: Arc<ConcurrentQueue<Down>>,
    host: Arc<OnceLock<std::thread::Thread>>,
    /// A room's server and match, for a remote host.
    remote: Option<(crate::net::Server, String)>,
}

impl Link {
    fn new(remote: Option<(crate::net::Server, String)>) -> Link {
        Link {
            up: Arc::new(ConcurrentQueue::unbounded()),
            down: Arc::new(ConcurrentQueue::unbounded()),
            host: Arc::new(OnceLock::new()),
            remote,
        }
    }

    /// Play a seat in a civ-server room. The socket says hello and the server welcomes; a lost socket is tried
    /// again (`net::Retry`), and every new connection starts from a full welcome.
    pub fn remote(server: crate::net::Server, match_id: String) -> Link {
        let link = Link::new(Some((server.clone(), match_id.clone())));
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (up, down, host) = (link.up.clone(), link.down.clone(), link.host.clone());
            thread::spawn("socket", move || {
                let _ = host.set(std::thread::current());
                let reason = crate::net::run_socket(server, match_id, &up, &down);
                let _ = down.push(Down::Stopped(reason));
            });
        }
        link
    }

    pub fn is_remote(&self) -> bool {
        self.remote.is_some()
    }

    /// Drive a remote host's socket in the browser, where it lives on the page's thread.
    #[cfg(target_arch = "wasm32")]
    pub fn pump(&self) {
        if let Some((server, match_id)) = &self.remote {
            crate::net::web::pump(server, match_id, &self.up, &self.down);
        }
    }

    /// Watch bots play one match in-process. The host sends its welcome unasked.
    pub fn watch(settings: Settings, sight: Sight) -> Link {
        let link = Link::new(None);
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

    /// Play a match against bots in-process, on seat 0. The host stages each command and answers with its
    /// receipt and the updates it caused; finishing the turn closes the round.
    pub fn play(settings: Settings) -> Link {
        Link::solo(Solo::New(settings))
    }

    /// Play on from a saved solo match. A save from another build is refused, and the host stops with why.
    pub fn resume(save: Vec<u8>) -> Link {
        Link::solo(Solo::Resume(save))
    }

    fn solo(start: Solo) -> Link {
        let link = Link::new(None);
        let (up, down, host) = (link.up.clone(), link.down.clone(), link.host.clone());
        thread::spawn("host", move || {
            let _ = host.set(std::thread::current());
            let reason = match play(start, &up, &down) {
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

impl Drop for Link {
    fn drop(&mut self) {
        self.send(Up::Close);
        #[cfg(target_arch = "wasm32")]
        if self.remote.is_some() {
            crate::net::web::close();
        }
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
            Up::Close => return Ok(()),
        }
    }
}

/// How a solo match begins.
enum Solo {
    New(Settings),
    Resume(Vec<u8>),
}

fn play(start: Solo, up: &ConcurrentQueue<Up>, down: &ConcurrentQueue<Down>) -> Result<(), String> {
    let mut session = match start {
        Solo::New(settings) => {
            let epoch = format!("play-{}", settings.seed);
            Session::new(settings, epoch, vec![0]).map_err(|e| e.to_string())?
        }
        Solo::Resume(save) => Session::restore(&save).map_err(|e| format!("this save cannot be opened by this build ({e})"))?,
    };
    let send = |message| {
        let _ = down.push(Down::Message(message));
    };
    let welcome = |session: &Session| -> Result<ServerMessage, String> {
        Ok(ServerMessage::Welcome {
            version: PROTOCOL_VERSION,
            content_hash: session.content_hash(),
            update: session.current_update(0).map_err(|e| e.to_string())?,
        })
    };
    send(welcome(&session)?);
    loop {
        let Ok(message) = up.pop() else {
            std::thread::park();
            continue;
        };
        match message {
            Up::Client(ClientMessage::Command { request_id, command }) => match session.stage_command(0, command) {
                Ok(transition) => {
                    send(ServerMessage::Receipt { request_id, receipt: transition.receipt.clone() });
                    for (seat, update) in transition.updates {
                        if seat == 0 {
                            send(session.update_message(0, update).map_err(|e| e.to_string())?);
                        }
                    }
                    let turn = session.turn();
                    session = transition.next;
                    // Autosave when a round closes (docs/ACTIONS-SPEC.md, A3).
                    if session.turn() > turn {
                        if let Ok(save) = session.save() {
                            let _ = down.push(Down::Saved(save));
                        }
                    }
                }
                Err(e) => send(ServerMessage::Error { code: e.to_string() }),
            },
            Up::Client(ClientMessage::Query { request_id, query }) => match session.query(0, query) {
                Ok(reply) => send(ServerMessage::QueryReply { request_id, reply }),
                Err(_) => send(ServerMessage::Error { code: "query".into() }),
            },
            Up::Client(ClientMessage::Resync) => send(ServerMessage::Update {
                update: session.current_update(0).map_err(|e| e.to_string())?,
            }),
            Up::Client(ClientMessage::Hello { .. }) => send(welcome(&session)?),
            // A played match's rounds close when its seat finishes; it has no other eyes.
            Up::Advance | Up::Watch(_) => {}
            Up::Close => return Ok(()),
        }
    }
}
