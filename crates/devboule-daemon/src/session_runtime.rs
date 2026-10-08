//! Session stream runtime: emulator, attach, journal, permission delivery.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicU8, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use devboule_protocol::{
    cursor_replay_ok, AgentActivityState, AgentTaskItem, AttachmentReference, Attention,
    AttentionReason, Cursor, ErrorCode, NoticeSeverity, QueuedMessage, Session, SessionEvent,
    SessionEventEnvelope, SessionKind, SessionModel, SessionOrigin, SessionResumeInfo, SessionTask,
    TranscriptIntegrity, UserMessageAuthor, UserMessageKind, WireError,
};

use super::permission_broker::PermissionBroker;
use super::session_types::{
    Attachment, AttachmentKey, Disposition, OutputMetrics, PendingItem, Scrollback, StreamState,
};
use super::{
    internal, process_gone, ConnHandle, EXIT_DRAIN, INITIAL_COLS, INITIAL_ROWS,
    PENDING_OUTPUT_BUDGET_BYTES, PENDING_OUTPUT_BUDGET_FRAMES, SESSION_SILENCE_THRESHOLD,
};
use crate::journal::{output_record, Journal, Replay};
use crate::origin_chain::Chain;
use crate::outbound::ConnOut;
use crate::process_tree::ProcessHandle;
use crate::screen::{Screen, ScreenSnapshot};

/// The byte a published status is remembered as. Written as a match beside
/// [`AgentActivityState`] rather than a cast: four states and a sentinel, and
/// the compiler checks the arms.
fn activity_code(state: AgentActivityState) -> u8 {
    match state {
        AgentActivityState::Idle => 0,
        AgentActivityState::Working => 1,
        AgentActivityState::Blocked => 2,
        AgentActivityState::Unknown => 3,
    }
}

fn integrity_counters(integrity: TranscriptIntegrity) -> (u64, u64) {
    match integrity {
        TranscriptIntegrity::Complete => (0, 0),
        TranscriptIntegrity::Truncated {
            dropped_frames,
            dropped_bytes,
            ..
        }
        | TranscriptIntegrity::Unverifiable {
            dropped_frames,
            dropped_bytes,
            ..
        } => (dropped_frames, dropped_bytes),
    }
}

fn remove_replayed_agent_items(
    queue: &mut VecDeque<PendingItem>,
    from_seq: u64,
    replayed_seqs: &HashSet<u64>,
    replace_manifest: bool,
) {
    queue.retain(|item| match item {
        PendingItem::Agent { seq, event, .. } => {
            let replayed = seq.is_some_and(|seq| seq <= from_seq || replayed_seqs.contains(&seq));
            let drop_manifest =
                replace_manifest && matches!(event, SessionEvent::SessionManifest { .. });
            !replayed && !drop_manifest
        }
        PendingItem::Output { .. } | PendingItem::Snapshot { .. } => true,
    });
}

fn agent_queue_extent(queue: &VecDeque<PendingItem>) -> (usize, u64) {
    let bytes = queue
        .iter()
        .filter_map(|item| match item {
            PendingItem::Agent { bytes, .. } => Some(*bytes),
            PendingItem::Output { data, .. } => Some(data.len()),
            PendingItem::Snapshot { .. } => None,
        })
        .sum();
    let frames = queue
        .iter()
        .filter(|item| !matches!(item, PendingItem::Snapshot { .. }))
        .count() as u64;
    (bytes, frames)
}

/// The A2A state word for one roster row (`S5` §1).
///
/// A pending card outranks "working": a session parked on an answer a human
/// must give is the one fact a creator most needs to see, and it is the state
/// [`crate::session::SessionRegistry::notify_child_input_required`] reports in
/// words. A terminal row keeps its terminal word — a leftover card on a dead
/// session is not `input_required` — and a run a stop ended reads `canceled`
/// whatever the process exited with.
pub(crate) fn roster_task_state(
    session: &Session,
    runtime: &SessionRuntime,
) -> devboule_protocol::AgentTaskState {
    if !session.state.is_live() {
        if runtime.stop_requested() {
            return devboule_protocol::AgentTaskState::Canceled;
        }
        return session.state.task_state(false);
    }
    let waiting = runtime
        .permission_broker()
        .is_some_and(|broker| broker.pending_len() > 0);
    if waiting {
        return devboule_protocol::AgentTaskState::InputRequired;
    }
    session.state.task_state(runtime.is_running_turn())
}

/// One `AgentMessage`, joined from the chunks that carried one message id.
///
/// A *record*, not runtime state: the finish hook reads it and nothing else
/// touches it (`S5` decision 10).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentMessageSnapshot {
    pub(crate) message_id: Option<String>,
    pub(crate) text: String,
}

/// Stream state is one mutex on purpose. Every step that must be atomic
/// with respect to output application happens under this single hold:
/// apply-to-emulator + boundary update + per-attachment enqueue, and screen
/// capture + attachment registration. Holding it across attach registration
/// makes attach ordering exact: the subscriber is registered with its
/// snapshot already captured, and only then can the reader publish the next
/// live chunk. Subscribe-with-state, subscribe-before-live.
pub(crate) struct SessionRuntime {
    pub(crate) session_id: String,
    pub(crate) journal: Option<Arc<Journal>>,
    pub(crate) permission_broker: Option<Arc<PermissionBroker>>,
    /// Who asked for this session (§8 R2), pushed in by the registry right
    /// after the runtime is built. `OnceLock` because it is set exactly once
    /// and read from the permission broker and the peer gate; an unset lock
    /// reads as `Local`, which is what a session built by a test without an
    /// origin is.
    origin: OnceLock<SessionOrigin>,
    pub(crate) stream: Mutex<StreamState>,
    /// The PTY input side, for emulator-generated replies (DSR/CPR). Writes
    /// here are the fast path: never behind the journal, a snapshot, or a
    /// client. `None` for transcript sessions and before spawn finishes.
    pub(crate) pty_writer: OnceLock<Arc<Mutex<Box<dyn Write + Send>>>>,
    /// A failed journal write is a fact about this session, not about the
    /// daemon or a later session. It remains true for the session lifetime.
    pub(crate) journal_degraded: AtomicBool,
    /// A poisoned stream lock or terminal parser panic means the screen can
    /// no longer be trusted. Such a session is dead, not a session to recover
    /// by continuing with possibly corrupted state.
    pub(crate) terminal_dead: AtomicBool,
    /// Kept outside `stream` so a poisoned stream can still wake its viewer
    /// and deliver the degraded + exit terminal markers.
    pub(crate) attachment_notify: Mutex<HashMap<AttachmentKey, Arc<ConnOut>>>,
    pub(crate) journal_dropped_frames: AtomicU64,
    pub(crate) journal_dropped_bytes: AtomicU64,
    /// The last generation is also needed if the stream lock is poisoned
    /// before the EOF path can read its generation.
    pub(crate) generation: AtomicU64,
    pub(crate) peak_pending_bytes: AtomicUsize,
    pub(crate) coalesced_bytes: AtomicU64,
    pub(crate) coalesced_frames: AtomicU64,
    pub(crate) journal_replays: AtomicU64,
    /// Live checklist seeds that failed before installing state. A failed
    /// seed retries on the next frame; the count names the failure in
    /// diagnostics instead of leaving a silent freeze.
    pub(crate) task_seed_failures: AtomicU64,
    /// Plan-mark pre-scan reads, success or failure. A failed read notices
    /// once (see `journal_lookback`) instead of failing open silently.
    pub(crate) plan_mark_scans: AtomicU64,
    /// Runs at the start of every plan-mark scan, so a test can observe the
    /// locks held around the read.
    #[cfg(test)]
    pub(crate) plan_mark_scan_probe: Mutex<Option<Box<dyn Fn() + Send>>>,
    turn_counter: AtomicU64,
    turn_active: AtomicBool,
    /// The turn status last *published* on this session's roster row, kept so a
    /// change can be announced and a non-change stays quiet. [`SessionRuntime::
    /// activity`] is the live reading; this is what clients were told.
    published_activity: AtomicU8,
    /// The turn-hold: taken by a `Steer`'s admission (`with_active_turn`),
    /// by `begin_turn`, and by the `AgentFinished` transition (`finish_turn`).
    /// Holding it across the provider write is what makes steer admission
    /// atomic, so a turn cannot end between the check that admits a steer and
    /// the write that delivers it. A plain `Mutex<()>`: the guarded
    /// value is nothing, so a lock poisoned by a panic cannot leave state
    /// behind that a later lock would have to distrust.
    turn_hold: Mutex<()>,
    /// One-shot callbacks fired when a turn ends on this runtime. The
    /// inter-agent message brakes register here so an in-flight message slot
    /// is released at the boundary that ends it, not on some later probe of a
    /// counter.
    turn_end_hooks: Mutex<Vec<TurnEndHook>>,
    next_turn_end_hook: AtomicU64,
    pub(crate) reader_finished: AtomicBool,
    pub(crate) child_reaped: AtomicBool,
    /// Transition notifications are suppressed until spawn has inserted all
    /// runtime state, so an extremely short-lived child cannot publish an
    /// exit before the corresponding create snapshot.
    pub(crate) transition_ready: AtomicBool,
    /// The child's last `AgentMessage`, accumulated across the chunks of one
    /// message (`S5` decision 10). The finish hook reads it: the whole message
    /// is what gets deposited as the artifact and its first 4000 characters are
    /// the human-readable summary.
    agent_message: Mutex<Option<AgentMessageSnapshot>>,
    /// The last `AgentFinished` stop reason this session reported, which is
    /// what decides `completed | failed | canceled` in the finish report.
    agent_stop_reason: Mutex<Option<String>>,
    /// Set once, when a stop reaches this run before any turn end was recorded,
    /// and never cleared: the run then reads as cancelled whatever the provider
    /// reports while the killed process dies.
    stop_requested: AtomicBool,
    /// A stop was sent to the process, whatever the turn state was then. A turn
    /// that begins after it is the one the kill ends, so it takes the request.
    stop_signalled: AtomicBool,
    /// The wait thread and the post-create race check can observe the same
    /// exit. Only one of them may publish the exit transition.
    pub(crate) exit_transition_sent: AtomicBool,
    pub(crate) published_frames: AtomicU64,
    pub(crate) published_bytes: AtomicUsize,
    pub(crate) session_manifest: Mutex<Option<SessionEvent>>,
    mode_before_plan: Mutex<Option<String>>,
    claude_reported_mode: Mutex<Option<String>>,
    mcp_bearer: Mutex<Option<String>>,
    mcp_url: Mutex<Option<String>>,
    mcp_readiness: Mutex<McpReadiness>,
    mcp_ready_cvar: Condvar,
    /// The S1 tools tri-state, beside `mcp_bearer`/`mcp_url`: set at
    /// registration, flipped by verification (S8), read by roster/result
    /// paths (S2/S8). Lock discipline only in S1 — no behaviour reads it yet.
    tools_state: Mutex<crate::mcp_broker::ToolsState>,
    pub(crate) agent_kind: Mutex<Option<SessionKind>>,
    goal: Mutex<Option<String>>,
    claude_catalog_state: Mutex<crate::claude_catalog::ClaudeCatalogState>,
    /// Attention is deliberately runtime-only. It is a user's current view
    /// state, not transcript history, so it is not journaled and does not
    /// survive a daemon restart.
    pub(crate) attention: Mutex<Option<Attention>>,
    attention_hooks: Mutex<Option<AttentionHooks>>,
    /// The delegated-surfacing observer: called once per parked card, at the
    /// moment it enters the pending table. Installed by the registry at
    /// birth, beside the attention hooks; never set by the broker itself.
    permission_park_hook: Mutex<Option<PermissionParkHook>>,
    /// The task-list refresh observer: called after a provider frame that
    /// can change this session's task list, at the moment it publishes.
    /// Installed by the registry at birth, beside the park hook.
    tasks_refresh_hook: Mutex<Option<TasksRefreshHook>>,
    /// Background tool calls still owed a terminal update, oldest first.
    /// A result for any other call cannot change the task list, so only
    /// these arm the refresh — every tool result does not re-derive it.
    /// Bounded like the published list: the oldest launch leaves first.
    background_tool_calls: Mutex<VecDeque<String>>,
    /// Triggering rows deferred by the debounce, oldest first: the trailing
    /// run merges them into its derive, so a trigger inside the window is
    /// never lost to a journal that has not landed it yet. Bounded — the
    /// journal is the primary source, this covers the commit gap.
    tasks_pending_extra: Mutex<Vec<(SessionEvent, u64)>>,
    /// Task-list revisions handed out, counting from 1 per session life.
    /// The publisher drops a snapshot that is not newer than the last one
    /// it sent, so overlapping derives cannot leave a stale list behind.
    tasks_revision: AtomicU64,
    /// Last task-list derive, for the debounce window below.
    tasks_last_refresh: Mutex<Option<Instant>>,
    /// A trailing refresh is already scheduled: one per session at most, so
    /// a burst inside the window costs one extra derive, not one per event.
    tasks_trailing_pending: AtomicBool,
    /// Wall time the process was observed dead, for task end times. Set
    /// once, on the first observed death; a transcript hydrated from the
    /// journal never saw its own end and keeps `None`.
    tasks_ended_wall_ms: Mutex<Option<u64>>,
    /// Duplicated OS process handle. Queried by the shared sweeper; never a
    /// PID, which the OS may reuse after the child dies.
    pub(crate) os_handle: Mutex<Option<ProcessHandle>>,
    /// ACP registers the killer cascade here. Fired once on newly observed
    /// OS death, on a detached thread so the 2s sweeper never blocks.
    pub(crate) on_os_death: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    pub(crate) os_death_started: AtomicBool,
    /// Roster `sessions_watch` notify. ACP publish uses this so Silent→Live
    /// is not swallowed (the PTY coalescer already notifies the registry).
    pub(crate) roster_notify: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// Called once after an `AgentFinished` is published (never for any other
    /// event): the finish report's trigger.
    ///
    /// It is deliberately not the attention hook: attention is a *priority*
    /// state (`Error` 2 outranks `Finished` 1), so a child whose provider
    /// emitted one malformed line or one error before finishing keeps the
    /// higher reason and the later `Finished` raise is dropped — taking the
    /// report with it. A turn that ended is a fact about the stream, not a
    /// display state, and this hook is where the stream says so.
    pub(crate) finish_notify: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    /// Provider-side session id (ACP `sessionId`, Claude `system/init`
    /// `session_id`). Stored for resume; not the Devboule session id.
    pub(crate) peer_session_id: Mutex<Option<String>>,
    /// This session still owes the daemon its **first** prompt
    /// (`create-from-profile`).
    ///
    /// Taken rather than read (`take_first_prompt`): the human's standing
    /// instructions ride the first prompt exactly once, and two prompts racing
    /// for it cannot both carry them. `true` for a session the daemon is starting
    /// and `false` for one built from a replay, because a session that comes back
    /// with a transcript already had a first prompt — the instructions were on
    /// it, or it predates them — so a resume never re-injects them.
    first_prompt_owed: AtomicBool,
    /// The conversation a **recovered** session carries into its first prompt
    /// (`session_recovery.rs`). `None` for every session that is not a
    /// replacement for one the provider could not reopen, and taken rather
    /// than read for the same reason [`Self::first_prompt_owed`] is: two
    /// prompts racing for the first one cannot both carry it.
    ///
    /// In memory, deliberately: a daemon that dies between the recovery and
    /// the human's first line loses it, and the notice the session's
    /// transcript already carries is what says so. The alternative — re-reading
    /// the source journal's events on the dispatch thread at prompt time — puts
    /// a blocking journal read on the send path for a fact that is only ever
    /// needed once.
    recovered_context: Mutex<Option<String>>,
    /// Where the untrusted content this session has read came from
    /// (`origin_chain`), so a message it sends on names it. Written by the daemon
    /// from ids it holds, never from a body; data hops stay until the person
    /// next types to the session.
    ingress_chain: Mutex<Chain>,
    /// Bounded recent event kinds for the activity answer. Metadata only;
    /// every publish appends, the oldest drops past the cap, and no payload
    /// text is ever kept here.
    activity_feed: Mutex<VecDeque<crate::agent_activity::ActivityMark>>,
    /// Prompts whose write has begun and whose turn has not yet started:
    /// taken when the send resolves the writer under the session map lock
    /// (`session_messaging.rs`) and given back when the send returns, so the
    /// idle-close section — which takes the same lock — sees a delivery in
    /// flight instead of a child that looks idle with a prompt half-written
    /// into it.
    deliveries_in_flight: AtomicU32,
}

struct McpReadiness {
    required: bool,
    ready: bool,
    failure: Option<String>,
}

/// The hook type, named once: the closure the registry installs to be told
/// when a card parks.
type PermissionParkHook = Arc<dyn Fn(&SessionEvent) + Send + Sync>;

/// The hook type, named once: the closure the registry installs to be told
/// when this session's task list may have changed. The published event is
/// passed so the refresh can fold it explicitly — the journal write may not
/// be visible yet when the hook runs. `None` is the session's own end, which
/// carries no event: the derive reads the ended state itself.
type TasksRefreshHook = Arc<dyn Fn(Option<&SessionEvent>) + Send + Sync>;

struct AttentionHooks {
    suppressed: Arc<dyn Fn() -> bool + Send + Sync>,
    prepare: Arc<dyn Fn() -> Box<dyn FnOnce() + Send> + Send + Sync>,
}

pub(crate) struct LiveAgentReplay {
    /// The generation side of the replay's start position. A cursor at
    /// seq 0 is a fresh or reset reader (a Reopen's new cursor included):
    /// it owns nothing yet, so the replay starts at generation 1. A cursor
    /// with seq > 0 bounds the replay to the current generation from that
    /// seq — it bounds the read, and does not certify the reader received
    /// the skipped rows: a lagging journal write can leave rows neither the
    /// replay nor the live queue reach, which no later cursor gets back.
    pub(crate) from_generation: u64,
    pub(crate) from_seq: u64,
    pub(crate) watermark: u64,
    /// The permission cards the reset tail in this attach's reply carries, by
    /// `tool_call_id`; `None` when the client keeps its own timeline. Carried
    /// to the seam because a reset replaced what the client had, and the seam
    /// is the only place that knows which cards crossed already.
    pub(crate) reset_tail_cards: Option<HashSet<String>>,
}

pub(crate) struct AttachOutcome {
    pub(crate) generation: u64,
    pub(crate) live_agent_replay: Option<LiveAgentReplay>,
    /// The resume outcome the attach reply carries, present only when the
    /// connection negotiated `session.resume_outcomes` and the session is a
    /// live structured agent that was handed a cursor.
    pub(crate) resume: Option<SessionResumeInfo>,
}

fn merge_claude_manifest(previous: &SessionEvent, incoming: SessionEvent) -> SessionEvent {
    let SessionEvent::SessionManifest {
        provider_id: previous_provider,
        current_model_id: previous_current,
        models: previous_models,
        modes: previous_modes,
        ..
    } = previous
    else {
        return incoming;
    };
    let (provider_id, current_model_id, models, modes) = match incoming {
        SessionEvent::SessionManifest {
            provider_id,
            current_model_id,
            models,
            modes,
            ..
        } => (provider_id, current_model_id, models, modes),
        other => return other,
    };
    let mut merged_models = previous_models.clone();
    for incoming_model in models {
        let match_id = merged_models.iter().position(|model| {
            crate::claude_catalog::model_ids_match(&model.model_id, &incoming_model.model_id)
        });
        if let Some(index) = match_id {
            let previous_model = &merged_models[index];
            let mut merged = incoming_model;
            merged.name = previous_model.name.clone();
            merged.description = merged
                .description
                .or_else(|| previous_model.description.clone());
            merged.context_tokens = merged.context_tokens.or(previous_model.context_tokens);
            merged.current_effort = merged
                .current_effort
                .or_else(|| previous_model.current_effort.clone());
            merged.efforts = merged
                .efforts
                .filter(|efforts| !efforts.is_empty())
                .or_else(|| previous_model.efforts.clone());
            if previous_model.model_id != merged.model_id {
                merged_models.push(merged);
            } else {
                merged_models[index] = merged;
            }
        } else {
            merged_models.push(incoming_model);
        }
    }
    SessionEvent::SessionManifest {
        provider_id: provider_id.or_else(|| previous_provider.clone()),
        current_model_id: current_model_id.or_else(|| previous_current.clone()),
        models: merged_models,
        modes: modes.or_else(|| previous_modes.clone()),
        current_model_provider_id: None,
    }
}

