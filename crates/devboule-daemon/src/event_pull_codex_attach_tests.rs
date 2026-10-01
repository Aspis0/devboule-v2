//! The Codex attach replay: the plan-mark scan and the turn it suppresses.

use super::super::*;
use super::*;

use serde_json::json;

use super::test_support::{attach_live_agent_replay, drain};

/// A live Codex runtime on a journal holding `rows`, attached and tracked
/// on a fresh connection.
fn attach_codex_replay(
    session_id: &str,
    dir: &std::path::Path,
    rows: Vec<crate::journal::EventRecord>,
) -> (Arc<SessionRuntime>, Arc<Journal>, Arc<ConnHandle>) {
    let mut record =
        new_session_record(session_id, "S-1-5-21-1", None, SessionKind::Codex, "Codex");
    record.peer_session_id = Some("t-1".to_string());
    let (journal, runtime, conn) =
        attach_live_agent_replay(dir, record, Some(SessionKind::Codex), rows);
    (runtime, journal, conn)
}

fn codex_plan_rows(session_id: &str) -> Vec<crate::journal::EventRecord> {
    let started = json!({"method": "turn/started", "params": {
        "threadId": "t-1", "turn": {"id": "turn-T"}}});
    let update = json!({"method": "turn/plan/updated", "params": {
        "threadId": "t-1",
        "plan": [{"step": "Planned", "status": "pending"}]}});
    vec![
        crate::journal::acp_envelope_record(session_id, 1, 1, &started).unwrap(),
        crate::journal::acp_envelope_record(session_id, 1, 2, &update).unwrap(),
    ]
}

#[test]
fn attach_replay_suppresses_codex_tasks_for_a_card_marked_turn() {
    // The verdict-at-seq-3 shape from journal_replay, driven through
    // pull_live_agent_replay_events instead: turn/started, turn/plan/updated,
    // then the verdict row AFTER the frames. The walk must still suppress —
    // the mode cannot be learned from rows walked so far.
    let session_id = "s.codex.attach.card";
    let dir = crate::test_dirs::test_temp_dir("devboule-codex-attach-card");
    let verdict = SessionEvent::AgentToolUpdate {
        tool_call_id: "turn-T-plan".to_string(),
        status: Some("completed".to_string()),
        text: None,
        title: Some("Approved".to_string()),
        kind: Some("plan".to_string()),
        locations: None,
        parent_tool_use_id: None,
        spawn_depth: None,
        command: None,
        exit_code: None,
    };
    let mut rows = codex_plan_rows(session_id);
    rows.push(crate::journal::agent_report_record(session_id, 1, 3, &verdict).unwrap());
    let (runtime, journal, conn) = attach_codex_replay(session_id, &dir, rows);
    let events = drain(&conn);
    assert!(
        events
            .iter()
            .all(|event| !matches!(event, SessionEvent::AgentTasks { .. })),
        "a card-marked turn replays suppressed on attach: {events:?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            SessionEvent::AgentToolUpdate { title: Some(title), .. } if title == "Approved"
        )),
        "the walk itself ran to the verdict row: {events:?}"
    );
    drop(conn);
    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn plan_mark_scan_runs_with_the_attachments_lock_released() {
    // The scan reads the whole session's reports; holding `attached` across
    // it would stall every pull on the connection for its duration.
    let session_id = "s.codex.attach.unlocked";
    let dir = crate::test_dirs::test_temp_dir("devboule-codex-attach-unlocked");
    let (runtime, journal, conn) =
        attach_codex_replay(session_id, &dir, codex_plan_rows(session_id));
    let lock_free_at_scan = Arc::new(Mutex::new(Vec::new()));
    {
        let conn = Arc::downgrade(&conn);
        let seen = Arc::clone(&lock_free_at_scan);
        *runtime.plan_mark_scan_probe.lock().unwrap() = Some(Box::new(move || {
            let free = conn
                .upgrade()
                .is_some_and(|conn| conn.attached.try_lock().is_ok());
            seen.lock().unwrap().push(free);
        }));
    }
    let events = drain(&conn);
    assert_eq!(*lock_free_at_scan.lock().unwrap(), [true]);
    assert_eq!(runtime.plan_mark_scan_count(), 1);
    assert!(
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::AgentTasks { .. })),
        "an unmarked turn replays its checklist once the marks load: {events:?}"
    );
    *runtime.plan_mark_scan_probe.lock().unwrap() = None;
    drop(conn);
    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn attach_without_codex_rows_does_no_mark_scan() {
    // A Codex attach with nothing to replay walks no envelopes, so the
    // plan-mark pre-scan never runs: no journal read for the marks.
    let session_id = "s.codex.attach.quiet";
    let dir = crate::test_dirs::test_temp_dir("devboule-codex-attach-quiet");
    let (runtime, journal, conn) = attach_codex_replay(session_id, &dir, Vec::new());
    let events = drain(&conn);
    assert!(events
        .iter()
        .all(|event| !matches!(event, SessionEvent::AgentTasks { .. })));
    assert_eq!(runtime.plan_mark_scan_count(), 0);
    drop(runtime);
    drop(journal);
    let _ = std::fs::remove_dir_all(&dir);
}
