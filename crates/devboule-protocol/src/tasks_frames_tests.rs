//! The background-task wire: the one request frame, the one reply and the
//! one snapshot event, plus the version guard that keeps an older peer off
//! the request.

use super::*;
use crate::{SessionEvent, SessionTask, SessionTaskKind, SessionTaskState, SubagentTaskStatus};

fn sample_tasks() -> Vec<SessionTask> {
    vec![SessionTask {
        id: "s.child.1".to_string(),
        kind: SessionTaskKind::Agent,
        title: "child".to_string(),
        state: SessionTaskState::Running,
        session_id: "s.1".to_string(),
        child_session_id: Some("s.child.1".to_string()),
        started_at_ms: 7,
        ended_at_ms: None,
        model: None,
        tool_call_count: None,
    }]
}

#[test]
fn the_tasks_request_round_trips_through_its_wire_shape() {
    let request = ClientMessage::SessionTasksGet {
        id: 3,
        session_id: "s.1".to_string(),
    };
    assert_eq!(request.name(), "SessionTasksGet");
    assert_eq!(request.request_id(), Some(3));
    assert!(
        !request.is_state_changing(),
        "a task list read must not be audited as a write"
    );
    assert_eq!(request.idempotency_key(), None);
    let json: serde_json::Value = serde_json::to_value(&request).expect("json");
    assert_eq!(json["type"], "session_tasks_get");
    let back: ClientMessage = serde_json::from_value(json).expect("deserialize");
    assert_eq!(request, back);

    let reply = DaemonMessage::SessionTasks {
        id: 3,
        session_id: "s.1".to_string(),
        tasks: sample_tasks(),
    };
    let json = serde_json::to_value(&reply).expect("json");
    let back: DaemonMessage = serde_json::from_value(json).expect("deserialize");
    assert_eq!(reply, back);
}

#[test]
fn the_tasks_snapshot_tag_is_its_snake_case_variant() {
    let event = SessionEvent::TasksSnapshot {
        tasks: sample_tasks(),
    };
    assert_eq!(event.kind(), "tasks_snapshot");
    let json: serde_json::Value = serde_json::to_value(&event).expect("json");
    assert_eq!(json["type"], "tasks_snapshot");
    let back: SessionEvent = serde_json::from_value(json).expect("deserialize");
    assert_eq!(event, back);
}

#[test]
fn the_task_state_words_match_the_wire() {
    // Four words, shared by agents and commands: a nonzero exit is failed,
    // a stop is cancelled, and nothing else is terminal.
    let words = [
        (SessionTaskState::Running, "running"),
        (SessionTaskState::Finished, "finished"),
        (SessionTaskState::Failed, "failed"),
        (SessionTaskState::Cancelled, "cancelled"),
    ];
    for (state, word) in words {
        assert_eq!(
            serde_json::to_value(state).expect("json"),
            serde_json::json!(word),
            "{word} must serialise to exactly its wire word"
        );
    }
    let kinds = [
        (SessionTaskKind::Agent, "agent"),
        (SessionTaskKind::Command, "command"),
    ];
    for (kind, word) in kinds {
        assert_eq!(
            serde_json::to_value(kind).expect("json"),
            serde_json::json!(word),
            "{word} must serialise to exactly its wire word"
        );
    }
}

#[test]
fn a_background_tool_call_marks_only_itself() {
    // The flag is opt-in on the call: a foreground call and a row written
    // before the field existed both read back as `None`, never as false.
    let flagged = SessionEvent::AgentToolCall {
        tool_call_id: "toolu_1".to_string(),
        title: "sleep 60".to_string(),
        status: "pending".to_string(),
        kind: Some("execute".to_string()),
        locations: None,
        subagent_type: None,
        parent_tool_use_id: None,
        spawn_depth: None,
        command: Some("sleep 60".to_string()),
        exit_code: None,
        background: Some(true),
    };
    let json = serde_json::to_value(&flagged).expect("json");
    assert_eq!(json["background"], true);
    let back: SessionEvent = serde_json::from_value(json).expect("deserialize");
    assert_eq!(flagged, back);

    let plain = SessionEvent::AgentToolCall {
        tool_call_id: "toolu_1".to_string(),
        title: "sleep 60".to_string(),
        status: "pending".to_string(),
        kind: Some("execute".to_string()),
        locations: None,
        subagent_type: None,
        parent_tool_use_id: None,
        spawn_depth: None,
        command: Some("sleep 60".to_string()),
        exit_code: None,
        background: None,
    };
    let json = serde_json::to_value(&plain).expect("json");
    assert!(
        json.get("background").is_none(),
        "an absent flag must stay absent on the wire"
    );
    // The provider's own task statuses keep their own words: this slice maps
    // them, it does not rename them.
    assert_eq!(
        serde_json::to_value(SubagentTaskStatus::Stopped).expect("json"),
        serde_json::json!("stopped")
    );
}
