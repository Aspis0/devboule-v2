//! Tests for the plan checklist Claude's task Tools feed: the state
//! machine itself and the replay property the per-session view state
//! exists to guarantee. The view-level tests (sidechain bypass, event
//! order) live with the view.

use devboule_protocol::{AgentTaskItem, AgentTaskStatus, SessionEvent};
use serde_json::{json, Value};

use super::ClaudeTaskState;

// SYNTHETIC: stream-json envelopes shaped after the CLI's task Tool
// traffic (Paseo's task-state.test.ts shapes) — invented ids, invented task
// texts, no measured run behind them.
const CLAUDE_TASKS_SYNTHETIC: &str = include_str!("../fixtures/wire/claude-tasks-synthetic.jsonl");

fn tool_use(id: &str, name: &str, input: Value) -> Value {
    json!({
        "type": "assistant",
        "message": {
            "id": format!("msg_{id}"),
            "role": "assistant",
            "content": [{"type": "tool_use", "id": id, "name": name, "input": input}],
        }
    })
}

fn tool_result(id: &str, result: Value) -> Value {
    json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": [{"type": "tool_result", "tool_use_id": id, "content": "ok"}]
        },
        "tool_use_result": result,
    })
}

fn tasks_of(event: Option<SessionEvent>) -> Vec<AgentTaskItem> {
    match event {
        Some(SessionEvent::AgentTasks { items }) => items,
        other => panic!("expected AgentTasks, got {other:?}"),
    }
}

fn observe(state: &mut ClaudeTaskState, envelope: &Value) -> Vec<AgentTaskItem> {
    tasks_of(state.observe(envelope))
}

/// Feed an envelope that changes nothing: a task `tool_use` only stashes its
/// input, so no snapshot exists until the result arrives.
fn record(state: &mut ClaudeTaskState, envelope: &Value) {
    state.observe(envelope);
}

#[test]
fn todowrite_replaces_the_whole_list_at_tool_use() {
    let mut state = ClaudeTaskState::default();
    let first = observe(
        &mut state,
        &tool_use(
            "legacy",
            "TodoWrite",
            json!({"todos": [
                {"content": "Legacy", "status": "in_progress", "activeForm": "Working"},
                {"subject": "Also legacy", "status": "completed"},
            ]}),
        ),
    );
    assert_eq!(first.len(), 2);
    // Entries without an id are keyed by position, Paseo's `legacy:${index}`.
    assert_eq!(first[0].id.as_deref(), Some("legacy:0"));
    assert_eq!(first[0].text, "Legacy");
    assert_eq!(first[0].status, AgentTaskStatus::InProgress);
    assert_eq!(first[1].id.as_deref(), Some("legacy:1"));
    assert_eq!(first[1].status, AgentTaskStatus::Completed);

    // A second TodoWrite blows the list away rather than merging.
    let second = observe(
        &mut state,
        &tool_use(
            "second",
            "TodoWrite",
            json!({"todos": [{"content": "Only"}]}),
        ),
    );
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].id.as_deref(), Some("legacy:0"));
    assert_eq!(second[0].text, "Only");
    assert_eq!(second[0].status, AgentTaskStatus::Pending);

    // An entry with no text at all is dropped, not kept blank.
    let blank = observe(
        &mut state,
        &tool_use(
            "blank",
            "TodoWrite",
            json!({"todos": [{"content": ""}, {"text": "Kept"}]}),
        ),
    );
    assert_eq!(blank.len(), 1);
    assert_eq!(blank[0].text, "Kept");
}

