//! What all five terminal tools share: the workspace scope every one of them
//! reads from the caller's own row, and the one reply path they answer
//! through.

use serde_json::{json, Value};

use crate::mcp_broker::dispatch::{rpc_error, tool_error};
use crate::mcp_broker::RegisteredSession;
use crate::peer_policy::ConnPeer;
use crate::server::ServerState;

/// Why one terminal tool did not answer, in the two shapes the broker
/// answers a `tools/call` with: a malformed request, and a refusal. Both are
/// built by the tool bodies and go out through [`terminal_reply`], so no
/// body invents its own way to fail.
pub(in crate::mcp_broker) enum TerminalError {
    Invalid(String),
    Refused(String),
}

/// One reply path for every terminal tool: the document as text and as
/// `structuredContent`, a malformed request as `-32602`, a refusal as a tool
/// error, and the encode failure as `-32603`. A refusal is never an empty
/// document — `[]` would read as "your workspace has no terminals".
pub(in crate::mcp_broker) fn terminal_reply(
    id: &Value,
    result: Result<Value, TerminalError>,
) -> Result<Option<Value>, Value> {
    match result {
        Ok(document) => {
            let text = serde_json::to_string(&document).map_err(|error| {
                json!({"jsonrpc":"2.0", "id": id, "error": {"code": -32603, "message": format!("Could not encode terminals: {error}")}})
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
        Err(TerminalError::Invalid(message)) => Ok(Some(rpc_error(id.clone(), -32602, &message))),
        Err(TerminalError::Refused(message)) => Ok(Some(tool_error(id, &message))),
    }
}

/// The caller's own workspace, which is the whole scope of the five tools. A
/// session with no workspace has no scope to reach a terminal in, so it is
/// refused rather than answered about every workspace-less terminal of its
/// user. The workspace comes from the caller's own row, never an argument.
pub(in crate::mcp_broker) fn caller_workspace(
    state: &ServerState,
    registration: &RegisteredSession,
    conn_peer: &Option<ConnPeer>,
) -> Result<String, TerminalError> {
    state
        .sessions
        .terminal_scope(&registration.session_id, &registration.owner, conn_peer)
        .map_err(|error| TerminalError::Refused(error.message))?
        .ok_or_else(|| {
            TerminalError::Refused(
                "This session has no workspace, so no terminal is in scope.".to_string(),
            )
        })
}
