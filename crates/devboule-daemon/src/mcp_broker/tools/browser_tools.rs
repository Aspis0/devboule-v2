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
//!
//! One answer is not text: a screenshot answers the picture itself as an MCP
//! image block, with one short line beside it. The bytes travel once — a
//! `structuredContent` copy would carry the same base64 a second time — and a
//! result that turns out to carry no image is read as text like any other, so a
//! host that answered a screenshot with something else shows the agent exactly
//! what came back.

use std::sync::Arc;

use serde_json::{json, Value};

use crate::mcp_broker::caller::{audit_mcp_tool, caller_conn, McpCaller};
use crate::mcp_broker::dispatch::{rpc_error, tool_error};
use crate::mcp_broker::RegisteredSession;
use crate::server::ServerState;
use crate::untrusted_frame::{escape_json_strings, Source};
use crate::visible_text::escape_for_model;

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
            Ok(Some(browser_reply(&id, spec.command, result)))
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
pub(in crate::mcp_broker) fn browser_caller(
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

/// The host's result: text and a structured document, or — for a screenshot
/// that really carries one — the picture and the one line that says how big it
/// is.
///
/// Whatever the page put in it is untrusted, so the content blocks sit between a
/// head and a tail the daemon writes (`untrusted_frame`): the head names the
/// page's address when the host reported one, and the tail carries the nonce
/// that is the only thing that ends the content. The blocks themselves are the
/// host's own, untouched but for hidden characters spelled out, so the text an
/// agent parses is exactly the text it parsed before the frame.
pub(in crate::mcp_broker) fn browser_reply(id: &Value, command: &str, result: Value) -> Value {
    let (head, tail) = Source::BrowserPage {
        url: page_address(&result),
    }
    .fence();
    let picture = if command == "screenshot" {
        picture(&result)
    } else {
        None
    };
    let Some((block, line)) = picture else {
        // The structured copy and the text read alike, so both are escaped; a
        // picture's bytes are not text and stay as the host sent them.
        let result = escape_json_strings(&result);
        let text = serde_json::to_string(&result).unwrap_or_else(|error| {
            json!({"browser": "the result could not be encoded", "detail": error.to_string()})
                .to_string()
        });
        return json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "content": [
                    {"type": "text", "text": head},
                    {"type": "text", "text": text},
                    {"type": "text", "text": tail},
                ],
                "structuredContent": result,
                "isError": false,
            },
        });
    };
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [
                {"type": "text", "text": head},
                block,
                {"type": "text", "text": escape_for_model(&line)},
                {"type": "text", "text": tail},
            ],
            "isError": false,
        },
    })
}

/// The address the host reported for the page the answer is about: the page's
/// own, or where a navigation landed. `None` when the answer names none.
fn page_address(result: &Value) -> Option<&str> {
    result
        .pointer("/url")
        .or_else(|| result.pointer("/delta/url"))
        .and_then(Value::as_str)
}

/// The picture a screenshot answers and the one line beside it, or `None` for
/// anything that is not a picture: the bytes and the type are the host's own,
/// so an answer without both is the text it is.
fn picture(result: &Value) -> Option<(Value, String)> {
    let mime = result.get("mimeType")?.as_str()?;
    let data = result.get("data")?.as_str()?;
    if data.is_empty() || !mime.starts_with("image/") {
        return None;
    }
    let mut line = format!(
        "{mime} {} px, viewport {} css px",
        size(result, "width", "height"),
        size(result, "cssWidth", "cssHeight"),
    );
    if let Some(factor) = result.get("zoom").and_then(Value::as_f64) {
        // The factor a picture pixel is worth in the points click_at takes: an
        // agent reads a point off the picture and divides by this.
        line.push_str(&format!(", {} per point", number(&json!(factor))));
    }
    if let Some(clip) = result.get("clip").filter(|clip| clip.is_object()) {
        let corner = |key: &str| clip.get(key).map_or_else(String::new, number);
        line.push_str(&format!(
            ", clip {},{},{},{}",
            corner("x"),
            corner("y"),
            corner("width"),
            corner("height")
        ));
    }
    Some((
        json!({"type": "image", "data": data, "mimeType": mime}),
        line,
    ))
}

/// The host computes every size it sends as an `f64`, so `serde_json` writes
/// `1280.0`: a line that read them as integers found nothing and said `x px`.
/// A whole size prints without its decimals; a fractional one keeps them,
/// because there the decimal part is the size.
fn number(value: &Value) -> String {
    let Some(size) = value.as_f64() else {
        return String::new();
    };
    if size.fract() == 0.0 {
        format!("{}", size as i64)
    } else {
        format!("{size}")
    }
}

fn size(at: &Value, first: &str, second: &str) -> String {
    format!(
        "{}x{}",
        at.get(first).map_or_else(String::new, number),
        at.get(second).map_or_else(String::new, number),
    )
}