#[test]
fn taskcreate_then_taskupdate_apply_by_id_at_the_result() {
    let mut state = ClaudeTaskState::default();
    // At tool_use only the input is stashed: no snapshot exists yet.
    let create_call = state.observe(&tool_use(
        "create-1",
        "TaskCreate",
        json!({"subject": "Alpha", "activeForm": "Doing alpha"}),
    ));
    assert!(create_call.is_none());

    let created = observe(
        &mut state,
        &tool_result("create-1", json!({"task": {"id": "1", "subject": "Alpha"}})),
    );
    assert_eq!(created.len(), 1);
    assert_eq!(created[0].id.as_deref(), Some("1"));
    assert_eq!(created[0].text, "Alpha");
    assert_eq!(created[0].status, AgentTaskStatus::Pending);

    let update_call = state.observe(&tool_use(
        "update-1",
        "TaskUpdate",
        json!({"taskId": "1", "status": "in_progress"}),
    ));
    assert!(update_call.is_none());

    let updated = observe(
        &mut state,
        &tool_result("update-1", json!({"success": true, "taskId": "1"})),
    );
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0].id.as_deref(), Some("1"));
    assert_eq!(updated[0].status, AgentTaskStatus::InProgress);

    // A rename that carries no status keeps the item's own.
    record(
        &mut state,
        &tool_use(
            "update-2",
            "TaskUpdate",
            json!({"taskId": "1", "subject": "Renamed"}),
        ),
    );
    let renamed = observe(
        &mut state,
        &tool_result("update-2", json!({"success": true, "taskId": "1"})),
    );
    assert_eq!(renamed[0].text, "Renamed");
    assert_eq!(renamed[0].status, AgentTaskStatus::InProgress);

    // An update for an id the list does not carry changes nothing.
    let unknown = state.observe(&tool_use(
        "update-2",
        "TaskUpdate",
        json!({"taskId": "ghost", "status": "completed"}),
    ));
    assert!(unknown.is_none());
    let unknown_result = state.observe(&tool_result(
        "update-2",
        json!({"success": true, "taskId": "ghost"}),
    ));
    assert!(unknown_result.is_none());
}

#[test]
fn tasklist_replaces_and_keeps_only_entries_with_an_id() {
    let mut state = ClaudeTaskState::default();
    record(
        &mut state,
        &tool_use("seed", "TaskCreate", json!({"subject": "Disposable"})),
    );
    observe(
        &mut state,
        &tool_result(
            "seed",
            json!({"task": {"id": "1", "subject": "Disposable"}}),
        ),
    );
    record(&mut state, &tool_use("list", "TaskList", json!({})));
    let listed = observe(
        &mut state,
        &tool_result(
            "list",
            json!({"tasks": [
                {"id": "7", "subject": "Current", "status": "completed"},
                {"content": "No id on this one", "status": "pending"},
            ]}),
        ),
    );
    assert_eq!(
        listed.len(),
        1,
        "TaskList keeps only entries carrying an id"
    );
    assert_eq!(listed[0].id.as_deref(), Some("7"));
    assert_eq!(listed[0].status, AgentTaskStatus::Completed);
}

#[test]
fn a_deleted_status_removes_the_item() {
    let mut state = ClaudeTaskState::default();
    record(
        &mut state,
        &tool_use("create", "TaskCreate", json!({"subject": "Disposable"})),
    );
    observe(
        &mut state,
        &tool_result(
            "create",
            json!({"task": {"id": "1", "subject": "Disposable"}}),
        ),
    );
    record(
        &mut state,
        &tool_use(
            "delete",
            "TaskUpdate",
            json!({"taskId": "1", "status": "deleted"}),
        ),
    );
    let deleted = observe(
        &mut state,
        &tool_result("delete", json!({"success": true, "taskId": "1"})),
    );
    assert!(deleted.is_empty());
    // A replayed result is a no-op, not a second deletion.
    let replayed = state.observe(&tool_result(
        "delete",
        json!({"success": true, "taskId": "1"}),
    ));
    assert!(replayed.is_none());
}

