//! The checklist a resumed pi restored: one read of pi's own conversation,
//! its tool calls paired with their results, and the last snapshot the task
//! adapter accepts, published once.
//!
//! Only the checklist is read back here; the transcript is never rebuilt.

use std::collections::HashMap;
use std::sync::Arc;

use devboule_protocol::AgentTaskItem;
use serde_json::Value;

use super::{ControlBudget, PiControl};
use crate::session::SessionRuntime;

/// Start the read on a thread of its own: its answer is delivered by the
/// session reader, so the reader must never be the one waiting for it.
pub(super) fn spawn(control: Arc<PiControl>, runtime: Arc<SessionRuntime>) {
    let started = std::thread::Builder::new()
        .name("pi-history-tasks".to_string())
        .spawn(move || read_last_tasks(&control, &runtime));
    if let Err(error) = started {
        eprintln!("pi history tasks could not start ({error}); no checklist was restored");
    }
}

/// One `get_messages`, then the one snapshot it carries. A refusal, a
/// timeout, or a reply with no acceptable snapshot publishes nothing and
/// leaves the session working from live events alone.
fn read_last_tasks(control: &PiControl, runtime: &SessionRuntime) {
    let reply = match control.request_within("get_messages", Value::Null, &ControlBudget::hot()) {
        Ok(reply) => reply,
        Err(error) => {
            // The code alone: pi's error text can carry a path or a config
            // value, and the failed read changes nothing else about the
            // session.
            eprintln!(
                "pi get_messages failed ({:?}); no checklist was restored",
                error.code
            );
            return;
        }
    };
    let Some(messages) = reply
        .get("data")
        .and_then(|data| data.get("messages"))
        .and_then(Value::as_array)
    else {
        return;
    };
    if let Some(items) = last_task_snapshot(messages) {
        let _ = runtime.publish_restored_agent_tasks(items);
    }
}

// Translated from Paseo packages/server/src/server/agent/providers/pi/history-mapper.ts:66-93.
fn last_task_snapshot(messages: &[Value]) -> Option<Vec<AgentTaskItem>> {
    let mut calls: HashMap<String, String> = HashMap::new();
    let mut last = None;
    for message in messages {
        match message.get("role").and_then(Value::as_str) {
            Some("assistant") => remember_tool_calls(message, &mut calls),
            Some("toolResult") => {
                if let Some(items) = task_snapshot(message, &mut calls) {
                    last = Some(items);
                }
            }
            _ => {}
        }
    }
    last
}

fn remember_tool_calls(message: &Value, calls: &mut HashMap<String, String>) {
    let Some(content) = message.get("content").and_then(Value::as_array) else {
        return;
    };
    for block in content {
        if block.get("type").and_then(Value::as_str) != Some("toolCall") {
            continue;
        }
        let (Some(id), Some(name)) = (
            block.get("id").and_then(Value::as_str),
            block.get("name").and_then(Value::as_str),
        ) else {
            continue;
        };
        calls.insert(id.to_string(), name.to_string());
    }
}

/// A result whose call this walk never saw keeps its own tool name, and an
/// error reaches the adapter as one — which answers with nothing.
fn task_snapshot(
    message: &Value,
    calls: &mut HashMap<String, String>,
) -> Option<Vec<AgentTaskItem>> {
    let tool_call_id = message
        .get("toolCallId")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let tool_name = calls.remove(tool_call_id).or_else(|| {
        message
            .get("toolName")
            .and_then(Value::as_str)
            .map(str::to_string)
    })?;
    let result = serde_json::json!({
        "content": message.get("content"),
        "details": message.get("details"),
    });
    crate::pi_task_adapters::agent_tasks(
        &tool_name,
        Some(&result),
        message
            .get("isError")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    )
}

/// The fake pi child, the reader/harness glue, and the task-item builder
/// both test files below share.
#[cfg(test)]
#[path = "pi_history_tasks_test_support.rs"]
mod test_support;

#[cfg(test)]
#[path = "pi_history_tasks_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "pi_history_tasks_resume_tests.rs"]
mod resume_tests;
