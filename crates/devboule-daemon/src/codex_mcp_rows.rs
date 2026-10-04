//! The `mcpToolCall` thread item as a tool row.
//!
//! The schema pins `server`, `tool`, opaque `arguments`, a `status` of
//! `inProgress`, `completed` or `failed`, and the answer in `result.content` or
//! `error.message`; nothing else is read. A started item becomes the call, a
//! completed item the update.
//!
//! The row is named the way another provider names the same call, so one call
//! reads the same whoever served it: the qualified name `mcp__<server>__<tool>`,
//! which the shared kind table and the app's browser-name rules already
//! understand. A tool of the browser lane is titled from the arguments it was
//! given; every other tool keeps its own name.

use devboule_protocol::SessionEvent;
use serde_json::Value;

use super::status_name;
use crate::browser_tool_title::browser_tool_title;
use crate::text_cap::capped;
use crate::wire_json::{blocks_text, tool_kind_from_name};

fn non_empty<'a>(item: &'a Value, key: &str) -> Option<&'a str> {
    item.get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
}

/// The call's qualified name, or `None` for an item with no tool to call. A
/// server that sent no name of its own leaves the tool bare rather than
/// qualifying it with nothing.
fn qualified_name(item: &Value) -> Option<String> {
    let tool = non_empty(item, "tool")?;
    Some(match non_empty(item, "server") {
        Some(server) => format!("mcp__{server}__{tool}"),
        None => tool.to_string(),
    })
}

/// What the row shows: the browser lane's own line for a tool of that lane,
/// and the name itself for every other tool.
fn row_title(name: &str, item: &Value) -> String {
    browser_tool_title(name, item.get("arguments").unwrap_or(&Value::Null))
        .unwrap_or_else(|| name.to_string())
}

/// The answer the row shows. A failure says what failed in its own words; a
/// result contributes the text of its content blocks and nothing else — an
/// image or an audio clip is not text, and copying its bytes into a journaled
/// row would outlive the call. Either way the answer is held to the shared
/// budget: a server answers with as much text as it likes.
fn result_text(item: &Value) -> Option<String> {
    let error = item
        .get("error")
        .and_then(|error| non_empty(error, "message"));
    let result = item
        .pointer("/result/content")
        .map(blocks_text)
        .filter(|text| !text.is_empty());
    let text = error.map(str::to_string).or(result)?;
    Some(capped(&text))
}

pub(super) fn mcp_tool_events(id: &str, item: &Value, completed: bool) -> Vec<SessionEvent> {
    let Some(name) = qualified_name(item) else {
        return Vec::new();
    };
    let kind = tool_kind_from_name(&name).to_string();
    let status = item.get("status").and_then(Value::as_str).map(status_name);
    if completed {
        vec![SessionEvent::AgentToolUpdate {
            tool_call_id: id.to_string(),
            status,
            text: result_text(item),
            // The item repeats the call's own presentation, and the app names
            // a row it never saw the call for from this title.
            title: Some(row_title(&name, item)),
            kind: Some(kind),
            locations: None,
            parent_tool_use_id: None,
            spawn_depth: None,
            command: None,
            exit_code: None,
            replace: false,
        }]
    } else {
        vec![SessionEvent::AgentToolCall {
            tool_call_id: id.to_string(),
            title: row_title(&name, item),
            status: status.unwrap_or_else(|| "in_progress".to_string()),
            kind: Some(kind),
            locations: None,
            subagent_type: None,
            parent_tool_use_id: None,
            spawn_depth: None,
            // No shell line and no exit code: an MCP item carries the tool's
            // own arguments, never a command an agent ran.
            command: None,
            exit_code: None,
        }]
    }
}

#[cfg(test)]
#[path = "codex_mcp_rows_tests.rs"]
mod tests;