#[test]
fn an_errored_tool_result_leaves_the_list_unchanged() {
    let mut state = ClaudeTaskState::default();
    record(
        &mut state,
        &tool_use("create", "TaskCreate", json!({"subject": "Alpha"})),
    );
    observe(
        &mut state,
        &tool_result("create", json!({"task": {"id": "1", "subject": "Alpha"}})),
    );
    record(
        &mut state,
        &tool_use(
            "update",
            "TaskUpdate",
            json!({"taskId": "1", "status": "completed"}),
        ),
    );
    // The tool's own failure report: Paseo's `result?.success === false`
    // short-circuits before any field is read.
    let errored = state.observe(&tool_result(
        "update",
        json!({"success": false, "error": "The tool failed"}),
    ));
    assert!(errored.is_none());
    // The errored result consumed the call id, so a re-delivered one is a
    // no-op; the list itself is unchanged — an independent snapshot still
    // shows the item pending.
    let redelivered = state.observe(&tool_result(
        "update",
        json!({"success": true, "taskId": "1"}),
    ));
    assert!(redelivered.is_none());
    record(
        &mut state,
        &tool_use("create-2", "TaskCreate", json!({"subject": "Beta"})),
    );
    let after = observe(
        &mut state,
        &tool_result("create-2", json!({"task": {"id": "2", "subject": "Beta"}})),
    );
    assert_eq!(after.len(), 2, "the list survived the errored result");
    assert_eq!(after[0].id.as_deref(), Some("1"));
    assert_eq!(after[0].status, AgentTaskStatus::Pending);
    assert_eq!(after[1].id.as_deref(), Some("2"));
}

#[test]
fn a_result_with_no_structured_body_leaves_the_list_unchanged() {
    let mut state = ClaudeTaskState::default();
    record(
        &mut state,
        &tool_use("create", "TaskCreate", json!({"subject": "Alpha"})),
    );
    // No tool_use_result at all: TaskCreate reads its id and text from the
    // structured result, so there is nothing to apply. The call itself is
    // consumed — a re-delivered result is a no-op.
    let bare = state.observe(&json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": [{"type": "tool_result", "tool_use_id": "create", "content": "ok"}]
        }
    }));
    assert!(bare.is_none());
    let replayed = state.observe(&json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": [{"type": "tool_result", "tool_use_id": "create", "content": "ok"}]
        }
    }));
    assert!(replayed.is_none());
}

#[test]
fn a_taskupdate_status_rides_the_tool_use_input_not_the_result() {
    // Paseo's `applyUpdate` reads `input.status` first and the result's
    // `statusChange.to` only as a fallback, so a result with no structured
    // body still applies the status the call carried.
    let mut state = ClaudeTaskState::default();
    record(
        &mut state,
        &tool_use("create", "TaskCreate", json!({"subject": "Alpha"})),
    );
    observe(
        &mut state,
        &tool_result("create", json!({"task": {"id": "1", "subject": "Alpha"}})),
    );
    record(
        &mut state,
        &tool_use(
            "update",
            "TaskUpdate",
            json!({"taskId": "1", "status": "completed"}),
        ),
    );
    let applied = state.observe(&json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": [{"type": "tool_result", "tool_use_id": "update", "content": "ok"}]
        }
    }));
    let items = tasks_of(applied);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].status, AgentTaskStatus::Completed);
}

#[test]
fn the_synthetic_fixture_drives_the_whole_state_machine() {
    let envelopes: Vec<Value> = CLAUDE_TASKS_SYNTHETIC
        .lines()
        .map(|line| serde_json::from_str(line).expect("fixture line"))
        .collect();
    assert_eq!(envelopes.len(), 8);
    let mut state = ClaudeTaskState::default();
    let mut snapshots = Vec::new();
    for envelope in &envelopes {
        if let Some(event) = state.observe(envelope) {
            snapshots.push(tasks_of(Some(event)));
        }
    }
    // TodoWrite at tool_use, then TaskCreate, TaskUpdate and TaskList each at
    // their tool_result. The TodoWrite result itself fires nothing.
    assert_eq!(snapshots.len(), 4);
    assert_eq!(snapshots[0].len(), 2);
    assert_eq!(snapshots[0][0].text, "Inspect the project layout");
    assert_eq!(snapshots[0][1].status, AgentTaskStatus::InProgress);
    // Every snapshot carries the whole list, not the diff.
    assert_eq!(snapshots[1].len(), 3);
    assert_eq!(snapshots[1][2].id.as_deref(), Some("1"));
    assert_eq!(snapshots[1][2].text, "Verify the file on disk");
    assert_eq!(snapshots[1][2].status, AgentTaskStatus::Pending);
    assert_eq!(snapshots[2].len(), 3);
    assert_eq!(snapshots[2][2].status, AgentTaskStatus::Completed);
    // TaskList keeps only the two entries that carry an id.
    assert_eq!(snapshots[3].len(), 2);
    assert_eq!(snapshots[3][0].id.as_deref(), Some("1"));
    assert_eq!(snapshots[3][1].id.as_deref(), Some("2"));
}

