use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use devboule_protocol::{
    CursorShape, ErrorCode, ScreenCursor, SessionEvent, SessionEventEnvelope, WireError,
};

use crate::agent_report::PeerIdentity;
use crate::outbound::ConnOut;
use crate::peer_policy::ConnPeer;
use crate::screen::{ScreenSnapshot, SnapshotCursorShape};

use super::session_runtime::LiveAgentReplay;
use super::session_types::{transcript_row_owed, AgentReplay, AttachmentKey};
use super::{Disposition, PendingEvent, PendingItem, PullState, SessionRuntime};

/// A live agent may keep publishing while SQLite is being paged. Eight page
/// boundary extensions give the journal a bounded chance to catch that tail;
/// after that point the client receives JournalDegraded instead of an
/// unbounded replay loop.
const LIVE_AGENT_REPLAY_MAX_CATCH_UPS: u8 = 8;

/// Subscriptions one connection may hold (`DESIGN-remote-agents.md` §8 R4).
///
/// A per-connection number, like every other brake in the design: a peer that
/// attaches the same session sixty-five times is either broken or probing, and
/// either way the daemon stops before the pull state does. 64 is far above a
/// client's working set (one attachment per open tab) and far below anything
/// that could grow the connection's bookkeeping without bound.
pub const MAX_SUBSCRIPTIONS: usize = 64;

fn mark_replay_parse_failure(
    runtime: &SessionRuntime,
    replay: &mut AgentReplay,
    kind: &str,
    seq: u64,
    error: impl std::fmt::Display,
) {
    runtime.mark_journal_degraded();
    replay.journal_lagged = true;
    eprintln!(
        "journal replay could not parse {kind} for live agent session {} at seq {seq}: {error}",
        runtime.session_id
    );
}

fn extend_live_agent_watermark(runtime: &SessionRuntime, replay: &mut AgentReplay) -> bool {
    let current_seq = runtime.current_agent_seq();
    if current_seq <= replay.watermark {
        return false;
    }
    if replay.catch_up_extensions < LIVE_AGENT_REPLAY_MAX_CATCH_UPS {
        replay.watermark = current_seq;
        replay.catch_up_extensions = replay.catch_up_extensions.saturating_add(1);
        return true;
    }
    // A producer that outruns the bounded catch-up window may have had older
    // pending items evicted. The journal is the recovery source, so report
    // the bounded hole explicitly instead of pretending the live tail is
    // complete.
    runtime.mark_journal_degraded();
    replay.journal_lagged = true;
    replay.force_finish = true;
    false
}

/// Stamp one event for the wire. `envelope_generation` is the generation the
/// row belongs to — the attach generation for live and current-generation
/// traffic, the record's own generation for history, because pre-attach
/// history is a record of what happened, never a position in the current
/// stream.
fn wire_event(
    session_id: &str,
    pull: &PullState,
    envelope_generation: u64,
    event: SessionEvent,
    transcript_seq: Option<u64>,
) -> PendingEvent {
    PendingEvent {
        session_id: session_id.to_string(),
        subscription_id: pull.attachment_key.subscription_id,
        attachment_generation: pull.attachment_generation,
        envelope: SessionEventEnvelope {
            session_id: session_id.to_string(),
            generation: envelope_generation,
            transcript_seq,
            event,
        },
    }
}

/// Render an owned captured screen into the wire snapshot event. Called with
/// no locks held: the ANSI presenter is O(rows x cols) and must never run
/// inside the state mutex.
fn snapshot_event(as_of_seq: u64, screen: ScreenSnapshot) -> SessionEvent {
    SessionEvent::Snapshot {
        as_of_seq,
        cols: screen.cols,
        rows: screen.rows,
        data: screen.render_ansi(),
        cursor: ScreenCursor {
            row: screen.cursor.row,
            col: screen.cursor.col,
            visible: screen.cursor.visible,
            blinking: screen.cursor.blinking,
            shape: match screen.cursor.shape {
                SnapshotCursorShape::Block => CursorShape::Block,
                SnapshotCursorShape::Underline => CursorShape::Underline,
                SnapshotCursorShape::Bar => CursorShape::Bar,
            },
        },
        alternate_screen: screen.alternate_screen,
        bracketed_paste: screen.bracketed_paste,
        line_wrap: screen.line_wrap,
        title: screen.title,
    }
}