fn replace_claude_catalog(previous: &SessionEvent, incoming: SessionEvent) -> SessionEvent {
    let SessionEvent::SessionManifest {
        provider_id: previous_provider,
        current_model_id: previous_current,
        modes: previous_modes,
        ..
    } = previous
    else {
        return incoming;
    };
    let (provider_id, current_model_id, models, modes) = match incoming {
        SessionEvent::SessionManifest {
            provider_id,
            current_model_id,
            models,
            modes,
            ..
        } => (provider_id, current_model_id, models, modes),
        other => return other,
    };
    let current_model_id = previous_current.clone().or(current_model_id);
    let mut merged_models = models;
    if let Some(current_model_id) = current_model_id.as_deref() {
        let present = merged_models
            .iter()
            .any(|model| model.model_id == current_model_id);
        if !present {
            if let Some(derived_model) = merged_models.iter().find(|model| {
                crate::claude_catalog::model_ids_match(&model.model_id, current_model_id)
            }) {
                let mut current_model = derived_model.clone();
                current_model.model_id = current_model_id.to_string();
                merged_models.push(current_model);
            } else {
                merged_models.push(SessionModel {
                    model_id: current_model_id.to_string(),
                    name: current_model_id.to_string(),
                    description: None,
                    context_tokens: None,
                    current_effort: None,
                    efforts: None,
                });
            }
        }
    }
    SessionEvent::SessionManifest {
        provider_id: provider_id.or_else(|| previous_provider.clone()),
        current_model_id,
        models: merged_models,
        modes: modes.or_else(|| previous_modes.clone()),
        current_model_provider_id: None,
    }
}

/// How a task-list snapshot is published: a refresh brings the revision it took
/// before deriving, the exit takes its own under the stream lock.
pub(crate) enum TasksPublish {
    /// `exit_sent_at_capture` is `tasks_exit_sent()` read before the refresh
    /// captured the parent and its children.
    Refresh {
        revision: u64,
        exit_sent_at_capture: bool,
    },
    /// Taken under the lock, the exit revision follows every revision already
    /// published, so it always passes the stale gate; a refresh that took its
    /// revision before the exit and publishes after it is dropped as stale.
    Exit,
}

/// One-shot callback registered on a runtime for the end of one of its turns.
struct TurnEndHook {
    id: u64,
    callback: Box<dyn Fn() + Send + Sync>,
}

/// The proof that a steer's admission was atomic, handed to the provider
/// adapter while the turn-hold is held.
///
/// The hold is the same lock the `AgentFinished` transition and `begin_turn`
/// take, so neither a turn ending nor a new turn starting can slip between
/// `with_active_turn`'s check and the provider write that follows it.
pub(crate) struct TurnToken<'a> {
    hold: Option<MutexGuard<'a, ()>>,
}

impl TurnToken<'_> {
    /// Run the provider write under the hold, then release it.
    ///
    /// A provider whose command is a round-trip splits here: the write must
    /// stay under the hold — that is what makes the admission atomic — while
    /// the wait for the answer must not. The answer is delivered by the reader
    /// thread that also publishes `AgentFinished`, so holding the hold across
    /// that wait would block the very thread the answer has to come from.
    pub(crate) fn write_then_release<R>(&mut self, write: impl FnOnce() -> R) -> R {
        let result = write();
        self.hold = None;
        result
    }
}

impl SessionRuntime {
    #[cfg(test)]
    pub(crate) fn new() -> Self {
        Self::with_journal(String::new(), None)
    }

    pub(crate) fn with_journal(session_id: String, journal: Option<Arc<Journal>>) -> Self {
        Self::with_journal_at_size(session_id, journal, INITIAL_COLS, INITIAL_ROWS)
    }

    /// The same constructor at an explicit first grid. The create road hands
    /// the geometry the terminal road opened its PTY at, so the emulator is
    /// born on the grid the child sees; every other road keeps the default.
    pub(crate) fn with_journal_at_size(
        session_id: String,
        journal: Option<Arc<Journal>>,
        cols: u16,
        rows: u16,
    ) -> Self {
        Self {
            session_id,
            journal,
            permission_broker: None,
            origin: OnceLock::new(),
            stream: Mutex::new(StreamState {
                next_seq: 1,
                last_applied_seq: 0,
                generation: 1,
                agent_tasks_published: false,
                screen: Some(Screen::new(cols, rows)),
                transcript: false,
                resize_owner: None,
                observers: HashMap::new(),
                agent_backlog: VecDeque::new(),
                agent_backlog_bytes: 0,
                agent_backlog_frames: 0,
                scrollback: Scrollback::default(),
                tasks_published: None,
                tasks_exit_published: true,
                tasks_exit_sent: false,
                output_closed: false,
                process_exited: false,
                exit_code: None,
                // A session with no first output yet is still observable as
                // alive; its age starts when the runtime is created.
                last_publish: Some(Instant::now()),
                exit_at: None,
                disposition: Disposition::Running,
                agent_reports: crate::agent_report::AgentReportState::default(),
                transcript_agent_reports: std::collections::BTreeMap::new(),
            }),
            pty_writer: OnceLock::new(),
            journal_degraded: AtomicBool::new(false),
            journal_dropped_frames: AtomicU64::new(0),
            journal_dropped_bytes: AtomicU64::new(0),
            terminal_dead: AtomicBool::new(false),
            attachment_notify: Mutex::new(HashMap::new()),
            generation: AtomicU64::new(1),
            peak_pending_bytes: AtomicUsize::new(0),
            coalesced_bytes: AtomicU64::new(0),
            coalesced_frames: AtomicU64::new(0),
            journal_replays: AtomicU64::new(0),
            task_seed_failures: AtomicU64::new(0),
            plan_mark_scans: AtomicU64::new(0),
            #[cfg(test)]
            plan_mark_scan_probe: Mutex::new(None),
            turn_counter: AtomicU64::new(0),
            turn_active: AtomicBool::new(false),
            // Nothing has been published yet, so the first reading of a live
            // session is a change and reaches the client that attached for it.
            published_activity: AtomicU8::new(activity_code(AgentActivityState::Idle)),
            turn_hold: Mutex::new(()),
            turn_end_hooks: Mutex::new(Vec::new()),
            next_turn_end_hook: AtomicU64::new(1),
            reader_finished: AtomicBool::new(false),
            child_reaped: AtomicBool::new(false),
            transition_ready: AtomicBool::new(false),
            exit_transition_sent: AtomicBool::new(false),
            published_frames: AtomicU64::new(0),
            published_bytes: AtomicUsize::new(0),
            session_manifest: Mutex::new(None),
            mode_before_plan: Mutex::new(None),
            claude_reported_mode: Mutex::new(None),
            agent_message: Mutex::new(None),
            agent_stop_reason: Mutex::new(None),
            stop_requested: AtomicBool::new(false),
            stop_signalled: AtomicBool::new(false),
            mcp_bearer: Mutex::new(None),
            mcp_url: Mutex::new(None),
            mcp_readiness: Mutex::new(McpReadiness {
                required: false,
                ready: false,
                failure: None,
            }),
            mcp_ready_cvar: Condvar::new(),
            tools_state: Mutex::new(crate::mcp_broker::ToolsState::Unavailable),
            agent_kind: Mutex::new(None),
            goal: Mutex::new(None),
            claude_catalog_state: Mutex::new(
                crate::claude_catalog::ClaudeCatalogState::Provisional,
            ),
            attention: Mutex::new(None),
            attention_hooks: Mutex::new(None),
            permission_park_hook: Mutex::new(None),
            tasks_refresh_hook: Mutex::new(None),
            background_tool_calls: Mutex::new(VecDeque::new()),
            tasks_pending_extra: Mutex::new(Vec::new()),
            tasks_revision: AtomicU64::new(0),
            tasks_last_refresh: Mutex::new(None),
            tasks_trailing_pending: AtomicBool::new(false),
            tasks_ended_wall_ms: Mutex::new(None),
            os_handle: Mutex::new(None),
            on_os_death: Mutex::new(None),
            os_death_started: AtomicBool::new(false),
            roster_notify: Mutex::new(None),
            finish_notify: Mutex::new(None),
            peer_session_id: Mutex::new(None),
            // A live session being started: nobody has sent it a prompt yet, so
            // the first one carries the standing instructions.
            first_prompt_owed: AtomicBool::new(true),
            recovered_context: Mutex::new(None),
            ingress_chain: Mutex::new(Chain::default()),
            activity_feed: Mutex::new(VecDeque::new()),
            deliveries_in_flight: AtomicU32::new(0),
        }
    }

    /// Whether *this* caller is the one that owes the session its first prompt.
    ///
    /// Exactly one caller can get `true` — the flag is taken, not read — so the
    /// standing instructions cannot be composed onto two prompts, and a prompt
    /// that arrives after a failed write does not get a second copy.
    pub(crate) fn take_first_prompt(&self) -> bool {
        self.first_prompt_owed.swap(false, Ordering::AcqRel)
    }

    /// Where the content this session has read came from.
    pub(crate) fn ingress_chain(&self) -> Chain {
        self.ingress_chain
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    /// Change the chain under one lock, so a delivery and a data read racing
    /// each other cannot lose one another's hop; answers the chain it replaced.
    pub(crate) fn update_ingress_chain(&self, change: impl FnOnce(&Chain) -> Chain) -> Chain {
        let mut chain = self
            .ingress_chain
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let next = change(&chain);
        std::mem::replace(&mut *chain, next)
    }

    /// A session being **resumed** owes no first prompt: the generation it
    /// resumes is mid-conversation, its first prompt already happened there,
    /// and the standing instructions were either on it or predate them. The
    /// resume road (`spawn_resumed_session` → `start_spawned_session`) calls
    /// this; a fresh session keeps the flag `with_journal` set.
    pub(crate) fn clear_first_prompt_owed(&self) {
        self.first_prompt_owed.store(false, Ordering::Release);
    }

    /// Hand this session the conversation a session it replaces could not hand
    /// over itself (`session_recovery.rs`). Set once, right after that session
    /// is created and before anyone can send it a prompt.
    pub(crate) fn set_recovered_context(&self, text: String) {
        if let Ok(mut slot) = self.recovered_context.lock() {
            *slot = Some(text);
        }
    }

    /// Take it, once: the recovered conversation rides exactly one prompt, the
    /// session's first — the same rule the standing instructions follow.
    pub(crate) fn take_recovered_context(&self) -> Option<String> {
        self.recovered_context
            .lock()
            .ok()
            .and_then(|mut slot| slot.take())
    }

    pub(crate) fn require_mcp(&self) {
        if let Ok(mut readiness) = self.mcp_readiness.lock() {
            readiness.required = true;
            self.mcp_ready_cvar.notify_all();
        }
    }

    pub(crate) fn set_tools_state(&self, state: crate::mcp_broker::ToolsState) {
        if let Ok(mut current) = self.tools_state.lock() {
            *current = state;
        }
    }

    pub(crate) fn tools_state(&self) -> crate::mcp_broker::ToolsState {
        self.tools_state
            .lock()
            .map(|state| *state)
            .unwrap_or(crate::mcp_broker::ToolsState::Unavailable)
    }

    pub(crate) fn set_mcp_bearer(&self, bearer: String) {
        if let Ok(mut secret) = self.mcp_bearer.lock() {
            *secret = Some(bearer);
        }
    }

    pub(crate) fn set_mcp_url(&self, url: String) {
        if let Ok(mut endpoint) = self.mcp_url.lock() {
            *endpoint = Some(url);
        }
    }

    pub(crate) fn redact_mcp_text(&self, text: &str) -> String {
        let bearer = self
            .mcp_bearer
            .lock()
            .ok()
            .and_then(|secret| secret.clone());
        let url = self
            .mcp_url
            .lock()
            .ok()
            .and_then(|endpoint| endpoint.clone());
        crate::mcp_broker::redact_broker_text(text, url.as_deref(), bearer.as_deref())
    }

    pub(crate) fn redact_mcp_value(&self, value: &serde_json::Value) -> serde_json::Value {
        let bearer = self
            .mcp_bearer
            .lock()
            .ok()
            .and_then(|secret| secret.clone());
        let url = self
            .mcp_url
            .lock()
            .ok()
            .and_then(|endpoint| endpoint.clone());
        if bearer.is_none() && url.is_none() {
            return value.clone();
        }
        fn redact(
            value: &serde_json::Value,
            url: Option<&str>,
            bearer: Option<&str>,
        ) -> serde_json::Value {
            match value {
                serde_json::Value::String(text) => serde_json::Value::String(
                    crate::mcp_broker::redact_broker_text(text, url, bearer),
                ),
                serde_json::Value::Array(values) => serde_json::Value::Array(
                    values
                        .iter()
                        .map(|value| redact(value, url, bearer))
                        .collect(),
                ),
                serde_json::Value::Object(values) => serde_json::Value::Object(
                    values
                        .iter()
                        .map(|(key, value)| {
                            (
                                crate::mcp_broker::redact_broker_text(key, url, bearer),
                                redact(value, url, bearer),
                            )
                        })
                        .collect(),
                ),
                other => other.clone(),
            }
        }
        redact(value, url.as_deref(), bearer.as_deref())
    }

    pub(crate) fn mark_mcp_ready(&self) {
        if let Ok(mut readiness) = self.mcp_readiness.lock() {
            if readiness.required && readiness.failure.is_none() {
                readiness.ready = true;
                self.mcp_ready_cvar.notify_all();
            }
        }
    }

    pub(crate) fn fail_mcp(&self, message: impl Into<String>) {
        if let Ok(mut readiness) = self.mcp_readiness.lock() {
            // Provider status is useful before readiness, but it is only a
            // hint. Once the broker has served authenticated tools/list, that
            // local proof outranks a later provider status flap.
            if readiness.required && !readiness.ready && readiness.failure.is_none() {
                readiness.failure = Some(message.into());
                self.mcp_ready_cvar.notify_all();
            }
        }
    }

    pub(crate) fn fail_mcp_broker(&self, message: impl Into<String>) {
        if let Ok(mut readiness) = self.mcp_readiness.lock() {
            if readiness.required && readiness.failure.is_none() {
                readiness.failure = Some(message.into());
                self.mcp_ready_cvar.notify_all();
            }
        }
    }

    pub(crate) fn fail_mcp_if_pending(&self, message: impl Into<String>) {
        if let Ok(mut readiness) = self.mcp_readiness.lock() {
            if readiness.required && !readiness.ready && readiness.failure.is_none() {
                readiness.failure = Some(message.into());
                self.mcp_ready_cvar.notify_all();
            }
        }
    }

    pub(crate) fn wait_for_mcp_ready(&self, timeout: Duration) -> Result<(), WireError> {
        let mut readiness = self
            .mcp_readiness
            .lock()
            .map_err(|_| WireError::new(ErrorCode::Internal, "Session state is unavailable."))?;
        if !readiness.required {
            return Ok(());
        }
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(message) = readiness.failure.clone() {
                return Err(WireError::new(ErrorCode::Io, message));
            }
            if readiness.ready {
                return Ok(());
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let (next, result) = self
                .mcp_ready_cvar
                .wait_timeout(readiness, remaining)
                .map_err(|_| {
                    WireError::new(ErrorCode::Internal, "Session state is unavailable.")
                })?;
            readiness = next;
            if result.timed_out() {
                break;
            }
        }
        let timeout_description = if timeout.as_secs() > 0 {
            format!("{} seconds", timeout.as_secs())
        } else {
            format!("{} milliseconds", timeout.as_millis())
        };
        Err(WireError::new(
            ErrorCode::Io,
            format!(
                "The MCP broker did not receive an authenticated tools/list within {timeout_description}; the first prompt was not sent."
            ),
        ))
    }

    pub(crate) fn from_replay(
        session_id: String,
        journal: Option<Arc<Journal>>,
        replay: Replay,
    ) -> Arc<Self> {
        let runtime = Arc::new(Self::with_journal(session_id, journal));
        // A session that comes back with a transcript is not owed a first prompt
        // (`create-from-profile`): it already had one, and the standing
        // instructions were either on it or predate them. This is what makes the
        // injection a session-start rule rather than a resume rule.
        runtime.clear_first_prompt_owed();
        // The transcript is back without the provenance of what it holds: a
        // replay with history starts fail-closed, until the person types.
        if replay.last_seq > 0 || !replay.events.is_empty() {
            runtime.update_ingress_chain(|_| Chain::restored());
        }
        let mut stream = runtime
            .stream
            .lock()
            .expect("new transcript runtime stream lock");
        runtime
            .generation
            .store(replay.generation, Ordering::Release);
        stream.generation = replay.generation;
        stream.next_seq = replay.last_seq.saturating_add(1);
        stream.last_applied_seq = replay.last_seq;
        stream.output_closed = true;
        stream.process_exited = true;
        stream.last_publish = None;
        stream.exit_at = None;
        let integrity = replay.integrity;
        stream.disposition = match integrity {
            TranscriptIntegrity::Unverifiable { .. } => Disposition::Recovered { integrity },
            TranscriptIntegrity::Complete | TranscriptIntegrity::Truncated { .. } => {
                Disposition::Exited { integrity }
            }
        };
        let (dropped_frames, dropped_bytes) = integrity_counters(integrity);
        runtime
            .journal_dropped_frames
            .store(dropped_frames, Ordering::Release);
        runtime
            .journal_dropped_bytes
            .store(dropped_bytes, Ordering::Release);
        // A recovered session is a transcript, not a live process: no
        // emulator, no snapshot, no live queue. Cursor-based journal
        // replay below serves its attaches.
        stream.screen = None;
        stream.transcript = true;
        for (index, event) in replay.events.into_iter().enumerate() {
            let journal_seq = replay.event_seqs.get(index).copied();
            match event {
                SessionEvent::Output { seq, data } => {
                    // The row's own generation, not the runtime's: a resumed
                    // session's transcript carries Output rows (the
                    // permission ledger) from every generation, and two
                    // generations can share a seq.
                    let generation = journal_seq.map_or(replay.generation, |(g, _)| g);
                    stream.scrollback.push(generation, seq, data.as_bytes());
                }
                SessionEvent::Exit { code } => {
                    stream.exit_code = code;
                    stream.disposition = Disposition::Exited { integrity };
                }
                SessionEvent::Recovered { integrity } => {
                    stream.disposition = Disposition::Recovered { integrity };
                }
                SessionEvent::Silent { .. } => {}
                SessionEvent::JournalDegraded {
                    dropped_frames,
                    dropped_bytes,
                } => {
                    runtime.journal_degraded.store(true, Ordering::Release);
                    runtime
                        .journal_dropped_frames
                        .fetch_max(dropped_frames, Ordering::AcqRel);
                    runtime
                        .journal_dropped_bytes
                        .fetch_max(dropped_bytes, Ordering::AcqRel);
                }
                SessionEvent::SessionsSnapshot { .. } => {}
                // A queue snapshot is daemon memory published to the sessions
                // attached right now; a recovered session replays transcript
                // events only.
                SessionEvent::QueueSnapshot { .. } => {}
                // The task list is derived the same way: live state for live
                // sessions, never a journal record.
                SessionEvent::TasksSnapshot { .. } => {}
                // Snapshots are screen state, never journal records; a
                // recovered session replays transcript events only.
                SessionEvent::Snapshot { .. } => {}
                SessionEvent::AgentReported { seq, .. } => {
                    // Seqs restart per generation, so the map key is the
                    // record's (generation, seq), not the seq alone: rows
                    // from different generations sharing a seq must all
                    // survive the replay.
                    let generation = journal_seq.map_or(replay.generation, |(g, _)| g);
                    stream
                        .transcript_agent_reports
                        .entry((generation, seq))
                        .or_default()
                        .push(event);
                }
                SessionEvent::AgentMessage { .. }
                | SessionEvent::AgentUserMessage { .. }
                | SessionEvent::Steered { .. }
                | SessionEvent::AgentThought { .. }
                | SessionEvent::AvailableCommands { .. }
                | SessionEvent::AgentToolCall { .. }
                | SessionEvent::AgentToolUpdate { .. }
                | SessionEvent::AgentFinished { .. }
                | SessionEvent::AgentTaskStarted { .. }
                | SessionEvent::AgentTaskNotification { .. }
                | SessionEvent::AgentBackgroundTasksChanged { .. }
                | SessionEvent::AgentTasks { .. }
                | SessionEvent::GoalChanged { .. }
                | SessionEvent::AgentError { .. }
                | SessionEvent::AgentStderr { .. }
                | SessionEvent::PermissionRequest { .. }
                | SessionEvent::PermissionResolved { .. }
                | SessionEvent::PermissionAnswered { .. }
                | SessionEvent::SessionNotice { .. }
                | SessionEvent::SessionManifest { .. }
                | SessionEvent::SessionFeatureState { .. }
                | SessionEvent::AgentCreated { .. }
                | SessionEvent::AgentResumed { .. }
                | SessionEvent::ChildFinished { .. }
                | SessionEvent::ContextUsage { .. }
                | SessionEvent::PlanUsage { .. } => {
                    // Same key as AgentReported above: (generation, seq),
                    // so colliding seqs across the resume seam coexist. A row
                    // that derived several views keeps them all, in view
                    // order — the key is the row, not the event.
                    let Some((generation, seq)) = journal_seq else {
                        continue;
                    };
                    stream
                        .transcript_agent_reports
                        .entry((generation, seq))
                        .or_default()
                        .push(event);
                }
                // Detached names one observer's view and is never journalled;
                // a replay can only meet it as a no-op marker.
                SessionEvent::Detached => {}
            }
        }
        drop(stream);
        runtime
    }

    /// True when this runtime is a recovered transcript (no emulator, no
    /// live process). Attaches to it replay the journal instead of
    /// synchronising a screen.
    pub(crate) fn is_transcript(&self) -> bool {
        self.lock_stream()
            .map(|stream| stream.transcript)
            .unwrap_or(false)
    }

    pub(crate) fn for_acp(
        session_id: String,
        journal: Option<Arc<Journal>>,
        permission_broker: Arc<PermissionBroker>,
    ) -> Arc<Self> {
        let mut state = Self::with_journal(session_id, journal);
        state.permission_broker = Some(permission_broker);
        let runtime = Arc::new(state);
        if let Ok(mut stream) = runtime.stream.lock() {
            // ACP has structured messages rather than a terminal screen, but
            // it is still a live session and must use the live attach path.
            stream.screen = None;
            stream.transcript = false;
        }
        runtime
    }

    pub(crate) fn lock_stream(&self) -> Result<MutexGuard<'_, StreamState>, ()> {
        match self.stream.lock() {
            Ok(stream) => Ok(stream),
            Err(error) => {
                // PoisonError owns the guard that was acquired before the
                // panic. Release it before the dead-session path takes any
                // other action, otherwise refreshing the disposition would
                // wait forever on the same poisoned mutex.
                drop(error);
                self.mark_terminal_dead("session stream lock poisoned");
                Err(())
            }
        }
    }

