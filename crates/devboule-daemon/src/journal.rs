//! Conversation journal: SQLite WAL, one writer thread.
//!
//! The PTY reader and coalesce threads never wait here. They `try_send` a
//! record into a bounded channel. If the channel is full or the disk is
//! full, the session is marked degraded and the live terminal continues.
//! A recovered session then replays a prefix — not a hang, and not a lie.
//!
//! Schema notes for M6: `events.kind` is an open string (`output`, `exit`,
//! later `turn` / `permission`). Additive columns on `sessions` and the
//! empty `turns` / `permissions` tables mean agent history does not require
//! a migration that rewrites terminal rows.

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};

use devboule_protocol::{ErrorCode, Session, SessionEvent, SessionKind, SessionState, WireError};

/// Stored in `PRAGMA user_version`. Bump only when existing tables change
/// shape. Additive tables and columns keep this number.
pub const JOURNAL_SCHEMA_VERSION: i32 = 1;

/// Bounded journal queue. Each slot is one coalesced frame (typically
/// ≤ 8 KiB). A full queue never blocks the PTY path.
pub const JOURNAL_QUEUE_CAP: usize = 1024;

/// Take a snapshot after this many payload bytes since the last one.
pub const SNAPSHOT_EVERY_BYTES: u64 = 64 * 1024;

/// Per-session cap on snapshot + event payload. Oldest windows go first.
/// The user loses the start of that session's scrollback, never a hole in
/// the middle of a replay already loaded into memory. 16 MiB keeps a
/// measured 13 MB ConPTY flood intact and still bounds a runaway dump.
pub const JOURNAL_SESSION_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// Drop the oldest unpinned non-live sessions when the logical payload
/// exceeds this.
pub const JOURNAL_MAX_BYTES: u64 = 64 * 1024 * 1024;

/// Maximum retained sessions, closed ones included. Oldest unpinned
/// non-live go first.
pub const JOURNAL_MAX_SESSIONS: usize = 50;

/// Age cap. The user loses recovered transcripts older than this.
pub const JOURNAL_MAX_AGE_MS: u64 = 7 * 24 * 60 * 60 * 1000;

const RPC_WAIT: Duration = Duration::from_secs(10);
const JOIN_BUDGET: Duration = Duration::from_millis(500);
/// Keep room for the degradation, reaped, and ended control records even
/// while output is arriving faster than SQLite can commit it.
const CONTROL_RESERVE: usize = 3;

#[derive(Clone, Copy, Debug)]
pub struct JournalLimits {
    pub snapshot_every_bytes: u64,
    pub session_max_bytes: u64,
    pub max_bytes: u64,
    pub max_sessions: usize,
    pub max_age_ms: u64,
}

impl Default for JournalLimits {
    fn default() -> Self {
        Self {
            snapshot_every_bytes: SNAPSHOT_EVERY_BYTES,
            session_max_bytes: JOURNAL_SESSION_MAX_BYTES,
            max_bytes: JOURNAL_MAX_BYTES,
            max_sessions: JOURNAL_MAX_SESSIONS,
            max_age_ms: JOURNAL_MAX_AGE_MS,
        }
    }
}

#[derive(Debug)]
pub enum JournalError {
    FutureSchema { found: i32, supported: i32 },
    Corrupt(String),
    Unavailable(String),
    SessionNotFound,
    Checksum { session_id: String, seq: u64 },
    Stopped,
}

impl fmt::Display for JournalError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FutureSchema { found, supported } => write!(
                formatter,
                "journal schema is version {found}; this daemon reads version {supported}"
            ),
            Self::Corrupt(message) => write!(formatter, "journal is corrupt: {message}"),
            Self::Unavailable(message) => write!(formatter, "journal is unavailable: {message}"),
            Self::SessionNotFound => write!(formatter, "No session with that id."),
            Self::Checksum { session_id, seq } => {
                write!(
                    formatter,
                    "journal checksum mismatch for {session_id} seq {seq}"
                )
            }
            Self::Stopped => write!(formatter, "journal writer has stopped"),
        }
    }
}

impl std::error::Error for JournalError {}

impl From<JournalError> for WireError {
    fn from(error: JournalError) -> Self {
        let code = match error {
            JournalError::SessionNotFound => ErrorCode::SessionNotFound,
            _ => ErrorCode::Journal,
        };
        WireError::new(code, error.to_string())
    }
}

