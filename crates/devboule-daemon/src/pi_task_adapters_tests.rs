use devboule_protocol::{AgentTaskItem, AgentTaskStatus};
use serde_json::{json, Value};

use super::agent_tasks;

const GOAL_FIXTURE: &str = include_str!("../fixtures/pi-tasks/paseo-pi-goal-x.rpc-session.json");
const RPIV_FIXTURE: &str = include_str!("../fixtures/pi-tasks/paseo-rpiv-todo.rpc-session.json");
const EXAMPLE_FIXTURE: &str =
    include_str!("../fixtures/pi-tasks/paseo-pi-example-todo.rpc-session.json");

/// The captured completed calls, in the order the session ran them.
fn completed_calls(fixture: &str) -> Vec<Value> {
    let captured: Value = serde_json::from_str(fixture).expect("fixture JSON");
    captured
        .get("events")
        .and_then(Value::as_array)
        .expect("events array")
        .iter()
        .filter(|event| event.get("type").and_then(Value::as_str) == Some("tool_execution_end"))
        .cloned()
        .collect()
}

/// The seam as the view calls it, on one captured call.
fn adapt(call: &Value) -> Option<Vec<AgentTaskItem>> {
    agent_tasks(
        call.get("toolName")
            .and_then(Value::as_str)
            .expect("tool name"),
        call.get("result"),
        call.get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    )
}

/// One `details` payload on a completed call of the named tool.
fn details_on(tool_name: &str, details: Value) -> Option<Vec<AgentTaskItem>> {
    agent_tasks(tool_name, Some(&json!({ "details": details })), false)
}

/// `(id, status)` per row: enough to pin a snapshot's shape.
fn shape(items: &[AgentTaskItem]) -> Vec<(&str, AgentTaskStatus)> {
    items
        .iter()
        .map(|item| (item.id.as_deref().unwrap_or_default(), item.status))
        .collect()
}

#[test]
fn captured_goal_calls_yield_the_goal_snapshots() {
    let calls = completed_calls(GOAL_FIXTURE);
    // The call that creates a goal carries no task list and yields nothing.
    assert_eq!(adapt(&calls[0]), None);
    let set = adapt(&calls[1]).expect("set tasks snapshot");
    assert_eq!(
        shape(&set),
        [
            ("task-1", AgentTaskStatus::Pending),
            ("task-2", AgentTaskStatus::Pending)
        ]
    );
    assert_eq!(set[0].text, "Inspect workspace");
    let updated = adapt(&calls[2]).expect("update task snapshot");
    assert_eq!(
        shape(&updated),
        [
            ("task-1", AgentTaskStatus::Completed),
            ("task-2", AgentTaskStatus::Pending)
        ]
    );
    assert_eq!(updated[1].text, "Summarize findings");
}

#[test]
fn every_goal_tool_name_maps_a_valid_payload_to_the_same_list() {
    // All five names of the goal set, positively: a name dropped or
    // misspelled in the dispatch must fail here.
    let details = json!({
        "version": 3,
        "goal": {
            "currentTaskId": "t-1",
            "taskList": { "tasks": [
                { "id": "t-1", "title": "Running", "status": "pending" },
                { "id": "t-2", "title": "Done", "status": "complete" }
            ] }
        }
    });
    let expected = [
        ("t-1", AgentTaskStatus::InProgress),
        ("t-2", AgentTaskStatus::Completed),
    ];
    for tool_name in [
        "create_goal",
        "get_goal",
        "update_goal",
        "set_goal_tasks",
        "update_goal_task",
    ] {
        let items = details_on(tool_name, details.clone())
            .unwrap_or_else(|| panic!("{tool_name} must map a valid payload"));
        assert_eq!(shape(&items), expected, "tool {tool_name}");
    }
}

