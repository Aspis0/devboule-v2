//! The transcript cursor a reattach keeps and the history it is owed.

use super::super::*;
use super::*;

use super::test_support::{drain, recovered_integrity};

/// The daemon's transcript cursor means "how far inside the current
/// generation this reader has got". History must not move it: a reader
/// that accounts a history row as current-generation progress starves
/// its own reattach of every current-generation row below that number.
#[test]
fn history_does_not_move_the_transcript_cursor_or_starve_the_reattach() {
    let integrity = recovered_integrity();
    let replay = crate::journal::Replay {
        generation: 2,
        last_seq: 2,
        integrity,
        event_seqs: vec![(1, 100), (2, 1), (2, 2), (2, 2)],
        events: vec![
            SessionEvent::AgentReported {
                seq: 100,
                source: "devboule:stub".to_string(),
                agent: "stub".to_string(),
                state: devboule_protocol::AgentActivityState::Working,
                message: None,
                report_seq: Some(1),
                agent_session_id: None,
                agent_session_path: None,
                session_start_source: None,
            },
            SessionEvent::AgentUserMessage {
                message_id: Some("m1".into()),
                text: "gen-2 user".into(),
                author: devboule_protocol::UserMessageAuthor::Human,
                message_kind: devboule_protocol::UserMessageKind::Unknown,
                at_ms: None,
            },
            SessionEvent::AgentMessage {
                message_id: Some("m2".into()),
                text: "still here".into(),
                parent_tool_use_id: None,
                spawn_depth: None,
            },
            SessionEvent::Recovered { integrity },
        ],
    };
    let runtime = SessionRuntime::from_replay("s.history.cursor".to_string(), None, replay);
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, false)
        .expect("attach");
    conn.track_with_agent_replay(
        "s.history.cursor",
        Arc::clone(&runtime),
        true,
        Some(0),
        outcome.generation,
        outcome.live_agent_replay,
    );
    // A reader that receives only the first envelope — the history row —
    // and then drops. Whatever cursor it accounts from that one row is
    // what its reattach will present.
    let first = conn.pull_events().remove(0);
    conn.event_sent(&first);
    let accounted = conn
        .attached
        .lock()
        .expect("attached")
        .get(&conn.id)
        .and_then(|pull| pull.transcript_cursor);
    runtime.detach_if_conn(conn.id);

    let conn2 = ConnHandle::new(2);
    let outcome = runtime
        .try_attach_with_replay(None, &conn2, false)
        .expect("reattach");
    conn2.track_with_agent_replay(
        "s.history.cursor",
        Arc::clone(&runtime),
        true,
        accounted,
        outcome.generation,
        outcome.live_agent_replay,
    );
    let events = drain(&conn2);
    let transcript: Vec<String> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::AgentUserMessage { text, .. } => Some(text.clone()),
            SessionEvent::AgentMessage { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        transcript,
        vec!["gen-2 user", "still here"],
        "the reattach must still be served the current generation: {transcript:?}"
    );
    // The mechanism, checked after the loss it causes: the accounted
    // cursor stayed at the attach position (0) — history moved it to
    // 100 when the event_sent gate is broken.
    assert_eq!(
        accounted,
        Some(0),
        "history must not advance the transcript cursor"
    );
}

/// One live agent attachment — no screen, no journal — with a single
/// published chat row at `seq`. The queue item is the only place that
/// position exists: no journal row was written under it.
fn live_agent_with_one_chat_row(
    session_id: &str,
    seq: u64,
) -> (Arc<SessionRuntime>, Arc<ConnHandle>) {
    let runtime = Arc::new(SessionRuntime::with_journal(session_id.to_string(), None));
    {
        let mut stream = runtime.stream.lock().unwrap();
        stream.screen = None;
        stream.transcript = false;
        stream.next_seq = seq.saturating_add(1);
        stream.last_applied_seq = seq;
    }
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, false)
        .expect("attach live agent");
    conn.track_with_agent_replay(
        session_id,
        Arc::clone(&runtime),
        false,
        Some(0),
        outcome.generation,
        outcome.live_agent_replay,
    );
    runtime.publish_agent_event_with_seq(
        SessionEvent::AgentMessage {
            message_id: Some("live".to_string()),
            text: "live row".to_string(),
            parent_tool_use_id: None,
            spawn_depth: None,
        },
        None,
        Some(seq),
    );
    (runtime, conn)
}