    /// The visible grid of this session's emulator, or `None` when the
    /// session has no screen — a transcript, or a session whose provider
    /// speaks structured messages instead of a screen. This is the one read
    /// of the screen outside an attachment, and like every snapshot the
    /// daemon sends it carries the visible grid only, never scrollback.
    ///
    /// The cells are copied while the stream lock is held and the copy is
    /// what leaves here, so a caller formats the snapshot and never the live
    /// grid under the lock.
    pub(crate) fn screen_snapshot(&self) -> Option<ScreenSnapshot> {
        let stream = self.lock_stream().ok()?;
        stream.screen.as_ref().map(Screen::snapshot)
    }

    pub(crate) fn mark_terminal_dead(&self, reason: &str) {
        if !self.terminal_dead.swap(true, Ordering::AcqRel) {
            eprintln!("session {} marked dead: {reason}", self.session_id);
        }
        self.mark_journal_degraded();
        self.notify_attachment();
    }

    pub(crate) fn set_attachment_notify(&self, key: AttachmentKey, outbound: Option<Arc<ConnOut>>) {
        match self.attachment_notify.lock() {
            Ok(mut current) => {
                if let Some(outbound) = outbound {
                    current.insert(key, outbound);
                } else {
                    current.remove(&key);
                }
            }
            Err(_) => eprintln!(
                "session {} could not update attachment notification: lock poisoned",
                self.session_id
            ),
        }
    }

    pub(crate) fn notify_attachment(&self) {
        match self.attachment_notify.lock() {
            Ok(current) => {
                for outbound in current.values() {
                    outbound.notify();
                }
            }
            Err(_) => eprintln!(
                "session {} could not notify its attachment: lock poisoned",
                self.session_id
            ),
        }
    }

    pub(crate) fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub(crate) fn transition_ready(&self) -> bool {
        self.transition_ready.load(Ordering::Acquire)
    }

    pub(crate) fn process_exited(&self) -> bool {
        self.lock_stream()
            .map(|stream| stream.process_exited)
            .unwrap_or(true)
    }

    pub(crate) fn should_publish_exit_transition(&self) -> bool {
        self.transition_ready() && !self.exit_transition_sent.swap(true, Ordering::AcqRel)
    }

    pub(crate) fn publish_output(&self, data: &str) -> bool {
        let pty_replies;
        let seq;
        let generation;
        let was_silent;
        {
            let Ok(mut stream) = self.lock_stream() else {
                return false;
            };
            if stream.output_closed {
                // Bytes after EOF are neither applied nor journalled; no
                // sequence is consumed for them.
                notify_observers(&stream);
                eprintln!(
                    "session {} dropped terminal output after EOF ({} bytes)",
                    self.session_id,
                    data.len()
                );
                return false;
            }
            // ONE critical section: allocate the sequence, apply the complete
            // chunk to the emulator, then — only after parsing completed —
            // advance the boundary and enqueue the live update. Releasing the
            // lock anywhere before the boundary update would let an attach
            // capture a snapshot that claims a chunk that was only queued.
            seq = stream.next_seq;
            stream.next_seq = stream.next_seq.saturating_add(1);
            pty_replies = match stream.screen.as_mut() {
                Some(screen) => {
                    match catch_unwind(AssertUnwindSafe(|| screen.feed(data.as_bytes()))) {
                        Ok(replies) => replies,
                        Err(_) => {
                            drop(stream);
                            self.mark_terminal_dead("terminal parser panicked");
                            return false;
                        }
                    }
                }
                // Transcript runtimes have no reader thread; unreachable, but
                // the boundary must still stay honest if it ever happened.
                None => Vec::new(),
            };
            stream.last_applied_seq = seq;
            stream.last_publish = Some(Instant::now());
            was_silent = matches!(stream.disposition, Disposition::Silent);
            // Output is an observed sign of life while the process is still
            // running. Bytes drained after Child::wait are not a revival and
            // must not turn an observed exit back into Live.
            if !stream.process_exited {
                stream.disposition = Disposition::Running;
            }
            generation = stream.generation;
            let (coalesced_bytes, coalesced_frames) = enqueue_output(&mut stream, seq, data);
            self.peak_pending_bytes.fetch_max(
                stream
                    .observers
                    .values()
                    .map(|attachment| attachment.pending_bytes)
                    .max()
                    .unwrap_or(0),
                Ordering::Relaxed,
            );
            self.coalesced_bytes
                .fetch_add(coalesced_bytes, Ordering::Relaxed);
            self.coalesced_frames
                .fetch_add(coalesced_frames, Ordering::Relaxed);
            notify_observers(&stream);
        }
        // Terminal query replies (DSR/CPR) go straight back to the PTY:
        // ConPTY stalls its render pipeline until they are answered, so they
        // must not wait for the journal, snapshot encoding, or a client.
        self.write_pty_replies(&pty_replies);
        self.published_frames.fetch_add(1, Ordering::Relaxed);
        self.published_bytes
            .fetch_add(data.len(), Ordering::Relaxed);
        // The journal append is asynchronous exactly as before: its failure
        // degrades the transcript, never the screen boundary.
        if let Some(journal) = &self.journal {
            let accepted = journal.try_append(output_record(
                self.session_id.clone(),
                generation,
                seq,
                data.as_bytes(),
            ));
            if !accepted || journal.is_session_degraded(&self.session_id) {
                self.mark_journal_degraded();
            }
        }
        was_silent
    }

    /// Journal the raw provider envelope — the row an ACP, Claude, Codex or
    /// Pi frame replays from — and hand back the stream seq it took.
    pub(crate) fn journal_agent_envelope(&self, envelope: &serde_json::Value) -> Option<u64> {
        let Ok(mut stream) = self.lock_stream() else {
            return None;
        };
        if stream.output_closed {
            return None;
        }
        let generation = stream.generation;
        let seq = stream.next_seq;
        stream.next_seq = stream.next_seq.saturating_add(1);
        drop(stream);
        if let Some(journal) = &self.journal {
            if let Some(record) = crate::journal::acp_envelope_record(
                self.session_id.clone(),
                generation,
                seq,
                envelope,
            ) {
                let accepted = journal.try_append(record);
                if !accepted || journal.is_session_degraded(&self.session_id) {
                    self.mark_journal_degraded();
                }
            }
        }
        Some(seq)
    }

    /// Journal two envelopes as one adjacent pair under a single closed
    /// check, returning the second row's seq. The suppression marker is
    /// only meaningful immediately ahead of the envelope it owns: two
    /// separate calls admit a `close_output` between them, which journals
    /// a marker with no envelope to own — and replay then spends it
    /// suppressing the next turn's genuine finish. The pair travels as one
    /// queue command and one journal transaction (`Journal::
    /// try_append_pair`): a full queue refuses both rows, and a second row
    /// the writer cannot insert rolls the first back, so no marker is ever
    /// stranded ahead of a hole.
    pub(crate) fn journal_agent_envelope_pair(
        &self,
        first: &serde_json::Value,
        second: &serde_json::Value,
    ) -> Option<u64> {
        let Ok(mut stream) = self.lock_stream() else {
            return None;
        };
        if stream.output_closed {
            return None;
        }
        let generation = stream.generation;
        let first_seq = stream.next_seq;
        stream.next_seq = stream.next_seq.saturating_add(2);
        drop(stream);
        if let Some(journal) = &self.journal {
            if let (Some(first), Some(second)) = (
                crate::journal::acp_envelope_record(
                    self.session_id.clone(),
                    generation,
                    first_seq,
                    first,
                ),
                crate::journal::acp_envelope_record(
                    self.session_id.clone(),
                    generation,
                    first_seq + 1,
                    second,
                ),
            ) {
                let accepted = journal.try_append_pair(first, second);
                if !accepted || journal.is_session_degraded(&self.session_id) {
                    self.mark_journal_degraded();
                }
            }
        }
        Some(first_seq + 1)
    }

    pub(crate) fn store_session_manifest(&self, event: SessionEvent) -> SessionEvent {
        let Ok(mut stored) = self.session_manifest.lock() else {
            return event;
        };
        let previous = stored.as_ref();
        let event = match event {
            SessionEvent::SessionManifest {
                provider_id,
                current_model_id,
                models,
                modes,
                current_model_provider_id,
                ..
            } => {
                let (current_model_id, models) = if models.is_empty() {
                    let previous_manifest = previous.and_then(|previous| match previous {
                        SessionEvent::SessionManifest {
                            current_model_id,
                            models,
                            ..
                        } => Some((current_model_id.clone(), models.clone())),
                        _ => None,
                    });
                    (
                        current_model_id.or_else(|| {
                            previous_manifest
                                .as_ref()
                                .and_then(|previous| previous.0.clone())
                        }),
                        previous_manifest
                            .map(|previous| previous.1)
                            .unwrap_or_default(),
                    )
                } else {
                    (current_model_id, models)
                };
                let modes = modes.or_else(|| {
                    previous.and_then(|previous| match previous {
                        SessionEvent::SessionManifest { modes, .. } => modes.clone(),
                        _ => None,
                    })
                });
                let event = SessionEvent::SessionManifest {
                    provider_id,
                    current_model_id,
                    models,
                    modes,
                    current_model_provider_id,
                };
                if matches!(
                    &event,
                    SessionEvent::SessionManifest {
                        provider_id: Some(provider_id),
                        ..
                    } if provider_id == "claude"
                ) {
                    previous
                        .map(|previous| merge_claude_manifest(previous, event.clone()))
                        .unwrap_or(event)
                } else {
                    event
                }
            }
            event => event,
        };
        if let SessionEvent::SessionManifest {
            provider_id,
            modes: Some(modes),
            ..
        } = &event
        {
            if provider_id.as_deref() != Some("claude") {
                let old_mode = previous.and_then(|previous| match previous {
                    SessionEvent::SessionManifest {
                        modes: Some(previous_modes),
                        ..
                    } => Some(previous_modes.current_mode_id.as_str()),
                    _ => None,
                });
                if let Err(error) = self.record_mode_before_plan(old_mode, &modes.current_mode_id) {
                    eprintln!(
                        "session {} could not update pre-plan mode: {}",
                        self.session_id, error.message
                    );
                }
            }
        }
        crate::quota_poller::note_manifest(&event);
        *stored = Some(event.clone());
        event
    }

    pub(crate) fn store_claude_manifest(
        &self,
        event: SessionEvent,
        state: crate::claude_catalog::ClaudeCatalogState,
    ) -> SessionEvent {
        let event = self.store_session_manifest(event);
        if let Ok(mut stored_state) = self.claude_catalog_state.lock() {
            *stored_state = state;
        }
        event
    }

    fn record_mode_before_plan(
        &self,
        previous_mode: Option<&str>,
        mode_id: &str,
    ) -> Result<(), WireError> {
        let mut stored = self.mode_before_plan.lock().map_err(|_| {
            WireError::new(
                ErrorCode::Io,
                "Pre-plan mode history is unavailable; mode history was not updated.",
            )
        })?;
        Self::update_mode_before_plan(&mut stored, previous_mode, mode_id);
        Ok(())
    }

    fn update_mode_before_plan(
        stored: &mut Option<String>,
        previous_mode: Option<&str>,
        mode_id: &str,
    ) {
        if mode_id == "plan" {
            if previous_mode != Some("plan") {
                *stored = previous_mode.map(str::to_string);
            }
        } else {
            *stored = None;
        }
    }

    pub(crate) fn store_claude_catalog(&self, event: SessionEvent) -> SessionEvent {
        let event = if let Ok(mut stored) = self.session_manifest.lock() {
            let event = stored
                .as_ref()
                .map(|previous| replace_claude_catalog(previous, event.clone()))
                .unwrap_or(event);
            *stored = Some(event.clone());
            event
        } else {
            event
        };
        if let Ok(mut stored_state) = self.claude_catalog_state.lock() {
            *stored_state = crate::claude_catalog::ClaudeCatalogState::Derived;
        }
        event
    }

    pub(crate) fn set_peer_session_id(&self, session_id: String) {
        self.restore_peer_session_id(session_id.clone());
        if let Some(journal) = &self.journal {
            if let Err(error) = journal.set_peer_session_id(&self.session_id, &session_id) {
                eprintln!(
                    "journal could not persist peer session id for {}: {error}",
                    self.session_id
                );
            }
        }
    }

    pub(crate) fn restore_peer_session_id(&self, session_id: String) {
        if let Ok(mut stored) = self.peer_session_id.lock() {
            *stored = Some(session_id);
        }
    }

    pub(crate) fn peer_session_id(&self) -> Option<String> {
        self.peer_session_id
            .lock()
            .ok()
            .and_then(|stored| stored.clone())
    }

    /// Set the generation for a freshly spawned replacement process. The
    /// caller has already reset the journal row; no old stream state is reused
    /// by the new ACP runtime.
    pub(crate) fn set_generation(&self, generation: u64) {
        if let Ok(mut stream) = self.lock_stream() {
            stream.generation = generation;
            // The restored checklist's gate belongs to one generation: a
            // replacement process runs its own history pass.
            stream.agent_tasks_published = false;
            self.generation.store(generation, Ordering::Release);
        }
    }

    pub(crate) fn publish_agent_event(
        &self,
        event: SessionEvent,
        journal_text: Option<&str>,
    ) -> bool {
        self.publish_agent_event_with_seq(event, journal_text, None)
    }

    /// Publish a daemon-owned agent event as an AgentReport row rather than an
    /// ACP envelope. Provider echo envelopes remain replayable for history
    /// written before the echo was suppressed.
    pub(crate) fn publish_agent_user_message(
        &self,
        text: String,
        author: UserMessageAuthor,
        message_kind: UserMessageKind,
    ) -> Option<String> {
        self.publish_agent_user_message_with_images(text, author, message_kind, Vec::new())
    }

    /// The same echo carrying the send's deposited image references, so the
    /// journal row and the replay resolve the stored bytes. References only:
    /// the prompt's path lines already name the files for the provider, and
    /// the base64 stays out of the transcript either way.
    pub(crate) fn publish_agent_user_message_with_images(
        &self,
        text: String,
        author: UserMessageAuthor,
        message_kind: UserMessageKind,
        images: Vec<AttachmentReference>,
    ) -> Option<String> {
        // The id is built by the publisher, so the event, the transcript and the
        // journal row that links to it (`Steered`) name one message: the caller
        // takes the id back out of the event that was actually published rather
        // than inventing a second one.
        //
        // Live publication attaches `at_ms` to Composer sends. Replay
        // additionally times kind-less native `agent_report` rows written
        // before `message_kind` existed, from their row.
        self.publish_journaled_agent_event(|generation, seq, at_ms| {
            SessionEvent::AgentUserMessage {
                message_id: Some(format!("devboule-user-{generation}-{seq}")),
                text,
                author,
                message_kind,
                at_ms: message_kind.is_user_turn().then_some(at_ms),
                images,
            }
        })
        .and_then(|event| match event {
            SessionEvent::AgentUserMessage { message_id, .. } => message_id,
            _ => None,
        })
    }

    pub(crate) fn publish_agent_error(&self, message: String) -> bool {
        self.publish_journaled_agent_event(|_, _, _| SessionEvent::AgentError { message })
            .is_some()
    }

    /// Publish an event the daemon authors itself and journal it as an
    /// `AgentReport` row, so replay derives it back the way it derives
    /// `AgentError` and `SessionNotice`. For an event whose only source is
    /// a decision made here: no provider row carries it, so the envelope
    /// path has nothing to re-derive from.
    pub(crate) fn publish_daemon_event(&self, event: SessionEvent) -> bool {
        self.publish_journaled_agent_event(|_, _, _| event)
            .is_some()
    }

    /// Publish a finish the daemon decided itself — no provider row carries
    /// it, so the envelope path has nothing to re-derive from — journaled on
    /// the same road every daemon-authored event takes, ending the turn only
    /// when the event really is an `AgentFinished` (the guard
    /// [`Self::publish_agent_event_with_seq`] keeps). The outcome is
    /// recorded first because the publish's own finish notify reads it
    /// back, so a publish the stream refuses (a closed session) leaves the
    /// outcome recorded with no event and no turn end — a session already
    /// over, and the one case where the two do not travel together.
    pub(crate) fn publish_journaled_finish(&self, event: SessionEvent) -> bool {
        let ends_the_turn = matches!(&event, SessionEvent::AgentFinished { .. });
        self.record_agent_outcome(&event);
        let published = self
            .publish_journaled_agent_event(|_, _, _| event)
            .is_some();
        if published && ends_the_turn {
            self.finish_turn();
        }
        published
    }

