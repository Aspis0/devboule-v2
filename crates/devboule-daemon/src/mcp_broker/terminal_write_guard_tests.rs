//! The refusals that surround a terminal write: what must be true before an
//! act runs, and who must have consented to it — one card per act, the cap's
//! reservation held across the card, the shutdown guard on every act, a
//! retry whose payload changed, an empty payload, a workspace that cannot
//! name its own directory, a name that is not plain text, the supervision
//! verbs' agent-only scope, and a kill whose close removed nothing.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::terminal_write_harness::{
    allow_group, answer, call, call_on_a_thread, caller_in, join_without_a_card, keepalive_spawn,
    owner, pending_cards, project_workspace, refusal, serve, wait_for_card,
};
use super::*;
use crate::mcp_broker::tools::first_use::{
    TERMINAL_CREATE_GROUP, TERMINAL_KEYS_GROUP, TERMINAL_KILL_GROUP,
};
use crate::provider_catalog::{
    MCP_CREATE_TERMINAL_TOOL, MCP_KILL_TERMINAL_TOOL, MCP_LIST_TERMINALS_TOOL,
    MCP_SEND_TERMINAL_KEYS_TOOL, MCP_STOP_AGENT_TOOL,
};
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

fn live_terminals(state: &Arc<ServerState>, token: &str) -> Vec<String> {
    let listed = call(&state.mcp.url, token, 99, MCP_LIST_TERMINALS_TOOL, "{}");
    listed["result"]["structuredContent"]["terminals"]
        .as_array()
        .expect("terminals")
        .iter()
        .map(|terminal| terminal["id"].as_str().expect("id").to_string())
        .collect()
}

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
