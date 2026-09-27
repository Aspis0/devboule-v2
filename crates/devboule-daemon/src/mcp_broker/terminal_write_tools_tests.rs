//! End-to-end tests for the three terminal writes: the kind gate in front of
//! every body, the workspace scope, the first-use card's three choices, the
//! create's cap, guard and retry, the bytes that reach a pty, and the audit
//! row that records the act and never the bytes.
//!
//! Sections: the kind gate, owner and origin scope, create, consent, keys,
//! kill, audit and privacy, the doors. The fixtures, the loopback call and
//! the card plumbing live in `terminal_write_harness`; the refusals around a
//! write live in its four topic siblings — `_consent_`, `_create_`, `_arg_`
//! and `_guard_` — one topic per file.

use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::dispatch::{enabled_tool_list, tool_call_refusal};
use super::terminal_write_harness::{
    allow_group, allow_terminal_writes, answer, call, call_on_a_thread, caller_in, field,
    join_without_a_card, keepalive_spawn, owner, pending_cards, project_workspace, refusal, serve,
    serve_with_provider, stranger_owner, wait_for_card,
};
use super::tests::{http_request, peer_row, response_json};
use super::*;
use crate::mcp_broker::tools::first_use::TERMINAL_KILL_GROUP;
use crate::provider_catalog::{
    ToolOverlay, MCP_CREATE_TERMINAL_TOOL, MCP_KILL_TERMINAL_TOOL, MCP_LIST_TERMINALS_TOOL,
    MCP_ROSTER_TOOL, MCP_SEND_TERMINAL_KEYS_TOOL,
};
use devboule_protocol::{PermissionOutcome, SessionEvent};

// --- The kind gate: the P1 -------------------------------------------------

#[test]
fn keys_and_kill_are_refused_for_an_agent_whose_stdin_stays_untouched() {
    // The target is an agent session in the *caller's own workspace*: owner
    // and workspace both pass, so the only rule that can refuse it is the
    // kind gate — and without that gate the bytes below would land in a
    // provider's stdin and the kill would end a provider session.
    let state = ServerState::new("mcp-tw-gate".to_string());
    let (workspace, _root) = project_workspace(&state, "gate");
    caller_in(&state, "tw-gate-caller", &workspace);
    let received = crate::session::insert_test_agent_in_workspace_with_recording_writer(
        &state.sessions,
        "tw-gate-agent",
        owner(),
        &workspace,
    );
    let token = serve(&state, "tw-gate-caller");
    allow_terminal_writes(&state, "tw-gate-caller");
    let _server = state.mcp.start(&state).expect("MCP server");

    let keys = call(
        &state.mcp.url,
        &token,
        1,
        MCP_SEND_TERMINAL_KEYS_TOOL,
        r#"{"terminalId":"tw-gate-agent","keys":"rm -rf /"}"#,
    );
    assert_eq!(refusal(&keys), "No session with that id.");
    let kill = call(
        &state.mcp.url,
        &token,
        2,
        MCP_KILL_TERMINAL_TOOL,
        r#"{"terminalId":"tw-gate-agent"}"#,
    );
    assert_eq!(refusal(&kill), "No session with that id.");
    assert!(
        received.lock().expect("recorder").is_empty(),
        "a refused write typed nothing into the agent"
    );

    // The agent is still running: the kill never reached the close path.
    let roster = call(&state.mcp.url, &token, 3, MCP_ROSTER_TOOL, "{}");
    let sessions = roster["result"]["structuredContent"]["agents"]
        .as_array()
        .expect("the roster answers with agents");
    let ids: Vec<&str> = sessions
        .iter()
        .map(|session| session["id"].as_str().expect("id"))
        .collect();
    assert!(
        ids.contains(&"tw-gate-agent"),
        "the agent survived both refusals: {ids:?}"
    );
}

// --- Owner and origin scope ------------------------------------------------

#[test]
fn another_users_terminal_answers_not_found_for_keys_and_kill() {
    let state = ServerState::new("mcp-tw-owner".to_string());
    let stranger = stranger_owner();
    let (workspace, _root) = project_workspace(&state, "owner");
    caller_in(&state, "tw-owner-caller", &workspace);
    let received = crate::session::insert_test_terminal_with_recording_writer(
        &state.sessions,
        "tw-owner-stranger",
        stranger.clone(),
        Some(workspace.clone()),
    );
    let token = serve(&state, "tw-owner-caller");
    allow_terminal_writes(&state, "tw-owner-caller");
    let _server = state.mcp.start(&state).expect("MCP server");

    let keys = call(
        &state.mcp.url,
        &token,
        1,
        MCP_SEND_TERMINAL_KEYS_TOOL,
        r#"{"terminalId":"tw-owner-stranger","keys":"whoami"}"#,
    );
    assert_eq!(refusal(&keys), "No session with that id.");
    let kill = call(
        &state.mcp.url,
        &token,
        2,
        MCP_KILL_TERMINAL_TOOL,
        r#"{"terminalId":"tw-owner-stranger"}"#,
    );
    assert_eq!(refusal(&kill), "No session with that id.");
    assert!(
        received.lock().expect("recorder").is_empty(),
        "a refused write typed nothing into another user's terminal"
    );

    // The stranger's terminal is still there, for the stranger's own door.
    state
        .sessions
        .terminal_target("tw-owner-stranger", &stranger, &None, &workspace)
        .expect("the refused kill changed nothing");
}

