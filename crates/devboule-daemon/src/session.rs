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

use std::collections::{BTreeMap, HashMap};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, Weak};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use portable_pty::{Child, ChildKiller, MasterPty, PtySize};

#[cfg(test)]
use devboule_protocol::CursorShape;
use devboule_protocol::{
    compose_session_id, cursor_replay_ok, validate_attachment_references, validate_attachments,
    validate_session_id, ActiveTurnBehavior, AgentTaskState, AttachmentReference, Cursor,
    DelegationRunState, DelegationState, ErrorCode, ErrorDetails, FinishArtifact,
    FinishArtifactPart, FinishArtifactPartMetadata, JournalRetention, JournalStats, OwnerId,
    PermissionOutcome, Project, PromptAttachment, RetentionPatch, Session, SessionEvent,
    SessionKind, SessionModel, SessionOrigin, SessionOriginKind, SessionState,
    SessionStateSnapshot, UnattendedState, WireError, Workspace, WorkspaceIsolation,
    MAX_WRITE_BYTES,
};
#[cfg(test)]
use std::sync::Barrier;

use crate::attachment_store::AttachmentStore;
use crate::journal::{new_session_record, Journal, PersistStatus, SessionRecord};
use crate::mcp_broker::McpSessionGuard;
use crate::paths::RuntimePaths;
use crate::peer_policy::{ConnPeer, PeerRole};
use crate::process_tree::{JobObject, ProcessHandle};
#[cfg(test)]
use crate::screen::Screen;
use crate::server::ServerState;
#[cfg(test)]
use devboule_protocol::TranscriptIntegrity;

#[path = "permission_broker.rs"]
mod permission_broker;
/// The daemon-wide peer card allowance, re-exported for its disconnect call
/// site: the boundary that drops a peer connection lives in `server.rs`, and
/// the counters live beside the brokers that spend them (H2).
pub(crate) use permission_broker::release_peer_cards;
#[path = "session_runtime.rs"]
mod session_runtime;
pub(crate) use session_runtime::{
    roster_task_state, AgentMessageSnapshot, SessionRuntime, TurnToken,
};
#[path = "acp_client.rs"]
mod acp_client;
#[path = "acp_host.rs"]
mod acp_host;
#[path = "claude_client.rs"]
mod claude_client;
#[path = "codex_client.rs"]
mod codex_client;
#[path = "event_pull.rs"]
mod event_pull;
#[path = "pi_client.rs"]
mod pi_client;
/// The class-level provider seam: the `Provider` trait, one implementation
/// per family, and the registry the spawn road resolves through. Declared
/// from here like the other children; since pass 2b the mode lists live in
/// the impls, and `peer_policy`'s mode functions read them through the
/// registry — so the registry lookup is re-exported for that caller.
#[path = "provider.rs"]
mod provider;
/// Pi's mode dictionary, re-exported for the `unattended` derivation: the
/// vocabulary lives in the client that writes the permission extension, and
/// `peer_policy::unattended_mode` reads it from there without this module
/// growing any judgement of its own.
pub(crate) use pi_client::unattended_answer as pi_unattended_answer;
pub(crate) use provider::{apply_user_rows, catalog_registry, native_family_ids, ProviderRegistry};
#[path = "session_types.rs"]
mod session_types;
#[path = "shell_command.rs"]
mod shell_command;
#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;

pub use event_pull::ConnHandle;
pub(crate) use session_types::PendingEvent;
pub use session_types::PtyCommand;
use session_types::{
    Disposition, OutputMetrics, PendingItem, PullState, RegistryEntry, TranscriptSession,
};
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

pub(super) trait SessionSteerer: Send + Sync {
    /// Deliver `text` into the turn the caller admitted.
    ///
    /// `turn` carries the admission: it is handed out by
    /// [`SessionRuntime::with_active_turn`] only while the daemon turn the
    /// caller checked is still the running one, and it holds the lock the
    /// `AgentFinished` transition takes until the adapter has issued its write
    /// (or released the hold through `TurnToken::write_then_release`, for a
    /// provider whose command is a round-trip). An adapter that cannot take a
    /// steer for that turn answers `Ok(false)` without writing; a transport
    /// failure is `Err`.
    fn steer_active_turn(
        &mut self,
        _text: &str,
        _turn: &mut TurnToken<'_>,
    ) -> Result<bool, WireError> {
        Ok(false)
    }
    fn clone_steerer(&self) -> Box<dyn SessionSteerer>;
}

struct UnsupportedSteerer;

impl SessionSteerer for UnsupportedSteerer {
    fn clone_steerer(&self) -> Box<dyn SessionSteerer> {
        Box::new(Self)
    }
}

pub(super) trait ModelSwitcher: Send + Sync {
    fn set_model(&self, model_id: Option<&str>, effort: Option<&str>) -> Result<(), WireError>;
    fn set_mode(&self, _mode_id: &str) -> Result<(), WireError> {
        Err(WireError::new(
            ErrorCode::InvalidRequest,
            "This provider does not support switching the session mode.",
        ))
    }
    fn manifest(&self) -> Option<SessionEvent> {
        None
    }
    fn clone_switcher(&self) -> Box<dyn ModelSwitcher>;
    fn clone_steerer(&self) -> Box<dyn SessionSteerer> {
        Box::new(UnsupportedSteerer)
    }
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
    steerer: Box<dyn SessionSteerer>,
    switcher: Option<Box<dyn ModelSwitcher>>,
    /// This is separate from the stdout reader: stderr must never be able to
    /// fill its pipe and stop the ACP child from producing responses.
    stderr_handle: Option<JoinHandle<()>>,
    child_wait: Option<JoinHandle<Option<u32>>>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    /// Structured prompt route for ACP image blocks, next to the plain-text
    /// `writer`. `Some` only for an ACP session: the spawn path clones the
    /// transport's request pieces here, and the other three providers and
    /// every terminal session leave it `None`. Whether a prompt *uses* it is
    /// a second, per-prompt capability decision (see [`ImageDelivery`]) — a
    /// present sibling with an Absent/Unsupported verdict still falls back
    /// to the path line. Locking: the sibling is an `Arc` cloned out of the
    /// registry lock next to `writer`; the structured send itself runs under
    /// the writer hold, in the same order the pre-existing `AcpWriter` path
    /// already used, so no new ordering is introduced.
    image_sink: Option<Arc<AcpPromptSink>>,
    /// Structured prompt route for a provider whose protocol carries images
    /// but whose handshake says nothing the daemon reads (Claude, Codex,
    /// Pi). `Some` only for those three sessions: the ACP route has the
    /// sibling above, and a terminal session leaves this `None`. The route
    /// owns the decision, the text and the frame for one prompt (see
    /// [`StaticImageSink`] and [`PlannedStaticPrompt`]): the send path plans
    /// it outside the writer lock and sends it under that hold, the shape the
    /// ACP sibling above already uses.
    static_image_sink: Option<Arc<dyn StaticImageSink>>,
    reader_handle: Option<JoinHandle<()>>,
    coalesce_handle: Option<JoinHandle<()>>,
    runtime: Arc<SessionRuntime>,
    mcp_session: Option<McpSessionGuard>,
    exited: Arc<AtomicBool>,
    /// Set by `stop`: the process dies but the session object stays. The
    /// reader must not remove the registry entry or call session_finished.
    preserve_on_exit: Arc<AtomicBool>,
}

pub(crate) struct SpawnedSession {
    process_job: JobObject,
    master: Option<Arc<Mutex<Box<dyn MasterPty + Send>>>>,
    killer: Box<dyn SessionKiller>,
    switcher: Option<Box<dyn ModelSwitcher>>,
    child: Box<dyn WaitableChild>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    /// Structured prompt route for ACP image blocks; `None` for the other
    /// three providers and for terminal sessions. Carried through spawn so
    /// `start_spawned_session` can install it next to `writer`.
    image_sink: Option<Arc<AcpPromptSink>>,
    /// Structured prompt route for the three providers the daemon statically
    /// knows carry images (Claude, Codex, Pi); `None` for an ACP session and
    /// for a terminal. Carried through spawn so `start_spawned_session` can
    /// install it next to `image_sink`.
    static_image_sink: Option<Arc<dyn StaticImageSink>>,
    reader: Box<dyn Read + Send>,
    /// ACP supplies a structured decoder. Terminal sessions use the shared
    /// byte coalescer, which is constructed by `start_spawned_session`.
    reader_dispatch: Option<Box<dyn ReaderDispatch>>,
    stderr: Option<Box<dyn StderrSource>>,
    permission_broker: Option<Arc<permission_broker::PermissionBroker>>,
    os_handle: Option<ProcessHandle>,
    peer_session_id: Option<String>,
    agent_version: Option<String>,
    /// A profile delivery the client could not apply before its session
    /// reader existed. Pi's switch is an awaited control rpc, and the only
    /// code that can deliver its answer is the reader thread this module
    /// starts, so the client hands the rpc over instead of blocking on an
    /// answer nobody can give yet. [`start_spawned_session`] runs it once
    /// that reader is live; a refusal tears the child down and fails the
    /// creation before any prompt can reach it. Every other client delivers
    /// inside its own `spawn_process` and passes `None`.
    pending_delivery: Option<Box<dyn FnOnce() -> Result<(), WireError> + Send>>,
    /// A Codex MCP verification to run detached once the session reader is
    /// live (S7/S8). The startup never waits for it and no outcome is fatal:
    /// [`start_spawned_session`] spawns one thread that polls
    /// `mcpServerStatus/list` and flips the runtime's `ToolsState`. Present
    /// only when a carrier was installed (`Some` road); `None` — today's only
    /// road — changes nothing.
    pending_codex_verify: Option<codex_client::CodexVerifyBundle>,
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
    // Read before the record is consumed field by field below.
    let context_id = record.context();
    let kind = record.kind.clone();
    Session {
        id: session_id.to_string(),
        workspace_id: record.workspace_id,
        cwd: Some(crate::workspace::display_path(
            &command.cwd.to_string_lossy(),
        )),
        // The record's own kind, which is the session's kind: it was decided
        // at create and journalled, and a resume does not re-decide it.
        //
        // Pass 2c derived this from the provider string instead
        // (`provider_for(&provider).wire_kind()`), to keep a future family's
        // resume from being reported as ACP. That was wrong, and the MAX
        // RECALL found why: `provider` is a **string on a row that can
        // disagree with its own kind**. `DEVBOULE_ACP_PROVIDER_ID` reaches
        // `command.provider_id` without passing the native-id strip
        // (`acp_client.rs`), so a journal row can read `kind=acp,
        // provider=codex` — and deriving from it stamped `Codex` on a session
        // whose peer is ACP. That is not a label: `start_spawned_session`
        // installs the stamped kind on the runtime, which then drives
        // `mcp_gates_first_prompt` (skipping the MCP invariant) and
        // `event_pull`'s `is_codex` (replaying ACP envelopes through the
        // Codex view). Reading the record keeps the old constant's answer for
        // every ACP row AND stays right for a future family, because that
        // family's rows carry its own kind.
        kind,
        title: record.title,
        provider: Some(provider),
        peer_session_id: Some(peer_session_id),
        state: SessionState::Live { generation },
        elapsed_ms: Some(0),
        created_at_ms: record.created_at_ms,
        // Resume does not re-origin a session: the row keeps the device that
        // created it.
        origin: record.origin.clone(),
        // Both of these are the journal's now (audit S5-12): a resumed session
        // is the same session, so it comes back under the name the human saw
        // and with the parent it was created by.
        display_name: record.display_name,
        created_by: record.created_by,
        // And so are the creation-from-profile facts (v11): the profile it was
        // started from, the context it belongs to, the marker it was born with
        // and its labels. A resume is not a creation, so none of them is
        // re-derived here — a child born `yes` comes back `yes` even if its
        // profile has been un-ticked or edited in the meantime.
        profile_id: record.profile_id,
        context_id: Some(context_id),
        unattended: record.unattended_state,
        labels: record.labels,
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

/// The refusal an id-addressed peer call gets when the id names an entry
/// that is still inside its delivery window (the re-audit's P2-1): the
/// session does not exist for its peers until the profile's delivery has
/// landed, so the honest answer is `SessionNotFound`, not "gone" — nothing
/// was ever visible to lose.
fn not_found_while_configuring(entry: &RegistryEntry) -> WireError {
    if entry.is_configuring() {
        not_found()
    } else {
        process_gone()
    }
}

/// The one door an id-addressed peer call resolves its id through: the
/// entry must exist, belong to this owner, and be past its delivery window.
/// A `Configuring` entry answers `SessionNotFound` here, because the session
/// does not exist for peers until the delivery has landed (the re-audit's
/// P2-1/P2-2 — the variant's own doc claims this refusal, and this door is
/// what makes the claim true rather than a per-site edit). A new peer path
/// cannot forget the window: there is no second lookup that skips it.
/// Daemon-side readers — teardown, EOF reaping, handle storage, the resume
/// guard — do not go through this door; they ask
/// `RegistryEntry::as_child_process` directly. `delete_session` cannot
/// resolve through the door either (an id absent from the map must fall
/// through to its journal-only branch), but for an entry the map holds it
/// repeats the door's answer — `Configuring` is refused `SessionNotFound`
/// there too, before its own close-first guard.
fn peer_entry<'a>(
    map: &'a HashMap<String, RegistryEntry>,
    session_id: &str,
    owner: &OwnerId,
    conn_peer: &Option<ConnPeer>,
) -> Result<&'a RegistryEntry, WireError> {
    let entry = map.get(session_id).ok_or_else(not_found)?;
    check_user_owner(entry, owner, conn_peer)?;
    if entry.is_configuring() {
        return Err(not_found());
    }
    Ok(entry)
}

/// The mutable half of [`peer_entry`].
fn peer_entry_mut<'a>(
    map: &'a mut HashMap<String, RegistryEntry>,
    session_id: &str,
    owner: &OwnerId,
    conn_peer: &Option<ConnPeer>,
) -> Result<&'a mut RegistryEntry, WireError> {
    let entry = map.get_mut(session_id).ok_or_else(not_found)?;
    check_user_owner(entry, owner, conn_peer)?;
    if entry.is_configuring() {
        return Err(not_found());
    }
    Ok(entry)
}

fn unauthorized() -> WireError {
    WireError::new(
        ErrorCode::Unauthorized,
        "This client is not authorized to use that session.",
    )
}

/// The origin a create from this connection writes.
///
/// A local connection — and a `Local` peer identity — is the person at this
/// machine. A remote one is the paired device with the role it was paired as,
/// so the stored origin can be rendered on a permission card and scoped on by
/// the `Daemon` role's ownership branch.
pub(crate) fn session_origin_for(conn_peer: &Option<ConnPeer>) -> SessionOrigin {
    match conn_peer {
        Some(ConnPeer::Remote {
            device_id, role, ..
        }) => SessionOrigin::peer(device_id.clone(), *role),
        _ => SessionOrigin::local(),
    }
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

fn check_user_owner(
    entry: &RegistryEntry,
    owner: &OwnerId,
    conn_peer: &Option<ConnPeer>,
) -> Result<(), WireError> {
    match conn_peer {
        // A paired `Client` speaks for the person who paired it: the register
        // of sessions it reaches is that user's, and only that user's. The
        // effective owner `server.rs` hands down is already that SID, so the
        // comparison here is the same one a local call makes — stated in the
        // role branch anyway, because "the peer reaches its paired user" is a
        // rule about the role, not a side effect of how dispatch built the
        // owner (`DESIGN-remote-agents.md` §8b A3).
        Some(ConnPeer::Remote {
            role: PeerRole::Client,
            paired_by_user,
            ..
        }) => match paired_by_user.as_deref() {
            Some(paired) if entry.owner().user == paired => Ok(()),
            // No recorded pairing user, or another account's session: refuse.
            _ => Err(unauthorized()),
        },
        // A `Daemon` peer's scope is the *origin*, not the owner name (§8 R2):
        // the sessions it created here, and nothing else. A session this
        // device created is refused even when the owner comparison would pass,
        // because the origin is the authority A3 names.
        Some(ConnPeer::Remote {
            role: PeerRole::Daemon,
            device_id,
            ..
        }) => {
            let origin = entry.origin();
            let own_origin = origin.kind == SessionOriginKind::Peer
                && origin.device_id.as_deref() == Some(device_id.as_str());
            if own_origin && entry.owner().user == owner.user {
                Ok(())
            } else {
                Err(unauthorized())
            }
        }
        // The pipe: the person at this machine, exactly as before.
        _ => {
            if entry.owner().user == owner.user {
                Ok(())
            } else {
                Err(unauthorized())
            }
        }
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

/// The prompt the writer receives: the user's text, a blank line, then one line
/// per attachment naming the absolute path its bytes were written to.
///
/// The line is Paseo's shape (`[Image available at: <path>]`), and every
/// attachment gets one — including an SVG, which no provider accepts as an
/// inline image block, so a path on disk is its only route to the agent both
/// now and after the per-provider blocks land. Nothing else about the prompt
/// changes, which is what lets the four provider writers stay untouched.
///
/// Every file is written before any of the text is built: a request that fails
/// on its third attachment leaves nothing to clean up out of the prompt that a
/// half-built string would otherwise have implied.
fn with_attachment_paths(
    store: &AttachmentStore,
    session_id: &str,
    text: &str,
    attachments: &[PromptAttachment],
) -> Result<String, WireError> {
    if attachments.is_empty() {
        return Ok(text.to_string());
    }
    let session = store
        .session(session_id)
        .ok_or_else(|| WireError::new(ErrorCode::InvalidRequest, "Invalid session id."))?;
    let mut paths = Vec::with_capacity(attachments.len());
    for attachment in attachments {
        paths.push(session.materialize(attachment)?);
    }
    let mut prompt = String::from(text);
    prompt.push_str("\n\n");
    push_path_lines(&mut prompt, &paths);
    Ok(prompt)
}

/// One `[Image available at: <path>]` line per path, separated by newlines.
///
/// The one place that line is written, so the inline attachments and the
/// resolved references cannot come out as two shapes: `with_attachment_paths`,
/// `prompt_text_with_fallback_paths` and `push_reference_path_lines` all write
/// their block through it, and each opens the block with its own separator.
fn push_path_lines(prompt: &mut String, paths: &[PathBuf]) {
    for (index, path) in paths.iter().enumerate() {
        if index > 0 {
            prompt.push('\n');
        }
        prompt.push_str("[Image available at: ");
        prompt.push_str(&path.to_string_lossy());
        prompt.push(']');
    }
}

/// Appends one path line per resolved reference to `prompt`, after whatever
/// path lines it already carries.
///
/// The references come last and in the order the client listed them: every
/// caller appends this after the inline attachments' own lines, which are the
/// paths this request's own bytes were written to. The block opens the way the
/// inline one does (`\n\n`), so a prompt that carries both reads as the inline
/// attachments first and the stored ones after them. An empty slice appends
/// nothing, and a prompt with no references is byte for byte what it was
/// before this existed.
///
/// # Why a reference is never an inline image block
///
/// Not an oversight, and not a missing case in the image-block routes: a
/// reference is a line here even on a provider that negotiated `image`
/// support. The whole reason a reference exists is that its bytes must not
/// travel in the frame — a deck is forty pages, and the frame is what the
/// deposit was made to keep them out of. Resolving one back into an image
/// block at the send would undo the deposit, spend the frame cap the deposit
/// saved, and hand the provider the same bytes by a longer road.
///
/// The path is the store's own absolute one. A reference carries a digest and
/// a size and nothing else, so there is no client-supplied name here to quote
/// and nothing untrusted to bound.
fn push_reference_path_lines(prompt: &mut String, reference_paths: &[PathBuf]) {
    if reference_paths.is_empty() {
        return;
    }
    prompt.push_str("\n\n");
    push_path_lines(prompt, reference_paths);
}

/// The paths of the stored attachments this request names, or the first reason
/// one of them cannot be sent.
///
/// The order is the point, and it is the order `deposit` keeps. The wire's own
/// rule for references runs before any of this
/// (`validate_attachment_references`, called on the send path before this
/// function), so a reference naming another session, a digest that is not a
/// digest, and a list past the count or the total budget are refused without a
/// lookup. Then, per reference, the store resolves the digest to a path inside
/// this session's folder — its own read side, which refuses a session folder
/// that is a link and a digest with no file behind it.
///
/// The size is compared, and a disagreement is a refusal rather than a
/// warning. `resolve` answers with the size the file *has*; the reference's
/// `stored_bytes` is advisory by the wire's own documentation and is never the
/// number the daemon trusts. A request that names a size the file does not
/// have is naming something it did not deposit, and handing a provider a path
/// to a file whose identity is in question is the substitution this whole path
/// exists to prevent.
///
/// Every reference is resolved before the caller builds a line of the prompt,
/// for the reason [`with_attachment_paths`] gives about the inline ones: a
/// request that fails on its third item must leave nothing half-built.
fn resolve_attachment_references(
    store: &AttachmentStore,
    session_id: &str,
    references: &[AttachmentReference],
) -> Result<Vec<PathBuf>, WireError> {
    let mut paths = Vec::with_capacity(references.len());
    for reference in references {
        // No extension hint: a reference carries a session, a digest and a
        // size, and no MIME type, so this caller knows nothing that would name
        // the file. The store's listing answers instead.
        let (path, stored_bytes) = store.resolve(session_id, &reference.digest, None)?;
        if stored_bytes != reference.stored_bytes {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                stored_size_mismatch_message(reference, stored_bytes),
            ));
        }
        paths.push(path);
    }
    Ok(paths)
}

/// The refusal for a reference whose `stored_bytes` is not the size the file
/// on disk has.
///
/// The digest is echoed and nothing else is: by the time this runs the digest
/// is 64 lowercase hex characters — `validate_attachment_references` refused
/// every other spelling before the store was asked, and `resolve` refuses a
/// non-digest again without echoing it — so there is no unbounded string here
/// to bound. Both numbers travel because together they are the whole
/// disagreement: which file, and how far the request's claim is from it.
fn stored_size_mismatch_message(reference: &AttachmentReference, stored_bytes: u64) -> String {
    format!(
        "The stored attachment '{}' is {stored_bytes} bytes; the request named {}.",
        reference.digest, reference.stored_bytes
    )
}

/// What the ACP handshake negotiated about sending images to the agent.
///
/// Two different kinds of fact: `NegotiatedImageBlock` came from this
/// session's `initialize` reply (`agentCapabilities.promptCapabilities.image`,
/// read by [`crate::acp_view::prompt_capabilities_from_initialize`]), while
/// `StaticImageBlock` is what this daemon knows about a provider whose
/// handshake says nothing — a fact about the protocol, not a fact the peer
/// agreed to. They are stored in one enum because the send path asks one
/// question (`may I send bytes?`), and that question must be answered the
/// same way whatever the source: only `Supported` is yes. `Absent` (the agent
/// said nothing) and `Unsupported` (the agent explicitly refused) are both
/// no, because Unknown must never silently mean yes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ImageDelivery {
    /// No image-capable route is available: fall back to the path line.
    #[default]
    PathLine,
    /// The ACP handshake negotiated `promptCapabilities.image == true` for
    /// this session. The only yes.
    NegotiatedImageBlock,
    /// A provider whose protocol carries images but whose handshake says
    /// nothing the daemon reads: Claude, Codex and Pi each answer with it, and
    /// their plans read it through this variant rather than against a literal.
    StaticImageBlock,
}

impl ImageDelivery {
    /// True only for a negotiated `Supported`. `Absent` and `Unsupported`
    /// both fall back to the path line: silence is not consent, and a
    /// refusal is not consent either.
    fn allows_image_block(state: crate::acp_view::PromptCapabilityState) -> bool {
        matches!(state, crate::acp_view::PromptCapabilityState::Supported)
    }

    /// Maps one handshake verdict to the delivery it authorises.
    pub(crate) fn from_negotiated(state: crate::acp_view::PromptCapabilityState) -> Self {
        if Self::allows_image_block(state) {
            Self::NegotiatedImageBlock
        } else {
            Self::PathLine
        }
    }
}

/// One image block for a `session/prompt` content array.
///
/// The bytes are the STRIPPED bytes — read back from the file
/// [`crate::attachment_store::SessionAttachments::materialize`] wrote, never
/// the base64 that arrived on the wire — so nothing leaving the house carries
/// identity metadata. The mime type is the stored attachment's declared type,
/// which `materialize` already checked against the sniffed container (a file
/// whose bytes and label disagree is refused, never stored).
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct AcpImageBlock {
    pub mime_type: String,
    pub data_base64: String,
}

/// Hand-written, and it must stay hand-written: `data_base64` holds a whole
/// image. One rendered PDF page is ~128 KiB of base64 and a deck is forty of
/// them, so a derived `Debug` would let any `{:?}` — a failing `assert_eq!`,
/// a log line, an error path, a future panic — spill the user's picture into
/// somewhere it was never meant to go. What a person debugging needs is the
/// type and the size; the bytes have never once been the answer.
impl std::fmt::Debug for AcpImageBlock {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AcpImageBlock")
            .field("mime_type", &self.mime_type)
            .field("data_base64_len", &self.data_base64.len())
            .finish()
    }
}

impl AcpImageBlock {
    /// Reads the stripped file back and encodes it for the wire. Reading the
    /// file — rather than keeping a parallel copy of the pre-strip bytes — is
    /// what guarantees the block carries what is on disk.
    fn from_stored_file(path: &std::path::Path, mime_type: &str) -> Result<Self, std::io::Error> {
        use base64::Engine;
        let bytes = std::fs::read(path)?;
        Ok(Self {
            mime_type: mime_type.to_string(),
            data_base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
        })
    }

    fn to_content_block(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "image",
            "mimeType": self.mime_type,
            "data": self.data_base64,
        })
    }
}

/// A structured sender for ACP prompts.
///
/// Two halves, one decision. The DECISION — which attachments become image
/// blocks, what text the journal records — is [`plan_structured_prompt`]: a
/// pure function of the request's `(text, attachments)`, tested directly
/// below without spawning a child. The DELIVERY — handing that plan to the
/// child — is [`AcpPromptSink::send_structured_prompt`], which needs the
/// live transport.
///
/// The plan carries both halves the send path needs: `fallback_text` (the
/// user's text plus the path lines for the non-raster attachments — today:
/// SVG, which no provider accepts inline) is the text block AND the exact
/// string the journal records, so the transcript can never carry image
/// base64; `images` are the blocks that travel. One constructor builds both,
/// so the journaled string and the sent text block cannot drift apart.
pub(crate) struct StructuredPromptPlan {
    /// The text block: the user's text plus one path line per non-raster
    /// attachment. Also the exact string the journal records.
    pub fallback_text: String,
    /// One block per raster attachment, in attachment order.
    pub images: Vec<AcpImageBlock>,
}

impl StructuredPromptPlan {
    /// The full `prompt` array the child receives: the text block, then one
    /// image block per raster attachment. The journal records
    /// `fallback_text` — element zero of this array — never the blocks.
    /// `pub(crate)` for the acp_client wire-shape test, which pins the
    /// exact JSON the read side already expects.
    pub(crate) fn content_blocks(&self) -> Vec<serde_json::Value> {
        let mut prompt = vec![serde_json::json!({ "type": "text", "text": self.fallback_text })];
        prompt.extend(self.images.iter().map(AcpImageBlock::to_content_block));
        prompt
    }
}

// HARD-WRITTEN ON PURPOSE — DO NOT REPLACE WITH `#[derive(Debug)]`.
//
// This struct carries the base64 of every attached image (one rendered PDF
// page is ~128 KiB, a deck is forty of them) and the user's own prompt text.
// A derived `Debug` would leave both exactly one `{:?}` away from a log line,
// a journal row, an assertion message or a future panic hook, and the standing
// rule from the earlier frame audit is that frame contents stay out of `Debug`
// output. This impl prints what a human debugging a prompt needs and nothing
// more: one `(mime type, base64 length in bytes)` pair per image block, plus
// the length of the text block. Never the base64, never the text.
//
// Each pair is a `&str` label and a `usize`, so this impl cannot copy base64
// into its output even by accident — the leak is excluded by the types, not by
// remembering. Note that `AcpImageBlock` above still derives `Debug`; do not
// route this impl (or anything else that renders a block) through it.
impl std::fmt::Debug for StructuredPromptPlan {
    /// The block as `(mime type, base64 byte length)`: the two facts a human
    /// needs to size a prompt up, and nothing that carries image bytes.
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let images: Vec<(&str, usize)> = self
            .images
            .iter()
            .map(|block| (block.mime_type.as_str(), block.data_base64.len()))
            .collect();
        formatter
            .debug_struct("StructuredPromptPlan")
            .field("images", &images)
            .field("fallback_text_bytes", &self.fallback_text.len())
            .finish()
    }
}

/// Decides the structured prompt for one request: materializes every
/// attachment (exactly the call `with_attachment_paths` makes — a request
/// that fails on its third attachment leaves nothing half-built), turns each
/// raster (`image/png`, `image/jpeg`) into an image block carrying the
/// STRIPPED bytes read back from disk, and keeps every other attachment
/// (today: `image/svg+xml`) as a path line in the text. A prompt can
/// therefore carry both blocks and path lines at once. Returns `None` when
/// there is nothing to send inline (no attachments, or an SVG-only prompt),
/// in which case the caller takes the legacy path-line write.
fn plan_structured_prompt(
    store: &AttachmentStore,
    session_id: &str,
    text: &str,
    attachments: &[PromptAttachment],
) -> Result<Option<StructuredPromptPlan>, WireError> {
    if attachments.is_empty() {
        return Ok(None);
    }
    let session = store
        .session(session_id)
        .ok_or_else(|| WireError::new(ErrorCode::InvalidRequest, "Invalid session id."))?;
    let mut images = Vec::new();
    let mut fallback_paths = Vec::new();
    for attachment in attachments {
        let path = session.materialize(attachment)?;
        if crate::raster_metadata::RasterMime::from_mime_type(&attachment.mime_type).is_some() {
            images.push(
                AcpImageBlock::from_stored_file(&path, &attachment.mime_type).map_err(|error| {
                    WireError::new(
                        ErrorCode::Io,
                        format!("Could not read a stored attachment: {error}"),
                    )
                })?,
            );
        } else {
            fallback_paths.push(path);
        }
    }
    if images.is_empty() {
        // SVG-only (or otherwise non-raster) on a capable session: nothing
        // would travel inline, so stay on the legacy write rather than
        // materializing twice — the fallback below re-materializes from the
        // content-addressed store, which is a wasted decode and strip, not a
        // double write, but there is no reason to pay it.
        return Ok(None);
    }
    Ok(Some(StructuredPromptPlan {
        fallback_text: prompt_text_with_fallback_paths(text, &fallback_paths),
        images,
    }))
}

/// The DELIVERY half: the text plus any image blocks land
/// as one `session/prompt` `prompt` array, instead of as a text blob with
/// path lines appended. It holds an `Arc` to the session's transport — the
/// same transport the plain-text [`Write`] half writes through — so both
/// halves share one session id, one request-id sequence, one pending table,
/// and the live negotiated capability slot.
///
/// The sibling is `Some` only for an ACP session; it is `None` for the other
/// three providers and for every terminal session. Whether it is *used* is a
/// second, per-prompt decision read from the negotiated capability (see
/// [`ImageDelivery`]): a present sibling with an Absent/Unsupported verdict
/// still falls back to the path line. The plain `Write` trait on `writer` is
/// untouched — text-only writes keep flowing through exactly the path they
/// use today.
///
/// There is deliberately no test seam here. One was written — a recording
/// double behind the delivery call — for a journal test on the structured
/// route that was never finished, and it sat unreachable: a seam shaped for
/// an imagined test, which is the shape least likely to fit the test someone
/// eventually writes. What the decision produces is pinned instead by
/// [`plan_structured_prompt`], which is pure and runs before anything is
/// sent, so the tests assert the exact value production would deliver. When
/// the journal on this route does get covered, the seam it needs should be
/// built against that test rather than ahead of it.
pub(crate) struct AcpPromptSink {
    transport: Arc<acp_client::AcpTransport>,
}