/// One connection's own quit intent: set when THAT connection's `Shutdown`
/// was refused because other local clients were connected, and read when its
/// slot is released. The daemon stops on the last local app out only when
/// that app is the one that asked — a crash, a relaunch, or a client that
/// never asked never stops it.
/// `pub` only so `ConnHandle`'s public constructor can take it; the private
/// `event_pull` module keeps the reach crate-local.
#[derive(Clone, Default)]
pub struct QuitIntent(Arc<AtomicBool>);

impl QuitIntent {
    pub(crate) fn refuse(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub(crate) fn refused(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

/// Per-connection handle: RPC outbound plus the sessions this client pulls.
pub struct ConnHandle {
    pub id: u64,
    pub outbound: Arc<ConnOut>,
    /// This connection's own quit slot, shared with its admission guard so
    /// the release reads what the connection did while it lived.
    quit_intent: QuitIntent,
    /// Kernel-derived identity, still read by `session.rs` for the local
    /// ownership checks. `Some` for a named-pipe client, `None` for a remote
    /// (Noise) peer, whose identity is [`ConnHandle::conn_peer`].
    pub peer: Option<PeerIdentity>,
    /// The connection-level peer: `None` for the pipe, `Some(Remote)` for a
    /// peer whose Noise static key matched a pinned `peers` row. Kept in
    /// addition to `peer` so `session.rs` does not have to learn a second type
    /// while the dispatch gate still needs the remote identity.
    pub conn_peer: Option<ConnPeer>,
    /// The capability set of that peer, resolved from its `peers` row once, at
    /// connect. Empty for a local connection. The gate reads it on every
    /// request (`peer_policy::peer_allows`); a `PeerSetCaps` that removes a
    /// capability also closes that device's live connections, so a running
    /// connection can never keep a capability the row no longer grants.
    pub peer_caps: Vec<String>,
    attached: Mutex<HashMap<u64, PullState>>,
    state_events: Mutex<VecDeque<SessionEventEnvelope>>,
    next_attachment_generation: AtomicU64,
}

impl ConnHandle {
    #[cfg(test)]
    pub fn new(id: u64) -> Arc<Self> {
        Self::with_peer(id, None)
    }

    /// This connection's own `Shutdown` was refused: its leaving may be
    /// the one that stops the daemon.
    pub(crate) fn mark_quit_refused(&self) {
        self.quit_intent.refuse();
    }

    pub fn with_peer(id: u64, peer: Option<PeerIdentity>) -> Arc<Self> {
        Self::with_conn_peer(id, peer, None)
    }

    pub fn with_conn_peer(
        id: u64,
        peer: Option<PeerIdentity>,
        conn_peer: Option<ConnPeer>,
    ) -> Arc<Self> {
        Self::with_peer_caps(id, peer, conn_peer, Vec::new(), QuitIntent::default())
    }

    /// The connection constructor the serve loop uses: it has just read the
    /// peer's capability set out of the `peers` row, and carries the quit
    /// slot its admission guard will read on the way out.
    pub fn with_peer_caps(
        id: u64,
        peer: Option<PeerIdentity>,
        conn_peer: Option<ConnPeer>,
        peer_caps: Vec<String>,
        quit_intent: QuitIntent,
    ) -> Arc<Self> {
        Arc::new(Self {
            id,
            outbound: ConnOut::new(),
            peer,
            conn_peer,
            peer_caps,
            quit_intent,
            attached: Mutex::new(HashMap::new()),
            state_events: Mutex::new(VecDeque::new()),
            next_attachment_generation: AtomicU64::new(1),
        })
    }

    #[cfg(test)]
    pub(super) fn track_with_agent_replay(
        &self,
        _session_id: &str,
        runtime: Arc<SessionRuntime>,
        transcript: bool,
        transcript_cursor: Option<u64>,
        generation: u64,
        live_agent_replay: Option<LiveAgentReplay>,
    ) {
        self.track_with_subscription(
            self.id,
            runtime,
            transcript,
            transcript_cursor,
            generation,
            live_agent_replay,
        )
        .expect("test subscription id must be unique");
    }

    pub(super) fn track_with_subscription(
        &self,
        subscription_id: u64,
        runtime: Arc<SessionRuntime>,
        transcript: bool,
        transcript_cursor: Option<u64>,
        generation: u64,
        live_agent_replay: Option<LiveAgentReplay>,
    ) -> Result<(), WireError> {
        let mut map = self
            .attached
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if map.contains_key(&subscription_id) {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "subscription id is already in use on this connection",
            ));
        }
        if map.len() >= MAX_SUBSCRIPTIONS {
            return Err(WireError::new(
                ErrorCode::CapabilityNotSupported,
                format!("too many subscriptions on this connection (max {MAX_SUBSCRIPTIONS})"),
            ));
        }
        let attachment_generation = self
            .next_attachment_generation
            .fetch_add(1, Ordering::Relaxed);
        let is_pi = runtime.agent_kind() == Some(devboule_protocol::SessionKind::Pi);
        let is_codex = runtime.agent_kind() == Some(devboule_protocol::SessionKind::Codex);
        map.insert(
            subscription_id,
            PullState {
                runtime,
                attachment_key: AttachmentKey {
                    conn_id: self.id,
                    subscription_id,
                },
                transcript,
                transcript_cursor,
                agent_replay: live_agent_replay.map(|replay| AgentReplay {
                    from_seq: replay.from_seq,
                    cursor_generation: replay.from_generation,
                    cursor: replay.from_seq,
                    watermark: replay.watermark,
                    generation,
                    pending: VecDeque::new(),
                    replayed_seqs: std::collections::HashSet::new(),
                    claude_view: None,
                    codex_view: None,
                    is_pi,
                    is_codex,
                    manifest_emitted: false,
                    catch_up_extensions: 0,
                    durable_done: false,
                    journal_lagged: false,
                    force_finish: false,
                }),
                exit_sent: false,
                journal_degraded_sent: false,
                generation,
                attachment_generation,
            },
        );
        self.outbound.notify();
        Ok(())
    }

    pub(super) fn untrack_subscription(&self, subscription_id: u64) {
        self.attached
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&subscription_id);
    }

