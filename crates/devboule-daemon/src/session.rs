//! Daemon-owned PTY sessions.
//!
//! This is the M2 terminal backend moved out of the Tauri process. The PTY
//! plumbing follows the permissively licensed `portable-pty` pattern used by
//! terax-ai (Apache-2.0): `native_pty_system`/`openpty`, an explicit
//! `PtySize`, `CommandBuilder`, `take_writer`, `try_clone_reader`, and a
//! reader thread. v2 deliberately has no sandbox/AppContainer broker, so
//! Windows and Unix use the same native portable-pty path.
//!
//! SCREEN STATE (M3.5):
//! Every output chunk is applied to a headless terminal emulator
//! ([`crate::screen::Screen`]) under the session state lock. The emulator is
//! the screen authority, the same shape Zed's pty-host RFC and tmux use: on
//! attach the client gets one `Snapshot(as_of_seq)` of the visible grid, then
//! ordinary live output chunks with strictly greater sequences. There is no
//! byte replay for a live screen and no replay cursor. Coalesced frames are
//! additionally enqueued to the conversation journal off this thread
//! (`try_send`, never a disk wait); the journal stays the durable transcript.
//! A recovered session has no emulator and replays the journal instead.
//! Terminal bytes are converted with UTF-8-lossy at the coalesced-flush
//! boundary so a read that splits a UTF-8 codepoint cannot panic.
//!
//! THE INVARIANT:
//! A snapshot carrying `as_of_seq = N` is exactly the emulator state after
//! every chunk with sequence `<= N` has been applied and before any chunk
//! with sequence `> N`. The boundary is on application to the emulator — not
//! the pipe write, not the journal commit, not client receipt. Capture of the
//! screen and registration of a new attachment happen under ONE hold of the
//! state lock, so output can never fall into neither the snapshot nor the
//! attachment's unsent queue. When an attachment's unsent queue exceeds
//! [`PENDING_OUTPUT_BUDGET_BYTES`], the unsent suffix is discarded and
//! replaced by a fresh snapshot at the current boundary.
//!
//! DEVICE STATUS REPLIES:
//! The emulator answers terminal queries (ConPTY's startup `ESC[6n` among
//! them) with `PtyWrite` events. Those replies go straight back to the PTY
//! writer from the publish path — never through the journal, a snapshot, or
//! a client pipe. ConPTY stalls its render pipeline until the query is
//! answered; the daemon is the single responder.
//!
//! LOCKING ORDER:
//! - The session registry lock is never held across blocking PTY I/O.
//! - `writer` and `master` are cloned under the registry lock, then their
//!   locks are taken after the registry lock has been released.
//! - Teardown removes the session first, then kills, drops writer/master,
//!   waits for the child, and only then bounded-joins the reader. This
//!   order is load-bearing on Windows because waiting while a ConPTY
//!   master remains open can deadlock.
//!
//! STREAMING:
//! M2 rejected coalescing because the in-process Channel was free (ConPTY
//! itself was the floor at ~0.52 MiB/s, ~7k msg/s, median 67-byte chunks).
//! M3b puts NDJSON and a named pipe on that path; 7k tiny frames/s is a
//! different proposition. The reader coalesces into one seq-assigned chunk
//! per [`COALESCE_MAX_BYTES`] or [`COALESCE_FLUSH`], whichever comes first.
//! Seq is assigned at flush so the stream stays contiguous.
//!
//! Unsent live output waits in one bounded per-attachment queue
//! ([`StreamState::pending`]), not in a byte-history ring. The connection
//! writer pulls at most [`PULL_BATCH`] items per turn, so a slow client
//! leaves the bulk of the backlog inside the budgeted queue, where the
//! snapshot replacement above can coalesce it. Blocking the PTY reader is
//! wrong (it stalls ConPTY's render pipeline), so back-pressure is expressed
//! as state: the slow viewer is resynchronised, the process is never stalled.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use portable_pty::{Child, ChildKiller, MasterPty, PtySize};

#[cfg(test)]
use devboule_protocol::CursorShape;
use devboule_protocol::{
    compose_session_id, cursor_replay_ok, validate_session_id, Cursor, ErrorCode, ErrorDetails,
    JournalRetention, JournalStats, OwnerId, PermissionOutcome, Project, RetentionPatch, Session,
    SessionEvent, SessionKind, SessionModel, SessionState, SessionStateSnapshot, WireError,
    Workspace, WorkspaceIsolation,
};
#[cfg(test)]
use std::sync::Barrier;

use crate::journal::{new_session_record, Journal, PersistStatus, SessionRecord};
use crate::paths::RuntimePaths;
use crate::process_tree::{JobObject, ProcessHandle};
#[cfg(test)]
use crate::screen::Screen;
use crate::server::ServerState;
#[cfg(test)]
use devboule_protocol::TranscriptIntegrity;

#[path = "permission_broker.rs"]
mod permission_broker;
#[path = "session_runtime.rs"]
mod session_runtime;
pub(crate) use session_runtime::SessionRuntime;
#[path = "acp_client.rs"]
mod acp_client;
#[path = "acp_host.rs"]
mod acp_host;
#[path = "claude_client.rs"]
mod claude_client;
#[path = "event_pull.rs"]
mod event_pull;
#[path = "session_types.rs"]
mod session_types;
#[path = "shell_command.rs"]
mod shell_command;

pub use event_pull::ConnHandle;
pub(crate) use session_types::PendingEvent;
pub use session_types::PtyCommand;
use session_types::{
    Disposition, OutputMetrics, PendingItem, PullState, RegistryEntry, TranscriptSession,
};
use shell_command::resolve_pty_command;
pub use shell_command::write_test_pty_command;

/// Unsent live output one attachment may hold before the unsent suffix is
/// dropped and replaced by a fresh screen snapshot at the current sequence.
///
/// 256 KiB is the old ring's capacity: large enough that a healthy client is
/// never resynchronised (32 full 8 KiB coalesce frames), small enough that a
/// stalled client's queue stays bounded by roughly one worst-case snapshot.
pub const PENDING_OUTPUT_BUDGET_BYTES: usize = 256 * 1024;
/// Frame-count twin of [`PENDING_OUTPUT_BUDGET_BYTES`]: bounds the per-frame
/// JSON envelope overhead of a backlog of tiny frames.
pub const PENDING_OUTPUT_BUDGET_FRAMES: u64 = 64;

/// Session events pulled from one session per connection-writer turn.
/// Deliberately small: items left behind stay in the session's budgeted
/// pending queue where slow-client coalescing can still replace them, and a
/// pull never moves an unbounded batch into connection-local state.
const PULL_BATCH: usize = 16;

const READ_CHUNK: usize = 16 * 1024;
const INITIAL_COLS: u16 = 120;
const INITIAL_ROWS: u16 = 32;
pub const MAX_WRITE_BYTES: usize = 64 * 1024;
const READER_JOIN_BUDGET: Duration = Duration::from_millis(150);

/// Accumulate reader output until this many bytes, then assign one seq.
/// 8 KiB is half a ConPTY read and far under the 1 MiB frame cap.
pub const COALESCE_MAX_BYTES: usize = 8 * 1024;
/// Flush multi-byte coalesced output after this much quiet. 4 ms is half a
/// 120 Hz frame and enough to merge a flood of 67-byte ConPTY chunks. The
/// one-byte interactive case uses [`COALESCE_EAGER_BYTES`] instead.
pub const COALESCE_FLUSH: Duration = Duration::from_millis(4);
/// A single-byte ConPTY read is the interactive keystroke case. Publish it
/// immediately so a platform timer rounding the quiet wait cannot add a
/// frame-sized delay; larger reads retain the flood coalescing path.
pub const COALESCE_EAGER_BYTES: usize = 1;

/// After the child has been reaped, keep draining ConPTY for this long
/// before emitting `exit`. `Child::wait` returns before the last bytes
/// have been read; dropping them would truncate the live stream.
const EXIT_DRAIN: Duration = Duration::from_millis(200);

/// Five minutes separates a real thinking pause from a session that deserves
/// a liveness warning. A shorter threshold would turn normal terminal pauses
/// into noise and make the signal less trustworthy.
pub const SESSION_SILENCE_THRESHOLD: Duration = Duration::from_secs(300);
/// Shared OS liveness sweeper interval. Under the 5 s UI bound: a Task
/// Manager kill is observed on the next WaitForSingleObject(0) pass.
pub const SESSION_OS_SWEEP_INTERVAL: Duration = Duration::from_secs(2);

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(1);

/// The transport-specific ACP module supplies these three small adapters;
/// the registry, runtime, coalescer, journal and attachment code stay shared.
pub(super) trait SessionKiller: Send + Sync {
    fn kill(&mut self);
    /// Interrupt the current turn without killing the session. The default
    /// no-op covers killers whose transport has no turn concept (pty).
    fn interrupt(&mut self) {}
    fn clone_killer(&self) -> Box<dyn SessionKiller>;
}

pub(super) trait ModelSwitcher: Send + Sync {
    fn set_model(&self, model_id: Option<&str>, effort: Option<&str>) -> Result<(), WireError>;
    fn clone_switcher(&self) -> Box<dyn ModelSwitcher>;
}

pub(super) trait WaitableChild: Send {
    fn wait(self: Box<Self>) -> Option<u32>;
}

pub(super) struct StdioWaitableChild {
    pub(super) process: Arc<Mutex<std::process::Child>>,
}

impl WaitableChild for StdioWaitableChild {
    fn wait(self: Box<Self>) -> Option<u32> {
        loop {
            let status = self.process.lock().ok()?.try_wait().ok()?;
            if let Some(status) = status {
                return status.code().and_then(|code| u32::try_from(code).ok());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

pub(super) fn write_child_stdin(
    stdin: &Mutex<Option<std::process::ChildStdin>>,
    bytes: &[u8],
    label: &'static str,
) -> std::io::Result<()> {
    let mut stdin = stdin
        .lock()
        .map_err(|_| std::io::Error::other(format!("{label} stdin lock poisoned")))?;
    let Some(stdin) = stdin.as_mut() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            format!("{label} stdin is closed"),
        ));
    };
    stdin.write_all(bytes)?;
    stdin.flush()
}

pub(super) trait ReaderDispatch: Send {
    fn feed(&mut self, bytes: &[u8], runtime: &Arc<SessionRuntime>) -> Result<(), String>;
    fn finish(&mut self, runtime: &Arc<SessionRuntime>);
}

pub(super) trait StderrSource: Send {
    fn spawn(self: Box<Self>, runtime: Arc<SessionRuntime>) -> std::io::Result<JoinHandle<()>>;
}

/// The registry owns this value; the reader and command paths keep Arcs to
/// the endpoints/runtime they need after releasing the map lock.
struct PtySession {
    metadata: Session,
    owner: OwnerId,
    process_job: Arc<JobObject>,
    master: Option<Arc<Mutex<Box<dyn MasterPty + Send>>>>,
    killer: Box<dyn SessionKiller>,
    switcher: Option<Box<dyn ModelSwitcher>>,
    /// This is separate from the stdout reader: stderr must never be able to
    /// fill its pipe and stop the ACP child from producing responses.
    stderr_handle: Option<JoinHandle<()>>,
    child_wait: Option<JoinHandle<Option<u32>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    reader_handle: Option<JoinHandle<()>>,
    coalesce_handle: Option<JoinHandle<()>>,
    runtime: Arc<SessionRuntime>,
    exited: Arc<AtomicBool>,
    /// Set by `stop`: the process dies but the session object stays. The
    /// reader must not remove the registry entry or call session_finished.
    preserve_on_exit: Arc<AtomicBool>,
}

struct SpawnedSession {
    process_job: JobObject,
    master: Option<Arc<Mutex<Box<dyn MasterPty + Send>>>>,
    killer: Box<dyn SessionKiller>,
    switcher: Option<Box<dyn ModelSwitcher>>,
    child: Box<dyn WaitableChild>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    reader: Box<dyn Read + Send>,
    /// ACP supplies a structured decoder. Terminal sessions use the shared
    /// byte coalescer, which is constructed by `start_spawned_session`.
    reader_dispatch: Option<Box<dyn ReaderDispatch>>,
    stderr: Option<Box<dyn StderrSource>>,
    permission_broker: Option<Arc<permission_broker::PermissionBroker>>,
    os_handle: Option<ProcessHandle>,
    peer_session_id: Option<String>,
    agent_version: Option<String>,
}

struct PtyKiller {
    inner: Box<dyn ChildKiller + Send + Sync>,
}

impl SessionKiller for PtyKiller {
    fn kill(&mut self) {
        let _ = self.inner.kill();
    }

    fn clone_killer(&self) -> Box<dyn SessionKiller> {
        Box::new(Self {
            inner: self.inner.clone_killer(),
        })
    }
}

struct PtyWaitableChild {
    child: Box<dyn Child + Send + Sync>,
}

impl WaitableChild for PtyWaitableChild {
    fn wait(mut self: Box<Self>) -> Option<u32> {
        self.child.wait().ok().map(|status| status.exit_code())
    }
}

struct TerminalReaderDispatch {
    tx: Option<mpsc::Sender<Vec<u8>>>,
}

impl ReaderDispatch for TerminalReaderDispatch {
    fn feed(&mut self, bytes: &[u8], _runtime: &Arc<SessionRuntime>) -> Result<(), String> {
        self.tx
            .as_ref()
            .ok_or_else(|| "terminal coalescer is unavailable".to_string())?
            .send(bytes.to_vec())
            .map_err(|_| "terminal coalescer is unavailable".to_string())
    }

    fn finish(&mut self, _runtime: &Arc<SessionRuntime>) {
        self.tx.take();
    }
}

fn elapsed_ms_since_last_life(
    last_publish: Option<Instant>,
    exit_at: Option<Instant>,
    process_exited: bool,
    now: Instant,
) -> Option<u64> {
    let origin = if process_exited {
        exit_at
    } else {
        last_publish
    }?;
    Some(
        now.saturating_duration_since(origin)
            .as_millis()
            .try_into()
            .unwrap_or(u64::MAX),
    )
}

/// Wire metadata for a resumed session. `created_at_ms` is copied from the
/// journal row — resume does not mint a new session, and a fresh timestamp
/// would make a stored `(id, created_at_ms)` pair look stale.
fn session_metadata_for_resume(
    session_id: &str,
    record: SessionRecord,
    command: &PtyCommand,
    provider: String,
    peer_session_id: String,
    generation: u64,
) -> Session {
    Session {
        id: session_id.to_string(),
        workspace_id: record.workspace_id,
        cwd: Some(crate::workspace::display_path(
            &command.cwd.to_string_lossy(),
        )),
        kind: SessionKind::Acp,
        title: record.title,
        provider: Some(provider),
        peer_session_id: Some(peer_session_id),
        state: SessionState::Live { generation },
        elapsed_ms: Some(0),
        created_at_ms: record.created_at_ms,
    }
}

fn live_session_view(session: &PtySession) -> Session {
    let mut metadata = session.metadata.clone();
    metadata.peer_session_id = session.runtime.peer_session_id();
    if session.runtime.terminal_dead.load(Ordering::Acquire) {
        metadata.state = SessionState::Ended {
            generation: session.runtime.generation(),
            code: None,
            integrity: session.runtime.terminated_integrity(),
        };
        return metadata;
    }
    let Ok(stream) = session.runtime.lock_stream() else {
        metadata.state = SessionState::Ended {
            generation: session.runtime.generation(),
            code: None,
            integrity: session.runtime.terminated_integrity(),
        };
        return metadata;
    };
    metadata.state = match stream.disposition {
        Disposition::Running => SessionState::Live {
            generation: stream.generation,
        },
        Disposition::Silent => SessionState::Silent {
            generation: stream.generation,
        },
        Disposition::Exited { integrity } => SessionState::Ended {
            generation: stream.generation,
            code: stream.exit_code,
            integrity,
        },
        Disposition::Recovered { integrity } => SessionState::Recovered {
            generation: stream.generation,
            integrity,
        },
    };
    metadata.elapsed_ms = elapsed_ms_since_last_life(
        stream.last_publish,
        stream.exit_at,
        stream.process_exited,
        Instant::now(),
    );
    metadata
}

pub(super) fn process_gone() -> WireError {
    WireError::new(ErrorCode::InvalidRequest, "This terminal process is gone.")
}

fn unauthorized() -> WireError {
    WireError::new(
        ErrorCode::Unauthorized,
        "This client is not authorized to use that session.",
    )
}

fn owner_from_session_id(session_id: &str, user: &str) -> Result<OwnerId, WireError> {
    let mut parts = session_id.splitn(3, '.');
    if parts.next() != Some("s") {
        return Err(unauthorized());
    }
    let client = parts.next().ok_or_else(unauthorized)?;
    if parts.next().is_none() {
        return Err(unauthorized());
    }
    OwnerId::new(user, client).map_err(|_| unauthorized())
}

// The client token embedded in a session id authenticates the session owner;
// resize authority is the explicit, transferable subscription claim instead.
#[cfg(test)]
fn check_owner(entry: &RegistryEntry, owner: &OwnerId) -> Result<(), WireError> {
    if entry.owner() == owner {
        Ok(())
    } else {
        Err(unauthorized())
    }
}

fn check_user_owner(entry: &RegistryEntry, owner: &OwnerId) -> Result<(), WireError> {
    if entry.owner().user == owner.user {
        Ok(())
    } else {
        Err(unauthorized())
    }
}

fn check_attached(
    runtime: &SessionRuntime,
    conn: &ConnHandle,
    subscription_id: u64,
) -> Result<(), WireError> {
    runtime.is_observer(conn.id, subscription_id)
}

fn check_resize_owner(
    runtime: &SessionRuntime,
    conn: &ConnHandle,
    subscription_id: u64,
) -> Result<(), WireError> {
    runtime.is_resize_owner(conn.id, subscription_id)
}

type TransitionSink = Arc<dyn Fn(OwnerId) + Send + Sync>;
type JournalRosterCache = Arc<Mutex<Option<(u64, Vec<SessionRecord>)>>>;

const WORKSPACE_PATH_CACHE_CAP: usize = 1024;

#[derive(Default)]
struct WorkspacePathCache {
    entries: HashMap<String, (PathBuf, u64)>,
    clock: u64,
}

impl WorkspacePathCache {
    fn next_stamp(&mut self) -> u64 {
        self.clock = self.clock.wrapping_add(1);
        self.clock
    }

    fn get(&mut self, workspace_id: &str) -> Option<PathBuf> {
        let path = self
            .entries
            .get(workspace_id)
            .map(|(path, _)| path.clone())?;
        let stamp = self.next_stamp();
        self.entries
            .insert(workspace_id.to_string(), (path.clone(), stamp));
        Some(path)
    }

    fn insert(&mut self, workspace_id: String, path: PathBuf) {
        if self.entries.len() >= WORKSPACE_PATH_CACHE_CAP
            && !self.entries.contains_key(&workspace_id)
        {
            if let Some(oldest) = self
                .entries
                .iter()
                .min_by_key(|(_, (_, stamp))| *stamp)
                .map(|(id, _)| id.clone())
            {
                self.entries.remove(&oldest);
            }
        }
        let stamp = self.next_stamp();
        self.entries.insert(workspace_id, (path, stamp));
    }

    fn remove(&mut self, workspace_id: &str) {
        self.entries.remove(workspace_id);
    }
}

#[cfg(test)]
type JournalRosterAfterListHook = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone)]
struct ConnectionPresence {
    user: String,
    focused_session_id: Option<String>,
    app_visible: bool,
}

#[derive(Clone)]
pub struct SessionRegistry {
    inner: Arc<Mutex<HashMap<String, RegistryEntry>>>,
    paths: RuntimePaths,
    journal: Option<Arc<Journal>>,
    transition_sink: Arc<Mutex<Option<TransitionSink>>>,
    presence: Arc<Mutex<HashMap<u64, ConnectionPresence>>>,
    /// Journal rows are the slow, mostly-static half of a roster. Keep them
    /// out of live-session transition broadcasts; lifecycle operations below
    /// invalidate this cache when they can change the row set.
    journal_roster: JournalRosterCache,
    /// Workspace paths change only through workspace mutations. Cache them
    /// after the first successful lookup so session creation does not enqueue
    /// a blocking SQLite RPC for every new process.
    workspace_paths: Arc<Mutex<WorkspacePathCache>>,
    /// Once materialized, a user's full wire roster is updated in place for
    /// one live-session transition. This keeps the full-snapshot contract
    /// while avoiding a second walk over every live entry.
    state_roster_cache: Arc<Mutex<HashMap<String, Vec<SessionStateSnapshot>>>>,
    #[cfg(test)]
    journal_list_calls: Arc<AtomicU64>,
    #[cfg(test)]
    full_roster_builds: Arc<AtomicU64>,
    #[cfg(test)]
    journal_roster_after_list_hook: Arc<Mutex<Option<JournalRosterAfterListHook>>>,
}

/// Whether a resolved provider id came from the session-create request
/// or from `DEVBOULE_AGENT_PROVIDER`. Consent for npx wrappers requires
/// the request; the env override cannot supply it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProviderProvenance {
    Request,
    Env,
}

impl SessionRegistry {
    pub(crate) fn runtime_dir(&self) -> &std::path::Path {
        &self.paths.dir
    }

    pub(crate) fn pipe_name(&self) -> &str {
        &self.paths.pipe_name
    }

    /// The journal worker owns SQLite, so its file-size query is a bounded
    /// RPC just like `list`. Diagnostics reports the failure instead of
    /// inventing a zero-sized database.
    pub(crate) fn journal_file_bytes(&self) -> Option<Result<u64, String>> {
        self.journal
            .as_ref()
            .map(|journal| journal.file_len().map_err(|error| error.to_string()))
    }

    pub fn new(paths: RuntimePaths, journal: Option<Arc<Journal>>) -> Self {
        let registry = Self {
            inner: Arc::new(Mutex::new(HashMap::new())),
            paths,
            journal,
            transition_sink: Arc::new(Mutex::new(None)),
            presence: Arc::new(Mutex::new(HashMap::new())),
            journal_roster: Arc::new(Mutex::new(None)),
            workspace_paths: Arc::new(Mutex::new(WorkspacePathCache::default())),
            state_roster_cache: Arc::new(Mutex::new(HashMap::new())),
            #[cfg(test)]
            journal_list_calls: Arc::new(AtomicU64::new(0)),
            #[cfg(test)]
            full_roster_builds: Arc::new(AtomicU64::new(0)),
            #[cfg(test)]
            journal_roster_after_list_hook: Arc::new(Mutex::new(None)),
        };
        spawn_os_liveness_sweeper(&registry);
        registry.reconcile_worktree_journal();
        registry
    }