fn delivered_chat_row(conn: &ConnHandle) -> PendingEvent {
    conn.pull_events()
        .into_iter()
        .find(|pending| matches!(pending.envelope.event, SessionEvent::AgentMessage { .. }))
        .expect("the live chat row reaches the wire")
}

/// A live row has no journal row behind it yet, so the position the queue
/// carries is the only one its envelope can hold. Without it the row
/// reaches the client positionless, indistinguishable from a marker.
#[test]
fn a_live_agent_row_carries_its_position_on_the_envelope() {
    let (runtime, conn) = live_agent_with_one_chat_row("s.live.position", 7);
    let pending = delivered_chat_row(&conn);
    let wire = serde_json::to_value(&pending.envelope).expect("envelope json");
    assert_eq!(
        wire["transcriptSeq"].as_u64(),
        Some(7),
        "a live row must carry the position its queue handed it: {wire}"
    );
    drop(runtime);
}

/// The transcript cursor is what a reattach presents. A delivered live
/// chat row is progress this reader must keep, or every reconnect replays
/// the conversation from the attach position.
#[test]
fn a_live_chat_row_moves_the_transcript_cursor() {
    let (runtime, conn) = live_agent_with_one_chat_row("s.live.cursor", 7);
    let pending = delivered_chat_row(&conn);
    conn.event_sent(&pending);
    let cursor = conn
        .attached
        .lock()
        .expect("attached")
        .get(&conn.id)
        .and_then(|pull| pull.transcript_cursor);
    assert_eq!(
        cursor,
        Some(7),
        "a delivered live row is progress this reader must keep: {cursor:?}"
    );
    drop(runtime);
}

/// A reattach to a multi-generation transcript with a non-zero cursor is
/// owed the whole history plus the current generation after the cursor.
/// Old-generation rows sit at seqs below the cursor here, which is the
/// case a bare-seq filter silently drops — for both row families: the
/// agent-report map and the in-memory Output chunks.
#[test]
fn reattach_to_history_serves_the_whole_conversation() {
    let integrity = recovered_integrity();
    let replay = crate::journal::Replay {
        generation: 2,
        last_seq: 7,
        integrity,
        event_seqs: vec![(1, 2), (1, 3), (2, 6), (2, 7), (2, 7)],
        events: vec![
            SessionEvent::Output {
                seq: 2,
                data: "gen-1 output".to_string(),
            },
            SessionEvent::AgentUserMessage {
                message_id: Some("m1".into()),
                text: "gen-1 report".into(),
                author: devboule_protocol::UserMessageAuthor::Human,
                message_kind: devboule_protocol::UserMessageKind::Unknown,
                at_ms: None,
            },
            SessionEvent::Output {
                seq: 6,
                data: "gen-2 output".to_string(),
            },
            SessionEvent::AgentMessage {
                message_id: Some("m2".into()),
                text: "still here".into(),
                parent_tool_use_id: None,
                spawn_depth: None,
            },
            SessionEvent::Recovered { integrity },
        ],
    };
    let runtime = SessionRuntime::from_replay("s.reattach.history".to_string(), None, replay);
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, false)
        .expect("attach");
    conn.track_with_agent_replay(
        "s.reattach.history",
        Arc::clone(&runtime),
        true,
        Some(5),
        outcome.generation,
        outcome.live_agent_replay,
    );
    let events = drain(&conn);
    let transcript: Vec<String> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Output { data, .. } => Some(data.clone()),
            SessionEvent::AgentUserMessage { text, .. } => Some(text.clone()),
            SessionEvent::AgentMessage { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        transcript,
        vec!["gen-1 output", "gen-1 report", "gen-2 output", "still here",],
        "history below the cursor is still owed on a reattach: {transcript:?}"
    );
}
