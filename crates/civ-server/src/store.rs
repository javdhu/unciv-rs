use crate::host::{HostError, Metadata};
use civ_sim::{protocol::CommandReceipt, session::Session, Settings};
use rusqlite::{params, Connection, OptionalExtension};
use std::{
    fs::{File, OpenOptions},
    path::Path,
    sync::{Mutex, MutexGuard},
    time::Duration,
};

pub(crate) struct Store {
    db: Mutex<Connection>,
    // Held for the Store lifetime. Two executors must never serve the same durable matches.
    _owner_lock: File,
}

pub(crate) struct Room {
    pub id: String,
    pub owner: String,
    pub invite: String,
    pub settings: Settings,
    pub turn_seconds: u32,
    pub match_id: Option<String>,
    pub seats: Vec<(u8, String)>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, HostError> {
        let mut options = OpenOptions::new();
        options.create(true).truncate(false).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        // Canonicalize after creation so relative paths and symlinks agree on the same lock file.
        let database_file = options.open(path).map_err(|_| HostError::Internal)?;
        let canonical = std::fs::canonicalize(path).map_err(|_| HostError::Internal)?;
        let mut lock_path = canonical.into_os_string();
        lock_path.push(".host-lock");
        let owner_lock = options.open(lock_path).map_err(|_| HostError::Internal)?;
        owner_lock.try_lock().map_err(|_| HostError::Conflict)?;
        drop(database_file);
        let connection = Connection::open(path)?;
        connection.busy_timeout(Duration::from_secs(5))?;
        let version: u32 = connection.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version > 2 {
            return Err(HostError::Internal);
        }
        connection.execute_batch(
            "PRAGMA foreign_keys=ON;
             PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             CREATE TABLE IF NOT EXISTS guests(id TEXT PRIMARY KEY, token_hash TEXT UNIQUE NOT NULL);
             CREATE TABLE IF NOT EXISTS rooms(
                id TEXT PRIMARY KEY, owner TEXT NOT NULL REFERENCES guests(id),
                invite TEXT UNIQUE NOT NULL, settings TEXT NOT NULL,
                turn_seconds INTEGER NOT NULL, match_id TEXT UNIQUE);
             CREATE TABLE IF NOT EXISTS seats(
                room_id TEXT NOT NULL REFERENCES rooms(id), seat INTEGER NOT NULL,
                guest TEXT NOT NULL REFERENCES guests(id), PRIMARY KEY(room_id,seat), UNIQUE(room_id,guest));
             CREATE TABLE IF NOT EXISTS matches(
                id TEXT PRIMARY KEY, room_id TEXT UNIQUE NOT NULL REFERENCES rooms(id),
                snapshot BLOB NOT NULL, metadata TEXT NOT NULL);
             CREATE TABLE IF NOT EXISTS receipts(
                match_id TEXT NOT NULL REFERENCES matches(id), seat INTEGER NOT NULL,
                epoch TEXT NOT NULL, command_id TEXT NOT NULL, receipt TEXT NOT NULL,
                PRIMARY KEY(match_id,seat,epoch,command_id));"
        )?;
        // Version 2: guests may carry a display name.
        if version < 2 {
            let named: bool = connection
                .prepare("SELECT 1 FROM pragma_table_info('guests') WHERE name='name'")?
                .exists([])?;
            if !named {
                connection.execute_batch("ALTER TABLE guests ADD COLUMN name TEXT;")?;
            }
        }
        connection.execute_batch("PRAGMA user_version=2;")?;
        Ok(Self {
            db: Mutex::new(connection),
            _owner_lock: owner_lock,
        })
    }
    /// Folds the WAL back into the database file, so a stopped host leaves one self-contained file.
    /// Every commit is already durable; this only makes the file tidy for a copy or a volume snapshot.
    pub fn checkpoint(&self) -> Result<(), HostError> {
        let db = self.lock()?;
        let busy: i64 = db.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |r| r.get(0))?;
        if busy != 0 {
            return Err(HostError::Conflict);
        }
        Ok(())
    }
    fn lock(&self) -> Result<MutexGuard<'_, Connection>, HostError> {
        self.db.lock().map_err(|_| HostError::Internal)
    }
    pub fn guest(&self, hash: &str) -> Result<Option<String>, HostError> {
        Ok(self
            .lock()?
            .query_row("SELECT id FROM guests WHERE token_hash=?1", [hash], |r| {
                r.get(0)
            })
            .optional()?)
    }
    pub fn create_guest(&self, id: &str, hash: &str) -> Result<(), HostError> {
        let db = self.lock()?;
        let count: i64 = db.query_row("SELECT COUNT(*) FROM guests", [], |r| r.get(0))?;
        if count >= 10_000 {
            return Err(HostError::Capacity);
        }
        db.execute("INSERT INTO guests(id,token_hash) VALUES (?1,?2)", params![id, hash])?;
        Ok(())
    }
    /// A guest's display name, if it chose one.
    pub fn name(&self, guest: &str) -> Result<Option<String>, HostError> {
        Ok(self
            .lock()?
            .query_row("SELECT name FROM guests WHERE id=?1", [guest], |r| r.get::<_, Option<String>>(0))
            .optional()?
            .flatten())
    }
    pub fn set_name(&self, guest: &str, name: &str) -> Result<(), HostError> {
        if self.lock()?.execute("UPDATE guests SET name=?1 WHERE id=?2", params![name, guest])? != 1 {
            return Err(HostError::NotFound);
        }
        Ok(())
    }
    /// Every room this guest holds a seat in, newest rooms last by id order:
    /// `(room, match, seat, owner)`.
    pub fn rooms_of(&self, guest: &str) -> Result<Vec<(String, Option<String>, u8, bool)>, HostError> {
        let db = self.lock()?;
        let mut stmt = db.prepare(
            "SELECT rooms.id, rooms.match_id, seats.seat, rooms.owner=?1 FROM seats JOIN rooms ON rooms.id=seats.room_id
             WHERE seats.guest=?1 ORDER BY rooms.id",
        )?;
        let rows = stmt
            .query_map([guest], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
    /// A guest gives up its seat before the match starts; its nation choice
    /// goes with it. Caller holds the lobby mutex.
    pub fn leave(&self, room: &Room, seat: u8) -> Result<(), HostError> {
        let mut settings = room.settings.clone();
        if let Some(seats) = settings.seats.as_mut() {
            seats.retain(|c| c.seat != seat);
        }
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        tx.execute("DELETE FROM seats WHERE room_id=?1 AND seat=?2", params![room.id, seat])?;
        if tx.execute(
            "UPDATE rooms SET settings=?1 WHERE id=?2 AND match_id IS NULL",
            params![serde_json::to_string(&settings)?, room.id],
        )? != 1
        {
            return Err(HostError::Conflict);
        }
        tx.commit()?;
        Ok(())
    }
    /// The owner closes a room that never started. Caller holds the lobby mutex.
    pub fn close(&self, room: &str) -> Result<(), HostError> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        tx.execute("DELETE FROM seats WHERE room_id=?1", [room])?;
        if tx.execute("DELETE FROM rooms WHERE id=?1 AND match_id IS NULL", [room])? != 1 {
            return Err(HostError::Conflict);
        }
        tx.commit()?;
        Ok(())
    }
    pub fn create_room(&self, room: &Room) -> Result<(), HostError> {
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        let count: i64 = tx.query_row("SELECT COUNT(*) FROM rooms", [], |r| r.get(0))?;
        let own: i64 = tx.query_row(
            "SELECT COUNT(*) FROM rooms WHERE owner=?1",
            [&room.owner],
            |r| r.get(0),
        )?;
        if count >= 1000 || own >= 20 {
            return Err(HostError::Capacity);
        }
        tx.execute(
            "INSERT INTO rooms VALUES (?1,?2,?3,?4,?5,NULL)",
            params![
                room.id,
                room.owner,
                room.invite,
                serde_json::to_string(&room.settings)?,
                room.turn_seconds
            ],
        )?;
        tx.execute(
            "INSERT INTO seats VALUES (?1,0,?2)",
            params![room.id, room.owner],
        )?;
        tx.commit()?;
        Ok(())
    }
    pub fn room(&self, id: &str) -> Result<Room, HostError> {
        let db = self.lock()?;
        let values: Option<(String, String, String, u32, Option<String>)> = db
            .query_row(
                "SELECT owner,invite,settings,turn_seconds,match_id FROM rooms WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .optional()?;
        let (owner, invite, settings, turn_seconds, match_id) =
            values.ok_or(HostError::NotFound)?;
        let mut stmt = db.prepare("SELECT seat,guest FROM seats WHERE room_id=?1 ORDER BY seat")?;
        let seats = stmt
            .query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Room {
            id: id.into(),
            owner,
            invite,
            settings: serde_json::from_str(&settings)?,
            turn_seconds,
            match_id,
            seats,
        })
    }
    pub fn invite_room(&self, invite: &str) -> Result<String, HostError> {
        self.lock()?
            .query_row("SELECT id FROM rooms WHERE invite=?1", [invite], |r| {
                r.get(0)
            })
            .optional()?
            .ok_or(HostError::NotFound)
    }
    /// A lobby change to a room's settings (a seat's nation or leader).
    /// Caller holds the lobby mutex; a started room is never changed.
    pub fn update_settings(&self, room: &str, settings: &civ_sim::Settings) -> Result<(), HostError> {
        if self.lock()?.execute(
            "UPDATE rooms SET settings=?1 WHERE id=?2 AND match_id IS NULL",
            params![serde_json::to_string(settings)?, room],
        )? != 1
        {
            return Err(HostError::Conflict);
        }
        Ok(())
    }
    /// Caller holds the lobby mutex, including its read of room capacity/start state.
    pub fn join(&self, room: &str, seat: u8, guest: &str) -> Result<(), HostError> {
        self.lock()?.execute(
            "INSERT INTO seats VALUES (?1,?2,?3)",
            params![room, seat, guest],
        )?;
        Ok(())
    }
    pub fn start(
        &self,
        room: &str,
        id: &str,
        session: &Session,
        metadata: &Metadata,
    ) -> Result<(), HostError> {
        let snapshot = session.save()?;
        let metadata = serde_json::to_string(metadata)?;
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        tx.execute(
            "INSERT INTO matches VALUES (?1,?2,?3,?4)",
            params![id, room, snapshot, metadata],
        )?;
        if tx.execute(
            "UPDATE rooms SET match_id=?1 WHERE id=?2 AND match_id IS NULL",
            params![id, room],
        )? != 1
        {
            return Err(HostError::Conflict);
        }
        tx.commit()?;
        Ok(())
    }
    pub fn matches(&self) -> Result<Vec<(String, String, Vec<u8>, String)>, HostError> {
        let db = self.lock()?;
        let mut stmt =
            db.prepare("SELECT id,room_id,snapshot,metadata FROM matches ORDER BY id")?;
        let rows = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }
    /// Snapshot, controller/deadline metadata, and the receipt commit together before ACK/publication.
    pub fn persist(
        &self,
        id: &str,
        session: &Session,
        metadata: &Metadata,
        receipt: Option<(u8, &str, &CommandReceipt)>,
    ) -> Result<(), HostError> {
        let snapshot = session.save()?;
        let metadata = serde_json::to_string(metadata)?;
        let mut db = self.lock()?;
        let tx = db.transaction()?;
        if tx.execute(
            "UPDATE matches SET snapshot=?1,metadata=?2 WHERE id=?3",
            params![snapshot, metadata, id],
        )? != 1
        {
            return Err(HostError::NotFound);
        }
        if let Some((seat, epoch, receipt)) = receipt {
            tx.execute(
                "INSERT INTO receipts VALUES (?1,?2,?3,?4,?5)",
                params![
                    id,
                    seat,
                    epoch,
                    receipt.command_id,
                    serde_json::to_string(receipt)?
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }
}
