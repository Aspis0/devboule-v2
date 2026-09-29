//! Accumulate Claude's task tools into checklist snapshots.
//!
//! Translated from Paseo's `claude/task-state.ts`: `TodoWrite` replaces the
//! whole list at `tool_use`; `TaskCreate`, `TaskUpdate` and `TaskList` apply
//! at `tool_result`, keyed by task id. The state is per session and lives in
//! the view, so a journal replay of the same envelopes produces the same
//! snapshots — the journal keeps the raw envelope, and this module reads
//! nothing else.

use std::collections::{HashMap, HashSet};

use devboule_protocol::{AgentTaskItem, AgentTaskStatus, SessionEvent};
use serde_json::Value;

/// The four tool names this state reads, and nothing else.
const TASK_TOOL_NAMES: [&str; 4] = ["TodoWrite", "TaskCreate", "TaskUpdate", "TaskList"];

#[derive(Clone, Default)]
pub(crate) struct ClaudeTaskState {
    /// The provider's tasks in the order the provider listed them. A plain
    /// list with a linear scan: plans are single-digit rows, and a scan
    /// keeps insertion order with re-insert-in-place, the way a JS
    /// Map does, without an order-preserving map dependency.
    tasks: Vec<AgentTaskItem>,
    /// Tool inputs stashed at `tool_use`, consumed at the matching
    /// `tool_result`. Per turn: a result never arrives after its turn's
    /// end, so the view drops unmatched entries there.
    calls: HashMap<String, PendingTaskTool>,
    applied_results: HashSet<String>,
}

#[derive(Clone, Default)]
struct PendingTaskTool {
    name: &'static str,
    input: Value,
}

impl ClaudeTaskState {
    /// Observe one envelope: record its task `tool_use` blocks, then apply a
    /// matching `tool_result`. Returns the snapshot the envelope produced —
    /// `TodoWrite` at `tool_use`, the other three at `tool_result` — or the
    /// current one when a result names no pending call.
    pub(crate) fn observe(&mut self, message: &Value) -> Option<SessionEvent> {
        if !message.is_object() {
            return None;
        }
        let mut snapshot = None;
        for block in tool_uses(message) {
            // One message routinely mixes task and non-task calls (parallel
            // tool use): a non-task block skips itself, never the message.
            let (Some(id), Some(name)) =
                (string(block.get("id")), task_tool_name(block.get("name")))
            else {
                continue;
            };
            let input = block.get("input").cloned().unwrap_or(Value::Null);
            if name == "TodoWrite" {
                snapshot = self.replace_legacy_todos(input.get("todos"));
            }
            self.calls.insert(id, PendingTaskTool { name, input });
        }
        // An assistant message carries no tool_result: the snapshot a
        // TodoWrite just produced is the answer.
        let Some(result_id) = tool_result_id(message) else {
            return snapshot;
        };
        if self.applied_results.contains(&result_id) {
            return snapshot;
        }
        // No pending call: the result belongs to no task tool this state
        // saw. The current list, if any, is still the answer.
        let Some(call) = self.calls.remove(&result_id) else {
            return snapshot;
        };
        self.applied_results.insert(result_id);
        self.apply_result(call, structured_result(message))
            .or(snapshot)
    }

    /// Session-level reset: drop the list, the pending calls and the result
    /// dedupe. The view is per child process, so a fresh session already
    /// starts empty; this exists for the rebind/history-load paths that
    /// reuse one.
    pub(crate) fn reset(&mut self) {
        self.tasks.clear();
        self.calls.clear();
        self.applied_results.clear();
    }

    /// Turn end: drop tool inputs whose result never arrived (interrupted
    /// turn, resume). The list itself is session state and stays, as does
    /// the result dedupe — ids never repeat, so a re-delivered result must
    /// stay a no-op across turns.
    pub(crate) fn end_turn(&mut self) {
        self.calls.clear();
    }

    /// Whether a task-tool input is still owed its result: while one is,
    /// the seed parses result envelopes too, so pairs complete.
    pub(crate) fn has_pending_calls(&self) -> bool {
        !self.calls.is_empty()
    }

    fn replace_legacy_todos(&mut self, value: Option<&Value>) -> Option<SessionEvent> {
        self.tasks.clear();
        let todos = value.and_then(Value::as_array)?;
        for (index, task_value) in todos.iter().enumerate() {
            let Some(item) = to_task_item(task_value) else {
                continue;
            };
            let id = item.id.clone().unwrap_or_else(|| format!("legacy:{index}"));
            self.upsert(AgentTaskItem {
                id: Some(id),
                ..item
            });
        }
        self.snapshot()
    }

    fn apply_result(
        &mut self,
        call: PendingTaskTool,
        result: Option<&Value>,
    ) -> Option<SessionEvent> {
        if result.and_then(|result| result.get("success")) == Some(&Value::Bool(false)) {
            return None;
        }
        match call.name {
            "TaskCreate" => self.apply_create(&call.input, result),
            "TaskUpdate" => self.apply_update(&call.input, result),
            "TaskList" => self.apply_list(result),
            _ => None,
        }
    }

    fn apply_create(&mut self, input: &Value, result: Option<&Value>) -> Option<SessionEvent> {
        let result_task = result
            .and_then(|result| result.get("task"))
            .filter(|v| v.is_object());
        let id = string(result_task.and_then(|task| task.get("id")))
            .or_else(|| string(result.and_then(|result| result.get("taskId"))))?;
        let text = string(result_task.and_then(|task| task.get("subject")))
            .or_else(|| string(input.get("subject")))?;
        self.upsert(AgentTaskItem {
            id: Some(id),
            text,
            status: AgentTaskStatus::Pending,
            active_form: string(input.get("activeForm")),
        });
        self.snapshot()
    }

