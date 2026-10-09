//! The registry's standing vocabulary: the budget and coalesce constants,
//! session-id minting, the PTY traits and reader plumbing, resume metadata,
//! the authorization doors, attachment and prompt planning, registry-state
//! types, and the message-brake and creation tables.
//!
//! A child of `session` because its items work on the registry's state, and
//! `SessionRegistry` itself stays in the parent so its private fields stay
//! visible to every sibling impl. Every line below this header is
//! byte-identical to its text in `session.rs`, apart from the `pub(super)`
//! markers the parent, its sibling modules or its tests reach in for.

use super::*;

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
/// pull moves no unbounded batch into connection-local state — the one
/// bounded exception is a replay's two-event tail completing a full round.
pub(super) const PULL_BATCH: usize = 16;

pub(super) const READ_CHUNK: usize = 16 * 1024;
pub(super) const INITIAL_COLS: u16 = 120;
pub(super) const INITIAL_ROWS: u16 = 32;
pub(super) const READER_JOIN_BUDGET: Duration = Duration::from_millis(150);

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
pub(super) const EXIT_DRAIN: Duration = Duration::from_millis(200);

/// Five minutes separates a real thinking pause from a session that deserves
/// a liveness warning. A shorter threshold would turn normal terminal pauses
/// into noise and make the signal less trustworthy.
pub const SESSION_SILENCE_THRESHOLD: Duration = Duration::from_secs(300);
/// Shared OS liveness sweeper interval. Under the 5 s UI bound: a Task
/// Manager kill is observed on the next WaitForSingleObject(0) pass.
pub const SESSION_OS_SWEEP_INTERVAL: Duration = Duration::from_secs(2);

static SESSION_COUNTER: AtomicU64 = AtomicU64::new(1);

/// Per-daemon-process entropy, drawn once per process, mixed into every
/// minted session id's unique component.
///
/// Why not seed the counter from the journal instead: rows can be deleted
/// and trimmed, and an id outlives its row — a deleted session's attachments,
/// peer references and artefacts still carry it — so "no row holds this id"
/// does not mean "no id ever meant this". A fresh process cannot reproduce
/// another process's nonce, which is the property the unique component
/// needs; the counter keeps ids short and ordered within one life.
static SESSION_NONCE: OnceLock<u64> = OnceLock::new();

pub(super) fn session_nonce() -> u64 {
    *SESSION_NONCE.get_or_init(draw_session_nonce)
}

fn draw_session_nonce() -> u64 {
    let mut bytes = [0u8; 8];
    if getrandom::fill(&mut bytes).is_ok() {
        return u64::from_le_bytes(bytes);
    }
    // The OS entropy source refused. Degrade to time and pid rather than
    // refuse sessions over eight bytes: still per-process, weaker only
    // against a clock set backwards between restarts.
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0);
    nanos ^ (u64::from(std::process::id()) << 32)
}

/// The unique half of a session id: the process's nonce and the counter.
/// The nonce is what a restart changes, so ids from two lives of the daemon
/// cannot meet even when both counters start over.
pub(super) fn session_unique(process_nonce: u64, counter: u64) -> String {
    // Guard in the mint, not surprise at composition: past 15 hex digits the
    // unique passes the 32-char budget (2^60 creates in one process — never;
    // at 1M creates/s that is 36,000 years) and compose refuses confusingly.
    // Debug-only is proportionate: release keeps the infallible mint, tests
    // fail loudly if the shape ever drifts past budget.
    debug_assert!(
        counter <= 0xFFF_FFFF_FFFF_FFFF,
        "counter {counter} passes the session id budget; restart the daemon for fresh ids"
    );
    format!("{counter:08x}-{process_nonce:016x}")
}

/// The unique component of every session id this process mints.
pub(super) fn mint_session_unique() -> String {
    session_unique(
        session_nonce(),
        SESSION_COUNTER.fetch_add(1, Ordering::Relaxed),
    )
}

