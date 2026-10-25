// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The network under rooms (docs/ROOMS-SPEC.md, R2, R3, R6): the server's address, the retry policy, HTTP for
//! the rooms API and the match socket.
//!
//! Natively, HTTP (ureq) and the socket (tungstenite) each run on their own thread and the guest token is a
//! file per server, sent as the `civ_guest` cookie with the server's own `Origin`, as a page served by it
//! would. In the browser the page shares the server's origin, so fetch and WebSocket carry the cookie the
//! browser keeps; both run on the page's thread through callbacks. Either way the Bevy side only pushes to and
//! pops from lock-free queues.

use crate::link::{Down, Status, Up};
use civ_sim::protocol::{ClientMessage, ServerMessage, PROTOCOL_VERSION};
use concurrent_queue::ConcurrentQueue;
use std::sync::Arc;

/// A civ-server, by the base URL its page and API share (`http://127.0.0.1:3001`, no trailing slash), and,
/// natively, which of this user's guests plays on it (`CIV_PROFILE`; two clients on one machine are two
/// people only with two profiles).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Server {
    pub base: String,
    pub profile: String,
}

impl Server {
    /// `CIV_SERVER` natively (civ-server's default address otherwise); the page's own origin in the browser.
    pub fn from_env() -> Server {
        #[cfg(not(target_arch = "wasm32"))]
        let base = std::env::var("CIV_SERVER").unwrap_or_else(|_| "http://127.0.0.1:3001".into());
        #[cfg(target_arch = "wasm32")]
        let base = web_sys::window().and_then(|w| w.location().origin().ok()).unwrap_or_default();
        #[cfg(not(target_arch = "wasm32"))]
        let profile = std::env::var("CIV_PROFILE").unwrap_or_default();
        #[cfg(target_arch = "wasm32")]
        let profile = String::new();
        Server { base: base.trim_end_matches('/').to_string(), profile }
    }

    pub fn new(base: &str, profile: &str) -> Server {
        Server { base: base.trim_end_matches('/').to_string(), profile: profile.to_string() }
    }

    /// The key its guest token is kept under.
    #[cfg(not(target_arch = "wasm32"))]
    fn guest_key(&self) -> String {
        if self.profile.is_empty() {
            self.origin()
        } else {
            format!("{} {}", self.origin(), self.profile)
        }
    }

    /// The `Origin` a page served by this server sends.
    pub fn origin(&self) -> String {
        let (scheme, rest) = self.base.split_once("://").unwrap_or(("http", &self.base));
        let host = rest.split('/').next().unwrap_or(rest);
        format!("{scheme}://{host}")
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    pub fn socket_url(&self, match_id: &str) -> String {
        let base = self.base.replacen("https://", "wss://", 1).replacen("http://", "ws://", 1);
        format!("{base}/api/matches/{match_id}/ws")
    }
}

/// When to try a lost socket again: 250 ms doubling to 10 s with jitter, at most `TRIES` in a row; the count
/// starts over once a connection has stayed up for 30 s.
#[derive(Default, Debug)]
pub struct Retry {
    tries: u32,
    up_since: Option<f64>,
}

impl Retry {
    pub const TRIES: u32 = 8;

    /// A welcome arrived at `now` (seconds).
    pub fn connected(&mut self, now: f64) {
        self.up_since = Some(now);
    }

    /// The connection was lost at `now`: how long to wait before the next try, or `None` when the tries are
    /// spent. `jitter` is uniform in [0, 1).
    pub fn lost(&mut self, now: f64, jitter: f64) -> Option<f64> {
        if self.up_since.take().is_some_and(|since| now - since >= 30.0) {
            self.tries = 0;
        }
        if self.tries >= Self::TRIES {
            return None;
        }
        let base = (0.25 * 2f64.powi(self.tries as i32)).min(10.0);
        self.tries += 1;
        Some(base * (0.5 + 0.5 * jitter))
    }

    pub fn tries(&self) -> u32 {
        self.tries
    }
}

/// Server error codes after which the socket is not tried again (`http.rs`): another connection holds the
/// seat, or this client speaks another protocol.
pub fn terminal(code: &str) -> bool {
    matches!(code, "connectionReplaced" | "protocolVersion")
}

/// A pseudo-random number in [0, 1) for jitter; the clock's low bits are enough.
fn jitter() -> f64 {
    let nanos = now_secs().fract() * 1e9;
    (nanos as u64).wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407) as f64 / u64::MAX as f64
}