impl From<rusqlite::Error> for JournalError {
    fn from(error: rusqlite::Error) -> Self {
        let message = error.to_string();
        let lower = message.to_ascii_lowercase();
        if lower.contains("not a database")
            || lower.contains("corrupt")
            || lower.contains("malformed")
            || lower.contains("disk image is malformed")
        {
            Self::Corrupt(message)
        } else {
            Self::Unavailable(message)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PersistStatus {
    Live,
    Ended,
    Interrupted,
}

impl PersistStatus {
    fn as_str(self) -> &'static str {
        match self {
            Self::Live => "live",
            Self::Ended => "ended",
            Self::Interrupted => "interrupted",
        }
    }

    fn parse(value: &str) -> Self {
        match value {
            "ended" => Self::Ended,
            "interrupted" => Self::Interrupted,
            _ => Self::Live,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SessionRecord {
    pub id: String,
    pub owner: String,
    pub workspace_id: Option<String>,
    pub kind: SessionKind,
    pub title: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub generation: u64,
    pub status: PersistStatus,
    pub exit_code: Option<u32>,
    pub closed: bool,
    pub last_seq: u64,
    pub degraded: bool,
    pub payload_bytes: u64,
    /// Child::wait returned. Output may still be arriving (ConPTY drain).
    pub reaped: bool,
}

impl SessionRecord {
    pub fn to_session(&self) -> Session {
        let state = match self.status {
            PersistStatus::Live if self.reaped => SessionState::Ended {
                generation: self.generation,
                code: self.exit_code,
            },
            PersistStatus::Live => SessionState::Live {
                generation: self.generation,
            },
            PersistStatus::Ended => SessionState::Ended {
                generation: self.generation,
                code: self.exit_code,
            },
            PersistStatus::Interrupted => SessionState::Recovered {
                generation: self.generation,
                truncated: self.degraded,
            },
        };
        Session {
            id: self.id.clone(),
            workspace_id: self.workspace_id.clone(),
            kind: self.kind.clone(),
            title: self.title.clone(),
            state,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    Output,
    Exit,
}

impl EventKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Output => "output",
            Self::Exit => "exit",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "output" => Some(Self::Output),
            "exit" => Some(Self::Exit),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct EventRecord {
    pub session_id: String,
    pub generation: u64,
    pub seq: u64,
    pub kind: EventKind,
    pub ts_ms: u64,
    pub payload: Vec<u8>,
}

#[derive(Debug)]
pub struct Replay {
    pub generation: u64,
    pub events: Vec<SessionEvent>,
    pub last_seq: u64,
    pub truncated: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JournalStatsSnapshot {
    pub accepted_frames: u64,
    pub accepted_bytes: u64,
    pub committed_frames: u64,
    pub committed_bytes: u64,
    pub failed_frames: u64,
}

#[derive(Default)]
struct JournalStats {
    accepted_frames: AtomicU64,
    accepted_bytes: AtomicU64,
    committed_frames: AtomicU64,
    committed_bytes: AtomicU64,
    failed_frames: AtomicU64,
}

impl JournalStats {
    fn snapshot(&self) -> JournalStatsSnapshot {
        JournalStatsSnapshot {
            accepted_frames: self.accepted_frames.load(Ordering::Relaxed),
            accepted_bytes: self.accepted_bytes.load(Ordering::Relaxed),
            committed_frames: self.committed_frames.load(Ordering::Relaxed),
            committed_bytes: self.committed_bytes.load(Ordering::Relaxed),
            failed_frames: self.failed_frames.load(Ordering::Relaxed),
        }
    }
}

enum JournalCmd {
    Upsert(SessionRecord),
    Append(EventRecord),
    MarkReaped {
        session_id: String,
        code: Option<u32>,
        degraded: bool,
    },
    MarkEnded {
        session_id: String,
        generation: u64,
        code: Option<u32>,
        degraded: bool,
    },
    MarkClosed {
        session_id: String,
    },
    MarkDegraded {
        session_id: String,
    },
    List {
        reply: mpsc::Sender<Result<Vec<SessionRecord>, JournalError>>,
    },
    Replay {
        session_id: String,
        from_seq: u64,
        reply: mpsc::Sender<Result<Replay, JournalError>>,
    },
    Pin {
        session_id: String,
        reply: mpsc::Sender<Result<(), JournalError>>,
    },
    Unpin {
        session_id: String,
    },
    Flush {
        reply: mpsc::Sender<Result<(), JournalError>>,
    },
    FileLen {
        reply: mpsc::Sender<Result<u64, JournalError>>,
    },
    Shutdown,
}

pub struct Journal {
    tx: SyncSender<JournalCmd>,
    join: Mutex<Option<JoinHandle<()>>>,
    queued: Arc<AtomicU64>,
    degraded_sessions: Arc<Mutex<HashSet<String>>>,
    stats: Arc<JournalStats>,
    path: PathBuf,
}

impl Journal {
    pub fn open(path: &Path) -> Result<Self, JournalError> {
        Self::open_with_limits(path, JournalLimits::default())
    }

    pub fn open_with_limits(path: &Path, limits: JournalLimits) -> Result<Self, JournalError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| {
                JournalError::Unavailable(format!("could not create journal directory: {error}"))
            })?;
        }
        let conn = open_connection(path)?;
        let (tx, rx) = mpsc::sync_channel(JOURNAL_QUEUE_CAP);
        let queued = Arc::new(AtomicU64::new(0));
        let queued_thread = Arc::clone(&queued);
        let degraded_sessions = Arc::new(Mutex::new(HashSet::new()));
        let degraded_sessions_thread = Arc::clone(&degraded_sessions);
        let stats = Arc::new(JournalStats::default());
        let stats_thread = Arc::clone(&stats);
        let path_buf = path.to_path_buf();
        let thread_path = path_buf.clone();
        let join = std::thread::Builder::new()
            .name("daemon-journal".into())
            .spawn(move || {
                journal_loop(
                    conn,
                    rx,
                    queued_thread,
                    degraded_sessions_thread,
                    stats_thread,
                    limits,
                    thread_path,
                )
            })
            .map_err(|error| JournalError::Unavailable(error.to_string()))?;
        Ok(Self {
            tx,
            join: Mutex::new(Some(join)),
            queued,
            degraded_sessions,
            stats,
            path: path_buf,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn is_session_degraded(&self, session_id: &str) -> bool {
        self.degraded_sessions
            .lock()
            .map(|sessions| sessions.contains(session_id))
            .unwrap_or(true)
    }

    pub fn stats(&self) -> JournalStatsSnapshot {
        self.stats.snapshot()
    }

    /// Never blocks. On a full queue or a dead writer the session is marked
    /// degraded and the PTY path continues.
    pub fn try_upsert(&self, record: SessionRecord) {
        self.try_send(JournalCmd::Upsert(record));
    }

    /// Returns false if the queue was full or the writer is dead. The PTY
    /// path never waits; a false return is a truncated journal.
    pub fn try_append(&self, record: EventRecord) -> bool {
        let payload_len = record.payload.len() as u64;
        let session_id = record.session_id.clone();
        if !self.reserve_output_slot() {
            self.note_session_degraded(&record.session_id);
            self.stats.failed_frames.fetch_add(1, Ordering::Relaxed);
            return false;
        }
        match self.tx.try_send(JournalCmd::Append(record)) {
            Ok(()) => {
                self.stats.accepted_frames.fetch_add(1, Ordering::Relaxed);
                self.stats
                    .accepted_bytes
                    .fetch_add(payload_len, Ordering::Relaxed);
                true
            }
            Err(TrySendError::Full(_)) | Err(TrySendError::Disconnected(_)) => {
                self.release_slot();
                self.note_session_degraded(&session_id);
                self.stats.failed_frames.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    /// Child::wait returned. Does not freeze last_seq and does not write an
    /// exit row: ConPTY may still deliver drain frames that need seqs.
    pub fn try_mark_reaped(&self, session_id: &str, code: Option<u32>) {
        self.try_send(JournalCmd::MarkReaped {
            session_id: session_id.to_string(),
            code,
            degraded: self.is_session_degraded(session_id),
        });
    }

    pub fn mark_reaped(&self, session_id: &str, code: Option<u32>) -> Result<(), JournalError> {
        self.send_cmd(
            JournalCmd::MarkReaped {
                session_id: session_id.to_string(),
                code,
                degraded: self.is_session_degraded(session_id),
            },
            RPC_WAIT,
        )?;
        self.flush()
    }

    pub fn mark_ended_blocking(
        &self,
        session_id: &str,
        generation: u64,
        code: Option<u32>,
    ) -> Result<(), JournalError> {
        self.send_cmd(
            JournalCmd::MarkEnded {
                session_id: session_id.to_string(),
                generation,
                code,
                degraded: self.is_session_degraded(session_id),
            },
            RPC_WAIT,
        )?;
        self.flush()
    }

    pub fn try_mark_ended(&self, session_id: &str, generation: u64, code: Option<u32>) {
        self.try_send(JournalCmd::MarkEnded {
            session_id: session_id.to_string(),
            generation,
            code,
            degraded: self.is_session_degraded(session_id),
        });
    }

    pub fn try_mark_closed(&self, session_id: &str) {
        self.try_send(JournalCmd::MarkClosed {
            session_id: session_id.to_string(),
        });
    }

    pub fn list(&self) -> Result<Vec<SessionRecord>, JournalError> {
        self.rpc(|reply| JournalCmd::List { reply })
    }

    pub fn replay(&self, session_id: &str, from_seq: u64) -> Result<Replay, JournalError> {
        self.rpc(|reply| JournalCmd::Replay {
            session_id: session_id.to_string(),
            from_seq,
            reply,
        })
    }

    pub fn pin(&self, session_id: &str) -> Result<(), JournalError> {
        self.rpc(|reply| JournalCmd::Pin {
            session_id: session_id.to_string(),
            reply,
        })
    }

    pub fn unpin(&self, session_id: &str) {
        self.try_send(JournalCmd::Unpin {
            session_id: session_id.to_string(),
        });
    }

    pub fn flush(&self) -> Result<(), JournalError> {
        self.rpc(|reply| JournalCmd::Flush { reply })
    }

    pub fn file_len(&self) -> Result<u64, JournalError> {
        self.rpc(|reply| JournalCmd::FileLen { reply })
    }

    pub fn shutdown(&self) {
        let _ = self.send_cmd(JournalCmd::Shutdown, Duration::from_millis(200));
        let handle = self.join.lock().ok().and_then(|mut guard| guard.take());
        if let Some(handle) = handle {
            let deadline = Instant::now() + JOIN_BUDGET;
            while !handle.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(10));
            }
            if handle.is_finished() {
                let _ = handle.join();
            }
        }
    }

    /// Test helper: enqueue and wait until the row is on disk.
    pub fn append_blocking(&self, record: EventRecord) -> Result<(), JournalError> {
        self.send_cmd(JournalCmd::Append(record), RPC_WAIT)?;
        self.flush()
    }

    pub fn upsert_blocking(&self, record: SessionRecord) -> Result<(), JournalError> {
        self.send_cmd(JournalCmd::Upsert(record), RPC_WAIT)?;
        self.flush()
    }

    fn send_cmd(&self, cmd: JournalCmd, wait: Duration) -> Result<(), JournalError> {
        let deadline = Instant::now() + wait;
        let mut pending = Some(cmd);
        while Instant::now() < deadline {
            let command = pending.take().expect("pending command");
            if !self.reserve_slot() {
                pending = Some(command);
                std::thread::sleep(Duration::from_millis(5));
                continue;
            }
            match self.tx.try_send(command) {
                Ok(()) => return Ok(()),
                Err(TrySendError::Full(cmd)) => {
                    self.release_slot();
                    pending = Some(cmd);
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(TrySendError::Disconnected(_)) => {
                    self.release_slot();
                    return Err(JournalError::Stopped);
                }
            }
        }
        Err(JournalError::Stopped)
    }

    fn try_send(&self, cmd: JournalCmd) {
        if !self.reserve_slot() {
            return;
        }
        match self.tx.try_send(cmd) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => {
                self.release_slot();
            }
            Err(TrySendError::Disconnected(_)) => {
                self.release_slot();
            }
        }
    }

    fn reserve_output_slot(&self) -> bool {
        self.reserve_below(JOURNAL_QUEUE_CAP.saturating_sub(CONTROL_RESERVE))
    }

    fn reserve_slot(&self) -> bool {
        self.reserve_below(JOURNAL_QUEUE_CAP)
    }

    fn reserve_below(&self, limit: usize) -> bool {
        let limit = limit as u64;
        let mut queued = self.queued.load(Ordering::Acquire);
        loop {
            if queued >= limit {
                return false;
            }
            match self.queued.compare_exchange_weak(
                queued,
                queued + 1,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return true,
                Err(next) => queued = next,
            }
        }
    }

    fn release_slot(&self) {
        self.queued
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                Some(value.saturating_sub(1))
            })
            .ok();
    }

    pub(crate) fn note_session_degraded(&self, session_id: &str) -> bool {
        if let Ok(mut sessions) = self.degraded_sessions.lock() {
            let first = sessions.insert(session_id.to_string());
            drop(sessions);
            if first {
                self.queue_degraded_marker(session_id);
            }
            return first;
        }
        false
    }

    fn queue_degraded_marker(&self, session_id: &str) {
        self.try_send(JournalCmd::MarkDegraded {
            session_id: session_id.to_string(),
        });
    }

    fn rpc<T>(
        &self,
        make: impl FnOnce(mpsc::Sender<Result<T, JournalError>>) -> JournalCmd,
    ) -> Result<T, JournalError> {
        let (tx, rx) = mpsc::channel();
        self.send_cmd(make(tx), RPC_WAIT)?;
        match rx.recv_timeout(RPC_WAIT) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => {
                Err(JournalError::Stopped)
            }
        }
    }
}

impl Drop for Journal {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn open_connection(path: &Path) -> Result<Connection, JournalError> {
    let conn = Connection::open(path)?;
    conn.busy_timeout(Duration::from_secs(5))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    // NORMAL for the 13 MB/s-class append path. Durability of process
    // end is the checkpoint in mark_ended / Flush, not a fsync per frame.
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    let version: i32 = conn.pragma_query_value(None, "user_version", |row| row.get(0))?;
    if version > JOURNAL_SCHEMA_VERSION {
        return Err(JournalError::FutureSchema {
            found: version,
            supported: JOURNAL_SCHEMA_VERSION,
        });
    }
    if version > 0 {
        let check: String = conn.query_row("PRAGMA integrity_check", [], |row| row.get(0))?;
        if check != "ok" {
            return Err(JournalError::Corrupt(check));
        }
    }
    if version < JOURNAL_SCHEMA_VERSION {
        conn.execute_batch(SCHEMA_SQL)?;
        conn.pragma_update(None, "user_version", JOURNAL_SCHEMA_VERSION)?;
    }
    let _ = conn.execute(
        "ALTER TABLE sessions ADD COLUMN reaped INTEGER NOT NULL DEFAULT 0",
        [],
    );
    // Reaped-but-still-live: the process was observed to exit, then the
    // daemon died during ConPTY drain. That is Ended (we saw the child),
    // not Recovered (we did not lose the process unobserved).
    conn.execute(
        "UPDATE sessions SET status = 'ended' WHERE status = 'live' AND reaped = 1",
        [],
    )?;
    conn.execute(
        "UPDATE sessions SET status = 'interrupted' WHERE status = 'live'",
        [],
    )?;
    Ok(conn)
}

const SCHEMA_SQL: &str = "
CREATE TABLE IF NOT EXISTS sessions (
    id TEXT PRIMARY KEY,
    owner TEXT NOT NULL,
    workspace_id TEXT,
    kind TEXT NOT NULL,
    title TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL,
    generation INTEGER NOT NULL,
    status TEXT NOT NULL,
    exit_code INTEGER,
    closed INTEGER NOT NULL DEFAULT 0,
    last_seq INTEGER NOT NULL DEFAULT 0,
    degraded INTEGER NOT NULL DEFAULT 0,
    payload_bytes INTEGER NOT NULL DEFAULT 0,
    unsnapshotted_bytes INTEGER NOT NULL DEFAULT 0,
    reaped INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS events (
    session_id TEXT NOT NULL,
    generation INTEGER NOT NULL,
    seq INTEGER NOT NULL,
    kind TEXT NOT NULL,
    ts_ms INTEGER NOT NULL,
    payload BLOB NOT NULL,
    checksum INTEGER NOT NULL,
    PRIMARY KEY (session_id, generation, seq)
);
CREATE TABLE IF NOT EXISTS snapshots (
    session_id TEXT NOT NULL,
    generation INTEGER NOT NULL,
    from_seq INTEGER NOT NULL,
    up_to_seq INTEGER NOT NULL,
    ts_ms INTEGER NOT NULL,
    blob BLOB NOT NULL,
    checksum INTEGER NOT NULL,
    payload_bytes INTEGER NOT NULL,
    PRIMARY KEY (session_id, generation, up_to_seq)
);
CREATE INDEX IF NOT EXISTS events_session ON events(session_id, generation, seq);
CREATE INDEX IF NOT EXISTS snapshots_session ON snapshots(session_id, generation, up_to_seq);
CREATE INDEX IF NOT EXISTS sessions_updated ON sessions(updated_at_ms);
CREATE TABLE IF NOT EXISTS turns (
    session_id TEXT NOT NULL,
    generation INTEGER NOT NULL,
    turn_seq INTEGER NOT NULL,
    ts_ms INTEGER NOT NULL,
    role TEXT NOT NULL,
    payload BLOB NOT NULL,
    checksum INTEGER NOT NULL,
    PRIMARY KEY (session_id, generation, turn_seq)
);
CREATE TABLE IF NOT EXISTS permissions (
    session_id TEXT NOT NULL,
    request_id TEXT NOT NULL,
    ts_ms INTEGER NOT NULL,
    outcome TEXT,
    payload BLOB NOT NULL,
    checksum INTEGER NOT NULL,
    PRIMARY KEY (session_id, request_id)
);
";

fn journal_loop(
    conn: Connection,
    rx: mpsc::Receiver<JournalCmd>,
    queued: Arc<AtomicU64>,
    degraded_sessions: Arc<Mutex<HashSet<String>>>,
    stats: Arc<JournalStats>,
    limits: JournalLimits,
    path: PathBuf,
) {
    let mut pins: HashSet<String> = HashSet::new();
    while let Ok(cmd) = rx.recv() {
        queued
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                Some(value.saturating_sub(1))
            })
            .ok();
        match cmd {
            JournalCmd::Upsert(record) => {
                if let Err(error) = upsert_session(&conn, &record) {
                    note_degraded(&degraded_sessions, &record.id);
                    on_write_error(&error);
                }
            }
            JournalCmd::Append(record) => {
                let is_output = record.kind == EventKind::Output;
                let payload_len = record.payload.len() as u64;
                if let Err(error) = append_event(&conn, &record, &pins, limits) {
                    if is_output {
                        stats.failed_frames.fetch_add(1, Ordering::Relaxed);
                    }
                    note_degraded(&degraded_sessions, &record.session_id);
                    on_write_error(&error);
                    let _ = mark_degraded(&conn, &record.session_id);
                } else if is_output {
                    stats.committed_frames.fetch_add(1, Ordering::Relaxed);
                    stats
                        .committed_bytes
                        .fetch_add(payload_len, Ordering::Relaxed);
                }
            }
            JournalCmd::MarkReaped {
                session_id,
                code,
                degraded,
            } => {
                if let Err(error) = mark_reaped(&conn, &session_id, code, degraded) {
                    note_degraded(&degraded_sessions, &session_id);
                    on_write_error(&error);
                }
            }
            JournalCmd::MarkEnded {
                session_id,
                generation,
                code,
                degraded,
            } => {
                if let Err(error) = mark_ended(&conn, &session_id, generation, code, degraded) {
                    note_degraded(&degraded_sessions, &session_id);
                    on_write_error(&error);
                }
            }
            JournalCmd::MarkClosed { session_id } => {
                if let Err(error) = mark_closed(&conn, &session_id) {
                    note_degraded(&degraded_sessions, &session_id);
                    on_write_error(&error);
                }
            }
            JournalCmd::MarkDegraded { session_id } => {
                if let Err(error) = mark_degraded(&conn, &session_id) {
                    on_write_error(&error);
                }
            }
            JournalCmd::List { reply } => {
                let _ = reply.send(list_sessions(&conn));
            }
            JournalCmd::Replay {
                session_id,
                from_seq,
                reply,
            } => {
                let _ = reply.send(replay_session(&conn, &session_id, from_seq));
            }
            JournalCmd::Pin { session_id, reply } => {
                pins.insert(session_id);
                let _ = reply.send(Ok(()));
            }
            JournalCmd::Unpin { session_id } => {
                pins.remove(&session_id);
            }
            JournalCmd::Flush { reply } => {
                let result = conn
                    .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
                    .map(|_| ())
                    .map_err(JournalError::from);
                let _ = reply.send(result);
            }
            JournalCmd::FileLen { reply } => {
                let result = std::fs::metadata(&path)
                    .map(|meta| meta.len())
                    .map_err(|error| JournalError::Unavailable(error.to_string()));
                let _ = reply.send(result);
            }
            JournalCmd::Shutdown => break,
        }
    }
}

fn note_degraded(degraded_sessions: &Mutex<HashSet<String>>, session_id: &str) {
    if let Ok(mut sessions) = degraded_sessions.lock() {
        sessions.insert(session_id.to_string());
    }
}

fn on_write_error(error: &JournalError) {
    eprintln!("journal write failed: {error}");
}

fn upsert_session(conn: &Connection, record: &SessionRecord) -> Result<(), JournalError> {
    conn.execute(
        "INSERT INTO sessions (
            id, owner, workspace_id, kind, title, created_at_ms, updated_at_ms,
            generation, status, exit_code, closed, last_seq, degraded, payload_bytes,
            unsnapshotted_bytes, reaped
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, 0, ?15)
        ON CONFLICT(id) DO UPDATE SET
            title = excluded.title,
            updated_at_ms = excluded.updated_at_ms,
            generation = excluded.generation,
            status = excluded.status,
            exit_code = excluded.exit_code,
            closed = excluded.closed,
            last_seq = excluded.last_seq,
            degraded = MAX(sessions.degraded, excluded.degraded),
            reaped = MAX(sessions.reaped, excluded.reaped)",
        params![
            record.id,
            record.owner,
            record.workspace_id,
            kind_str(&record.kind),
            record.title,
            record.created_at_ms as i64,
            record.updated_at_ms as i64,
            record.generation as i64,
            record.status.as_str(),
            record.exit_code.map(|code| code as i64),
            if record.closed { 1 } else { 0 },
            record.last_seq as i64,
            if record.degraded { 1 } else { 0 },
            record.payload_bytes as i64,
            if record.reaped { 1 } else { 0 },
        ],
    )?;
    Ok(())
}

fn append_event(
    conn: &Connection,
    record: &EventRecord,
    pins: &HashSet<String>,
    limits: JournalLimits,
) -> Result<(), JournalError> {
    let checksum = crc32(&record.payload) as i64;
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO events (session_id, generation, seq, kind, ts_ms, payload, checksum)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            record.session_id,
            record.generation as i64,
            record.seq as i64,
            record.kind.as_str(),
            record.ts_ms as i64,
            record.payload,
            checksum,
        ],
    )?;
    let add = if record.kind == EventKind::Output {
        record.payload.len() as i64
    } else {
        0
    };
    tx.execute(
        "UPDATE sessions SET
            last_seq = MAX(last_seq, ?1),
            updated_at_ms = ?2,
            payload_bytes = payload_bytes + ?3,
            unsnapshotted_bytes = unsnapshotted_bytes + ?3
         WHERE id = ?4",
        params![
            record.seq as i64,
            record.ts_ms as i64,
            add,
            record.session_id
        ],
    )?;
    maybe_snapshot(&tx, &record.session_id, record.generation, limits)?;
    retain(&tx, pins, now_ms(), limits)?;
    tx.commit()?;
    Ok(())
}

fn maybe_snapshot(
    conn: &rusqlite::Transaction<'_>,
    session_id: &str,
    generation: u64,
    limits: JournalLimits,
) -> Result<(), JournalError> {
    let unsnapshotted: i64 = conn.query_row(
        "SELECT unsnapshotted_bytes FROM sessions WHERE id = ?1",
        [session_id],
        |row| row.get(0),
    )?;
    if unsnapshotted < limits.snapshot_every_bytes as i64 {
        return Ok(());
    }
    let last_up: i64 = conn
        .query_row(
            "SELECT COALESCE(MAX(up_to_seq), 0) FROM snapshots WHERE session_id = ?1 AND generation = ?2",
            params![session_id, generation as i64],
            |row| row.get(0),
        )?;
    let mut stmt = conn.prepare(
        "SELECT seq, payload FROM events
         WHERE session_id = ?1 AND generation = ?2 AND kind = 'output' AND seq > ?3
         ORDER BY seq",
    )?;
    let rows = stmt.query_map(params![session_id, generation as i64, last_up], |row| {
        Ok((row.get::<_, i64>(0)? as u64, row.get::<_, Vec<u8>>(1)?))
    })?;
    let mut chunks: Vec<(u64, Vec<u8>)> = Vec::new();
    let mut up_to = last_up as u64;
    let mut payload_bytes: u64 = 0;
    for row in rows {
        let (seq, payload) = row?;
        payload_bytes += payload.len() as u64;
        up_to = seq;
        chunks.push((seq, payload));
    }
    if chunks.is_empty() {
        return Ok(());
    }
    let blob = encode_chunks(&chunks);
    let checksum = crc32(&blob) as i64;
    let from_seq = chunks[0].0;
    conn.execute(
        "INSERT INTO snapshots (session_id, generation, from_seq, up_to_seq, ts_ms, blob, checksum, payload_bytes)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            session_id,
            generation as i64,
            from_seq as i64,
            up_to as i64,
            now_ms() as i64,
            blob,
            checksum,
            payload_bytes as i64,
        ],
    )?;
    conn.execute(
        "DELETE FROM events WHERE session_id = ?1 AND generation = ?2 AND kind = 'output' AND seq <= ?3",
        params![session_id, generation as i64, up_to as i64],
    )?;
    conn.execute(
        "UPDATE sessions SET unsnapshotted_bytes = 0 WHERE id = ?1",
        [session_id],
    )?;
    Ok(())
}

fn retain(
    conn: &rusqlite::Transaction<'_>,
    pins: &HashSet<String>,
    now_ms: u64,
    limits: JournalLimits,
) -> Result<(), JournalError> {
    let ids: Vec<(String, String, i64, i64, i64)> = {
        let mut stmt =
            conn.prepare("SELECT id, status, closed, updated_at_ms, payload_bytes FROM sessions")?;
        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, i64>(4)?,
            ))
        })?;
        rows.collect::<Result<Vec<_>, _>>()?
    };

    for (id, _status, _closed, _updated, payload) in &ids {
        if pins.contains(id) {
            continue;
        }
        let mut remaining = *payload;
        while remaining > limits.session_max_bytes as i64 {
            let oldest: Option<(i64, i64)> = conn
                .query_row(
                    "SELECT up_to_seq, payload_bytes FROM snapshots WHERE session_id = ?1 ORDER BY up_to_seq ASC LIMIT 1",
                    [id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            if let Some((up_to, snap_bytes)) = oldest {
                conn.execute(
                    "DELETE FROM snapshots WHERE session_id = ?1 AND up_to_seq = ?2",
                    params![id, up_to],
                )?;
                conn.execute(
                    "UPDATE sessions SET payload_bytes = MAX(payload_bytes - ?1, 0) WHERE id = ?2",
                    params![snap_bytes, id],
                )?;
                remaining -= snap_bytes;
            } else {
                let oldest_event: Option<(i64, i64)> = conn
                    .query_row(
                        "SELECT seq, LENGTH(payload) FROM events WHERE session_id = ?1 AND kind = 'output' ORDER BY seq ASC LIMIT 1",
                        [id],
                        |row| Ok((row.get(0)?, row.get(1)?)),
                    )
                    .optional()?;
                if let Some((seq, bytes)) = oldest_event {
                    conn.execute(
                        "DELETE FROM events WHERE session_id = ?1 AND seq = ?2 AND kind = 'output'",
                        params![id, seq],
                    )?;
                    conn.execute(
                        "UPDATE sessions SET payload_bytes = MAX(payload_bytes - ?1, 0) WHERE id = ?2",
                        params![bytes, id],
                    )?;
                    remaining -= bytes;
                } else {
                    break;
                }
            }
        }
    }

    let cutoff = now_ms.saturating_sub(limits.max_age_ms) as i64;
    let aged: Vec<String> = ids
        .iter()
        .filter(|(id, status, _closed, updated, _payload)| {
            !pins.contains(id) && *status != "live" && *updated < cutoff
        })
        .map(|(id, ..)| id.clone())
        .collect();
    for id in aged {
        delete_session(conn, &id)?;
    }

    loop {
        let (count, total) = session_totals(conn)?;
        if count <= limits.max_sessions && total <= limits.max_bytes {
            break;
        }
        match pick_trim_victim(conn, pins)? {
            Some(id) => delete_session(conn, &id)?,
            None => break,
        }
    }
    Ok(())
}

fn session_totals(conn: &rusqlite::Transaction<'_>) -> Result<(usize, u64), JournalError> {
    let count: i64 = conn.query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))?;
    let total: i64 = conn.query_row(
        "SELECT COALESCE(SUM(payload_bytes), 0) FROM sessions",
        [],
        |row| row.get(0),
    )?;
    Ok((count as usize, total as u64))
}

