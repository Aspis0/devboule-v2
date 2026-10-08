//! Claude task frames: the subagent and background task lifecycle, and
//! the envelope field readers the lifecycle shares. Provider descriptions
//! are masked on the way in: the event is what the wire, the journal and
//! the UI all carry.

use devboule_protocol::{AgentBackgroundTask, SessionEvent, SubagentTaskStatus};
use serde_json::Value;

use crate::process_argv_redact::redact_line;

use super::ClaudeView;

impl ClaudeView {
    pub(super) fn ingest_task_started(&self, envelope: &Value) -> Vec<SessionEvent> {
        let Some(task_id) = task_id(envelope) else {
            return Vec::new();
        };
        vec![SessionEvent::AgentTaskStarted {
            task_id,
            title: envelope
                .get("description")
                .and_then(Value::as_str)
                .map(|text| redact_line(text)),
            subagent_type: envelope
                .get("subagent_type")
                .and_then(Value::as_str)
                .map(str::to_string),
            tool_use_id: tool_use_id(envelope),
            is_backgrounded: envelope.get("is_backgrounded").and_then(Value::as_bool),
            spawn_depth: spawn_depth(envelope),
        }]
    }

    pub(super) fn ingest_task_notification(&mut self, envelope: &Value) -> Vec<SessionEvent> {
        let Some(task_id) = task_id(envelope) else {
            return Vec::new();
        };
        let Some(status) = envelope
            .get("status")
            .and_then(Value::as_str)
            .and_then(parse_task_status)
        else {
            return Vec::new();
        };
        let tool_use_id = tool_use_id(envelope);
        if let Some(tool_use_id) = tool_use_id.as_deref() {
            self.streamed
                .retain(|(parent, _), _| parent.as_deref() != Some(tool_use_id));
            self.current_message_ids
                .remove(&Some(tool_use_id.to_string()));
        }
        vec![SessionEvent::AgentTaskNotification {
            task_id,
            tool_use_id,
            status,
            summary: envelope
                .get("summary")
                .and_then(Value::as_str)
                .map(str::to_string),
        }]
    }

    pub(super) fn ingest_background_tasks_changed(&self, envelope: &Value) -> Vec<SessionEvent> {
        let Some(tasks) = envelope.get("tasks").and_then(Value::as_array) else {
            return Vec::new();
        };
        let tasks = tasks
            .iter()
            .filter_map(|task| {
                Some(AgentBackgroundTask {
                    task_id: task.get("task_id").and_then(Value::as_str)?.to_string(),
                    task_type: task.get("task_type").and_then(Value::as_str)?.to_string(),
                    title: redact_line(task.get("description").and_then(Value::as_str)?),
                })
            })
            .collect();
        vec![SessionEvent::AgentBackgroundTasksChanged { tasks }]
    }
}

pub(super) fn parent_tool_use_id(envelope: &Value) -> Option<String> {
    envelope
        .get("parent_tool_use_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn task_id(envelope: &Value) -> Option<String> {
    envelope
        .get("task_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn tool_use_id(envelope: &Value) -> Option<String> {
    envelope
        .get("tool_use_id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn parse_task_status(status: &str) -> Option<SubagentTaskStatus> {
    match status {
        "completed" => Some(SubagentTaskStatus::Completed),
        "failed" => Some(SubagentTaskStatus::Failed),
        "stopped" => Some(SubagentTaskStatus::Stopped),
        _ => None,
    }
}

pub(super) fn spawn_depth(envelope: &Value) -> Option<u32> {
    envelope
        .get("spawn_depth")
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
}

#[cfg(test)]
#[path = "claude_view_tasks_tests.rs"]
mod tests;