fn now_secs() -> f64 {
    #[cfg(not(target_arch = "wasm32"))]
    return std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0.0, |d| d.as_secs_f64());
    #[cfg(target_arch = "wasm32")]
    return js_sys::Date::now() / 1000.0;
}

fn hello() -> String {
    serde_json::to_string(&ClientMessage::Hello { version: PROTOCOL_VERSION }).expect("hello serializes")
}

/// One HTTP exchange's outcome: the status (0 when the server could not be reached), the body, and the
/// server's clock from its `Date` header (Unix seconds).
#[derive(Debug, Clone)]
pub struct HttpReply {
    pub id: u64,
    pub status: u16,
    pub body: String,
    pub date: Option<f64>,
}

/// An HTTP date (`Tue, 13 Oct 2026 16:09:52 GMT`) in Unix seconds.
pub fn http_date(s: &str) -> Option<f64> {
    let parts: Vec<&str> = s.split_whitespace().collect();
    let [_, day, month, year, time, "GMT"] = parts.as_slice() else { return None };
    let month = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"]
        .iter()
        .position(|m| m == month)? as i64
        + 1;
    let (day, year): (i64, i64) = (day.parse().ok()?, year.parse().ok()?);
    let mut hms = time.split(':').map(|p| p.parse::<i64>());
    let (h, m, sec) = (hms.next()?.ok()?, hms.next()?.ok()?, hms.next()?.ok()?);
    // Days from the civil date (Howard Hinnant's algorithm).
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    Some((days * 86400 + h * 3600 + m * 60 + sec) as f64)
}

/// HTTP to one server, answered through a queue.
#[derive(Clone)]
pub struct Http {
    server: Server,
    replies: Arc<ConcurrentQueue<HttpReply>>,
    #[cfg(not(target_arch = "wasm32"))]
    requests: Arc<ConcurrentQueue<(u64, &'static str, String, Option<String>)>>,
    #[cfg(not(target_arch = "wasm32"))]
    worker: Arc<std::sync::OnceLock<std::thread::Thread>>,
}

impl Http {
    pub fn new(server: Server) -> Http {
        let http = Http {
            server,
            replies: Arc::new(ConcurrentQueue::unbounded()),
            #[cfg(not(target_arch = "wasm32"))]
            requests: Arc::new(ConcurrentQueue::unbounded()),
            #[cfg(not(target_arch = "wasm32"))]
            worker: Arc::new(std::sync::OnceLock::new()),
        };
        #[cfg(not(target_arch = "wasm32"))]
        {
            let (server, requests, replies, worker) =
                (http.server.clone(), http.requests.clone(), http.replies.clone(), http.worker.clone());
            crate::thread::spawn("http", move || {
                let _ = worker.set(std::thread::current());
                native::serve_http(&server, &requests, &replies);
            });
        }
        http
    }

    pub fn server(&self) -> &Server {
        &self.server
    }

    /// Send `method path` with an optional JSON body; the reply comes back with `id`.
    pub fn send(&self, id: u64, method: &'static str, path: &str, body: Option<String>) {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = self.requests.push((id, method, path.to_string(), body));
            if let Some(worker) = self.worker.get() {
                worker.unpark();
            }
        }
        #[cfg(target_arch = "wasm32")]
        web::fetch(&self.server, &self.replies, id, method, path, body);
    }

    pub fn receive(&self) -> Option<HttpReply> {
        self.replies.pop().ok()
    }
}

/// The match socket's side of a `Link`: run natively on its own thread until the match is over for this client.
#[cfg(not(target_arch = "wasm32"))]
pub fn run_socket(server: Server, match_id: String, up: &ConcurrentQueue<Up>, down: &ConcurrentQueue<Down>) -> String {
    native::run_socket(&server, &match_id, up, down)
}

/// This user's directory for guest tokens and saves.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn config_dir() -> std::path::PathBuf {
    native::config_dir()
}

/// Keep guest tokens in `dir` from now on (tests: one directory per run).
#[cfg(not(target_arch = "wasm32"))]
pub fn use_config_dir(dir: std::path::PathBuf) {
    let _ = native::CONFIG_DIR.set(dir);
}