    fn reconcile_worktree_journal(&self) {
        let Some(journal) = &self.journal else {
            return;
        };
        let Ok(projects) = journal.projects_list() else {
            return;
        };
        for project in projects {
            let Ok(workspaces) = journal.workspaces_list(&project.id) else {
                continue;
            };
            let project_path = PathBuf::from(&project.path);
            let root = crate::worktree::worktree_root_beside_project(&project_path);
            let mut known_checkouts = Vec::new();
            for workspace in workspaces {
                if workspace.isolation != WorkspaceIsolation::Worktree {
                    continue;
                }
                let checkout = PathBuf::from(&workspace.path);
                known_checkouts.push(checkout.clone());
                if !checkout.exists() {
                    eprintln!(
                        "worktree row '{}' points at missing checkout '{}'; detaching the row",
                        workspace.id, workspace.path
                    );
                    let _ = journal.workspace_delete(&workspace.id);
                }
            }
            let Some(root) = root else {
                continue;
            };
            let Ok(entries) = std::fs::read_dir(&root) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }
                let known = known_checkouts.iter().any(|known| {
                    crate::worktree::canonical_or_original(known)
                        == crate::worktree::canonical_or_original(&path)
                });
                if !known {
                    eprintln!(
                        "orphan worktree checkout '{}' has no journal row; leaving it on disk",
                        path.display()
                    );
                }
            }
        }
    }

    #[cfg(test)]
    fn journal_list_call_count(&self) -> u64 {
        self.journal_list_calls.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    fn full_roster_build_count(&self) -> u64 {
        self.full_roster_builds.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    fn set_journal_roster_after_list_hook(&self, hook: JournalRosterAfterListHook) {
        *self
            .journal_roster_after_list_hook
            .lock()
            .expect("journal roster test hook") = Some(hook);
    }

    fn invalidate_journal_roster(&self) {
        // A poisoned cache is not a reason to keep serving possibly stale
        // roster data. Recover the guard and clear it so the next read is
        // forced to consult the journal again.
        *self
            .journal_roster
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = None;
        self.invalidate_state_roster();
    }

    fn invalidate_state_roster(&self) {
        self.state_roster_cache
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clear();
    }

    fn cached_workspace_path(&self, workspace_id: &str) -> Option<PathBuf> {
        self.workspace_paths
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(workspace_id)
    }

    fn remember_workspace_path(&self, workspace_id: &str, path: PathBuf) {
        self.workspace_paths
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(workspace_id.to_string(), path);
    }

    fn invalidate_workspace_path(&self, workspace_id: &str) {
        self.workspace_paths
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(workspace_id);
    }

    fn invalidate_stale_journal_roster(&self) {
        let Some(journal) = self.journal.as_ref() else {
            return;
        };
        let revision = journal.session_set_revision();
        let stale = self
            .journal_roster
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .as_ref()
            .is_some_and(|(cached, _)| *cached != revision);
        if stale {
            self.invalidate_journal_roster();
        }
    }

    fn journal_roster(&self) -> Option<Vec<SessionRecord>> {
        let journal = self.journal.as_ref()?;
        let before_revision = journal.session_set_revision();
        let cached_rows = self
            .journal_roster
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .as_ref()
            .and_then(|(cached_revision, rows)| {
                (*cached_revision == before_revision).then(|| rows.clone())
            });
        if cached_rows.is_some() {
            return cached_rows;
        }

        #[cfg(test)]
        self.journal_list_calls.fetch_add(1, Ordering::Relaxed);
        let rows = journal.list().ok()?;

        #[cfg(test)]
        if let Some(hook) = self
            .journal_roster_after_list_hook
            .lock()
            .ok()
            .and_then(|mut hook| hook.take())
        {
            hook();
        }

        let after_revision = journal.session_set_revision();
        if after_revision != before_revision {
            // The rows and revision came from different points in the
            // journal's mutation stream. Returning this point-in-time result
            // is safe, but caching it under either revision would make the
            // next reader trust data that it did not actually read at that
            // revision. Let the next call retry instead.
            return Some(rows);
        }

        let mut cache = self
            .journal_roster
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if let Some((cached_revision, cached)) = cache.as_ref() {
            if *cached_revision == after_revision {
                return Some(cached.clone());
            }
        }
        *cache = Some((before_revision, rows.clone()));
        Some(rows)
    }

    pub(crate) fn set_transition_sink(&self, sink: TransitionSink) {
        if let Ok(mut current) = self.transition_sink.lock() {
            *current = Some(sink);
        }
    }

    fn emit_transition(&self, owner: &OwnerId) {
        let sink = self
            .transition_sink
            .lock()
            .ok()
            .and_then(|current| current.clone());
        if let Some(sink) = sink {
            sink(owner.clone());
        }
    }

    fn notify_session_transition(&self, owner: &OwnerId, session_id: &str) {
        self.refresh_state_snapshot(owner, session_id);
        self.emit_transition(owner);
    }

    pub(crate) fn state_snapshots(&self, owner: &OwnerId) -> Vec<SessionStateSnapshot> {
        self.invalidate_stale_journal_roster();
        if let Ok(cache) = self.state_roster_cache.lock() {
            if let Some(snapshots) = cache.get(&owner.user) {
                return snapshots.clone();
            }
        }

        let snapshots = self.build_state_snapshots(owner);
        // A failed journal read must remain retryable. Live state is still
        // useful to return now, but do not let that partial roster become an
        // unbounded cache entry.
        let journal_is_cached = self.journal.is_none()
            || self
                .journal_roster
                .lock()
                .ok()
                .is_some_and(|cache| cache.is_some());
        if journal_is_cached {
            if let Ok(mut cache) = self.state_roster_cache.lock() {
                cache.insert(owner.user.clone(), snapshots.clone());
            }
        }
        snapshots
    }

    fn build_state_snapshots(&self, owner: &OwnerId) -> Vec<SessionStateSnapshot> {
        #[cfg(test)]
        self.full_roster_builds.fetch_add(1, Ordering::Relaxed);
        let mut sessions = self
            .inner
            .lock()
            .map(|map| {
                map.values()
                    .filter(|entry| entry.owner().user == owner.user)
                    .map(|entry| (entry.to_session(), entry.runtime().attention()))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let live_ids = sessions
            .iter()
            .map(|(session, _)| session.id.clone())
            .collect::<std::collections::HashSet<_>>();
        if let Some(rows) = self.journal_roster() {
            sessions.extend(rows.into_iter().filter_map(|row| {
                if live_ids.contains(&row.id) {
                    return None;
                }
                (row.owner == owner.user).then(|| (row.to_session(), None))
            }));
        }
        sessions.sort_by(|left, right| left.0.id.cmp(&right.0.id));
        sessions
            .into_iter()
            .map(|(session, attention)| SessionStateSnapshot {
                id: session.id,
                workspace_id: session.workspace_id,
                kind: session.kind,
                title: session.title,
                state: session.state,
                elapsed_ms: session.elapsed_ms,
                attention,
            })
            .collect()
    }

    fn refresh_state_snapshot(&self, owner: &OwnerId, session_id: &str) {
        let snapshot = self.inner.lock().ok().and_then(|map| {
            map.get(session_id)
                .filter(|entry| entry.owner().user == owner.user)
                .map(|entry| {
                    let session = entry.to_session();
                    SessionStateSnapshot {
                        id: session.id,
                        workspace_id: session.workspace_id,
                        kind: session.kind,
                        title: session.title,
                        state: session.state,
                        elapsed_ms: session.elapsed_ms,
                        attention: entry.runtime().attention(),
                    }
                })
        });
        if let Ok(mut cache) = self.state_roster_cache.lock() {
            let Some(roster) = cache.get_mut(&owner.user) else {
                return;
            };
            roster.retain(|session| session.id != session_id);
            if let Some(snapshot) = snapshot {
                roster.push(snapshot);
                roster.sort_by(|left, right| left.id.cmp(&right.id));
            }
        }
    }

    fn configure_runtime_attention(&self, runtime: &Arc<SessionRuntime>, owner: &OwnerId) {
        let presence = Arc::clone(&self.presence);
        let user = owner.user.clone();
        let session_id = runtime.session_id.clone();
        let suppressed_session_id = session_id.clone();
        let suppressed = Arc::new(move || {
            presence.lock().is_ok_and(|connections| {
                connections.values().any(|connection| {
                    connection.user == user
                        && connection.app_visible
                        && connection.focused_session_id.as_deref()
                            == Some(suppressed_session_id.as_str())
                })
            })
        });
        let registry = self.clone();
        let owner = owner.clone();
        let notify = Arc::new(move || {
            registry.notify_session_transition(&owner, &session_id);
        });
        runtime.set_attention_hooks(suppressed, notify);
    }

    pub(crate) fn set_presence(
        &self,
        conn_id: u64,
        owner: &OwnerId,
        focused_session_id: Option<String>,
        app_visible: bool,
    ) -> Result<(), WireError> {
        if let Some(session_id) = focused_session_id.as_deref() {
            validate_session_id(session_id)
                .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        }
        if let Ok(mut presence) = self.presence.lock() {
            presence.insert(
                conn_id,
                ConnectionPresence {
                    user: owner.user.clone(),
                    focused_session_id: focused_session_id.clone(),
                    app_visible,
                },
            );
        } else {
            return Err(internal("Session state is unavailable."));
        }
        // The presence guard is intentionally released before clearing
        // attention: raises use the global attention -> presence order.
        if app_visible {
            if let Some(session_id) = focused_session_id {
                let runtime = self.inner.lock().ok().and_then(|map| {
                    map.get(&session_id).and_then(|entry| {
                        (entry.owner().user == owner.user).then(|| entry.runtime())
                    })
                });
                if runtime.is_some_and(|runtime| runtime.clear_attention()) {
                    self.notify_session_transition(owner, &session_id);
                }
            }
        }
        Ok(())
    }

    pub(crate) fn clear_presence(&self, conn_id: u64) {
        if let Ok(mut presence) = self.presence.lock() {
            presence.remove(&conn_id);
        }
    }

    pub(crate) fn output_metrics(&self) -> OutputMetrics {
        let Ok(map) = self.inner.lock() else {
            return OutputMetrics::default();
        };
        map.values()
            .map(RegistryEntry::runtime)
            .map(|runtime| runtime.output_metrics())
            .fold(OutputMetrics::default(), |mut total, metrics| {
                total.peak_pending_bytes = total.peak_pending_bytes.max(metrics.peak_pending_bytes);
                total.coalesced_bytes = total
                    .coalesced_bytes
                    .saturating_add(metrics.coalesced_bytes);
                total.coalesced_frames = total
                    .coalesced_frames
                    .saturating_add(metrics.coalesced_frames);
                total
            })
    }

    /// Wire view of the journal writer's counters for the Status reply.
    /// `None` when the journal could not be opened: there is no writer
    /// whose behaviour could be counted, and inventing zeros would claim
    /// an integrity nobody observed.
    pub fn journal_stats(&self) -> Option<JournalStats> {
        self.journal.as_ref().map(|journal| {
            let snapshot = journal.stats();
            JournalStats {
                accepted_frames: snapshot.accepted_frames,
                accepted_bytes: snapshot.accepted_bytes,
                committed_frames: snapshot.committed_frames,
                committed_bytes: snapshot.committed_bytes,
                failed_frames: snapshot.failed_frames,
            }
        })
    }

    pub fn flush_journal(&self) {
        if let Some(journal) = &self.journal {
            let _ = journal.flush();
            journal.shutdown();
        }
    }

    pub fn journal_usage(&self) -> Result<crate::journal::JournalUsage, WireError> {
        self.journal
            .as_ref()
            .ok_or_else(journal_unavailable)?
            .usage()
            .map_err(Into::into)
    }

    pub fn journal_retention_get(&self) -> Result<JournalRetention, WireError> {
        self.journal
            .as_ref()
            .ok_or_else(journal_unavailable)?
            .retention_get()
            .map_err(Into::into)
    }

    pub fn journal_retention_set(
        &self,
        patch: RetentionPatch,
    ) -> Result<JournalRetention, WireError> {
        let result = self
            .journal
            .as_ref()
            .ok_or_else(journal_unavailable)?
            .retention_set(patch)
            .map_err(WireError::from);
        if result.is_ok() {
            self.invalidate_journal_roster();
        }
        result
    }

    pub fn projects_list(&self) -> Result<Vec<Project>, WireError> {
        self.journal
            .as_ref()
            .ok_or_else(journal_unavailable)?
            .projects_list()
            .map(|projects| {
                projects
                    .into_iter()
                    .map(|project| project.to_project())
                    .collect()
            })
            .map_err(WireError::from)
    }

    pub fn project_add(&self, path: &str) -> Result<Project, WireError> {
        let record = crate::workspace::project_record(path)?;
        self.journal
            .as_ref()
            .ok_or_else(journal_unavailable)?
            .project_add(record)
            .map(|project| project.to_project())
            .map_err(WireError::from)
    }

    pub fn workspaces_list(&self, project_id: &str) -> Result<Vec<Workspace>, WireError> {
        self.journal
            .as_ref()
            .ok_or_else(journal_unavailable)?
            .workspaces_list(project_id)
            .map(|workspaces| {
                workspaces
                    .into_iter()
                    .map(|workspace| workspace.to_workspace())
                    .collect()
            })
            .map_err(WireError::from)
    }

    pub fn workspace_create(
        &self,
        project_id: &str,
        isolation: WorkspaceIsolation,
        branch: Option<String>,
    ) -> Result<Workspace, WireError> {
        match isolation {
            WorkspaceIsolation::Local => {
                if branch.is_some() {
                    return Err(WireError::new(
                        ErrorCode::InvalidRequest,
                        "Local workspaces do not take a branch.",
                    ));
                }
                self.create_local_workspace(project_id)
            }
            WorkspaceIsolation::Worktree => self.create_worktree_workspace(project_id, branch),
        }
    }

    fn create_local_workspace(&self, project_id: &str) -> Result<Workspace, WireError> {
        let journal = self.journal.as_ref().ok_or_else(journal_unavailable)?;
        let project = self.require_project(journal, project_id)?;
        let workspace = journal
            .workspace_create(crate::workspace::local_workspace_record(&project))
            .map_err(WireError::from)?;
        self.remember_workspace_path(&workspace.id, PathBuf::from(&workspace.path));
        Ok(workspace.to_workspace())
    }

    fn create_worktree_workspace(
        &self,
        project_id: &str,
        branch: Option<String>,
    ) -> Result<Workspace, WireError> {
        let journal = self.journal.as_ref().ok_or_else(journal_unavailable)?;
        let project = self.require_project(journal, project_id)?;
        let project_path = PathBuf::from(&project.path);
        let live = crate::git::detect_git_repository(&project_path);
        refuse_worktree_unless_live_git_allows(&project.git_state, live.as_str(), project_id)?;
        let branch = match branch.filter(|value| !value.trim().is_empty()) {
            Some(branch) => branch,
            None => crate::worktree::generated_branch_slug(worktree_branch_seed()),
        };
        let Some(checkout) = crate::worktree::checkout_path_for_branch(&project_path, &branch)
        else {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                format!("Project '{project_id}' has no parent directory for a sibling worktree."),
            ));
        };
        let Some(root) = checkout.parent() else {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                format!("Project '{project_id}' has no parent directory for a sibling worktree."),
            ));
        };
        std::fs::create_dir_all(root).map_err(|error| {
            WireError::new(
                ErrorCode::Io,
                format!(
                    "Worktree directory '{}' is not writable: {error}",
                    crate::workspace::display_path(&root.to_string_lossy())
                ),
            )
        })?;
        if let Err(error) =
            crate::worktree::run_worktree_add_command(&project_path, &checkout, &branch, "HEAD")
        {
            let cleanup = cleanup_failed_worktree_add(&project_path, &checkout);
            return Err(WireError::new(
                ErrorCode::WorkspaceUnavailable,
                match cleanup {
                    Ok(()) => format!("Could not add git worktree for '{project_id}': {error}"),
                    Err(cleanup_error) => format!(
                        "Could not add git worktree for '{project_id}': {error}; leftover checkout at '{}' ({cleanup_error})",
                        crate::workspace::display_path(&checkout.to_string_lossy())
                    ),
                },
            ));
        }
        let checkout = std::fs::canonicalize(&checkout).unwrap_or(checkout);
        let record = crate::workspace::worktree_workspace_record(&project, &checkout, &branch);
        let workspace = match journal.workspace_create(record) {
            Ok(workspace) => workspace,
            Err(error) => {
                if let Err(cleanup_error) = cleanup_failed_worktree_add(&project_path, &checkout) {
                    return Err(WireError::new(
                        ErrorCode::Journal,
                        format!(
                            "{error}; leftover checkout at '{}' ({cleanup_error})",
                            crate::workspace::display_path(&checkout.to_string_lossy())
                        ),
                    ));
                }
                return Err(WireError::from(error));
            }
        };
        self.remember_workspace_path(&workspace.id, PathBuf::from(&workspace.path));
        Ok(workspace.to_workspace())
    }

    fn require_project(
        &self,
        journal: &Journal,
        project_id: &str,
    ) -> Result<crate::journal::ProjectRecord, WireError> {
        let project = journal
            .project_get(project_id)
            .map_err(WireError::from)?
            .ok_or_else(|| {
                WireError::new(
                    ErrorCode::WorkspaceUnavailable,
                    format!("Project '{project_id}' does not exist."),
                )
            })?;
        if !std::path::Path::new(&project.path).is_dir() {
            return Err(WireError::new(
                ErrorCode::WorkspaceUnavailable,
                format!("Project '{project_id}' is no longer an existing folder."),
            ));
        }
        Ok(project)
    }

    pub fn workspace_delete(&self, workspace_id: &str, force: bool) -> Result<(), WireError> {
        let journal = self.journal.as_ref().ok_or_else(journal_unavailable)?;
        let workspace = journal
            .workspace_get(workspace_id)
            .map_err(WireError::from)?
            .ok_or_else(|| workspace_unavailable(workspace_id, "it does not exist"))?;
        match workspace.isolation {
            WorkspaceIsolation::Local => {
                return Err(WireError::new(
                    ErrorCode::InvalidRequest,
                    "The local workspace is the project folder and is not removed as a worktree.",
                ));
            }
            WorkspaceIsolation::Worktree => {}
        }
        let checkout = PathBuf::from(&workspace.path);
        let project = journal
            .project_get(&workspace.project_id)
            .map_err(WireError::from)?;
        let Some(project) = project else {
            return self.detach_worktree_row(
                journal,
                workspace_id,
                &checkout,
                "its project row is gone",
            );
        };
        let repo = PathBuf::from(&project.path);
        if !repo.is_dir() {
            return self.detach_worktree_row(
                journal,
                workspace_id,
                &checkout,
                "its project folder is gone",
            );
        }
        let Some(root) = crate::worktree::worktree_root_beside_project(&repo) else {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "Project has no parent directory for a sibling worktree.",
            ));
        };
        if !crate::worktree::path_is_within(&checkout, &root) {
            let path = crate::workspace::display_path(&checkout.to_string_lossy());
            let root = crate::workspace::display_path(&root.to_string_lossy());
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                format!("Checkout '{path}' is not inside worktree root '{root}'."),
            )
            .with_details(ErrorDetails::WorktreeNotConfined { path, root }));
        }
        let expected_branch = workspace.branch.as_deref().unwrap_or("");
        match crate::worktree::list_existing_worktrees(&repo) {
            Ok(entries) => {
                match crate::worktree::identify_worktree_at_path(
                    &entries,
                    &checkout,
                    expected_branch,
                ) {
                    crate::worktree::WorktreeIdentity::Locked => {
                        let path = crate::workspace::display_path(&checkout.to_string_lossy());
                        return Err(WireError::new(
                            ErrorCode::InvalidRequest,
                            format!(
                                "Worktree '{path}' is locked. Unlock it before removing; --force does not override a lock."
                            ),
                        )
                        .with_details(ErrorDetails::WorktreeLocked { path }));
                    }
                    crate::worktree::WorktreeIdentity::BranchMismatch { observed } => {
                        let path = crate::workspace::display_path(&checkout.to_string_lossy());
                        return Err(WireError::new(
                            ErrorCode::InvalidRequest,
                            format!(
                                "Worktree at '{path}' is branch '{}', not '{}'. Refusing to remove another workspace's checkout.",
                                observed.as_deref().unwrap_or("(detached)"),
                                expected_branch
                            ),
                        )
                        .with_details(ErrorDetails::WorktreeMismatch {
                            path,
                            expected_branch: expected_branch.to_string(),
                            observed_branch: observed,
                        }));
                    }
                    crate::worktree::WorktreeIdentity::Match
                    | crate::worktree::WorktreeIdentity::Missing => {}
                }
            }
            Err(error) => {
                return Err(WireError::new(
                    ErrorCode::WorkspaceUnavailable,
                    format!("Could not list worktrees for '{workspace_id}': {error}"),
                ));
            }
        }
        if !force {
            if let Ok(true) = crate::worktree::checkout_has_dirty_files(&checkout) {
                return Err(WireError::new(
                    ErrorCode::InvalidRequest,
                    crate::worktree::worktree_dirty_remove_message(&checkout),
                )
                .with_details(ErrorDetails::WorktreeDirty {
                    path: crate::workspace::display_path(&checkout.to_string_lossy()),
                    force_required: true,
                }));
            }
        }
        let command = crate::worktree::build_worktree_remove_command(&repo, &checkout, force);
        if let Err(error) = crate::worktree::run_worktree_remove_command_with_recovery(
            &command, &repo, &checkout, force,
        ) {
            if crate::worktree::is_dirty_worktree_remove_error(&error) {
                return Err(WireError::new(
                    ErrorCode::InvalidRequest,
                    crate::worktree::worktree_dirty_remove_message(&checkout),
                )
                .with_details(ErrorDetails::WorktreeDirty {
                    path: crate::workspace::display_path(&checkout.to_string_lossy()),
                    force_required: true,
                }));
            }
            return Err(WireError::new(
                ErrorCode::WorkspaceUnavailable,
                format!("Could not remove worktree '{workspace_id}': {error}"),
            ));
        }
        journal
            .workspace_delete(workspace_id)
            .map_err(WireError::from)?;
        self.invalidate_workspace_path(workspace_id);
        Ok(())
    }

    fn detach_worktree_row(
        &self,
        journal: &Journal,
        workspace_id: &str,
        checkout: &Path,
        reason: &str,
    ) -> Result<(), WireError> {
        let leftover = checkout
            .exists()
            .then(|| crate::workspace::display_path(&checkout.to_string_lossy()));
        journal
            .workspace_delete(workspace_id)
            .map_err(WireError::from)?;
        self.invalidate_workspace_path(workspace_id);
        eprintln!(
            "workspace '{workspace_id}' detached because {reason}; checkout left at {leftover:?}"
        );
        Ok(())
    }

    fn workspace_cwd(&self, workspace_id: &str) -> Result<PathBuf, WireError> {
        if let Some(path) = self.cached_workspace_path(workspace_id) {
            if path.is_dir() {
                return Ok(path);
            }
            // The path can disappear after it was cached. Drop it before a
            // bounded journal refresh so a later mutation can repair it.
            self.invalidate_workspace_path(workspace_id);
        }
        let journal = self.journal.as_ref().ok_or_else(|| {
            workspace_journal_error(
                workspace_id,
                crate::journal::JournalError::Unavailable("journal is not open".to_string()),
            )
        })?;
        let workspace = journal
            .workspace_get_for_session(workspace_id)
            .map_err(|error| workspace_journal_error(workspace_id, error))?
            .ok_or_else(|| workspace_unavailable(workspace_id, "it does not exist"))?;
        let path = PathBuf::from(workspace.path);
        if !path.is_dir() {
            return Err(workspace_unavailable(
                workspace_id,
                "its folder is no longer available",
            ));
        }
        self.remember_workspace_path(workspace_id, path.clone());
        Ok(path)
    }

    fn apply_workspace_cwd(
        &self,
        workspace_id: Option<&str>,
        command: &mut PtyCommand,
    ) -> Result<(), WireError> {
        if let Some(workspace_id) = workspace_id {
            command.cwd = self.workspace_cwd(workspace_id)?;
        }
        Ok(())
    }

    pub fn delete_session(&self, session_id: &str, owner: &OwnerId) -> Result<(), WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let transcript_in_registry = {
            let map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            match map.get(session_id) {
                Some(entry) => {
                    if entry.owner().user != owner.user {
                        return Err(unauthorized());
                    }
                    if entry.as_live().is_some() {
                        return Err(WireError::new(
                            ErrorCode::InvalidRequest,
                            "Close the session before deleting it.",
                        ));
                    }
                    true
                }
                None => false,
            }
        };
        let journal = self.journal.as_ref().ok_or_else(journal_unavailable)?;
        if !transcript_in_registry {
            let record = journal
                .list()
                .map_err(WireError::from)?
                .into_iter()
                .find(|record| record.id == session_id)
                .ok_or_else(not_found)?;
            if record.owner != owner.user {
                return Err(unauthorized());
            }
        }
        journal
            .delete_session(session_id)
            .map_err(WireError::from)?;
        self.invalidate_journal_roster();
        if transcript_in_registry {
            if let Ok(mut map) = self.inner.lock() {
                map.remove(session_id);
            }
            journal.unpin(session_id);
        }
        self.notify_session_transition(owner, session_id);
        Ok(())
    }

    /// Env override applies only when the request did not name a provider.
    /// An explicit `provider` is the frontend's choice and must not be
    /// silently replaced by `DEVBOULE_AGENT_PROVIDER`.
    fn resolve_session_provider(
        kind: SessionKind,
        provider: Option<String>,
        env_provider: Option<&str>,
    ) -> (SessionKind, Option<String>, Option<ProviderProvenance>) {
        let requested = provider.filter(|id| !id.is_empty());
        let env_provider = env_provider.filter(|value| !value.is_empty());
        let kind =
            if kind == SessionKind::Acp && requested.is_none() && env_provider == Some("claude") {
                SessionKind::Claude
            } else {
                kind
            };
        let (provider, provenance) = if requested.is_some() {
            let provider = requested.filter(|id| id != "claude");
            let provenance = provider.as_ref().map(|_| ProviderProvenance::Request);
            (provider, provenance)
        } else {
            let provider = env_provider
                .map(str::to_string)
                .filter(|id| id != "claude" && !id.is_empty());
            let provenance = provider.as_ref().map(|_| ProviderProvenance::Env);
            (provider, provenance)
        };
        (kind, provider, provenance)
    }

    /// Consent for npx wrappers is explicit `provider` on the request.
    /// An env override must not launch third-party npx code.
    fn env_override_cannot_launch_npx(
        id: &str,
        provenance: Option<ProviderProvenance>,
        origin: Option<crate::provider_catalog::ProviderOrigin>,
    ) -> Result<(), WireError> {
        if provenance == Some(ProviderProvenance::Env)
            && origin == Some(crate::provider_catalog::ProviderOrigin::NpxWrapper)
        {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                format!(
                    "provider '{id}' is an npx wrapper; npx wrappers require explicit selection, the env override cannot launch them"
                ),
            ));
        }
        Ok(())
    }

    fn reject_env_npx_wrapper(
        id: &str,
        provenance: Option<ProviderProvenance>,
        paths: &RuntimePaths,
    ) -> Result<(), WireError> {
        let origin = crate::provider_catalog::find_in_catalog(
            id,
            &crate::registry::CdnRegistryFetch,
            &paths.dir,
        )
        .map(|agent| agent.origin);
        Self::env_override_cannot_launch_npx(id, provenance, origin)
    }

    pub fn create(
        &self,
        state: &Arc<ServerState>,
        owner: &OwnerId,
        workspace_id: Option<String>,
        kind: SessionKind,
        provider: Option<String>,
        command: Option<PtyCommand>,
    ) -> Result<Session, WireError> {
        let env_provider = std::env::var("DEVBOULE_AGENT_PROVIDER").ok();
        self.create_with_provider_env(
            state,
            owner,
            workspace_id,
            kind,
            provider,
            command,
            env_provider.as_deref(),
        )
    }

    // Env is a seventh caller argument so tests inject DEVBOULE_AGENT_PROVIDER
    // without mutating process env (which races under cargo's parallel harness).
    #[allow(clippy::too_many_arguments)]
    fn create_with_provider_env(
        &self,
        state: &Arc<ServerState>,
        owner: &OwnerId,
        workspace_id: Option<String>,
        kind: SessionKind,
        provider: Option<String>,
        command: Option<PtyCommand>,
        env_provider: Option<&str>,
    ) -> Result<Session, WireError> {
        let workspace_id_ref = workspace_id.as_deref();
        let workspace_cwd = workspace_id_ref
            .map(|workspace_id| self.workspace_cwd(workspace_id))
            .transpose()?;
        let unique = format!("{:08x}", SESSION_COUNTER.fetch_add(1, Ordering::Relaxed));
        let id = compose_session_id(&owner.session_token(), &unique)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let (kind, provider, provenance) =
            Self::resolve_session_provider(kind, provider, env_provider);
        let mut command = match command {
            Some(command) => command,
            None if kind == SessionKind::Claude => claude_client::resolve_command(&self.paths)?,
            None if kind == SessionKind::Acp => match provider.clone() {
                Some(id) => {
                    Self::reject_env_npx_wrapper(&id, provenance, &self.paths)?;
                    acp_client::resolve_named(&id, &self.paths)?
                }
                None => acp_client::resolve_command(&self.paths)?,
            },
            None => resolve_pty_command(&self.paths)?,
        };
        if let Some(cwd) = workspace_cwd {
            command.cwd = cwd;
        }
        let session_provider = match kind {
            SessionKind::Acp => provider.or_else(|| command.provider_id.clone()),
            SessionKind::Claude => Some("claude".to_string()),
            SessionKind::Terminal => None,
        };
        // One clock read: the journal row and the wire metadata must carry
        // the same instant so a caller can compare them.
        let mut record = new_session_record(
            id.clone(),
            owner.user.clone(),
            workspace_id.clone(),
            kind.clone(),
            match kind {
                SessionKind::Terminal => "Terminal",
                SessionKind::Acp | SessionKind::Claude => "Agent",
            }
            .to_string(),
        );
        record.provider = session_provider.clone();
        record.status = PersistStatus::Live;
        let record_generation = record.generation;
        let metadata = Session {
            id: id.clone(),
            workspace_id,
            cwd: Some(crate::workspace::display_path(
                &command.cwd.to_string_lossy(),
            )),
            kind: kind.clone(),
            title: record.title.clone(),
            provider: session_provider.clone(),
            peer_session_id: None,
            state: SessionState::Live { generation: 1 },
            elapsed_ms: Some(0),
            created_at_ms: record.created_at_ms,
        };
        crate::agent_env::inject_session_env(
            &mut command,
            &metadata.id,
            metadata.workspace_id.as_deref(),
            &self.paths,
        );
        // Journal the row BEFORE spawn. A short-lived command (cmd /c echo)
        // can EOF and enqueue MarkEnded before this function would otherwise
        // reach try_upsert, and the journal thread would then see a missing
        // session and leave status=live — recovered-as-killed on reopen.
        if let Some(journal) = &self.journal {
            journal.try_upsert(record);
            self.invalidate_journal_roster();
        }
        // The journal row above is the durable product boundary. A failed
        // spawn must end that row, or the next roster render resurrects a
        // phantom recovered session with zero events.
        match spawn_session(state, self, metadata.clone(), owner.clone(), command) {
            Ok(()) => {
                // A completed ACP handshake proves the provider started and
                // accepted a session, so it measures provider health. A
                // claude process spawn proves nothing about the provider,
                // so claude only records failures (below).
                if kind == SessionKind::Acp {
                    if let Some(provider_id) = &metadata.provider {
                        state.record_provider_health(provider_id, Ok(()));
                    }
                }
            }
            Err(error) => {
                if let Some(journal) = &self.journal {
                    // Trade, made deliberately: the end marker must not be
                    // silently lost (try_send drops on a saturated queue)
                    // and must not freeze this dispatch thread either — the
                    // blocking send is an unbounded 5 ms busy-loop with no
                    // timeout. A rare failure path affords a throwaway
                    // thread, and the row still ends once the queue drains,
                    // so the integration test's sessions_list deadline-poll
                    // stays valid.
                    let journal = Arc::clone(journal);
                    let id = metadata.id.clone();
                    let _ = std::thread::Builder::new()
                        .name("journal-end-marker".into())
                        .spawn(move || {
                            let _ = journal.mark_ended_blocking(&id, record_generation, None);
                        });
                }
                if let Some(provider_id) = &metadata.provider {
                    state.record_provider_health(provider_id, Err(&error));
                }
                return Err(error);
            }
        }
        Ok(metadata)
    }

    #[cfg(test)]
    pub fn attach(
        &self,
        session_id: &str,
        from_cursor: Option<Cursor>,
        conn: &ConnHandle,
        owner: &OwnerId,
        typed_permissions: bool,
    ) -> Result<(), WireError> {
        self.attach_with_subscription(
            session_id,
            conn.id,
            from_cursor,
            conn,
            owner,
            typed_permissions,
        )?;
        self.claim_resize_with_subscription(session_id, conn.id, owner, conn)
    }

    pub fn attach_with_subscription(
        &self,
        session_id: &str,
        subscription_id: u64,
        from_cursor: Option<Cursor>,
        conn: &ConnHandle,
        owner: &OwnerId,
        typed_permissions: bool,
    ) -> Result<(), WireError> {
        let runtime = match self.runtime_for_user(session_id, owner) {
            Ok(runtime) => runtime,
            Err(error) if error.code == ErrorCode::SessionNotFound => {
                self.hydrate_transcript(session_id, from_cursor, owner)?
            }
            Err(error) => return Err(error),
        };
        let outcome = runtime.try_attach_with_subscription(
            subscription_id,
            from_cursor,
            conn,
            typed_permissions,
        )?;
        // A terminal attach synchronises the screen (snapshot first, live
        // after). A transcript attach replays its journal. A live headless
        // agent needs the third contract: durable replay through a locked
        // watermark, then the live queue. Keeping these states explicit avoids
        // letting an agent's bounded backlog masquerade as history.
        let transcript = runtime.is_transcript();
        let transcript_cursor = if transcript {
            Some(from_cursor.map(|cursor| cursor.seq).unwrap_or(0))
        } else {
            None
        };
        if let Err(error) = conn.track_with_subscription(
            subscription_id,
            Arc::clone(&runtime),
            transcript,
            transcript_cursor,
            outcome.generation,
            outcome.live_agent_replay,
        ) {
            runtime.detach_subscription(conn.id, subscription_id);
            return Err(error);
        }
        // The journal writer records asynchronous failures in shared state;
        // attach must import that fact before returning even when the PTY is
        // otherwise quiet and no status request or later output occurs.
        runtime.refresh_journal_degradation();
        Ok(())
    }

    pub fn claim_resize_with_subscription(
        &self,
        session_id: &str,
        subscription_id: u64,
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<(), WireError> {
        let runtime = self.runtime_for_user(session_id, owner)?;
        runtime.claim_resize(conn.id, subscription_id)
    }

    pub fn resume(
        &self,
        state: &Arc<ServerState>,
        session_id: &str,
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<Session, WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let journal = self.journal.as_ref().ok_or_else(journal_unavailable)?;
        let record = journal
            .list()?
            .into_iter()
            .find(|record| record.id == session_id)
            .ok_or_else(not_found)?;
        let (provider, peer_session_id) = resume_handle(&record, owner)?;
        // The persisted provider is the original explicit provider choice.
        // In particular, a persisted npx wrapper is allowed through this
        // named path because its original create already supplied consent.
        let mut command = acp_client::resolve_named(&provider, &self.paths)?;
        self.apply_workspace_cwd(record.workspace_id.as_deref(), &mut command)?;
        let generation = record.generation.saturating_add(1);

        // A previous-run transcript is replaced. A stopped live entry is also
        // replaced, but only after it has been observed dead; resuming a still
        // live process would create two writers for one session id.
        let (old_entry, had_live_slot) = {
            let mut map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            if let Some(entry) = map.get(session_id) {
                check_user_owner(entry, owner)?;
                if entry
                    .as_live()
                    .is_some_and(|session| !session.runtime.process_exited())
                {
                    return Err(WireError::new(
                        ErrorCode::InvalidRequest,
                        "This session cannot be resumed while its process is running.",
                    ));
                }
            }
            let old_entry = map.remove(session_id);
            let had_live_slot = matches!(old_entry, Some(RegistryEntry::Live(_)));
            (old_entry, had_live_slot)
        };
        if let Some(old_entry) = old_entry {
            match old_entry {
                RegistryEntry::Live(session) => {
                    session.runtime.detach_if_conn(conn.id);
                    session.runtime.notify_generation_replaced(conn.id);
                    teardown_session_for_resume(session);
                }
                RegistryEntry::Transcript(session) => {
                    session.runtime.detach_if_conn(conn.id);
                    session.runtime.notify_generation_replaced(conn.id);
                    journal.unpin(session_id);
                }
            }
        }
        conn.untrack_session(session_id);

        if !had_live_slot && !state.session_started() {
            return Err(WireError::new(
                ErrorCode::ShuttingDown,
                "daemon is shutting down",
            ));
        }
        if let Err(error) = journal.start_generation(session_id, generation) {
            state.session_finished();
            return Err(error.into());
        }
        self.invalidate_journal_roster();
        // Health is measured per provider id; `provider` is moved into the
        // metadata below, so keep a copy for the spawn outcome recording.
        let health_provider = provider.clone();
        // Resume does not create a session: echo the journal's original
        // created_at_ms. Re-stamping now would break the staleness check
        // this field exists for.
        let metadata = session_metadata_for_resume(
            session_id,
            record,
            &command,
            provider,
            peer_session_id.clone(),
            generation,
        );
        match spawn_resumed_session(
            state,
            self,
            metadata,
            owner.clone(),
            command,
            peer_session_id,
            generation,
        ) {
            Ok(()) => state.record_provider_health(&health_provider, Ok(())),
            Err(error) => {
                state.session_finished();
                // The generation was already started on the journal row; a
                // failed respawn must end it, or the row stays live and the
                // roster renders a phantom recovered session. The end marker
                // must not be silently lost (try_send drops on a saturated
                // queue) and must not freeze this dispatch thread (the
                // blocking send is an unbounded 5 ms busy-loop with no
                // timeout), so this rare failure path gets a throwaway
                // thread; the row still ends once the queue drains.
                let journal = Arc::clone(journal);
                let id = session_id.to_string();
                let _ = std::thread::Builder::new()
                    .name("journal-end-marker".into())
                    .spawn(move || {
                        let _ = journal.mark_ended_blocking(&id, generation, None);
                    });
                state.record_provider_health(&health_provider, Err(&error));
                return Err(error);
            }
        }
        let map = self
            .inner
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        map.get(session_id)
            .map(RegistryEntry::to_session)
            .ok_or_else(|| internal("resumed session was not registered"))
    }

    fn hydrate_transcript(
        &self,
        session_id: &str,
        from_cursor: Option<Cursor>,
        owner: &OwnerId,
    ) -> Result<Arc<SessionRuntime>, WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let journal = self.journal.as_ref().ok_or_else(not_found)?;
        let record = journal
            .list()?
            .into_iter()
            .find(|row| row.id == session_id)
            .ok_or_else(not_found)?;
        let session_owner = owner_from_session_id(session_id, &record.owner)?;
        if session_owner.user != owner.user {
            return Err(unauthorized());
        }
        journal.pin(session_id)?;
        let from_seq = from_cursor.map(|cursor| cursor.seq).unwrap_or(0);
        let replay = match journal.replay(session_id, from_seq) {
            Ok(replay) => replay,
            Err(error) => {
                journal.unpin(session_id);
                return Err(error.into());
            }
        };
        if let Some(cursor) = from_cursor {
            if let Err(error) = cursor_replay_ok(replay.generation, cursor) {
                journal.unpin(session_id);
                return Err(error);
            }
        }
        let metadata = record.to_session();
        let runtime =
            SessionRuntime::from_replay(session_id.to_string(), Some(Arc::clone(journal)), replay);
        if let Some(peer_session_id) = record.peer_session_id.clone() {
            runtime.restore_peer_session_id(peer_session_id);
        }
        {
            let Ok(mut map) = self.inner.lock() else {
                journal.unpin(session_id);
                return Err(internal("Session state is unavailable."));
            };
            if let Some(existing) = map.get(session_id) {
                check_user_owner(existing, owner)?;
                journal.unpin(session_id);
                return Ok(existing.runtime());
            }
            map.insert(
                session_id.to_string(),
                RegistryEntry::Transcript(TranscriptSession {
                    metadata,
                    owner: session_owner,
                    runtime: Arc::clone(&runtime),
                }),
            );
        }
        Ok(runtime)
    }

    #[cfg(test)]
    pub fn detach(
        &self,
        session_id: &str,
        conn: &ConnHandle,
        owner: &OwnerId,
    ) -> Result<(), WireError> {
        self.detach_with_subscription(session_id, conn.id, conn, owner)
    }

    pub fn detach_with_subscription(
        &self,
        session_id: &str,
        subscription_id: u64,
        conn: &ConnHandle,
        owner: &OwnerId,
    ) -> Result<(), WireError> {
        let runtime = self.runtime_for_user(session_id, owner)?;
        runtime.detach_subscription(conn.id, subscription_id);
        conn.untrack_subscription(subscription_id);
        self.drop_transcript_if_idle(session_id);
        Ok(())
    }

    #[cfg(test)]
    pub fn permission_respond(
        &self,
        session_id: &str,
        request_id: &str,
        outcome: PermissionOutcome,
        conn: &ConnHandle,
        owner: &OwnerId,
    ) -> Result<(), WireError> {
        self.permission_respond_with_subscription(
            session_id, request_id, outcome, conn.id, conn, owner,
        )
    }

    pub fn permission_respond_with_subscription(
        &self,
        session_id: &str,
        request_id: &str,
        outcome: PermissionOutcome,
        subscription_id: u64,
        conn: &ConnHandle,
        owner: &OwnerId,
    ) -> Result<(), WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        if request_id.is_empty() {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "Permission request id is required.",
            ));
        }
        let runtime = self.runtime_for_user(session_id, owner)?;
        check_attached(&runtime, conn, subscription_id)?;
        let broker = runtime.permission_broker().ok_or_else(|| {
            WireError::new(
                ErrorCode::InvalidRequest,
                "Session has no live ACP permission broker.",
            )
        })?;
        broker.respond(request_id, outcome).map_err(|error| {
            let code = match error {
                permission_broker::PermissionResponseError::NotFound => ErrorCode::InvalidRequest,
                permission_broker::PermissionResponseError::InvalidRequest(_) => {
                    ErrorCode::InvalidRequest
                }
                permission_broker::PermissionResponseError::Io(_) => ErrorCode::Io,
            };
            WireError::new(code, error.to_string())
        })?;
        if runtime.clear_attention() {
            self.notify_session_transition(owner, session_id);
        }
        Ok(())
    }

    #[cfg(test)]
    pub fn stop(&self, session_id: &str, owner: &OwnerId) -> Result<(), WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let mut killer = {
            let mut map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            let session = map.get_mut(session_id).ok_or_else(not_found)?;
            check_user_owner(session, owner)?;
            let session = session.as_live_mut().ok_or_else(process_gone)?;
            session.preserve_on_exit.store(true, Ordering::SeqCst);
            session.killer.clone_killer()
        };
        killer.kill();
        Ok(())
    }

    pub fn stop_with_subscription(
        &self,
        session_id: &str,
        subscription_id: u64,
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<(), WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let (mut killer, runtime) = {
            let mut map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            let session = map.get_mut(session_id).ok_or_else(not_found)?;
            check_user_owner(session, owner)?;
            let session = session.as_live_mut().ok_or_else(process_gone)?;
            (session.killer.clone_killer(), Arc::clone(&session.runtime))
        };
        check_attached(&runtime, conn, subscription_id)?;
        {
            let mut map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            if let Some(session) = map.get_mut(session_id).and_then(RegistryEntry::as_live_mut) {
                session.preserve_on_exit.store(true, Ordering::SeqCst);
            }
        }
        killer.kill();
        Ok(())
    }

    /// Drop every subscription this connection holds. The processes stay.
    pub fn detach_conn(&self, conn: &ConnHandle) {
        let ids = conn.take_attached_ids();
        for (subscription_id, session_id) in ids {
            self.detach_runtime(&session_id, conn.id, subscription_id);
            self.drop_transcript_if_idle(&session_id);
        }
    }

    pub(crate) fn subscription_event_sent(&self, session_id: &str) {
        self.drop_transcript_if_idle(session_id);
    }

    fn detach_runtime(&self, session_id: &str, conn_id: u64, subscription_id: u64) {
        if let Ok(runtime) = self.runtime(session_id) {
            runtime.detach_subscription(conn_id, subscription_id);
        }
    }

    fn drop_transcript_if_idle(&self, session_id: &str) {
        let Ok(mut map) = self.inner.lock() else {
            return;
        };
        let is_idle_transcript = map.get(session_id).is_some_and(|entry| {
            matches!(entry, RegistryEntry::Transcript(session) if {
                session
                    .runtime
                    .stream
                    .lock()
                    .map(|stream| stream.observers.is_empty())
                    .unwrap_or(true)
            })
        });
        if is_idle_transcript {
            map.remove(session_id);
            if let Some(journal) = &self.journal {
                journal.unpin(session_id);
            }
        }
    }

    pub fn close(&self, session_id: &str, owner: &OwnerId) -> Result<bool, WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let session = {
            let mut map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            if let Some(entry) = map.get(session_id) {
                check_user_owner(entry, owner)?;
                if let Some(session) = entry.as_live() {
                    session
                        .runtime
                        .transition_ready
                        .store(false, Ordering::Release);
                }
            }
            map.remove(session_id)
        };
        match session {
            Some(RegistryEntry::Live(session)) => {
                if let Some(journal) = &self.journal {
                    journal.try_mark_closed(session_id);
                    journal.unpin(session_id);
                    self.invalidate_journal_roster();
                }
                teardown_session(session);
                self.notify_session_transition(owner, session_id);
                Ok(true)
            }
            Some(RegistryEntry::Transcript(_)) => {
                if let Some(journal) = &self.journal {
                    journal.try_mark_closed(session_id);
                    journal.unpin(session_id);
                    self.invalidate_journal_roster();
                }
                self.notify_session_transition(owner, session_id);
                Ok(false)
            }
            None => {
                if let Some(journal) = &self.journal {
                    let known = journal.list()?.into_iter().find(|row| row.id == session_id);
                    if let Some(record) = known {
                        let session_owner = owner_from_session_id(session_id, &record.owner)?;
                        if session_owner.user != owner.user {
                            return Err(unauthorized());
                        }
                        journal.try_mark_closed(session_id);
                        self.invalidate_journal_roster();
                        self.notify_session_transition(owner, session_id);
                        return Ok(false);
                    }
                }
                Err(not_found())
            }
        }
    }

    /// Interrupt the current turn of an agent session without killing the
    /// process. Unlike `stop`, the registry entry stays live and later
    /// turns keep working.
    pub fn interrupt_with_subscription(
        &self,
        session_id: &str,
        subscription_id: u64,
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<(), WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let (mut killer, runtime) = {
            let mut map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            let entry = map.get_mut(session_id).ok_or_else(not_found)?;
            check_user_owner(entry, owner)?;
            let session = entry.as_live_mut().ok_or_else(process_gone)?;
            if !session.metadata.kind.is_agent() {
                return Err(WireError::new(
                    ErrorCode::InvalidRequest,
                    "Only agent sessions support interrupting a turn.",
                ));
            }
            (session.killer.clone_killer(), Arc::clone(&session.runtime))
        };
        check_attached(&runtime, conn, subscription_id)?;
        killer.interrupt();
        Ok(())
    }

    pub fn set_model(
        &self,
        session_id: &str,
        owner: &OwnerId,
        model_id: Option<&str>,
        effort: Option<&str>,
    ) -> Result<(), WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        if model_id.is_none() && effort.is_none() {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "A model or effort is required.",
            ));
        }
        let (switcher, kind, runtime) = {
            let mut map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            let entry = map.get_mut(session_id).ok_or_else(not_found)?;
            check_user_owner(entry, owner)?;
            let session = entry.as_live_mut().ok_or_else(process_gone)?;
            if !session.metadata.kind.is_agent() {
                return Err(WireError::new(
                    ErrorCode::InvalidRequest,
                    "Only agent sessions support switching the model or effort.",
                ));
            }
            let switcher = session
                .switcher
                .as_ref()
                .map(|switcher| switcher.clone_switcher())
                .ok_or_else(|| {
                    WireError::new(
                        ErrorCode::InvalidRequest,
                        "This provider does not support switching the model or effort.",
                    )
                })?;
            (
                switcher,
                session.metadata.kind.clone(),
                Arc::clone(&session.runtime),
            )
        };
        if kind == SessionKind::Claude {
            Self::validate_claude_effort(
                runtime.session_manifest().as_ref(),
                runtime.claude_catalog_state(),
                model_id,
                effort,
            )?;
        }
        switcher.set_model(model_id, effort)
    }

    fn validate_claude_effort(
        manifest: Option<&SessionEvent>,
        catalog_state: crate::claude_catalog::ClaudeCatalogState,
        model_id: Option<&str>,
        effort: Option<&str>,
    ) -> Result<(), WireError> {
        if model_id.is_none() && effort.is_none() {
            return Ok(());
        }
        if catalog_state == crate::claude_catalog::ClaudeCatalogState::Provisional {
            return Ok(());
        }
        let Some(SessionEvent::SessionManifest {
            current_model_id,
            models,
            ..
        }) = manifest
        else {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "Claude has not published its model catalog yet.",
            ));
        };
        let model_id = model_id
            .filter(|model_id| !model_id.is_empty())
            .or(current_model_id.as_deref())
            .ok_or_else(|| {
                WireError::new(
                    ErrorCode::InvalidRequest,
                    "Claude has not reported a current model yet.",
                )
            })?;
        let model = models
            .iter()
            .find(|model| crate::claude_catalog::model_ids_match(&model.model_id, model_id))
            .ok_or_else(|| {
                WireError::new(
                    ErrorCode::InvalidRequest,
                    format!("Claude model '{model_id}' is not in the current catalog."),
                )
            })?;
        let Some(effort) = effort else {
            return Ok(());
        };
        let valid = model
            .efforts
            .as_ref()
            .is_some_and(|efforts| efforts.iter().any(|entry| entry.id == effort));
        if valid {
            Ok(())
        } else {
            Err(WireError::new(
                ErrorCode::InvalidRequest,
                format!("Effort '{effort}' is not supported by Claude model '{model_id}'."),
            ))
        }
    }

    #[cfg(test)]
    pub fn send(
        &self,
        session_id: &str,
        text: &str,
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<(), WireError> {
        self.send_with_subscription(session_id, conn.id, text, owner, conn)
    }

    pub fn send_with_subscription(
        &self,
        session_id: &str,
        subscription_id: u64,
        text: &str,
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<(), WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        if text.len() > MAX_WRITE_BYTES {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "Session input is too large.",
            ));
        }
        let (writer, runtime, is_agent) = {
            let map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            let entry = map.get(session_id).ok_or_else(not_found)?;
            check_user_owner(entry, owner)?;
            let session = entry.as_live().ok_or_else(process_gone)?;
            (
                Arc::clone(&session.writer),
                Arc::clone(&session.runtime),
                session.metadata.kind.is_agent(),
            )
        };
        check_attached(&runtime, conn, subscription_id)?;
        let agent_runtime = is_agent.then_some(runtime);
        if let Some(runtime) = agent_runtime.as_ref() {
            if !text.is_empty() && !runtime.can_publish_agent_user_message() {
                return Err(internal("Agent input could not be recorded."));
            }
        }
        // Keep the complete write and its transcript event under this lock so
        // the journal preserves the same order the process receives.
        let mut writer = match writer.lock() {
            Ok(writer) => writer,
            Err(_) => {
                let error = internal("Session state is unavailable.");
                if let Some(runtime) = agent_runtime.as_ref() {
                    runtime.publish_agent_error(error.message.clone());
                }
                return Err(error);
            }
        };
        if let Err(error) = writer.write_all(text.as_bytes()).map_err(|error| {
            WireError::new(
                ErrorCode::Io,
                format!("Could not send input to the terminal: {error}"),
            )
        }) {
            drop(writer);
            if let Some(runtime) = agent_runtime.as_ref() {
                runtime.publish_agent_error(error.message.clone());
            }
            return Err(error);
        }
        if let Err(error) = writer.flush().map_err(|error| {
            WireError::new(
                ErrorCode::Io,
                format!("Could not flush input to the terminal: {error}"),
            )
        }) {
            drop(writer);
            if let Some(runtime) = agent_runtime.as_ref() {
                runtime.publish_agent_error(error.message.clone());
            }
            return Err(error);
        }
        if !text.is_empty() {
            if let Some(runtime) = agent_runtime.as_ref() {
                if !runtime.publish_agent_user_message(text.to_string()) {
                    return Err(internal("Agent input could not be recorded."));
                }
                if runtime.clear_attention() {
                    self.notify_session_transition(owner, session_id);
                }
            }
        }
        drop(writer);
        Ok(())
    }

    pub fn report_agent(
        &self,
        session_id: &str,
        report: crate::agent_report::AgentReport,
        peer: Option<&crate::agent_report::PeerIdentity>,
    ) -> Result<bool, WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        crate::agent_report::validate_announcement(&report)?;
        #[cfg(not(windows))]
        {
            let _ = peer;
            return Err(crate::agent_report::peer_identity_unavailable_on_platform());
        }
        let runtime = {
            let map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            let entry = map.get(session_id).ok_or_else(not_found)?;
            let live = entry.as_live().ok_or_else(process_gone)?;
            #[cfg(windows)]
            {
                let daemon_sid = crate::security::current_user_sid().map_err(|error| {
                    crate::agent_report::unauthorized_peer(format!(
                        "Could not verify the announcing process identity: {error}"
                    ))
                })?;
                crate::agent_report::verify_announcement_peer(peer, &daemon_sid)?;
                crate::agent_report::verify_announcement_peer(peer, &live.owner.user)?;
            }
            Arc::clone(&live.runtime)
        };
        runtime.accept_agent_report(report)
    }

    #[cfg(test)]
    pub fn resize(
        &self,
        session_id: &str,
        cols: u16,
        rows: u16,
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<(), WireError> {
        self.resize_with_subscription(session_id, conn.id, cols, rows, owner, conn)
    }

    pub fn resize_with_subscription(
        &self,
        session_id: &str,
        subscription_id: u64,
        cols: u16,
        rows: u16,
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<(), WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let (runtime, master) = {
            let map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            let entry = map.get(session_id).ok_or_else(not_found)?;
            check_user_owner(entry, owner)?;
            let session = entry.as_live().ok_or_else(process_gone)?;
            (Arc::clone(&session.runtime), session.master.clone())
        };
        check_resize_owner(&runtime, conn, subscription_id)?;
        // Resize is serialized with emulator parsing under the SAME state
        // lock as publish_output, in one defined order: emulator dimensions
        // first, then the PTY. A snapshot therefore sees the resize as wholly
        // before or wholly after itself, and no chunk is parsed into a grid
        // that is mid-resize.
        let mut stream = runtime
            .stream
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        let Some(screen) = stream.screen.as_mut() else {
            // ACP sessions are structured streams and deliberately have no
            // terminal dimensions. Resize is already kind-agnostic at the
            // RPC seam; it is simply a no-op for this transport.
            return Ok(());
        };
        let (previous_cols, previous_rows) = screen.dimensions();
        screen.resize(cols.max(1), rows.max(1));
        let Some(master) = master else {
            return Ok(());
        };
        let master = master
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        if master
            .resize(PtySize {
                rows: rows.max(1),
                cols: cols.max(1),
                pixel_width: 0,
                pixel_height: 0,
            })
            .is_err()
        {
            // Keep emulator and PTY in agreement: undo the grid change.
            screen.resize(previous_cols, previous_rows);
            return Err(WireError::new(
                ErrorCode::Io,
                "Could not resize the terminal.",
            ));
        }
        Ok(())
    }

    pub fn list(&self, owner: &OwnerId) -> Result<Vec<Session>, WireError> {
        let map = self
            .inner
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        let mut sessions: Vec<Session> = map
            .values()
            .filter(|entry| entry.owner().user == owner.user)
            .map(RegistryEntry::to_session)
            .collect();
        drop(map);
        if let Some(journal) = &self.journal {
            if let Ok(rows) = journal.list() {
                for row in rows {
                    if row.owner != owner.user {
                        continue;
                    }
                    if sessions.iter().any(|session| session.id == row.id) {
                        continue;
                    }
                    sessions.push(row.to_session());
                }
            }
        }
        sessions.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(sessions)
    }

    /// Pull asynchronous journal-loop failures into live runtimes so their
    /// attached event channels can report degradation even when the PTY has
    /// gone quiet since the failed write.
    pub fn refresh_journal_degradation(&self) {
        let runtimes = self
            .inner
            .lock()
            .map(|map| map.values().map(RegistryEntry::runtime).collect::<Vec<_>>())
            .unwrap_or_default();
        for runtime in runtimes {
            runtime.refresh_journal_degradation();
        }
    }

    pub fn has_live_journal_degradation(&self) -> bool {
        self.inner
            .lock()
            .map(|map| map.values().any(|entry| entry.runtime().journal_degraded()))
            .unwrap_or(true)
    }

    pub(crate) fn publish_claude_catalog(&self, models: Vec<SessionModel>) {
        let runtimes = self
            .inner
            .lock()
            .map(|map| {
                map.values()
                    .filter_map(|entry| {
                        let session = entry.as_live()?;
                        (session.metadata.kind == SessionKind::Claude)
                            .then(|| Arc::clone(&session.runtime))
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for runtime in runtimes {
            let manifest = runtime.store_claude_catalog(
                crate::claude_catalog::manifest_with_current(models.clone(), None),
            );
            runtime.publish_agent_event(manifest, None);
        }
    }

    fn runtime(&self, session_id: &str) -> Result<Arc<SessionRuntime>, WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let map = self
            .inner
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        let session = map.get(session_id).ok_or_else(not_found)?;
        Ok(session.runtime())
    }

    fn runtime_for_user(
        &self,
        session_id: &str,
        owner: &OwnerId,
    ) -> Result<Arc<SessionRuntime>, WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let map = self
            .inner
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        let session = map.get(session_id).ok_or_else(not_found)?;
        check_user_owner(session, owner)?;
        Ok(session.runtime())
    }
}

fn spawn_os_liveness_sweeper(registry: &SessionRegistry) {
    let inner = Arc::downgrade(&registry.inner);
    let sink = Arc::downgrade(&registry.transition_sink);
    if let Err(error) = std::thread::Builder::new()
        .name("session-os-liveness".to_string())
        .spawn(move || loop {
            std::thread::sleep(SESSION_OS_SWEEP_INTERVAL);
            let Some(inner) = inner.upgrade() else {
                return;
            };
            let Some(sink) = sink.upgrade() else {
                return;
            };
            sweep_os_liveness(&inner, &sink);
        })
    {
        eprintln!("could not start OS liveness sweeper: {error}");
    }
}

fn sweep_os_liveness(
    inner: &Mutex<HashMap<String, RegistryEntry>>,
    sink: &Mutex<Option<TransitionSink>>,
) {
    let Ok(map) = inner.lock() else {
        return;
    };
    let work: Vec<(Arc<SessionRuntime>, OwnerId)> = map
        .values()
        .filter_map(|entry| {
            let session = entry.as_live()?;
            Some((Arc::clone(&session.runtime), session.owner.clone()))
        })
        .collect();
    drop(map);
    for (runtime, owner) in work {
        let newly_dead = runtime.observe_os_liveness();
        if newly_dead {
            runtime.fire_os_death();
        }
        let notify = if newly_dead || runtime.process_exited() {
            runtime.should_publish_exit_transition()
        } else {
            runtime.mark_silent_if_due(Instant::now()).is_some() && runtime.transition_ready()
        };
        if !notify {
            continue;
        }
        let callback = sink.lock().ok().and_then(|guard| guard.clone());
        if let Some(callback) = callback {
            callback(owner);
        }
    }
}

pub fn spawn_session(
    state: &Arc<ServerState>,
    registry: &SessionRegistry,
    metadata: Session,
    owner: OwnerId,
    command: PtyCommand,
) -> Result<(), WireError> {
    if metadata.kind == SessionKind::Claude {
        let workspace_id = metadata.workspace_id.clone();
        let workspace_path = command.cwd.clone();
        let spawned = claude_client::spawn_process(state, command).map_err(|error| {
            map_workspace_spawn_wire_error(workspace_id.as_deref(), &workspace_path, error)
        })?;
        return start_spawned_session(state, registry, metadata, owner, None, spawned);
    }
    if metadata.kind == SessionKind::Acp {
        let workspace_id = metadata.workspace_id.clone();
        let workspace_path = command.cwd.clone();
        let spawned = acp_client::spawn_process(state, command).map_err(|error| {
            map_workspace_spawn_wire_error(workspace_id.as_deref(), &workspace_path, error)
        })?;
        return start_spawned_session(state, registry, metadata, owner, None, spawned);
    }

    // On Windows portable-pty selects ConPTY internally. ConPTY may issue a
    // DSR query (`ESC[6n`) at startup and stalls its render pipeline until it
    // is answered. The DAEMON is the single responder: publish_output routes
    // the emulator's PtyWrite replies straight back to this writer. Clients
    // must not answer DSR themselves (a second reply would reach the child).
    let pty_system = portable_pty::native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: INITIAL_ROWS,
            cols: INITIAL_COLS,
            pixel_width: 0,
            pixel_height: 0,
        })
        .map_err(|error| pty_wire_error("Could not open the terminal.", error))?;
    let workspace_id = metadata.workspace_id.clone();
    let workspace_path = command.cwd.clone();
    let mut child = pair
        .slave
        .spawn_command(command.to_command_builder())
        .map_err(|error| workspace_spawn_error(workspace_id.as_deref(), &workspace_path, error))?;

    // portable-pty 0.9 exposes the native Windows process handle on Child,
    // but does not expose CREATE_SUSPENDED. Assign immediately after spawn so
    // the normal race window is only the interval between CreateProcessW and
    // these calls. Closing it completely would require adapting portable-pty's
    // ConPTY CreateProcessW seam to create suspended and resume after both
    // assignments; that is deliberately not part of this milestone.
    #[cfg(windows)]
    let (process_job, os_handle) = {
        let process_job = match JobObject::new() {
            Ok(process_job) => process_job,
            Err(error) => {
                terminate_spawned_child(pair, child);
                return Err(WireError::new(
                    ErrorCode::Io,
                    format!("Could not create the terminal process job: {error}"),
                ));
            }
        };
        let process_handle = match child.as_raw_handle() {
            Some(process_handle) => process_handle,
            None => {
                terminate_spawned_child(pair, child);
                return Err(WireError::new(
                    ErrorCode::Io,
                    "The terminal process has no native handle.",
                ));
            }
        };
        if let Err(error) = state
            .process_job
            .assign(process_handle)
            .and_then(|()| process_job.assign(process_handle))
        {
            terminate_spawned_child(pair, child);
            return Err(WireError::new(
                ErrorCode::Io,
                format!("Could not contain the terminal process: {error}"),
            ));
        }
        let os_handle = match ProcessHandle::duplicate(process_handle) {
            Ok(handle) => Some(handle),
            Err(error) => {
                eprintln!("could not duplicate terminal process handle for OS liveness: {error}");
                None
            }
        };
        (process_job, os_handle)
    };

    #[cfg(not(windows))]
    let process_job = JobObject::new().map_err(|error| {
        WireError::new(
            ErrorCode::Io,
            format!("Could not create the terminal process job: {error}"),
        )
    })?;
    #[cfg(not(windows))]
    let os_handle = None;

    let killer = child.clone_killer();

    let writer = match pair.master.take_writer() {
        Ok(writer) => writer,
        Err(_) => {
            let mut killer = killer;
            let _ = killer.kill();
            drop(pair.master);
            let _ = child.wait();
            return Err(WireError::new(
                ErrorCode::Io,
                "Could not attach to the terminal.",
            ));
        }
    };
    let reader = match pair.master.try_clone_reader() {
        Ok(reader) => reader,
        Err(_) => {
            let mut killer = killer;
            let _ = killer.kill();
            drop(writer);
            drop(pair.master);
            let _ = child.wait();
            return Err(WireError::new(
                ErrorCode::Io,
                "Could not read from the terminal.",
            ));
        }
    };

    let spawned = SpawnedSession {
        process_job,
        master: Some(Arc::new(Mutex::new(pair.master))),
        killer: Box::new(PtyKiller { inner: killer }),
        switcher: None,
        child: Box::new(PtyWaitableChild { child }),
        writer: Arc::new(Mutex::new(writer)),
        reader,
        reader_dispatch: None,
        stderr: None,
        permission_broker: None,
        os_handle,
        peer_session_id: None,
        agent_version: None,
    };
    start_spawned_session(state, registry, metadata, owner, None, spawned)
}

