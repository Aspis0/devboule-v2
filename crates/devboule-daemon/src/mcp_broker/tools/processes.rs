//! The read tools: who owns a port or pid, and what one session provably
//! runs — plus the reply and argument shapes the cleanup handler shares.
//!
//! Every answer is rooted in the process index's proof (job or group, pid
//! plus creation time). There is no arbitrary-pid path anywhere: the owner
//! and list tools read only proven members.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::mcp_broker::caller::{audit_mcp_tool, McpCaller};
use crate::mcp_broker::dispatch::{rpc_error, tool_error};
use crate::mcp_broker::RegisteredSession;
use crate::process_index::ProcessEntry;
use crate::server::ServerState;

/// One refresh, or the platform's own failure as the tool's answer: a helper
/// that timed out surfaces as `platform_unavailable`, never as an empty list
/// that would read as "no processes".
pub(super) fn refreshed(state: &Arc<ServerState>, id: &Value) -> Result<(), Value> {
    state
        .process_index
        .refresh(state.sessions.live_process_roots())
        .map_err(|error| tool_error(id, &format!("platform_unavailable: {error}")))
}

/// The MCP reply for one result: the encoded document as text, the same
/// document as structured content, and the error flag.
pub(super) fn reply(id: Value, document: Value, is_error: bool) -> Result<Option<Value>, Value> {
    let text = serde_json::to_string(&document).map_err(|error| {
        json!({"jsonrpc":"2.0", "id": id, "error": {"code": -32603, "message": format!("Could not encode the answer: {error}")}})
    })?;
    Ok(Some(json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{"type": "text", "text": text}],
            "structuredContent": document,
            "isError": is_error,
        },
    })))
}

/// The arguments object with no key beyond `allowed`: an unknown key is a
/// refusal, not a silently ignored claim.
pub(super) fn strict_arguments<'a>(
    id: &Value,
    message: &'a Value,
    allowed: &[&str],
) -> Result<&'a Value, Value> {
    let Some(arguments) = message.pointer("/params/arguments") else {
        return Ok(&Value::Null);
    };
    match arguments {
        Value::Null => Ok(arguments),
        Value::Object(map) => {
            let unknown = map
                .keys()
                .find(|key| !allowed.contains(&key.as_str()))
                .map(String::as_str);
            match unknown {
                Some(key) => Err(rpc_error(
                    id.clone(),
                    -32602,
                    &format!("unknown argument: {key}"),
                )),
                None => Ok(arguments),
            }
        }
        _ => Err(rpc_error(id.clone(), -32602, "arguments must be an object")),
    }
}

fn argument_port(id: &Value, arguments: &Value) -> Result<Option<u16>, Value> {
    match arguments.get("port") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .filter(|port| *port <= u16::MAX as u64)
            .map(|port| Some(port as u16))
            .ok_or_else(|| rpc_error(id.clone(), -32602, "port must be a u16")),
    }
}

fn argument_pid(id: &Value, arguments: &Value) -> Result<Option<u32>, Value> {
    match arguments.get("pid") {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .filter(|pid| *pid <= u32::MAX as u64)
            .map(|pid| Some(pid as u32))
            .ok_or_else(|| rpc_error(id.clone(), -32602, "pid must be a u32")),
    }
}

/// The owner answer's row: identity, provenance and the redacted command
/// line, with the session context only a proven match may carry.
fn owner_value(
    entry: &ProcessEntry,
    session_id: String,
    agent: String,
    workspace_id: Option<String>,
) -> Value {
    json!({
        "pid": entry.pid,
        "startedAt": entry.started_at_ms,
        "sessionId": session_id,
        "agent": agent,
        "workspaceId": workspace_id,
        "exe": entry.exe,
        "commandLineRedacted": entry.argv.join(" "),
        "ports": entry.ports,
        "proof": entry.proof,
    })
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or(0)
}

/// The list answer's row: the same identity with the age a reader came here
/// for, and no session context — the caller already owns the session.
fn process_value(entry: &ProcessEntry) -> Value {
    json!({
        "pid": entry.pid,
        "startedAt": entry.started_at_ms,
        "elapsed": now_ms().saturating_sub(entry.started_at_ms),
        "exe": entry.exe,
        "argvRedacted": entry.argv.join(" "),
        "ports": entry.ports,
        "proof": entry.proof,
    })
}

/// `devboule_process_owner`: which proven session member owns a port or a
/// pid. Exactly one of the two; no hit is an empty match list, never an
/// error, and never a detail about anything outside the proof.
pub(in crate::mcp_broker) fn owner(
    state: &Arc<ServerState>,
    registration: &RegisteredSession,
    caller: McpCaller,
    id: Value,
    message: &Value,
) -> Result<Option<Value>, Value> {
    let arguments = strict_arguments(&id, message, &["port", "pid"])?;
    let port = argument_port(&id, arguments)?;
    let pid = argument_pid(&id, arguments)?;
    if port.is_some() == pid.is_some() {
        return Err(rpc_error(
            id.clone(),
            -32602,
            "exactly one of port or pid is required",
        ));
    }
    refreshed(state, &id)?;
    let matches = state
        .process_index
        .owner_matches(port, pid)
        .into_iter()
        .map(|(entry, session_id, agent, workspace_id)| {
            owner_value(&entry, session_id, agent, workspace_id)
        })
        .collect::<Vec<_>>();
    audit_mcp_tool(
        state,
        &caller,
        crate::provider_catalog::MCP_PROCESS_OWNER_TOOL,
        &registration.session_id,
        "ok",
    );
    reply(id, json!({"matches": matches}), false)
}

/// `devboule_session_processes`: the caller's own session's proven members.
/// Another session's id is `not_owned`: no owner or admin context exists on
/// this surface, so the refusal never confirms whether that session exists.
pub(in crate::mcp_broker) fn list(
    state: &Arc<ServerState>,
    registration: &RegisteredSession,
    caller: McpCaller,
    id: Value,
    message: &Value,
) -> Result<Option<Value>, Value> {
    let arguments = strict_arguments(&id, message, &["sessionId"])?;
    if let Some(session_id) = arguments.get("sessionId").and_then(Value::as_str) {
        if session_id != registration.session_id {
            audit_mcp_tool(
                state,
                &caller,
                crate::provider_catalog::MCP_SESSION_PROCESSES_TOOL,
                &registration.session_id,
                "denied",
            );
            return Err(tool_error(
                &id,
                "not_owned: this tool lists the calling session's processes only",
            ));
        }
    }
    refreshed(state, &id)?;
    let processes = state
        .process_index
        .session_entries(&registration.session_id)
        .into_iter()
        .map(|entry| process_value(&entry))
        .collect::<Vec<_>>();
    audit_mcp_tool(
        state,
        &caller,
        crate::provider_catalog::MCP_SESSION_PROCESSES_TOOL,
        &registration.session_id,
        "ok",
    );
    reply(id, json!({"processes": processes}), false)
}

#[cfg(test)]
#[path = "mcp_process_tools_tests.rs"]
mod tests;