#[cfg(not(target_arch = "wasm32"))]
mod native {
    pub(super) static CONFIG_DIR: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

    use super::*;
    use std::io::ErrorKind;
    use std::time::Duration;
    use tungstenite::Message;

    /// Where guest tokens are kept: `use_config_dir`, `CIV_CONFIG_DIR`, else the platform's per-user config
    /// directory.
    pub(super) fn config_dir() -> std::path::PathBuf {
        if let Some(dir) = CONFIG_DIR.get() {
            return dir.clone();
        }
        if let Ok(dir) = std::env::var("CIV_CONFIG_DIR") {
            return dir.into();
        }
        let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_else(|_| ".".into());
        let home = std::path::PathBuf::from(home);
        if cfg!(target_os = "macos") {
            home.join("Library/Application Support/civ")
        } else if cfg!(windows) {
            std::env::var("APPDATA").map_or(home.join("civ"), |d| std::path::PathBuf::from(d).join("civ"))
        } else {
            std::env::var("XDG_CONFIG_HOME").map_or(home.join(".config/civ"), |d| std::path::PathBuf::from(d).join("civ"))
        }
    }

    fn tokens_file() -> std::path::PathBuf {
        config_dir().join("guests.json")
    }

    /// This client's guest token for `server`, if it has one.
    pub fn token(server: &Server) -> Option<String> {
        let text = std::fs::read_to_string(tokens_file()).ok()?;
        let tokens: std::collections::BTreeMap<String, String> = serde_json::from_str(&text).ok()?;
        tokens.get(&server.guest_key()).cloned()
    }

    /// Keep the token a server set, readable by this user only.
    fn save_token(server: &Server, token: &str) {
        let path = tokens_file();
        let mut tokens: std::collections::BTreeMap<String, String> = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default();
        if tokens.get(&server.guest_key()).map(String::as_str) == Some(token) {
            return;
        }
        tokens.insert(server.guest_key(), token.to_string());
        let _ = std::fs::create_dir_all(config_dir());
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        if let Ok(mut file) = options.open(&path) {
            use std::io::Write;
            let _ = file.write_all(serde_json::to_string_pretty(&tokens).unwrap_or_default().as_bytes());
        }
    }

    /// The `civ_guest` value in a `Set-Cookie` header.
    fn guest_cookie(set_cookie: &str) -> Option<&str> {
        let pair = set_cookie.split(';').next()?.trim();
        pair.strip_prefix("civ_guest=").filter(|t| !t.is_empty())
    }