    /// Publish this session's whole follow-up queue to every attached
    /// subscriber that negotiated the queue, and nowhere else.
    ///
    /// Not a journaled event and not a replayed one: the queue is the daemon's
    /// memory, so a client that attaches finds it through the attach snapshot
    /// rather than by replaying rows. Nothing goes into `agent_backlog`, which
    /// is what a later attach would replay from — a snapshot sitting in that
    /// backlog would be a stale view delivered to a client that should get the
    /// current one.
    ///
    /// An observer exists only for a subscription that passed this session's own
    /// scope check, so "attached" already means "authorized to see the
    /// session"; the capability check on top is the narrower one — a client
    /// whose hello did not offer `session.queue` cannot read this event, and a
    /// daemon that knows that must not put it on that connection.
    pub(crate) fn publish_queue_snapshot(
        &self,
        epoch: String,
        revision: u64,
        items: Vec<QueuedMessage>,
        dropped: Vec<devboule_protocol::DroppedQueuedMessage>,
    ) {
        let Ok(mut stream) = self.lock_stream() else {
            return;
        };
        if stream.output_closed {
            return;
        }
        let event = SessionEvent::QueueSnapshot {
            epoch,
            revision,
            items,
            dropped,
        };
        for attachment in stream.observers.values_mut() {
            if !attachment.session_queue {
                continue;
            }
            enqueue_agent_for_attachment(attachment, event.clone(), None);
        }
        notify_observers(&stream);
    }

    /// Publish this session's whole background-task list to every attached
    /// subscriber that negotiated the task list, and nowhere else. True
    /// when the enqueue ran — even with no observers, which is itself the
    /// proof there is nothing to send. False when the stream lock is
    /// poisoned, when a refresh meets closed output, when a refresh captured
    /// its state before the exit went out and the exit is already sent, or
    /// when a refresh is stale (see below).
    ///
    /// Transient like the queue snapshot above — derived, never journaled,
    /// never backlogged — including its cap gate: a client that did not
    /// offer `session.tasks` cannot parse the event and is never sent it.
    /// A snapshot that is not newer than the last one sent — same epoch and
    /// an older or equal revision — is dropped, so overlapping derives
    /// cannot leave a stale list behind. A new epoch restarts the gate.
    ///
    /// The exit skips the closed flag: it guards output bytes, and a
    /// transient snapshot is not output — its subscribers are still
    /// attached, and the reader-EOF road closes output before the exit
    /// thread derives.
    pub(crate) fn publish_tasks_snapshot(
        &self,
        epoch: String,
        tasks: Vec<SessionTask>,
        publish: TasksPublish,
        omitted: u32,
    ) -> bool {
        let Ok(mut stream) = self.lock_stream() else {
            return false;
        };
        let (revision, is_exit) = match publish {
            TasksPublish::Refresh {
                revision,
                exit_sent_at_capture,
            } => {
                // A derive that captured the parent before the exit went out
                // may still see it live, so it must not land after the exit.
                if stream.tasks_exit_sent && !exit_sent_at_capture {
                    return false;
                }
                (revision, false)
            }
            TasksPublish::Exit => (self.next_tasks_revision(), true),
        };
        if stream.output_closed && !is_exit {
            return false;
        }
        let stale = stream
            .tasks_published
            .as_ref()
            .is_some_and(|(last_epoch, last_revision)| {
                *last_epoch == epoch && revision <= *last_revision
            });
        if stale {
            return false;
        }
        stream.tasks_published = Some((epoch.clone(), revision));
        if is_exit {
            stream.tasks_exit_sent = true;
        }
        let event = SessionEvent::TasksSnapshot {
            epoch,
            revision,
            tasks,
            omitted,
        };
        for attachment in stream.observers.values_mut() {
            if !attachment.session_tasks {
                continue;
            }
            enqueue_agent_for_attachment(attachment, event.clone(), None);
        }
        notify_observers(&stream);
        true
    }

    /// Sends one live plan-usage reading to the attached observers whose
    /// connection negotiated `session.plan_usage`. Transient, the same road as
    /// the queue snapshot: it goes to those observers' pending queues and
    /// nowhere else. It is never journaled and never put in `agent_backlog`, so
    /// a later attach replays the journal without it. Observers that did not
    /// agree the name are skipped, and so is a session whose output is closed.
    pub(crate) fn publish_plan_usage_live(&self, reading: SessionEvent) {
        let Ok(mut stream) = self.lock_stream() else {
            return;
        };
        if stream.output_closed {
            return;
        }
        for attachment in stream.observers.values_mut() {
            if !attachment.plan_usage_live {
                continue;
            }
            enqueue_agent_for_attachment(attachment, reading.clone(), None);
        }
        notify_observers(&stream);
    }

    /// Whether any client is attached to this session right now.
    pub(crate) fn has_observers(&self) -> bool {
        self.lock_stream()
            .map(|stream| !stream.observers.is_empty())
            .unwrap_or(false)
    }

    pub(crate) fn publish_session_notice(&self, text: String, severity: NoticeSeverity) -> bool {
        let (event, generation, seq) = {
            let Ok(mut stream) = self.lock_stream() else {
                return false;
            };
            if stream.output_closed {
                return false;
            }
            let generation = stream.generation;
            let seq = stream.next_seq;
            stream.next_seq = stream.next_seq.saturating_add(1);
            let event = SessionEvent::SessionNotice { text, severity };
            enqueue_agent(&mut stream, event.clone(), Some(seq));
            self.published_frames.fetch_add(1, Ordering::Relaxed);
            self.published_bytes.fetch_add(
                serde_json::to_vec(&event)
                    .map(|bytes| bytes.len())
                    .unwrap_or(0),
                Ordering::Relaxed,
            );
            notify_observers(&stream);
            (event, generation, seq)
        };
        if let Some(journal) = &self.journal {
            if let Some(record) = crate::journal::agent_report_record(
                self.session_id.clone(),
                generation,
                seq,
                &event,
            ) {
                let accepted = journal.try_append(record);
                if !accepted || journal.is_session_degraded(&self.session_id) {
                    self.mark_journal_degraded();
                }
            }
        }
        true
    }

    /// Publish one daemon-owned event and journal it as an AgentReport row.
    ///
    /// `Some(event)` is the event exactly as published — the caller needs the
    /// `message_id` it carries, so the same id can be written into the row that
    /// links to it. `None` means the stream refused it (closed, or the
    /// lock is gone), which callers report as a degraded session and never as an
    /// error.
    fn publish_journaled_agent_event<F>(&self, build: F) -> Option<SessionEvent>
    where
        F: FnOnce(u64, u64, u64) -> SessionEvent,
    {
        self.publish_journaled_agent_event_if(|_| true, build)
    }

    /// The restored checklist's publish: the snapshot is the fallback for a
    /// generation with no AgentTasks yet, and the refusal and the enqueue
    /// share the stream lock every live publish takes.
    pub(crate) fn publish_restored_agent_tasks(&self, items: Vec<AgentTaskItem>) -> bool {
        self.publish_journaled_agent_event_if(
            |stream| !stream.agent_tasks_published,
            |_, _, _| SessionEvent::AgentTasks { items },
        )
        .is_some()
    }

    /// The same publish while `accept` holds, decided under the stream lock
    /// that also enqueues — a live publish cannot slip between them.
    fn publish_journaled_agent_event_if<P, F>(&self, accept: P, build: F) -> Option<SessionEvent>
    where
        P: FnOnce(&StreamState) -> bool,
        F: FnOnce(u64, u64, u64) -> SessionEvent,
    {
        // One reading of the journal's clock for the event's `at_ms` and the
        // row's `ts_ms`: replay carries the column back, so the two must be
        // the same instant, not two ticks of the same source.
        let at_ms = crate::journal::now_ms();
        let (event, generation, seq, was_silent) = {
            let Ok(mut stream) = self.lock_stream() else {
                // `None` means the stream cannot accept the event.
                return None;
            };
            if stream.output_closed {
                return None;
            }
            if !accept(&stream) {
                return None;
            }
            let was_silent = matches!(stream.disposition, Disposition::Silent);
            if !stream.process_exited {
                stream.disposition = Disposition::Running;
            }
            let generation = stream.generation;
            let seq = stream.next_seq;
            stream.next_seq = stream.next_seq.saturating_add(1);
            let event = build(generation, seq, at_ms);
            if matches!(&event, SessionEvent::AgentTasks { .. }) {
                stream.agent_tasks_published = true;
            }
            stream.last_publish = Some(Instant::now());
            self.record_activity(Some(seq), &event);
            enqueue_agent(&mut stream, event.clone(), Some(seq));
            self.published_frames.fetch_add(1, Ordering::Relaxed);
            self.published_bytes.fetch_add(
                serde_json::to_vec(&event)
                    .map(|bytes| bytes.len())
                    .unwrap_or(0),
                Ordering::Relaxed,
            );
            notify_observers(&stream);
            (event, generation, seq, was_silent)
        };
        if let Some(journal) = &self.journal {
            if let Some(record) = crate::journal::agent_report_record_at(
                self.session_id.clone(),
                generation,
                seq,
                &event,
                at_ms,
            ) {
                let accepted = journal.try_append(record);
                if !accepted || journal.is_session_degraded(&self.session_id) {
                    self.mark_journal_degraded();
                }
            }
        }
        if was_silent {
            self.notify_roster();
        }
        let changed = self.mark_activity_changed();
        let raised = self.raise_attention_for_event(&event);
        if changed && !raised {
            self.request_transition();
        }
        // A turn that ended is reported here rather than through attention:
        // see [`Self::finish_notify`]. One call per `AgentFinished`, and the
        // report path itself is gated on the child actually being one.
        if matches!(&event, SessionEvent::AgentFinished { .. }) {
            self.notify_finished();
        }
        Some(event)
    }

    /// Start a turn before the provider write, so a steer's admission cannot
    /// observe "a turn is running" before the provider has the prompt that
    /// starts it.
    ///
    /// The previous turn's stop reason dies here: it describes the turn that
    /// ended, not the run, and a kill mid-turn with no further end must not
    /// read as that turn's own verdict (`child_finish_state`).
    pub(crate) fn begin_turn(&self) {
        let push = {
            let _hold = self.lock_turn_hold();
            self.turn_active.store(true, Ordering::Release);
            if let Ok(mut slot) = self.agent_stop_reason.lock() {
                *slot = None;
                // A stop sent before this turn began is the kill that ends it.
                if self.stop_signalled.load(Ordering::Acquire) {
                    self.stop_requested.store(true, Ordering::Release);
                }
            }
            self.mark_activity_changed()
                .then(|| self.prepare_roster_transition())
                .flatten()
        };
        // Captured under the same lock as finish_turn, so each snapshot matches
        // the edge it follows. Delivery runs after the release, so two racing
        // transitions on one session can still arrive out of order.
        if let Some(push) = push {
            push();
        }
    }

    /// The turn status as the roster publishes it: `blocked` while a card waits
    /// for an answer, `working` while a turn runs, `idle` for a live session with
    /// neither, `unknown` once the process is gone. The same derivation the child
    /// tool reads, so a row and a tool can never disagree.
    pub(crate) fn activity(&self) -> AgentActivityState {
        crate::agent_activity::derive_activity(
            !self.process_exited(),
            self.is_running_turn(),
            self.permission_pending(),
        )
    }

    /// Announce this session's turn status unless it is the status this runtime
    /// last announced. Every change is told, because the row a client holds is a
    /// value it has to be able to trust.
    pub(crate) fn publish_activity_change(&self) {
        if self.mark_activity_changed() {
            self.request_transition();
        }
    }

    /// Record the status and say whether it moved, without pushing. Callers that
    /// also publish attention can use that push to carry the changed status.
    pub(crate) fn mark_activity_changed(&self) -> bool {
        let code = activity_code(self.activity());
        self.published_activity.swap(code, Ordering::AcqRel) != code
    }

    /// Push the current roster after an activity or attention change.
    pub(crate) fn request_transition(&self) {
        if let Some(push) = self.prepare_roster_transition() {
            push();
        }
    }

    pub(crate) fn turn_counter(&self) -> u64 {
        self.turn_counter.load(Ordering::Acquire)
    }

    pub(crate) fn is_turn_active(&self, expected_turn_id: u64) -> bool {
        self.turn_active.load(Ordering::Acquire)
            && self.turn_counter() == expected_turn_id
            && !self.process_exited()
    }

