//! Conversation journal: SQLite WAL, one writer thread.
//!
//! The PTY reader and coalesce threads never wait here. They `try_send` a
//! record into a bounded channel. If the channel is full or the disk is
//! full, the session is marked degraded and the live terminal continues.
//! A recovered session then replays everything that had COMMITTED before
//! the previous process died. That is a prefix of what the process
//! produced, and the replay cannot tell how long the prefix is: whatever
//! was still uncommitted in the queue died with the process and left no
//! record. The degraded flag covers only losses that were observed while
//! the daemon was alive; the Recovered marker itself is what says the
//! tail is unverifiable. Nothing here claims completeness except an
//! orderly close.
//!
//! Schema notes for M6: `events.kind` is an open string (`output`, `exit`,
//! later `turn` / `permission`). Additive columns on `sessions` and the
//! empty `turns` / `permissions` tables mean agent history does not require
//! a migration that rewrites terminal rows.

use std::collections::{hash_map::Entry, HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rusqlite::{params, Connection, OptionalExtension};

use devboule_protocol::{
    ErrorCode, JournalRetention, RetentionPatch, Session, SessionEvent, SessionKind, SessionState,
    TranscriptIntegrity, WireError,
};

#[path = "journal_replay.rs"]
mod journal_replay;
#[path = "journal_retention.rs"]
mod journal_retention;
#[path = "journal_schema.rs"]
mod journal_schema;

use journal_replay::{list_sessions, replay_session};
use journal_retention::{
    delete_session_user, effective_limits, journal_retention, journal_usage, retain,
    set_journal_retention, RetentionState,
};
use journal_schema::open_connection;

/// Stored in `PRAGMA user_version`. Bump whenever the journal schema gains
/// tables or columns that need migration.
pub const JOURNAL_SCHEMA_VERSION: i32 = 3;

/// Bounded journal queue. Each slot is one coalesced frame (typically
/// ≤ 8 KiB). A full queue never blocks the PTY path.
pub const JOURNAL_QUEUE_CAP: usize = 1024;

/// Take a snapshot after this many payload bytes since the last one.
pub const SNAPSHOT_EVERY_BYTES: u64 = 64 * 1024;

/// Per-session cap on snapshot + event payload. Oldest windows go first.
/// The user loses the start of that session's scrollback, never a hole in
/// the middle of a replay already loaded into memory. 512 MiB is a safety
/// net for a runaway dump, not a history policy.
pub const JOURNAL_SESSION_MAX_BYTES: u64 = 512 * 1024 * 1024;

/// Drop the oldest unpinned non-live sessions when the logical payload
/// exceeds this.
pub const JOURNAL_MAX_BYTES: u64 = 8 * 1024 * 1024 * 1024;

/// Maximum retained sessions, closed ones included. Oldest unpinned
/// non-live go first.
pub const JOURNAL_MAX_SESSIONS: usize = 10_000;

/// Age cap. The user loses recovered transcripts older than this.
pub const JOURNAL_MAX_AGE_MS: u64 = 0;

const RPC_WAIT: Duration = Duration::from_secs(10);
const JOIN_BUDGET: Duration = Duration::from_millis(500);
/// Keep room for the degradation, reaped, and ended control records even
/// while output is arriving faster than SQLite can commit it.
const CONTROL_RESERVE: usize = 3;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
    LiveSession,
    InvalidRequest(String),
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
            Self::LiveSession => write!(formatter, "Close the session before deleting it."),
            Self::InvalidRequest(message) => write!(formatter, "{message}"),
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
            JournalError::LiveSession => ErrorCode::InvalidRequest,
            JournalError::InvalidRequest(_) => ErrorCode::InvalidRequest,
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
    pub dropped_frames: u64,
    pub dropped_bytes: u64,
    pub payload_bytes: u64,
    pub trimmed_bytes: u64,
    /// Child::wait returned. Output may still be arriving (ConPTY drain).
    pub reaped: bool,
}

impl SessionRecord {
    fn integrity(&self, terminated: bool) -> TranscriptIntegrity {
        if terminated {
            if self.degraded {
                TranscriptIntegrity::Truncated {
                    dropped_frames: self.dropped_frames,
                    dropped_bytes: self.dropped_bytes,
                    trimmed_bytes: self.trimmed_bytes,
                }
            } else if self.trimmed_bytes > 0 {
                TranscriptIntegrity::Truncated {
                    dropped_frames: 0,
                    dropped_bytes: 0,
                    trimmed_bytes: self.trimmed_bytes,
                }
            } else {
                TranscriptIntegrity::Complete
            }
        } else {
            TranscriptIntegrity::Unverifiable {
                dropped_frames: if self.degraded {
                    self.dropped_frames
                } else {
                    0
                },
                dropped_bytes: if self.degraded { self.dropped_bytes } else { 0 },
                trimmed_bytes: self.trimmed_bytes,
            }
        }
    }

