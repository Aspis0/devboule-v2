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
    // A named device sends across the peer link; only an absent property
    // keeps today's local road. An explicit null, an empty id, or a
    // wrong-typed id is a caller error, never a quiet local send: the message
    // was meant for another machine.
    let device_id = match message.pointer("/params/arguments/deviceId") {
        None => None,
        Some(Value::String(value)) if !value.is_empty() => Some(value.as_str()),
        Some(_) => {
            return Ok(Some(rpc_error(
                id,
                -32602,
                "deviceId must be a non-empty paired-device id from devboule_list_devices; \
                 omit it to message a session on this machine",
            )))
        }
    };
    if let Some(device_id) = device_id {
        return super::messaging_peer::send(state, registration, id, device_id, to_agent, text);
    }
    let internal_conn = caller_conn(state, &caller);
    let lookup_error = |error: devboule_protocol::WireError| json!({"jsonrpc":"2.0", "id": id, "error": {"code": -32603, "message": error.message}});
    let mut target_id = state
        .sessions
        .live_agent_id(&registration.owner, to_agent)
        .map_err(lookup_error)?;
    if target_id.is_none() {
        // A stopped or closed child of the caller's own, named by id: the
        // message resumes its provider session and then goes to it. Only the
        // creator's message does this, and only for its own child.
        let stored = state
            .sessions
            .own_stored_child_id(&registration.session_id, &registration.owner, to_agent)
            .map_err(lookup_error)?;
        if let Some(child) = stored {
            if let Err(error) =
                state
                    .sessions
                    .resume(state, &child, &registration.owner, &internal_conn)
            {
                return Ok(Some(rpc_error(id, -32602, &error.message)));
            }
            // The child's own transcript names who started it again; a refused
            // notice does not stop the message, which is the point of the send.
            let _ = state.sessions.note_resumed_by(
                &registration.owner,
                &child,
                &registration.session_id,
            );
            target_id = state
                .sessions
                .live_agent_id(&registration.owner, &child)
                .map_err(lookup_error)?;
        }
    }
    let Some(target_id) = target_id else {
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
    match state.sessions.agent_message_send(
        &registration.session_id,
        &target_id,
        text,
        &registration.owner,
        &internal_conn,
    ) {
        Ok(()) => {
            // The result names the recipient the way the roster does, so the
            // app can say who the message went to without a session id. A child
            // whose finish wakes this caller also says so: the caller ends its
            // turn instead of polling the child.
            let mut document = json!({
                "state": "accepted",
                "displayName": crate::session::roster_title(&target.session),
            });
            if state
                .sessions
                .child_wakes_creator(&registration.session_id, &target_id)
            {
                document["guidance"] = json!(crate::provider_catalog::AGENT_WAKE_SENTENCE);
            }
            Ok(Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [{"type": "text", "text": document.to_string()}],
                    "structuredContent": document,
                    "isError": false,
                },
            })))
        }
        Err(error) => {
            // The lookup found the child and the delivery did not: a close
            // took it in between. The ordered sentence says what happened
            // there; a child that is still present keeps its own error.
            let refusal = state.sessions.closed_child_refusal(
                &registration.owner,
                &registration.session_id,
                to_agent,
            );
            let message = refusal.unwrap_or(error.message);
            Ok(Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [{"type": "text", "text": message}],
                    "isError": true,
                },
            })))
        }
    }
}