fn pick_trim_victim(
    conn: &rusqlite::Transaction<'_>,
    pins: &HashSet<String>,
) -> Result<Option<String>, JournalError> {
    let mut stmt = conn.prepare(
        "SELECT id FROM sessions
         WHERE status != 'live'
         ORDER BY closed DESC, updated_at_ms ASC",
    )?;
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let id: String = row.get(0)?;
        if !pins.contains(&id) {
            return Ok(Some(id));
        }
    }
    Ok(None)
}

fn delete_session(conn: &rusqlite::Transaction<'_>, id: &str) -> Result<(), JournalError> {
    conn.execute("DELETE FROM events WHERE session_id = ?1", [id])?;
    conn.execute("DELETE FROM snapshots WHERE session_id = ?1", [id])?;
    conn.execute("DELETE FROM turns WHERE session_id = ?1", [id])?;
    conn.execute("DELETE FROM permissions WHERE session_id = ?1", [id])?;
    conn.execute("DELETE FROM sessions WHERE id = ?1", [id])?;
    Ok(())
}

fn mark_reaped(
    conn: &Connection,
    session_id: &str,
    code: Option<u32>,
    degraded: bool,
) -> Result<(), JournalError> {
    let n = conn.execute(
        "UPDATE sessions SET reaped = 1, exit_code = COALESCE(?1, exit_code), degraded = MAX(degraded, ?2), updated_at_ms = ?3 WHERE id = ?4",
        params![
            code.map(|value| value as i64),
            if degraded { 1 } else { 0 },
            now_ms() as i64,
            session_id
        ],
    )?;
    if n == 0 {
        Err(JournalError::SessionNotFound)
    } else {
        Ok(())
    }
}

