//! The harness the event-pull test topics share: the pull drain, the tracked
//! attach, and the live-agent replay fixtures.

use std::sync::Arc;

use devboule_protocol::{SessionEvent, SessionKind, TranscriptIntegrity};

use crate::journal::{new_session_record, EventRecord, Journal, SessionRecord};

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

/// A live agent runtime on a fresh journal under `dir` holding `session` and
/// `rows`, attached and tracked on a fresh connection. The runtime carries the
/// row's peer session id, as a restored session does.
pub(super) fn attach_live_agent_replay(
    dir: &std::path::Path,
    session: SessionRecord,
    agent_kind: Option<SessionKind>,
    rows: Vec<EventRecord>,
) -> (Arc<Journal>, Arc<SessionRuntime>, Arc<ConnHandle>) {
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
    let session_id = session.id.clone();
    let peer_session_id = session.peer_session_id.clone();
    journal.upsert_blocking(session).unwrap();
    let next_seq = rows.iter().map(|row| row.seq + 1).max().unwrap_or(1);
    for row in rows {
        journal.append_blocking(row).unwrap();
    }

    let runtime = Arc::new(SessionRuntime::with_journal(
        session_id.clone(),
        Some(Arc::clone(&journal)),
    ));
    if let Some(agent_kind) = agent_kind {
        runtime.set_agent_kind(agent_kind);
    }
    if let Some(peer_session_id) = peer_session_id {
        runtime.restore_peer_session_id(peer_session_id);
    }
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
        &session_id,
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    (journal, runtime, conn)
}

/// One journalled row behind an ACP session row, attached live.
pub(super) fn live_agent_replay_fixture(
    session_id: &str,
    record: EventRecord,
) -> (
    std::path::PathBuf,
    Arc<Journal>,
    Arc<SessionRuntime>,
    Arc<ConnHandle>,
) {
    let dir = crate::test_dirs::test_temp_dir("devboule-live-agent-replay-parse");
    let session = new_session_record(session_id, "S-1-5-21-1", None, SessionKind::Acp, "Agent");
    let (journal, runtime, conn) = attach_live_agent_replay(&dir, session, None, vec![record]);
    (dir, journal, runtime, conn)
}

pub(super) fn recovered_integrity() -> TranscriptIntegrity {
    TranscriptIntegrity::Unverifiable {
        dropped_frames: 0,
        dropped_bytes: 0,
        trimmed_bytes: 0,
    }
}