#[test]
fn goal_subtasks_flatten_depth_first_and_the_current_task_reads_in_progress() {
    let items = details_on(
        "get_goal",
        json!({
            "version": 3,
            "goal": {
                "currentTaskId": "t-sub",
                "taskList": { "tasks": [
                    { "id": "t-1", "title": "Parent", "status": "pending", "subtasks": [
                        { "id": "t-sub", "title": "Running child", "status": "pending",
                          "subtasks": [
                              { "id": "t-leaf", "title": "Grandchild", "status": "complete" }
                          ] },
                        { "id": "t-2", "title": "Skipped child", "status": "skipped" }
                    ] }
                ] }
            }
        }),
    )
    .expect("goal snapshot");
    assert_eq!(
        shape(&items),
        [
            ("t-1", AgentTaskStatus::Pending),
            ("t-sub", AgentTaskStatus::InProgress),
            ("t-leaf", AgentTaskStatus::Completed),
            ("t-2", AgentTaskStatus::Completed)
        ]
    );
    assert_eq!(items[2].text, "Grandchild");
}

#[test]
fn an_empty_task_list_still_replaces_the_snapshot() {
    let items = details_on(
        "set_goal_tasks",
        json!({ "version": 3, "goal": { "taskList": { "tasks": [] } } }),
    )
    .expect("the empty list is a snapshot of its own");
    assert!(items.is_empty());
}

#[test]
fn a_goal_payload_shape_we_do_not_know_yields_nothing() {
    assert_eq!(
        details_on(
            "get_goal",
            json!({ "version": 2, "goal": { "taskList": { "tasks": [] } } })
        ),
        None
    );
    assert_eq!(
        details_on(
            "get_goal",
            json!({ "version": "3", "goal": { "taskList": { "tasks": [] } } })
        ),
        None
    );
    assert_eq!(
        details_on("get_goal", json!({ "version": 3, "goal": {} })),
        None
    );
    assert_eq!(
        details_on("get_goal", json!({ "version": 3, "goal": null })),
        None
    );
    assert_eq!(details_on("get_goal", json!({ "version": 3 })), None);
    assert_eq!(
        details_on(
            "get_goal",
            json!({ "version": 3, "goal": { "taskList": { "tasks": [
                { "id": "1", "title": "x", "status": "done" }
            ] } } })
        ),
        None
    );
}

#[test]
fn captured_todo_calls_yield_the_todo_snapshots() {
    let snapshots: Vec<Vec<AgentTaskItem>> = completed_calls(RPIV_FIXTURE)
        .iter()
        .map(|call| adapt(call).expect("todo snapshot"))
        .collect();
    assert_eq!(snapshots.len(), 4);
    assert_eq!(shape(&snapshots[0]), [("1", AgentTaskStatus::Pending)]);
    assert_eq!(snapshots[0][0].text, "Inspect workspace");
    assert_eq!(
        shape(&snapshots[1]),
        [
            ("1", AgentTaskStatus::Pending),
            ("2", AgentTaskStatus::Pending)
        ]
    );
    assert_eq!(
        shape(&snapshots[2]),
        [
            ("1", AgentTaskStatus::InProgress),
            ("2", AgentTaskStatus::Pending)
        ]
    );
    assert_eq!(
        snapshots[2][0].active_form.as_deref(),
        Some("inspecting workspace")
    );
    assert_eq!(
        shape(&snapshots[3]),
        [
            ("1", AgentTaskStatus::Completed),
            ("2", AgentTaskStatus::Pending)
        ]
    );
}

#[test]
fn captured_example_todo_calls_yield_their_own_snapshots() {
    let snapshots: Vec<Vec<AgentTaskItem>> = completed_calls(EXAMPLE_FIXTURE)
        .iter()
        .map(|call| adapt(call).expect("todo snapshot"))
        .collect();
    assert_eq!(snapshots.len(), 3);
    assert_eq!(shape(&snapshots[0]), [("1", AgentTaskStatus::Pending)]);
    assert_eq!(
        shape(&snapshots[1]),
        [
            ("1", AgentTaskStatus::Pending),
            ("2", AgentTaskStatus::Pending)
        ]
    );
    assert_eq!(
        shape(&snapshots[2]),
        [
            ("1", AgentTaskStatus::Completed),
            ("2", AgentTaskStatus::Pending)
        ]
    );
    assert!(snapshots
        .iter()
        .flatten()
        .all(|item| item.active_form.is_none()));
}

