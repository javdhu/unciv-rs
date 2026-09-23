use crate::{host::ConnectionLease, opaque_id, store::Room, token_hash, Host, HostError};
use axum::{
    extract::{
        rejection::JsonRejection,
        ws::{Message, WebSocket},
        DefaultBodyLimit, Path, State, WebSocketUpgrade,
    },
    http::{header, HeaderMap, HeaderValue, Method, StatusCode},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use civ_sim::{
    content,
    protocol::{ClientMessage, ServerMessage, PROTOCOL_VERSION},
    Settings,
};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::sync::broadcast;

const COOKIE: &str = "civ_guest";
const MAX_MESSAGE: usize = 64 * 1024;

pub fn router(host: Arc<Host>) -> Router {
    Router::new()
        .route("/health", get(|| async { "ok" }))
        .route("/api/rules", get(rules))
        .route("/api/guest", post(guest))
        .route("/api/rooms", post(create_room))
        .route("/api/rooms/join", post(join_room))
        .route("/api/rooms/{id}", get(room))
        .route("/api/rooms/{id}/start", post(start_room))
        .route("/api/matches/{id}/ws", get(upgrade))
        .layer(DefaultBodyLimit::max(MAX_MESSAGE))
        .layer(middleware::from_fn_with_state(host.clone(), origin))
        .with_state(host)
}

impl IntoResponse for HostError {
    fn into_response(self) -> Response {
        let status = match self {
            Self::Unauthorized => StatusCode::UNAUTHORIZED,
            Self::Forbidden => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::Conflict => StatusCode::CONFLICT,
            Self::Invalid => StatusCode::BAD_REQUEST,
            Self::Capacity => StatusCode::TOO_MANY_REQUESTS,
            Self::Fenced => StatusCode::CONFLICT,
            Self::Internal => StatusCode::SERVICE_UNAVAILABLE,
        };
        (status, Json(serde_json::json!({"error":self.code()}))).into_response()
    }
}

async fn origin(
    State(host): State<Arc<Host>>,
    request: axum::extract::Request,
    next: Next,
) -> Response {
    let needs_origin = !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    ) || request.uri().path().ends_with("/ws");
    if needs_origin
        && !request
            .headers()
            .get(header::ORIGIN)
            .and_then(|h| h.to_str().ok())
            .is_some_and(|origin| {
                host.config
                    .allowed_origins
                    .iter()
                    .any(|allowed| allowed == origin)
            })
    {
        return HostError::Forbidden.into_response();
    }
    let mut response = next.run(request).await;
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    response
}

fn authenticate(host: &Host, headers: &HeaderMap) -> Result<String, HostError> {
    let token = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|c| {
            c.split(';')
                .filter_map(|p| p.trim().split_once('='))
                .find(|(k, _)| *k == COOKIE)
                .map(|(_, v)| v)
        })
        .filter(|t| t.len() == 64 && t.bytes().all(|c| c.is_ascii_hexdigit()))
        .ok_or(HostError::Unauthorized)?;
    host.store
        .guest(&token_hash(token))?
        .ok_or(HostError::Unauthorized)
}

async fn guest(State(host): State<Arc<Host>>, headers: HeaderMap) -> Result<Response, HostError> {
    match authenticate(&host, &headers) {
        Ok(id) => return Ok(Json(serde_json::json!({"guestId":id})).into_response()),
        Err(HostError::Unauthorized) => {}
        Err(e) => return Err(e),
    }
    let id = opaque_id();
    let token = format!("{}{}", opaque_id(), opaque_id());
    host.store.create_guest(&id, &token_hash(&token))?;
    let cookie = format!(
        "{COOKIE}={token}; HttpOnly; SameSite=Lax; Path=/; Max-Age=2592000{}",
        if host.config.secure_cookie {
            "; Secure"
        } else {
            ""
        }
    );
    let mut response = Json(serde_json::json!({"guestId":id})).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).map_err(|_| HostError::Internal)?,
    );
    Ok(response)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
