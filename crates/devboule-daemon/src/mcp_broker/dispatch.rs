//! The JSON-RPC router: protocol methods, the door, and the tools/call policy gate.

use serde_json::{json, Value};
use std::sync::Arc;

use devboule_protocol::ToolPolicyEntry;

use crate::provider_catalog::ToolOverlay;
use crate::server::ServerState;

use super::caller::{audit_mcp_tool, mcp_peer_door, resolve_mcp_caller, McpCaller};
use super::tools;
use super::tools::always_card::{always_card, ask_peer_command, AlwaysCard};
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
    let card_wait = message
        .pointer("/params/name")
        .and_then(Value::as_str)
        .is_some_and(crate::provider_catalog::mcp_tool_waits_on_card);
    let _call_scope = if method == "tools/call" && card_wait {
        match broker.begin_mcp_call(&registration.session_id, &id) {
            Ok(scope) => Some(scope),
            Err(()) => {
                return Ok(Some(rpc_error(
                    id,
                    -32600,
                    "A request with this id is already in flight.",
                )))
            }
        }
    } else {
        None
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
        "notifications/initialized" => Ok(None),
        "notifications/cancelled" => {
            if let Some(request_id) = message.pointer("/params/requestId") {
                broker.cancel_mcp_call(registration, request_id);
            }
            Ok(None)
        }
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
            let mut tools = enabled_tool_list(
                crate::provider_catalog::MCP_BROKER_TOOLS,
                policy.as_ref(),
                registration.overlay.clone(),
            );
            // A paired device is shown what it may call and nothing else: the
            // same door that would refuse the call decides the listing.
            let caller = resolve_mcp_caller(state, &registration.session_id);
            if matches!(caller, McpCaller::Peer { .. }) {
                tools.retain(|tool| {
                    tool.get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|name| {
                            mcp_peer_door(&caller, Some(name), &Value::Null).is_none()
                        })
                });
            }
            Ok(Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {"tools": tools},
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
                    // A name with no row is whatever the caller sent: the row
                    // records that it was refused, never the text.
                    let action = if crate::peer_policy::mcp_tool_wire(tool).is_some() {
                        tool
                    } else {
                        crate::peer_policy::UNLISTED_TOOL
                    };
                    audit_mcp_tool(state, &caller, action, &registration.session_id, "denied");
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
                    if tool_name == crate::provider_catalog::MCP_ARCHIVE_WORKSPACE_TOOL {
                        audit_mcp_tool(
                            state,
                            &caller,
                            tool_name,
                            &registration.session_id,
                            "denied",
                        );
                    }
                    return Ok(Some(rpc_error(id, -32601, reason)));
                }
                if super::current_mcp_call_cancelled(&registration.session_id) {
                    audit_mcp_tool(
                        state,
                        &caller,
                        tool_name,
                        &registration.session_id,
                        "cancelled",
                    );
                    return Ok(Some(rpc_error(id, -32800, "Request cancelled")));
                }
                // The overlay is folded into the same refusal, above: one
                // sentence for both rules (`S5` §2).
                let arguments = message.pointer("/params/arguments").unwrap_or(&Value::Null);
                match always_card(tool_name, arguments) {
                    // A command bound for another machine asks the person in
                    // every mode, before anything is dialled.
                    Some(AlwaysCard::PairedDevice(command)) => {
                        let audit = |decision: &str| {
                            audit_mcp_tool(
                                state,
                                &caller,
                                tool_name,
                                &registration.session_id,
                                &command.audit_outcome(decision),
                            );
                        };
                        if let Err(reply) = ask_peer_command(state, registration, &id, &command) {
                            audit("denied");
                            return Ok(Some(reply));
                        }
                        // The person may have answered and the agent withdrawn
                        // the call since: a call that was cancelled sends nothing.
                        if super::current_mcp_call_withdrawn(&registration.session_id) {
                            audit("cancelled");
                            return Ok(Some(rpc_error(id, -32800, "Request cancelled")));
                        }
                        audit("approved");
                    }
                    // The saved-login card names the site and the login the
                    // host's preview finds, so the tool body raises it.
                    Some(AlwaysCard::SavedLogin) | None => {}
                }
            }
            if tool_name == Some(crate::provider_catalog::MCP_SEND_MESSAGE_TOOL) {
                tools::messaging::send(state, registration, caller, id, message)
            } else if tool_name == Some(crate::provider_catalog::MCP_LIST_PROFILES_TOOL) {
                // Deliberately do not read params.arguments, like the roster
                // tool: the list is the human's, and the bearer is the only
                // identity this call needs.
                Ok(Some(tools::creation::profile::list_profiles(
                    &state.agent_profiles,
                    &id,
                )))
            } else if tool_name == Some(crate::provider_catalog::MCP_CREATE_AGENT_TOOL) {
                let result = tools::creation::run::create_agent_tool(
                    state,
                    broker,
                    caller,
                    registration,
                    id,
                    message,
                );
                let outcome = result
                    .as_ref()
                    .ok()
                    .and_then(Option::as_ref)
                    .map(|reply| {
                        if reply.get("error").is_some()
                            || reply.pointer("/result/isError") == Some(&Value::Bool(true))
                        {
                            "denied"
                        } else {
                            "ok"
                        }
                    })
                    .unwrap_or("denied");
                let caller = resolve_mcp_caller(state, &registration.session_id);
                audit_mcp_tool(
                    state,
                    &caller,
                    crate::provider_catalog::MCP_CREATE_AGENT_TOOL,
                    &registration.session_id,
                    outcome,
                );
                result
            } else if tool_name == Some(crate::provider_catalog::MCP_ANSWER_PERMISSION_TOOL) {
                tools::permissions::answer(state, registration, caller, id, message)
            } else if tool_name == Some(crate::provider_catalog::MCP_SET_AGENT_PROFILE_TOOL) {
                tools::agents::set_profile(state, registration, caller, id, message)
            } else if tool_name == Some(crate::provider_catalog::MCP_ACTIVITY_TOOL) {
                tools::agents::activity(state, registration, id, message)
            } else if tool_name == Some(crate::provider_catalog::MCP_STOP_AGENT_TOOL)
                || tool_name == Some(crate::provider_catalog::MCP_CLOSE_AGENT_TOOL)
            {
                tools::agents::stop_or_close(state, registration, caller, id, message, tool_name)
            } else if tool_name == Some(crate::provider_catalog::MCP_CANCEL_AGENT_TOOL) {
                tools::commands::cancel(state, registration, caller, id, message)
            } else if tool_name == Some(crate::provider_catalog::MCP_LIST_PENDING_PERMISSIONS_TOOL)
            {
                tools::commands::list_pending(state, registration, id)
            } else if tool_name == Some(crate::provider_catalog::MCP_GET_AGENT_STATUS_TOOL) {
                tools::commands::status(state, registration, id, message)
            } else if tool_name == Some(crate::provider_catalog::MCP_LIST_DEVICES_TOOL) {
                tools::peers::list_devices(state, registration, id)
            } else if tool_name == Some(crate::provider_catalog::MCP_LIST_PEER_AGENTS_TOOL) {
                tools::peers::list_peer_agents(state, registration, caller, id, message)
            } else if tool_name == Some(crate::provider_catalog::MCP_NEIGHBORHOOD_TOOL) {
                tools::graph::neighborhood(state, registration, id, message)
            } else if tool_name == Some(crate::provider_catalog::MCP_IMPORTS_TOOL) {
                tools::graph::imports(state, registration, id, message)
            } else if tool_name == Some(crate::provider_catalog::MCP_IMPORTERS_TOOL) {
                tools::graph::importers(state, registration, id, message)
            } else if tool_name == Some(crate::provider_catalog::MCP_ORACLE_SEARCH_TOOL) {
                tools::graph::oracle_search(state, registration, id, message)
            } else if tool_name == Some(crate::provider_catalog::MCP_LIST_WORKSPACES_TOOL) {
                tools::workspaces::list(state, registration, id)
            } else if tool_name == Some(crate::provider_catalog::MCP_CREATE_WORKSPACE_TOOL) {
                tools::workspaces::create(state, broker, caller, registration, id, message)
            } else if tool_name == Some(crate::provider_catalog::MCP_FILE_COLLISIONS_TOOL) {
                tools::collisions::collisions(state, registration, id, message)
            } else if tool_name == Some(crate::provider_catalog::MCP_LIST_TERMINALS_TOOL) {
                tools::terminals::list(state, registration, caller, id)
            } else if tool_name == Some(crate::provider_catalog::MCP_CAPTURE_TERMINAL_TOOL) {
                tools::terminals::capture(state, registration, caller, id, message)
            } else if tool_name == Some(crate::provider_catalog::MCP_ARCHIVE_WORKSPACE_TOOL) {
                tools::archive::archive(state, broker, caller, registration, id, message)
            } else if tool_name == Some(crate::provider_catalog::MCP_CREATE_TERMINAL_TOOL) {
                tools::terminal_writes::create(state, broker, registration, caller, id, message)
            } else if tool_name == Some(crate::provider_catalog::MCP_SEND_TERMINAL_KEYS_TOOL) {
                tools::terminal_writes::send_keys(state, broker, registration, caller, id, message)
            } else if tool_name == Some(crate::provider_catalog::MCP_KILL_TERMINAL_TOOL) {
                tools::terminal_writes::kill(state, broker, registration, caller, id, message)
            } else if tool_name == Some(crate::provider_catalog::MCP_PROCESS_OWNER_TOOL) {
                tools::processes::owner(state, registration, caller, id, message)
            } else if tool_name == Some(crate::provider_catalog::MCP_SESSION_PROCESSES_TOOL) {
                tools::processes::list(state, registration, caller, id, message)
            } else if tool_name == Some(crate::provider_catalog::MCP_CLEANUP_PROCESSES_TOOL) {
                tools::process_cleanup::cleanup(state, broker, registration, caller, id, message)
            } else if tool_name == Some(crate::provider_catalog::MCP_CI_WATCH_TOOL) {
                tools::ci_watch::call(state, broker, registration, caller, id, message)
            } else if tool_name == Some(crate::provider_catalog::MCP_BROWSER_FILL_LOGIN_TOOL) {
                tools::browser_login::call(state, broker, registration, caller, id, message)
            } else if tool_name.is_some_and(tools::browser_commands::serves) {
                tools::browser_tools::call(state, registration, caller, id, message)
            } else if tool_name != Some(crate::provider_catalog::MCP_ROSTER_TOOL) {
                Ok(Some(rpc_error(id, -32601, "Unknown tool")))
            } else {
                tools::agents::roster(state, broker, registration, id)
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
            let input_schema = if tools::browser_commands::serves(name) {
                // The lane's schemas in one table, spelled out where the
                // declaration is (`tools::browser_commands`) rather than as
                // one arm per tool here. That same table is what the daemon
                // checks a call against, so the two cannot disagree.
                tools::browser_commands::schema_for(name).unwrap_or(Value::Null)
            } else if *name == crate::provider_catalog::MCP_SEND_MESSAGE_TOOL {
                json!({
                    "type": "object",
                    "properties": {
                        "to_agent": {"type": "string"},
                        "text": {"type": "string"},
                        // Optional, and only this tool spells it: a local
                        // target and a paired device's target are two
                        // different lookups, and the value that decides
                        // which one runs is the caller's own device id.
                        "deviceId": {
                            "type": "string",
                            "description": "A paired device's id from devboule_list_devices; to_agent is then a session id devboule_list_peer_agents answered for that device.",
                        },
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
            } else if *name == crate::provider_catalog::MCP_LIST_WORKSPACES_TOOL {
                // Spelled in its own arm like the device list: a parameterless
                // tool's schema is a claim about the tool, and nothing walks
                // this table to keep a silent default true.
                json!({"type": "object", "properties": {}, "additionalProperties": false})
            } else if *name == crate::provider_catalog::MCP_CREATE_WORKSPACE_TOOL {
                // Closed like the create-agent schema it rhymes with: the
                // project is never a parameter — identity is imposed from the
                // bearer's registration — and no path is accepted at all.
                json!({
                    "type": "object",
                    "properties": {
                        "isolation": {"type": "string", "enum": ["local", "worktree"], "description": "The workspace shape: the project folder itself, or a new git worktree beside it."},
                        "name": {"type": "string", "description": "The workspace title."},
                        "branch": {"type": "string", "description": "The worktree branch. Worktree only."},
                        "path": {"type": "string", "description": "Not accepted: workspaces are created inside your project, never at an agent-named directory."},
                        "projectId": {"type": "string", "description": "Must be your own project, when given."},
                    },
                    "required": ["isolation"],
                    "additionalProperties": false,
                })
            } else if *name == crate::provider_catalog::MCP_ARCHIVE_WORKSPACE_TOOL {
                json!({
                    "type": "object",
                    "properties": {"workspaceId": {"type": "string"}},
                    "required": ["workspaceId"],
                    "additionalProperties": false,
                })
            } else if *name == crate::provider_catalog::MCP_FILE_COLLISIONS_TOOL {
                crate::provider_catalog::file_collisions_input_schema()
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
            } else if *name == crate::provider_catalog::MCP_CANCEL_AGENT_TOOL
                || *name == crate::provider_catalog::MCP_GET_AGENT_STATUS_TOOL
            {
                // One closed document for both readers of one child: they
                // differ in what they answer, not in what they accept.
                crate::provider_catalog::agent_id_input_schema()
            } else if *name == crate::provider_catalog::MCP_LIST_DEVICES_TOOL
                || *name == crate::provider_catalog::MCP_LIST_PENDING_PERMISSIONS_TOOL
                || *name == crate::provider_catalog::MCP_LIST_TERMINALS_TOOL
            {
                // Spelled in their own arm rather than left to the default arm
                // at the bottom: a parameterless tool's schema is a claim
                // about the tool, and nothing walks this table to keep a
                // silent default true.
                json!({"type": "object", "properties": {}, "additionalProperties": false})
            } else if *name == crate::provider_catalog::MCP_CAPTURE_TERMINAL_TOOL {
                crate::provider_catalog::terminal_capture_input_schema()
            } else if *name == crate::provider_catalog::MCP_CREATE_TERMINAL_TOOL {
                // Closed like the create-workspace schema it rhymes with: the
                // workspace is never a parameter — the caller's own row
                // decides it — so the only thing to accept is the optional
                // title.
                json!({
                    "type": "object",
                    "properties": {
                        "name": {"type": "string", "description": "The terminal's display name, at most 60 characters. Empty means untitled."},
                    },
                    "additionalProperties": false,
                })
            } else if *name == crate::provider_catalog::MCP_SEND_TERMINAL_KEYS_TOOL {
                // Closed and stated in full, because the token list is the
                // tool's whole vocabulary: what is not named here is refused
                // by name at the parser.
                json!({
                    "type": "object",
                    "properties": {
                        "terminalId": {"type": "string", "description": "The id of one terminal from devboule_list_terminals."},
                        "keys": {"type": "string", "description": "Literal text, or one named key: Enter, Tab, Escape, Space, BSpace, C-c, C-d, C-z, C-l, C-a, C-e. At most 64 KiB."},
                        "literal": {"type": "boolean", "description": "Write keys exactly as typed instead of resolving a named key. Default false."},
                    },
                    "required": ["terminalId", "keys"],
                    "additionalProperties": false,
                })
            } else if *name == crate::provider_catalog::MCP_KILL_TERMINAL_TOOL {
                json!({
                    "type": "object",
                    "properties": {
                        "terminalId": {"type": "string", "description": "The id of one terminal from devboule_list_terminals."},
                    },
                    "required": ["terminalId"],
                    "additionalProperties": false,
                })
            } else if *name == crate::provider_catalog::MCP_CI_WATCH_TOOL {
                // Exactly one key at runtime; the schema states both as
                // optional because a JSON schema cannot express xor.
                json!({
                    "type": "object",
                    "properties": {
                        "sha": {"type": "string", "description": "The full 40-character commit id to watch. Exactly one of sha and branch."},
                        "branch": {"type": "string", "description": "A branch whose remote head is resolved now and followed. Exactly one of sha and branch."},
                        "repo": {"type": "string", "description": "owner/repo. Default: the repository the workspace's origin remote names."},
                        "retryInfra": {"type": "boolean", "description": "Re-run the failed jobs once if every failure is infrastructure. Shows a permission card."},
                    },
                    "additionalProperties": false,
                })
            } else if *name == crate::provider_catalog::MCP_ACTIVITY_TOOL {
                crate::provider_catalog::agent_activity_input_schema()
            } else if *name == crate::provider_catalog::MCP_STOP_AGENT_TOOL
                || *name == crate::provider_catalog::MCP_CLOSE_AGENT_TOOL
            {
                // One closed document for both verbs: they differ in what
                // they do, not in what they accept.
                crate::provider_catalog::agent_end_input_schema()
            } else if *name == crate::provider_catalog::MCP_PROCESS_OWNER_TOOL {
                // Exactly one key at runtime; the schema states both as
                // optional because a JSON schema cannot express xor.
                json!({
                    "type": "object",
                    "properties": {
                        "port": {"type": "integer", "minimum": 0, "maximum": 65535, "description": "A local TCP port to look up. Exactly one of port and pid."},
                        "pid": {"type": "integer", "minimum": 0, "description": "A process id to look up. Exactly one of port and pid."},
                    },
                    "additionalProperties": false,
                })
            } else if *name == crate::provider_catalog::MCP_SESSION_PROCESSES_TOOL {
                json!({
                    "type": "object",
                    "properties": {
                        "sessionId": {"type": "string", "description": "Only your own session id is accepted; omit it for the calling session."},
                    },
                    "additionalProperties": false,
                })
            } else if *name == crate::provider_catalog::MCP_CLEANUP_PROCESSES_TOOL {
                json!({
                    "type": "object",
                    "properties": {
                        "graceMs": {"type": "integer", "minimum": 0, "maximum": 30000, "description": "Grace period for the graceful phase, in milliseconds. Default 2000."},
                    },
                    "additionalProperties": false,
                })
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

/// A refusal an agent reads: the sentence, and never a session id.
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
