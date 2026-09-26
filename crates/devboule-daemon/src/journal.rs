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
    ErrorCode, JournalRetention, PeerRole, Project, RetentionPatch, Session, SessionEvent,
    SessionKind, SessionOrigin, SessionOriginKind, SessionState, TranscriptIntegrity,
    UnattendedState, WireError, Workspace, WorkspaceIsolation,
};

#[path = "journal_replay.rs"]
mod journal_replay;
#[path = "journal_retention.rs"]
mod journal_retention;
#[path = "journal_schema.rs"]
mod journal_schema;

pub(crate) use journal_replay::AgentReplayPage;
use journal_replay::{list_sessions, owned_child_record, replay_agent_page, replay_session};
use journal_retention::{
    delete_session_user, effective_limits, journal_retention, journal_usage, retain,
    set_journal_retention, RetentionState,
};
use journal_schema::{open_connection, sweep_audit};

/// Stored in `PRAGMA user_version`. Bump whenever the journal schema gains
/// tables or columns that need migration.
pub const JOURNAL_SCHEMA_VERSION: i32 = 15;

/// How often the append path enforces the audit age floor and per-device cap.
/// The session retention sweep is byte-driven, not time-driven, so the hourly
/// clock belongs to this loop; an idle daemon writes no audit rows anyway.
const AUDIT_SWEEP_INTERVAL: Duration = Duration::from_secs(60 * 60);

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
/// A cold workspace lookup is allowed to fail fast because it is on the
/// session-create path. Warm lookups use SessionRegistry's in-memory cache;
/// a busy writer must never make a new process wait ten seconds for a cwd.
const WORKSPACE_LOOKUP_WAIT: Duration = Duration::from_millis(500);
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
    FutureSchema {
        found: i32,
        supported: i32,
    },
    Corrupt(String),
    Unavailable(String),
    SessionNotFound,
    /// A session row already holds this id and a birth was aimed at it.
    /// Two sessions on one row cannot be told apart afterwards, so the
    /// birth stops instead of averaging the two.
    SessionExists {
        id: String,
    },
    LiveSession,
    InvalidRequest(String),
    Checksum {
        session_id: String,
        seq: u64,
    },
    Timeout,
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
            Self::SessionExists { id } => write!(
                formatter,
                "session id {id} already names a journalled session; refusing to write a second session onto it"
            ),
            Self::LiveSession => write!(formatter, "Close the session before deleting it."),
            Self::InvalidRequest(message) => write!(formatter, "{message}"),
            Self::Checksum { session_id, seq } => {
                write!(
                    formatter,
                    "journal checksum mismatch for {session_id} seq {seq}"
                )
            }
            Self::Timeout => write!(formatter, "journal request timed out"),
            Self::Stopped => write!(formatter, "journal writer has stopped"),
        }
    }
}

impl std::error::Error for JournalError {}