fn mark_ended(
    conn: &Connection,
    session_id: &str,
    generation: u64,
    code: Option<u32>,
    degraded: bool,
) -> Result<(), JournalError> {
    let ts = now_ms();
    let (last_seq, status): (i64, String) = conn
        .query_row(
            "SELECT last_seq, status FROM sessions WHERE id = ?1",
            [session_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .ok_or(JournalError::SessionNotFound)?;
    if status == "ended" {
        return Ok(());
    }
    let seq = (last_seq as u64).saturating_add(1);
    let payload = match code {
        Some(value) => value.to_le_bytes().to_vec(),
        None => Vec::new(),
    };
    let checksum = crc32(&payload) as i64;
    let tx = conn.unchecked_transaction()?;
    tx.execute(
        "INSERT INTO events (session_id, generation, seq, kind, ts_ms, payload, checksum)
         VALUES (?1, ?2, ?3, 'exit', ?4, ?5, ?6)",
        params![
            session_id,
            generation as i64,
            seq as i64,
            ts as i64,
            payload,
            checksum,
        ],
    )?;
    tx.execute(
        "UPDATE sessions SET status = 'ended', exit_code = ?1, last_seq = ?2, degraded = MAX(degraded, ?3), updated_at_ms = ?4 WHERE id = ?5",
        params![
            code.map(|value| value as i64),
            seq as i64,
            if degraded { 1 } else { 0 },
            ts as i64,
            session_id
        ],
    )?;
    tx.commit()?;
    let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    Ok(())
}

fn mark_closed(conn: &Connection, session_id: &str) -> Result<(), JournalError> {
    let n = conn.execute(
        "UPDATE sessions SET closed = 1, updated_at_ms = ?1 WHERE id = ?2",
        params![now_ms() as i64, session_id],
    )?;
    if n == 0 {
        Err(JournalError::SessionNotFound)
    } else {
        Ok(())
    }
}

fn mark_degraded(conn: &Connection, session_id: &str) -> Result<(), JournalError> {
    conn.execute(
        "UPDATE sessions SET degraded = 1, updated_at_ms = ?1 WHERE id = ?2",
        params![now_ms() as i64, session_id],
    )?;
    Ok(())
}

fn list_sessions(conn: &Connection) -> Result<Vec<SessionRecord>, JournalError> {
    let mut stmt = conn.prepare(
        "SELECT id, owner, workspace_id, kind, title, created_at_ms, updated_at_ms,
                generation, status, exit_code, closed, last_seq, degraded, payload_bytes, reaped
         FROM sessions WHERE closed = 0 ORDER BY id",
    )?;
    let rows = stmt.query_map([], row_to_session)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(JournalError::from)
}

fn row_to_session(row: &rusqlite::Row<'_>) -> rusqlite::Result<SessionRecord> {
    Ok(SessionRecord {
        id: row.get(0)?,
        owner: row.get(1)?,
        workspace_id: row.get(2)?,
        kind: parse_kind(&row.get::<_, String>(3)?),
        title: row.get(4)?,
        created_at_ms: row.get::<_, i64>(5)? as u64,
        updated_at_ms: row.get::<_, i64>(6)? as u64,
        generation: row.get::<_, i64>(7)? as u64,
        status: PersistStatus::parse(&row.get::<_, String>(8)?),
        exit_code: row.get::<_, Option<i64>>(9)?.map(|code| code as u32),
        closed: row.get::<_, i64>(10)? != 0,
        last_seq: row.get::<_, i64>(11)? as u64,
        degraded: row.get::<_, i64>(12)? != 0,
        payload_bytes: row.get::<_, i64>(13)? as u64,
        reaped: row.get::<_, i64>(14)? != 0,
    })
}

fn replay_session(
    conn: &Connection,
    session_id: &str,
    from_seq: u64,
) -> Result<Replay, JournalError> {
    let record = conn
        .query_row(
            "SELECT id, owner, workspace_id, kind, title, created_at_ms, updated_at_ms,
                    generation, status, exit_code, closed, last_seq, degraded, payload_bytes, reaped
             FROM sessions WHERE id = ?1",
            [session_id],
            row_to_session,
        )
        .optional()?
        .ok_or(JournalError::SessionNotFound)?;
    if record.closed {
        return Err(JournalError::SessionNotFound);
    }
    let generation = record.generation;
    let mut events: Vec<SessionEvent> = Vec::new();
    let mut covered = from_seq;

    let mut snap_stmt = conn.prepare(
        "SELECT from_seq, up_to_seq, blob, checksum FROM snapshots
         WHERE session_id = ?1 AND generation = ?2 AND up_to_seq > ?3
         ORDER BY up_to_seq",
    )?;
    let snaps = snap_stmt.query_map(
        params![session_id, generation as i64, from_seq as i64],
        |row| {
            Ok((
                row.get::<_, i64>(0)? as u64,
                row.get::<_, i64>(1)? as u64,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, i64>(3)? as u32,
            ))
        },
    )?;
    for snap in snaps {
        let (_from, up_to, blob, checksum) = snap?;
        if crc32(&blob) != checksum {
            return Err(JournalError::Checksum {
                session_id: session_id.to_string(),
                seq: up_to,
            });
        }
        let chunks = decode_chunks(&blob).ok_or_else(|| {
            JournalError::Corrupt(format!("snapshot blob for {session_id} up_to {up_to}"))
        })?;
        for (seq, data) in chunks {
            if seq > from_seq {
                events.push(SessionEvent::Output {
                    seq,
                    data: String::from_utf8_lossy(&data).into_owned(),
                });
            }
        }
        covered = covered.max(up_to);
    }

    let mut event_stmt = conn.prepare(
        "SELECT seq, kind, payload, checksum FROM events
         WHERE session_id = ?1 AND generation = ?2 AND seq > ?3
         ORDER BY seq",
    )?;
    let event_rows = event_stmt.query_map(
        params![session_id, generation as i64, covered as i64],
        |row| {
            Ok((
                row.get::<_, i64>(0)? as u64,
                row.get::<_, String>(1)?,
                row.get::<_, Vec<u8>>(2)?,
                row.get::<_, i64>(3)? as u32,
            ))
        },
    )?;
    let mut exit_event: Option<SessionEvent> = None;
    for row in event_rows {
        let (seq, kind, payload, checksum) = row?;
        if crc32(&payload) != checksum {
            return Err(JournalError::Checksum {
                session_id: session_id.to_string(),
                seq,
            });
        }
        match EventKind::parse(&kind) {
            Some(EventKind::Output) => events.push(SessionEvent::Output {
                seq,
                data: String::from_utf8_lossy(&payload).into_owned(),
            }),
            Some(EventKind::Exit) => {
                let code = if payload.len() == 4 {
                    Some(u32::from_le_bytes(
                        payload.as_slice().try_into().unwrap_or([0; 4]),
                    ))
                } else {
                    None
                };
                exit_event = Some(SessionEvent::Exit { code });
            }
            None => {}
        }
    }

    match record.status {
        PersistStatus::Ended => {
            events.push(exit_event.unwrap_or(SessionEvent::Exit {
                code: record.exit_code,
            }));
        }
        PersistStatus::Live if record.reaped => {
            events.push(exit_event.unwrap_or(SessionEvent::Exit {
                code: record.exit_code,
            }));
        }
        PersistStatus::Interrupted | PersistStatus::Live => {
            events.push(SessionEvent::Recovered {
                truncated: record.degraded,
            });
        }
    }

    Ok(Replay {
        generation,
        last_seq: record.last_seq,
        truncated: record.degraded,
        events,
    })
}

fn kind_str(kind: &SessionKind) -> &'static str {
    match kind {
        SessionKind::Terminal => "terminal",
        SessionKind::Acp => "acp",
    }
}

fn parse_kind(value: &str) -> SessionKind {
    match value {
        "acp" => SessionKind::Acp,
        _ => SessionKind::Terminal,
    }
}

fn encode_chunks(chunks: &[(u64, Vec<u8>)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&(chunks.len() as u32).to_le_bytes());
    for (seq, data) in chunks {
        out.extend_from_slice(&seq.to_le_bytes());
        out.extend_from_slice(&(data.len() as u32).to_le_bytes());
        out.extend_from_slice(data);
    }
    out
}

fn decode_chunks(blob: &[u8]) -> Option<Vec<(u64, Vec<u8>)>> {
    if blob.len() < 4 {
        return None;
    }
    let count = u32::from_le_bytes(blob[0..4].try_into().ok()?) as usize;
    let mut offset = 4;
    let mut chunks = Vec::with_capacity(count);
    for _ in 0..count {
        if offset + 12 > blob.len() {
            return None;
        }
        let seq = u64::from_le_bytes(blob[offset..offset + 8].try_into().ok()?);
        offset += 8;
        let len = u32::from_le_bytes(blob[offset..offset + 4].try_into().ok()?) as usize;
        offset += 4;
        if offset + len > blob.len() {
            return None;
        }
        chunks.push((seq, blob[offset..offset + len].to_vec()));
        offset += len;
    }
    Some(chunks)
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(0))
        .unwrap_or(0)
}