/// Test-only face of [`session_unique`], so the attachment store's folder-name
/// test tracks the minter instead of copying its spelling. `#[cfg(test)]`
/// keeps it out of release builds entirely; production callers use the mint.
#[cfg(test)]
pub(crate) fn session_unique_for_test(process_nonce: u64, counter: u64) -> String {
    session_unique(process_nonce, counter)
}

/// The transport-specific ACP module supplies these three small adapters;
/// the registry, runtime, coalescer, journal and attachment code stay shared.
pub(crate) trait SessionKiller: Send + Sync {
    fn kill(&mut self);
    /// Interrupt the current turn without killing the session. The default
    /// no-op covers killers whose transport has no turn concept (pty).
    fn interrupt(&mut self) {}
    /// Interrupt while no turn is running. The default is the same
    /// interrupt; a provider whose aborted result can outlive its turn
    /// (Claude's stale abort) overrides this to skip expecting one, because
    /// no result ever answers the interrupt of a turn that does not exist —
    /// expected, it would swallow the next run's only completion.
    fn interrupt_idle(&mut self) {
        self.interrupt();
    }
    fn clone_killer(&self) -> Box<dyn SessionKiller>;
}

pub(crate) trait SessionSteerer: Send + Sync {
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
    ///
    /// `origin` states, from the ingress, whose words these are: only a
    /// person's own send may dismiss what a person is being asked
    /// (permission cards), so an adapter that supersedes cards on a steer
    /// does it for [`SteerOrigin::Person`] alone.
    fn steer_active_turn(
        &mut self,
        _text: &str,
        _turn: &mut TurnToken<'_>,
        _origin: SteerOrigin,
    ) -> Result<bool, WireError> {
        Ok(false)
    }
    fn clone_steerer(&self) -> Box<dyn SessionSteerer>;
}

/// Whose words a delivery carries, stated explicitly at the ingress and
/// never inferred from the shape of the call: the local composer and the
/// paired device's composer are a person; an agent-to-agent message and a
/// daemon-authored prompt are not.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SteerOrigin {
    /// A person's own send: it may dismiss permission cards.
    Person,
    /// An agent-to-agent message or a daemon-authored prompt: it may not.
    Agent,
}

pub(super) struct UnsupportedSteerer;

impl SessionSteerer for UnsupportedSteerer {
    fn clone_steerer(&self) -> Box<dyn SessionSteerer> {
        Box::new(Self)
    }
}

/// A provider's own side-effect commands, dispatched before a turn starts.
/// Translated from Paseo's agent-prompt
/// (packages/server/src/server/agent/agent-prompt.ts), which calls
/// `tryRunOutOfBand` in agent-manager
/// (packages/server/src/server/agent/agent-manager.ts): the text is a command
/// the provider executes itself — never written as a prompt, and no turn
/// begun for it. `handles_out_of_band` recognises the text; `run_out_of_band`
/// performs it, and a caller runs it only after recording the input.
pub(crate) trait OutOfBandCommands: Send + Sync {
    fn handles_out_of_band(&self, text: &str) -> bool;
    fn run_out_of_band(&self, text: &str, runtime: &Arc<SessionRuntime>);
}

pub(crate) trait ModelSwitcher: Send + Sync {
    fn set_model(&self, model_id: Option<&str>, effort: Option<&str>) -> Result<(), WireError>;
    /// The same switch with the serving provider named (pi: openrouter,
    /// opencode-go, …). Providers without a per-model serving dimension
    /// ignore it; pi resolves the pair, falling back to the bare id for
    /// profiles older builds wrote.
    fn set_model_full(
        &self,
        provider: Option<&str>,
        model_id: Option<&str>,
        effort: Option<&str>,
    ) -> Result<(), WireError> {
        let _ = provider;
        self.set_model(model_id, effort)
    }
    fn set_mode(&self, _mode_id: &str) -> Result<(), WireError> {
        Err(WireError::new(
            ErrorCode::InvalidRequest,
            "This provider does not support switching the session mode.",
        ))
    }
    fn set_feature(&self, _feature_id: &str, _enabled: bool) -> Result<(), WireError> {
        Err(WireError::new(
            ErrorCode::InvalidRequest,
            "This provider does not support runtime features.",
        ))
    }
    fn feature_state(&self, _feature_id: &str) -> Option<SessionEvent> {
        None
    }
    fn manifest(&self) -> Option<SessionEvent> {
        None
    }
    fn reports_mode_from_provider(&self) -> bool {
        false
    }
    fn clone_switcher(&self) -> Box<dyn ModelSwitcher>;
    fn clone_steerer(&self) -> Box<dyn SessionSteerer> {
        Box::new(UnsupportedSteerer)
    }
}

