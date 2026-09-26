//! The consent surface, one act at a time: a session granted one terminal
//! write is still carded for the other two, and the card says whose terminal
//! the act is about.

use std::sync::Arc;

use serde_json::{json, Value};

use super::terminal_write_harness::{
    allow_group, answer, call, call_on_a_thread, caller_in, keepalive_spawn, live_terminals, owner,
    pending_cards, project_workspace, serve, wait_for_card,
};
use super::*;
use crate::mcp_broker::tools::first_use::{
    TERMINAL_CREATE_GROUP, TERMINAL_KEYS_GROUP, TERMINAL_KILL_GROUP,
};
use crate::provider_catalog::{
    MCP_CREATE_TERMINAL_TOOL, MCP_KILL_TERMINAL_TOOL, MCP_SEND_TERMINAL_KEYS_TOOL,
};
use devboule_protocol::PermissionOutcome;

/// The card's description and the reply it was raised for: answered before
/// anything is read out of it, because an assertion that fails while the
/// call is still parked wedges the run on that open connection.
fn answer_and_read(
    state: &Arc<ServerState>,
    session: &str,
    card: &str,
    handle: std::thread::JoinHandle<Value>,
    outcome: PermissionOutcome,
    option: &str,
) -> (String, Value) {
    let pending = state
        .sessions
        .live_runtime(session, &owner())
        .expect("live session")
        .permission_broker()
        .expect("test broker")
        .test_pending_request(card)
        .expect("the pending card");
    let devboule_protocol::SessionEvent::PermissionRequest { description, .. } = pending else {
        // Answered before the panic for the same reason as below: the call
        // behind this card must not outlive the assertion.
        answer(state, session, card, PermissionOutcome::Deny, "deny");
        panic!("the gate raises a permission request");
    };
    answer(state, session, card, outcome, option);
    let reply = handle.join().expect("write thread");
    (description.expect("description"), reply)
}