pub fn new_session_record(
    id: impl Into<String>,
    owner: impl Into<String>,
    workspace_id: Option<String>,
    kind: SessionKind,
    title: impl Into<String>,
) -> SessionRecord {
    let now = now_ms();
    SessionRecord {
        id: id.into(),
        owner: owner.into(),
        workspace_id,
        kind,
        title: title.into(),
        created_at_ms: now,
        updated_at_ms: now,
        generation: 1,
        status: PersistStatus::Live,
        exit_code: None,
        closed: false,
        last_seq: 0,
        degraded: false,
        payload_bytes: 0,
        reaped: false,
    }
}

pub fn output_record(
    session_id: impl Into<String>,
    generation: u64,
    seq: u64,
    data: impl AsRef<[u8]>,
) -> EventRecord {
    EventRecord {
        session_id: session_id.into(),
        generation,
        seq,
        kind: EventKind::Output,
        ts_ms: now_ms(),
        payload: data.as_ref().to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;
    use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

    fn tmp_journal() -> (PathBuf, PathBuf) {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        let dir = std::env::temp_dir().join(format!(
            "devboule journal {}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, AtomicOrdering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("journal.db");
        (dir, path)
    }

    fn snapshot_limits() -> JournalLimits {
        JournalLimits {
            snapshot_every_bytes: 32,
            session_max_bytes: JOURNAL_SESSION_MAX_BYTES,
            max_bytes: JOURNAL_MAX_BYTES,
            max_sessions: JOURNAL_MAX_SESSIONS,
            max_age_ms: JOURNAL_MAX_AGE_MS,
        }
    }

    fn tiny_limits() -> JournalLimits {
        JournalLimits {
            snapshot_every_bytes: JOURNAL_SESSION_MAX_BYTES,
            session_max_bytes: JOURNAL_SESSION_MAX_BYTES,
            max_bytes: JOURNAL_MAX_BYTES,
            max_sessions: 2,
            max_age_ms: JOURNAL_MAX_AGE_MS,
        }
    }

    fn sample_session(id: &str) -> SessionRecord {
        new_session_record(id, "S-1-5-21-1", None, SessionKind::Terminal, "Terminal")
    }

    #[test]
    fn open_creates_schema_and_agent_tables() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open(&path).expect("open");
        journal.flush().expect("flush");
        drop(journal);
        let conn = Connection::open(&path).expect("reopen");
        let version: i32 = conn
            .pragma_query_value(None, "user_version", |row| row.get(0))
            .expect("version");
        assert_eq!(version, JOURNAL_SCHEMA_VERSION);
        let turns: i64 = conn
            .query_row("SELECT COUNT(*) FROM turns", [], |row| row.get(0))
            .expect("turns");
        let perms: i64 = conn
            .query_row("SELECT COUNT(*) FROM permissions", [], |row| row.get(0))
            .expect("permissions");
        assert_eq!(turns, 0);
        assert_eq!(perms, 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn append_and_replay_preserves_seq() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open(&path).expect("open");
        journal
            .upsert_blocking(sample_session("s.a.1"))
            .expect("upsert");
        journal
            .append_blocking(output_record("s.a.1", 1, 1, b"one"))
            .expect("a");
        journal
            .append_blocking(output_record("s.a.1", 1, 2, b"two"))
            .expect("b");
        let replay = journal.replay("s.a.1", 0).expect("replay");
        match &replay.events[..] {
            [SessionEvent::Output { seq: 1, data: a }, SessionEvent::Output { seq: 2, data: b }, SessionEvent::Recovered { truncated: false }] =>
            {
                assert_eq!(a, "one");
                assert_eq!(b, "two");
            }
            other => panic!("unexpected replay: {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn snapshot_compacts_and_replay_has_no_gap_or_duplicate() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open_with_limits(&path, snapshot_limits()).expect("open");
        journal
            .upsert_blocking(sample_session("s.a.1"))
            .expect("upsert");
        for seq in 1..=8 {
            journal
                .append_blocking(output_record(
                    "s.a.1",
                    1,
                    seq,
                    format!("chunk-{seq:02}....").as_bytes(),
                ))
                .expect("append");
        }
        let replay = journal.replay("s.a.1", 0).expect("replay");
        let seqs: Vec<u64> = replay
            .events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::Output { seq, .. } => Some(*seq),
                _ => None,
            })
            .collect();
        assert_eq!(seqs, vec![1, 2, 3, 4, 5, 6, 7, 8]);
        let conn = Connection::open(&path).expect("read");
        let snaps: i64 = conn
            .query_row("SELECT COUNT(*) FROM snapshots", [], |row| row.get(0))
            .expect("snaps");
        assert!(snaps >= 1, "expected at least one snapshot, got {snaps}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn retention_drops_oldest_unpinned_and_skips_pinned() {
        let (dir, path) = tmp_journal();
        {
            let journal = Journal::open_with_limits(&path, tiny_limits()).expect("open");
            for (id, body) in [
                ("s.a.1", "alpha-payload-xxxx"),
                ("s.a.2", "bravo-payload-xxxx"),
            ] {
                journal.upsert_blocking(sample_session(id)).expect("upsert");
                journal
                    .append_blocking(output_record(id, 1, 1, body.as_bytes()))
                    .expect("append");
            }
        }
        // Reopen so leftover live rows become interrupted (the daemon-kill path).
        let journal = Journal::open_with_limits(&path, tiny_limits()).expect("reopen");
        journal.pin("s.a.2").expect("pin");
        journal
            .upsert_blocking(sample_session("s.a.3"))
            .expect("third");
        journal
            .append_blocking(output_record("s.a.3", 1, 1, b"charlie-payload-xxxx"))
            .expect("third body");
        let listed: Vec<String> = journal
            .list()
            .expect("list")
            .into_iter()
            .map(|row| row.id)
            .collect();
        assert!(
            listed.contains(&"s.a.2".to_string()),
            "pinned session was trimmed: {listed:?}"
        );
        assert!(
            !listed.contains(&"s.a.1".to_string()),
            "retention should have dropped the oldest unpinned session: {listed:?}"
        );
        let replay = journal.replay("s.a.2", 0).expect("pinned replay");
        assert!(replay.events.iter().any(|event| matches!(
            event,
            SessionEvent::Output { data, .. } if data.contains("bravo")
        )));
        journal.unpin("s.a.2");
        journal.flush().expect("flush");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn future_schema_is_a_clear_error() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open(&path).expect("open");
        drop(journal);
        let conn = Connection::open(&path).expect("bump");
        conn.pragma_update(None, "user_version", 99)
            .expect("version");
        drop(conn);
        match Journal::open(&path) {
            Err(JournalError::FutureSchema {
                found: 99,
                supported: JOURNAL_SCHEMA_VERSION,
            }) => {}
            Err(other) => panic!("expected future schema, got {other}"),
            Ok(_) => panic!("future schema opened"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_file_is_a_clear_error() {
        let (dir, path) = tmp_journal();
        std::fs::write(&path, b"this is not sqlite").expect("garbage");
        let error = match Journal::open(&path) {
            Err(error) => error,
            Ok(_) => panic!("corrupt journal opened"),
        };
        assert!(
            matches!(
                error,
                JournalError::Corrupt(_) | JournalError::Unavailable(_)
            ),
            "expected corrupt/unavailable, got {error}"
        );
        let message = error.to_string();
        assert!(
            message.contains("journal"),
            "error should name the journal: {message}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_file_creates_new() {
        let (dir, path) = tmp_journal();
        assert!(!path.exists());
        let journal = Journal::open(&path).expect("create");
        assert!(path.exists());
        drop(journal);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn degradation_is_scoped_to_a_session_and_journal_lifetime() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open(&path).expect("open");
        journal
            .upsert_blocking(sample_session("s.a.1"))
            .expect("upsert");
        assert!(!journal.is_session_degraded("s.a.1"));
        assert!(journal.note_session_degraded("s.a.1"));
        assert!(journal.is_session_degraded("s.a.1"));
        assert!(!journal.is_session_degraded("s.a.2"));
        assert!(!journal.note_session_degraded("s.a.1"));
        journal.flush().expect("degradation marker");
        assert!(journal
            .list()
            .expect("list")
            .into_iter()
            .find(|row| row.id == "s.a.1")
            .is_some_and(|row| row.degraded));
        drop(journal);

        let journal = Journal::open(&path).expect("reopen");
        assert!(!journal.is_session_degraded("s.a.1"));
        assert!(journal
            .list()
            .expect("reopen list")
            .into_iter()
            .find(|row| row.id == "s.a.1")
            .is_some_and(|row| row.degraded));
        drop(journal);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn uncommitted_write_is_not_visible_after_reopen() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open(&path).expect("open");
        journal
            .upsert_blocking(sample_session("s.a.1"))
            .expect("upsert");
        drop(journal);
        let conn = Connection::open(&path).expect("raw");
        conn.execute("BEGIN", []).expect("begin");
        conn.execute(
            "INSERT INTO events (session_id, generation, seq, kind, ts_ms, payload, checksum)
             VALUES ('s.a.1', 1, 99, 'output', 1, X'00', 0)",
            [],
        )
        .expect("insert");
        drop(conn);
        let journal = Journal::open(&path).expect("reopen");
        let replay = journal.replay("s.a.1", 0).expect("replay");
        assert!(replay.events.iter().all(|event| match event {
            SessionEvent::Output { seq, .. } => *seq != 99,
            _ => true,
        }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn drain_output_after_process_exit_is_not_dropped() {
        // ConPTY keeps delivering after Child::wait (ARCHITETTURA §1.7).
        // Marking the journal ended at wait-time steals last_seq+1 for the
        // exit row; the drain frame then collides and vanishes. This is the
        // silent tail loss: live ring has the bytes, replay does not.
        let (dir, path) = tmp_journal();
        let journal = Journal::open(&path).expect("open");
        journal
            .upsert_blocking(sample_session("s.drain.1"))
            .expect("upsert");
        journal
            .append_blocking(output_record("s.drain.1", 1, 1, b"HEAD"))
            .expect("head");
        // Waiter path: process reaped, output still live.
        journal.try_mark_reaped("s.drain.1", Some(0));
        journal.flush().expect("flush reaped");
        let drain = vec![b'X'; 3953];
        journal
            .append_blocking(output_record("s.drain.1", 1, 2, &drain))
            .expect("drain append must succeed; an exit row must not occupy seq 2");
        // EOF path: now freeze last_seq with the exit row.
        journal.try_mark_ended("s.drain.1", 1, Some(0));
        journal.flush().expect("flush ended");
        let replay = journal.replay("s.drain.1", 0).expect("replay");
        let output: String = replay
            .events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::Output { data, .. } => Some(data.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            output.len(),
            4 + 3953,
            "journal silently lost the drain tail: {output:?}"
        );
        assert!(output.starts_with("HEAD"), "{output:?}");
        assert!(output.ends_with(&"X".repeat(3953)), "drain tail missing");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ended_and_interrupted_replay_are_distinct() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open(&path).expect("open");
        journal
            .upsert_blocking(sample_session("s.ended"))
            .expect("upsert");
        journal
            .append_blocking(output_record("s.ended", 1, 1, b"bye"))
            .expect("out");
        journal
            .send_cmd(
                JournalCmd::MarkEnded {
                    session_id: "s.ended".to_string(),
                    generation: 1,
                    code: Some(0),
                    degraded: false,
                },
                RPC_WAIT,
            )
            .expect("mark");
        journal.flush().expect("flush");
        journal
            .upsert_blocking(sample_session("s.kill"))
            .expect("kill");
        journal
            .append_blocking(output_record("s.kill", 1, 1, b"still"))
            .expect("out");
        drop(journal);

        let journal = Journal::open(&path).expect("reopen");
        let ended = journal.replay("s.ended", 0).expect("ended");
        let killed = journal.replay("s.kill", 0).expect("killed");
        assert!(matches!(
            ended.events.last(),
            Some(SessionEvent::Exit { code: Some(0) })
        ));
        assert!(matches!(
            killed.events.last(),
            Some(SessionEvent::Recovered { truncated: false })
        ));
        let listed = journal.list().expect("list");
        assert!(listed.iter().any(|row| {
            row.id == "s.ended" && matches!(row.to_session().state, SessionState::Ended { .. })
        }));
        assert!(listed.iter().any(|row| {
            row.id == "s.kill" && matches!(row.to_session().state, SessionState::Recovered { .. })
        }));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn hammer_writer_loop() {
        let Ok(path) = std::env::var("DEVBOULE_JOURNAL_HAMMER_PATH") else {
            return;
        };
        let journal = Journal::open(Path::new(&path)).expect("hammer open");
        journal
            .upsert_blocking(sample_session("s.hammer.1"))
            .expect("upsert");
        let ready = Path::new(&path).with_file_name("hammer.ready");
        std::fs::write(&ready, b"ok").expect("ready");
        let payload = vec![b'x'; 4096];
        let mut seq = 1u64;
        loop {
            let _ = journal.tx.send(JournalCmd::Append(output_record(
                "s.hammer.1",
                1,
                seq,
                &payload,
            )));
            seq = seq.saturating_add(1);
        }
    }

    #[test]
    fn kill_writer_mid_append_journal_is_readable() {
        if std::env::var_os("DEVBOULE_JOURNAL_HAMMER_PATH").is_some() {
            return;
        }
        let (dir, path) = tmp_journal();
        {
            let journal = Journal::open(&path).expect("seed");
            drop(journal);
        }
        let exe = std::env::current_exe().expect("exe");
        let mut child = Command::new(&exe)
            .env("DEVBOULE_JOURNAL_HAMMER_PATH", &path)
            .args([
                "--exact",
                "--test-threads=1",
                "journal::tests::hammer_writer_loop",
            ])
            .spawn()
            .expect("spawn hammer");
        let ready = path.with_file_name("hammer.ready");
        let deadline = Instant::now() + Duration::from_secs(5);
        while !ready.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        if !ready.exists() {
            let _ = child.kill();
            let status = child.wait();
            panic!("hammer child did not start writing; wait={status:?}");
        }
        std::thread::sleep(Duration::from_millis(300));
        let _ = child.kill();
        let _ = child.wait();
        let journal = Journal::open(&path).expect("reopen after kill");
        let replay = journal.replay("s.hammer.1", 0);
        assert!(replay.is_ok(), "journal unreadable after kill: {replay:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
