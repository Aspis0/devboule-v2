//! The harness the event-pull test topics share: the pull drain, the tracked
//! attach, and the two live-agent replay fixtures.

use std::sync::Arc;

use devboule_protocol::{SessionEvent, SessionKind, TranscriptIntegrity};

use crate::journal::{new_session_record, Journal};

use super::super::SessionRuntime;
use super::ConnHandle;

/// Pull until the session queue is empty, recording delivery like the
/// connection writer does.
pub(super) fn drain(conn: &ConnHandle) -> Vec<SessionEvent> {
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

pub(super) fn attach_tracked(runtime: &Arc<SessionRuntime>, conn: &Arc<ConnHandle>) -> u64 {
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

pub(super) fn live_agent_replay_fixture(
    session_id: &str,
    record: crate::journal::EventRecord,
) -> (
    std::path::PathBuf,
    Arc<Journal>,
    Arc<SessionRuntime>,
    Arc<ConnHandle>,
) {
    let dir = crate::test_dirs::test_temp_dir("devboule-live-agent-replay-parse");
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

pub(super) fn recovered_integrity() -> TranscriptIntegrity {
    TranscriptIntegrity::Unverifiable {
        dropped_frames: 0,
        dropped_bytes: 0,
        trimmed_bytes: 0,
    }
}
