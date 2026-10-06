//! `devboule_ci_watch`: watch one pushed commit's CI and get woken with a
//! verdict, instead of polling `gh` from the agent.
//!
//! The arguments are a closed set (`sha`, optional `repo`). The repository is
//! the one the caller's own workspace's `origin` names unless `repo` says
//! otherwise; the daemon's own `gh` login does the asking. A malformed call is
//! a protocol error; everything the tool can refuse for a reason an owner can
//! act on (no `gh`, not logged in, an unknown commit) is a tool error in the
//! `{hostId, ok, data?, error?}` envelope.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::ci_gh::{parse_repo_argument, CiError};
use crate::ci_watch::is_commit_id;
use crate::mcp_broker::caller::{audit_mcp_tool, McpCaller};
use crate::mcp_broker::dispatch::rpc_error;
use crate::mcp_broker::RegisteredSession;
use crate::server::ServerState;

struct Arguments {
    sha: String,
    repo: Option<(String, String)>,
}

/// What the call accepts, and nothing else: a full 40-hex commit id and an
/// optional `owner/repo` (or `host/owner/repo`).
fn parse_arguments(arguments: &Value) -> Result<Arguments, String> {
    let object = match arguments {
        Value::Object(map) => map,
        Value::Null => return Err("sha is required".to_string()),
        _ => return Err("arguments must be an object".to_string()),
    };
    for key in object.keys() {
        if key != "sha" && key != "repo" {
            return Err(format!("unknown parameter '{key}'"));
        }
    }
    let sha = match object.get("sha") {
        Some(Value::String(sha)) if is_commit_id(sha) => sha.to_ascii_lowercase(),
        Some(_) => return Err("sha must be a full 40-character commit id".to_string()),
        None => return Err("sha is required".to_string()),
    };
    let repo =
        match object.get("repo") {
            None | Some(Value::Null) => None,
            Some(Value::String(text)) => Some(parse_repo_argument(text).ok_or(
                "repo must be owner/repo (the host always comes from the workspace origin)",
            )?),
            Some(_) => return Err("repo must be a string".to_string()),
        };
    Ok(Arguments { sha, repo })
}

pub(in crate::mcp_broker) fn call(
    state: &Arc<ServerState>,
    registration: &RegisteredSession,
    caller: McpCaller,
    id: Value,
    message: &Value,
) -> Result<Option<Value>, Value> {
    let tool = crate::provider_catalog::MCP_CI_WATCH_TOOL;
    let audit = |outcome: &str| {
        audit_mcp_tool(state, &caller, tool, &registration.session_id, outcome);
    };
    let arguments = message
        .pointer("/params/arguments")
        .cloned()
        .unwrap_or(Value::Null);
    let arguments = match parse_arguments(&arguments) {
        Ok(arguments) => arguments,
        Err(sentence) => {
            audit("invalid");
            return Ok(Some(rpc_error(id, -32602, &sentence)));
        }
    };
    let host_id = host_id(state);
    match watch(state, registration, arguments) {
        Ok(data) => {
            audit("ok");
            Ok(Some(reply(&id, &host_id, Ok(data))))
        }
        Err(error) => {
            audit("failed");
            Ok(Some(reply(&id, &host_id, Err(error))))
        }
    }
}

fn watch(
    state: &Arc<ServerState>,
    registration: &RegisteredSession,
    arguments: Arguments,
) -> Result<Value, CiError> {
    let watches = &state.ci_watches;
    // The host always comes from the workspace origin — never the argument —
    // so resolving needs the workspace even when the repo names owner/repo.
    let root = state
        .sessions
        .session_workspace_root(&registration.session_id, &registration.owner)
        .map_err(|error| CiError::new("not_found", error.message, false))?;
    let repo = watches
        .gh_tool()
        .resolve_repo(root.as_deref(), arguments.repo)?;
    let record = watches.start(
        &registration.session_id,
        &registration.owner,
        &repo,
        &arguments.sha,
    )?;
    let wake = watches.wake_status(&record, &state.sessions);
    Ok(json!({
        "watchId": record.watch_id,
        "resolvedSha": record.sha,
        "state": record.state.as_str(),
        "repo": record.slug(),
        "wake": wake.as_str(),
    }))
}

/// The id that tells a caller which machine's daemon answered: its device id
/// when one exists, and a fixed word otherwise.
fn host_id(state: &ServerState) -> String {
    state
        .device_identity()
        .as_ref()
        .map(|identity| identity.device_id.clone())
        .unwrap_or_else(|_| "local".to_string())
}

fn reply(id: &Value, host_id: &str, outcome: Result<Value, CiError>) -> Value {
    let (envelope, text, is_error) = match outcome {
        Ok(data) => {
            let text = data.to_string();
            (
                json!({"hostId": host_id, "ok": true, "data": data}),
                text,
                false,
            )
        }
        Err(error) => {
            let envelope = json!({
                "hostId": host_id,
                "ok": false,
                "error": {
                    "code": error.code,
                    "message": error.message,
                    "retryable": error.retryable,
                },
            });
            (envelope, format!("{}: {}", error.code, error.message), true)
        }
    };
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{"type": "text", "text": text}],
            "structuredContent": envelope,
            "isError": is_error,
        },
    })
}

#[cfg(test)]
#[path = "ci_watch_tool_tests.rs"]
mod tests;