impl AcpPromptSink {
    fn new(transport: &Arc<acp_client::AcpTransport>) -> Self {
        Self {
            transport: Arc::clone(transport),
        }
    }

    /// The delivery this prompt is authorised for, read live from the
    /// session's negotiated capability — not from a copy taken at spawn.
    /// A `session/load` handshake re-derives the verdict like the rest of
    /// the negotiated state, so a resumed session cannot send on a stale yes.
    pub(crate) fn delivery(&self) -> ImageDelivery {
        ImageDelivery::from_negotiated(self.transport.prompt_capabilities().image)
    }

    /// Sends one planned prompt as structured content: the plan's text block,
    /// then one image block per raster attachment, in attachment order.
    /// Takes the whole plan (not `fallback_text` + `images` separately) so
    /// the text block the child receives and the string the journal records
    /// are the same value by construction — a later edit cannot pass one
    /// string to the wire and journal another.
    pub(crate) fn send_structured_prompt(
        &self,
        plan: StructuredPromptPlan,
    ) -> Result<(), WireError> {
        // The capability is re-read here, at send time: only a negotiated
        // `Supported` takes this path. Anything else never reaches the sink
        // — the caller falls back to the path line instead.
        debug_assert!(matches!(
            self.delivery(),
            ImageDelivery::NegotiatedImageBlock
        ));
        self.deliver(plan)?;
        Ok(())
    }

    /// The delivery call: the plan's content blocks go to the child through
    /// the shared transport. Kept separate from
    /// [`Self::send_structured_prompt`] so the capability re-read and the
    /// write stay two readable steps rather than one.
    fn deliver(&self, plan: StructuredPromptPlan) -> Result<(), WireError> {
        self.transport
            .send_structured_prompt(plan.content_blocks())
            .map_err(|error| {
                WireError::new(
                    ErrorCode::Io,
                    format!("Could not send input to the terminal: {error}"),
                )
            })?;
        Ok(())
    }
}

/// The static counterpart of [`AcpPromptSink`]: the structured prompt route
/// for a provider whose protocol carries images but whose handshake says
/// nothing the daemon reads — Claude, Codex and Pi, the three that answer
/// [`ImageDelivery::StaticImageBlock`].
///
/// Planning and sending are two steps here for the same reason they are two
/// on the ACP route: the base64 decode, the container sniff and the strip
/// walk run before the writer is locked, and the frame goes out under that
/// hold, so the journal entry that follows keeps the order the child sees.
pub(crate) trait StaticImageSink: Send + Sync {
    /// Decides one prompt: materializes every attachment exactly once and
    /// answers with the plan — or `None` when this route does not run for the
    /// request, which is no attachments at all or a provider that is not
    /// authorised for inline bytes right now (a Pi model that declared no
    /// `image`, an unknown model, no current model). A `None` means nothing
    /// was materialized either, so the caller's legacy path-line write is the
    /// only walk of this request.
    ///
    /// The text travels inside the plan rather than beside it, so the string
    /// the frame carries and the string the journal records cannot be two
    /// different values, and so the caller never has to walk the attachments
    /// a second time through [`with_attachment_paths`].
    fn plan_prompt(
        &self,
        store: &AttachmentStore,
        session_id: &str,
        text: &str,
        attachments: &[PromptAttachment],
    ) -> Result<Option<Box<dyn PlannedStaticPrompt>>, WireError>;
}

/// One static provider's planned prompt: the text it carries and whatever that
/// provider's own protocol sends beside it — Claude's `content[]` blocks,
/// Codex's `localImage` paths, Pi's `images[]` entries.
///
/// The blocks may be empty. A prompt whose attachments all take a path line
/// (an SVG, a gif whose container no walk follows) still travels as a plan,
/// and the frame it sends is the text-only frame byte for byte — which is what
/// keeps one send at one materialization per attachment, on every send.
pub(crate) trait PlannedStaticPrompt: Send + Sync {
    /// The text the frame carries — the same value the journal records.
    fn text(&self) -> &str;

    /// Appends one path line per resolved reference to the text this plan
    /// carries, after the path lines its own attachments left behind.
    ///
    /// A mutation rather than a parameter of
    /// [`StaticImageSink::plan_prompt`] because the plan owns its text on
    /// purpose: the frame this provider builds and the string the caller
    /// journals are one value, so the references have to be added to that
    /// value rather than to a copy beside it. The caller appends this before
    /// either of them reads the plan, and the route's own composition of the
    /// inline lines stays exactly where it is.
    ///
    /// A reference is a path line even for a provider on this list, which is
    /// the route that carries bytes inline — see
    /// [`push_reference_path_lines`] for why that is a decision.
    fn append_reference_path_lines(&mut self, reference_paths: &[PathBuf]);

    /// Frames and sends this prompt.
    fn send(&self) -> Result<(), WireError>;
}

/// The text block for a structured prompt: the user's text, a blank line,
/// then one path line per non-raster attachment. The same line shape
/// `with_attachment_paths` writes — both go through [`push_path_lines`], which
/// is where that line is written once — so the fallback reads identically
/// whether it travels alone or beside image blocks. `plan_structured_prompt`
/// is its only caller; it stays separate (rather than inlined) so the legacy
/// write and the structured text block visibly share one line shape.
fn prompt_text_with_fallback_paths(text: &str, fallback_paths: &[PathBuf]) -> String {
    if fallback_paths.is_empty() {
        return text.to_string();
    }
    let mut prompt = String::from(text);
    prompt.push_str("\n\n");
    push_path_lines(&mut prompt, fallback_paths);
    prompt
}

type TransitionSink = Arc<dyn Fn(OwnerId) + Send + Sync>;
type JournalRosterCache = Arc<Mutex<Option<(u64, Vec<SessionRecord>)>>>;

/// One client answer to a pending permission request.
pub struct PermissionResponse<'a> {
    pub session_id: &'a str,
    pub request_id: &'a str,
    pub outcome: PermissionOutcome,
    pub option_id: Option<&'a str>,
}

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

/// Runs between the brake admission and the delivery of an agent message (S4-10).
/// Test-only: it is the only way to land a turn's end inside that gap.
#[cfg(test)]
type AgentMessageAfterAdmissionHook = Arc<dyn Fn() + Send + Sync>;

/// Runs between a deposit's ownership check and the store write (HND-01).
/// Test-only: it is the only way to land a close inside that gap.
#[cfg(test)]
type DepositAfterOwnershipHook = Arc<dyn Fn() + Send + Sync>;

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
    /// The bytes of prompt attachments, on disk under the runtime dir. Files
    /// are written here and never in the workspace: a workspace is a git
    /// checkout whose `git status` the user reads.
    attachments: AttachmentStore,
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
    message_brakes: Arc<Mutex<MessageBrakeTable>>,
    /// The creation budget of agent-created sessions (`S5` decision 5), beside
    /// the message brakes and under the same discipline: one lock over the
    /// whole table, taken on its own and never across another.
    creations: Arc<Mutex<AgentCreationTable>>,
    #[cfg(test)]
    journal_list_calls: Arc<AtomicU64>,
    #[cfg(test)]
    full_roster_builds: Arc<AtomicU64>,
    #[cfg(test)]
    journal_roster_after_list_hook: Arc<Mutex<Option<JournalRosterAfterListHook>>>,
    #[cfg(test)]
    agent_message_after_admission_hook: Arc<Mutex<Option<AgentMessageAfterAdmissionHook>>>,
    #[cfg(test)]
    deposit_after_ownership_hook: Arc<Mutex<Option<DepositAfterOwnershipHook>>>,
    /// The agent-profile store, attached by `ServerState` once both exist
    /// (`create-from-profile`).
    ///
    /// `SessionRegistry::new` cannot take it: the registry is a field of the
    /// state that holds the store, so the two are built in one expression and
    /// the store is attached immediately afterwards. A registry without one —
    /// every unit test that builds its own — has no standing instructions, which
    /// is the honest reading of "no store, no rules": nothing is cached, and the
    /// store is asked again on the next session's first prompt.
    agent_profiles: std::sync::OnceLock<Arc<crate::agent_profiles::AgentProfilesStore>>,
    /// The delegation switch, attached by `ServerState` like the profile
    /// store above. The handle is the store, never a copy of the boolean:
    /// every reader here asks it at the moment it decides, per the
    /// read-cadence rule at `delegation_store.rs`.
    delegation: std::sync::OnceLock<Arc<crate::delegation_store::DelegationStore>>,
}

pub(crate) struct MessageBrake {
    outstanding: Vec<OutstandingMessage>,
    recipients: Vec<Recipient>,
    next_slot: u64,
    window_started: Instant,
    sent_in_window: u32,
}

/// One message that was admitted and has not reached its boundary yet.
struct OutstandingMessage {
    slot: u64,
    sent_at: Instant,
    /// The session this message was sent to: the target whose turn end releases
    /// the slot, and the name the recipient window counts.
    to_session: String,
    /// Set once the delivery has returned — the text is in the provider's hands,
    /// or the delivery failed. A boundary that is already reached releases the
    /// slot as soon as this is set.
    delivered: bool,
    /// Set when the boundary arrived while the delivery was still in flight.
    ///
    /// The slot stays counted until then: releasing it at the boundary would let
    /// the next message through while this one is still being written, which is
    /// exactly what the outstanding count is there to prevent (A2-05).
    boundary_reached: bool,
    /// Where this slot's release arrives: the target runtime whose turn end
    /// releases it, and the id of the one-shot hook registered on it. `None`
    /// once the hook has fired or been unregistered again.
    release: Option<(Weak<SessionRuntime>, u64)>,
}

/// One recipient inside the sliding window.
struct Recipient {
    session_id: String,
    sent_at: Instant,
}

/// At most this many messages may be in flight from one sender.
const MAX_MESSAGE_OUTSTANDING: usize = 5;
/// At most this many distinct recipients may be reached inside the recipient
/// window.
const MAX_MESSAGE_RECIPIENTS: usize = 3;
/// At most this many messages may leave one sender inside the rate window.
const MAX_MESSAGE_SENT_PER_WINDOW: u32 = 5;
/// The rate window: the brief's one second, unchanged by this fix.
const MESSAGE_RATE_WINDOW: Duration = Duration::from_secs(1);
/// How long one in-flight message may hold a sender's slot, and how long a
/// recipient stays inside the recipient window. A target that never ends a turn
/// — or never starts one — must not park a sender's budget forever.
const MESSAGE_SLOT_EXPIRY: Duration = Duration::from_secs(60);

impl MessageBrake {
    fn new() -> Self {
        Self {
            outstanding: Vec::new(),
            recipients: Vec::new(),
            next_slot: 1,
            window_started: Instant::now(),
            sent_in_window: 0,
        }
    }

    /// Drop the slots that are over and the recipients that have aged out of the
    /// window, measured against `now`, answering the hooks that were armed for
    /// slots the expiry just ended.
    ///
    /// The expiry is the backstop for a slot whose *delivery* never returns — a
    /// write wedged in a provider's pipe must not park a sender's budget forever
    /// (S4-03) — so it ends the slot whether or not the delivery came back. The
    /// *boundary* (the target's turn ending) is the one that waits for the
    /// delivery, because there the message is still on its way (A2-05).
    ///
    /// A recipient, by contrast, is time-bounded (S4-01): it stays in the window
    /// for [`MESSAGE_SLOT_EXPIRY`] after its last send, whether or not a slot for
    /// it is still in flight, because the window is the fan-out brake — how many
    /// *different* agents one sender has reached lately — and a set that emptied
    /// itself as slots retired would let a sender rotate through targets instead.
    fn prune(&mut self, now: Instant) -> Vec<(Weak<SessionRuntime>, u64)> {
        let mut expired: Vec<(Weak<SessionRuntime>, u64)> = Vec::new();
        let mut live: Vec<OutstandingMessage> = Vec::with_capacity(self.outstanding.len());
        for mut slot in self.outstanding.drain(..) {
            if now.saturating_duration_since(slot.sent_at) < MESSAGE_SLOT_EXPIRY {
                live.push(slot);
            } else if let Some((runtime, hook)) = slot.release.take() {
                expired.push((runtime, hook));
            }
        }
        self.outstanding = live;
        // Written out rather than called as a method so the closure borrows only
        // `outstanding` and `recipients`' own `sent_at`, which cannot conflict.
        self.recipients.retain(|recipient| {
            now.saturating_duration_since(recipient.sent_at) < MESSAGE_SLOT_EXPIRY
                || self
                    .outstanding
                    .iter()
                    .any(|entry| entry.to_session == recipient.session_id)
        });
        expired
    }

    fn holds_recipient(&self, session_id: &str) -> bool {
        self.recipients
            .iter()
            .any(|recipient| recipient.session_id == session_id)
    }

    /// Remove one slot, answering with its still-armed hook so the caller can
    /// unregister it.
    fn take_slot(&mut self, slot: u64) -> Option<(Weak<SessionRuntime>, u64)> {
        let index = self
            .outstanding
            .iter()
            .position(|entry| entry.slot == slot)?;
        self.outstanding.remove(index).release
    }

    /// Whether an outstanding message still names this session (A2-06).
    fn has_slot_for(&self, session_id: &str) -> bool {
        self.outstanding
            .iter()
            .any(|entry| entry.to_session == session_id)
    }

    /// Drop one recipient once it has nothing in flight *and* has aged out of
    /// the window (S4-01).
    ///
    /// A recipient younger than [`MESSAGE_SLOT_EXPIRY`] stays, even when its last
    /// slot is gone: the window is the fan-out brake, and dropping the entry the
    /// moment a slot retires would let a sender reach an unbounded number of
    /// agents by rotating through them.
    fn drop_recipient_if_idle(&mut self, session_id: &str, now: Instant) {
        if self.has_slot_for(session_id) || self.recipient_in_window(session_id, now) {
            return;
        }
        self.recipients
            .retain(|recipient| recipient.session_id != session_id);
    }

    /// Whether this session is still inside the recipient window (S4-01).
    fn recipient_in_window(&self, session_id: &str, now: Instant) -> bool {
        self.recipients.iter().any(|recipient| {
            recipient.session_id == session_id
                && now.saturating_duration_since(recipient.sent_at) < MESSAGE_SLOT_EXPIRY
        })
    }

    /// Nothing left to remember: the sender's entry can leave the table.
    fn is_idle(&self) -> bool {
        self.outstanding.is_empty() && self.recipients.is_empty()
    }
}

/// The agent-message brakes, with the clock of the last global sweep (S4-16).
///
/// One mutex covers both: an admission that sweeps and an admission that reserves
/// cannot interleave half-way, and the sweep cannot run more often than
/// [`MESSAGE_RATE_WINDOW`] no matter how many senders are active. Every access
/// site still reads the map directly through `Deref`, so the entries and the
/// sweep clock cannot drift apart.
#[derive(Default)]
pub(crate) struct MessageBrakeTable {
    entries: HashMap<String, MessageBrake>,
    last_sweep: Option<Instant>,
    /// How many sweeps actually ran (S4-16). Test-only: the cadence is otherwise
    /// invisible from outside the table.
    #[cfg(test)]
    sweeps: u64,
}

impl std::ops::Deref for MessageBrakeTable {
    type Target = HashMap<String, MessageBrake>;

    fn deref(&self) -> &Self::Target {
        &self.entries
    }
}

impl std::ops::DerefMut for MessageBrakeTable {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.entries
    }
}

impl MessageBrakeTable {
    /// Whether the global sweep may run now (S4-16).
    ///
    /// The sweep walks every other sender's entry while this single lock is held,
    /// so it is a per-window cost rather than a per-send one. The caller's own
    /// entry is still pruned on every reserve, which is what its own braking
    /// needs; the sweep only bounds the table.
    fn sweep_is_due(&self, now: Instant) -> bool {
        self.last_sweep
            .is_none_or(|last| now.saturating_duration_since(last) >= MESSAGE_RATE_WINDOW)
    }

    fn note_sweep(&mut self, now: Instant) {
        self.last_sweep = Some(now);
        #[cfg(test)]
        {
            self.sweeps = self.sweeps.saturating_add(1);
        }
    }
}

pub(crate) struct LiveAgentEntry {
    pub(crate) session: Session,
    pub(crate) runtime: Arc<SessionRuntime>,
}

/// At most this many live children may one creator session hold at once
/// (`S5` decision 5).
pub(crate) const MAX_LIVE_CHILDREN_PER_CREATOR: usize = 3;
/// At most this many creations may leave one creator session inside the window.
pub(crate) const MAX_CREATIONS_PER_WINDOW: u32 = 10;
/// The creation window: one hour, from the first creation that opened it.
pub(crate) const CREATION_WINDOW: Duration = Duration::from_secs(60 * 60);
/// The deepest a created agent may be. A child of a child is depth 2; a
/// session at depth 2 may not create (`S5` decision 5).
pub(crate) const MAX_AGENT_DEPTH: u32 = 2;
/// At most this many agent-created sessions may be live in the whole daemon.
pub(crate) const MAX_LIVE_AGENT_SESSIONS: usize = 8;
/// Largest artifact one finish report deposits (32 KiB).
///
/// A cap, not a target: a child's last message is usually a few hundred bytes,
/// and the whole message — not the truncated summary — is what is stored. A
/// message over this is reported with a note instead, which is the same shape
/// as a deposit the store refused.
pub(crate) const MAX_AGENT_ARTIFACT_BYTES: usize = 32 * 1024;
/// How long one in-flight creation holds its idempotency key (`S5-03`).
///
/// Long enough for a card a human answers and a provider handshake behind it;
/// short enough that a thread which died mid-creation cannot make a key
/// permanently unusable.
pub(crate) const CREATION_PENDING_TTL: Duration = Duration::from_secs(5 * 60);

/// How long a parked child end, or a pending-creation marker, can belong to a
/// live creation: the slot's own expiry. Past it, [`AgentCreationTable::sweep`]
/// drops them (audit-3 §2) — a creation slower than this is a thread that died,
/// not a provider still starting.
pub(crate) const DEFERRED_SLOT_EXPIRY: Duration = Duration::from_secs(60);

/// The once-per-creator-session creation gate (`S5` decision 4, hardened by
/// audit S5-06).
///
/// Three states, not a bool, because the decision is made *outside* this lock
/// (the human answers a card) and a second caller must not be able to raise a
/// second card while the first one is unanswered: `Closed` has not been asked
/// yet, `Pending` has been asked and no one has answered, `Open` was answered
/// with an allow and stays open for as long as this entry lives.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CreationGate {
    Closed,
    Pending,
    Open,
}

/// What one creator session's budget currently holds.
struct AgentCreatorCaps {
    /// Children that exist.
    live_children: usize,
    /// Children this creator has reserved and not yet committed or abandoned:
    /// the slot is taken *before* the card is raised, so two creations racing
    /// on one session cannot both see the third slot free.
    /// The reservations in flight, by id, each naming the child session id it
    /// reserved (audit S5B-02). The id is the identity: releasing one is a
    /// removal that answers whether it was there, so a failure handled on two
    /// paths cannot subtract a neighbour's creation.
    in_flight: BTreeMap<u64, String>,
    window_started: Instant,
    creations_in_window: u32,
    /// The once-per-creator-session accept (`S5` decision 4, S5-06). It lives
    /// exactly as long as this entry does, and it is read and written only
    /// under this table's lock so two creations racing on one session cannot
    /// both be told to ask.
    gate: CreationGate,
    /// Set when the creator session is gone: the entry then lives until its
    /// last child finishes, because that is what releases the daemon-wide
    /// count.
    creator_gone: bool,
}

impl AgentCreatorCaps {
    fn new(now: Instant) -> Self {
        Self {
            live_children: 0,
            in_flight: BTreeMap::new(),
            window_started: now,
            creations_in_window: 0,
            gate: CreationGate::Closed,
            creator_gone: false,
        }
    }

    /// How many children this creator holds or is about to hold: what the
    /// three-child cap counts.
    fn held(&self) -> usize {
        self.live_children + self.in_flight.len()
    }

    /// Roll the window if it has expired. Called on every admission *and* on
    /// the sweep, so the count a caller reads is never one window stale.
    fn roll_window(&mut self, now: Instant) {
        if now.saturating_duration_since(self.window_started) >= CREATION_WINDOW {
            self.window_started = now;
            self.creations_in_window = 0;
        }
    }
}

/// One child, as its creator's bookkeeping sees it.
struct AgentChild {
    creator: String,
    /// Whether the creator asked to be told (the tool's `notifyOnFinish`).
    notify: bool,
    /// Whether the session behind this link was actually started (audit
    /// S5B-04). The link is registered when the reservation is taken — before
    /// the spawn — so a child that exits on the instant cannot outrun the row
    /// that catches its end; the child counts against its creator only once
    /// the spawn returned a session.
    started: bool,
    /// The `input_required` notice is owed until it has been sent once
    /// (`S5` §3): one notice per child, not one per card.
    notice_owed: bool,
    /// The finish report is owed until it has been written once. This is what
    /// makes the report idempotent across the three paths that can observe the
    /// same end (a finished turn, a process exit, a close).
    report_owed: bool,
}

/// One parked child end (audit-2 §2): what the end path still had in hand when
/// the child was gone. Any slot may be `None`; a unit test drives that shape,
/// and the type is named so the table and the commit read as one thing.
type DeferredChildEnd = (
    Option<Session>,
    Option<Arc<SessionRuntime>>,
    Option<OwnerId>,
);

/// The creation budget, beside [`MessageBrakeTable`] and under the same lock
/// discipline: one mutex covers the whole table, the sweep runs at most once
/// per window, and no other lock is taken while it is held.
#[derive(Default)]
pub(crate) struct AgentCreationTable {
    creators: HashMap<String, AgentCreatorCaps>,
    children: HashMap<String, AgentChild>,
    /// Children an agent's creation has spawned but not committed yet
    /// (audit-2 §2): their end waits instead of running against a link that
    /// does not exist yet.
    pending_children: HashMap<String, (Instant, u64)>,
    /// Ends that arrived while their child was still pending, kept whole
    /// (session view, runtime, owner) so the commit can run the routine the
    /// moment the link exists.
    deferred_child_ends: HashMap<String, (DeferredChildEnd, Instant)>,
    /// The idempotency keys of creations that are in flight right now
    /// (audit S5-03): a retry that arrives while its key is here is refused
    /// without spending anything, because the first call has not answered yet.
    pending: HashMap<String, Instant>,
    /// The next reservation id (audit S5B-02). Unique for the life of the
    /// table, which is what makes a release answerable.
    next_reservation: u64,
    last_sweep: Option<Instant>,
    #[cfg(test)]
    sweeps: u64,
}

impl AgentCreationTable {
    fn sweep_is_due(&self, now: Instant) -> bool {
        self.last_sweep
            .is_none_or(|last| now.saturating_duration_since(last) >= CREATION_WINDOW)
    }

    /// Drop the entries that can no longer say anything: a creator whose
    /// session is gone and whose children have all finished.
    ///
    /// The window is rolled unconditionally (every entry, whatever its age) so
    /// a table that is swept once an hour still reports this hour's count.
    fn sweep(&mut self, now: Instant) {
        for caps in self.creators.values_mut() {
            caps.roll_window(now);
        }
        self.creators
            .retain(|_, caps| !(caps.creator_gone && caps.held() == 0));
        // The backstop for the parked ends (audit-3 §2): a creation whose thread
        // died, or a creator that closed in the wrong instant, leaves a parked
        // end behind, and past the slot expiry it cannot belong to a live
        // creation any more.
        //
        // `pending_children` is deliberately **not** aged here (audit-3 S5D-01):
        // a marker is the link between an end that arrived early and the commit
        // that has not run yet, and a spawn slower than the expiry — an ACP
        // handshake is not fast — would lose that link here, stranding the
        // reservation and the finish report with it. A marker lives exactly as
        // long as its reservation: the commit and the abandon remove it by name,
        // and `release_agent_creation` removes it with the reservation.
        self.deferred_child_ends
            .retain(|_, (_, at)| now.saturating_duration_since(*at) < DEFERRED_SLOT_EXPIRY);
        self.last_sweep = Some(now);
        #[cfg(test)]
        {
            self.sweeps = self.sweeps.saturating_add(1);
        }
    }

    /// How many agent-created sessions the daemon holds or is about to hold
    /// (audit S5-02): committed children **plus** every reservation that has
    /// not been committed or abandoned yet.
    ///
    /// Counting only `children` let concurrent creators each pass the global
    /// check and then commit past the cap; a reservation is a session the
    /// daemon has already promised to someone, so it is counted from the
    /// moment it is taken.
    fn live_agent_sessions(&self) -> usize {
        // Committed children **plus** every reservation still in flight (audit
        // S5-02): a reservation is a session the daemon has already promised,
        // and the two sets are disjoint — a reservation is dropped when its
        // child is committed.
        self.children.len()
            + self
                .creators
                .values()
                .map(|caps| caps.in_flight.len())
                .sum::<usize>()
    }

    /// Claim the idempotency key of a creation that is starting (`S5-03`).
    ///
    /// False means another call with the same key is in flight: that call is
    /// refused before a slot, a card or a session is spent on it. An entry
    /// older than [`CREATION_PENDING_TTL`] is taken over rather than honoured,
    /// because a thread that died mid-creation must not make its key
    /// permanently unusable.
    fn begin_creation(&mut self, key: &str, now: Instant) -> bool {
        match self.pending.get(key) {
            Some(started) if now.saturating_duration_since(*started) < CREATION_PENDING_TTL => {
                false
            }
            _ => {
                self.pending.insert(key.to_string(), now);
                true
            }
        }
    }

    /// The creation this key was claimed for is over, either way: its result is
    /// in the idempotency store, or it failed and stored nothing.
    fn end_creation(&mut self, key: &str) {
        self.pending.remove(key);
    }
}

/// One creation's hold on its idempotency key (audit S5-03).
///
/// The key is claimed before the idempotency store is read and released when
/// this goes out of scope, so the handler's refusals — a bad workspace, a
/// refused card, a provider that would not spawn — do not each need a release
/// line, and a panic in between cannot wedge the key for good: the mark also
/// expires on its own ([`CREATION_PENDING_TTL`]).
pub(crate) struct CreationKeyHold<'a> {
    sessions: &'a SessionRegistry,
    key: Option<String>,
}

impl CreationKeyHold<'_> {
    /// The call answered and remembered its result: the key stops being in
    /// flight now, while the idempotency store keeps the answer a retry reads.
    ///
    /// It takes `&mut self` rather than `self` so a caller can commit through
    /// `Option::as_mut` without moving the guard out of it.
    pub(crate) fn commit(&mut self) {
        self.release();
    }

    fn release(&mut self) {
        if let Some(key) = self.key.take() {
            self.sessions.end_agent_creation(&key);
        }
    }
}

impl Drop for CreationKeyHold<'_> {
    fn drop(&mut self) {
        self.release();
    }
}

/// What one reservation answers: the numbers the creation card states, and
/// whether the card is still owed for this creator session.
///
/// It is also the reservation's identity (audit S5B-02) and it releases the
/// reservation when it is dropped, so every refusal and every failure between
/// the reserve and the commit — a bad workspace, a refused card, a spawn that
/// returned an error — gives the slot back exactly once without a release line
/// per path, and a release cannot happen twice.
pub(crate) struct AgentCreationTicket<'a> {
    registry: &'a SessionRegistry,
    creator: String,
    reservation: u64,
    /// The child session id reserved for this creation (audit S5B-04).
    child: String,
    card_owed: bool,
    committed: bool,
    caps: devboule_protocol::CreateAgentCaps,
}

impl AgentCreationTicket<'_> {
    pub(crate) fn card_owed(&self) -> bool {
        self.card_owed
    }

    pub(crate) fn caps(&self) -> &devboule_protocol::CreateAgentCaps {
        &self.caps
    }

    pub(crate) fn reservation(&self) -> u64 {
        self.reservation
    }

    /// The slot is now a child: it stops being a reservation, and nothing is
    /// given back when this is dropped.
    pub(crate) fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for AgentCreationTicket<'_> {
    fn drop(&mut self) {
        if !self.committed {
            self.registry
                .release_agent_creation(&self.creator, self.reservation);
        }
    }
}

impl std::fmt::Debug for AgentCreationTicket<'_> {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentCreationTicket")
            .field("creator", &self.creator)
            .field("reservation", &self.reservation)
            .field("child", &self.child)
            .field("card_owed", &self.card_owed)
            .field("committed", &self.committed)
            .finish()
    }
}

/// What a create carries beyond the wire's own frame (`S5` §3).
///
/// The human path fills in `display_name` and nothing else. Every other field
/// is written by the daemon for a create an *agent* asked for, and none of them
/// is reachable from `ClientMessage::SessionCreate`: a client cannot name its
/// parent, choose its depth, hand itself a tool overlay, or declare an origin.
#[derive(Default, Clone)]
pub(crate) struct SessionCreateMeta {
    /// Whether this session is an agent's child whose creation has not
    /// committed yet (audit-2 §2). Its end can arrive before the link exists,
    /// so an end with no link is parked rather than reported twice or lost.
    pub(crate) creation_pending: bool,
    /// The reservation whose ticket owns this creation (audit-3 S5D-01). The
    /// spawn notes the child's pending marker with it, so the reservation's
    /// release clears that marker the way the commit and the abandon do: the
    /// marker's life is the reservation's, and the sweep never ages it.
    pub(crate) reservation: Option<u64>,
    /// The id this session must use, when the caller reserved one (audit
    /// S5B-04: an agent's child id is composed by the reservation so the link
    /// can exist before the spawn). `None` means "compose one now", which is
    /// every other caller.
    pub(crate) session_id: Option<String>,
    pub(crate) display_name: Option<String>,
    /// The session that created this one (`None` when a human or a client asked
    /// for it).
    pub(crate) created_by: Option<String>,
    /// How far this session is from a human root: 0 for a human's session, 1
    /// for its child, 2 for a grandchild.
    pub(crate) depth: u32,
    /// The preset's tool overlay, which the broker consults per session.
    pub(crate) overlay: crate::provider_catalog::ToolOverlay,
    /// The origin to record. `None` means "this connection's", which is every
    /// human-started create; a created child passes its creator's stored origin.
    pub(crate) origin: Option<SessionOrigin>,
    /// An already-confined working directory for the child.
    pub(crate) cwd: Option<PathBuf>,
    /// The profile this creation resolved, by its stable id
    /// (`create-from-profile`). `None` for every create that resolved no
    /// profile, which is the human's provider picker and every terminal.
    pub(crate) profile_id: Option<String>,
    /// The labels the creation stamped — the caller's own map plus the daemon's
    /// four `devboule.` keys. Empty for a create that is not an agent's.
    pub(crate) labels: std::collections::BTreeMap<String, String>,
    /// The context this session inherits. `None` means "its own id", which is
    /// every create that is not another session's child; a created child passes
    /// its creator's context, so a creator and everything it commissions share
    /// one at any depth.
    pub(crate) context_id: Option<String>,
}