    /// The turn-hold. A poisoned lock is recovered rather than propagated: the
    /// guarded value is `()`, so nothing a panic could have left half-written
    /// is behind it, and refusing to lock would freeze every later turn
    /// transition on this session.
    fn lock_turn_hold(&self) -> MutexGuard<'_, ()> {
        self.turn_hold
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Compare-and-deliver: run `deliver` with a [`TurnToken`] only while this
    /// runtime's turn is the one `expected_turn_id` names, holding the same
    /// lock `finish_turn` and `begin_turn` take for as long as `deliver` runs
    /// (unless the adapter releases it through `TurnToken::write_then_release`
    /// once its write is done).
    ///
    /// `None` means the turn was over — or the process already gone — at the
    /// moment of admission, so nothing may be written for it.
    pub(crate) fn with_active_turn<T>(
        &self,
        expected_turn_id: u64,
        deliver: impl FnOnce(&mut TurnToken<'_>) -> T,
    ) -> Option<T> {
        let mut token = TurnToken {
            hold: Some(self.lock_turn_hold()),
        };
        if !self.is_turn_active(expected_turn_id) {
            return None;
        }
        Some(deliver(&mut token))
    }

    /// End the turn if one is running, advancing the turn counter. The
    /// transition takes the turn-hold, so a steer admitted for this turn has
    /// already issued its write by the time the counter moves. The
    /// hooks run after the hold is released: they take other locks, and this
    /// is the reader thread.
    fn finish_turn(&self) {
        let (ended, push) = {
            let _hold = self.lock_turn_hold();
            self.end_turn_under_hold()
        };
        if ended {
            self.fire_turn_end_hooks();
        }
        if let Some(push) = push {
            push();
        }
    }

    /// The turn transition itself, under the hold both callers already own.
    fn end_turn_under_hold(&self) -> (bool, Option<Box<dyn FnOnce() + Send>>) {
        let ended = if self.turn_active.swap(false, Ordering::AcqRel) {
            self.turn_counter.fetch_add(1, Ordering::AcqRel);
            true
        } else {
            false
        };
        let push = (ended && self.mark_activity_changed())
            .then(|| self.prepare_roster_transition())
            .flatten();
        (ended, push)
    }

    /// End the running turn unless `withhold` says to keep it, deciding AND
    /// transitioning under the turn-hold so the decision is atomic with steer
    /// admission: a steer either is admitted while the hold is still yours to
    /// take (its delivery has already happened, so `withhold` can see it and
    /// spare the turn) or finds the turn already ended (admission refused,
    /// the text goes as a plain prompt for a new run). The closure may take
    /// the provider's abort-gate lock; that lock is always acquired BELOW the
    /// turn-hold — the steer path takes the same order (hold, then gate) — so
    /// the two can never deadlock. The hooks run after the release, as
    /// `finish_turn`'s do.
    pub(crate) fn settle_turn_finish(&self, withhold: impl FnOnce() -> bool) -> bool {
        let (withheld, ended, push) = {
            let _hold = self.lock_turn_hold();
            if withhold() {
                (true, false, None)
            } else {
                let (ended, push) = self.end_turn_under_hold();
                (false, ended, push)
            }
        };
        if ended {
            self.fire_turn_end_hooks();
        }
        if let Some(push) = push {
            push();
        }
        withheld
    }

    /// Register a one-shot callback for the end of this runtime's next turn.
    /// The inter-agent message brakes register here, so an in-flight slot is
    /// released at the boundary that ends it. Returns the id `off_turn_end`
    /// needs to forget a hook whose slot expired before any turn ended.
    pub(crate) fn on_turn_end(&self, callback: impl Fn() + Send + Sync + 'static) -> u64 {
        let id = self.next_turn_end_hook.fetch_add(1, Ordering::AcqRel);
        if let Ok(mut hooks) = self.turn_end_hooks.lock() {
            hooks.push(TurnEndHook {
                id,
                callback: Box::new(callback),
            });
        }
        id
    }

    /// Forget a hook that never fired, so the target's list cannot outgrow the
    /// slots that are still waiting for a boundary.
    pub(crate) fn off_turn_end(&self, id: u64) {
        if let Ok(mut hooks) = self.turn_end_hooks.lock() {
            hooks.retain(|hook| hook.id != id);
        }
    }

    /// Register a one-shot callback for the end of the turn `expected_turn_id`
    /// names, but only while that turn is still the running one.
    ///
    /// The check and the registration are one critical section under the same
    /// lock `finish_turn` takes, so a turn that ends between a caller's earlier
    /// look and this call is *observed* here instead of raced past: `None` is the
    /// answer the caller must use to decide that there is no turn to join — so
    /// the message goes as a plain prompt — and that the slot it is admitting has
    /// no boundary to be released on.
    pub(crate) fn on_turn_end_if_active(
        &self,
        expected_turn_id: u64,
        callback: impl Fn() + Send + Sync + 'static,
    ) -> Option<u64> {
        let _hold = self.lock_turn_hold();
        if !self.is_turn_active(expected_turn_id) {
            return None;
        }
        Some(self.on_turn_end(callback))
    }

    /// How many boundary hooks are armed on this runtime right now.
    /// Test-only: a hook that is never unregistered is invisible from outside.
    #[cfg(test)]
    pub(crate) fn turn_end_hook_count(&self) -> usize {
        self.turn_end_hooks
            .lock()
            .map(|hooks| hooks.len())
            .unwrap_or(0)
    }

    fn fire_turn_end_hooks(&self) {
        // Drained under this lock and called outside it: a hook takes the
        // registry's message-brake lock, and holding this list across that
        // would order two locks the messaging path does not order.
        let hooks = match self.turn_end_hooks.lock() {
            Ok(mut hooks) => std::mem::take(&mut *hooks),
            Err(_) => return,
        };
        for hook in hooks {
            (hook.callback)();
        }
    }

    /// Journal one accepted steer as the `Steered` audit row.
    ///
    /// `message_id` is the id of the `AgentUserMessage` echo this steer also
    /// published — the *same* id, so a reader can pair the transcript message
    /// with the journal row that recorded the steer. It is `None` only
    /// when there was no echo to point at (the stream refused it), never a fresh
    /// id invented here: an id that names nothing would be worse than no id.
    pub(crate) fn journal_steered(&self, message_id: Option<String>, text: String) -> bool {
        let (event, generation, seq) = {
            let Ok(mut stream) = self.lock_stream() else {
                return false;
            };
            if stream.output_closed {
                return false;
            }
            let generation = stream.generation;
            let seq = stream.next_seq;
            stream.next_seq = stream.next_seq.saturating_add(1);
            stream.last_publish = Some(Instant::now());
            (SessionEvent::Steered { message_id, text }, generation, seq)
        };
        if let Some(journal) = &self.journal {
            if let Some(record) = crate::journal::agent_report_record(
                self.session_id.clone(),
                generation,
                seq,
                &event,
            ) {
                let accepted = journal.try_append(record);
                if !accepted || journal.is_session_degraded(&self.session_id) {
                    self.mark_journal_degraded();
                }
            }
        }
        true
    }

    pub(crate) fn publish_agent_event_with_seq(
        &self,
        event: SessionEvent,
        journal_text: Option<&str>,
        event_seq: Option<u64>,
    ) -> bool {
        self.publish_agent_event_core(event, journal_text, event_seq, true)
    }

    /// Publish a finish whose turn transition the caller already performed
    /// (`settle_turn_finish`): identical to
    /// [`Self::publish_agent_event_with_seq`] except that it does NOT end the
    /// turn again. The settle-to-publish window can carry a `begin_turn` —
    /// the inter-agent brakes release at the settle — and a second transition
    /// there would end that new turn while it streams.
    pub(crate) fn publish_agent_event_settled_with_seq(
        &self,
        event: SessionEvent,
        event_seq: Option<u64>,
    ) -> bool {
        self.publish_agent_event_core(event, None, event_seq, false)
    }

    fn publish_agent_event_core(
        &self,
        event: SessionEvent,
        journal_text: Option<&str>,
        event_seq: Option<u64>,
        finish_the_turn: bool,
    ) -> bool {
        // The provider publish path's one pass over the event —
        // `publish_agent_event` and the `*_with_seq` wrappers land here. The
        // journaled daemon-owned publisher (`publish_journaled_agent_event`)
        // keeps its own path and does not. This is where the finish hook's
        // raw material is remembered without a second matching pass over the
        // stream (`S5` decisions 7 and 10).
        self.record_agent_outcome(&event);
        // One place, every publisher: a provider client writes `local` as a
        // placeholder and never has to know which device asked for the session,
        // because the request is overwritten with the session's stored origin
        // here — on the way to the journal and to every subscriber (§8b A14).
        // Overwrite, not fill-if-empty: an absent origin is not expressible, so
        // the placeholder would otherwise survive as a lie.
        let event = if matches!(&event, SessionEvent::PermissionRequest { .. }) {
            let event = super::permission_broker::stamp_origin(event, self.origin());
            super::permission_broker::stamp_chooser(event)
        } else {
            event
        };
        let was_silent;
        let journal_output;
        {
            let Ok(mut stream) = self.lock_stream() else {
                return false;
            };
            if stream.output_closed {
                // The kind only: variants carry prompt text, terminal output,
                // and permission environment values, which never reach a log.
                eprintln!(
                    "session {} dropped ACP event after EOF: {}",
                    self.session_id,
                    event.kind()
                );
                return false;
            }
            was_silent = matches!(stream.disposition, Disposition::Silent);
            if !stream.process_exited {
                stream.disposition = Disposition::Running;
            }
            stream.last_publish = Some(Instant::now());
            journal_output = journal_text.map(|text| {
                let seq = stream.next_seq;
                stream.next_seq = stream.next_seq.saturating_add(1);
                (stream.generation, seq, text.to_string())
            });
            let event_seq = event_seq.or_else(|| journal_output.as_ref().map(|(_, seq, _)| *seq));
            self.record_activity(event_seq, &event);
            if matches!(&event, SessionEvent::AgentTasks { .. }) {
                stream.agent_tasks_published = true;
            }
            enqueue_agent(&mut stream, event.clone(), event_seq);
            self.published_frames.fetch_add(1, Ordering::Relaxed);
            self.published_bytes.fetch_add(
                serde_json::to_vec(&event)
                    .map(|bytes| bytes.len())
                    .unwrap_or(0),
                Ordering::Relaxed,
            );
            notify_observers(&stream);
        }
        if let (Some(journal), Some((generation, seq, text))) = (&self.journal, journal_output) {
            let accepted = journal.try_append(output_record(
                self.session_id.clone(),
                generation,
                seq,
                text.as_bytes(),
            ));
            if !accepted || journal.is_session_degraded(&self.session_id) {
                self.mark_journal_degraded();
            }
        }
        if was_silent {
            self.notify_roster();
        }
        if finish_the_turn && matches!(&event, SessionEvent::AgentFinished { .. }) {
            self.finish_turn();
        }
        let changed = self.mark_activity_changed();
        let raised = self.raise_attention_for_event(&event);
        if changed && !raised {
            self.request_transition();
        }
        if matches!(&event, SessionEvent::AgentFinished { .. }) {
            // A turn that ended is reported here rather than through
            // attention: see [`Self::finish_notify`].
            self.notify_finished();
        }
        if self.tasks_refresh_due(&event) {
            self.notify_tasks_changed(&event);
        }
        was_silent
    }

    /// Whether the just-published provider frame can change the task list.
    /// Task frames always can; a background call arms its id. A result ends
    /// a background row only on a failed launch — success is the launch
    /// acknowledgement, and the notification that really ends it fires the
    /// refresh on its own arm below.
    fn tasks_refresh_due(&self, event: &SessionEvent) -> bool {
        match event {
            SessionEvent::AgentTaskStarted { .. }
            | SessionEvent::AgentBackgroundTasksChanged { .. } => true,
            SessionEvent::AgentTaskNotification {
                task_id,
                tool_use_id,
                ..
            } => {
                self.disarm_background(task_id);
                if let Some(id) = tool_use_id {
                    self.disarm_background(id);
                }
                true
            }
            SessionEvent::AgentToolCall {
                tool_call_id,
                background: Some(true),
                ..
            } => {
                self.arm_background(tool_call_id);
                true
            }
            SessionEvent::AgentToolUpdate {
                tool_call_id,
                status,
                ..
            } => {
                if !crate::session_tasks::background_launch_failed(status.as_deref()) {
                    return false;
                }
                self.disarm_background(tool_call_id)
            }
            _ => false,
        }
    }

    /// Arm one background launch, oldest first and bounded: past the cap
    /// the oldest launch leaves, its row already ended or the next trigger
    /// re-arms it.
    fn arm_background(&self, tool_call_id: &str) {
        if let Ok(mut armed) = self.background_tool_calls.lock() {
            if !armed.contains(&tool_call_id.to_string()) {
                armed.push_back(tool_call_id.to_string());
            }
            while armed.len() > crate::session_tasks::TASKS_MAX_ROWS {
                armed.pop_front();
            }
        }
    }

    /// Disarm one background launch: its task ended, by notification or by
    /// failed launch. True when the id was armed — the refresh fires only
    /// for a launch this runtime saw start.
    fn disarm_background(&self, tool_call_id: &str) -> bool {
        self.background_tool_calls
            .lock()
            .map(|mut armed| {
                armed
                    .iter()
                    .position(|id| id == tool_call_id)
                    .map(|index| armed.remove(index).is_some())
                    .unwrap_or(false)
            })
            .unwrap_or(false)
    }

    /// Whether a task-list derive may run now: outside the debounce window.
    /// Records the run, so the trailing scheduler cannot double-derive.
    pub(crate) fn tasks_derive_due(&self) -> bool {
        let now = Instant::now();
        let Ok(mut last) = self.tasks_last_refresh.lock() else {
            return true;
        };
        if super::session_task_list::refresh_due(*last, now) {
            *last = Some(now);
            return true;
        }
        false
    }

    /// Defer one trailing refresh to the window's end. At most one per
    /// session is ever scheduled: the flag is set before spawning, and the
    /// thread clears it by running the ordinary refresh path, so the last
    /// state is always published exactly once.
    pub(crate) fn schedule_tasks_trailing(
        &self,
        registry: super::SessionRegistry,
        session_id: String,
    ) {
        if self.tasks_trailing_pending.swap(true, Ordering::SeqCst) {
            return;
        }
        std::thread::Builder::new()
            .name("tasks-trailing-refresh".to_string())
            .spawn(move || {
                std::thread::sleep(super::session_task_list::TASKS_REFRESH_DEBOUNCE);
                registry.clear_tasks_trailing(&session_id);
                registry.refresh_session_tasks(&session_id, &[]);
            })
            .ok();
    }

    /// Clear a scheduled trailing refresh, called by the scheduled run as
    /// it starts: a newer trigger may schedule the next one from here.
    pub(crate) fn clear_tasks_trailing_flag(&self) {
        self.tasks_trailing_pending.store(false, Ordering::SeqCst);
    }

    /// Test seam: forget the last derive and any scheduled trailing run, so
    /// a test that asserts on the published snapshot derives deterministically
    /// instead of waiting out the debounce window.
    #[cfg(test)]
    pub(crate) fn test_reset_tasks_throttle(&self) {
        if let Ok(mut last) = self.tasks_last_refresh.lock() {
            *last = None;
        }
        self.tasks_trailing_pending.store(false, Ordering::SeqCst);
    }

    /// Test seam: the armed background launches, oldest first.
    #[cfg(test)]
    pub(crate) fn test_armed_background_calls(&self) -> Vec<String> {
        self.background_tool_calls
            .lock()
            .map(|armed| armed.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// Test seam: rows stashed for the trailing run.
    #[cfg(test)]
    pub(crate) fn test_pending_tasks_extra(&self) -> Vec<(SessionEvent, u64)> {
        self.tasks_pending_extra
            .lock()
            .map(|pending| pending.clone())
            .unwrap_or_default()
    }

    /// Stash triggering rows for the scheduled trailing run: the journal
    /// may not have landed them yet when it derives. Bounded — the journal
    /// is the primary source, this covers the commit gap.
    pub(crate) fn stash_tasks_extra(&self, extra: &[(SessionEvent, u64)]) {
        if extra.is_empty() {
            return;
        }
        if let Ok(mut pending) = self.tasks_pending_extra.lock() {
            pending.extend(extra.iter().cloned());
            while pending.len() > super::session_task_list::TASKS_PENDING_EXTRA_MAX {
                pending.remove(0);
            }
        }
    }

    /// Drain the stashed triggering rows for one derive.
    pub(crate) fn take_tasks_pending(&self) -> Vec<(SessionEvent, u64)> {
        self.tasks_pending_extra
            .lock()
            .map(|mut pending| std::mem::take(&mut *pending))
            .unwrap_or_default()
    }

    /// Record a derive that bypassed the debounce gate (the urgent exit
    /// path), so the window stays coherent after it.
    pub(crate) fn mark_tasks_derived(&self) {
        if let Ok(mut last) = self.tasks_last_refresh.lock() {
            *last = Some(Instant::now());
        }
    }

    /// Mark the exit task publish settled: the pull path may synthesize
    /// Exit from here on. Called on every urgent attempt, sent or not — a
    /// session with no task list, or a failed derive, must not hold its own
    /// death forever; the 2 s fallback covers a thread that never finishes.
    pub(crate) fn mark_tasks_exit_published(&self) {
        if let Ok(mut stream) = self.lock_stream() {
            stream.tasks_exit_published = true;
        }
    }

    /// Whether the exit task snapshot is already accepted by the stream.
    pub(crate) fn tasks_exit_sent(&self) -> bool {
        self.lock_stream()
            .map(|stream| stream.tasks_exit_sent)
            .unwrap_or(false)
    }

    /// The next task-list revision for this session, counting from 1.
    pub(crate) fn next_tasks_revision(&self) -> u64 {
        self.tasks_revision.fetch_add(1, Ordering::SeqCst) + 1
    }

    /// The wall time the process was observed dead, when recorded.
    pub(crate) fn tasks_ended_wall_ms(&self) -> Option<u64> {
        self.tasks_ended_wall_ms.lock().ok().and_then(|wall| *wall)
    }

    pub(crate) fn accept_agent_report(
        &self,
        report: crate::agent_report::AgentReport,
    ) -> Result<bool, WireError> {
        let journaled;
        {
            let Ok(mut stream) = self.lock_stream() else {
                return Err(internal("Session state is unavailable."));
            };
            match stream.agent_reports.apply(report.clone()) {
                Ok(false) => return Ok(false),
                Ok(true) => {}
                Err(error) => return Err(error),
            }
            let seq = stream.next_seq;
            stream.next_seq = stream.next_seq.saturating_add(1);
            stream.last_publish = Some(Instant::now());
            let event = SessionEvent::AgentReported {
                seq,
                source: report.source,
                agent: report.agent,
                state: report.state,
                message: report.message,
                report_seq: report.seq,
                agent_session_id: report.agent_session_id,
                agent_session_path: report.agent_session_path,
                session_start_source: report.session_start_source,
            };
            self.record_activity(Some(seq), &event);
            enqueue_agent(&mut stream, event.clone(), Some(seq));
            notify_observers(&stream);
            journaled = (stream.generation, seq, event);
        }
        if let Some(journal) = &self.journal {
            if let Some(record) = crate::journal::agent_report_record(
                self.session_id.clone(),
                journaled.0,
                journaled.1,
                &journaled.2,
            ) {
                let accepted = journal.try_append(record);
                if !accepted || journal.is_session_degraded(&self.session_id) {
                    self.mark_journal_degraded();
                }
            }
        }
        Ok(true)
    }

    pub(crate) fn permission_broker(&self) -> Option<Arc<PermissionBroker>> {
        self.permission_broker.as_ref().map(Arc::clone)
    }

    /// Whether a prompt turn is running right now. The attention hooks' own
    /// question, asked without a turn id.
    pub(crate) fn is_running_turn(&self) -> bool {
        self.turn_active.load(Ordering::Acquire)
    }

    /// Whether a permission card is parked right now. Ground truth for the
    /// Blocked headline; a hook cannot clear it.
    pub(crate) fn permission_pending(&self) -> bool {
        self.permission_broker()
            .is_some_and(|broker| broker.pending_len() > 0)
    }

    /// Capture the roster now; the returned closure pushes that immutable
    /// snapshot after the caller releases its state lock.
    fn prepare_roster_transition(&self) -> Option<Box<dyn FnOnce() + Send>> {
        let prepare = self
            .attention_hooks
            .lock()
            .ok()
            .and_then(|hooks| hooks.as_ref().map(|hooks| Arc::clone(&hooks.prepare)));
        prepare.map(|prepare| prepare())
    }

    /// Append one metadata mark. Called by the central agent-event
    /// publishers only, so the feed holds the supervision-relevant stream —
    /// published agent events, newest last — without keeping any text. It is
    /// not a mirror of journal order: paths that consume a sequence without
    /// publishing an agent event (a steer audit row, a session notice, a raw
    /// envelope, terminal output) take no mark, so `last_seq` can run ahead
    /// of the last mark.
    pub(crate) fn record_activity(&self, seq: Option<u64>, event: &SessionEvent) {
        let mark = crate::agent_activity::ActivityMark {
            seq,
            kind: crate::agent_activity::event_kind(event),
            ts_ms: crate::agent_activity::wall_now_ms(),
        };
        if let Ok(mut feed) = self.activity_feed.lock() {
            feed.push_back(mark);
            while feed.len() > crate::agent_activity::ACTIVITY_FEED_CAP {
                feed.pop_front();
            }
        }
    }

    /// Tail of the feed, oldest first, at most `limit` marks. The caller
    /// clamps the limit; this never touches the journal.
    pub(crate) fn recent_activity(&self, limit: usize) -> Vec<crate::agent_activity::ActivityMark> {
        self.activity_feed
            .lock()
            .map(|feed| {
                let skip = feed.len().saturating_sub(limit);
                feed.iter().skip(skip).copied().collect()
            })
            .unwrap_or_default()
    }

    /// Time since the last publish at `now`. `None` never published (besides
    /// the creation stamp): the quiet rule reads that as Unknown, not idle.
    pub(crate) fn activity_idle_at(&self, now: Instant) -> Option<Duration> {
        self.lock_stream()
            .ok()
            .and_then(|stream| stream.last_publish)
            .map(|last| now.saturating_duration_since(last))
    }

    /// Last allocated stream sequence, for the activity answer's `lastSeq`.
    /// It can run ahead of the feed's last mark: unmarked paths consume
    /// sequences too (see `record_activity`).
    pub(crate) fn last_seq(&self) -> u64 {
        self.lock_stream()
            .map(|stream| stream.next_seq.saturating_sub(1))
            .unwrap_or(0)
    }

    /// Hook headline without its text: state plus the hook's own seq. The
    /// activity answer carries this beside the derived state, never merged.
    pub(crate) fn hook_activity(&self) -> Option<(AgentActivityState, Option<u64>)> {
        self.lock_stream()
            .ok()
            .and_then(|stream| stream.agent_reports.last_state())
    }

    /// The last `AgentMessage` this provider published, chunks of one message
    /// already joined.
    pub(crate) fn agent_message_snapshot(&self) -> Option<AgentMessageSnapshot> {
        self.agent_message.lock().ok().and_then(|slot| slot.clone())
    }

    /// The last `AgentFinished` stop reason, or `None` when the provider never
    /// reported one.
    pub(crate) fn agent_stop_reason(&self) -> Option<String> {
        self.agent_stop_reason
            .lock()
            .ok()
            .and_then(|slot| slot.clone())
    }

    /// Record a stop before the process is signalled. Decided under the stop
    /// reason's lock, which `begin_turn` also takes, so a turn end recorded
    /// first keeps its verdict and a turn starting after this is still judged.
    pub(crate) fn request_stop(&self) {
        if let Ok(slot) = self.agent_stop_reason.lock() {
            self.stop_signalled.store(true, Ordering::Release);
            if slot.is_none() {
                self.stop_requested.store(true, Ordering::Release);
            }
        }
    }

    pub(crate) fn stop_requested(&self) -> bool {
        self.stop_requested.load(Ordering::Acquire)
    }

    /// Remember what the finish hook needs from the event stream (`S5`
    /// decisions 7 and 10).
    ///
    /// Chunks of one message carry one `message_id`, so a chunk whose id is this
    /// session's remembered one *continues* the message and any other chunk
    /// starts a new one. A provider that sends `null` ids therefore accumulates
    /// one message until it says something else — the same shape the transcript
    /// renders, which is the point: the summary and the artifact are the last
    /// message a person can see.
    fn record_agent_outcome(&self, event: &SessionEvent) {
        match event {
            SessionEvent::AgentMessage {
                message_id, text, ..
            } => {
                // An image-only message says nothing; it must not become a
                // child's outcome or clobber one that carried text.
                if text.is_empty() {
                    return;
                }
                let Ok(mut slot) = self.agent_message.lock() else {
                    return;
                };
                match slot.as_mut() {
                    Some(current) if current.message_id == *message_id => {
                        current.text.push_str(text);
                    }
                    _ => {
                        *slot = Some(AgentMessageSnapshot {
                            message_id: message_id.clone(),
                            text: text.clone(),
                        });
                    }
                }
            }
            SessionEvent::AgentFinished { stop_reason, .. } => {
                if let Ok(mut slot) = self.agent_stop_reason.lock() {
                    *slot = Some(stop_reason.clone());
                }
            }
            _ => {}
        }
    }

    /// Publish the creation record on the **creator's** transcript and journal
    /// it as an agent report row (`S5` §1).
    ///
    /// The id is built by the publisher for the same reason
    /// [`Self::publish_agent_user_message`]'s is: the event the caller sees and
    /// the row that links to it name one message.
    ///
    /// Returns the published event so the caller can fold it into the
    /// creator's task list without waiting for the journal write.
    pub(crate) fn publish_child_created(
        &self,
        child_session_id: &str,
        display_name: &str,
        provider: &str,
        profile: &str,
    ) -> Option<SessionEvent> {
        self.publish_journaled_agent_event(|generation, seq, _| SessionEvent::AgentCreated {
            message_id: Some(format!("devboule-agent-created-{generation}-{seq}")),
            child_session_id: child_session_id.to_string(),
            display_name: display_name.to_string(),
            provider: provider.to_string(),
            profile: profile.to_string(),
        })
    }

    /// Publish that a created child is running again (`AgentResumed`), on the
    /// creator's journal. Returns the event for the caller to fold.
    pub(crate) fn publish_child_resumed(
        &self,
        child_session_id: &str,
        display_name: &str,
    ) -> Option<SessionEvent> {
        self.publish_journaled_agent_event(|_, _, _| SessionEvent::AgentResumed {
            child_session_id: child_session_id.to_string(),
            display_name: display_name.to_string(),
        })
    }

    /// Publish the structured finish record beside the text message, with the
    /// **same** `message_id` (`S5` §3, rev 4).
    ///
    /// `Some(id)` is what the caller wants: the app correlates the two records
    /// on it, which is why this one is not published through the id-building
    /// sibling above — the id is the text message's, handed in.
    ///
    /// Returns the published event so the caller can fold it into the
    /// creator's task list without waiting for the journal write.
    pub(crate) fn publish_child_finished(
        &self,
        message_id: Option<String>,
        child_session_id: &str,
        display_name: &str,
        state: devboule_protocol::AgentTaskState,
        note: Option<String>,
        artifacts: Vec<devboule_protocol::FinishArtifact>,
    ) -> Option<SessionEvent> {
        let event = SessionEvent::ChildFinished {
            message_id,
            child_session_id: child_session_id.to_string(),
            display_name: display_name.to_string(),
            state,
            note,
            artifacts,
        };
        // Journaled like the creation record: the creator's journal is what
        // an unattached Workspace, or one that restarts, reads the finish
        // out of, so the structured record cannot live on the stream alone.
        self.publish_journaled_agent_event(|_, _, _| event)
    }

    pub(crate) fn can_publish_agent_event(&self) -> bool {
        self.lock_stream()
            .map(|stream| !stream.output_closed)
            .unwrap_or(false)
    }

    /// A prompt's write is under way for this session: taken under the
    /// session map lock, next to the writer it resolved, and given back when
    /// the send returns — the map lock is what makes the mark and the
    /// idle-close section's removal two things that cannot interleave.
    pub(crate) fn begin_delivery(&self) {
        self.deliveries_in_flight.fetch_add(1, Ordering::AcqRel);
    }

    pub(crate) fn end_delivery(&self) {
        self.deliveries_in_flight.fetch_sub(1, Ordering::Release);
    }

    /// Whether a prompt is being written to this session right now — the
    /// fifth way a child is not idle, for the road that arms no brake.
    pub(crate) fn delivery_in_flight(&self) -> bool {
        self.deliveries_in_flight.load(Ordering::Acquire) > 0
    }

    /// `None` means no client is attached. A pending request is retained until
    /// resolved so a later capable attach can display it; `Some(false)` means
    /// observers exist but none negotiated typed permissions.
    pub(crate) fn permission_delivery_enabled(&self) -> Option<bool> {
        self.lock_stream().ok().and_then(|stream| {
            (!stream.observers.is_empty())
                .then(|| stream.observers.values().any(|a| a.typed_permissions))
        })
    }

    pub(crate) fn remove_permission_request(&self, tool_call_id: &str) {
        let Ok(mut stream) = self.lock_stream() else {
            return;
        };
        {
            for attachment in stream.observers.values_mut() {
                remove_permission_from_queue(
                    &mut attachment.pending,
                    &mut attachment.pending_bytes,
                    &mut attachment.pending_frames,
                    tool_call_id,
                );
            }
        }
        {
            let StreamState {
                agent_backlog,
                agent_backlog_bytes,
                agent_backlog_frames,
                ..
            } = &mut *stream;
            remove_permission_from_queue(
                agent_backlog,
                agent_backlog_bytes,
                agent_backlog_frames,
                tool_call_id,
            );
        }
        notify_observers(&stream);
    }

    /// Clear a standing permission attention once its last card is gone.
    /// Withdrawal doors only: answers keep their own clear and its push.
    /// Lock order attention -> broker pending; the withdrawal doors call it
    /// after take()/drain() released the table, holding neither.
    pub(crate) fn clear_permission_attention_if_idle(&self) -> bool {
        let Ok(mut attention) = self.attention.lock() else {
            return false;
        };
        if !attention
            .as_ref()
            .is_some_and(|current| current.reason == AttentionReason::Permission)
        {
            return false;
        }
        // Re-checked under the attention lock: a card parking now inserts
        // before its raise lands, so it either stops this clear or raises
        // after it — its attention survives either way.
        if self.permission_pending() {
            return false;
        }
        attention.take();
        drop(attention);
        self.request_transition();
        true
    }

    /// Whether this session's journal already holds a decision for this
    /// request id — the register-time refusal reads it: the permissions row
    /// is write-once per (session, id), so a second card for an answered id
    /// could never record its own answer. No journal means no decisions, and
    /// a sick journal does not block registration either — the answer's own
    /// write is where a journal failure is already reported.
    pub(crate) fn permission_already_recorded(&self, tool_call_id: &str) -> bool {
        self.journal.as_ref().is_some_and(|journal| {
            journal
                .permission_was_recorded_in_session(&self.session_id, tool_call_id)
                .unwrap_or(false)
        })
    }

    pub(crate) fn record_permission_decision(
        &self,
        tool_call_id: &str,
        outcome: &str,
        request: &SessionEvent,
    ) -> bool {
        let Some(journal) = &self.journal else {
            return false;
        };
        let Ok(payload) = serde_json::to_vec(request) else {
            self.mark_journal_degraded();
            return false;
        };
        match journal.record_permission(&self.session_id, tool_call_id, outcome, &payload) {
            Ok(()) => true,
            Err(_) => {
                self.mark_journal_degraded();
                false
            }
        }
    }

    /// Forward emulator-generated replies to the PTY input side. Best
    /// effort: a dead PTY has a dead reader that ends the session anyway.
    pub(crate) fn write_pty_replies(&self, replies: &[String]) {
        if replies.is_empty() {
            return;
        }
        let Some(writer) = self.pty_writer.get() else {
            eprintln!(
                "session {} dropped {} terminal query replies: PTY writer unavailable",
                self.session_id,
                replies.len()
            );
            return;
        };
        let Ok(mut writer) = writer.lock() else {
            self.mark_terminal_dead("PTY writer lock poisoned");
            return;
        };
        for reply in replies {
            if let Err(error) = writer.write_all(reply.as_bytes()) {
                eprintln!(
                    "session {} could not answer a terminal query: {error}",
                    self.session_id
                );
                return;
            }
        }
        if let Err(error) = writer.flush() {
            eprintln!(
                "session {} could not flush a terminal query reply: {error}",
                self.session_id
            );
        }
    }

    pub(crate) fn record_output_loss(&self) {
        let Ok(stream) = self.lock_stream() else {
            return;
        };
        // Bytes lost between the reader and the emulator were never applied
        // to the screen and never journalled, so no sequence is consumed:
        // seq counts applied chunks, and the boundary stays an honest
        // statement about the emulator. The lost bytes are simply absent
        // from the transcript.
        notify_observers(&stream);
    }

    pub(crate) fn output_metrics(&self) -> OutputMetrics {
        OutputMetrics {
            peak_pending_bytes: self.peak_pending_bytes.load(Ordering::Relaxed) as u64,
            coalesced_bytes: self.coalesced_bytes.load(Ordering::Relaxed),
            coalesced_frames: self.coalesced_frames.load(Ordering::Relaxed),
        }
    }

    pub(crate) fn mark_journal_degraded(&self) {
        if let Some(journal) = &self.journal {
            let (frames, bytes) = journal.session_drop_counters(&self.session_id);
            self.journal_dropped_frames
                .fetch_max(frames, Ordering::AcqRel);
            self.journal_dropped_bytes
                .fetch_max(bytes, Ordering::AcqRel);
        }
        if !self.journal_degraded.swap(true, Ordering::AcqRel) {
            if let Some(journal) = &self.journal {
                journal.note_session_degraded(&self.session_id);
            }
        }
        self.refresh_exit_integrity();
        self.notify_attachment();
    }

    /// Mark the running stream silent at an injected observation time. The
    /// monitor uses `Instant::now`; the parameter keeps the transition
    /// boundary deterministic in unit tests.
    pub(crate) fn mark_silent_if_due(&self, now: Instant) -> Option<u64> {
        let elapsed_ms;
        {
            let mut stream = self.lock_stream().ok()?;
            if stream.process_exited || !matches!(stream.disposition, Disposition::Running) {
                return None;
            }
            let last_publish = stream.last_publish?;
            let elapsed = now.saturating_duration_since(last_publish);
            if elapsed <= SESSION_SILENCE_THRESHOLD {
                return None;
            }
            elapsed_ms = elapsed.as_millis().try_into().unwrap_or(u64::MAX);
            stream.disposition = Disposition::Silent;
            for attachment in stream.observers.values_mut() {
                attachment.pending_silences.push_back(elapsed_ms);
            }
        }
        self.notify_attachment();
        Some(elapsed_ms)
    }

    pub(crate) fn install_os_handle(&self, handle: ProcessHandle) {
        if let Ok(mut slot) = self.os_handle.lock() {
            *slot = Some(handle);
        }
    }

    /// Observe the OS process handle. Returns true when this call newly
    /// marked the session exited. Does not wait on pipe EOF or Child::wait.
    pub(crate) fn observe_os_liveness(&self) -> bool {
        if self.process_exited() {
            return false;
        }
        let Ok(slot) = self.os_handle.lock() else {
            return false;
        };
        let Some(handle) = slot.as_ref() else {
            return false;
        };
        if handle.is_alive() {
            return false;
        }
        let code = handle.exit_code();
        drop(slot);
        self.mark_exited(code);
        true
    }

    pub(crate) fn set_on_os_death(&self, callback: Arc<dyn Fn() + Send + Sync>) {
        if let Ok(mut slot) = self.on_os_death.lock() {
            *slot = Some(callback);
        }
    }

    /// Drop the cascade without firing it: teardown's move after a failed job
    /// termination, where its own Arc drops right after this one — the
    /// closure's Arc going first is what lets that drop be the job's last
    /// handle. `fire_os_death`'s detached thread and `stop` may hold the Arc
    /// past this point; every holder runs its own `terminate()`, so a close
    /// this misses arrives with the last of them.
    pub(crate) fn release_on_os_death(&self) {
        if let Ok(mut slot) = self.on_os_death.lock() {
            *slot = None;
        }
    }

    pub(crate) fn fire_os_death(&self) {
        if self.os_death_started.swap(true, Ordering::AcqRel) {
            return;
        }
        let callback = self.on_os_death.lock().ok().and_then(|slot| slot.clone());
        let Some(callback) = callback else {
            return;
        };
        let id = self.session_id.clone();
        let spawned = callback.clone();
        if let Err(error) = std::thread::Builder::new()
            .name(format!("session-os-death-{id}"))
            .spawn(move || spawned())
        {
            eprintln!("session {id} could not detach OS-death cascade: {error}");
            callback();
        }
    }

    pub(crate) fn set_roster_notify(&self, callback: Arc<dyn Fn() + Send + Sync>) {
        if let Ok(mut slot) = self.roster_notify.lock() {
            *slot = Some(callback);
        }
    }

    /// Set the hook that runs once per published `AgentFinished` (`S5` §3).
    pub(crate) fn set_finish_notify(&self, callback: Arc<dyn Fn() + Send + Sync>) {
        if let Ok(mut slot) = self.finish_notify.lock() {
            *slot = Some(callback);
        }
    }

    fn notify_finished(&self) {
        let callback = self.finish_notify.lock().ok().and_then(|slot| slot.clone());
        if let Some(callback) = callback {
            callback();
        }
    }

    pub(crate) fn set_attention_hooks(
        &self,
        suppressed: Arc<dyn Fn() -> bool + Send + Sync>,
        prepare: Arc<dyn Fn() -> Box<dyn FnOnce() + Send> + Send + Sync>,
    ) {
        if let Ok(mut hooks) = self.attention_hooks.lock() {
            *hooks = Some(AttentionHooks {
                suppressed,
                prepare,
            });
        }
    }

    /// Install the delegated-surfacing observer. The registry installs it
    /// where it installs the attention hooks: one place, at birth, with the
    /// child's own facts in scope.
    pub(crate) fn set_permission_park_hook(&self, hook: PermissionParkHook) {
        if let Ok(mut slot) = self.permission_park_hook.lock() {
            *slot = Some(hook);
        }
    }

    /// Called by the permission broker when a card parks. Best effort and
    /// silent on a missing hook: a session the registry never dressed (a
    /// test runtime) simply surfaces nothing.
    pub(crate) fn notify_permission_park(&self, request: &SessionEvent) {
        let hook = self
            .permission_park_hook
            .lock()
            .ok()
            .and_then(|slot| slot.clone());
        if let Some(hook) = hook {
            hook(request);
        }
    }

    /// Install the task-list refresh observer. The registry installs it
    /// where it installs the park hook: one place, at birth. Installing
    /// arms the exit wait: from here on the pull holds Exit for the exit
    /// publish, so a runtime without a hook keeps the default `true` and
    /// never waits. The arm applies only when the hook is actually stored:
    /// a poisoned slot stores nothing, so it must not arm a wait nothing
    /// can satisfy.
    pub(crate) fn set_tasks_refresh_hook(&self, hook: TasksRefreshHook) {
        let stored = if let Ok(mut slot) = self.tasks_refresh_hook.lock() {
            *slot = Some(hook);
            true
        } else {
            false
        };
        if stored {
            if let Ok(mut stream) = self.lock_stream() {
                stream.tasks_exit_published = false;
            }
        }
    }

    /// Called after a provider frame that can change the task list
    /// publishes. Best effort and silent on a missing hook, like the park
    /// notify above; the stream lock is released before this runs, so the
    /// refresh's registry read cannot nest inside a publish.
    fn notify_tasks_changed(&self, event: &SessionEvent) {
        let hook = self
            .tasks_refresh_hook
            .lock()
            .ok()
            .and_then(|slot| slot.clone());
        if let Some(hook) = hook {
            hook(Some(event));
        }
    }

    /// Called when this session's process is observed dead: the task list
    /// refresh runs against the ended state, cancelling what still ran.
    /// Best effort and silent like the notify above.
    fn notify_tasks_ended(&self) {
        let hook = self
            .tasks_refresh_hook
            .lock()
            .ok()
            .and_then(|slot| slot.clone());
        if let Some(hook) = hook {
            hook(None);
        }
    }

    pub(crate) fn attention(&self) -> Option<Attention> {
        self.attention.lock().ok().and_then(|attention| *attention)
    }

    pub(crate) fn clear_attention(&self) -> bool {
        let Ok(mut attention) = self.attention.lock() else {
            return false;
        };
        attention.take().is_some()
    }

    /// Whether the attention step of a publication pushed this session's row. It
    /// returns false when presence suppressed the raise and when an existing
    /// raise of equal or higher priority was kept — the cases where a status
    /// change would otherwise go unannounced.
    fn raise_attention_for_event(&self, event: &SessionEvent) -> bool {
        let reason = match event {
            SessionEvent::AgentFinished { .. } => AttentionReason::Finished,
            SessionEvent::AgentError { .. } => AttentionReason::Error,
            SessionEvent::PermissionRequest { .. } => AttentionReason::Permission,
            _ => return false,
        };
        self.raise_attention(reason)
    }

    /// Whether the raise took the row to a client: `false` on every path that
    /// stayed silent — no hooks, a suppressed presence, a raise already standing
    /// at equal or higher priority — which is exactly when a status change has to
    /// speak for itself.
    fn raise_attention(&self, reason: AttentionReason) -> bool {
        let hooks = self
            .attention_hooks
            .lock()
            .ok()
            .and_then(|hooks| hooks.as_ref().map(|hooks| Arc::clone(&hooks.suppressed)));
        // Attention is the transaction guard: hold it while consulting
        // presence so the suppression decision and the write cannot be
        // separated by a focus update. The global order is attention ->
        // presence; set_presence releases its presence guard before it calls
        // clear_attention, so it never holds these locks in reverse.
        let Ok(mut attention) = self.attention.lock() else {
            return false;
        };
        if hooks.as_ref().is_some_and(|suppressed| suppressed()) {
            return false;
        }
        if attention
            .as_ref()
            .is_some_and(|current| current.reason.priority() >= reason.priority())
        {
            return false;
        }
        *attention = Some(Attention {
            reason,
            at_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
                .try_into()
                .unwrap_or(u64::MAX),
        });
        drop(attention);
        self.request_transition();
        hooks.is_some()
    }

    pub(crate) fn notify_roster(&self) {
        if !self.transition_ready() {
            return;
        }
        if let Ok(slot) = self.roster_notify.lock() {
            if let Some(callback) = slot.as_ref() {
                callback();
            }
        }
    }

    pub(crate) fn refresh_journal_degradation(&self) {
        if self
            .journal
            .as_ref()
            .is_some_and(|journal| journal.is_session_degraded(&self.session_id))
        {
            self.mark_journal_degraded();
        }
    }

    pub(crate) fn journal_degraded(&self) -> bool {
        self.journal_degraded.load(Ordering::Acquire)
    }

    pub(crate) fn journal_degraded_event(&self) -> SessionEvent {
        SessionEvent::JournalDegraded {
            dropped_frames: self.journal_dropped_frames.load(Ordering::Acquire),
            dropped_bytes: self.journal_dropped_bytes.load(Ordering::Acquire),
        }
    }

    pub(crate) fn session_manifest(&self) -> Option<SessionEvent> {
        self.session_manifest
            .lock()
            .ok()
            .and_then(|stored| stored.clone())
    }

    /// The provider serving the stored manifest's current model, when the agent reported one.
    pub(crate) fn current_model_provider_id(&self) -> Option<String> {
        match self.session_manifest()? {
            SessionEvent::SessionManifest {
                current_model_provider_id,
                ..
            } => current_model_provider_id,
            _ => None,
        }
    }

    pub(crate) fn current_mode_id(&self) -> Option<String> {
        match self.session_manifest()? {
            SessionEvent::SessionManifest {
                modes: Some(modes), ..
            } => Some(modes.current_mode_id),
            _ => None,
        }
    }

    pub(crate) fn mode_before_plan_id(&self) -> Option<String> {
        self.mode_before_plan
            .lock()
            .ok()
            .and_then(|mode| mode.clone())
    }

    pub(crate) fn set_current_mode_id(&self, mode_id: &str) -> Result<(), WireError> {
        if !self.can_publish_agent_event() {
            return Err(WireError::new(
                ErrorCode::Io,
                "Session event stream is unavailable; mode change was not applied.",
            ));
        }
        let mut stored = self.session_manifest.lock().map_err(|_| {
            WireError::new(
                ErrorCode::Io,
                "Session manifest is unavailable; mode change was not applied.",
            )
        })?;
        let Some(manifest) = stored.as_mut() else {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "Session mode manifest is missing; mode change was not applied.",
            ));
        };
        let (previous_mode, modes) = match manifest {
            SessionEvent::SessionManifest {
                modes: Some(modes), ..
            } => (modes.current_mode_id.clone(), modes),
            _ => {
                return Err(WireError::new(
                    ErrorCode::InvalidRequest,
                    "Session mode state is missing; mode change was not applied.",
                ));
            }
        };
        let mut mode_before_plan = self.mode_before_plan.lock().map_err(|_| {
            WireError::new(
                ErrorCode::Io,
                "Pre-plan mode history is unavailable; mode change was not applied.",
            )
        })?;
        Self::update_mode_before_plan(&mut mode_before_plan, Some(&previous_mode), mode_id);
        modes.current_mode_id = mode_id.to_string();
        let manifest = manifest.clone();
        drop(mode_before_plan);
        drop(stored);
        // The stream preflight decides whether to mutate; publication can race with EOF.
        let _was_silent = self.publish_agent_event(manifest, None);
        Ok(())
    }

    pub(crate) fn record_claude_mode_report(&self, mode_id: &str) -> Result<(), WireError> {
        if !self.can_publish_agent_event() {
            return Err(WireError::new(
                ErrorCode::Io,
                "Session event stream is unavailable; reported mode was not applied.",
            ));
        }
        let mut reported_mode = self.claude_reported_mode.lock().map_err(|_| {
            WireError::new(
                ErrorCode::Io,
                "Claude reported-mode history is unavailable; mode was not applied.",
            )
        })?;
        let mut mode_before_plan = self.mode_before_plan.lock().map_err(|_| {
            WireError::new(
                ErrorCode::Io,
                "Claude pre-plan mode history is unavailable; mode was not applied.",
            )
        })?;
        Self::update_mode_before_plan(&mut mode_before_plan, reported_mode.as_deref(), mode_id);
        *reported_mode = Some(mode_id.to_string());
        Ok(())
    }

    pub(crate) fn set_claude_reported_mode_id(&self, mode_id: &str) -> Result<(), WireError> {
        if !self.can_publish_agent_event() {
            return Err(WireError::new(
                ErrorCode::Io,
                "Session event stream is unavailable; reported mode was not applied.",
            ));
        }
        let mut stored = self.session_manifest.lock().map_err(|_| {
            WireError::new(
                ErrorCode::Io,
                "Session manifest is unavailable; reported mode was not applied.",
            )
        })?;
        let Some(manifest) = stored.as_mut() else {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "Claude mode manifest is missing; reported mode was not applied.",
            ));
        };
        let modes = match manifest {
            SessionEvent::SessionManifest {
                provider_id: Some(provider_id),
                modes: Some(modes),
                ..
            } if provider_id == "claude" => modes,
            _ => {
                return Err(WireError::new(
                    ErrorCode::InvalidRequest,
                    "Claude mode state is missing; reported mode was not applied.",
                ));
            }
        };
        let mut reported_mode = self.claude_reported_mode.lock().map_err(|_| {
            WireError::new(
                ErrorCode::Io,
                "Claude reported-mode history is unavailable; mode was not applied.",
            )
        })?;
        let mut mode_before_plan = self.mode_before_plan.lock().map_err(|_| {
            WireError::new(
                ErrorCode::Io,
                "Claude pre-plan mode history is unavailable; mode was not applied.",
            )
        })?;
        Self::update_mode_before_plan(&mut mode_before_plan, reported_mode.as_deref(), mode_id);
        *reported_mode = Some(mode_id.to_string());
        modes.current_mode_id = mode_id.to_string();
        let manifest = manifest.clone();
        drop(mode_before_plan);
        drop(reported_mode);
        drop(stored);
        // The stream preflight decides whether to mutate; publication can race with EOF.
        let _was_silent = self.publish_agent_event(manifest, None);
        Ok(())
    }

    /// What this session's mode says about a write-shaped act
    /// ([`crate::provider_catalog::ModeGate`]): read at call time, never
    /// cached. A session with no kind or no mode fails toward the card.
    pub(crate) fn mode_gate(&self) -> crate::provider_catalog::ModeGate {
        crate::provider_catalog::mode_gate_for(self.agent_kind(), self.current_mode_id().as_deref())
    }

    pub(crate) fn set_agent_kind(&self, kind: SessionKind) {
        if let Ok(mut stored) = self.agent_kind.lock() {
            *stored = Some(kind);
        }
    }

    /// The session's current goal, as `/goal` last stored it. Runtime-only,
    /// like attention: the journal column is the durable copy, and a restart
    /// seeds this back from it without publishing — the transcript already
    /// holds the `GoalChanged` event that said it.
    pub(crate) fn goal(&self) -> Option<String> {
        self.goal.lock().ok().and_then(|stored| stored.clone())
    }

    pub(crate) fn set_goal(&self, goal: Option<String>) {
        if let Ok(mut stored) = self.goal.lock() {
            *stored = goal;
        }
    }

    /// Install the session's origin. Called once, by the registry, right after
    /// the runtime exists; a second call is ignored rather than a panic,
    /// because the value is a fact about the session and both writers would
    /// have the same one.
    pub(crate) fn set_origin(&self, origin: SessionOrigin) {
        let _ = self.origin.set(origin);
    }

    /// The session's origin. `Unknown` before the registry has installed one:
    /// `local` is measured for a session this machine created, never assumed,
    /// so a runtime nobody told — a test-built one, or a session whose create
    /// never reached the registry — cannot have its cards or its session row
    /// read as this machine's own.
    pub(crate) fn origin(&self) -> SessionOrigin {
        self.origin
            .get()
            .cloned()
            .unwrap_or_else(SessionOrigin::unknown)
    }

    pub(crate) fn agent_kind(&self) -> Option<SessionKind> {
        self.agent_kind
            .lock()
            .ok()
            .and_then(|stored| stored.clone())
    }

    pub(crate) fn claude_catalog_state(&self) -> crate::claude_catalog::ClaudeCatalogState {
        self.claude_catalog_state
            .lock()
            .map(|state| *state)
            .unwrap_or(crate::claude_catalog::ClaudeCatalogState::Provisional)
    }

    pub(crate) fn has_journal(&self) -> bool {
        self.journal.is_some()
    }

    pub(crate) fn current_agent_seq(&self) -> u64 {
        self.lock_stream()
            .map(|stream| stream.next_seq.saturating_sub(1))
            .unwrap_or_default()
    }

    pub(crate) fn finish_live_agent_replay(
        &self,
        key: AttachmentKey,
        from_seq: u64,
        replayed_seqs: &HashSet<u64>,
        reset_tail_cards: Option<&HashSet<String>>,
    ) -> (u64, Option<SessionEvent>) {
        // Publication stores a manifest before it queues the live event. Take
        // the snapshot in that order so the replay copy and the late-queue
        // suppression below describe the same summary.
        let manifest_guard = self.session_manifest.lock().ok();
        let manifest = manifest_guard
            .as_ref()
            .and_then(|stored| stored.as_ref().cloned());
        let replace_manifest = manifest.is_some();
        let Ok(mut stream) = self.lock_stream() else {
            return (0, manifest);
        };
        let current_seq = stream.next_seq.saturating_sub(1);
        // The stream backlog is shared; leave it intact so each observer can
        // apply its own replay boundary. Only this observer's queue is pruned
        // at the replay seam.
        let backlog = stream.agent_backlog.iter().cloned().collect::<Vec<_>>();
        if let Some(attachment) = stream.observers.get_mut(&key) {
            // The replay seam emits one stored summary below. A queued live
            // manifest is the same positionless state crossing that seam, so
            // discard it only when that stored replacement exists. Keep the
            // value so a publication already past the manifest lock but not
            // yet past the stream lock is recognized as the same summary.
            attachment.suppressed_manifest = manifest.clone();
            remove_replayed_agent_items(
                &mut attachment.pending,
                from_seq,
                replayed_seqs,
                replace_manifest,
            );
            for item in backlog {
                let eligible = backlog_item_eligible(
                    &item,
                    from_seq,
                    replayed_seqs,
                    reset_tail_cards,
                    attachment.typed_permissions,
                );
                let already_pending = matches!(
                    &item,
                    PendingItem::Agent {
                        event: SessionEvent::PermissionRequest { tool_call_id, .. },
                        ..
                    } if attachment.pending.iter().any(|pending| matches!(
                        pending,
                        PendingItem::Agent {
                            event: SessionEvent::PermissionRequest { tool_call_id: pending_id, .. },
                            ..
                        } if pending_id == tool_call_id
                    ))
                );
                if eligible && !already_pending {
                    attachment.pending.push_back(item);
                }
            }
            let (pending_bytes, pending_frames) = agent_queue_extent(&attachment.pending);
            attachment.pending_bytes = pending_bytes;
            attachment.pending_frames = pending_frames;
        }
        (current_seq, manifest)
    }

    pub(crate) fn replay_journal_agent_page(
        &self,
        generation: u64,
        from_generation: u64,
        from_seq: u64,
        through_seq: u64,
        limit: usize,
    ) -> Result<Option<crate::journal::AgentReplayPage>, crate::journal::JournalError> {
        let Some(journal) = &self.journal else {
            return Ok(None);
        };
        let page = journal.replay_agent_page(
            &self.session_id,
            generation,
            from_generation,
            from_seq,
            through_seq,
            limit,
        )?;
        self.journal_replays.fetch_add(1, Ordering::Relaxed);
        Ok(Some(page))
    }

    /// The retained sequence domain of one generation, for the attach resume
    /// decision. `None` when this runtime never promised durability, or when
    /// the read failed: the caller keeps today's behaviour and says so through
    /// the degraded signal.
    pub(crate) fn resume_domain(&self, generation: u64) -> Option<crate::journal::ResumeRange> {
        let journal = self.journal.as_ref()?;
        match journal.resume_range(&self.session_id, generation) {
            Ok(range) => Some(range),
            Err(error) => {
                self.mark_journal_degraded();
                eprintln!(
                    "resume range read failed for session {}: {error}",
                    self.session_id
                );
                None
            }
        }
    }

    /// The rows a reset tail may be built from: the newest of this generation
    /// up to the captured head, inside the scan budget.
    pub(crate) fn tail_candidate_rows(
        &self,
        generation: u64,
        head: u64,
    ) -> Option<crate::journal::ResetTailPage> {
        let journal = self.journal.as_ref()?;
        match journal.reset_tail_rows(
            &self.session_id,
            generation,
            head,
            crate::journal_resume::TAIL_SCAN_BYTES,
            crate::journal_resume::TAIL_SCAN_ROWS,
        ) {
            Ok(page) => Some(page),
            Err(error) => {
                self.mark_journal_degraded();
                eprintln!(
                    "reset tail read failed for session {}: {error}",
                    self.session_id
                );
                None
            }
        }
    }

    /// Fresh journal copies of a session's Output rows, as
    /// `(generation, seq, data)`. The read is unpositioned — from seq 0 —
    /// because the caller filters with the owed-row predicate: history rows
    /// are owed whatever their seq, and a cursor-positioned read would
    /// never even fetch them.
    pub(crate) fn replay_journal_outputs(&self, generation: u64) -> Vec<(u64, u64, String)> {
        let Some(journal) = &self.journal else {
            return Vec::new();
        };
        let replay = match journal.replay(&self.session_id) {
            Ok(replay) => replay,
            Err(error) => {
                self.mark_journal_degraded();
                eprintln!(
                    "journal replay failed for live session {}: {error}",
                    self.session_id
                );
                return Vec::new();
            }
        };
        if replay.generation != generation {
            self.mark_journal_degraded();
            eprintln!(
                "journal replay generation mismatch for live session {}: journal={} live={}",
                self.session_id, replay.generation, generation
            );
            return Vec::new();
        }
        self.journal_replays.fetch_add(1, Ordering::Relaxed);
        replay
            .events
            .into_iter()
            .zip(replay.event_seqs)
            .filter_map(|(event, (row_generation, seq))| match event {
                SessionEvent::Output { data, .. } => Some((row_generation, seq, data)),
                SessionEvent::Exit { .. }
                | SessionEvent::Recovered { .. }
                | SessionEvent::Silent { .. }
                | SessionEvent::JournalDegraded { .. }
                | SessionEvent::SessionsSnapshot { .. }
                // Snapshots are not output chunks and are not sourced from
                // the historical journal replay path.
                | SessionEvent::Snapshot { .. }
                | SessionEvent::QueueSnapshot { .. }
                | SessionEvent::TasksSnapshot { .. }
                | SessionEvent::AgentMessage { .. }
                | SessionEvent::AgentUserMessage { .. }
                | SessionEvent::Steered { .. }
                | SessionEvent::AgentThought { .. }
                | SessionEvent::AvailableCommands { .. }
                | SessionEvent::AgentToolCall { .. }
                | SessionEvent::AgentToolUpdate { .. }
                | SessionEvent::AgentFinished { .. }
                | SessionEvent::AgentTaskStarted { .. }
                | SessionEvent::AgentTaskNotification { .. }
                | SessionEvent::AgentBackgroundTasksChanged { .. }
                | SessionEvent::AgentTasks { .. }
                | SessionEvent::GoalChanged { .. }
                | SessionEvent::AgentError { .. }
                | SessionEvent::AgentStderr { .. }
                | SessionEvent::PermissionRequest { .. }
                | SessionEvent::PermissionResolved { .. }
                | SessionEvent::PermissionAnswered { .. }
                | SessionEvent::SessionNotice { .. }
                | SessionEvent::SessionManifest { .. }
                | SessionEvent::SessionFeatureState { .. }
                | SessionEvent::AgentCreated { .. }
                | SessionEvent::AgentResumed { .. }
                | SessionEvent::ChildFinished { .. }
                | SessionEvent::AgentReported { .. }
                | SessionEvent::ContextUsage { .. }
                | SessionEvent::PlanUsage { .. }
                | SessionEvent::Detached => None,
            })
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn journal_replay_count(&self) -> u64 {
        self.journal_replays.load(Ordering::Relaxed)
    }

    pub(crate) fn note_task_seed_failure(&self) {
        self.task_seed_failures.fetch_add(1, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(crate) fn task_seed_failure_count(&self) -> u64 {
        self.task_seed_failures.load(Ordering::Relaxed)
    }

    pub(crate) fn note_plan_mark_scan(&self) {
        self.plan_mark_scans.fetch_add(1, Ordering::Relaxed);
    }

    #[cfg(test)]
    pub(crate) fn plan_mark_scan_count(&self) -> u64 {
        self.plan_mark_scans.load(Ordering::Relaxed)
    }

    /// The one journal lookback read (see `journal_lookback`): the plan
    /// marks a Codex rebuild replays from, or the cost baseline a
    /// mid-generation Claude pull seeds its latch with. A failed read — the
    /// rpc deadline among them — is never an answer: it maps per lookback,
    /// to an empty set with the standing notice for the marks, and to
    /// [`CostBaseline::Unknown`] for the baseline, so a busy journal degrades
    /// to no figure rather than a wrong one.
    pub(crate) fn journal_lookback(
        &self,
        request: crate::journal_lookback::LookbackRequest,
    ) -> crate::journal_lookback::LookbackAnswer {
        use crate::claude_view::CostBaseline;
        use crate::journal_lookback::{LookbackAnswer, LookbackRequest};
        #[cfg(test)]
        if matches!(request, LookbackRequest::CodexPlanMarks) {
            if let Some(probe) = self
                .plan_mark_scan_probe
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_ref()
            {
                probe();
            }
        }
        let Some(journal) = &self.journal else {
            return match request {
                LookbackRequest::CodexPlanMarks => LookbackAnswer::PlanMarks(HashSet::new()),
                LookbackRequest::ClaudeCostBaseline { .. } => {
                    LookbackAnswer::CostBaseline(CostBaseline::Unknown)
                }
            };
        };
        match journal.lookback(&self.session_id, request) {
            Ok(answer) => {
                if matches!(request, LookbackRequest::CodexPlanMarks) {
                    self.note_plan_mark_scan();
                }
                answer
            }
            Err(error) => {
                eprintln!("lookback scan failed: {error}");
                match request {
                    LookbackRequest::CodexPlanMarks => {
                        self.note_plan_mark_scan();
                        let _ = self.publish_session_notice(
                            "The agent's plan approval history could not be read; plan checklists may reappear for approved plans."
                                .to_string(),
                            NoticeSeverity::Warning,
                        );
                        LookbackAnswer::PlanMarks(HashSet::new())
                    }
                    LookbackRequest::ClaudeCostBaseline { .. } => {
                        LookbackAnswer::CostBaseline(CostBaseline::Unknown)
                    }
                }
            }
        }
    }

    /// Attach through the same wire path used by the session registry. A
    /// headless live agent captures a journal replay watermark while holding
    /// the stream lock; terminals retain their snapshot-first contract.
    #[cfg(test)]
    pub(crate) fn try_attach_with_replay(
        &self,
        from_cursor: Option<Cursor>,
        conn: &ConnHandle,
        typed_permissions: bool,
    ) -> Result<AttachOutcome, WireError> {
        // The test helper keeps the old single-view setup; the wire path
        // receives a caller-owned token and only a claim grants resize rights.
        self.detach_subscription(conn.id, conn.id);
        conn.untrack_subscription(conn.id);
        let outcome =
            self.try_attach_with_subscription(conn.id, from_cursor, conn, typed_permissions)?;
        self.claim_resize(conn.id, conn.id)?;
        Ok(outcome)
    }

    pub(crate) fn try_attach_with_subscription(
        &self,
        subscription_id: u64,
        from_cursor: Option<Cursor>,
        conn: &ConnHandle,
        typed_permissions: bool,
    ) -> Result<AttachOutcome, WireError> {
        self.try_attach_inner(subscription_id, from_cursor, conn, typed_permissions)
    }

    fn try_attach_inner(
        &self,
        subscription_id: u64,
        from_cursor: Option<Cursor>,
        conn: &ConnHandle,
        typed_permissions: bool,
    ) -> Result<AttachOutcome, WireError> {
        if self.terminal_dead.load(Ordering::Acquire) {
            return Err(process_gone());
        }
        let Ok(mut stream) = self.lock_stream() else {
            return Err(internal("Session state is unavailable."));
        };
        if subscription_id == 0 {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "subscription id must be non-zero",
            ));
        }
        let key = AttachmentKey {
            conn_id: conn.id,
            subscription_id,
        };
        if stream.observers.contains_key(&key) {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "session subscription is already attached",
            ));
        }
        let live_agent = !stream.transcript && stream.screen.is_none();
        // The resume decision is taken here, under the stream lock, so the
        // floor, the head and the replay watermark below name one instant. The
        // journal writer thread never takes this lock, so the reads it makes
        // cannot invert the two.
        let resume = self.resolve_attach_resume(&stream, conn, live_agent, from_cursor);
        // Terminal attaches start at the current screen snapshot. Headless
        // live agents instead use the cursor as the start of a journal replay.
        // Both still validate the generation, except on the negotiated resume
        // road, where a cursor from a recreated process is an `epoch_changed`
        // reset rather than an error.
        if let Some(cursor) = from_cursor.filter(|_| resume.is_none()) {
            cursor_replay_ok(stream.generation, cursor)?;
        }
        // A reset hands the client its tail in the reply, so the replay after
        // it starts where the tail stopped and the replay-to-live seam prunes
        // what the tail already carried. Without an outcome this is exactly
        // the start a bare cursor has always meant.
        let (from_generation, from_seq) = match (&resume, from_cursor) {
            (Some(decision), _) => (decision.from_generation, decision.resume_from),
            (None, Some(cursor)) if cursor.seq > 0 => (cursor.generation, cursor.seq),
            _ => (0, from_cursor.map(|cursor| cursor.seq).unwrap_or(0)),
        };
        // A runtime without a journal has made no durability promise. Keep
        // its ordinary live queue contract; a configured journal gets the
        // lazy history replay and stored-manifest seam below.
        let live_agent_replay = if live_agent && self.journal.is_some() {
            Some(LiveAgentReplay {
                from_generation,
                from_seq,
                watermark: stream.next_seq.saturating_sub(1),
                reset_tail_cards: resume
                    .as_ref()
                    .and_then(|decision| decision.reset_tail_cards.clone()),
            })
        } else {
            None
        };
        let resume = resume.map(|decision| decision.info);
        let as_of_seq = stream.last_applied_seq;
        let screen = stream.screen.as_ref().map(Screen::snapshot);
        let mut attachment = Attachment {
            outbound: Arc::clone(&conn.outbound),
            typed_permissions,
            session_queue: conn.session_queue_negotiated(),
            session_tasks: conn.session_tasks_negotiated(),
            plan_usage_live: conn.plan_usage_live_negotiated(),
            suppressed_manifest: None,
            pending: VecDeque::new(),
            pending_bytes: 0,
            pending_frames: 0,
            pending_silences: VecDeque::new(),
        };
        if let Some(screen) = screen {
            attachment
                .pending
                .push_back(PendingItem::Snapshot { as_of_seq, screen });
        } else if !stream.transcript && live_agent_replay.is_none() {
            let agent_backlog = std::mem::take(&mut stream.agent_backlog);
            stream.agent_backlog_bytes = 0;
            stream.agent_backlog_frames = 0;
            let mut deferred = VecDeque::new();
            for item in agent_backlog {
                let is_permission = matches!(
                    &item,
                    PendingItem::Agent {
                        event: SessionEvent::PermissionRequest { .. },
                        ..
                    }
                );
                if is_permission {
                    if typed_permissions {
                        attachment.pending.push_back(item.clone());
                    }
                    deferred.push_back(item);
                } else {
                    attachment.pending.push_back(item);
                }
            }
            attachment.pending_bytes = attachment
                .pending
                .iter()
                .filter_map(|item| match item {
                    PendingItem::Agent { bytes, .. } => Some(*bytes),
                    PendingItem::Output { data, .. } => Some(data.len()),
                    PendingItem::Snapshot { .. } => None,
                })
                .sum();
            attachment.pending_frames = attachment
                .pending
                .iter()
                .filter(|item| !matches!(item, PendingItem::Snapshot { .. }))
                .count() as u64;
            stream.agent_backlog = deferred;
            let (backlog_bytes, backlog_frames) = agent_queue_extent(&stream.agent_backlog);
            stream.agent_backlog_bytes = backlog_bytes;
            stream.agent_backlog_frames = backlog_frames;
        }
        let has_manifest = attachment.pending.iter().any(|item| {
            matches!(
                item,
                PendingItem::Agent {
                    event: SessionEvent::SessionManifest { .. },
                    ..
                }
            )
        });
        stream.observers.insert(key, attachment);
        self.set_attachment_notify(key, Some(Arc::clone(&conn.outbound)));
        if live_agent_replay.is_none()
            && !stream.transcript
            && !has_manifest
            && !stream.observers.is_empty()
        {
            if let Some(event) = self
                .session_manifest
                .lock()
                .ok()
                .and_then(|guard| guard.clone())
            {
                if let Some(attachment) = stream.observers.get_mut(&key) {
                    enqueue_agent_for_attachment(attachment, event, None);
                }
            }
        }
        Ok(AttachOutcome {
            generation: stream.generation,
            live_agent_replay,
            resume,
        })
    }

    /// The attach reply's resume fields for this connection, or `None` for every
    /// case where today's behaviour stands: no cursor sent, no negotiated
    /// capability, not a live structured agent, or a journal this runtime does
    /// not have.
    fn resolve_attach_resume(
        &self,
        stream: &StreamState,
        conn: &ConnHandle,
        live_agent: bool,
        from_cursor: Option<Cursor>,
    ) -> Option<crate::journal_resume::ResumeDecision> {
        let cursor = from_cursor?;
        // A terminal has a screen to redraw and a transcript replays its whole
        // stored history: neither has a cursor to reinterpret.
        if !conn.resume_outcomes_negotiated() || !live_agent || !self.has_journal() {
            return None;
        }
        let generation = stream.generation;
        // The head this attach captures is the same watermark the replay pages
        // to, so the tail and the events that follow it agree on one instant
        // even when the journal writer is behind.
        let head = stream.next_seq.saturating_sub(1);
        crate::journal_resume::resolve(self, generation, head, cursor)
    }

    pub(crate) fn claim_resize(&self, conn_id: u64, subscription_id: u64) -> Result<(), WireError> {
        let mut stream = self
            .stream
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        let key = AttachmentKey {
            conn_id,
            subscription_id,
        };
        if !stream.observers.contains_key(&key) {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "Session is not attached to this subscription.",
            ));
        }
        stream.resize_owner = Some(key);
        Ok(())
    }

    pub(crate) fn detach_subscription(&self, conn_id: u64, subscription_id: u64) {
        let Ok(mut stream) = self.lock_stream() else {
            return;
        };
        let key = AttachmentKey {
            conn_id,
            subscription_id,
        };
        if let Some(attachment) = stream.observers.remove(&key) {
            if stream.resize_owner == Some(key) {
                stream.resize_owner = None;
            }
            self.set_attachment_notify(key, None);
            if !stream.transcript && stream.screen.is_none() {
                move_agent_pending_to_backlog(&mut stream, attachment);
            }
        }
    }

    pub(crate) fn detach_if_conn(&self, conn_id: u64) {
        let Ok(mut stream) = self.lock_stream() else {
            return;
        };
        let keys = stream
            .observers
            .keys()
            .copied()
            .filter(|key| key.conn_id == conn_id)
            .collect::<Vec<_>>();
        for key in keys {
            if let Some(attachment) = stream.observers.remove(&key) {
                if stream.resize_owner == Some(key) {
                    stream.resize_owner = None;
                }
                self.set_attachment_notify(key, None);
                if !stream.transcript && stream.screen.is_none() {
                    move_agent_pending_to_backlog(&mut stream, attachment);
                }
            }
        }
    }

    pub(crate) fn notify_generation_replaced(&self, conn_id: u64) {
        let Ok(mut stream) = self.lock_stream() else {
            return;
        };
        let generation = stream.generation;
        for (key, attachment) in &mut stream.observers {
            if key.conn_id != conn_id {
                // The old pull may already have consumed Exit; direct queueing
                // keeps this replacement signal deliverable through that seam.
                // The event names the case structurally: this observer's view
                // was replaced and the session lives on. Never AgentError —
                // the event a single malformed line rides on — which the app
                // could only tell apart by comparing English.
                attachment.outbound.enqueue_reply(
                    devboule_protocol::DaemonMessage::SubscriptionEvent {
                        subscription_id: key.subscription_id,
                        envelope: SessionEventEnvelope {
                            session_id: self.session_id.clone(),
                            generation,
                            transcript_seq: None,
                            event: SessionEvent::Detached,
                        },
                    },
                );
            }
        }
    }

    pub(crate) fn is_observer(&self, conn_id: u64, subscription_id: u64) -> Result<(), WireError> {
        let stream = self
            .stream
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        let key = AttachmentKey {
            conn_id,
            subscription_id,
        };
        if stream.observers.contains_key(&key) {
            Ok(())
        } else {
            Err(WireError::new(
                ErrorCode::InvalidRequest,
                "Session is not attached to this subscription.",
            ))
        }
    }

    pub(crate) fn is_resize_owner(
        &self,
        conn_id: u64,
        subscription_id: u64,
    ) -> Result<(), WireError> {
        let stream = self
            .stream
            .lock()
            .map_err(|_| internal("Session state is unavailable."))?;
        let key = AttachmentKey {
            conn_id,
            subscription_id,
        };
        if !stream.observers.contains_key(&key) {
            Err(WireError::new(
                ErrorCode::InvalidRequest,
                "Session is not attached to this subscription.",
            ))
        } else if stream.resize_owner == Some(key) {
            Ok(())
        } else {
            Err(WireError::new(
                ErrorCode::InvalidRequest,
                "Session observer does not own resize control; another subscription owns it.",
            ))
        }
    }

    pub(crate) fn mark_exited(&self, code: Option<u32>) {
        let Ok(mut stream) = self.lock_stream() else {
            return;
        };
        if stream.process_exited {
            return;
        }
        stream.process_exited = true;
        stream.exit_code = code;
        stream.exit_at = Some(Instant::now());
        stream.disposition = Disposition::Exited {
            integrity: self.terminated_integrity(),
        };
        for attachment in stream.observers.values_mut() {
            attachment.pending_silences.clear();
        }
        notify_observers(&stream);
        drop(stream);
        // The death wall time and the cleared launch set belong to the exit:
        // a background command the session outlives is over, and its row
        // ends here rather than on a result that will never arrive.
        if let Ok(mut ended) = self.tasks_ended_wall_ms.lock() {
            if ended.is_none() {
                *ended = Some(crate::agent_activity::wall_now_ms());
            }
        }
        if let Ok(mut armed) = self.background_tool_calls.lock() {
            armed.clear();
        }
        self.notify_tasks_ended();
        self.fail_mcp_if_pending("The agent process exited before the MCP broker was ready.");
        // Child::wait returns before ConPTY EOFs. Record
        // that the process was observed, but do not freeze last_seq: drain
        // frames still need seqs. Ended (exit row) is written at EOF.
        // Fire-and-forget: a blocking journal RPC here would stall
        // sessions_watch past the 5s OS-liveness bound.
        if let Some(journal) = &self.journal {
            journal.try_mark_reaped(&self.session_id, code);
        }
        self.refresh_exit_integrity();
        self.fire_os_death();
    }

    pub(crate) fn terminated_integrity(&self) -> TranscriptIntegrity {
        if self.journal_degraded() {
            TranscriptIntegrity::Truncated {
                dropped_frames: self.journal_dropped_frames.load(Ordering::Acquire),
                dropped_bytes: self.journal_dropped_bytes.load(Ordering::Acquire),
                trimmed_bytes: 0,
            }
        } else {
            TranscriptIntegrity::Complete
        }
    }

    pub(crate) fn refresh_exit_integrity(&self) {
        // A poisoned stream is already handled by the caller's terminal-dead
        // path; do not re-enter that path while refreshing the disposition.
        let Ok(mut stream) = self.stream.lock() else {
            return;
        };
        if let Disposition::Exited { integrity } = &mut stream.disposition {
            *integrity = self.terminated_integrity();
        }
    }

    pub(crate) fn close_output(&self) {
        let Ok(mut stream) = self.lock_stream() else {
            return;
        };
        stream.output_closed = true;
        notify_observers(&stream);
        drop(stream);
    }

    pub(crate) fn finish(&self, code: Option<u32>) {
        self.mark_exited(code);
        self.close_output();
    }

    pub(crate) fn ready_for_exit(stream: &StreamState) -> bool {
        if stream.output_closed {
            return true;
        }
        if !stream.process_exited {
            return false;
        }
        let origin = stream.last_publish.or(stream.exit_at);
        origin.is_none_or(|instant| instant.elapsed() >= EXIT_DRAIN)
    }

    /// Test door: publish an agent event as a journalled `AgentReport` row. It
    /// is the only publish that both spends a stream sequence and writes a
    /// durable row, so it is the one a journal-reading test needs.
    #[cfg(test)]
    pub(crate) fn test_publish_journaled(&self, event: SessionEvent) -> bool {
        self.publish_journaled_agent_event(|_, _, _| event)
            .is_some()
    }

    /// Test door: make this runtime a live structured agent whose stream sits at
    /// `generation` with `next_seq` as its first unspent sequence, the shape the
    /// replay tests stage by hand.
    #[cfg(test)]
    pub(crate) fn test_live_agent_at(&self, generation: u64, next_seq: u64) {
        let Ok(mut stream) = self.lock_stream() else {
            return;
        };
        stream.screen = None;
        stream.transcript = false;
        stream.generation = generation;
        stream.next_seq = next_seq;
        drop(stream);
        self.generation.store(generation, Ordering::Release);
    }

    #[cfg(test)]
    pub(crate) fn bump_generation(&self) -> u64 {
        let Ok(mut stream) = self.lock_stream() else {
            return 1;
        };
        stream.generation = stream.generation.saturating_add(1);
        self.generation.store(stream.generation, Ordering::Release);
        stream.next_seq = 1;
        stream.last_applied_seq = 0;
        // A new generation is a new process: a fresh emulator, not the old
        // grid carrying over.
        stream.screen = Some(Screen::new(INITIAL_COLS, INITIAL_ROWS));
        stream.agent_backlog.clear();
        stream.agent_backlog_bytes = 0;
        stream.agent_backlog_frames = 0;
        for attachment in stream.observers.values_mut() {
            attachment.pending.clear();
            attachment.pending_bytes = 0;
            attachment.pending_frames = 0;
            attachment.pending_silences.clear();
        }
        stream.output_closed = false;
        stream.process_exited = false;
        stream.exit_code = None;
        stream.last_publish = None;
        stream.exit_at = None;
        stream.disposition = Disposition::Running;
        stream.generation
    }

    #[cfg(test)]
    pub(crate) fn transcript_chunks(&self) -> Vec<(u64, String)> {
        let stream = self.stream.lock().unwrap();
        stream
            .scrollback
            .chunks
            .iter()
            .map(|chunk| (chunk.seq, String::from_utf8_lossy(&chunk.data).into_owned()))
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn last_applied_seq(&self) -> u64 {
        self.stream.lock().unwrap().last_applied_seq
    }

    #[cfg(test)]
    pub(crate) fn resize_owner_conn_id(&self) -> Option<u64> {
        self.stream
            .lock()
            .unwrap()
            .resize_owner
            .map(|owner| owner.conn_id)
    }
}

