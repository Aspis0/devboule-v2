use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use devboule_protocol::{CursorShape, ScreenCursor, SessionEvent, SessionEventEnvelope};

use crate::agent_report::PeerIdentity;
use crate::outbound::ConnOut;
use crate::screen::{ScreenSnapshot, SnapshotCursorShape};

use super::{Disposition, PendingEvent, PendingItem, PullState, SessionRuntime};

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

    pub(super) fn track(
        &self,
        session_id: &str,
        runtime: Arc<SessionRuntime>,
        transcript: bool,
        transcript_cursor: Option<u64>,
        generation: u64,
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
                SessionEvent::Output { seq, .. } => {
                    if let Some(cursor) = pull.transcript_cursor.as_mut() {
                        *cursor = *seq;
                    }
                    false
                }
                SessionEvent::Exit { .. } | SessionEvent::Recovered { .. } => true,
                SessionEvent::Silent { .. }
                | SessionEvent::JournalDegraded { .. }
                | SessionEvent::SessionsSnapshot { .. }
                | SessionEvent::Snapshot { .. }
                | SessionEvent::AgentMessage { .. }
                | SessionEvent::AgentToolCall { .. }
                | SessionEvent::AgentToolUpdate { .. }
                | SessionEvent::AgentFinished { .. }
                | SessionEvent::AgentError { .. }
                | SessionEvent::AgentStderr { .. }
                | SessionEvent::PermissionRequest { .. }
                | SessionEvent::AgentReported { .. } => false,
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

/// Live pull: drain a bounded batch from the attachment's pending queue and
/// convert it to wire events. Snapshot ANSI is rendered HERE, with no locks
/// held — never inside the state mutex. Only when the queue is fully empty
/// may JournalDegraded or the exit event be appended, so neither can
/// overtake output that is still queued.
fn pull_live_events(session_id: &str, pull: &mut PullState, events: &mut Vec<PendingEvent>) {
    if pull.runtime.terminal_dead.load(Ordering::Acquire) {
        push_dead_events(session_id, pull, events);
        return;
    }
    let mut drained = Vec::with_capacity(super::PULL_BATCH);
    let degraded;
    let silent_event;
    let mut exit_event = None;
    {
        let Ok(mut stream) = pull.runtime.lock_stream() else {
            push_dead_events(session_id, pull, events);
            return;
        };
        while drained.len() < super::PULL_BATCH {
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
    for item in drained {
        let event = match item {
            PendingItem::Snapshot { as_of_seq, screen } => snapshot_event(as_of_seq, screen),
            PendingItem::Output { seq, data } => SessionEvent::Output { seq, data },
            PendingItem::Agent { event, .. } => event,
        };
        events.push(PendingEvent {
            session_id: session_id.to_string(),
            attachment_generation: pull.attachment_generation,
            envelope: SessionEventEnvelope {
                session_id: session_id.to_string(),
                generation: pull.generation,
                event,
            },
        });
    }
    if degraded {
        events.push(PendingEvent {
            session_id: session_id.to_string(),
            attachment_generation: pull.attachment_generation,
            envelope: SessionEventEnvelope {
                session_id: session_id.to_string(),
                generation: pull.generation,
                event: pull.runtime.journal_degraded_event(),
            },
        });
    }
    if let Some(event) = silent_event {
        events.push(PendingEvent {
            session_id: session_id.to_string(),
            attachment_generation: pull.attachment_generation,
            envelope: SessionEventEnvelope {
                session_id: session_id.to_string(),
                generation: pull.generation,
                event,
            },
        });
    }
    if let Some(event) = exit_event {
        events.push(PendingEvent {
            session_id: session_id.to_string(),
            attachment_generation: pull.attachment_generation,
            envelope: SessionEventEnvelope {
                session_id: session_id.to_string(),
                generation: pull.generation,
                event,
            },
        });
    }
}

fn push_dead_events(session_id: &str, pull: &mut PullState, events: &mut Vec<PendingEvent>) {
    if !pull.journal_degraded_sent {
        events.push(PendingEvent {
            session_id: session_id.to_string(),
            attachment_generation: pull.attachment_generation,
            envelope: SessionEventEnvelope {
                session_id: session_id.to_string(),
                generation: pull.generation,
                event: pull.runtime.journal_degraded_event(),
            },
        });
        pull.journal_degraded_sent = true;
    }
    if !pull.exit_sent {
        events.push(PendingEvent {
            session_id: session_id.to_string(),
            attachment_generation: pull.attachment_generation,
            envelope: SessionEventEnvelope {
                session_id: session_id.to_string(),
                generation: pull.generation,
                event: SessionEvent::Exit { code: None },
            },
        });
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
        let mut replay = stream
            .scrollback
            .replay_after_with_journal(cursor, &journal_outputs);
        let cursor_seq = cursor.unwrap_or(0);
        for (seq, event) in &stream.transcript_agent_reports {
            if *seq > cursor_seq {
                replay.push(event.clone());
            }
        }
        replay.sort_by_key(|event| match event {
            SessionEvent::Output { seq, .. } | SessionEvent::AgentReported { seq, .. } => *seq,
            _ => u64::MAX,
        });
        replay
    };
    for event in replay {
        events.push(PendingEvent {
            session_id: session_id.to_string(),
            attachment_generation: pull.attachment_generation,
            envelope: SessionEventEnvelope {
                session_id: session_id.to_string(),
                generation: pull.generation,
                event,
            },
        });
    }
    if !pull.journal_degraded_sent && pull.runtime.journal_degraded() {
        events.push(PendingEvent {
            session_id: session_id.to_string(),
            attachment_generation: pull.attachment_generation,
            envelope: SessionEventEnvelope {
                session_id: session_id.to_string(),
                generation: pull.generation,
                event: pull.runtime.journal_degraded_event(),
            },
        });
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
            events.push(PendingEvent {
                session_id: session_id.to_string(),
                attachment_generation: pull.attachment_generation,
                envelope: SessionEventEnvelope {
                    session_id: session_id.to_string(),
                    generation: pull.generation,
                    event,
                },
            });
            pull.exit_sent = true;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::*;
    use super::*;

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
        let generation = runtime.try_attach(None, conn, false).expect("attach");
        let transcript = runtime.is_transcript();
        conn.track(
            "s.a.1",
            Arc::clone(runtime),
            transcript,
            Some(0),
            generation,
        );
        generation
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
        assert_eq!(runtime.try_attach(None, &conn, false), Err(process_gone()));
    }

    #[test]
    fn transcript_cursor_replays_only_after() {
        let runtime = Arc::new(SessionRuntime::new());
        runtime.bump_generation();
        let conn = ConnHandle::new(1);
        let generation = runtime
            .try_attach(
                Some(Cursor {
                    generation: 2,
                    seq: 1,
                }),
                &conn,
                false,
            )
            .unwrap();
        conn.track("s.a.1", Arc::clone(&runtime), true, Some(1), generation);
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
                SessionEvent::AgentToolCall { .. } => "agent_tool_call",
                SessionEvent::AgentToolUpdate { .. } => "agent_tool_update",
                SessionEvent::AgentFinished { .. } => "agent_finished",
                SessionEvent::AgentError { .. } => "agent_error",
                SessionEvent::AgentStderr { .. } => "agent_stderr",
                SessionEvent::PermissionRequest { .. } => "permission_request",
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
        runtime.try_attach(None, &conn, false).unwrap();
        conn.track("s.a.1", Arc::clone(&runtime), false, None, 1);
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
                SessionEvent::AgentToolCall { .. } => "agent_tool_call",
                SessionEvent::AgentToolUpdate { .. } => "agent_tool_update",
                SessionEvent::AgentFinished { .. } => "agent_finished",
                SessionEvent::AgentError { .. } => "agent_error",
                SessionEvent::AgentStderr { .. } => "agent_stderr",
                SessionEvent::PermissionRequest { .. } => "permission_request",
                SessionEvent::AgentReported { .. } => "agent_reported",
            })
            .collect();
        assert_eq!(kinds, ["output", "recovered"]);
    }
}
