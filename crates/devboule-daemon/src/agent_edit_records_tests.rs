//! One session's replay into edit records, and the workspace read over a real
//! journal: a call is one record per file change, a call recorded again keeps
//! only its last edits, and a row written before edits existed reads as no edit.

use devboule_protocol::{AgentEditKind, AgentEditText, AgentToolEdit, SessionEvent, SessionKind};
use serde_json::json;

use super::{
    read, session_records, AgentEditRecord, SessionScope, MAX_READ_BYTES, MAX_READ_SESSIONS,
};
use crate::journal::{
    new_session_record, now_ms, tmp_journal, EventKind, EventRecord, Journal, WorkspaceRecord,
};
use devboule_protocol::WorkspaceIsolation;

fn root() -> &'static std::path::Path {
    std::path::Path::new(crate::test_support::FIXTURE_ROOT)
}

fn scoped(events: &[SessionEvent], times: &[Option<u64>]) -> Vec<AgentEditRecord> {
    let scope = SessionScope {
        session_id: "s.a",
        agent: "agent",
        fallback_ms: 900,
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
    session_in(journal, id, workspace_id, title, closed, None);
}

/// A session row whose provider ran in `cwd`, in a workspace rooted at the
/// fixture root.
fn session_in(
    journal: &Journal,
    id: &str,
    workspace_id: &str,
    title: &str,
    closed: bool,
    cwd: Option<&std::path::Path>,
) {
    if journal
        .workspace_get(workspace_id)
        .expect("workspace read")
        .is_none()
    {
        ensure_project(journal, "p.1");
        journal
            .workspace_create(WorkspaceRecord {
                id: workspace_id.to_string(),
                project_id: "p.1".to_string(),
                title: "Workspace".to_string(),
                isolation: WorkspaceIsolation::Local,
                path: root().to_string_lossy().into_owned(),
                branch: None,
                created_at_ms: now_ms(),
                updated_at_ms: now_ms(),
            })
            .expect("workspace row");
    }
    let mut record = new_session_record(
        id,
        "S-1-5-21-1",
        Some(workspace_id.to_string()),
        SessionKind::Claude,
        title,
    );
    record.closed = closed;
    record.cwd = cwd.map(|cwd| cwd.to_string_lossy().into_owned());
    journal.upsert_blocking(record).expect("session row");
}

fn ensure_project(journal: &Journal, project_id: &str) {
    if journal
        .project_get(project_id)
        .expect("project read")
        .is_none()
    {
        journal
            .project_add(crate::journal::ProjectRecord {
                id: project_id.to_string(),
                name: "Project".to_string(),
                path: crate::test_support::FIXTURE_ROOT.to_string(),
                git_state: "not_repository".to_string(),
                created_at_ms: now_ms(),
                updated_at_ms: now_ms(),
            })
            .expect("project row");
    }
}

fn call_json(tool_call_id: &str, edits: Vec<AgentToolEdit>) -> serde_json::Value {
    serde_json::to_value(call(tool_call_id, Some(edits))).expect("event")
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

    let found = read(&journal, "ws.1", now_ms());
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

    let found = read(&journal, "ws.1", now_ms());
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
fn an_edit_outside_the_workspace_root_is_not_listed() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    session(&journal, "s.a.1", "ws.1", "Ada", false);
    let outside = root()
        .parent()
        .expect("the fixture root has a parent")
        .join("other.txt");
    append_event(
        &journal,
        "s.a.1",
        1,
        call_json("escape", vec![edit("../secret", 1)]),
    );
    append_event(
        &journal,
        "s.a.1",
        2,
        call_json("absolute", vec![edit(&outside.to_string_lossy(), 1)]),
    );
    append_event(
        &journal,
        "s.a.1",
        3,
        call_json("inside", vec![edit("src/lib.rs", 1)]),
    );

    let found = read(&journal, "ws.1", now_ms());
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);

    let paths: Vec<&str> = found
        .records
        .iter()
        .map(|record| record.path.as_str())
        .collect();
    assert_eq!(paths, ["src/lib.rs"]);
}

#[test]
fn an_edit_in_a_nested_child_cwd_is_root_relative() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    let child = crate::test_support::fixture_path("sub");
    session_in(
        &journal,
        "s.child.1",
        "ws.1",
        "Worker",
        false,
        Some(child.as_path()),
    );
    append_event(
        &journal,
        "s.child.1",
        1,
        call_json("own", vec![edit("a.rs", 1)]),
    );
    append_event(
        &journal,
        "s.child.1",
        2,
        call_json(
            "parent",
            vec![edit(
                &crate::test_support::fixture_path("x.rs").to_string_lossy(),
                1,
            )],
        ),
    );

    let found = read(&journal, "ws.1", now_ms());
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);

    let paths: Vec<&str> = found
        .records
        .iter()
        .map(|record| record.path.as_str())
        .collect();
    assert_eq!(paths, ["sub/a.rs", "x.rs"]);
}

#[test]
fn a_workspace_past_the_session_cap_is_reported_truncated() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    for index in 0..=MAX_READ_SESSIONS {
        session(&journal, &format!("s.{index:03}"), "ws.1", "Agent", false);
    }

    let found = read(&journal, "ws.1", now_ms());
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

    let found = read(&journal, "ws.1", now_ms());
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

