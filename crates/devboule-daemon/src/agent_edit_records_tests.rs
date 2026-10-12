//! One session's replay into edit records, and the workspace read over a real
//! journal: a call is one record per file change, a call recorded again keeps
//! only its last edits, and a row written before edits existed reads as no edit.

use devboule_protocol::{AgentEditKind, AgentEditText, AgentToolEdit, SessionEvent, SessionKind};
use serde_json::json;

use super::{
    read, session_records, AgentEditRecord, SessionScope, MAX_READ_BYTES, MAX_READ_SESSIONS,
};
use crate::journal::{new_session_record, now_ms, tmp_journal, EventKind, EventRecord, Journal};

fn root() -> &'static std::path::Path {
    std::path::Path::new(crate::test_support::FIXTURE_ROOT)
}

fn scoped(events: &[SessionEvent], times: &[Option<u64>]) -> Vec<AgentEditRecord> {
    let scope = SessionScope {
        session_id: "s.a",
        agent: "agent",
        fallback_ms: 900,
        cwd: None,
        root: root(),
    };
    session_records(&scope, events, times)
}

fn edit(path: &str, added: u64) -> AgentToolEdit {
    AgentToolEdit {
        path: path.to_string(),
        kind: AgentEditKind::Edit,
        text: None,
        truncated: false,
        added,
        removed: 0,
    }
}

fn call(tool_call_id: &str, edits: Option<Vec<AgentToolEdit>>) -> SessionEvent {
    SessionEvent::AgentToolCall {
        tool_call_id: tool_call_id.to_string(),
        title: "Edit".to_string(),
        status: "completed".to_string(),
        kind: Some("edit".to_string()),
        locations: None,
        edits,
        subagent_type: None,
        parent_tool_use_id: None,
        spawn_depth: None,
        command: None,
        exit_code: None,
        background: None,
    }
}

#[test]
fn a_call_recorded_twice_is_one_record_with_its_last_edits() {
    let events = vec![
        call("c1", Some(vec![edit("src/a.rs", 1)])),
        call("c1", Some(vec![edit("src/a.rs", 3)])),
    ];
    let records = scoped(&events, &[Some(100), Some(200)]);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].added, 3);
    assert_eq!(records[0].at_ms, 200);
}

#[test]
fn each_file_change_of_each_call_is_its_own_record() {
    let events = vec![
        call("c1", Some(vec![edit("src/a.rs", 1), edit("src/b.rs", 2)])),
        call("c2", Some(vec![edit("src/a.rs", 4)])),
    ];
    let records = scoped(&events, &[Some(100), Some(200)]);
    let paths: Vec<&str> = records.iter().map(|record| record.path.as_str()).collect();
    assert_eq!(paths, ["src/a.rs", "src/b.rs", "src/a.rs"]);
    assert!(records.iter().all(|record| record.session_id == "s.a"));
}

#[test]
fn an_event_without_a_time_takes_the_fallback_time() {
    let events = vec![call("c1", Some(vec![edit("src/a.rs", 1)]))];
    let records = scoped(&events, &[None]);
    assert_eq!(records[0].at_ms, 900);
}

#[test]
fn a_call_without_edits_contributes_nothing() {
    let events = vec![call("c1", None)];
    assert!(scoped(&events, &[Some(100)]).is_empty());
}

fn session(journal: &Journal, id: &str, workspace_id: &str, title: &str, closed: bool) {
    let mut record = new_session_record(
        id,
        "S-1-5-21-1",
        Some(workspace_id.to_string()),
        SessionKind::Claude,
        title,
    );
    record.closed = closed;
    journal.upsert_blocking(record).expect("session row");
}

fn append_event(journal: &Journal, session_id: &str, seq: u64, event: serde_json::Value) {
    journal
        .append_blocking(EventRecord {
            session_id: session_id.to_string(),
            generation: 1,
            seq,
            kind: EventKind::AgentReport,
            ts_ms: now_ms(),
            payload: serde_json::to_vec(&event).expect("payload"),
        })
        .expect("append");
}

fn replace_call(tool_call_id: &str, old: &str, new: &str) -> serde_json::Value {
    let edit = AgentToolEdit {
        path: "src/lib.rs".to_string(),
        kind: AgentEditKind::Edit,
        text: Some(AgentEditText::Replace {
            old: old.to_string(),
            new: new.to_string(),
        }),
        truncated: false,
        added: 1,
        removed: 1,
    };
    serde_json::to_value(call(tool_call_id, Some(vec![edit]))).expect("event")
}

#[test]
fn the_workspace_read_lists_each_sessions_edits_and_skips_a_row_without_them() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    session(&journal, "s.a.1", "ws.1", "Ada", false);
    session(&journal, "s.b.1", "ws.1", "Bob", false);
    session(&journal, "s.other.1", "ws.2", "Other", false);
    append_event(&journal, "s.a.1", 1, replace_call("c1", "one\n", "two\n"));
    append_event(&journal, "s.b.1", 1, replace_call("c2", "two\n", "three\n"));
    // A row written before edits existed: no `edits` key, so no record.
    append_event(
        &journal,
        "s.a.1",
        2,
        json!({"type": "agent_tool_call", "toolCallId": "c3", "title": "Read", "status": "completed"}),
    );
    append_event(&journal, "s.other.1", 1, replace_call("c4", "x\n", "y\n"));

    let found = read(&journal, "ws.1", root(), now_ms());
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);

    assert!(found.error.is_none());
    assert!(!found.truncated);
    let calls: Vec<&str> = found
        .records
        .iter()
        .map(|record| record.tool_call_id.as_str())
        .collect();
    assert_eq!(calls, ["c1", "c2"]);
    let agents: Vec<&str> = found
        .records
        .iter()
        .map(|record| record.agent.as_str())
        .collect();
    assert_eq!(agents, ["Ada", "Bob"]);
}

