//! What a journalled row becomes on replay: its event, or a degraded notice.

use super::super::*;
use super::*;

use serde_json::json;

use super::test_support::{attach_tracked, drain, live_agent_replay_fixture};

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
fn an_available_command_list_replays_as_an_agent_report() {
    let session_id = "s.live.agent.replay.available-commands";
    let event = SessionEvent::AvailableCommands {
        commands: vec![devboule_protocol::AvailableCommandView {
            name: "compact".to_string(),
            description: "Summarize context".to_string(),
            hint: None,
        }],
    };
    let record = crate::journal::EventRecord {
        session_id: session_id.to_string(),
        generation: 1,
        seq: 1,
        kind: crate::journal::EventKind::AgentReport,
        ts_ms: 0,
        payload: serde_json::to_vec(&event).expect("the command event serializes"),
    };
    let (_dir, _journal, _runtime, conn) = live_agent_replay_fixture(session_id, record);
    assert!(drain(&conn).iter().any(|event| matches!(
        event,
        SessionEvent::AvailableCommands { commands }
            if commands.iter().any(|command| command.name == "compact")
    )));
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
fn live_agent_replay_marks_an_empty_journal_prefix_degraded() {
    let dir = crate::test_dirs::test_temp_dir("devboule-live-agent-replay-empty");
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
