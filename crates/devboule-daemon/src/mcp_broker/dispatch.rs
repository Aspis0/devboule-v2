//! The JSON-RPC router: protocol methods, the door, and the tools/call policy gate.

use serde_json::{json, Value};
use std::sync::Arc;

use devboule_protocol::{PermissionOutcome, ToolPolicyEntry};

use crate::provider_catalog::ToolOverlay;
use crate::server::ServerState;

use super::caller::{audit_mcp_tool, caller_conn, mcp_peer_door, resolve_mcp_caller};
use super::tools::agents::{agent_value, parse_activity_arguments};
use super::tools::creation::profile::{list_profiles, resolve_profile_for_move};
use super::tools::creation::request::AgentCreateRequest;
use super::tools::creation::run::create_agent;
use super::tools::graph::{project_graph_arguments, project_graph_reply};
use super::{McpBroker, RegisteredSession, MCP_SERVER_NAME};

pub(super) fn handle_rpc(
    state: &Arc<ServerState>,
    broker: &McpBroker,
    registration: &RegisteredSession,
    message: &Value,
) -> Result<Option<Value>, Value> {
    let id = message.get("id").cloned().unwrap_or(Value::Null);
    let method = message.get("method").and_then(Value::as_str);
    let Some(method) = method else {
        return Ok(Some(rpc_error(id, -32600, "Invalid Request")));
    };
    match method {
        "initialize" => Ok(Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": message
                    .pointer("/params/protocolVersion")
                    .cloned()
                    .unwrap_or_else(|| json!("2025-03-26")),
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": MCP_SERVER_NAME, "version": env!("CARGO_PKG_VERSION")},
            },
        }))),
        "notifications/initialized" | "notifications/cancelled" => Ok(None),
        "ping" => Ok(Some(json!({"jsonrpc": "2.0", "id": id, "result": {}}))),
        "server/discover" => Ok(Some(rpc_error(
            id,
            -32601,
            "Method not found: server/discover",
        ))),
        "tools/list" => {
            // An authenticated tools/list is the broker's proof that this
            // provider has connected with this session's Bearer.
            broker.mark_broker_ready(registration);
            let policy = state.tool_policy.get(registration.provider_id.as_deref());
            Ok(Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {"tools": enabled_tool_list(
                    crate::provider_catalog::MCP_BROKER_TOOLS,
                    policy.as_ref(),
                    registration.overlay.clone(),
                )},
            })))
        }
        "tools/call" => {
            let tool_name = message.pointer("/params/name").and_then(Value::as_str);
            // The origin door runs before every other guard: who is calling is
            // resolved once from the registry row, and a peer is judged with the
            // same `peer_allows` the wire dispatcher uses before anything is
            // touched. Local callers pass through untouched.
            let caller = resolve_mcp_caller(state, &registration.session_id);
            if let Some(refusal) = mcp_peer_door(&caller, tool_name, &id) {
                if let Some(tool) = tool_name {
                    audit_mcp_tool(state, &caller, tool, &registration.session_id, "denied");
                }
                return Ok(Some(refusal));
            }
            // The policy guard runs before the name check, so a disabled tool
            // is refused for the reason that actually applies and an
            // unserved name cannot be probed past the policy.
            let policy = state.tool_policy.get(registration.provider_id.as_deref());
            if let Some(tool_name) = tool_name {
                if let Some(reason) =
                    tool_call_refusal(policy.as_ref(), &registration.overlay, tool_name)
                {
                    return Ok(Some(rpc_error(id, -32601, reason)));
                }
                // The overlay is folded into the same refusal, above: one
                // sentence for both rules (`S5` §2).
            }
            if tool_name == Some(crate::provider_catalog::MCP_SEND_MESSAGE_TOOL) {
                let to_agent = message
                    .pointer("/params/arguments/to_agent")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty());
                let text = message
                    .pointer("/params/arguments/text")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty());
                let (Some(to_agent), Some(text)) = (to_agent, text) else {
                    return Ok(Some(rpc_error(
                        id,
                        -32602,
                        "to_agent and text are required",
                    )));
                };
                let target = state
                    .sessions
                    .live_agent_entries(&registration.owner)
                    .map_err(|error| {
                        json!({"jsonrpc":"2.0", "id": id, "error": {"code": -32603, "message": error.message}})
                    })?
                    .into_iter()
                    .find(|entry| entry.session.id == to_agent || entry.session.title == to_agent);
                let Some(target) = target else {
                    return Ok(Some(rpc_error(id, -32602, "target agent not found")));
                };
                let internal_conn = caller_conn(state, &caller);
                match state.sessions.agent_message_send(
                    &registration.session_id,
                    &target.session.id,
                    text,
                    &registration.owner,
                    &internal_conn,
                ) {
                    Ok(()) => Ok(Some(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [{"type": "text", "text": "accepted"}],
                            "structuredContent": {"state": "accepted"},
                            "isError": false,
                        },
                    }))),
                    Err(error) => Ok(Some(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [{"type": "text", "text": error.message}],
                            "isError": true,
                        },
                    }))),
                }
            } else if tool_name == Some(crate::provider_catalog::MCP_LIST_PROFILES_TOOL) {
                // Deliberately do not read params.arguments, like the roster
                // tool: the list is the human's, and the bearer is the only
                // identity this call needs.
                Ok(Some(list_profiles(&state.agent_profiles, &id)))
            } else if tool_name == Some(crate::provider_catalog::MCP_CREATE_AGENT_TOOL) {
                let arguments = message
                    .pointer("/params/arguments")
                    .cloned()
                    .unwrap_or(Value::Null);
                match AgentCreateRequest::parse(&arguments) {
                    Ok(request) => Ok(Some(create_agent(
                        state,
                        broker,
                        &caller,
                        registration,
                        &id,
                        request,
                    ))),
                    Err(message) => Ok(Some(rpc_error(id, -32602, &message))),
                }
            } else if tool_name == Some(crate::provider_catalog::MCP_ANSWER_PERMISSION_TOOL) {
                // Identity is the bearer, never the arguments: the card this
                // answers must belong to a child of the session that called.
                let card_id = message
                    .pointer("/params/arguments/cardId")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty());
                let outcome_str = message
                    .pointer("/params/arguments/outcome")
                    .and_then(Value::as_str);
                let (Some(card_id), Some(outcome_str)) = (card_id, outcome_str) else {
                    return Ok(Some(rpc_error(
                        id,
                        -32602,
                        "cardId and outcome are required",
                    )));
                };
                // The closed outcome table, enforced again at the door: an
                // agent's allow is one-shot or nothing.
                let outcome = match outcome_str {
                    "allow_once" => PermissionOutcome::AllowOnce,
                    "deny" => PermissionOutcome::Deny,
                    other => {
                        return Ok(Some(rpc_error(
                            id,
                            -32602,
                            &format!("unknown outcome {other:?}; use allow_once or deny"),
                        )));
                    }
                };
                let result = state.sessions.answer_child_permission(
                    &registration.session_id,
                    card_id,
                    outcome,
                    &|device_id| state.peer_caps(device_id),
                );
                // The audit names who called: the caller's device and role for a
                // peer (`resolve_mcp_caller` above), never `"local"` for one.
                let audit = |outcome_label: &str| {
                    audit_mcp_tool(
                        state,
                        &caller,
                        crate::provider_catalog::MCP_ANSWER_PERMISSION_TOOL,
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
                                "content": [{"type": "text", "text": "answered"}],
                                "structuredContent": {"state": "answered", "cardId": card_id},
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
            } else if tool_name == Some(crate::provider_catalog::MCP_SET_AGENT_PROFILE_TOOL) {
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
            } else if tool_name == Some(crate::provider_catalog::MCP_ACTIVITY_TOOL) {
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
            } else if tool_name == Some(crate::provider_catalog::MCP_STOP_AGENT_TOOL)
                || tool_name == Some(crate::provider_catalog::MCP_CLOSE_AGENT_TOOL)
            {
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
            } else if tool_name == Some(crate::provider_catalog::MCP_LIST_DEVICES_TOOL) {
                // Deliberately do not read params.arguments, like the roster
                // tool: the calling session's own user scopes the list, and
                // the answer never leaves this process — no dial, ever.
                let document = match crate::mcp_device_roster::list_devices_document(
                    state,
                    &registration.owner,
                ) {
                    Ok(document) => document,
                    Err(message) => {
                        return Ok(Some(json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": {"code": -32603, "message": message},
                        })))
                    }
                };
                let text = serde_json::to_string(&document).map_err(|error| {
                    json!({"jsonrpc":"2.0", "id": id, "error": {"code": -32603, "message": format!("Could not encode device list: {error}")}})
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
            } else if tool_name == Some(crate::provider_catalog::MCP_LIST_PEER_AGENTS_TOOL) {
                // One dial, one device, named by argument; the calling
                // session's own rows decide which names are dialable.
                let device_id = message
                    .pointer("/params/arguments/deviceId")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty());
                let Some(device_id) = device_id else {
                    return Ok(Some(rpc_error(id, -32602, "deviceId is required")));
                };
                // This is the tool that dials other machines and comes back
                // with their roster, so every outcome is audited with the
                // caller, like the answer and move tools — and the failure
                // carries its cause (`denied`, `unscoped`, `failed`), because
                // a scope refusal is a different fact from a dead dial.
                let audit = |outcome_label: &str| {
                    audit_mcp_tool(
                        state,
                        &caller,
                        crate::provider_catalog::MCP_LIST_PEER_AGENTS_TOOL,
                        &registration.session_id,
                        outcome_label,
                    );
                };
                match crate::mcp_peer_agents::list_peer_agents(
                    state,
                    &registration.owner,
                    device_id,
                ) {
                    Ok(document) => {
                        audit("ok");
                        let text = serde_json::to_string(&document).map_err(|error| {
                            json!({"jsonrpc":"2.0", "id": id, "error": {"code": -32603, "message": format!("Could not encode peer agents: {error}")}})
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
                    Err(error) => {
                        audit(error.outcome);
                        Ok(Some(rpc_error(id, error.code, &error.sentence)))
                    }
                }
            } else if tool_name == Some(crate::provider_catalog::MCP_NEIGHBORHOOD_TOOL) {
                // The caller's own workspace decides the graph; the bearer is
                // the identity, and no argument names a path.
                project_graph_reply(
                    &id,
                    crate::mcp_project_graph::neighborhood(
                        state,
                        &registration.session_id,
                        &registration.owner,
                        &project_graph_arguments(message),
                    ),
                )
            } else if tool_name == Some(crate::provider_catalog::MCP_IMPORTS_TOOL) {
                project_graph_reply(
                    &id,
                    crate::mcp_project_graph::imports(
                        state,
                        &registration.session_id,
                        &registration.owner,
                        &project_graph_arguments(message),
                    ),
                )
            } else if tool_name == Some(crate::provider_catalog::MCP_IMPORTERS_TOOL) {
                project_graph_reply(
                    &id,
                    crate::mcp_project_graph::importers(
                        state,
                        &registration.session_id,
                        &registration.owner,
                        &project_graph_arguments(message),
                    ),
                )
            } else if tool_name == Some(crate::provider_catalog::MCP_ORACLE_SEARCH_TOOL) {
                // The caller's own workspace decides the search; the bearer
                // is the identity, and no argument names a path.
                project_graph_reply(
                    &id,
                    crate::oracle_forward::search(
                        state,
                        &registration.session_id,
                        &registration.owner,
                        &project_graph_arguments(message),
                    ),
                )
            } else if tool_name != Some(crate::provider_catalog::MCP_ROSTER_TOOL) {
                Ok(Some(rpc_error(id, -32601, "Unknown tool")))
            } else {
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
        }
        _ if message.get("id").is_none() => Ok(None),
        _ => Ok(Some(rpc_error(id, -32601, "Method not found"))),
    }
}

pub(super) fn rpc_error(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// The `tools/list` body for one policy: the catalog minus the tools that
/// policy disables. The catalog is a parameter so the filter can be tested
/// against a tool other than the always-on roster tool.
pub(super) fn enabled_tool_list(
    catalog: &[(&str, &str)],
    policy: Option<&ToolPolicyEntry>,
    overlay: crate::provider_catalog::ToolOverlay,
) -> Vec<Value> {
    catalog
        .iter()
        .filter(|(name, _)| crate::tool_policy::is_tool_enabled(policy, name))
        // The preset's overlay, on top of the stored policy and never instead
        // of it (`S5` §2): a `design` child sees neither the tool it may not
        // call nor any tool its provider's policy already turned off.
        .filter(|(name, _)| overlay.allows(name))
        .map(|(name, description)| {
            let input_schema = if *name == crate::provider_catalog::MCP_SEND_MESSAGE_TOOL {
                json!({
                    "type": "object",
                    "properties": {
                        "to_agent": {"type": "string"},
                        "text": {"type": "string"},
                    },
                    "required": ["to_agent", "text"],
                    "additionalProperties": false,
                })
            } else if *name == crate::provider_catalog::MCP_CREATE_AGENT_TOOL {
                crate::provider_catalog::agent_create_input_schema()
            } else if *name == crate::provider_catalog::MCP_ANSWER_PERMISSION_TOOL {
                // The outcome enum is closed at the schema too: an agent is
                // offered allow_once or deny, and nothing else. There is no
                // allow_always from an agent, ever — the protocol's
                // PermissionOutcome has no such variant, and this table does
                // not name one.
                json!({
                    "type": "object",
                    "properties": {
                        "cardId": {"type": "string"},
                        "outcome": {"type": "string", "enum": ["allow_once", "deny"]},
                    },
                    "required": ["cardId", "outcome"],
                    "additionalProperties": false,
                })
            } else if *name == crate::provider_catalog::MCP_SET_AGENT_PROFILE_TOOL {
                // Closed, like the create schema it rhymes with: the child is
                // named by id or display name, the profile by its name, and
                // nothing a caller could state as identity is offered at all.
                crate::provider_catalog::agent_set_profile_input_schema()
            } else if *name == crate::provider_catalog::MCP_LIST_PEER_AGENTS_TOOL {
                // Closed like its siblings: the device is named by the id
                // `devboule_list_devices` answered, and there is deliberately
                // no scope argument — whose roster answers is the responder's
                // own pairing-user fact.
                crate::provider_catalog::peer_agents_input_schema()
            } else if *name == crate::provider_catalog::MCP_NEIGHBORHOOD_TOOL {
                // Closed like its siblings, and bounded: `depth` is capped at
                // the walk the engine is willing to do (the tools' own parser
                // refuses anything wider), and `kind` is the graph's whole edge
                // vocabulary.
                json!({
                    "type": "object",
                    "properties": {
                        "node": {"type": "string"},
                        "depth": {"type": "integer", "minimum": 1, "maximum": 4},
                        "kind": {"type": "string", "enum": ["IMPORT", "CONTAIN"]},
                    },
                    "required": ["node"],
                    "additionalProperties": false,
                })
            } else if *name == crate::provider_catalog::MCP_IMPORTS_TOOL
                || *name == crate::provider_catalog::MCP_IMPORTERS_TOOL
            {
                // One closed document for both directions: they differ in
                // which way they read the edge, not in what they accept.
                json!({
                    "type": "object",
                    "properties": {"file": {"type": "string"}},
                    "required": ["file"],
                    "additionalProperties": false,
                })
            } else if *name == crate::provider_catalog::MCP_ORACLE_SEARCH_TOOL {
                // Closed and bounded like its siblings: `limit` is clamped to
                // the range the app's own route clamps to, and `root` is
                // deliberately absent — the session's row names the folder.
                json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string"},
                        "limit": {"type": "integer", "minimum": 1, "maximum": 10},
                    },
                    "required": ["query"],
                    "additionalProperties": false,
                })
            } else if *name == crate::provider_catalog::MCP_LIST_DEVICES_TOOL {
                // Spelled in its own arm rather than left to the default arm
                // at the bottom: a parameterless tool's schema is a claim
                // about the tool, and nothing walks this table to keep a
                // silent default true.
                json!({"type": "object", "properties": {}, "additionalProperties": false})
            } else if *name == crate::provider_catalog::MCP_ACTIVITY_TOOL {
                crate::provider_catalog::agent_activity_input_schema()
            } else if *name == crate::provider_catalog::MCP_STOP_AGENT_TOOL
                || *name == crate::provider_catalog::MCP_CLOSE_AGENT_TOOL
            {
                // One closed document for both verbs: they differ in what
                // they do, not in what they accept.
                crate::provider_catalog::agent_end_input_schema()
            } else {
                json!({"type": "object", "properties": {}, "additionalProperties": false})
            };
            json!({
                "name": name,
                "description": description,
                "inputSchema": input_schema,
            })
        })
        .collect()
}

pub(super) fn tool_error(id: &Value, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{"type": "text", "text": message}],
            "isError": true,
        },
    })
}

/// Why a `tools/call` name is refused, if it is (`S5` §2).
///
/// The stored policy first, then the preset's overlay — and one sentence for
/// both, so a `design` child that calls a name it was never offered is refused
/// exactly like one a policy disabled, and cannot probe past the list.
pub(super) fn tool_call_refusal(
    policy: Option<&ToolPolicyEntry>,
    overlay: &ToolOverlay,
    name: &str,
) -> Option<&'static str> {
    if !crate::tool_policy::is_tool_enabled(policy, name) || !overlay.allows(name) {
        return Some("Tool disabled by policy");
    }
    None
}