pub fn spawn_resumed_session(
    state: &Arc<ServerState>,
    registry: &SessionRegistry,
    metadata: Session,
    owner: OwnerId,
    command: PtyCommand,
    peer_session_id: String,
    generation: u64,
) -> Result<(), WireError> {
    start_spawned_session(
        state,
        registry,
        metadata,
        owner,
        Some(generation),
        acp_client::spawn_process_resuming(state, command, peer_session_id)?,
    )
}

fn start_spawned_session(
    state: &Arc<ServerState>,
    registry: &SessionRegistry,
    metadata: Session,
    owner: OwnerId,
    generation: Option<u64>,
    spawned: SpawnedSession,
) -> Result<(), WireError> {
    let SpawnedSession {
        process_job,
        master,
        killer,
        switcher,
        child,
        writer,
        reader,
        reader_dispatch,
        stderr,
        permission_broker,
        os_handle,
        peer_session_id,
        agent_version,
    } = spawned;
    if let (Some(provider_id), Some(version)) = (&metadata.provider, agent_version.as_deref()) {
        state.record_provider_version(provider_id, version);
    }
    let runtime = if metadata.kind.is_agent() {
        SessionRuntime::for_acp(
            metadata.id.clone(),
            registry.journal.clone(),
            permission_broker.expect("agent sessions have a permission broker"),
        )
    } else {
        Arc::new(SessionRuntime::with_journal(
            metadata.id.clone(),
            registry.journal.clone(),
        ))
    };
    if let Some(peer_session_id) = peer_session_id {
        runtime.set_peer_session_id(peer_session_id);
    }
    if let Some(generation) = generation {
        runtime.set_generation(generation);
    }
    if metadata.kind == SessionKind::Claude {
        let catalog = state.claude_models();
        runtime.store_claude_manifest(
            crate::claude_catalog::initial_manifest(catalog.models),
            catalog.state,
        );
    }
    if let Some(handle) = os_handle {
        runtime.install_os_handle(handle);
    }
    let process_job = Arc::new(process_job);
    {
        let registry = registry.clone();
        let owner = owner.clone();
        let session_id = metadata.id.clone();
        runtime.set_roster_notify(Arc::new(move || {
            registry.notify_session_transition(&owner, &session_id);
        }));
    }
    registry.configure_runtime_attention(&runtime, &owner);
    if metadata.kind.is_agent() {
        let death_killer = Mutex::new(killer.clone_killer());
        let job = Arc::clone(&process_job);
        runtime.set_on_os_death(Arc::new(move || {
            if let Ok(mut killer) = death_killer.lock() {
                killer.kill();
            }
            let _ = job.terminate();
        }));
    }
    let exited = Arc::new(AtomicBool::new(false));
    // Register before the reader thread starts: ConPTY's startup DSR can be
    // read within milliseconds, and the reply path needs the writer.
    if metadata.kind == SessionKind::Terminal {
        runtime
            .pty_writer
            .set(Arc::clone(&writer))
            .ok()
            .expect("pty writer registered exactly once");
    }
    let id = metadata.id.clone();
    let wait_id = id.clone();
    let wait_runtime = Arc::clone(&runtime);
    let wait_registry = registry.clone();
    let wait_owner = owner.clone();
    let child_wait = std::thread::Builder::new()
        .name(format!("session-wait-{id}"))
        .spawn(move || {
            let code = child.wait();
            wait_runtime
                .child_reaped
                .store(code.is_some(), Ordering::Release);
            wait_runtime.mark_exited(code);
            if wait_runtime.should_publish_exit_transition() {
                wait_registry.notify_session_transition(&wait_owner, &wait_id);
            }
            code
        })
        .ok();
    let session = PtySession {
        metadata,
        owner: owner.clone(),
        process_job,
        master,
        killer,
        switcher,
        child_wait,
        writer,
        reader_handle: None,
        coalesce_handle: None,
        stderr_handle: None,
        runtime: Arc::clone(&runtime),
        exited: Arc::clone(&exited),
        preserve_on_exit: Arc::new(AtomicBool::new(false)),
    };

    // Insert BEFORE starting the reader. A shell can exit before the reader
    // thread gets scheduled; inserting later would let EOF cleanup miss the
    // map entry and strand the session.
    {
        let Ok(mut map) = registry.inner.lock() else {
            teardown_session(session);
            return Err(internal("Session state is unavailable."));
        };
        map.insert(id.clone(), RegistryEntry::Live(session));
    }

    let (coalesce_handle, reader_dispatch) = match reader_dispatch {
        Some(dispatch) => (None, dispatch),
        None => {
            let (coalesce_tx, coalesce_rx) = mpsc::channel::<Vec<u8>>();
            let coalesce_runtime = Arc::clone(&runtime);
            let coalesce_registry = registry.clone();
            let coalesce_session_id = id.clone();
            let coalesce_owner = owner.clone();
            let coalesce_handle = match std::thread::Builder::new()
                .name(format!("session-coalesce-{id}"))
                .spawn(move || {
                    coalesce_loop(
                        coalesce_rx,
                        coalesce_runtime,
                        coalesce_registry,
                        coalesce_session_id,
                        coalesce_owner,
                    )
                }) {
                Ok(handle) => Some(handle),
                Err(_) => {
                    let _ = registry.close(&id, &owner);
                    return Err(WireError::new(
                        ErrorCode::Internal,
                        "Could not start the terminal reader.",
                    ));
                }
            };
            (
                coalesce_handle,
                Box::new(TerminalReaderDispatch {
                    tx: Some(coalesce_tx),
                }) as Box<dyn ReaderDispatch>,
            )
        }
    };

    let stderr_handle = stderr.and_then(|source| match source.spawn(Arc::clone(&runtime)) {
        Ok(handle) => Some(handle),
        Err(error) => {
            runtime.publish_agent_event(
                SessionEvent::AgentError {
                    message: format!("Could not drain agent stderr: {error}"),
                },
                None,
            );
            None
        }
    });
    if let Ok(mut map) = registry.inner.lock() {
        if let Some(session) = map.get_mut(&id).and_then(RegistryEntry::as_live_mut) {
            session.coalesce_handle = coalesce_handle;
            session.stderr_handle = stderr_handle;
        }
    }

    let reader_registry = registry.clone();
    let reader_id = id.clone();
    let reader_runtime = Arc::clone(&runtime);
    let reader_state = Arc::downgrade(state);
    let reader_handle = match std::thread::Builder::new()
        .name(format!("session-pty-{id}"))
        .spawn(move || {
            reader_loop(
                reader_registry,
                reader_state,
                reader_id,
                reader,
                reader_runtime,
                reader_dispatch,
            );
        }) {
        Ok(handle) => handle,
        Err(_) => {
            let _ = registry.close(&id, &owner);
            return Err(WireError::new(
                ErrorCode::Internal,
                "Could not start the terminal reader.",
            ));
        }
    };

    // The child can exit before this lock is acquired. In that case EOF
    // cleanup already removed the session; join the now-finished reader
    // here instead of leaking its handle.
    let mut orphaned_reader = Some(reader_handle);
    let mut orphaned_coalesce = None;
    if let Ok(mut map) = registry.inner.lock() {
        if let Some(session) = map.get_mut(&id).and_then(RegistryEntry::as_live_mut) {
            session.reader_handle = orphaned_reader.take();
            session.coalesce_handle = orphaned_coalesce.take();
        }
    }
    if let Some(reader_handle) = orphaned_reader {
        let _ = reader_handle.join();
    }
    if let Some(coalesce_handle) = orphaned_coalesce {
        let _ = coalesce_handle.join();
    }
    // A child can die before the create transition is published. Mark that
    // exit as covered by this first snapshot; the second check catches an
    // exit racing the publication without allowing the wait thread to report
    // the same transition twice.
    if runtime.process_exited() {
        runtime.exit_transition_sent.store(true, Ordering::Release);
    }
    registry.notify_session_transition(&owner, &id);
    runtime.transition_ready.store(true, Ordering::Release);
    if runtime.process_exited() && runtime.should_publish_exit_transition() {
        registry.notify_session_transition(&owner, &id);
    }
    Ok(())
}

