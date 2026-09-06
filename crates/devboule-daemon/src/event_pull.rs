use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use devboule_protocol::{CursorShape, ScreenCursor, SessionEvent, SessionEventEnvelope};

use crate::agent_report::PeerIdentity;
use crate::outbound::ConnOut;
use crate::screen::{ScreenSnapshot, SnapshotCursorShape};

use super::session_runtime::LiveAgentReplay;
use super::session_types::AgentReplay;
use super::{Disposition, PendingEvent, PendingItem, PullState, SessionRuntime};

/// A live agent may keep publishing while SQLite is being paged. Eight page
/// boundary extensions give the journal a bounded chance to catch that tail;
/// after that point the client receives JournalDegraded instead of an
/// unbounded replay loop.
const LIVE_AGENT_REPLAY_MAX_CATCH_UPS: u8 = 8;

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

fn wire_event(
    session_id: &str,
    pull: &PullState,
    event: SessionEvent,
    transcript_seq: Option<u64>,
) -> PendingEvent {
    PendingEvent {
        session_id: session_id.to_string(),
        attachment_generation: pull.attachment_generation,
        envelope: SessionEventEnvelope {
            session_id: session_id.to_string(),
            generation: pull.generation,
            event,
        },
        transcript_seq,
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

/// Per-connection handle: RPC outbound plus the sessions this client pulls.
pub struct ConnHandle {
    pub id: u64,
    pub outbound: Arc<ConnOut>,
    pub peer: Option<PeerIdentity>,
    attached: Mutex<HashMap<String, PullState>>,
    state_events: Mutex<VecDeque<SessionEventEnvelope>>,
    next_attachment_generation: AtomicU64,
}

impl ConnHandle {
    #[cfg(test)]
    pub fn new(id: u64) -> Arc<Self> {
        Self::with_peer(id, None)
    }

    pub fn with_peer(id: u64, peer: Option<PeerIdentity>) -> Arc<Self> {
        Arc::new(Self {
            id,
            outbound: ConnOut::new(),
            peer,
            attached: Mutex::new(HashMap::new()),
            state_events: Mutex::new(VecDeque::new()),
            next_attachment_generation: AtomicU64::new(1),
        })
    }

    pub(super) fn track_with_agent_replay(
        &self,
        session_id: &str,
        runtime: Arc<SessionRuntime>,
        transcript: bool,
        transcript_cursor: Option<u64>,
        generation: u64,
        live_agent_replay: Option<LiveAgentReplay>,
    ) {
        let attachment_generation = self
            .next_attachment_generation
            .fetch_add(1, Ordering::Relaxed);
        let mut map = self
            .attached
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        map.insert(
            session_id.to_string(),
            PullState {
                runtime,
                transcript,
                transcript_cursor,
                agent_replay: live_agent_replay.map(|replay| AgentReplay {
                    from_seq: replay.from_seq,
                    cursor: replay.from_seq,
                    watermark: replay.watermark,
                    generation,
                    pending: VecDeque::new(),
                    replayed_seqs: std::collections::HashSet::new(),
                    claude_view: None,
                    manifest_emitted: false,
                    catch_up_extensions: 0,
                    durable_done: false,
                    journal_lagged: false,
                    force_finish: false,
                }),
                agent_backlog_after_replay: false,
                exit_sent: false,
                journal_degraded_sent: false,
                generation,
                attachment_generation,
            },
        );
        self.outbound.notify();
    }

    pub(super) fn untrack(&self, session_id: &str) {
        self.attached
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(session_id);
    }

    pub(super) fn take_attached_ids(&self) -> Vec<String> {
        self.attached
            .lock()
            .map(|mut map| map.drain().map(|(id, _)| id).collect::<Vec<_>>())
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
    pub(crate) fn event_is_current(&self, session_id: &str, attachment_generation: u64) -> bool {
        self.attached
            .lock()
            .map(|map| map.get(session_id).map(|pull| pull.attachment_generation))
            .unwrap_or_else(|error| {
                let map = error.into_inner();
                map.get(session_id).map(|pull| pull.attachment_generation)
            })
            == Some(attachment_generation)
    }

    /// Record delivery only after the corresponding envelope was written to
    /// the connection. Only the transcript replay cursor advances here: live
    /// screen state is synchronised by snapshots, so an Output written to a
    /// live stream must not look like a replay position.
    pub(crate) fn event_sent(&self, event: &PendingEvent) {
        let mut map = self
            .attached
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let remove = {
            let Some(pull) = map.get_mut(&event.session_id) else {
                return;
            };
            if pull.attachment_generation != event.attachment_generation {
                return;
            }
            match &event.envelope.event {
                SessionEvent::Output { seq, .. } | SessionEvent::AgentReported { seq, .. } => {
                    if let Some(cursor) = pull.transcript_cursor.as_mut() {
                        *cursor = (*cursor).max(*seq);
                    }
                    false
                }
                SessionEvent::Exit { .. } | SessionEvent::Recovered { .. } => true,
                SessionEvent::Silent { .. }
                | SessionEvent::JournalDegraded { .. }
                | SessionEvent::SessionsSnapshot { .. }
                | SessionEvent::Snapshot { .. }
                | SessionEvent::AgentMessage { .. }
                | SessionEvent::AgentUserMessage { .. }
                | SessionEvent::AgentThought { .. }
                | SessionEvent::AvailableCommands { .. }
                | SessionEvent::AgentToolCall { .. }
                | SessionEvent::AgentToolUpdate { .. }
                | SessionEvent::AgentFinished { .. }
                | SessionEvent::AgentError { .. }
                | SessionEvent::AgentStderr { .. }
                | SessionEvent::PermissionRequest { .. }
                | SessionEvent::PermissionResolved { .. }
                | SessionEvent::SessionManifest { .. } => {
                    if let (Some(cursor), Some(seq)) =
                        (pull.transcript_cursor.as_mut(), event.transcript_seq)
                    {
                        *cursor = (*cursor).max(seq);
                    }
                    false
                }
            }
        };
        if remove {
            map.remove(&event.session_id);
        }
    }

    pub(crate) fn pull_events(&self) -> Vec<PendingEvent> {
        let mut map = self
            .attached
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let mut events = Vec::new();
        for (session_id, pull) in map.iter_mut() {
            if pull.transcript {
                pull_transcript_events(session_id, pull, &mut events);
            } else {
                pull_live_events(session_id, pull, &mut events);
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

        if let Some((seq, event)) = replay.pending.pop_front() {
            events.push(wire_event(session_id, pull, event, Some(seq)));
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
                let current_seq = pull
                    .runtime
                    .finish_live_agent_replay(replay.from_seq, &replay.replayed_seqs);
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
                pull.agent_backlog_after_replay = true;
                if let Some(manifest) = pull.runtime.session_manifest() {
                    replay.pending.push_back((replay.watermark, manifest));
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
                    pull.runtime.journal_degraded_event(),
                    None,
                ));
                pull.journal_degraded_sent = true;
            }
            pull.agent_replay = None;
            return;
        }

        if replay.cursor >= replay.watermark {
            replay.durable_done = true;
            continue;
        }

        let from_seq = replay.cursor;
        let page_result = pull.runtime.replay_journal_agent_page(
            replay.generation,
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

        for record in page.records {
            replay.cursor = replay.cursor.max(record.seq);
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
                            if let Some(event) = crate::acp_view::view_from_envelope(&value, "") {
                                vec![event]
                            } else {
                                let view = replay.claude_view.get_or_insert_with(|| {
                                    crate::claude_view::ClaudeView::new(None)
                                });
                                view.ingest(&value)
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
            // backlog de-duplication at the replay/live seam.
            if !derived.is_empty() {
                replay.replayed_seqs.insert(record.seq);
            }
            for event in derived {
                // A journaled manifest is historical catalog state and can
                // clobber the live provider/effort enrichment. The stored
                // runtime manifest is emitted at the replay seam instead.
                if matches!(event, SessionEvent::SessionManifest { .. }) {
                    continue;
                }
                replay.pending.push_back((record.seq, event));
            }
        }
        let watermark_extended = extend_live_agent_watermark(pull.runtime.as_ref(), replay);
        if replay.force_finish || (!watermark_extended && replay.cursor >= replay.watermark) {
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
    if pull.agent_backlog_after_replay {
        while drained.len() < budget {
            let Some(item) = pull.runtime.pop_replay_backlog_item() else {
                pull.agent_backlog_after_replay = false;
                break;
            };
            drained.push(item);
        }
        if drained.len() >= budget {
            emit_live_items(session_id, pull, events, drained);
            return;
        }
    }
    let degraded;
    let silent_event;
    let mut exit_event = None;
    {
        let Ok(mut stream) = pull.runtime.lock_stream() else {
            push_dead_events(session_id, pull, events);
            return;
        };
        while drained.len() < budget {
            let Some(item) = stream.pending.pop_front() else {
                break;
            };
            match &item {
                PendingItem::Output { data, .. } => {
                    stream.pending_bytes = stream.pending_bytes.saturating_sub(data.len());
                    stream.pending_frames = stream.pending_frames.saturating_sub(1);
                }
                PendingItem::Snapshot { .. } => {}
                PendingItem::Agent { bytes, .. } => {
                    stream.pending_bytes = stream.pending_bytes.saturating_sub(*bytes);
                    stream.pending_frames = stream.pending_frames.saturating_sub(1);
                }
            }
            drained.push(item);
        }
        degraded = !pull.journal_degraded_sent && pull.runtime.journal_degraded();
        if degraded {
            pull.journal_degraded_sent = true;
        }
        silent_event = stream
            .pending_silences
            .pop_front()
            .map(|elapsed_ms| SessionEvent::Silent { elapsed_ms });
        if !pull.exit_sent && stream.pending.is_empty() && SessionRuntime::ready_for_exit(&stream) {
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
            pull.runtime.journal_degraded_event(),
            None,
        ));
    }
    if let Some(event) = silent_event {
        events.push(wire_event(session_id, pull, event, None));
    }
    if let Some(event) = exit_event {
        events.push(wire_event(session_id, pull, event, None));
    }
}

fn emit_live_items(
    session_id: &str,
    pull: &PullState,
    events: &mut Vec<PendingEvent>,
    drained: Vec<PendingItem>,
) {
    for item in drained {
        let event = match item {
            PendingItem::Snapshot { as_of_seq, screen } => snapshot_event(as_of_seq, screen),
            PendingItem::Output { seq, data } => SessionEvent::Output { seq, data },
            PendingItem::Agent { event, .. } => event,
        };
        events.push(wire_event(session_id, pull, event, None));
    }
}

fn push_dead_events(session_id: &str, pull: &mut PullState, events: &mut Vec<PendingEvent>) {
    if !pull.journal_degraded_sent {
        events.push(wire_event(
            session_id,
            pull,
            pull.runtime.journal_degraded_event(),
            None,
        ));
        pull.journal_degraded_sent = true;
    }
    if !pull.exit_sent {
        events.push(wire_event(
            session_id,
            pull,
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
        pull.runtime
            .replay_journal_outputs(cursor.unwrap_or(0), pull.generation)
    } else {
        Vec::new()
    };
    let replay = {
        let Ok(stream) = pull.runtime.lock_stream() else {
            push_dead_events(session_id, pull, events);
            return;
        };
        let mut replay: Vec<(u64, SessionEvent)> = stream
            .scrollback
            .replay_after_with_journal(cursor, &journal_outputs)
            .into_iter()
            .filter_map(|event| match event {
                SessionEvent::Output { seq, .. } => Some((seq, event)),
                _ => None,
            })
            .collect();
        let cursor_seq = cursor.unwrap_or(0);
        for (seq, event) in &stream.transcript_agent_reports {
            if *seq > cursor_seq {
                replay.push((*seq, event.clone()));
            }
        }
        replay.sort_by_key(|(seq, _)| *seq);
        replay
    };
    for (seq, event) in replay {
        events.push(wire_event(session_id, pull, event, Some(seq)));
    }
    if !pull.journal_degraded_sent && pull.runtime.journal_degraded() {
        events.push(wire_event(
            session_id,
            pull,
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
            events.push(wire_event(session_id, pull, event, None));
            pull.exit_sent = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;
    use serde_json::json;

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

    fn live_agent_replay_fixture(
        session_id: &str,
        record: crate::journal::EventRecord,
    ) -> (
        std::path::PathBuf,
        Arc<Journal>,
        Arc<SessionRuntime>,
        Arc<ConnHandle>,
    ) {
        let dir = std::env::temp_dir().join(format!(
            "devboule-live-agent-replay-parse-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
        journal
            .upsert_blocking(new_session_record(
                session_id,
                "S-1-5-21-1",
                None,
                SessionKind::Acp,
                "Agent",
            ))
            .unwrap();
        let next_seq = record.seq.saturating_add(1);
        journal.append_blocking(record).unwrap();

        let runtime = Arc::new(SessionRuntime::with_journal(
            session_id.to_string(),
            Some(Arc::clone(&journal)),
        ));
        {
            let mut stream = runtime.stream.lock().unwrap();
            stream.screen = None;
            stream.transcript = false;
            stream.next_seq = next_seq;
        }
        let conn = ConnHandle::new(1);
        let outcome = runtime
            .try_attach_with_replay(None, &conn, true)
            .expect("attach live agent");
        conn.track_with_agent_replay(
            session_id,
            Arc::clone(&runtime),
            false,
            None,
            outcome.generation,
            outcome.live_agent_replay,
        );
        (dir, journal, runtime, conn)
    }

    #[test]
    fn malformed_agent_report_replay_marks_journal_degraded() {
        let session_id = "s.live.agent.replay.malformed-report";
        let record = crate::journal::EventRecord {
            session_id: session_id.to_string(),
            generation: 1,
            seq: 1,
            kind: crate::journal::EventKind::AgentReport,
            ts_ms: 0,
            payload: b"not a SessionEvent".to_vec(),
        };
        let (dir, journal, runtime, conn) = live_agent_replay_fixture(session_id, record);
        let events = drain(&conn);
        assert!(events
            .iter()
            .any(|event| matches!(event, SessionEvent::JournalDegraded { .. })));
        assert!(runtime.journal_degraded());

        drop(runtime);
        drop(journal);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn malformed_acp_envelope_replay_marks_journal_degraded() {
        let session_id = "s.live.agent.replay.malformed-acp";
        let record = crate::journal::EventRecord {
            session_id: session_id.to_string(),
            generation: 1,
            seq: 1,
            kind: crate::journal::EventKind::AcpEnvelope,
            ts_ms: 0,
            payload: b"not JSON".to_vec(),
        };
        let (dir, journal, runtime, conn) = live_agent_replay_fixture(session_id, record);
        let events = drain(&conn);
        assert!(events
            .iter()
            .any(|event| matches!(event, SessionEvent::JournalDegraded { .. })));
        assert!(runtime.journal_degraded());

        drop(runtime);
        drop(journal);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unmodeled_acp_envelope_replay_stays_quiet() {
        let session_id = "s.live.agent.replay.unmodeled";
        let record = crate::journal::EventRecord {
            session_id: session_id.to_string(),
            generation: 1,
            seq: 1,
            kind: crate::journal::EventKind::AcpEnvelope,
            ts_ms: 0,
            payload: serde_json::to_vec(&json!({
                "method": "_auth/status_update",
                "params": {"status": "ok"}
            }))
            .unwrap(),
        };
        let (dir, journal, runtime, conn) = live_agent_replay_fixture(session_id, record);
        let events = drain(&conn);
        assert!(!events
            .iter()
            .any(|event| matches!(event, SessionEvent::JournalDegraded { .. })));
        assert!(!runtime.journal_degraded());

        drop(runtime);
        drop(journal);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn live_agent_replay_is_complete_ordered_deduplicated_and_not_pending() {
        let dir = std::env::temp_dir().join(format!(
            "devboule-live-agent-replay-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
        let session_id = "s.live.agent.replay";
        journal
            .upsert_blocking(new_session_record(
                session_id,
                "S-1-5-21-1",
                None,
                SessionKind::Acp,
                "Agent",
            ))
            .unwrap();
        let first_envelope = json!({
            "method": "session/update",
            "params": {
                "sessionId": session_id,
                "update": {
                    "sessionUpdate": "agent_message_chunk",
                    "messageId": "m1",
                    "content": {"type": "text", "text": "first"}
                }
            }
        });
        let second_envelope = json!({
            "method": "session/update",
            "params": {
                "sessionId": session_id,
                "update": {
                    "sessionUpdate": "agent_thought_chunk",
                    "content": {"type": "text", "text": "second"}
                }
            }
        });
        journal
            .append_blocking(
                crate::journal::acp_envelope_record(session_id, 1, 1, &first_envelope).unwrap(),
            )
            .unwrap();
        journal
            .append_blocking(
                crate::journal::acp_envelope_record(session_id, 1, 2, &second_envelope).unwrap(),
            )
            .unwrap();

        let runtime = Arc::new(SessionRuntime::with_journal(
            session_id.to_string(),
            Some(Arc::clone(&journal)),
        ));
        runtime.publish_agent_event_with_seq(
            SessionEvent::AgentMessage {
                message_id: Some("m1".to_string()),
                text: "first".to_string(),
            },
            None,
            Some(1),
        );
        {
            let mut stream = runtime.stream.lock().unwrap();
            stream.screen = None;
            stream.transcript = false;
            stream.next_seq = 3;
        }
        let conn = ConnHandle::new(1);
        let outcome = runtime
            .try_attach_with_replay(None, &conn, true)
            .expect("attach live agent");
        assert_eq!(
            outcome
                .live_agent_replay
                .as_ref()
                .map(|replay| replay.watermark),
            Some(2)
        );
        conn.track_with_agent_replay(
            session_id,
            Arc::clone(&runtime),
            false,
            None,
            outcome.generation,
            outcome.live_agent_replay,
        );
        assert_eq!(runtime.stream.lock().unwrap().pending.len(), 0);

        runtime.publish_agent_event(
            SessionEvent::AgentMessage {
                message_id: Some("m2".to_string()),
                text: "live".to_string(),
            },
            None,
        );
        let events = drain(&conn);
        assert_eq!(
            events,
            vec![
                SessionEvent::AgentMessage {
                    message_id: Some("m1".to_string()),
                    text: "first".to_string(),
                },
                SessionEvent::AgentThought {
                    message_id: None,
                    text: "second".to_string(),
                },
                SessionEvent::AgentMessage {
                    message_id: Some("m2".to_string()),
                    text: "live".to_string(),
                },
            ]
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event,
                    SessionEvent::AgentMessage { text, .. } if text == "first"
                ))
                .count(),
            1
        );

        drop(runtime);
        drop(journal);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn same_connection_reattach_preserves_agent_pending_once() {
        let dir = std::env::temp_dir().join(format!(
            "devboule-live-agent-same-connection-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
        let session_id = "s.live.agent.same-connection";
        journal
            .upsert_blocking(new_session_record(
                session_id,
                "S-1-5-21-1",
                None,
                SessionKind::Acp,
                "Agent",
            ))
            .unwrap();
        let runtime = Arc::new(SessionRuntime::with_journal(
            session_id.to_string(),
            Some(Arc::clone(&journal)),
        ));
        {
            let mut stream = runtime.stream.lock().unwrap();
            stream.screen = None;
            stream.transcript = false;
        }
        let conn = ConnHandle::new(1);
        let first = runtime
            .try_attach_with_replay(None, &conn, true)
            .expect("first attach");
        conn.track_with_agent_replay(
            session_id,
            Arc::clone(&runtime),
            false,
            None,
            first.generation,
            first.live_agent_replay,
        );

        for (message_id, text) in [("m1", "first"), ("m2", "second")] {
            let envelope = json!({
                "method": "session/update",
                "params": {
                    "sessionId": session_id,
                    "update": {
                        "sessionUpdate": "agent_message_chunk",
                        "messageId": message_id,
                        "content": {"type": "text", "text": text}
                    }
                }
            });
            let seq = runtime
                .journal_agent_envelope(&envelope)
                .expect("journal pending event");
            runtime.publish_agent_event_with_seq(
                SessionEvent::AgentMessage {
                    message_id: Some(message_id.to_string()),
                    text: text.to_string(),
                },
                None,
                Some(seq),
            );
        }
        journal.flush().expect("flush pending events");

        let second = runtime
            .try_attach_with_replay(None, &conn, true)
            .expect("same-connection reattach");
        conn.track_with_agent_replay(
            session_id,
            Arc::clone(&runtime),
            false,
            None,
            second.generation,
            second.live_agent_replay,
        );
        let events = drain(&conn);
        let texts = events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::AgentMessage { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(texts, vec!["first", "second"]);
        assert!(runtime.stream.lock().unwrap().agent_backlog.is_empty());

        drop(runtime);
        drop(journal);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn live_agent_replay_pages_without_filling_any_stream_queue() {
        let dir = std::env::temp_dir().join(format!(
            "devboule-live-agent-replay-pages-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
        let session_id = "s.live.agent.replay.pages";
        journal
            .upsert_blocking(new_session_record(
                session_id,
                "S-1-5-21-1",
                None,
                SessionKind::Acp,
                "Agent",
            ))
            .unwrap();
        let total = PULL_BATCH as u64 * 2 + 1;
        for seq in 1..=total {
            let envelope = json!({
                "method": "session/update",
                "params": {
                    "sessionId": session_id,
                    "update": {
                        "sessionUpdate": "agent_message_chunk",
                        "messageId": format!("m-{seq}"),
                        "content": {"type": "text", "text": format!("history-{seq}")}
                    }
                }
            });
            journal
                .append_blocking(
                    crate::journal::acp_envelope_record(session_id, 1, seq, &envelope).unwrap(),
                )
                .unwrap();
        }

        let runtime = Arc::new(SessionRuntime::with_journal(
            session_id.to_string(),
            Some(Arc::clone(&journal)),
        ));
        {
            let mut stream = runtime.stream.lock().unwrap();
            stream.screen = None;
            stream.transcript = false;
            stream.next_seq = total + 1;
        }
        let conn = ConnHandle::new(1);
        let outcome = runtime
            .try_attach_with_replay(None, &conn, true)
            .expect("attach live agent");
        conn.track_with_agent_replay(
            session_id,
            Arc::clone(&runtime),
            false,
            None,
            outcome.generation,
            outcome.live_agent_replay,
        );
        assert_eq!(runtime.stream.lock().unwrap().pending.len(), 0);

        let first = conn.pull_events();
        assert!(!first.is_empty());
        assert!(first.len() <= PULL_BATCH);
        assert_eq!(runtime.stream.lock().unwrap().pending.len(), 0);
        let replay_pending = conn
            .attached
            .lock()
            .unwrap()
            .get(session_id)
            .and_then(|pull| pull.agent_replay.as_ref())
            .map(|replay| replay.pending.len())
            .unwrap_or(0);
        assert!(
            replay_pending <= PULL_BATCH,
            "replay page exceeded pull budget: {replay_pending}"
        );

        let mut events = Vec::new();
        for event in &first {
            conn.event_sent(event);
        }
        events.extend(first.into_iter().map(|pending| pending.envelope.event));
        events.extend(drain(&conn));
        let texts: Vec<String> = events
            .into_iter()
            .filter_map(|event| match event {
                SessionEvent::AgentMessage { text, .. } => Some(text),
                _ => None,
            })
            .collect();
        assert_eq!(texts.len(), total as usize);
        for (index, text) in texts.into_iter().enumerate() {
            assert_eq!(text, format!("history-{}", index + 1));
        }

        drop(runtime);
        drop(journal);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn live_agent_replay_recovers_live_tail_after_pending_overflow() {
        let dir = std::env::temp_dir().join(format!(
            "devboule-live-agent-replay-tail-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
        let session_id = "s.live.agent.replay.tail";
        journal
            .upsert_blocking(new_session_record(
                session_id,
                "S-1-5-21-1",
                None,
                SessionKind::Acp,
                "Agent",
            ))
            .unwrap();
        let history = 80_u64;
        for seq in 1..=history {
            let envelope = json!({
                "method": "session/update",
                "params": {
                    "sessionId": session_id,
                    "update": {
                        "sessionUpdate": "agent_message_chunk",
                        "messageId": format!("history-{seq}"),
                        "content": {"type": "text", "text": format!("history-{seq}")}
                    }
                }
            });
            journal
                .append_blocking(
                    crate::journal::acp_envelope_record(session_id, 1, seq, &envelope).unwrap(),
                )
                .unwrap();
        }

        let runtime = Arc::new(SessionRuntime::with_journal(
            session_id.to_string(),
            Some(Arc::clone(&journal)),
        ));
        {
            let mut stream = runtime.stream.lock().unwrap();
            stream.screen = None;
            stream.transcript = false;
            stream.next_seq = history + 1;
        }
        let conn = ConnHandle::new(1);
        let outcome = runtime
            .try_attach_with_replay(None, &conn, true)
            .expect("attach live agent");
        conn.track_with_agent_replay(
            session_id,
            Arc::clone(&runtime),
            false,
            None,
            outcome.generation,
            outcome.live_agent_replay,
        );

        let first = conn.pull_events();
        for event in &first {
            conn.event_sent(event);
        }
        let mut events = first
            .into_iter()
            .map(|pending| pending.envelope.event)
            .collect::<Vec<_>>();

        for seq in 1..=80_u64 {
            let envelope = json!({
                "method": "session/update",
                "params": {
                    "sessionId": session_id,
                    "update": {
                        "sessionUpdate": "agent_message_chunk",
                        "messageId": format!("live-{seq}"),
                        "content": {"type": "text", "text": format!("live-{seq}")}
                    }
                }
            });
            let journal_seq = runtime
                .journal_agent_envelope(&envelope)
                .expect("journal live tail envelope");
            assert_eq!(journal_seq, history + seq);
            runtime.publish_agent_event_with_seq(
                SessionEvent::AgentMessage {
                    message_id: Some(format!("live-{seq}")),
                    text: format!("live-{seq}"),
                },
                None,
                Some(journal_seq),
            );
        }
        journal.flush().expect("flush live tail envelopes");
        events.extend(drain(&conn));

        let texts: Vec<String> = events
            .into_iter()
            .filter_map(|event| match event {
                SessionEvent::AgentMessage { text, .. } => Some(text),
                _ => None,
            })
            .collect();
        let expected: Vec<String> = (1..=history)
            .map(|seq| format!("history-{seq}"))
            .chain((1..=80).map(|seq| format!("live-{seq}")))
            .collect();
        assert_eq!(texts, expected);

        drop(runtime);
        drop(journal);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn live_agent_replay_uses_stored_manifest_state() {
        let dir = std::env::temp_dir().join(format!(
            "devboule-live-agent-replay-manifest-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
        let session_id = "s.live.agent.replay.manifest";
        journal
            .upsert_blocking(new_session_record(
                session_id,
                "S-1-5-21-1",
                None,
                SessionKind::Acp,
                "Agent",
            ))
            .unwrap();
        let journal_manifest = json!({
            "method": "_x.ai/models/update",
            "params": {
                "currentModelId": "grok-live",
                "availableModels": [{
                    "modelId": "grok-live",
                    "name": "Grok Live",
                    "_meta": {
                        "supportsReasoningEffort": true,
                        "reasoningEffort": "low",
                        "reasoningEfforts": [{"id": "low", "label": "Low"}, {"id": "high", "label": "High"}]
                    }
                }]
            }
        });
        journal
            .append_blocking(
                crate::journal::acp_envelope_record(session_id, 1, 1, &journal_manifest).unwrap(),
            )
            .unwrap();

        let runtime = Arc::new(SessionRuntime::with_journal(
            session_id.to_string(),
            Some(Arc::clone(&journal)),
        ));
        runtime.store_session_manifest(SessionEvent::SessionManifest {
            provider_id: Some("grok".to_string()),
            current_model_id: Some("grok-live".to_string()),
            models: vec![devboule_protocol::SessionModel {
                model_id: "grok-live".to_string(),
                name: "Grok Live".to_string(),
                description: None,
                context_tokens: None,
                current_effort: Some("high".to_string()),
                efforts: None,
            }],
            modes: None,
        });
        {
            let mut stream = runtime.stream.lock().unwrap();
            stream.screen = None;
            stream.transcript = false;
            stream.next_seq = 2;
        }
        let conn = ConnHandle::new(1);
        let outcome = runtime
            .try_attach_with_replay(None, &conn, true)
            .expect("attach live agent");
        conn.track_with_agent_replay(
            session_id,
            Arc::clone(&runtime),
            false,
            None,
            outcome.generation,
            outcome.live_agent_replay,
        );
        let events = drain(&conn);
        let manifests: Vec<&SessionEvent> = events
            .iter()
            .filter(|event| matches!(event, SessionEvent::SessionManifest { .. }))
            .collect();
        assert_eq!(manifests.len(), 1, "manifest must arrive exactly once");
        let SessionEvent::SessionManifest {
            provider_id,
            current_model_id,
            models,
            ..
        } = manifests[0]
        else {
            unreachable!();
        };
        assert_eq!(provider_id.as_deref(), Some("grok"));
        assert_eq!(current_model_id.as_deref(), Some("grok-live"));
        assert_eq!(models[0].current_effort.as_deref(), Some("high"));

        drop(runtime);
        drop(journal);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn live_agent_replay_marks_an_empty_journal_prefix_degraded() {
        let dir = std::env::temp_dir().join(format!(
            "devboule-live-agent-replay-empty-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
        let session_id = "s.live.agent.replay.empty";
        journal
            .upsert_blocking(new_session_record(
                session_id,
                "S-1-5-21-1",
                None,
                SessionKind::Acp,
                "Agent",
            ))
            .unwrap();
        let runtime = Arc::new(SessionRuntime::with_journal(
            session_id.to_string(),
            Some(Arc::clone(&journal)),
        ));
        {
            let mut stream = runtime.stream.lock().unwrap();
            stream.screen = None;
            stream.transcript = false;
            stream.next_seq = 2;
        }
        let conn = ConnHandle::new(1);
        let outcome = runtime
            .try_attach_with_replay(None, &conn, true)
            .expect("attach live agent");
        conn.track_with_agent_replay(
            session_id,
            Arc::clone(&runtime),
            false,
            None,
            outcome.generation,
            outcome.live_agent_replay,
        );
        let events = drain(&conn);
        assert!(events
            .iter()
            .any(|event| matches!(event, SessionEvent::JournalDegraded { .. })));
        assert!(runtime.journal_degraded());

        drop(runtime);
        drop(journal);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn live_agent_attach_keeps_generation_mismatch_loud() {
        let runtime = Arc::new(SessionRuntime::new());
        {
            let mut stream = runtime.stream.lock().unwrap();
            stream.screen = None;
            stream.transcript = false;
            stream.generation = 2;
        }
        runtime.generation.store(2, Ordering::Release);
        let conn = ConnHandle::new(1);
        let error = match runtime.try_attach_with_replay(
            Some(Cursor {
                generation: 1,
                seq: 0,
            }),
            &conn,
            true,
        ) {
            Ok(_) => panic!("stale live-agent cursor was accepted"),
            Err(error) => error,
        };
        assert_eq!(error.code, ErrorCode::SessionGenerationMismatch);
    }

    #[test]
    fn live_claude_replay_derives_journaled_views() {
        let dir = std::env::temp_dir().join(format!(
            "devboule-live-claude-replay-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
        let session_id = "s.live.claude.replay";
        journal
            .upsert_blocking(new_session_record(
                session_id,
                "S-1-5-21-1",
                None,
                SessionKind::Claude,
                "Claude",
            ))
            .unwrap();
        let init = json!({
            "type": "system",
            "subtype": "init",
            "session_id": "claude-peer",
            "model": "claude-test"
        });
        let assistant = json!({
            "type": "assistant",
            "message": {
                "id": "message-1",
                "model": "claude-test",
                "content": [{"type": "text", "text": "claude replay"}]
            }
        });
        journal
            .append_blocking(crate::journal::acp_envelope_record(session_id, 1, 1, &init).unwrap())
            .unwrap();
        journal
            .append_blocking(
                crate::journal::acp_envelope_record(session_id, 1, 2, &assistant).unwrap(),
            )
            .unwrap();

        let runtime = Arc::new(SessionRuntime::with_journal(
            session_id.to_string(),
            Some(Arc::clone(&journal)),
        ));
        runtime.store_session_manifest(SessionEvent::SessionManifest {
            provider_id: Some("claude".to_string()),
            current_model_id: Some("claude-test".to_string()),
            models: vec![devboule_protocol::SessionModel {
                model_id: "claude-test".to_string(),
                name: "Claude Test".to_string(),
                description: None,
                context_tokens: None,
                current_effort: None,
                efforts: None,
            }],
            modes: None,
        });
        {
            let mut stream = runtime.stream.lock().unwrap();
            stream.screen = None;
            stream.transcript = false;
            stream.next_seq = 3;
        }
        let conn = ConnHandle::new(1);
        let outcome = runtime
            .try_attach_with_replay(None, &conn, true)
            .expect("attach live Claude session");
        conn.track_with_agent_replay(
            session_id,
            Arc::clone(&runtime),
            false,
            None,
            outcome.generation,
            outcome.live_agent_replay,
        );
        let events = drain(&conn);
        assert!(events.iter().any(|event| {
            matches!(event, SessionEvent::SessionManifest { current_model_id, .. } if current_model_id.as_deref() == Some("claude-test"))
        }));
        assert!(events.iter().any(|event| {
            matches!(event, SessionEvent::AgentMessage { text, .. } if text == "claude replay")
        }));

        drop(runtime);
        drop(journal);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn poisoned_stream_is_dead_and_not_reused() {
        let runtime = Arc::new(SessionRuntime::new());
        let conn = ConnHandle::new(1);
        attach_tracked(&runtime, &conn);
        let poisoned = Arc::clone(&runtime);
        let panic = std::thread::spawn(move || {
            let _stream = poisoned.stream.lock().expect("stream lock");
            panic!("simulate a terminal-state panic");
        });
        assert!(panic.join().is_err());

        runtime.publish_output("must not be applied");
        let events = drain(&conn);
        assert!(runtime.terminal_dead.load(Ordering::Acquire));
        assert!(matches!(
            events.as_slice(),
            [
                SessionEvent::JournalDegraded {
                    dropped_frames: 0,
                    dropped_bytes: 0,
                },
                SessionEvent::Exit { code: None }
            ]
        ));
        assert!(matches!(
            runtime.try_attach_with_replay(None, &conn, false),
            Err(error) if error == process_gone()
        ));
    }

    #[test]
    fn transcript_cursor_replays_only_after() {
        let runtime = Arc::new(SessionRuntime::new());
        runtime.bump_generation();
        let conn = ConnHandle::new(1);
        let outcome = runtime
            .try_attach_with_replay(
                Some(Cursor {
                    generation: 2,
                    seq: 1,
                }),
                &conn,
                false,
            )
            .unwrap();
        conn.track_with_agent_replay(
            "s.a.1",
            Arc::clone(&runtime),
            true,
            Some(1),
            outcome.generation,
            outcome.live_agent_replay,
        );
        runtime.stream.lock().unwrap().scrollback.push(2, b"two");
        runtime.finish(Some(0));
        let events = drain(&conn);
        assert_eq!(
            events,
            vec![
                SessionEvent::Output {
                    seq: 2,
                    data: "two".to_string()
                },
                SessionEvent::Exit { code: Some(0) },
            ]
        );
    }

    fn recovered_integrity() -> TranscriptIntegrity {
        TranscriptIntegrity::Unverifiable {
            dropped_frames: 0,
            dropped_bytes: 0,
            trimmed_bytes: 0,
        }
    }

    #[test]
    fn recovered_acp_views_must_not_vanish_behind_a_high_output_cursor() {
        let integrity = recovered_integrity();
        let replay = crate::journal::Replay {
            generation: 1,
            last_seq: 11,
            integrity,
            event_seqs: vec![10, 11, 11],
            events: vec![
                SessionEvent::Output {
                    seq: 10,
                    data: "shell".to_string(),
                },
                SessionEvent::AgentThought {
                    message_id: None,
                    text: "The".to_string(),
                },
                SessionEvent::Recovered { integrity },
            ],
        };
        let runtime = SessionRuntime::from_replay("s.acp.replay".to_string(), None, replay);
        let conn = ConnHandle::new(1);
        let outcome = runtime
            .try_attach_with_replay(None, &conn, false)
            .expect("attach");
        conn.track_with_agent_replay(
            "s.acp.replay",
            Arc::clone(&runtime),
            true,
            Some(10),
            outcome.generation,
            outcome.live_agent_replay,
        );
        let events = drain(&conn);
        assert!(
            events.iter().any(|event| matches!(
                event,
                SessionEvent::AgentThought { text, .. } if text == "The"
            )),
            "reattach after seq 10 dropped the ACP thought: {events:?}"
        );
    }

    #[test]
    fn transcript_cursor_advances_past_agent_reported() {
        let integrity = recovered_integrity();
        let replay = crate::journal::Replay {
            generation: 1,
            last_seq: 3,
            integrity,
            event_seqs: vec![2, 3, 3],
            events: vec![
                SessionEvent::Output {
                    seq: 2,
                    data: "out".to_string(),
                },
                SessionEvent::AgentReported {
                    seq: 3,
                    source: "devboule:stub".to_string(),
                    agent: "stub".to_string(),
                    state: devboule_protocol::AgentActivityState::Working,
                    message: None,
                    report_seq: Some(1),
                    agent_session_id: None,
                    agent_session_path: None,
                    session_start_source: None,
                },
                SessionEvent::Recovered { integrity },
            ],
        };
        let runtime = SessionRuntime::from_replay("s.report.cursor".to_string(), None, replay);
        let conn = ConnHandle::new(1);
        let outcome = runtime
            .try_attach_with_replay(None, &conn, false)
            .expect("attach");
        conn.track_with_agent_replay(
            "s.report.cursor",
            Arc::clone(&runtime),
            true,
            Some(0),
            outcome.generation,
            outcome.live_agent_replay,
        );
        let first = {
            let batch = conn.pull_events();
            for event in &batch {
                if matches!(
                    event.envelope.event,
                    SessionEvent::Recovered { .. } | SessionEvent::Exit { .. }
                ) {
                    continue;
                }
                conn.event_sent(event);
            }
            batch
        };
        assert!(first.iter().any(|event| matches!(
            event.envelope.event,
            SessionEvent::AgentReported { seq: 3, .. }
        )));
        let cursor = conn
            .attached
            .lock()
            .expect("attached")
            .get("s.report.cursor")
            .and_then(|pull| pull.transcript_cursor);
        assert_eq!(
            cursor,
            Some(3),
            "cursor stayed at {cursor:?} after delivering AgentReported seq 3"
        );
        runtime.detach_if_conn(conn.id);
        let conn2 = ConnHandle::new(2);
        let outcome = runtime
            .try_attach_with_replay(None, &conn2, false)
            .expect("reattach");
        conn2.track_with_agent_replay(
            "s.report.cursor",
            Arc::clone(&runtime),
            true,
            cursor,
            outcome.generation,
            outcome.live_agent_replay,
        );
        let second = conn2.pull_events();
        assert!(
            !second
                .iter()
                .any(|event| matches!(event.envelope.event, SessionEvent::AgentReported { .. })),
            "AgentReported was delivered again on reattach: {:?}",
            second
                .iter()
                .map(|event| format!("{:?}", event.envelope.event))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn live_attach_ends_with_snapshot_output_then_exit() {
        let runtime = Arc::new(SessionRuntime::new());
        runtime.publish_output("before");
        let conn = ConnHandle::new(1);
        attach_tracked(&runtime, &conn);
        runtime.publish_output("after");
        runtime.finish(Some(0));
        let events = drain(&conn);
        let kinds: Vec<&str> = events
            .iter()
            .map(|event| match event {
                SessionEvent::Snapshot { .. } => "snapshot",
                SessionEvent::Output { .. } => "output",
                SessionEvent::Exit { .. } => "exit",
                SessionEvent::Recovered { .. } => "recovered",
                SessionEvent::Silent { .. } => "silent",
                SessionEvent::JournalDegraded { .. } => "journal_degraded",
                SessionEvent::SessionsSnapshot { .. } => "sessions_snapshot",
                SessionEvent::AgentMessage { .. } => "agent_message",
                SessionEvent::AgentUserMessage { .. } => "agent_user_message",
                SessionEvent::AgentThought { .. } => "agent_thought",
                SessionEvent::AvailableCommands { .. } => "available_commands",
                SessionEvent::AgentToolCall { .. } => "agent_tool_call",
                SessionEvent::AgentToolUpdate { .. } => "agent_tool_update",
                SessionEvent::AgentFinished { .. } => "agent_finished",
                SessionEvent::AgentError { .. } => "agent_error",
                SessionEvent::AgentStderr { .. } => "agent_stderr",
                SessionEvent::PermissionRequest { .. } => "permission_request",
                SessionEvent::PermissionResolved { .. } => "permission_resolved",
                SessionEvent::SessionManifest { .. } => "session_manifest",
                SessionEvent::AgentReported { .. } => "agent_reported",
            })
            .collect();
        assert_eq!(kinds, ["snapshot", "output", "exit"]);
        // Exit must not overtake queued output.
        assert!(matches!(
            events.last(),
            Some(SessionEvent::Exit { code: Some(0) })
        ));
    }

    #[test]
    fn journal_keeps_every_frame_for_recovery() {
        let dir = std::env::temp_dir().join(format!(
            "devboule-recovery-{}-{}",
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
                "s.recover.1",
                "S-1-5-21-1",
                None,
                SessionKind::Terminal,
                "Terminal",
            ))
            .unwrap();
        let runtime = Arc::new(SessionRuntime::with_journal(
            "s.recover.1".into(),
            Some(Arc::clone(&journal)),
        ));
        let payload = "x".repeat(8192);
        for _ in 1..=300 {
            runtime.publish_output(&payload);
        }
        runtime.finish(Some(0));
        journal.flush().unwrap();

        let replay = journal.replay("s.recover.1", 0).unwrap();
        let seqs: Vec<u64> = replay
            .events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::Output { seq, .. } => Some(*seq),
                _ => None,
            })
            .collect();
        assert_eq!(seqs, (1..=300).collect::<Vec<_>>());

        // The recovered runtime is a transcript: no emulator, journal replay
        // instead of a snapshot. Hydrating at 150 leaves a prefix that only
        // a journal read can fill, so the attach below exercises the seam.
        let replay_late = journal.replay("s.recover.1", 150).unwrap();
        let recovered = SessionRuntime::from_replay(
            "s.recover.1".into(),
            Some(Arc::clone(&journal)),
            replay_late,
        );
        assert!(recovered.is_transcript());
        assert_eq!(recovered.transcript_chunks().len(), 150);
        let conn = ConnHandle::new(1);
        attach_tracked(&recovered, &conn);
        let events = drain(&conn);
        let seqs: Vec<u64> = events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::Output { seq, .. } => Some(*seq),
                _ => None,
            })
            .collect();
        assert_eq!(seqs, (1..=300).collect::<Vec<_>>());
        assert_eq!(
            recovered.journal_replay_count(),
            1,
            "one journal read filled the prefix, not one per pull"
        );
        drop(recovered);
        drop(runtime);
        drop(journal);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn next_exit_wake_is_zero_once_the_drain_has_elapsed() {
        let runtime = Arc::new(SessionRuntime::new());
        let conn = ConnHandle::new(1);
        let outcome = runtime.try_attach_with_replay(None, &conn, false).unwrap();
        conn.track_with_agent_replay(
            "s.a.1",
            Arc::clone(&runtime),
            false,
            None,
            outcome.generation,
            outcome.live_agent_replay,
        );
        assert_eq!(conn.next_exit_wake(), None);
        runtime.mark_exited(Some(0));
        let wake = conn.next_exit_wake().expect("drain timer");
        assert!(wake <= EXIT_DRAIN);
        std::thread::sleep(EXIT_DRAIN + Duration::from_millis(10));
        assert_eq!(conn.next_exit_wake(), Some(Duration::ZERO));
        let events = conn.pull_events();
        assert!(
            events
                .iter()
                .any(|envelope| matches!(envelope.envelope.event, SessionEvent::Exit { .. })),
            "zero wake must let the writer emit Exit, got {events:?}"
        );
        for event in &events {
            conn.event_sent(event);
        }
    }

    #[test]
    fn live_journal_degradation_reaches_attached_client_once() {
        let runtime = Arc::new(SessionRuntime::new());
        let conn = ConnHandle::new(1);
        attach_tracked(&runtime, &conn);

        runtime.publish_output("still live");
        runtime.mark_journal_degraded();
        runtime.mark_journal_degraded();

        let events = drain(&conn);
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, SessionEvent::JournalDegraded { .. }))
                .count(),
            1,
            "degradation must be delivered exactly once: {events:?}"
        );
        assert!(events
            .iter()
            .all(|event| !matches!(event, SessionEvent::Exit { .. })));
        assert!(!runtime.stream.lock().unwrap().process_exited);
    }

    #[test]
    fn recovered_pull_ends_with_recovered_not_exit() {
        let replay = crate::journal::Replay {
            generation: 1,
            last_seq: 1,
            integrity: TranscriptIntegrity::Unverifiable {
                dropped_frames: 0,
                dropped_bytes: 0,
                trimmed_bytes: 0,
            },
            event_seqs: vec![1, 1],
            events: vec![
                SessionEvent::Output {
                    seq: 1,
                    data: "hello".to_string(),
                },
                SessionEvent::Recovered {
                    integrity: TranscriptIntegrity::Unverifiable {
                        dropped_frames: 0,
                        dropped_bytes: 0,
                        trimmed_bytes: 0,
                    },
                },
            ],
        };
        let runtime = SessionRuntime::from_replay("s.a.1".to_string(), None, replay);
        let conn = ConnHandle::new(1);
        attach_tracked(&runtime, &conn);
        let events = drain(&conn);
        let kinds: Vec<&str> = events
            .iter()
            .map(|event| match event {
                SessionEvent::Output { .. } => "output",
                SessionEvent::Exit { .. } => "exit",
                SessionEvent::Recovered { .. } => "recovered",
                SessionEvent::Silent { .. } => "silent",
                SessionEvent::JournalDegraded { .. } => "journal_degraded",
                SessionEvent::SessionsSnapshot { .. } => "sessions_snapshot",
                SessionEvent::Snapshot { .. } => "snapshot",
                SessionEvent::AgentMessage { .. } => "agent_message",
                SessionEvent::AgentUserMessage { .. } => "agent_user_message",
                SessionEvent::AgentThought { .. } => "agent_thought",
                SessionEvent::AvailableCommands { .. } => "available_commands",
                SessionEvent::AgentToolCall { .. } => "agent_tool_call",
                SessionEvent::AgentToolUpdate { .. } => "agent_tool_update",
                SessionEvent::AgentFinished { .. } => "agent_finished",
                SessionEvent::AgentError { .. } => "agent_error",
                SessionEvent::AgentStderr { .. } => "agent_stderr",
                SessionEvent::PermissionRequest { .. } => "permission_request",
                SessionEvent::PermissionResolved { .. } => "permission_resolved",
                SessionEvent::SessionManifest { .. } => "session_manifest",
                SessionEvent::AgentReported { .. } => "agent_reported",
            })
            .collect();
        assert_eq!(kinds, ["output", "recovered"]);
    }
}
