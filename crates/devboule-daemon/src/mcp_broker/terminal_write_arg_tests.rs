//! The closed argument sets: what a terminal write refuses before anything
//! is asked or typed — a payload that would type nothing, and a name that is
//! not one line of plain text.

use serde_json::{json, Value};

use super::terminal_write_harness::{
    call_on_a_thread, caller_in, join_without_a_card, live_terminals, owner, pending_cards,
    project_workspace, serve,
};
use super::*;
use crate::provider_catalog::{MCP_CREATE_TERMINAL_TOOL, MCP_SEND_TERMINAL_KEYS_TOOL};

#[test]
fn an_empty_keys_payload_is_refused_before_the_card() {
    // Nothing would be typed and something would be approved: an empty
    // payload is a malformed call, answered before the person is asked.
    let state = ServerState::new("mcp-twg-empty".to_string());
    let (workspace, _root) = project_workspace(&state, "empty");
    caller_in(&state, "twg-empty-caller", &workspace);
    let received = crate::session::insert_test_terminal_with_recording_writer(
        &state.sessions,
        "twg-empty-terminal",
        owner(),
        Some(workspace.clone()),
    );
    let token = serve(&state, "twg-empty-caller");
    let _server = state.mcp.start(&state).expect("MCP server");

    let handle = call_on_a_thread(
        &state.mcp.url,
        &token,
        1,
        MCP_SEND_TERMINAL_KEYS_TOOL,
        r#"{"terminalId":"twg-empty-terminal","keys":""}"#,
    );
    let reply = join_without_a_card(&state, "twg-empty-caller", handle);
    assert_eq!(
        reply.pointer("/error/code"),
        Some(&json!(-32602)),
        "{reply}"
    );
    assert_eq!(
        reply.pointer("/error/message"),
        Some(&json!("keys must not be empty")),
        "{reply}"
    );
    assert!(received.lock().expect("recorder").is_empty());
    assert!(pending_cards(&state, "twg-empty-caller").is_empty());
}

#[test]
fn a_name_that_is_not_plain_text_is_refused() {
    // The title the roster prints and the card shows is judged by the same
    // character rule a device name is: no invisible formatting, no control
    // bytes, no line break a renderer could honour.
    let state = ServerState::new("mcp-twg-name".to_string());
    let (workspace, _root) = project_workspace(&state, "name");
    caller_in(&state, "twg-name-caller", &workspace);
    let token = serve(&state, "twg-name-caller");
    let _server = state.mcp.start(&state).expect("MCP server");

    for (id, arguments, category) in [
        (1, r#"{"name":"zero\u200bwidth"}"#, "invisible formatting"),
        (2, r#"{"name":"escape\u001btitle"}"#, "control character"),
        (3, r#"{"name":"line\u000bbreak"}"#, "control character"),
    ] {
        let handle = call_on_a_thread(
            &state.mcp.url,
            &token,
            id,
            MCP_CREATE_TERMINAL_TOOL,
            arguments,
        );
        let reply = join_without_a_card(&state, "twg-name-caller", handle);
        assert_eq!(
            reply.pointer("/error/code"),
            Some(&json!(-32602)),
            "{arguments}: {reply}"
        );
        assert!(
            refusal_or_message(&reply).contains(category),
            "{arguments} names {category}: {reply}"
        );
    }
    assert!(live_terminals(&state, &token).is_empty());
}

/// The sentence a reply carries, whether it arrived as a tool error or as a
/// JSON-RPC one — a malformed request answers with the second.
fn refusal_or_message(reply: &Value) -> String {
    reply
        .pointer("/result/content/0/text")
        .or_else(|| reply.pointer("/error/message"))
        .and_then(Value::as_str)
        .expect("a sentence")
        .to_string()
}
