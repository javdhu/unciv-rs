// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Rooms against a real civ-server on a free port, behind a TCP relay the test can cut (docs/ROOMS-SPEC.md, How
//! it will be verified): two guests make and join a room, start it and finish a round together through
//! `Link::remote`; a command given while the connection is down is applied once it comes back; after the
//! deadline an old turn's command is refused `staleTurn`; a newer connection for a seat stops the older one, and
//! its own orders are not taken for the old client's.

use bevy::prelude::*;
use civ_client::api::{Answer, Call};
use civ_client::client::{self, LinkState, Outbox, ReceiptArrived};
use civ_client::link::{Link, Status, Up};
use civ_client::net::{Http, Server};
use civ_client::view_state::ViewState;
use civ_sim::protocol::{Action, ClientMessage, CommandRequest, CommandStatus, RejectionCode};
use civ_sim::rooms::RoomRequest;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// civ-server on its own runtime, and a relay in front of it whose connections can be cut.
struct Running {
    relay: String,
    open: Arc<Mutex<Vec<TcpStream>>>,
    _runtime: tokio::runtime::Runtime,
    _dir: tempfile::TempDir,
}

impl Running {
    fn start() -> Running {
        let dir = tempfile::tempdir().unwrap();
        civ_client::net::use_config_dir(dir.path().join("guests"));
        let relay_listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let relay = format!("http://{}", relay_listener.local_addr().unwrap());
        let runtime = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().unwrap();
        let db = dir.path().join("game.sqlite");
        let origin = relay.clone();
        let target = runtime.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let config = civ_server::Config { allowed_origins: vec![origin], ..civ_server::Config::default() };
            let host = civ_server::Host::open(&db, config).unwrap();
            // Deadlines and disconnects are the clock task's, as in civ-server's main.
            tokio::spawn(host.clone().run_clock());
            tokio::spawn(async move { axum::serve(listener, civ_server::router(host)).await.unwrap() });
            address
        });
        let open = Arc::new(Mutex::new(Vec::new()));
        let kept = open.clone();
        std::thread::spawn(move || {
            for client in relay_listener.incoming().flatten() {
                let Ok(server) = TcpStream::connect(target) else { continue };
                kept.lock().unwrap().extend([client.try_clone().unwrap(), server.try_clone().unwrap()]);
                let (c2, s2) = (client.try_clone().unwrap(), server.try_clone().unwrap());
                std::thread::spawn(move || pipe(client, server));
                std::thread::spawn(move || pipe(s2, c2));
            }
        });
        Running { relay, open, _runtime: runtime, _dir: dir }
    }

    /// Drop every connection through the relay, as a network would.
    fn cut(&self) {
        for stream in self.open.lock().unwrap().drain(..) {
            let _ = stream.shutdown(Shutdown::Both);
        }
    }
}

fn pipe(mut from: TcpStream, mut to: TcpStream) {
    let mut buf = [0u8; 16 * 1024];
    while let Ok(n) = from.read(&mut buf) {
        if n == 0 || to.write_all(&buf[..n]).is_err() {
            break;
        }
    }
    let _ = to.shutdown(Shutdown::Both);
}

