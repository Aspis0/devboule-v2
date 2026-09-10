//! Session stream runtime: emulator, attach, journal, permission delivery.

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use devboule_protocol::{
    cursor_replay_ok, Attention, AttentionReason, Cursor, ErrorCode, SessionEvent,
    SessionEventEnvelope, SessionKind, SessionModel, TranscriptIntegrity, WireError,
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
use crate::outbound::ConnOut;
use crate::process_tree::ProcessHandle;
use crate::screen::Screen;

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
) {
    queue.retain(|item| match item {
        PendingItem::Agent { seq, event, .. } => {
            let replayed = seq.is_some_and(|seq| seq <= from_seq || replayed_seqs.contains(&seq));
            let drop_manifest = matches!(event, SessionEvent::SessionManifest { .. }) && replayed;
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
    pub(crate) reader_finished: AtomicBool,
    pub(crate) child_reaped: AtomicBool,
    /// Transition notifications are suppressed until spawn has inserted all
    /// runtime state, so an extremely short-lived child cannot publish an
    /// exit before the corresponding create snapshot.
    pub(crate) transition_ready: AtomicBool,
    /// The wait thread and the post-create race check can observe the same
    /// exit. Only one of them may publish the exit transition.
    pub(crate) exit_transition_sent: AtomicBool,
    pub(crate) published_frames: AtomicU64,
    pub(crate) published_bytes: AtomicUsize,
    pub(crate) session_manifest: Mutex<Option<SessionEvent>>,
    mcp_bearer: Mutex<Option<String>>,
    mcp_url: Mutex<Option<String>>,
    mcp_readiness: Mutex<McpReadiness>,
    mcp_ready_cvar: Condvar,
    pub(crate) agent_kind: Mutex<Option<SessionKind>>,
    claude_catalog_state: Mutex<crate::claude_catalog::ClaudeCatalogState>,
    /// Attention is deliberately runtime-only. It is a user's current view
    /// state, not transcript history, so it is not journaled and does not
    /// survive a daemon restart.
    pub(crate) attention: Mutex<Option<Attention>>,
    attention_hooks: Mutex<Option<AttentionHooks>>,
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
    /// Provider-side session id (ACP `sessionId`, Claude `system/init`
    /// `session_id`). Stored for resume; not the Devboule session id.
    pub(crate) peer_session_id: Mutex<Option<String>>,
}

struct McpReadiness {
    required: bool,
    ready: bool,
    failure: Option<String>,
}

struct AttentionHooks {
    suppressed: Arc<dyn Fn() -> bool + Send + Sync>,
    notify: Arc<dyn Fn() + Send + Sync>,
}

pub(crate) struct LiveAgentReplay {
    pub(crate) from_seq: u64,
    pub(crate) watermark: u64,
}

pub(crate) struct AttachOutcome {
    pub(crate) generation: u64,
    pub(crate) live_agent_replay: Option<LiveAgentReplay>,
}

fn merge_claude_manifest(previous: &SessionEvent, incoming: SessionEvent) -> SessionEvent {
    let SessionEvent::SessionManifest {
        provider_id: previous_provider,
        current_model_id: previous_current,
        models: previous_models,
        modes: previous_modes,
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
    }
}

impl SessionRuntime {
    #[cfg(test)]
    pub(crate) fn new() -> Self {
        Self::with_journal(String::new(), None)
    }

    pub(crate) fn with_journal(session_id: String, journal: Option<Arc<Journal>>) -> Self {
        Self {
            session_id,
            journal,
            permission_broker: None,
            stream: Mutex::new(StreamState {
                next_seq: 1,
                last_applied_seq: 0,
                generation: 1,
                screen: Some(Screen::new(INITIAL_COLS, INITIAL_ROWS)),
                transcript: false,
                resize_owner: None,
                observers: HashMap::new(),
                agent_backlog: VecDeque::new(),
                agent_backlog_bytes: 0,
                agent_backlog_frames: 0,
                scrollback: Scrollback::default(),
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
            reader_finished: AtomicBool::new(false),
            child_reaped: AtomicBool::new(false),
            transition_ready: AtomicBool::new(false),
            exit_transition_sent: AtomicBool::new(false),
            published_frames: AtomicU64::new(0),
            published_bytes: AtomicUsize::new(0),
            session_manifest: Mutex::new(None),
            mcp_bearer: Mutex::new(None),
            mcp_url: Mutex::new(None),
            mcp_readiness: Mutex::new(McpReadiness {
                required: false,
                ready: false,
                failure: None,
            }),
            mcp_ready_cvar: Condvar::new(),
            agent_kind: Mutex::new(None),
            claude_catalog_state: Mutex::new(
                crate::claude_catalog::ClaudeCatalogState::Provisional,
            ),
            attention: Mutex::new(None),
            attention_hooks: Mutex::new(None),
            os_handle: Mutex::new(None),
            on_os_death: Mutex::new(None),
            os_death_started: AtomicBool::new(false),
            roster_notify: Mutex::new(None),
            peer_session_id: Mutex::new(None),
        }
    }

    pub(crate) fn require_mcp(&self) {
        if let Ok(mut readiness) = self.mcp_readiness.lock() {
            readiness.required = true;
            self.mcp_ready_cvar.notify_all();
        }
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
                    stream.scrollback.push(seq, data.as_bytes());
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
                // Snapshots are screen state, never journal records; a
                // recovered session replays transcript events only.
                SessionEvent::Snapshot { .. } => {}
                SessionEvent::AgentReported { seq, .. } => {
                    stream.transcript_agent_reports.insert(seq, event);
                }
                SessionEvent::AgentMessage { .. }
                | SessionEvent::AgentUserMessage { .. }
                | SessionEvent::AgentThought { .. }
                | SessionEvent::AvailableCommands { .. }
                | SessionEvent::AgentToolCall { .. }
                | SessionEvent::AgentToolUpdate { .. }
                | SessionEvent::AgentFinished { .. }
                | SessionEvent::AgentTaskStarted { .. }
                | SessionEvent::AgentTaskNotification { .. }
                | SessionEvent::AgentBackgroundTasksChanged { .. }
                | SessionEvent::AgentError { .. }
                | SessionEvent::AgentStderr { .. }
                | SessionEvent::PermissionRequest { .. }
                | SessionEvent::PermissionResolved { .. }
                | SessionEvent::SessionManifest { .. } => {
                    let Some(seq) = journal_seq else {
                        continue;
                    };
                    stream.transcript_agent_reports.insert(seq, event);
                }
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

    pub(crate) fn store_session_manifest(&self, event: SessionEvent) -> SessionEvent {
        let event = if matches!(
            &event,
            SessionEvent::SessionManifest {
                provider_id: Some(provider_id),
                ..
            } if provider_id == "claude"
        ) {
            let previous = self
                .session_manifest
                .lock()
                .ok()
                .and_then(|stored| stored.clone());
            previous
                .as_ref()
                .map(|previous| merge_claude_manifest(previous, event.clone()))
                .unwrap_or(event)
        } else {
            event
        };
        if let Ok(mut stored) = self.session_manifest.lock() {
            *stored = Some(event.clone());
        }
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
    pub(crate) fn publish_agent_user_message(&self, text: String) -> bool {
        self.publish_journaled_agent_event(|generation, seq| SessionEvent::AgentUserMessage {
            message_id: Some(format!("devboule-user-{generation}-{seq}")),
            text,
        })
    }

    pub(crate) fn publish_agent_error(&self, message: String) -> bool {
        self.publish_journaled_agent_event(|_, _| SessionEvent::AgentError { message })
    }

    fn publish_journaled_agent_event<F>(&self, build: F) -> bool
    where
        F: FnOnce(u64, u64) -> SessionEvent,
    {
        let (event, generation, seq, was_silent) = {
            let Ok(mut stream) = self.lock_stream() else {
                // A false result means the stream cannot accept the event.
                return false;
            };
            if stream.output_closed {
                return false;
            }
            let was_silent = matches!(stream.disposition, Disposition::Silent);
            if !stream.process_exited {
                stream.disposition = Disposition::Running;
            }
            let generation = stream.generation;
            let seq = stream.next_seq;
            stream.next_seq = stream.next_seq.saturating_add(1);
            let event = build(generation, seq);
            stream.last_publish = Some(Instant::now());
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
        if was_silent {
            self.notify_roster();
        }
        self.raise_attention_for_event(&event);
        true
    }

    pub(crate) fn publish_agent_event_with_seq(
        &self,
        event: SessionEvent,
        journal_text: Option<&str>,
        event_seq: Option<u64>,
    ) -> bool {
        let was_silent;
        let journal_output;
        {
            let Ok(mut stream) = self.lock_stream() else {
                return false;
            };
            if stream.output_closed {
                eprintln!(
                    "session {} dropped ACP event after EOF: {:?}",
                    self.session_id, event
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
        self.raise_attention_for_event(&event);
        was_silent
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

    pub(crate) fn can_publish_agent_user_message(&self) -> bool {
        self.lock_stream()
            .map(|stream| !stream.output_closed)
            .unwrap_or(false)
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

    pub(crate) fn set_attention_hooks(
        &self,
        suppressed: Arc<dyn Fn() -> bool + Send + Sync>,
        notify: Arc<dyn Fn() + Send + Sync>,
    ) {
        if let Ok(mut hooks) = self.attention_hooks.lock() {
            *hooks = Some(AttentionHooks { suppressed, notify });
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

    fn raise_attention_for_event(&self, event: &SessionEvent) {
        let reason = match event {
            SessionEvent::AgentFinished { .. } => AttentionReason::Finished,
            SessionEvent::AgentError { .. } => AttentionReason::Error,
            SessionEvent::PermissionRequest { .. } => AttentionReason::Permission,
            _ => return,
        };
        self.raise_attention(reason);
    }

    fn raise_attention(&self, reason: AttentionReason) {
        let hooks = self.attention_hooks.lock().ok().and_then(|hooks| {
            hooks
                .as_ref()
                .map(|hooks| (Arc::clone(&hooks.suppressed), Arc::clone(&hooks.notify)))
        });
        // Attention is the transaction guard: hold it while consulting
        // presence so the suppression decision and the write cannot be
        // separated by a focus update. The global order is attention ->
        // presence; set_presence releases its presence guard before it calls
        // clear_attention, so it never holds these locks in reverse.
        let Ok(mut attention) = self.attention.lock() else {
            return;
        };
        if hooks.as_ref().is_some_and(|(suppressed, _)| suppressed()) {
            return;
        }
        if attention
            .as_ref()
            .is_some_and(|current| current.reason.priority() >= reason.priority())
        {
            return;
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
        if let Some((_, notify)) = hooks {
            notify();
        }
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

    pub(crate) fn set_agent_kind(&self, kind: SessionKind) {
        if let Ok(mut stored) = self.agent_kind.lock() {
            *stored = Some(kind);
        }
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
    ) -> u64 {
        let Ok(mut stream) = self.lock_stream() else {
            return 0;
        };
        let current_seq = stream.next_seq.saturating_sub(1);
        // The stream backlog is shared; leave it intact so each observer can
        // apply its own replay boundary. Only this observer's queue is pruned
        // at the replay seam.
        let backlog = stream.agent_backlog.iter().cloned().collect::<Vec<_>>();
        if let Some(attachment) = stream.observers.get_mut(&key) {
            remove_replayed_agent_items(&mut attachment.pending, from_seq, replayed_seqs);
            for item in backlog {
                let eligible = match &item {
                    PendingItem::Agent { seq, event, .. } => {
                        !seq.is_some_and(|seq| seq <= from_seq || replayed_seqs.contains(&seq))
                            && !matches!(event, SessionEvent::SessionManifest { .. })
                            && (!matches!(event, SessionEvent::PermissionRequest { .. })
                                || attachment.typed_permissions)
                    }
                    PendingItem::Output { .. } | PendingItem::Snapshot { .. } => false,
                };
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
        current_seq
    }

    pub(crate) fn replay_journal_agent_page(
        &self,
        generation: u64,
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
            from_seq,
            through_seq,
            limit,
        )?;
        self.journal_replays.fetch_add(1, Ordering::Relaxed);
        Ok(Some(page))
    }

    pub(crate) fn replay_journal_outputs(
        &self,
        from_seq: u64,
        generation: u64,
    ) -> Vec<(u64, String)> {
        let Some(journal) = &self.journal else {
            return Vec::new();
        };
        let replay = match journal.replay(&self.session_id, from_seq) {
            Ok(replay) => replay,
            Err(error) => {
                self.mark_journal_degraded();
                eprintln!(
                    "journal replay failed for live session {} from seq {}: {error}",
                    self.session_id, from_seq
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
            .filter_map(|event| match event {
                SessionEvent::Output { seq, data } => Some((seq, data)),
                SessionEvent::Exit { .. }
                | SessionEvent::Recovered { .. }
                | SessionEvent::Silent { .. }
                | SessionEvent::JournalDegraded { .. }
                | SessionEvent::SessionsSnapshot { .. }
                // Snapshots are not output chunks and are not sourced from
                // the historical journal replay path.
                | SessionEvent::Snapshot { .. }
                | SessionEvent::AgentMessage { .. }
                | SessionEvent::AgentUserMessage { .. }
                | SessionEvent::AgentThought { .. }
                | SessionEvent::AvailableCommands { .. }
                | SessionEvent::AgentToolCall { .. }
                | SessionEvent::AgentToolUpdate { .. }
                | SessionEvent::AgentFinished { .. }
                | SessionEvent::AgentTaskStarted { .. }
                | SessionEvent::AgentTaskNotification { .. }
                | SessionEvent::AgentBackgroundTasksChanged { .. }
                | SessionEvent::AgentError { .. }
                | SessionEvent::AgentStderr { .. }
                | SessionEvent::PermissionRequest { .. }
                | SessionEvent::PermissionResolved { .. }
                | SessionEvent::SessionManifest { .. }
                | SessionEvent::AgentReported { .. } => None,
            })
            .collect()
    }

    #[cfg(test)]
    pub(crate) fn journal_replay_count(&self) -> u64 {
        self.journal_replays.load(Ordering::Relaxed)
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
        // Terminal attaches start at the current screen snapshot. Headless
        // live agents instead use the cursor as the start of a journal
        // replay, but both paths still validate generation here so a cursor
        // from a recreated process fails loudly.
        if let Some(cursor) = from_cursor {
            cursor_replay_ok(stream.generation, cursor)?;
        }
        let live_agent = !stream.transcript && stream.screen.is_none();
        // A runtime without a journal has made no durability promise. Keep
        // its ordinary live queue contract; a configured journal gets the
        // lazy history replay and stored-manifest seam below.
        let live_agent_replay = if live_agent && self.journal.is_some() {
            Some(LiveAgentReplay {
                from_seq: from_cursor.map(|cursor| cursor.seq).unwrap_or(0),
                watermark: stream.next_seq.saturating_sub(1),
            })
        } else {
            None
        };
        let as_of_seq = stream.last_applied_seq;
        let screen = stream.screen.as_ref().map(Screen::snapshot);
        let mut attachment = Attachment {
            outbound: Arc::clone(&conn.outbound),
            typed_permissions,
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
        })
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
                attachment
                    .outbound
                    .enqueue_reply(devboule_protocol::DaemonMessage::SubscriptionEvent {
                    subscription_id: key.subscription_id,
                    envelope: SessionEventEnvelope {
                        session_id: self.session_id.clone(),
                        generation,
                        event: SessionEvent::AgentError {
                            message:
                                "Session generation was replaced; reattach to continue observing."
                                    .to_string(),
                        },
                    },
                });
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

fn enqueue_agent(stream: &mut StreamState, event: SessionEvent, seq: Option<u64>) {
    let permission = matches!(&event, SessionEvent::PermissionRequest { .. });
    let mut delivered = false;
    for attachment in stream.observers.values_mut() {
        if permission && !attachment.typed_permissions {
            continue;
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
    // ACP has no terminal screen to use as a replacement snapshot. Keep the
    // newest structured event and bound both the attached queue and the
    // detached backlog with the same limits.
    let newest = queue.pop_back();
    queue.clear();
    *bytes_total = 0;
    *frames_total = 0;
    if let Some(item) = newest {
        if let PendingItem::Agent { bytes, .. } = &item {
            *bytes_total = *bytes;
            *frames_total = 1;
        }
        queue.push_back(item);
    }
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
