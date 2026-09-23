//! The deployed binary stops on SIGTERM (Fly.io, Docker) as it does on Ctrl-C: drains within a
//! bound even with a match socket open, flushes the store, and exits 0.
#![cfg(unix)]
use civ_sim::protocol::{ClientMessage, ServerMessage, PROTOCOL_VERSION};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader, Read},
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use tokio_tungstenite::{
    connect_async,
    tungstenite::{client::IntoClientRequest, Message},
};

const ORIGIN: &str = "http://127.0.0.1:5199";

async fn post(
    client: &reqwest::Client,
    base: &str,
    cookie: Option<&str>,
    path: &str,
    body: Value,
) -> reqwest::Response {
    let mut request = client
        .post(format!("{base}{path}"))
        .header("Origin", ORIGIN)
        .json(&body);
    if let Some(cookie) = cookie {
        request = request.header("Cookie", cookie);
    }
    request.send().await.unwrap()
}

fn stop(signal: &str) {
    let dir = tempfile::tempdir().unwrap();
    let database = dir.path().join("host.sqlite3");
    let mut child = Command::new(env!("CARGO_BIN_EXE_civ-server"))
        .env("CIV_BIND", "127.0.0.1:0")
        .env("CIV_DATABASE", &database)
        .env_remove("CIV_ALLOWED_ORIGINS")
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stderr = BufReader::new(child.stderr.take().unwrap());
    let mut line = String::new();
    stderr.read_line(&mut line).unwrap();
    let base = line.trim().rsplit(' ').next().unwrap().to_owned();
    assert!(base.starts_with("http://127.0.0.1:"), "{line}");

    let runtime = tokio::runtime::Runtime::new().unwrap();
    // Leave a live match socket open: the drain must not wait for it forever.
    let _socket = runtime.block_on(async {
        let client = reqwest::Client::new();
        let guest = post(&client, &base, None, "/api/guest", json!({})).await;
        let cookie = guest.headers()["set-cookie"].to_str().unwrap();
        let cookie = cookie.split(';').next().unwrap().to_owned();
        let room: Value = post(
            &client,
            &base,
            Some(&cookie),
            "/api/rooms",
            json!({"majors":2,"cityStates":0,"turnSeconds":30}),
        )
        .await
        .json()
        .await
        .unwrap();
        let path = format!("/api/rooms/{}/start", room["roomId"].as_str().unwrap());
        let started: Value = post(&client, &base, Some(&cookie), &path, json!({}))
            .await
            .json()
            .await
            .unwrap();
        let url = format!(
            "{}/api/matches/{}/ws",
            base.replacen("http", "ws", 1),
            started["matchId"].as_str().unwrap()
        );
        let mut request = url.into_client_request().unwrap();
        request
            .headers_mut()
            .insert("Origin", ORIGIN.parse().unwrap());
        request
            .headers_mut()
            .insert("Cookie", cookie.parse().unwrap());
        let (mut socket, _) = connect_async(request).await.unwrap();
        let hello = ClientMessage::Hello {
            version: PROTOCOL_VERSION,
        };
        socket
            .send(Message::Text(serde_json::to_string(&hello).unwrap().into()))
            .await
            .unwrap();
        loop {
            if let Message::Text(text) = socket.next().await.unwrap().unwrap() {
                let message: ServerMessage = serde_json::from_str(&text).unwrap();
                assert!(matches!(message, ServerMessage::Welcome { .. }));
                break socket;
            }
        }
    });

    let asked = Instant::now();
    let status = Command::new("kill")
        .args([signal, &child.id().to_string()])
        .status()
        .unwrap();
    assert!(status.success());
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(
            asked.elapsed() < Duration::from_secs(9),
            "host did not stop inside Fly's kill_timeout"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    let mut rest = String::new();
    stderr.read_to_string(&mut rest).unwrap();
    assert!(status.success(), "{status}: {rest}");
    assert!(rest.contains("store flushed"), "{rest}");

    // The WAL was folded back: the database file alone holds the match.
    let wal = dir.path().join("host.sqlite3-wal");
    assert!(std::fs::metadata(&wal).map_or(true, |m| m.len() == 0));
    let db = rusqlite::Connection::open(&database).unwrap();
    let matches: i64 = db
        .query_row("SELECT count(*) FROM matches", [], |r| r.get(0))
        .unwrap();
    assert_eq!(matches, 1);
}

#[test]
fn sigterm_flushes_the_store_and_exits_zero() {
    stop("-TERM");
}

#[test]
fn ctrl_c_still_does() {
    stop("-INT");
}
