//! Agent roster, activity, stop/close and profile-move tools.

use std::sync::Arc;

use serde_json::{json, Value};

use devboule_protocol::SessionEvent;

use crate::mcp_broker::caller::{audit_mcp_tool, McpCaller};
use crate::mcp_broker::dispatch::rpc_error;
use crate::mcp_broker::{McpBroker, RegisteredSession};
use crate::server::ServerState;

use super::creation::profile::resolve_profile_for_move;

pub(in crate::mcp_broker) fn agent_value(
    session: &devboule_protocol::Session,
    runtime: &crate::session::SessionRuntime,
    depth: u32,
) -> Value {
    let manifest = runtime.session_manifest();
    let manifest_provider = manifest.as_ref().and_then(|event| match event {
        SessionEvent::SessionManifest { provider_id, .. } => provider_id.clone(),
        _ => None,
    });
    let model = manifest.as_ref().and_then(|event| match event {
        SessionEvent::SessionManifest {
            current_model_id, ..
        } => current_model_id.clone(),
        _ => None,
    });
    // `name` is the display name a created agent was given; a session a person
    // started has none, and the row still needs a name a caller can address, so
    // it falls back to the same title the app renders (`S5` §1). `state` is the
    // A2A word, with a pending card outranking "working": a child parked on a
    // human's answer is the one fact a creator most needs to see.
    let name = session
        .display_name
        .clone()
        .unwrap_or_else(|| session.title.clone());
    let state = crate::session::roster_task_state(session, runtime);
    // S2: every roster entry carries the S1 word, read off the runtime the
    // broker stored at spawn and flipped on verification. S9 lists all agent
    // kinds; the card and result carry the promise for children too young to
    // have verified.
    json!({
        "id": session.id,
        "provider": session.provider.clone().or(manifest_provider),
        "model": model,
        "state": state,
        "name": name,
        "title": session.title,
        "createdBy": session.created_by,
        "depth": depth,
        "tools": runtime.tools_state().as_str(),
    })
}

/// One validated `devboule_agent_activity` call: the child to read plus the
/// bounded recent-lines limit. The known-parameter check is read out of the
/// published schema, so the document and the check cannot disagree.
fn parse_activity_arguments(arguments: &Value) -> Result<(String, usize), String> {
    let empty = json!({});
    let arguments = match arguments {
        Value::Null => &empty,
        Value::Object(_) => arguments,
        _ => return Err("arguments must be an object".to_string()),
    };
    let object = arguments
        .as_object()
        .ok_or_else(|| "arguments must be an object".to_string())?;
    let known: Vec<String> = crate::provider_catalog::agent_activity_input_schema()["properties"]
        .as_object()
        .map(|properties| properties.keys().cloned().collect())
        .unwrap_or_default();
    for key in object.keys() {
        if !known.iter().any(|known| known == key) {
            return Err(format!("unknown parameter '{key}'"));
        }
    }
    let session = object
        .get("session")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "session is required".to_string())?;
    let limit = match object.get("limit") {
        None | Some(Value::Null) => crate::agent_activity::clamp_limit(None),
        Some(Value::Number(n)) => {
            let n = n
                .as_u64()
                .ok_or_else(|| "limit must be an integer 0..50".to_string())?;
            crate::agent_activity::clamp_limit(Some(n))
        }
        Some(_) => return Err("limit must be an integer 0..50".to_string()),
    };
    Ok((session.to_string(), limit))
}

pub(in crate::mcp_broker) fn set_profile(
    state: &Arc<ServerState>,
    registration: &RegisteredSession,
    caller: McpCaller,
    id: Value,
    message: &Value,
) -> Result<Option<Value>, Value> {
    // Identity is the bearer, never the arguments: the child this
    // moves must be a live child of the session that called, and
    // "mine" is something the daemon knows from the registration —
    // a caller id in the arguments would be a claim, not a fact.
    let session_arg = message
        .pointer("/params/arguments/session")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let profile_arg = message
        .pointer("/params/arguments/profile")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let (Some(session_arg), Some(profile_arg)) = (session_arg, profile_arg) else {
        return Ok(Some(rpc_error(
            id,
            -32602,
            "session and profile are required",
        )));
    };
    let result = state.sessions.set_agent_child_profile(
        &registration.session_id,
        session_arg,
        profile_arg,
        &|requested| resolve_profile_for_move(&state.agent_profiles, requested),
    );
    // The audit names who called: the caller's device and role for a
    // peer (`resolve_mcp_caller` above), never `"local"` for one.
    let audit = |outcome_label: &str| {
        audit_mcp_tool(
            state,
            &caller,
            crate::provider_catalog::MCP_SET_AGENT_PROFILE_TOOL,
            &registration.session_id,
            outcome_label,
        );
    };
    match result {
        Ok(()) => {
            audit("ok");
            Ok(Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [{"type": "text", "text": "moved"}],
                    "structuredContent": {
                        "state": "moved",
                        "sessionId": session_arg,
                        "profile": profile_arg,
                    },
                    "isError": false,
                },
            })))
        }
        Err(sentence) => {
            audit("denied");
            Ok(Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [{"type": "text", "text": sentence}],
                    "isError": true,
                },
            })))
        }
    }
}