fn notify_observers(stream: &StreamState) {
    for attachment in stream.observers.values() {
        attachment.outbound.notify();
    }
}

/// Enqueue one applied chunk for every observer and enforce the
/// slow-viewer budget. Called with the state lock held, after the emulator
/// boundary advanced.
///
/// When the unsent Output extent exceeds the budget, the WHOLE unsent queue
/// is discarded and replaced by a fresh snapshot at the current boundary.
/// Pipe order still delivers the newer snapshot after anything older that
/// already reached the wire, and the snapshot subsumes everything before it.
fn enqueue_output(stream: &mut StreamState, seq: u64, data: &str) -> (u64, u64) {
    let as_of_seq = stream.last_applied_seq;
    let screen = stream.screen.as_ref().map(Screen::snapshot);
    let mut discarded_bytes: u64 = 0;
    let mut discarded_frames: u64 = 0;
    for attachment in stream.observers.values_mut() {
        attachment.pending.push_back(PendingItem::Output {
            seq,
            data: data.to_owned(),
        });
        attachment.pending_bytes += data.len();
        attachment.pending_frames += 1;
        if attachment.pending_bytes <= PENDING_OUTPUT_BUDGET_BYTES
            && attachment.pending_frames <= PENDING_OUTPUT_BUDGET_FRAMES
        {
            continue;
        }
        discarded_bytes = discarded_bytes.saturating_add(attachment.pending_bytes as u64);
        discarded_frames = discarded_frames.saturating_add(attachment.pending_frames);
        attachment.pending.clear();
        attachment.pending_bytes = 0;
        attachment.pending_frames = 0;
        if let Some(screen) = screen.clone() {
            attachment
                .pending
                .push_back(PendingItem::Snapshot { as_of_seq, screen });
        }
    }
    (discarded_bytes, discarded_frames)
}