pub(super) trait WaitableChild: Send {
    fn wait(self: Box<Self>) -> Option<u32>;
}

pub(crate) struct StdioWaitableChild {
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
            TransportClosed(label),
        ));
    };
    stdin.write_all(bytes)?;
    stdin.flush()
}

/// A child's stdin that is already closed, so the write never started and not
/// one byte of the prompt reached the child.
///
/// It rides inside the [`std::io::Error`] every write road already reports with,
/// because that is the type a plain `Write` implementation can carry and the
/// one the ACP writer's buffered flush hands back. Whether it is there decides
/// whether a caller that owns the message may offer it again, so it is read as
/// the marker it is and never as the prose around it.
#[derive(Debug)]
pub(super) struct TransportClosed(&'static str);

impl std::fmt::Display for TransportClosed {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{} stdin is closed", self.0)
    }
}

impl std::error::Error for TransportClosed {}

/// Whether this write failed because the transport was already closed — a
/// failure that happened before any byte went out.
pub(super) fn transport_was_closed(error: &std::io::Error) -> bool {
    error
        .get_ref()
        .and_then(|cause| cause.downcast_ref::<TransportClosed>())
        .is_some()
}

/// What one write to a child cost, split by what it says about delivery.
///
/// A closed transport took no byte, so a caller that owns the message — the
/// queue does — may offer it again and nothing is said twice. Every other
/// failure may have written part of the prompt, and offering it again may. The
/// split is made where the failing write still is an [`std::io::Error`], and
/// `describe` writes the prose the wire carries, so each road keeps the
/// sentence its caller has always read.
#[derive(Debug)]
pub(crate) enum WriteAttempt {
    Refused(WireError),
    Uncertain(WireError),
}

impl WriteAttempt {
    /// One transport failure, placed by whether the transport was closed.
    pub(crate) fn of(error: std::io::Error, describe: fn(&std::io::Error) -> WireError) -> Self {
        let wire = describe(&error);
        if transport_was_closed(&error) {
            Self::Refused(wire)
        } else {
            Self::Uncertain(wire)
        }
    }
}

impl WriteAttempt {
    /// The same failure as the `io::Error` a `Write` hands back, so a flush
    /// that wrapped a classified write keeps the classification across it: a
    /// refused attempt carries the closed-transport marker for `label`.
    pub(crate) fn into_io(self, label: &'static str) -> std::io::Error {
        match self {
            Self::Refused(_) => {
                std::io::Error::new(std::io::ErrorKind::BrokenPipe, TransportClosed(label))
            }
            Self::Uncertain(wire) => std::io::Error::other(wire.message),
        }
    }
}

impl From<WriteAttempt> for WireError {
    fn from(attempt: WriteAttempt) -> Self {
        match attempt {
            WriteAttempt::Refused(error) | WriteAttempt::Uncertain(error) => error,
        }
    }
}

pub(crate) trait ReaderDispatch: Send {
    fn feed(&mut self, bytes: &[u8], runtime: &Arc<SessionRuntime>) -> Result<(), String>;
    fn finish(&mut self, runtime: &Arc<SessionRuntime>);
}

pub(crate) trait StderrSource: Send {
    fn spawn(self: Box<Self>, runtime: Arc<SessionRuntime>) -> std::io::Result<JoinHandle<()>>;
}

