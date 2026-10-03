//! One browser tool call: the tool's own arguments become a host command, the
//! host's answer becomes the agent's tool result.
//!
//! The caller context is this session's own row — its session id and its
//! workspace — so a tab belongs to the workspace that opened it and an argument
//! can never name another one. A session with no workspace is not refused: it
//! gets a scope of its own, which is what the broker's tab map already gives a
//! caller whose workspace is absent.
//!
//! A failed command is a tool error, not a transport error: the agent reads it,
//! so it carries the code (`browser_tab_not_found`) and the host's own sentence
//! — which is where `stale_ref:` and its advice live. A refused argument is a
//! malformed call instead, so an agent can tell "you asked wrong" from "the page
//! said no". Every outcome is audited like every other tool's, so a row
//! attributed to the calling device says what an agent did on these pages.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::mcp_broker::caller::{audit_mcp_tool, caller_conn, McpCaller};
use crate::mcp_broker::dispatch::{rpc_error, tool_error};
use crate::mcp_broker::RegisteredSession;
use crate::server::ServerState;

use super::browser_args::parse;
use super::browser_commands::spec_for;

/// Answer one `tools/call` for any of the browser tools.
///
/// The name is read here rather than matched by the caller, because the peer
/// door has already judged it and the table below is what that door's names mean
/// to a host.
pub(in crate::mcp_broker) fn call(
    state: &Arc<ServerState>,
    registration: &RegisteredSession,
    caller: McpCaller,
    id: Value,
    message: &Value,
) -> Result<Option<Value>, Value> {
    let Some(tool) = message.pointer("/params/name").and_then(Value::as_str) else {
        return Ok(Some(rpc_error(id, -32600, "Invalid Request")));
    };
    let Some(spec) = spec_for(tool) else {
        return Ok(Some(rpc_error(id, -32601, "Unknown tool")));
    };
    let arguments = message
        .pointer("/params/arguments")
        .cloned()
        .unwrap_or(Value::Null);
    let audit = |outcome: &str| {
        audit_mcp_tool(state, &caller, tool, &registration.session_id, outcome);
    };
    let call = match parse(spec, tool, &arguments) {
        Ok(call) => call,
        Err(sentence) => {
            audit("invalid");
            return Ok(Some(rpc_error(id, -32602, &sentence)));
        }
    };
    let context = match browser_caller(state, registration, &caller) {
        Ok(context) => context,
        // The ownership door refused this caller its own row, so the daemon
        // cannot say which workspace its tabs belong to. Answering anyway would
        // quietly hand it a scope of its own.
        Err(sentence) => {
            audit("denied");
            return Ok(Some(tool_error(&id, &sentence)));
        }
    };
    match state.browser.execute(
        &context,
        spec.command,
        call.args,
        call.browser_id.as_deref(),
        call.timeout,
    ) {
        Ok(result) => {
            audit("ok");
            Ok(Some(browser_reply(&id, result)))
        }
        Err(error) => {
            audit("failed");
            Ok(Some(tool_error(
                &id,
                &format!("{}: {}", error.code.as_str(), error.message),
            )))
        }
    }
}

/// Who the host is told is asking: this session's own row, read through the same
/// ownership door every other scoped tool reads its scope through.
///
/// `Ok(None)` is a real answer — a session started outside any workspace has one,
/// and the broker's tab map gives such a caller a scope of its own.
fn browser_caller(
    state: &ServerState,
    registration: &RegisteredSession,
    caller: &McpCaller,
) -> Result<devboule_protocol::BrowserCaller, String> {
    let conn = caller_conn(state, caller);
    let workspace_id = state
        .sessions
        .caller_workspace_id(
            &registration.session_id,
            &registration.owner,
            &conn.conn_peer,
        )
        .map_err(|error| error.message)?;
    Ok(devboule_protocol::BrowserCaller {
        caller_session_id: registration.session_id.clone(),
        workspace_id,
    })
}

/// The host's result, unchanged, as text and as structured content.
fn browser_reply(id: &Value, result: Value) -> Value {
    let text = serde_json::to_string(&result).unwrap_or_else(|error| {
        json!({"browser": "the result could not be encoded", "detail": error.to_string()})
            .to_string()
    });
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{"type": "text", "text": text}],
            "structuredContent": result,
            "isError": false,
        },
    })
}
