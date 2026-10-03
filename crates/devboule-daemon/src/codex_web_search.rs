//! The `webSearch` thread item as a tool row.
//!
//! The schema pins `id`, `query`, an optional `action` and an opaque
//! `results`; nothing else is read. A started item becomes the call, a
//! completed item becomes the update.

use devboule_protocol::SessionEvent;
use serde_json::Value;

struct Presentation {
    title: String,
    kind: &'static str,
}

fn non_empty<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
}

/// What the action says about the row. `None` when the action is absent or
/// names nothing usable, so a completed update never replaces a call's title
/// with a weaker one.
fn action_presentation(item: &Value) -> Option<Presentation> {
    let action = item.get("action").filter(|action| action.is_object())?;
    match action.get("type").and_then(Value::as_str) {
        Some("openPage" | "findInPage") => Some(Presentation {
            title: non_empty(action, "url")?.to_string(),
            kind: "fetch",
        }),
        _ => Some(Presentation {
            title: query_title(item, Some(action)),
            kind: "search",
        }),
    }
}

/// The top-level `query` is required by the schema; an empty one falls back
/// to the action's own query.
fn query_title(item: &Value, action: Option<&Value>) -> String {
    non_empty(item, "query")
        .or_else(|| action.and_then(|action| non_empty(action, "query")))
        .unwrap_or_default()
        .to_string()
}

fn result_count_text(item: &Value) -> Option<String> {
    let count = item.get("results")?.as_array()?.len();
    match count {
        0 => None,
        1 => Some("1 result".to_string()),
        n => Some(format!("{n} results")),
    }
}

pub(super) fn web_search_events(id: &str, item: &Value, completed: bool) -> Vec<SessionEvent> {
    let presentation = action_presentation(item);
    if completed {
        vec![SessionEvent::AgentToolUpdate {
            tool_call_id: id.to_string(),
            status: Some("completed".to_string()),
            text: result_count_text(item),
            title: presentation.as_ref().map(|shown| shown.title.clone()),
            kind: presentation.map(|shown| shown.kind.to_string()),
            locations: None,
            parent_tool_use_id: None,
            spawn_depth: None,
            command: None,
            exit_code: None,
            replace: false,
        }]
    } else {
        let shown = presentation.unwrap_or_else(|| Presentation {
            title: query_title(item, None),
            kind: "search",
        });
        vec![SessionEvent::AgentToolCall {
            tool_call_id: id.to_string(),
            title: shown.title,
            status: "in_progress".to_string(),
            kind: Some(shown.kind.to_string()),
            locations: None,
            subagent_type: None,
            parent_tool_use_id: None,
            spawn_depth: None,
            command: None,
            exit_code: None,
        }]
    }
}

#[cfg(test)]
#[path = "codex_web_search_tests.rs"]
mod tests;
