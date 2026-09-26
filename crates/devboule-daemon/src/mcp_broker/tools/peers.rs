//! The device roster and the dialled peer-agent roster.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::mcp_broker::caller::{audit_mcp_tool, McpCaller};
use crate::mcp_broker::dispatch::rpc_error;
use crate::mcp_broker::RegisteredSession;
use crate::server::ServerState;

pub(in crate::mcp_broker) fn list_devices(
    state: &Arc<ServerState>,
    registration: &RegisteredSession,
    id: Value,
) -> Result<Option<Value>, Value> {
    // Deliberately do not read params.arguments, like the roster
    // tool: the calling session's own user scopes the list, and
    // the answer never leaves this process — no dial, ever.
    let document = match crate::mcp_device_roster::list_devices_document(state, &registration.owner)
    {
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
}

pub(in crate::mcp_broker) fn list_peer_agents(
    state: &Arc<ServerState>,
    registration: &RegisteredSession,
    caller: McpCaller,
    id: Value,
    message: &Value,
) -> Result<Option<Value>, Value> {
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
    match crate::mcp_peer_agents::list_peer_agents(state, &registration.owner, device_id) {
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
}