    pub fn serve_http(
        server: &Server,
        requests: &ConcurrentQueue<(u64, &'static str, String, Option<String>)>,
        replies: &ConcurrentQueue<HttpReply>,
    ) {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .http_status_as_error(false)
            .timeout_global(Some(Duration::from_secs(15)))
            .build()
            .into();
        loop {
            let Ok((id, method, path, body)) = requests.pop() else {
                std::thread::park();
                continue;
            };
            let url = server.url(&path);
            let mut request = ureq::http::Request::builder().method(method).uri(&url).header("Origin", server.origin());
            if let Some(token) = token(server) {
                request = request.header("Cookie", format!("civ_guest={token}"));
            }
            if body.is_some() {
                request = request.header("Content-Type", "application/json");
            }
            let request = request.body(body.unwrap_or_default());
            let reply = match request.map_err(|e| e.to_string()).and_then(|r| agent.run(r).map_err(|e| e.to_string())) {
                Ok(mut response) => {
                    for value in response.headers().get_all("set-cookie") {
                        if let Some(token) = value.to_str().ok().and_then(guest_cookie) {
                            save_token(server, token);
                        }
                    }
                    let status = response.status().as_u16();
                    let date = response.headers().get("date").and_then(|d| d.to_str().ok()).and_then(http_date);
                    let body = response.body_mut().read_to_string().unwrap_or_default();
                    HttpReply { id, status, body, date }
                }
                Err(e) => HttpReply { id, status: 0, body: e, date: None },
            };
            let _ = replies.push(reply);
        }
    }

    enum Ended {
        /// Lost; try again.
        Lost,
        /// Over for this client, with the reason.
        Stopped(String),
    }

    pub fn run_socket(server: &Server, match_id: &str, up: &ConcurrentQueue<Up>, down: &ConcurrentQueue<Down>) -> String {
        let mut retry = Retry::default();
        loop {
            match connect(server, match_id) {
                Ok(mut socket) => match serve(&mut socket, &mut retry, up, down) {
                    Ended::Lost => {}
                    Ended::Stopped(reason) => return reason,
                },
                Err(Some(reason)) => return reason,
                Err(None) => {}
            }
            let Some(wait) = retry.lost(now_secs(), jitter()) else {
                return "the server could not be reached".into();
            };
            let _ = down.push(Down::Status(Status::Retrying(retry.tries())));
            let until = now_secs() + wait;
            while now_secs() < until {
                std::thread::park_timeout(Duration::from_secs_f64((until - now_secs()).max(0.0)));
            }
            // What the client sent while the socket was down is not sent late: after the welcome, the Outbox
            // resends its command and the screens ask their questions again.
            while let Ok(message) = up.pop() {
                if matches!(message, Up::Close) {
                    return "left".into();
                }
            }
        }
    }

    type Socket = tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>;

    /// Open the match socket. `Err(Some(reason))` when the server refuses this client for good (403: not a
    /// member, or it left).
    fn connect(server: &Server, match_id: &str) -> Result<Socket, Option<String>> {
        use tungstenite::client::IntoClientRequest;
        let mut request = server.socket_url(match_id).into_client_request().map_err(|e| Some(e.to_string()))?;
        let headers = request.headers_mut();
        headers.insert("Origin", server.origin().parse().map_err(|_| Some("bad origin".to_string()))?);
        if let Some(token) = token(server) {
            headers.insert("Cookie", format!("civ_guest={token}").parse().map_err(|_| Some("bad token".to_string()))?);
        }
        match tungstenite::connect(request) {
            Ok((socket, _)) => {
                let timeout = Some(Duration::from_millis(20));
                match socket.get_ref() {
                    tungstenite::stream::MaybeTlsStream::Plain(s) => {
                        let _ = s.set_read_timeout(timeout);
                    }
                    tungstenite::stream::MaybeTlsStream::Rustls(s) => {
                        let _ = s.get_ref().set_read_timeout(timeout);
                    }
                    _ => {}
                }
                Ok(socket)
            }
            Err(tungstenite::Error::Http(response)) if matches!(response.status().as_u16(), 401 | 403 | 404) => {
                Err(Some(format!("the server refused this seat ({})", response.status().as_u16())))
            }
            Err(_) => Err(None),
        }
    }

    fn serve(socket: &mut Socket, retry: &mut Retry, up: &ConcurrentQueue<Up>, down: &ConcurrentQueue<Down>) -> Ended {
        if socket.send(Message::text(hello())).is_err() {
            return Ended::Lost;
        }
        loop {
            while let Ok(message) = up.pop() {
                if matches!(message, Up::Close) {
                    let _ = socket.close(None);
                    let _ = socket.flush();
                    return Ended::Stopped("left".into());
                }
                // Rounds and eyes are the server's in a room.
                let Up::Client(message) = message else { continue };
                let Ok(text) = serde_json::to_string(&message) else { continue };
                if socket.send(Message::text(text)).is_err() {
                    return Ended::Lost;
                }
            }
            match socket.read() {
                Ok(Message::Text(text)) => {
                    let Ok(message) = serde_json::from_str::<ServerMessage>(&text) else { continue };
                    match &message {
                        ServerMessage::Welcome { .. } => {
                            retry.connected(now_secs());
                            let _ = down.push(Down::Status(Status::Live));
                        }
                        ServerMessage::Error { code } if terminal(code) => return Ended::Stopped(code.clone()),
                        _ => {}
                    }
                    let _ = down.push(Down::Message(message));
                }
                Ok(Message::Close(_)) => return Ended::Lost,
                Ok(_) => {}
                Err(tungstenite::Error::Io(e)) if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
                    // Pongs to the server's pings go out here.
                    if !matches!(socket.flush(), Ok(()) | Err(tungstenite::Error::Io(_))) {
                        return Ended::Lost;
                    }
                }
                Err(_) => return Ended::Lost,
            }
        }
    }
}