#[test]
fn a_closed_child_still_contributes_its_edits() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    session(&journal, "s.child.1", "ws.1", "Worker", true);
    append_event(
        &journal,
        "s.child.1",
        1,
        replace_call("c1", "one\n", "two\n"),
    );

    let found = read(&journal, "ws.1", root(), now_ms());
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);

    assert_eq!(found.records.len(), 1);
    assert_eq!(found.records[0].agent, "Worker");
}

fn status_update(tool_call_id: &str, status: &str) -> SessionEvent {
    SessionEvent::AgentToolUpdate {
        edits: None,
        tool_call_id: tool_call_id.to_string(),
        status: Some(status.to_string()),
        text: None,
        title: None,
        kind: None,
        locations: None,
        parent_tool_use_id: None,
        spawn_depth: None,
        command: None,
        exit_code: None,
        replace: false,
        images: Vec::new(),
    }
}

fn pending_call(tool_call_id: &str, edits: Vec<AgentToolEdit>) -> SessionEvent {
    let mut event = call(tool_call_id, Some(edits));
    if let SessionEvent::AgentToolCall { status, .. } = &mut event {
        *status = "pending".to_string();
    }
    event
}

#[test]
fn only_a_call_that_finished_completed_is_listed() {
    let events = vec![
        pending_call("denied", vec![edit("src/a.rs", 1)]),
        status_update("denied", "failed"),
        pending_call("declined", vec![edit("src/b.rs", 1)]),
        status_update("declined", "declined"),
        pending_call("unfinished", vec![edit("src/c.rs", 1)]),
        pending_call("finished", vec![edit("src/d.rs", 1)]),
        status_update("finished", "completed"),
    ];
    let times = vec![Some(100); events.len()];
    let records = scoped(&events, &times);
    let calls: Vec<&str> = records
        .iter()
        .map(|record| record.tool_call_id.as_str())
        .collect();
    assert_eq!(calls, ["finished"]);
}

#[test]
fn a_path_outside_the_workspace_root_is_not_listed() {
    let outside = root()
        .parent()
        .expect("the fixture root has a parent")
        .join("other.txt");
    let events = vec![
        call("escape", Some(vec![edit("../secret", 1)])),
        call("absolute", Some(vec![edit(&outside.to_string_lossy(), 1)])),
        call("inside", Some(vec![edit("src/lib.rs", 1)])),
    ];
    let records = scoped(&events, &[Some(100), Some(100), Some(100)]);
    let paths: Vec<&str> = records.iter().map(|record| record.path.as_str()).collect();
    assert_eq!(paths, ["src/lib.rs"]);
}

#[test]
fn an_edit_in_a_nested_child_cwd_is_root_relative() {
    let child = crate::test_support::fixture_path("sub");
    let scope = SessionScope {
        session_id: "s.a",
        agent: "agent",
        fallback_ms: 900,
        cwd: Some(&child),
        root: root(),
    };
    let events = vec![call("c1", Some(vec![edit("a.rs", 1)]))];
    let records = session_records(&scope, &events, &[Some(100)]);
    assert_eq!(records[0].path, "sub/a.rs");
}

#[test]
fn a_workspace_past_the_session_cap_is_reported_truncated() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    for index in 0..=MAX_READ_SESSIONS {
        session(&journal, &format!("s.{index:03}"), "ws.1", "Agent", false);
    }

    let found = read(&journal, "ws.1", root(), now_ms());
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);

    assert!(found.truncated);
    assert!(found.records.is_empty());
}

#[test]
fn a_read_skips_sessions_once_the_replay_byte_budget_is_spent() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    let each = MAX_READ_BYTES / 2 - 1;
    for id in ["s.a.1", "s.b.1", "s.c.1"] {
        let mut record = new_session_record(
            id,
            "S-1-5-21-1",
            Some("ws.1".to_string()),
            SessionKind::Claude,
            "Agent",
        );
        record.payload_bytes = each;
        journal.upsert_blocking(record).expect("session row");
    }

    let found = read(&journal, "ws.1", root(), now_ms());
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);

    assert!(found.truncated, "the third session would pass the budget");
}

fn update_with_edits(
    tool_call_id: &str,
    status: Option<&str>,
    edits: Vec<AgentToolEdit>,
) -> SessionEvent {
    SessionEvent::AgentToolUpdate {
        tool_call_id: tool_call_id.to_string(),
        status: status.map(str::to_string),
        text: None,
        title: None,
        kind: None,
        locations: None,
        parent_tool_use_id: None,
        spawn_depth: None,
        command: None,
        exit_code: None,
        replace: false,
        images: Vec::new(),
        edits: Some(edits),
    }
}

#[test]
fn edits_carried_on_an_update_are_listed_once_the_call_completes() {
    let call_without_edits = SessionEvent::AgentToolCall {
        tool_call_id: "pi-1".to_string(),
        title: "edit".to_string(),
        status: "pending".to_string(),
        kind: Some("edit".to_string()),
        locations: None,
        edits: None,
        subagent_type: None,
        parent_tool_use_id: None,
        spawn_depth: None,
        command: None,
        exit_code: None,
        background: None,
    };
    let events = vec![
        call_without_edits,
        update_with_edits("pi-1", Some("in_progress"), vec![edit("src/lib.rs", 2)]),
        status_update("pi-1", "completed"),
    ];
    let records = scoped(&events, &[Some(100), Some(110), Some(120)]);
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].added, 2);
    assert_eq!(records[0].at_ms, 120);
}