    #[cfg(test)]
    pub(super) fn untrack(&self, session_id: &str) {
        let mut attached = self
            .attached
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(subscription_id) = attached
            .iter()
            .find(|(_, pull)| pull.runtime.session_id == session_id)
            .map(|(subscription_id, _)| *subscription_id)
        {
            attached.remove(&subscription_id);
        }
    }

    pub(super) fn untrack_session(&self, session_id: &str) {
        self.attached
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .retain(|_, pull| pull.runtime.session_id != session_id);
    }

    pub(super) fn take_attached_ids(&self) -> Vec<(u64, String)> {
        self.attached
            .lock()
            .map(|mut map| {
                map.drain()
                    .map(|(subscription_id, pull)| {
                        (subscription_id, pull.runtime.session_id.clone())
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default()
    }

    /// Return the next one-shot wake needed to emit an exit after its drain
    /// window. Ordinary live sessions return `None`, so the connection writer
    /// remains asleep until a request or PTY notification arrives.
    pub fn next_exit_wake(&self) -> Option<Duration> {
        let map = self
            .attached
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        map.values()
            .filter_map(|pull| {
                if pull.exit_sent {
                    return None;
                }
                if pull.runtime.terminal_dead.load(Ordering::Acquire) {
                    return Some(Duration::ZERO);
                }
                let Ok(stream) = pull.runtime.lock_stream() else {
                    return Some(Duration::ZERO);
                };
                if SessionRuntime::ready_for_exit(&stream) {
                    // Drain elapsed (or EOF) since the last pull. There is no
                    // notify at that instant; a zero timeout makes the writer
                    // loop instead of waiting forever.
                    return Some(Duration::ZERO);
                }
                if !stream.process_exited {
                    return None;
                }
                let origin = stream.last_publish.or(stream.exit_at)?;
                Some(super::EXIT_DRAIN.saturating_sub(origin.elapsed()))
            })
            .min()
    }

    pub(crate) fn queue_state_event(&self, envelope: SessionEventEnvelope) {
        let mut events = self
            .state_events
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        // These are full rosters, so a newer transition subsumes an older
        // one. Keeping one pending snapshot prevents a slow client from
        // turning sparse lifecycle changes into an unbounded queue.
        events.clear();
        events.push_back(envelope);
        self.outbound.notify();
    }

    pub(crate) fn clear_state_events(&self) {
        self.state_events
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clear();
    }

    pub(crate) fn pull_state_events(&self) -> Vec<SessionEventEnvelope> {
        self.state_events
            .lock()
            .map(|mut events| events.drain(..).collect())
            .unwrap_or_default()
    }

    /// Pull replay + live output + exit for every session this connection
    /// is attached to. Called from the writer thread; does not send.
    pub(crate) fn event_is_current(
        &self,
        subscription_id: u64,
        attachment_generation: u64,
    ) -> bool {
        self.attached
            .lock()
            .map(|map| {
                map.get(&subscription_id)
                    .map(|pull| pull.attachment_generation)
            })
            .unwrap_or_else(|error| {
                let map = error.into_inner();
                map.get(&subscription_id)
                    .map(|pull| pull.attachment_generation)
            })
            == Some(attachment_generation)
    }

    /// Record delivery only after the corresponding envelope was written to
    /// the connection. Positioned current-generation frames advance the
    /// transcript cursor; positionless markers do not.
    pub(crate) fn event_sent(&self, event: &PendingEvent) -> Option<String> {
        let mut map = self
            .attached
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let remove = {
            let pull = map.get_mut(&event.subscription_id)?;
            if pull.attachment_generation != event.attachment_generation {
                return None;
            }
            match &event.envelope.event {
                SessionEvent::Output { seq, .. } | SessionEvent::AgentReported { seq, .. } => {
                    // A history row's seq belongs to another numbering space:
                    // only a current-generation row is a position here.
                    if event.envelope.generation == pull.generation {
                        if let Some(cursor) = pull.transcript_cursor.as_mut() {
                            *cursor = (*cursor).max(*seq);
                        }
                    }
                    false
                }
                SessionEvent::Exit { .. } | SessionEvent::Recovered { .. } => true,
                SessionEvent::Silent { .. }
                | SessionEvent::Detached
                | SessionEvent::JournalDegraded { .. }
                | SessionEvent::SessionsSnapshot { .. }
                | SessionEvent::Snapshot { .. }
                | SessionEvent::AgentMessage { .. }
                | SessionEvent::AgentUserMessage { .. }
                | SessionEvent::Steered { .. }
                | SessionEvent::AgentThought { .. }
                | SessionEvent::AvailableCommands { .. }
                | SessionEvent::AgentToolCall { .. }
                | SessionEvent::AgentToolUpdate { .. }
                | SessionEvent::AgentFinished { .. }
                | SessionEvent::ContextUsage { .. }
                | SessionEvent::PlanUsage { .. }
                | SessionEvent::AgentTaskStarted { .. }
                | SessionEvent::AgentTaskNotification { .. }
                | SessionEvent::AgentBackgroundTasksChanged { .. }
                | SessionEvent::AgentError { .. }
                | SessionEvent::AgentStderr { .. }
                | SessionEvent::PermissionRequest { .. }
                | SessionEvent::PermissionResolved { .. }
                | SessionEvent::PermissionAnswered { .. }
                | SessionEvent::SessionManifest { .. }
                | SessionEvent::SessionNotice { .. }
                | SessionEvent::AgentCreated { .. }
                | SessionEvent::ChildFinished { .. } => {
                    if let (Some(cursor), Some(seq)) = (
                        pull.transcript_cursor.as_mut(),
                        event.envelope.transcript_seq,
                    ) {
                        *cursor = (*cursor).max(seq);
                    }
                    false
                }
            }
        };
        let pull = remove
            .then(|| map.remove(&event.subscription_id))
            .flatten()?;
        drop(map);
        // Exit and Recovered are terminal for this pull. Remove the runtime
        // observer at the same delivery boundary so transcript idle cleanup
        // cannot be stranded behind an already-drained connection entry.
        let session_id = pull.runtime.session_id.clone();
        pull.runtime.detach_subscription(
            pull.attachment_key.conn_id,
            pull.attachment_key.subscription_id,
        );
        Some(session_id)
    }

    pub(crate) fn pull_events(&self) -> Vec<PendingEvent> {
        let mut map = self
            .attached
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut events = Vec::new();
        for pull in map.values_mut() {
            let session_id = pull.runtime.session_id.clone();
            if pull.transcript {
                pull_transcript_events(&session_id, pull, &mut events);
            } else {
                pull_live_events(&session_id, pull, &mut events);
            }
        }
        events
    }
}

/// Live-agent replay pull: read at most one bounded journal page at a time,
/// derive its view events, and keep them in a connection-local page-sized
/// queue. The live attachment queue is not touched until the durable
/// watermark is complete, which makes the replay/live boundary strict.
fn pull_live_agent_replay_events(
    session_id: &str,
    pull: &mut PullState,
    events: &mut Vec<PendingEvent>,
) {
    let budget = super::PULL_BATCH.saturating_sub(events.len());
    if budget == 0 {
        return;
    }
    loop {
        let Some(replay) = pull.agent_replay.as_mut() else {
            return;
        };

        if let Some((generation, seq, event)) = replay.pending.pop_front() {
            // History is a record, not a position: it carries its own
            // generation and no transcript position, so neither reader's
            // cursor can be dragged into another generation's numbering.
            let transcript_seq = if matches!(&event, SessionEvent::SessionManifest { .. }) {
                // A stored manifest summarizes runtime state; its replay watermark
                // is not the position of a transcript row.
                None
            } else {
                (generation == pull.generation).then_some(seq)
            };
            events.push(wire_event(
                session_id,
                pull,
                generation,
                event,
                transcript_seq,
            ));
            if events.len() >= super::PULL_BATCH {
                return;
            }
            continue;
        }

        if replay.durable_done {
            if !replay.force_finish && extend_live_agent_watermark(pull.runtime.as_ref(), replay) {
                replay.durable_done = false;
                continue;
            }

            if !replay.manifest_emitted {
                // Manifests are current runtime state, not replay history:
                // drop their journal-derived views and append the enriched
                // stored manifest exactly once after durable conversation
                // replay. This preserves provider and selected effort.
                let (current_seq, manifest) = pull.runtime.finish_live_agent_replay(
                    pull.attachment_key,
                    replay.from_seq,
                    &replay.replayed_seqs,
                );
                if current_seq > replay.watermark && !replay.force_finish {
                    if replay.catch_up_extensions < LIVE_AGENT_REPLAY_MAX_CATCH_UPS {
                        replay.watermark = current_seq;
                        replay.catch_up_extensions = replay.catch_up_extensions.saturating_add(1);
                        continue;
                    }
                    pull.runtime.mark_journal_degraded();
                    replay.journal_lagged = true;
                    replay.force_finish = true;
                }
                if let Some(manifest) = manifest {
                    replay
                        .pending
                        .push_back((pull.generation, replay.watermark, manifest));
                }
                replay.manifest_emitted = true;
                continue;
            }

            let needs_degraded = replay.journal_lagged
                || (!pull.journal_degraded_sent && pull.runtime.journal_degraded());
            if needs_degraded && events.len() + 1 >= super::PULL_BATCH {
                return;
            }
            if needs_degraded {
                events.push(wire_event(
                    session_id,
                    pull,
                    pull.generation,
                    pull.runtime.journal_degraded_event(),
                    None,
                ));
                pull.journal_degraded_sent = true;
            }
            pull.agent_replay = None;
            return;
        }

        if (replay.cursor_generation, replay.cursor) >= (replay.generation, replay.watermark) {
            replay.durable_done = true;
            continue;
        }

        let from_seq = replay.cursor;
        let page_result = pull.runtime.replay_journal_agent_page(
            replay.generation,
            replay.cursor_generation,
            from_seq,
            replay.watermark,
            super::PULL_BATCH,
        );
        let page = match page_result {
            Ok(Some(page)) => page,
            Ok(None) => {
                // No Journal means this runtime never promised durable
                // replay. A configured journal returning no page, however,
                // is a missing-history signal and must be loud.
                if pull.runtime.has_journal() {
                    pull.runtime.mark_journal_degraded();
                    replay.journal_lagged = true;
                }
                replay.durable_done = true;
                replay.force_finish = true;
                continue;
            }
            Err(error) => {
                pull.runtime.mark_journal_degraded();
                eprintln!(
                    "journal replay failed for live agent session {} from seq {}: {error}",
                    pull.runtime.session_id, from_seq
                );
                replay.durable_done = true;
                replay.journal_lagged = true;
                continue;
            }
        };
        if page.generation != replay.generation {
            pull.runtime.mark_journal_degraded();
            replay.durable_done = true;
            replay.journal_lagged = true;
            continue;
        }
        if page.records.is_empty() {
            if extend_live_agent_watermark(pull.runtime.as_ref(), replay) {
                continue;
            }
            replay.durable_done = true;
            if page.last_seq < replay.watermark {
                pull.runtime.mark_journal_degraded();
                replay.journal_lagged = true;
                replay.force_finish = true;
            }
            continue;
        }

        let mut page_generation = replay.cursor_generation;
        for record in page.records {
            if record.generation != page_generation {
                // The view builders are per-provider-process state; a resume
                // seam is a new process, so its rows must not be parsed with
                // the previous generation's partial view.
                replay.claude_view = None;
                replay.codex_view = None;
                page_generation = record.generation;
            }
            // Pages arrive in (generation, seq) order, so each record
            // advances the cursor lexicographically.
            replay.cursor_generation = record.generation;
            replay.cursor = record.seq;
            let derived = match record.kind {
                crate::journal::EventKind::AgentReport => {
                    match serde_json::from_slice::<SessionEvent>(&record.payload) {
                        Ok(event) => vec![event],
                        Err(error) => {
                            mark_replay_parse_failure(
                                pull.runtime.as_ref(),
                                replay,
                                "agent report",
                                record.seq,
                                error,
                            );
                            Vec::new()
                        }
                    }
                }
                crate::journal::EventKind::AcpEnvelope => {
                    match serde_json::from_slice::<serde_json::Value>(&record.payload) {
                        Ok(value) => {
                            if replay.is_codex {
                                let view = replay
                                    .codex_view
                                    .get_or_insert_with(|| crate::codex_view::CodexView::new(None));
                                view.ingest(&value)
                            } else if replay.is_pi {
                                crate::pi_view::events_from_line(&value)
                            } else {
                                let views = crate::acp_view::view_from_envelope(&value, "");
                                if !views.is_empty() {
                                    views
                                } else {
                                    let view = replay.claude_view.get_or_insert_with(|| {
                                        crate::claude_view::ClaudeView::new(None)
                                    });
                                    view.ingest(&value)
                                }
                            }
                        }
                        Err(error) => {
                            mark_replay_parse_failure(
                                pull.runtime.as_ref(),
                                replay,
                                "ACP envelope",
                                record.seq,
                                error,
                            );
                            Vec::new()
                        }
                    }
                }
                crate::journal::EventKind::Output | crate::journal::EventKind::Exit => Vec::new(),
            };
            // A journal row is not proof that replay emitted a view. ACP
            // request envelopes (notably permission requests) are retained
            // in the detached backlog because `acp_view` deliberately leaves
            // those protocol requests to the live permission broker. Only
            // rows that produced at least one replay event are eligible for
            // backlog de-duplication at the replay/live seam — and only
            // current-generation rows: the seam dedupes against live items,
            // whose seqs belong to the attach generation alone.
            if !derived.is_empty() && record.generation == replay.generation {
                replay.replayed_seqs.insert(record.seq);
            }
            for event in derived {
                // A journaled manifest is historical catalog state and can
                // clobber the live provider/effort enrichment. The stored
                // runtime manifest is emitted at the replay seam instead.
                if matches!(event, SessionEvent::SessionManifest { .. }) {
                    continue;
                }
                replay
                    .pending
                    .push_back((record.generation, record.seq, event));
            }
        }
        let watermark_extended = extend_live_agent_watermark(pull.runtime.as_ref(), replay);
        if replay.force_finish
            || (!watermark_extended
                && (replay.cursor_generation, replay.cursor)
                    >= (replay.generation, replay.watermark))
        {
            replay.durable_done = true;
            // `try_append` is intentionally asynchronous. If the journal
            // writer has not reached the attach watermark yet, the rows
            // below are only a prefix; keep that hole loud through the
            // existing JournalDegraded signal rather than claiming a
            // complete replay.
            replay.journal_lagged |= page.last_seq < replay.watermark;
        }
        // A page can contain more derived events than its row count (Claude
        // assistant frames may yield several views). Let the next loop drain
        // only the bounded local page before asking SQLite for another page.
    }
}

/// Live pull: drain a bounded batch from the attachment's pending queue and
/// convert it to wire events. Snapshot ANSI is rendered HERE, with no locks
/// held — never inside the state mutex. Only when the queue is fully empty
/// may JournalDegraded or the exit event be appended, so neither can
/// overtake output that is still queued.
fn pull_live_events(session_id: &str, pull: &mut PullState, events: &mut Vec<PendingEvent>) {
    if pull.agent_replay.is_some() {
        pull_live_agent_replay_events(session_id, pull, events);
        if pull.agent_replay.is_some() || events.len() >= super::PULL_BATCH {
            return;
        }
    }
    if pull.runtime.terminal_dead.load(Ordering::Acquire) {
        push_dead_events(session_id, pull, events);
        return;
    }
    let budget = super::PULL_BATCH.saturating_sub(events.len());
    let mut drained: Vec<PendingItem> = Vec::with_capacity(budget);
    let degraded;
    let silent_event;
    let mut exit_event = None;
    {
        let Ok(mut stream) = pull.runtime.lock_stream() else {
            push_dead_events(session_id, pull, events);
            return;
        };
        let Some(attachment) = stream.observers.get_mut(&pull.attachment_key) else {
            return;
        };
        while drained.len() < budget {
            let Some(item) = attachment.pending.pop_front() else {
                break;
            };
            match &item {
                PendingItem::Output { data, .. } => {
                    attachment.pending_bytes = attachment.pending_bytes.saturating_sub(data.len());
                    attachment.pending_frames = attachment.pending_frames.saturating_sub(1);
                }
                PendingItem::Snapshot { .. } => {}
                PendingItem::Agent { bytes, .. } => {
                    attachment.pending_bytes = attachment.pending_bytes.saturating_sub(*bytes);
                    attachment.pending_frames = attachment.pending_frames.saturating_sub(1);
                }
            }
            drained.push(item);
        }
        degraded = !pull.journal_degraded_sent && pull.runtime.journal_degraded();
        if degraded {
            pull.journal_degraded_sent = true;
        }
        silent_event = attachment
            .pending_silences
            .pop_front()
            .map(|elapsed_ms| SessionEvent::Silent { elapsed_ms });
        if !pull.exit_sent
            && attachment.pending.is_empty()
            && SessionRuntime::ready_for_exit(&stream)
        {
            exit_event = Some(match stream.disposition {
                Disposition::Recovered { integrity } => SessionEvent::Recovered { integrity },
                Disposition::Running | Disposition::Silent | Disposition::Exited { .. } => {
                    SessionEvent::Exit {
                        code: stream.exit_code,
                    }
                }
            });
            pull.exit_sent = true;
        }
    }
    emit_live_items(session_id, pull, events, drained);
    if degraded {
        events.push(wire_event(
            session_id,
            pull,
            pull.generation,
            pull.runtime.journal_degraded_event(),
            None,
        ));
    }
    if let Some(event) = silent_event {
        events.push(wire_event(session_id, pull, pull.generation, event, None));
    }
    if let Some(event) = exit_event {
        events.push(wire_event(session_id, pull, pull.generation, event, None));
    }
}

fn emit_live_items(
    session_id: &str,
    pull: &PullState,
    events: &mut Vec<PendingEvent>,
    drained: Vec<PendingItem>,
) {
    for item in drained {
        let (event, transcript_seq) = match item {
            PendingItem::Snapshot { as_of_seq, screen } => {
                (snapshot_event(as_of_seq, screen), Some(as_of_seq))
            }
            PendingItem::Output { seq, data } => (SessionEvent::Output { seq, data }, Some(seq)),
            PendingItem::Agent { seq, event, .. } => (event, seq),
        };
        events.push(wire_event(
            session_id,
            pull,
            pull.generation,
            event,
            transcript_seq,
        ));
    }
}

fn push_dead_events(session_id: &str, pull: &mut PullState, events: &mut Vec<PendingEvent>) {
    if !pull.journal_degraded_sent {
        events.push(wire_event(
            session_id,
            pull,
            pull.generation,
            pull.runtime.journal_degraded_event(),
            None,
        ));
        pull.journal_degraded_sent = true;
    }
    if !pull.exit_sent {
        events.push(wire_event(
            session_id,
            pull,
            pull.generation,
            SessionEvent::Exit { code: None },
            None,
        ));
        pull.exit_sent = true;
    }
}

/// Transcript pull: cursor-based journal/scrollback replay for a recovered
/// session. This is the M2/M3 replay contract, kept for transcripts only.
fn pull_transcript_events(session_id: &str, pull: &mut PullState, events: &mut Vec<PendingEvent>) {
    if pull.runtime.terminal_dead.load(Ordering::Acquire) {
        push_dead_events(session_id, pull, events);
        return;
    }
    let cursor = pull.transcript_cursor;
    let needs_journal = {
        let Ok(stream) = pull.runtime.lock_stream() else {
            push_dead_events(session_id, pull, events);
            return;
        };
        stream
            .scrollback
            .needs_journal_replay(cursor, stream.next_seq)
    };
    let journal_outputs = if needs_journal {
        pull.runtime.replay_journal_outputs(pull.generation)
    } else {
        Vec::new()
    };
    let replay = {
        let Ok(stream) = pull.runtime.lock_stream() else {
            push_dead_events(session_id, pull, events);
            return;
        };
        // Order key is (generation, seq): a transcript can span a resume
        // seam, and seqs restart per generation. Each row keeps the
        // generation it was written under — never the attach generation.
        let mut replay: Vec<((u64, u64), SessionEvent)> = stream
            .scrollback
            .replay_after_with_journal(cursor, pull.generation, &journal_outputs)
            .into_iter()
            .map(|((generation, seq), data)| {
                ((generation, seq), SessionEvent::Output { seq, data })
            })
            .collect();
        let cursor_seq = cursor.unwrap_or(0);
        for ((generation, seq), row_events) in &stream.transcript_agent_reports {
            if transcript_row_owed(*generation, *seq, pull.generation, cursor_seq) {
                // One row can hold several views (a finish and the context
                // reading off the same frame); `sort_by_key` is stable, so
                // they reach the wire in the order the view produced them.
                for event in row_events {
                    replay.push(((*generation, *seq), event.clone()));
                }
            }
        }
        replay.sort_by_key(|(key, _)| *key);
        replay
    };
    for ((generation, seq), event) in replay {
        // History is a record, not a position: it carries its own
        // generation and no transcript position, so neither reader's
        // cursor can be dragged into another generation's numbering.
        let transcript_seq = (generation == pull.generation).then_some(seq);
        events.push(wire_event(
            session_id,
            pull,
            generation,
            event,
            transcript_seq,
        ));
    }
    if !pull.journal_degraded_sent && pull.runtime.journal_degraded() {
        events.push(wire_event(
            session_id,
            pull,
            pull.generation,
            pull.runtime.journal_degraded_event(),
            None,
        ));
        pull.journal_degraded_sent = true;
    }
    if !pull.exit_sent {
        let Ok(stream) = pull.runtime.lock_stream() else {
            push_dead_events(session_id, pull, events);
            return;
        };
        if SessionRuntime::ready_for_exit(&stream) {
            let event = match stream.disposition {
                Disposition::Recovered { integrity } => SessionEvent::Recovered { integrity },
                Disposition::Running | Disposition::Silent | Disposition::Exited { .. } => {
                    SessionEvent::Exit {
                        code: stream.exit_code,
                    }
                }
            };
            events.push(wire_event(session_id, pull, pull.generation, event, None));
            pull.exit_sent = true;
        }
    }
}

#[cfg(test)]
#[path = "event_pull_tests.rs"]
mod tests;