fn reader_loop(
    registry: SessionRegistry,
    state: Weak<ServerState>,
    id: String,
    mut reader: Box<dyn Read + Send>,
    runtime: Arc<SessionRuntime>,
    mut reader_dispatch: Box<dyn ReaderDispatch>,
) {
    let mut buf = [0u8; READ_CHUNK];
    if let Err(error) = reader_dispatch.feed(&[], &runtime) {
        runtime.record_output_loss();
        eprintln!("session {id} stopped before the first child read: {error}");
        reader_dispatch.finish(&runtime);
        return;
    }
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if let Err(error) = reader_dispatch.feed(&buf[..n], &runtime) {
                    runtime.record_output_loss();
                    eprintln!("session {id} stopped reading child output: {error}");
                    break;
                }
            }
            Err(ref error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => {
                runtime.record_output_loss();
                eprintln!("session {id} stopped reading terminal output: {error}");
                break;
            }
        }
    }
    reader_dispatch.finish(&runtime);

    // EOF means the child ended. `stop` keeps the session object; `close`
    // and a natural exit remove it. session_finished is only for a removal
    // so a stopped-but-listed session still holds the idle-exit gate.
    let removed = finish_reader_session(&registry, &id, &runtime);
    runtime.reader_finished.store(true, Ordering::Release);
    if removed {
        if let Some(state) = state.upgrade() {
            state.session_finished();
        }
    }
}

fn coalesce_loop(
    rx: mpsc::Receiver<Vec<u8>>,
    runtime: Arc<SessionRuntime>,
    registry: SessionRegistry,
    session_id: String,
    owner: OwnerId,
) {
    let mut pending = Vec::new();
    loop {
        let received = if pending.is_empty() {
            rx.recv().ok()
        } else {
            match rx.recv_timeout(COALESCE_FLUSH) {
                Ok(bytes) => Some(bytes),
                Err(RecvTimeoutError::Timeout) => {
                    flush_coalesced(&mut pending, &runtime, &registry, &session_id, &owner);
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => None,
            }
        };
        match received {
            Some(bytes) => {
                pending.extend_from_slice(&bytes);
                if pending.len() >= COALESCE_MAX_BYTES || pending.len() == COALESCE_EAGER_BYTES {
                    flush_coalesced(&mut pending, &runtime, &registry, &session_id, &owner);
                }
            }
            None => {
                flush_coalesced(&mut pending, &runtime, &registry, &session_id, &owner);
                break;
            }
        }
    }
}

fn flush_coalesced(
    pending: &mut Vec<u8>,
    runtime: &SessionRuntime,
    registry: &SessionRegistry,
    session_id: &str,
    owner: &OwnerId,
) {
    if pending.is_empty() {
        return;
    }
    let data = String::from_utf8_lossy(pending).into_owned();
    pending.clear();
    if runtime.publish_output(&data) && runtime.transition_ready() {
        registry.notify_session_transition(owner, session_id);
    }
}

/// Returns whether the registry entry was removed (so the caller can
/// decrement the live-session count). `None` from the lock means another
/// path already took the session — do not session_finished again.
fn finish_reader_session(registry: &SessionRegistry, id: &str, runtime: &SessionRuntime) -> bool {
    let Ok(mut map) = registry.inner.lock() else {
        return false;
    };
    let Some(session) = map.get_mut(id).and_then(RegistryEntry::as_live_mut) else {
        return false;
    };
    session.reader_handle = None;
    let preserve = session.preserve_on_exit.load(Ordering::SeqCst);
    if preserve {
        let coalesce = session.coalesce_handle.take();
        session.exited.store(true, Ordering::SeqCst);
        drop(map);
        join_coalesce(coalesce, runtime);
        journal_mark_ended(registry, runtime);
        runtime.close_output();
        return false;
    }
    let Some(RegistryEntry::Live(mut session)) = map.remove(id) else {
        return false;
    };
    drop(map);
    session.reader_handle = None;
    let coalesce = session.coalesce_handle.take();
    let stderr = session.stderr_handle.take();
    let child_wait = session.child_wait.take();
    let PtySession {
        master,
        writer,
        killer,
        runtime: session_runtime,
        exited,
        ..
    } = session;
    exited.store(true, Ordering::SeqCst);
    drop(killer);
    drop(writer);
    drop(master);
    bounded_join(stderr);
    bounded_join(child_wait);
    join_coalesce(coalesce, runtime);
    let _ = session_runtime;
    journal_mark_ended(registry, runtime);
    runtime.close_output();
    true
}

fn journal_mark_ended(registry: &SessionRegistry, runtime: &SessionRuntime) {
    let Some(journal) = &registry.journal else {
        return;
    };
    let (generation, code) = match runtime.lock_stream() {
        Ok(stream) => (stream.generation, stream.exit_code),
        Err(_) => (runtime.generation(), None),
    };
    // EOF path: waiting on the journal here does not stall a live PTY. The
    // terminal marker is critical and must not be dropped behind a full
    // output queue.
    if let Err(error) = journal.mark_ended_blocking(&runtime.session_id, generation, code) {
        runtime.mark_journal_degraded();
        eprintln!(
            "journal could not record terminal exit for {}: {error}",
            runtime.session_id
        );
    }
    registry.invalidate_journal_roster();
}

fn terminate_spawned_child(pair: portable_pty::PtyPair, mut child: Box<dyn Child + Send + Sync>) {
    let mut killer = child.clone_killer();
    let _ = killer.kill();
    drop(pair.master);
    let _ = child.wait();
}

/// Kill + drop writer/master + wait + bounded reader join. ORDER IS
/// LOAD-BEARING: on Windows, waiting while the ConPTY master is alive can
/// deadlock the ConPTY host. Dropping the master also unblocks the
/// reader's blocking read.
fn teardown_session(session: PtySession) {
    teardown_session_inner(session, true);
}

/// Tear down a replaced provider generation without marking the journal row
/// ended. `resume` immediately starts the next generation on this same row.
fn teardown_session_for_resume(session: PtySession) {
    teardown_session_inner(session, false);
}

fn teardown_session_inner(session: PtySession, finish_runtime: bool) {
    session.exited.store(true, Ordering::SeqCst);
    let PtySession {
        process_job,
        master,
        mut killer,
        switcher: _,
        child_wait,
        writer,
        reader_handle,
        coalesce_handle,
        stderr_handle,
        runtime,
        exited: _,
        owner: _,
        metadata: _,
        preserve_on_exit: _,
    } = session;

    // 1) Kill first. The killer is separate so this cannot race with wait().
    killer.kill();
    drop(killer);
    // 2) Drop writer and master BEFORE wait(). The writer owns another
    //    master-side handle, and ConPTY's host can remain alive while either
    //    handle is open. Closing them also unblocks the reader. The registry
    //    entry was removed before this function, so only transient
    //    command-side Arc clones remain.
    drop(writer);
    drop(master);
    // Closing the per-session KILL_ON_JOB_CLOSE job terminates the root and
    // every descendant before wait(). The daemon-wide job remains open for
    // other sessions and is the crash/no-cleanup backstop.
    drop(process_job);
    // 3) Reap after the PTY endpoints are closed; this prevents a zombie
    //    and avoids the Windows ConPTY wait deadlock. The waiter thread
    //    owns Child::wait so we join it here instead of calling wait()
    //    ourselves.
    bounded_join(child_wait);
    bounded_join(stderr_handle);
    // 4) Best-effort bounded join. JoinHandle has no timed join; the
    //    endpoint close above makes the reader finish promptly, while this
    //    small budget prevents shutdown from accumulating a hang across
    //    sessions.
    // The order above is intentional: the coalescer gets every chance to
    // publish before output is closed. If its bounded join still gives up,
    // any pending bytes may be discarded by the later finish(). Surface that
    // loss through the same per-session degradation signal used for journal
    // failures instead of silently claiming completeness.
    join_coalesce(coalesce_handle, &runtime);
    bounded_join(reader_handle);
    if finish_runtime {
        runtime.finish(None);
    }
}

fn join_coalesce(handle: Option<JoinHandle<()>>, runtime: &SessionRuntime) {
    if !bounded_join(handle) {
        runtime.mark_journal_degraded();
        eprintln!(
            "session {} coalesce thread exceeded teardown join budget; scrollback may be truncated",
            runtime.session_id
        );
    }
}

fn bounded_join<T>(handle: Option<JoinHandle<T>>) -> bool {
    if let Some(handle) = handle {
        let deadline = Instant::now() + READER_JOIN_BUDGET;
        while !handle.is_finished() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if handle.is_finished() {
            let _ = handle.join();
            return true;
        }
        return false;
    }
    true
}

fn not_found() -> WireError {
    WireError::new(ErrorCode::SessionNotFound, "No session with that id.")
}

fn cannot_resume(reason: &str) -> WireError {
    WireError::new(
        ErrorCode::InvalidRequest,
        format!("This session cannot be resumed: {reason}."),
    )
}

fn resume_handle(
    record: &crate::journal::SessionRecord,
    owner: &OwnerId,
) -> Result<(String, String), WireError> {
    if record.owner != owner.user {
        return Err(unauthorized());
    }
    if record.kind != SessionKind::Acp {
        return Err(cannot_resume("only ACP sessions support this resume path"));
    }
    let provider = record
        .provider
        .clone()
        .ok_or_else(|| cannot_resume("the provider was not persisted"))?;
    let peer_session_id = record
        .peer_session_id
        .clone()
        .ok_or_else(|| cannot_resume("the provider session id was not persisted"))?;
    Ok((provider, peer_session_id))
}

fn journal_unavailable() -> WireError {
    WireError::new(
        ErrorCode::Journal,
        "The conversation journal is unavailable.",
    )
}

fn git_state_allows_worktree(state: &str) -> bool {
    matches!(state, "repository" | "inside_repository")
}

fn refuse_worktree_unless_live_git_allows(
    recorded: &str,
    observed: &str,
    project_id: &str,
) -> Result<(), WireError> {
    if git_state_allows_worktree(observed) {
        return Ok(());
    }
    Err(
        WireError::new(
            ErrorCode::WorkspaceUnavailable,
            format!(
                "Project '{project_id}' cannot host a worktree (git state is '{observed}'; recorded '{recorded}')."
            ),
        )
        .with_details(ErrorDetails::WorktreeGitState {
            recorded: recorded.to_string(),
            observed: observed.to_string(),
        }),
    )
}

fn cleanup_failed_worktree_add(repo: &Path, checkout: &Path) -> Result<(), String> {
    let remove = crate::worktree::build_worktree_remove_command(repo, checkout, true);
    crate::worktree::run_worktree_remove_command_with_recovery(&remove, repo, checkout, true)
}

fn worktree_branch_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0)
        ^ u64::from(std::process::id())
}

