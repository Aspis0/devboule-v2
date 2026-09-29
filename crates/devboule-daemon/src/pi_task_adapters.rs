//! Translate a completed pi extension tool call into checklist items.
//!
//! The event loop never names an extension: one function takes the tool
//! name, the result and the error flag and answers with the snapshot the
//! payload carries, or with nothing.

use devboule_protocol::{AgentTaskItem, AgentTaskStatus};
use serde_json::Value;

/// The snapshot of one completed call, or `None` when there is none: a
/// failed call, a tool no adapter claims, a payload that does not parse.
/// An unknown version or shape answers with nothing, never a guessed list.
pub(crate) fn agent_tasks(
    tool_name: &str,
    result: Option<&Value>,
    is_error: bool,
) -> Option<Vec<AgentTaskItem>> {
    if is_error {
        return None;
    }
    let details = result?.get("details")?;
    if !details.is_object() {
        return None;
    }
    match tool_name {
        "create_goal" | "get_goal" | "update_goal" | "set_goal_tasks" | "update_goal_task" => {
            goal_tasks(details)
        }
        // Both todo extensions answer to the same tool name and their
        // payloads never parse for the other: the first shape that
        // validates claims the call.
        "todo" => rpiv_tasks(details).or_else(|| example_tasks(details)),
        _ => None,
    }
}

// Translated from Paseo packages/server/src/server/agent/providers/pi/extensions/pi-goal-x/index.ts.
fn goal_tasks(details: &Value) -> Option<Vec<AgentTaskItem>> {
    if details.get("version").and_then(Value::as_f64) != Some(3.0) {
        return None;
    }
    // The goal key is always present and may be null: a goal without a task
    // list carries nothing to show, and it is not a cleared list — the
    // source adapter emits no snapshot for it either.
    let goal = details.get("goal")?;
    if goal.is_null() {
        return None;
    }
    let current_task_id = match goal.get("currentTaskId") {
        None => None,
        Some(id) => Some(id.as_str()?),
    };
    let tasks = goal.get("taskList")?.get("tasks")?.as_array()?;
    goal_items(tasks, current_task_id)
}

/// The goal's tasks in list order, each parent followed by its subtasks,
/// recursively; the pending task the goal marks current reads in progress.
fn goal_items(tasks: &[Value], current_task_id: Option<&str>) -> Option<Vec<AgentTaskItem>> {
    let mut items = Vec::new();
    for task in tasks {
        let id = task.get("id")?.as_str()?;
        let text = task.get("title")?.as_str()?;
        let status = match task.get("status")?.as_str()? {
            "complete" | "skipped" => AgentTaskStatus::Completed,
            "pending" if Some(id) == current_task_id => AgentTaskStatus::InProgress,
            "pending" => AgentTaskStatus::Pending,
            _ => return None,
        };
        items.push(AgentTaskItem {
            id: Some(id.to_string()),
            text: text.to_string(),
            status,
            active_form: None,
        });
        if let Some(subtasks) = task.get("subtasks") {
            items.extend(goal_items(subtasks.as_array()?, current_task_id)?);
        }
    }
    Some(items)
}

// Translated from Paseo packages/server/src/server/agent/providers/pi/extensions/rpiv-todo/index.ts.
fn rpiv_tasks(details: &Value) -> Option<Vec<AgentTaskItem>> {
    if !matches!(
        details.get("action").and_then(Value::as_str),
        Some("create" | "update" | "list" | "get" | "delete" | "clear")
    ) {
        return None;
    }
    if !details.get("params")?.is_object() {
        return None;
    }
    whole_number(details.get("nextId"))?;
    if let Some(error) = details.get("error") {
        if !error.as_str()?.is_empty() {
            return None;
        }
    }
    let mut items = Vec::new();
    for task in details.get("tasks")?.as_array()? {
        let id = whole_number(task.get("id"))?;
        let text = task.get("subject")?.as_str()?;
        let active_form = match task.get("activeForm") {
            None => None,
            Some(form) => {
                let form = form.as_str()?;
                (!form.is_empty()).then_some(form.to_string())
            }
        };
        let status = match task.get("status").and_then(Value::as_str)? {
            "pending" => AgentTaskStatus::Pending,
            "in_progress" => AgentTaskStatus::InProgress,
            "completed" => AgentTaskStatus::Completed,
            // Every row validates before this match, so a deleted row that
            // does not parse still fails the whole list.
            "deleted" => continue,
            _ => return None,
        };
        items.push(AgentTaskItem {
            id: Some(id.to_string()),
            text: text.to_string(),
            status,
            active_form,
        });
    }
    Some(items)
}

// Translated from Paseo packages/server/src/server/agent/providers/pi/extensions/pi-example-todo/index.ts.
fn example_tasks(details: &Value) -> Option<Vec<AgentTaskItem>> {
    if !matches!(
        details.get("action").and_then(Value::as_str),
        Some("list" | "add" | "toggle" | "clear")
    ) {
        return None;
    }
    whole_number(details.get("nextId"))?;
    if let Some(error) = details.get("error") {
        if !error.as_str()?.is_empty() {
            return None;
        }
    }
    let mut items = Vec::new();
    for todo in details.get("todos")?.as_array()? {
        let id = whole_number(todo.get("id"))?;
        let text = todo.get("text")?.as_str()?;
        let done = todo.get("done")?.as_bool()?;
        items.push(AgentTaskItem {
            id: Some(id.to_string()),
            text: text.to_string(),
            status: if done {
                AgentTaskStatus::Completed
            } else {
                AgentTaskStatus::Pending
            },
            active_form: None,
        });
    }
    Some(items)
}

/// A whole JSON number: `2` and `2.0` are the same number to the payload's
/// own parser, `2.5` and `"2"` are not numbers at all.
fn whole_number(value: Option<&Value>) -> Option<i64> {
    let number = value?.as_f64()?;
    (number.fract() == 0.0).then_some(number as i64)
}

#[cfg(test)]
#[path = "pi_task_adapters_tests.rs"]
mod tests;
