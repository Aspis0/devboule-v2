//! What a create refuses before it opens a shell: a retry whose payload
//! changed, two calls racing for the last slot of the cap while they wait on
//! the same card, and a workspace that cannot name the directory the shell
//! would start in.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;

use super::terminal_write_harness::{
    allow_group, answer, call, call_on_a_thread, caller_in, join_without_a_card, keepalive_spawn,
    live_terminals, owner, pending_cards, project_workspace, refusal, serve, wait_for_card,
};
use super::*;
use crate::mcp_broker::tools::first_use::{TERMINAL_CREATE_GROUP, TERMINAL_KILL_GROUP};
use crate::provider_catalog::{MCP_CREATE_TERMINAL_TOOL, MCP_KILL_TERMINAL_TOOL};
use crate::session::MAX_LIVE_TERMINALS_PER_CREATOR;
use devboule_protocol::PermissionOutcome;

/// The cap's own sentence, named here because the act it refuses is the act
/// this test races for.
const AT_CAPACITY: &str = "too many live terminals; close one first";

/// Another card, if one arrives within `limit`: a second card means a second
/// call reached the consent wait.
fn another_card(state: &Arc<ServerState>, session: &str, limit: Duration) -> Option<String> {
    let start = Instant::now();
    loop {
        if let Some(card) = pending_cards(state, session).pop() {
            return Some(card);
        }
        if start.elapsed() >= limit {
            return None;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn a_create_retried_with_a_different_name_is_refused() {
    // The same frame id with a changed payload is not a retry: the wire
    // answers it with the idempotency conflict, and neither call opens a
    // second terminal for a key that already answered once.
    let state = ServerState::new("mcp-twg-conflict".to_string());
    let (workspace, _root) = project_workspace(&state, "conflict");
    caller_in(&state, "twg-conflict-caller", &workspace);
    let token = serve(&state, "twg-conflict-caller");
    allow_group(&state, "twg-conflict-caller", TERMINAL_CREATE_GROUP);
    let _server = state.mcp.start(&state).expect("MCP server");

    keepalive_spawn(&state);
    let first = call(
        &state.mcp.url,
        &token,
        1,
        MCP_CREATE_TERMINAL_TOOL,
        r#"{"name":"First"}"#,
    );
    assert_eq!(first.pointer("/result/isError"), Some(&json!(false)));

    keepalive_spawn(&state);
    let conflicting = call(
        &state.mcp.url,
        &token,
        1,
        MCP_CREATE_TERMINAL_TOOL,
        r#"{"name":"Second"}"#,
    );
    assert_eq!(
        refusal(&conflicting),
        "idempotency key reused with a different payload",
        "{conflicting}"
    );
    let open = live_terminals(&state, &token);
    assert_eq!(open.len(), 1, "the conflict opened nothing: {open:?}");

    allow_group(&state, "twg-conflict-caller", TERMINAL_KILL_GROUP);
    let killed = call(
        &state.mcp.url,
        &token,
        2,
        MCP_KILL_TERMINAL_TOOL,
        &format!(r#"{{"terminalId":"{}"}}"#, open[0]),
    );
    assert_eq!(killed.pointer("/result/isError"), Some(&json!(false)));
}

#[test]
fn two_creates_racing_at_the_cap_open_exactly_one_terminal() {
    // The cap's window is a person looking at a card, not a microsecond:
    // both creates start while the group is still shut, so both are inside
    // the consent wait at the same time. The slot must be held from before
    // that wait, or both read room and both spawn.
    let state = ServerState::new("mcp-twg-race".to_string());
    let (workspace, _root) = project_workspace(&state, "race");
    caller_in(&state, "twg-race-caller", &workspace);
    for index in 0..MAX_LIVE_TERMINALS_PER_CREATOR - 1 {
        crate::session::insert_test_terminal_created_by(
            &state.sessions,
            &format!("twg-race-open{index}"),
            owner(),
            Some(workspace.clone()),
            "twg-race-caller",
        );
    }
    let token = serve(&state, "twg-race-caller");
    let _server = state.mcp.start(&state).expect("MCP server");

    let first = call_on_a_thread(
        &state.mcp.url,
        &token,
        10,
        MCP_CREATE_TERMINAL_TOOL,
        r#"{"name":"Racer A"}"#,
    );
    let second = call_on_a_thread(
        &state.mcp.url,
        &token,
        11,
        MCP_CREATE_TERMINAL_TOOL,
        r#"{"name":"Racer B"}"#,
    );
    let card = wait_for_card(&state, "twg-race-caller");
    answer(
        &state,
        "twg-race-caller",
        &card,
        PermissionOutcome::AllowOnce,
        "session",
    );
    // Exactly one create may be waiting on the person. A second card means
    // the cap let both ask: answer it too, so both calls finish and the
    // assertions name the overshoot instead of hanging on it.
    if let Some(second_card) = another_card(&state, "twg-race-caller", Duration::from_secs(2)) {
        answer(
            &state,
            "twg-race-caller",
            &second_card,
            PermissionOutcome::AllowOnce,
            "session",
        );
    }
    let replies = [
        first.join().expect("first create"),
        second.join().expect("second create"),
    ];
    let won = replies
        .iter()
        .filter(|reply| reply.pointer("/result/isError") == Some(&json!(false)))
        .count();
    let sentences: Vec<&str> = replies
        .iter()
        .filter_map(|reply| {
            reply
                .pointer("/result/content/0/text")
                .and_then(Value::as_str)
        })
        .collect();
    assert_eq!(won, 1, "one create wins the last slot: {replies:?}");
    assert!(
        sentences.contains(&AT_CAPACITY),
        "the loser is refused by the cap: {sentences:?}"
    );
    assert_eq!(
        live_terminals(&state, &token).len(),
        MAX_LIVE_TERMINALS_PER_CREATOR,
        "the cap holds under the race"
    );

    allow_group(&state, "twg-race-caller", TERMINAL_KILL_GROUP);
    for terminal in live_terminals(&state, &token) {
        let killed = call(
            &state.mcp.url,
            &token,
            12,
            MCP_KILL_TERMINAL_TOOL,
            &format!(r#"{{"terminalId":"{terminal}"}}"#),
        );
        assert_eq!(killed.pointer("/result/isError"), Some(&json!(false)));
    }
}

#[test]
fn a_workspace_that_cannot_name_its_directory_refuses_the_create() {
    // The card must state the directory the shell opens in; a caller whose
    // row names a workspace the daemon cannot resolve is refused here, with
    // no placeholder standing in for the fact.
    let state = ServerState::new("mcp-twg-cwd".to_string());
    caller_in(&state, "twg-cwd-caller", "ws-not-a-row");
    let token = serve(&state, "twg-cwd-caller");
    let _server = state.mcp.start(&state).expect("MCP server");

    let handle = call_on_a_thread(
        &state.mcp.url,
        &token,
        1,
        MCP_CREATE_TERMINAL_TOOL,
        r#"{"name":"Nowhere"}"#,
    );
    let reply = join_without_a_card(&state, "twg-cwd-caller", handle);
    assert!(
        refusal(&reply).contains("unavailable"),
        "the refusal names the workspace it could not read: {reply}"
    );
    assert!(
        live_terminals(&state, &token).is_empty(),
        "nothing was created"
    );
}