impl SessionCreateMeta {
    /// What one agent creation carries into the `SessionCreate` path.
    ///
    /// Pure on purpose: this is where the child *inherits*, and the two rules
    /// that matter are readable here in one place. The origin is the creator's
    /// **stored** origin, so a child of a peer's session stays on that peer's
    /// device and with that peer's role (`S5` decision 3: never invented, and
    /// never taken from a connection — an MCP call has no connection). The
    /// creator, the depth and the overlay are the daemon's own facts about the
    /// child, written when the child's MCP registration is made; no parameter of
    /// `devboule_create_agent` reaches any of the four.
    pub(crate) fn for_agent_child(
        creator_session_id: &str,
        origin: &SessionOrigin,
        display_name: &str,
        depth: u32,
        overlay: crate::provider_catalog::ToolOverlay,
        cwd: Option<PathBuf>,
    ) -> Self {
        Self {
            session_id: None,
            creation_pending: true,
            // Written by the creation that holds the ticket, below.
            reservation: None,
            display_name: Some(display_name.to_string()),
            created_by: Some(creator_session_id.to_string()),
            depth,
            overlay,
            origin: Some(origin.clone()),
            cwd,
            // The creation-from-profile facts are written by the creation
            // that resolved a profile, beside the reservation above: this
            // function is the part of a child's birth that does not depend on
            // which profile made it. `context_id: None` here would be "this
            // child is its own context", which is the truth only until the
            // caller puts the creator's context in.
            profile_id: None,
            labels: std::collections::BTreeMap::new(),
            context_id: None,
        }
    }
}

/// The creator's own facts, as a creation reads them (`S5` §3): the child
/// inherits every one of them and invents none.
pub(crate) struct AgentCreator {
    pub(crate) owner: OwnerId,
    pub(crate) origin: SessionOrigin,
    pub(crate) workspace_id: Option<String>,
    pub(crate) display_name: Option<String>,
    pub(crate) title: String,
    /// The context this creator belongs to: its own id, or the context of the
    /// session that created *it*. A child inherits this — that inheritance is
    /// the whole rule, and it is what makes a human's session and every
    /// generation under it one context (`create-from-profile`).
    pub(crate) context_id: String,
}

impl AgentCreator {
    /// Whether the device behind this creator may still create sessions
    /// (`S5` §3).
    ///
    /// A local creator is this daemon's own person: allowed. A peer's creator is
    /// a session that device already created, so its child is a session on that
    /// device and the same capability gate applies to it — judged with the same
    /// `peer_allows` function the dispatcher and the broker door use, on the same
    /// wire message the door names for this tool (`SessionCreate`; that arm reads
    /// only the capability set, so the placeholder kind never decides). The lookup
    /// is fail-closed — an unknown, unreadable or revoked device holds nothing —
    /// and an origin the daemon cannot read (peer-shaped without device or role)
    /// is not a licence either.
    pub(crate) fn may_create_sessions(&self, state: &crate::server::ServerState) -> bool {
        match self.origin.kind {
            SessionOriginKind::Local => true,
            SessionOriginKind::Peer => {
                let (Some(device), Some(role)) =
                    (self.origin.device_id.as_deref(), self.origin.role)
                else {
                    return false;
                };
                let caps = state.peer_caps(device);
                let request = devboule_protocol::ClientMessage::SessionCreate {
                    id: 0,
                    workspace_id: None,
                    kind: SessionKind::Claude,
                    provider: None,
                    mode: None,
                    display_name: None,
                    idempotency_key: None,
                };
                matches!(
                    crate::peer_policy::peer_allows(role, &caps, &request),
                    crate::peer_policy::PeerDecision::Allow
                )
            }
            SessionOriginKind::Unknown => false,
        }
    }
}

impl AgentCreator {
    /// The name to tell the human a creation came from: the creator's display
    /// name when it has one, otherwise its title — the same fallback the app
    /// renders, so the sentence names a row the human can see.
    pub(crate) fn name(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.title)
    }
}

/// One creation an agent asked for.
pub(crate) struct AgentCreation {
    pub(crate) creator_session_id: String,
    pub(crate) creator: AgentCreator,
    /// The creator's runtime, taken *before* the spawn (audit-2 §1): the same
    /// handle the card was raised through. The creation record is published
    /// through it after the spawn, so a lookup that would miss by then cannot
    /// take the record with it.
    pub(crate) creator_runtime: Option<Arc<SessionRuntime>>,
    pub(crate) display_name: String,
    pub(crate) provider: String,
    /// The profile the creation resolved, by its **stable id**: this is what the
    /// session records, so a rename later cannot make a running child misreport
    /// what it was started from.
    pub(crate) profile_id: String,
    /// The profile's name at the moment of the call, which is what the creator's
    /// transcript shows (`SessionEvent::AgentCreated`). A record of a birth: a
    /// rename afterwards does not rewrite it.
    pub(crate) profile_name: String,
    /// Everything the profile delivers to the child — the mode, the model, the
    /// thinking option and the `autoAccept` constraint — as one typed value.
    /// The card names all of these; the child is started on all of these or the
    /// creation is refused, so a child that exists was delivered everything its
    /// card printed.
    pub(crate) delivery: crate::profile_delivery::ProfileDelivery,
    pub(crate) overlay: crate::provider_catalog::ToolOverlay,
    /// The labels the child carries: the caller's own plus the four the daemon
    /// stamped.
    pub(crate) labels: std::collections::BTreeMap<String, String>,
    /// The context the child inherits: its creator's.
    pub(crate) context_id: Option<String>,
    pub(crate) depth: u32,
    pub(crate) cwd: Option<PathBuf>,
    pub(crate) initial_prompt: String,
    pub(crate) notify: bool,
    /// The workspace the child is created in: the one the caller named, or the
    /// creator's when it named none. Both are the caller's own business to
    /// reach, and the registry resolves the path.
    pub(crate) workspace_id: Option<String>,
}

/// Whether a resolved provider id came from the session-create request
/// or from `DEVBOULE_AGENT_PROVIDER`. Consent for npx wrappers requires
/// the request; the env override cannot supply it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProviderProvenance {
    Request,
    Env,
}

/// Everything one send needs beyond the registry itself.
///
/// Folded into one value rather than seven positional arguments: the call
/// shape is read in one place, the send path stops growing a parameter per
/// slice, and `server.rs::session_send` builds it from the frame in one
/// literal. `origin` is deliberately not here — it is set once at create,
/// stored on the session, and read from there.
pub struct SendRequest<'a> {
    pub session_id: &'a str,
    pub subscription_id: u64,
    pub text: &'a str,
    pub attachments: &'a [PromptAttachment],
    /// The stored attachments this prompt refers to, beside the inline ones.
    ///
    /// Resolved against `session_id`, not against any session the references
    /// themselves name: the protocol refuses a reference whose session is not
    /// the request's before the store is asked anything, so the two can never
    /// disagree about where a digest resolves. Both are empty in the common
    /// case, which is what keeps a text-only prompt free of every check these
    /// two fields bring.
    pub attachment_references: &'a [AttachmentReference],
    pub owner: &'a OwnerId,
    pub conn: &'a ConnHandle,
    pub mcp_timeout: Duration,
    pub active_turn_behavior: Option<ActiveTurnBehavior>,
    pub require_attachment: bool,
    /// Whether a `Steer` the provider cannot take may fall back to interrupting
    /// the running turn and replacing it (S4-01).
    ///
    /// True for the person at this machine and for a local agent's own message
    /// delivery. False for a paired device: interrupting a turn is the act
    /// `SessionInterrupt` decides, and no capability opens it to a peer, so a
    /// peer's steer must not reach `killer.interrupt()` the long way round.
    pub interrupt_on_steer_refusal: bool,
    /// The brake slot this delivery belongs to, when the send is an agent message
    /// that reserved one (S4-10).
    ///
    /// The plain-prompt fallback re-arms this slot's boundary through it: the hook
    /// admission armed belongs to the turn the message was admitted into, and that
    /// turn can end before the delivery writes — the prompt that replaces the
    /// steer then starts a turn of its own, and that turn is the boundary the slot
    /// has to end on.
    pub message_slot: Option<&'a MessageSlotRef<'a>>,
    /// The preset preamble this prompt carries in front of its own text, when the
    /// caller is a creation that has one.
    ///
    /// `None` for every other caller — a human's message, the app's own first
    /// prompt for a Design run, an agent message — and `Some(AGENT_PREAMBLE)` for
    /// the prompt an agent's creation sends to its child. It is a field of the
    /// request rather than something the send path looks up, so the ordering rule
    /// (standing instructions, then this, then the prompt) is composed in exactly
    /// one place and no session has to be searched for its preamble
    /// (`create-from-profile`).
    pub preset_preamble: Option<&'a str>,
}

/// What one delivery needs to re-key its brake slot (S4-10): the table, the
/// sender's key in it, and the slot.
pub(crate) struct MessageSlotRef<'a> {
    pub(crate) brakes: &'a Arc<Mutex<MessageBrakeTable>>,
    pub(crate) from_session: &'a str,
    pub(crate) slot: u64,
    /// The turn the admission registered the boundary against (S4-14). The delivery
    /// compares it with the turn that is running when it writes, so a message whose
    /// admitted turn has been replaced is re-keyed onto the turn it actually enters.
    pub(crate) admitted_turn_id: u64,
}

/// A session's first prompt, composed in the one place (`create-from-profile`).
///
/// The order is fixed, and pinned by
/// `standing_instructions_come_before_the_preset_preamble`: the human's
/// **standing instructions**, then the **preset preamble** where the caller has
/// one, then the prompt itself.
///
/// One glue point, on the shared send path every provider's writer sits behind.
/// That is the measured decision, not a preference: the daemon sends no system
/// prompt on any provider, and the preamble reaches the model today as the first
/// *user* message (`reports/remote-agents/recon-system-prompt-seams.md` §2 — the
/// glue at this same site, four writers, and ACP v1's `session/new` and
/// `session/prompt` carry no field for one). Composing here is what makes every
/// provider get the same text the same way, so none of them can be the silent
/// exception.
///
/// Everything empty means the prompt itself, **byte for byte**: a human who has
/// written no standing instructions and a caller with no preamble get exactly
/// today's prompt, with no separator and no trailing newline to show for a
/// feature they are not using.
pub(crate) fn compose_first_prompt(standing: &str, preamble: Option<&str>, prompt: &str) -> String {
    match (standing.is_empty(), preamble) {
        (true, None) => prompt.to_string(),
        (true, Some(preamble)) => format!("{preamble}\n\n{prompt}"),
        (false, None) => format!("{standing}\n\n{prompt}"),
        (false, Some(preamble)) => format!("{standing}\n\n{preamble}\n\n{prompt}"),
    }
}

/// The profile facts a `devboule_set_agent_profile` move delivers, resolved on
/// the caller's side at the moment of the move.
///
/// Plain data, so the registry never reads the profile store and the broker
/// never touches a session: the broker resolves the profile (§2 check 3) and
/// hands over what the move will ask the provider to apply. Resolved *inside*
/// the move, after the child check, so the refusals keep the spec's order
/// whatever the caller's convenience.
#[derive(Debug)]
pub(crate) struct ChildProfileFacts {
    pub(crate) profile_id: String,
    pub(crate) mode_id: String,
    pub(crate) model: String,
    pub(crate) thinking_option_id: Option<String>,
}

impl SessionRegistry {
    pub(crate) fn runtime_dir(&self) -> &std::path::Path {
        &self.paths.dir
    }