struct RoomRequest {
    width: u16,
    height: u16,
    majors: u8,
    city_states: u8,
    nation: Option<String>,
    wrap_x: bool,
    max_turns: Option<u32>,
    speed: Option<String>,
    victories: Option<Vec<String>>,
    turn_seconds: u32,
}
impl Default for RoomRequest {
    fn default() -> Self {
        Self {
            width: 32,
            height: 20,
            majors: 4,
            city_states: 2,
            nation: None,
            wrap_x: false,
            max_turns: None,
            speed: None,
            victories: None,
            turn_seconds: 120,
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JoinRequest {
    invite: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RoomResponse {
    room_id: String,
    own_seat: u8,
    occupied_seats: usize,
    capacity: u8,
    width: u16,
    height: u16,
    city_states: u8,
    turn_seconds: u32,
    is_owner: bool,
    wrap_x: bool,
    /// The effective limit: explicit, or the speed's length.
    max_turns: u32,
    speed: String,
    victories: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    invite: Option<String>,
    match_id: Option<String>,
}
fn room_response(room: Room, guest: &str) -> Result<Json<RoomResponse>, HostError> {
    let own_seat = room
        .seats
        .iter()
        .find_map(|(s, g)| (g == guest).then_some(*s))
        .ok_or(HostError::Forbidden)?;
    let is_owner = room.owner == guest;
    let rules = civ_sim::content::shared().map_err(|_| HostError::Internal)?;
    Ok(Json(RoomResponse {
        room_id: room.id,
        own_seat,
        occupied_seats: room.seats.len(),
        capacity: room.settings.majors,
        width: room.settings.width,
        height: room.settings.height,
        city_states: room.settings.city_states,
        turn_seconds: room.turn_seconds,
        is_owner,
        wrap_x: room.settings.wrap_x,
        max_turns: room
            .settings
            .turn_limit(rules)
            .map_err(|_| HostError::Invalid)?,
        speed: rules.speeds[room
            .settings
            .speed_in(rules)
            .map_err(|_| HostError::Invalid)?]
        .name
        .clone(),
        victories: room.settings.victories,
        invite: is_owner.then_some(room.invite),
        match_id: room.match_id,
    }))
}
async fn create_room(
    State(host): State<Arc<Host>>,
    headers: HeaderMap,
    request: Result<Json<RoomRequest>, JsonRejection>,
) -> Result<Json<RoomResponse>, HostError> {
    let guest = authenticate(&host, &headers)?;
    let Json(request) = request.map_err(|_| HostError::Invalid)?;
    let random = uuid::Uuid::new_v4().as_u128() as u64 & ((1 << 53) - 1);
    let settings = Settings {
        seed: random,
        width: request.width,
        height: request.height,
        majors: request.majors,
        city_states: request.city_states,
        nation: request.nation,
        wrap_x: request.wrap_x,
        max_turns: request.max_turns,
        speed: request.speed,
        victories: request.victories,
    };
    room_response(
        host.create_room(&guest, settings, request.turn_seconds)
            .await?,
        &guest,
    )
}
async fn join_room(
    State(host): State<Arc<Host>>,
    headers: HeaderMap,
    request: Result<Json<JoinRequest>, JsonRejection>,
) -> Result<Json<RoomResponse>, HostError> {
    let guest = authenticate(&host, &headers)?;
    let Json(request) = request.map_err(|_| HostError::Invalid)?;
    if request.invite.len() != 32 {
        return Err(HostError::Invalid);
    }
    room_response(host.join(&guest, &request.invite).await?, &guest)
}
async fn room(
    State(host): State<Arc<Host>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<RoomResponse>, HostError> {
    let guest = authenticate(&host, &headers)?;
    room_response(host.store.room(&id)?, &guest)
}
async fn start_room(
    State(host): State<Arc<Host>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<RoomResponse>, HostError> {
    let guest = authenticate(&host, &headers)?;
    room_response(host.start(&guest, &id).await?, &guest)
}
async fn rules() -> Result<Json<serde_json::Value>, HostError> {
    static RULES: OnceLock<serde_json::Value> = OnceLock::new();
    if let Some(value) = RULES.get() {
        return Ok(Json(value.clone()));
    }
    let rules = content::load().map_err(|_| HostError::Internal)?;
    let value = serde_json::json!({"contentHash":content::hash(),"tables":civ_sim::tables(&rules)});
    let _ = RULES.set(value.clone());
    Ok(Json(value))
}
async fn upgrade(
    State(host): State<Arc<Host>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    ws: WebSocketUpgrade,
) -> Result<Response, HostError> {
    let guest = authenticate(&host, &headers)?;
    host.check_seat(&id, &guest).await?;
    Ok(ws
        .max_message_size(MAX_MESSAGE)
        .max_frame_size(MAX_MESSAGE)
        .max_write_buffer_size(4 * 1024 * 1024)
        .on_upgrade(move |socket| socket_loop(socket, host, id, guest)))
}

async fn socket_loop(mut socket: WebSocket, host: Arc<Host>, id: String, guest: String) {
    let hello = tokio::time::timeout(Duration::from_secs(5), socket.recv()).await;
    let valid = matches!(hello, Ok(Some(Ok(Message::Text(ref text)))) if matches!(serde_json::from_str::<ClientMessage>(text),
        Ok(ClientMessage::Hello { version: PROTOCOL_VERSION })));
    if !valid {
        let _ = send(
            &mut socket,
            ServerMessage::Error {
                code: "protocolVersion".into(),
            },
        )
        .await;
        return;
    }
    let lease = match host.connect(&id, &guest).await {
        Ok(lease) => lease,
        Err(e) => {
            let _ = send(
                &mut socket,
                ServerMessage::Error {
                    code: e.code().into(),
                },
            )
            .await;
            return;
        }
    };
    let ConnectionLease {
        seat,
        generation,
        update,
        mut updates,
        mut fence,
    } = lease;
    let mut sent_revision = update.cursor.revision.parse::<u64>().unwrap_or(0);
    if !send(
        &mut socket,
        ServerMessage::Welcome {
            version: PROTOCOL_VERSION,
            content_hash: content::hash(),
            update,
        },
    )
    .await
    {
        let _ = host.disconnect(&id, seat, generation).await;
        return;
    }
    let (mut sink, mut stream) = socket.split();
    let mut window = tokio::time::Instant::now();
    let mut count = 0;
    let mut last_seen = tokio::time::Instant::now();
    let mut heartbeat = tokio::time::interval(Duration::from_secs(10));
    heartbeat.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        let mut forced_snapshot = false;
        let message = tokio::select! {
            biased;
            changed = fence.changed() => {
                if changed.is_err() || *fence.borrow() != generation {
                    let _ = send_sink(&mut sink, ServerMessage::Error { code: "connectionReplaced".into() }).await;
                    break;
                }
                continue;
            }
            incoming = stream.next() => {
                let Some(Ok(incoming)) = incoming else { break };
                last_seen = tokio::time::Instant::now();
                if window.elapsed() >= Duration::from_secs(1) { window = tokio::time::Instant::now(); count = 0; }
                count += 1;
                if count > 128 {
                    let _ = send_sink(&mut sink,ServerMessage::Error { code: "rateLimited".into() }).await;
                    break;
                }
                match incoming {
                    Message::Close(_) => break,
                    Message::Ping(_) | Message::Pong(_) => continue,
                    Message::Binary(_) => Some(ServerMessage::Error { code: "invalidRequest".into() }),
                    Message::Text(text) => {
                        let result = match serde_json::from_str::<ClientMessage>(&text) {
                            Ok(ClientMessage::Command { request_id, command }) if request_id.len() <= 100 =>
                                host.command(&id,seat,generation,command).await.map(|receipt| ServerMessage::Receipt { request_id, receipt }),
                            Ok(ClientMessage::Query { request_id, query }) if request_id.len() <= 100 =>
                                host.query(&id,seat,generation,query).await.map(|reply| ServerMessage::QueryReply { request_id, reply }),
                            Ok(ClientMessage::Resync) => {
                                forced_snapshot = true;
                                host.update(&id,seat,generation).await.map(|update| ServerMessage::Update { update })
                            }
                            _ => Err(HostError::Invalid),
                        };
                        Some(result.unwrap_or_else(|e| ServerMessage::Error { code: e.code().into() }))
                    }
                }
            }
            update = updates.recv() => {
                match update {
                    // Snapshots are absolute; if the consumer missed an event, resynchronize without replaying transient events.
                    Ok(update) => Some(ServerMessage::Update { update }),
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        // Discard queued older revisions after obtaining an authoritative snapshot.
                        while updates.try_recv().is_ok() {}
                        forced_snapshot = true;
                        host.update(&id,seat,generation).await.ok().map(|update| ServerMessage::Update { update })
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            _ = heartbeat.tick() => {
                if last_seen.elapsed() >= Duration::from_secs(30) { break; }
                if !matches!(tokio::time::timeout(Duration::from_secs(5),sink.send(Message::Ping(Vec::new().into()))).await,Ok(Ok(()))) { break; }
                continue;
            }
        };
        if let Some(message) = message {
            if let ServerMessage::Update { update } = &message {
                let revision = update.cursor.revision.parse::<u64>().unwrap_or(0);
                // A resync can overtake already queued updates. Never roll the client back or replay their events.
                if revision < sent_revision || (revision == sent_revision && !forced_snapshot) {
                    continue;
                }
                sent_revision = revision;
            }
            if !send_sink(&mut sink, message).await {
                break;
            }
        } else {
            break;
        }
    }
    let _ = tokio::time::timeout(Duration::from_secs(1), sink.close()).await;
    let _ = host.disconnect(&id, seat, generation).await;
}
async fn send(socket: &mut WebSocket, message: ServerMessage) -> bool {
    let Ok(json) = serde_json::to_string(&message) else {
        return false;
    };
    matches!(
        tokio::time::timeout(
            Duration::from_secs(5),
            socket.send(Message::Text(json.into()))
        )
        .await,
        Ok(Ok(()))
    )
}
async fn send_sink(
    sink: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    message: ServerMessage,
) -> bool {
    let Ok(json) = serde_json::to_string(&message) else {
        return false;
    };
    matches!(
        tokio::time::timeout(
            Duration::from_secs(5),
            sink.send(Message::Text(json.into()))
        )
        .await,
        Ok(Ok(()))
    )
}
