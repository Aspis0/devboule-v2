//! The permission-answer tool: the closed outcome table at the broker door.

use std::sync::Arc;

use serde_json::{json, Value};

use devboule_protocol::PermissionOutcome;

use crate::mcp_broker::caller::{audit_mcp_tool, McpCaller};
use crate::mcp_broker::dispatch::rpc_error;
use crate::mcp_broker::RegisteredSession;
use crate::server::ServerState;

pub(in crate::mcp_broker) fn answer(
    state: &Arc<ServerState>,
    registration: &RegisteredSession,
    caller: McpCaller,
    id: Value,
    message: &Value,
) -> Result<Option<Value>, Value> {
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
}