    /// Sweep attachment folders left behind by a session that never closed.
    ///
    /// Returns what it reclaimed — one entry per swept session, with the bytes
    /// it held, or `None` when the folder could not be read and the size is
    /// unknown. A count alone would not be enough: whoever meters deposits per
    /// device decrements against these numbers, and a sweep that did not say
    /// what it freed would leave that counter charging for bytes that no longer
    /// exist. An unreadable folder is `None` rather than zero for the same
    /// reason — unknown is not empty.
    pub(crate) fn sweep_attachments(
        &self,
        now: std::time::SystemTime,
    ) -> Vec<(String, Option<u64>)> {
        self.attachments
            .sweep_older_than(now, crate::attachment_store::ATTACHMENT_RETENTION)
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
            attachments: AttachmentStore::new(&paths.dir),
            paths,
            journal,
            transition_sink: Arc::new(Mutex::new(None)),
            presence: Arc::new(Mutex::new(HashMap::new())),
            journal_roster: Arc::new(Mutex::new(None)),
            workspace_paths: Arc::new(Mutex::new(WorkspacePathCache::default())),
            state_roster_cache: Arc::new(Mutex::new(HashMap::new())),
            message_brakes: Arc::new(Mutex::new(MessageBrakeTable::default())),
            creations: Arc::new(Mutex::new(AgentCreationTable::default())),
            #[cfg(test)]
            journal_list_calls: Arc::new(AtomicU64::new(0)),
            #[cfg(test)]
            full_roster_builds: Arc::new(AtomicU64::new(0)),
            #[cfg(test)]
            journal_roster_after_list_hook: Arc::new(Mutex::new(None)),
            #[cfg(test)]
            agent_message_after_admission_hook: Arc::new(Mutex::new(None)),
            #[cfg(test)]
            deposit_after_ownership_hook: Arc::new(Mutex::new(None)),
            agent_profiles: std::sync::OnceLock::new(),
            delegation: std::sync::OnceLock::new(),
        };
        spawn_os_liveness_sweeper(&registry);
        registry.reconcile_worktree_journal();
        registry
    }

    /// Hand the registry the agent-profile store (`create-from-profile`).
    ///
    /// Called once, by `ServerState`, right after both exist. The registry reads
    /// the store at exactly one moment — a session's first prompt — and never
    /// keeps a copy of its document, so an edit to the profiles takes effect on
    /// the next session's first prompt rather than at the next restart.
    pub(crate) fn attach_agent_profiles(
        &self,
        store: Arc<crate::agent_profiles::AgentProfilesStore>,
    ) {
        let _ = self.agent_profiles.set(store);
    }

    /// Attach the delegation switch, exactly like the profile store above.
    pub(crate) fn attach_delegation(&self, store: Arc<crate::delegation_store::DelegationStore>) {
        let _ = self.delegation.set(store);
    }

    /// The switch, read **now** — the one getter, for the one decision this
    /// call is making. `false` when no store is attached, which is the safe
    /// direction for every caller (a test registry surfaces and answers
    /// nothing).
    pub(crate) fn delegation_enabled(&self) -> bool {
        self.delegation
            .get()
            .map(|store| store.get().0)
            .unwrap_or(false)
    }

    /// The human's standing instructions, read **now**, or nothing when this
    /// registry has no store.
    ///
    /// A copy of the string, not a borrowed handle: the caller puts it in front of
    /// a prompt that is about to be written, and the store may be replaced while
    /// that prompt is being composed.
    pub(crate) fn standing_instructions(&self) -> String {
        self.agent_profiles
            .get()
            .map(|store| store.document().standing_instructions)
            .unwrap_or_default()
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

    /// Arm a one-shot callback that runs after an agent message's brake admission
    /// and before its delivery (S4-10).
    #[cfg(test)]
    fn set_agent_message_after_admission_hook(&self, hook: AgentMessageAfterAdmissionHook) {
        *self
            .agent_message_after_admission_hook
            .lock()
            .expect("agent message test hook") = Some(hook);
    }

    #[cfg(test)]
    fn fire_agent_message_after_admission_hook(&self) {
        let hook = self
            .agent_message_after_admission_hook
            .lock()
            .ok()
            .and_then(|mut hook| hook.take());
        if let Some(hook) = hook {
            hook();
        }
    }

    /// Arm a one-shot callback that runs after a deposit's ownership check and
    /// before the store write (HND-01).
    #[cfg(test)]
    fn set_deposit_after_ownership_hook(&self, hook: DepositAfterOwnershipHook) {
        *self
            .deposit_after_ownership_hook
            .lock()
            .expect("deposit test hook") = Some(hook);
    }

    #[cfg(test)]
    fn fire_deposit_after_ownership_hook(&self) {
        let hook = self
            .deposit_after_ownership_hook
            .lock()
            .ok()
            .and_then(|mut hook| hook.take());
        if let Some(hook) = hook {
            hook();
        }
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

    /// The delegation facts one snapshot row carries, or `None` for a session
    /// that is not an agent-created child — an absence that must never be
    /// read as `off` (a child whose switch a human turned off). The
    /// `unattended` state is the birth fact the row already carries (read
    /// from the journal's ratcheted column, never recomputed and never
    /// derived from the live switch); the switch itself is asked **now**,
    /// per the read-cadence rule; the count is read from the resolution
    /// ledger the replay reads back.
    fn delegation_state_for(&self, session: &Session) -> Option<DelegationState> {
        session.created_by.as_ref()?;
        let state = if session.unattended == UnattendedState::Yes {
            DelegationRunState::Unattended
        } else if self.delegation_enabled() {
            DelegationRunState::Active
        } else {
            DelegationRunState::Off
        };
        let answered = self
            .journal
            .as_ref()
            .and_then(|journal| journal.permission_count(&session.id).ok())
            .unwrap_or(0);
        Some(DelegationState { answered, state })
    }

    /// Drop the cached roster, so the next snapshot rebuilds. The delegation
    /// facts ride every row, and a switch flip must not be served stale from
    /// a cache a transition never invalidated: `DelegationSet` clears this
    /// before the watchers are re-pushed.
    pub(crate) fn invalidate_state_roster_cache(&self) {
        if let Ok(mut cache) = self.state_roster_cache.lock() {
            cache.clear();
        }
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
        let (sessions_from_map, hidden_ids) = self
            .inner
            .lock()
            .map(|map| {
                let hidden_ids: std::collections::HashSet<String> = map
                    .values()
                    .filter(|entry| entry.is_configuring())
                    .map(|entry| entry.metadata().id.clone())
                    .collect();
                let sessions = map
                    .values()
                    .filter(|entry| entry.owner().user == owner.user && !entry.is_configuring())
                    .map(|entry| (entry.to_session(), entry.runtime().attention()))
                    .collect::<Vec<_>>();
                (sessions, hidden_ids)
            })
            .unwrap_or_default();
        let mut sessions = sessions_from_map;
        let live_ids = sessions
            .iter()
            .map(|(session, _)| session.id.clone())
            .collect::<std::collections::HashSet<_>>();
        if let Some(rows) = self.journal_roster() {
            sessions.extend(rows.into_iter().filter_map(|row| {
                if live_ids.contains(&row.id) || hidden_ids.contains(&row.id) {
                    return None;
                }
                (row.owner == owner.user).then(|| (row.to_session(), None))
            }));
        }
        sessions.sort_by(|left, right| left.0.id.cmp(&right.0.id));
        sessions
            .into_iter()
            .map(|(session, attention)| {
                let delegation = self.delegation_state_for(&session);
                SessionStateSnapshot {
                    id: session.id,
                    workspace_id: session.workspace_id,
                    kind: session.kind,
                    title: session.title,
                    state: session.state,
                    elapsed_ms: session.elapsed_ms,
                    attention,
                    origin: session.origin,
                    // The two fields a push-only row needs (S5-09, S5-04): the row
                    // this client is sent must name the child and its creator, not
                    // only the row the next list would build. The
                    // creation-from-profile facts travel with them for the same
                    // reason: a child created while the app is open arrives as a
                    // push-only row, and a row without its profile, its context, its
                    // marker and its labels would stay that way until the next full
                    // list.
                    display_name: session.display_name,
                    created_by: session.created_by,
                    profile_id: session.profile_id,
                    context_id: session.context_id,
                    unattended: session.unattended,
                    labels: session.labels,
                    delegation,
                }
            })
            .collect()
    }

    fn refresh_state_snapshot(&self, owner: &OwnerId, session_id: &str) {
        let snapshot = self.inner.lock().ok().and_then(|map| {
            map.get(session_id)
                .filter(|entry| entry.owner().user == owner.user)
                .filter(|entry| !entry.is_configuring())
                .map(|entry| {
                    let session = entry.to_session();
                    let delegation = self.delegation_state_for(&session);
                    SessionStateSnapshot {
                        id: session.id,
                        workspace_id: session.workspace_id,
                        kind: session.kind,
                        title: session.title,
                        state: session.state,
                        elapsed_ms: session.elapsed_ms,
                        attention: entry.runtime().attention(),
                        origin: session.origin,
                        display_name: session.display_name,
                        created_by: session.created_by,
                        profile_id: session.profile_id,
                        context_id: session.context_id,
                        unattended: session.unattended,
                        labels: session.labels,
                        delegation,
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
            // The same transition is what a creator is owed a report about
            // (`S5` §3). The claim inside is idempotent, so the many
            // transitions an ordinary session raises cost one hash lookup.
            registry.report_child_events(&session_id);
        });
        runtime.set_attention_hooks(suppressed, notify);
        // The delegated-surfacing observer, installed in the same place with
        // the same facts in scope: one observer per child, called once per
        // parked card, deciding at that moment whether the creator is told.
        let registry = self.clone();
        let child = runtime.session_id.clone();
        runtime.set_permission_park_hook(Arc::new(move |request| {
            registry.notify_creator_of_parked_card(&child, request);
        }));
    }

    /// One parked card, surfaced to its creator under the delegation switch
    /// (§4.3): the `<devboule-system>` `agent_permission_request` envelope
    /// joins the moment a card parks — the same park that raises attention
    /// and, once per child, sends the `input_required` notice.
    ///
    /// The switch is read **here**, at the park: a switch that was on when
    /// the daemon started surfaces nothing after a human turned it off, and
    /// the answer side re-reads it again before it accepts anything (the
    /// read-cadence rule at `delegation_store.rs`). Surfacing was a copy,
    /// never a transfer — a card surfaced to a creator stays pending for the
    /// human exactly as before.
    fn notify_creator_of_parked_card(&self, child: &str, request: &SessionEvent) {
        if !self.delegation_enabled() {
            return;
        }
        let SessionEvent::PermissionRequest {
            tool_call_id,
            title,
            description,
            command,
            ..
        } = request
        else {
            return;
        };
        let Some((session, _runtime, owner)) = self.child_view(child) else {
            return;
        };
        let Some(creator) = session.created_by.clone() else {
            return;
        };
        let display_name = session
            .display_name
            .clone()
            .unwrap_or_else(|| session.title.clone());
        // The child's own words on the card: the description it wrote, or the
        // command it asked to run. Capped and neutralised inside the builder.
        let excerpt = description
            .clone()
            .filter(|text| !text.trim().is_empty())
            .or_else(|| command.clone())
            .unwrap_or_else(|| title.clone());
        let envelope = agent_permission_request_envelope(
            &session.id,
            &session.origin,
            tool_call_id,
            title,
            &display_name,
            &excerpt,
        );
        let _ = self.deliver_to_creator(&creator, &owner, &envelope);
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
                    // Inside the delivery window the delete answers what
                    // every id-addressed peer call answers through
                    // `peer_entry`: `SessionNotFound`. A `Configuring`
                    // entry does not exist for peers — `sessions_list`
                    // never names the id, and no peer legitimately holds it
                    // (the create returns it only after promotion) — so
                    // "close the session before deleting it" would
                    // contradict the roster and confirm an id the caller
                    // should not know. One door, one answer.
                    if entry.is_configuring() {
                        return Err(not_found());
                    }
                    // Past the window the close-first guard stands (the
                    // re-audit's P1-1): a session a peer can see holds a
                    // running child, and an unrefused delete here would
                    // remove that child's row and entry out from under it.
                    if entry.as_peer_visible().is_some() {
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
        // The family remap routes through the provider registry: the id
        // names a provider, the provider's own `wire_kind` is the kind a
        // native family answers with, and both the requested/env asymmetry
        // and the arm order are the impls' `acp_create_remap_rank`. The
        // env override participates only when the request named nothing or
        // named a native family itself — the gate the literal spelling
        // closed with `requested.is_none()` — and the rank minimum
        // reproduces the arm order (pi, then codex, then claude) for every
        // input, including the ones where the env road outranks the
        // request's.
        let kind = if kind == SessionKind::Acp {
            let providers = provider::catalog_registry();
            let requested_provider = requested.as_deref().map(|id| providers.provider_for(id));
            let request_opens = requested_provider
                .as_ref()
                .map(|candidate| {
                    candidate
                        .acp_create_remap_rank(ProviderProvenance::Request)
                        .is_some()
                })
                .unwrap_or(true);
            let env_candidate = if request_opens {
                env_provider.map(|id| providers.provider_for(id))
            } else {
                None
            };
            requested_provider
                .into_iter()
                .map(|candidate| (candidate, ProviderProvenance::Request))
                .chain(env_candidate.map(|candidate| (candidate, ProviderProvenance::Env)))
                .filter_map(|(candidate, provenance)| {
                    candidate
                        .acp_create_remap_rank(provenance)
                        .map(|rank| (rank, candidate))
                })
                .min_by_key(|(rank, _)| *rank)
                .map(|(_, candidate)| candidate.wire_kind())
                .unwrap_or(SessionKind::Acp)
        } else {
            kind
        };
        let (provider, provenance) = if requested.is_some() {
            let provider = requested.filter(|id| id != "claude" && id != "pi");
            let provenance = provider.as_ref().map(|_| ProviderProvenance::Request);
            (provider, provenance)
        } else {
            let provider = env_provider
                .map(str::to_string)
                .filter(|id| id != "claude" && id != "pi" && !id.is_empty());
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

    /// Create a session owned by `owner`, originating from `conn_peer` (`None`
    /// for the local pipe).
    ///
    /// The origin is written here, once, and read-only afterwards: the journal
    /// row and the wire metadata must agree on who asked for this session
    /// (`DESIGN-remote-agents.md` §8 R2).
    #[allow(clippy::too_many_arguments)]
    pub fn create(
        &self,
        state: &Arc<ServerState>,
        owner: &OwnerId,
        workspace_id: Option<String>,
        kind: SessionKind,
        provider: Option<String>,
        mode: Option<String>,
        display_name: Option<String>,
        conn_peer: &Option<ConnPeer>,
    ) -> Result<Session, WireError> {
        let env_provider = std::env::var("DEVBOULE_AGENT_PROVIDER").ok();
        let meta = SessionCreateMeta {
            display_name,
            ..SessionCreateMeta::default()
        };
        self.create_with_provider_env(
            state,
            owner,
            workspace_id,
            kind,
            provider,
            crate::profile_delivery::ProfileDelivery::for_request(mode),
            None,
            conn_peer,
            env_provider.as_deref(),
            &meta,
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
        delivery: crate::profile_delivery::ProfileDelivery,
        command: Option<PtyCommand>,
        conn_peer: &Option<ConnPeer>,
        env_provider: Option<&str>,
        meta: &SessionCreateMeta,
    ) -> Result<Session, WireError> {
        // The create road is the boundary that answers "which providers
        // exist": the file is read here (and at resume) so an edit takes
        // effect on the next creation rather than at the next restart — the
        // liveness rule the profile store states for itself. A read that
        // finds nothing to change swaps nothing.
        crate::user_providers::refresh_user_rows(self.runtime_dir());
        let workspace_id_ref = workspace_id.as_deref();
        let workspace_cwd = workspace_id_ref
            .map(|workspace_id| self.workspace_cwd(workspace_id))
            .transpose()?;
        // A created child may start in a subdirectory of the creator's
        // workspace. It was resolved and confined on the way in
        // (`confined_child_cwd`), so a path that reaches here is already inside
        // the workspace, canonical, and an existing directory.
        let workspace_cwd = match meta.cwd.clone() {
            Some(cwd) => Some(cwd),
            None => workspace_cwd,
        };
        let id = match meta.session_id.clone() {
            Some(id) => id,
            None => {
                let unique = format!("{:08x}", SESSION_COUNTER.fetch_add(1, Ordering::Relaxed));
                compose_session_id(&owner.session_token(), &unique)
                    .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?
            }
        };
        let (kind, provider, provenance) =
            Self::resolve_session_provider(kind, provider, env_provider);
        let mut command = match command {
            Some(command) => command,
            None => {
                // The command road is the registry's to resolve: the kind
                // names its family, the family resolves its own command —
                // the ACP family's named road consults the catalog, the
                // native families run their fixed roads, the terminal road
                // resolves the shell.
                let family = provider::catalog_registry().provider_for_kind(&kind);
                // The npx consent gate is catalog policy (design §3.3.5) and
                // stays in this file; it keys on the one family whose named
                // road can resolve a catalog wrapper, not on the kind. Same
                // refusals as the kind-keyed arm it replaces, and no new
                // catalog read on any road that never had one.
                if let Some(id) = provider.as_deref() {
                    if family.resolves_named_from_catalog() {
                        Self::reject_env_npx_wrapper(id, provenance, &self.paths)?;
                    }
                }
                family.resolve_command(&self.paths, provider.as_deref())?
            }
        };
        if let Some(cwd) = workspace_cwd {
            command.cwd = cwd;
        }
        let session_provider = provider::catalog_registry()
            .provider_for_kind(&kind)
            .stamp_session_provider(provider.clone(), command.provider_id.clone());
        // One clock read: the journal row and the wire metadata must carry
        // the same instant so a caller can compare them.
        //
        // A created child inherits its creator's stored origin and never
        // re-derives one from the connection this thread happens to hold: the
        // MCP connection of a peer's session is a loopback socket, and reading
        // *it* would label a peer's child as this machine's own (S5 checklist).
        let origin = meta
            .origin
            .clone()
            .unwrap_or_else(|| session_origin_for(conn_peer));
        let title = match meta.display_name.clone() {
            Some(name) => name,
            // S9: agent-ness is one protocol predicate, not a kind list — the
            // same four kinds `hosts_mcp` serves, spelled once in the protocol.
            None => {
                if kind.is_agent() {
                    "Agent".to_string()
                } else {
                    "Terminal".to_string()
                }
            }
        };
        let mut record = new_session_record(
            id.clone(),
            owner.user.clone(),
            workspace_id.clone(),
            kind.clone(),
            title,
        );
        record.provider = session_provider.clone();
        // The name a human reads and the session that asked for this one are
        // the row's, not just the wire metadata's (audit S5-12): an app that
        // attaches to this daemon after a restart lists its sessions from the
        // journal, and a child that came back without its name and its parent
        // would be a different session than the one that was created.
        record.display_name = meta.display_name.clone();
        record.created_by = meta.created_by.clone();
        // The creation-from-profile facts, written once, here, and never
        // re-derived from the store afterwards (v11). A create that resolved no
        // profile — the human's provider picker, a terminal — leaves them at
        // their defaults, and a create that did leaves the daemon's own record
        // of it: the profile's **stable id** (a rename later cannot make this
        // child misreport what it was started from), the labels the creation
        // stamped, and the context this session belongs to.
        record.profile_id = meta.profile_id.clone();
        // The marker, derived here from the **delivered** mode (R2b): this is
        // the one place the kind and the delivery the child is started on meet
        // the row, so the marker is the delivery's own judgement — a profile's
        // feature tick is not an input, and a create that resolved no profile
        // is judged by its family's own default. ACP vocabularies are the
        // agent's own prose, so they answer `unknown` unless the daemon's
        // broker itself answers the delivered id.
        let unattended_state =
            crate::peer_policy::unattended_mode(kind.clone(), delivery.mode_id.as_deref());
        record.unattended_state = unattended_state;
        record.labels = meta.labels.clone();
        // Its own id, unless its creator's context came in with the creation:
        // that inheritance is the whole rule, and it is applied once, here, so
        // every reader — the roster, the journal, the A2A answer — sees one
        // value.
        let context_id = meta.context_id.clone().unwrap_or_else(|| id.clone());
        record.context_id = Some(context_id.clone());
        record.status = PersistStatus::Live;
        // The origin is a property of the create, not of the spawn: it is
        // recorded before the row is journaled, so a create that dies during
        // spawn still reads back as the device that asked for it.
        record.origin = origin.clone();
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
            origin,
            display_name: meta.display_name.clone(),
            created_by: meta.created_by.clone(),
            profile_id: meta.profile_id.clone(),
            context_id: Some(context_id),
            unattended: unattended_state,
            labels: meta.labels.clone(),
        };
        crate::agent_env::inject_session_env(
            &mut command,
            &metadata.id,
            metadata.workspace_id.as_deref(),
            &self.paths,
        );
        let mcp_session = if crate::mcp_broker::hosts_mcp(&kind) {
            state.mcp.register_with_provider(
                &metadata.id,
                owner,
                &kind,
                session_provider.as_deref(),
                crate::mcp_broker::AgentLineage {
                    depth: meta.depth,
                    overlay: meta.overlay.clone(),
                },
            )?
        } else {
            None
        };
        // Journal the row BEFORE spawn. A short-lived command (cmd /c echo)
        // can EOF and enqueue MarkEnded before this function would otherwise
        // reach try_upsert, and the journal thread would then see a missing
        // session and leave status=live — recovered-as-killed on reopen.
        if let Some(journal) = &self.journal {
            journal.try_upsert(record);
            self.invalidate_journal_roster();
        }
        // An agent's child is a creation that has not committed yet (audit-2
        // §2): its end can arrive before the link exists, so the end is parked
        // for the commit rather than run against a link that is not there.
        if meta.creation_pending {
            self.note_pending_child(
                &id,
                meta.reservation
                    .expect("an agent child holds a reservation"),
            );
        }
        // The journal row above is the durable product boundary. A failed
        // spawn must end that row, or the next roster render resurrects a
        // phantom recovered session with zero events.
        match spawn_session(
            state,
            self,
            metadata.clone(),
            owner.clone(),
            command,
            mcp_session,
            delivery,
        ) {
            Ok(()) => {
                // Whose spawn success measures provider health is the impls'
                // `spawn_measures_health`, read through the registry; the
                // per-family reasons live there.
                if provider::catalog_registry()
                    .provider_for_kind(&kind)
                    .spawn_measures_health()
                {
                    if let Some(provider_id) = &metadata.provider {
                        state.record_provider_health(provider_id, Ok(()));
                    }
                }
            }
            Err(error) => {
                // The token rollback clears what the reservation noted: the
                // creation never became a child, so nothing is owed to anyone
                // (audit-2 §2).
                self.clear_pending_child(&metadata.id);
                if let Some(journal) = &self.journal {
                    // The row is ended **synchronously**: this is the last
                    // line between the row this function wrote and the
                    // caller's refusal, and an end left to a fire-and-forget
                    // thread is an end a daemon death in that window undoes —
                    // the row would come back `status=live` and resurrect a
                    // phantom recovered session, the exact fate the
                    // row-before-spawn rule exists to prevent (the R2a
                    // audit's F8). The blocking send is a ~5 ms busy-loop on
                    // a queue that just accepted this process's writes; on
                    // this rare failure path that wait is cheaper than the
                    // phantom.
                    let _ = journal.mark_ended_blocking(&metadata.id, record_generation, None);
                }
                if let Some(provider_id) = &metadata.provider {
                    // Only a failure of the provider or the pipe says
                    // anything about the provider's health. A creation-time
                    // refusal the profile alone decides — an unknown model
                    // or mode, an `autoAccept` contradiction, an agent
                    // refusing the delivered switch — is `InvalidRequest` by
                    // convention across the clients, and a profile mistake
                    // must not mark a healthy provider unhealthy (the R2a
                    // audit's F6).
                    if spawn_failure_is_provider_health(&error) {
                        state.record_provider_health(provider_id, Err(&error));
                    }
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
        let runtime = match self.runtime_for_user(session_id, owner, conn) {
            Ok(runtime) => runtime,
            Err(error) if error.code == ErrorCode::SessionNotFound => {
                self.hydrate_transcript(session_id, from_cursor, owner, conn)?
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
        let runtime = self.runtime_for_user(session_id, owner, conn)?;
        runtime.claim_resize(conn.id, subscription_id)
    }

    pub fn resume(
        &self,
        state: &Arc<ServerState>,
        session_id: &str,
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<Session, WireError> {
        // The resume road answers "which providers exist" too: the row a
        // session was created under must still resolve here, so the file is
        // refreshed at the boundary like the create road's.
        crate::user_providers::refresh_user_rows(self.runtime_dir());
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
                check_user_owner(entry, owner, &conn.conn_peer)?;
                // *"Is the child that holds this entry still running?"* —
                // asked over `as_child_process`, because a `Configuring`
                // entry is a running child the same way a `Live` one is.
                // Over the peer-visibility accessor the refusal silently
                // stopped covering the delivery window, and a resume there
                // replaced a running child out from under its in-flight
                // create (the re-audit's P2-1).
                if entry
                    .as_child_process()
                    .is_some_and(|session| !session.runtime.process_exited())
                {
                    return Err(WireError::new(
                        ErrorCode::InvalidRequest,
                        "This session cannot be resumed while its process is running.",
                    ));
                }
            }
            let old_entry = map.remove(session_id);
            let had_live_slot = matches!(
                old_entry,
                Some(RegistryEntry::Live(_)) | Some(RegistryEntry::Configuring(_))
            );
            (old_entry, had_live_slot)
        };
        if let Some(old_entry) = old_entry {
            match old_entry {
                RegistryEntry::Live(session) | RegistryEntry::Configuring(session) => {
                    // A resume replaces a live entry: the process that held it
                    // is gone, so this is a child's end like any other (`S5`
                    // decisions 7 and 8, audit S5-01) — reported once and its
                    // slot released, whether the resume then succeeds or fails.
                    // A resumed session is not a creation, so the session that
                    // comes back has no row to release later.
                    self.child_ended_with(
                        session_id,
                        Some(&session.metadata),
                        Some(&session.runtime),
                        Some(owner),
                    );
                    session.runtime.detach_if_conn(conn.id);
                    session.runtime.notify_generation_replaced(conn.id);
                    teardown_session_for_resume(*session);
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
        // A resumed agent-created session is still that creator's child (audit
        // S5B-05). The journal has carried `created_by` since the slice-5
        // migration, so the lineage is read back instead of being dropped: with
        // the creator live the session re-enters the bookkeeping at the same
        // depth, and a session whose creator is gone stays an ordinary one
        // (`created_by` is kept on the row for the roster, and nothing is
        // counted). The *overlay* and the *quiet* preference are not persisted;
        // a resume comes back with the root's overlay and reports its end.
        let resumed_child = self
            .journal_roster()
            .and_then(|rows| rows.into_iter().find(|row| row.id == session_id))
            .and_then(|row| row.created_by);
        let lineage = match resumed_child.as_deref() {
            Some(creator) if self.live_runtime(creator, owner).is_some() => {
                crate::mcp_broker::AgentLineage {
                    depth: 1,
                    overlay: crate::provider_catalog::ToolOverlay::NONE,
                }
            }
            _ => crate::mcp_broker::AgentLineage::root(),
        };
        // S9 kind-preserving fix: the gate above (`resume_handle`) admits ACP only,
        // so this IS Acp today — but the kind comes from the record, never from a
        // literal, so a resumed session re-registers with its own kind rather than
        // as whatever the last author assumed. Pi/Codex resume stays refused at the
        // gate (deliberate: family resume is undesigned — see `resume_handle`).
        let mcp_session = match state.mcp.register_with_provider(
            session_id,
            owner,
            &record.kind,
            Some(provider.as_str()),
            lineage,
        ) {
            Ok(mcp_session) => mcp_session,
            Err(error) => {
                state.session_finished();
                return Err(error);
            }
        };
        if let Err(error) = journal.start_generation(session_id, generation) {
            drop(mcp_session);
            state.session_finished();
            return Err(error.into());
        }
        self.readmit_agent_child(session_id, resumed_child.as_deref(), owner);
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
            ResumedSessionContext {
                peer_session_id,
                generation,
                mcp_session,
            },
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
        conn: &ConnHandle,
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
        // A recovered session carries the origin of the create that made it,
        // so the peer gate and the permission card read the same fact a live
        // session would have had.
        runtime.set_origin(metadata.origin.clone());
        if let Some(peer_session_id) = record.peer_session_id.clone() {
            runtime.restore_peer_session_id(peer_session_id);
        }
        {
            let Ok(mut map) = self.inner.lock() else {
                journal.unpin(session_id);
                return Err(internal("Session state is unavailable."));
            };
            if let Some(existing) = map.get(session_id) {
                check_user_owner(existing, owner, &conn.conn_peer)?;
                // Attach reaches this arm whenever the registry holds the
                // id — including inside the delivery window, where
                // `runtime_for_user` answered `SessionNotFound` and the
                // caller fell back here on exactly that code (the
                // re-audit's P2-2). Handing back the windowed child's
                // runtime through the fallback would re-open the window the
                // door just closed.
                if existing.is_configuring() {
                    journal.unpin(session_id);
                    return Err(not_found());
                }
                journal.unpin(session_id);
                return Ok(existing.runtime());
            }
            map.insert(
                session_id.to_string(),
                RegistryEntry::Transcript(Box::new(TranscriptSession {
                    metadata,
                    owner: session_owner,
                    runtime: Arc::clone(&runtime),
                })),
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
        let runtime = self.runtime_for_user(session_id, owner, conn)?;
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
            PermissionResponse {
                session_id,
                request_id,
                outcome,
                option_id: None,
            },
            conn.id,
            conn,
            owner,
        )
    }

    pub fn permission_respond_with_subscription(
        &self,
        response: PermissionResponse<'_>,
        subscription_id: u64,
        conn: &ConnHandle,
        owner: &OwnerId,
    ) -> Result<(), WireError> {
        let PermissionResponse {
            session_id,
            request_id,
            outcome,
            option_id,
        } = response;
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        if request_id.is_empty() {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "Permission request id is required.",
            ));
        }
        let runtime = self.runtime_for_user(session_id, owner, conn)?;
        check_attached(&runtime, conn, subscription_id)?;
        let broker = runtime.permission_broker().ok_or_else(|| {
            WireError::new(
                ErrorCode::InvalidRequest,
                "Session has no live ACP permission broker.",
            )
        })?;
        broker
            .respond_with_option(request_id, outcome, option_id.map(str::to_string))
            .map_err(|error| {
                let code = match error {
                    permission_broker::PermissionResponseError::NotFound => {
                        ErrorCode::InvalidRequest
                    }
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

    /// The delegated answer (`§4.1`): an agent answering its own child's
    /// permission card, through `devboule_answer_permission`.
    ///
    /// Identity is imposed — `creator_session_id` is the caller's bearer-
    /// mapped session, never a tool argument (§0.1) — and every check runs
    /// inside [`permission_broker::PermissionBroker::answer_delegated_on`],
    /// in the spec's order, with this registry's facts supplied as the
    /// testimony the checks consume. A refusal from anywhere in the chain
    /// leaves the card pending and untouched.
    pub(crate) fn answer_child_permission(
        &self,
        creator_session_id: &str,
        card_id: &str,
        outcome: PermissionOutcome,
        device_caps: &dyn Fn(&str) -> Vec<String>,
    ) -> Result<(), String> {
        // The caller's own row: its owner scopes the card scan, its origin
        // decides whether the capability check applies. The MCP registration
        // guarantees the caller is live, so an absent row is a refusal, not a
        // panic.
        let (owner_user, creator_origin) = {
            let map = self
                .inner
                .lock()
                .map_err(|_| "session state is unavailable".to_string())?;
            let entry = map.get(creator_session_id).ok_or_else(|| {
                "the calling session is not registered on this daemon".to_string()
            })?;
            (entry.owner().user.clone(), entry.to_session().origin)
        };
        // Locate the broker that holds the card. The scan is read-only:
        // locating is not answering, and every check still runs below.
        //
        // Owner-scoped, so a card that exists on one of the owner's sessions
        // but not on a child's stays "found" and the chain's child check
        // answers it with the not-your-child sentence — a state distinct
        // from "unknown card" (§1.5's three states). But the id is
        // provider-chosen and carries no session qualifier, so the holder
        // that answers must be the caller's own child: a child holder is
        // preferred over a non-child one, and more than one child holding
        // the same id is refused ambiguous rather than answered against
        // whichever session the map yields first.
        let (found, child_holders) = {
            let map = self
                .inner
                .lock()
                .map_err(|_| "session state is unavailable".to_string())?;
            let mut found: Option<std::sync::Arc<permission_broker::PermissionBroker>> = None;
            let mut found_is_child = false;
            let mut child_holders: usize = 0;
            for entry in map
                .values()
                .filter(|entry| entry.owner().user == owner_user)
            {
                let Some(broker) = entry.runtime().permission_broker() else {
                    continue;
                };
                if !matches!(
                    broker.peek_delegated(card_id),
                    permission_broker::DelegatedPeek::Found { .. }
                ) {
                    continue;
                }
                let is_child = entry.as_peer_visible().is_some_and(|live| {
                    live.metadata.created_by.as_deref() == Some(creator_session_id)
                });
                if is_child {
                    child_holders += 1;
                }
                if found.is_none() || (is_child && !found_is_child) {
                    found = Some(std::sync::Arc::clone(&broker));
                    found_is_child = is_child;
                }
            }
            (found, child_holders)
        };
        if child_holders > 1 {
            return Err(format!(
                "more than one of your live children holds permission card {card_id}; the cards stay pending for the human"
            ));
        }
        // Check 2's closure: the switch, read at the moment the check runs.
        let switch_on = || self.delegation_enabled();
        // Check 3's closure: a resolved card is a row in the ledger replay
        // reads back. A journal that cannot answer reads `false` — the
        // sentence becomes "unknown", which is inert in both cases.
        let resolved_elsewhere = |request_id: &str| {
            self.journal
                .as_ref()
                .map(|journal| journal.permission_was_recorded(request_id).unwrap_or(false))
                .unwrap_or(false)
        };
        // Check 4's closure: the card's session is a **live child of the
        // caller** — `created_by` equals the bearer's session, and the view
        // exists. A sibling, a grandchild, a human-started session or a dead
        // one fails here without learning which session owns the card. The
        // session that passed the check is remembered so the attention it was
        // waiting under clears when the answer lands.
        let answered_child: std::cell::RefCell<Option<String>> = std::cell::RefCell::new(None);
        let child_check = |card_session: &str| -> Result<(), String> {
            let Some((session, _runtime, _owner)) = self.child_view(card_session) else {
                return Err(format!(
                    "permission card {card_id} is not pending on one of your live sessions"
                ));
            };
            if session.created_by.as_deref() != Some(creator_session_id) {
                return Err(format!(
                    "permission card {card_id} belongs to a session that is not your child; it stays pending for whoever may answer it"
                ));
            }
            *answered_child.borrow_mut() = Some(card_session.to_string());
            Ok(())
        };
        // Check 5's closure: a creator whose stored origin is a paired
        // device answers only what the peer gate allows — judged with the same
        // `peer_allows` function the dispatcher uses, on the same wire message
        // the broker door names for this tool (`SessionPermissionRespond`), never
        // a copy of its conclusions. A local creator is the person at this
        // machine's own agent. A peer-shaped row without a device or role is
        // an unknown, and the unknown never renders as the benign one.
        let caps_check = |_: &str| -> Result<(), String> {
            if creator_origin.kind == SessionOriginKind::Peer {
                let (Some(device_id), Some(role)) =
                    (creator_origin.device_id.as_deref(), creator_origin.role)
                else {
                    return Err(
                        "the calling session's origin is unknown; the card stays pending"
                            .to_string(),
                    );
                };
                let caps = device_caps(device_id);
                let request = devboule_protocol::ClientMessage::SessionPermissionRespond {
                    id: 0,
                    session_id: String::new(),
                    subscription_id: 0,
                    request_id: String::new(),
                    outcome: devboule_protocol::PermissionOutcome::Deny,
                    option_id: None,
                    idempotency_key: None,
                };
                if let crate::peer_policy::PeerDecision::Deny(reason) =
                    crate::peer_policy::peer_allows(role, &caps, &request)
                {
                    return Err(format!(
                        "{}; the card stays pending",
                        crate::peer_policy::capability_refusal_message(reason)
                    ));
                }
            }
            Ok(())
        };
        permission_broker::PermissionBroker::answer_delegated_on(
            found.as_deref(),
            card_id,
            outcome,
            &switch_on,
            &resolved_elsewhere,
            &child_check,
            &caps_check,
            creator_session_id,
        )?;
        // The child may have been waiting in attention for this answer: the
        // card that just resolved was the reason it was raised.
        if let Some(child) = answered_child.into_inner() {
            if let Some((_session, runtime, owner)) = self.child_view(&child) {
                if runtime.clear_attention() {
                    self.notify_session_transition(&owner, &child);
                }
            }
        }
        Ok(())
    }

    /// A creator moves its own live child onto a profile (slice 5b §2, Pass
    /// A), through `devboule_set_agent_profile`.
    ///
    /// Identity is imposed — `creator_session_id` is the caller's bearer-mapped
    /// session, never a tool argument (§0.1) — and the checks run in the
    /// spec's order, each refusal naming its reason and leaving the child
    /// untouched:
    ///
    /// 1. The caller is registered. The MCP registration guarantees it; a row
    ///    that has gone is a refusal, not a panic.
    /// 2. The target resolves **by id or display name among the caller's own
    ///    live children only** — visible, and `created_by` equals the caller.
    ///    The caller itself, a sibling, a grandchild, a human-started session
    ///    and an invented or dead name each get the sentence that case earns
    ///    without leaking anything a roster does not already show the same
    ///    owner; a name two live children share is refused ambiguous rather
    ///    than resolved to one of them.
    /// 3. The profile is resolved **now** by the caller's closure — the
    ///    broker's `resolve_profile`, with the unticked refusal §1.2 demands —
    ///    and never from a list read earlier.
    /// 4. The mode ask goes through [`Self::set_mode`] on the internal
    ///    connection (`ConnHandle::with_peer(0, None)`, the `send_message`
    ///    precedent): the child's **own manifest** must advertise the id, and
    ///    a provider that cannot switch a live session answers on its own
    ///    wire. **The child is never restarted.** A manifest nobody has
    ///    delivered yet is the third state: the daemon cannot say yet, and the
    ///    refusal withholds.
    /// 5. Only after the mode landed is the model asked, through
    ///    [`Self::set_model`]. A refusal there is a **partial** success: the
    ///    answer reports exactly what landed, records **no** profile change —
    ///    and the `unattended` ratchet still fires, because the child has in
    ///    fact been able to run in that mode and that cannot be un-lived.
    ///
    /// On a full success the child's row records the profile's stable id and
    /// the marker is the delivered mode's own judgement — the same
    /// `peer_policy::unattended_mode` the birth calls, raised (never lowered)
    /// through the journal's `MAX` ratchet and in the live metadata the
    /// snapshot serves.
    pub(crate) fn set_agent_child_profile(
        &self,
        creator_session_id: &str,
        target: &str,
        profile_name: &str,
        resolve_profile: &dyn Fn(&str) -> Result<ChildProfileFacts, String>,
    ) -> Result<(), String> {
        // Check 1: the caller's row. Its owner scopes the scan below; the
        // registry is the only place "mine" is a fact.
        let caller_owner = {
            let map = self
                .inner
                .lock()
                .map_err(|_| "session state is unavailable".to_string())?;
            let entry = map.get(creator_session_id).ok_or_else(|| {
                "the calling session is not registered on this daemon".to_string()
            })?;
            entry.owner().clone()
        };
        if target == creator_session_id {
            return Err("a session is not its own child; name a session you created".to_string());
        }
        // The name a child is addressed by, the same one the roster shows:
        // the display name a creation gave it, or the title beneath it.
        let display = |session: &Session| {
            session
                .display_name
                .clone()
                .unwrap_or_else(|| session.title.clone())
        };
        // Check 2: resolve among the caller's own live children only. The scan
        // is read-only; nothing is asked of any provider until a profile and a
        // mode have both been agreed.
        let (child_session, child_runtime, child_owner) = {
            let map = self
                .inner
                .lock()
                .map_err(|_| "session state is unavailable".to_string())?;
            let mut matches: Vec<(Session, Arc<SessionRuntime>, OwnerId)> = map
                .values()
                .filter(|entry| entry.owner().user == caller_owner.user)
                .filter_map(|entry| {
                    let live = entry.as_peer_visible()?;
                    Some((
                        live_session_view(live),
                        Arc::clone(&live.runtime),
                        entry.owner().clone(),
                    ))
                })
                .filter(|(session, _, _)| session.created_by.as_deref() == Some(creator_session_id))
                .filter(|(session, _, _)| session.id == target || display(session) == target)
                .collect();
            match matches.len() {
                1 => Ok(matches.pop().expect("exactly one match")),
                0 => {
                    // What this owner's own live roster distinguishes is
                    // distinguished: a live session of theirs that is not the
                    // caller's child is told what it is. Everything else — an
                    // invented name, a dead child, a stranger's session — is
                    // one refusal, because the daemon cannot and must not say
                    // which.
                    let not_child = map.values().any(|entry| {
                        entry.owner().user == caller_owner.user
                            && entry.as_peer_visible().is_some_and(|live| {
                                let session = live_session_view(live);
                                session.created_by.as_deref() != Some(creator_session_id)
                                    && (session.id == target || display(&session) == target)
                            })
                    });
                    if not_child {
                        Err(format!(
                            "'{target}' is not your child; only a session you created can be moved onto a profile"
                        ))
                    } else {
                        Err(format!(
                            "none of your live children is called '{target}'; devboule_list_agents names them"
                        ))
                    }
                }
                _ => Err(format!(
                    "more than one of your live children is called '{target}'; use the session id"
                )),
            }
        }?;
        // Check 3: the profile, read at the moment of the call — the closure
        // owns the store and the three refusals §1.2 names.
        let facts = resolve_profile(profile_name)?;
        // Check 4's pre-read: a manifest nobody has delivered yet is not "the
        // mode is unavailable" — it is "the daemon cannot say yet", and the
        // refusal withholds. A manifest that arrived and names no modes is the
        // provider's own say-so, and `set_mode`'s sentence for it stands.
        if child_runtime.session_manifest().is_none() {
            return Err(format!(
                "the daemon cannot say yet whether mode '{}' is available on this child: its provider has not reported the session's manifest; ask again once the child is up",
                facts.mode_id
            ));
        }
        let internal_conn = ConnHandle::with_peer(0, None);
        self.set_mode(
            &child_session.id,
            &child_owner,
            &facts.mode_id,
            &internal_conn,
        )
        .map_err(|error| error.message)?;
        // Check 5: the model, only after the mode landed. A child already
        // running the profile's model with no thinking option to deliver asks
        // nothing — there is no ask to make — and every other combination is
        // asserted on the provider's own wire, Claude's effort validation
        // included where it applies.
        let current_model = child_runtime
            .session_manifest()
            .and_then(|event| match event {
                SessionEvent::SessionManifest {
                    current_model_id, ..
                } => current_model_id,
                _ => None,
            });
        let model_ask_needed = current_model.as_deref() != Some(facts.model.as_str())
            || facts.thinking_option_id.is_some();
        if model_ask_needed {
            if let Err(error) = self.set_model(
                &child_session.id,
                &child_owner,
                Some(&facts.model),
                facts.thinking_option_id.as_deref(),
            ) {
                // The partial state: the mode landed, the model ask did not.
                // The ratchet still fires — the child has been able to run in
                // that mode, and that cannot be un-lived — but **no** profile
                // change is recorded, and the answer says exactly what stands.
                self.record_child_profile_move(
                    &child_session.id,
                    &child_session.kind,
                    &facts.mode_id,
                    None,
                );
                return Err(format!(
                    "the mode was switched to '{}', but the model ask was refused: {}. the child runs in mode '{}' on its previous model, and no profile change is recorded",
                    facts.mode_id, error.message, facts.mode_id
                ));
            }
        }
        // Full success: record the profile and raise the marker through the
        // one predicate — the delivered mode's own judgement, the same
        // function the birth calls.
        self.record_child_profile_move(
            &child_session.id,
            &child_session.kind,
            &facts.mode_id,
            Some(&facts.profile_id),
        );
        Ok(())
    }

    /// The recording half of a move: the journal row's `profile_id` and the
    /// `unattended` ratchet, then the live metadata the snapshot serves,
    /// raised — never lowered — with the same rank the SQL `MAX` compares.
    ///
    /// The asks that already landed cannot be un-lived, so a journal that
    /// cannot take the write degrades the recording; it never refuses the
    /// move and never erases the marker.
    fn record_child_profile_move(
        &self,
        child_id: &str,
        child_kind: &SessionKind,
        delivered_mode: &str,
        profile_id: Option<&str>,
    ) {
        let marker = crate::peer_policy::unattended_mode(child_kind.clone(), Some(delivered_mode));
        if let Some(journal) = &self.journal {
            if let Err(error) = journal.set_agent_profile_row(child_id, profile_id, marker) {
                eprintln!("agent profile row update failed for {child_id}: {error}");
            }
        }
        if let Ok(mut map) = self.inner.lock() {
            if let Some(live) = map
                .get_mut(child_id)
                .and_then(RegistryEntry::as_peer_visible_mut)
            {
                if let Some(profile_id) = profile_id {
                    live.metadata.profile_id = Some(profile_id.to_string());
                }
                if crate::journal::unattended_state_rank(marker)
                    > crate::journal::unattended_state_rank(live.metadata.unattended)
                {
                    live.metadata.unattended = marker;
                }
            }
        }
        self.invalidate_journal_roster();
        self.invalidate_state_roster_cache();
        if let Some((_session, _runtime, owner)) = self.child_view(child_id) {
            self.notify_session_transition(&owner, child_id);
        }
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
            let entry = peer_entry_mut(&mut map, session_id, owner, &None)?;
            let session = entry.as_peer_visible_mut().ok_or_else(process_gone)?;
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
            let entry = peer_entry_mut(&mut map, session_id, owner, &conn.conn_peer)?;
            let session = entry.as_peer_visible_mut().ok_or_else(process_gone)?;
            (session.killer.clone_killer(), Arc::clone(&session.runtime))
        };
        check_attached(&runtime, conn, subscription_id)?;
        {
            let mut map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            // Peer-visible shape on purpose, though this is bookkeeping: a
            // `Configuring` entry here would be a *different* child — the
            // resume that replaced the one just killed — and must not
            // inherit its `preserve_on_exit`.
            if let Some(session) = map
                .get_mut(session_id)
                .and_then(RegistryEntry::as_peer_visible_mut)
            {
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

    /// Close a session, or a previous run's row for the same user.
    ///
    /// `conn_peer` is the requestor's connection identity: a paired device
    /// may close only what `check_user_owner` opens to it, and the internal
    /// callers that reap a half-started session pass `&None` because no peer
    /// asked for that close.
    pub fn close(
        &self,
        session_id: &str,
        owner: &OwnerId,
        conn_peer: &Option<ConnPeer>,
    ) -> Result<bool, WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let session = {
            let mut map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            if let Some(entry) = map.get(session_id) {
                check_user_owner(entry, owner, conn_peer)?;
                // Close is teardown: it reaches through the delivery window
                // exactly like the `Configuring` arm below, so the same
                // child-slot accessor answers for both variants here. (For a
                // windowed child the store is a no-op — `transition_ready`
                // is not raised until the delivery lands and promotes.)
                if let Some(session) = entry.as_child_process() {
                    session
                        .runtime
                        .transition_ready
                        .store(false, Ordering::Release);
                }
            }
            // The closed session's message-brake entries go in the same critical
            // section that takes it out of the map (A2-06): a send that found it
            // here cannot reserve a slot for it afterwards (A2-05).
            forget_message_brake_target(&self.message_brakes, session_id);
            map.remove(session_id)
        };
        self.forget_agent_creator(session_id);
        match session {
            // A `Configuring` entry closes exactly like a live one: the
            // delivery-refusal path tears a half-started child down through
            // this arm, and teardown is the one thing the delivery window
            // must never block.
            Some(RegistryEntry::Live(session)) | Some(RegistryEntry::Configuring(session)) => {
                // The last chance to report this child to its creator (`S5` §3,
                // audit S5-01): the row is out of the map, the runtime is still
                // here, and the report is claimed exactly once, so a child whose
                // turn already reported finds nothing owed.
                //
                // Report *then* release: the claim of the report reads the
                // child's link in the creation table, which the release removes.
                // An end that released first would silently owe the creator
                // nothing but the caps, which is what the audit found.
                self.child_ended_with(
                    session_id,
                    Some(&session.metadata),
                    Some(&session.runtime),
                    Some(owner),
                );
                if let Some(journal) = &self.journal {
                    journal.try_mark_closed(session_id);
                    journal.unpin(session_id);
                    self.invalidate_journal_roster();
                }
                teardown_session(*session);
                // The attachments existed for this session's turns. Removing
                // them here is the normal path; `sweep_attachments` on the next
                // daemon start is the fallback for a close that never ran.
                self.attachments.remove_session(session_id);
                self.notify_session_transition(owner, session_id);
                Ok(true)
            }
            Some(RegistryEntry::Transcript(_)) => {
                // A transcript carries no runtime to report with, so it only
                // gives a slot back. A child that ended by EOF was released by
                // `finish_reader_session` already, and a recovered transcript
                // has no row at all after a restart.
                self.release_agent_child(session_id);
                if let Some(journal) = &self.journal {
                    journal.try_mark_closed(session_id);
                    journal.unpin(session_id);
                    self.invalidate_journal_roster();
                }
                self.attachments.remove_session(session_id);
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
                        self.attachments.remove_session(session_id);
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
            let entry = peer_entry_mut(&mut map, session_id, owner, &conn.conn_peer)?;
            let session = entry.as_peer_visible_mut().ok_or_else(process_gone)?;
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
            let entry = peer_entry_mut(&mut map, session_id, owner, &None)?;
            let session = entry.as_peer_visible_mut().ok_or_else(process_gone)?;
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
        let result = switcher.set_model(model_id, effort);
        if result.is_ok() {
            if let Some(manifest) = switcher.manifest() {
                let manifest = runtime.store_session_manifest(manifest);
                let _ = runtime.publish_agent_event(manifest, None);
            }
        }
        result
    }

    /// Switch a live agent session's mode.
    ///
    /// The connection is threaded through like `interrupt_with_subscription`
    /// and `close`: `SessionSetMode` is under `CAP_SEND`, so it *is* reachable
    /// from a paired device, and the identity of the caller is part of the
    /// authorization the ownership check makes (§8b A3/A4/A5, H5). Without the
    /// connection the call site could only answer with the owner comparison,
    /// which is what let a mode change arrive with no origin attached.
    pub fn set_mode(
        &self,
        session_id: &str,
        owner: &OwnerId,
        mode_id: &str,
        conn: &ConnHandle,
    ) -> Result<(), WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        if mode_id.is_empty() {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "A mode is required.",
            ));
        }
        let (switcher, runtime) = {
            let mut map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            let entry = peer_entry_mut(&mut map, session_id, owner, &conn.conn_peer)?;
            let session = entry.as_peer_visible_mut().ok_or_else(process_gone)?;
            if !session.metadata.kind.is_agent() {
                return Err(WireError::new(
                    ErrorCode::InvalidRequest,
                    "Only agent sessions support switching the session mode.",
                ));
            }
            let manifest = session.runtime.session_manifest();
            let modes = match manifest.as_ref() {
                Some(SessionEvent::SessionManifest {
                    modes: Some(modes), ..
                }) => modes,
                _ => {
                    return Err(WireError::new(
                        ErrorCode::InvalidRequest,
                        "This provider has not advertised any session modes.",
                    ));
                }
            };
            if !modes.available_modes.iter().any(|mode| mode.id == mode_id) {
                return Err(WireError::new(
                    ErrorCode::InvalidRequest,
                    format!("Session mode '{mode_id}' is not available."),
                ));
            }
            let switcher = session
                .switcher
                .as_ref()
                .map(|switcher| switcher.clone_switcher())
                .ok_or_else(|| {
                    WireError::new(
                        ErrorCode::InvalidRequest,
                        "This provider does not support switching the session mode.",
                    )
                })?;
            (switcher, Arc::clone(&session.runtime))
        };
        switcher.set_mode(mode_id)?;
        let Some(SessionEvent::SessionManifest {
            provider_id,
            current_model_id,
            models,
            modes: Some(mut modes),
        }) = runtime.session_manifest()
        else {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "Session mode state disappeared while switching.",
            ));
        };
        modes.current_mode_id = mode_id.to_string();
        let manifest = runtime.store_session_manifest(SessionEvent::SessionManifest {
            provider_id,
            current_model_id,
            models,
            modes: Some(modes),
        });
        let _ = runtime.publish_agent_event(manifest, None);
        Ok(())
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

    /// Store one prompt attachment for a session and answer the reference the
    /// send that follows will name.
    ///
    /// The connection is threaded through for the same reason `set_mode`'s is:
    /// `SessionDeposit` is under `CAP_SEND` (`peer_policy.rs`), so a paired
    /// device *is* reachable here, and the requestor's identity is part of the
    /// authorization the ownership check makes (§8b A3/A4/A5, H5).
    ///
    /// The wire's own limits are enforced before the store sees the attachment
    /// (DEP-06). The store's `prepare` decodes the base64 and walks the image,
    /// which is the expensive half of a deposit, and a frame the protocol
    /// already refuses must not pay for it; the refusal is also the protocol's
    /// sentence rather than a store error, so an attachment that is too large
    /// reads the same here as it does on a send.
    ///
    /// The reference's digest and `stored_bytes` are the store's to state, not
    /// this function's: the digest names the bytes *as stored* (the strip makes
    /// them differ from what was sent) and the size is the file's own, read from
    /// the disk.
    ///
    /// The window between the ownership check and the store write is closed from
    /// the far side: the store writes with no registry lock held, so a `close`
    /// that lands in the middle of it is detected by the re-check below and the
    /// write is undone with it. A close that lands *after* that re-check is the
    /// same race every operation in this file has with close, and it is
    /// accepted — the folder goes away with the session, as it would for a send
    /// whose bytes were already in the provider's hands.
    pub(crate) fn deposit(
        &self,
        session_id: &str,
        owner: &OwnerId,
        conn: &ConnHandle,
        attachment: &PromptAttachment,
    ) -> Result<AttachmentReference, WireError> {
        {
            let map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            let entry = map.get(session_id).ok_or_else(not_found)?;
            check_user_owner(entry, owner, &conn.conn_peer)?;
        }
        #[cfg(test)]
        self.fire_deposit_after_ownership_hook();
        validate_attachments(std::slice::from_ref(attachment))
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let deposited = self.attachments.deposit(session_id, attachment)?;
        // The store wrote outside the registry lock, so a `close` that landed in
        // the meantime has already removed this session's folder and the file
        // just written belongs to a session that no longer exists: nothing will
        // ever close it, and it stays charged to the store's budget until the
        // retention sweep. The entry's absence is the receipt that the close won,
        // so the write is undone under the lock that decides it — taken *after*
        // the store released its own, never across it.
        let gone = {
            let map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            map.get(session_id).is_none()
        };
        if gone {
            self.attachments.remove_session(session_id);
            return Err(not_found());
        }
        Ok(AttachmentReference {
            session_id: session_id.to_string(),
            digest: deposited.digest,
            stored_bytes: deposited.stored_bytes,
        })
    }

    /// Deposit a finished child's whole last message in the **creator's**
    /// folder and answer the artifact the report names (`S5` decision 10).
    ///
    /// The same door every deposit uses ([`Self::deposit`]): ownership, the
    /// wire's own limits, the store's type table and its budget. The artifact is
    /// charged to the creator's folder exactly like any other attachment, which
    /// is the point of depositing it as one — a child's result is not a way
    /// around the meter.
    fn deposit_child_message(
        &self,
        creator: &str,
        owner: &OwnerId,
        message: &AgentMessageSnapshot,
    ) -> Result<FinishArtifact, String> {
        use base64::Engine as _;
        let bytes = message.text.as_bytes();
        if bytes.len() > MAX_AGENT_ARTIFACT_BYTES {
            return Err(format!(
                "Its message is {} bytes and was not deposited; the artifact cap is {MAX_AGENT_ARTIFACT_BYTES}.",
                bytes.len()
            ));
        }
        let attachment = PromptAttachment {
            name: "agent-finished.md".to_string(),
            mime_type: "text/markdown".to_string(),
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        };
        let internal_conn = ConnHandle::with_peer(0, None);
        let reference = self
            .deposit(creator, owner, &internal_conn, &attachment)
            .map_err(|error| format!("Its message was not stored: {}", error.message))?;
        let url = format!(
            "devboule-attachment:{}/{}",
            reference.session_id, reference.digest
        );
        Ok(FinishArtifact {
            artifact_id: url.clone(),
            parts: vec![FinishArtifactPart {
                url,
                mime_type: "text/markdown".to_string(),
                metadata: Some(FinishArtifactPartMetadata {
                    stored_bytes: reference.stored_bytes,
                }),
            }],
        })
    }

    #[cfg(test)]
    pub fn send(
        &self,
        session_id: &str,
        text: &str,
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<(), WireError> {
        self.send_with_subscription(session_id, conn.id, text, &[], &[], owner, conn)
    }

    /// One prompt: the text, the inline attachments, and the references to
    /// attachments already deposited under this session.
    ///
    /// The two halves are separate arguments rather than one list because they
    /// travel differently — the inline bytes are in the frame the client built,
    /// a reference is a digest the daemon resolves against the store — and the
    /// send path keeps them apart from validation through to the prompt.
    ///
    /// The argument list is one past clippy's limit and stays a list: the
    /// struct that would collapse it exists (`SendRequest`), and the layer
    /// below already takes it — this is the thin entry point 20 call sites use,
    /// and giving them a struct to build would move the argument count into
    /// them rather than remove it. The crate makes this trade in nine other
    /// places.
    #[allow(clippy::too_many_arguments)]
    pub fn send_with_subscription(
        &self,
        session_id: &str,
        subscription_id: u64,
        text: &str,
        attachments: &[PromptAttachment],
        attachment_references: &[AttachmentReference],
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<(), WireError> {
        self.send_with_subscription_behavior(
            session_id,
            subscription_id,
            text,
            attachments,
            attachment_references,
            owner,
            conn,
            None,
        )
        .map(|_| ())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn send_with_subscription_behavior(
        &self,
        session_id: &str,
        subscription_id: u64,
        text: &str,
        attachments: &[PromptAttachment],
        attachment_references: &[AttachmentReference],
        owner: &OwnerId,
        conn: &ConnHandle,
        active_turn_behavior: Option<ActiveTurnBehavior>,
    ) -> Result<(), WireError> {
        self.send_with_subscription_timeout(&SendRequest {
            session_id,
            subscription_id,
            text,
            attachments,
            attachment_references,
            owner,
            conn,
            mcp_timeout: crate::mcp_broker::ready_timeout(),
            active_turn_behavior,
            require_attachment: true,
            // The person at this machine, or a paired device: only the former
            // may have a refused steer fall back to an interrupt (S4-01).
            interrupt_on_steer_refusal: session_origin_for(&conn.conn_peer).is_local(),
            message_slot: None,
            // No preset preamble: a client's prompt is not a creation's.
            preset_preamble: None,
        })
        .map(|_| ())
    }

    pub(crate) fn agent_message_send(
        &self,
        from_session: &str,
        to_session: &str,
        text: &str,
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<(), WireError> {
        if from_session == to_session {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "An agent cannot send a message to itself.",
            ));
        }
        // Target admission and the brake slot are one critical section (A2-05).
        // While this holds the session map, no close can take the target out from
        // under the check and no second send of the same sender can take the slot
        // this one is taking: "the target is there and this caller may reach it"
        // and "the sender has a slot for it" cannot answer differently, and the
        // sender's entry in the brake table cannot outlive the target it names.
        // The brake table's own lock is taken underneath this one — never the
        // other way round — and released with it.
        //
        // The turn this message joins is *not* snapshotted here (S4-03): the
        // reservation below asks the target's runtime for it, in the same critical
        // section `finish_turn` takes, and its answer is what decides steer versus
        // prompt. A turn that ends after that answer cannot make the decision
        // wrong, because the answer arrived with the boundary registration.
        let (from_runtime, target_owner, admission) = {
            let map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            let source = peer_entry(&map, from_session, owner, &conn.conn_peer)?;
            let source = source.as_peer_visible().ok_or_else(process_gone)?;
            let target = peer_entry(&map, to_session, owner, &conn.conn_peer)?;
            let target = target.as_peer_visible().ok_or_else(process_gone)?;
            // Refused here, inside the same section: a message that would cross
            // two peer hops never reaches the brake table, so the refusal cannot
            // leave a slot behind it.
            let from_origin = source.metadata.origin.clone();
            let target_origin = target.metadata.origin.clone();
            if from_origin.kind == SessionOriginKind::Peer
                && target_origin.kind == SessionOriginKind::Peer
            {
                return Err(WireError::new(
                    ErrorCode::CapabilityNotSupported,
                    "Forwarding agent messages beyond one peer hop is not supported; do not retry.",
                ));
            }
            let admission = reserve_message_brake(
                &self.message_brakes,
                from_session,
                to_session,
                Some((&target.runtime, target.runtime.turn_counter())),
                Instant::now(),
            )?;
            (Arc::clone(&source.runtime), target.owner.clone(), admission)
        };
        #[cfg(test)]
        self.fire_agent_message_after_admission_hook();
        // Who is speaking is the *caller's* connection, never the named source
        // session: a paired device that names one of this machine's own sessions
        // as `from_session` (its ownership check passes, because the session
        // belongs to the user that paired it) must not be described to the
        // receiving agent as `local` (S4-05). The named session is the agent the
        // text is attributed to, and that is the `from_agent` line.
        let caller_origin = session_origin_for(&conn.conn_peer);
        let origin = match caller_origin.kind {
            SessionOriginKind::Peer => {
                format!(
                    "peer:{}",
                    caller_origin.device_id.as_deref().unwrap_or_default()
                )
            }
            SessionOriginKind::Local => "local".to_string(),
            // A caller whose peer record says neither fact is not the person at
            // this machine (§8 R2): the envelope names it as the journal spells
            // it, and claims no device.
            SessionOriginKind::Unknown => "unknown".to_string(),
        };
        let role = match caller_origin.role {
            Some(PeerRole::Daemon) => "daemon",
            Some(PeerRole::Client) | None => "client",
        };
        let envelope = agent_message_envelope(&origin, role, from_session, text);
        let internal_conn = ConnHandle::with_peer(0, None);
        // (S4-10) The slot this delivery holds, so the plain-prompt fallback can
        // re-key its boundary if the turn it was admitted into ends first.
        let slot_ref = MessageSlotRef {
            brakes: &self.message_brakes,
            from_session,
            slot: admission.slot,
            admitted_turn_id: admission.expected_turn_id,
        };
        let result = self.send_with_subscription_timeout(&SendRequest {
            session_id: to_session,
            subscription_id: 0,
            text: &envelope,
            attachments: &[],
            // `AgentMessageSend` has no field for either half: an agent
            // message is the envelope's text, and nothing in this delivery
            // could have named a stored attachment.
            attachment_references: &[],
            owner: &target_owner,
            conn: &internal_conn,
            mcp_timeout: crate::mcp_broker::ready_timeout(),
            // (S4-03) Steer only if the runtime answered that the turn the
            // caller checked was still running when the boundary was registered:
            // that answer, not an earlier look, is what makes the delivery match
            // the decision.
            active_turn_behavior: admission
                .steered_into_turn
                .then_some(ActiveTurnBehavior::Steer),
            require_attachment: false,
            // The delivery itself is the daemon acting on the caller's behalf,
            // so a refused steer may only interrupt when the caller could have
            // asked for an interrupt itself (S4-01): a paired device's agent
            // message must not replace a running turn it may not stop.
            interrupt_on_steer_refusal: caller_origin.is_local(),
            message_slot: Some(&slot_ref),
            // No preset preamble: an agent message is not a creation's prompt.
            preset_preamble: None,
        });
        if result.is_ok() {
            // The sender sees the raw peer message in its own transcript; the
            // receiver sees the daemon envelope delivered above. The publish is
            // checked and surfaced like the receiver-side journal: the target
            // already has the text, so a sender-side recording failure is a
            // degraded session, never an error the caller could retry (S4-09).
            if from_runtime
                .publish_agent_user_message(text.to_string())
                .is_none()
            {
                from_runtime.mark_journal_degraded();
            }
            // The delivery returned: the slot now waits only for its boundary,
            // if this admission found one — the turn end it was admitted for.
            finish_message_delivery(&self.message_brakes, from_session, admission.slot, true);
        } else {
            // The message is in flight nowhere: give the slot back now instead
            // of holding the sender's budget until a boundary that will never see
            // this message arrives.
            finish_message_delivery(&self.message_brakes, from_session, admission.slot, false);
        }
        // The delivery's own id is not what this act answers with: the *sender*
        // is the caller here, and its echo (if any) is published above. The
        // receiver-side id is nobody's correlation key (S4-09).
        result.map(|_| ())
    }

    #[cfg(test)]
    fn send_with_mcp_timeout(
        &self,
        session_id: &str,
        text: &str,
        owner: &OwnerId,
        conn: &ConnHandle,
        timeout: Duration,
    ) -> Result<(), WireError> {
        self.send_with_subscription_timeout(&SendRequest {
            session_id,
            subscription_id: conn.id,
            text,
            attachments: &[],
            attachment_references: &[],
            owner,
            conn,
            mcp_timeout: timeout,
            active_turn_behavior: None,
            require_attachment: true,
            interrupt_on_steer_refusal: true,
            message_slot: None,
            preset_preamble: None,
        })
        .map(|_| ())
    }

    fn send_with_subscription_timeout(
        &self,
        request: &SendRequest<'_>,
    ) -> Result<Option<String>, WireError> {
        let SendRequest {
            session_id,
            subscription_id,
            text,
            attachments,
            attachment_references,
            owner,
            conn,
            mcp_timeout,
            active_turn_behavior,
            require_attachment,
            interrupt_on_steer_refusal,
            message_slot,
            preset_preamble,
        } = *request;
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        // The daemon does not trust the app's copy of these checks: the pipe
        // accepts frames from any client that can open it, so the limits are
        // enforced here too, on the payload as it arrived.
        if text.len() > MAX_WRITE_BYTES {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "Session input is too large.",
            ));
        }
        validate_attachments(attachments)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        // The references are the half of an attachment send that does not
        // travel in the frame, and the wire's rules for them are enforced here
        // with the inline ones — before the store is asked anything, the same
        // order `deposit` keeps: ownership and the wire's limits first, the
        // disk after. `resolve_attachment_references` reads the store, so a
        // reference the protocol refuses costs no digest lookup and no file
        // read, and it is refused before the ownership check below rather than
        // after it for the same reason `validate_attachments` is.
        validate_attachment_references(session_id, attachment_references)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        // A prompt is whatever it carries: text, inline attachments, or the
        // stored ones it refers to. A send with none of the three is not a
        // prompt and skips the readiness gates below, which is the behaviour
        // it had before references existed.
        let has_prompt =
            !text.is_empty() || !attachments.is_empty() || !attachment_references.is_empty();
        let (
            writer,
            image_sink,
            static_image_sink,
            runtime,
            killer,
            mut steerer,
            is_agent,
            mcp_required,
        ) = {
            let map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            let entry = peer_entry(&map, session_id, owner, &conn.conn_peer)?;
            let session = entry.as_peer_visible().ok_or_else(process_gone)?;
            (
                Arc::clone(&session.writer),
                session.image_sink.clone(),
                session.static_image_sink.clone(),
                Arc::clone(&session.runtime),
                session.killer.clone_killer(),
                session.steerer.clone_steerer(),
                session.metadata.kind.is_agent(),
                // S9: readiness waits only where the wait rule says so (never
                // pi/Codex — the S8 never-block default, twin-pinned). The wait
                // itself no-ops without `require_mcp`, so this flag is uniform
                // while the guarantee lives in the require gate.
                crate::mcp_broker::hosts_mcp(&session.metadata.kind),
            )
        };
        // A terminal's writer is a PTY, so an appended line is typed, not
        // read: nothing there can open a path. Writing the bytes would leave a
        // file behind for a session that can never consume it, and the pipe
        // accepts frames from any process that can open it, so the daemon does
        // not rely on the app never attaching to a terminal.
        //
        // A reference is refused by the same check for the same reason: what a
        // terminal would receive is the path line, and a path typed into a PTY
        // is input, not a file anything can open. The two halves are one
        // refusal here because neither reaches a terminal.
        if (!attachments.is_empty() || !attachment_references.is_empty()) && !is_agent {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "This session does not accept attachments.",
            ));
        }
        // A steer is text only, and that is refused before a single attachment
        // byte is planned, decoded or written anywhere (S4-10): the steer
        // branch below writes the text into a turn that is already running, and
        // there is no path from an attachment to a provider frame on it. The
        // refusal names the way to send one.
        //
        // A reference is text-only in the same sense and is refused by the
        // same check: the steer branch writes `text` and nothing else, so a
        // steer that named a stored attachment would drop it without a word —
        // the vanishing deck this whole path exists to prevent.
        if active_turn_behavior == Some(ActiveTurnBehavior::Steer)
            && (!attachments.is_empty() || !attachment_references.is_empty())
        {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "a steer carries text only; send attachments as a new message",
            ));
        }
        if require_attachment {
            check_attached(&runtime, conn, subscription_id)?;
        }
        let agent_runtime = is_agent.then(|| Arc::clone(&runtime));
        if let Some(runtime) = agent_runtime.as_ref() {
            if has_prompt && mcp_required {
                runtime.wait_for_mcp_ready(mcp_timeout)?;
            }
            if has_prompt && !runtime.can_publish_agent_user_message() {
                return Err(internal("Agent input could not be recorded."));
            }
        }
        if active_turn_behavior == Some(ActiveTurnBehavior::Steer) && is_agent {
            // (S4-14) The steer writes into whichever turn is running now, and the
            // admission registered this slot's boundary against the turn that was
            // running then. If that is not the same turn any more, the boundary is
            // re-keyed here — before the steer write, under the brakes lock — so the
            // slot ends with the turn the text actually enters.
            if let Some(slot) = message_slot {
                if message_slot_boundary_is_stale(slot, runtime.turn_counter()) {
                    rearm_message_slot_boundary(slot, &runtime);
                }
            }
            let expected_turn_id = runtime.turn_counter();
            // Compare-and-deliver (S4-02): the runtime hands the provider
            // adapter a token only while the daemon turn the caller checked is
            // still the running one, holding the same lock the `AgentFinished`
            // transition takes across the adapter's write. A turn therefore
            // cannot end — and the next one cannot start — between the check
            // and the write, so the text can only land in the turn it was
            // admitted for. `None` means the turn was over before admission:
            // there is nothing to steer and the text is delivered as the plain
            // send it would have been if the caller had not asked to join a
            // turn, with no interrupt, because nothing is running to replace.
            let steered = runtime.with_active_turn(expected_turn_id, |turn| {
                steerer.steer_active_turn(text, turn)
            });
            match steered {
                Some(Ok(true)) => {
                    // Cards are cancelled only now, once the provider has taken
                    // the input. Cancelling before this point would take a card
                    // away for a steer that never landed: `Ok(false)` (the
                    // provider cannot take a steer for this turn) and `Err` (the
                    // transport failed) both leave the cards exactly as they
                    // were, because the turn they belong to is still running.
                    // No provider needs them cleared *before* it can accept: on
                    // a refusal the local fallback's own `interrupt()` clears
                    // them, and each provider's killer does the same.
                    if let Some(permission_broker) = runtime.permission_broker() {
                        permission_broker.cancel_pending();
                    }
                    // The steered text is echoed into the session's own
                    // transcript as the `AgentUserMessage` every accepted input
                    // produces, so the sender's chat surface shows the steer
                    // inside the running turn; `Steered` stays the journal's
                    // audit row for the same text (it is not published to
                    // observers), carrying the echo's own `message_id` so the
                    // row and the transcript message name one message (A2-10).
                    // Both are best effort: the provider has already taken the
                    // text, so a recording failure is reported as a degraded
                    // session and never as an error — the caller must not be
                    // invited to retry a steer that already landed
                    // (S4-06/S4-09).
                    let echo_message_id = runtime.publish_agent_user_message(text.to_string());
                    if echo_message_id.is_none() {
                        runtime.mark_journal_degraded();
                    }
                    // `journal_steered` takes the id by value (the audit row
                    // and the transcript message name one message): the
                    // correlation key this delivery answers with is kept
                    // beside it rather than moved into the journal.
                    let delivered_message_id = echo_message_id.clone();
                    if !runtime.journal_steered(echo_message_id, text.to_string()) {
                        runtime.mark_journal_degraded();
                    }
                    if runtime.clear_attention() {
                        self.notify_session_transition(owner, session_id);
                    }
                    // The steer's own echo id is the message the text became,
                    // so it is what this delivery answers with (audit S5-04):
                    // a caller that correlates to it names the message the
                    // creator's transcript actually shows.
                    return Ok(delivered_message_id);
                }
                Some(Ok(false)) => {
                    // The provider cannot take a steer for this turn. The
                    // person at this machine gets the pre-existing
                    // interrupt-and-replace; a paired device gets a refusal,
                    // because interrupting a running turn is the act
                    // `SessionInterrupt` decides and no capability opens it to
                    // a peer, so a steer must not reach it the long way round
                    // (S4-01).
                    if !interrupt_on_steer_refusal {
                        return Err(WireError::new(
                            ErrorCode::Unauthorized,
                            "this agent cannot take a steer and interrupting is not permitted for a paired device",
                        ));
                    }
                    let mut killer = killer;
                    killer.interrupt();
                }
                Some(Err(error)) => return Err(error),
                // (S4-10) The turn ended between the admission and this write: the
                // text goes as an ordinary prompt. `boundary_reached` is already
                // set by the fired hook, and the re-key below — which looks at
                // exactly that flag — moves the slot onto the turn this prompt
                // starts.
                None => {}
            }
        }
        // ---- the session's first prompt carries the standing instructions ----
        //
        // The human's standing instructions ride the first prompt of every session
        // the daemon starts, and this is the one place a prompt is composed: a
        // session a human opens, a child an agent creates (which passes its preset
        // preamble in `preset_preamble`) and the Design host all reach this line,
        // and every provider's writer sits behind it (`session.rs:4899`-style
        // writes in `acp_client.rs`, `claude_client.rs`, `codex_client.rs`,
        // `pi_client.rs`). The order — standing instructions, then the preamble,
        // then the prompt — is fixed in `compose_first_prompt` and pinned by
        // `standing_instructions_come_before_the_preset_preamble`.
        //
        // Three deliberate narrowings:
        //
        // - **Agent sessions only.** A terminal's writer is a PTY: prefixing a
        //   human's first shell line with their standing instructions would type
        //   prose into a shell.
        // - **The first prompt that has text.** A prompt made only of attachments
        //   has nothing to prefix, so the flag stays owed and the session's first
        //   *text* prompt carries them.
        // - **The flag is taken, once.** `take_first_prompt` swaps it, so a second
        //   prompt racing the first cannot compose a second copy, and a prompt
        //   that arrives after a failed write does not get one either.
        //
        // The store is read *now*, at the moment of the first prompt, and never
        // cached on the session: an edit to the standing instructions takes effect
        // on the next session the daemon starts, not at the next restart.
        let first_prompt = (is_agent && !text.is_empty() && runtime.take_first_prompt())
            .then(|| compose_first_prompt(&self.standing_instructions(), preset_preamble, text));
        let text = first_prompt.as_deref().unwrap_or(text);
        // (S4-10, S4-14) The last thing before the write: the slot's boundary must
        // be the turn this text actually enters. The admission registered it
        // against the turn that was running then, and that turn can have ended —
        // and another can have started — while the delivery was on its way here.
        // Both cases look the same from the slot's side (`boundary_reached` set by
        // the old turn's hook, or a turn id that is not the admitted one), and both
        // are fixed the same way: re-key the boundary onto whichever turn is
        // running now, or onto the turn the prompt is about to start. Steering into
        // the turn that is running is still the right delivery; only the slot's
        // bookkeeping has to follow it.
        if let Some(slot) = message_slot {
            if message_slot_boundary_is_stale(slot, runtime.turn_counter()) {
                rearm_message_slot_boundary(slot, &runtime);
            }
        }
        // The user's text was checked against MAX_WRITE_BYTES above, before a
        // single line of ours is added, so the cap can never refuse a prompt
        // that was legal on arrival. The appended block is bounded by a fixed
        // number of absolute paths the daemon composed itself: at most
        // MAX_ATTACHMENT_COUNT of them for the inline attachments and at most
        // MAX_ATTACHMENT_REFERENCES for the stored references, both enforced by
        // the wire validation above. Re-checking the extended prompt could only
        // refuse a prompt the daemon lengthened; the write is not re-checked
        // against the cap.
        //
        // The stored references are resolved before any of the prompt text is
        // built, which is the rule `with_attachment_paths` states for the
        // inline attachments: a request that fails on its third item must leave
        // nothing half-built. Every reference is either resolved here or the
        // call returns, so the string built below is never a prompt missing one
        // of the files it named. The store read is the first disk work this
        // request does, and the wire validation above is what keeps a malformed
        // reference from reaching it.
        let reference_paths =
            resolve_attachment_references(&self.attachments, session_id, attachment_references)?;
        // The structured route: the sibling is present (an ACP session) AND
        // the live negotiated capability says images are supported. The plan
        // decides both halves — the blocks that travel and the exact string
        // the journal records — so they cannot drift apart. Otherwise —
        // sibling absent (terminals, and the three providers that take the
        // static route below), or the handshake said no or nothing — fall
        // through to exactly today's path-line write, byte for byte
        // unchanged.
        let mut plan = match image_sink.as_ref() {
            Some(sink) if sink.delivery() == ImageDelivery::NegotiatedImageBlock => {
                plan_structured_prompt(&self.attachments, session_id, text, attachments)?
            }
            _ => None,
        };
        // The static route: the sibling is present only for the three
        // providers the daemon statically knows carry images (Claude, Codex,
        // Pi). Its plan is built here, before the writer is locked, for the
        // same reason the ACP plan is: the decode and the strip walk must not
        // run under that hold. `None` means the route did not run (no
        // attachments, or a provider not authorised for inline bytes) and
        // nothing was materialized for it.
        let mut static_plan = match static_image_sink.as_ref() {
            Some(sink) => sink.plan_prompt(&self.attachments, session_id, text, attachments)?,
            None => None,
        };
        // The references join the text of whichever route planned this prompt,
        // as path lines, before anything reads that text — the frame the
        // provider receives and the string the journal records are one value in
        // both plans, so appending to it here is appending to both. A
        // reference never becomes an image block, on any route: see
        // [`push_reference_path_lines`] for why that is a decision.
        if let Some(plan) = plan.as_mut() {
            push_reference_path_lines(&mut plan.fallback_text, &reference_paths);
        }
        if let Some(plan) = static_plan.as_mut() {
            plan.append_reference_path_lines(&reference_paths);
        }
        // A session carries one route or the other, never both: `image_sink`
        // is the ACP one and `static_image_sink` the three static providers'.
        // `plan` is `Some` only when at least one raster became a block, so
        // an SVG-only prompt on a capable session takes this arm too: the
        // legacy write, materialized once, never twice — and on the static
        // route the same holds for a prompt whose every block became a path
        // line, because the plan answers with its own text either way.
        let prompt = match plan.as_ref() {
            Some(plan) => plan.fallback_text.clone(),
            None => match static_plan.as_ref() {
                // The plan decoded, sniffed and stripped every attachment
                // already and built the text from the paths it holds, so
                // reaching for `with_attachment_paths` here would do all of
                // that a second time for each of them.
                Some(plan) => plan.text().to_string(),
                // The only route that composes its text here rather than in a
                // plan, so the references are appended here — with the same
                // function and the same separator the two plans use, since a
                // prompt's shape must not depend on which route wrote it.
                None => {
                    let mut prompt =
                        with_attachment_paths(&self.attachments, session_id, text, attachments)?;
                    push_reference_path_lines(&mut prompt, &reference_paths);
                    prompt
                }
            },
        };
        // Lock the writer FIRST, as today: the journaled transcript event is
        // published under this same hold further down, so the journal keeps
        // the order the process sees. The structured send runs under this
        // hold too, locking the transport's pending table and child stdin —
        // the same order the pre-existing `AcpWriter::flush` path already
        // used when it issued its request from under this hold — so no new
        // lock ordering is introduced.
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
        if let Err(error) = match plan {
            // Structured route: the text block (with any SVG path lines) plus
            // the image blocks go as one `session/prompt` content array on
            // the sibling. The plain-text `writer` is not touched. The plan
            // travels whole, so the text block the child receives IS the
            // string journaled below — one value, two destinations.
            Some(plan) => {
                let sink = image_sink.as_ref().expect("plan implies a capable sibling");
                sink.send_structured_prompt(plan)
            }
            // The static route's frame goes out here, under the same hold and
            // for the same reason: the text on the wire and the text journaled
            // below come from the one plan.
            None => match static_plan {
                Some(plan) => plan.send(),
                // Today's path, unchanged: the prompt (with path lines) is
                // typed into the plain-text writer.
                None => writer.write_all(prompt.as_bytes()).map_err(|error| {
                    WireError::new(
                        ErrorCode::Io,
                        format!("Could not send input to the terminal: {error}"),
                    )
                }),
            },
        } {
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
        // The transcript id this delivery produced, when it produced one: the
        // `AgentUserMessage` an agent session echoes for accepted input. A
        // terminal has no transcript record and answers `None`.
        let mut delivered_message_id: Option<String> = None;
        if has_prompt {
            if let Some(runtime) = agent_runtime.as_ref() {
                // The journal records `prompt`: on the fallback path that is
                // the same string the writer got (the user's text plus one
                // path per attachment); on the structured path it is the
                // text block (the user's text plus any SVG path lines). The
                // base64 never leaves `PromptAttachment` either way — a
                // turn's row must not grow by hundreds of KiB, and the user's
                // images must not be copied into the history database.
                match runtime.publish_agent_user_message(prompt.clone()) {
                    Some(message_id) => delivered_message_id = Some(message_id),
                    None => return Err(internal("Agent input could not be recorded.")),
                }
                runtime.begin_turn();
                if runtime.clear_attention() {
                    self.notify_session_transition(owner, session_id);
                }
            }
        }
        drop(writer);
        Ok(delivered_message_id)
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
            let live = entry
                .as_peer_visible()
                .ok_or_else(|| not_found_while_configuring(entry))?;
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
            let entry = peer_entry(&map, session_id, owner, &conn.conn_peer)?;
            let session = entry.as_peer_visible().ok_or_else(process_gone)?;
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
        // A session inside its delivery window does not exist for its peers
        // (the re-audit's P2-1): the entry is skipped, and the row the
        // journal wrote before the spawn is skipped with it, so no roster
        // read can hand out an id a prompt would be lost on.
        let hidden: std::collections::HashSet<String> = map
            .values()
            .filter(|entry| entry.is_configuring())
            .map(|entry| entry.metadata().id.clone())
            .collect();
        let mut sessions: Vec<Session> = map
            .values()
            .filter(|entry| entry.owner().user == owner.user && !entry.is_configuring())
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
                    if hidden.contains(&row.id) {
                        continue;
                    }
                    sessions.push(row.to_session());
                }
            }
        }
        sessions.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(sessions)
    }

    /// The directory a created child starts in (`S5` checklist).
    ///
    /// `requested` is a *relative* path inside the creator's own workspace, or
    /// `None` for the workspace root. The answer is canonicalised and checked to
    /// be inside that root: an absolute path, a `..`, a symlink pointing out, or
    /// a path that does not exist is refused rather than handed to a provider.
    /// A caller with no workspace cannot ask for a subdirectory of one.
    pub(crate) fn resolve_child_cwd(
        &self,
        workspace_id: Option<&str>,
        requested: Option<&str>,
    ) -> Result<Option<PathBuf>, WireError> {
        let Some(workspace_id) = workspace_id else {
            if requested.is_some() {
                return Err(WireError::new(
                    ErrorCode::InvalidRequest,
                    "cwd needs a workspace; this session has none.",
                ));
            }
            return Ok(None);
        };
        let root = self.workspace_cwd(workspace_id)?;
        let Some(requested) = requested.filter(|value| !value.is_empty()) else {
            return Ok(None);
        };
        let refused = || {
            WireError::new(
                ErrorCode::InvalidRequest,
                "cwd must be a directory inside the workspace.",
            )
        };
        let relative = Path::new(requested);
        if relative.is_absolute() {
            return Err(refused());
        }
        let canonical_root = root.canonicalize().map_err(|_| refused())?;
        let canonical = canonical_root
            .join(relative)
            .canonicalize()
            .map_err(|_| refused())?;
        if !canonical.starts_with(&canonical_root) || !canonical.is_dir() {
            return Err(refused());
        }
        Ok(Some(canonical))
    }

    /// What a creation needs to know about the session that asked for it.
    ///
    /// Three facts, all read from the creator's own row and never from the
    /// request: its owner (the child's owner), its stored origin (the child's
    /// origin) and its workspace (the child's workspace). A session that is not
    /// this owner's is `session_not_found`, so a registration cannot be used to
    /// read a row it does not own.
    pub(crate) fn agent_creator(
        &self,
        session_id: &str,
        owner: &OwnerId,
    ) -> Result<AgentCreator, WireError> {
        let map = self
            .inner
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        let entry = map.get(session_id).ok_or_else(not_found)?;
        if entry.owner().user != owner.user {
            return Err(not_found());
        }
        let live = entry
            .as_peer_visible()
            .ok_or_else(|| not_found_while_configuring(entry))?;
        Ok(AgentCreator {
            owner: entry.owner().clone(),
            origin: live.metadata.origin.clone(),
            workspace_id: live.metadata.workspace_id.clone(),
            display_name: live.metadata.display_name.clone(),
            title: live.metadata.title.clone(),
            // The context this creator belongs to, which is what its child
            // inherits (`create-from-profile`): one context for a creator and
            // everything it commissions, at any depth. Read from the creator's
            // own metadata, with the fallback the field states for a session
            // that is its own context — a live session created before v11 has
            // no context column to have read.
            context_id: live
                .metadata
                .context_id
                .clone()
                .unwrap_or_else(|| live.metadata.id.clone()),
        })
    }

    /// Hold one creation's idempotency key for as long as the call that claimed
    /// it runs (audit S5-03).
    ///
    /// Taken *before* the idempotency store is read, so a second call with the
    /// same key — a client that re-sent while the first is still raising a card
    /// — is refused with `creation in progress; retry` and spends nothing. The
    /// guard hands the key back when it is dropped, so every refusal between
    /// here and the answer releases it without a cleanup line per path.
    pub(crate) fn hold_creation_key<'a>(
        &'a self,
        key: &str,
    ) -> Result<CreationKeyHold<'a>, WireError> {
        let mut table = self
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !table.begin_creation(key, Instant::now()) {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "creation in progress; retry",
            ));
        }
        Ok(CreationKeyHold {
            sessions: self,
            key: Some(key.to_string()),
        })
    }

    /// The creation this key was held for is over, either way: its result is in
    /// the idempotency store, or it failed and stored nothing.
    pub(crate) fn end_agent_creation(&self, key: &str) {
        let mut table = self
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        table.end_creation(key);
    }

    /// Take one creation slot for `creator`, or say why not (`S5` decision 5).
    ///
    /// The slot is taken *before* the card is raised and before anything is
    /// spawned, which is what makes the caps hold under two creations racing on
    /// one session: an admission that later fails releases it
    /// ([`Self::release_agent_creation`]) and one that succeeds commits it
    /// ([`Self::commit_agent_creation`]).
    ///
    /// `depth` is the child's depth — the creator's own, plus one — and comes
    /// from the caller's MCP registration, which the daemon wrote when that
    /// session was created. A caller-supplied depth is not accepted anywhere.
    pub(crate) fn reserve_agent_creation(
        &self,
        creator: &str,
        depth: u32,
    ) -> Result<AgentCreationTicket<'_>, WireError> {
        if depth > MAX_AGENT_DEPTH {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "depth limit; do not retry",
            ));
        }
        let now = Instant::now();
        let mut table = self
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if table.sweep_is_due(now) {
            table.sweep(now);
        }
        if table.live_agent_sessions() >= MAX_LIVE_AGENT_SESSIONS {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "creation limit exceeded; do not retry",
            ));
        }
        let reservation = table.next_reservation;
        table.next_reservation += 1;
        let caps = table
            .creators
            .entry(creator.to_string())
            .or_insert_with(|| AgentCreatorCaps::new(now));
        caps.roll_window(now);
        if caps.held() >= MAX_LIVE_CHILDREN_PER_CREATOR {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "creation limit exceeded; do not retry",
            ));
        }
        if caps.creations_in_window >= MAX_CREATIONS_PER_WINDOW {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "creation limit exceeded; do not retry",
            ));
        }
        // The once-per-session card, decided here rather than by the caller
        // (audit S5-06). A card that is already with the human blocks this
        // caller *before* it spends a slot: it is not a refusal the caller can
        // act on by retrying something else, it is "wait for the answer".
        let card_owed = match caps.gate {
            CreationGate::Pending => {
                return Err(WireError::new(
                    ErrorCode::InvalidRequest,
                    "creation permission pending; retry",
                ))
            }
            CreationGate::Closed => {
                caps.gate = CreationGate::Pending;
                true
            }
            CreationGate::Open => false,
        };
        // The child's id is reserved here rather than inside the spawn
        // (audit S5B-04): the link below names it, and the link has to exist
        // before the process does, because a provider that exits on the
        // instant would otherwise reach EOF with nothing to release.
        let caps = table
            .creators
            .get_mut(creator)
            .expect("the entry taken for this reservation");
        // The reservation carries no child id: the spawn composes that one, and
        // the link is registered at the commit under the id the child really
        // has. Registering it here instead was tried in this pass and closed
        // the child's own transport before its handshake (see the report).
        caps.in_flight.insert(reservation, String::new());
        caps.creations_in_window += 1;
        let child = String::new();
        Ok(AgentCreationTicket {
            registry: self,
            creator: creator.to_string(),
            reservation,
            child,
            card_owed,
            committed: false,
            caps: devboule_protocol::CreateAgentCaps {
                // The numbers the card states are what the budget reads
                // *including* the creation being asked about: the human is
                // deciding whether to spend this slot, so it is counted.
                live_children: caps.held() as u32,
                max_live_children: MAX_LIVE_CHILDREN_PER_CREATOR as u32,
                creations_this_hour: caps.creations_in_window,
                max_creations_per_hour: MAX_CREATIONS_PER_WINDOW,
                depth,
                max_depth: MAX_AGENT_DEPTH,
                live_agent_sessions: table.live_agent_sessions() as u32,
                max_live_agent_sessions: MAX_LIVE_AGENT_SESSIONS as u32,
            },
        })
    }

    /// The human allowed this creator to create: the once-per-creator-session
    /// gate opens and stays open for as long as the entry lives (`S5` decision
    /// 4, S5-06).
    pub(crate) fn accept_agent_creation(&self, creator: &str) {
        let mut table = self
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(caps) = table.creators.get_mut(creator) {
            caps.gate = CreationGate::Open;
        }
    }

    /// Release one reservation by identity, for a caller that holds the id
    /// rather than the ticket (the tests, and the rollback paths that need to
    /// know whether anything was still outstanding).
    ///
    /// The ticket's `Drop` is the normal way in; this answers `false` for an
    /// id that is not outstanding, which is what makes a double release a
    /// no-op (audit S5B-02).
    pub(crate) fn release_agent_creation(&self, creator: &str, reservation: u64) -> bool {
        let mut table = self
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(child) = table
            .creators
            .get_mut(creator)
            .and_then(|caps| caps.in_flight.remove(&reservation))
        else {
            return false;
        };
        // A marker belongs to the reservation that noted it and goes with it
        // (audit-3 S5D-01): the sweep is no longer a backstop for a marker, so
        // this release is the last of the four paths that end one.
        table
            .pending_children
            .retain(|_, (_, owned_by)| *owned_by != reservation);
        {
            let caps = table.creators.get_mut(creator).expect("the entry above");
            caps.creations_in_window = caps.creations_in_window.saturating_sub(1);
            if caps.gate == CreationGate::Pending {
                caps.gate = CreationGate::Closed;
            }
        }
        if table.children.get(&child).is_some_and(|link| !link.started) {
            table.children.remove(&child);
        }
        let drop_creator = table
            .creators
            .get(creator)
            .is_some_and(|caps| caps.creator_gone && caps.held() == 0);
        if drop_creator {
            table.creators.remove(creator);
        }
        true
    }

    /// A resumed child is a child again (audit S5B-05).
    ///
    /// With its creator live, the session re-enters the children table: the cap
    /// counts one child, not none, and a later end releases the slot and reports
    /// through the same routine as any other child. A creator that is gone, or
    /// an entry already marked gone, leaves the session ordinary — the roster
    /// still names the parent, and the caps ignore it.
    ///
    /// The *depth* is not persisted (the journal has no column for it, and
    /// adding one is a migration of its own): a resumed child comes back at
    /// depth 1. `notify` comes back `true` for the same reason — the report is
    /// what the link is for, and a resumed child that could end in silence
    /// would be the worse surprise.
    fn readmit_agent_child(&self, child: &str, creator: Option<&str>, owner: &OwnerId) {
        let Some(creator) = creator else {
            return;
        };
        if self.live_runtime(creator, owner).is_none() {
            return;
        }
        let mut table = self
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if table
            .creators
            .get(creator)
            .is_some_and(|caps| caps.creator_gone)
        {
            return;
        }
        // Resuming the same child again must not count it again (audit S5B-05):
        // the link is written once, and only a child that was *not* linked
        // spends a slot here.
        if table.children.contains_key(child) {
            return;
        }
        table.children.insert(
            child.to_string(),
            AgentChild {
                creator: creator.to_string(),
                notify: true,
                started: true,
                notice_owed: true,
                report_owed: true,
            },
        );
        let caps = table
            .creators
            .entry(creator.to_string())
            .or_insert_with(|| AgentCreatorCaps::new(Instant::now()));
        caps.live_children += 1;
    }

    /// Reserve with the owner a test does not care about, keeping the cap
    /// tests readable now that a reservation carries an identity.
    #[cfg(test)]
    fn test_ticket(&self, creator: &str, depth: u32) -> Result<AgentCreationTicket<'_>, WireError> {
        self.reserve_agent_creation(creator, depth)
    }

    /// Register a child the test named itself, the way the spawn path does for
    /// a real one. The newest reservation, if any, becomes that child.
    #[cfg(test)]
    fn commit_agent_child_for_test(&self, creator: &str, child: &str, notify: bool) {
        let mut table = self
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        // The reservation's own link (registered under the id the reservation
        // composed) goes: the child this call names takes its place, so the
        // budget sees one child either way and nothing is leaked.
        let reserved = table
            .creators
            .get_mut(creator)
            .and_then(|caps| caps.in_flight.keys().next_back().copied());
        if let Some(reserved) = reserved {
            if let Some(caps) = table.creators.get_mut(creator) {
                if let Some(composed) = caps.in_flight.remove(&reserved) {
                    table.children.remove(&composed);
                }
            }
            if let Some(caps) = table.creators.get_mut(creator) {
                caps.live_children += 1;
            }
        } else if let Some(caps) = table.creators.get_mut(creator) {
            caps.live_children += 1;
        }
        table.children.insert(
            child.to_string(),
            AgentChild {
                creator: creator.to_string(),
                notify,
                started: true,
                notice_owed: true,
                report_owed: true,
            },
        );
    }

    /// The tests reason in "this creator's newest reservation": release it.
    #[cfg(test)]
    fn abandon_agent_creation_for_test(&self, creator: &str) {
        let newest = {
            let table = self
                .creations
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            table
                .creators
                .get(creator)
                .and_then(|caps| caps.in_flight.keys().next_back().copied())
        };
        if let Some(reservation) = newest {
            assert!(
                self.release_agent_creation(creator, reservation),
                "the newest reservation of {creator}"
            );
        }
    }

    /// The creator session is gone: its own entry follows its last child out.
    ///
    /// Called from `close`, so a session that ends normally is forgotten here
    /// rather than by the sweep — the sweep is the backstop for entries whose
    /// creator vanished without one, and it is the reason the table cannot grow
    /// without bound.
    pub(crate) fn forget_agent_creator(&self, creator: &str) {
        let mut table = self
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(caps) = table.creators.get_mut(creator) {
            caps.creator_gone = true;
            if caps.held() == 0 {
                table.creators.remove(creator);
            }
        }
    }

    /// Create the child an agent asked for, from the preset's own answers.
    ///
    /// Reuses the `SessionCreate` path end to end — the same spawn, the same
    /// `register_with_provider`, the same journal row — with the four facts a
    /// client can never supply: the creator's origin, the creator's owner, this
    /// session's parent and depth, and the preset's tool overlay. The bounds
    /// (the title, the workspace, the prompt) were enforced by the broker
    /// before this is called; nothing here is asked of the caller again.
    pub(crate) fn create_session_for_agent(
        &self,
        state: &Arc<ServerState>,
        creation: AgentCreation,
        ticket: AgentCreationTicket<'_>,
    ) -> Result<Session, WireError> {
        let creator_owner = creation.creator.owner.clone();
        let mut meta = SessionCreateMeta::for_agent_child(
            &creation.creator_session_id,
            &creation.creator.origin,
            &creation.display_name,
            creation.depth,
            creation.overlay,
            creation.cwd.clone(),
        );
        // The marker the spawn notes carries this reservation (audit-3 S5D-01),
        // so the reservation's own release clears it as surely as the commit and
        // the abandon do: no path can leave a marker behind its creation.
        meta.reservation = Some(ticket.reservation());
        // The creation-from-profile facts, on the same meta the reservation
        // travels on: one place describes a child's birth. The mode and the
        // overlay already went through `for_agent_child` above; these are
        // what the profile added to the creation, and none of them is
        // re-derived later — the row keeps what the birth measured. The
        // marker itself is derived inside `create_with_provider_env` from the
        // delivery this creation carries, which is the same mode the child
        // will actually be started in.
        meta.profile_id = Some(creation.profile_id.clone());
        meta.labels = creation.labels.clone();
        meta.context_id = creation.context_id.clone();
        // The id the reservation already registered a link for (audit S5B-04):
        // the spawn must use it, so an exit on the instant finds the row that
        // releases the slot and reports the end.
        // `SessionCreateMeta::session_id` stays None: the spawn composes the
        // child's id (see `commit_agent_creation`).
        let kind = crate::provider_catalog::session_kind_for(&creation.provider);
        let child = self.create_with_provider_env(
            state,
            &creation.creator.owner,
            creation.workspace_id.clone(),
            kind,
            Some(creation.provider.clone()),
            creation.delivery.clone(),
            None,
            // The MCP connection is not a client connection: every ownership
            // check below uses the creator's own owner, and the origin was
            // passed explicitly rather than derived from this.
            &None,
            None,
            &meta,
        )?;
        // The ticket's `Drop` releases the reservation; the journal row a failed
        // spawn leaves behind is ended by `create_with_provider_env` itself.
        let (committed, deferred) = self.commit_agent_creation(
            &creation.creator_session_id,
            ticket.reservation(),
            &child.id,
            creation.notify,
        );
        if !committed {
            // The creator closed while its child was starting (audit S5B-09):
            // a session nobody owns is not a creation that succeeded, so the
            // child is closed again and the caller is told why. The ticket's
            // `Drop` gives the reservation back.
            self.abandon_uncommitted_child(&child.id, &creator_owner);
            return Err(WireError::new(ErrorCode::InvalidRequest, "creator closed"));
        }
        // The reservation is a child now: nothing is given back on this path.
        ticket.commit();
        // The creation is recorded on the *creator's* transcript, beside the
        // children the roster names: the child's own transcript begins with its
        // prompt and must not explain where it came from.
        //
        // Before the prompt, not after (audit S5-10): a child that answers and
        // ends inside the write would otherwise reach the creator as a finish
        // with no creation in front of it. Publishing first also means the
        // creator has a name for the child before anything can end it, so a
        // prompt that fails to write closes a session the human can see.
        // The handle is the one the card was raised through, taken before the
        // spawn (audit-2 §1): looking the creator up again afterwards can miss
        // it (a closed or replaced entry) and the creation record would vanish
        // with it, taking S5-10's ordering guarantee with it.
        let creator_runtime = creation
            .creator_runtime
            .clone()
            .or_else(|| self.live_runtime(&creation.creator_session_id, &creator_owner));
        self.publish_child_created_then_end(
            creator_runtime.as_ref(),
            &child.id,
            &creation.display_name,
            &creation.provider,
            &creation.profile_name,
            deferred,
        );
        // The child's first prompt. The preset preamble is no longer glued here:
        // it travels as `preset_preamble` and is composed by the send path, in one
        // place with the human's standing instructions in front of it
        // (`compose_first_prompt`), so every provider receives one string built by
        // one rule.
        let prompt = creation.initial_prompt.clone();
        let owner = creation.creator.owner.clone();
        let internal_conn = ConnHandle::with_peer(0, None);
        let sent = self.send_with_subscription_timeout(&SendRequest {
            session_id: &child.id,
            subscription_id: 0,
            text: &prompt,
            attachments: &[],
            // Empty by construction: the standing instructions, the preamble and
            // the caller's text are the whole prompt, and `devboule_create_agent`
            // has no parameter that names a stored attachment.
            attachment_references: &[],
            owner: &owner,
            conn: &internal_conn,
            mcp_timeout: crate::mcp_broker::ready_timeout(),
            active_turn_behavior: None,
            require_attachment: false,
            interrupt_on_steer_refusal: true,
            message_slot: None,
            preset_preamble: Some(crate::provider_catalog::AGENT_PREAMBLE),
        });
        if let Err(error) = sent {
            let _ = self.close(&child.id, &owner, &None);
            return Err(error);
        }
        Ok(child)
    }

    /// The child exists: its reservation becomes a live child (audit S5B-02),
    /// or the commit is refused because the creator is gone (audit S5B-09).
    ///
    /// `false` means the creator closed while its child was starting. The
    /// caller then closes the child again: a session nobody owns is not a
    /// creation that succeeded, and the caller's `Drop` releases the
    /// reservation.
    fn commit_agent_creation(
        &self,
        creator: &str,
        reservation: u64,
        child: &str,
        notify: bool,
    ) -> (bool, Option<DeferredChildEnd>) {
        let mut table = self
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(caps) = table.creators.get_mut(creator) else {
            return (false, None);
        };
        if caps.creator_gone {
            return (false, None);
        }
        let Some(reserved) = caps.in_flight.remove(&reservation) else {
            return (false, None);
        };
        // The reservation was registered without a child id (the spawn composes
        // that one): the link this child gets is registered here, and it is the
        // link every later end path finds.
        debug_assert!(reserved.is_empty(), "a reservation never names a child");
        caps.live_children += 1;
        // An end that arrived before this link existed runs now, on this
        // thread, exactly as if the order had been the other way round.
        table.pending_children.remove(child);
        let deferred = table.deferred_child_ends.remove(child).map(|(end, _)| end);
        table.children.insert(
            child.to_string(),
            AgentChild {
                creator: creator.to_string(),
                notify,
                started: true,
                notice_owed: true,
                report_owed: true,
            },
        );
        (true, deferred)
    }

    /// A child has ended, whatever ended it (`S5` decisions 7 and 8; audit
    /// S5-01).
    ///
    /// One routine, called from every path that can take a child out of the
    /// live map, so the caps row is released and the finish report is produced
    /// on the *first* of them and on no later one:
    ///
    /// * `close` — the explicit close, with the row it just removed;
    /// * [`Self::finish_reader_session`] — the reader's EOF, which is also how a
    ///   process exit is observed, with the row it just removed;
    /// * `resume` — the live entry a resume replaces;
    ///
    /// and the runtime's transition notify ([`Self::report_child_events`])
    /// *reports* without releasing, because the session is still live there.
    ///
    /// It takes what the caller still has in hand rather than looking the row up
    /// again: the paths that ended the child have already removed it.
    /// Note that this child's creation has not committed yet (audit-2 §2).
    /// The spawn noted the child it is starting: until the commit (or the
    /// abandon) an end that beats it is parked instead of lost. The marker
    /// carries the reservation that owns it, which is its whole lifetime
    /// (audit-3 S5D-01) — the sweep does not age it.
    pub(crate) fn note_pending_child(&self, child: &str, reservation: u64) {
        let mut table = self
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        table
            .pending_children
            .insert(child.to_string(), (Instant::now(), reservation));
    }

    /// The creation never got as far as a link: nothing is owed anywhere.
    /// A creation that did not commit because its creator is gone (audit-3
    /// §2): the child is closed **and** nothing its start recorded outlives it.
    ///
    /// The clear comes first and under its own lock: the close can end the child,
    /// and an end that found the marker still set would park a second deferred
    /// entry that no commit is left to consume.
    fn abandon_uncommitted_child(&self, child: &str, owner: &OwnerId) {
        self.clear_pending_child(child);
        let _ = self.close(child, owner, &None);
    }

    pub(crate) fn clear_pending_child(&self, child: &str) {
        let mut table = self
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        table.pending_children.remove(child);
        table.deferred_child_ends.remove(child);
    }

    /// Park an end that arrived first, **and decide that under the same lock**
    /// (audit-3 §1).
    ///
    /// `true` means the end is parked and the commit will run it. `false` means
    /// this child's creation has already committed (or its marker was taken
    /// away), so the caller runs the routine itself — the check and the insert
    /// are one critical section, which is what keeps a commit from landing
    /// between them and leaving a parked end with no consumer.
    fn defer_child_end_if_pending(
        &self,
        child: &str,
        session: &Session,
        runtime: &Arc<SessionRuntime>,
        owner: &OwnerId,
    ) -> bool {
        let mut table = self
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !table.pending_children.contains_key(child) {
            return false;
        }
        table.deferred_child_ends.insert(
            child.to_string(),
            (
                (
                    Some(session.clone()),
                    Some(Arc::clone(runtime)),
                    Some(owner.clone()),
                ),
                Instant::now(),
            ),
        );
        true
    }

    /// Record the new child on its creator, **then** pay off an end that arrived
    /// before the creation committed (audit-3 §3).
    ///
    /// The order is the point: a parked end deposits artifacts, steers the
    /// creator and publishes `ChildFinished`, and the creator has to be able to
    /// name the child before any of that happens.
    fn publish_child_created_then_end(
        &self,
        creator_runtime: Option<&Arc<SessionRuntime>>,
        child: &str,
        display_name: &str,
        provider: &str,
        preset: &str,
        deferred: Option<DeferredChildEnd>,
    ) {
        if let Some(runtime) = creator_runtime {
            if !runtime.publish_child_created(child, display_name, provider, preset) {
                runtime.mark_journal_degraded();
            }
            // The record is in the creator's journal through the runtime above;
            // a runtime that is gone by now cannot be written to, and that is
            // stated rather than hidden.
        } else {
            // The creator's runtime is gone (its session closed while the child
            // was starting) and a journal record cannot be written without the
            // runtime's stream state — generation and sequence are its own. The
            // loss is stated rather than silent, and the S5B-09 commit check is
            // what keeps this path from being reachable by a creation that
            // should have been refused.
            eprintln!(
                "agent creation {child}: the creator's runtime is gone, so the creation record was not published"
            );
        }
        if let Some((session, runtime, owner)) = deferred {
            // The end that arrived before the link existed: report and release
            // it now, on this creation's thread (audit-2 §2).
            self.child_ended_with(child, session.as_ref(), runtime.as_deref(), owner.as_ref());
        }
    }

    pub(crate) fn child_ended_with(
        &self,
        child: &str,
        session: Option<&Session>,
        runtime: Option<&SessionRuntime>,
        owner: Option<&OwnerId>,
    ) {
        if let (Some(session), Some(runtime), Some(owner)) = (session, runtime, owner) {
            self.report_child_finish_with(child, session, runtime, owner);
        }
        self.release_agent_child(child);
    }

    /// The child is gone: give its slot back to its creator, and let a creator
    /// entry that is waiting for it go with it.
    pub(crate) fn release_agent_child(&self, child: &str) {
        let mut table = self
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let Some(link) = table.children.remove(child) else {
            return;
        };
        let creator = link.creator;
        let started = link.started;
        if let Some(caps) = table.creators.get_mut(&creator) {
            if started {
                caps.live_children = caps.live_children.saturating_sub(1);
            } else if let Some((reservation, _)) = caps
                .in_flight
                .iter()
                .find(|(_, reserved)| reserved.as_str() == child)
                .map(|(id, reserved)| (*id, reserved.clone()))
            {
                // A child that ended before its spawn returned was never a
                // child: the reservation it still holds goes with it (audit
                // S5B-04), so an immediate exit leaks neither the slot nor the
                // window's count.
                caps.in_flight.remove(&reservation);
                caps.creations_in_window = caps.creations_in_window.saturating_sub(1);
                if caps.gate == CreationGate::Pending {
                    // The same rule as a released reservation (`S5B-02`): the
                    // question left with the child, so the next creation asks
                    // it again instead of being refused forever.
                    caps.gate = CreationGate::Closed;
                }
            }
        }
        let drop_creator = table
            .creators
            .get(&creator)
            .is_some_and(|caps| caps.creator_gone && caps.held() == 0);
        if drop_creator {
            table.creators.remove(&creator);
        }
    }

    /// The moment one child's `input_required` notice is owed, and the creator
    /// it is owed to. Answers `Some` exactly once per child.
    fn claim_child_notice(&self, child: &str) -> Option<String> {
        let mut table = self
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let link = table.children.get_mut(child)?;
        if !link.notice_owed || !link.notify {
            return None;
        }
        link.notice_owed = false;
        Some(link.creator.clone())
    }

    /// The same, for the finish report: `Some(creator, notify)` once per child,
    /// whatever path observes the end first.
    fn claim_child_report(&self, child: &str) -> Option<(String, bool)> {
        let mut table = self
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let link = table.children.get_mut(child)?;
        if !link.report_owed {
            return None;
        }
        link.report_owed = false;
        Some((link.creator.clone(), link.notify))
    }

    /// A live session's runtime, by id, when the session is this owner's.
    ///
    /// The owner check is the same one every registry read performs: a caller
    /// that learned an id may still not be its owner.
    pub(crate) fn live_runtime(
        &self,
        session_id: &str,
        owner: &OwnerId,
    ) -> Option<Arc<SessionRuntime>> {
        self.inner
            .lock()
            .ok()?
            .get(session_id)
            .filter(|entry| entry.owner().user == owner.user)
            .and_then(|entry| entry.as_peer_visible())
            .map(|live| Arc::clone(&live.runtime))
    }

    /// Raise the creation card on `creator` and wait for the person's answer
    /// (`S5` decision 4).
    ///
    /// False covers every way the answer was not an allow: a deny, the
    /// broker's timeout, a creator that is no longer running, a session with
    /// no broker to hold the card, and a card the broker refused because the
    /// device already had three pending. The caller refuses the creation in all
    /// of them, and the gate stays shut.
    pub(crate) fn ask_creation_card(
        &self,
        creator: &str,
        owner: &OwnerId,
        card: SessionEvent,
    ) -> bool {
        let Some(runtime) = self.live_runtime(creator, owner) else {
            return false;
        };
        let Some(broker) = runtime.permission_broker() else {
            return false;
        };
        broker.request_host_permission(card, &runtime) == permission_broker::HostDecision::Allow
    }

    /// A child's row and runtime, for the finish report.
    fn child_view(&self, child: &str) -> Option<(Session, Arc<SessionRuntime>, OwnerId)> {
        let map = self.inner.lock().ok()?;
        let entry = map.get(child)?;
        let live = entry.as_peer_visible()?;
        Some((
            live_session_view(live),
            Arc::clone(&live.runtime),
            entry.owner().clone(),
        ))
    }

    /// Everything a child's transition owes its creator (`S5` §3): the
    /// `input_required` notice on its first parked card, and the finish report.
    ///
    /// Called on every transition the runtime notifies and on `close`, so it
    /// must be cheap when nothing is owed — both claims are one lock and one
    /// hash lookup, and they answer `None` for a session that is not an
    /// agent-created child at all, which is every session in a daemon nobody
    /// has commissioned from.
    pub(crate) fn report_child_events(&self, child: &str) {
        self.notify_child_input_required(child);
        self.report_child_finish(child);
    }

    /// One notice per child, on the first permission card it parks on.
    fn notify_child_input_required(&self, child: &str) {
        let Some((session, runtime, owner)) = self.child_view(child) else {
            return;
        };
        let parked = runtime
            .permission_broker()
            .is_some_and(|broker| broker.pending_len() > 0);
        if !parked {
            return;
        }
        let Some(creator) = self.claim_child_notice(child) else {
            return;
        };
        // A creator that is gone gets nothing: the child's card stays visible
        // on the child's own session, which is where a human answers it.
        let display_name = session
            .display_name
            .clone()
            .unwrap_or_else(|| session.title.clone());
        let envelope = agent_input_required_envelope(&session.id, &display_name, &session.origin);
        // The notice has no event beside it, so its delivery id is not needed:
        // a parked child is visible on its own session, and the text is the
        // whole message.
        let _ = self.deliver_to_creator(&creator, &owner, &envelope);
    }

    /// The finish report: the deposit, the text message and the structured
    /// event, in that order (`S5` decisions 7 and 10).
    ///
    /// This is the *transition* caller, and a transition is not a finish: a
    /// child that parks on a card, or whose provider emits one malformed line,
    /// raises attention while it is still working. Reporting there would send
    /// the creator a `canceled` finish for a live child and spend the one
    /// report it is ever owed (`S5-01`), which the slice-5 e2e battery caught:
    /// the creator got "no message to deposit" half a second into a creation
    /// whose turn had not started. So the transition reports only for a child
    /// that has finished a turn ([`SessionRuntime::agent_stop_reason`], recorded
    /// before the attention raise in the same publish) or that is no longer
    /// live. Every path that *ends* a child reports through
    /// [`Self::child_ended_with`], unconditionally, because there the child is
    /// gone whatever its last turn said.
    fn report_child_finish(&self, child: &str) {
        let Some((session, runtime, owner)) = self.child_view(child) else {
            return;
        };
        let ended = !matches!(
            session.state,
            SessionState::Live { .. } | SessionState::Silent { .. }
        );
        if !ended && runtime.agent_stop_reason().is_none() {
            return;
        }
        self.report_child_finish_with(child, &session, &runtime, &owner);
    }

    /// The same, with the child's row in hand.
    ///
    /// `close` takes the row out of the map before the runtime is torn down, so
    /// the caller that has the row passes it in rather than looking it up again.
    fn report_child_finish_with(
        &self,
        child: &str,
        session: &Session,
        runtime: &SessionRuntime,
        owner: &OwnerId,
    ) {
        let Some((creator, notify)) = self.claim_child_report(child) else {
            return;
        };
        // A caller that asked not to be told still gets its child's end
        // recorded on the child's own journal (the provider wrote it there);
        // what it asked to skip is this report.
        if !notify {
            return;
        }
        // The creator is gone: nothing is deposited and nothing is sent. The
        // child's journal still has its end; the creator's is closed.
        let Some(creator_runtime) = self.live_runtime(&creator, owner) else {
            return;
        };
        let (state, note) = child_finish_state(session, runtime);
        let snapshot = runtime.agent_message_snapshot();
        let (artifacts, note) = match snapshot.as_ref() {
            Some(snapshot) if !snapshot.text.is_empty() => {
                match self.deposit_child_message(&creator, owner, snapshot) {
                    Ok(artifact) => (vec![artifact], note),
                    // A deposit that fails never fails the report: the human
                    // still learns the child finished, and the note says the
                    // artifact is not there.
                    Err(reason) => (
                        Vec::new(),
                        Some(match note {
                            Some(note) => format!("{note} {reason}"),
                            None => reason,
                        }),
                    ),
                }
            }
            _ => (
                Vec::new(),
                Some(match note {
                    Some(note) => note,
                    None => "The child produced no message to deposit.".to_string(),
                }),
            ),
        };
        let display_name = session
            .display_name
            .clone()
            .unwrap_or_else(|| session.title.clone());
        let summary = summary_of(snapshot.as_ref().map(|snapshot| snapshot.text.as_str()));
        let envelope = bound_finish_envelope(agent_finished_envelope(
            &session.id,
            &display_name,
            state,
            &summary,
            &artifacts,
            note.as_deref(),
            &session.origin,
        ));
        // The delivery answers the id of the message it left on the creator's
        // transcript, and that id is what the event carries (audit S5-04). A
        // creator that is live but did not take the text is told so in the
        // note and gets no id at all: naming whatever message happened to be
        // last would point the app at somebody else's turn.
        let (message_id, note) = match self.deliver_to_creator(&creator, owner, &envelope) {
            Ok(Some(message_id)) => (Some(message_id), note),
            // Delivered, and the creator's provider kept no transcript record
            // for it: there is nothing to correlate to, and the text is there.
            Ok(None) => (None, note),
            Err(_) => (
                None,
                Some(match note {
                    Some(note) => format!("{note} finish report not delivered"),
                    None => "finish report not delivered".to_string(),
                }),
            ),
        };
        if !creator_runtime.publish_child_finished(
            message_id,
            &session.id,
            &display_name,
            state,
            note,
            artifacts,
        ) {
            creator_runtime.mark_journal_degraded();
        }
    }

    /// Hand one daemon-originated line to the creator through the slice-4
    /// steer-or-prompt path, **without** a sender brake slot.
    ///
    /// The exemption is deliberate and narrow: the brakes bound what an *agent*
    /// may spend on its peers, and this is the daemon's own report, raised by a
    /// child's end rather than by a caller. It is one delivery per finish.
    ///
    /// A peer's creator is never steered: the steer's refusal fallback is an
    /// interrupt, and interrupting a turn is `SessionInterrupt`'s act, which no
    /// capability opens to a peer (S4-01). A peer's report is a plain prompt.
    ///
    /// The answer is the transcript id the delivered text got (`S5-04`): the
    /// caller correlates an event to the message that is actually there, rather
    /// than reading a "last message" that may belong to somebody else. `None`
    /// means the text was delivered but left no transcript record.
    fn deliver_to_creator(
        &self,
        creator: &str,
        owner: &OwnerId,
        text: &str,
    ) -> Result<Option<String>, WireError> {
        let local = self.creator_is_local(creator);
        // A refused steer must not take the report with it (audit S5B-06): the
        // steer is the preferred shape (it lands in the creator's turn instead
        // of queueing behind it), and when it is refused the same envelope goes
        // out once as a plain prompt. Only if that fails too does the caller
        // see an Err.
        let steer = self.send_to_creator(creator, owner, text, true);
        if steer.is_ok() || !local {
            return steer;
        }
        self.send_to_creator(creator, owner, text, false)
    }

    fn send_to_creator(
        &self,
        creator: &str,
        owner: &OwnerId,
        text: &str,
        steer: bool,
    ) -> Result<Option<String>, WireError> {
        let internal_conn = ConnHandle::with_peer(0, None);
        self.send_with_subscription_timeout(&SendRequest {
            session_id: creator,
            subscription_id: 0,
            text,
            attachments: &[],
            // The daemon's own report carries no attachment: an agent message
            // may name a stored file, a `<devboule-system>` line may not.
            attachment_references: &[],
            owner,
            conn: &internal_conn,
            mcp_timeout: crate::mcp_broker::ready_timeout(),
            active_turn_behavior: steer.then_some(ActiveTurnBehavior::Steer),
            require_attachment: false,
            interrupt_on_steer_refusal: steer,
            message_slot: None,
            // No preset preamble: the daemon's own report is not a creation's
            // prompt, and a child that was created already had its first one.
            preset_preamble: None,
        })
    }

    /// Whether the stored row for `session_id` says the person at this machine
    /// asked for it. Read from the row, never from a connection.
    fn creator_is_local(&self, session_id: &str) -> bool {
        self.inner
            .lock()
            .ok()
            .and_then(|map| map.get(session_id).map(|entry| entry.to_session()))
            .is_some_and(|session| session.origin.is_local())
    }

    pub(crate) fn live_agent_entries(
        &self,
        owner: &OwnerId,
    ) -> Result<Vec<LiveAgentEntry>, WireError> {
        let map = self
            .inner
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        let mut sessions = map
            .values()
            .filter_map(|entry| {
                let live = entry.as_peer_visible()?;
                if live.owner.user != owner.user
                    || !crate::mcp_broker::hosts_mcp(&live.metadata.kind)
                {
                    return None;
                }
                let session = live_session_view(live);
                matches!(
                    session.state,
                    SessionState::Live { .. } | SessionState::Silent { .. }
                )
                .then(|| LiveAgentEntry {
                    session,
                    runtime: Arc::clone(&live.runtime),
                })
            })
            .collect::<Vec<_>>();
        sessions.sort_by(|left, right| left.session.id.cmp(&right.session.id));
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
                        let session = entry.as_peer_visible()?;
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

    /// What the peer gate needs to refuse a session that runs without asking
    /// the user's permission (§8b A4/A5): the session's provider kind and the
    /// mode it is in now, when it has advertised one. `None` for a session
    /// this daemon does not know.
    pub(crate) fn session_mode_guard(
        &self,
        session_id: &str,
    ) -> Option<(SessionKind, Option<String>)> {
        let map = self.inner.lock().ok()?;
        let entry = map.get(session_id)?;
        // A session inside its delivery window does not exist for the peer
        // gate either (the re-audit's P2-2): `None` is this function's
        // "the daemon does not know this session", and the caller refuses
        // on that.
        if entry.is_configuring() {
            return None;
        }
        let kind = entry.metadata().kind.clone();
        Some((kind, entry.runtime().current_mode_id()))
    }

    /// The stored origin of the session behind `session_id`, for the MCP tool
    /// door (`mcp_broker.rs`). Read from the registry row, never from the
    /// loopback connection the broker holds: that socket is this machine's own
    /// by construction, so reading it would label a peer's child as local.
    ///
    /// `None` is every way there is no readable row — absent, or a poisoned
    /// lock — and the door refuses it with the pre-existing retryable absence
    /// sentence, never as the local person. Absence is transient by construction:
    /// an agent's first call can land before its own commit (the stub documents
    /// the race and retries exactly that sentence), and a reaped session's
    /// in-flight calls outlive its row; in both cases the row a retry finds a
    /// moment later is judged normally. A stored `Unknown` origin reads back as
    /// itself (`Some`), and is refused hard at the door: unlike absence it never
    /// resolves.
    ///
    /// This deliberately reads through the delivery window (`Configuring`): the
    /// window hides a session from its *targets* (`peer_entry` refuses it, so no
    /// peer path can act on a half-born session), but the caller's own origin
    /// was written at the create before the journal row and is already a fact.
    /// Refusing a configuring caller would turn a transient local birth into a
    /// refusal for the session's own first tool calls.
    pub(crate) fn caller_origin(&self, session_id: &str) -> Option<SessionOrigin> {
        let map = self.inner.lock().ok()?;
        let entry = map.get(session_id)?;
        Some(entry.metadata().origin.clone())
    }

    /// Whether `conn_peer` may reach `session_id` at all — asked *before* any
    /// question about what that session is (`session_mode_guard`, H6).
    ///
    /// The two failures are one answer on purpose. A session owned by someone
    /// else and a session this daemon does not know both give `unauthorized()`
    /// here, so a peer that probes another device's session ids learns nothing
    /// from comparing the replies: without this, `SessionSetMode` on a
    /// reachable-looking id answered "that session exists and runs this
    /// provider" through the mode policy gate, before the ownership check ever
    /// ran (§8b A1/A3).
    ///
    /// It is deliberately not a substitute for the checks the session methods
    /// make: this is the *ordering* the gate needs, and every operation still
    /// authorizes itself again at the point it touches the session.
    pub(crate) fn session_scope(
        &self,
        session_id: &str,
        owner: &OwnerId,
        conn_peer: &Option<ConnPeer>,
    ) -> Result<(), WireError> {
        let map = self
            .inner
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        match map.get(session_id) {
            Some(entry) => {
                check_user_owner(entry, owner, conn_peer)?;
                // The ordering gate must not admit a session that is still
                // inside its delivery window (the re-audit's P2-2): the
                // honest answer is the one the operation behind this gate
                // would give — `SessionNotFound` — while the unknown-id
                // refusal above stays `unauthorized`, so a probe still
                // learns nothing from comparing replies.
                if entry.is_configuring() {
                    return Err(not_found());
                }
                Ok(())
            }
            None => Err(unauthorized()),
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
        conn: &ConnHandle,
    ) -> Result<Arc<SessionRuntime>, WireError> {
        validate_session_id(session_id)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        let map = self
            .inner
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        // The peer door: attach, resize, detach and permission responses
        // reach a `Configuring` session through here, and the door refuses
        // the delivery window (the re-audit's P2-2). A transcript entry is
        // addressable — it is a roster member — so the door lets it through
        // and the runtime below serves it.
        let entry = peer_entry(&map, session_id, owner, &conn.conn_peer)?;
        Ok(entry.runtime())
    }
}

fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

/// The finish report's envelope (`S5` decision 7).
///
/// The same `<devboule-system>` frame an agent message arrives in — one builder,
/// one escaping rule — with a `kind` line and the structured body. Every value
/// that came from the child goes through [`neutralise_envelope_text`], so the
/// child's own words cannot close the envelope or open a second one, and CR/LF
/// is normalised on the way in: a summary with a lone carriage return cannot
/// forge a line the envelope did not write.
fn agent_finished_envelope(
    child_session_id: &str,
    display_name: &str,
    state: AgentTaskState,
    summary: &str,
    artifacts: &[FinishArtifact],
    note: Option<&str>,
    child_origin: &SessionOrigin,
) -> String {
    let clean = |value: &str| neutralise_envelope_text(value);
    let mut body = format!(
        "childSessionId: {}\ndisplayName: {}\nstate: {}\nsummary: {}",
        clean(child_session_id),
        clean(display_name),
        state.as_str(),
        clean(summary)
    );
    if let Some(note) = note {
        body.push_str(&format!("\nnote: {}", clean(note)));
    }
    let artifacts = serde_json::to_string(artifacts).unwrap_or_else(|_| "[]".to_string());
    body.push_str(&format!("\nartifacts: {}", clean(&artifacts)));
    format!(
        "<devboule-system>\norigin: {}\nrole: daemon\nfrom_agent: {}\nkind: agent_finished\ntimestamp: {}\n{}\n</devboule-system>",
        origin_line(child_origin),
        clean(child_session_id),
        unix_millis(),
        body
    )
}

/// The `input_required` notice (`S5` §3): one line saying the child is parked on
/// a card, and deliberately not what the card says.
fn agent_input_required_envelope(
    child_session_id: &str,
    display_name: &str,
    child_origin: &SessionOrigin,
) -> String {
    format!(
        "<devboule-system>\norigin: {}\nrole: daemon\nfrom_agent: {}\nkind: agent_input_required\ntimestamp: {}\nchildSessionId: {}\ndisplayName: {}\nstate: input_required\nsummary: This agent is waiting for a person to answer a permission card.\n</devboule-system>",
        origin_line(child_origin),
        neutralise_envelope_text(child_session_id),
        unix_millis(),
        neutralise_envelope_text(child_session_id),
        neutralise_envelope_text(display_name)
    )
}

/// The delegated-surfacing envelope (§4.3, §6.7 of the app contract): the
/// daemon's facts in the header — `cardId`, `toolTitle`, `displayName`, one
/// line each, exactly those keys — and the child's own words fenced between
/// the exact lines `child-said:` and `end child-said`. The fence markers are
/// neutralised inside the excerpt the same way the envelope tags are, so a
/// child that writes a closer into its own words cannot close its quoted
/// block early: the human must see at least as much of the card as the model
/// does.
///
/// **This quoting is a mitigation, not a fix.** The excerpt is text the child
/// chose, entering the creator's prompt; a confused or hostile child can
/// still try to steer its creator in those words. The fence and the system
/// styling exist so the creator's model — and the human reading over its
/// shoulder — can tell whose words they are, and nothing more.
///
/// Every header value is single-line by construction of this builder's
/// inputs (the card id is daemon-minted; the title and name are sanitised
/// below), because a header value carrying a newline would grow the frame a
/// second quoted block — the malformed frame the app refuses rather than
/// half-parse.
fn agent_permission_request_envelope(
    child_session_id: &str,
    child_origin: &SessionOrigin,
    card_id: &str,
    tool_title: &str,
    display_name: &str,
    excerpt: &str,
) -> String {
    // One header line per field: a newline in the child-chosen values would
    // impersonate frame structure, so it becomes a space before anything else
    // runs. The cap on the excerpt is the scalar cap below.
    let single_line = |text: &str| -> String {
        let normalised = text.replace("\r\n", " ").replace(['\r', '\n'], " ");
        normalised.chars().take(TITLE_LINE_MAX_CHARS).collect()
    };
    format!(
        "<devboule-system>\norigin: {}\nrole: daemon\nfrom_agent: {}\nkind: agent_permission_request\ntimestamp: {}\ncardId: {}\ntoolTitle: {}\ndisplayName: {}\nchild-said:\n{}\nend child-said\n</devboule-system>",
        origin_line(child_origin),
        neutralise_envelope_text(child_session_id),
        unix_millis(),
        neutralise_envelope_text(&single_line(card_id)),
        neutralise_envelope_text(&single_line(tool_title)),
        neutralise_envelope_text(&single_line(display_name)),
        neutralise_envelope_text(&cap_excerpt_scalars(excerpt)),
    )
}

/// The most characters one child-chosen header line may carry, after
/// newlines became spaces. A card title is provider text of unbounded shape;
/// this bounds the frame, not the card.
const TITLE_LINE_MAX_CHARS: usize = 256;

/// The excerpt cap (§4.3): 512 Unicode **scalar values**, counted on the raw
/// text after CR/LF normalisation and before any escaping, cut at a scalar
/// boundary — never inside one. The escaped wire form may exceed 512 units;
/// the app never re-truncates, so this is the only cut the excerpt gets.
fn cap_excerpt_scalars(text: &str) -> String {
    let normalised = text.replace("\r\n", "\n").replace('\r', "\n");
    normalised.chars().take(EXCERPT_MAX_SCALARS).collect()
}

const EXCERPT_MAX_SCALARS: usize = 512;

/// The envelope's `origin:` line for a session's own stored origin. Never read
/// from a connection: the finish hook runs on whatever thread the child's
/// provider ended on.
fn origin_line(origin: &SessionOrigin) -> String {
    match origin.kind {
        SessionOriginKind::Peer => {
            format!("peer:{}", origin.device_id.as_deref().unwrap_or_default())
        }
        SessionOriginKind::Local => "local".to_string(),
        SessionOriginKind::Unknown => "unknown".to_string(),
    }
}

/// The first `FINISH_SUMMARY_CHARS` characters of the child's last message.
///
/// Paseo's number, and characters rather than bytes so the cut cannot land
/// inside one. The whole message is still what gets deposited: the summary is
/// what a person reads in the transcript.
fn summary_of(message: Option<&str>) -> String {
    const FINISH_SUMMARY_CHARS: usize = 4000;
    let message = message.unwrap_or_default();
    message.chars().take(FINISH_SUMMARY_CHARS).collect()
}

/// How a child ended, in the vocabulary the finish report uses (`S5` §3).
///
/// A stop reason is the provider's own word for why the turn ended. Only
/// `end_turn` is a completed run: `max_tokens`, `max_turn_requests` and
/// `refusal` all mean the agent stopped short of doing what it was asked, and
/// saying `completed` there would be a claim the provider contradicts. A
/// session that never reported one is judged by its exit: a clean end is
/// `completed`, an unclean one `failed`, and a session the human closed (or one
/// whose daemon died) is `canceled` — it did not report and nothing says it
/// failed.
fn child_finish_state(
    session: &Session,
    runtime: &SessionRuntime,
) -> (AgentTaskState, Option<String>) {
    if let Some(stop_reason) = runtime.agent_stop_reason() {
        let state = stop_reason_state(&stop_reason);
        let note = (state != AgentTaskState::Completed).then(|| {
            format!(
                "The agent stopped with stop reason '{}'.",
                excerpt(&stop_reason, MAX_STOP_REASON_IN_NOTE)
            )
        });
        return (state, note);
    }
    match &session.state {
        SessionState::Ended { code: Some(0), .. } => (AgentTaskState::Completed, None),
        SessionState::Ended { code, .. } => (
            AgentTaskState::Failed,
            Some(match code {
                Some(code) => format!("The agent process exited with code {code}."),
                None => "The agent process exited without reporting a status.".to_string(),
            }),
        ),
        SessionState::Recovered { .. } => (
            AgentTaskState::Canceled,
            Some("The daemon that owned this agent died before it reported.".to_string()),
        ),
        SessionState::Live { .. } | SessionState::Silent { .. } => (AgentTaskState::Canceled, None),
    }
}

/// The A2A state one provider's stop reason means (`S5` decision 8, audit
/// S5-07).
///
/// The words the providers actually use, measured in this tree:
///
/// * `end_turn` — ACP's normal stop (`acp_client.rs`, `claude_view.rs` default)
///   and the Claude stream's own `stop_reason`;
/// * `completed` — codex's turn status (`codex_view.rs:849`);
/// * `interrupted` — codex's interrupted turn (`codex_view.rs:857`, `:1144`);
/// * `cancelled` / `canceled` — the daemon's own cancel path and ACP's
///   `cancelled`;
/// * anything else — `max_tokens`, `max_turn_requests`, `refusal`, pi's
///   `unknown` default (`pi_view.rs:165`), a reason from a provider version
///   this daemon has never seen — is `failed`. Failing closed is the point: a
///   creator that reads `completed` will believe work happened.
fn stop_reason_state(stop_reason: &str) -> AgentTaskState {
    match stop_reason {
        "end_turn" | "completed" => AgentTaskState::Completed,
        "interrupted" | "cancelled" | "canceled" => AgentTaskState::Canceled,
        _ => AgentTaskState::Failed,
    }
}

/// How long a `stop_reason` may be in the note's excerpt (`S5` decision 8,
/// audit S5-13).
///
/// The reason is provider data and may be any string; the note is prose a human
/// reads, and the whole envelope is bounded, so the excerpt is cut here rather
/// than trusted to be small.
const MAX_STOP_REASON_IN_NOTE: usize = 64;

/// The whole finish text's bound (`S5` decision 7, audit S5-13).
///
/// The summary is 4000 characters at most and the note is bounded, so this is
/// the ceiling the assembled envelope cannot pass: it is what keeps a finish
/// report deliverable at all, because the send path refuses an input larger
/// than its own write cap and a refused report is a creator that never learns
/// its child ended.
pub(crate) const MAX_FINISH_ENVELOPE_CHARS: usize = 8192;

/// The first `limit` characters of `text`, with an ellipsis when it was cut.
///
/// Characters, not bytes: a provider's reason may be any UTF-8, and cutting a
/// multi-byte sequence in half would panic rather than truncate.
fn excerpt(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let mut cut: String = text.chars().take(limit.saturating_sub(1)).collect();
    cut.push('…');
    cut
}

/// The envelope a creation's finish arrives in, bounded as a whole (`S5`
/// decision 7, audit S5-13).
///
/// The body is assembled once and then cut to
/// [`MAX_FINISH_ENVELOPE_CHARS`]; the artifact array is daemon-composed and
/// small, so the only field that can make this long is the summary (already cut
/// to `FINISH_SUMMARY_CHARS` by `summarise_message`) and the note, and cutting
/// the assembled text here is what keeps it deliverable at all.
fn bound_finish_envelope(envelope: String) -> String {
    excerpt(&envelope, MAX_FINISH_ENVELOPE_CHARS)
}

/// The envelope one agent's message arrives in (S4-04).
///
/// The envelope is *prose for a model*, not a parser boundary: nothing on this
/// daemon's side reads it back, and the receiving agent is asked to treat it as
/// a note about who is speaking. That is exactly why the sender's own text must
/// not be able to write the daemon's delimiters: see
/// [`neutralise_envelope_text`]. `origin`, `role` and `from_agent` are composed
/// from daemon state (the caller's authenticated connection, a validated
/// session id), never from the message text.
fn agent_message_envelope(origin: &str, role: &str, from_session: &str, text: &str) -> String {
    format!(
        "<devboule-system>\norigin: {origin}\nrole: {role}\nfrom_agent: {from_session}\ntimestamp: {}\n{}\n</devboule-system>",
        unix_millis(),
        neutralise_envelope_text(text)
    )
}

/// Make the sender's text unable to close or reopen the envelope: every
/// case-insensitive occurrence of `<devboule-system` or `</devboule-system` is
/// escaped to `&lt;devboule-system`, and CRLF/CR are normalised to LF first so
/// the escaped text cannot smuggle a carriage return past the line the envelope
/// writes it on.
///
/// The excerpt fence markers are neutralised here too — **one rule, one
/// place**. The `agent_permission_request` frame quotes the child's words
/// between the exact lines `child-said:` and `end child-said`, and a child
/// that writes a line `end child-said` inside its own words would close its
/// quoted block early: the human would see less of the card than the model
/// does, with no marker that anything was cut. Any line that is exactly a
/// fence marker has its first scalar entity-escaped — the same escape the
/// tags get, applied at the marker's first character (`child-said:` becomes
/// `&#99;hild-said:`), which the app's exact-line parser can no longer match.
/// The app cannot tell an injected closer from a real one, which is why the
/// cure has to be here.
///
/// Escaping rather than stripping: the text still reads the way its author
/// wrote it, minus the delimiter it was trying to be.
fn neutralise_envelope_text(text: &str) -> String {
    let normalised = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut neutral = String::with_capacity(normalised.len());
    let mut cursor = 0;
    while let Some((start, len)) = next_envelope_delimiter(&normalised, cursor) {
        neutral.push_str(&normalised[cursor..start]);
        neutral.push_str("&lt;");
        neutral.push_str(&normalised[start + 1..start + len]);
        cursor = start + len;
    }
    neutral.push_str(&normalised[cursor..]);
    neutralise_excerpt_fences(&neutral)
}

/// The exact lines a `child-said:` fence is made of, and the entity escape of
/// each one's first scalar. Exact, never trimmed — the app's parser matches
/// the exact line only, so a padded or tabbed fence line is the child's own
/// text and is left alone here too.
const EXCERPT_FENCE_MARKERS: [(&str, &str); 2] = [
    ("child-said:", "&#99;hild-said:"),
    ("end child-said", "&#101;nd child-said"),
];

/// Escape any line that is exactly a fence marker, after the tag pass. Line
/// scoped, because the fence is line scoped: a marker buried inside a line is
/// words, not structure. The walk keeps every line ending byte-for-byte —
/// only the marker line's leading scalar changes.
fn neutralise_excerpt_fences(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut touched = false;
    for segment in text.split_inclusive('\n') {
        let line = segment.strip_suffix('\n').unwrap_or(segment);
        let escaped = EXCERPT_FENCE_MARKERS
            .iter()
            .find(|(marker, _)| line == *marker)
            .map(|(_, escaped)| *escaped);
        match escaped {
            Some(escaped) => {
                out.push_str(escaped);
                if segment.ends_with('\n') {
                    out.push('\n');
                }
                touched = true;
            }
            None => out.push_str(segment),
        }
    }
    if touched {
        out
    } else {
        text.to_string()
    }
}

/// Byte offset and length of the next envelope delimiter at or after `from`,
/// compared case-insensitively. Both tags are scanned for, in one pass: the
/// closing tag does not contain the opening one character for character, so a
/// search for the opening tag alone would miss it.
fn next_envelope_delimiter(text: &str, from: usize) -> Option<(usize, usize)> {
    const OPEN: &[u8] = b"<devboule-system";
    const CLOSE: &[u8] = b"</devboule-system";
    let bytes = text.as_bytes();
    for start in from..bytes.len() {
        if bytes[start] != b'<' {
            continue;
        }
        for tag in [OPEN, CLOSE] {
            if bytes.len() - start >= tag.len()
                && bytes[start..start + tag.len()].eq_ignore_ascii_case(tag)
            {
                return Some((start, tag.len()));
            }
        }
    }
    None
}

/// What one admission answered: the slot it took, and whether the message went
/// into the target's running turn (S4-03).
///
/// `steered_into_turn` is the *runtime's* answer, taken under the same lock
/// `finish_turn` takes, not the caller's earlier look: it is what decides steer
/// versus plain prompt. The slot always has a boundary either way — the turn this
/// message joined, or the turn the plain prompt it became started; the expiry is
/// only the fallback for a turn that never ends.
#[derive(Debug)]
pub(crate) struct MessageAdmission {
    pub(crate) slot: u64,
    pub(crate) steered_into_turn: bool,
    /// The turn the admission registered its boundary against (S4-14): the target's
    /// counter at that moment. The delivery compares it with the turn that is
    /// running when it writes, so a message that ends up in a *different* turn is
    /// re-keyed onto that one instead of staying on the boundary of a turn that has
    /// already ended.
    pub(crate) expected_turn_id: u64,
}

/// The boundary callback of one slot, with the cell that tells it which hook it is
/// (S4-15, S5-01).
///
/// The callback compares its own hook id — read out of the cell *while it holds the
/// brakes lock* — with the id the slot currently holds, and acts only when they are
/// the same. A callback whose hook has been replaced by a re-arm therefore does
/// nothing: without that check it would take the *new* hook, unregister it, and mark
/// the slot `boundary_reached`, which is exactly how a re-armed slot loses its live
/// boundary when the old callback was already waiting for the brakes lock.
///
/// **Invariant:** registration stores the id into the cell *before* it releases
/// `brakes`, and the callback loads the cell *after* it takes `brakes`. Both halves
/// are required: the store under the lock serializes it against every callback that
/// acquires the lock, and the load under the lock is what makes the callback see a
/// value that is already stored. A callback that read the cell before taking the lock
/// could read the initial `0` in the window between `on_turn_end` returning and the
/// store, be rejected against the live id, and leave its slot without an effective
/// boundary until the expiry (S5-01). Both registrations — `reserve_message_brake`
/// and `rearm_message_slot_boundary` — keep the store inside their `brakes` hold.
///
/// A callback that fires before its slot's entry exists finds nothing to act on and
/// does nothing — never a panic and never a release — so the slot falls back to its
/// expiry, the safe direction. With the invariant in place that window is not
/// observable from a callback that runs after the registration completes: the lock
/// serializes it behind the store.
fn message_slot_boundary(
    brakes: &Arc<Mutex<MessageBrakeTable>>,
    from_session: &str,
    slot: u64,
) -> (Arc<dyn Fn() + Send + Sync>, Arc<AtomicU64>) {
    let hook_id = Arc::new(AtomicU64::new(0));
    let callback: Arc<dyn Fn() + Send + Sync> = {
        let brakes = Arc::clone(brakes);
        let from = from_session.to_string();
        let hook_id = Arc::clone(&hook_id);
        // (S5-01) The cell is handed to the callback, not a value read here: the
        // load happens inside `boundary_reached_message_slot`, under the brakes
        // lock, so it cannot observe the window before the registering side stored
        // the id.
        Arc::new(move || boundary_reached_message_slot(&brakes, &from, slot, &hook_id))
    };
    (callback, hook_id)
}

/// Whether this slot's boundary is stale (S4-14): its admitted turn has already
/// ended, or the runtime has moved on to a different turn than the one the
/// admission checked.
///
/// Read under the brakes lock, but as its own step: the delivery uses it to decide
/// whether the slot has to be re-keyed onto the turn its text is about to enter,
/// and the re-arm itself is the only writer.
fn message_slot_boundary_is_stale(slot: &MessageSlotRef<'_>, entering_turn_id: u64) -> bool {
    let Ok(table) = slot.brakes.lock() else {
        return false;
    };
    let Some(brake) = table.get(slot.from_session) else {
        return false;
    };
    let Some(entry) = brake
        .outstanding
        .iter()
        .find(|entry| entry.slot == slot.slot)
    else {
        return false;
    };
    entry.boundary_reached || entering_turn_id != slot.admitted_turn_id
}

/// Re-key one slot's boundary onto the turn its text actually enters (S4-10,
/// S4-14).
///
/// Called by the delivery before the write, under the brakes lock, once the slot's
/// boundary is known to be stale: the turn the message was admitted into ended —
/// its hook has already fired and set `boundary_reached` — and the text is about to
/// steer into a newer turn or become an ordinary prompt. Either way the turn it
/// enters is the turn that ends it, so: clear the flag, drop the old hook (a no-op
/// when it already fired, and harmless when it is still armed), and arm the same
/// boundary the admission arms, for the turn that is coming.
///
/// The expiry and a failed delivery still end the slot on their own; the point is
/// that a *successful* delivery never retires a slot whose turn is still running.
fn rearm_message_slot_boundary(slot: &MessageSlotRef<'_>, runtime: &Arc<SessionRuntime>) {
    let Ok(mut table) = slot.brakes.lock() else {
        return;
    };
    let Some(brake) = table.get_mut(slot.from_session) else {
        return;
    };
    let Some(entry) = brake
        .outstanding
        .iter_mut()
        .find(|entry| entry.slot == slot.slot)
    else {
        return;
    };
    entry.boundary_reached = false;
    if let Some((previous, hook)) = entry.release.take() {
        if let Some(previous) = previous.upgrade() {
            previous.off_turn_end(hook);
        }
    }
    // (S4-15) Registered first, then the id is written back into the cell: the
    // callback compares that id with the one this entry holds, so the hook that
    // was just replaced can no longer unregister its successor.
    let (boundary, hook_id) = message_slot_boundary(slot.brakes, slot.from_session, slot.slot);
    let armed = runtime.on_turn_end(move || boundary());
    hook_id.store(armed, Ordering::Release);
    entry.release = Some((Arc::downgrade(runtime), armed));
}

/// Admit one inter-agent message, answering the slot it took and the turn it
/// joined (S4-03).
///
/// The brakes are the sender's budget: at most [`MAX_MESSAGE_OUTSTANDING`]
/// messages in flight, at most [`MAX_MESSAGE_SENT_PER_WINDOW`] inside the
/// rate window, and at most [`MAX_MESSAGE_RECIPIENTS`] distinct recipients
/// inside the recipient window. "In flight" ends at a boundary, not at the
/// next probe: the slot is released by the turn it went into ending — the hook
/// registered on `target` here — by [`finish_message_delivery`] when the
/// delivery fails, or by expiry at [`MESSAGE_SLOT_EXPIRY`], whichever comes
/// first.
///
/// `target` is the runtime whose turn the message joins plus the turn id the
/// caller checked. The check and the registration are one atomic step on that
/// runtime, and its answer — not the caller's snapshot — is what decides between
/// steer and prompt (S4-03). The boundary is armed for either outcome: the turn
/// the message joined, or the turn the plain prompt it became started.
///
/// `now` is the caller's clock rather than `Instant::now()`, so the two windows
/// are testable by moving the clock instead of sleeping through it.
fn reserve_message_brake(
    brakes: &Arc<Mutex<MessageBrakeTable>>,
    from_session: &str,
    to_session: &str,
    target: Option<(&Arc<SessionRuntime>, u64)>,
    now: Instant,
) -> Result<MessageAdmission, WireError> {
    let mut table = brakes
        .lock()
        .map_err(|_| internal("Agent message state is unavailable."))?;
    // (S4-12, S4-16) The table is swept here, before this sender's own entry is
    // touched: a session that closed keeps its recipient window (that is the point
    // — a close-and-resume must not buy a fresh set of three), so something has to
    // age those entries out, and this is the path that sees the whole table with a
    // clock. Entries whose window has run out are pruned with the caller's clock,
    // their expired hooks join the list this function unregisters below, and an
    // entry with nothing left in it goes.
    //
    // The sweep costs one pass over every other sender while the single brakes lock
    // is held, so it runs at most once per [`MESSAGE_RATE_WINDOW`] (S4-16) — a
    // sender that never sweeps cannot make every other sender's admission pay for
    // it. The caller's own entry is still pruned on every reserve, which is what
    // its own braking needs.
    let mut expired: Vec<(Weak<SessionRuntime>, u64)> = Vec::new();
    if table.sweep_is_due(now) {
        let mut swept: Vec<String> = Vec::new();
        for (other, other_brake) in table.iter_mut() {
            if other == from_session {
                continue;
            }
            expired.extend(other_brake.prune(now));
            if other_brake.is_idle() {
                swept.push(other.clone());
            }
        }
        for other in swept {
            table.remove(&other);
        }
        table.note_sweep(now);
    }
    let brake = table
        .entry(from_session.to_string())
        .or_insert_with(MessageBrake::new);
    expired.extend(brake.prune(now));
    if now.saturating_duration_since(brake.window_started) >= MESSAGE_RATE_WINDOW {
        brake.window_started = now;
        brake.sent_in_window = 0;
    }
    // The refusals are *collected* rather than returned on the spot: the expired
    // hooks from `prune` are unregistered below, after this lock is released
    // (S4-02), and an early return here would leave them armed on their runtimes
    // forever.
    let refused = if brake.outstanding.len() >= MAX_MESSAGE_OUTSTANDING
        || brake.sent_in_window >= MAX_MESSAGE_SENT_PER_WINDOW
    {
        Some(WireError::new(
            ErrorCode::CapabilityNotSupported,
            "Agent message limit exceeded; do not retry.",
        ))
    } else if !brake.holds_recipient(to_session) && brake.recipients.len() >= MAX_MESSAGE_RECIPIENTS
    {
        Some(WireError::new(
            ErrorCode::CapabilityNotSupported,
            "Agent message recipient limit exceeded; do not retry.",
        ))
    } else {
        None
    };
    let admission = if refused.is_some() {
        None
    } else {
        let slot = brake.next_slot;
        brake.next_slot = brake.next_slot.saturating_add(1);
        // (S4-03) The turn this message joins and the boundary that releases its
        // slot are decided by one atomic step on the runtime: the turn cannot end
        // between the check and the registration without this answering `None`.
        //
        // A slot always has a boundary, whichever turn it turns out to be: the
        // turn this message joins when the answer is `Some` — and when it is
        // `None` the text goes as a plain prompt, so the boundary is the end of
        // the turn that prompt starts. The expiry is only the fallback for a
        // turn that never ends.
        let mut steered_into_turn = false;
        // (S4-15) The callback is built once and its id cell filled in as soon as
        // the runtime answers with the hook it registered; the second arm reads the
        // same cell, so whichever hook is live compares itself against the id this
        // entry ends up holding.
        let (boundary, hook_id) = message_slot_boundary(brakes, from_session, slot);
        let release = target.map(|(runtime, expected_turn)| {
            let target = Arc::downgrade(runtime);
            let armed = {
                let first = Arc::clone(&boundary);
                match runtime.on_turn_end_if_active(expected_turn, move || first()) {
                    Some(hook) => {
                        steered_into_turn = true;
                        hook
                    }
                    None => {
                        let second = Arc::clone(&boundary);
                        runtime.on_turn_end(move || second())
                    }
                }
            };
            hook_id.store(armed, Ordering::Release);
            (target, armed)
        });
        brake.outstanding.push(OutstandingMessage {
            slot,
            sent_at: now,
            to_session: to_session.to_string(),
            delivered: false,
            boundary_reached: false,
            release,
        });
        brake.sent_in_window = brake.sent_in_window.saturating_add(1);
        if let Some(recipient) = brake
            .recipients
            .iter_mut()
            .find(|recipient| recipient.session_id == to_session)
        {
            // A recipient the sender keeps writing to stays in the window: the
            // window answers "who has this sender written to lately".
            recipient.sent_at = now;
        } else {
            brake.recipients.push(Recipient {
                session_id: to_session.to_string(),
                sent_at: now,
            });
        }
        Some(MessageAdmission {
            slot,
            steered_into_turn,
            expected_turn_id: target.map(|(_, turn)| turn).unwrap_or(0),
        })
    };
    // Released before any runtime lock is taken, the order
    // `boundary_reached_message_slot` uses.
    drop(table);
    // The brake lock is released before any runtime lock is taken: the expiry
    // hooks go back on their runtimes here, outside it (S4-02), the same order
    // `boundary_reached_message_slot` uses.
    for (runtime, hook) in expired {
        if let Some(runtime) = runtime.upgrade() {
            runtime.off_turn_end(hook);
        }
    }
    match (admission, refused) {
        (_, Some(error)) => Err(error),
        (Some(admission), None) => Ok(admission),
        (None, None) => Err(internal("Agent message state is unavailable.")),
    }
}

/// The boundary arrived for one slot: the target's turn ended, or the slot's own
/// expiry ran out.
///
/// A slot whose delivery has already returned is over and goes here, with its
/// hook unregistered and its recipient entry dropped once no other slot names
/// that target (A2-06). One whose delivery is still in flight keeps its place: the message it counts is still being
/// written, and releasing it now would let the next send past the cap this
/// count exists to keep (A2-05).
///
/// `hook_id` is the cell holding the id of the hook this callback was armed as
/// (S4-15, S5-01). It is loaded *inside* the locked section below — never before —
/// so that the registering side's store, which it performs while it holds the same
/// lock, is always visible here. A callback that ran after the slot was re-keyed
/// holds the *old* id, while the entry holds the new one: it is a no-op, because
/// its turn is not the turn the slot is waiting on any more and taking the live
/// hook here would leave the slot without a boundary.
fn boundary_reached_message_slot(
    brakes: &Arc<Mutex<MessageBrakeTable>>,
    from_session: &str,
    slot: u64,
    hook_id: &AtomicU64,
) {
    let mut hooks: Vec<(Weak<SessionRuntime>, u64)> = Vec::new();
    {
        let Ok(mut table) = brakes.lock() else {
            return;
        };
        // (S5-01) Under the lock: the id the registering side stored before it
        // released this same lock, so a turn end dispatched in the window between
        // `on_turn_end` returning and the store cannot be rejected with the initial
        // zero.
        let hook_id = hook_id.load(Ordering::Acquire);
        let mut to_session = None;
        let mut drop_sender = false;
        if let Some(brake) = table.get_mut(from_session) {
            let mut remove_slot = false;
            if let Some(entry) = brake
                .outstanding
                .iter_mut()
                .find(|entry| entry.slot == slot)
            {
                if entry.release.as_ref().map(|(_, armed)| *armed) != Some(hook_id) {
                    return;
                }
                entry.boundary_reached = true;
                if let Some(hook) = entry.release.take() {
                    hooks.push(hook);
                }
                if entry.delivered {
                    remove_slot = true;
                    to_session = Some(entry.to_session.clone());
                }
            }
            if remove_slot {
                let _ = brake.take_slot(slot);
                if let Some(to_session) = &to_session {
                    brake.drop_recipient_if_idle(to_session, Instant::now());
                }
            }
            drop_sender = brake.is_idle();
        }
        if drop_sender {
            table.remove(from_session);
        }
    }
    for (runtime, hook) in hooks {
        if let Some(runtime) = runtime.upgrade() {
            runtime.off_turn_end(hook);
        }
    }
}

/// Report one delivery back to the bookkeeping (A2-05).
///
/// `delivered` false is a delivery that reached nothing: the slot goes back at
/// once, because holding a sender's budget for a turn that will never see the
/// message is exactly what the release-on-failure rule is for. `delivered` true
/// keeps the slot until its boundary — one already reached releases it here, one
/// still ahead releases it when it arrives.
fn finish_message_delivery(
    brakes: &Arc<Mutex<MessageBrakeTable>>,
    from_session: &str,
    slot: u64,
    delivered: bool,
) {
    let mut hooks: Vec<(Weak<SessionRuntime>, u64)> = Vec::new();
    {
        let Ok(mut table) = brakes.lock() else {
            return;
        };
        let mut to_session = None;
        let mut drop_sender = false;
        if let Some(brake) = table.get_mut(from_session) {
            let mut remove_slot = false;
            if let Some(entry) = brake
                .outstanding
                .iter_mut()
                .find(|entry| entry.slot == slot)
            {
                entry.delivered = true;
                if !delivered || entry.boundary_reached {
                    remove_slot = true;
                    to_session = Some(entry.to_session.clone());
                }
            }
            if remove_slot {
                if let Some(hook) = brake.take_slot(slot) {
                    hooks.push(hook);
                }
                if let Some(to_session) = &to_session {
                    brake.drop_recipient_if_idle(to_session, Instant::now());
                }
            }
            drop_sender = brake.is_idle();
        }
        if drop_sender {
            table.remove(from_session);
        }
    }
    for (runtime, hook) in hooks {
        if let Some(runtime) = runtime.upgrade() {
            runtime.off_turn_end(hook);
        }
    }
}

/// Forget one session as a *target* (A2-06): every slot pointing at it, and its
/// recipient entries once they age out of the window (S4-01).
///
/// Called where the target closes, inside the same session-map critical section
/// that removes it from the registry, so a send that found the target cannot
/// reserve a slot for it afterwards (A2-05). A closed target's turn can never
/// end, so its slots would otherwise sit out their whole expiry holding their
/// senders' budgets for a session that is gone — those go at once.
///
/// A recipient entry is *deliberately* kept while it is still inside the window:
/// the window is the fan-out brake, and letting a close erase it early would
/// hand the sender a free slot to reach a fresh agent, which is the rotation the
/// window exists to stop.
///
/// The closing session's own entry is kept for the same reason (S4-12): closing
/// and resuming the same session id must not buy a fresh set of three recipients
/// inside the window. Its *slots* go, to this target or to any other — a closed
/// session will not send again, so those messages have no turn left to be
/// answered by — and every hook of theirs is collected here and unregistered
/// below, outside the lock, exactly as expiry does (S4-11).
///
/// The table stays bounded because the expiry sweep inside
/// [`reserve_message_brake`] drops an entry once its window has aged out.
fn forget_message_brake_target(brakes: &Arc<Mutex<MessageBrakeTable>>, target_session: &str) {
    let now = Instant::now();
    let mut hooks: Vec<(Weak<SessionRuntime>, u64)> = Vec::new();
    {
        let Ok(mut table) = brakes.lock() else {
            return;
        };
        let mut idle: Vec<String> = Vec::new();
        for (from_session, brake) in table.iter_mut() {
            let closing_sender = from_session == target_session;
            let mut kept: Vec<OutstandingMessage> = Vec::with_capacity(brake.outstanding.len());
            for mut entry in brake.outstanding.drain(..) {
                if entry.to_session == target_session || closing_sender {
                    if let Some(hook) = entry.release.take() {
                        hooks.push(hook);
                    }
                } else {
                    kept.push(entry);
                }
            }
            brake.outstanding = kept;
            // Written out rather than called as a method so the closure borrows
            // only `recipients`.
            brake.recipients.retain(|recipient| {
                recipient.session_id != target_session
                    || now.saturating_duration_since(recipient.sent_at) < MESSAGE_SLOT_EXPIRY
            });
            if brake.is_idle() {
                idle.push(from_session.clone());
            }
        }
        for from_session in idle {
            table.remove(&from_session);
        }
    }
    for (runtime, hook) in hooks {
        if let Some(runtime) = runtime.upgrade() {
            runtime.off_turn_end(hook);
        }
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
        // Peer visibility, deliberately: a windowed child's death is the
        // delivery's own refusal to observe (the awaited rpc times out and
        // the close tears it down), and the sweep's transitions must not
        // fire for a session no roster lists.
        .filter_map(|entry| {
            let session = entry.as_peer_visible()?;
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

/// Whether a failed spawn says anything about the **provider's** health.
///
/// The clients refuse, before and around the spawn, every value the profile
/// alone decides — an unknown model, mode or thinking option, a catalogue
/// that publishes nothing, an `autoAccept` contradiction, an agent refusing
/// the delivered switch — and every one of those refusals is
/// `ErrorCode::InvalidRequest` by convention; nothing else on a spawn path
/// raises that code (a provider-side failure is `Io`/`Internal`, including
/// Pi's extension not activating). A profile mistake is the human's to fix
/// in the profile: recording it against the provider degrades the Settings
/// health line for a correctly installed provider (the R2a audit's F6).
fn spawn_failure_is_provider_health(error: &WireError) -> bool {
    error.code != ErrorCode::InvalidRequest
}

pub fn spawn_session(
    state: &Arc<ServerState>,
    registry: &SessionRegistry,
    metadata: Session,
    owner: OwnerId,
    command: PtyCommand,
    mut mcp_session: Option<McpSessionGuard>,
    delivery: crate::profile_delivery::ProfileDelivery,
) -> Result<(), WireError> {
    // The delivery travels as the one typed value: each family's `spawn`
    // validates what it can refuse and applies what it owns. The dispatch is
    // the registry's — no arm matches on the kind or names a family — and
    // each family's workspace error mapping happens inside its own `spawn`,
    // which is why none is applied here.
    let workspace_id = metadata.workspace_id.clone();
    let spawned = provider::catalog_registry()
        .provider_for_kind(&metadata.kind)
        .spawn(
            state,
            command,
            state.mcp.launch_config(&metadata.id),
            delivery.clone(),
            workspace_id.as_deref(),
        )?;
    start_spawned_session(
        state,
        registry,
        metadata,
        owner,
        None,
        delivery.mode_id,
        spawned,
        mcp_session.take(),
    )
}

pub(crate) struct ResumedSessionContext {
    peer_session_id: String,
    generation: u64,
    mcp_session: Option<McpSessionGuard>,
}

pub fn spawn_resumed_session(
    state: &Arc<ServerState>,
    registry: &SessionRegistry,
    metadata: Session,
    owner: OwnerId,
    command: PtyCommand,
    context: ResumedSessionContext,
) -> Result<(), WireError> {
    let mcp = state.mcp.launch_config(&metadata.id);
    start_spawned_session(
        state,
        registry,
        metadata,
        owner,
        Some(context.generation),
        None,
        acp_client::spawn_process_resuming(state, command, context.peer_session_id, mcp)?,
        context.mcp_session,
    )
}

#[allow(clippy::too_many_arguments)]
fn start_spawned_session(
    state: &Arc<ServerState>,
    registry: &SessionRegistry,
    metadata: Session,
    owner: OwnerId,
    generation: Option<u64>,
    requested_mode: Option<String>,
    spawned: SpawnedSession,
    mcp_session: Option<McpSessionGuard>,
) -> Result<(), WireError> {
    let SpawnedSession {
        process_job,
        master,
        killer,
        switcher,
        child,
        writer,
        image_sink,
        static_image_sink,
        reader,
        reader_dispatch,
        stderr,
        permission_broker,
        os_handle,
        peer_session_id,
        agent_version,
        pending_delivery,
        pending_codex_verify,
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
    if metadata.kind.is_agent() {
        runtime.set_agent_kind(metadata.kind.clone());
    }
    // The origin the create wrote travels with the metadata; installing it on
    // the runtime is what lets the permission broker stamp a card and the peer
    // gate answer `prompt_skipping` without a registry lookup.
    runtime.set_origin(metadata.origin.clone());
    // S9: hosting is one predicate; waiting is the narrower rule (S8: never
    // pi/Codex — the twin tests pin it). Binding is registration-fact-gated
    // inside `bind_runtime` itself (a lookup that no-ops without a row), so it
    // runs unconditionally: identical for every road but a minted carrier. The
    // guard is strict exactly where MCP gates the send path (a lost bearer
    // there must fail loudly, never leak); elsewhere the create road's
    // `Option` flows through untouched (tests and unregistered spawns).
    if crate::mcp_broker::mcp_gates_first_prompt(&metadata.kind) {
        runtime.require_mcp();
    }
    state.mcp.bind_runtime(&metadata.id, &runtime);
    let mcp_session = if crate::mcp_broker::mcp_gates_first_prompt(&metadata.kind) {
        Some(mcp_session.ok_or_else(|| {
            internal("MCP session registration was lost before provider startup.")
        })?)
    } else {
        mcp_session
    };
    if let Some(peer_session_id) = peer_session_id {
        runtime.set_peer_session_id(peer_session_id);
    }
    if let Some(generation) = generation {
        runtime.set_generation(generation);
        // `generation` is `Some` on exactly one road: a resume
        // (`spawn_resumed_session`). A fresh spawn starts unnumbered and the
        // journal numbers its first generation. The resumed generation is
        // mid-conversation, so the session owes no first prompt — the comment
        // on `first_prompt_owed` promises a resume never re-injects the
        // standing instructions into the next prompt the human sends.
        runtime.clear_first_prompt_owed();
    }
    if metadata.kind == SessionKind::Claude {
        let catalog = state.claude_models();
        runtime.store_claude_manifest(
            crate::claude_catalog::initial_manifest_with_mode(
                catalog.models,
                requested_mode.as_deref().unwrap_or("default"),
            ),
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
    {
        // The finish report's trigger (`S5` §3; the slice-5 e2e battery is why
        // it is not the attention hook): a published `AgentFinished` calls this
        // once, and [`SessionRegistry::report_child_finish`] no-ops for a
        // session that is not an agent-created child of ours.
        let registry = registry.clone();
        let session_id = metadata.id.clone();
        runtime.set_finish_notify(Arc::new(move || {
            registry.report_child_finish(&session_id);
        }));
    }
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
        steerer: switcher
            .as_ref()
            .map(|switcher| switcher.clone_steerer())
            .unwrap_or_else(|| Box::new(UnsupportedSteerer)),
        switcher,
        child_wait,
        writer,
        image_sink,
        static_image_sink,
        reader_handle: None,
        coalesce_handle: None,
        stderr_handle: None,
        runtime: Arc::clone(&runtime),
        mcp_session,
        exited: Arc::clone(&exited),
        preserve_on_exit: Arc::new(AtomicBool::new(false)),
    };

    // Insert BEFORE starting the reader. A shell can exit before the reader
    // thread gets scheduled; inserting later would let EOF cleanup miss the
    // map entry and strand the session.
    //
    // The entry goes in as `Configuring` and is promoted to `Live` only
    // after the delivery below has landed (the re-audit's P2-1). A child
    // that is live but not yet configured is the authority gap this slice
    // exists to close: between this insert and the delivery there used to be
    // a listed, promptable session whose card had not been honoured — a peer
    // could see it, send it work, and have that work silently die with a
    // refused delivery. A `Configuring` entry is invisible to every roster
    // read and refused by every id-addressed peer call, while the daemon's
    // own teardown paths (the refusal's `close`, EOF reaping) still reach
    // it.
    {
        let Ok(mut map) = registry.inner.lock() else {
            teardown_session(session);
            return Err(internal("Session state is unavailable."));
        };
        map.insert(id.clone(), RegistryEntry::Configuring(Box::new(session)));
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
                    let _ = registry.close(&id, &owner, &None);
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
        // The entry is `Configuring` until the delivery lands; the daemon's
        // own bookkeeping reaches through the window, peers do not.
        if let Some(session) = map
            .get_mut(&id)
            .and_then(RegistryEntry::as_child_process_mut)
        {
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
            let _ = registry.close(&id, &owner, &None);
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
        if let Some(session) = map
            .get_mut(&id)
            .and_then(RegistryEntry::as_child_process_mut)
        {
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
    // The pending delivery runs here and only here: it is an awaited rpc
    // whose answers only the session reader delivers, and that reader is now
    // live. Run any earlier and the wait outlives its deliverer — fifteen
    // seconds of stall, then a refusal, for every child a profile creates
    // (the R2a audit's F1). A refused delivery tears the child down — the
    // registry entry is still in its `Configuring` state, whose teardown the
    // close serves — and fails the creation, before any prompt can reach a
    // child the card did not describe.
    if let Some(deliver) = pending_delivery {
        if let Err(error) = deliver() {
            let _ = registry.close(&id, &owner, &None);
            return Err(error);
        }
    }
    // The delivery landed: the session exists. The promotion is one
    // critical section — remove and reinsert under the same lock hold — so
    // no other thread can observe the id absent, and from here on the
    // rosters list it and every id-addressed call reaches it.
    if let Ok(mut map) = registry.inner.lock() {
        if let Some(RegistryEntry::Configuring(session)) = map.remove(&id) {
            map.insert(id.clone(), RegistryEntry::Live(session));
        }
    }
    // S8 trigger: a Codex carrier verification runs detached — never blocking
    // this thread, never fatal whatever it answers. The reader above is live,
    // so the poll's answer has a deliverer; the flip lands whenever it lands
    // and the first prompt proceeds meanwhile. `None` on every road but the
    // carrier road (today: all of them).
    spawn_codex_verify_thread(pending_codex_verify, &runtime, &id);
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

/// S8 trigger body, one function so the test drives the real code: run a Codex
/// carrier verification detached and flip the runtime whenever it lands. `None`
/// is a no-op (today's only road). Detached, never blocking, never fatal — the
/// first prompt proceeds whatever the poll answers, and a late answer still
/// flips the roster (S8) whenever it arrives.
pub(crate) fn spawn_codex_verify_thread(
    bundle: Option<codex_client::CodexVerifyBundle>,
    runtime: &Arc<SessionRuntime>,
    session_id: &str,
) {
    let Some(bundle) = bundle else {
        return;
    };
    let verify_runtime = Arc::clone(runtime);
    let verify_id = session_id.to_string();
    let _ = std::thread::Builder::new()
        .name(format!("codex-verify-{verify_id}"))
        .spawn(move || {
            let state = codex_client::verify_codex_mcp(&bundle);
            verify_runtime.set_tools_state(state);
        });
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
        runtime.fail_mcp_if_pending("The agent closed its output before the MCP broker was ready.");
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
    runtime.fail_mcp_if_pending("The agent closed its output before the MCP broker was ready.");

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
    // Captured before the mutable borrow below: a preserved session stays in
    // the map, and its end still owes its creator a report (audit S5B-03).
    let owner = map.get(id).map(|entry| entry.owner().clone());
    let Some(session) = map
        .get_mut(id)
        .and_then(RegistryEntry::as_child_process_mut)
    else {
        return false;
    };
    session.reader_handle = None;
    let preserve = session.preserve_on_exit.load(Ordering::SeqCst);
    if preserve {
        let coalesce = session.coalesce_handle.take();
        let mcp_session = session.mcp_session.take();
        session.exited.store(true, Ordering::SeqCst);
        let ended = owner.clone().map(|owner| {
            (
                live_session_view(session),
                Arc::clone(&session.runtime),
                owner,
            )
        });
        drop(map);
        drop(mcp_session);
        join_coalesce(coalesce, runtime);
        journal_mark_ended(registry, runtime);
        runtime.close_output();
        // A stopped child that kept its transcript is a child that ended (audit
        // S5B-03): the session stays listed on purpose, and its slot and its
        // report are still owed to the creator.
        if let Some((session, child_runtime, owner)) = ended {
            // A child whose creation has not committed yet has no link and a
            // creator that does not know about it: the end waits for the
            // commit (audit-2 §2).
            if !registry.defer_child_end_if_pending(id, &session, &child_runtime, &owner) {
                registry.child_ended_with(id, Some(&session), Some(&child_runtime), Some(&owner));
            }
        }
        return false;
    }
    // The target's message-brake entries leave with it (A2-06), inside this
    // same critical section: an admission that found the session in the map
    // cannot reserve a slot for it after this point (A2-05).
    forget_message_brake_target(&registry.message_brakes, id);
    // What this child's end owes its creator is copied out of the row *before*
    // it is removed (`S5` decisions 7 and 8, audit S5-01): the report needs the
    // row's metadata, its runtime and its owner, and this is the path that ends
    // a child whose provider exited on its own — the common end — which used to
    // take the row out without releasing the slot or telling the creator.
    let ended = map.get(id).and_then(|entry| {
        entry.as_child_process().map(|live| {
            (
                live_session_view(live),
                Arc::clone(&live.runtime),
                entry.owner().clone(),
            )
        })
    });
    // `Configuring` is taken too: a child whose delivery never landed is
    // still a child whose end owes the teardown below.
    let (Some(RegistryEntry::Live(session)) | Some(RegistryEntry::Configuring(session))) =
        map.remove(id)
    else {
        return false;
    };
    let mut session = *session;
    drop(map);
    session.reader_handle = None;
    let coalesce = session.coalesce_handle.take();
    let stderr = session.stderr_handle.take();
    let child_wait = session.child_wait.take();
    let PtySession {
        master,
        writer,
        image_sink: _,
        static_image_sink: _,
        killer,
        runtime: session_runtime,
        mcp_session,
        exited,
        ..
    } = session;
    exited.store(true, Ordering::SeqCst);
    // Revoke MCP before killing the child: an in-flight provider request may
    // race teardown, and a closed session must not authorize new work.
    drop(mcp_session);
    drop(killer);
    drop(writer);
    drop(master);
    bounded_join(stderr);
    bounded_join(child_wait);
    join_coalesce(coalesce, runtime);
    let _ = session_runtime;
    journal_mark_ended(registry, runtime);
    runtime.close_output();
    // The end is complete: the report (once per child, whatever path got here
    // first) and the release of the child's slot, in that order.
    if let Some((session, child_runtime, owner)) = ended {
        // The same deferral as the preserve branch: an end that beats the
        // commit waits for it (audit-2 §2).
        if !registry.defer_child_end_if_pending(id, &session, &child_runtime, &owner) {
            registry.child_ended_with(id, Some(&session), Some(&child_runtime), Some(&owner));
        }
    }
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
    session
        .runtime
        .fail_mcp_if_pending("The agent session closed before the MCP broker was ready.");
    let PtySession {
        process_job,
        master,
        mut killer,
        steerer: _,
        switcher: _,
        child_wait,
        writer,
        image_sink: _,
        static_image_sink: _,
        reader_handle,
        coalesce_handle,
        stderr_handle,
        runtime,
        exited: _,
        owner: _,
        metadata: _,
        preserve_on_exit: _,
        mcp_session,
    } = session;

    // Revocation intentionally precedes child death. The provider may still
    // have an in-flight request, but the closed session must already be
    // unauthorized by the time teardown starts.
    drop(mcp_session);
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
    // Pi can resume on its own wire, but the end-to-end design is not done:
    // family resume stays refused deliberately, not by accident. The fact is
    // the impls' `resumable()`; `resume_refusal()` is only the wording of
    // the refusal, so the decision is never expressible in two places.
    let family = provider::catalog_registry().provider_for_kind(&record.kind);
    if !family.resumable() {
        return Err(cannot_resume(family.resume_refusal()));
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
pub(crate) fn insert_test_live_agent(
    registry: &SessionRegistry,
    id: &str,
    owner: OwnerId,
) -> Arc<SessionRuntime> {
    tests::insert_live_agent(registry, id, owner)
}

/// Test-only live agent of one explicit kind (S9): the door reads origin, not
/// kind, so a pi/Codex-kind caller must meet exactly the judgment an ACP-kind
/// caller meets. Delegates to the same helper as the default insert.
#[cfg(test)]
pub(crate) fn insert_test_live_agent_with_kind(
    registry: &SessionRegistry,
    id: &str,
    owner: OwnerId,
    kind: SessionKind,
) -> Arc<SessionRuntime> {
    tests::insert_live_agent_with_kind_and_writer(
        registry,
        id,
        owner,
        kind,
        Box::new(tests::FailingWriter) as Box<dyn Write + Send>,
    )
}

/// One test-only live agent whose delivered bytes a test can read back: the
/// shape the broker-level attribution tests observe the envelope through.
#[cfg(test)]
pub(crate) fn insert_test_live_agent_with_recording_writer(
    registry: &SessionRegistry,
    id: &str,
    owner: OwnerId,
    kind: SessionKind,
) -> Arc<Mutex<Vec<u8>>> {
    let received = Arc::new(Mutex::new(Vec::new()));
    tests::insert_live_agent_with_kind_and_writer(
        registry,
        id,
        owner,
        kind,
        Box::new(tests::RecordingWriter(Arc::clone(&received))),
    );
    received
}

#[cfg(test)]
impl SessionRegistry {
    /// One test-only live agent session that is `creator`'s child, with a
    /// display name — the shape `devboule_answer_permission`'s chain checks.
    pub(crate) fn insert_test_child(
        &self,
        id: &str,
        owner: OwnerId,
        creator: &str,
    ) -> Arc<SessionRuntime> {
        let runtime = tests::insert_live_agent(self, id, owner);
        {
            let mut map = self.inner.lock().expect("registry");
            let live = map
                .get_mut(id)
                .and_then(RegistryEntry::as_peer_visible_mut)
                .expect("live entry");
            live.metadata.created_by = Some(creator.to_string());
            live.metadata.display_name = Some("child".to_string());
        }
        runtime
    }

    /// Test-only: overwrite one live row's stored origin, so an out-of-module
    /// test can drive the tool door as a peer's agent.
    pub(crate) fn set_test_origin(&self, session_id: &str, origin: SessionOrigin) {
        let mut map = self.inner.lock().expect("registry");
        let live = map
            .get_mut(session_id)
            .and_then(RegistryEntry::as_peer_visible_mut)
            .expect("live entry");
        live.metadata.origin = origin;
    }

    /// Test-only: park one permission card on a live session's broker, so an
    /// out-of-module test can answer one.
    pub(crate) fn test_park_card(&self, session_id: &str, card_id: &str) {
        let runtime = self.runtime(session_id).expect("runtime");
        let broker = runtime.permission_broker().expect("broker");
        broker
            .register(1, permission_broker::permission(card_id), &runtime)
            .expect("the card parks");
    }

    /// Test-only: a live agent child of `creator` with a display name, a
    /// manifest advertising `available_modes`, a switcher the move's asks land
    /// on, and the journal row the move's recording updates — the full shape
    /// `devboule_set_agent_profile` reads and records, so an out-of-module
    /// test can drive one end to end. `model_fails` aims the switcher's model
    /// ask at a refusal, for the partial-failure path.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn insert_test_move_child(
        &self,
        id: &str,
        owner: OwnerId,
        creator: &str,
        display_name: &str,
        available_modes: &[&str],
        current_model: Option<&str>,
        model_fails: bool,
    ) -> Arc<SessionRuntime> {
        let journal = self.journal.as_ref().expect("the registry's journal");
        let (runtime, _mode_calls, _model_calls, _order) = tests::insert_move_child(
            self,
            journal,
            id,
            owner,
            creator,
            display_name,
            available_modes,
            current_model,
            true,
            false,
            model_fails,
        );
        runtime
    }
}

/// One test-only live agent session with a writer of the caller's choosing.
///
/// `insert_test_live_agent` deliberately carries a writer that fails, which is
/// what a test about a *write* failure wants. A test that needs the session to
/// accept a prompt (the `AgentMessageSend` receipt path, for one) needs the
/// other half.
#[cfg(test)]
pub(crate) fn insert_test_live_agent_with_writer(
    registry: &SessionRegistry,
    id: &str,
    owner: OwnerId,
    kind: SessionKind,
    writer: Box<dyn Write + Send>,
) -> Arc<SessionRuntime> {
    tests::insert_live_agent_with_kind_and_writer(registry, id, owner, kind, writer)
}
