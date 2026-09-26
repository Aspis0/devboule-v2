//! The terminal reads: the caller's terminal roster and one terminal's screen.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::mcp_broker::caller::{audit_mcp_tool, caller_conn, McpCaller};
use crate::mcp_broker::dispatch::{rpc_error, tool_error};
use crate::mcp_broker::RegisteredSession;
use crate::peer_policy::ConnPeer;
use crate::server::ServerState;

/// The capture's window: what a caller that states nothing gets, and the
/// closed range the published schema states and the parser enforces. The
/// source is the visible grid only — scrollback is never delivered — so the
/// default already answers a whole screen.
const DEFAULT_CAPTURE_LINES: usize = 40;
const MIN_CAPTURE_LINES: usize = 1;
const MAX_CAPTURE_LINES: usize = 200;

/// Why one read did not answer, in the two shapes the broker answers a
/// `tools/call` with: a malformed request, and a refusal. Both are built
/// here and go out through [`terminal_reply`], so neither handler invents
/// its own way to fail.
enum TerminalReadError {
    Invalid(String),
    Refused(String),
}

/// One reply path for both reads: the document as text and as
/// `structuredContent`, a malformed request as `-32602`, a refusal as a tool
/// error, and the encode failure as `-32603`. A refusal is never an empty
/// document — `[]` would read as "your workspace has no terminals".
fn terminal_reply(
    id: &Value,
    result: Result<Value, TerminalReadError>,
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
        Err(TerminalReadError::Invalid(message)) => {
            Ok(Some(rpc_error(id.clone(), -32602, &message)))
        }
        Err(TerminalReadError::Refused(message)) => Ok(Some(tool_error(id, &message))),
    }
}

/// The caller's own workspace, which is the whole scope of both reads. A
/// session with no workspace has no scope to read terminals in, so it is
/// refused rather than answered about every workspace-less terminal of its
/// user. The workspace comes from the caller's own row, never an argument.
fn caller_workspace(
    state: &ServerState,
    registration: &RegisteredSession,
    conn_peer: &Option<ConnPeer>,
) -> Result<String, TerminalReadError> {
    state
        .sessions
        .terminal_scope(&registration.session_id, &registration.owner, conn_peer)
        .map_err(|error| TerminalReadError::Refused(error.message))?
        .ok_or_else(|| {
            TerminalReadError::Refused(
                "This session has no workspace, so no terminal is in scope.".to_string(),
            )
        })
}

pub(in crate::mcp_broker) fn list(
    state: &Arc<ServerState>,
    registration: &RegisteredSession,
    caller: McpCaller,
    id: Value,
) -> Result<Option<Value>, Value> {
    // Deliberately do not read params.arguments, like the roster tool: the
    // bearer is the identity and the caller's own row is the scope, so no
    // argument can name a workspace or a terminal.
    let conn = caller_conn(state, &caller);
    let audit = |outcome_label: &str| {
        audit_mcp_tool(
            state,
            &caller,
            crate::provider_catalog::MCP_LIST_TERMINALS_TOOL,
            &registration.session_id,
            outcome_label,
        );
    };
    let result = read_list(state, registration, &conn.conn_peer);
    audit(if result.is_ok() { "ok" } else { "denied" });
    terminal_reply(&id, result)
}

pub(in crate::mcp_broker) fn capture(
    state: &Arc<ServerState>,
    registration: &RegisteredSession,
    caller: McpCaller,
    id: Value,
    message: &Value,
) -> Result<Option<Value>, Value> {
    let arguments = message
        .pointer("/params/arguments")
        .cloned()
        .unwrap_or(Value::Null);
    // A malformed request is answered before anything is audited or touched,
    // the way the sibling tools answer one.
    let (terminal, lines) = match parse_capture_arguments(&arguments) {
        Ok(parsed) => parsed,
        Err(sentence) => return terminal_reply(&id, Err(TerminalReadError::Invalid(sentence))),
    };
    let conn = caller_conn(state, &caller);
    let audit = |outcome_label: &str| {
        audit_mcp_tool(
            state,
            &caller,
            crate::provider_catalog::MCP_CAPTURE_TERMINAL_TOOL,
            &registration.session_id,
            outcome_label,
        );
    };
    let result = read_capture(state, registration, &conn.conn_peer, &terminal, lines);
    audit(if result.is_ok() { "ok" } else { "denied" });
    terminal_reply(&id, result)
}

fn read_list(
    state: &ServerState,
    registration: &RegisteredSession,
    conn_peer: &Option<ConnPeer>,
) -> Result<Value, TerminalReadError> {
    let workspace = caller_workspace(state, registration, conn_peer)?;
    let terminals = state
        .sessions
        .terminals_in_workspace(&registration.owner, conn_peer, &workspace)
        .map_err(|error| TerminalReadError::Refused(error.message))?;
    let terminals = terminals
        .iter()
        .map(|session| {
            json!({
                "id": session.id,
                "title": session.title,
                "cwd": session.cwd,
                "createdBy": session.created_by,
            })
        })
        .collect::<Vec<_>>();
    Ok(json!({"terminals": terminals}))
}

fn read_capture(
    state: &ServerState,
    registration: &RegisteredSession,
    conn_peer: &Option<ConnPeer>,
    terminal: &str,
    lines: usize,
) -> Result<Value, TerminalReadError> {
    let workspace = caller_workspace(state, registration, conn_peer)?;
    let (rows, total) = state
        .sessions
        .terminal_screen(terminal, &registration.owner, conn_peer, &workspace, lines)
        .map_err(|error| TerminalReadError::Refused(error.message))?;
    Ok(json!({
        "terminalId": terminal,
        "lines": rows,
        "totalLines": total,
        "truncated": total > rows.len(),
    }))
}

/// The closed argument set: `terminalId` required, `lines` optional. An
/// integer outside the published schema's `1..=200` is refused with the
/// range, and a value that is not an integer at all says so — the check and
/// the schema are one rule, not two that have to be kept in step.
fn parse_capture_arguments(arguments: &Value) -> Result<(String, usize), String> {
    let empty = json!({});
    let arguments = match arguments {
        Value::Null => &empty,
        Value::Object(_) => arguments,
        _ => return Err("arguments must be an object".to_string()),
    };
    let object = arguments
        .as_object()
        .ok_or_else(|| "arguments must be an object".to_string())?;
    // The known-parameter list is read out of the published schema, so the
    // document and the check cannot disagree about what the tool accepts.
    let known = crate::provider_catalog::terminal_capture_input_schema()["properties"]
        .as_object()
        .map(|properties| properties.keys().cloned().collect::<Vec<String>>())
        .unwrap_or_default();
    for key in object.keys() {
        if !known.iter().any(|known| known == key) {
            return Err(format!("unknown parameter '{key}'"));
        }
    }
    let terminal = object
        .get("terminalId")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "terminalId is required".to_string())?;
    let lines = match object.get("lines") {
        None | Some(Value::Null) => DEFAULT_CAPTURE_LINES,
        Some(value) => {
            let requested = value
                .as_i64()
                .ok_or_else(|| "lines must be an integer".to_string())?;
            if requested < MIN_CAPTURE_LINES as i64 || requested > MAX_CAPTURE_LINES as i64 {
                return Err(format!(
                    "lines must be between {MIN_CAPTURE_LINES} and {MAX_CAPTURE_LINES}"
                ));
            }
            requested as usize
        }
    };
    Ok((terminal.to_string(), lines))
}
