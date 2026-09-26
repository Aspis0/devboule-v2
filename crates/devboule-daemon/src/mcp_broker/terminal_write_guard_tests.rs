//! The guards that make a terminal write refuse instead of act: the daemon
//! going down, a supervision verb pointed at a terminal, and a kill whose
//! close removed nothing.

use std::sync::Arc;

use serde_json::json;

use super::terminal_write_harness::{
    allow_group, call, caller_in, keepalive_spawn, live_terminals, owner, project_workspace,
    refusal, serve,
};
use super::*;
use crate::mcp_broker::tools::first_use::{
    TERMINAL_CREATE_GROUP, TERMINAL_KEYS_GROUP, TERMINAL_KILL_GROUP,
};
use crate::provider_catalog::{
    MCP_CREATE_TERMINAL_TOOL, MCP_KILL_TERMINAL_TOOL, MCP_SEND_TERMINAL_KEYS_TOOL,
    MCP_STOP_AGENT_TOOL,
};

#[test]
fn the_shutdown_guard_covers_keys_and_kill() {
    // A teardown is not a moment to type into a pty or to race a kill
    // against it: both acts answer the guard the create already had.
    let state = ServerState::new("mcp-twg-shutdown".to_string());
    let (workspace, _root) = project_workspace(&state, "down");
    caller_in(&state, "twg-down-caller", &workspace);
    let received = crate::session::insert_test_terminal_with_recording_writer(
        &state.sessions,
        "twg-down-terminal",
        owner(),
        Some(workspace.clone()),
    );
    let token = serve(&state, "twg-down-caller");
    allow_group(&state, "twg-down-caller", TERMINAL_KEYS_GROUP);
    allow_group(&state, "twg-down-caller", TERMINAL_KILL_GROUP);
    let _server = state.mcp.start(&state).expect("MCP server");

    state.request_shutdown();

    let keys = call(
        &state.mcp.url,
        &token,
        1,
        MCP_SEND_TERMINAL_KEYS_TOOL,
        r#"{"terminalId":"twg-down-terminal","keys":"too late"}"#,
    );
    assert!(
        refusal(&keys).contains("shutting down"),
        "keys answers the guard: {keys}"
    );
    assert!(
        received.lock().expect("recorder").is_empty(),
        "a guarded keys typed nothing"
    );
    let kill = call(
        &state.mcp.url,
        &token,
        2,
        MCP_KILL_TERMINAL_TOOL,
        r#"{"terminalId":"twg-down-terminal"}"#,
    );
    assert!(
        refusal(&kill).contains("shutting down"),
        "kill answers the guard: {kill}"
    );
    assert_eq!(
        live_terminals(&state, &token).len(),
        1,
        "the guarded kill removed nothing"
    );
}

#[test]
fn a_supervision_verb_does_not_resolve_a_terminal() {
    // The stop/close/cancel/status verbs act on agents: a terminal this
    // caller opened answers there the way an id it never created answers,
    // whatever the created_by link says.
    let state = ServerState::new("mcp-twg-supervision".to_string());
    let (workspace, _root) = project_workspace(&state, "supervision");
    caller_in(&state, "twg-supervision-caller", &workspace);
    let token = serve(&state, "twg-supervision-caller");
    allow_group(&state, "twg-supervision-caller", TERMINAL_CREATE_GROUP);
    let _server = state.mcp.start(&state).expect("MCP server");

    keepalive_spawn(&state);
    let created = call(
        &state.mcp.url,
        &token,
        1,
        MCP_CREATE_TERMINAL_TOOL,
        r#"{"name":"Not a child"}"#,
    );
    assert_eq!(created.pointer("/result/isError"), Some(&json!(false)));
    let terminal_id = created["result"]["structuredContent"]["terminalId"]
        .as_str()
        .expect("terminalId")
        .to_string();

    let stopped = call(
        &state.mcp.url,
        &token,
        2,
        MCP_STOP_AGENT_TOOL,
        &format!(r#"{{"session":"{terminal_id}"}}"#),
    );
    assert!(
        refusal(&stopped).contains("none of your live children"),
        "a terminal is not a supervision target: {stopped}"
    );
    assert_eq!(
        live_terminals(&state, &token).len(),
        1,
        "the stopped-against terminal still runs"
    );

    allow_group(&state, "twg-supervision-caller", TERMINAL_KILL_GROUP);
    let killed = call(
        &state.mcp.url,
        &token,
        3,
        MCP_KILL_TERMINAL_TOOL,
        &format!(r#"{{"terminalId":"{terminal_id}"}}"#),
    );
    assert_eq!(killed.pointer("/result/isError"), Some(&json!(false)));
}

#[test]
fn a_kill_whose_close_removed_nothing_does_not_answer_success() {
    // The gate held a moment ago; the close then found nothing to remove.
    // The terminal is gone either way, and a kill that killed nothing must
    // say so the way every other unknown id says it.
    let state = ServerState::new("mcp-twg-vanished".to_string());
    let (workspace, _root) = project_workspace(&state, "vanished");
    caller_in(&state, "twg-vanished-caller", &workspace);
    let token = serve(&state, "twg-vanished-caller");
    allow_group(&state, "twg-vanished-caller", TERMINAL_CREATE_GROUP);
    allow_group(&state, "twg-vanished-caller", TERMINAL_KILL_GROUP);
    let _server = state.mcp.start(&state).expect("MCP server");

    keepalive_spawn(&state);
    let created = call(
        &state.mcp.url,
        &token,
        1,
        MCP_CREATE_TERMINAL_TOOL,
        r#"{"name":"Vanishing"}"#,
    );
    assert_eq!(created.pointer("/result/isError"), Some(&json!(false)));
    let terminal_id = created["result"]["structuredContent"]["terminalId"]
        .as_str()
        .expect("terminalId")
        .to_string();

    // Land a close in the gap the hook names: the gate has passed, the
    // tool's own close has not run yet. The row is reopened afterwards —
    // entry gone, row still standing — because that is the state a kill
    // finds when something else removed the session in between.
    let hook_state = Arc::clone(&state);
    let hook_terminal = terminal_id.clone();
    let hook_owner = owner();
    let hook_db = state.sessions.runtime_dir().join("journal.db");
    state.sessions.set_kill_after_gate_hook(Arc::new(move || {
        let _ = hook_state
            .sessions
            .close(&hook_terminal, &hook_owner, &None);
        // The journal answers in order, so this read runs after the close
        // mark above has landed, and before the tool's own read below.
        let _ = hook_state.sessions.list(&hook_owner);
        let connection = rusqlite::Connection::open(&hook_db).expect("journal db");
        connection
            .execute(
                "UPDATE sessions SET closed = 0 WHERE id = ?1",
                [&hook_terminal],
            )
            .expect("the row stands again");
    }));

    let killed = call(
        &state.mcp.url,
        &token,
        2,
        MCP_KILL_TERMINAL_TOOL,
        &format!(r#"{{"terminalId":"{terminal_id}"}}"#),
    );
    assert_eq!(
        refusal(&killed),
        "No session with that id.",
        "a kill that removed nothing must not answer success: {killed}"
    );
    assert!(
        live_terminals(&state, &token).is_empty(),
        "the hook's close is what ended it"
    );
}