impl From<JournalError> for WireError {
    fn from(error: JournalError) -> Self {
        let code = match error {
            JournalError::SessionNotFound => ErrorCode::SessionNotFound,
            JournalError::SessionExists { .. } => ErrorCode::InvalidRequest,
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
    /// The directory the session's process was **actually launched in**, as
    /// the daemon passed it to the child — the workspace's own path, an
    /// agent child's confined subdirectory, or whatever a session with no
    /// workspace was started from. `None` for every row that predates v15,
    /// which is the honest "nobody recorded it": such a row resumes exactly
    /// as it did before the column existed.
    ///
    /// This is not a second source of truth for a workspace session: the
    /// workspace is resolved from its id, as it always was. It is the only
    /// record at all for a session that has no workspace — and the one fact
    /// a resume can check before spawning, so a folder that is gone is
    /// refused in words instead of in a provider's crash.
    pub cwd: Option<String>,
    pub kind: SessionKind,
    /// Catalog provider id used to start an ACP/Claude session. NULL means
    /// this row predates provider persistence or was not resumable.
    pub provider: Option<String>,
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
    /// Provider-side session id used by a future resume/load handshake.
    pub peer_session_id: Option<String>,
    /// The handle a provider refused, recorded — never destroyed. Evidence
    /// for a refusal is approximate (a locked directory reads like a deleted
    /// conversation; an echoed request can wear a resource miss), so the act
    /// it triggers must be reversible: the handle stays, this column names
    /// the refusal, and a later announce of a different handle clears it.
    /// NULL says nothing happened; a value here is a fact learned from the
    /// provider about the handle it names.
    pub disowned_peer_session_id: Option<String>,
    /// The resume gate (`resume_handle`) deliberately does NOT consult this
    /// column, and that is the design, not an oversight: the evidence for a
    /// refusal is approximate, so the mark hides an offer without forbidding
    /// the act. It is the road by which a wrong mark gets cured — a retry
    /// that works clears it from the resume's success arm — and a mark that
    /// both hid the button and refused the call would make a false positive
    /// permanent, which is exactly what this mechanism exists to avoid.
    /// Who asked for this session (§8 R2). Written once, by the create that
    /// made the row; read by the peer gate and the permission card's
    /// provenance line. Every row that predates v9 is `local`.
    pub origin: SessionOrigin,
    /// The name the human reads for this session (`S5` decision 9b, audit
    /// S5-12). NULL for a session created without one and for every row that
    /// predates v10: the surfaces fall back to the title, exactly as they did
    /// before the column existed.
    pub display_name: Option<String>,
    /// The session that created this one, when an agent did (`S5` decision 9a).
    /// NULL for a session a human asked for, and for every row that predates
    /// v10.
    pub created_by: Option<String>,
    /// The profile this session was created from, by its stable **id** — the
    /// one value a rename cannot change. NULL for a session a human started
    /// from the provider picker and for every row that predates v11.
    pub profile_id: Option<String>,
    /// The context this session belongs to: its own id, or the context of the
    /// session that created it. NULL only for rows that predate v11, which
    /// [`SessionRecord::to_session`] reads back as the session's own id.
    pub context_id: Option<String>,
    /// Whether this session can pass a permission moment with no human
    /// answering, as the tri-state [`UnattendedState`] knows it. Derived once,
    /// by the creation that delivered the mode, and never re-derived:
    /// un-ticking the profile afterwards does not change the row.
    ///
    /// The boolean `unattended` **column** stays beside the tri-state
    /// `unattended_state` column this value is stored in: it is ratcheted by
    /// the same `MAX` as before and absorbs the `yes` half for rows written
    /// before v12, so a database this daemon upgrades never has a rewritten
    /// column. The v12 backfill maps an old `1` to `yes` and an old `0` to
    /// **`unknown`** — never `no`: the old bool never distinguished "we knew
    /// a human was watching" from "we were not told", and reading it as `no`
    /// would manufacture a certainty that was never recorded.
    pub unattended_state: UnattendedState,
    /// The session's labels, as the JSON object the daemon stamped. Empty for a
    /// session with none (a human's own sessions carry none), and for every row
    /// that predates v11.
    pub labels: std::collections::BTreeMap<String, String>,
    /// The tool overlay the creation stamped on this child, as the deny list
    /// it resolved at birth. Read back at resume instead of re-resolving the
    /// profile, whose answer may have changed since. `None` is an unreadable
    /// cell — never a recorded value, since every write carries a definite
    /// overlay — and only the resume path judges it; the roster reads the
    /// cell but decides nothing from it.
    ///
    /// `pub(crate)` while the sibling fields are `pub`: the overlay type
    /// itself is crate-internal, and widening it for one field would grow
    /// the crate's API for nothing the app ever names.
    pub(crate) overlay: Option<crate::provider_catalog::ToolOverlay>,
    /// The child's own depth at birth: its creator's depth, plus one. A
    /// birth fact like the overlay above, read back at resume so the depth
    /// cap survives a restart. `None` is a row that predates the column;
    /// the resume mapping, not this field, decides what that resumes as —
    /// and only for rows that still name a creator.
    pub(crate) depth: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectRecord {
    pub id: String,
    pub name: String,
    pub path: String,
    /// `repository`, `inside_repository`, `not_repository`, or `unknown`.
    /// This is persisted so missing git and a non-repository stay distinct.
    pub git_state: String,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

impl ProjectRecord {
    pub fn to_project(&self) -> Project {
        Project {
            id: self.id.clone(),
            name: self.name.clone(),
            path: crate::workspace::plain_path(&self.path),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceRecord {
    pub id: String,
    pub project_id: String,
    pub title: String,
    pub isolation: WorkspaceIsolation,
    pub path: String,
    /// Exact git branch for a worktree workspace. `None` for Local.
    pub branch: Option<String>,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

impl WorkspaceRecord {
    pub fn to_workspace(&self) -> Workspace {
        Workspace {
            id: self.id.clone(),
            project_id: self.project_id.clone(),
            title: self.title.clone(),
            isolation: self.isolation,
            path: crate::workspace::plain_path(&self.path),
        }
    }
}

/// One paired device, as stored in `peers`.
///
/// `role` is `client` or `daemon` (the CHECK constraint holds the same set).
/// `caps` is a JSON array of capability names. `paired_by_user` is **this**
/// daemon's own user SID at pairing time, written by this side: it is never
/// received from the peer and never trusted from a frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PeerRecord {
    pub device_id: String,
    pub display_name: String,
    pub role: String,
    pub public_key: Vec<u8>,
    pub paired_by_user: Option<String>,
    pub binding_kind: String,
    pub binding_stable_id: Option<String>,
    pub binding_node_name: Option<String>,
    pub binding_login_name: Option<String>,
    pub address: String,
    pub paired_at: i64,
    pub revoked_at: Option<i64>,
    pub caps: Vec<String>,
}

impl PeerRecord {
    pub fn is_revoked(&self) -> bool {
        self.revoked_at.is_some()
    }

    /// The numeric address half of the stored `address` (`ip:port` at
    /// pairing time, a bare IP in early rows).
    pub fn address_ip(&self) -> Option<std::net::IpAddr> {
        let raw = self.address.trim();
        if let Ok(address) = raw.parse::<std::net::IpAddr>() {
            return Some(address);
        }
        raw.parse::<std::net::SocketAddr>()
            .ok()
            .map(|socket| socket.ip())
    }

    /// Whether `address` is this peer's stored address. Numeric comparison,
    /// so `100.64.0.1` and `100.64.0.10` cannot match each other. Used by the
    /// pre-Noise filter, which is the only check that runs before a byte is
    /// read.
    pub fn owns_address(&self, address: &std::net::IpAddr) -> bool {
        self.address_ip()
            .map(|owned| owned == *address)
            .unwrap_or(false)
    }
}

/// One append-only audit row. `device_id` and `role` always come from the
/// authenticated connection, never from a frame; for a local connection
/// `device_id` is this device's UUID and `role` is `"local"`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuditRecord {
    pub device_id: String,
    pub role: String,
    pub claimed_origin: Option<String>,
    pub action: String,
    pub session_id: Option<String>,
    pub outcome: String,
}

/// What one `audit_sweep` removed. Both halves are reported so a test can
/// tell the age floor from the per-device cap.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AuditSweep {
    pub deleted_by_age: u64,
    pub deleted_by_cap: u64,
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
        // Read before the struct moves `state`: a journal row is never a
        // running child, so this is the transcript half of the verdict.
        let is_live = state.is_live();
        Session {
            id: self.id.clone(),
            workspace_id: self.workspace_id.clone(),
            // The recorded directory, in the same display form the creation
            // echoed: it is what the process really received, and the resume
            // reads it back rather than re-deriving it from `workspace_id` —
            // a session with no workspace has no other record of where it
            // worked, and a workspace whose folder moved would send the next
            // process somewhere the dead one never was.
            cwd: self.cwd.as_deref().map(crate::workspace::plain_path),
            kind: self.kind.clone(),
            title: self.title.clone(),
            provider: self.provider.clone(),
            peer_session_id: self.peer_session_id.clone(),
            state,
            elapsed_ms: None,
            created_at_ms: self.created_at_ms,
            origin: self.origin.clone(),
            // Both are the row's now (audit S5-12): a transcript recovered
            // after a restart lists under the name the human saw and keeps the
            // parent it was created by.
            display_name: self.display_name.clone(),
            created_by: self.created_by.clone(),
            profile_id: self.profile_id.clone(),
            context_id: Some(self.context()),
            unattended: self.unattended_state,
            labels: self.labels.clone(),
            // The verdict the app renders: dead process, admitted family,
            // both columns present, and the handle not being the one a
            // provider refused. Computed here, from the trait, so the wire
            // never re-spells it.
            resumable: crate::session::session_resumable(
                &self.kind,
                self.provider.as_deref(),
                self.peer_session_id.as_deref(),
                is_live,
                self.disowned_peer_session_id.as_deref(),
            ),
        }
    }

    /// The context this session belongs to: the row's own value when it has one,
    /// and its own id otherwise.
    ///
    /// The second half is the rule [`Session::context_id`] states for a session
    /// no other session created, and it is applied here rather than written into
    /// the v11 migration because a row that predates the column has no creator
    /// to inherit from in the daemon's own words — deriving it keeps one place
    /// that answers "what context is this session in".
    pub fn context(&self) -> String {
        self.context_id.clone().unwrap_or_else(|| self.id.clone())
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
    /// Journal position for each `events` entry, as `(generation, seq)`.
    /// Stream seqs restart per generation, so the pair — the order the
    /// `events_session` index serves — is the transcript's real order.
    pub event_seqs: Vec<(u64, u64)>,
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
    /// The journal's own `display_name` column, verbatim: `None` for a row that
    /// has no name of its own. Usage reports what the row says; the fallback a
    /// nameless row is shown under is the app's business, not the query's.
    pub display_name: Option<String>,
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
    CreateSession {
        record: SessionRecord,
        reply: mpsc::Sender<Result<(), JournalError>>,
    },
    Append(EventRecord),
    Permission {
        record: PermissionRecord,
        reply: mpsc::Sender<Result<(), JournalError>>,
    },
    PermissionWasRecorded {
        request_id: String,
        reply: mpsc::Sender<Result<bool, JournalError>>,
    },
    PermissionWasRecordedIn {
        session_id: String,
        request_id: String,
        reply: mpsc::Sender<Result<bool, JournalError>>,
    },
    PermissionCount {
        session_id: String,
        reply: mpsc::Sender<Result<u32, JournalError>>,
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
    SetPeerSessionId {
        session_id: String,
        peer_session_id: String,
        reply: mpsc::Sender<Result<(), JournalError>>,
    },
    /// The resume road's disown mark: the provider refused this handle.
    /// `expected` names the refused handle, so a concurrent respawn's NEWER
    /// handle is never silenced; the refused handle itself is never
    /// destroyed.
    MarkPeerSessionDisowned {
        session_id: String,
        expected: String,
        reply: mpsc::Sender<Result<(), JournalError>>,
    },
    /// The success road's clear: the provider honoured `handle`, so a
    /// refusal recorded against it is stale.
    ClearPeerSessionDisown {
        session_id: String,
        handle: String,
        reply: mpsc::Sender<Result<(), JournalError>>,
    },
    /// A `devboule_set_agent_profile` move's recording: the child's profile
    /// column and the `unattended` ratchet, nothing else — see
    /// [`Journal::set_agent_profile_row`].
    SetAgentProfile {
        session_id: String,
        profile_id: Option<String>,
        unattended: UnattendedState,
        reply: mpsc::Sender<Result<(), JournalError>>,
    },
    StartGeneration {
        session_id: String,
        generation: u64,
        reply: mpsc::Sender<Result<(), JournalError>>,
    },
    MarkDegraded {
        session_id: String,
    },
    List {
        reply: mpsc::Sender<Result<Vec<SessionRecord>, JournalError>>,
    },
    OwnedChildRecord {
        session_id: String,
        owner: String,
        created_by: String,
        reply: mpsc::Sender<Result<Option<SessionRecord>, JournalError>>,
    },
    ProjectsList {
        reply: mpsc::Sender<Result<Vec<ProjectRecord>, JournalError>>,
    },
    ProjectAdd {
        record: ProjectRecord,
        reply: mpsc::Sender<Result<ProjectRecord, JournalError>>,
    },
    ProjectGet {
        id: String,
        reply: mpsc::Sender<Result<Option<ProjectRecord>, JournalError>>,
    },
    WorkspacesList {
        project_id: String,
        reply: mpsc::Sender<Result<Vec<WorkspaceRecord>, JournalError>>,
    },
    WorkspaceCreate {
        record: WorkspaceRecord,
        reply: mpsc::Sender<Result<WorkspaceRecord, JournalError>>,
    },
    WorkspaceGet {
        id: String,
        reply: mpsc::Sender<Result<Option<WorkspaceRecord>, JournalError>>,
    },
    WorkspaceDelete {
        id: String,
        reply: mpsc::Sender<Result<(), JournalError>>,
    },
    Replay {
        session_id: String,
        reply: mpsc::Sender<Result<Replay, JournalError>>,
    },
    ReplayAgentPage {
        session_id: String,
        generation: u64,
        from_generation: u64,
        from_seq: u64,
        through_seq: u64,
        limit: usize,
        reply: mpsc::Sender<Result<AgentReplayPage, JournalError>>,
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
    PeersList {
        reply: mpsc::Sender<Result<Vec<PeerRecord>, JournalError>>,
    },
    PeerUpsert {
        record: PeerRecord,
        reply: mpsc::Sender<Result<PeerRecord, JournalError>>,
    },
    PeerGet {
        device_id: String,
        reply: mpsc::Sender<Result<Option<PeerRecord>, JournalError>>,
    },
    PeerRevoke {
        device_id: String,
        at: i64,
        reply: mpsc::Sender<Result<PeerMutation, JournalError>>,
    },
    PeerSetCaps {
        device_id: String,
        caps: Vec<String>,
        reply: mpsc::Sender<Result<PeerMutation, JournalError>>,
    },
    AuditAppend {
        record: AuditRecord,
        at: Option<i64>,
        reply: mpsc::Sender<Result<(), JournalError>>,
    },
    AuditSweep {
        reply: mpsc::Sender<Result<AuditSweep, JournalError>>,
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
    session_set_revision: Arc<AtomicU64>,
    path: PathBuf,
}

struct JournalLoopState {
    degraded_sessions: Arc<Mutex<HashMap<String, DropCounters>>>,
    stats: Arc<JournalStats>,
    session_set_revision: Arc<AtomicU64>,
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
        let session_set_revision = Arc::new(AtomicU64::new(0));
        let session_set_revision_thread = Arc::clone(&session_set_revision);
        let path_buf = path.to_path_buf();
        let thread_path = path_buf.clone();
        let join = std::thread::Builder::new()
            .name("daemon-journal".into())
            .spawn(move || {
                journal_loop(
                    conn,
                    rx,
                    queued_thread,
                    JournalLoopState {
                        degraded_sessions: degraded_sessions_thread,
                        stats: stats_thread,
                        session_set_revision: session_set_revision_thread,
                    },
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
            session_set_revision,
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

    /// Changes only when a journal operation can change the session roster.
    /// Roster readers use this cheap atomic to notice background retention
    /// without querying SQLite on every live-session transition.
    pub(crate) fn session_set_revision(&self) -> u64 {
        self.session_set_revision.load(Ordering::Acquire)
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

    /// Whether any permission card with this request id has ever been
    /// resolved, by any session. The delegated answer's "already resolved"
    /// sentence: a replayed id must read as resolved, an invented one as
    /// unknown, and neither may touch anything. Asked by request id alone —
    /// the caller does not know which session held the card, and the answer
    /// reveals nothing but the fact.
    pub fn permission_was_recorded(&self, request_id: &str) -> Result<bool, JournalError> {
        self.rpc(|reply| JournalCmd::PermissionWasRecorded {
            request_id: request_id.to_string(),
            reply,
        })
    }

    /// Whether THIS session's journal holds a decision for this request id —
    /// what the register-time refusal reads: the permissions row is
    /// write-once per (session, id), so a second card for an id that is
    /// already answered could never record its own answer. Both key parts,
    /// unlike [`Self::permission_was_recorded`]: two sessions legitimately
    /// use ids of their own.
    pub fn permission_was_recorded_in_session(
        &self,
        session_id: &str,
        request_id: &str,
    ) -> Result<bool, JournalError> {
        self.rpc(|reply| JournalCmd::PermissionWasRecordedIn {
            session_id: session_id.to_string(),
            request_id: request_id.to_string(),
            reply,
        })
    }

    /// How many permission cards of one session were resolved — the
    /// snapshot's delegation count. Counted from the `permissions` table,
    /// which is the resolution ledger replay reads back, so the count
    /// survives a restart the way the ledger does.
    pub fn permission_count(&self, session_id: &str) -> Result<u32, JournalError> {
        self.rpc(|reply| JournalCmd::PermissionCount {
            session_id: session_id.to_string(),
            reply,
        })
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

    pub fn set_peer_session_id(
        &self,
        session_id: &str,
        peer_session_id: &str,
    ) -> Result<(), JournalError> {
        self.rpc(|reply| JournalCmd::SetPeerSessionId {
            session_id: session_id.to_string(),
            peer_session_id: peer_session_id.to_string(),
            reply,
        })
    }

    /// The failed-resume road's disown mark, issued from the dispatch thread:
    /// the rpc returns only after the write is committed, so any roster read
    /// issued after the failing resume's answer is behind it. `expected` is
    /// the handle the resume tried to load; a row that already carries a
    /// different handle keeps it and stays unmarked.
    pub fn mark_peer_session_disowned(
        &self,
        session_id: &str,
        expected: &str,
    ) -> Result<(), JournalError> {
        self.rpc(|reply| JournalCmd::MarkPeerSessionDisowned {
            session_id: session_id.to_string(),
            expected: expected.to_string(),
            reply,
        })
    }

    /// The mark's fallback for a saturated queue, on the failed resume's
    /// throwaway thread: it waits for queue space like `mark_ended_blocking`
    /// instead of timing out and leaving the defect's offer standing. It runs
    /// BEFORE the end marker on that thread, so the fallback — the road that
    /// matters exactly when the queue is slow — is not stuck behind an
    /// unbounded wait. Both roads write the same idempotent mark.
    pub fn mark_peer_session_disowned_blocking(
        &self,
        session_id: &str,
        expected: &str,
    ) -> Result<(), JournalError> {
        self.rpc_until_stopped(|reply| JournalCmd::MarkPeerSessionDisowned {
            session_id: session_id.to_string(),
            expected: expected.to_string(),
            reply,
        })
    }

    /// The successful resume's clear, issued from the dispatch thread beside
    /// the health recording: the provider honoured `handle`, so a refusal
    /// recorded against it is stale and the offer returns on its own when
    /// the session ends.
    pub fn clear_peer_session_disown(
        &self,
        session_id: &str,
        handle: &str,
    ) -> Result<(), JournalError> {
        self.rpc(|reply| JournalCmd::ClearPeerSessionDisown {
            session_id: session_id.to_string(),
            handle: handle.to_string(),
            reply,
        })
    }

    /// A `devboule_set_agent_profile` move's recording, and nothing else: the
    /// child's `profile_id` column and the `unattended` ratchet.
    ///
    /// Two columns, two rules, both the row's own:
    ///
    /// - `profile_id` is `COALESCE(?, profile_id)` — a move that records a
    ///   profile overwrites, and one that must record **no** profile change
    ///   (the partial failure: the mode landed, the model ask was refused)
    ///   passes `None` and the column stays.
    /// - Both `unattended` columns move only upward — `MAX`, the same
    ///   never-downward ratchet `upsert_session` enforces at `unattended = MAX
    ///   (sessions.unattended, excluded.unattended)` and its tri-state twin. A
    ///   child that was able to run unattended keeps the marker whatever it is
    ///   moved onto later; that fact cannot be un-lived.
    ///
    /// This is control traffic and waits, like `record_permission`: the tool's
    /// answer means the row says what the move did. A journal that cannot take
    /// the write degrades the recording; the asks that already landed cannot
    /// be undone by refusing to write them down.
    ///
    /// A targeted UPDATE rather than an upsert of a rebuilt record: the
    /// full-row upsert overwrites `status`, `generation`, `last_seq` and
    /// `created_at_ms` from the record, and a caller holding only wire
    /// metadata would clobber journal-internal facts with defaults.
    pub fn set_agent_profile_row(
        &self,
        session_id: &str,
        profile_id: Option<&str>,
        unattended: UnattendedState,
    ) -> Result<(), JournalError> {
        self.rpc(|reply| JournalCmd::SetAgentProfile {
            session_id: session_id.to_string(),
            profile_id: profile_id.map(str::to_string),
            unattended,
            reply,
        })
    }

    /// Begin a fresh provider process generation while retaining the same
    /// Devboule session id and all earlier generations' events.
    pub fn start_generation(&self, session_id: &str, generation: u64) -> Result<(), JournalError> {
        self.rpc(|reply| JournalCmd::StartGeneration {
            session_id: session_id.to_string(),
            generation,
            reply,
        })
    }

    pub fn list(&self) -> Result<Vec<SessionRecord>, JournalError> {
        self.rpc(|reply| JournalCmd::List { reply })
    }

    /// One session row that belongs to `owner` and was created by
    /// `created_by`, by id alone — the status fallback's stored read
    /// (`devboule_get_agent_status`). One row, filtered in SQL: never the
    /// roster's shape, never a scan.
    pub fn owned_child_record(
        &self,
        session_id: &str,
        owner: &str,
        created_by: &str,
    ) -> Result<Option<SessionRecord>, JournalError> {
        self.rpc(|reply| JournalCmd::OwnedChildRecord {
            session_id: session_id.to_string(),
            owner: owner.to_string(),
            created_by: created_by.to_string(),
            reply,
        })
    }

    pub fn projects_list(&self) -> Result<Vec<ProjectRecord>, JournalError> {
        self.rpc(|reply| JournalCmd::ProjectsList { reply })
    }

    /// Insert a project once per canonical path. Re-registering the same path
    /// returns its existing stable record and never creates a second id.
    pub fn project_add(&self, record: ProjectRecord) -> Result<ProjectRecord, JournalError> {
        self.rpc(|reply| JournalCmd::ProjectAdd { record, reply })
    }

    pub fn project_get(&self, id: &str) -> Result<Option<ProjectRecord>, JournalError> {
        self.rpc(|reply| JournalCmd::ProjectGet {
            id: id.to_string(),
            reply,
        })
    }

    pub fn workspaces_list(&self, project_id: &str) -> Result<Vec<WorkspaceRecord>, JournalError> {
        self.rpc(|reply| JournalCmd::WorkspacesList {
            project_id: project_id.to_string(),
            reply,
        })
    }

    pub fn workspace_create(
        &self,
        record: WorkspaceRecord,
    ) -> Result<WorkspaceRecord, JournalError> {
        self.rpc(|reply| JournalCmd::WorkspaceCreate { record, reply })
    }

    pub fn workspace_get(&self, id: &str) -> Result<Option<WorkspaceRecord>, JournalError> {
        self.rpc(|reply| JournalCmd::WorkspaceGet {
            id: id.to_string(),
            reply,
        })
    }

    pub fn workspace_delete(&self, id: &str) -> Result<(), JournalError> {
        self.rpc(|reply| JournalCmd::WorkspaceDelete {
            id: id.to_string(),
            reply,
        })
    }

    pub(crate) fn workspace_get_for_session(
        &self,
        id: &str,
    ) -> Result<Option<WorkspaceRecord>, JournalError> {
        self.rpc_with_wait(
            |reply| JournalCmd::WorkspaceGet {
                id: id.to_string(),
                reply,
            },
            WORKSPACE_LOOKUP_WAIT,
        )
    }

    /// Every peer row, revoked ones included: the audit trail outlives the
    /// pairing, and callers filter on [`PeerRecord::is_revoked`] for the
    /// live set.
    pub fn peers_list(&self) -> Result<Vec<PeerRecord>, JournalError> {
        self.rpc(|reply| JournalCmd::PeersList { reply })
    }

    pub fn peer_upsert(&self, record: PeerRecord) -> Result<PeerRecord, JournalError> {
        self.rpc(|reply| JournalCmd::PeerUpsert { record, reply })
    }

    pub fn peer_get(&self, device_id: &str) -> Result<Option<PeerRecord>, JournalError> {
        self.rpc(|reply| JournalCmd::PeerGet {
            device_id: device_id.to_string(),
            reply,
        })
    }

    /// Mark a peer revoked. `Ok(false)` means there was nothing to revoke
    /// (unknown device, or already revoked).
    /// Mark a peer revoked. The outcome distinguishes a row that was revoked
    /// from one that was already revoked and from one that does not exist (C9).
    pub fn peer_revoke(&self, device_id: &str, at: i64) -> Result<PeerMutation, JournalError> {
        self.rpc(|reply| JournalCmd::PeerRevoke {
            device_id: device_id.to_string(),
            at,
            reply,
        })
    }

    /// Replace a peer's capability set. A revoked row is refused (C8).
    pub fn peer_set_caps(
        &self,
        device_id: &str,
        caps: Vec<String>,
    ) -> Result<PeerMutation, JournalError> {
        self.rpc(|reply| JournalCmd::PeerSetCaps {
            device_id: device_id.to_string(),
            caps,
            reply,
        })
    }

    /// Append one audit row. The timestamp is stamped by the writer, not the
    /// caller, so a bug upstream cannot backdate the trail.
    pub fn audit_append(&self, record: AuditRecord) -> Result<(), JournalError> {
        self.rpc(|reply| JournalCmd::AuditAppend {
            record,
            at: None,
            reply,
        })
    }

    /// Enforce the audit age floor and the per-device row cap. This is the
    /// only operation allowed to drop the append-only triggers, and it does
    /// so inside one transaction.
    pub fn audit_sweep(&self) -> Result<AuditSweep, JournalError> {
        self.rpc(|reply| JournalCmd::AuditSweep { reply })
    }

    /// Test-only: append with an explicit timestamp so a sweep test can build
    /// aged rows. Production callers always use [`Journal::audit_append`].
    #[cfg(test)]
    pub fn audit_append_at(&self, record: AuditRecord, at: i64) -> Result<(), JournalError> {
        self.rpc(|reply| JournalCmd::AuditAppend {
            record,
            at: Some(at),
            reply,
        })
    }

    /// The whole transcript, all generations, in (generation, seq) order.
    /// The read is unpositioned by design: the store holds everything, and
    /// what a reader is owed is decided later, by the pull.
    pub fn replay(&self, session_id: &str) -> Result<Replay, JournalError> {
        self.rpc(|reply| JournalCmd::Replay {
            session_id: session_id.to_string(),
            reply,
        })
    }

    pub(crate) fn replay_agent_page(
        &self,
        session_id: &str,
        generation: u64,
        from_generation: u64,
        from_seq: u64,
        through_seq: u64,
        limit: usize,
    ) -> Result<AgentReplayPage, JournalError> {
        self.rpc(|reply| JournalCmd::ReplayAgentPage {
            session_id: session_id.to_string(),
            generation,
            from_generation,
            from_seq,
            through_seq,
            limit,
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

    /// Stop the writer thread and wait until it is gone. The join is the
    /// barrier: on return the writer thread — and with it the SQLite
    /// connection — no longer exists, so the database directory can be
    /// removed immediately.
    pub fn shutdown(&self) {
        // `Shutdown` bypasses the data-queue cap (see `send_shutdown`): a
        // send error means the writer is already gone, which already
        // satisfies the barrier, so there is nothing to report — the join
        // below reaps it either way.
        let _ = self.send_shutdown();
        let handle = match self.join.lock() {
            Ok(mut guard) => guard.take(),
            Err(error) => {
                eprintln!("journal shutdown could not take the writer handle: {error}");
                return;
            }
        };
        // Already shut down: the barrier was met by the earlier call.
        let Some(handle) = handle else {
            return;
        };
        // Unconditional: a budgeted poll that gives up without joining
        // leaves the thread alive with the connection open (os error 32
        // on Windows). Slow drains stay visible instead: report every
        // two seconds while waiting, then join no matter what. A writer
        // panic is reported, never swallowed.
        let start = Instant::now();
        let mut next_report_at_secs = 2u64;
        while !handle.is_finished() {
            std::thread::sleep(Duration::from_millis(50));
            if start.elapsed().as_secs() >= next_report_at_secs {
                eprintln!("journal shutdown still waiting for the writer to drain the backlog");
                next_report_at_secs += 2;
            }
        }
        if let Err(error) = handle.join() {
            eprintln!("journal writer thread panicked during shutdown: {error:?}");
        }
    }

    /// Enqueue `Shutdown` past the data-queue cap. Data producers are
    /// bounded by `reserve_slot` so a flooded queue degrades instead of
    /// growing; a control command must not share that fate. `shutdown`
    /// used to reserve a data slot with a 200 ms budget and discard the
    /// failure, so a saturated queue meant the command never entered, the
    /// writer never exited, and `shutdown` returned with the SQLite
    /// connection still open. The slot counter is still incremented (the
    /// loop decrements every received command, saturating), only the cap
    /// check is skipped. Blocking `send` fails solely on disconnect, i.e.
    /// the writer is already gone.
    fn send_shutdown(&self) -> Result<(), JournalError> {
        self.queued.fetch_add(1, Ordering::AcqRel);
        match self.tx.send(JournalCmd::Shutdown) {
            Ok(()) => Ok(()),
            Err(_) => {
                self.release_slot();
                Err(JournalError::Stopped)
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

    /// The birth door: the one way a session row is created. An id the
    /// journal already holds is refused instead of merged, and the answer
    /// reaches the caller — a create that cannot own its id fails loudly
    /// here, before anything spawns against it.
    ///
    /// One round trip, not two: the writer checkpoints inline (best effort),
    /// so a checkpoint stall never turns a landed row into a reported
    /// failure. A birth that times out leaves no row — the writer deletes
    /// the INSERT when the reply is abandoned — so the answer and the
    /// journal never disagree.
    pub fn create_session(&self, record: SessionRecord) -> Result<(), JournalError> {
        self.rpc(|reply| JournalCmd::CreateSession { record, reply })
    }

    fn send_cmd(&self, cmd: JournalCmd, wait: Duration) -> Result<(), JournalError> {
        self.send_cmd_until(cmd, Instant::now() + wait)
    }

    fn send_cmd_until(&self, cmd: JournalCmd, deadline: Instant) -> Result<(), JournalError> {
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
        Err(JournalError::Timeout)
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
        self.rpc_with_wait(make, RPC_WAIT)
    }

    fn rpc_with_wait<T>(
        &self,
        make: impl FnOnce(mpsc::Sender<Result<T, JournalError>>) -> JournalCmd,
        wait: Duration,
    ) -> Result<T, JournalError> {
        let (tx, rx) = mpsc::channel();
        let deadline = Instant::now() + wait;
        self.send_cmd_until(make(tx), deadline)?;
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(result) => result,
            Err(RecvTimeoutError::Timeout) => Err(JournalError::Timeout),
            Err(RecvTimeoutError::Disconnected) => Err(JournalError::Stopped),
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
    loop_state: JournalLoopState,
    limits: JournalLimits,
    path: PathBuf,
) {
    let JournalLoopState {
        degraded_sessions,
        stats,
        session_set_revision,
    } = loop_state;
    let mut pins: HashSet<String> = HashSet::new();
    let mut retention_state = RetentionState::default();
    let mut last_audit_sweep = Instant::now();
    while let Ok(cmd) = rx.recv() {
        queued
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |value| {
                Some(value.saturating_sub(1))
            })
            .ok();
        if last_audit_sweep.elapsed() >= AUDIT_SWEEP_INTERVAL {
            last_audit_sweep = Instant::now();
            if let Err(error) = sweep_audit(&conn, now_ms() as i64) {
                on_write_error(&error);
            }
        }
        match cmd {
            JournalCmd::Upsert(record) => {
                if let Err(error) = upsert_session(&conn, &record) {
                    note_degraded(&degraded_sessions, &record.id, DropCounters::default());
                    on_write_error(&error);
                } else {
                    retention_state.session_set_changed();
                    session_set_revision.fetch_add(1, Ordering::AcqRel);
                }
            }
            JournalCmd::CreateSession { record, reply } => {
                let result = create_session_row(&conn, &record);
                let inserted = result.is_ok();
                if inserted {
                    // The bump lives here, where success is known: a refusal
                    // changes no roster, and the call-site invalidate is
                    // per-registry while this revision is the global signal.
                    retention_state.session_set_changed();
                    session_set_revision.fetch_add(1, Ordering::AcqRel);
                    // Durability without a second round trip: the row is
                    // committed in WAL already, so a checkpoint stall is
                    // logged, never returned as a birth failure.
                    if let Err(error) = conn
                        .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
                        .map_err(JournalError::from)
                    {
                        on_write_error(&error);
                    }
                }
                if reply.send(result).is_err() && inserted {
                    // The caller timed out and is gone: remove the row it
                    // was told it never got, so a failed birth leaves
                    // nothing behind and frees the id.
                    let _ = conn.execute("DELETE FROM events WHERE session_id = ?1", [&record.id]);
                    let _ = conn.execute("DELETE FROM sessions WHERE id = ?1", [&record.id]);
                    retention_state.session_set_changed();
                    session_set_revision.fetch_add(1, Ordering::AcqRel);
                }
            }
            JournalCmd::Append(record) => {
                let is_output = matches!(record.kind, EventKind::Output | EventKind::AcpEnvelope);
                let payload_len = record.payload.len() as u64;
                match append_event(&conn, &record, &pins, limits, &mut retention_state) {
                    Ok(roster_changed) => {
                        if roster_changed {
                            session_set_revision.fetch_add(1, Ordering::AcqRel);
                        }
                        if is_output {
                            stats.committed_frames.fetch_add(1, Ordering::Relaxed);
                            stats
                                .committed_bytes
                                .fetch_add(payload_len, Ordering::Relaxed);
                        }
                    }
                    Err(error) => {
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
                    }
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
            JournalCmd::PermissionWasRecorded { request_id, reply } => {
                let count: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM permissions WHERE request_id = ?1",
                        params![&request_id],
                        |row| row.get(0),
                    )
                    .unwrap_or(0);
                let _ = reply.send(Ok(count > 0));
            }
            JournalCmd::PermissionWasRecordedIn {
                session_id,
                request_id,
                reply,
            } => {
                let count: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM permissions
                             WHERE session_id = ?1 AND request_id = ?2",
                        params![&session_id, &request_id],
                        |row| row.get(0),
                    )
                    .unwrap_or(0);
                let _ = reply.send(Ok(count > 0));
            }
            JournalCmd::PermissionCount { session_id, reply } => {
                let count: i64 = conn
                    .query_row(
                        "SELECT COUNT(*) FROM permissions WHERE session_id = ?1",
                        params![&session_id],
                        |row| row.get(0),
                    )
                    .unwrap_or(0);
                let _ = reply.send(Ok(count.clamp(0, u32::MAX as i64) as u32));
            }
            JournalCmd::MarkReaped { session_id, code } => {
                let (degraded, dropped) = degradation_state(&degraded_sessions, &session_id);
                if let Err(error) = mark_reaped(&conn, &session_id, code, degraded, dropped) {
                    note_degraded(&degraded_sessions, &session_id, DropCounters::default());
                    on_write_error(&error);
                } else {
                    session_set_revision.fetch_add(1, Ordering::AcqRel);
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
                    session_set_revision.fetch_add(1, Ordering::AcqRel);
                }
            }
            JournalCmd::MarkClosed { session_id } => {
                if let Err(error) = mark_closed(&conn, &session_id) {
                    note_degraded(&degraded_sessions, &session_id, DropCounters::default());
                    on_write_error(&error);
                } else {
                    retention_state.session_set_changed();
                    session_set_revision.fetch_add(1, Ordering::AcqRel);
                }
            }
            JournalCmd::SetPeerSessionId {
                session_id,
                peer_session_id,
                reply,
            } => {
                let result = set_peer_session_id(&conn, &session_id, &peer_session_id);
                match &result {
                    Err(error) => on_write_error(error),
                    Ok(true) => {
                        // The handle and its recorded refusal are the roster's
                        // resume verdict: a changed handle, or a refusal
                        // cleared by an announce of a different handle, must
                        // reach the next reader — the same global signal as
                        // every other roster-visible row write (the cached
                        // roster is keyed by this revision). An unchanged
                        // write — the announce-time writers re-persist the
                        // same id on every frame — changed nothing the roster
                        // renders, so it costs no rebuild.
                        session_set_revision.fetch_add(1, Ordering::AcqRel);
                    }
                    Ok(false) => {}
                }
                let _ = reply.send(result.map(|_| ()));
            }
            JournalCmd::MarkPeerSessionDisowned {
                session_id,
                expected,
                reply,
            } => {
                let result = mark_peer_session_disowned(&conn, &session_id, &expected);
                if let Err(error) = &result {
                    on_write_error(error);
                }
                if matches!(&result, Ok(true)) {
                    // Same rule as the set: the revision moves only when the
                    // roster's verdict moved. A mark that matched nothing —
                    // the handle already moved on, or the row is gone —
                    // already holds its postcondition.
                    session_set_revision.fetch_add(1, Ordering::AcqRel);
                }
                let _ = reply.send(result.map(|_| ()));
            }
            JournalCmd::ClearPeerSessionDisown {
                session_id,
                handle,
                reply,
            } => {
                let result = clear_peer_session_disown(&conn, &session_id, &handle);
                if let Err(error) = &result {
                    on_write_error(error);
                }
                if matches!(&result, Ok(true)) {
                    // The verdict moved: a marked row just became unmarked.
                    session_set_revision.fetch_add(1, Ordering::AcqRel);
                }
                let _ = reply.send(result.map(|_| ()));
            }
            JournalCmd::SetAgentProfile {
                session_id,
                profile_id,
                unattended,
                reply,
            } => {
                let result =
                    set_agent_profile_row(&conn, &session_id, profile_id.as_deref(), unattended);
                if let Err(error) = &result {
                    on_write_error(error);
                } else {
                    session_set_revision.fetch_add(1, Ordering::AcqRel);
                }
                let _ = reply.send(result);
            }
            JournalCmd::StartGeneration {
                session_id,
                generation,
                reply,
            } => {
                let result = start_generation(&conn, &session_id, generation);
                if let Err(error) = &result {
                    on_write_error(error);
                } else {
                    session_set_revision.fetch_add(1, Ordering::AcqRel);
                }
                let _ = reply.send(result);
            }
            JournalCmd::MarkDegraded { session_id } => {
                let (degraded, dropped) = degradation_state(&degraded_sessions, &session_id);
                if let Err(error) = mark_degraded(&conn, &session_id, degraded, dropped) {
                    on_write_error(&error);
                } else {
                    session_set_revision.fetch_add(1, Ordering::AcqRel);
                }
            }
            JournalCmd::List { reply } => {
                let _ = reply.send(list_sessions(&conn));
            }
            JournalCmd::OwnedChildRecord {
                session_id,
                owner,
                created_by,
                reply,
            } => {
                let _ = reply.send(owned_child_record(&conn, &session_id, &owner, &created_by));
            }
            JournalCmd::ProjectsList { reply } => {
                let _ = reply.send(list_projects(&conn));
            }
            JournalCmd::ProjectAdd { record, reply } => {
                let result = add_project(&conn, &record);
                if let Err(error) = &result {
                    on_write_error(error);
                }
                let _ = reply.send(result);
            }
            JournalCmd::ProjectGet { id, reply } => {
                let _ = reply.send(get_project(&conn, &id));
            }
            JournalCmd::WorkspacesList { project_id, reply } => {
                let _ = reply.send(list_workspaces(&conn, &project_id));
            }
            JournalCmd::WorkspaceCreate { record, reply } => {
                let result = add_workspace(&conn, &record);
                if let Err(error) = &result {
                    on_write_error(error);
                }
                let _ = reply.send(result);
            }
            JournalCmd::WorkspaceGet { id, reply } => {
                let _ = reply.send(get_workspace(&conn, &id));
            }
            JournalCmd::WorkspaceDelete { id, reply } => {
                let result = delete_workspace(&conn, &id);
                if let Err(error) = &result {
                    on_write_error(error);
                }
                let _ = reply.send(result);
            }
            JournalCmd::Replay { session_id, reply } => {
                let _ = reply.send(replay_session(&conn, &session_id));
            }
            JournalCmd::ReplayAgentPage {
                session_id,
                generation,
                from_generation,
                from_seq,
                through_seq,
                limit,
                reply,
            } => {
                let _ = reply.send(replay_agent_page(
                    &conn,
                    &session_id,
                    generation,
                    from_generation,
                    from_seq,
                    through_seq,
                    limit,
                ));
            }
            JournalCmd::DeleteSession { session_id, reply } => {
                let result = delete_session_user(&conn, &session_id);
                if result.is_ok() {
                    retention_state.session_set_changed();
                    session_set_revision.fetch_add(1, Ordering::AcqRel);
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
            JournalCmd::PeersList { reply } => {
                let _ = reply.send(list_peers(&conn));
            }
            JournalCmd::PeerUpsert { record, reply } => {
                let result = upsert_peer(&conn, &record);
                if let Err(error) = &result {
                    on_write_error(error);
                }
                let _ = reply.send(result);
            }
            JournalCmd::PeerGet { device_id, reply } => {
                let _ = reply.send(get_peer(&conn, &device_id));
            }
            JournalCmd::PeerRevoke {
                device_id,
                at,
                reply,
            } => {
                let result = revoke_peer(&conn, &device_id, at);
                if let Err(error) = &result {
                    on_write_error(error);
                }
                let _ = reply.send(result);
            }
            JournalCmd::PeerSetCaps {
                device_id,
                caps,
                reply,
            } => {
                let result = set_peer_caps(&conn, &device_id, &caps);
                if let Err(error) = &result {
                    on_write_error(error);
                }
                let _ = reply.send(result);
            }
            JournalCmd::AuditAppend { record, at, reply } => {
                let result = append_audit(&conn, &record, at.unwrap_or_else(|| now_ms() as i64));
                if let Err(error) = &result {
                    on_write_error(error);
                }
                let _ = reply.send(result);
            }
            JournalCmd::AuditSweep { reply } => {
                let result = sweep_audit(&conn, now_ms() as i64).map(|(aged, capped)| AuditSweep {
                    deleted_by_age: aged,
                    deleted_by_cap: capped,
                });
                if let Err(error) = &result {
                    on_write_error(error);
                }
                let _ = reply.send(result);
            }
            JournalCmd::Flush { reply } => {
                let result = conn
                    .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
                    .map(|_| ())
                    .map_err(JournalError::from);
                let _ = reply.send(result);
            }
            JournalCmd::FileLen { reply } => {
                let result = journal_disk_footprint(&path)
                    .map_err(|error| JournalError::Unavailable(error.to_string()));
                let _ = reply.send(result);
            }
            JournalCmd::Shutdown => break,
        }
    }
}

fn journal_disk_footprint(path: &Path) -> std::io::Result<u64> {
    let main_bytes = std::fs::metadata(path)?.len();
    let mut wal_name = path.as_os_str().to_os_string();
    wal_name.push("-wal");
    let wal_path = PathBuf::from(wal_name);
    let wal_bytes = match std::fs::metadata(wal_path) {
        Ok(meta) => meta.len(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => return Err(error),
    };

    // SQLite's -shm file is a transient shared-memory index, not journal
    // content. Exclude it so this reports the durable database plus WAL
    // footprint that represents the journal's retained data.
    Ok(main_bytes.saturating_add(wal_bytes))
}

fn list_projects(conn: &Connection) -> Result<Vec<ProjectRecord>, JournalError> {
    let mut stmt = conn.prepare(
        "SELECT id, name, path, git_state, created_at_ms, updated_at_ms
         FROM projects ORDER BY id",
    )?;
    let rows = stmt.query_map([], project_from_row)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(JournalError::from)
}

fn get_project(conn: &Connection, id: &str) -> Result<Option<ProjectRecord>, JournalError> {
    conn.query_row(
        "SELECT id, name, path, git_state, created_at_ms, updated_at_ms
         FROM projects WHERE id = ?1",
        [id],
        project_from_row,
    )
    .optional()
    .map_err(JournalError::from)
}

fn add_project(conn: &Connection, record: &ProjectRecord) -> Result<ProjectRecord, JournalError> {
    let tx = conn.unchecked_transaction()?;
    if let Some(existing) = tx
        .query_row(
            "SELECT id, name, path, git_state, created_at_ms, updated_at_ms
             FROM projects WHERE path = ?1",
            [&record.path],
            project_from_row,
        )
        .optional()?
    {
        // Re-registering is the explicit refresh boundary for project
        // metadata. Listing stays read-only: probing git for every project
        // on every list would spawn one process per row and make a cheap UI
        // read pay that cost. A future dedicated refresh command can reuse
        // this UPDATE without changing the storage contract.
        tx.execute(
            "UPDATE projects
                SET name = ?2, git_state = ?3, updated_at_ms = ?4
              WHERE path = ?1",
            params![
                record.path,
                record.name,
                record.git_state,
                record.updated_at_ms as i64,
            ],
        )?;
        let refreshed = ProjectRecord {
            id: existing.id,
            name: record.name.clone(),
            path: existing.path,
            git_state: record.git_state.clone(),
            created_at_ms: existing.created_at_ms,
            updated_at_ms: record.updated_at_ms,
        };
        tx.commit()?;
        return Ok(refreshed);
    }
    tx.execute(
        "INSERT INTO projects (
            id, name, path, git_state, created_at_ms, updated_at_ms
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            record.id,
            record.name,
            record.path,
            record.git_state,
            record.created_at_ms as i64,
            record.updated_at_ms as i64,
        ],
    )?;
    tx.commit()?;
    Ok(record.clone())
}

fn list_workspaces(
    conn: &Connection,
    project_id: &str,
) -> Result<Vec<WorkspaceRecord>, JournalError> {
    if get_project(conn, project_id)?.is_none() {
        return Err(JournalError::InvalidRequest(format!(
            "Project '{project_id}' does not exist."
        )));
    }
    let mut stmt = conn.prepare(
        "SELECT id, project_id, title, isolation, path, created_at_ms, updated_at_ms, branch
         FROM workspaces WHERE project_id = ?1 ORDER BY id",
    )?;
    let rows = stmt.query_map([project_id], workspace_from_row)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(JournalError::from)
}

fn get_workspace(conn: &Connection, id: &str) -> Result<Option<WorkspaceRecord>, JournalError> {
    conn.query_row(
        "SELECT id, project_id, title, isolation, path, created_at_ms, updated_at_ms, branch
         FROM workspaces WHERE id = ?1",
        [id],
        workspace_from_row,
    )
    .optional()
    .map_err(JournalError::from)
}

fn add_workspace(
    conn: &Connection,
    record: &WorkspaceRecord,
) -> Result<WorkspaceRecord, JournalError> {
    if get_project(conn, &record.project_id)?.is_none() {
        return Err(JournalError::InvalidRequest(format!(
            "Project '{}' does not exist.",
            record.project_id
        )));
    }
    conn.execute(
        "INSERT INTO workspaces (
            id, project_id, title, isolation, path, created_at_ms, updated_at_ms, branch
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            record.id,
            record.project_id,
            record.title,
            isolation_str(record.isolation),
            record.path,
            record.created_at_ms as i64,
            record.updated_at_ms as i64,
            record.branch,
        ],
    )?;
    Ok(record.clone())
}

fn delete_workspace(conn: &Connection, id: &str) -> Result<(), JournalError> {
    let deleted = conn.execute("DELETE FROM workspaces WHERE id = ?1", [id])?;
    if deleted == 0 {
        return Err(JournalError::InvalidRequest(format!(
            "Workspace '{id}' does not exist."
        )));
    }
    Ok(())
}

const PEER_COLUMNS: &str = "device_id, display_name, role, public_key, paired_by_user, \
     binding_kind, binding_stable_id, binding_node_name, binding_login_name, address, \
     paired_at, revoked_at, caps";

fn peers_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<PeerRecord> {
    let caps_json: String = row.get(12)?;
    let caps = serde_json::from_str::<Vec<String>>(&caps_json).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(12, rusqlite::types::Type::Text, Box::new(error))
    })?;
    Ok(PeerRecord {
        device_id: row.get(0)?,
        display_name: row.get(1)?,
        role: row.get(2)?,
        public_key: row.get(3)?,
        paired_by_user: row.get(4)?,
        binding_kind: row.get(5)?,
        binding_stable_id: row.get(6)?,
        binding_node_name: row.get(7)?,
        binding_login_name: row.get(8)?,
        address: row.get(9)?,
        paired_at: row.get(10)?,
        revoked_at: row.get(11)?,
        caps,
    })
}

fn list_peers(conn: &Connection) -> Result<Vec<PeerRecord>, JournalError> {
    let mut statement = conn.prepare(&format!(
        "SELECT {PEER_COLUMNS} FROM peers ORDER BY device_id"
    ))?;
    let rows = statement.query_map([], peers_from_row)?;
    rows.collect::<Result<Vec<_>, _>>()
        .map_err(JournalError::from)
}

fn get_peer(conn: &Connection, device_id: &str) -> Result<Option<PeerRecord>, JournalError> {
    conn.query_row(
        &format!("SELECT {PEER_COLUMNS} FROM peers WHERE device_id = ?1"),
        [device_id],
        peers_from_row,
    )
    .optional()
    .map_err(JournalError::from)
}

fn upsert_peer(conn: &Connection, record: &PeerRecord) -> Result<PeerRecord, JournalError> {
    if record.role != "client" && record.role != "daemon" {
        return Err(JournalError::InvalidRequest(format!(
            "peer role {:?} is not client or daemon",
            record.role
        )));
    }
    if record.public_key.len() != 32 {
        return Err(JournalError::InvalidRequest(format!(
            "peer public key is {} bytes, expected 32",
            record.public_key.len()
        )));
    }
    let caps = serde_json::to_string(&record.caps)
        .map_err(|error| JournalError::InvalidRequest(error.to_string()))?;
    conn.execute(
        "INSERT INTO peers (
                device_id, display_name, role, public_key, paired_by_user,
                binding_kind, binding_stable_id, binding_node_name, binding_login_name,
                address, paired_at, revoked_at, caps
             ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)
             ON CONFLICT(device_id) DO UPDATE SET
                display_name = excluded.display_name,
                role = excluded.role,
                public_key = excluded.public_key,
                paired_by_user = excluded.paired_by_user,
                binding_kind = excluded.binding_kind,
                binding_stable_id = excluded.binding_stable_id,
                binding_node_name = excluded.binding_node_name,
                binding_login_name = excluded.binding_login_name,
                address = excluded.address,
                paired_at = excluded.paired_at,
                revoked_at = excluded.revoked_at,
                caps = excluded.caps",
        params![
            record.device_id,
            record.display_name,
            record.role,
            record.public_key,
            record.paired_by_user,
            record.binding_kind,
            record.binding_stable_id,
            record.binding_node_name,
            record.binding_login_name,
            record.address,
            record.paired_at,
            record.revoked_at,
            caps,
        ],
    )?;
    get_peer(conn, &record.device_id)?.ok_or_else(|| {
        JournalError::Unavailable("peer row vanished immediately after upsert".to_string())
    })
}

/// What one peer mutation did.
///
/// `revoke_peer` and `set_peer_caps` both used to answer a bare `bool`, which
/// made "there is no such row" and "the row is already revoked" the same answer
/// — and the dispatch site rendered both as "No such peer", which is a lie in
/// the second case (C9). The caller can now say which happened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeerMutation {
    /// The row was changed.
    Updated,
    /// The row exists and is revoked, so it was left alone.
    Revoked,
    /// No row with that device id.
    NotFound,
}

/// Whether a row exists, so a mutation that changed nothing can say which kind
/// of nothing it was.
fn peer_row_exists(conn: &Connection, device_id: &str) -> Result<bool, JournalError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM peers WHERE device_id = ?1",
        [device_id],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

fn revoke_peer(conn: &Connection, device_id: &str, at: i64) -> Result<PeerMutation, JournalError> {
    let updated = conn.execute(
        "UPDATE peers SET revoked_at = ?2 WHERE device_id = ?1 AND revoked_at IS NULL",
        params![device_id, at],
    )?;
    if updated > 0 {
        return Ok(PeerMutation::Updated);
    }
    Ok(if peer_row_exists(conn, device_id)? {
        PeerMutation::Revoked
    } else {
        PeerMutation::NotFound
    })
}

fn set_peer_caps(
    conn: &Connection,
    device_id: &str,
    caps: &[String],
) -> Result<PeerMutation, JournalError> {
    let caps = serde_json::to_string(caps)
        .map_err(|error| JournalError::InvalidRequest(error.to_string()))?;
    // The revoked filter is the point (C8): a revoked row is a device this
    // daemon no longer trusts, and rewriting its capabilities would leave the
    // stored state at odds with the panel's "Revoked" story — and would silently
    // revive the old capability set if the row is ever re-paired without a
    // fresh `caps` value.
    let updated = conn.execute(
        "UPDATE peers SET caps = ?2 WHERE device_id = ?1 AND revoked_at IS NULL",
        params![device_id, caps],
    )?;
    if updated > 0 {
        return Ok(PeerMutation::Updated);
    }
    Ok(if peer_row_exists(conn, device_id)? {
        PeerMutation::Revoked
    } else {
        PeerMutation::NotFound
    })
}

fn append_audit(conn: &Connection, record: &AuditRecord, at: i64) -> Result<(), JournalError> {
    conn.execute(
        "INSERT INTO audit (at, device_id, role, claimed_origin, action, session_id, outcome)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            at,
            record.device_id,
            record.role,
            record.claimed_origin,
            record.action,
            record.session_id,
            record.outcome,
        ],
    )?;
    Ok(())
}

fn project_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProjectRecord> {
    Ok(ProjectRecord {
        id: row.get(0)?,
        name: row.get(1)?,
        path: row.get(2)?,
        git_state: row.get(3)?,
        created_at_ms: row.get::<_, i64>(4)? as u64,
        updated_at_ms: row.get::<_, i64>(5)? as u64,
    })
}

fn workspace_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<WorkspaceRecord> {
    let isolation = match row.get::<_, String>(3)?.as_str() {
        "local" => WorkspaceIsolation::Local,
        "worktree" => WorkspaceIsolation::Worktree,
        _ => {
            return Err(rusqlite::Error::FromSqlConversionFailure(
                3,
                rusqlite::types::Type::Text,
                "unknown workspace isolation".into(),
            ))
        }
    };
    Ok(WorkspaceRecord {
        id: row.get(0)?,
        project_id: row.get(1)?,
        title: row.get(2)?,
        isolation,
        path: row.get(4)?,
        created_at_ms: row.get::<_, i64>(5)? as u64,
        updated_at_ms: row.get::<_, i64>(6)? as u64,
        branch: row.get(7)?,
    })
}

fn isolation_str(isolation: WorkspaceIsolation) -> &'static str {
    match isolation {
        WorkspaceIsolation::Local => "local",
        WorkspaceIsolation::Worktree => "worktree",
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

/// The session row's insert — 34 columns, 33 bindings plus the literal `0`
/// for `unsnapshotted_bytes` — shared by the birth insert and the
/// update-or-insert upsert, so neither can grow a column the other does not
/// write.
const SESSION_INSERT: &str = "INSERT INTO sessions (
    id, owner, workspace_id, kind, title, created_at_ms, updated_at_ms,
    generation, status, exit_code, closed, last_seq, degraded,
    dropped_frames, dropped_bytes, trimmed_bytes, payload_bytes, unsnapshotted_bytes,
    reaped, peer_session_id, provider, origin_kind, origin_device, origin_role,
    display_name, created_by, profile_id, context_id, unattended, unattended_state, labels,
    overlay, depth, disowned_peer_session_id, cwd
) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, 0, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25, ?26, ?27, ?28, ?29, ?30, ?31, ?32, ?33, ?34)";

/// The upsert's conflict clause: an existing id is *updated*, with the
/// never-downward ratchets and the birth-fact protections below.
const SESSION_UPSERT_CLAUSE: &str = "
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
        reaped = MAX(sessions.reaped, excluded.reaped),
        peer_session_id = COALESCE(excluded.peer_session_id, sessions.peer_session_id),
        provider = COALESCE(excluded.provider, sessions.provider),
        origin_kind = COALESCE(excluded.origin_kind, sessions.origin_kind),
        origin_device = COALESCE(excluded.origin_device, sessions.origin_device),
        origin_role = COALESCE(excluded.origin_role, sessions.origin_role),
        display_name = COALESCE(excluded.display_name, sessions.display_name),
        created_by = COALESCE(excluded.created_by, sessions.created_by),
        profile_id = COALESCE(excluded.profile_id, sessions.profile_id),
        context_id = COALESCE(excluded.context_id, sessions.context_id),
        -- Unattended is a fact of the birth and only ever goes one way: a
        -- later write that says `0` (a resume rebuilt from a row that
        -- predates the marker, an ordinary end) must not erase what the
        -- creation recorded.
        unattended = MAX(sessions.unattended, excluded.unattended),
        -- The tri-state the marker actually travels in ratchets under the
        -- same never-downward rule, ordered `no < unknown < yes`: a row
        -- may move up that order and never down, because the asymmetry
        -- says a session that ran alone and does not show is worse than
        -- one that shows and did not need to.
        unattended_state = MAX(sessions.unattended_state, excluded.unattended_state),
        -- Same rule: labels are written once, at the creation. A later
        -- upsert with an empty map (the common one, every end marker)
        -- must not erase them.
        labels = COALESCE(NULLIF(excluded.labels, '{}'), sessions.labels),
        -- Same rule again: the overlay and the depth are birth facts. Later
        -- upserts carry NULL (their records never re-derive them), so the
        -- birth values stay.
        overlay = COALESCE(excluded.overlay, sessions.overlay),
        depth = COALESCE(excluded.depth, sessions.depth),
        -- The disown mark is the daemon's own fact about the provider's
        -- answer. Later upserts never carry it (their records are wire
        -- metadata), so the recorded refusal stays until the announce road
        -- clears it.
        disowned_peer_session_id = COALESCE(excluded.disowned_peer_session_id, sessions.disowned_peer_session_id),
        -- The directory is a birth fact like the overlay above: it is written
        -- once, by the row's own creation, and a later upsert carries NULL
        -- because the records it builds are wire metadata with no directory
        -- of their own. Without the ratchet an end marker would erase the one
        -- record of where the session worked.
        cwd = COALESCE(excluded.cwd, sessions.cwd)";

fn upsert_session(conn: &Connection, record: &SessionRecord) -> Result<(), JournalError> {
    write_session_row(conn, record, true)
}

/// The birth door's write: a plain INSERT, so an id the journal already
/// holds is a primary-key refusal, never a merge. The create road is the
/// only caller; every other session write updates a row that exists.
fn create_session_row(conn: &Connection, record: &SessionRecord) -> Result<(), JournalError> {
    write_session_row(conn, record, false)
}

fn write_session_row(
    conn: &Connection,
    record: &SessionRecord,
    upsert: bool,
) -> Result<(), JournalError> {
    let labels = labels_json(&record.labels);
    let overlay = overlay_json(&record.overlay)?;
    let sql = if upsert {
        format!("{SESSION_INSERT}{SESSION_UPSERT_CLAUSE}")
    } else {
        SESSION_INSERT.to_string()
    };
    conn.execute(
        &sql,
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
            record.peer_session_id,
            record.provider,
            origin_kind_str(&record.origin),
            record.origin.device_id,
            record.origin.role.map(|role| role.as_str().to_string()),
            record.display_name,
            record.created_by,
            record.profile_id,
            record.context_id,
            if record.unattended_state == UnattendedState::Yes {
                1
            } else {
                0
            },
            unattended_state_rank(record.unattended_state),
            labels,
            overlay,
            record.depth.map(|depth| depth as i64),
            record.disowned_peer_session_id,
            record.cwd,
        ],
    )
    .map_err(|error| {
        if session_id_taken(&error) {
            JournalError::SessionExists {
                id: record.id.clone(),
            }
        } else {
            error.into()
        }
    })?;
    Ok(())
}

/// The one constraint the sessions table puts on `id` is its primary key,
/// so a primary-key violation naming that column is a held id, nothing else.
/// The code leads and the prose follows: prose alone trusts a third-party
/// string, code alone would claim another table's key.
fn session_id_taken(error: &rusqlite::Error) -> bool {
    let rusqlite::Error::SqliteFailure(ffi_error, message) = error else {
        return false;
    };
    if ffi_error.extended_code != rusqlite::ffi::SQLITE_CONSTRAINT_PRIMARYKEY {
        return false;
    }
    message
        .as_deref()
        .is_some_and(|message| message.contains("sessions.id"))
}

/// The tri-state's integer encoding, in the never-downward order
/// `no < unknown < yes`: the SQL `MAX` ratchet compares these, so the order
/// is load-bearing and the numbers are the order. `pub(crate)` so the one
/// in-memory reader that must apply the same ratchet — the live metadata
/// update after a profile move — compares with the same numbers instead of a
/// second copy of the order.
pub(crate) fn unattended_state_rank(state: UnattendedState) -> i64 {
    match state {
        UnattendedState::No => 0,
        UnattendedState::Unknown => 1,
        UnattendedState::Yes => 2,
    }
}

/// The inverse of [`unattended_state_rank`], for reading the column back. A
/// value the daemon does not write — a hand-edited row, a future encoding —
/// reads as `unknown`, the honest default, rather than as a certainty nobody
/// recorded.
fn unattended_state_from_rank(rank: i64) -> UnattendedState {
    match rank {
        0 => UnattendedState::No,
        2 => UnattendedState::Yes,
        _ => UnattendedState::Unknown,
    }
}

/// The labels column: one JSON object, and `{}` for a session that carries none
/// (which is every session a human started).
///
/// A map whose encoding fails is written as `{}` rather than as a half-object:
/// nothing reads a label to decide anything, so the worst case is a display that
/// shows no labels — never a session that cannot be listed.
fn labels_json(labels: &std::collections::BTreeMap<String, String>) -> String {
    serde_json::to_string(labels).unwrap_or_else(|_| "{}".to_string())
}

/// The overlay column: one JSON array of denied tool names, canonicalised
/// (sorted, deduped — the deny check is order-free), and NULL for a session
/// that carries no overlay (which is every session a human started, and
/// every row that predates v13). NULL is the one representation of "no
/// overlay": a birth with no restriction writes the same bytes a pre-v13
/// row already has, so absence keeps meaning absence and no backfill can
/// manufacture a restriction nobody recorded.
///
/// The encoding cannot fail for the names this function is given, and the
/// failure is still loud: a restriction silently unwritten would read back
/// as no restriction, which is the open direction the read side refuses.
fn overlay_json(
    overlay: &Option<crate::provider_catalog::ToolOverlay>,
) -> Result<Option<String>, JournalError> {
    let Some(overlay) = overlay else {
        return Ok(None);
    };
    let mut names = overlay.disabled_names();
    names.sort();
    names.dedup();
    if names.is_empty() {
        return Ok(None);
    }
    serde_json::to_string(&names)
        .map(Some)
        .map_err(|error| JournalError::Corrupt(format!("could not encode tool overlay: {error}")))
}

fn append_event(
    conn: &Connection,
    record: &EventRecord,
    pins: &HashSet<String>,
    limits: JournalLimits,
    retention_state: &mut RetentionState,
) -> Result<bool, JournalError> {
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
    let roster_changed = retain(
        &tx,
        pins,
        now_ms(),
        limits,
        &record.session_id,
        global_sweep,
    )?;
    tx.commit()?;
    retention_state.append_committed(add as u64, global_sweep);
    Ok(roster_changed)
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

/// The row's handle for a future resume/load handshake. Returns whether the
/// stored state changed: the roster revision may only churn when the
/// roster-visible verdict actually moved, and a missing row stays the error
/// it has always been. Announcing a handle that differs from the recorded
/// refusal also clears that refusal — it was about a handle that no longer
/// applies, and it must not silence the new one.
fn set_peer_session_id(
    conn: &Connection,
    session_id: &str,
    peer_session_id: &str,
) -> Result<bool, JournalError> {
    // Row presence and column value are two different None-s: a row whose
    // handle was never set (NULL) is the normal announce-time write, and
    // only a MISSING row is the error it has always been.
    let (current, disowned): (Option<String>, Option<String>) = match conn.query_row(
        "SELECT peer_session_id, disowned_peer_session_id FROM sessions WHERE id = ?1",
        [session_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    ) {
        Ok(value) => value,
        Err(rusqlite::Error::QueryReturnedNoRows) => return Err(JournalError::SessionNotFound),
        Err(error) => return Err(error.into()),
    };
    // `current` is None for a row whose handle was never set (NULL) — that
    // is the normal announce-time write — never for a missing row, which the
    // no-rows arm above already refused.
    let handle_changed = current.as_deref() != Some(peer_session_id);
    let mark_cleared = disowned.is_some() && disowned.as_deref() != Some(peer_session_id);
    if !handle_changed && !mark_cleared {
        return Ok(false);
    }
    let n = conn.execute(
        "UPDATE sessions SET peer_session_id = ?1,
                disowned_peer_session_id = CASE
                    WHEN disowned_peer_session_id IS NOT NULL AND disowned_peer_session_id <> ?1
                    THEN NULL ELSE disowned_peer_session_id END,
                updated_at_ms = ?2
         WHERE id = ?3",
        params![peer_session_id, now_ms() as i64, session_id],
    )?;
    if n == 0 {
        Err(JournalError::SessionNotFound)
    } else {
        Ok(true)
    }
}

/// The disown mark: the provider refused this exact handle, recorded beside
/// it — `peer_session_id` itself is never destroyed, because the evidence
/// for a refusal is approximate and the handle is the only route back to the
/// conversation. Conditional on the row still carrying the refused handle, so
/// a concurrent respawn's newer handle is not retroactively silenced, and
/// idempotent, so the ordered road and its fallback can both write. Zero rows
/// — the handle moved on, or the row is gone — is the postcondition already
/// holding, never an error.
fn mark_peer_session_disowned(
    conn: &Connection,
    session_id: &str,
    expected: &str,
) -> Result<bool, JournalError> {
    let n = conn.execute(
        "UPDATE sessions SET disowned_peer_session_id = ?1, updated_at_ms = ?2
         WHERE id = ?3 AND peer_session_id = ?1
           AND (disowned_peer_session_id IS NULL OR disowned_peer_session_id <> ?1)",
        params![expected, now_ms() as i64, session_id],
    )?;
    Ok(n > 0)
}

/// The success road's clear: the provider honoured this exact handle, so a
/// refusal recorded against it is stale and must stop hiding the offer. A
/// mark about a different handle stays — it is not this resume's to judge.
/// Zero rows (no such mark, or no row) is already the postcondition.
fn clear_peer_session_disown(
    conn: &Connection,
    session_id: &str,
    handle: &str,
) -> Result<bool, JournalError> {
    let n = conn.execute(
        "UPDATE sessions SET disowned_peer_session_id = NULL, updated_at_ms = ?2
         WHERE id = ?3 AND disowned_peer_session_id = ?1",
        params![handle, now_ms() as i64, session_id],
    )?;
    Ok(n > 0)
}

/// The row half of a `devboule_set_agent_profile` move — see
/// [`Journal::set_agent_profile_row`] for the two rules this enforces.
fn set_agent_profile_row(
    conn: &Connection,
    session_id: &str,
    profile_id: Option<&str>,
    unattended: UnattendedState,
) -> Result<(), JournalError> {
    let n = conn.execute(
        "UPDATE sessions SET
            profile_id = COALESCE(?2, profile_id),
            unattended = MAX(unattended, ?3),
            unattended_state = MAX(unattended_state, ?4),
            updated_at_ms = ?5
         WHERE id = ?1",
        params![
            session_id,
            profile_id,
            if unattended == UnattendedState::Yes {
                1
            } else {
                0
            },
            unattended_state_rank(unattended),
            now_ms() as i64,
        ],
    )?;
    if n == 0 {
        Err(JournalError::SessionNotFound)
    } else {
        Ok(())
    }
}

fn start_generation(
    conn: &Connection,
    session_id: &str,
    generation: u64,
) -> Result<(), JournalError> {
    let n = conn.execute(
        "UPDATE sessions SET
             generation = ?1,
             status = 'live',
             exit_code = NULL,
             closed = 0,
             last_seq = 0,
             reaped = 0,
             updated_at_ms = ?2
         WHERE id = ?3",
        params![generation as i64, now_ms() as i64, session_id],
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
        SessionKind::Claude => "claude",
        SessionKind::Pi => "pi",
        SessionKind::Codex => "codex",
    }
}

pub(super) fn parse_kind(value: &str) -> Result<SessionKind, JournalError> {
    match value {
        "terminal" => Ok(SessionKind::Terminal),
        "acp" => Ok(SessionKind::Acp),
        "claude" => Ok(SessionKind::Claude),
        "pi" => Ok(SessionKind::Pi),
        "codex" => Ok(SessionKind::Codex),
        other => Err(JournalError::Corrupt(format!(
            "unknown session kind '{other}'"
        ))),
    }
}

/// The stored spelling of an origin kind. `local` is the v9 column default, so
/// a row whose writer never touched the column still reads back as the person
/// at this machine; `unknown` is what a row reads back as when the column says
/// something this build does not know (`origin_from_columns`).
fn origin_kind_str(origin: &SessionOrigin) -> &'static str {
    match origin.kind {
        SessionOriginKind::Local => "local",
        SessionOriginKind::Peer => "peer",
        SessionOriginKind::Unknown => "unknown",
    }
}

/// The origin one row carries.
///
/// `peer` is the only kind that names a device, so it is the only one that
/// opens anything to a peer: the `Daemon` branch of `check_user_owner` matches
/// on `origin.device_id`, and a row whose `kind` is missing or is a spelling
/// this build does not recognise reads back as [`SessionOriginKind::Unknown`]
/// — not `local`. `local` is written by every pre-v9 row's `DEFAULT`, and it
/// is a *claim* about who owns the session that only this machine's own user
/// may act on; a `NULL` or unreadable column is not that claim.
///
/// A `peer` row missing its device id keeps `kind = peer` with no device, which
/// refuses for the same reason: an origin that cannot name the device that
/// asked is not authority for any device.
pub(super) fn origin_from_columns(
    kind: Option<String>,
    device: Option<String>,
    role: Option<String>,
) -> SessionOrigin {
    match kind.as_deref() {
        Some("peer") => SessionOrigin {
            kind: SessionOriginKind::Peer,
            device_id: device,
            role: role.as_deref().and_then(PeerRole::parse),
        },
        Some("local") => SessionOrigin::local(),
        _ => SessionOrigin {
            kind: SessionOriginKind::Unknown,
            device_id: None,
            role: None,
        },
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
        // Nobody has launched this session yet, so no directory has been
        // handed to a process: the creation stamps this when it stages the
        // command it is about to run.
        cwd: None,
        kind,
        provider: None,
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
        peer_session_id: None,
        origin: SessionOrigin::local(),
        // A caller that wants either of these sets them on the record it gets
        // back (`S5` decision 9); a row with no name and no parent is the
        // honest default for a session a human asked for.
        display_name: None,
        created_by: None,
        // Same rule for the creation-from-profile facts: a human's own session
        // resolves no profile, carries no labels and approves nothing, and its
        // context is itself — which `to_session` derives from the row's own id
        // rather than storing, so there is one place that answers that question.
        profile_id: None,
        context_id: None,
        // `unknown` is the honest default: a record whose creation has not
        // derived the marker yet is a record nobody has said anything about.
        unattended_state: UnattendedState::Unknown,
        labels: std::collections::BTreeMap::new(),
        // No overlay: a human's own session is the root lineage, and a row
        // that predates the column reads the same way (NULL).
        overlay: None,
        // No depth either: the birth stamps its own, and a row that predates
        // the column resumes at the closed end of the cap.
        depth: None,
        // No refusal recorded: a birth has heard nothing from any provider.
        disowned_peer_session_id: None,
    }
}

/// The wire tag of a permission request in a stored event payload
/// (`SessionEvent`'s internal tag).
const PERMISSION_REQUEST_TAG: &str = "permission_request";

/// What the v9 migration does with one stored payload.
pub(crate) enum OriginBackfill {
    /// Not a permission request, or it already carries an origin: leave it.
    Nothing,
    /// The same payload with a `local` origin written into it.
    Rewritten(Vec<u8>),
    /// The bytes are not a complete `SessionEvent` — leave them and count them.
    Unreadable,
    /// Larger than [`MAX_ORIGIN_BACKFILL_PAYLOAD_BYTES`]: never read at all,
    /// left byte-for-byte and counted (H8).
    Oversized,
}

/// The largest stored payload the v9 backfill will look at (H8).
///
/// The migration runs at startup, before anything serves, and it used to read
/// every `agent_report` payload into memory to decide what to do with it. One
/// mebibyte is far above any real permission request (the field caps in
/// `permission_broker.rs` are kilobytes) and bounds what a hostile or damaged
/// journal can make the daemon allocate while nobody is watching.
pub(crate) const MAX_ORIGIN_BACKFILL_PAYLOAD_BYTES: usize = 1024 * 1024;

/// The payload with a `local` origin written into it, when it is a permission
/// request stored before the field existed.
///
/// The v9 migration (`journal_schema.rs`) calls this so that old data is made
/// *valid* rather than left to degrade at replay — `origin` is required on the
/// wire, and a pre-origin payload would be dropped by hydration and flagged by
/// the live replay. `local` is a fact about those rows, not a guess: the daemon
/// that wrote them had no paired devices.
///
/// Two bounds, both of them H8. The payload is read at all only under
/// [`MAX_ORIGIN_BACKFILL_PAYLOAD_BYTES`], and it is rewritten only when the
/// **complete** `SessionEvent` it claims to be deserializes: a JSON object
/// tagged `permission_request` whose event fields are wrong is a row replay
/// would drop, and giving it an origin would make that damage look repaired.
/// Anything not rewritten keeps its bytes and its checksum exactly.
pub(crate) fn payload_with_origin(payload: &[u8]) -> OriginBackfill {
    if payload.len() > MAX_ORIGIN_BACKFILL_PAYLOAD_BYTES {
        return OriginBackfill::Oversized;
    }
    let Ok(serde_json::Value::Object(mut object)) =
        serde_json::from_slice::<serde_json::Value>(payload)
    else {
        return OriginBackfill::Unreadable;
    };
    if object.get("type").and_then(serde_json::Value::as_str) != Some(PERMISSION_REQUEST_TAG) {
        return OriginBackfill::Nothing;
    }
    if object.contains_key("origin") {
        return OriginBackfill::Nothing;
    }
    let mut origin = serde_json::Map::new();
    origin.insert(
        "kind".to_string(),
        serde_json::Value::String("local".to_string()),
    );
    object.insert("origin".to_string(), serde_json::Value::Object(origin));
    let rewritten = match serde_json::to_vec(&serde_json::Value::Object(object)) {
        Ok(rewritten) => rewritten,
        // Serializing a value that just parsed cannot fail in practice; if it
        // ever did, the row is left alone rather than written half-valid.
        Err(_) => return OriginBackfill::Unreadable,
    };
    // The shape is not the contract: only a payload the replay path could
    // deserialize is worth rewriting, because that is the path this backfill
    // exists to keep working.
    if serde_json::from_slice::<SessionEvent>(&rewritten).is_err() {
        return OriginBackfill::Unreadable;
    }
    OriginBackfill::Rewritten(rewritten)
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
    let dir = crate::test_dirs::test_temp_dir("devboule-journal");
    let path = dir.join("journal.db");
    (dir, path)
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
#[path = "journal_tests.rs"]
mod tests;
