use crate::{
    now_ms, opaque_id,
    store::{Room, Store},
};
use civ_sim::{
    protocol::{
        CommandReceipt, CommandRequest, CommandStatus, GameUpdate, Query, QueryReply, RejectionCode,
    },
    session::{Session, SessionError},
    Settings,
};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path, sync::Arc};
use tokio::sync::{broadcast, watch, Mutex, RwLock};

#[derive(Debug, Clone)]
pub struct Config {
    /// Exact browser origins, including scheme and port. No wildcard/Host-derived trust.
    pub allowed_origins: Vec<String>,
    pub secure_cookie: bool,
    pub disconnect_grace_ms: u64,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            allowed_origins: vec![
                "http://127.0.0.1:3001".into(),
                "http://localhost:3001".into(),
                "http://127.0.0.1:5199".into(),
                "http://localhost:5199".into(),
            ],
            secure_cookie: false,
            disconnect_grace_ms: 15_000,
        }
    }
}

#[derive(Debug)]
pub enum HostError {
    Unauthorized,
    Forbidden,
    NotFound,
    Conflict,
    Invalid,
    Capacity,
    Fenced,
    Internal,
}
impl std::fmt::Display for HostError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.code())
    }
}
impl std::error::Error for HostError {}
impl HostError {
    pub(crate) fn code(&self) -> &'static str {
        match self {
            Self::Unauthorized => "unauthorized",
            Self::Forbidden => "forbidden",
            Self::NotFound => "notFound",
            Self::Conflict => "conflict",
            Self::Invalid => "invalidRequest",
            Self::Capacity => "capacity",
            Self::Fenced => "connectionReplaced",
            Self::Internal => "unavailable",
        }
    }
}
impl From<rusqlite::Error> for HostError {
    fn from(_: rusqlite::Error) -> Self {
        Self::Internal
    }
}
impl From<serde_json::Error> for HostError {
    fn from(_: serde_json::Error) -> Self {
        Self::Internal
    }
}
impl From<SessionError> for HostError {
    fn from(e: SessionError) -> Self {
        match e {
            SessionError::Capacity => Self::Capacity,
            SessionError::InvalidSettings(_) => Self::Invalid,
            SessionError::InvalidSeat => Self::Forbidden,
            _ => Self::Internal,
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SeatMetadata {
    guest: String,
    generation: u64,
    connected: bool,
    grace_until: Option<u64>,
    // The latest scheduled controller; Session applies it only at a round boundary.
    desired_human: bool,
    // The guest gave up the seat for good: a bot plays it and it cannot reconnect.
    #[serde(default)]
    left: bool,
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Metadata {
    version: u32,
    turn_seconds: u32,
    deadline_turn: u32,
    deadline_ms: Option<u64>,
    seats: BTreeMap<u8, SeatMetadata>,
}
struct Channels {
    updates: broadcast::Sender<GameUpdate>,
    generation: watch::Sender<u64>,
}
struct Match {
    session: Session,
    metadata: Metadata,
    channels: BTreeMap<u8, Channels>,
    /// Each seat's display name, read from its guest.
    names: BTreeMap<u8, String>,
}
impl Match {
    fn new(session: Session, metadata: Metadata, store: &Store) -> Self {
        let names = metadata
            .seats
            .iter()
            .filter_map(|(&seat, m)| store.name(&m.guest).ok().flatten().map(|name| (seat, name)))
            .collect();
        let channels = metadata
            .seats
            .iter()
            .map(|(&seat, m)| {
                let (updates, _) = broadcast::channel(16);
                let (generation, _) = watch::channel(m.generation);
                (
                    seat,
                    Channels {
                        updates,
                        generation,
                    },
                )
            })
            .collect();
        Self {
            session,
            metadata,
            channels,
            names,
        }
    }
    fn authorize(&self, seat: u8, generation: u64) -> Result<(), HostError> {
        let m = self.metadata.seats.get(&seat).ok_or(HostError::Forbidden)?;
        if !m.connected || generation != m.generation {
            return Err(HostError::Fenced);
        }
        Ok(())
    }
    fn publish(&self, updates: Vec<(u8, GameUpdate)>) {
        // Controller and timeout decisions can commit together; publish their final projection per seat.
        let updates: BTreeMap<_, _> = updates.into_iter().collect();
        for (seat, update) in updates {
            if let Some(channel) = self.channels.get(&seat) {
                let _ = channel.updates.send(self.decorate(update));
            }
        }
    }
    fn decorate(&self, mut update: GameUpdate) -> GameUpdate {
        update.deadline_ms = self.metadata.deadline_ms.map(|at| at.to_string());
        for status in update.seats.iter_mut().flatten() {
            let meta = self.metadata.seats.get(&status.seat);
            status.name = self.names.get(&status.seat).cloned();
            status.connected = meta.map(|m| m.connected);
            status.left = meta.is_some_and(|m| m.left).then_some(true);
        }
        update
    }
    /// Stage a connection change for `seat` when the session does not hold it yet.
    fn presence(&self, next: Session, seat: u8, connected: bool) -> Result<(Session, Vec<(u8, GameUpdate)>), HostError> {
        let known = next.seat_status().iter().find(|s| s.seat == seat).and_then(|s| s.connected);
        if known == Some(connected) {
            return Ok((next, Vec::new()));
        }
        match next.stage_presence(seat, connected) {
            Ok(transition) => Ok((transition.next, transition.updates)),
            // Presence is only shown to the room: a full input log must not
            // stop a guest from connecting.
            Err(SessionError::Capacity) => Ok((next, Vec::new())),
            Err(e) => Err(e.into()),
        }
    }
    fn deadline_for(&self, next: &Session, meta: &mut Metadata, now: u64) {
        if next.finished() {
            meta.deadline_ms = None;
        } else if next.turn() != self.session.turn() {
            meta.deadline_turn = next.turn();
            meta.deadline_ms = Some(now.saturating_add(u64::from(meta.turn_seconds) * 1000));
        }
    }
}

pub(crate) struct ConnectionLease {
    pub seat: u8,
    pub generation: u64,
    pub update: GameUpdate,
    /// The match's ruleset content hash, for the welcome.
    pub content_hash: String,
    pub updates: broadcast::Receiver<GameUpdate>,
    pub fence: watch::Receiver<u64>,
}

pub struct Host {
    pub(crate) config: Config,
    pub(crate) store: Store,
    lobby: Mutex<()>,
    matches: RwLock<BTreeMap<String, Arc<Mutex<Match>>>>,
}
impl Host {
    /// Restores durable sessions. Connections lost with the process enter a fresh reconnect grace.
    pub fn open(path: impl AsRef<Path>, config: Config) -> Result<Arc<Self>, HostError> {
        if config.allowed_origins.is_empty()
            || config.allowed_origins.iter().any(|o| {
                !(o.starts_with("http://") || o.starts_with("https://"))
                    || o.ends_with('/')
                    || o.contains('*')
            })
            || config.disconnect_grace_ms > 300_000
        {
            return Err(HostError::Invalid);
        }
        let store = Store::open(path.as_ref())?;
        let mut matches = BTreeMap::new();
        let now = now_ms();
        for (id, room_id, snapshot, metadata) in store.matches()? {
            let session = Session::restore(&snapshot)?;
            let mut metadata: Metadata = serde_json::from_str(&metadata)?;
            let room = store.room(&room_id)?;
            if metadata.version != 1
                || !(5..=3600).contains(&metadata.turn_seconds)
                || metadata.deadline_turn != session.turn()
                || metadata.deadline_ms.is_none() != session.finished()
                || room.match_id.as_deref() != Some(&id)
                || &room.settings != session.settings()
                || room.turn_seconds != metadata.turn_seconds
                || room.seats
                    != metadata
                        .seats
                        .iter()
                        .map(|(&s, m)| (s, m.guest.clone()))
                        .collect::<Vec<_>>()
                || session.seats().collect::<Vec<_>>()
                    != metadata.seats.keys().copied().collect::<Vec<_>>()
            {
                return Err(HostError::Internal);
            }
            for (&id, meta) in &metadata.seats {
                let update = session.current_update(id)?;
                if update.pending_control.unwrap_or(update.controlled) != meta.desired_human {
                    return Err(HostError::Internal);
                }
            }
            let mut session = session;
            for (&seat, meta) in metadata.seats.iter_mut() {
                if meta.connected {
                    meta.connected = false;
                    meta.grace_until = Some(now.saturating_add(config.disconnect_grace_ms));
                }
                // Connections do not survive the process; tell the room once it is back.
                if session.seat_status().iter().any(|s| s.seat == seat && s.connected == Some(true)) {
                    session = session.stage_presence(seat, false)?.next;
                }
            }
            store.persist(&id, &session, &metadata, None)?;
            let game = Match::new(session, metadata, &store);
            matches.insert(id, Arc::new(Mutex::new(game)));
        }
        Ok(Arc::new(Self {
            config,
            store,
            lobby: Mutex::new(()),
            matches: RwLock::new(matches),
        }))
    }
    pub(crate) async fn create_room(
        &self,
        guest: &str,
        settings: Settings,
        turn_seconds: u32,
    ) -> Result<Room, HostError> {
        if !(5..=3600).contains(&turn_seconds)
            || !(16..=64).contains(&settings.width)
            || !(10..=40).contains(&settings.height)
            || !(1..=8).contains(&settings.majors)
            || settings.city_states > 8
            || settings.nation.as_ref().is_some_and(|n| n.len() > 100)
        {
            return Err(HostError::Invalid);
        }
        if let Some(slice) = &settings.slice {
            let rules = civ_sim::content::shared_ruleset(settings.ruleset_name()).map_err(|_| HostError::Invalid)?;
            if !rules.slices.iter().any(|s| s.name == *slice) {
                return Err(HostError::Invalid);
            }
        }
        let _lobby = self.lobby.lock().await;
        let room = Room {
            id: opaque_id(),
            owner: guest.into(),
            invite: opaque_id(),
            settings,
            turn_seconds,
            match_id: None,
            seats: vec![(0, guest.into())],
        };
        self.store.create_room(&room)?;
        Ok(room)
    }
    pub(crate) async fn join(&self, guest: &str, invite: &str) -> Result<Room, HostError> {
        let _lobby = self.lobby.lock().await;
        let id = self.store.invite_room(invite)?;
        let mut room = self.store.room(&id)?;
        if room.seats.iter().any(|(_, g)| g == guest) {
            return Ok(room);
        }
        if room.match_id.is_some() || room.seats.len() >= usize::from(room.settings.majors) {
            return Err(HostError::Conflict);
        }
        // The lowest seat nobody holds: a guest who left freed theirs.
        let seat = (0..room.settings.majors)
            .find(|s| room.seats.iter().all(|(taken, _)| taken != s))
            .ok_or(HostError::Conflict)?;
        self.store.join(&id, seat, guest)?;
        room.seats.push((seat, guest.into()));
        room.seats.sort_by_key(|(s, _)| *s);
        Ok(room)
    }
    /// Leave a room. Before the match a member gives up its seat (and its
    /// nation choice), and the owner closes the room. After it, the seat
    /// passes to a bot for good at the next round boundary: the guest's
    /// connection is closed and it cannot reconnect.
    pub(crate) async fn leave(&self, guest: &str, room_id: &str) -> Result<(), HostError> {
        let _lobby = self.lobby.lock().await;
        let room = self.store.room(room_id)?;
        let seat = room.seats.iter().find_map(|(s, g)| (g == guest).then_some(*s)).ok_or(HostError::Forbidden)?;
        let Some(match_id) = room.match_id.clone() else {
            return if room.owner == guest { self.store.close(&room.id) } else { self.store.leave(&room, seat) };
        };
        let game = self.game(&match_id).await?;
        let mut game = game.lock().await;
        let mut meta = game.metadata.clone();
        let m = meta.seats.get_mut(&seat).ok_or(HostError::Internal)?;
        if m.left {
            return Ok(());
        }
        m.left = true;
        m.connected = false;
        m.grace_until = None;
        m.generation = m.generation.checked_add(1).ok_or(HostError::Capacity)?;
        let generation = m.generation;
        let mut next = game.session.clone();
        let mut updates = Vec::new();
        if m.desired_human && !next.finished() {
            let transition = next.stage_controller(seat, false)?;
            next = transition.next;
            updates.extend(transition.updates);
        }
        m.desired_human = false;
        let (next, more) = game.presence(next, seat, false)?;
        updates.extend(more);
        self.store.persist(&match_id, &next, &meta, None)?;
        game.session = next;
        game.metadata = meta;
        if let Some(channels) = game.channels.get(&seat) {
            // Fences a live socket for this seat.
            channels.generation.send_replace(generation);
        }
        game.publish(updates);
        Ok(())
    }
    /// A guest's display name, shown to the members of its rooms.
    pub(crate) async fn rename(&self, guest: &str, name: &str) -> Result<(), HostError> {
        self.store.set_name(guest, name)?;
        let games: Vec<_> = self.matches.read().await.values().cloned().collect();
        for game in games {
            let mut game = game.lock().await;
            let seats: Vec<u8> =
                game.metadata.seats.iter().filter(|(_, m)| m.guest == guest).map(|(&seat, _)| seat).collect();
            for seat in seats {
                game.names.insert(seat, name.to_string());
            }
        }
        Ok(())
    }
    /// A started match's turn, whether it finished, and whether `seat` left it.
    pub(crate) async fn match_summary(&self, match_id: &str, seat: u8) -> Result<Option<(u32, bool, bool)>, HostError> {
        let Ok(game) = self.game(match_id).await else { return Ok(None) };
        let game = game.lock().await;
        let left = game.metadata.seats.get(&seat).is_some_and(|m| m.left);
        Ok(Some((game.session.turn(), game.session.finished(), left)))
    }
    /// Every seat's live state in a started match, for its room's members.
    pub(crate) async fn seat_states(&self, match_id: &str) -> Result<Vec<civ_sim::protocol::SeatStatusView>, HostError> {
        let game = self.game(match_id).await?;
        let game = game.lock().await;
        let mut states = game.session.seat_status();
        for status in &mut states {
            let meta = game.metadata.seats.get(&status.seat);
            status.connected = meta.map(|m| m.connected);
            status.left = meta.is_some_and(|m| m.left).then_some(true);
            status.name = game.names.get(&status.seat).cloned();
        }
        Ok(states)
    }
    /// A seated guest chooses their nation and leader before the match starts.
    pub(crate) async fn choose_seat(
        &self,
        guest: &str,
        room_id: &str,
        nation: Option<String>,
        leader: Option<String>,
    ) -> Result<Room, HostError> {
        let _lobby = self.lobby.lock().await;
        let mut room = self.store.room(room_id)?;
        let seat = room.seats.iter().find_map(|(s, g)| (g == guest).then_some(*s)).ok_or(HostError::Forbidden)?;
        if room.match_id.is_some() {
            return Err(HostError::Conflict);
        }
        let rules = civ_sim::content::shared_ruleset(room.settings.ruleset_name()).map_err(|_| HostError::Internal)?;
        if nation.as_ref().is_some_and(|n| n.len() > 100) || leader.as_ref().is_some_and(|n| n.len() > 100) {
            return Err(HostError::Invalid);
        }
        if let Some(name) = &nation {
            let n = rules
                .nations
                .by_name(name)
                .filter(|n| n.is_major() && n.in_waves(rules.max_wave))
                .ok_or(HostError::Invalid)?;
            if leader.as_ref().is_some_and(|l| !n.leaders().any(|x| x == l)) {
                return Err(HostError::Invalid);
            }
            let taken = (0..usize::from(room.settings.majors))
                .filter(|s| *s != usize::from(seat))
                .any(|s| room.settings.choice(s).0 == Some(name.as_str()));
            if taken {
                return Err(HostError::Conflict);
            }
        } else if leader.is_some() {
            return Err(HostError::Invalid);
        }
        let seats = room.settings.seats.get_or_insert_with(Vec::new);
        seats.retain(|c| c.seat != seat);
        seats.push(civ_sim::SeatChoice { seat, nation, leader });
        seats.sort_by_key(|c| c.seat);
        self.store.update_settings(&room.id, &room.settings)?;
        Ok(room)
    }
    pub(crate) async fn start(&self, guest: &str, room_id: &str) -> Result<Room, HostError> {
        let _lobby = self.lobby.lock().await;
        let mut room = self.store.room(room_id)?;
        if room.owner != guest {
            return Err(HostError::Forbidden);
        }
        if room.match_id.is_some() {
            return Ok(room);
        }
        let id = opaque_id();
        let session = Session::new(
            room.settings.clone(),
            opaque_id(),
            room.seats.iter().map(|(s, _)| *s).collect(),
        )?;
        let now = now_ms();
        let metadata = Metadata {
            version: 1,
            turn_seconds: room.turn_seconds,
            deadline_turn: session.turn(),
            deadline_ms: Some(now + u64::from(room.turn_seconds) * 1000),
            seats: room
                .seats
                .iter()
                .map(|(seat, guest)| {
                    (
                        *seat,
                        SeatMetadata {
                            guest: guest.clone(),
                            generation: 0,
                            connected: false,
                            grace_until: Some(now.saturating_add(self.config.disconnect_grace_ms)),
                            desired_human: true,
                            left: false,
                        },
                    )
                })
                .collect(),
        };
        self.store.start(&room.id, &id, &session, &metadata)?;
        let game = Match::new(session, metadata, &self.store);
        self.matches.write().await.insert(id.clone(), Arc::new(Mutex::new(game)));
        room.match_id = Some(id);
        Ok(room)
    }
    async fn game(&self, id: &str) -> Result<Arc<Mutex<Match>>, HostError> {
        self.matches
            .read()
            .await
            .get(id)
            .cloned()
            .ok_or(HostError::NotFound)
    }
    pub(crate) async fn check_seat(&self, id: &str, guest: &str) -> Result<(), HostError> {
        let game = self.game(id).await?;
        let game = game.lock().await;
        if game.metadata.seats.values().any(|m| m.guest == guest && !m.left) {
            Ok(())
        } else {
            Err(HostError::Forbidden)
        }
    }
    pub(crate) async fn connect(
        &self,
        id: &str,
        guest: &str,
    ) -> Result<ConnectionLease, HostError> {
        let game = self.game(id).await?;
        let mut game = game.lock().await;
        let seat = game
            .metadata
            .seats
            .iter()
            .find_map(|(&seat, m)| (m.guest == guest && !m.left).then_some(seat))
            .ok_or(HostError::Forbidden)?;
        self.process_due(id, &mut game, now_ms())?;
        let mut meta = game.metadata.clone();
        let m = meta.seats.get_mut(&seat).ok_or(HostError::Internal)?;
        m.generation = m.generation.checked_add(1).ok_or(HostError::Capacity)?;
        m.connected = true;
        m.grace_until = None;
        let next = if !m.desired_human {
            game.session.stage_controller(seat, true)?.next
        } else {
            game.session.clone()
        };
        m.desired_human = true;
        let generation = m.generation;
        // The rest of the room learns this seat is back.
        let (next, updates) = game.presence(next, seat, true)?;
        self.store.persist(id, &next, &meta, None)?;
        game.session = next;
        game.metadata = meta;
        game.publish(updates.into_iter().filter(|(s, _)| *s != seat).collect());
        let channels = game.channels.get(&seat).ok_or(HostError::Internal)?;
        channels.generation.send_replace(generation);
        Ok(ConnectionLease {
            seat,
            generation,
            update: game.decorate(game.session.current_update(seat)?),
            content_hash: game.session.content_hash(),
            updates: channels.updates.subscribe(),
            fence: channels.generation.subscribe(),
        })
    }
    pub(crate) async fn disconnect(
        &self,
        id: &str,
        seat: u8,
        generation: u64,
    ) -> Result<(), HostError> {
        let game = self.game(id).await?;
        let mut game = game.lock().await;
        if game.authorize(seat, generation).is_err() {
            return Ok(());
        }
        let mut meta = game.metadata.clone();
        let m = meta.seats.get_mut(&seat).ok_or(HostError::Internal)?;
        m.connected = false;
        m.grace_until = Some(now_ms().saturating_add(self.config.disconnect_grace_ms));
        let (next, updates) = game.presence(game.session.clone(), seat, false)?;
        self.store.persist(id, &next, &meta, None)?;
        game.session = next;
        game.metadata = meta;
        game.publish(updates);
        Ok(())
    }
    pub(crate) async fn command(
        &self,
        id: &str,
        seat: u8,
        generation: u64,
        request: CommandRequest,
    ) -> Result<CommandReceipt, HostError> {
        let game = self.game(id).await?;
        let mut game = game.lock().await;
        game.authorize(seat, generation)?;
        // An input arriving after the deadline cannot race ahead of the periodic clock task.
        self.process_due(id, &mut game, now_ms())?;
        let transition = game.session.stage_command(seat, request.clone())?;
        if transition.duplicate
            || transition.receipt.result
                == (CommandStatus::Rejected {
                    code: RejectionCode::InvalidRequest,
                })
        {
            return Ok(transition.receipt);
        }
        let mut meta = game.metadata.clone();
        game.deadline_for(&transition.next, &mut meta, now_ms());
        self.store.persist(
            id,
            &transition.next,
            &meta,
            Some((seat, &request.epoch, &transition.receipt)),
        )?;
        game.session = transition.next;
        game.metadata = meta;
        game.publish(transition.updates);
        Ok(transition.receipt)
    }
    pub(crate) async fn query(
        &self,
        id: &str,
        seat: u8,
        generation: u64,
        query: Query,
    ) -> Result<QueryReply, HostError> {
        let game = self.game(id).await?;
        let game = game.lock().await;
        game.authorize(seat, generation)?;
        Ok(game.session.query(seat, query)?)
    }
    pub(crate) async fn update(
        &self,
        id: &str,
        seat: u8,
        generation: u64,
    ) -> Result<GameUpdate, HostError> {
        let game = self.game(id).await?;
        let game = game.lock().await;
        game.authorize(seat, generation)?;
        Ok(game.decorate(game.session.current_update(seat)?))
    }
    /// The stored expected turn prevents repeated scheduler ticks from advancing a round twice.
    pub async fn tick_at(&self, now: u64) -> Result<(), HostError> {
        let games: Vec<_> = self
            .matches
            .read()
            .await
            .iter()
            .map(|(id, m)| (id.clone(), m.clone()))
            .collect();
        for (id, game) in games {
            let mut game = game.lock().await;
            self.process_due(&id, &mut game, now)?;
        }
        Ok(())
    }
    fn process_due(&self, id: &str, game: &mut Match, now: u64) -> Result<(), HostError> {
        let closed: Vec<_> = game
            .metadata
            .seats
            .iter()
            .filter_map(|(&seat, m)| {
                (m.connected && game.channels[&seat].generation.receiver_count() == 0)
                    .then_some(seat)
            })
            .collect();
        if closed.is_empty()
            && game.metadata.deadline_ms.is_none_or(|at| at > now)
            && !game
                .metadata
                .seats
                .values()
                .any(|m| !m.connected && m.grace_until.is_some_and(|at| at <= now))
        {
            return Ok(());
        }
        let mut next = game.session.clone();
        let mut meta = game.metadata.clone();
        let mut changed = false;
        let mut updates = Vec::new();
        // A failed disconnect write is retried by the clock after the socket lease is dropped.
        for seat in closed {
            let m = meta.seats.get_mut(&seat).ok_or(HostError::Internal)?;
            m.connected = false;
            m.grace_until = Some(now.saturating_add(self.config.disconnect_grace_ms));
            let (staged, more) = game.presence(next, seat, false)?;
            next = staged;
            updates.extend(more);
            changed = true;
        }
        for (&seat, m) in &mut meta.seats {
            if !m.connected && m.grace_until.is_some_and(|at| at <= now) {
                m.grace_until = None;
                if m.desired_human && !next.finished() {
                    let transition = next.stage_controller(seat, false)?;
                    next = transition.next;
                    updates.extend(transition.updates);
                    m.desired_human = false;
                }
                changed = true;
            }
        }
        if meta.deadline_ms.is_some_and(|at| at <= now) {
            if let Some(transition) = next.stage_timeout(meta.deadline_turn)? {
                next = transition.next;
                updates.extend(transition.updates);
            }
            // Finished games clear their clock; advanced rounds get one new full deadline.
            game.deadline_for(&next, &mut meta, now);
            changed = true;
        }
        if changed {
            self.store.persist(id, &next, &meta, None)?;
            game.session = next;
            game.metadata = meta;
            game.publish(updates);
        }
        Ok(())
    }
    /// Call once the server and clock have stopped: takes every match's lock, so no command or
    /// clock decision is mid-commit, then checkpoints the store.
    pub async fn flush(&self) -> Result<(), HostError> {
        let _lobby = self.lobby.lock().await;
        let matches: Vec<_> = self.matches.read().await.values().cloned().collect();
        let mut held = Vec::with_capacity(matches.len());
        for game in &matches {
            held.push(game.lock().await);
        }
        self.store.checkpoint()
    }
    /// Run this alongside the HTTP server. Dropping/aborting the task stops the clock.
    pub async fn run_clock(self: Arc<Self>) {
        let mut interval = tokio::time::interval(std::time::Duration::from_millis(250));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            if let Err(e) = self.tick_at(now_ms()).await {
                eprintln!("match clock: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use civ_sim::protocol::{Action, CommandStatus};

    async fn game(path: &Path) -> (Arc<Host>, String, String, String) {
        let host = Host::open(path, Config::default()).unwrap();
        let a = "owner".to_string();
        let b = "friend".to_string();
        host.store.create_guest(&a, "hash-a").unwrap();
        host.store.create_guest(&b, "hash-b").unwrap();
        let room = host
            .create_room(
                &a,
                Settings {
                    seed: 7,
                    majors: 2,
                    city_states: 0,
                    ..Settings::default()
                },
                5,
            )
            .await
            .unwrap();
        host.join(&b, &room.invite).await.unwrap();
        let room = host.start(&a, &room.id).await.unwrap();
        (host, room.match_id.unwrap(), a, b)
    }
    fn finish(update: &GameUpdate, id: &str) -> CommandRequest {
        CommandRequest {
            command_id: id.into(),
            epoch: update.cursor.epoch.clone(),
            turn_token: update.turn_token.clone(),
            action: Action::FinishTurn,
        }
    }

    #[tokio::test]
    async fn commit_receipt_restart_retry_and_fence_are_durable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.sqlite");
        let (host, id, a, _) = game(&path).await;
        let lease = host.connect(&id, &a).await.unwrap();
        let request = finish(&lease.update, "finish-first");
        let receipt = host
            .command(&id, 0, lease.generation, request.clone())
            .await
            .unwrap();
        assert_eq!(receipt.result, CommandStatus::Applied);
        let newer = host.connect(&id, &a).await.unwrap();
        assert!(matches!(
            host.command(&id, 0, lease.generation, request.clone())
                .await,
            Err(HostError::Fenced)
        ));
        assert_eq!(
            host.command(&id, 0, newer.generation, request.clone())
                .await
                .unwrap(),
            receipt
        );
        let db = rusqlite::Connection::open(&path).unwrap();
        let count: i64 = db
            .query_row("SELECT COUNT(*) FROM receipts", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 1);
        drop(db);
        drop(host);
        let restarted = Host::open(&path, Config::default()).unwrap();
        let after = restarted.connect(&id, &a).await.unwrap();
        assert!(after.update.ready);
        assert!(after.generation > newer.generation);
        assert_eq!(
            restarted
                .command(&id, 0, after.generation, request)
                .await
                .unwrap(),
            receipt
        );
        let game = restarted.game(&id).await.unwrap();
        game.lock().await.session.verify_replay().unwrap();
    }

    #[tokio::test]
    async fn failed_commit_never_replaces_world_or_acknowledges_and_retry_can_succeed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.sqlite");
        let (host, id, a, _) = game(&path).await;
        let lease = host.connect(&id, &a).await.unwrap();
        let request = finish(&lease.update, "failure-retry");
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch("CREATE TRIGGER fail_receipt BEFORE INSERT ON receipts BEGIN SELECT RAISE(ABORT,'failure injection'); END;").unwrap();
        assert!(matches!(
            host.command(&id, 0, lease.generation, request.clone())
                .await,
            Err(HostError::Internal)
        ));
        assert!(!host.update(&id, 0, lease.generation).await.unwrap().ready);
        let stored: String = db
            .query_row("SELECT snapshot FROM matches WHERE id=?1", [&id], |r| {
                r.get(0)
            })
            .unwrap();
        assert!(
            !Session::restore(&stored)
                .unwrap()
                .current_update(0)
                .unwrap()
                .ready
        );
        db.execute_batch("DROP TRIGGER fail_receipt;").unwrap();
        assert_eq!(
            host.command(&id, 0, lease.generation, request)
                .await
                .unwrap()
                .result,
            CommandStatus::Applied
        );
    }

    #[tokio::test]
    async fn timeout_runs_once_and_grace_takeover_return_wait_for_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let (host, id, a, b) = game(&dir.path().join("state.sqlite")).await;
        let one = host.connect(&id, &a).await.unwrap();
        let two = host.connect(&id, &b).await.unwrap();
        host.disconnect(&id, 0, one.generation).await.unwrap();
        let game = host.game(&id).await.unwrap();
        let (turn, grace) = {
            let mut game = game.lock().await;
            let grace = game.metadata.seats[&0].grace_until.unwrap();
            // Exercise grace before the deadline independently of wall-clock timing.
            game.metadata.deadline_ms = Some(grace + 10_000);
            (game.session.turn(), grace)
        };
        host.tick_at(grace).await.unwrap();
        {
            let game = game.lock().await;
            let update = game.session.current_update(0).unwrap();
            assert!(update.controlled);
            assert_eq!(update.pending_control, Some(false));
            assert_eq!(game.session.turn(), turn);
        }
        host.tick_at(grace + 10_000).await.unwrap();
        host.tick_at(grace + 10_000).await.unwrap();
        {
            let game = game.lock().await;
            assert_eq!(game.session.turn(), turn + 1);
            assert!(!game.session.current_update(0).unwrap().controlled);
        }
        let returned = host.connect(&id, &a).await.unwrap();
        assert!(!returned.update.controlled);
        assert_eq!(returned.update.pending_control, Some(true));
        assert!(host
            .query(&id, 1, two.generation, Query::View)
            .await
            .is_ok());
        host.tick_at(grace + 15_000).await.unwrap();
        let update = host.update(&id, 0, returned.generation).await.unwrap();
        assert_eq!(update.view.turn, turn + 2);
        assert!(update.controlled);
        assert_eq!(update.pending_control, None);
        game.lock().await.session.verify_replay().unwrap();
    }

    #[tokio::test]
    async fn command_after_deadline_cannot_overtake_clock_and_retry_still_recovers() {
        let dir = tempfile::tempdir().unwrap();
        let (host, id, a, _) = game(&dir.path().join("state.sqlite")).await;
        let one = host.connect(&id, &a).await.unwrap();
        let original = finish(&one.update, "before-expiry");
        let receipt = host
            .command(&id, 0, one.generation, original.clone())
            .await
            .unwrap();
        let game = host.game(&id).await.unwrap();
        game.lock().await.metadata.deadline_ms = Some(now_ms().saturating_sub(1));
        let stale = host
            .command(&id, 0, one.generation, finish(&one.update, "after-expiry"))
            .await
            .unwrap();
        assert_eq!(
            stale.result,
            CommandStatus::Rejected {
                code: RejectionCode::StaleTurn
            }
        );
        assert_eq!(
            host.command(&id, 0, one.generation, original)
                .await
                .unwrap(),
            receipt
        );
        assert_eq!(game.lock().await.session.turn(), one.update.view.turn + 1);
        host.tick_at(now_ms()).await.unwrap();
        assert_eq!(game.lock().await.session.turn(), one.update.view.turn + 1);
    }

    #[test]
    fn database_has_one_live_executor_even_through_an_alias() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.sqlite");
        let host = Host::open(&path, Config::default()).unwrap();
        assert!(matches!(
            Host::open(&path, Config::default()),
            Err(HostError::Conflict)
        ));
        #[cfg(unix)]
        {
            let alias = dir.path().join("alias.sqlite");
            std::os::unix::fs::symlink(&path, &alias).unwrap();
            assert!(matches!(
                Host::open(alias, Config::default()),
                Err(HostError::Conflict)
            ));
        }
        drop(host);
        assert!(Host::open(path, Config::default()).is_ok());
    }

    #[tokio::test]
    async fn pending_controller_and_round_deadline_survive_restart() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.sqlite");
        let (host, id, a, b) = game(&path).await;
        let one = host.connect(&id, &a).await.unwrap();
        let _two = host.connect(&id, &b).await.unwrap();
        host.disconnect(&id, 0, one.generation).await.unwrap();
        let game = host.game(&id).await.unwrap();
        let at = game.lock().await.metadata.seats[&0].grace_until.unwrap();
        game.lock().await.metadata.deadline_ms = Some(at + 30_000);
        host.tick_at(at).await.unwrap();
        drop(game);
        drop(host);
        let restored = Host::open(path, Config::default()).unwrap();
        let game = restored.game(&id).await.unwrap();
        let game = game.lock().await;
        assert_eq!(game.metadata.deadline_ms, Some(at + 30_000));
        assert_eq!(game.metadata.seats[&0].grace_until, None);
        assert!(!game.metadata.seats[&0].desired_human);
        let update = game.session.current_update(0).unwrap();
        assert!(update.controlled);
        assert_eq!(update.pending_control, Some(false));
        game.session.verify_replay().unwrap();
    }

    #[tokio::test]
    async fn every_member_sees_who_is_connected_ready_and_named() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.sqlite");
        let (host, id, a, b) = game(&path).await;
        host.rename(&a, "Ada").await.unwrap();
        let mut two = host.connect(&id, &b).await.unwrap();
        let seats = two.update.seats.clone().unwrap();
        assert_eq!((seats[0].connected, seats[1].connected), (Some(false), Some(true)));
        assert_eq!((seats[0].name.as_deref(), seats[1].name.as_deref()), (Some("Ada"), None));
        let one = host.connect(&id, &a).await.unwrap();
        let heard = latest(&mut two.updates);
        assert_eq!(heard.seats.as_ref().unwrap()[0].connected, Some(true), "the other seat is told");
        host.command(&id, 0, one.generation, finish(&one.update, "done")).await.unwrap();
        let heard = latest(&mut two.updates);
        assert!(heard.seats.as_ref().unwrap()[0].ready);
        host.disconnect(&id, 0, one.generation).await.unwrap();
        let heard = latest(&mut two.updates);
        assert_eq!(heard.seats.as_ref().unwrap()[0].connected, Some(false));
        let states = host.seat_states(&id).await.unwrap();
        assert_eq!(states[0].name.as_deref(), Some("Ada"));
        assert!(states[0].ready && states[0].connected == Some(false));
        drop(host);
        let restarted = Host::open(&path, Config::default()).unwrap();
        let game = restarted.game(&id).await.unwrap();
        let game = game.lock().await;
        assert_eq!(game.names.get(&0).map(String::as_str), Some("Ada"), "names come back with the store");
        assert!(game.session.seat_status().iter().all(|s| s.connected == Some(false)), "nobody is connected after a restart");
        game.session.verify_replay().unwrap();
    }

    #[tokio::test]
    async fn guests_leave_lobbies_and_matches_and_find_their_rooms_again() {
        let dir = tempfile::tempdir().unwrap();
        let host = Host::open(dir.path().join("state.sqlite"), Config::default()).unwrap();
        let (a, b, c) = ("owner".to_string(), "friend".to_string(), "late".to_string());
        for (guest, hash) in [(&a, "hash-a"), (&b, "hash-b"), (&c, "hash-c")] {
            host.store.create_guest(guest, hash).unwrap();
        }
        let settings = Settings { seed: 7, majors: 3, city_states: 0, ..Settings::default() };
        // Before the start: a member gives up its seat, and the next guest takes the lowest free one.
        let room = host.create_room(&a, settings.clone(), 30).await.unwrap();
        host.join(&b, &room.invite).await.unwrap();
        host.choose_seat(&b, &room.id, Some(host_nation(&room)), None).await.unwrap();
        host.leave(&b, &room.id).await.unwrap();
        let left = host.store.room(&room.id).unwrap();
        assert_eq!(left.seats, vec![(0, a.clone())]);
        assert!(left.settings.seats.unwrap_or_default().iter().all(|c| c.seat != 1), "its nation is free again");
        let joined = host.join(&c, &room.invite).await.unwrap();
        assert!(joined.seats.contains(&(1, c.clone())));
        assert!(matches!(host.leave(&b, &room.id).await, Err(HostError::Forbidden)));
        // The owner closes a room that never started.
        let closed = host.create_room(&a, settings.clone(), 30).await.unwrap();
        host.leave(&a, &closed.id).await.unwrap();
        assert!(matches!(host.store.room(&closed.id), Err(HostError::NotFound)));
        assert_eq!(host.store.rooms_of(&a).unwrap().iter().map(|r| r.0.clone()).collect::<Vec<_>>(), vec![room.id.clone()]);
        // After the start: the seat goes to a bot for good.
        let started = host.start(&a, &room.id).await.unwrap();
        let id = started.match_id.unwrap();
        let lease = host.connect(&id, &c).await.unwrap();
        host.leave(&c, &room.id).await.unwrap();
        assert!(*lease.fence.borrow() != lease.generation, "its connection is closed");
        assert!(matches!(host.connect(&id, &c).await, Err(HostError::Forbidden)));
        assert!(matches!(host.check_seat(&id, &c).await, Err(HostError::Forbidden)));
        assert!(host.match_summary(&id, 1).await.unwrap().unwrap().2, "the room shows it left");
        let game = host.game(&id).await.unwrap();
        let update = game.lock().await.session.current_update(1).unwrap();
        assert_eq!(update.pending_control, Some(false), "a bot takes over at the boundary");
        let owner = host.connect(&id, &a).await.unwrap();
        assert_eq!(owner.update.seats.unwrap()[1].left, Some(true));
        game.lock().await.session.verify_replay().unwrap();
    }

    /// The newest update queued for a seat; panics when nothing was sent.
    fn latest(updates: &mut broadcast::Receiver<GameUpdate>) -> GameUpdate {
        let mut last = updates.try_recv().expect("an update was sent");
        while let Ok(next) = updates.try_recv() {
            last = next;
        }
        last
    }

    fn host_nation(room: &Room) -> String {
        let rules = civ_sim::content::shared_ruleset(room.settings.ruleset_name()).unwrap();
        rules.nations.iter().find(|(_, n)| n.is_major() && n.in_waves(rules.max_wave)).unwrap().1.name.clone()
    }

    #[test]
    fn a_version_one_database_gains_display_names() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("old.sqlite");
        let db = rusqlite::Connection::open(&path).unwrap();
        db.execute_batch(
            "CREATE TABLE guests(id TEXT PRIMARY KEY, token_hash TEXT UNIQUE NOT NULL);
             INSERT INTO guests VALUES ('old','hash-old');
             PRAGMA user_version=1;",
        )
        .unwrap();
        drop(db);
        let host = Host::open(&path, Config::default()).unwrap();
        assert_eq!(host.store.name("old").unwrap(), None);
        host.store.set_name("old", "Grace").unwrap();
        assert_eq!(host.store.name("old").unwrap().as_deref(), Some("Grace"));
        host.store.create_guest("new", "hash-new").unwrap();
        drop(host);
        let version: u32 =
            rusqlite::Connection::open(&path).unwrap().query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, 2);
    }
}