fn workspace_unavailable(workspace_id: &str, reason: &str) -> WireError {
    WireError::new(
        ErrorCode::WorkspaceUnavailable,
        format!("Workspace '{workspace_id}' is unavailable: {reason}."),
    )
}

fn workspace_journal_error(workspace_id: &str, error: crate::journal::JournalError) -> WireError {
    let mut wire = WireError::from(error);
    wire.message = format!(
        "Workspace '{workspace_id}' could not be read from the journal: {}",
        wire.message
    );
    wire
}

pub(super) fn internal(message: impl Into<String>) -> WireError {
    WireError::new(ErrorCode::Internal, message)
}

fn pty_wire_error(context: &str, error: impl std::fmt::Display) -> WireError {
    let detail = error.to_string();
    let message = match extract_os_error_code(&detail) {
        Some(code) => {
            eprintln!("{context} (OS error {code})");
            format!(
                "{context} (OS error {code}: {}).",
                os_error_description(code)
            )
        }
        None => {
            eprintln!("{context} (unknown OS error)");
            format!("{context} (unknown OS error).")
        }
    };
    WireError::new(ErrorCode::Io, message)
}

fn workspace_spawn_error(
    workspace_id: Option<&str>,
    path: &std::path::Path,
    error: impl std::fmt::Display,
) -> WireError {
    let detail = error.to_string();
    workspace_directory_error(workspace_id, path, &detail)
        .unwrap_or_else(|| pty_wire_error("Could not start the terminal shell.", detail))
}

fn map_workspace_spawn_wire_error(
    workspace_id: Option<&str>,
    path: &std::path::Path,
    error: WireError,
) -> WireError {
    workspace_directory_error(workspace_id, path, &error.message).unwrap_or(error)
}

fn workspace_directory_error(
    workspace_id: Option<&str>,
    path: &std::path::Path,
    detail: &str,
) -> Option<WireError> {
    let workspace_id = workspace_id?;
    let code = extract_os_error_code(detail)?;
    if !matches!(code, 2 | 3 | 267) {
        return None;
    }
    // The path is intentionally included only in the user-facing error. Do
    // not put this personal location in daemon logs or diagnostics.
    let display_path = crate::workspace::display_path(path.to_string_lossy().as_ref());
    eprintln!("workspace working directory became unavailable during spawn (OS error {code})");
    Some(WireError::new(
        ErrorCode::WorkspaceUnavailable,
        format!(
            "Workspace '{workspace_id}' at '{display_path}' became unavailable while starting the session (OS error {code}: {}).",
            os_error_description(code)
        ),
    ))
}

fn extract_os_error_code(detail: &str) -> Option<u32> {
    detail
        .rsplit_once("(os error ")?
        .1
        .strip_suffix(')')?
        .parse()
        .ok()
}