    fn apply_update(&mut self, input: &Value, result: Option<&Value>) -> Option<SessionEvent> {
        let id = string(input.get("taskId"))
            .or_else(|| string(result.and_then(|result| result.get("taskId"))))?;
        let current = self.task(&id)?.clone();
        let status_value = input
            .get("status")
            .filter(|value| !value.is_null())
            .or_else(|| {
                result
                    .and_then(|result| result.get("statusChange"))
                    .and_then(|change| change.get("to"))
            });
        // No status in the envelope keeps the item's own; a present one
        // replaces it, and an unrecognized value reads as pending —
        // the recognized vocabulary is completed/deleted/in_progress
        // and nothing else.
        let status = match status_value {
            None => current.status,
            Some(value) => match value.as_str() {
                Some("completed") => AgentTaskStatus::Completed,
                Some("in_progress") => AgentTaskStatus::InProgress,
                Some("deleted") => {
                    self.remove(&id);
                    return self.snapshot();
                }
                _ => AgentTaskStatus::Pending,
            },
        };
        let mut item = current;
        if let Some(text) = string(input.get("subject")) {
            item.text = text;
        }
        // A present form replaces the running row's text; an absent one
        // keeps it — the current item is copied first, so a missing key
        // leaves the running text alone.
        if let Some(active_form) = string(input.get("activeForm")) {
            item.active_form = Some(active_form);
        }
        item.status = status;
        self.upsert(item);
        self.snapshot()
    }

    fn apply_list(&mut self, result: Option<&Value>) -> Option<SessionEvent> {
        let tasks = result.and_then(|result| result.get("tasks"))?.as_array()?;
        self.tasks.clear();
        for task_value in tasks {
            let Some(item) = to_task_item(task_value) else {
                continue;
            };
            // Unlike TodoWrite, TaskList keeps only the entries that carry
            // an id of their own.
            if item.id.is_none() {
                continue;
            }
            self.upsert(item);
        }
        self.snapshot()
    }

    fn snapshot(&self) -> Option<SessionEvent> {
        Some(SessionEvent::AgentTasks {
            items: self.tasks.to_vec(),
        })
    }

    /// Re-insert in place: an id the list already carries keeps its
    /// position, a new one appends — the re-set order of a `Map`.
    fn upsert(&mut self, item: AgentTaskItem) {
        let id = item.id.clone();
        if let Some(slot) = self.tasks.iter_mut().find(|task| task.id == id) {
            *slot = item;
        } else {
            self.tasks.push(item);
        }
    }

    fn task(&self, id: &str) -> Option<&AgentTaskItem> {
        self.tasks
            .iter()
            .find(|task| task.id.as_deref() == Some(id))
    }

    fn remove(&mut self, id: &str) {
        if let Some(index) = self
            .tasks
            .iter()
            .position(|task| task.id.as_deref() == Some(id))
        {
            self.tasks.remove(index);
        }
    }
}

/// The task tool name when the block carries one of the four, and nothing
/// else.
fn task_tool_name(value: Option<&Value>) -> Option<&'static str> {
    let name = value.and_then(Value::as_str)?.trim();
    TASK_TOOL_NAMES
        .into_iter()
        .find(|candidate| *candidate == name)
}

/// The `tool_use` blocks of one message, assistant or user.
fn tool_uses(message: &Value) -> Vec<&Value> {
    let Some(content) = message
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)
    else {
        return Vec::new();
    };
    content
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("tool_use"))
        .collect()
}

/// The `tool_use_id` of the first `tool_result` block of one message,
/// trimmed on both sides — the same trim `string()` applies to stored text.
fn tool_result_id(message: &Value) -> Option<String> {
    let content = message
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(Value::as_array)?;
    content
        .iter()
        .find(|block| block.get("type").and_then(Value::as_str) == Some("tool_result"))
        .and_then(|block| block.get("tool_use_id"))
        .and_then(Value::as_str)
        .map(|id| id.trim().to_string())
        .filter(|id| !id.is_empty())
}

/// The structured tool output the CLI carries beside the `tool_result`
/// blocks: the tool's own Output object, not the string content sent to the
/// model. The SDK's camelCase `toolUseResult` is read first, then the
/// wire's `tool_use_result` — and a non-object under the first key does not
/// hide an object under the second.
fn structured_result(message: &Value) -> Option<&Value> {
    message
        .get("toolUseResult")
        .filter(|value| value.is_object())
        .or_else(|| {
            message
                .get("tool_use_result")
                .filter(|value| value.is_object())
        })
}

/// One task entry as the provider described it. `None` when the entry carries
/// no text or says `deleted` — either drops the entry.
fn to_task_item(value: &Value) -> Option<AgentTaskItem> {
    if !value.is_object() {
        return None;
    }
    let text = string(value.get("subject"))
        .or_else(|| string(value.get("content")))
        .or_else(|| string(value.get("text")))?;
    let status = match value.get("status").and_then(Value::as_str) {
        Some("completed") => AgentTaskStatus::Completed,
        Some("in_progress") => AgentTaskStatus::InProgress,
        // `deleted` drops the item; anything else reads as pending.
        Some("deleted") => return None,
        _ => AgentTaskStatus::Pending,
    };
    let id = string(value.get("id")).or_else(|| string(value.get("taskId")));
    let active_form = string(value.get("activeForm")).or_else(|| string(value.get("active_form")));
    Some(AgentTaskItem {
        id,
        text,
        status,
        active_form,
    })
}

fn string(value: Option<&Value>) -> Option<String> {
    let value = value.and_then(Value::as_str)?;
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[cfg(test)]
#[path = "claude_task_state_tests.rs"]
mod tests;
