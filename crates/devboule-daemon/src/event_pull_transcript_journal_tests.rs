//! What the journal still owes a reattach, and the stop tail that owes
//! nothing.

use super::super::*;
use super::*;

use super::test_support::{drain, recovered_integrity};

/// The journal-copies branch of the transcript replay must serve history
/// too: a runtime whose in-memory scrollback begins above the cursor —
/// hydrated with a catch-up cursor — still owes the older generations'
/// Output rows the journal holds, whatever seq they sit at.
#[test]
fn journal_copies_of_history_survive_the_reattach() {
    let dir = crate::test_dirs::test_temp_dir("devboule-journal-history");
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).unwrap());
    let session_id = "s.journal.history";
    journal
        .upsert_blocking(new_session_record(
            session_id,
            "S-1-5-21-1",
            None,
            SessionKind::Acp,
            "Agent",
        ))
        .unwrap();
    // A history Output row the hydrated scrollback does not hold: the
    // runtime below is built from a hand-built replay that starts at
    // the current generation, exactly like a hydrate that arrived with
    // a catch-up cursor.
    journal
        .append_blocking(crate::journal::output_record(
            session_id,
            1,
            2,
            "gen-1 journal row".as_bytes(),
        ))
        .expect("gen-1 journal row");
    journal.start_generation(session_id, 2).unwrap();
    journal
        .append_blocking(crate::journal::output_record(
            session_id,
            2,
            8,
            "journal hole row".as_bytes(),
        ))
        .expect("journal hole row");
    let integrity = recovered_integrity();
    let replay = crate::journal::Replay {
        generation: 2,
        last_seq: 7,
        integrity,
        event_seqs: vec![(2, 7), (2, 7)],
        event_ts_ms: vec![None; 2],
        events: vec![
            SessionEvent::Output {
                seq: 7,
                data: "gen-2 chunk".to_string(),
            },
            SessionEvent::Recovered { integrity },
        ],
    };
    let runtime =
        SessionRuntime::from_replay(session_id.to_string(), Some(Arc::clone(&journal)), replay);
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, false)
        .expect("attach");
    conn.track_with_agent_replay(
        session_id,
        Arc::clone(&runtime),
        true,
        Some(5),
        outcome.generation,
        outcome.live_agent_replay,
    );
    let events = drain(&conn);
    let ledger: Vec<String> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Output { data, .. } => Some(data.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        ledger,
        vec!["gen-1 journal row", "gen-2 chunk", "journal hole row",],
        "journal copies of history are owed whatever their seq: {ledger:?}"
    );

    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The stop-tail attach carries the nothing-owed sentinel as its seq: a
/// stop wants no rows, not "everything below the attach generation".
/// History is owed to readers, which is exactly why the sentinel must
/// mean nothing rather than merely a very large seq.
#[test]
fn a_stop_tail_cursor_owes_nothing_not_even_history() {
    let integrity = recovered_integrity();
    let replay = crate::journal::Replay {
        generation: 2,
        last_seq: 1,
        integrity,
        event_seqs: vec![(1, 1), (1, 2), (2, 1), (2, 1)],
        event_ts_ms: vec![None; 4],
        events: vec![
            SessionEvent::Output {
                seq: 1,
                data: "gen-1 ledger".to_string(),
            },
            SessionEvent::AgentUserMessage {
                message_id: Some("m1".into()),
                text: "gen-1 report".into(),
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
    let runtime = SessionRuntime::from_replay("s.stop.tail".to_string(), None, replay);

    // The store is not degenerate: a fresh reader is served the history.
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, false)
        .expect("attach");
    conn.track_with_agent_replay(
        "s.stop.tail",
        Arc::clone(&runtime),
        true,
        Some(0),
        outcome.generation,
        outcome.live_agent_replay,
    );
    let seen = drain(&conn);
    assert!(!seen.is_empty(), "the fixture must hold rows");

    // The stop-tail reader: the cursor the client's stop sends.
    let conn2 = ConnHandle::new(2);
    let outcome = runtime
        .try_attach_with_replay(
            Some(Cursor {
                generation: 2,
                seq: u64::MAX,
            }),
            &conn2,
            false,
        )
        .expect("stop-tail attach");
    conn2.track_with_agent_replay(
        "s.stop.tail",
        Arc::clone(&runtime),
        true,
        Some(u64::MAX),
        outcome.generation,
        outcome.live_agent_replay,
    );
    let events = drain(&conn2);
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
        Vec::<String>::new(),
        "a stop-tail cursor must owe nothing, not the history: {transcript:?}"
    );
}