/// Whether the replay this attach just ran already carried the position one
/// shared-backlog item holds. `None` is a position nothing has claimed: a
/// daemon-local event, which no replay can deliver.
fn replay_covered(seq: Option<u64>, from_seq: u64, replayed_seqs: &HashSet<u64>) -> bool {
    seq.is_some_and(|seq| seq <= from_seq || replayed_seqs.contains(&seq))
}

/// Whether one shared-backlog item belongs on the observer whose replay just
/// finished.
///
/// A still-pending permission card is the exception, and only for the observer
/// that is resetting: its timeline was replaced by the tail in the attach
/// reply, so a card at or below `from_seq` is no longer on its screen and the
/// agent is waiting on a card nobody can answer. It crosses unless this attach
/// already delivered it — `reset_tail_cards` names the cards the reply's own
/// tail carried, and the caller drops a card the observer's queue already holds.
/// Every other item, and every card on an attach that keeps the client's own
/// timeline, follows the replay boundary as before: that client's cursor says
/// what it has already seen.
fn backlog_item_eligible(
    item: &PendingItem,
    from_seq: u64,
    replayed_seqs: &HashSet<u64>,
    reset_tail_cards: Option<&HashSet<String>>,
    typed_permissions: bool,
) -> bool {
    let PendingItem::Agent { seq, event, .. } = item else {
        return false;
    };
    match event {
        SessionEvent::PermissionRequest { tool_call_id, .. } => {
            typed_permissions
                && (!replay_covered(*seq, from_seq, replayed_seqs)
                    || reset_tail_cards.is_some_and(|cards| !cards.contains(tool_call_id)))
        }
        // The seam emits the stored summary in its place.
        SessionEvent::SessionManifest { .. } => false,
        _ => !replay_covered(*seq, from_seq, replayed_seqs),
    }
}