/// The match socket in the browser: driven once a frame from a Bevy system on the page's thread.
#[cfg(target_arch = "wasm32")]
pub mod web {
    use super::*;
    use std::cell::RefCell;
    use wasm_bindgen::closure::Closure;
    use wasm_bindgen::JsCast;

    pub fn fetch(
        server: &Server,
        replies: &Arc<ConcurrentQueue<HttpReply>>,
        id: u64,
        method: &str,
        path: &str,
        body: Option<String>,
    ) {
        let fail = |replies: &ConcurrentQueue<HttpReply>, e: String| {
            let _ = replies.push(HttpReply { id, status: 0, body: e, date: None });
        };
        let Some(window) = web_sys::window() else { return fail(replies, "no window".into()) };
        let init = web_sys::RequestInit::new();
        init.set_method(method);
        init.set_credentials(web_sys::RequestCredentials::SameOrigin);
        if let Some(body) = &body {
            init.set_body(&body.into());
            let headers = web_sys::Headers::new().expect("headers");
            let _ = headers.set("Content-Type", "application/json");
            init.set_headers(&headers);
        }
        let Ok(request) = web_sys::Request::new_with_str_and_init(&server.url(path), &init) else {
            return fail(replies, "bad request".into());
        };
        let (ok, err) = (replies.clone(), replies.clone());
        let on_response = Closure::once(move |value: wasm_bindgen::JsValue| {
            let response: web_sys::Response = value.unchecked_into();
            let status = response.status();
            let date = response.headers().get("date").ok().flatten().as_deref().and_then(http_date);
            let (ok2, err2) = (ok.clone(), ok.clone());
            let on_text = Closure::once(move |text: wasm_bindgen::JsValue| {
                let _ = ok2.push(HttpReply { id, status, body: text.as_string().unwrap_or_default(), date });
            });
            let on_fail = Closure::once(move |_: wasm_bindgen::JsValue| {
                let _ = err2.push(HttpReply { id, status, body: String::new(), date });
            });
            if let Ok(text) = response.text() {
                let _ = text.then2(&on_text, &on_fail);
            }
            on_text.forget();
            on_fail.forget();
        });
        let on_error = Closure::once(move |e: wasm_bindgen::JsValue| {
            let _ = err.push(HttpReply { id, status: 0, body: format!("{e:?}"), date: None });
        });
        let _ = window.fetch_with_request(&request).then2(&on_response, &on_error);
        on_response.forget();
        on_error.forget();
    }

    struct Open {
        socket: web_sys::WebSocket,
        _on_message: Closure<dyn FnMut(web_sys::MessageEvent)>,
        _on_close: Closure<dyn FnMut(web_sys::CloseEvent)>,
    }

    #[derive(Default)]
    struct State {
        open: Option<Open>,
        retry: Retry,
        /// When to try again, while waiting.
        next_try: Option<f64>,
        /// The socket closed; seen on the next pump.
        closed: Rc<RefCell<bool>>,
        stopped: bool,
    }

    use std::rc::Rc;

    thread_local! {
        static STATE: RefCell<State> = RefCell::new(State::default());
    }