fn os_error_description(code: u32) -> &'static str {
    match code {
        2 => "no such file or directory",
        3 => "path not found",
        8 => "not enough memory",
        232 => "no data",
        1450 => "no system resources",
        267 => "directory name is invalid",
        _ => "unknown error",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A Write sink that records everything, standing in for the PTY input
    /// side so the DSR fast path is observable without a ConPTY.
    struct SharedSink(Arc<Mutex<Vec<u8>>>);

    impl Write for SharedSink {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn sink_runtime() -> (Arc<SessionRuntime>, Arc<Mutex<Vec<u8>>>) {
        let runtime = Arc::new(SessionRuntime::new());
        let received = Arc::new(Mutex::new(Vec::new()));
        let sink: Arc<Mutex<Box<dyn Write + Send>>> =
            Arc::new(Mutex::new(Box::new(SharedSink(Arc::clone(&received)))));
        runtime
            .pty_writer
            .set(sink)
            .ok()
            .expect("sink registered once");
        (runtime, received)
    }

    /// Pull until the session queue is empty, recording delivery like the
    /// connection writer does.
    fn drain(conn: &ConnHandle) -> Vec<SessionEvent> {
        let mut events = Vec::new();
        loop {
            let batch = conn.pull_events();
            if batch.is_empty() {
                return events;
            }
            for event in &batch {
                conn.event_sent(event);
            }
            events.extend(batch.into_iter().map(|pending| pending.envelope.event));
        }
    }

    fn apply_snapshot_state(screen: &mut Screen, event: &SessionEvent) {
        let SessionEvent::Snapshot {
            data,
            cursor,
            bracketed_paste,
            line_wrap,
            title,
            ..
        } = event
        else {
            return;
        };
        screen.process(data.as_bytes());
        let shape = match cursor.shape {
            CursorShape::Block => 1,
            CursorShape::Underline => 3,
            CursorShape::Bar => 5,
        } + u16::from(!cursor.blinking);
        let state = format!(
            "\x1b[{};{}H\x1b[?25{}\x1b[{shape} q\x1b[?2004{}\x1b[?7{}{}",
            cursor.row + 1,
            cursor.col + 1,
            if cursor.visible { 'h' } else { 'l' },
            if *bracketed_paste { 'h' } else { 'l' },
            if *line_wrap { 'h' } else { 'l' },
            title
                .as_deref()
                .map(|title| format!("\x1b]2;{}\x1b\\", title))
                .unwrap_or_default(),
        );
        screen.process(state.as_bytes());
    }

    /// Deterministic flood chunk with attributes, cursor motion, CJK and a
    /// line break, so screen equality is exercised beyond plain text.
    fn flood_chunk(index: usize) -> String {
        let shade = 31 + (index % 7);
        format!("\x1b[{shade}mchunk {index:06}\x1b[0m \u{754c}\r\n")
    }

    fn attach_tracked(runtime: &Arc<SessionRuntime>, conn: &Arc<ConnHandle>) -> u64 {
        let outcome = runtime
            .try_attach_with_replay(None, conn, false)
            .expect("attach");
        let transcript = runtime.is_transcript();
        conn.track_with_agent_replay(
            "s.a.1",
            Arc::clone(runtime),
            transcript,
            Some(0),
            outcome.generation,
            outcome.live_agent_replay,
        );
        outcome.generation
    }

    #[test]
    fn silence_transition_is_emitted_once_after_the_threshold() {
        let runtime = Arc::new(SessionRuntime::new());
        let conn = ConnHandle::new(1);
        attach_tracked(&runtime, &conn);
        let _ = drain(&conn);
        let last_publish = runtime
            .stream
            .lock()
            .expect("stream lock")
            .last_publish
            .expect("new sessions have an observed start time");

        assert_eq!(
            runtime.mark_silent_if_due(
                last_publish + SESSION_SILENCE_THRESHOLD + Duration::from_millis(42)
            ),
            Some(SESSION_SILENCE_THRESHOLD.as_millis() as u64 + 42)
        );
        assert_eq!(
            drain(&conn),
            vec![SessionEvent::Silent {
                elapsed_ms: SESSION_SILENCE_THRESHOLD.as_millis() as u64 + 42,
            }]
        );
        assert_eq!(
            runtime.mark_silent_if_due(
                last_publish + SESSION_SILENCE_THRESHOLD + Duration::from_secs(1)
            ),
            None
        );
        assert!(
            drain(&conn).is_empty(),
            "silence is a transition, not a tick"
        );
    }

    #[test]
    fn queued_silence_is_dropped_when_output_precedes_a_reattach() {
        let runtime = Arc::new(SessionRuntime::new());
        let first = Arc::new(ConnHandle::new(1));
        attach_tracked(&runtime, &first);
        let _ = drain(&first);
        let last_publish = runtime
            .stream
            .lock()
            .expect("stream lock")
            .last_publish
            .expect("new sessions have an observed start time");

        runtime.mark_silent_if_due(
            last_publish + SESSION_SILENCE_THRESHOLD + Duration::from_millis(1),
        );
        runtime.publish_output("resumed");
        runtime.detach_if_conn(first.id);

        let second = Arc::new(ConnHandle::new(2));
        attach_tracked(&runtime, &second);
        let events = drain(&second);
        assert!(
            events
                .iter()
                .all(|event| !matches!(event, SessionEvent::Silent { .. })),
            "a reattached client must not receive stale silence: {events:?}"
        );
    }

    #[test]
    fn silence_is_dropped_when_the_session_exits() {
        let runtime = Arc::new(SessionRuntime::new());
        let conn = Arc::new(ConnHandle::new(1));
        attach_tracked(&runtime, &conn);
        let _ = drain(&conn);
        let last_publish = runtime
            .stream
            .lock()
            .expect("stream lock")
            .last_publish
            .expect("new sessions have an observed start time");

        runtime.mark_silent_if_due(
            last_publish + SESSION_SILENCE_THRESHOLD + Duration::from_millis(1),
        );
        runtime.finish(Some(7));

        assert_eq!(
            drain(&conn),
            vec![SessionEvent::Exit { code: Some(7) }],
            "exit must be the only terminal transition delivered after silence"
        );
    }

    #[test]
    fn acp_publish_notifies_roster_when_leaving_silent() {
        let runtime = Arc::new(SessionRuntime::new());
        runtime.transition_ready.store(true, Ordering::Release);
        let notified = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let flag = Arc::clone(&notified);
        runtime.set_roster_notify(Arc::new(move || {
            flag.store(true, Ordering::SeqCst);
        }));
        let last_publish = runtime
            .stream
            .lock()
            .expect("stream lock")
            .last_publish
            .expect("new sessions have an observed start time");
        runtime.mark_silent_if_due(
            last_publish + SESSION_SILENCE_THRESHOLD + Duration::from_millis(1),
        );
        assert!(
            matches!(
                runtime.lock_stream().expect("stream").disposition,
                Disposition::Silent
            ),
            "precondition: session is Silent"
        );
        runtime.publish_agent_event(
            SessionEvent::AgentMessage {
                message_id: Some("m1".to_string()),
                text: "back".to_string(),
            },
            None,
        );
        assert!(
            matches!(
                runtime.lock_stream().expect("stream").disposition,
                Disposition::Running
            ),
            "ACP output must return the stream to Running"
        );
        assert!(
            notified.load(Ordering::SeqCst),
            "ACP Silent→Live must notify the sessions_watch roster, like PTY output"
        );
    }

    #[cfg(windows)]
    fn spawn_innocuous_os_child() -> std::process::Child {
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        Command::new("cmd.exe")
            .args(["/d", "/c", "ping", "-n", "30", "127.0.0.1"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .expect("spawn innocuous ping")
    }

    #[cfg(windows)]
    #[test]
    fn os_liveness_observation_marks_exited_without_eof() {
        use std::os::windows::io::AsRawHandle;
        let runtime = Arc::new(SessionRuntime::new());
        runtime.transition_ready.store(true, Ordering::Release);
        let mut child = spawn_innocuous_os_child();
        let handle =
            ProcessHandle::duplicate(AsRawHandle::as_raw_handle(&child)).expect("duplicate");
        runtime.install_os_handle(handle);
        assert!(!runtime.process_exited(), "a live OS process is not Exited");
        assert!(
            !runtime.observe_os_liveness(),
            "an alive process must not be marked exited"
        );
        child.kill().expect("kill ping");
        let _ = child.wait();
        assert!(
            runtime.observe_os_liveness(),
            "OS observation must mark Exited without waiting on the PTY/ACP pipe EOF"
        );
        assert!(runtime.process_exited());
        let stream = runtime.lock_stream().expect("stream");
        assert!(
            matches!(stream.disposition, Disposition::Exited { .. }),
            "disposition must be Exited from the OS query, not from child.wait: {:?}",
            stream.disposition
        );
    }

    #[test]
    fn elapsed_time_uses_exit_for_ended_and_stays_unknown_for_recovered() {
        let now = Instant::now();
        let last_publish = Some(now - Duration::from_secs(3600));
        let exit_at = Some(now - Duration::from_secs(7));

        assert_eq!(
            elapsed_ms_since_last_life(last_publish, exit_at, true, now),
            Some(7_000)
        );
        assert_eq!(
            elapsed_ms_since_last_life(last_publish, None, false, now),
            Some(3_600_000)
        );
        assert_eq!(
            elapsed_ms_since_last_life(None, None, true, now),
            None,
            "journal-only recovered sessions have no monotonic timestamp"
        );
    }

    #[test]
    fn attach_delivers_snapshot_then_live_with_exact_boundary() {
        let runtime = Arc::new(SessionRuntime::new());
        runtime.publish_output("before");
        let conn = ConnHandle::new(1);
        attach_tracked(&runtime, &conn);
        assert_eq!(runtime.last_applied_seq(), 1);

        let events = drain(&conn);
        let [SessionEvent::Snapshot {
            as_of_seq, data, ..
        }] = &events[..]
        else {
            panic!("expected a single snapshot, got {events:?}");
        };
        assert_eq!(*as_of_seq, 1);
        assert!(data.contains("before"), "snapshot data: {data:?}");

        runtime.publish_output("after");
        let events = drain(&conn);
        assert_eq!(
            events,
            vec![SessionEvent::Output {
                seq: 2,
                data: "after".to_string()
            }]
        );
    }

    #[test]
    fn attach_during_flood_never_duplicates_or_skips() {
        let runtime = Arc::new(SessionRuntime::new());
        let flood_runtime = Arc::clone(&runtime);
        let flood = std::thread::Builder::new()
            .name("flood".into())
            .spawn(move || {
                for index in 1..=4_000 {
                    flood_runtime.publish_output(&flood_chunk(index));
                    if index % 32 == 0 {
                        std::thread::sleep(Duration::from_millis(1));
                    }
                }
            })
            .expect("flood thread");

        let mut seen = std::collections::HashSet::new();
        let mut covered_to = 0u64;
        for epoch in 0u64..25 {
            let conn = ConnHandle::new(epoch + 1);
            attach_tracked(&runtime, &conn);
            let events = drain(&conn);
            assert!(
                !events.is_empty(),
                "epoch {epoch} saw nothing: attach must enqueue a snapshot"
            );
            // Spread the attach epochs across the flood's lifetime.
            std::thread::sleep(Duration::from_millis(4));
            let mut expected = None;
            for event in &events {
                match event {
                    SessionEvent::Snapshot { as_of_seq, .. } => {
                        assert!(
                            *as_of_seq >= covered_to,
                            "snapshot boundary moved backwards at epoch {epoch}"
                        );
                        covered_to = (*as_of_seq).max(covered_to);
                        expected = Some(as_of_seq + 1);
                    }
                    SessionEvent::Output { seq, .. } => {
                        if let Some(expected_seq) = expected {
                            assert_eq!(
                                *seq, expected_seq,
                                "output skipped or duplicated at epoch {epoch}"
                            );
                        }
                        expected = Some(seq + 1);
                        assert!(seen.insert(*seq), "sequence {seq} delivered twice");
                        covered_to = (*seq).max(covered_to);
                    }
                    SessionEvent::Exit { .. } => {}
                    other => panic!("unexpected event at epoch {epoch}: {other:?}"),
                }
            }
            runtime.detach_if_conn(conn.id);
        }
        flood.join().expect("flood thread joins");

        // The flood is complete: one final attach must now deliver (or
        // subsume) everything it published.
        let conn = ConnHandle::new(999);
        attach_tracked(&runtime, &conn);
        for event in drain(&conn) {
            match event {
                SessionEvent::Snapshot { as_of_seq, .. } => covered_to = as_of_seq.max(covered_to),
                SessionEvent::Output { seq, .. } => {
                    assert!(seen.insert(seq), "sequence {seq} delivered twice");
                    covered_to = seq.max(covered_to);
                }
                _ => {}
            }
        }
        assert_eq!(
            covered_to, 4_000,
            "the flood was not fully delivered or subsumed"
        );
    }

    #[test]
    fn reattach_mid_flood_screen_equals_a_fresh_emulator() {
        let runtime = Arc::new(SessionRuntime::new());
        let mut reference = Screen::new(INITIAL_COLS, INITIAL_ROWS);

        fn apply(screen: &mut Screen, event: &SessionEvent) {
            match event {
                SessionEvent::Snapshot { .. } => apply_snapshot_state(screen, event),
                SessionEvent::Output { data, .. } => screen.process(data.as_bytes()),
                _ => {}
            }
        }

        // Phase 1: publish while detached, then attach and synchronise.
        for index in 1..=60 {
            let chunk = flood_chunk(index);
            runtime.publish_output(&chunk);
            reference.process(chunk.as_bytes());
        }
        let conn = ConnHandle::new(1);
        attach_tracked(&runtime, &conn);
        let mut client = Screen::new(INITIAL_COLS, INITIAL_ROWS);
        for event in drain(&conn) {
            apply(&mut client, &event);
        }
        assert_eq!(
            client.snapshot(),
            reference.snapshot(),
            "snapshot state must equal the emulator after phase 1"
        );

        // Phase 2: live chunks while attached, then reattach from scratch.
        for index in 61..=120 {
            let chunk = flood_chunk(index);
            runtime.publish_output(&chunk);
            reference.process(chunk.as_bytes());
        }
        for event in drain(&conn) {
            apply(&mut client, &event);
        }
        assert_eq!(client.snapshot(), reference.snapshot());

        runtime.detach_if_conn(conn.id);
        let conn = ConnHandle::new(2);
        attach_tracked(&runtime, &conn);
        let mut client = Screen::new(INITIAL_COLS, INITIAL_ROWS);
        for event in drain(&conn) {
            apply(&mut client, &event);
        }
        assert_eq!(
            client.snapshot(),
            reference.snapshot(),
            "snapshot + subsequent events must equal a fresh emulator fed the whole stream"
        );
    }

    #[test]
    fn slow_client_is_resynchronised_with_a_snapshot() {
        let runtime = Arc::new(SessionRuntime::new());
        let conn = ConnHandle::new(1);
        attach_tracked(&runtime, &conn);

        // Stop reading: publish well past the frame budget without pulling.
        for index in 1..=200 {
            runtime.publish_output(&format!("slow-{index:04}\r\n"));
        }

        let events = drain(&conn);
        let mut expected = None;
        let mut client = Screen::new(INITIAL_COLS, INITIAL_ROWS);
        let mut reference = Screen::new(INITIAL_COLS, INITIAL_ROWS);
        let mut seen = std::collections::HashSet::new();
        for event in &events {
            match event {
                SessionEvent::Snapshot { as_of_seq, .. } => {
                    expected = Some(as_of_seq + 1);
                    apply_snapshot_state(&mut client, event);
                }
                SessionEvent::Output { seq, data } => {
                    assert_eq!(*seq, expected.expect("outputs follow the snapshot"));
                    expected = Some(seq + 1);
                    assert!(seen.insert(*seq), "sequence {seq} delivered twice");
                    client.process(data.as_bytes());
                }
                other => panic!("unexpected event: {other:?}"),
            }
        }
        for index in 1..=200 {
            reference.process(format!("slow-{index:04}\r\n").as_bytes());
        }
        assert_eq!(
            client.snapshot(),
            reference.snapshot(),
            "the resynchronised screen must still be the true screen"
        );
    }

    #[test]
    fn pending_queue_never_exceeds_byte_or_frame_budget() {
        let runtime = Arc::new(SessionRuntime::new());
        let conn = ConnHandle::new(1);
        attach_tracked(&runtime, &conn);
        let payload = "x".repeat(COALESCE_MAX_BYTES);

        for _ in 0..200 {
            runtime.publish_output(&payload);
            let stream = runtime.stream.lock().expect("stream lock");
            assert!(stream
                .observers
                .values()
                .all(|attachment| attachment.pending_bytes <= PENDING_OUTPUT_BUDGET_BYTES));
            assert!(stream
                .observers
                .values()
                .all(|attachment| attachment.pending_frames <= PENDING_OUTPUT_BUDGET_FRAMES));
        }
    }

    #[test]
    fn dsr_reply_is_written_straight_to_the_pty() {
        let (runtime, received) = sink_runtime();
        // No attachment, no journal, no snapshot: the query is answered on
        // the publish path itself.
        runtime.publish_output("\x1b[2;3H\x1b[6n");
        assert_eq!(
            String::from_utf8(received.lock().unwrap().clone()).expect("utf8"),
            "\x1b[2;3R",
            "one one-based CPR reply, routed to the PTY writer"
        );
        runtime.publish_output("plain");
        assert_eq!(received.lock().unwrap().len(), 6, "no extra replies");
    }

    #[test]
    fn control_path_stays_responsive_under_flood() {
        let runtime = Arc::new(SessionRuntime::new());
        let flood_runtime = Arc::clone(&runtime);
        let stop = Arc::new(AtomicBool::new(false));
        let flood_stop = Arc::clone(&stop);
        let flood = std::thread::Builder::new()
            .name("flood".into())
            .spawn(move || {
                let chunk = "x".repeat(COALESCE_MAX_BYTES);
                while !flood_stop.load(Ordering::Acquire) {
                    for _ in 0..16 {
                        flood_runtime.publish_output(&chunk);
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
            })
            .expect("flood thread");

        let mut worst = Duration::ZERO;
        for epoch in 0..200u64 {
            let started = Instant::now();
            let conn = ConnHandle::new(epoch + 1);
            runtime
                .try_attach_with_replay(None, &conn, false)
                .expect("attach under flood");
            runtime.detach_if_conn(conn.id);
            worst = worst.max(started.elapsed());
        }
        stop.store(true, Ordering::Release);
        flood.join().expect("flood thread joins");
        // Screen capture + registration is two grid copies under the lock;
        // if the publish path ever held the mutex across slow work, this
        // would blow far past the bound. 1 s is orders of magnitude above
        // the observed cost and 30x below the RPC timeout this milestone
        // exists to fix.
        assert!(
            worst < Duration::from_secs(1),
            "state lock starved under flood: {worst:?}"
        );
    }

    #[test]
    fn two_observers_receive_the_same_output() {
        let runtime = Arc::new(SessionRuntime::new());
        let first = ConnHandle::new(1);
        let second = ConnHandle::new(2);
        let first_outcome = runtime
            .try_attach_with_subscription(101, None, &first, false)
            .expect("first observer");
        first
            .track_with_subscription(
                101,
                Arc::clone(&runtime),
                false,
                None,
                first_outcome.generation,
                first_outcome.live_agent_replay,
            )
            .expect("first subscription");
        let second_outcome = runtime
            .try_attach_with_subscription(202, None, &second, false)
            .expect("second observer");
        second
            .track_with_subscription(
                202,
                Arc::clone(&runtime),
                false,
                None,
                second_outcome.generation,
                second_outcome.live_agent_replay,
            )
            .expect("second subscription");
        runtime
            .claim_resize(first.id, 101)
            .expect("first observer claims resize control");
        let _ = drain(&first);
        let _ = drain(&second);

        runtime.publish_output("shared");

        assert_eq!(
            drain(&first),
            vec![SessionEvent::Output {
                seq: 1,
                data: "shared".to_string(),
            }]
        );
        assert_eq!(
            drain(&second),
            vec![SessionEvent::Output {
                seq: 1,
                data: "shared".to_string(),
            }]
        );
        assert_eq!(runtime.resize_owner_conn_id(), Some(1));
    }

    #[test]
    fn same_connection_can_reattach() {
        let runtime = SessionRuntime::new();
        let conn = ConnHandle::new(7);
        runtime
            .try_attach_with_replay(None, &conn, false)
            .expect("first");
        runtime
            .try_attach_with_replay(
                Some(Cursor {
                    generation: 1,
                    seq: 0,
                }),
                &conn,
                false,
            )
            .expect("reattach");
        assert_eq!(runtime.resize_owner_conn_id(), Some(7));
    }

    #[test]
    fn detaching_one_observer_leaves_the_other_live() {
        let runtime = Arc::new(SessionRuntime::new());
        let first = ConnHandle::new(3);
        let second = ConnHandle::new(4);
        let first_outcome = runtime
            .try_attach_with_subscription(301, None, &first, false)
            .expect("first observer");
        first
            .track_with_subscription(
                301,
                Arc::clone(&runtime),
                false,
                None,
                first_outcome.generation,
                first_outcome.live_agent_replay,
            )
            .expect("first subscription");
        let second_outcome = runtime
            .try_attach_with_subscription(402, None, &second, false)
            .expect("second observer");
        second
            .track_with_subscription(
                402,
                Arc::clone(&runtime),
                false,
                None,
                second_outcome.generation,
                second_outcome.live_agent_replay,
            )
            .expect("second subscription");
        let _ = drain(&first);
        let _ = drain(&second);

        runtime.detach_subscription(first.id, 301);
        first.untrack_subscription(301);
        runtime.publish_output("still-live");

        assert!(drain(&first).is_empty());
        assert_eq!(
            drain(&second),
            vec![SessionEvent::Output {
                seq: 1,
                data: "still-live".to_string(),
            }]
        );
    }

    #[test]
    fn typed_permission_request_reaches_a_late_observer() {
        let runtime = Arc::new(SessionRuntime::new());
        runtime.stream.lock().unwrap().screen = None;
        let first = ConnHandle::new(5);
        let first_outcome = runtime
            .try_attach_with_subscription(501, None, &first, true)
            .expect("first observer");
        first
            .track_with_subscription(
                501,
                Arc::clone(&runtime),
                false,
                None,
                first_outcome.generation,
                first_outcome.live_agent_replay,
            )
            .expect("first subscription");

        runtime.publish_agent_event(permission_attention_event(), None);
        let first_events = first.pull_events();
        assert!(first_events.iter().any(|event| matches!(
            event.envelope.event,
            SessionEvent::PermissionRequest { ref tool_call_id, .. } if tool_call_id == "tool-attention"
        )));
        for event in &first_events {
            first.event_sent(event);
        }

        let second = ConnHandle::new(6);
        let second_outcome = runtime
            .try_attach_with_subscription(602, None, &second, true)
            .expect("late observer");
        second
            .track_with_subscription(
                602,
                Arc::clone(&runtime),
                false,
                None,
                second_outcome.generation,
                second_outcome.live_agent_replay,
            )
            .expect("second subscription");
        let second_events = second.pull_events();
        assert!(second_events.iter().any(|event| matches!(
            event.envelope.event,
            SessionEvent::PermissionRequest { ref tool_call_id, .. } if tool_call_id == "tool-attention"
        )));
    }

    #[test]
    fn detached_permission_request_reaches_a_late_observer_once() {
        let runtime = Arc::new(SessionRuntime::new());
        runtime.stream.lock().unwrap().screen = None;
        let first = ConnHandle::new(7);
        runtime
            .try_attach_with_subscription(701, None, &first, true)
            .expect("first observer");

        runtime.publish_agent_event(permission_attention_event(), None);
        runtime.detach_subscription(first.id, 701);

        let second = ConnHandle::new(8);
        let second_outcome = runtime
            .try_attach_with_subscription(802, None, &second, true)
            .expect("late observer");
        second
            .track_with_subscription(
                802,
                Arc::clone(&runtime),
                false,
                None,
                second_outcome.generation,
                second_outcome.live_agent_replay,
            )
            .expect("second subscription");
        let events = second.pull_events();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event.envelope.event,
                    SessionEvent::PermissionRequest { ref tool_call_id, .. }
                        if tool_call_id == "tool-attention"
                ))
                .count(),
            1
        );
    }

    #[test]
    fn last_detach_keeps_runtime_and_allows_later_attach() {
        let (dir, registry, journal) = tmp_delete_registry();
        let owner = test_owner("S-1-5-21-last-detach", "process-last-detach");
        let session_id = "s.last-detach.1";
        insert_live(&registry, session_id, owner.clone());
        let runtime = registry.runtime(session_id).expect("runtime");
        let first = ConnHandle::new(5);
        registry
            .attach_with_subscription(session_id, 501, None, &first, &owner, false)
            .expect("first observer");
        let _ = drain(&first);

        registry
            .detach_with_subscription(session_id, 501, &first, &owner)
            .expect("first observer detaches");
        assert!(!runtime.process_exited());
        assert!(registry.runtime(session_id).is_ok());
        assert!(runtime.stream.lock().expect("stream").observers.is_empty());

        let third = ConnHandle::new(6);
        registry
            .attach_with_subscription(session_id, 603, None, &third, &owner, false)
            .expect("later observer");
        let _ = drain(&third);
        runtime.publish_output("after-detach");

        assert_eq!(
            drain(&third),
            vec![SessionEvent::Output {
                seq: 1,
                data: "after-detach".to_string(),
            }]
        );
        journal.shutdown();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn last_transcript_detach_removes_idle_registry_entry() {
        let (dir, registry, journal) = tmp_delete_registry();
        let owner = test_owner("S-1-5-21-transcript-idle", "process-transcript-idle");
        let session_id = "s.transcript-idle.1";
        journal
            .upsert_blocking(ended_record(session_id, &owner.user))
            .expect("journal row");
        insert_transcript(&registry, session_id, owner.clone());

        let conn = ConnHandle::new(7);
        registry
            .attach_with_subscription(session_id, 701, None, &conn, &owner, false)
            .expect("transcript observer attaches");
        registry
            .detach_with_subscription(session_id, 701, &conn, &owner)
            .expect("transcript observer detaches");

        assert!(registry.runtime(session_id).is_err());
        journal.shutdown();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn delivered_transcript_exit_removes_the_idle_registry_entry() {
        let (dir, registry, journal) = tmp_delete_registry();
        let owner = test_owner("S-1-5-21-transcript-exit", "process-transcript-exit");
        let session_id = "s.transcript-exit.1";
        journal
            .upsert_blocking(ended_record(session_id, &owner.user))
            .expect("journal row");
        insert_transcript(&registry, session_id, owner.clone());

        let conn = ConnHandle::new(8);
        registry
            .attach_with_subscription(session_id, 801, None, &conn, &owner, false)
            .expect("transcript observer attaches");
        let events = conn.pull_events();
        assert!(events
            .iter()
            .any(|event| matches!(event.envelope.event, SessionEvent::Exit { .. })));
        for event in &events {
            conn.event_sent(event);
        }
        registry.subscription_event_sent(session_id);

        assert!(registry.runtime(session_id).is_err());
        journal.shutdown();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn stale_generation_is_rejected() {
        let runtime = SessionRuntime::new();
        runtime.bump_generation();
        let conn = ConnHandle::new(1);
        let err = runtime
            .try_attach_with_replay(
                Some(Cursor {
                    generation: 1,
                    seq: 0,
                }),
                &conn,
                false,
            )
            .err()
            .expect("stale generation must be rejected");
        assert_eq!(err.code, ErrorCode::SessionGenerationMismatch);
    }

    #[test]
    fn detach_clears_only_this_connection() {
        let runtime = SessionRuntime::new();
        let conn = ConnHandle::new(3);
        runtime
            .try_attach_with_replay(None, &conn, false)
            .expect("attach");
        runtime.detach_if_conn(3);
        assert_eq!(runtime.resize_owner_conn_id(), None);
    }

    #[test]
    fn journal_keeps_drain_bytes_after_reap() {
        let dir = std::env::temp_dir().join(format!(
            "devboule-drain-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_millis()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
        journal
            .upsert_blocking(new_session_record(
                "s.drain.1",
                "S-1-5-21-1",
                None,
                SessionKind::Terminal,
                "Terminal",
            ))
            .unwrap();
        let runtime = Arc::new(SessionRuntime::with_journal(
            "s.drain.1".into(),
            Some(Arc::clone(&journal)),
        ));
        runtime.publish_output("HEAD");
        journal.flush().unwrap();
        runtime.mark_exited(Some(0));
        journal.flush().unwrap();
        let tail = "X".repeat(3953);
        runtime.publish_output(&tail);
        journal.flush().unwrap();
        runtime.close_output();
        journal.try_mark_ended("s.drain.1", 1, Some(0));
        journal.flush().unwrap();
        assert_eq!(runtime.published_frames.load(Ordering::Relaxed), 2);
        let stats = journal.stats();
        assert_eq!(stats.accepted_frames, 2);
        assert_eq!(stats.committed_frames, 2);
        assert_eq!(stats.failed_frames, 0);
        let replay = journal.replay("s.drain.1", 0).unwrap();
        let replay_bytes: usize = replay
            .events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::Output { data, .. } => Some(data.len()),
                _ => None,
            })
            .sum();
        assert_eq!(replay_bytes, 4 + 3953, "journal silently lost drain bytes");
        drop(runtime);
        drop(journal);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn coalesce_constants_are_small_enough_for_echo() {
        const {
            assert!(COALESCE_MAX_BYTES <= 16 * 1024);
        }
        const {
            assert!(COALESCE_MAX_BYTES >= 1024);
        }
        assert!(COALESCE_FLUSH <= Duration::from_millis(16));
    }

    #[test]
    fn pty_error_exposes_only_the_os_code_to_clients() {
        let detail = "CreateProcessW command=C:\\Users\\secret\\shell.exe (os error 1450)";
        let code = extract_os_error_code(detail).expect("OS error code");
        assert_eq!(code, 1450);
        assert_eq!(os_error_description(code), "no system resources");
        let wire = pty_wire_error("Could not start the terminal shell.", detail);
        assert_eq!(
            wire.message,
            "Could not start the terminal shell. (OS error 1450: no system resources)."
        );
        assert!(!wire.message.contains("secret"));
    }

    #[test]
    #[cfg(windows)]
    fn workspace_spawn_directory_error_names_workspace_and_display_path() {
        let parent =
            std::env::temp_dir().join(format!("devboule-missing-cwd-{}", std::process::id()));
        std::fs::create_dir_all(&parent).expect("parent");
        let path = parent.join("Project With Spaces");
        let error = std::process::Command::new("cmd.exe")
            .current_dir(&path)
            .spawn()
            .expect_err("CreateProcess must reject the missing cwd");
        let wire = workspace_spawn_error(Some("w.race"), &path, error);
        assert_eq!(wire.code, ErrorCode::WorkspaceUnavailable);
        assert!(wire.message.contains("w.race"));
        assert!(wire.message.contains("Project With Spaces"));
        assert!(!wire.message.contains(r"\\?\"));
        let _ = std::fs::remove_dir_all(parent);
    }

    #[test]
    fn a_real_local_workspace_supplies_the_session_command_cwd() {
        let (dir, registry, journal) = tmp_delete_registry();
        let project_path = dir.join("Project With Spaces");
        std::fs::create_dir(&project_path).expect("project folder");
        let project = crate::workspace::project_record(
            project_path.to_str().expect("project path is valid UTF-8"),
        )
        .expect("project record");
        let project = journal.project_add(project).expect("persist project");
        let workspace = journal
            .workspace_create(crate::workspace::local_workspace_record(&project))
            .expect("persist workspace");

        let mut command = PtyCommand::new("cmd.exe", Vec::new(), dir.clone(), Vec::new());
        registry
            .apply_workspace_cwd(Some(&workspace.id), &mut command)
            .expect("workspace cwd");
        assert_eq!(
            command.cwd,
            project_path.canonicalize().expect("canonical cwd")
        );

        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_unknown_workspace_fails_without_using_the_daemon_cwd() {
        let (dir, registry, journal) = tmp_delete_registry();
        let daemon_cwd = dir.clone();
        let mut command = PtyCommand::new("cmd.exe", Vec::new(), daemon_cwd.clone(), Vec::new());
        let error = registry
            .apply_workspace_cwd(Some("w.missing"), &mut command)
            .expect_err("unknown workspace must fail");
        assert_eq!(error.code, ErrorCode::WorkspaceUnavailable);
        assert!(error.message.contains("w.missing"));
        assert_eq!(command.cwd, daemon_cwd);

        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workspace_cwd_cache_avoids_a_journal_rpc_after_first_lookup() {
        let (dir, registry, journal) = tmp_delete_registry();
        let project_path = dir.join("cached-project");
        std::fs::create_dir(&project_path).expect("project folder");
        let project = crate::workspace::project_record(
            project_path.to_str().expect("project path is valid UTF-8"),
        )
        .expect("project record");
        let project = journal.project_add(project).expect("persist project");
        let workspace = journal
            .workspace_create(crate::workspace::local_workspace_record(&project))
            .expect("persist workspace");

        let mut first = PtyCommand::new("cmd.exe", Vec::new(), dir.clone(), Vec::new());
        registry
            .apply_workspace_cwd(Some(&workspace.id), &mut first)
            .expect("first workspace lookup");
        journal.shutdown();

        let mut cached = PtyCommand::new("cmd.exe", Vec::new(), dir.clone(), Vec::new());
        registry
            .apply_workspace_cwd(Some(&workspace.id), &mut cached)
            .expect("cached workspace lookup");
        assert_eq!(
            cached.cwd,
            project_path.canonicalize().expect("canonical path")
        );
        // This second call succeeds with the journal already shut down, so
        // it proves the hit did not enqueue another workspace RPC.
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_session_against_a_real_local_workspace_echoes_cwd_in_display_form() {
        let (dir, registry, journal) = tmp_delete_registry();
        let project_path = dir.join("Project With Spaces");
        std::fs::create_dir(&project_path).expect("project folder");
        let project = crate::workspace::project_record(
            project_path.to_str().expect("project path is valid UTF-8"),
        )
        .expect("project record");
        let project = journal.project_add(project).expect("persist project");
        let workspace = journal
            .workspace_create(crate::workspace::local_workspace_record(&project))
            .expect("persist workspace");

        let mut command = PtyCommand::new("cmd.exe", Vec::new(), dir.clone(), Vec::new());
        registry
            .apply_workspace_cwd(Some(&workspace.id), &mut command)
            .expect("workspace cwd");
        // The spawn sites echo this exact value onto Session.cwd. A real
        // process is not required to observe the echo: command.cwd is final
        // once apply_workspace_cwd has run.
        let cwd = Some(crate::workspace::display_path(
            &command.cwd.to_string_lossy(),
        ));
        let expected = crate::workspace::display_path(
            project_path
                .canonicalize()
                .expect("canonical cwd")
                .to_str()
                .expect("canonical cwd is valid UTF-8"),
        );
        assert_eq!(cwd.as_deref(), Some(expected.as_str()));
        assert!(
            !cwd.as_deref().expect("cwd echo").starts_with(r"\\?\"),
            "wire cwd must not carry the verbatim prefix: {cwd:?}"
        );

        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn journal_only_transcript_session_does_not_invent_a_cwd() {
        let record = new_session_record(
            "s.client.1",
            "S-1-5-21-1",
            Some("w.1".to_string()),
            SessionKind::Terminal,
            "Terminal",
        );
        let session = record.to_session();
        assert_eq!(session.workspace_id.as_deref(), Some("w.1"));
        assert_eq!(
            session.cwd, None,
            "journal rows have no cwd column; None means unknown, not a guessed workspace path"
        );
        assert_eq!(session.created_at_ms, record.created_at_ms);
    }

    #[test]
    fn resume_preserves_the_original_created_at_ms() {
        let mut record = new_session_record(
            "s.client.1",
            "S-1-5-21-1",
            Some("w.1".to_string()),
            SessionKind::Acp,
            "Agent",
        );
        record.created_at_ms = 1_700_000_000_123;
        let command = PtyCommand::new("cmd.exe", Vec::new(), std::env::temp_dir(), Vec::new());
        let session = session_metadata_for_resume(
            "s.client.1",
            record,
            &command,
            "grok".to_string(),
            "peer-1".to_string(),
            2,
        );
        assert_eq!(session.created_at_ms, 1_700_000_000_123);
        assert_eq!(session.id, "s.client.1");
        assert_eq!(session.state, SessionState::Live { generation: 2 });
    }

    #[test]
    fn workspace_lookup_reports_journal_failure_not_a_missing_workspace() {
        let (dir, registry, journal) = tmp_delete_registry();
        journal.shutdown();
        let mut command = PtyCommand::new("cmd.exe", Vec::new(), dir.clone(), Vec::new());
        let error = registry
            .apply_workspace_cwd(Some("w.journal-stopped"), &mut command)
            .expect_err("stopped journal must fail");
        assert_eq!(error.code, ErrorCode::Journal);
        assert!(error.message.contains("journal writer has stopped"));
        assert!(!error.message.contains("does not exist"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workspace_path_cache_evicts_old_entries_at_its_bound() {
        let mut cache = WorkspacePathCache::default();
        for index in 0..=WORKSPACE_PATH_CACHE_CAP {
            cache.insert(
                format!("w.{index}"),
                PathBuf::from(format!("C:\\workspace-{index}")),
            );
        }
        assert_eq!(cache.entries.len(), WORKSPACE_PATH_CACHE_CAP);
        assert!(cache.get("w.0").is_none());
        assert!(cache
            .get(&format!("w.{WORKSPACE_PATH_CACHE_CAP}"))
            .is_some());
    }

    #[test]
    fn workspace_cache_invalidation_reports_a_missing_folder_not_a_deadline() {
        let (dir, registry, journal) = tmp_delete_registry();
        let project_path = dir.join("missing-project");
        std::fs::create_dir(&project_path).expect("project folder");
        let project = crate::workspace::project_record(
            project_path.to_str().expect("project path is valid UTF-8"),
        )
        .expect("project record");
        let project = journal.project_add(project).expect("persist project");
        let workspace = journal
            .workspace_create(crate::workspace::local_workspace_record(&project))
            .expect("persist workspace");
        let mut command = PtyCommand::new("cmd.exe", Vec::new(), dir.clone(), Vec::new());
        registry
            .apply_workspace_cwd(Some(&workspace.id), &mut command)
            .expect("cache workspace");
        std::fs::remove_dir_all(&project_path).expect("remove workspace folder");

        let mut missing = PtyCommand::new("cmd.exe", Vec::new(), dir.clone(), Vec::new());
        let error = registry
            .apply_workspace_cwd(Some(&workspace.id), &mut missing)
            .expect_err("missing workspace folder");
        assert_eq!(error.code, ErrorCode::WorkspaceUnavailable);
        assert!(error.message.contains("folder is no longer available"));
        assert!(!error.message.contains("deadline"));
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workspace_create_rejects_a_branch_on_a_local_workspace() {
        let (dir, registry, journal) = tmp_delete_registry();
        let error = registry
            .workspace_create(
                "p.not-needed-for-branch-rejection",
                WorkspaceIsolation::Local,
                Some("feature-x".to_string()),
            )
            .expect_err("branch must not be silently ignored");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error.message.contains("branch"));
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn worktree_create_refuses_when_live_git_is_not_a_repository() {
        let error = refuse_worktree_unless_live_git_allows("repository", "not_repository", "p.one")
            .expect_err("live not_repository must refuse even if recorded says repository");
        assert_eq!(error.code, ErrorCode::WorkspaceUnavailable);
        match error.details {
            Some(devboule_protocol::ErrorDetails::WorktreeGitState { recorded, observed }) => {
                assert_eq!(recorded, "repository");
                assert_eq!(observed, "not_repository");
            }
            other => panic!("expected WorktreeGitState, got {other:?}"),
        }
        assert!(
            refuse_worktree_unless_live_git_allows("not_repository", "repository", "p.one").is_ok(),
            "live repository must win over a stale recorded not_repository"
        );
    }

    #[test]
    fn worktree_workspace_cwd_uses_the_checkout_path_not_the_project() {
        let (dir, registry, journal) = tmp_delete_registry();
        let project_path = dir.join("project");
        let checkout = dir.join("checkout");
        std::fs::create_dir(&project_path).expect("project folder");
        std::fs::create_dir(&checkout).expect("checkout folder");
        let project = crate::workspace::project_record(
            project_path.to_str().expect("project path is valid UTF-8"),
        )
        .expect("project record");
        let project = journal.project_add(project).expect("persist project");
        let workspace = journal
            .workspace_create(crate::workspace::worktree_workspace_record(
                &project,
                &checkout,
                "feature-x",
            ))
            .expect("persist worktree workspace");
        assert_eq!(workspace.isolation, WorkspaceIsolation::Worktree);
        let mut command = PtyCommand::new("cmd.exe", Vec::new(), dir.clone(), Vec::new());
        registry
            .apply_workspace_cwd(Some(&workspace.id), &mut command)
            .expect("worktree cwd");
        assert_eq!(command.cwd, checkout);
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workspace_delete_detaches_the_row_when_the_project_folder_is_gone() {
        let (dir, registry, journal) = tmp_delete_registry();
        let project_path = dir.join("project");
        let checkout = dir.join("project.worktrees").join("kept");
        std::fs::create_dir(&project_path).expect("project folder");
        std::fs::create_dir_all(&checkout).expect("checkout");
        std::fs::write(checkout.join("uncommitted.txt"), "keep me").expect("work");
        let project = crate::workspace::project_record(
            project_path.to_str().expect("project path is valid UTF-8"),
        )
        .expect("project record");
        let project = journal.project_add(project).expect("persist project");
        let workspace = journal
            .workspace_create(crate::workspace::worktree_workspace_record(
                &project,
                &checkout,
                "feature/a",
            ))
            .expect("persist worktree workspace");
        std::fs::remove_dir_all(&project_path).expect("remove project folder");
        registry
            .workspace_delete(&workspace.id, false)
            .expect("row must be removable when the project folder is gone");
        assert!(
            journal.workspace_get(&workspace.id).expect("get").is_none(),
            "stale row must be detached"
        );
        assert!(
            checkout.join("uncommitted.txt").is_file(),
            "uncommitted work must stay on disk"
        );
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn workspace_delete_refuses_a_checkout_outside_the_project_worktree_root() {
        let (dir, registry, journal) = tmp_delete_registry();
        let project_path = dir.join("project");
        let outsider = dir.join("someone-else");
        std::fs::create_dir(&project_path).expect("project folder");
        std::fs::create_dir(&outsider).expect("outsider");
        let project = crate::workspace::project_record(
            project_path.to_str().expect("project path is valid UTF-8"),
        )
        .expect("project record");
        let project = journal.project_add(project).expect("persist project");
        let workspace = journal
            .workspace_create(crate::workspace::worktree_workspace_record(
                &project,
                &outsider,
                "feature/a",
            ))
            .expect("persist worktree workspace");
        let error = registry
            .workspace_delete(&workspace.id, true)
            .expect_err("must not git-remove a path outside the worktree root");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(matches!(
            error.details,
            Some(devboule_protocol::ErrorDetails::WorktreeNotConfined { .. })
        ));
        assert!(outsider.is_dir(), "outsider checkout must be untouched");
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn local_workspace_delete_does_not_remove_the_project_folder() {
        let (dir, registry, journal) = tmp_delete_registry();
        let project_path = dir.join("project");
        std::fs::create_dir(&project_path).expect("project folder");
        let project = crate::workspace::project_record(
            project_path.to_str().expect("project path is valid UTF-8"),
        )
        .expect("project record");
        let project = journal.project_add(project).expect("persist project");
        let workspace = registry
            .workspace_create(&project.id, WorkspaceIsolation::Local, None)
            .expect("local workspace");
        let error = registry
            .workspace_delete(&workspace.id, false)
            .expect_err("local workspace must not be deleted as a worktree");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(project_path.is_dir());
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn tmp_delete_registry() -> (std::path::PathBuf, SessionRegistry, Arc<Journal>) {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        let process_id = std::process::id();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or(0);
        let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!(
            "devboule-delete-session-{process_id}-{stamp}-{counter}"
        ));
        std::fs::create_dir(&dir).expect("tmp dir");
        let journal = Arc::new(Journal::open(&dir.join("journal.db")).expect("journal"));
        let registry =
            SessionRegistry::new(RuntimePaths::from_dir(&dir), Some(Arc::clone(&journal)));
        (dir, registry, journal)
    }

    fn test_owner(user: &str, client: &str) -> OwnerId {
        OwnerId::new(user, client).expect("owner")
    }

    fn permission_attention_event() -> SessionEvent {
        SessionEvent::PermissionRequest {
            tool_call_id: "tool-attention".to_string(),
            title: "Run attention test".to_string(),
            description: None,
            command: None,
            args: None,
            cwd: None,
            env: None,
            options: Vec::new(),
        }
    }

    #[test]
    fn attention_priority_preserves_permission_and_allows_escalation() {
        let runtime = Arc::new(SessionRuntime::new());
        runtime.publish_agent_event(
            SessionEvent::AgentFinished {
                stop_reason: "end_turn".to_string(),
                model_id: None,
                usage: None,
            },
            None,
        );
        let finished_at = runtime.attention().expect("finished attention");
        assert_eq!(
            finished_at.reason,
            devboule_protocol::AttentionReason::Finished
        );
        std::thread::sleep(Duration::from_millis(2));
        runtime.publish_agent_event(
            SessionEvent::AgentError {
                message: "attention error".to_string(),
            },
            None,
        );
        let error_at = runtime.attention().expect("error attention");
        assert_eq!(error_at.reason, devboule_protocol::AttentionReason::Error);
        assert!(error_at.at_ms > finished_at.at_ms);
        runtime.publish_agent_event(permission_attention_event(), None);
        assert_eq!(
            runtime.attention().expect("permission attention").reason,
            devboule_protocol::AttentionReason::Permission
        );
        runtime.publish_agent_event(
            SessionEvent::AgentFinished {
                stop_reason: "end_turn".to_string(),
                model_id: None,
                usage: None,
            },
            None,
        );
        assert_eq!(
            runtime
                .attention()
                .expect("permission stays pending")
                .reason,
            devboule_protocol::AttentionReason::Permission
        );
    }

    #[test]
    fn attention_clear_cannot_complete_during_the_suppression_decision() {
        let runtime = Arc::new(SessionRuntime::new());
        let suppression_entered = Arc::new(std::sync::Barrier::new(2));
        let release_suppression = Arc::new(std::sync::Barrier::new(2));
        let entered = Arc::clone(&suppression_entered);
        let release = Arc::clone(&release_suppression);
        runtime.set_attention_hooks(
            Arc::new(move || {
                entered.wait();
                release.wait();
                false
            }),
            Arc::new(|| {}),
        );

        let raising = Arc::clone(&runtime);
        let raise_thread = std::thread::spawn(move || {
            raising.publish_agent_event(
                SessionEvent::AgentFinished {
                    stop_reason: "end_turn".to_string(),
                    model_id: None,
                    usage: None,
                },
                None,
            );
        });
        suppression_entered.wait();

        let (clear_started, clear_started_rx) = std::sync::mpsc::channel();
        let (clear_done, clear_done_rx) = std::sync::mpsc::channel();
        let clearing = Arc::clone(&runtime);
        let clear_thread = std::thread::spawn(move || {
            clear_started.send(()).expect("clear thread started");
            clear_done
                .send(clearing.clear_attention())
                .expect("clear result");
        });
        clear_started_rx
            .recv()
            .expect("clear thread reached the call");
        let clear_was_blocked = clear_done_rx
            .recv_timeout(Duration::from_millis(100))
            .is_err();

        release_suppression.wait();
        raise_thread.join().expect("raise thread");
        clear_thread.join().expect("clear thread");
        assert!(
            clear_was_blocked,
            "clear completed while the suppression decision was still open"
        );
        assert!(runtime.attention().is_none());
    }

    #[test]
    fn visible_focus_suppresses_attention_and_presence_clears_it() {
        let (_dir, registry, journal) = tmp_delete_registry();
        let owner = test_owner("S-1-5-21-attention", "process-attention");
        let runtime = insert_live_agent(&registry, "s.attention.1", owner.clone());
        registry
            .set_presence(1, &owner, Some("s.attention.1".to_string()), true)
            .expect("presence");
        runtime.publish_agent_event(
            SessionEvent::AgentFinished {
                stop_reason: "end_turn".to_string(),
                model_id: None,
                usage: None,
            },
            None,
        );
        assert!(
            runtime.attention().is_none(),
            "visible focus suppresses raise"
        );
        registry.clear_presence(1);
        runtime.publish_agent_event(
            SessionEvent::AgentFinished {
                stop_reason: "end_turn".to_string(),
                model_id: None,
                usage: None,
            },
            None,
        );
        assert!(runtime.attention().is_some());
        registry
            .set_presence(1, &owner, Some("s.attention.1".to_string()), true)
            .expect("focus clears attention");
        assert!(
            runtime.attention().is_none(),
            "focus acknowledges attention"
        );
        drop(journal);
        let _ = std::fs::remove_dir_all(_dir);
    }

    #[test]
    fn invisible_presence_raises_and_a_second_connection_elsewhere_does_not_suppress() {
        let (_dir, registry, journal) = tmp_delete_registry();
        let owner = test_owner("S-1-5-21-presence", "process-presence");
        let runtime = insert_live_agent(&registry, "s.presence.1", owner.clone());
        registry
            .set_presence(1, &owner, None, false)
            .expect("invisible presence");
        runtime.publish_agent_event(
            SessionEvent::AgentFinished {
                stop_reason: "end_turn".to_string(),
                model_id: None,
                usage: None,
            },
            None,
        );
        assert!(
            runtime.attention().is_some(),
            "invisible app is not watching"
        );
        assert!(runtime.clear_attention());
        registry
            .set_presence(1, &owner, Some("s.presence.1".to_string()), true)
            .expect("focused connection");
        registry
            .set_presence(2, &owner, Some("s.other.1".to_string()), true)
            .expect("second connection elsewhere");
        runtime.publish_agent_event(
            SessionEvent::AgentFinished {
                stop_reason: "end_turn".to_string(),
                model_id: None,
                usage: None,
            },
            None,
        );
        assert!(
            runtime.attention().is_none(),
            "the focused connection suppresses"
        );
        drop(journal);
        let _ = std::fs::remove_dir_all(_dir);
    }

    #[test]
    fn sending_a_prompt_acknowledges_attention() {
        let (_dir, registry, journal) = tmp_delete_registry();
        let owner = test_owner("S-1-5-21-send-attention", "process-send-attention");
        let runtime = insert_live_agent_with_writer(
            &registry,
            "s.send.1",
            owner.clone(),
            Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
        );
        let conn = ConnHandle::new(7);
        registry
            .attach("s.send.1", None, &conn, &owner, true)
            .expect("attach");
        runtime.publish_agent_event(
            SessionEvent::AgentFinished {
                stop_reason: "end_turn".to_string(),
                model_id: None,
                usage: None,
            },
            None,
        );
        assert!(runtime.attention().is_some());
        registry
            .send("s.send.1", "next", &owner, &conn)
            .expect("send");
        assert!(
            runtime.attention().is_none(),
            "prompt acknowledges attention"
        );
        drop(journal);
        let _ = std::fs::remove_dir_all(_dir);
    }

    #[test]
    fn answering_permission_acknowledges_attention() {
        let (dir, registry, journal) = tmp_delete_registry();
        let owner = test_owner(
            "S-1-5-21-permission-attention",
            "process-permission-attention",
        );
        let session_id = "s.permission-attention.1";
        let runtime = insert_live_agent(&registry, session_id, owner.clone());
        journal
            .upsert_blocking(new_session_record(
                session_id,
                &owner.user,
                None,
                SessionKind::Acp,
                "Agent",
            ))
            .expect("session row");
        let conn = ConnHandle::new(8);
        registry
            .attach(session_id, None, &conn, &owner, true)
            .expect("attach");
        let request = permission_broker::permission("ack-permission");
        runtime.publish_agent_event(request.clone(), None);
        runtime
            .permission_broker()
            .expect("permission broker")
            .register(12, request, &runtime)
            .expect("permission request");
        assert_eq!(
            runtime.attention().expect("permission attention").reason,
            devboule_protocol::AttentionReason::Permission
        );

        registry
            .permission_respond(
                session_id,
                "ack-permission",
                PermissionOutcome::AllowOnce,
                &conn,
                &owner,
            )
            .expect("permission response");
        assert!(
            runtime.attention().is_none(),
            "answering permission acknowledges attention"
        );
        journal.shutdown();
        let _ = std::fs::remove_dir_all(dir);
    }

    fn ended_record(id: &str, user: &str) -> crate::journal::SessionRecord {
        let mut record = new_session_record(id, user, None, SessionKind::Terminal, "Terminal");
        record.status = PersistStatus::Ended;
        record
    }

    fn insert_transcript(registry: &SessionRegistry, id: &str, owner: OwnerId) {
        let metadata = Session {
            id: id.to_string(),
            workspace_id: None,
            cwd: None,
            kind: SessionKind::Terminal,
            title: "Terminal".to_string(),
            state: SessionState::Ended {
                generation: 1,
                code: Some(0),
                integrity: TranscriptIntegrity::Complete,
            },
            elapsed_ms: Some(0),
            provider: None,
            peer_session_id: None,
            created_at_ms: 1,
        };
        let runtime = SessionRuntime::from_replay(
            id.to_string(),
            registry.journal.clone(),
            crate::journal::Replay {
                generation: 1,
                last_seq: 0,
                integrity: TranscriptIntegrity::Complete,
                event_seqs: Vec::new(),
                events: Vec::new(),
            },
        );
        registry.inner.lock().expect("registry").insert(
            id.to_string(),
            RegistryEntry::Transcript(TranscriptSession {
                metadata,
                owner,
                runtime,
            }),
        );
    }

    struct NoopKiller;

    impl SessionKiller for NoopKiller {
        fn kill(&mut self) {}
        fn clone_killer(&self) -> Box<dyn SessionKiller> {
            Box::new(NoopKiller)
        }
    }

    struct RecordingSwitcher(Arc<AtomicU64>);

    impl ModelSwitcher for RecordingSwitcher {
        fn set_model(
            &self,
            _model_id: Option<&str>,
            _effort: Option<&str>,
        ) -> Result<(), WireError> {
            self.0.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }

        fn clone_switcher(&self) -> Box<dyn ModelSwitcher> {
            Box::new(Self(Arc::clone(&self.0)))
        }
    }

    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _bytes: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "forced writer failure",
            ))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    struct RecordingWriter(Arc<Mutex<Vec<u8>>>);

    impl Write for RecordingWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0
                .lock()
                .expect("recording writer lock")
                .extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    struct BytewiseRecordingWriter {
        bytes: Arc<Mutex<Vec<u8>>>,
        first_write: Arc<Barrier>,
        first_write_seen: AtomicBool,
    }

    impl Write for BytewiseRecordingWriter {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            let Some(byte) = bytes.first() else {
                return Ok(0);
            };
            self.bytes
                .lock()
                .expect("recording writer lock")
                .push(*byte);
            if !self.first_write_seen.swap(true, Ordering::AcqRel) {
                self.first_write.wait();
            }
            Ok(1)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn insert_live_agent(
        registry: &SessionRegistry,
        id: &str,
        owner: OwnerId,
    ) -> Arc<SessionRuntime> {
        insert_live_agent_with_writer(
            registry,
            id,
            owner,
            Box::new(FailingWriter) as Box<dyn Write + Send>,
        )
    }

    fn insert_live_agent_with_writer(
        registry: &SessionRegistry,
        id: &str,
        owner: OwnerId,
        writer: Box<dyn Write + Send>,
    ) -> Arc<SessionRuntime> {
        let metadata = Session {
            id: id.to_string(),
            workspace_id: None,
            cwd: None,
            kind: SessionKind::Acp,
            title: "Agent".to_string(),
            state: SessionState::Live { generation: 1 },
            elapsed_ms: Some(0),
            provider: Some("test-agent".to_string()),
            peer_session_id: None,
            created_at_ms: 1,
        };
        let (broker, _) = permission_broker::test_broker();
        let runtime = SessionRuntime::for_acp(id.to_string(), registry.journal.clone(), broker);
        registry.configure_runtime_attention(&runtime, &owner);
        let session = PtySession {
            metadata,
            owner,
            process_job: Arc::new(JobObject::new().expect("job")),
            master: None,
            killer: Box::new(NoopKiller),
            switcher: None,
            stderr_handle: None,
            child_wait: None,
            writer: Arc::new(Mutex::new(writer)),
            reader_handle: None,
            coalesce_handle: None,
            runtime: Arc::clone(&runtime),
            exited: Arc::new(AtomicBool::new(false)),
            preserve_on_exit: Arc::new(AtomicBool::new(false)),
        };
        registry
            .inner
            .lock()
            .expect("registry")
            .insert(id.to_string(), RegistryEntry::Live(session));
        runtime
    }

    fn attach_live_agent_for_test(
        runtime: &Arc<SessionRuntime>,
        session_id: &str,
        conn_id: u64,
    ) -> Arc<ConnHandle> {
        let conn = ConnHandle::new(conn_id);
        let outcome = runtime
            .try_attach_with_replay(None, &conn, true)
            .expect("attach");
        conn.track_with_agent_replay(
            session_id,
            Arc::clone(runtime),
            false,
            None,
            outcome.generation,
            outcome.live_agent_replay,
        );
        conn
    }

    #[test]
    fn multiple_observers_can_send_complete_inputs_concurrently() {
        let (dir, registry, journal) = tmp_delete_registry();
        let owner = test_owner("S-1-5-21-multi-writer", "process-multi-writer");
        let written = Arc::new(Mutex::new(Vec::new()));
        let session_id = "s.multi-writer.1";
        let first_text = "first observer input\n".repeat(32);
        let second_text = "second observer input\n".repeat(32);
        let start = Arc::new(Barrier::new(3));
        let first_write = Arc::new(Barrier::new(2));
        insert_live_agent_with_writer(
            &registry,
            session_id,
            owner.clone(),
            Box::new(BytewiseRecordingWriter {
                bytes: Arc::clone(&written),
                first_write: Arc::clone(&first_write),
                first_write_seen: AtomicBool::new(false),
            }),
        );
        let first = ConnHandle::new(1);
        let second = ConnHandle::new(2);
        registry
            .attach_with_subscription(session_id, 101, None, &first, &owner, true)
            .expect("first observer attaches");
        registry
            .attach_with_subscription(session_id, 202, None, &second, &owner, true)
            .expect("second observer attaches");

        let first_registry = registry.clone();
        let first_start = Arc::clone(&start);
        let first_owner = owner.clone();
        let first_session_id = session_id.to_string();
        let first_handle = std::thread::spawn(move || {
            first_start.wait();
            first_registry
                .send_with_subscription(&first_session_id, 101, &first_text, &first_owner, &first)
                .expect("first input");
            first_text
        });
        let second_registry = registry.clone();
        let second_start = Arc::clone(&start);
        let second_owner = owner.clone();
        let second_session_id = session_id.to_string();
        let second_handle = std::thread::spawn(move || {
            second_start.wait();
            second_registry
                .send_with_subscription(
                    &second_session_id,
                    202,
                    &second_text,
                    &second_owner,
                    &second,
                )
                .expect("second input");
            second_text
        });
        start.wait();
        first_write.wait();
        let first_text = first_handle.join().expect("first sender joins");
        let second_text = second_handle.join().expect("second sender joins");
        let received = written.lock().expect("writer").clone();
        let first_then_second = [first_text.as_bytes(), second_text.as_bytes()].concat();
        let second_then_first = [second_text.as_bytes(), first_text.as_bytes()].concat();
        assert!(
            received == first_then_second || received == second_then_first,
            "concurrent inputs were interleaved"
        );
        journal.shutdown();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn only_resize_owner_can_resize_terminal() {
        let (dir, registry, journal) = tmp_delete_registry();
        let owner = test_owner("S-1-5-21-resize-owner", "process-resize-owner");
        let session_id = "s.resize-owner.1";
        insert_live(&registry, session_id, owner.clone());
        let first = ConnHandle::new(1);
        let second = ConnHandle::new(2);
        registry
            .attach_with_subscription(session_id, 101, None, &first, &owner, false)
            .expect("first observer attaches");
        registry
            .attach_with_subscription(session_id, 202, None, &second, &owner, false)
            .expect("second observer attaches");
        registry
            .claim_resize_with_subscription(session_id, 101, &owner, &first)
            .expect("first observer claims resize control");

        let error = registry
            .resize_with_subscription(session_id, 202, 100, 30, &owner, &second)
            .expect_err("non-owner resize must be rejected");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error.message.contains("resize control"));
        registry
            .resize_with_subscription(session_id, 101, 100, 30, &owner, &first)
            .expect("resize owner can resize");
        let runtime = registry.runtime(session_id).expect("runtime");
        assert_eq!(
            runtime
                .stream
                .lock()
                .expect("stream")
                .screen
                .as_ref()
                .expect("screen")
                .dimensions(),
            (100, 30)
        );
        journal.shutdown();
        let _ = std::fs::remove_dir_all(dir);
    }

    fn insert_live(registry: &SessionRegistry, id: &str, owner: OwnerId) {
        let metadata = Session {
            id: id.to_string(),
            workspace_id: None,
            cwd: None,
            kind: SessionKind::Terminal,
            title: "Terminal".to_string(),
            state: SessionState::Live { generation: 1 },
            elapsed_ms: Some(0),
            provider: None,
            peer_session_id: None,
            created_at_ms: 1,
        };
        let runtime = Arc::new(SessionRuntime::with_journal(
            id.to_string(),
            registry.journal.clone(),
        ));
        registry.configure_runtime_attention(&runtime, &owner);
        let session = PtySession {
            metadata,
            owner,
            process_job: Arc::new(JobObject::new().expect("job")),
            master: None,
            killer: Box::new(NoopKiller),
            switcher: None,
            stderr_handle: None,
            child_wait: None,
            writer: Arc::new(Mutex::new(Box::new(std::io::sink()))),
            reader_handle: None,
            coalesce_handle: None,
            runtime,
            exited: Arc::new(AtomicBool::new(false)),
            preserve_on_exit: Arc::new(AtomicBool::new(false)),
        };
        registry
            .inner
            .lock()
            .expect("registry")
            .insert(id.to_string(), RegistryEntry::Live(session));
    }

    #[test]
    fn terminal_send_does_not_publish_an_agent_user_message() {
        let (dir, registry, journal) = tmp_delete_registry();
        let owner = test_owner("S-1-5-21-terminal", "process-terminal");
        insert_live(&registry, "terminal-send", owner.clone());
        let conn = ConnHandle::new(108);
        registry
            .attach("terminal-send", None, &conn, &owner, false)
            .expect("terminal attaches");
        registry
            .send("terminal-send", "typed terminal input", &owner, &conn)
            .expect("terminal send");
        let runtime = registry.runtime("terminal-send").expect("runtime");
        assert_eq!(runtime.current_agent_seq(), 0);
        assert!(!runtime
            .stream
            .lock()
            .expect("stream")
            .observers
            .values()
            .flat_map(|attachment| attachment.pending.iter())
            .any(|item| matches!(
                item,
                PendingItem::Agent {
                    event: SessionEvent::AgentUserMessage { .. },
                    ..
                }
            )));
        journal.shutdown();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn same_user_attached_restarted_client_can_send() {
        let (dir, registry, journal) = tmp_delete_registry();
        let original = test_owner("S-1-5-21-reconnect-send", "process-1111");
        let restarted = test_owner("S-1-5-21-reconnect-send", "process-2222");
        let session_id = compose_session_id(&original.session_token(), "send01").expect("id");
        insert_live(&registry, &session_id, original);
        let conn = ConnHandle::new(101);
        registry
            .attach(&session_id, None, &conn, &restarted, false)
            .expect("restarted same-user client attaches");
        registry
            .send(&session_id, "restart input", &restarted, &conn)
            .expect("attached restarted client can send");
        journal.shutdown();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn same_user_unattached_client_cannot_send() {
        let (dir, registry, journal) = tmp_delete_registry();
        let owner = test_owner("S-1-5-21-unattached-send", "process-1111");
        let caller = test_owner("S-1-5-21-unattached-send", "process-2222");
        let session_id = compose_session_id(&owner.session_token(), "send02").expect("id");
        insert_live(&registry, &session_id, owner);
        let attached = ConnHandle::new(113);
        registry
            .attach(&session_id, None, &attached, &caller, false)
            .expect("a same-user connection attaches");
        let conn = ConnHandle::new(111);
        let error = registry
            .send(&session_id, "unattached input", &caller, &conn)
            .expect_err("unattached client must not send");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error.message.contains("not attached"));
        journal.shutdown();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn same_user_attached_restarted_client_can_resize_terminal() {
        let (dir, registry, journal) = tmp_delete_registry();
        let original = test_owner("S-1-5-21-reconnect-resize", "process-1111");
        let restarted = test_owner("S-1-5-21-reconnect-resize", "process-2222");
        let session_id = compose_session_id(&original.session_token(), "resize01").expect("id");
        insert_live(&registry, &session_id, original);
        let conn = ConnHandle::new(102);
        registry
            .attach(&session_id, None, &conn, &restarted, false)
            .expect("restarted same-user client attaches");
        registry
            .resize(&session_id, 100, 30, &restarted, &conn)
            .expect("attached restarted client can resize");
        let runtime = registry.runtime(&session_id).expect("runtime");
        assert_eq!(
            runtime
                .stream
                .lock()
                .expect("stream")
                .screen
                .as_ref()
                .expect("terminal screen")
                .dimensions(),
            (100, 30)
        );
        journal.shutdown();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn same_user_unattached_client_cannot_resize_terminal() {
        let (dir, registry, journal) = tmp_delete_registry();
        let owner = test_owner("S-1-5-21-unattached-resize", "process-1111");
        let caller = test_owner("S-1-5-21-unattached-resize", "process-2222");
        let session_id = compose_session_id(&owner.session_token(), "resize02").expect("id");
        insert_live(&registry, &session_id, owner);
        let attached = ConnHandle::new(114);
        registry
            .attach(&session_id, None, &attached, &caller, false)
            .expect("a same-user connection attaches");
        let conn = ConnHandle::new(112);
        let error = registry
            .resize(&session_id, 100, 30, &caller, &conn)
            .expect_err("unattached client must not resize");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error.message.contains("not attached"));
        journal.shutdown();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn same_user_attached_restarted_client_can_respond_to_permission() {
        let (dir, registry, journal) = tmp_delete_registry();
        let original = test_owner("S-1-5-21-reconnect-permission", "process-1111");
        let restarted = test_owner("S-1-5-21-reconnect-permission", "process-2222");
        let session_id = compose_session_id(&original.session_token(), "perm01").expect("id");
        let runtime = insert_live_agent(&registry, &session_id, original);
        let conn = ConnHandle::new(103);
        registry
            .attach(&session_id, None, &conn, &restarted, false)
            .expect("restarted same-user client attaches");
        runtime
            .permission_broker()
            .expect("permission broker")
            .register(
                7,
                permission_broker::permission("restart-permission"),
                &runtime,
            )
            .expect("permission request");
        registry
            .permission_respond(
                &session_id,
                "restart-permission",
                PermissionOutcome::AllowOnce,
                &conn,
                &restarted,
            )
            .expect("attached restarted client can respond");
        journal.shutdown();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn same_user_unattached_client_cannot_respond_to_permission() {
        let (dir, registry, journal) = tmp_delete_registry();
        let owner = test_owner("S-1-5-21-unattached-permission", "process-1111");
        let caller = test_owner("S-1-5-21-unattached-permission", "process-2222");
        let session_id = compose_session_id(&owner.session_token(), "perm02").expect("id");
        let runtime = insert_live_agent(&registry, &session_id, owner.clone());
        let attached = ConnHandle::new(104);
        registry
            .attach(&session_id, None, &attached, &owner, false)
            .expect("owner attaches");
        runtime
            .permission_broker()
            .expect("permission broker")
            .register(
                8,
                permission_broker::permission("unattached-permission"),
                &runtime,
            )
            .expect("permission request");
        let unattached = ConnHandle::new(105);
        let error = registry
            .permission_respond(
                &session_id,
                "unattached-permission",
                PermissionOutcome::AllowOnce,
                &unattached,
                &caller,
            )
            .expect_err("unattached client must not respond");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error.message.contains("not attached"));
        journal.shutdown();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn different_user_cannot_send_or_resize() {
        let (dir, registry, journal) = tmp_delete_registry();
        let owner = test_owner("S-1-5-21-security-terminal", "process-1111");
        let stranger = test_owner("S-1-5-21-security-stranger", "process-2222");
        let session_id = compose_session_id(&owner.session_token(), "secure01").expect("id");
        insert_live(&registry, &session_id, owner.clone());
        let conn = ConnHandle::new(106);
        registry
            .attach(&session_id, None, &conn, &owner, false)
            .expect("owner attaches");
        assert_eq!(
            registry
                .send(&session_id, "hostile input", &stranger, &conn)
                .expect_err("different user must not send")
                .code,
            ErrorCode::Unauthorized
        );
        assert_eq!(
            registry
                .resize(&session_id, 100, 30, &stranger, &conn)
                .expect_err("different user must not resize")
                .code,
            ErrorCode::Unauthorized
        );
        journal.shutdown();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn different_user_cannot_respond_to_permission() {
        let (dir, registry, journal) = tmp_delete_registry();
        let owner = test_owner("S-1-5-21-security-permission", "process-1111");
        let stranger = test_owner("S-1-5-21-security-stranger-2", "process-2222");
        let session_id = compose_session_id(&owner.session_token(), "secure02").expect("id");
        let runtime = insert_live_agent(&registry, &session_id, owner.clone());
        let conn = ConnHandle::new(107);
        registry
            .attach(&session_id, None, &conn, &owner, false)
            .expect("owner attaches");
        runtime
            .permission_broker()
            .expect("permission broker")
            .register(
                9,
                permission_broker::permission("foreign-permission"),
                &runtime,
            )
            .expect("permission request");
        let error = registry
            .permission_respond(
                &session_id,
                "foreign-permission",
                PermissionOutcome::AllowOnce,
                &conn,
                &stranger,
            )
            .expect_err("different user must not respond");
        assert_eq!(error.code, ErrorCode::Unauthorized);
        journal.shutdown();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn failed_agent_send_replays_error_without_prompt() {
        let (dir, registry, journal) = tmp_delete_registry();
        let owner = test_owner("S-1-5-21-agent", "process-agent");
        let runtime = insert_live_agent(&registry, "agent-send-failure", owner.clone());
        journal
            .upsert_blocking(new_session_record(
                "agent-send-failure",
                &owner.user,
                None,
                SessionKind::Acp,
                "Agent",
            ))
            .expect("agent session row");
        let conn = ConnHandle::new(1);
        let outcome = runtime
            .try_attach_with_replay(None, &conn, true)
            .expect("attach");
        conn.track_with_agent_replay(
            "agent-send-failure",
            Arc::clone(&runtime),
            false,
            None,
            outcome.generation,
            outcome.live_agent_replay,
        );

        let error = registry
            .send(
                "agent-send-failure",
                "prompt that cannot be sent",
                &owner,
                &conn,
            )
            .expect_err("writer must fail");
        assert_eq!(error.code, ErrorCode::Io);
        journal.flush().expect("flush prompt and error");

        let live = conn
            .pull_events()
            .into_iter()
            .map(|event| event.envelope.event)
            .collect::<Vec<_>>();
        assert!(!live.iter().any(|event| {
            matches!(event, SessionEvent::AgentUserMessage { text, .. } if text == "prompt that cannot be sent")
        }));
        let error_index = live
            .iter()
            .position(|event| {
                matches!(event, SessionEvent::AgentError { message } if message.contains("forced writer failure"))
            })
            .expect("failed send error must reach the live client");
        assert!(
            error_index < live.len(),
            "live failed send events: {live:?}"
        );

        runtime.detach_if_conn(conn.id);
        conn.untrack("agent-send-failure");
        let reattached = ConnHandle::new(2);
        let outcome = runtime
            .try_attach_with_replay(None, &reattached, true)
            .expect("reattach");
        reattached.track_with_agent_replay(
            "agent-send-failure",
            Arc::clone(&runtime),
            false,
            None,
            outcome.generation,
            outcome.live_agent_replay,
        );
        let replayed = reattached
            .pull_events()
            .into_iter()
            .map(|event| event.envelope.event)
            .collect::<Vec<_>>();
        assert!(!replayed.iter().any(|event| {
            matches!(event, SessionEvent::AgentUserMessage { text, .. } if text == "prompt that cannot be sent")
        }));
        let error_index = replayed
            .iter()
            .position(|event| {
                matches!(event, SessionEvent::AgentError { message } if message.contains("forced writer failure"))
            })
            .expect("failed send error must replay");
        assert!(
            error_index < replayed.len(),
            "replayed failed send events: {replayed:?}"
        );
        journal.shutdown();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn poisoned_agent_writer_publishes_error_without_prompt() {
        let (dir, registry, journal) = tmp_delete_registry();
        let owner = test_owner("S-1-5-21-poisoned-writer", "process-agent");
        let runtime = insert_live_agent(&registry, "agent-poisoned-writer", owner.clone());
        journal
            .upsert_blocking(new_session_record(
                "agent-poisoned-writer",
                &owner.user,
                None,
                SessionKind::Acp,
                "Agent",
            ))
            .expect("agent session row");
        let conn = attach_live_agent_for_test(&runtime, "agent-poisoned-writer", 3);
        let writer = {
            let map = registry.inner.lock().expect("registry");
            match map.get("agent-poisoned-writer").expect("session") {
                RegistryEntry::Live(session) => Arc::clone(&session.writer),
                RegistryEntry::Transcript(_) => panic!("expected live session"),
            }
        };
        std::thread::spawn(move || {
            let _guard = writer.lock().expect("writer lock");
            panic!("poison writer for test");
        })
        .join()
        .expect_err("writer lock must be poisoned");

        let error = registry
            .send(
                "agent-poisoned-writer",
                "prompt with poisoned writer",
                &owner,
                &conn,
            )
            .expect_err("poisoned writer must reject the send");
        assert_eq!(error.code, ErrorCode::Internal);
        journal.flush().expect("flush prompt and writer error");

        let live = conn
            .pull_events()
            .into_iter()
            .map(|event| event.envelope.event)
            .collect::<Vec<_>>();
        assert!(!live.iter().any(|event| {
            matches!(event, SessionEvent::AgentUserMessage { text, .. } if text == "prompt with poisoned writer")
        }));
        let error_index = live
            .iter()
            .position(|event| {
                matches!(event, SessionEvent::AgentError { message } if message == "Session state is unavailable.")
            })
            .expect("poisoned writer error must reach the client");
        assert!(
            error_index < live.len(),
            "live poisoned writer events: {live:?}"
        );

        let replay = journal
            .replay("agent-poisoned-writer", 0)
            .expect("replay poisoned writer");
        let replayed = replay.events;
        assert!(!replayed.iter().any(|event| {
            matches!(event, SessionEvent::AgentUserMessage { text, .. } if text == "prompt with poisoned writer")
        }));
        let error_index = replayed
            .iter()
            .position(|event| {
                matches!(event, SessionEvent::AgentError { message } if message == "Session state is unavailable.")
            })
            .expect("poisoned writer error must replay");
        assert!(
            error_index < replayed.len(),
            "replayed poisoned writer: {replayed:?}"
        );
        journal.shutdown();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn closed_agent_output_refuses_unrecordable_prompt() {
        let (dir, registry, journal) = tmp_delete_registry();
        let owner = test_owner("S-1-5-21-closed-agent", "process-agent");
        let written = Arc::new(Mutex::new(Vec::new()));
        let runtime = insert_live_agent_with_writer(
            &registry,
            "agent-closed-output",
            owner.clone(),
            Box::new(RecordingWriter(Arc::clone(&written))),
        );
        journal
            .upsert_blocking(new_session_record(
                "agent-closed-output",
                &owner.user,
                None,
                SessionKind::Acp,
                "Agent",
            ))
            .expect("agent session row");
        let conn = attach_live_agent_for_test(&runtime, "agent-closed-output", 109);
        runtime.close_output();

        let error = registry
            .send(
                "agent-closed-output",
                "prompt after output closed",
                &owner,
                &conn,
            )
            .expect_err("closed output must reject an unrecordable prompt");
        assert_eq!(error.code, ErrorCode::Internal);
        assert_eq!(error.message, "Agent input could not be recorded.");
        assert!(written.lock().expect("written lock").is_empty());
        assert_eq!(runtime.current_agent_seq(), 0);
        journal.flush().expect("flush closed-output journal");
        let replayed = journal
            .replay("agent-closed-output", 0)
            .expect("replay closed output")
            .events;
        assert!(!replayed.iter().any(|event| matches!(
            event,
            SessionEvent::AgentUserMessage { text, .. } if text == "prompt after output closed"
        )));
        journal.shutdown();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn poisoned_agent_stream_refuses_unrecordable_prompt() {
        let (dir, registry, journal) = tmp_delete_registry();
        let owner = test_owner("S-1-5-21-poisoned-stream", "process-agent");
        let written = Arc::new(Mutex::new(Vec::new()));
        let runtime = insert_live_agent_with_writer(
            &registry,
            "agent-poisoned-stream",
            owner.clone(),
            Box::new(RecordingWriter(Arc::clone(&written))),
        );
        journal
            .upsert_blocking(new_session_record(
                "agent-poisoned-stream",
                &owner.user,
                None,
                SessionKind::Acp,
                "Agent",
            ))
            .expect("agent session row");
        let conn = attach_live_agent_for_test(&runtime, "agent-poisoned-stream", 110);
        let poisoned_runtime = Arc::clone(&runtime);
        std::thread::spawn(move || {
            let _guard = poisoned_runtime.stream.lock().expect("stream lock");
            panic!("poison stream for test");
        })
        .join()
        .expect_err("stream lock must be poisoned");

        let error = registry
            .send(
                "agent-poisoned-stream",
                "prompt after stream poison",
                &owner,
                &conn,
            )
            .expect_err("poisoned stream must reject an unrecordable prompt");
        assert_eq!(error.code, ErrorCode::Internal);
        assert_eq!(error.message, "Session state is unavailable.");
        assert!(written.lock().expect("written lock").is_empty());
        journal.flush().expect("flush poisoned-stream journal");
        let replayed = journal
            .replay("agent-poisoned-stream", 0)
            .expect("replay poisoned stream")
            .events;
        assert!(!replayed.iter().any(|event| matches!(
            event,
            SessionEvent::AgentUserMessage { text, .. } if text == "prompt after stream poison"
        )));
        journal.shutdown();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn delete_session_allows_journal_only_record_from_another_client_of_the_same_user() {
        let (dir, registry, journal) = tmp_delete_registry();
        let original = test_owner("S-1-5-21-1", "process-1111");
        let caller = test_owner("S-1-5-21-1", "process-2222");
        let session_id = compose_session_id(&original.session_token(), "dead01").expect("id");
        journal
            .upsert_blocking(ended_record(&session_id, &original.user))
            .expect("row");

        let result = registry.delete_session(&session_id, &caller);
        assert!(
            result.is_ok(),
            "same user, different client must be able to delete a journal-only history row: {result:?}"
        );
        assert!(
            journal
                .list()
                .expect("list")
                .iter()
                .all(|row| row.id != session_id),
            "journal-only delete must remove the row"
        );
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_session_rejects_journal_only_record_owned_by_another_user() {
        let (dir, registry, journal) = tmp_delete_registry();
        let owner = test_owner("user-alice", "process-1111");
        let stranger = test_owner("user-bob", "process-1111");
        let session_id = compose_session_id(&owner.session_token(), "dead02").expect("id");
        journal
            .upsert_blocking(ended_record(&session_id, &owner.user))
            .expect("row");

        let error = registry
            .delete_session(&session_id, &stranger)
            .expect_err("different user must stay unauthorized");
        assert_eq!(error.code, ErrorCode::Unauthorized);
        assert!(
            journal
                .list()
                .expect("list")
                .iter()
                .any(|row| row.id == session_id),
            "unauthorized delete must leave the row"
        );
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_session_allows_dead_registry_entry_from_another_client_of_the_same_user() {
        let (dir, registry, journal) = tmp_delete_registry();
        let original = test_owner("S-1-5-21-1", "process-1111");
        let caller = test_owner("S-1-5-21-1", "process-2222");
        let session_id = compose_session_id(&original.session_token(), "dead03").expect("id");
        journal
            .upsert_blocking(ended_record(&session_id, &original.user))
            .expect("row");
        insert_transcript(&registry, &session_id, original);

        let result = registry.delete_session(&session_id, &caller);
        assert!(
            result.is_ok(),
            "same user, different client must delete a dead registry entry: {result:?}"
        );
        assert!(
            registry
                .inner
                .lock()
                .expect("registry")
                .get(&session_id)
                .is_none(),
            "dead registry entry must be removed"
        );
        assert!(journal
            .list()
            .expect("list")
            .iter()
            .all(|row| row.id != session_id));
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn delete_session_refuses_live_registry_entry_until_closed() {
        let (dir, registry, journal) = tmp_delete_registry();
        let original = test_owner("S-1-5-21-1", "process-1111");
        let caller = test_owner("S-1-5-21-1", "process-2222");
        let session_id = compose_session_id(&original.session_token(), "live01").expect("id");
        insert_live(&registry, &session_id, original);

        let error = registry
            .delete_session(&session_id, &caller)
            .expect_err("live session must refuse delete");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.message, "Close the session before deleting it.");
        assert!(
            registry
                .inner
                .lock()
                .expect("registry")
                .get(&session_id)
                .is_some(),
            "live registry entry must stay"
        );
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn learned_peer_session_id_is_durable_and_restored_on_hydration() {
        let (dir, registry, journal) = tmp_delete_registry();
        let original = test_owner("S-1-5-21-peer", "process-1111");
        let caller = test_owner("S-1-5-21-peer", "process-2222");
        let session_id = compose_session_id(&original.session_token(), "peer01").expect("id");
        let mut record = ended_record(&session_id, &original.user);
        record.kind = SessionKind::Acp;
        journal.upsert_blocking(record).expect("row");

        let runtime = SessionRuntime::with_journal(session_id.clone(), Some(Arc::clone(&journal)));
        runtime.set_peer_session_id("peer-session-1".to_string());
        journal.flush().expect("peer id");
        let row = journal
            .list()
            .expect("list")
            .into_iter()
            .find(|row| row.id == session_id)
            .expect("row");
        assert_eq!(row.peer_session_id.as_deref(), Some("peer-session-1"));

        let conn = ConnHandle::new(1);
        registry
            .attach(&session_id, None, &conn, &caller, true)
            .expect("same-user hydration");
        let hydrated = registry
            .inner
            .lock()
            .expect("registry")
            .get(&session_id)
            .expect("hydrated entry")
            .runtime()
            .peer_session_id();
        assert_eq!(hydrated.as_deref(), Some("peer-session-1"));
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_attach_allows_a_previous_run_session_for_the_same_user() {
        let (dir, registry, journal) = tmp_delete_registry();
        let original = test_owner("S-1-5-21-attach", "process-1111");
        let caller = test_owner("S-1-5-21-attach", "process-2222");
        let session_id = compose_session_id(&original.session_token(), "attach01").expect("id");
        journal
            .upsert_blocking(ended_record(&session_id, &original.user))
            .expect("row");
        registry
            .attach(&session_id, None, &ConnHandle::new(1), &caller, false)
            .expect("same user, different client must attach");
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_attach_allows_a_live_registry_session_from_a_dead_client_same_user() {
        // The most common restart shape: the daemon survives, the app does
        // not. The registry still holds the LIVE entry under the old client
        // token; the new client (same user) must attach through
        // runtime_for_user, not through journal hydration.
        let (dir, registry, journal) = tmp_delete_registry();
        let original = test_owner("S-1-5-21-attach-live", "process-1111");
        let caller = test_owner("S-1-5-21-attach-live", "process-2222");
        let session_id = compose_session_id(&original.session_token(), "attach03").expect("id");
        insert_live(&registry, &session_id, original);
        registry
            .attach(&session_id, None, &ConnHandle::new(1), &caller, false)
            .expect("same user, different client must attach to the live entry");
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_attach_rejects_a_live_registry_session_from_another_user() {
        let (dir, registry, journal) = tmp_delete_registry();
        let original = test_owner("S-1-5-21-attach-live-owner", "process-1111");
        let stranger = test_owner("S-1-5-21-attach-live-stranger", "process-2222");
        let session_id = compose_session_id(&original.session_token(), "attach04").expect("id");
        insert_live(&registry, &session_id, original);
        let error = registry
            .attach(&session_id, None, &ConnHandle::new(1), &stranger, false)
            .expect_err("different user must stay unauthorized on the live entry");
        assert_eq!(error.code, ErrorCode::Unauthorized);
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_attach_rejects_a_previous_run_session_from_another_user() {
        let (dir, registry, journal) = tmp_delete_registry();
        let original = test_owner("S-1-5-21-attach-owner", "process-1111");
        let stranger = test_owner("S-1-5-21-attach-stranger", "process-2222");
        let session_id = compose_session_id(&original.session_token(), "attach02").expect("id");
        journal
            .upsert_blocking(ended_record(&session_id, &original.user))
            .expect("row");
        let error = registry
            .attach(&session_id, None, &ConnHandle::new(1), &stranger, false)
            .expect_err("different user must stay unauthorized");
        assert_eq!(error.code, ErrorCode::Unauthorized);
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_close_allows_a_previous_run_session_for_the_same_user() {
        let (dir, registry, journal) = tmp_delete_registry();
        let original = test_owner("S-1-5-21-close", "process-1111");
        let caller = test_owner("S-1-5-21-close", "process-2222");
        let session_id = compose_session_id(&original.session_token(), "close01").expect("id");
        journal
            .upsert_blocking(ended_record(&session_id, &original.user))
            .expect("row");
        assert!(!registry
            .close(&session_id, &caller)
            .expect("same user, different client must close"));
        assert!(journal
            .list()
            .expect("list")
            .iter()
            .all(|row| row.id != session_id));
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_close_rejects_a_previous_run_session_from_another_user() {
        let (dir, registry, journal) = tmp_delete_registry();
        let original = test_owner("S-1-5-21-close-owner", "process-1111");
        let stranger = test_owner("S-1-5-21-close-stranger", "process-2222");
        let session_id = compose_session_id(&original.session_token(), "close02").expect("id");
        journal
            .upsert_blocking(ended_record(&session_id, &original.user))
            .expect("row");
        let error = registry
            .close(&session_id, &stranger)
            .expect_err("different user must stay unauthorized");
        assert_eq!(error.code, ErrorCode::Unauthorized);
        assert!(journal
            .list()
            .expect("list")
            .iter()
            .any(|row| row.id == session_id));
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_stop_allows_a_previous_run_live_session_for_the_same_user() {
        let (dir, registry, journal) = tmp_delete_registry();
        let original = test_owner("S-1-5-21-stop", "process-1111");
        let caller = test_owner("S-1-5-21-stop", "process-2222");
        let session_id = compose_session_id(&original.session_token(), "stop01").expect("id");
        insert_live(&registry, &session_id, original);
        registry
            .stop(&session_id, &caller)
            .expect("same user, different client must stop");
        assert!(registry
            .inner
            .lock()
            .expect("registry")
            .get(&session_id)
            .and_then(RegistryEntry::as_live)
            .is_some_and(|session| session.preserve_on_exit.load(Ordering::Acquire)));
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_stop_rejects_a_previous_run_live_session_from_another_user() {
        let (dir, registry, journal) = tmp_delete_registry();
        let original = test_owner("S-1-5-21-stop-owner", "process-1111");
        let stranger = test_owner("S-1-5-21-stop-stranger", "process-2222");
        let session_id = compose_session_id(&original.session_token(), "stop02").expect("id");
        insert_live(&registry, &session_id, original);
        let error = registry
            .stop(&session_id, &stranger)
            .expect_err("different user must stay unauthorized");
        assert_eq!(error.code, ErrorCode::Unauthorized);
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn roster_and_history_are_user_scoped_and_include_previous_run_sessions() {
        let (dir, registry, journal) = tmp_delete_registry();
        let previous_run = test_owner("S-1-5-21-roster", "process-1111");
        let caller = test_owner("S-1-5-21-roster", "process-2222");
        let stranger = test_owner("S-1-5-21-other", "process-3333");
        let previous_id =
            compose_session_id(&previous_run.session_token(), "roster01").expect("id");
        let stranger_id = compose_session_id(&stranger.session_token(), "roster02").expect("id");
        journal
            .upsert_blocking(ended_record(&previous_id, &previous_run.user))
            .expect("previous row");
        journal
            .upsert_blocking(ended_record(&stranger_id, &stranger.user))
            .expect("stranger row");

        let roster = registry.state_snapshots(&caller);
        assert!(roster.iter().any(|session| session.id == previous_id));
        assert!(roster.iter().all(|session| session.id != stranger_id));
        let history = registry.list(&caller).expect("history");
        assert!(history.iter().any(|session| session.id == previous_id));
        assert!(history.iter().all(|session| session.id != stranger_id));
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn live_transition_does_not_requery_the_journal_roster() {
        let (dir, registry, journal) = tmp_delete_registry();
        let owner = test_owner("S-1-5-21-roster-cache", "process-roster-cache");
        let runtime = insert_live_agent(&registry, "s.roster-cache.1", owner.clone());
        let journal_id =
            compose_session_id(&owner.session_token(), "roster-cache-history").expect("journal id");
        journal
            .upsert_blocking(ended_record(&journal_id, &owner.user))
            .expect("journal row");

        let _ = registry.state_snapshots(&owner);
        assert_eq!(registry.journal_list_call_count(), 1);

        runtime.publish_agent_event(
            SessionEvent::AgentFinished {
                stop_reason: "end_turn".to_string(),
                model_id: None,
                usage: None,
            },
            None,
        );
        let _ = registry.state_snapshots(&owner);

        assert_eq!(
            registry.journal_list_call_count(),
            1,
            "a live transition must reuse the cached journal roster"
        );
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn journal_roster_does_not_cache_rows_under_revision_that_changed_after_list() {
        let (dir, registry, journal) = tmp_delete_registry();
        let owner = test_owner("S-1-5-21-roster-race", "process-roster-race");
        let initial_id = compose_session_id(&owner.session_token(), "roster-race-initial")
            .expect("initial journal id");
        let added_after_list_id =
            compose_session_id(&owner.session_token(), "roster-race-after-list")
                .expect("post-list journal id");
        journal
            .upsert_blocking(ended_record(&initial_id, &owner.user))
            .expect("initial journal row");

        let hook_journal = Arc::clone(&journal);
        let hook_owner = owner.clone();
        let hook_id = added_after_list_id.clone();
        registry.set_journal_roster_after_list_hook(Arc::new(move || {
            hook_journal
                .upsert_blocking(ended_record(&hook_id, &hook_owner.user))
                .expect("post-list journal row");
        }));

        // The hook queues a real roster mutation after list() has returned,
        // deterministically reproducing the revision/data mismatch without
        // depending on sleeps or scheduler timing.
        let first = registry.state_snapshots(&owner);
        assert!(first
            .iter()
            .all(|session| session.id != added_after_list_id));

        let second = registry.state_snapshots(&owner);
        assert!(
            second
                .iter()
                .any(|session| session.id == added_after_list_id),
            "a row added after list() must not be hidden by a stale cache"
        );

        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn live_transition_does_not_rebuild_a_large_roster() {
        let (dir, registry, journal) = tmp_delete_registry();
        let owner = test_owner("S-1-5-21-large-roster", "process-large-roster");
        for index in 0..64 {
            let id = compose_session_id(
                &owner.session_token(),
                &format!("roster-history-{index:02}"),
            )
            .expect("journal id");
            journal
                .upsert_blocking(ended_record(&id, &owner.user))
                .expect("journal row");
        }
        let runtimes = (0..8)
            .map(|index| {
                insert_live_agent(&registry, &format!("s.large-roster-{index}"), owner.clone())
            })
            .collect::<Vec<_>>();

        let roster = registry.state_snapshots(&owner);
        assert_eq!(roster.len(), 72);
        assert_eq!(registry.full_roster_build_count(), 1);
        assert_eq!(registry.journal_list_call_count(), 1);

        runtimes[0].publish_agent_event(
            SessionEvent::AgentFinished {
                stop_reason: "end_turn".to_string(),
                model_id: None,
                usage: None,
            },
            None,
        );
        let updated = registry.state_snapshots(&owner);

        assert_eq!(updated.len(), 72);
        assert_eq!(registry.full_roster_build_count(), 1);
        assert_eq!(
            registry.journal_list_call_count(),
            1,
            "the transition must not make work proportional to journal-only sessions"
        );
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn resume_refuses_a_session_without_a_persisted_provider() {
        let owner = test_owner("S-1-5-21-resume-provider", "process-1111");
        let session_id = compose_session_id(&owner.session_token(), "resume01").expect("id");
        let mut record = ended_record(&session_id, &owner.user);
        record.kind = SessionKind::Acp;
        record.peer_session_id = Some("peer-session".to_string());
        let error = resume_handle(&record, &owner).expect_err("missing provider must refuse");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error.message.contains("provider was not persisted"));
    }

    #[test]
    fn resume_refuses_a_session_without_a_persisted_peer_id() {
        let owner = test_owner("S-1-5-21-resume-peer", "process-1111");
        let session_id = compose_session_id(&owner.session_token(), "resume02").expect("id");
        let mut record = ended_record(&session_id, &owner.user);
        record.kind = SessionKind::Acp;
        record.provider = Some("grok".to_string());
        let error = resume_handle(&record, &owner).expect_err("missing peer id must refuse");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error
            .message
            .contains("provider session id was not persisted"));
    }

    #[test]
    fn resume_refuses_a_session_from_another_user() {
        let owner = test_owner("S-1-5-21-resume-owner", "process-1111");
        let stranger = test_owner("S-1-5-21-resume-stranger", "process-2222");
        let session_id = compose_session_id(&owner.session_token(), "resume03").expect("id");
        let mut record = ended_record(&session_id, &owner.user);
        record.kind = SessionKind::Acp;
        record.provider = Some("grok".to_string());
        record.peer_session_id = Some("peer-session".to_string());
        let error = resume_handle(&record, &stranger).expect_err("wrong user must refuse");
        assert_eq!(error.code, ErrorCode::Unauthorized);
    }

    #[test]
    fn resume_owner_transfer_allows_the_resumer_and_rejects_a_third_client() {
        let (dir, registry, journal) = tmp_delete_registry();
        let original = test_owner("S-1-5-21-resume-transfer", "process-1111");
        let resumer = test_owner("S-1-5-21-resume-transfer", "process-2222");
        let third = test_owner("S-1-5-21-resume-transfer", "process-3333");
        let session_id = compose_session_id(&original.session_token(), "resume04").expect("id");
        insert_live(&registry, &session_id, original);
        {
            let mut map = registry.inner.lock().expect("registry");
            let entry = map.get_mut(&session_id).expect("live entry");
            entry.as_live_mut().expect("live session").owner = resumer.clone();
        }
        let map = registry.inner.lock().expect("registry");
        let entry = map.get(&session_id).expect("transferred entry");
        assert!(check_owner(entry, &resumer).is_ok());
        assert_eq!(
            check_owner(entry, &third)
                .expect_err("third client must not drive resumed session")
                .code,
            ErrorCode::Unauthorized
        );
        drop(map);
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn explicit_provider_is_not_hijacked_by_claude_env_override() {
        let (kind, provider, provenance) = SessionRegistry::resolve_session_provider(
            SessionKind::Acp,
            Some("grok".to_string()),
            Some("claude"),
        );
        assert_eq!(kind, SessionKind::Acp);
        assert_eq!(provider.as_deref(), Some("grok"));
        assert_eq!(provenance, Some(ProviderProvenance::Request));
    }

    #[test]
    fn claude_env_override_applies_when_the_request_has_no_provider() {
        let (kind, provider, provenance) =
            SessionRegistry::resolve_session_provider(SessionKind::Acp, None, Some("claude"));
        assert_eq!(kind, SessionKind::Claude);
        assert_eq!(provider, None);
        assert_eq!(provenance, None);
    }

    #[test]
    fn env_named_provider_is_marked_as_env_provenance() {
        let (kind, provider, provenance) =
            SessionRegistry::resolve_session_provider(SessionKind::Acp, None, Some("codex-acp"));
        assert_eq!(kind, SessionKind::Acp);
        assert_eq!(provider.as_deref(), Some("codex-acp"));
        assert_eq!(provenance, Some(ProviderProvenance::Env));
    }

    #[test]
    fn env_override_cannot_launch_npx_wrapper() {
        let error = SessionRegistry::env_override_cannot_launch_npx(
            "codex-acp",
            Some(ProviderProvenance::Env),
            Some(crate::provider_catalog::ProviderOrigin::NpxWrapper),
        )
        .expect_err("env npx must be denied");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "provider 'codex-acp' is an npx wrapper; npx wrappers require explicit selection, the env override cannot launch them"
        );
    }

    #[test]
    fn env_override_still_allows_native_user_binary() {
        SessionRegistry::env_override_cannot_launch_npx(
            "grok",
            Some(ProviderProvenance::Env),
            Some(crate::provider_catalog::ProviderOrigin::UserBinary),
        )
        .expect("env native must still resolve");
    }

    #[test]
    fn request_provided_npx_wrapper_is_not_blocked_by_env_policy() {
        SessionRegistry::env_override_cannot_launch_npx(
            "codex-acp",
            Some(ProviderProvenance::Request),
            Some(crate::provider_catalog::ProviderOrigin::NpxWrapper),
        )
        .expect("explicit npx is the consent path");
    }

    #[test]
    fn invalid_claude_effort_is_rejected_before_switcher() {
        let (dir, registry, journal) = tmp_delete_registry();
        let owner = test_owner("S-1-5-21-set-model", "process-set-model");
        let session_id = "claude-set-model";
        let runtime = Arc::new(SessionRuntime::with_journal(
            session_id.to_string(),
            registry.journal.clone(),
        ));
        runtime.store_claude_manifest(
            crate::claude_catalog::initial_manifest(crate::claude_catalog::fallback_models()),
            crate::claude_catalog::ClaudeCatalogState::Provisional,
        );
        let calls = Arc::new(AtomicU64::new(0));
        let metadata = Session {
            id: session_id.to_string(),
            workspace_id: None,
            cwd: None,
            kind: SessionKind::Claude,
            title: "Claude".to_string(),
            state: SessionState::Live { generation: 1 },
            elapsed_ms: Some(0),
            provider: Some("claude".to_string()),
            peer_session_id: None,
            created_at_ms: 1,
        };
        let session = PtySession {
            metadata,
            owner: owner.clone(),
            process_job: Arc::new(JobObject::new().expect("job")),
            master: None,
            killer: Box::new(NoopKiller),
            switcher: Some(Box::new(RecordingSwitcher(Arc::clone(&calls)))),
            stderr_handle: None,
            child_wait: None,
            writer: Arc::new(Mutex::new(Box::new(std::io::sink()))),
            reader_handle: None,
            coalesce_handle: None,
            runtime: Arc::clone(&runtime),
            exited: Arc::new(AtomicBool::new(false)),
            preserve_on_exit: Arc::new(AtomicBool::new(false)),
        };
        registry
            .inner
            .lock()
            .expect("registry")
            .insert(session_id.to_string(), RegistryEntry::Live(session));

        registry
            .set_model(session_id, &owner, Some("claude-opus-5"), None)
            .expect("a provisional catalog must not reject a model");
        assert_eq!(calls.load(Ordering::Acquire), 1);

        runtime.store_claude_catalog(SessionEvent::SessionManifest {
            provider_id: Some("claude".to_string()),
            current_model_id: Some("claude-sonnet-5".to_string()),
            models: vec![devboule_protocol::SessionModel {
                model_id: "claude-sonnet-5".to_string(),
                name: "Claude Sonnet 5".to_string(),
                description: None,
                context_tokens: None,
                current_effort: Some("high".to_string()),
                efforts: Some(vec![devboule_protocol::SessionModelEffort {
                    id: "high".to_string(),
                    label: "High".to_string(),
                    description: None,
                    default: Some(true),
                }]),
            }],
            modes: None,
        });

        let error = registry
            .set_model(session_id, &owner, Some("claude-bogus-999"), None)
            .expect_err("unknown model must be rejected before the switcher");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error.message.contains("not in the current catalog"));
        assert_eq!(calls.load(Ordering::Acquire), 1);

        let error = registry
            .set_model(session_id, &owner, None, Some("bogus"))
            .expect_err("unknown effort must be rejected before the switcher");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error.message.contains("not supported"));
        assert_eq!(calls.load(Ordering::Acquire), 1);

        registry
            .set_model(
                session_id,
                &owner,
                Some("claude-sonnet-5[1m]"),
                Some("high"),
            )
            .expect("model variants must use the base model catalog");
        assert_eq!(calls.load(Ordering::Acquire), 2);
        journal.shutdown();
        let _ = std::fs::remove_dir_all(dir);
    }

    fn tmp_registry_cache() -> std::path::PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        let process_id = std::process::id();
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or(0);
        let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("devboule-env-npx-{process_id}-{stamp}-{counter}"));
        std::fs::create_dir(&dir).expect("tmp dir");
        crate::registry::write_cache(&dir, crate::registry::TEST_REGISTRY_FIXTURE);
        dir
    }

    #[test]
    fn env_provided_npx_wrapper_is_denied_on_session_create() {
        let dir = tmp_registry_cache();
        let state = ServerState::with_paths(
            "test-instance".to_string(),
            RuntimePaths::from_dir(dir.clone()),
        )
        .expect("state");
        let owner = test_owner("S-1-5-21-env-npx", "process-env-npx");
        let error = state
            .sessions
            .create_with_provider_env(
                &state,
                &owner,
                None,
                SessionKind::Acp,
                None,
                None,
                Some("codex-acp"),
            )
            .expect_err("env npx create must fail");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(
            error.message,
            "provider 'codex-acp' is an npx wrapper; npx wrappers require explicit selection, the env override cannot launch them"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn env_provided_native_id_passes_env_reject_gate() {
        let dir = tmp_registry_cache();
        let paths = RuntimePaths::from_dir(&dir);
        SessionRegistry::reject_env_npx_wrapper("grok", Some(ProviderProvenance::Env), &paths)
            .expect("env native must pass the env gate");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn request_provided_npx_id_still_resolves_past_env_gate() {
        let dir = tmp_registry_cache();
        let paths = RuntimePaths::from_dir(&dir);
        SessionRegistry::reject_env_npx_wrapper(
            "codex-acp",
            Some(ProviderProvenance::Request),
            &paths,
        )
        .expect("request npx must pass the env gate");
        let agent = crate::provider_catalog::find_in_catalog(
            "codex-acp",
            &crate::registry::CdnRegistryFetch,
            &dir,
        )
        .expect("explicit npx id must still resolve in the catalog");
        assert_eq!(
            agent.origin,
            crate::provider_catalog::ProviderOrigin::NpxWrapper
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
