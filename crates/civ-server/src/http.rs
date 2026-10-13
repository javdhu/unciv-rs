use crate::{host::ConnectionLease, now_ms, opaque_id, store::Room, token_hash, Host, HostError};
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
    rooms::{
        ErrorBody, GuestResponse, JoinRequest, LeaveResponse, MemberView, NameRequest, RoomRequest, RoomResponse,
        RoomSummary, RoomsResponse, RulesResponse, SeatRequest,
    },
    Settings,
};
use futures_util::{SinkExt, StreamExt};
use std::{
    sync::{Arc, OnceLock},
    time::Duration,
};
use tokio::sync::broadcast;

const COOKIE: &str = "civ_guest";
const MAX_MESSAGE: usize = 64 * 1024;

pub fn router(host: Arc<Host>) -> Router {
    let api = Router::new()
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
        .with_state(host.clone());
    match &host.config.static_dir {
        // Outermost, so a file request never reaches the Origin check; every other request passes through.
        Some(root) => api.layer(middleware::from_fn_with_state(Arc::new(root.clone()), crate::site::serve)),
        None => api,
    }
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
        (status, Json(ErrorBody { error: self.error_code() })).into_response()
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
            let name = host.store.name(&id)?;
            return Ok(Json(GuestResponse { guest_id: id, name }).into_response());
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
    let mut response = Json(GuestResponse { guest_id: id, name: None }).into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).map_err(|_| HostError::Internal)?,
    );
    Ok(response)
}

async fn room_response(host: &Host, room: Room, guest: &str) -> Result<Json<RoomResponse>, HostError> {
    let own_seat = room
        .seats
        .iter()
        .find_map(|(s, g)| (g == guest).then_some(*s))
        .ok_or(HostError::Forbidden)?;
    let is_owner = room.owner == guest;
    let rules = civ_sim::content::shared_ruleset(room.settings.ruleset_name()).map_err(|_| HostError::Internal)?;
    let now = now_ms();
    let live = match &room.match_id {
        Some(id) => host.seat_states(id).await?,
        None => {
            // Every room call a member makes before the start, the lobby's reading of it above all, says it is there.
            host.saw(&room.id, guest, now);
            Vec::new()
        }
    };
    let mut members = Vec::new();
    for (seat, member) in &room.seats {
        let state = live.iter().find(|s| s.seat == *seat);
        let connected = match room.match_id {
            Some(_) => state.and_then(|s| s.connected),
            None => Some(host.present(&room.id, member, now)),
        };
        members.push(MemberView {
            seat: *seat,
            name: host.store.name(member)?,
            is_owner: *member == room.owner,
            connected,
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
        slice: room.settings.slice.clone(),
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
/// The name other members see: 1 to 32 characters once trimmed, no control
/// characters.
async fn name_guest(
    State(host): State<Arc<Host>>,
    headers: HeaderMap,
    request: Result<Json<NameRequest>, JsonRejection>,
) -> Result<Json<GuestResponse>, HostError> {
    let guest = authenticate(&host, &headers)?;
    let Json(request) = request.map_err(|_| HostError::Invalid)?;
    let name = request.name.trim();
    if name.is_empty() || name.chars().count() > 32 || name.chars().any(char::is_control) {
        return Err(HostError::Invalid);
    }
    host.rename(&guest, name).await?;
    Ok(Json(GuestResponse { guest_id: guest, name: Some(name.to_string()) }))
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
    // Through a Value as before, so each summary's keys keep their sorted order on the wire.
    let rooms = serde_json::to_value(RoomsResponse { rooms }).map_err(|_| HostError::Internal)?;
    Ok(Json(rooms))
}
async fn leave_room(
    State(host): State<Arc<Host>>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Result<Json<LeaveResponse>, HostError> {
    let guest = authenticate(&host, &headers)?;
    host.leave(&guest, &id).await?;
    Ok(Json(LeaveResponse { left: true }))
}
async fn rules() -> Result<Json<serde_json::Value>, HostError> {
    static RULES: OnceLock<serde_json::Value> = OnceLock::new();
    if let Some(value) = RULES.get() {
        return Ok(Json(value.clone()));
    }
    let rules = content::load().map_err(|_| HostError::Internal)?;
    // Cached as a Value as before, so the tables' keys keep their sorted order on the wire.
    let value = serde_json::to_value(RulesResponse { content_hash: content::hash(), tables: civ_sim::tables(&rules) })
        .map_err(|_| HostError::Internal)?;
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
    let mut previous = update.clone();
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
        if let Some(mut message) = message {
            if let ServerMessage::Update { update } = message {
                let revision = update.cursor.revision.parse::<u64>().unwrap_or(0);
                // A resync can overtake already queued updates. Never roll the client back or replay their events.
                if revision < sent_revision || (revision == sent_revision && !forced_snapshot) {
                    continue;
                }
                sent_revision = revision;
                message = if forced_snapshot {
                    ServerMessage::Update { update: update.clone() }
                } else {
                    civ_sim::delta::message(&previous.cursor, &previous.view, update.clone())
                };
                previous = update;
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