#[test]
fn granting_the_create_group_still_cards_keys_and_kill() {
    // One consent per act: the session may open shells, and typing into one
    // or killing one still asks the person, with its own card.
    let state = ServerState::new("mcp-twg-create".to_string());
    let (workspace, _root) = project_workspace(&state, "cgrant");
    caller_in(&state, "twg-create-caller", &workspace);
    let token = serve(&state, "twg-create-caller");
    allow_group(&state, "twg-create-caller", TERMINAL_CREATE_GROUP);
    let _server = state.mcp.start(&state).expect("MCP server");

    keepalive_spawn(&state);
    let created = call(
        &state.mcp.url,
        &token,
        1,
        MCP_CREATE_TERMINAL_TOOL,
        r#"{"name":"Granted"}"#,
    );
    assert_eq!(
        created.pointer("/result/isError"),
        Some(&json!(false)),
        "the granted act just runs: {created}"
    );
    let opened = created["result"]["structuredContent"]["terminalId"]
        .as_str()
        .expect("terminalId")
        .to_string();

    // The keys card, raised as if the grant had never happened — and it
    // names whose terminal this is: the caller opened it itself.
    let keys_handle = call_on_a_thread(
        &state.mcp.url,
        &token,
        2,
        MCP_SEND_TERMINAL_KEYS_TOOL,
        &format!(r#"{{"terminalId":"{opened}","keys":"typed anyway"}}"#),
    );
    let card = wait_for_card(&state, "twg-create-caller");
    let (description, keys_reply) = answer_and_read(
        &state,
        "twg-create-caller",
        &card,
        keys_handle,
        PermissionOutcome::Deny,
        "deny",
    );
    assert_eq!(
        keys_reply.pointer("/result/isError"),
        Some(&json!(true)),
        "keys still asks: {keys_reply}"
    );
    assert!(
        description.contains("opened by: this agent"),
        "the card says who opened the terminal: {description}"
    );
    assert!(
        description.contains(&workspace),
        "the card says which workspace: {description}"
    );

    // The kill card, same story: asked for, and refused.
    let kill_handle = call_on_a_thread(
        &state.mcp.url,
        &token,
        3,
        MCP_KILL_TERMINAL_TOOL,
        &format!(r#"{{"terminalId":"{opened}"}}"#),
    );
    let card = wait_for_card(&state, "twg-create-caller");
    answer(
        &state,
        "twg-create-caller",
        &card,
        PermissionOutcome::Deny,
        "deny",
    );
    let kill_reply = kill_handle.join().expect("kill thread");
    assert_eq!(
        kill_reply.pointer("/result/isError"),
        Some(&json!(true)),
        "kill still asks: {kill_reply}"
    );
    assert_eq!(
        live_terminals(&state, &token).len(),
        1,
        "the terminal the granted act opened survives both refusals"
    );

    // Cleanup is its own act, granted here.
    allow_group(&state, "twg-create-caller", TERMINAL_KILL_GROUP);
    let killed = call(
        &state.mcp.url,
        &token,
        4,
        MCP_KILL_TERMINAL_TOOL,
        &format!(r#"{{"terminalId":"{opened}"}}"#),
    );
    assert_eq!(killed.pointer("/result/isError"), Some(&json!(false)));
}

#[test]
fn granting_the_keys_group_still_cards_create_and_kill() {
    let state = ServerState::new("mcp-twg-keys".to_string());
    let (workspace, _root) = project_workspace(&state, "kgrant");
    caller_in(&state, "twg-keys-caller", &workspace);
    let received = crate::session::insert_test_terminal_with_recording_writer(
        &state.sessions,
        "twg-keys-typed",
        owner(),
        Some(workspace.clone()),
    );
    let token = serve(&state, "twg-keys-caller");
    allow_group(&state, "twg-keys-caller", TERMINAL_KEYS_GROUP);
    let _server = state.mcp.start(&state).expect("MCP server");

    let typed = call(
        &state.mcp.url,
        &token,
        1,
        MCP_SEND_TERMINAL_KEYS_TOOL,
        r#"{"terminalId":"twg-keys-typed","keys":"granted"}"#,
    );
    assert_eq!(
        typed.pointer("/result/isError"),
        Some(&json!(false)),
        "the granted act just runs: {typed}"
    );
    assert_eq!(received.lock().expect("recorder").clone(), b"granted");

    let create_handle = call_on_a_thread(
        &state.mcp.url,
        &token,
        2,
        MCP_CREATE_TERMINAL_TOOL,
        r#"{"name":"Still asked"}"#,
    );
    let card = wait_for_card(&state, "twg-keys-caller");
    answer(
        &state,
        "twg-keys-caller",
        &card,
        PermissionOutcome::Deny,
        "deny",
    );
    let create_reply = create_handle.join().expect("create thread");
    assert_eq!(
        create_reply.pointer("/result/isError"),
        Some(&json!(true)),
        "create still asks: {create_reply}"
    );
    assert_eq!(
        live_terminals(&state, &token).len(),
        1,
        "the denied create opened no second terminal"
    );

    let kill_handle = call_on_a_thread(
        &state.mcp.url,
        &token,
        3,
        MCP_KILL_TERMINAL_TOOL,
        r#"{"terminalId":"twg-keys-typed"}"#,
    );
    let card = wait_for_card(&state, "twg-keys-caller");
    answer(
        &state,
        "twg-keys-caller",
        &card,
        PermissionOutcome::Deny,
        "deny",
    );
    let kill_reply = kill_handle.join().expect("kill thread");
    assert_eq!(
        kill_reply.pointer("/result/isError"),
        Some(&json!(true)),
        "kill still asks: {kill_reply}"
    );

    // The terminal survived both refusals: typing into it still works.
    let again = call(
        &state.mcp.url,
        &token,
        4,
        MCP_SEND_TERMINAL_KEYS_TOOL,
        r#"{"terminalId":"twg-keys-typed","keys":"still here"}"#,
    );
    assert_eq!(again.pointer("/result/isError"), Some(&json!(false)));
}

#[test]
fn granting_the_kill_group_still_cards_create_and_keys() {
    let state = ServerState::new("mcp-twg-kill".to_string());
    let (workspace, _root) = project_workspace(&state, "kgrant");
    caller_in(&state, "twg-kill-caller", &workspace);
    // A terminal some *other* session opened: its card must say so, which is
    // also what this test's keys card is for.
    let received = crate::session::insert_test_terminal_created_by(
        &state.sessions,
        "twg-kill-foreign",
        owner(),
        Some(workspace.clone()),
        "twg-other-agent",
    );
    let token = serve(&state, "twg-kill-caller");
    allow_group(&state, "twg-kill-caller", TERMINAL_KILL_GROUP);
    let _server = state.mcp.start(&state).expect("MCP server");

    let create_handle = call_on_a_thread(
        &state.mcp.url,
        &token,
        1,
        MCP_CREATE_TERMINAL_TOOL,
        r#"{"name":"Still asked"}"#,
    );
    let card = wait_for_card(&state, "twg-kill-caller");
    answer(
        &state,
        "twg-kill-caller",
        &card,
        PermissionOutcome::Deny,
        "deny",
    );
    let create_reply = create_handle.join().expect("create thread");
    assert_eq!(
        create_reply.pointer("/result/isError"),
        Some(&json!(true)),
        "create still asks: {create_reply}"
    );

    let keys_handle = call_on_a_thread(
        &state.mcp.url,
        &token,
        2,
        MCP_SEND_TERMINAL_KEYS_TOOL,
        r#"{"terminalId":"twg-kill-foreign","keys":"typed anyway"}"#,
    );
    let card = wait_for_card(&state, "twg-kill-caller");
    let (description, keys_reply) = answer_and_read(
        &state,
        "twg-kill-caller",
        &card,
        keys_handle,
        PermissionOutcome::Deny,
        "deny",
    );
    assert!(
        description.contains("opened by: another agent"),
        "the card says whose terminal it is: {description}"
    );
    assert_eq!(
        keys_reply.pointer("/result/isError"),
        Some(&json!(true)),
        "keys still asks: {keys_reply}"
    );
    assert!(received.lock().expect("recorder").is_empty());

    // The granted act: it runs, and it was never asked about.
    let killed = call(
        &state.mcp.url,
        &token,
        3,
        MCP_KILL_TERMINAL_TOOL,
        r#"{"terminalId":"twg-kill-foreign"}"#,
    );
    assert_eq!(
        killed.pointer("/result/isError"),
        Some(&json!(false)),
        "the granted act just runs: {killed}"
    );
    assert!(live_terminals(&state, &token).is_empty());
    assert!(pending_cards(&state, "twg-kill-caller").is_empty());
}
