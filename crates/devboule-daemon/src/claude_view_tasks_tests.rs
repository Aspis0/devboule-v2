//! Tests for one topic: Claude task frames — the lifecycle events and
//! the stream state a terminal notification cleans.

use devboule_protocol::{AgentBackgroundTask, SessionEvent, SubagentTaskStatus};
use serde_json::json;

use crate::claude_view::test_support::view;

#[test]
fn task_lifecycle_uses_the_measured_claude_envelopes() {
    let mut mapper = view();
    let _ = mapper.ingest(&json!({
        "type": "assistant",
        "message": {
            "id": "msg_agent",
            "role": "assistant",
            "content": [{
                "type": "tool_use",
                "id": "toolu_agent",
                "name": "Agent",
                "input": {
                    "description": "Find the relevant files",
                    "subagent_type": "explorer",
                    "run_in_background": false
                }
            }]
        }
    }));

    let started = mapper.ingest(&json!({
        "type": "system",
        "subtype": "task_started",
        "task_id": "task-1",
        "tool_use_id": "toolu_agent",
        "description": "Find the relevant files",
        "subagent_type": "explorer",
        "is_backgrounded": false,
        "spawn_depth": 1,
        "owned_by_subagent": false,
        "task_type": "local_agent",
        "uuid": "uuid-task-1",
        "session_id": "session-child-1"
    }));
    assert_eq!(
        started,
        vec![SessionEvent::AgentTaskStarted {
            task_id: "task-1".to_string(),
            title: Some("Find the relevant files".to_string()),
            subagent_type: Some("explorer".to_string()),
            tool_use_id: Some("toolu_agent".to_string()),
            is_backgrounded: Some(false),
            spawn_depth: Some(1),
        }]
    );

    let updated = mapper.ingest(&json!({
        "type": "system",
        "subtype": "task_updated",
        "task_id": "task-1",
        "patch": {"description": "Reading candidate files"}
    }));
    assert!(updated.is_empty());

    let notified = mapper.ingest(&json!({
        "type": "system",
        "subtype": "task_notification",
        "task_id": "task-1",
        "tool_use_id": "toolu_agent",
        "status": "failed",
        "output_file": "C:\\tmp\\task-1.txt",
        "summary": "The search failed",
        "usage": {"total_tokens": 10, "tool_uses": 2, "duration_ms": 30}
    }));
    assert!(matches!(
        notified.as_slice(),
        [SessionEvent::AgentTaskNotification {
            task_id,
            tool_use_id,
            status: SubagentTaskStatus::Failed,
            summary: Some(summary),
        }]
            if task_id == "task-1"
                && tool_use_id.as_deref() == Some("toolu_agent")
                && summary == "The search failed"
    ));

    let stopped = mapper.ingest(&json!({
        "type": "system",
        "subtype": "task_notification",
        "task_id": "task-2",
        "status": "stopped",
        "output_file": "C:\\tmp\\task-2.txt",
        "summary": "The task was stopped"
    }));
    assert!(matches!(
        stopped.as_slice(),
        [SessionEvent::AgentTaskNotification {
            status: SubagentTaskStatus::Stopped,
            summary: Some(summary),
            ..
        }] if summary == "The task was stopped"
    ));
}

#[test]
fn background_tasks_changed_forwards_its_replacement_set() {
    let mut mapper = view();
    let events = mapper.ingest(&json!({
        "type": "system",
        "subtype": "background_tasks_changed",
        "tasks": [
            {
                "task_id": "task-1",
                "task_type": "local_agent",
                "description": "Find the relevant files"
            }
        ]
    }));
    assert_eq!(
        events,
        vec![SessionEvent::AgentBackgroundTasksChanged {
            tasks: vec![AgentBackgroundTask {
                task_id: "task-1".to_string(),
                task_type: "local_agent".to_string(),
                title: "Find the relevant files".to_string(),
            }]
        }]
    );
}

#[test]
fn terminal_task_notification_cleans_child_stream_state() {
    let mut mapper = view();
    let _ = mapper.ingest(&json!({
        "type": "stream_event",
        "event": {"type": "message_start", "message": {"id": "msg_child"}},
        "parent_tool_use_id": "toolu_child"
    }));
    let _ = mapper.ingest(&json!({
        "type": "stream_event",
        "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "child"}},
        "parent_tool_use_id": "toolu_child"
    }));
    assert_eq!(mapper.current_message_ids.len(), 1);
    assert_eq!(mapper.streamed.len(), 1);

    let events = mapper.ingest(&json!({
        "type": "system",
        "subtype": "task_notification",
        "task_id": "task_child",
        "tool_use_id": "toolu_child",
        "status": "completed",
        "summary": "done"
    }));

    assert!(matches!(
        events.as_slice(),
        [SessionEvent::AgentTaskNotification { .. }]
    ));
    assert!(mapper.current_message_ids.is_empty());
    assert!(mapper.streamed.is_empty());
}