#[test]
fn a_deleted_todo_row_is_dropped() {
    let items = details_on(
        "todo",
        json!({
            "action": "update",
            "params": { "id": 1 },
            "nextId": 4,
            "tasks": [
                { "id": 1, "subject": "Gone", "status": "deleted" },
                { "id": 2, "subject": "Kept", "status": "pending" }
            ]
        }),
    )
    .expect("snapshot without the deleted row");
    assert_eq!(shape(&items), [("2", AgentTaskStatus::Pending)]);
}

#[test]
fn a_result_that_reports_an_error_yields_nothing() {
    assert_eq!(
        details_on(
            "todo",
            json!({
                "action": "list",
                "params": {},
                "nextId": 2,
                "tasks": [{ "id": 1, "subject": "x", "status": "pending" }],
                "error": "no such id"
            })
        ),
        None
    );
    assert_eq!(
        details_on(
            "todo",
            json!({
                "action": "list",
                "nextId": 2,
                "todos": [{ "id": 1, "text": "x", "done": false }],
                "error": "no such id"
            })
        ),
        None
    );
}

#[test]
fn a_todo_payload_that_does_not_parse_yields_nothing() {
    let cases = [
        // nextId is not a whole number
        json!({ "action": "list", "params": {}, "nextId": 1.5, "tasks": [] }),
        // an unknown row status
        json!({ "action": "list", "params": {}, "nextId": 2,
                "tasks": [{ "id": 1, "subject": "x", "status": "archived" }] }),
        // a null running form
        json!({ "action": "list", "params": {}, "nextId": 2,
                "tasks": [{ "id": 1, "subject": "x", "status": "pending", "activeForm": null }] }),
        // a deleted row that does not parse fails the whole list
        json!({ "action": "list", "params": {}, "nextId": 2,
                "tasks": [{ "id": 1, "status": "deleted" }] }),
        // an unknown action
        json!({ "action": "reorder", "params": {}, "nextId": 2, "tasks": [] }),
        // the example todo shape with a non-boolean done flag
        json!({ "action": "list", "nextId": 2,
                "todos": [{ "id": 1, "text": "x", "done": "yes" }] }),
        // details that is not an object at all
        json!("not a payload"),
        Value::Null,
    ];
    for details in cases {
        assert_eq!(details_on("todo", details), None);
    }
}

#[test]
fn a_tool_no_adapter_claims_yields_nothing() {
    assert_eq!(
        details_on(
            "bash",
            json!({ "version": 3, "goal": { "taskList": { "tasks": [
                { "id": "1", "title": "x", "status": "pending" }
            ] } } })
        ),
        None
    );
    assert_eq!(
        details_on(
            "write",
            json!({ "action": "list", "params": {}, "nextId": 1, "tasks": [] })
        ),
        None
    );
}

#[test]
fn a_failed_or_missing_call_yields_nothing() {
    let details = json!({ "version": 3, "goal": { "taskList": { "tasks": [
        { "id": "1", "title": "x", "status": "pending" }
    ] } } });
    let result = json!({ "details": details });
    assert_eq!(agent_tasks("get_goal", Some(&result), true), None);
    assert_eq!(agent_tasks("get_goal", None, false), None);
    assert!(agent_tasks("get_goal", Some(&result), false).is_some());
}

#[test]
fn each_todo_shape_is_claimed_by_exactly_one_adapter() {
    let rpiv = json!({ "action": "list", "params": {}, "nextId": 1,
                       "tasks": [{ "id": 1, "subject": "x", "status": "pending" }] });
    let example = json!({ "action": "list", "nextId": 1,
                          "todos": [{ "id": 1, "text": "x", "done": false }] });
    assert!(super::rpiv_tasks(&rpiv).is_some());
    assert!(super::example_tasks(&example).is_some());
    assert_eq!(super::rpiv_tasks(&example), None);
    assert_eq!(super::example_tasks(&rpiv), None);
}