#[test]
fn mixed_tool_use_blocks_each_feed_the_state() {
    // One assistant message routinely mixes task and non-task calls
    // (parallel tool use): the non-task block skips itself, never the
    // message — the TodoWrite snapshot fires and the TaskCreate beside the
    // Bash call is recorded for its later result.
    let mut state = ClaudeTaskState::default();
    let mixed = json!({
        "type": "assistant",
        "message": {
            "id": "msg_mixed",
            "role": "assistant",
            "content": [
                {"type": "tool_use", "id": "bash-1", "name": "Bash", "input": {"command": "ls"}},
                {"type": "tool_use", "id": "create-1", "name": "TaskCreate", "input": {"subject": "Alpha", "activeForm": "Doing alpha"}},
                {"type": "tool_use", "id": "todo-1", "name": "TodoWrite", "input": {"todos": [{"content": "Legacy", "status": "pending"}]}},
            ],
        }
    });
    let snapshot = tasks_of(state.observe(&mixed));
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].text, "Legacy");
    let created = observe(
        &mut state,
        &tool_result("create-1", json!({"task": {"id": "1", "subject": "Alpha"}})),
    );
    assert_eq!(created.len(), 2);
    assert_eq!(created[1].id.as_deref(), Some("1"));
    assert_eq!(created[1].active_form.as_deref(), Some("Doing alpha"));
}

#[test]
fn a_tool_result_for_an_unknown_id_keeps_the_envelopes_snapshot() {
    // A result for an id no call stashed — a retried delivery, a compacted
    // row — applies nothing, and whatever snapshot the same envelope
    // produced is still the answer. A bare unknown result alone answers
    // nothing, as Paseo's `if (!call) return snapshot` does.
    let mut state = ClaudeTaskState::default();
    assert!(state
        .observe(&tool_result(
            "ghost",
            json!({"task": {"id": "9", "subject": "Ghost"}}),
        ))
        .is_none());
    let envelope = json!({
        "type": "assistant",
        "message": {
            "id": "msg_both",
            "role": "assistant",
            "content": [
                {"type": "tool_use", "id": "todo-1", "name": "TodoWrite",
                 "input": {"todos": [{"content": "Legacy"}]}},
                {"type": "tool_result", "tool_use_id": "ghost", "content": "ok"},
            ],
        }
    });
    let snapshot = tasks_of(state.observe(&envelope));
    assert_eq!(snapshot.len(), 1);
    assert_eq!(snapshot[0].text, "Legacy");
}

#[test]
fn active_forms_ride_the_running_rows() {
    let mut state = ClaudeTaskState::default();
    // TodoWrite carries the running form per entry.
    let listed = observe(
        &mut state,
        &tool_use(
            "todo",
            "TodoWrite",
            json!({"todos": [
                {"content": "Legacy", "status": "in_progress", "activeForm": "Working the legacy"},
                {"content": "Plain", "status": "pending"},
            ]}),
        ),
    );
    assert_eq!(listed[0].active_form.as_deref(), Some("Working the legacy"));
    assert_eq!(listed[1].active_form, None);
    // TaskCreate takes the form from the call input ...
    record(
        &mut state,
        &tool_use(
            "create",
            "TaskCreate",
            json!({"subject": "Alpha", "activeForm": "Doing alpha"}),
        ),
    );
    let created = observe(
        &mut state,
        &tool_result("create", json!({"task": {"id": "1", "subject": "Alpha"}})),
    );
    let alpha = created
        .iter()
        .find(|item| item.id.as_deref() == Some("1"))
        .expect("created");
    assert_eq!(alpha.active_form.as_deref(), Some("Doing alpha"));
    // ... TaskUpdate replaces it when present and keeps it when absent.
    record(
        &mut state,
        &tool_use(
            "update",
            "TaskUpdate",
            json!({"taskId": "1", "status": "in_progress", "activeForm": "Finishing alpha"}),
        ),
    );
    let updated = observe(
        &mut state,
        &tool_result("update", json!({"success": true, "taskId": "1"})),
    );
    let alpha = updated
        .iter()
        .find(|item| item.id.as_deref() == Some("1"))
        .expect("updated");
    assert_eq!(alpha.status, AgentTaskStatus::InProgress);
    assert_eq!(alpha.active_form.as_deref(), Some("Finishing alpha"));
    record(
        &mut state,
        &tool_use(
            "rename",
            "TaskUpdate",
            json!({"taskId": "1", "subject": "Renamed"}),
        ),
    );
    let renamed = observe(
        &mut state,
        &tool_result("rename", json!({"success": true, "taskId": "1"})),
    );
    let alpha = renamed
        .iter()
        .find(|item| item.id.as_deref() == Some("1"))
        .expect("renamed");
    assert_eq!(alpha.text, "Renamed");
    assert_eq!(alpha.active_form.as_deref(), Some("Finishing alpha"));
    // TaskList reads the form off the entries, both spellings.
    record(&mut state, &tool_use("list", "TaskList", json!({})));
    let relisted = observe(
        &mut state,
        &tool_result(
            "list",
            json!({"tasks": [
                {"id": "7", "subject": "Current", "status": "in_progress", "active_form": "Finishing current"},
                {"id": "8", "subject": "Other", "status": "pending"},
            ]}),
        ),
    );
    assert_eq!(relisted.len(), 2);
    let current = relisted
        .iter()
        .find(|item| item.id.as_deref() == Some("7"))
        .expect("listed");
    assert_eq!(current.active_form.as_deref(), Some("Finishing current"));
}

