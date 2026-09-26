//! The terminal reads: the caller's terminal roster and one terminal's screen.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::mcp_broker::dispatch::{rpc_error, tool_error};
use crate::mcp_broker::RegisteredSession;
use crate::server::ServerState;

/// The capture's default window and its hard maximum, in grid lines. The
/// source is the visible grid only — scrollback is never delivered — so the
/// default already answers a whole screen, and the maximum bounds one reply,
/// where Paseo's capture answers its whole buffer uncapped.
const DEFAULT_CAPTURE_LINES: usize = 40;
const MAX_CAPTURE_LINES: usize = 200;

/// The caller's own workspace, which is the whole scope of both reads.
///
/// A session with no workspace has no scope to read terminals in, so it is
/// refused rather than answered about every workspace-less terminal of its
/// user. The workspace is read from the caller's own row, never from an
/// argument.
fn caller_workspace(
    state: &ServerState,
    registration: &RegisteredSession,
) -> Result<String, String> {
    state
        .sessions
        .session_workspace_id(&registration.session_id, &registration.owner)
        .map_err(|error| error.message)?
        .ok_or_else(|| "This session has no workspace, so no terminal is in scope.".to_string())
}

/// One reply shape for both reads: the document as text and as
/// `structuredContent`. A refusal is never an empty document — `[]` would
/// read as "your workspace has no terminals".
fn tool_document(id: &Value, document: Value) -> Result<Option<Value>, Value> {
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

pub(in crate::mcp_broker) fn list(
    state: &Arc<ServerState>,
    registration: &RegisteredSession,
    id: Value,
) -> Result<Option<Value>, Value> {
    // Deliberately do not read params.arguments, like the roster tool: the
    // bearer is the identity and the caller's own row is the scope, so no
    // argument can name a workspace or a terminal.
    let workspace = match caller_workspace(state, registration) {
        Ok(workspace) => workspace,
        Err(sentence) => return Ok(Some(tool_error(&id, &sentence))),
    };
    let terminals = match state
        .sessions
        .terminals_in_workspace(&registration.owner, &workspace)
    {
        Ok(terminals) => terminals,
        Err(error) => return Ok(Some(tool_error(&id, &error.message))),
    };
    let terminals = terminals
        .iter()
        .map(|session| {
            json!({
                "id": session.id,
                "title": session.title,
                "cwd": session.cwd,
                "createdBy": session.created_by,
                "live": session.state.is_live(),
            })
        })
        .collect::<Vec<_>>();
    tool_document(&id, json!({"terminals": terminals}))
}

pub(in crate::mcp_broker) fn capture(
    state: &Arc<ServerState>,
    registration: &RegisteredSession,
    id: Value,
    message: &Value,
) -> Result<Option<Value>, Value> {
    let arguments = message
        .pointer("/params/arguments")
        .cloned()
        .unwrap_or(Value::Null);
    let (terminal, lines) = match parse_capture_arguments(&arguments) {
        Ok(parsed) => parsed,
        Err(sentence) => return Ok(Some(rpc_error(id, -32602, &sentence))),
    };
    let workspace = match caller_workspace(state, registration) {
        Ok(workspace) => workspace,
        Err(sentence) => return Ok(Some(tool_error(&id, &sentence))),
    };
    let rows = match state
        .sessions
        .terminal_screen(&terminal, &registration.owner, &workspace)
    {
        Ok(rows) => rows,
        Err(error) => return Ok(Some(tool_error(&id, &error.message))),
    };
    let total = rows.len();
    let from = total.saturating_sub(lines);
    tool_document(
        &id,
        json!({
            "terminalId": terminal,
            "lines": &rows[from..],
            "totalLines": total,
        }),
    )
}

/// The closed argument set: `terminalId` required, `lines` optional — the
/// bottom window to answer, clamped into the `1..=200` the published schema
/// states, the way the Oracle search clamps `limit`.
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
            let lines = value
                .as_u64()
                .ok_or_else(|| "lines must be an integer".to_string())?;
            usize::try_from(lines)
                .unwrap_or(MAX_CAPTURE_LINES)
                .clamp(1, MAX_CAPTURE_LINES)
        }
    };
    Ok((terminal.to_string(), lines))
}