#[test]
fn a_replayed_session_carries_no_edit_outside_the_workspace() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    session(&journal, "s.a.1", "ws.1", "Ada", false);
    let outside = root()
        .parent()
        .expect("the fixture root has a parent")
        .join("other.txt");
    append_event(
        &journal,
        "s.a.1",
        1,
        call_json("escape", vec![edit("../secret", 1)]),
    );
    append_event(
        &journal,
        "s.a.1",
        2,
        call_json(
            "mixed",
            vec![edit("src/lib.rs", 1), edit(&outside.to_string_lossy(), 1)],
        ),
    );

    let reader = journal.open_reader().expect("reader");
    let replay =
        crate::journal::replay_session_including_closed_on(&reader, "s.a.1").expect("replay");
    drop(reader);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);

    let carried: Vec<Vec<String>> = replay
        .events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::AgentToolCall {
                edits: Some(edits), ..
            } => Some(edits.iter().map(|edit| edit.path.clone()).collect()),
            _ => None,
        })
        .collect();
    assert_eq!(carried, vec![vec!["src/lib.rs".to_string()]]);
}

/// A Claude `Write` call as the stream carries it: the new text, which is all
/// the call's input holds.
fn claude_write_call(tool_call_id: &str, path: &str, content: &str) -> serde_json::Value {
    json!({
        "type": "assistant",
        "message": {
            "model": "claude-opus-5",
            "id": "msg_1",
            "role": "assistant",
            "content": [{
                "type": "tool_use",
                "id": tool_call_id,
                "name": "Write",
                "input": {"file_path": path, "content": content},
            }],
        }
    })
}

/// The result envelope of a `Write`, its structured result under `key`: the
/// transcript spells it `toolUseResult`, the live stream `tool_use_result`.
fn claude_write_result(
    tool_call_id: &str,
    key: &str,
    result: serde_json::Value,
) -> serde_json::Value {
    let mut frame = json!({
        "type": "user",
        "message": {"content": [{"type": "tool_result", "tool_use_id": tool_call_id}]}
    });
    frame[key] = result;
    frame
}

fn append_envelope(journal: &Journal, session_id: &str, seq: u64, envelope: &serde_json::Value) {
    journal
        .append_blocking(
            crate::journal::acp_envelope_record(session_id.to_string(), 1, seq, envelope)
                .expect("row"),
        )
        .expect("append");
}

#[test]
fn a_claude_overwrite_records_the_text_its_result_replaced() {
    for key in ["toolUseResult", "tool_use_result"] {
        let (dir, path) = tmp_journal();
        let journal = Journal::open(&path).expect("open");
        session(&journal, "s.a.1", "ws.1", "Ada", false);
        let file = root().join("notes2.txt").to_string_lossy().into_owned();
        append_envelope(&journal, "s.a.1", 1, &claude_write_call("c1", &file, "one"));
        append_envelope(
            &journal,
            "s.a.1",
            2,
            &claude_write_result(
                "c1",
                key,
                json!({"type": "create", "filePath": file, "content": "one",
                    "structuredPatch": [], "originalFile": null}),
            ),
        );
        append_envelope(
            &journal,
            "s.a.1",
            3,
            &claude_write_call("c2", &file, "one two"),
        );
        append_envelope(
            &journal,
            "s.a.1",
            4,
            &claude_write_result(
                "c2",
                key,
                json!({"type": "update", "filePath": file, "content": "one two",
                    "structuredPatch": [{"oldStart": 1, "oldLines": 1, "newStart": 1,
                        "newLines": 1, "lines": ["-one", "\\ No newline at end of file",
                        "+one two", "\\ No newline at end of file"]}],
                    "originalFile": "one"}),
            ),
        );

        let found = read(&journal, "ws.1", now_ms());
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);

        assert!(found.error.is_none(), "{key}");
        let overwrite = found
            .records
            .iter()
            .find(|record| record.tool_call_id == "c2")
            .expect("the overwrite is listed");
        assert_eq!(overwrite.path, "notes2.txt", "{key}");
        assert_eq!(
            overwrite.text,
            Some(AgentEditText::Replace {
                old: "one".to_string(),
                new: "one two".to_string(),
            }),
            "{key}"
        );
        assert_eq!((overwrite.added, overwrite.removed), (1, 1), "{key}");
    }
}

#[test]
fn a_claude_write_outside_the_workspace_root_is_listed_nowhere() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    session(&journal, "s.a.1", "ws.1", "Ada", false);
    let outside = root()
        .parent()
        .expect("the fixture root has a parent")
        .join("other.txt")
        .to_string_lossy()
        .into_owned();
    append_envelope(
        &journal,
        "s.a.1",
        1,
        &claude_write_call("c1", &outside, "one two"),
    );
    append_envelope(
        &journal,
        "s.a.1",
        2,
        &claude_write_result(
            "c1",
            "toolUseResult",
            json!({"type": "update", "filePath": outside, "content": "one two",
                "originalFile": "one"}),
        ),
    );

    let found = read(&journal, "ws.1", now_ms());
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);

    assert!(found.records.is_empty());
}
