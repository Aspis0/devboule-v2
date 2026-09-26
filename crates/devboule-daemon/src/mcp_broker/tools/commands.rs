//! The agent-command tools: cancel, the pending-permission list, one child's
//! status. Each handler parses its one argument (or deliberately none), asks
//! the registry for the scoped fact, and shapes the wire reply; identity is
//! always the bearer's session, never the argument.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::mcp_broker::caller::{audit_mcp_tool, McpCaller};
use crate::mcp_broker::dispatch::rpc_error;
use crate::mcp_broker::RegisteredSession;
use crate::server::ServerState;
use crate::session::CancelOutcome;

/// The caller names one of its children: the id or display name the other
/// child tools take, never identity — that is the bearer's registration.
fn agent_id(message: &Value) -> Option<&str> {
    message
        .pointer("/params/arguments/agentId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
}

/// `devboule_cancel_agent`: interrupt the current turn of one of the caller's
/// own live children and keep the child. `success` is the measured answer —
/// true only when the turn was running and stopped within the wait; false when
/// nothing was running or nothing acknowledged, and the text says which.
pub(in crate::mcp_broker) fn cancel(
    state: &Arc<ServerState>,
    registration: &RegisteredSession,
    caller: McpCaller,
    id: Value,
    message: &Value,
) -> Result<Option<Value>, Value> {
    let Some(target) = agent_id(message) else {
        return Ok(Some(rpc_error(id, -32602, "agentId is required")));
    };
    // Audited like stop and close: the refusal the caller reads and the row
    // the owner reads are the same act, and a peer's denial was already
    // audited by the door before this body ran.
    let audit = |outcome_label: &str| {
        audit_mcp_tool(
            state,
            &caller,
            crate::provider_catalog::MCP_CANCEL_AGENT_TOOL,
            &registration.session_id,
            outcome_label,
        );
    };
    match state
        .sessions
        .interrupt_agent_child(&registration.session_id, target)
    {
        Ok(outcome) => {
            audit("ok");
            let (success, text) = match outcome {
                CancelOutcome::Interrupted => (true, "interrupted"),
                CancelOutcome::NotRunning => (false, "no turn was running"),
                CancelOutcome::TurnStillRunning => (false, "the turn did not stop in time"),
            };
            Ok(Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [{"type": "text", "text": text}],
                    "structuredContent": {"success": success},
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

/// `devboule_list_pending_permissions`: every card the caller's own live
/// children are parked on. Deliberately no arguments — the bearer names the
/// caller, and an argument could only try to widen the list.
pub(in crate::mcp_broker) fn list_pending(
    state: &Arc<ServerState>,
    registration: &RegisteredSession,
    id: Value,
) -> Result<Option<Value>, Value> {
    let cards = match state
        .sessions
        .list_child_permission_cards(&registration.session_id)
    {
        Ok(cards) => cards,
        Err(error) => {
            return Ok(Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [{"type": "text", "text": error.message}],
                    "isError": true,
                },
            })))
        }
    };
    let document = json!({ "permissions": cards });
    let text = serde_json::to_string(&document).map_err(|error| {
        json!({"jsonrpc":"2.0", "id": id, "error": {"code": -32603, "message": format!("Could not encode the pending list: {error}")}})
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

/// `devboule_get_agent_status`: one child's snapshot — live from its runtime,
/// a closed child from its stored row, anything else not found.
pub(in crate::mcp_broker) fn status(
    state: &Arc<ServerState>,
    registration: &RegisteredSession,
    id: Value,
    message: &Value,
) -> Result<Option<Value>, Value> {
    let Some(target) = agent_id(message) else {
        return Ok(Some(rpc_error(id, -32602, "agentId is required")));
    };
    match state
        .sessions
        .agent_status_snapshot(state, &registration.session_id, target)
    {
        Ok(document) => {
            let text = serde_json::to_string(&document).map_err(|error| {
                json!({"jsonrpc":"2.0", "id": id, "error": {"code": -32603, "message": format!("Could not encode the agent status: {error}")}})
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
        Err(error) => Ok(Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "content": [{"type": "text", "text": error.message}],
                "isError": true,
            },
        }))),
    }
}
