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
}
impl Match {
    fn new(session: Session, metadata: Metadata) -> Self {
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
        update
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
            for seat in metadata.seats.values_mut() {
                if seat.connected {
                    seat.connected = false;
                    seat.grace_until = Some(now.saturating_add(config.disconnect_grace_ms));
                }
            }
            store.persist(&id, &session, &metadata, None)?;
            matches.insert(id, Arc::new(Mutex::new(Match::new(session, metadata))));
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
        let seat = u8::try_from(room.seats.len()).map_err(|_| HostError::Capacity)?;
        self.store.join(&id, seat, guest)?;
        room.seats.push((seat, guest.into()));
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
                        },
                    )
                })
                .collect(),
        };
        self.store.start(&room.id, &id, &session, &metadata)?;
        self.matches.write().await.insert(
            id.clone(),
            Arc::new(Mutex::new(Match::new(session, metadata))),
        );
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
        if game.metadata.seats.values().any(|m| m.guest == guest) {
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
            .find_map(|(&seat, m)| (m.guest == guest).then_some(seat))
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
        self.store.persist(id, &next, &meta, None)?;
        game.session = next;
        game.metadata = meta;
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
        self.store.persist(id, &game.session, &meta, None)?;
        game.metadata = meta;
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
}