fn enqueue_agent(stream: &mut StreamState, event: SessionEvent, seq: Option<u64>) {
    let permission = matches!(&event, SessionEvent::PermissionRequest { .. });
    let mut delivered = false;
    for attachment in stream.observers.values_mut() {
        if permission && !attachment.typed_permissions {
            continue;
        }
        if attachment.suppressed_manifest.as_ref() == Some(&event) {
            delivered = true;
            continue;
        }
        if matches!(&event, SessionEvent::SessionManifest { .. }) {
            attachment.suppressed_manifest = None;
        }
        enqueue_agent_for_attachment(attachment, event.clone(), seq);
        delivered = true;
    }
    if permission || !delivered {
        push_bounded_agent(
            &mut stream.agent_backlog,
            &mut stream.agent_backlog_bytes,
            &mut stream.agent_backlog_frames,
            event,
            seq,
        );
    }
}

fn enqueue_agent_for_attachment(
    attachment: &mut Attachment,
    event: SessionEvent,
    seq: Option<u64>,
) {
    push_bounded_agent(
        &mut attachment.pending,
        &mut attachment.pending_bytes,
        &mut attachment.pending_frames,
        event,
        seq,
    );
}

fn remove_permission_from_queue(
    queue: &mut VecDeque<PendingItem>,
    bytes_total: &mut usize,
    frames_total: &mut u64,
    tool_call_id: &str,
) {
    let mut retained = VecDeque::with_capacity(queue.len());
    while let Some(item) = queue.pop_front() {
        let remove = matches!(
            &item,
            PendingItem::Agent {
                event: SessionEvent::PermissionRequest { tool_call_id: current, .. },
                ..
            } if current == tool_call_id
        );
        if remove {
            if let PendingItem::Agent { bytes, .. } = &item {
                *bytes_total = bytes_total.saturating_sub(*bytes);
                *frames_total = frames_total.saturating_sub(1);
            }
        } else {
            retained.push_back(item);
        }
    }
    *queue = retained;
}

fn push_bounded_agent(
    queue: &mut VecDeque<PendingItem>,
    bytes_total: &mut usize,
    frames_total: &mut u64,
    event: SessionEvent,
    seq: Option<u64>,
) {
    if let SessionEvent::PermissionRequest { tool_call_id, .. } = &event {
        let already_queued = queue.iter().any(|item| {
            matches!(
                item,
                PendingItem::Agent {
                    event: SessionEvent::PermissionRequest {
                        tool_call_id: queued_id,
                        ..
                    },
                    ..
                } if queued_id == tool_call_id
            )
        });
        if already_queued {
            return;
        }
    }
    let bytes = serde_json::to_vec(&event)
        .map(|value| value.len())
        .unwrap_or(0);
    queue.push_back(PendingItem::Agent { seq, event, bytes });
    *bytes_total = bytes_total.saturating_add(bytes);
    *frames_total = frames_total.saturating_add(1);
    if *bytes_total <= PENDING_OUTPUT_BUDGET_BYTES && *frames_total <= PENDING_OUTPUT_BUDGET_FRAMES
    {
        return;
    }
    // A pending card is the agent's wait, and the permission broker already
    // caps how many a session holds: the budget measures what is left once
    // they are set aside, and eviction never takes one.
    let (card_bytes, card_frames) = pending_card_extent(queue);
    if bytes_total.saturating_sub(card_bytes) <= PENDING_OUTPUT_BUDGET_BYTES
        && frames_total.saturating_sub(card_frames) <= PENDING_OUTPUT_BUDGET_FRAMES
    {
        return;
    }
    // ACP has no terminal screen to use as a replacement snapshot. Keep the
    // newest structured event and bound both the attached queue and the
    // detached backlog with the same limits.
    let newest = queue.len().saturating_sub(1);
    let mut index = 0;
    queue.retain(|item| {
        let keep = index == newest || is_pending_card(item);
        index += 1;
        keep
    });
    (*bytes_total, *frames_total) = agent_queue_extent(queue);
}

fn is_pending_card(item: &PendingItem) -> bool {
    matches!(
        item,
        PendingItem::Agent {
            event: SessionEvent::PermissionRequest { .. },
            ..
        }
    )
}

fn pending_card_extent(queue: &VecDeque<PendingItem>) -> (usize, u64) {
    queue
        .iter()
        .filter(|item| is_pending_card(item))
        .fold((0, 0), |(bytes, frames), item| match item {
            PendingItem::Agent { bytes: size, .. } => (bytes + size, frames + 1),
            _ => (bytes, frames),
        })
}

fn move_agent_pending_to_backlog(stream: &mut StreamState, mut attachment: Attachment) {
    while let Some(item) = attachment.pending.pop_front() {
        if let PendingItem::Agent { event, seq, .. } = item {
            push_bounded_agent(
                &mut stream.agent_backlog,
                &mut stream.agent_backlog_bytes,
                &mut stream.agent_backlog_frames,
                event,
                seq,
            );
        }
    }
}

#[cfg(test)]
#[path = "session_runtime_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "session_runtime_marker_pair_tests.rs"]
mod marker_pair_tests;