pub(in crate::mcp_broker) fn activity(
    state: &Arc<ServerState>,
    registration: &RegisteredSession,
    id: Value,
    message: &Value,
) -> Result<Option<Value>, Value> {
    // Identity is the bearer; the argument names which of the
    // caller's own live agents to read, by id or display name.
    // A read like the roster: no new identity and no text, only
    // timing metadata (idle age, seqs, kind timestamps) the
    // roster does not show.
    let arguments = message
        .pointer("/params/arguments")
        .cloned()
        .unwrap_or(Value::Null);
    let (session_arg, limit) = match parse_activity_arguments(&arguments) {
        Ok(parsed) => parsed,
        Err(message) => return Ok(Some(rpc_error(id, -32602, &message))),
    };
    let mut candidates = state
        .sessions
        .live_agent_entries(&registration.owner)
        .map_err(|error| {
            json!({"jsonrpc":"2.0", "id": id, "error": {"code": -32603, "message": error.message}})
        })?
        .into_iter()
        .filter(|entry| {
            entry.session.id == session_arg
                || entry.session.title == session_arg
                || entry
                    .session
                    .display_name
                    .as_deref()
                    .unwrap_or(&entry.session.title)
                    == session_arg
        })
        .map(|entry| entry.session.id)
        .collect::<Vec<_>>();
    if candidates.len() > 1 {
        return Ok(Some(rpc_error(
            id,
            -32602,
            &format!(
                "more than one of your live agents is called '{session_arg}'; use the session id"
            ),
        )));
    }
    let Some(target) = candidates.pop() else {
        return Ok(Some(rpc_error(id, -32602, "target agent not found")));
    };
    match state
        .sessions
        .agent_activity(&target, &registration.owner, limit)
    {
        Ok(document) => {
            let text = serde_json::to_string(&document).map_err(|error| {
                json!({"jsonrpc":"2.0", "id": id, "error": {"code": -32603, "message": format!("Could not encode agent activity: {error}")}})
            })?;
            Ok(Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [{"type": "text", "text": text}],
                    "structuredContent": document,
                    "isError": false,
                },
            })))
        }
        Err(_) => Ok(Some(rpc_error(id, -32602, "target agent not found"))),
    }
}

pub(in crate::mcp_broker) fn stop_or_close(
    state: &Arc<ServerState>,
    registration: &RegisteredSession,
    caller: McpCaller,
    id: Value,
    message: &Value,
    tool_name: Option<&str>,
) -> Result<Option<Value>, Value> {
    // The destructive pair. Identity is the bearer, never an
    // argument: the sessions layer refuses everything that is
    // not the caller's own live child with one sentence that
    // does not say whether the id exists, and refuses the
    // caller itself outright. Both outcomes are audited, like
    // the profile move.
    let stopping = tool_name == Some(crate::provider_catalog::MCP_STOP_AGENT_TOOL);
    let session_arg = message
        .pointer("/params/arguments/session")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty());
    let Some(session_arg) = session_arg else {
        return Ok(Some(rpc_error(id, -32602, "session is required")));
    };
    let tool = if stopping {
        crate::provider_catalog::MCP_STOP_AGENT_TOOL
    } else {
        crate::provider_catalog::MCP_CLOSE_AGENT_TOOL
    };
    let audit = |outcome_label: &str| {
        audit_mcp_tool(
            state,
            &caller,
            tool,
            &registration.session_id,
            outcome_label,
        );
    };
    let action = if stopping {
        state
            .sessions
            .stop_agent_child(&registration.session_id, session_arg)
    } else {
        state
            .sessions
            .close_agent_child(state, &registration.session_id, session_arg)
            .map(|_| ())
    };
    match action {
        Ok(()) => {
            audit("ok");
            let word = if stopping { "stopped" } else { "closed" };
            Ok(Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [{"type": "text", "text": word}],
                    "structuredContent": {"state": word, "sessionId": session_arg},
                    "isError": false,
                },
            })))
        }
        Err(error) => {
            audit("denied");
            Ok(Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [{"type": "text", "text": error.message}],
                    "isError": true,
                },
            })))
        }
    }
}

pub(in crate::mcp_broker) fn roster(
    state: &Arc<ServerState>,
    broker: &McpBroker,
    registration: &RegisteredSession,
    id: Value,
) -> Result<Option<Value>, Value> {
    // Deliberately do not read params.arguments. The bearer maps to
    // the caller; an agent id supplied by the model is not identity.
    let agents = state
    .sessions
    .live_agent_entries(&registration.owner)
    .map_err(|error| json!({"jsonrpc":"2.0", "id": id, "error": {"code": -32603, "message": error.message}}))?
    .into_iter()
    .map(|entry| {
        agent_value(
            &entry.session,
            &entry.runtime,
            broker.depth_of(&entry.session.id),
        )
    })
    .collect::<Vec<_>>();
    let document = json!({"agents": agents});
    let text = serde_json::to_string(&document).map_err(|error| {
    json!({"jsonrpc":"2.0", "id": id, "error": {"code": -32603, "message": format!("Could not encode agent roster: {error}")}})
})?;
    Ok(Some(json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{"type": "text", "text": text}],
            "structuredContent": document,
            "isError": false,
        },
    })))
}