#[test]
fn a_daemon_origin_caller_writes_only_its_own_terminals() {
    // The origin rule, not the owner name: every session below shares one
    // owner user, so the owner filter alone would hand a device paired as
    // `Daemon` the person's terminal to type into and to kill. The door
    // decides, and it grants as well as refuses — the device's own terminal
    // stays writable.
    let state = ServerState::new("mcp-tw-daemon".to_string());
    let (workspace, _root) = project_workspace(&state, "daemon");
    caller_in(&state, "tw-daemon-caller", &workspace);
    state.sessions.set_test_origin(
        "tw-daemon-caller",
        devboule_protocol::SessionOrigin::peer("device-tw", crate::peer_policy::PeerRole::Daemon),
    );
    let human = crate::session::insert_test_terminal_with_recording_writer(
        &state.sessions,
        "tw-daemon-human",
        owner(),
        Some(workspace.clone()),
    );
    let own = crate::session::insert_test_terminal_with_recording_writer(
        &state.sessions,
        "tw-daemon-own",
        owner(),
        Some(workspace.clone()),
    );
    state.sessions.set_test_origin(
        "tw-daemon-own",
        devboule_protocol::SessionOrigin::peer("device-tw", crate::peer_policy::PeerRole::Daemon),
    );
    state
        .peer_upsert(peer_row(
            "device-tw",
            &["view", "send", "admin", "create_sessions"],
        ))
        .expect("store a peer");

    let token = serve(&state, "tw-daemon-caller");
    allow_terminal_writes(&state, "tw-daemon-caller");
    let _server = state.mcp.start(&state).expect("MCP server");

    let refused = call(
        &state.mcp.url,
        &token,
        1,
        MCP_SEND_TERMINAL_KEYS_TOOL,
        r#"{"terminalId":"tw-daemon-human","keys":"echo into the persons terminal"}"#,
    );
    assert_eq!(refusal(&refused), "No session with that id.");
    assert!(human.lock().expect("recorder").is_empty());

    let typed = call(
        &state.mcp.url,
        &token,
        2,
        MCP_SEND_TERMINAL_KEYS_TOOL,
        r#"{"terminalId":"tw-daemon-own","keys":"Enter"}"#,
    );
    assert_eq!(
        typed.pointer("/result/isError"),
        Some(&json!(false)),
        "its own terminal stays writable: {typed}"
    );
    assert_eq!(own.lock().expect("recorder").clone(), b"\r");

    let kill = call(
        &state.mcp.url,
        &token,
        3,
        MCP_KILL_TERMINAL_TOOL,
        r#"{"terminalId":"tw-daemon-human"}"#,
    );
    assert_eq!(refusal(&kill), "No session with that id.");

    let kill_own = call(
        &state.mcp.url,
        &token,
        4,
        MCP_KILL_TERMINAL_TOOL,
        r#"{"terminalId":"tw-daemon-own"}"#,
    );
    assert_eq!(
        kill_own.pointer("/result/isError"),
        Some(&json!(false)),
        "its own terminal is killable: {kill_own}"
    );
    let listed = call(&state.mcp.url, &token, 5, MCP_LIST_TERMINALS_TOOL, "{}");
    let terminals = listed["result"]["structuredContent"]["terminals"]
        .as_array()
        .expect("terminals");
    assert!(
        terminals.is_empty(),
        "the device's own terminal is gone and the person's was never this caller's: {terminals:?}"
    );
}

// --- Create ---------------------------------------------------------------

