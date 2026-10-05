//! The process tools: who owns a port or pid, what one session provably
//! runs, and the carded cleanup of the caller's own session's members.
//!
//! Every answer is rooted in the process index's proof (job or group, pid
//! plus creation time). There is no arbitrary-pid path anywhere: the owner
//! and list tools read only proven members, and cleanup takes no pid at all.

use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};

use crate::mcp_broker::caller::{audit_mcp_tool, McpCaller};
use crate::mcp_broker::dispatch::{rpc_error, tool_error};
use crate::mcp_broker::tools::first_use::{ensure_carded, PROCESS_CLEANUP_GROUP};
use crate::mcp_broker::{McpBroker, RegisteredSession};
use crate::process_index::{CleanupPlan, ProcessEntry};
use crate::server::ServerState;

/// The default and the ceiling for `graceMs`: the graceful phase is a wait
/// the caller names, and neither it nor the forced phase may be unbounded.
const DEFAULT_GRACE_MS: u32 = 2_000;
const MAX_GRACE_MS: u32 = 30_000;

/// The MCP reply for one result: the encoded document as text, the same
/// document as structured content, and the error flag.
fn reply(id: Value, document: Value, is_error: bool) -> Result<Option<Value>, Value> {
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
fn strict_arguments<'a>(
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
    state
        .process_index
        .refresh(state.sessions.live_process_roots());
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
    state
        .process_index
        .refresh(state.sessions.live_process_roots());
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

/// `devboule_cleanup_processes`: the caller's own session's proven members,
/// graceful then forced, behind the human card that names the session and
/// the count. The session's own root (the agent) is never in the plan, and
/// there is no pid argument to smuggle one in through.
pub(in crate::mcp_broker) fn cleanup(
    state: &Arc<ServerState>,
    broker: &McpBroker,
    registration: &RegisteredSession,
    caller: McpCaller,
    id: Value,
    message: &Value,
) -> Result<Option<Value>, Value> {
    let arguments = strict_arguments(&id, message, &["graceMs"])?;
    let grace = match arguments.get("graceMs") {
        None | Some(Value::Null) => DEFAULT_GRACE_MS,
        Some(value) => value
            .as_u64()
            .filter(|millis| *millis <= MAX_GRACE_MS as u64)
            .map(|millis| millis as u32)
            .ok_or_else(|| {
                rpc_error(
                    id.clone(),
                    -32602,
                    "graceMs must be an integer of at most 30000",
                )
            })?,
    };
    state
        .process_index
        .refresh(state.sessions.live_process_roots());
    let plan = state
        .process_index
        .cleanup_plan(&registration.session_id)
        .unwrap_or(CleanupPlan {
            targets: Vec::new(),
            unproven: Vec::new(),
        });
    let label = state
        .process_index
        .session_label(&registration.session_id)
        .unwrap_or_else(|| registration.session_id.clone());
    let count = plan.targets.len().to_string();
    let facts: [(&str, &str); 2] = [("session", label.as_str()), ("processes", count.as_str())];
    if let Err(sentence) = ensure_carded(
        state,
        broker,
        &registration.session_id,
        &registration.owner,
        PROCESS_CLEANUP_GROUP,
        &label,
        &facts,
    ) {
        audit_mcp_tool(
            state,
            &caller,
            crate::provider_catalog::MCP_CLEANUP_PROCESSES_TOOL,
            &registration.session_id,
            "denied",
        );
        return Err(tool_error(&id, &sentence));
    }
    // Re-read after the card: the set may have moved while a person read it.
    state
        .process_index
        .refresh(state.sessions.live_process_roots());
    let plan = state
        .process_index
        .cleanup_plan(&registration.session_id)
        .unwrap_or(CleanupPlan {
            targets: Vec::new(),
            unproven: Vec::new(),
        });
    let mut termination = crate::process_terminate::terminate_all(
        &plan.targets,
        Duration::from_millis(u64::from(grace)),
    )
    .map_err(|error| tool_error(&id, &format!("platform_unavailable: {error}")))?;
    termination.terminated.sort_unstable();
    termination.still_running.sort_unstable();
    let mut unproven = plan.unproven;
    unproven.sort_unstable();
    audit_mcp_tool(
        state,
        &caller,
        crate::provider_catalog::MCP_CLEANUP_PROCESSES_TOOL,
        &registration.session_id,
        "ok",
    );
    reply(
        id,
        json!({
            "terminated": termination.terminated,
            "stillRunning": termination.still_running,
            "unproven": unproven,
        }),
        false,
    )
}

#[cfg(test)]
#[path = "mcp_process_tools_tests.rs"]
mod tests;
