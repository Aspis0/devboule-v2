//! The agent-to-agent message tool: one call, one delivery.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::mcp_broker::caller::{caller_conn, McpCaller};
use crate::mcp_broker::dispatch::rpc_error;
use crate::mcp_broker::RegisteredSession;
use crate::server::ServerState;

pub(in crate::mcp_broker) fn send(
    state: &Arc<ServerState>,
    registration: &RegisteredSession,
    caller: McpCaller,
    id: Value,
    message: &Value,
) -> Result<Option<Value>, Value> {
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
        // A miss that is one of the caller's own children, closed, says so
        // with its reason; the sentence promises no reopen, and this road
        // holds none — nothing here writes a row or starts a session.
        let refusal = state.sessions.closed_child_refusal(
            &registration.owner,
            &registration.session_id,
            to_agent,
        );
        return Ok(Some(rpc_error(
            id,
            -32602,
            &refusal.unwrap_or_else(|| "target agent not found".to_string()),
        )));
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
}