#[test]
fn create_opens_a_terminal_in_the_callers_workspace_and_answers_its_own_row() {
    let state = ServerState::new("mcp-tw-create".to_string());
    let (workspace, root) = project_workspace(&state, "create");
    caller_in(&state, "tw-create-caller", &workspace);
    let token = serve(&state, "tw-create-caller");
    allow_terminal_writes(&state, "tw-create-caller");
    let _server = state.mcp.start(&state).expect("MCP server");

    keepalive_spawn(&state);
    let created = call(
        &state.mcp.url,
        &token,
        1,
        MCP_CREATE_TERMINAL_TOOL,
        r#"{"name":"Build"}"#,
    );
    assert_eq!(
        created.pointer("/result/isError"),
        Some(&json!(false)),
        "the create answers: {created}"
    );
    let document = &created["result"]["structuredContent"];
    let terminal_id = field(document, "terminalId").as_str().expect("terminalId");
    assert_eq!(field(document, "title"), &json!("Build"));
    // The directory is the workspace's own, not anything an argument named.
    let expected_cwd = crate::verbatim_path::plain_path(
        &state
            .sessions
            .workspace_cwd(&workspace)
            .expect("workspace cwd")
            .to_string_lossy(),
    );
    assert_eq!(
        field(document, "cwd"),
        &json!(expected_cwd),
        "the shell opens in the caller's workspace"
    );
    assert!(
        root.ends_with("Projectcreate") || expected_cwd.contains("Projectcreate"),
        "the workspace points at this test's project folder: {expected_cwd}"
    );

    // No argument can select another workspace: the shape is closed.
    let foreign = call(
        &state.mcp.url,
        &token,
        2,
        MCP_CREATE_TERMINAL_TOOL,
        r#"{"name":"Other","workspaceId":"ws-elsewhere"}"#,
    );
    assert_eq!(foreign.pointer("/error/code"), Some(&json!(-32602)));
    assert_eq!(
        foreign.pointer("/error/message"),
        Some(&json!("unknown parameter 'workspaceId'"))
    );

    // The roster names the creator: the link the live-terminal cap counts.
    let listed = call(&state.mcp.url, &token, 3, MCP_LIST_TERMINALS_TOOL, "{}");
    let terminals = listed["result"]["structuredContent"]["terminals"]
        .as_array()
        .expect("terminals");
    assert_eq!(terminals.len(), 1, "{terminals:?}");
    assert_eq!(terminals[0]["id"], json!(terminal_id));
    assert_eq!(terminals[0]["createdBy"], json!("tw-create-caller"));

    // Cleanup through the tool itself.
    let killed = call(
        &state.mcp.url,
        &token,
        4,
        MCP_KILL_TERMINAL_TOOL,
        &format!(r#"{{"terminalId":"{terminal_id}"}}"#),
    );
    assert_eq!(killed.pointer("/result/isError"), Some(&json!(false)));
}

#[test]
fn the_live_terminal_cap_stops_one_creator() {
    let state = ServerState::new("mcp-tw-cap".to_string());
    let (workspace, _root) = project_workspace(&state, "cap");
    caller_in(&state, "tw-cap-caller", &workspace);
    let token = serve(&state, "tw-cap-caller");
    allow_terminal_writes(&state, "tw-cap-caller");
    let _server = state.mcp.start(&state).expect("MCP server");

    let mut opened = Vec::new();
    for index in 0..crate::session::MAX_LIVE_TERMINALS_PER_CREATOR {
        keepalive_spawn(&state);
        let created = call(
            &state.mcp.url,
            &token,
            (index + 1) as u64,
            MCP_CREATE_TERMINAL_TOOL,
            r#"{"name":"Capped"}"#,
        );
        assert_eq!(
            created.pointer("/result/isError"),
            Some(&json!(false)),
            "terminal {index} opens: {created}"
        );
        opened.push(
            created["result"]["structuredContent"]["terminalId"]
                .as_str()
                .expect("terminalId")
                .to_string(),
        );
    }
    // The cap refuses the next one with its one sentence — before any card
    // is spent, before any process starts.
    let refused = call(
        &state.mcp.url,
        &token,
        99,
        MCP_CREATE_TERMINAL_TOOL,
        r#"{"name":"One too many"}"#,
    );
    assert_eq!(
        refusal(&refused),
        "too many live terminals; close one first"
    );
    let listed = call(&state.mcp.url, &token, 100, MCP_LIST_TERMINALS_TOOL, "{}");
    assert_eq!(
        listed["result"]["structuredContent"]["terminals"]
            .as_array()
            .expect("terminals")
            .len(),
        crate::session::MAX_LIVE_TERMINALS_PER_CREATOR
    );

    // Closing one makes room again — the refusal's own advice, taken.
    let freed = call(
        &state.mcp.url,
        &token,
        101,
        MCP_KILL_TERMINAL_TOOL,
        &format!(r#"{{"terminalId":"{}"}}"#, opened[0]),
    );
    assert_eq!(freed.pointer("/result/isError"), Some(&json!(false)));
    keepalive_spawn(&state);
    let after = call(
        &state.mcp.url,
        &token,
        102,
        MCP_CREATE_TERMINAL_TOOL,
        r#"{"name":"Room again"}"#,
    );
    assert_eq!(
        after.pointer("/result/isError"),
        Some(&json!(false)),
        "one slot freed: {after}"
    );

    for terminal in opened.into_iter().skip(1) {
        let killed = call(
            &state.mcp.url,
            &token,
            103,
            MCP_KILL_TERMINAL_TOOL,
            &format!(r#"{{"terminalId":"{terminal}"}}"#),
        );
        assert_eq!(killed.pointer("/result/isError"), Some(&json!(false)));
    }
    let last = after["result"]["structuredContent"]["terminalId"]
        .as_str()
        .expect("terminalId")
        .to_string();
    let killed = call(
        &state.mcp.url,
        &token,
        104,
        MCP_KILL_TERMINAL_TOOL,
        &format!(r#"{{"terminalId":"{last}"}}"#),
    );
    assert_eq!(killed.pointer("/result/isError"), Some(&json!(false)));
}

#[test]
fn the_shutdown_guard_refuses_the_create_before_any_card() {
    // The guard runs before the consent card: a daemon that is going down
    // must not ask a person for anything, and it must not open a shell.
    let state = ServerState::new("mcp-tw-guard".to_string());
    let (workspace, _root) = project_workspace(&state, "guard");
    caller_in(&state, "tw-guard-caller", &workspace);
    // Deliberately no `allow_terminal_writes`: a card here would be the bug.
    let token = serve(&state, "tw-guard-caller");
    let _server = state.mcp.start(&state).expect("MCP server");

    state.request_shutdown();
    // On a thread, so a guard that stopped guarding reads as a card this
    // test names instead of a call waiting on a person who is not coming.
    let handle = call_on_a_thread(
        &state.mcp.url,
        &token,
        1,
        MCP_CREATE_TERMINAL_TOOL,
        r#"{"name":"Too late"}"#,
    );
    let refused = join_without_a_card(&state, "tw-guard-caller", handle);
    assert!(
        refusal(&refused).contains("shutting down"),
        "the guard's own sentence: {refused}"
    );
    assert!(
        pending_cards(&state, "tw-guard-caller").is_empty(),
        "a shutting-down daemon raises no card"
    );
    let listed = call(&state.mcp.url, &token, 2, MCP_LIST_TERMINALS_TOOL, "{}");
    assert_eq!(
        listed["result"]["structuredContent"]["terminals"]
            .as_array()
            .expect("terminals")
            .len(),
        0,
        "the refused create spawned nothing"
    );
}

#[test]
fn the_create_retry_answers_the_first_terminal_and_opens_no_second() {
    let state = ServerState::new("mcp-tw-retry".to_string());
    let (workspace, _root) = project_workspace(&state, "retry");
    caller_in(&state, "tw-retry-caller", &workspace);
    let token = serve(&state, "tw-retry-caller");
    allow_terminal_writes(&state, "tw-retry-caller");
    let _server = state.mcp.start(&state).expect("MCP server");

    keepalive_spawn(&state);
    let first = call(
        &state.mcp.url,
        &token,
        1,
        MCP_CREATE_TERMINAL_TOOL,
        r#"{"name":"Twice"}"#,
    );
    assert_eq!(first.pointer("/result/isError"), Some(&json!(false)));
    let opened = first["result"]["structuredContent"]["terminalId"]
        .as_str()
        .expect("terminalId")
        .to_string();

    // The same frame id with the same arguments: the first answer again,
    // no second shell, no second card.
    let retry = call(
        &state.mcp.url,
        &token,
        1,
        MCP_CREATE_TERMINAL_TOOL,
        r#"{"name":"Twice"}"#,
    );
    assert_eq!(retry.pointer("/result/isError"), Some(&json!(false)));
    assert_eq!(
        retry["result"]["structuredContent"]["terminalId"],
        json!(opened),
        "the retry answers the terminal the first call opened"
    );
    let listed = call(&state.mcp.url, &token, 2, MCP_LIST_TERMINALS_TOOL, "{}");
    assert_eq!(
        listed["result"]["structuredContent"]["terminals"]
            .as_array()
            .expect("terminals")
            .len(),
        1,
        "the retry opened nothing"
    );

    let killed = call(
        &state.mcp.url,
        &token,
        3,
        MCP_KILL_TERMINAL_TOOL,
        &format!(r#"{{"terminalId":"{opened}"}}"#),
    );
    assert_eq!(killed.pointer("/result/isError"), Some(&json!(false)));
}

// --- Consent --------------------------------------------------------------

#[test]
fn the_create_card_states_the_workspace_and_a_denied_create_writes_nothing() {
    let state = ServerState::new("mcp-tw-deny".to_string());
    let (workspace, _root) = project_workspace(&state, "deny");
    caller_in(&state, "tw-deny-caller", &workspace);
    let token = serve(&state, "tw-deny-caller");
    let _server = state.mcp.start(&state).expect("MCP server");

    let handle = call_on_a_thread(
        &state.mcp.url,
        &token,
        1,
        MCP_CREATE_TERMINAL_TOOL,
        r#"{"name":"Denied"}"#,
    );
    let card = wait_for_card(&state, "tw-deny-caller");
    let pending = state
        .sessions
        .live_runtime("tw-deny-caller", &owner())
        .expect("live session")
        .permission_broker()
        .expect("test broker")
        .test_pending_request(&card)
        .expect("the pending card");
    let SessionEvent::PermissionRequest {
        title,
        description,
        options,
        is_chooser,
        ..
    } = pending
    else {
        panic!("the gate raises a permission request");
    };
    // Answered before anything is asserted about it: an assertion that
    // failed here would leave the call parked on a card nobody answers, and
    // the suite would wait on that connection instead of reddening.
    answer(
        &state,
        "tw-deny-caller",
        &card,
        PermissionOutcome::Deny,
        "deny",
    );
    let answered = handle.join().expect("create thread");
    assert!(
        title.contains("Approve request to create a terminal"),
        "the card names the act in the infinitive the title reads after 'to': {title}"
    );
    let description = description.expect("description");
    assert!(
        description.contains(&workspace),
        "the card names the workspace: {description}"
    );
    let expected_cwd = crate::verbatim_path::plain_path(
        &state
            .sessions
            .workspace_cwd(&workspace)
            .expect("workspace cwd")
            .to_string_lossy(),
    );
    assert!(
        description.contains(&expected_cwd),
        "the card names the directory the shell would open in: {description}"
    );
    assert!(
        description.contains("Denied"),
        "the card carries the name: {description}"
    );
    assert!(
        description.contains("Allow this call") && description.contains("for this session"),
        "both choices: {description}"
    );
    let kinds: Vec<(&str, &str)> = options
        .iter()
        .map(|option| (option.option_id.as_str(), option.kind.as_str()))
        .collect();
    assert_eq!(
        kinds,
        vec![
            ("once", "allow_once"),
            ("session", "allow_session"),
            ("deny", "reject_once")
        ],
        "the ledger tells the choices apart by kind"
    );
    assert_eq!(is_chooser, Some(true), "the card is a chooser");
    assert_eq!(
        answered.pointer("/result/isError"),
        Some(&json!(true)),
        "deny refuses the call: {answered}"
    );
    assert!(
        answered["result"]["content"][0]["text"]
            .as_str()
            .is_some_and(|text| text.contains("permission refused")),
        "the refusal says what happened: {answered}"
    );

    let listed = call(&state.mcp.url, &token, 2, MCP_LIST_TERMINALS_TOOL, "{}");
    assert_eq!(
        listed["result"]["structuredContent"]["terminals"]
            .as_array()
            .expect("terminals")
            .len(),
        0,
        "a denied create opens nothing"
    );
    assert!(
        pending_cards(&state, "tw-deny-caller").is_empty(),
        "the denial leaves no card parked"
    );
}

#[test]
fn allow_once_asks_again_and_allow_for_the_session_does_not() {
    let state = ServerState::new("mcp-tw-choices".to_string());
    let (workspace, _root) = project_workspace(&state, "choices");
    caller_in(&state, "tw-choices-caller", &workspace);
    let token = serve(&state, "tw-choices-caller");
    let _server = state.mcp.start(&state).expect("MCP server");

    // "Allow this call": the call proceeds and the group stays shut.
    keepalive_spawn(&state);
    let once = call_on_a_thread(
        &state.mcp.url,
        &token,
        1,
        MCP_CREATE_TERMINAL_TOOL,
        r#"{"name":"Once"}"#,
    );
    let card = wait_for_card(&state, "tw-choices-caller");
    answer(
        &state,
        "tw-choices-caller",
        &card,
        PermissionOutcome::AllowOnce,
        "once",
    );
    let first = once.join().expect("create thread");
    assert_eq!(first.pointer("/result/isError"), Some(&json!(false)));

    // So the next call asks again — and this time the person opens the group.
    keepalive_spawn(&state);
    let session = call_on_a_thread(
        &state.mcp.url,
        &token,
        2,
        MCP_CREATE_TERMINAL_TOOL,
        r#"{"name":"Session"}"#,
    );
    let card = wait_for_card(&state, "tw-choices-caller");
    answer(
        &state,
        "tw-choices-caller",
        &card,
        PermissionOutcome::AllowOnce,
        "session",
    );
    let second = session.join().expect("create thread");
    assert_eq!(second.pointer("/result/isError"), Some(&json!(false)));

    // "For this session": no further card, and the write just runs.
    keepalive_spawn(&state);
    let third_handle = call_on_a_thread(
        &state.mcp.url,
        &token,
        3,
        MCP_CREATE_TERMINAL_TOOL,
        r#"{"name":"Third"}"#,
    );
    let third = join_without_a_card(&state, "tw-choices-caller", third_handle);
    assert_eq!(
        third.pointer("/result/isError"),
        Some(&json!(false)),
        "the open group asks nothing: {third}"
    );
    assert!(pending_cards(&state, "tw-choices-caller").is_empty());

    // Killing is its own act: the create group opened above says nothing
    // about it, so cleanup asks for it once here.
    allow_group(&state, "tw-choices-caller", TERMINAL_KILL_GROUP);
    let listed = call(&state.mcp.url, &token, 4, MCP_LIST_TERMINALS_TOOL, "{}");
    let terminals = listed["result"]["structuredContent"]["terminals"]
        .as_array()
        .expect("terminals");
    assert_eq!(terminals.len(), 3, "{terminals:?}");
    for terminal in terminals {
        let terminal_id = terminal["id"].as_str().expect("id");
        let killed = call(
            &state.mcp.url,
            &token,
            5,
            MCP_KILL_TERMINAL_TOOL,
            &format!(r#"{{"terminalId":"{terminal_id}"}}"#),
        );
        assert_eq!(killed.pointer("/result/isError"), Some(&json!(false)));
    }
}

#[test]
fn the_keys_card_counts_the_keys_and_never_carries_them() {
    // Keys may be a password: the card names the terminal and how much will
    // be typed, and a denial writes nothing at all.
    let state = ServerState::new("mcp-tw-keycard".to_string());
    let (workspace, _root) = project_workspace(&state, "keycard");
    caller_in(&state, "tw-keycard-caller", &workspace);
    let received = crate::session::insert_test_terminal_with_recording_writer(
        &state.sessions,
        "tw-keycard-terminal",
        owner(),
        Some(workspace.clone()),
    );
    let token = serve(&state, "tw-keycard-caller");
    let _server = state.mcp.start(&state).expect("MCP server");

    let secret = "s3cr3t-passphrase";
    let handle = call_on_a_thread(
        &state.mcp.url,
        &token,
        1,
        MCP_SEND_TERMINAL_KEYS_TOOL,
        &format!(r#"{{"terminalId":"tw-keycard-terminal","keys":"{secret}","literal":true}}"#),
    );
    let card = wait_for_card(&state, "tw-keycard-caller");
    let pending = state
        .sessions
        .live_runtime("tw-keycard-caller", &owner())
        .expect("live session")
        .permission_broker()
        .expect("test broker")
        .test_pending_request(&card)
        .expect("the pending card");
    let SessionEvent::PermissionRequest {
        title, description, ..
    } = pending
    else {
        panic!("the gate raises a permission request");
    };
    // Answered before anything is asserted about it, so a failing assertion
    // reddens the run instead of parking the call on a card nobody answers.
    answer(
        &state,
        "tw-keycard-caller",
        &card,
        PermissionOutcome::Deny,
        "deny",
    );
    let answered = handle.join().expect("keys thread");
    let description = description.expect("description");
    for text in [&title, &description] {
        assert!(
            !text.contains(secret),
            "the card never carries the typed keys: {text}"
        );
        assert!(
            !text.contains("s3cr3t"),
            "not even a prefix of them: {text}"
        );
    }
    assert!(
        title.contains("Approve request to send keys to terminal 'Terminal'"),
        "the card names the act and the target: {title}"
    );
    assert!(
        description.contains("17 characters, not shown"),
        "the card counts the keys instead of showing them: {description}"
    );
    assert!(
        description.contains(&workspace),
        "the card says which workspace the terminal is in: {description}"
    );
    assert!(
        description.contains("opened by: you"),
        "the card says whose terminal it is: {description}"
    );
    assert!(
        description.contains("started in: /tmp/devboule-terminal"),
        "the card says where the terminal started, not where it may be now: {description}"
    );
    assert_eq!(answered.pointer("/result/isError"), Some(&json!(true)));
    assert!(
        received.lock().expect("recorder").is_empty(),
        "a denied write typed nothing"
    );
}

// --- Keys -----------------------------------------------------------------

#[test]
fn keys_type_literal_text_and_named_keys_into_the_pty() {
    // Paseo's input shape, byte for byte: a named key is resolved, literal
    // text is written as typed, and a name the token list does not hold is
    // the text it is (Paseo's switch falls through the same way).
    let state = ServerState::new("mcp-tw-keys".to_string());
    let (workspace, _root) = project_workspace(&state, "keys");
    caller_in(&state, "tw-keys-caller", &workspace);
    let received = crate::session::insert_test_terminal_with_recording_writer(
        &state.sessions,
        "tw-keys-terminal",
        owner(),
        Some(workspace.clone()),
    );
    let token = serve(&state, "tw-keys-caller");
    allow_terminal_writes(&state, "tw-keys-caller");
    let _server = state.mcp.start(&state).expect("MCP server");

    let type_keys = |id: u64, arguments: &str| {
        let answer = call(
            &state.mcp.url,
            &token,
            id,
            MCP_SEND_TERMINAL_KEYS_TOOL,
            arguments,
        );
        assert_eq!(
            answer.pointer("/result/isError"),
            Some(&json!(false)),
            "{arguments}: {answer}"
        );
        answer
    };

    type_keys(1, r#"{"terminalId":"tw-keys-terminal","keys":"echo hi"}"#);
    type_keys(2, r#"{"terminalId":"tw-keys-terminal","keys":"Enter"}"#);
    type_keys(3, r#"{"terminalId":"tw-keys-terminal","keys":"C-c"}"#);
    type_keys(4, r#"{"terminalId":"tw-keys-terminal","keys":"Tab"}"#);
    type_keys(
        5,
        r#"{"terminalId":"tw-keys-terminal","keys":"Enter","literal":true}"#,
    );
    assert_eq!(
        received.lock().expect("recorder").clone(),
        b"echo hi\r\x03\tEnter",
        "the pty received exactly what Paseo would have sent"
    );

    // The shape is closed: unknown parameters and a missing payload are
    // malformed requests, not silently ignored.
    let unknown = call(
        &state.mcp.url,
        &token,
        6,
        MCP_SEND_TERMINAL_KEYS_TOOL,
        r#"{"terminalId":"tw-keys-terminal","keys":"x","up":true}"#,
    );
    assert_eq!(unknown.pointer("/error/code"), Some(&json!(-32602)));
    let missing = call(
        &state.mcp.url,
        &token,
        7,
        MCP_SEND_TERMINAL_KEYS_TOOL,
        r#"{"terminalId":"tw-keys-terminal"}"#,
    );
    assert_eq!(
        missing.pointer("/error/message"),
        Some(&json!("keys is required"))
    );
}

#[test]
fn keys_refuse_a_payload_over_the_write_cap() {
    // The cap is the tool's own contract — `MAX_WRITE_BYTES`, the wire's
    // number — and it is stated where no transport is assumed to enforce it:
    // the loopback HTTP door has a body limit of its own, so both halves of
    // the boundary are driven through the body directly. The gate is opened
    // first, so the second call types instead of asking.
    let state = ServerState::new("mcp-tw-bigkeys".to_string());
    let (workspace, _root) = project_workspace(&state, "bigkeys");
    caller_in(&state, "tw-bigkeys-caller", &workspace);
    let received = crate::session::insert_test_terminal_with_recording_writer(
        &state.sessions,
        "tw-bigkeys-terminal",
        owner(),
        Some(workspace.clone()),
    );
    // No bearer and no listener here: the body is called directly, which is
    // the point of the test — the HTTP door's own 64 KiB body limit would
    // refuse these bytes before the tool ever saw them.
    allow_terminal_writes(&state, "tw-bigkeys-caller");

    let registration = RegisteredSession {
        session_id: "tw-bigkeys-caller".to_string(),
        owner: owner(),
        provider_id: None,
        depth: 0,
        overlay: ToolOverlay::NONE,
        bearer: String::new(),
        claude_config_path: None,
        runtime: None,
        broker_ready: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    let type_body = |keys: String| {
        let message = json!({
            "params": {
                "name": MCP_SEND_TERMINAL_KEYS_TOOL,
                "arguments": {"terminalId": "tw-bigkeys-terminal", "keys": keys},
            }
        });
        super::tools::terminal_writes::send_keys(
            &state,
            &state.mcp,
            &registration,
            super::caller::McpCaller::Local,
            json!(1),
            &message,
        )
        .expect("the body answers")
        .expect("a reply document")
    };

    // One byte over the cap: refused with the wire's own sentence, and
    // nothing reaches the pty.
    let refused = type_body("x".repeat(devboule_protocol::MAX_WRITE_BYTES + 1));
    assert_eq!(refused["result"]["isError"], json!(true), "{refused}");
    assert_eq!(
        refused["result"]["content"][0]["text"],
        json!("Session input is too large."),
        "the wire's own sentence, on the bytes as they arrived"
    );
    assert!(
        received.lock().expect("recorder").is_empty(),
        "an over-cap payload never reaches the pty"
    );

    // Exactly the cap: typed. The boundary is a cap, not a wall.
    let at_cap = type_body("y".repeat(devboule_protocol::MAX_WRITE_BYTES));
    assert_eq!(at_cap["result"]["isError"], json!(false), "{at_cap}");
    assert_eq!(
        received.lock().expect("recorder").len(),
        devboule_protocol::MAX_WRITE_BYTES
    );
}

// --- Kill -----------------------------------------------------------------

#[test]
fn kill_ends_the_terminal_and_keeps_its_row() {
    let state = ServerState::new("mcp-tw-kill".to_string());
    let (workspace, _root) = project_workspace(&state, "kill");
    caller_in(&state, "tw-kill-caller", &workspace);
    let token = serve(&state, "tw-kill-caller");
    allow_terminal_writes(&state, "tw-kill-caller");
    let _server = state.mcp.start(&state).expect("MCP server");

    keepalive_spawn(&state);
    let created = call(
        &state.mcp.url,
        &token,
        1,
        MCP_CREATE_TERMINAL_TOOL,
        r#"{"name":"Doomed"}"#,
    );
    assert_eq!(created.pointer("/result/isError"), Some(&json!(false)));
    let terminal_id = created["result"]["structuredContent"]["terminalId"]
        .as_str()
        .expect("terminalId")
        .to_string();

    let killed = call(
        &state.mcp.url,
        &token,
        2,
        MCP_KILL_TERMINAL_TOOL,
        &format!(r#"{{"terminalId":"{terminal_id}"}}"#),
    );
    assert_eq!(killed.pointer("/result/isError"), Some(&json!(false)));

    // The live entry is gone from the roster, and a second kill is as
    // unknown as an id the daemon never saw.
    let listed = call(&state.mcp.url, &token, 3, MCP_LIST_TERMINALS_TOOL, "{}");
    assert_eq!(
        listed["result"]["structuredContent"]["terminals"]
            .as_array()
            .expect("terminals")
            .len(),
        0
    );
    let again = call(
        &state.mcp.url,
        &token,
        4,
        MCP_KILL_TERMINAL_TOOL,
        &format!(r#"{{"terminalId":"{terminal_id}"}}"#),
    );
    assert_eq!(refusal(&again), "No session with that id.");

    // The row survives with the close mark, and the daemon observes the
    // process gone: the exit mark the child waiter writes is what makes
    // "the process tree is dead" a fact the journal can answer to.
    let journal = state.sessions.runtime_dir().join("journal.db");
    let start = Instant::now();
    loop {
        let connection = rusqlite::Connection::open(&journal).expect("journal db");
        let closed: Option<i64> = connection
            .query_row(
                "SELECT closed FROM sessions WHERE id = ?1",
                [&terminal_id],
                |row| row.get(0),
            )
            .ok();
        let reaped: Option<i64> = connection
            .query_row(
                "SELECT reaped FROM sessions WHERE id = ?1",
                [&terminal_id],
                |row| row.get(0),
            )
            .ok();
        let title: Option<String> = connection
            .query_row(
                "SELECT title FROM sessions WHERE id = ?1",
                [&terminal_id],
                |row| row.get(0),
            )
            .ok();
        assert_eq!(
            title.as_deref(),
            Some("Doomed"),
            "the row survives with its title: {title:?}"
        );
        if closed == Some(1) && reaped == Some(1) {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "the close mark and the exit mark: closed={closed:?} reaped={reaped:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

// --- Audit and privacy ----------------------------------------------------

#[test]
fn every_terminal_write_is_audited_and_the_keys_are_not() {
    let state = ServerState::new("mcp-tw-audit".to_string());
    let (workspace, _root) = project_workspace(&state, "audit");
    caller_in(&state, "tw-audit-caller", &workspace);
    let received = crate::session::insert_test_terminal_with_recording_writer(
        &state.sessions,
        "tw-audit-terminal",
        owner(),
        Some(workspace.clone()),
    );
    let token = serve(&state, "tw-audit-caller");
    allow_terminal_writes(&state, "tw-audit-caller");
    let _server = state.mcp.start(&state).expect("MCP server");

    let secret = "audit-must-not-see-this";
    let typed = call(
        &state.mcp.url,
        &token,
        1,
        MCP_SEND_TERMINAL_KEYS_TOOL,
        &format!(r#"{{"terminalId":"tw-audit-terminal","keys":"{secret}","literal":true}}"#),
    );
    assert_eq!(typed.pointer("/result/isError"), Some(&json!(false)));
    assert_eq!(
        received.lock().expect("recorder").clone(),
        secret.as_bytes(),
        "the pty got the bytes"
    );
    let unknown = call(
        &state.mcp.url,
        &token,
        2,
        MCP_KILL_TERMINAL_TOOL,
        r#"{"terminalId":"tw-audit-unknown"}"#,
    );
    assert_eq!(unknown.pointer("/result/isError"), Some(&json!(true)));

    let runtime_dir = state.sessions.runtime_dir().to_path_buf();
    let connection =
        rusqlite::Connection::open(runtime_dir.join("journal.db")).expect("journal db");
    let mut statement = connection
        .prepare("SELECT action, session_id, outcome FROM audit ORDER BY id")
        .expect("prepare");
    let rows: Vec<(String, Option<String>, String)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .expect("query")
        .map(Result::unwrap)
        .collect();
    let caller = Some("tw-audit-caller".to_string());
    for (action, outcome) in [
        (MCP_SEND_TERMINAL_KEYS_TOOL, "ok"),
        (MCP_KILL_TERMINAL_TOOL, "denied"),
    ] {
        assert!(
            rows.contains(&(action.to_string(), caller.clone(), outcome.to_string())),
            "{action} as {outcome} is audited with its actor session: {rows:?}"
        );
    }

    // The act, never the bytes: nothing in the whole audit table spells what
    // was typed.
    let mut dump = connection.prepare("SELECT * FROM audit").expect("prepare");
    let columns = dump.column_count();
    let mut leaks = Vec::new();
    let mut rows_iter = dump.query([]).expect("query");
    while let Some(row) = rows_iter.next().expect("row") {
        for column in 0..columns {
            if let Ok(text) = row.get::<_, String>(column) {
                if text.contains(secret) {
                    leaks.push(text);
                }
            }
        }
    }
    assert!(
        leaks.is_empty(),
        "the audit row records the act, not the bytes: {leaks:?}"
    );
}

// --- The doors ------------------------------------------------------------

#[test]
fn the_writes_are_served_with_closed_schemas_and_design_hides_them() {
    let state = ServerState::new("mcp-tw-doors".to_string());
    let (workspace, _root) = project_workspace(&state, "doors");
    caller_in(&state, "tw-doors-caller", &workspace);
    let token = serve(&state, "tw-doors-caller");
    let _server = state.mcp.start(&state).expect("MCP server");

    let listed = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
    );
    let listed_body = response_json(&listed);
    let tools = listed_body["result"]["tools"].as_array().expect("tools");
    let schema_of = |name: &str| {
        tools
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap_or_else(|| panic!("{name} is served"))["inputSchema"]
            .clone()
    };
    assert_eq!(
        schema_of(MCP_CREATE_TERMINAL_TOOL),
        json!({
            "type": "object",
            "properties": {
                "name": {"type": "string", "description": "The terminal's display name, at most 60 characters. Empty means untitled."},
            },
            "additionalProperties": false,
        }),
        "the create schema states its one optional field and accepts nothing else"
    );
    let keys = schema_of(MCP_SEND_TERMINAL_KEYS_TOOL);
    assert_eq!(keys["additionalProperties"], json!(false));
    assert_eq!(keys["required"], json!(["terminalId", "keys"]));
    assert_eq!(keys["properties"].as_object().expect("properties").len(), 3);
    let kill = schema_of(MCP_KILL_TERMINAL_TOOL);
    assert_eq!(kill["required"], json!(["terminalId"]));
    assert_eq!(kill["additionalProperties"], json!(false));

    // The design preset denies all three and keeps the reads.
    let design = enabled_tool_list(
        crate::provider_catalog::MCP_BROKER_TOOLS,
        None,
        ToolOverlay::DESIGN.clone(),
    );
    let names: Vec<&str> = design
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .collect();
    for name in [
        MCP_CREATE_TERMINAL_TOOL,
        MCP_SEND_TERMINAL_KEYS_TOOL,
        MCP_KILL_TERMINAL_TOOL,
    ] {
        assert!(!names.contains(&name), "{name} is hidden from design");
        assert_eq!(
            tool_call_refusal(None, &ToolOverlay::DESIGN, name),
            Some("Tool disabled by policy"),
            "{name}"
        );
        assert_eq!(tool_call_refusal(None, &ToolOverlay::NONE, name), None);
    }
    assert!(names.contains(&MCP_LIST_TERMINALS_TOOL));
    assert!(names.contains(&MCP_ROSTER_TOOL));
}

#[test]
fn a_stored_policy_can_take_the_terminal_writes_away() {
    let state = ServerState::new("mcp-tw-policy".to_string());
    let (workspace, _root) = project_workspace(&state, "policy");
    caller_in(&state, "tw-policy-caller", &workspace);
    allow_terminal_writes(&state, "tw-policy-caller");
    let token = serve_with_provider(&state, "tw-policy-caller", "claude");
    state
        .tool_policy
        .set(
            "claude",
            Some(true),
            vec![
                MCP_CREATE_TERMINAL_TOOL.to_string(),
                MCP_SEND_TERMINAL_KEYS_TOOL.to_string(),
                MCP_KILL_TERMINAL_TOOL.to_string(),
            ],
        )
        .expect("policy");
    let _server = state.mcp.start(&state).expect("MCP server");

    let listed = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
    );
    let listed_body = response_json(&listed);
    let names: Vec<&str> = listed_body["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    for name in [
        MCP_CREATE_TERMINAL_TOOL,
        MCP_SEND_TERMINAL_KEYS_TOOL,
        MCP_KILL_TERMINAL_TOOL,
    ] {
        assert!(!names.contains(&name), "{name} is disabled by policy");
        let refused = call(&state.mcp.url, &token, 2, name, r#"{"terminalId":"x"}"#);
        assert_eq!(refused.pointer("/error/code"), Some(&json!(-32601)));
        assert_eq!(
            refused.pointer("/error/message"),
            Some(&json!("Tool disabled by policy")),
            "{name}"
        );
    }
}