fn wait<T>(what: &str, mut done: impl FnMut() -> Option<T>) -> T {
    let started = Instant::now();
    loop {
        if let Some(t) = done() {
            return t;
        }
        assert!(started.elapsed() < Duration::from_secs(60), "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn call(http: &Http, call: Call) -> Answer {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    call.send(http, id);
    let reply = wait("an HTTP reply", || http.receive().filter(|r| r.id == id));
    call.read(&reply).unwrap_or_else(|e| panic!("{call:?}: {e:?}"))
}

#[derive(Resource, Default)]
struct Seen {
    receipts: Vec<ReceiptArrived>,
}

fn collect(mut receipts: MessageReader<ReceiptArrived>, mut seen: ResMut<Seen>) {
    seen.receipts.extend(receipts.read().cloned());
}

fn player(server: Server, match_id: &str) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(client::plugin)
        .init_resource::<Seen>()
        .add_systems(Update, collect)
        .insert_resource(Link::remote(server, match_id.into()));
    app
}

fn turn(app: &App) -> Option<u32> {
    app.world().resource::<ViewState>().view().map(|v| v.turn)
}

fn receipt(app: &App, id: &str) -> Option<CommandStatus> {
    app.world().resource::<Seen>().receipts.iter().find(|r| r.request_id == id).map(|r| r.receipt.result.clone())
}

fn pump(apps: &mut [&mut App]) {
    for app in apps.iter_mut() {
        app.update();
    }
}

#[test]
fn two_guests_play_a_room_through_cuts_deadlines_and_a_second_connection() {
    let running = Running::start();
    let (a, b) = (Server::new(&running.relay, "a"), Server::new(&running.relay, "b"));
    let (http_a, http_b) = (Http::new(a.clone()), Http::new(b.clone()));

    // A makes a room; B joins with its invite; A starts it.
    assert!(matches!(call(&http_a, Call::Guest), Answer::Guest(_)));
    assert!(matches!(call(&http_a, Call::Rename("Ada".into())), Answer::Guest(g) if g.name.as_deref() == Some("Ada")));
    let settings = RoomRequest { width: 24, height: 16, majors: 2, city_states: 0, turn_seconds: 5, ..RoomRequest::default() };
    let Answer::Room(room) = call(&http_a, Call::Create(settings)) else { panic!("a room") };
    let invite = room.invite.clone().expect("the owner sees the invite");
    assert!(matches!(call(&http_b, Call::Guest), Answer::Guest(_)));
    let Answer::Room(joined) = call(&http_b, Call::Join(invite)) else { panic!("joined") };
    assert_eq!((joined.room_id.as_str(), joined.own_seat), (room.room_id.as_str(), 1));
    let Answer::Room(started) = call(&http_a, Call::Start(room.room_id.clone())) else { panic!("started") };
    let match_id = started.match_id.expect("a started room has its match");
    let Answer::Rooms(listed) = call(&http_b, Call::Rooms) else { panic!("rooms") };
    assert_eq!(listed.rooms[0].match_id.as_deref(), Some(match_id.as_str()));

    // Both seats finish the turn: the round closes for both.
    let (mut pa, mut pb) = (player(a.clone(), &match_id), player(b.clone(), &match_id));
    wait("both welcomes", || {
        pump(&mut [&mut pa, &mut pb]);
        (turn(&pa).is_some() && turn(&pb).is_some()).then_some(())
    });
    let start = turn(&pa).unwrap();
    for app in [&mut pa, &mut pb] {
        app.world_mut().resource_mut::<Outbox>().submit(Action::FinishTurn);
    }
    wait("the next turn for both", || {
        pump(&mut [&mut pa, &mut pb]);
        (turn(&pa) > Some(start) && turn(&pb) > Some(start)).then_some(())
    });

    // A's connection is cut and an order given while it is down; it goes once the socket is back.
    running.cut();
    wait("A to notice", || {
        pump(&mut [&mut pa]);
        matches!(pa.world().resource::<LinkState>().connection, Status::Retrying(_)).then_some(())
    });
    let research = pa.world_mut().resource_mut::<Outbox>().submit(Action::SetResearch { technology: 0 });
    let status = wait("the order's receipt after the reconnect", || {
        pump(&mut [&mut pa, &mut pb]);
        receipt(&pa, &research)
    });
    assert!(matches!(status, CommandStatus::Applied | CommandStatus::Rejected { .. }), "{status:?}");
    assert_eq!(pa.world().resource::<LinkState>().connection, Status::Live);
    assert_eq!(pa.world().resource::<Seen>().receipts.iter().filter(|r| r.request_id == research).count(), 1);

    // Nobody finishes; the deadline closes the round; a command with the old turn's token is refused.
    let old = pa.world().resource::<ViewState>().update().unwrap().clone();
    wait("the deadline's round", || {
        pump(&mut [&mut pa, &mut pb]);
        (turn(&pa) > Some(old.view.turn)).then_some(())
    });
    let stale = CommandRequest {
        command_id: "stale".into(),
        epoch: old.cursor.epoch.clone(),
        turn_token: old.turn_token.clone(),
        action: Action::FinishTurn,
    };
    pa.world().resource::<Link>().send(Up::Client(ClientMessage::Command { request_id: "stale".into(), command: stale }));
    let refused = wait("the stale receipt", || {
        pump(&mut [&mut pa]);
        receipt(&pa, "stale")
    });
    assert_eq!(refused, CommandStatus::Rejected { code: RejectionCode::StaleTurn });

    // A second client opens A's seat: the first is told and stops for good.
    let mut again = player(a, &match_id);
    wait("the older connection to stop", || {
        pump(&mut [&mut pa, &mut again]);
        pa.world().resource::<LinkState>().stopped.clone()
    });
    assert_eq!(pa.world().resource::<LinkState>().stopped.as_deref(), Some("connectionReplaced"));
    wait("the newer one to be welcomed", || {
        pump(&mut [&mut again]);
        turn(&again)
    });
    // A new client for the seat (a reload, a restart) gives orders under ids of its own, not the old client's.
    let order = again.world_mut().resource_mut::<Outbox>().submit(Action::FinishTurn);
    let status = wait("the new client's order", || {
        pump(&mut [&mut again]);
        receipt(&again, &order)
    });
    assert_ne!(status, CommandStatus::Rejected { code: RejectionCode::CommandIdReused });
}