/// The registry owns this value; the reader and command paths keep Arcs to
/// the endpoints/runtime they need after releasing the map lock.
pub(super) struct PtySession {
    pub(super) metadata: Session,
    pub(super) owner: OwnerId,
    pub(super) process_job: Arc<JobObject>,
    pub(super) master: Option<Arc<Mutex<Box<dyn MasterPty + Send>>>>,
    pub(super) killer: Box<dyn SessionKiller>,
    pub(super) steerer: Box<dyn SessionSteerer>,
    pub(super) switcher: Option<Box<dyn ModelSwitcher>>,
    /// This is separate from the stdout reader: stderr must never be able to
    /// fill its pipe and stop the ACP child from producing responses.
    pub(super) stderr_handle: Option<JoinHandle<()>>,
    pub(super) child_wait: Option<JoinHandle<Option<u32>>>,
    pub(super) writer: Arc<Mutex<Box<dyn Write + Send>>>,
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
    pub(super) image_sink: Option<Arc<AcpPromptSink>>,
    /// Structured prompt route for a provider whose protocol carries images
    /// but whose handshake says nothing the daemon reads (Claude, Codex,
    /// Pi). `Some` only for those three sessions: the ACP route has the
    /// sibling above, and a terminal session leaves this `None`. The route
    /// owns the decision, the text and the frame for one prompt (see
    /// [`StaticImageSink`] and [`PlannedStaticPrompt`]): the send path plans
    /// it outside the writer lock and sends it under that hold, the shape the
    /// ACP sibling above already uses.
    pub(super) static_image_sink: Option<Arc<dyn StaticImageSink>>,
    /// The same side-effect command seam the spawn carries, next to the
    /// routes a send consults. `Some` only for the family that has such
    /// commands (pi and Codex).
    pub(super) out_of_band: Option<Arc<dyn OutOfBandCommands>>,
    pub(super) reader_handle: Option<JoinHandle<()>>,
    pub(super) coalesce_handle: Option<JoinHandle<()>>,
    pub(super) runtime: Arc<SessionRuntime>,
    pub(super) mcp_session: Option<McpSessionGuard>,
    pub(super) exited: Arc<AtomicBool>,
    /// Set by `stop`: the process dies but the session object stays. The
    /// reader must not remove the registry entry or call session_finished.
    pub(super) preserve_on_exit: Arc<AtomicBool>,
}

pub(crate) struct SpawnedSession {
    pub(super) process_job: JobObject,
    pub(super) master: Option<Arc<Mutex<Box<dyn MasterPty + Send>>>>,
    pub(super) killer: Box<dyn SessionKiller>,
    pub(super) switcher: Option<Box<dyn ModelSwitcher>>,
    pub(super) child: Box<dyn WaitableChild>,
    pub(super) writer: Arc<Mutex<Box<dyn Write + Send>>>,
    /// Structured prompt route for ACP image blocks; `None` for the other
    /// three providers and for terminal sessions. Carried through spawn so
    /// `start_spawned_session` can install it next to `writer`.
    pub(super) image_sink: Option<Arc<AcpPromptSink>>,
    /// Structured prompt route for the three providers the daemon statically
    /// knows carry images (Claude, Codex, Pi); `None` for an ACP session and
    /// for a terminal. Carried through spawn so `start_spawned_session` can
    /// install it next to `image_sink`.
    pub(super) static_image_sink: Option<Arc<dyn StaticImageSink>>,
    /// The same side-effect command seam the spawn carries, next to the
    /// routes a send consults. `Some` only for the family that has such
    /// commands (today: pi).
    pub(super) reader: Box<dyn Read + Send>,
    /// ACP supplies a structured decoder. Terminal sessions use the shared
    /// byte coalescer, which is constructed by `start_spawned_session`.
    pub(super) reader_dispatch: Option<Box<dyn ReaderDispatch>>,
    pub(super) stderr: Option<Box<dyn StderrSource>>,
    pub(super) permission_broker: Option<Arc<permission_broker::PermissionBroker>>,
    pub(super) os_handle: Option<ProcessHandle>,
    pub(super) peer_session_id: Option<String>,
    pub(super) agent_version: Option<String>,
    /// A profile delivery the client could not apply before its session
    /// reader existed. Pi's switch is an awaited control rpc, and the only
    /// code that can deliver its answer is the reader thread this module
    /// starts, so the client hands the rpc over instead of blocking on an
    /// answer nobody can give yet. [`start_spawned_session`] runs it once
    /// that reader is live; a refusal tears the child down and fails the
    /// creation before any prompt can reach it. Every other client delivers
    /// inside its own `spawn_process` and passes `None`.
    pub(super) pending_delivery: Option<Box<dyn FnOnce() -> Result<(), WireError> + Send>>,
    /// A Codex MCP verification to run detached once the session reader is
    /// live (S7/S8). The startup never waits for it and no outcome is fatal:
    /// [`start_spawned_session`] spawns one thread that polls
    /// `mcpServerStatus/list` and flips the runtime's `ToolsState`. Present
    /// only when a carrier was installed (`Some` road); `None` — today's only
    /// road — changes nothing.
    pub(super) pending_codex_verify: Option<codex_client::CodexVerifyBundle>,
    /// A provider's side-effect commands, consulted by a send before a turn
    /// or a steer is considered. `Some` only where such commands exist (pi
    /// and Codex); every other family passes `None`.
    pub(super) out_of_band: Option<Arc<dyn OutOfBandCommands>>,
    /// The geometry the terminal road opened its PTY at, stamped by the road
    /// that called `openpty` so the emulator is born at the same grid
    /// (`start_spawned_session` reads it). `None` for the families without a
    /// PTY grid.
    pub(super) pty_size: Option<(u16, u16)>,
}

