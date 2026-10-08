//! Tool presentation: kinds, titles, exact commands and relativized
//! locations, and the call and result events built from them.

use std::path::Path;

use devboule_protocol::{SessionEvent, ToolLocation};
use serde_json::Value;

use crate::browser_tool_title::browser_tool_title;
use crate::text_cap::capped;
use crate::tool_paths::relativize_tool_path;
use crate::wire_json::{blocks_text, shell_command_from_tool, tool_kind_from_name, tool_status};

fn tool_title(name: &str, input: &Value, cwd: Option<&Path>) -> String {
    let field = |key: &str| {
        input
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
    };
    let truncated_command = |command: &str| {
        if command.chars().count() > 80 {
            format!("{}...", command.chars().take(80).collect::<String>())
        } else {
            command.to_string()
        }
    };
    // The summary only; the frontend derives the display name from `kind`.
    match name {
        "ExitPlanMode" => "Plan".to_string(),
        // The same table the command gate uses: a shell name in any casing
        // titles its row with the command, never the description fallback.
        _ if tool_kind_from_name(name) == "execute" => {
            field("command").map(truncated_command).unwrap_or_default()
        }
        "Read" | "Edit" | "Write" | "NotebookEdit" => field("file_path")
            .or_else(|| field("path"))
            .map(|path| relativize_tool_path(path, cwd))
            .unwrap_or_default(),
        "Grep" | "Glob" => field("pattern").unwrap_or_default().to_string(),
        "WebSearch" => field("query").unwrap_or_default().to_string(),
        "WebFetch" => field("url").unwrap_or_default().to_string(),
        "Agent" | "Task" => field("description").unwrap_or_default().to_string(),
        "Skill" => field("skill").unwrap_or_default().to_string(),
        // The browser lane titles itself: `click e33` names the call, where the
        // tool's name alone would name twenty tools and no argument at all.
        _ => browser_tool_title(name, input)
            .or_else(|| field("description").map(str::to_string))
            .or_else(|| field("command").map(truncated_command))
            .or_else(|| {
                field("file_path")
                    .or_else(|| field("path"))
                    .map(|path| relativize_tool_path(path, cwd))
            })
            .unwrap_or_else(|| name.to_string()),
    }
}

/// The exact line the agent sent, for the shell tools. Kept separate from
/// `title`, which `tool_title` may truncate, and kept raw: a Bash command may
/// itself be a wrapper (`bash -lc "npm test"`), and unwrapping it would store
/// a line the agent never sent.
fn tool_command(name: &str, input: &Value) -> Option<String> {
    shell_command_from_tool(name, input.get("command"))
}

fn tool_locations(name: &str, input: &Value, cwd: Option<&Path>) -> Option<Vec<ToolLocation>> {
    let path = match name {
        "Read" | "Edit" | "Write" | "NotebookEdit" => {
            input.get("file_path").or_else(|| input.get("path"))
        }
        "Glob" | "Grep" => input.get("path"),
        _ => None,
    }
    .and_then(Value::as_str)
    .filter(|path| !path.is_empty())?;
    let line = input
        .get("offset")
        .or_else(|| input.get("line"))
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok());
    Some(vec![ToolLocation {
        path: relativize_tool_path(path, cwd),
        line,
    }])
}

pub(super) fn tool_call_from_block(
    block: &Value,
    cwd: Option<&Path>,
    parent_tool_use_id: Option<String>,
    spawn_depth: Option<u32>,
) -> Option<SessionEvent> {
    let tool_call_id = block.get("id").and_then(Value::as_str)?.to_string();
    let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
    let input = block.get("input").unwrap_or(&Value::Null);
    Some(SessionEvent::AgentToolCall {
        tool_call_id,
        title: tool_title(name, input, cwd),
        status: "pending".to_string(),
        kind: Some(if name == "ExitPlanMode" {
            "plan".to_string()
        } else {
            tool_kind_from_name(name).to_string()
        }),
        locations: tool_locations(name, input, cwd),
        subagent_type: (name == "Agent")
            .then(|| input.get("subagent_type"))
            .flatten()
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        parent_tool_use_id,
        spawn_depth,
        command: tool_command(name, input),
        exit_code: None,
        // Only Bash carries the flag, and only `true` is kept: an absent
        // or false flag reads back as `None`, never as foreground-proof.
        background: (name == "Bash"
            && input.get("run_in_background").and_then(Value::as_bool) == Some(true))
        .then_some(true),
    })
}

pub(super) fn tool_update_from_result(
    block: &Value,
    parent_tool_use_id: Option<String>,
    spawn_depth: Option<u32>,
) -> Option<SessionEvent> {
    if block.get("type").and_then(Value::as_str) != Some("tool_result") {
        return None;
    }
    let tool_call_id = block
        .get("tool_use_id")
        .and_then(Value::as_str)?
        .to_string();
    let failed = block
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    // Every tool's answer lands here, so a `browser_read_text` of a large page
    // would be the row's whole text; the shared budget keeps it out of a frame.
    let text = block
        .get("content")
        .map(|content| capped(&blocks_text(content)));
    Some(SessionEvent::AgentToolUpdate {
        tool_call_id,
        status: Some(tool_status(failed).to_string()),
        text,
        title: None,
        kind: None,
        locations: None,
        parent_tool_use_id,
        spawn_depth,
        command: None,
        exit_code: None,
        replace: false,

        images: Vec::new(),
    })
}

#[cfg(test)]
#[path = "claude_view_tools_tests.rs"]
mod tests;
