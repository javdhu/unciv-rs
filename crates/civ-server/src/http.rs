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
        .route("/api/guest/name", post(name_guest))
        .route("/api/rooms", post(create_room).get(my_rooms))
        .route("/api/rooms/join", post(join_room))
        .route("/api/rooms/{id}", get(room))
        .route("/api/rooms/{id}/start", post(start_room))
        .route("/api/rooms/{id}/seat", post(choose_seat))
        .route("/api/rooms/{id}/leave", post(leave_room))
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
        Ok(id) => {
            let mut body = serde_json::json!({ "guestId": id });
            // Present once the guest chose a display name.
            if let Some(name) = host.store.name(&id)? {
                body["name"] = name.into();
            }
            return Ok(Json(body).into_response());
        }
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
    let mut response = Json(serde_json::json!({ "guestId": id })).into_response();
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
    leader: Option<String>,
    slice: Option<String>,
    wrap_x: bool,
    max_turns: Option<u32>,
    speed: Option<String>,
    ruleset: Option<String>,
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
            leader: None,
            slice: None,
            wrap_x: false,
            max_turns: None,
            speed: None,
            ruleset: None,
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
    ruleset: String,
    victories: Option<Vec<String>>,
    /// Each occupied seat's chosen nation and leader, if any.
    seats: Vec<civ_sim::SeatChoice>,
    #[serde(skip_serializing_if = "Option::is_none")]
    invite: Option<String>,
    match_id: Option<String>,
    /// Every occupied seat: who holds it, and once the match runs, whether
    /// they are connected, have finished the turn, or left it to a bot.
    members: Vec<MemberView>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MemberView {
    seat: u8,
    name: Option<String>,
    is_owner: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    connected: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    ready: Option<bool>,
    /// A person plays it; false while a bot has taken over.
    #[serde(skip_serializing_if = "Option::is_none")]
    controlled: Option<bool>,
    left: bool,
}
async fn room_response(host: &Host, room: Room, guest: &str) -> Result<Json<RoomResponse>, HostError> {
    let own_seat = room
        .seats
        .iter()
        .find_map(|(s, g)| (g == guest).then_some(*s))
        .ok_or(HostError::Forbidden)?;
    let is_owner = room.owner == guest;
    let rules = civ_sim::content::shared_ruleset(room.settings.ruleset_name()).map_err(|_| HostError::Internal)?;
    let live = match &room.match_id {
        Some(id) => host.seat_states(id).await?,
        None => Vec::new(),
    };
    let mut members = Vec::new();
    for (seat, member) in &room.seats {
        let state = live.iter().find(|s| s.seat == *seat);
        members.push(MemberView {
            seat: *seat,
            name: host.store.name(member)?,
            is_owner: *member == room.owner,
            connected: state.and_then(|s| s.connected),
            ready: state.map(|s| s.ready),
            controlled: state.map(|s| s.controlled),
            left: state.is_some_and(|s| s.left == Some(true)),
        });
    }
    Ok(Json(RoomResponse {
        members,
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
        ruleset: room.settings.ruleset_name().to_string(),
        seats: room
            .seats
            .iter()
            .map(|(seat, _)| {
                let (nation, leader) = room.settings.choice(usize::from(*seat));
                civ_sim::SeatChoice { seat: *seat, nation: nation.map(String::from), leader: leader.map(String::from) }
            })
            .collect(),
        victories: room.settings.victories.clone(),
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
        ruleset: request.ruleset,
        victories: request.victories,
        leader: request.leader,
        slice: request.slice,
        seats: None,
    };
    let room = host.create_room(&guest, settings, request.turn_seconds).await?;
    room_response(&host, room, &guest).await
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
    let room = host.join(&guest, &request.invite).await?;
    room_response(&host, room, &guest).await
}
async fn room(
    State(host): State<Arc<Host>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<RoomResponse>, HostError> {
    let guest = authenticate(&host, &headers)?;
    let room = host.store.room(&id)?;
    room_response(&host, room, &guest).await
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SeatRequest {
    #[serde(default)]
    nation: Option<String>,
    #[serde(default)]
    leader: Option<String>,
}
async fn choose_seat(
    State(host): State<Arc<Host>>,
    headers: HeaderMap,
    Path(id): Path<String>,
    request: Result<Json<SeatRequest>, JsonRejection>,
) -> Result<Json<RoomResponse>, HostError> {
    let guest = authenticate(&host, &headers)?;
    let Json(request) = request.map_err(|_| HostError::Invalid)?;
    let room = host.choose_seat(&guest, &id, request.nation, request.leader).await?;
    room_response(&host, room, &guest).await
}
async fn start_room(
    State(host): State<Arc<Host>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<RoomResponse>, HostError> {
    let guest = authenticate(&host, &headers)?;
    let room = host.start(&guest, &id).await?;
    room_response(&host, room, &guest).await
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NameRequest {
    name: String,
}
/// The name other members see: 1 to 32 characters once trimmed, no control
/// characters.
async fn name_guest(
    State(host): State<Arc<Host>>,
    headers: HeaderMap,
    request: Result<Json<NameRequest>, JsonRejection>,
) -> Result<Json<serde_json::Value>, HostError> {
    let guest = authenticate(&host, &headers)?;
    let Json(request) = request.map_err(|_| HostError::Invalid)?;
    let name = request.name.trim();
    if name.is_empty() || name.chars().count() > 32 || name.chars().any(char::is_control) {
        return Err(HostError::Invalid);
    }
    host.rename(&guest, name).await?;
    Ok(Json(serde_json::json!({"guestId":guest,"name":name})))
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RoomSummary {
    room_id: String,
    match_id: Option<String>,
    own_seat: u8,
    is_owner: bool,
    occupied_seats: usize,
    capacity: u8,
    /// The match's turn, once it has started.
    #[serde(skip_serializing_if = "Option::is_none")]
    turn: Option<u32>,
    finished: bool,
}
/// The rooms a returning guest holds a seat in, lobbies and matches alike,
/// leaving out any it left.
async fn my_rooms(State(host): State<Arc<Host>>, headers: HeaderMap) -> Result<Json<serde_json::Value>, HostError> {
    let guest = authenticate(&host, &headers)?;
    let mut rooms = Vec::new();
    for (room_id, match_id, own_seat, is_owner) in host.store.rooms_of(&guest)? {
        let room = host.store.room(&room_id)?;
        let (turn, finished) = match &match_id {
            Some(id) => {
                let Some((turn, finished, left)) = host.match_summary(id, own_seat).await? else { continue };
                if left {
                    continue;
                }
                (Some(turn), finished)
            }
            None => (None, false),
        };
        rooms.push(RoomSummary {
            room_id,
            match_id,
            own_seat,
            is_owner,
            occupied_seats: room.seats.len(),
            capacity: room.settings.majors,
            turn,
            finished,
        });
    }
    Ok(Json(serde_json::json!({ "rooms": rooms })))
}
async fn leave_room(
    State(host): State<Arc<Host>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, HostError> {
    let guest = authenticate(&host, &headers)?;
    host.leave(&guest, &id).await?;
    Ok(Json(serde_json::json!({"left":true})))
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
        content_hash,
        mut updates,
        mut fence,
    } = lease;
    let mut sent_revision = update.cursor.revision.parse::<u64>().unwrap_or(0);
    if !send(
        &mut socket,
        ServerMessage::Welcome {
            version: PROTOCOL_VERSION,
            content_hash,
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