    pub fn to_session(&self) -> Session {
        let state = match self.status {
            PersistStatus::Live if self.reaped => SessionState::Ended {
                generation: self.generation,
                code: self.exit_code,
                integrity: self.integrity(true),
            },
            PersistStatus::Live => SessionState::Recovered {
                generation: self.generation,
                integrity: self.integrity(false),
            },
            PersistStatus::Ended => SessionState::Ended {
                generation: self.generation,
                code: self.exit_code,
                integrity: self.integrity(true),
            },
            PersistStatus::Interrupted => SessionState::Recovered {
                generation: self.generation,
                integrity: self.integrity(false),
            },
        };
        Session {
            id: self.id.clone(),
            workspace_id: self.workspace_id.clone(),
            kind: self.kind.clone(),
            title: self.title.clone(),
            state,
            elapsed_ms: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    Output,
    Exit,
    AgentReport,
    AcpEnvelope,
}

impl EventKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Output => "output",
            Self::Exit => "exit",
            Self::AgentReport => "agent_report",
            Self::AcpEnvelope => "acp_envelope",
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "output" => Some(Self::Output),
            "exit" => Some(Self::Exit),
            "agent_report" => Some(Self::AgentReport),
            "acp_envelope" => Some(Self::AcpEnvelope),
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

#[derive(Clone, Debug)]
struct PermissionRecord {
    session_id: String,
    request_id: String,
    ts_ms: u64,
    outcome: String,
    payload: Vec<u8>,
}

#[derive(Debug)]
pub struct Replay {
    pub generation: u64,
    pub events: Vec<SessionEvent>,
    /// Journal stream sequence for each `events` entry. ACP views have no
    /// seq on the event itself; this is the same space as Output.
    pub event_seqs: Vec<u64>,
    pub last_seq: u64,
    pub integrity: TranscriptIntegrity,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct DropCounters {
    frames: u64,
    bytes: u64,
}

/// Live counters of the journal writer, process-wide.
///
/// The pair `(failed_frames, committed_frames < accepted_frames)` is what
/// makes two otherwise-identical-looking losses distinguishable while the
/// daemon is alive: output dropped knowing it (counted in `failed_frames`,
/// also recorded as per-session degradation) versus output sitting in the
/// bounded queue uncommitted, which dies with the process without any
/// record. After a death only the second kind is invisible to the
/// database — that is why a recovered transcript's tail is unverifiable.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct JournalStatsSnapshot {
    pub accepted_frames: u64,
    pub accepted_bytes: u64,
    pub committed_frames: u64,
    pub committed_bytes: u64,
    pub failed_frames: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalSessionUsage {
    pub id: String,
    pub title: String,
    pub kind: SessionKind,
    pub bytes: u64,
    pub updated_at_ms: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Unreclaimable {
    /// Bytes over `max_bytes` that retention is not allowed to reclaim.
    pub bytes_over: u64,
    /// Sessions over `max_sessions` that retention is not allowed to reclaim.
    pub sessions_over: usize,
    /// Sessions past `max_age_ms` that retention is not allowed to delete.
    pub aged_out: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct JournalUsage {
    pub total_bytes: u64,
    pub session_count: usize,
    pub deleted_by_user: usize,
    pub deleted_by_retention: usize,
    pub unreclaimable: Unreclaimable,
    pub limits: JournalLimits,
    pub per_session: Vec<JournalSessionUsage>,
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
    Permission {
        record: PermissionRecord,
        reply: mpsc::Sender<Result<(), JournalError>>,
    },
    MarkReaped {
        session_id: String,
        code: Option<u32>,
    },
    MarkEnded {
        session_id: String,
        generation: u64,
        code: Option<u32>,
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
    DeleteSession {
        session_id: String,
        reply: mpsc::Sender<Result<(), JournalError>>,
    },
    Usage {
        reply: mpsc::Sender<Result<JournalUsage, JournalError>>,
    },
    RetentionGet {
        reply: mpsc::Sender<Result<JournalRetention, JournalError>>,
    },
    RetentionSet {
        patch: RetentionPatch,
        reply: mpsc::Sender<Result<JournalRetention, JournalError>>,
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
    degraded_sessions: Arc<Mutex<HashMap<String, DropCounters>>>,
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
        let degraded_sessions = Arc::new(Mutex::new(HashMap::new()));
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
            .map(|sessions| sessions.contains_key(session_id))
            .unwrap_or(true)
    }

    pub(crate) fn session_drop_counters(&self, session_id: &str) -> (u64, u64) {
        self.degraded_sessions
            .lock()
            .ok()
            .and_then(|sessions| sessions.get(session_id).copied())
            .map(|counters| (counters.frames, counters.bytes))
            .unwrap_or_default()
    }

    /// Point-in-time read of the writer's counters. Cheap: atomic loads,
    /// no queue lock, safe from any thread. This is the only honesty
    /// instrument that outlives neither the process nor the queue — read
    /// it while the writer is alive, because after a kill there is
    /// nothing left to consult.
    pub fn stats(&self) -> JournalStatsSnapshot {
        self.stats.snapshot()
    }

    /// Never blocks. On a full queue or a dead writer the session is marked
    /// degraded and the PTY path continues.
    pub fn try_upsert(&self, record: SessionRecord) {
        self.try_send(JournalCmd::Upsert(record));
    }

    /// Returns false if the queue was full or the writer is dead. The PTY
    /// path never waits; a false return marks the journal as degraded.
    pub fn try_append(&self, record: EventRecord) -> bool {
        let payload_len = record.payload.len() as u64;
        let session_id = record.session_id.clone();
        if !self.reserve_output_slot() {
            self.note_dropped_frame(&record.session_id, payload_len);
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
                self.note_dropped_frame(&session_id, payload_len);
                self.stats.failed_frames.fetch_add(1, Ordering::Relaxed);
                false
            }
        }
    }

    /// Record a permission decision and wait until SQLite has accepted it.
    /// Permission rows are control traffic, and a caller must not send an ACP
    /// grant before this returns: otherwise a crash can leave an invisible
    /// authorization in the audit log.
    pub fn record_permission(
        &self,
        session_id: &str,
        request_id: &str,
        outcome: &str,
        payload: &[u8],
    ) -> Result<(), JournalError> {
        let record = PermissionRecord {
            session_id: session_id.to_string(),
            request_id: request_id.to_string(),
            ts_ms: now_ms(),
            outcome: outcome.to_string(),
            payload: payload.to_vec(),
        };
        self.rpc(|reply| JournalCmd::Permission { record, reply })
    }

    /// Child::wait returned. Does not freeze last_seq and does not write an
    /// exit row: ConPTY may still deliver drain frames that need seqs.
    pub fn try_mark_reaped(&self, session_id: &str, code: Option<u32>) {
        self.try_send(JournalCmd::MarkReaped {
            session_id: session_id.to_string(),
            code,
        });
    }

    pub fn mark_reaped(&self, session_id: &str, code: Option<u32>) -> Result<(), JournalError> {
        self.send_cmd(
            JournalCmd::MarkReaped {
                session_id: session_id.to_string(),
                code,
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
        // End markers are the durable product boundary. Unlike output, they
        // must wait for a full queue instead of timing out and making History
        // report a truncated transcript as complete.
        self.send_cmd_until_stopped(JournalCmd::MarkEnded {
            session_id: session_id.to_string(),
            generation,
            code,
        })?;
        self.rpc_until_stopped(|reply| JournalCmd::Flush { reply })
    }

    pub fn try_mark_ended(&self, session_id: &str, generation: u64, code: Option<u32>) {
        self.try_send(JournalCmd::MarkEnded {
            session_id: session_id.to_string(),
            generation,
            code,
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

    pub fn delete_session(&self, session_id: &str) -> Result<(), JournalError> {
        self.rpc(|reply| JournalCmd::DeleteSession {
            session_id: session_id.to_string(),
            reply,
        })
    }

    pub fn usage(&self) -> Result<JournalUsage, JournalError> {
        self.rpc(|reply| JournalCmd::Usage { reply })
    }

    pub fn retention_get(&self) -> Result<JournalRetention, JournalError> {
        self.rpc(|reply| JournalCmd::RetentionGet { reply })
    }

    pub fn retention_set(&self, patch: RetentionPatch) -> Result<JournalRetention, JournalError> {
        self.rpc(|reply| JournalCmd::RetentionSet { patch, reply })
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

    fn send_cmd_until_stopped(&self, cmd: JournalCmd) -> Result<(), JournalError> {
        let mut pending = Some(cmd);
        loop {
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
        self.note_degraded(session_id, DropCounters::default())
    }

    fn note_dropped_frame(&self, session_id: &str, bytes: u64) -> bool {
        self.note_degraded(session_id, DropCounters { frames: 1, bytes })
    }

    fn note_degraded(&self, session_id: &str, dropped: DropCounters) -> bool {
        let first = match self.degraded_sessions.lock() {
            Ok(mut sessions) => match sessions.entry(session_id.to_string()) {
                Entry::Vacant(entry) => {
                    entry.insert(dropped);
                    true
                }
                Entry::Occupied(mut entry) => {
                    let counters = entry.get_mut();
                    counters.frames = counters.frames.saturating_add(dropped.frames);
                    counters.bytes = counters.bytes.saturating_add(dropped.bytes);
                    false
                }
            },
            Err(_) => {
                eprintln!(
                    "journal degradation set is poisoned; treating session {session_id} as degraded"
                );
                true
            }
        };
        if first {
            self.queue_degraded_marker(session_id);
        }
        first
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

    fn rpc_until_stopped<T>(
        &self,
        make: impl FnOnce(mpsc::Sender<Result<T, JournalError>>) -> JournalCmd,
    ) -> Result<T, JournalError> {
        let (tx, rx) = mpsc::channel();
        self.send_cmd_until_stopped(make(tx))?;
        rx.recv().map_err(|_| JournalError::Stopped)?
    }
}

impl Drop for Journal {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn journal_loop(
    conn: Connection,
    rx: mpsc::Receiver<JournalCmd>,
    queued: Arc<AtomicU64>,
    degraded_sessions: Arc<Mutex<HashMap<String, DropCounters>>>,
    stats: Arc<JournalStats>,
    limits: JournalLimits,
    path: PathBuf,
) {
    let mut pins: HashSet<String> = HashSet::new();
    let mut retention_state = RetentionState::default();
    while let Ok(cmd) = rx.recv() {
        queued
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                Some(value.saturating_sub(1))
            })
            .ok();
        match cmd {
            JournalCmd::Upsert(record) => {
                if let Err(error) = upsert_session(&conn, &record) {
                    note_degraded(&degraded_sessions, &record.id, DropCounters::default());
                    on_write_error(&error);
                } else {
                    retention_state.session_set_changed();
                }
            }
            JournalCmd::Append(record) => {
                let is_output = matches!(record.kind, EventKind::Output | EventKind::AcpEnvelope);
                let payload_len = record.payload.len() as u64;
                if let Err(error) =
                    append_event(&conn, &record, &pins, limits, &mut retention_state)
                {
                    if is_output {
                        stats.failed_frames.fetch_add(1, Ordering::Relaxed);
                        note_degraded(
                            &degraded_sessions,
                            &record.session_id,
                            DropCounters {
                                frames: 1,
                                bytes: payload_len,
                            },
                        );
                    } else {
                        note_degraded(
                            &degraded_sessions,
                            &record.session_id,
                            DropCounters::default(),
                        );
                    }
                    on_write_error(&error);
                    let (degraded, dropped) =
                        degradation_state(&degraded_sessions, &record.session_id);
                    let _ = mark_degraded(&conn, &record.session_id, degraded, dropped);
                } else if is_output {
                    stats.committed_frames.fetch_add(1, Ordering::Relaxed);
                    stats
                        .committed_bytes
                        .fetch_add(payload_len, Ordering::Relaxed);
                }
            }
            JournalCmd::Permission { record, reply } => {
                let result = append_permission(&conn, &record);
                if let Err(error) = &result {
                    note_degraded(
                        &degraded_sessions,
                        &record.session_id,
                        DropCounters::default(),
                    );
                    on_write_error(error);
                }
                let _ = reply.send(result);
            }
            JournalCmd::MarkReaped { session_id, code } => {
                let (degraded, dropped) = degradation_state(&degraded_sessions, &session_id);
                if let Err(error) = mark_reaped(&conn, &session_id, code, degraded, dropped) {
                    note_degraded(&degraded_sessions, &session_id, DropCounters::default());
                    on_write_error(&error);
                }
            }
            JournalCmd::MarkEnded {
                session_id,
                generation,
                code,
            } => {
                let (degraded, dropped) = degradation_state(&degraded_sessions, &session_id);
                if let Err(error) =
                    mark_ended(&conn, &session_id, generation, code, degraded, dropped)
                {
                    note_degraded(&degraded_sessions, &session_id, DropCounters::default());
                    on_write_error(&error);
                } else {
                    retention_state.session_set_changed();
                }
            }
            JournalCmd::MarkClosed { session_id } => {
                if let Err(error) = mark_closed(&conn, &session_id) {
                    note_degraded(&degraded_sessions, &session_id, DropCounters::default());
                    on_write_error(&error);
                } else {
                    retention_state.session_set_changed();
                }
            }
            JournalCmd::MarkDegraded { session_id } => {
                let (degraded, dropped) = degradation_state(&degraded_sessions, &session_id);
                if let Err(error) = mark_degraded(&conn, &session_id, degraded, dropped) {
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
            JournalCmd::DeleteSession { session_id, reply } => {
                let result = delete_session_user(&conn, &session_id);
                if result.is_ok() {
                    retention_state.session_set_changed();
                }
                let _ = reply.send(result);
            }
            JournalCmd::Usage { reply } => {
                let result = journal_usage(&conn, &pins, limits);
                let _ = reply.send(result);
            }
            JournalCmd::RetentionGet { reply } => {
                let result = journal_retention(&conn, limits);
                let _ = reply.send(result);
            }
            JournalCmd::RetentionSet { patch, reply } => {
                let result = set_journal_retention(&conn, limits, patch);
                let _ = reply.send(result);
            }
            JournalCmd::Pin { session_id, reply } => {
                pins.insert(session_id);
                retention_state.session_set_changed();
                let _ = reply.send(Ok(()));
            }
            JournalCmd::Unpin { session_id } => {
                pins.remove(&session_id);
                retention_state.session_set_changed();
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

fn note_degraded(
    degraded_sessions: &Mutex<HashMap<String, DropCounters>>,
    session_id: &str,
    dropped: DropCounters,
) {
    if let Ok(mut sessions) = degraded_sessions.lock() {
        let counters = sessions.entry(session_id.to_string()).or_default();
        counters.frames = counters.frames.saturating_add(dropped.frames);
        counters.bytes = counters.bytes.saturating_add(dropped.bytes);
    } else {
        eprintln!("journal degradation set is poisoned; treating session {session_id} as degraded");
    }
}

fn degradation_state(
    degraded_sessions: &Mutex<HashMap<String, DropCounters>>,
    session_id: &str,
) -> (bool, DropCounters) {
    match degraded_sessions.lock() {
        Ok(sessions) => sessions
            .get(session_id)
            .copied()
            .map(|counters| (true, counters))
            .unwrap_or_default(),
        Err(_) => {
            eprintln!(
                "journal degradation set is poisoned; treating session {session_id} as degraded"
            );
            (true, DropCounters::default())
        }
    }
}

fn on_write_error(error: &JournalError) {
    eprintln!("journal write failed: {error}");
}

fn upsert_session(conn: &Connection, record: &SessionRecord) -> Result<(), JournalError> {
    conn.execute(
        "INSERT INTO sessions (
            id, owner, workspace_id, kind, title, created_at_ms, updated_at_ms,
            generation, status, exit_code, closed, last_seq, degraded,
            dropped_frames, dropped_bytes, trimmed_bytes, payload_bytes, unsnapshotted_bytes, reaped
        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, 0, ?18)
        ON CONFLICT(id) DO UPDATE SET
            title = excluded.title,
            updated_at_ms = excluded.updated_at_ms,
            generation = excluded.generation,
            status = excluded.status,
            exit_code = excluded.exit_code,
            closed = excluded.closed,
            last_seq = excluded.last_seq,
            degraded = MAX(sessions.degraded, excluded.degraded),
            dropped_frames = MAX(sessions.dropped_frames, excluded.dropped_frames),
            dropped_bytes = MAX(sessions.dropped_bytes, excluded.dropped_bytes),
            trimmed_bytes = MAX(sessions.trimmed_bytes, excluded.trimmed_bytes),
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
            record.dropped_frames as i64,
            record.dropped_bytes as i64,
            record.trimmed_bytes as i64,
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
    retention_state: &mut RetentionState,
) -> Result<(), JournalError> {
    let checksum = crc32(&record.payload) as i64;
    let tx = conn.unchecked_transaction()?;
    let limits = effective_limits(&tx, limits)?;
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
    let add = match record.kind {
        EventKind::Output | EventKind::AcpEnvelope | EventKind::AgentReport => {
            record.payload.len() as i64
        }
        EventKind::Exit => 0,
    };
    let unsnapshotted_add = if matches!(record.kind, EventKind::Output) {
        add
    } else {
        0
    };
    let updated = tx.execute(
        "UPDATE sessions SET
            last_seq = MAX(last_seq, ?1),
            updated_at_ms = ?2,
            payload_bytes = payload_bytes + ?3,
            unsnapshotted_bytes = unsnapshotted_bytes + ?4
         WHERE id = ?5",
        params![
            record.seq as i64,
            record.ts_ms as i64,
            add,
            unsnapshotted_add,
            record.session_id
        ],
    )?;
    if updated == 0 {
        return Err(JournalError::SessionNotFound);
    }
    maybe_snapshot(&tx, &record.session_id, record.generation, limits)?;
    let global_sweep = retention_state.global_sweep_due(add as u64);
    retain(
        &tx,
        pins,
        now_ms(),
        limits,
        &record.session_id,
        global_sweep,
    )?;
    tx.commit()?;
    retention_state.append_committed(add as u64, global_sweep);
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

fn mark_reaped(
    conn: &Connection,
    session_id: &str,
    code: Option<u32>,
    degraded: bool,
    dropped: DropCounters,
) -> Result<(), JournalError> {
    let n = conn.execute(
        "UPDATE sessions SET
            reaped = 1,
            exit_code = COALESCE(?1, exit_code),
            degraded = MAX(degraded, ?2),
            dropped_frames = MAX(dropped_frames, ?3),
            dropped_bytes = MAX(dropped_bytes, ?4),
            updated_at_ms = ?5
         WHERE id = ?6",
        params![
            code.map(|value| value as i64),
            if degraded { 1 } else { 0 },
            dropped.frames as i64,
            dropped.bytes as i64,
            now_ms() as i64,
            session_id,
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
    dropped: DropCounters,
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
        "UPDATE sessions SET
            status = 'ended',
            exit_code = ?1,
            last_seq = ?2,
            degraded = MAX(degraded, ?3),
            dropped_frames = MAX(dropped_frames, ?4),
            dropped_bytes = MAX(dropped_bytes, ?5),
            updated_at_ms = ?6
         WHERE id = ?7",
        params![
            code.map(|value| value as i64),
            seq as i64,
            if degraded { 1 } else { 0 },
            dropped.frames as i64,
            dropped.bytes as i64,
            ts as i64,
            session_id,
        ],
    )?;
    tx.commit()?;
    let _ = conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);");
    Ok(())
}

fn append_permission(conn: &Connection, record: &PermissionRecord) -> Result<(), JournalError> {
    conn.execute(
        "INSERT INTO permissions (session_id, request_id, ts_ms, outcome, payload, checksum)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            &record.session_id,
            &record.request_id,
            record.ts_ms as i64,
            &record.outcome,
            &record.payload,
            crc32(&record.payload) as i64,
        ],
    )?;
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

fn mark_degraded(
    conn: &Connection,
    session_id: &str,
    degraded: bool,
    dropped: DropCounters,
) -> Result<(), JournalError> {
    conn.execute(
        "UPDATE sessions SET
            degraded = MAX(degraded, ?1),
            dropped_frames = MAX(dropped_frames, ?2),
            dropped_bytes = MAX(dropped_bytes, ?3),
            updated_at_ms = ?4
         WHERE id = ?5",
        params![
            if degraded { 1 } else { 0 },
            dropped.frames as i64,
            dropped.bytes as i64,
            now_ms() as i64,
            session_id,
        ],
    )?;
    Ok(())
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
        dropped_frames: 0,
        dropped_bytes: 0,
        payload_bytes: 0,
        trimmed_bytes: 0,
        reaped: false,
    }
}

pub fn agent_report_record(
    session_id: impl Into<String>,
    generation: u64,
    seq: u64,
    event: &SessionEvent,
) -> Option<EventRecord> {
    Some(EventRecord {
        session_id: session_id.into(),
        generation,
        seq,
        kind: EventKind::AgentReport,
        ts_ms: now_ms(),
        payload: serde_json::to_vec(event).ok()?,
    })
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

pub fn acp_envelope_record(
    session_id: impl Into<String>,
    generation: u64,
    seq: u64,
    envelope: &serde_json::Value,
) -> Option<EventRecord> {
    Some(EventRecord {
        session_id: session_id.into(),
        generation,
        seq,
        kind: EventKind::AcpEnvelope,
        ts_ms: now_ms(),
        payload: serde_json::to_vec(envelope).ok()?,
    })
}

#[cfg(test)]
fn tmp_journal() -> (PathBuf, PathBuf) {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let process_id = std::process::id();
    let stamp = now_ms();
    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = create_unique_directory(|attempt| {
        std::env::temp_dir().join(format!(
            "devboule journal {process_id}-{stamp}-{counter}-{attempt}"
        ))
    });
    let path = dir.join("journal.db");
    (dir, path)
}

// A failed test leaves its directory behind. Use create_dir, rather than
// create_dir_all, so a reused PID can never reopen another run's database.
#[cfg(test)]
fn create_unique_directory(mut candidate: impl FnMut(u64) -> PathBuf) -> PathBuf {
    for attempt in 0..1000 {
        let dir = candidate(attempt);
        match std::fs::create_dir(&dir) {
            Ok(()) => return dir,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => panic!("could not create test directory: {error}"),
        }
    }
    panic!("could not find an unused test directory");
}

#[cfg(test)]
fn snapshot_limits() -> JournalLimits {
    JournalLimits {
        snapshot_every_bytes: 32,
        session_max_bytes: JOURNAL_SESSION_MAX_BYTES,
        max_bytes: JOURNAL_MAX_BYTES,
        max_sessions: JOURNAL_MAX_SESSIONS,
        max_age_ms: JOURNAL_MAX_AGE_MS,
    }
}

#[cfg(test)]
fn tiny_limits() -> JournalLimits {
    JournalLimits {
        snapshot_every_bytes: JOURNAL_SESSION_MAX_BYTES,
        session_max_bytes: JOURNAL_SESSION_MAX_BYTES,
        max_bytes: JOURNAL_MAX_BYTES,
        max_sessions: 2,
        max_age_ms: JOURNAL_MAX_AGE_MS,
    }
}

#[cfg(test)]
fn sample_session(id: &str) -> SessionRecord {
    new_session_record(id, "S-1-5-21-1", None, SessionKind::Terminal, "Terminal")
}

#[cfg(test)]
mod tests {
    use super::*;
    use devboule_protocol::TranscriptIntegrity;
    use std::process::Command;

    #[test]
    fn journal_test_directory_does_not_reuse_pid_counter_candidate() {
        let process_id = std::process::id();
        let mut candidates = Vec::new();
        let mut created = Vec::new();
        for counter in 1..=256 {
            let dir = std::env::temp_dir().join(format!("devboule journal {process_id}-{counter}"));
            candidates.push(dir.clone());
            if std::fs::create_dir(&dir).is_ok() {
                created.push(dir);
            }
        }

        let (selected, _) = tmp_journal();
        let reused = candidates.iter().any(|dir| dir == &selected);
        let selected_display = selected.display().to_string();
        if !reused {
            let _ = std::fs::remove_dir_all(&selected);
        }
        for dir in created {
            let _ = std::fs::remove_dir_all(dir);
        }
        assert!(
            !reused,
            "reused legacy journal test directory: {selected_display}"
        );
    }

    #[test]
    fn records_without_terminators_are_always_unverifiable() {
        for status in [PersistStatus::Interrupted, PersistStatus::Live] {
            let mut record = sample_session("s.unverifiable");
            record.status = status;

            assert_eq!(
                record.to_session().state,
                SessionState::Recovered {
                    generation: 1,
                    integrity: TranscriptIntegrity::Unverifiable {
                        dropped_frames: 0,
                        dropped_bytes: 0,
                        trimmed_bytes: 0,
                    },
                }
            );
        }
    }

    #[test]
    fn interrupted_loss_keeps_counters_but_not_certification() {
        let mut record = sample_session("s.unverifiable.loss");
        record.status = PersistStatus::Interrupted;
        record.degraded = true;
        record.dropped_frames = 7;
        record.dropped_bytes = 4096;

        assert_eq!(
            record.to_session().state,
            SessionState::Recovered {
                generation: 1,
                integrity: TranscriptIntegrity::Unverifiable {
                    dropped_frames: 7,
                    dropped_bytes: 4096,
                    trimmed_bytes: 0,
                },
            }
        );
    }

    #[test]
    fn ended_loss_replays_degraded_before_exit_and_reports_truncated() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open(&path).expect("open");
        let mut record = sample_session("s.ended.loss");
        record.status = PersistStatus::Ended;
        record.degraded = true;
        record.dropped_frames = 3;
        record.dropped_bytes = 4096;
        journal.upsert_blocking(record).expect("upsert");

        let replay = journal.replay("s.ended.loss", 0).expect("replay");
        assert_eq!(
            replay.events,
            vec![
                SessionEvent::JournalDegraded {
                    dropped_frames: 3,
                    dropped_bytes: 4096,
                },
                SessionEvent::Exit { code: None },
            ]
        );
        assert_eq!(
            replay.integrity,
            TranscriptIntegrity::Truncated {
                dropped_frames: 3,
                dropped_bytes: 4096,
                trimmed_bytes: 0,
            }
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ended_clean_replays_exit_only_and_reports_complete() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open(&path).expect("open");
        let mut record = sample_session("s.ended.clean");
        record.status = PersistStatus::Ended;
        journal.upsert_blocking(record).expect("upsert");

        let replay = journal.replay("s.ended.clean", 0).expect("replay");
        assert_eq!(replay.events, vec![SessionEvent::Exit { code: None }]);
        assert_eq!(replay.integrity, TranscriptIntegrity::Complete);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn enqueue_drops_count_the_exact_payload_sizes() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open(&path).expect("open");
        journal
            .upsert_blocking(sample_session("s.drop").clone())
            .expect("upsert");
        let blocker = Connection::open(&path).expect("blocker");
        blocker
            .execute_batch("BEGIN EXCLUSIVE")
            .expect("hold sqlite writer lock");

        while journal.queued.load(Ordering::Acquire)
            < JOURNAL_QUEUE_CAP.saturating_sub(CONTROL_RESERVE) as u64
        {
            assert!(journal.try_append(output_record("s.drop", 1, 1, b"fill")));
        }
        let baseline = journal
            .degraded_sessions
            .lock()
            .expect("degradation baseline")
            .get("s.drop")
            .copied()
            .unwrap_or_default();
        assert!(!journal.try_append(output_record("s.drop", 1, 2, b"abc")));
        assert!(!journal.try_append(output_record("s.drop", 1, 3, b"12345678")));

        let counters = journal
            .degraded_sessions
            .lock()
            .expect("degradation counters")
            .get("s.drop")
            .copied()
            .expect("dropped session");
        assert_eq!(counters.frames - baseline.frames, 2);
        assert_eq!(counters.bytes - baseline.bytes, 3 + 8);

        drop(blocker);
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn permission_decision_is_written_with_outcome_timestamp_and_payload() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open(&path).expect("open");
        journal
            .upsert_blocking(sample_session("s.permission.audit"))
            .expect("session row");
        let payload = br#"{"type":"permission_request","toolCallId":"tool-1"}"#;
        journal
            .record_permission("s.permission.audit", "tool-1", "allow_once", payload)
            .expect("permission row");
        journal.flush().expect("permission flush");
        let conn = Connection::open(&path).expect("inspect");
        let row = conn
            .query_row(
                "SELECT ts_ms, outcome, payload, checksum FROM permissions
                 WHERE session_id = ?1 AND request_id = ?2",
                ["s.permission.audit", "tool-1"],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Vec<u8>>(2)?,
                        row.get::<_, i64>(3)?,
                    ))
                },
            )
            .expect("permission row");
        assert!(row.0 > 0);
        assert_eq!(row.1, "allow_once");
        assert_eq!(row.2, payload);
        assert_eq!(row.3, crc32(payload) as i64);
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn permission_reuse_is_rejected_without_overwriting_the_audit_row() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open(&path).expect("open");
        journal
            .upsert_blocking(sample_session("s.permission.reuse"))
            .expect("session row");
        journal
            .record_permission(
                "s.permission.reuse",
                "tool-1",
                "allow_once",
                br#"{"decision":1}"#,
            )
            .expect("first permission row");
        assert!(journal
            .record_permission("s.permission.reuse", "tool-1", "deny", br#"{"decision":2}"#,)
            .is_err());
        journal.flush().expect("permission flush");
        let conn = Connection::open(&path).expect("inspect");
        let rows: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM permissions WHERE session_id = ?1 AND request_id = ?2",
                ["s.permission.reuse", "tool-1"],
                |row| row.get(0),
            )
            .expect("permission rows");
        assert_eq!(rows, 1);
        let row: (String, Vec<u8>) = conn
            .query_row(
                "SELECT outcome, payload FROM permissions
                 WHERE session_id = ?1 AND request_id = ?2",
                ["s.permission.reuse", "tool-1"],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("original permission row");
        assert_eq!(row.0, "allow_once");
        assert_eq!(row.1, br#"{"decision":1}"#);
        journal.shutdown();
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
            [SessionEvent::Output { seq: 1, data: a }, SessionEvent::Output { seq: 2, data: b }, SessionEvent::Recovered {
                integrity:
                    TranscriptIntegrity::Unverifiable {
                        dropped_frames: 0,
                        dropped_bytes: 0,
                        ..
                    },
            }] => {
                assert_eq!(a, "one");
                assert_eq!(b, "two");
            }
            other => panic!("unexpected replay: {other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn agent_report_survives_replay() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open(&path).expect("open");
        journal
            .upsert_blocking(sample_session("s.a.1"))
            .expect("upsert");
        journal
            .append_blocking(output_record("s.a.1", 1, 1, b"one"))
            .expect("output");
        let event = SessionEvent::AgentReported {
            seq: 2,
            source: "devboule:stub".to_string(),
            agent: "stub".to_string(),
            state: devboule_protocol::AgentActivityState::Working,
            message: None,
            report_seq: Some(7),
            agent_session_id: Some("agent-1".to_string()),
            agent_session_path: None,
            session_start_source: Some("startup".to_string()),
        };
        journal
            .append_blocking(agent_report_record("s.a.1", 1, 2, &event).expect("record"))
            .expect("report");
        let replay = journal.replay("s.a.1", 0).expect("replay");
        assert!(
            replay.events.iter().any(|item| item == &event),
            "replay missing agent report: {:?}",
            replay.events
        );
        let payload = serde_json::to_vec(&event).expect("payload");
        let stored = journal
            .list()
            .expect("list")
            .into_iter()
            .find(|row| row.id == "s.a.1")
            .expect("session");
        assert_eq!(
            stored.payload_bytes,
            b"one".len() as u64 + payload.len() as u64,
            "agent_report must count toward payload_bytes"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn acp_envelopes_do_not_leave_unsnapshotted_bytes_stuck() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open_with_limits(
            &path,
            JournalLimits {
                snapshot_every_bytes: 8,
                ..JournalLimits::default()
            },
        )
        .expect("open");
        let mut session = sample_session("s.acp.snap");
        session.kind = SessionKind::Acp;
        journal.upsert_blocking(session).expect("upsert");
        let envelope = serde_json::json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": {
                "sessionId": "s",
                "update": {
                    "sessionUpdate": "agent_thought_chunk",
                    "content": {"type": "text", "text": "The"}
                }
            }
        });
        for seq in 1..=4 {
            journal
                .append_blocking(acp_envelope_record("s.acp.snap", 1, seq, &envelope).expect("rec"))
                .expect("append");
        }
        journal.flush().expect("flush");
        let conn = Connection::open(&path).expect("inspect");
        let unsnapshotted: i64 = conn
            .query_row(
                "SELECT unsnapshotted_bytes FROM sessions WHERE id = ?1",
                ["s.acp.snap"],
                |row| row.get(0),
            )
            .expect("unsnapshotted");
        assert_eq!(
            unsnapshotted, 0,
            "ACP envelopes must not accumulate unsnapshotted_bytes"
        );
        let snapshots: i64 = conn
            .query_row("SELECT COUNT(*) FROM snapshots", [], |row| row.get(0))
            .expect("snapshots");
        assert_eq!(
            snapshots, 0,
            "ACP envelopes must not create output snapshots"
        );
        journal.shutdown();
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
    fn poisoned_degradation_set_is_fail_closed() {
        let (dir, path) = tmp_journal();
        let journal = Journal::open(&path).expect("open");
        journal
            .upsert_blocking(sample_session("s.poisoned"))
            .expect("upsert");
        let poisoned = Arc::clone(&journal.degraded_sessions);
        let panic = std::thread::spawn(move || {
            let _sessions = poisoned.lock().expect("degradation lock");
            panic!("simulate a journal-state panic");
        });
        assert!(panic.join().is_err());

        assert!(journal.note_session_degraded("s.poisoned"));
        assert!(journal.is_session_degraded("s.poisoned"));
        journal.flush().expect("degradation marker");
        assert!(journal
            .list()
            .expect("list")
            .into_iter()
            .find(|row| row.id == "s.poisoned")
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
            Some(SessionEvent::Recovered {
                integrity: TranscriptIntegrity::Unverifiable {
                    dropped_frames: 0,
                    dropped_bytes: 0,
                    ..
                },
            })
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
    fn unclean_reopen_reports_doubt_not_completeness() {
        // The cross the protocol contract requires: measured missing bytes
        // and the honest flags must agree. After a process death the
        // database is the only witness, and it cannot know what was still
        // uncommitted in the dying writer queue, so:
        //
        // - a session whose journal was not closed orderly is ALWAYS
        //   Recovered, whatever the degraded column says: Recovered means
        //   "tail unverifiable";
        // - no terminator is always Unverifiable, with any measured loss
        //   carried as counters. A terminator is required before Truncated
        //   can certify the remaining tail.
        let (dir, path) = tmp_journal();

        // Silent queue death: five frames committed, five more produced
        // into the queue and lost with the process. No failure was ever
        // observed, so no truncation may be claimed — but the session
        // must be Recovered, never presented as complete.
        {
            let journal = Journal::open(&path).expect("open");
            journal
                .upsert_blocking(sample_session("s.cross.silent"))
                .expect("upsert");
            for seq in 1..=5 {
                journal
                    .append_blocking(output_record("s.cross.silent", 1, seq, b"data"))
                    .expect("append");
            }
            // The drop simulates the kill: the row stays status=live, so
            // the reopen sees a journal nobody closed orderly.
            drop(journal);
        }
        {
            let journal = Journal::open(&path).expect("reopen");
            let listed = journal.list().expect("list");
            let row = listed
                .iter()
                .find(|row| row.id == "s.cross.silent")
                .expect("session row");
            assert!(matches!(
                row.to_session().state,
                SessionState::Recovered {
                    integrity: TranscriptIntegrity::Unverifiable {
                        dropped_frames: 0,
                        dropped_bytes: 0,
                        ..
                    },
                    ..
                }
            ));
            let replay = journal.replay("s.cross.silent", 0).expect("replay");
            let replay_bytes: usize = replay
                .events
                .iter()
                .filter_map(|event| match event {
                    SessionEvent::Output { data, .. } => Some(data.len()),
                    _ => None,
                })
                .sum();
            assert_eq!(replay_bytes, 20, "committed frames must replay intact");
            assert!(matches!(
                replay.events.last(),
                Some(SessionEvent::Recovered {
                    integrity: TranscriptIntegrity::Unverifiable {
                        dropped_frames: 0,
                        dropped_bytes: 0,
                        ..
                    },
                })
            ));
            assert!(
                matches!(
                    replay.integrity,
                    TranscriptIntegrity::Unverifiable {
                        dropped_frames: 0,
                        dropped_bytes: 0,
                        ..
                    }
                ),
                "an unclosed journal must stay unverifiable"
            );
        }

        // Observed loss: the same five committed frames, but the previous
        // daemon recorded degradation (queue pressure). The tail is still
        // unverifiable because no terminator committed.
        {
            let journal = Journal::open(&path).expect("open");
            journal
                .upsert_blocking(sample_session("s.cross.declared"))
                .expect("upsert");
            for seq in 1..=5 {
                journal
                    .append_blocking(output_record("s.cross.declared", 1, seq, b"data"))
                    .expect("append");
            }
            journal.note_session_degraded("s.cross.declared");
            journal.flush().expect("degradation marker");
            drop(journal);
        }
        {
            let journal = Journal::open(&path).expect("reopen");
            let listed = journal.list().expect("list");
            let row = listed
                .iter()
                .find(|row| row.id == "s.cross.declared")
                .expect("session row");
            assert!(matches!(
                row.to_session().state,
                SessionState::Recovered {
                    integrity: TranscriptIntegrity::Unverifiable {
                        dropped_frames: 0,
                        dropped_bytes: 0,
                        ..
                    },
                    ..
                }
            ));
            let replay = journal.replay("s.cross.declared", 0).expect("replay");
            assert_eq!(
                replay.integrity,
                TranscriptIntegrity::Unverifiable {
                    dropped_frames: 0,
                    dropped_bytes: 0,
                    trimmed_bytes: 0,
                }
            );
            assert!(matches!(
                replay.events.last(),
                Some(SessionEvent::Recovered {
                    integrity: TranscriptIntegrity::Unverifiable {
                        dropped_frames: 0,
                        dropped_bytes: 0,
                        ..
                    },
                })
            ));
        }
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
