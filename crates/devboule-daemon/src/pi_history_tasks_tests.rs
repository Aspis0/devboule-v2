//! The walk over pi's stored messages: the last checklist snapshot, and the
//! call/result pairing that decides which result carries it.

use super::last_task_snapshot;
use super::test_support::item;
use devboule_protocol::AgentTaskStatus;
use serde_json::{json, Value};

/// One stored assistant call followed by its result, the way pi's own
/// `get_messages` carries them.
fn call_and_result(call_id: &str, tasks: Value, is_error: bool) -> Vec<Value> {
    vec![
        json!({
            "role": "assistant",
            "content": [{"type": "toolCall", "id": call_id, "name": "set_goal_tasks", "arguments": {}}],
        }),
        json!({
            "role": "toolResult",
            "toolCallId": call_id,
            "toolName": "set_goal_tasks",
            "isError": is_error,
            "content": [{"type": "text", "text": "Task list set."}],
            "details": {"version": 3, "goal": {"taskList": {"tasks": tasks}}},
        }),
    ]
}

fn task(id: &str, title: &str, status: &str) -> Value {
    json!({"id": id, "title": title, "status": status})
}

/// The last snapshot in the walk wins, and each result consumes the call it
/// pairs with: the snapshot the later call carries is the one published, and
/// an unrelated tool's result changes nothing.
#[test]
fn the_last_snapshot_in_history_wins() {
    let mut messages =
        call_and_result("call-1", json!([task("task-1", "First", "pending")]), false);
    messages.extend(call_and_result(
        "call-2",
        json!([
            task("task-1", "First", "complete"),
            task("task-2", "Second", "pending"),
        ]),
        false,
    ));
    messages.push(json!({
        "role": "assistant",
        "content": [{"type": "toolCall", "id": "call-3", "name": "bash", "arguments": {"command": "ls"}}],
    }));
    messages.push(json!({
        "role": "toolResult",
        "toolCallId": "call-3",
        "toolName": "bash",
        "isError": false,
        "content": [{"type": "text", "text": "file"}],
        "details": {"exitCode": 0},
    }));
    assert_eq!(
        last_task_snapshot(&messages),
        Some(vec![
            item("task-1", "First", AgentTaskStatus::Completed),
            item("task-2", "Second", AgentTaskStatus::Pending),
        ]),
        "the second call's snapshot is the state history ends in"
    );
}

/// The fallback the source mapper keeps: a result whose assistant call the
/// walk never saw is still adapted under its own tool name.
#[test]
fn a_result_whose_call_is_untracked_keeps_its_own_tool_name() {
    let messages = vec![json!({
        "role": "toolResult",
        "toolCallId": "call-1",
        "toolName": "set_goal_tasks",
        "isError": false,
        "content": [],
        "details": {"version": 3, "goal": {"taskList": {"tasks": [task("task-1", "First", "pending")]}}},
    })];
    assert_eq!(
        last_task_snapshot(&messages),
        Some(vec![item("task-1", "First", AgentTaskStatus::Pending)])
    );
}

/// A failed call is no snapshot: it neither creates one nor replaces the one
/// before it.
#[test]
fn a_failed_result_contributes_nothing() {
    let failed = call_and_result("call-1", json!([task("task-1", "First", "pending")]), true);
    assert_eq!(last_task_snapshot(&failed), None);

    let mut after_a_snapshot =
        call_and_result("call-1", json!([task("task-1", "First", "pending")]), false);
    after_a_snapshot.extend(call_and_result(
        "call-2",
        json!([task("task-1", "First", "complete")]),
        true,
    ));
    assert_eq!(
        last_task_snapshot(&after_a_snapshot),
        Some(vec![item("task-1", "First", AgentTaskStatus::Pending)]),
        "the failed call never replaces the successful snapshot behind it"
    );
}

/// A history without a checklist answers with nothing, however much it
/// carries: an empty one, another tool's result, or a payload the adapter
/// refuses.
#[test]
fn a_history_without_a_checklist_answers_nothing() {
    assert_eq!(last_task_snapshot(&[]), None);
    assert_eq!(
        last_task_snapshot(&[json!({
            "role": "toolResult",
            "toolCallId": "call-1",
            "toolName": "bash",
            "isError": false,
            "content": [{"type": "text", "text": "file"}],
            "details": {"exitCode": 0},
        })]),
        None
    );
    assert_eq!(
        last_task_snapshot(&call_and_result(
            "call-1",
            json!([task("task-1", "First", "unknown-status")]),
            false
        )),
        None,
        "a payload the adapter cannot read is no snapshot"
    );
}
