use civ_server::{router, Config, Host};
use civ_sim::protocol::{
    Action, ClientMessage, CommandRequest, CommandStatus, EntityRef, GameUpdate, ServerMessage,
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
    MaybeTlsStream, WebSocketStream,
};

type Socket = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;
struct Server {
    base: String,
    host: Arc<Host>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Server {
    async fn start(path: &std::path::Path) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let host = Host::open(
            path,
            Config {
                allowed_origins: vec![base.clone()],
                ..Config::default()
            },
        )
        .unwrap();
        let app = router(host.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self { base, host, task }
    }
    async fn guest(&self, client: &reqwest::Client) -> String {
        let response = client
            .post(format!("{}/api/guest", self.base))
            .header("Origin", &self.base)
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let cookie = response.headers()["set-cookie"].to_str().unwrap();
        assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Lax"));
        let cookie = cookie.split(';').next().unwrap().to_owned();
        let body: Value = response.json().await.unwrap();
        assert_eq!(body.as_object().unwrap().len(), 1);
        cookie
    }
    async fn post(
        &self,
        client: &reqwest::Client,
        cookie: &str,
        path: &str,
        body: Value,
    ) -> reqwest::Response {
        client
            .post(format!("{}{path}", self.base))
            .header("Origin", &self.base)
            .header("Cookie", cookie)
            .json(&body)
            .send()
            .await
            .unwrap()
    }
    async fn socket(&self, cookie: &str, id: &str) -> (Socket, GameUpdate) {
        let mut request = format!(
            "{}/api/matches/{id}/ws",
            self.base.replacen("http", "ws", 1)
        )
        .into_client_request()
        .unwrap();
        request
            .headers_mut()
            .insert("Origin", self.base.parse().unwrap());
        request
            .headers_mut()
            .insert("Cookie", cookie.parse().unwrap());
        let (mut socket, _) = connect_async(request).await.unwrap();
        send(
            &mut socket,
            ClientMessage::Hello {
                version: civ_sim::protocol::PROTOCOL_VERSION,
            },
        )
        .await;
        let ServerMessage::Welcome { update, .. } = receive(&mut socket).await else {
            panic!("expected welcome")
        };
        (socket, update)
    }
}
async fn send(socket: &mut Socket, message: ClientMessage) {
    socket
        .send(Message::Text(
            serde_json::to_string(&message).unwrap().into(),
        ))
        .await
        .unwrap();
}
async fn receive(socket: &mut Socket) -> ServerMessage {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let message = socket.next().await.unwrap().unwrap();
            if let Message::Text(text) = message {
                return serde_json::from_str(&text).unwrap();
            }
        }
    })
    .await
    .unwrap()
}
async fn command(
    socket: &mut Socket,
    update: &GameUpdate,
    id: &str,
    action: Action,
) -> civ_sim::protocol::CommandReceipt {
    send(
        socket,
        ClientMessage::Command {
            request_id: id.into(),
            command: CommandRequest {
                command_id: id.into(),
                epoch: update.cursor.epoch.clone(),
                turn_token: update.turn_token.clone(),
                action,
            },
        },
    )
    .await;
    loop {
        if let ServerMessage::Receipt { receipt, .. } = receive(socket).await {
            return receipt;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_real_sockets_are_seat_authorized_fenced_and_share_one_round() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(&dir.path().join("game.sqlite")).await;
    let client = reqwest::Client::new();
    let a = server.guest(&client).await;
    let b = server.guest(&client).await;
    let outsider = server.guest(&client).await;
    let room: Value = server
        .post(
            &client,
            &a,
            "/api/rooms",
            json!({"majors":2,"cityStates":0,"turnSeconds":30}),
        )
        .await
        .json()
        .await
        .unwrap();
    assert!(room.get("seed").is_none() && room.get("settings").is_none());
    let room_id = room["roomId"].as_str().unwrap();
    let joined: Value = server
        .post(
            &client,
            &b,
            "/api/rooms/join",
            json!({"invite":room["invite"]}),
        )
        .await
        .json()
        .await
        .unwrap();
    assert_eq!(joined["ownSeat"], 1);
    assert!(joined.get("invite").is_none());
    assert_eq!(
        server
            .post(
                &client,
                &b,
                &format!("/api/rooms/{room_id}/start"),
                json!({})
            )
            .await
            .status(),
        403
    );
    assert_eq!(
        client
            .get(format!("{}/api/rooms/{room_id}", server.base))
            .header("Cookie", &outsider)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        client
            .post(format!("{}/api/guest", server.base))
            .header("Origin", "http://evil.example")
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        client
            .post(format!("{}/api/guest", server.base))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let started: Value = server
        .post(
            &client,
            &a,
            &format!("/api/rooms/{room_id}/start"),
            json!({}),
        )
        .await
        .json()
        .await
        .unwrap();
    let id = started["matchId"].as_str().unwrap();
    assert_eq!(
        server
            .post(
                &client,
                &outsider,
                "/api/rooms/join",
                json!({"invite":room["invite"]})
            )
            .await
            .status(),
        409
    );
    let mut unauthorized = format!(
        "{}/api/matches/{id}/ws",
        server.base.replacen("http", "ws", 1)
    )
    .into_client_request()
    .unwrap();
    unauthorized
        .headers_mut()
        .insert("Origin", server.base.parse().unwrap());
    unauthorized
        .headers_mut()
        .insert("Cookie", outsider.parse().unwrap());
    assert!(connect_async(unauthorized).await.is_err());
    let (mut first, one) = server.socket(&a, id).await;
    let (mut second, two) = server.socket(&b, id).await;
    assert_eq!((one.view.me, two.view.me), (0, 1));
    assert!(one
        .deadline_ms
        .as_ref()
        .is_some_and(|d| d.parse::<u64>().is_ok()));
    let other = two.view.units.iter().find(|u| u.owner == 1).unwrap();
    let denied = command(
        &mut first,
        &one,
        "foreign-unit",
        Action::SkipUnit {
            unit: EntityRef {
                id: other.id,
                gen: other.gen,
            },
        },
    )
    .await;
    assert!(matches!(denied.result, CommandStatus::Rejected { .. }));
    let (mut replacement, new_one) = server.socket(&a, id).await;
    loop {
        if let ServerMessage::Error { code } = receive(&mut first).await {
            assert_eq!(code, "connectionReplaced");
            break;
        }
    }
    assert_eq!(
        command(&mut replacement, &new_one, "finish-a", Action::FinishTurn)
            .await
            .result,
        CommandStatus::Applied
    );
    assert_eq!(
        command(&mut second, &two, "finish-b", Action::FinishTurn)
            .await
            .result,
        CommandStatus::Applied
    );
    send(&mut replacement, ClientMessage::Resync).await;
    loop {
        if let ServerMessage::Update { update } = receive(&mut replacement).await {
            if update.view.turn == one.view.turn + 1 {
                assert_eq!(update.view.me, 0);
                assert!(!update.ready);
                assert!(update.deadline_ms > one.deadline_ms);
                break;
            }
        }
    }
    let db = rusqlite::Connection::open(dir.path().join("game.sqlite")).unwrap();
    let receipts: i64 = db
        .query_row("SELECT COUNT(*) FROM receipts", [], |r| r.get(0))
        .unwrap();
    assert_eq!(receipts, 3);
    let token = a.split('=').nth(1).unwrap();
    let hash: String = db
        .query_row("SELECT token_hash FROM guests LIMIT 1", [], |r| r.get(0))
        .unwrap();
    assert_ne!(hash, token);
    let stored: String = db
        .query_row("SELECT snapshot FROM matches", [], |r| r.get(0))
        .unwrap();
    let restored = civ_sim::session::Session::restore(&stored).unwrap();
    assert_eq!(restored.turn(), one.view.turn + 1);
    restored.verify_replay().unwrap();
    replacement.close(None).await.unwrap();
    second.close(None).await.unwrap();
    // Keeps the host available until socket close is submitted; no external service is involved.
    assert!(Arc::strong_count(&server.host) >= 1);
}

#[tokio::test]
async fn every_seat_chooses_its_nation_in_the_lobby() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(&dir.path().join("game.sqlite")).await;
    let client = reqwest::Client::new();
    let (a, b) = (server.guest(&client).await, server.guest(&client).await);
    let room: Value = server
        .post(&client, &a, "/api/rooms", json!({"majors":2,"cityStates":0,"turnSeconds":30,"nation":"Rome"}))
        .await
        .json()
        .await
        .unwrap();
    let room_id = room["roomId"].as_str().unwrap().to_owned();
    server.post(&client, &b, "/api/rooms/join", json!({"invite":room["invite"]})).await;
    let seat = |nation: &str| json!({ "nation": nation });
    let path = format!("/api/rooms/{room_id}/seat");
    assert_eq!(server.post(&client, &b, &path, seat("Rome")).await.status(), 409, "Rome is seat 0's");
    assert_eq!(server.post(&client, &b, &path, seat("Atlantis")).await.status(), 400);
    let chosen: Value = server.post(&client, &b, &path, seat("Persia")).await.json().await.unwrap();
    let seats = chosen["seats"].as_array().unwrap();
    assert_eq!(seats[0]["nation"], "Rome");
    assert_eq!(seats[1]["nation"], "Persia");

    let started: Value =
        server.post(&client, &a, &format!("/api/rooms/{room_id}/start"), json!({})).await.json().await.unwrap();
    assert_eq!(server.post(&client, &b, &path, seat("Egypt")).await.status(), 409, "the lobby is closed");
    let (_socket, update) = server.socket(&b, started["matchId"].as_str().unwrap()).await;
    let rules = civ_sim::content::load().unwrap();
    let mine = update.view.civs.iter().find(|c| c.id == update.view.me).unwrap();
    assert_eq!(rules.nations.iter().nth(mine.nation as usize).unwrap().1.name, "Persia");
}

async fn fetch(server: &Server, client: &reqwest::Client, cookie: &str, path: &str) -> Value {
    let response = client.get(format!("{}{path}", server.base)).header("Cookie", cookie).send().await.unwrap();
    assert_eq!(response.status(), 200);
    response.json().await.unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guests_name_themselves_find_their_rooms_and_leave() {
    let dir = tempfile::tempdir().unwrap();
    let server = Server::start(&dir.path().join("game.sqlite")).await;
    let client = reqwest::Client::new();
    let a = server.guest(&client).await;
    let b = server.guest(&client).await;
    let named: Value =
        server.post(&client, &a, "/api/guest/name", json!({"name":"  Ada  "})).await.json().await.unwrap();
    assert_eq!(named["name"], "Ada");
    assert_eq!(server.post(&client, &a, "/api/guest/name", json!({"name":" "})).await.status(), 400);
    let again: Value = server.post(&client, &a, "/api/guest", json!({})).await.json().await.unwrap();
    assert_eq!(again["name"], "Ada", "a returning guest keeps its name");
    let room: Value = server
        .post(&client, &a, "/api/rooms", json!({"majors":3,"cityStates":0,"turnSeconds":30}))
        .await
        .json()
        .await
        .unwrap();
    let room_id = room["roomId"].as_str().unwrap().to_string();
    let joined: Value =
        server.post(&client, &b, "/api/rooms/join", json!({"invite":room["invite"]})).await.json().await.unwrap();
    let members = joined["members"].as_array().unwrap();
    assert_eq!(members.len(), 2);
    assert_eq!((&members[0]["name"], &members[0]["isOwner"]), (&json!("Ada"), &json!(true)));
    assert!(members[1]["name"].is_null() && members[1].get("ready").is_none(), "no live state before the start");
    assert_eq!((&members[0]["connected"], &members[1]["connected"]), (&json!(true), &json!(true)), "both just called for the room");
    let mine = fetch(&server, &client, &b, "/api/rooms").await;
    assert_eq!(mine["rooms"][0]["roomId"], room_id.as_str());
    assert_eq!(mine["rooms"][0]["ownSeat"], 1);
    let started: Value =
        server.post(&client, &a, &format!("/api/rooms/{room_id}/start"), json!({})).await.json().await.unwrap();
    let match_id = started["matchId"].as_str().unwrap().to_string();
    let (_socket, update) = server.socket(&a, &match_id).await;
    let seats = update.seats.unwrap();
    assert_eq!(seats[0].name.as_deref(), Some("Ada"));
    assert_eq!(seats[0].connected, Some(true));
    let view = fetch(&server, &client, &b, &format!("/api/rooms/{room_id}")).await;
    assert_eq!(view["members"][0]["connected"], true);
    assert_eq!(view["members"][1]["ready"], false);
    let mine = fetch(&server, &client, &a, "/api/rooms").await;
    assert_eq!(mine["rooms"][0]["matchId"], match_id.as_str());
    assert!(mine["rooms"][0]["turn"].is_u64());
    assert_eq!(server.post(&client, &b, &format!("/api/rooms/{room_id}/leave"), json!({})).await.status(), 200);
    let mine = fetch(&server, &client, &b, "/api/rooms").await;
    assert_eq!(mine["rooms"], json!([]), "a room it left is not offered again");
    let view = fetch(&server, &client, &a, &format!("/api/rooms/{room_id}")).await;
    assert_eq!(view["members"][1]["left"], true);
}
