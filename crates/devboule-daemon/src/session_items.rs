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
/// pull never moves an unbounded batch into connection-local state.
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
    fn steer_active_turn(
        &mut self,
        _text: &str,
        _turn: &mut TurnToken<'_>,
    ) -> Result<bool, WireError> {
        Ok(false)
    }
    fn clone_steerer(&self) -> Box<dyn SessionSteerer>;
}

pub(super) struct UnsupportedSteerer;

impl SessionSteerer for UnsupportedSteerer {
    fn clone_steerer(&self) -> Box<dyn SessionSteerer> {
        Box::new(Self)
    }
}

pub(crate) trait ModelSwitcher: Send + Sync {
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
            format!("{label} stdin is closed"),
        ));
    };
    stdin.write_all(bytes)?;
    stdin.flush()
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
pub(super) fn session_metadata_for_resume(
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
/// that is still inside its delivery window (the re-audit's P2-1): the
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
/// does not exist for peers until the delivery has landed (the re-audit's
/// P2-1/P2-2 — the variant's own doc claims this refusal, and this door is
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
/// consumers. The daemon allowance is scoped to the user who paired the
/// device: `send` is consent to write into that user's local sessions, not
/// into every owner's sessions on this machine.
pub(super) fn classify_agent_message_target(
    entry: &RegistryEntry,
    conn_peer: &Option<ConnPeer>,
) -> AgentMessageTargetClass {
    let Some(ConnPeer::Remote {
        device_id,
        role,
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
            if *role == PeerRole::Daemon
                && paired_by_user.as_deref() == Some(entry.owner().user.as_str()) =>
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
pub(super) fn with_attachment_paths(
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
pub(super) fn push_reference_path_lines(prompt: &mut String, reference_paths: &[PathBuf]) {
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
pub(super) fn resolve_attachment_references(
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
    pub(super) fn from_stored_file(
        path: &std::path::Path,
        mime_type: &str,
    ) -> Result<Self, std::io::Error> {
        use base64::Engine;
        let bytes = std::fs::read(path)?;
        Ok(Self {
            mime_type: mime_type.to_string(),
            data_base64: base64::engine::general_purpose::STANDARD.encode(&bytes),
        })
    }

    pub(super) fn to_content_block(&self) -> serde_json::Value {
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
pub(super) fn plan_structured_prompt(
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
    pub(super) fn new(transport: &Arc<acp_client::AcpTransport>) -> Self {
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
pub(super) fn prompt_text_with_fallback_paths(text: &str, fallback_paths: &[PathBuf]) -> String {
    if fallback_paths.is_empty() {
        return text.to_string();
    }
    let mut prompt = String::from(text);
    prompt.push_str("\n\n");
    push_path_lines(&mut prompt, fallback_paths);
    prompt
}

pub(super) type TransitionSink = Arc<dyn Fn(OwnerId) + Send + Sync>;
pub(super) type JournalRosterCache = Arc<Mutex<Option<(u64, Vec<SessionRecord>)>>>;

/// One client answer to a pending permission request.
pub struct PermissionResponse<'a> {
    pub session_id: &'a str,
    pub request_id: &'a str,
    pub outcome: PermissionOutcome,
    pub option_id: Option<&'a str>,
}

pub(super) const WORKSPACE_PATH_CACHE_CAP: usize = 1024;

#[derive(Default)]
pub(super) struct WorkspacePathCache {
    pub(super) entries: HashMap<String, (PathBuf, u64)>,
    clock: u64,
}

impl WorkspacePathCache {
    fn next_stamp(&mut self) -> u64 {
        self.clock = self.clock.wrapping_add(1);
        self.clock
    }

    pub(super) fn get(&mut self, workspace_id: &str) -> Option<PathBuf> {
        let path = self
            .entries
            .get(workspace_id)
            .map(|(path, _)| path.clone())?;
        let stamp = self.next_stamp();
        self.entries
            .insert(workspace_id.to_string(), (path.clone(), stamp));
        Some(path)
    }

    pub(super) fn insert(&mut self, workspace_id: String, path: PathBuf) {
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

    pub(super) fn remove(&mut self, workspace_id: &str) {
        self.entries.remove(workspace_id);
    }
}

#[cfg(test)]
pub(super) type JournalRosterAfterListHook = Arc<dyn Fn() + Send + Sync>;

/// Runs between the brake admission and the delivery of an agent message (S4-10).
/// Test-only: it is the only way to land a turn's end inside that gap.
#[cfg(test)]
pub(super) type AgentMessageAfterAdmissionHook = Arc<dyn Fn() + Send + Sync>;

/// Runs between a deposit's ownership check and the store write (HND-01).
/// Test-only: it is the only way to land a close inside that gap.
#[cfg(test)]
pub(super) type DepositAfterOwnershipHook = Arc<dyn Fn() + Send + Sync>;

#[derive(Clone)]
pub(super) struct ConnectionPresence {
    pub(super) user: String,
    pub(super) focused_session_id: Option<String>,
    pub(super) app_visible: bool,
}

pub(crate) struct MessageBrake {
    pub(super) outstanding: Vec<OutstandingMessage>,
    pub(super) recipients: Vec<Recipient>,
    pub(super) next_slot: u64,
    pub(super) window_started: Instant,
    pub(super) sent_in_window: u32,
}

/// One message that was admitted and has not reached its boundary yet.
pub(super) struct OutstandingMessage {
    pub(super) slot: u64,
    pub(super) sent_at: Instant,
    /// The session this message was sent to: the target whose turn end releases
    /// the slot, and the name the recipient window counts.
    pub(super) to_session: String,
    /// Set once the delivery has returned — the text is in the provider's hands,
    /// or the delivery failed. A boundary that is already reached releases the
    /// slot as soon as this is set.
    pub(super) delivered: bool,
    /// Set when the boundary arrived while the delivery was still in flight.
    ///
    /// The slot stays counted until then: releasing it at the boundary would let
    /// the next message through while this one is still being written, which is
    /// exactly what the outstanding count is there to prevent (A2-05).
    pub(super) boundary_reached: bool,
    /// Where this slot's release arrives: the target runtime whose turn end
    /// releases it, and the id of the one-shot hook registered on it. `None`
    /// once the hook has fired or been unregistered again.
    pub(super) release: Option<(Weak<SessionRuntime>, u64)>,
}

/// One recipient inside the sliding window.
pub(super) struct Recipient {
    pub(super) session_id: String,
    pub(super) sent_at: Instant,
}

/// At most this many messages may be in flight from one sender.
pub(super) const MAX_MESSAGE_OUTSTANDING: usize = 5;
/// At most this many distinct recipients may be reached inside the recipient
/// window.
pub(super) const MAX_MESSAGE_RECIPIENTS: usize = 3;
/// At most this many messages may leave one sender inside the rate window.
pub(super) const MAX_MESSAGE_SENT_PER_WINDOW: u32 = 5;
/// The rate window: the brief's one second, unchanged by this fix.
pub(super) const MESSAGE_RATE_WINDOW: Duration = Duration::from_secs(1);
/// How long one in-flight message may hold a sender's slot, and how long a
/// recipient stays inside the recipient window. A target that never ends a turn
/// — or never starts one — must not park a sender's budget forever.
pub(super) const MESSAGE_SLOT_EXPIRY: Duration = Duration::from_secs(60);

impl MessageBrake {
    pub(super) fn new() -> Self {
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
    pub(super) fn prune(&mut self, now: Instant) -> Vec<(Weak<SessionRuntime>, u64)> {
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

    pub(super) fn holds_recipient(&self, session_id: &str) -> bool {
        self.recipients
            .iter()
            .any(|recipient| recipient.session_id == session_id)
    }

    /// Remove one slot, answering with its still-armed hook so the caller can
    /// unregister it.
    pub(super) fn take_slot(&mut self, slot: u64) -> Option<(Weak<SessionRuntime>, u64)> {
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
    pub(super) fn drop_recipient_if_idle(&mut self, session_id: &str, now: Instant) {
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
    pub(super) fn is_idle(&self) -> bool {
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
    pub(super) sweeps: u64,
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
    pub(super) fn sweep_is_due(&self, now: Instant) -> bool {
        self.last_sweep
            .is_none_or(|last| now.saturating_duration_since(last) >= MESSAGE_RATE_WINDOW)
    }

    pub(super) fn note_sweep(&mut self, now: Instant) {
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
pub(super) enum CreationGate {
    Closed,
    Pending,
    Open,
}

/// What one creator session's budget currently holds.
pub(super) struct AgentCreatorCaps {
    /// Children that exist.
    pub(super) live_children: usize,
    /// Children this creator has reserved and not yet committed or abandoned:
    /// the slot is taken *before* the card is raised, so two creations racing
    /// on one session cannot both see the third slot free.
    /// The reservations in flight, by id, each naming the child session id it
    /// reserved (audit S5B-02). The id is the identity: releasing one is a
    /// removal that answers whether it was there, so a failure handled on two
    /// paths cannot subtract a neighbour's creation.
    pub(super) in_flight: BTreeMap<u64, String>,
    window_started: Instant,
    pub(super) creations_in_window: u32,
    /// The once-per-creator-session accept (`S5` decision 4, S5-06). It lives
    /// exactly as long as this entry does, and it is read and written only
    /// under this table's lock so two creations racing on one session cannot
    /// both be told to ask.
    pub(super) gate: CreationGate,
    /// Set when the creator session is gone: the entry then lives until its
    /// last child finishes, because that is what releases the daemon-wide
    /// count.
    pub(super) creator_gone: bool,
}

impl AgentCreatorCaps {
    pub(super) fn new(now: Instant) -> Self {
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
    pub(super) fn held(&self) -> usize {
        self.live_children + self.in_flight.len()
    }

    /// Roll the window if it has expired. Called on every admission *and* on
    /// the sweep, so the count a caller reads is never one window stale.
    pub(super) fn roll_window(&mut self, now: Instant) {
        if now.saturating_duration_since(self.window_started) >= CREATION_WINDOW {
            self.window_started = now;
            self.creations_in_window = 0;
        }
    }
}

/// One child, as its creator's bookkeeping sees it.
pub(super) struct AgentChild {
    pub(super) creator: String,
    /// Whether the creator asked to be told (the tool's `notifyOnFinish`).
    pub(super) notify: bool,
    /// Whether the session behind this link was actually started (audit
    /// S5B-04). The link is registered when the reservation is taken — before
    /// the spawn — so a child that exits on the instant cannot outrun the row
    /// that catches its end; the child counts against its creator only once
    /// the spawn returned a session.
    pub(super) started: bool,
    /// The `input_required` notice is owed until it has been sent once
    /// (`S5` §3): one notice per child, not one per card.
    pub(super) notice_owed: bool,
    /// The finish report is owed until it has been written once. This is what
    /// makes the report idempotent across the three paths that can observe the
    /// same end (a finished turn, a process exit, a close).
    pub(super) report_owed: bool,
    /// The quiet notice is owed until it has been sent once per quiet spell.
    /// Cleared when the child publishes again, so one spell is one notice.
    pub(super) quiet_notified: bool,
}

/// One parked child end (audit-2 §2): what the end path still had in hand when
/// the child was gone. Any slot may be `None`; a unit test drives that shape,
/// and the type is named so the table and the commit read as one thing.
pub(super) type DeferredChildEnd = (
    Option<Session>,
    Option<Arc<SessionRuntime>>,
    Option<OwnerId>,
);

/// The creation budget, beside [`MessageBrakeTable`] and under the same lock
/// discipline: one mutex covers the whole table, the sweep runs at most once
/// per window, and no other lock is taken while it is held.
#[derive(Default)]
pub(crate) struct AgentCreationTable {
    pub(super) creators: HashMap<String, AgentCreatorCaps>,
    pub(super) children: HashMap<String, AgentChild>,
    /// Children an agent's creation has spawned but not committed yet
    /// (audit-2 §2): their end waits instead of running against a link that
    /// does not exist yet.
    pub(super) pending_children: HashMap<String, (Instant, u64)>,
    /// Ends that arrived while their child was still pending, kept whole
    /// (session view, runtime, owner) so the commit can run the routine the
    /// moment the link exists.
    pub(super) deferred_child_ends: HashMap<String, (DeferredChildEnd, Instant)>,
    /// The idempotency keys of creations that are in flight right now
    /// (audit S5-03): a retry that arrives while its key is here is refused
    /// without spending anything, because the first call has not answered yet.
    pending: HashMap<String, Instant>,
    /// The next reservation id (audit S5B-02). Unique for the life of the
    /// table, which is what makes a release answerable.
    pub(super) next_reservation: u64,
    pub(super) last_sweep: Option<Instant>,
    #[cfg(test)]
    sweeps: u64,
}

impl AgentCreationTable {
    pub(super) fn sweep_is_due(&self, now: Instant) -> bool {
        self.last_sweep
            .is_none_or(|last| now.saturating_duration_since(last) >= CREATION_WINDOW)
    }

    /// Drop the entries that can no longer say anything: a creator whose
    /// session is gone and whose children have all finished.
    ///
    /// The window is rolled unconditionally (every entry, whatever its age) so
    /// a table that is swept once an hour still reports this hour's count.
    pub(super) fn sweep(&mut self, now: Instant) {
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
    pub(super) fn live_agent_sessions(&self) -> usize {
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
    pub(super) fn begin_creation(&mut self, key: &str, now: Instant) -> bool {
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
    pub(super) fn end_creation(&mut self, key: &str) {
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
    pub(super) sessions: &'a SessionRegistry,
    pub(super) key: Option<String>,
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
    pub(super) registry: &'a SessionRegistry,
    pub(super) creator: String,
    pub(super) reservation: u64,
    /// The child session id reserved for this creation (audit S5B-04).
    pub(super) child: String,
    pub(super) card_owed: bool,
    pub(super) committed: bool,
    pub(super) caps: devboule_protocol::CreateAgentCaps,
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
    /// The profile's tool overlay, which the broker consults per session.
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
    /// Who authored this prompt and what part it plays in the target transcript.
    /// Required so every caller states both facts independently.
    pub author: UserMessageAuthor,
    pub message_kind: UserMessageKind,
}

/// What one delivery needs to re-key its brake slot (S4-10): the table, the
/// sender's key in it, and the slot.
pub(crate) struct MessageSlotRef<'a> {
    pub(crate) brakes: &'a Arc<Mutex<MessageBrakeTable>>,
    pub(crate) brake_key: &'a str,
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