    /// Forget the socket of a match that was left.
    pub fn close() {
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            if let Some(open) = s.open.take() {
                let _ = open.socket.close();
            }
            *s = State::default();
        });
    }

    /// Connect, send what is queued, notice a closed socket and try again on time.
    pub fn pump(server: &Server, match_id: &str, up: &Arc<ConcurrentQueue<Up>>, down: &Arc<ConcurrentQueue<Down>>) {
        STATE.with(|state| {
            let mut s = state.borrow_mut();
            if s.stopped {
                return;
            }
            let now = now_secs();
            if *s.closed.borrow() {
                *s.closed.borrow_mut() = false;
                s.open = None;
                match s.retry.lost(now, jitter()) {
                    Some(wait) => {
                        s.next_try = Some(now + wait);
                        let _ = down.push(Down::Status(Status::Retrying(s.retry.tries())));
                    }
                    None => {
                        s.stopped = true;
                        let _ = down.push(Down::Stopped("the server could not be reached".into()));
                        return;
                    }
                }
            }
            if s.open.is_none() {
                if s.next_try.is_some_and(|t| now < t) {
                    return;
                }
                s.next_try = None;
                while up.pop().is_ok() {}
                match open(server, match_id, down, s.closed.clone()) {
                    Some(o) => s.open = Some(o),
                    None => *s.closed.borrow_mut() = true,
                }
                return;
            }
            let socket = &s.open.as_ref().expect("open").socket;
            if socket.ready_state() != web_sys::WebSocket::OPEN {
                return;
            }
            while let Ok(message) = up.pop() {
                let Up::Client(message) = message else { continue };
                if let Ok(text) = serde_json::to_string(&message) {
                    let _ = socket.send_with_str(&text);
                }
            }
        });
    }

    /// Note a welcome (the connection is up) or a terminal error (stop trying).
    pub fn seen(message: &ServerMessage) {
        STATE.with(|s| {
            let mut s = s.borrow_mut();
            match message {
                ServerMessage::Welcome { .. } => s.retry.connected(now_secs()),
                ServerMessage::Error { code } if terminal(code) => s.stopped = true,
                _ => {}
            }
        });
    }

    fn open(
        server: &Server,
        match_id: &str,
        down: &Arc<ConcurrentQueue<Down>>,
        closed: Rc<RefCell<bool>>,
    ) -> Option<Open> {
        let socket = web_sys::WebSocket::new(&server.socket_url(match_id)).ok()?;
        let on_message = {
            let down = down.clone();
            Closure::<dyn FnMut(web_sys::MessageEvent)>::new(move |e: web_sys::MessageEvent| {
                let Some(text) = e.data().as_string() else { return };
                let Ok(message) = serde_json::from_str::<ServerMessage>(&text) else { return };
                seen(&message);
                match &message {
                    ServerMessage::Welcome { .. } => {
                        let _ = down.push(Down::Status(Status::Live));
                    }
                    ServerMessage::Error { code } if terminal(code) => {
                        let _ = down.push(Down::Stopped(code.clone()));
                        return;
                    }
                    _ => {}
                }
                let _ = down.push(Down::Message(message));
            })
        };
        let on_close = Closure::<dyn FnMut(web_sys::CloseEvent)>::new(move |_| {
            *closed.borrow_mut() = true;
        });
        let hello = hello();
        let on_open = Closure::once_into_js({
            let socket = socket.clone();
            move || {
                let _ = socket.send_with_str(&hello);
            }
        });
        socket.set_onopen(Some(on_open.unchecked_ref()));
        socket.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        socket.set_onclose(Some(on_close.as_ref().unchecked_ref()));
        Some(Open { socket, _on_message: on_message, _on_close: on_close })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_dates_read_as_unix_seconds() {
        assert_eq!(http_date("Thu, 01 Jan 1970 00:00:00 GMT"), Some(0.0));
        assert_eq!(http_date("Tue, 13 Oct 2026 16:09:52 GMT"), Some(1_791_907_792.0));
        assert_eq!(http_date("Tue, 29 Feb 2028 00:00:01 GMT"), Some(1_835_395_201.0));
        assert_eq!(http_date("yesterday"), None);
    }

    #[test]
    fn retries_back_off_give_up_and_start_over_after_a_stable_connection() {
        let mut retry = Retry::default();
        let waits: Vec<f64> = (0..Retry::TRIES).map(|_| retry.lost(0.0, 1.0).unwrap()).collect();
        assert_eq!(waits, vec![0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 10.0, 10.0]);
        assert_eq!(retry.lost(0.0, 1.0), None);
        retry.connected(100.0);
        assert_eq!(retry.lost(131.0, 0.0), Some(0.125), "30 s up resets the count; jitter halves at most");
        retry.connected(200.0);
        assert_eq!(retry.lost(210.0, 1.0), Some(0.5), "a short connection keeps counting");
    }

    #[test]
    fn origins_and_socket_urls_follow_the_base() {
        let server = Server::new("https://civ.game/", "");
        assert_eq!(server.origin(), "https://civ.game");
        assert_eq!(server.socket_url("m1"), "wss://civ.game/api/matches/m1/ws");
        let local = Server::new("http://127.0.0.1:3001", "b");
        assert_eq!(local.origin(), "http://127.0.0.1:3001");
        assert_eq!(local.socket_url("m"), "ws://127.0.0.1:3001/api/matches/m/ws");
    }
}