pub(super) struct PtyKiller {
    pub(super) inner: Box<dyn ChildKiller + Send + Sync>,
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

pub(super) struct PtyWaitableChild {
    pub(super) child: Box<dyn Child + Send + Sync>,
}

impl WaitableChild for PtyWaitableChild {
    fn wait(mut self: Box<Self>) -> Option<u32> {
        self.child.wait().ok().map(|status| status.exit_code())
    }
}

pub(super) struct TerminalReaderDispatch {
    pub(super) tx: Option<mpsc::Sender<Vec<u8>>>,
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

pub(super) fn elapsed_ms_since_last_life(
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
///
/// The row is borrowed, not consumed: the failed-spawn arm rebuilds a session
/// from that same row when the provider refuses the handle, so `resume` keeps
/// it for the length of the call.
pub(super) fn session_metadata_for_resume(
    session_id: &str,
    record: &SessionRecord,
    command: &PtyCommand,
    provider: String,
    peer_session_id: String,
    generation: u64,
) -> Session {
    // Read before the fields are copied out below.
    let context_id = record.context();
    let kind = record.kind.clone();
    Session {
        id: session_id.to_string(),
        workspace_id: record.workspace_id.clone(),
        cwd: Some(crate::verbatim_path::plain_path(
            &command.cwd.to_string_lossy(),
        )),
        // The record's own kind, which is the session's kind: it was decided
        // at create and journalled, and a resume does not re-decide it.
        //
        // The kind is never derived from the provider string:
        // `provider` is a **string on a row that can disagree with its own
        // kind**. `DEVBOULE_ACP_PROVIDER_ID` reaches `command.provider_id`
        // without passing the native-id strip (`acp_client.rs`), so a journal
        // row can read `kind=acp, provider=codex` — and deriving from it
        // stamped `Codex` on a session whose peer is ACP. That is not a
        // label: `start_spawned_session` installs the stamped kind on the
        // runtime, which then drives `mcp_gates_first_prompt` (skipping the
        // MCP invariant) and `event_pull`'s `is_codex` (replaying ACP
        // envelopes through the Codex view). Reading the record keeps the
        // old constant's answer for every ACP row AND stays right for a
        // future family, because that family's rows carry its own kind.
        kind,
        title: record.title.clone(),
        provider: Some(provider),
        peer_session_id: Some(peer_session_id),
        state: SessionState::Live { generation },
        elapsed_ms: Some(0),
        created_at_ms: record.created_at_ms,
        // Resume does not re-origin a session: the row keeps the device that
        // created it.
        origin: record.origin.clone(),
        // Both of these are the journal's since v10: a resumed session is the same
        // session, so it comes back under the name the human saw and with the
        // parent it was created by.
        display_name: record.display_name.clone(),
        created_by: record.created_by.clone(),
        // And so are the creation-from-profile facts, since v11: the profile it was
        // started from, the context it belongs to, the marker it was born with
        // and its labels. A resume is not a creation, so none of them is
        // re-derived here — a child born `yes` comes back `yes` even if its
        // profile has been un-ticked or edited in the meantime.
        profile_id: record.profile_id.clone(),
        context_id: Some(context_id),
        unattended: record.unattended_state,
        labels: record.labels.clone(),
        // Live under a new generation: resume-while-running is refused, so a
        // just-resumed row never offers it. Views recompute on every serve.
        resumable: false,
    }
}

pub(super) fn live_session_view(session: &PtySession) -> Session {
    let mut metadata = session.metadata.clone();
    metadata.peer_session_id = session.runtime.peer_session_id();
    // The verdict travels on the view, recomputed from the live state every
    // time: a running child never offers resume, a dead admitted one does.
    let stamp_resumable = |metadata: &mut Session| {
        metadata.resumable = provider::session_resumable(
            &metadata.kind,
            metadata.provider.as_deref(),
            metadata.peer_session_id.as_deref(),
            metadata.state.is_live(),
            // No mark can apply here: the live view's own metadata was last
            // stamped by the spawn whose success clears the mark for the
            // handle it announced, and a live entry shadows the journal row
            // on a roster read. Terminals reach this arm too, and no
            // provider refusal ever applies to them.
            None,
        );
    };
    if session.runtime.terminal_dead.load(Ordering::Acquire) {
        metadata.state = SessionState::Ended {
            generation: session.runtime.generation(),
            code: None,
            integrity: session.runtime.terminated_integrity(),
        };
        stamp_resumable(&mut metadata);
        return metadata;
    }
    let Ok(stream) = session.runtime.lock_stream() else {
        metadata.state = SessionState::Ended {
            generation: session.runtime.generation(),
            code: None,
            integrity: session.runtime.terminated_integrity(),
        };
        stamp_resumable(&mut metadata);
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
    stamp_resumable(&mut metadata);
    metadata
}

pub(super) fn process_gone() -> WireError {
    WireError::new(ErrorCode::InvalidRequest, "This terminal process is gone.")
}

/// The refusal an id-addressed peer call gets when the id names an entry
/// that is still inside its delivery window: the
/// session does not exist for its peers until the profile's delivery has
/// landed, so the honest answer is `SessionNotFound`, not "gone" — nothing
/// was ever visible to lose.
pub(super) fn not_found_while_configuring(entry: &RegistryEntry) -> WireError {
    if entry.is_configuring() {
        not_found()
    } else {
        process_gone()
    }
}

/// The door most id-addressed calls resolve their id through: the
/// entry must exist, belong to this owner, and be past its delivery window.
/// A `Configuring` entry answers `SessionNotFound` here, because the session
/// does not exist for peers until the delivery has landed (the variant's
/// own doc claims this refusal, and this door is
/// what makes the claim true rather than a per-site edit).
///
/// Two callers do not come through here: `deposit` and `read_attachment`
/// resolve with a hand-rolled `map.get` plus the ownership check, so they
/// answer about a `Configuring` session instead of refusing it
/// `SessionNotFound`. A new peer path that needs the window refused must use
/// this door rather than copying those two.
///
/// Daemon-side readers — teardown, EOF reaping, handle storage, the resume
/// guard — do not go through this door; they ask
/// `RegistryEntry::as_child_process` directly. `delete_session` cannot
/// resolve through the door either (an id absent from the map must fall
/// through to its journal-only branch), but for an entry the map holds it
/// repeats the door's answer — `Configuring` is refused `SessionNotFound`
/// there too, before its own close-first guard.
pub(super) fn peer_entry<'a>(
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

/// The observing twin of [`peer_entry`], for the read-only attach door.
pub(super) fn peer_entry_observing<'a>(
    map: &'a HashMap<String, RegistryEntry>,
    session_id: &str,
    owner: &OwnerId,
    conn_peer: &Option<ConnPeer>,
) -> Result<&'a RegistryEntry, WireError> {
    let entry = map.get(session_id).ok_or_else(not_found)?;
    check_user_owner_observing(entry, owner, conn_peer)?;
    if entry.is_configuring() {
        return Err(not_found());
    }
    Ok(entry)
}

pub(super) fn agent_message_target_entry<'a>(
    map: &'a HashMap<String, RegistryEntry>,
    session_id: &str,
    owner: &OwnerId,
    conn_peer: &Option<ConnPeer>,
) -> Result<&'a RegistryEntry, WireError> {
    let entry = map.get(session_id).ok_or_else(not_found)?;
    // Scope is decided before the delivery window: a configuring relay must
    // get the same denial as any other relay, not an existence-shaped answer.
    check_agent_message_target(entry, owner, conn_peer)?;
    if entry.is_configuring() {
        return Err(not_found());
    }
    Ok(entry)
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum AgentMessageTargetClass {
    /// The paired daemon may write into this machine's local session.
    Local,
    /// The target belongs to the authenticated caller's own peer origin.
    OwnPeer,
    /// The target belongs to a different peer and must not be relayed.
    Relay,
    /// The target is outside the authenticated caller's scope.
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum AgentMessageSourceNamespace {
    Local,
    Far,
}

/// Classify an agent-message target once for the registry, gate, and mode
/// consumers. The allowance is scoped to the user who paired the device:
/// `send` is consent to write into that user's local sessions, not into every
/// owner's sessions on this machine, and no role decides it — the verified
/// pairing user and the send grant do.
pub(super) fn classify_agent_message_target(
    entry: &RegistryEntry,
    conn_peer: &Option<ConnPeer>,
) -> AgentMessageTargetClass {
    let Some(ConnPeer::Remote {
        device_id,
        paired_by_user,
        ..
    }) = conn_peer
    else {
        return AgentMessageTargetClass::Other;
    };

    let origin = entry.origin();
    match origin.kind {
        SessionOriginKind::Peer if origin.device_id.as_deref() == Some(device_id.as_str()) => {
            AgentMessageTargetClass::OwnPeer
        }
        SessionOriginKind::Peer => AgentMessageTargetClass::Relay,
        SessionOriginKind::Local
            if paired_by_user.as_deref() == Some(entry.owner().user.as_str()) =>
        {
            AgentMessageTargetClass::Local
        }
        SessionOriginKind::Local | SessionOriginKind::Unknown => AgentMessageTargetClass::Other,
    }
}

/// The target rule for a message whose sender may live on the far daemon.
/// This is deliberately separate from [`check_user_owner`]: the shared door
/// stays strict for every other operation, while the classification above
/// gives a paired daemon's `send` capability its narrow local-target allowance.
fn check_agent_message_target(
    entry: &RegistryEntry,
    owner: &OwnerId,
    conn_peer: &Option<ConnPeer>,
) -> Result<(), WireError> {
    match classify_agent_message_target(entry, conn_peer) {
        AgentMessageTargetClass::Relay => {
            // A remote sender may not turn this daemon into a relay for a
            // different peer. The caller's authenticated connection proves
            // which peer the far sender belongs to; the frame carries no
            // device claim to compare.
            Err(unauthorized())
        }
        AgentMessageTargetClass::Local => {
            // The classification has already tied this local target to the
            // user who paired the daemon peer; that is the scope of consent.
            Ok(())
        }
        AgentMessageTargetClass::OwnPeer | AgentMessageTargetClass::Other => {
            check_user_owner(entry, owner, conn_peer)
        }
    }
}

/// The mutable half of [`peer_entry`].
pub(super) fn peer_entry_mut<'a>(
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

pub(super) fn unauthorized() -> WireError {
    WireError::new(
        ErrorCode::Unauthorized,
        "This client is not authorized to use that session.",
    )
}

/// Whether `created_by` names `creator_session_id` — the one spelling of
/// "this session is a child of that caller". The delegated answer, the
/// profile move and the stop/close scope all read it; a scope rule written
/// twice is a scope rule that will eventually disagree with itself.
pub(super) fn is_child_of(created_by: Option<&str>, creator_session_id: &str) -> bool {
    created_by == Some(creator_session_id)
}

/// The origin a create from this connection writes.
///
/// A connection with no peer identity is the person at this machine. A remote
/// one is the paired device, with the role word projected from the scope of
/// the connection that asked: stored attribution for a permission card and the
/// journal, never consulted by a later authorization decision.
pub(crate) fn session_origin_for(conn_peer: &Option<ConnPeer>) -> SessionOrigin {
    match conn_peer {
        Some(ConnPeer::Remote {
            device_id, scope, ..
        }) => SessionOrigin::peer(device_id.clone(), scope.projected_role()),
        _ => SessionOrigin::local(),
    }
}

pub(super) fn owner_from_session_id(session_id: &str, user: &str) -> Result<OwnerId, WireError> {
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
pub(super) fn check_owner(entry: &RegistryEntry, owner: &OwnerId) -> Result<(), WireError> {
    if entry.owner() == owner {
        Ok(())
    } else {
        Err(unauthorized())
    }
}

pub(super) fn check_user_owner(
    entry: &RegistryEntry,
    owner: &OwnerId,
    conn_peer: &Option<ConnPeer>,
) -> Result<(), WireError> {
    check_user_owner_with(entry, owner, conn_peer, false)
}

/// The observing door: reading a session — its transcript, its terminal
/// screen, its roster row — is not operating on it. A machine peer observes
/// the pairing user's sessions (the owner's two PCs each see the other's
/// agents), while every operation keeps the origin-only rule below. The
/// product decision is explicit: read-only opens up, writes do not.
pub(super) fn check_user_owner_observing(
    entry: &RegistryEntry,
    owner: &OwnerId,
    conn_peer: &Option<ConnPeer>,
) -> Result<(), WireError> {
    check_user_owner_with(entry, owner, conn_peer, true)
}

fn check_user_owner_with(
    entry: &RegistryEntry,
    owner: &OwnerId,
    conn_peer: &Option<ConnPeer>,
    observing: bool,
) -> Result<(), WireError> {
    match conn_peer {
        // A client-scoped peer speaks for the person who paired it: the register
        // of sessions it reaches is that user's, and only that user's. The
        // effective owner `server.rs` hands down is already that SID, so the
        // comparison here is the same one a local call makes — stated in the
        // scope branch anyway, because "the peer reaches its paired user" is a
        // rule about the connection, not a side effect of how dispatch built the
        // owner (`DESIGN-remote-agents.md` §8b A3).
        Some(ConnPeer::Remote {
            scope: crate::peer_policy::PeerScope::PairedUser,
            paired_by_user,
            ..
        }) => match paired_by_user.as_deref() {
            Some(paired) if entry.owner().user == paired => Ok(()),
            // No recorded pairing user, or another account's session: refuse.
            _ => Err(unauthorized()),
        },
        // A machine peer's scope is the *origin*, not the owner name (§8 R2):
        // the sessions it created here, and nothing else. A session this
        // device created is refused even when the owner comparison would pass,
        // because the origin is the authority A3 names.
        Some(ConnPeer::Remote {
            scope: crate::peer_policy::PeerScope::PeerDevice,
            device_id,
            paired_by_user,
            ..
        }) => {
            let origin = entry.origin();
            let own_origin = origin.kind == SessionOriginKind::Peer
                && origin.device_id.as_deref() == Some(device_id.as_str());
            // Observing also reaches the sessions of the person who paired
            // this machine here; operating does not.
            let pairing_users_session = observing
                && paired_by_user
                    .as_deref()
                    .is_some_and(|paired| entry.owner().user == paired);
            if (own_origin && entry.owner().user == owner.user) || pairing_users_session {
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

pub(super) fn check_attached(
    runtime: &SessionRuntime,
    conn: &ConnHandle,
    subscription_id: u64,
) -> Result<(), WireError> {
    runtime.is_observer(conn.id, subscription_id)
}

pub(super) fn check_resize_owner(
    runtime: &SessionRuntime,
    conn: &ConnHandle,
    subscription_id: u64,
) -> Result<(), WireError> {
    runtime.is_resize_owner(conn.id, subscription_id)
}