#[test]
fn a_string_tool_use_result_does_not_hide_the_structured_one() {
    // The SDK types `toolUseResult` as unknown and it is frequently a plain
    // string; a string there must not hide an object under `tool_use_result`.
    let mut state = ClaudeTaskState::default();
    record(
        &mut state,
        &tool_use("create", "TaskCreate", json!({"subject": "Alpha"})),
    );
    let created = state.observe(&json!({
        "type": "user",
        "message": {
            "role": "user",
            "content": [{"type": "tool_result", "tool_use_id": "create", "content": "ok"}]
        },
        "toolUseResult": "Task #1 created",
        "tool_use_result": {"task": {"id": "1", "subject": "Alpha"}},
    }));
    let items = tasks_of(created);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].id.as_deref(), Some("1"));
}

#[test]
fn reset_drops_the_list_and_the_pending_calls() {
    let mut state = ClaudeTaskState::default();
    observe(
        &mut state,
        &tool_use("todo", "TodoWrite", json!({"todos": [{"content": "A"}]})),
    );
    record(
        &mut state,
        &tool_use("create", "TaskCreate", json!({"subject": "B"})),
    );
    state.reset();
    // The pending call is gone: its result applies nothing.
    assert!(state
        .observe(&tool_result(
            "create",
            json!({"task": {"id": "9", "subject": "B"}})
        ))
        .is_none());
    // The list is gone: an update for the old task finds nothing.
    record(
        &mut state,
        &tool_use(
            "u",
            "TaskUpdate",
            json!({"taskId": "legacy:0", "status": "completed"}),
        ),
    );
    assert!(state
        .observe(&tool_result(
            "u",
            json!({"success": true, "taskId": "legacy:0"})
        ))
        .is_none());
}

#[test]
fn turn_end_drops_pending_calls_but_keeps_the_list() {
    let mut state = ClaudeTaskState::default();
    observe(
        &mut state,
        &tool_use("todo", "TodoWrite", json!({"todos": [{"content": "A"}]})),
    );
    record(
        &mut state,
        &tool_use("create", "TaskCreate", json!({"subject": "B"})),
    );
    state.end_turn();
    // The interrupted call never resolves: its late result applies nothing ...
    assert!(state
        .observe(&tool_result(
            "create",
            json!({"task": {"id": "9", "subject": "B"}})
        ))
        .is_none());
    // ... but the list survived the turn.
    record(
        &mut state,
        &tool_use("c2", "TaskCreate", json!({"subject": "C"})),
    );
    let items = observe(
        &mut state,
        &tool_result("c2", json!({"task": {"id": "2", "subject": "C"}})),
    );
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].text, "A");
    assert_eq!(items[1].id.as_deref(), Some("2"));
}
