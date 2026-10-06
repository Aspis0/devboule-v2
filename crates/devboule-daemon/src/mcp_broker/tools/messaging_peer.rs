//! The paired-device half of `devboule_send_message`: one dial, one
//! delivery, and the far daemon's own receipt as the answer.

use std::sync::Arc;

use devboule_protocol::{AgentMessageState, ClientMessage, DaemonMessage, ErrorCode, WireError};
use serde_json::{json, Value};

use crate::mcp_broker::dispatch::rpc_error;
use crate::mcp_broker::RegisteredSession;
use crate::server::{call_peer, ServerState};

/// Send one message to a session the named paired device is running, and
/// answer with that daemon's own receipt: `accepted`, `rejected_absent`, or
/// whichever state names what it did. The device is resolved and attributed
/// by the door the roster tool shares, so both tools answer the same
/// refusals; the dial is the bounded one-request call, with no retry and no
/// queue.
pub(super) fn send(
    state: &Arc<ServerState>,
    registration: &RegisteredSession,
    id: Value,
    device_id: &str,
    to_agent: &str,
    text: &str,
) -> Result<Option<Value>, Value> {
    let row = match crate::mcp_peer_agents::resolve_paired_device(
        state,
        &registration.owner,
        device_id,
    ) {
        Ok(row) => row,
        Err(error) => return Ok(Some(rpc_error(id, error.code, &error.sentence))),
    };
    // MCP frame ids restart with the bridge process, so they cannot dedupe:
    // every call is a new message, traced by the key in its answer.
    let key = fresh_key();
    let request = ClientMessage::AgentMessageSend {
        id: 0,
        from_session: registration.session_id.clone(),
        to_session: to_agent.to_string(),
        text: text.to_string(),
        idempotency_key: Some(key.clone()),
    };
    match call_peer(state, device_id, request) {
        Ok(DaemonMessage::AgentMessageReceipt { state: receipt, .. }) => {
            receipt_result(id, receipt, &key)
        }
        Ok(DaemonMessage::Error(error)) => Ok(Some(rpc_error(
            id,
            -32602,
            &far_error_sentence(&row.display_name, &error, &key),
        ))),
        // The request was written before this reply arrived, so the far daemon may have delivered it.
        Ok(_) => Ok(Some(rpc_error(
            id,
            -32602,
            &format!(
                "{} answered the send with an unexpected message, so the delivery outcome is \
                 unknown — the message may have been delivered; do not resend unless you \
                 accept a duplicate (idempotency key {key}).",
                row.display_name
            ),
        ))),
        // Written but unanswered; every other dial failure happens before the request leaves.
        Err(error) if error.step() == "reply" => Ok(Some(rpc_error(
            id,
            -32602,
            &format!(
                "{} did not answer in time, so the delivery outcome is unknown — the message \
                 may have been delivered; do not resend unless you accept a duplicate \
                 (idempotency key {key}).",
                row.display_name
            ),
        ))),
        Err(error) => Ok(Some(rpc_error(
            id,
            -32602,
            &format!(
                "{} The message was not sent.",
                crate::mcp_peer_agents::dial_error_sentence(&error, &row)
            ),
        ))),
    }
}

/// The retry identity one send carries: a fresh random key per call, spelled
/// in the wire alphabet, so the far daemon's table can hold it and the
/// answer can name it.
fn fresh_key() -> String {
    format!("mcp-peer-send-{}", uuid::Uuid::new_v4())
}

/// The sentence for the far daemon's own error: refusals stay refusals, an
/// idempotency conflict names the key it collided on, and anything else
/// names its code rather than borrowing "refused".
fn far_error_sentence(device: &str, error: &WireError, key: &str) -> String {
    match error.code {
        ErrorCode::Unauthorized | ErrorCode::CapabilityNotSupported => {
            format!("{device} refused the send: {}", error.message)
        }
        ErrorCode::IdempotencyConflict => format!(
            "{device} reported an idempotency conflict for idempotency key {key}: {}",
            error.message
        ),
        _ => {
            let name = serde_json::to_value(error.code)
                .ok()
                .and_then(|code| code.as_str().map(str::to_string))
                .unwrap_or_else(|| format!("{:?}", error.code));
            format!(
                "{device} answered the send with an error ({name}): {}",
                error.message
            )
        }
    }
}

/// The tool result for one far-daemon receipt. The receipt is the answer and
/// the call succeeded, so `isError` is false: `rejected_absent` is the far
/// machine's verdict on the message, not a failure to ask it.
fn receipt_result(
    id: Value,
    receipt: AgentMessageState,
    key: &str,
) -> Result<Option<Value>, Value> {
    let state = match serde_json::to_value(receipt) {
        Ok(state) => state,
        Err(error) => {
            return Ok(Some(rpc_error(
                id,
                -32603,
                &format!("Could not encode the remote receipt: {error}"),
            )))
        }
    };
    // `AgentMessageState` is a unit enum: its JSON is one snake_case string.
    let name = state.as_str().unwrap_or_default().to_string();
    Ok(Some(json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{"type": "text", "text": format!("{name} (idempotency key {key})")}],
            "structuredContent": {"state": state, "idempotencyKey": key},
            "isError": false,
        },
    })))
}

#[cfg(test)]
#[path = "messaging_peer_tests.rs"]
pub(in crate::mcp_broker) mod tests;

#[cfg(test)]
#[path = "messaging_peer_refusal_tests.rs"]
mod refusal_tests;
