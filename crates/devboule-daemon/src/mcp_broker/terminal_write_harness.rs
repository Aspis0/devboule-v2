//! The harness the terminal-write tests drive: one owner, one project
//! workspace with a caller in it, the loopback call, the spawn override that
//! keeps a test's terminals cheap, and the first-use card's plumbing —
//! raised, answered and (where a test needs silence) waited out.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::tests::{http_request, response_json};
use super::*;
use crate::mcp_broker::tools::first_use::{
    ensure_write_allowed, TERMINAL_CREATE_GROUP, TERMINAL_KEYS_GROUP, TERMINAL_KILL_GROUP,
};
use crate::provider_catalog::MCP_LIST_TERMINALS_TOOL;
use devboule_protocol::{OwnerId, PermissionOutcome, WorkspaceIsolation};

/// The one owner every terminal-write test belongs to: each test builds its
/// own `ServerState`, so a fixed owner is enough and the fixtures below it
/// need no parameter for it.
pub(super) fn owner() -> OwnerId {
    OwnerId::new("S-1-5-21-term-writes", "term-writes-client").expect("owner")
}

/// Somebody else's account, for the one test that needs a terminal this
/// caller may not touch.
pub(super) fn stranger_owner() -> OwnerId {
    OwnerId::new("S-1-5-21-term-writes-other", "term-writes-other-client").expect("stranger owner")
}

/// One project folder with a local workspace row, as production builds it:
/// the workspace the caller's row will name, and the whole scope the three
/// writes have. Returns the workspace id and the project folder it points at.
pub(super) fn project_workspace(
    state: &Arc<ServerState>,
    tag: &str,
) -> (String, std::path::PathBuf) {
    let dir = crate::test_dirs::test_temp_dir(&format!("devboule-term-write-{tag}"));
    let root = dir.join(format!("Project{tag}"));
    std::fs::create_dir_all(&root).expect("project folder");
    let project = state
        .sessions
        .project_add(root.to_str().expect("project path"))
        .expect("project row");
    let workspace = state
        .sessions
        .workspace_create(&project.id, WorkspaceIsolation::Local, None)
        .expect("workspace row");
    (workspace.id, root)
}

/// The caller session the bearer authenticates, living in `workspace_id`.
pub(super) fn caller_in(state: &Arc<ServerState>, id: &str, workspace_id: &str) {
    crate::session::insert_test_live_agent_in_workspace(&state.sessions, id, owner(), workspace_id);
}

/// The bearer for one registered caller, with the registration forgotten on
/// purpose: the guard outlives this frame, so the session stays registered
/// for the whole test (the shape `mcp_workspaces_tests` uses).
pub(super) fn serve(state: &Arc<ServerState>, session: &str) -> String {
    let guard = state
        .mcp
        .register(session, &owner(), &devboule_protocol::SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    std::mem::forget(guard);
    state.mcp.test_token(session).expect("token")
}

/// The bearer of a caller whose provider a stored policy can speak to.
pub(super) fn serve_with_provider(
    state: &Arc<ServerState>,
    session: &str,
    provider: &str,
) -> String {
    let guard = state
        .mcp
        .register_with_provider(
            session,
            &owner(),
            &devboule_protocol::SessionKind::Acp,
            Some(provider),
            AgentLineage::root(),
        )
        .expect("registration")
        .expect("MCP guard");
    std::mem::forget(guard);
    state.mcp.test_token(session).expect("token")
}

/// One `tools/call` against the loopback broker, answered already parsed.
pub(super) fn call(url: &str, token: &str, id: u64, name: &str, arguments: &str) -> Value {
    response_json(&http_request(
        url,
        Some(&format!("Bearer {token}")),
        &format!(
            r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"{name}","arguments":{arguments}}}}}"#
        ),
    ))
}

/// The sentence every id outside the scope answers, and never a word about
/// which refusal it was.
pub(super) fn refusal(body: &Value) -> &str {
    assert_eq!(
        body.pointer("/result/isError"),
        Some(&json!(true)),
        "a write refusal is a tool error: {body}"
    );
    body.pointer("/result/content/0/text")
        .and_then(Value::as_str)
        .expect("the refusal sentence")
}

/// The spawn override the create road consumes in debug builds: a terminal
/// that lingers for a minute and then leaves on its own, so a create here
/// starts something cheap and a test that forgets to kill its terminals
/// does not leave a shell behind. Written before *each* create — the road
/// consumes the file.
pub(super) fn keepalive_spawn(state: &Arc<ServerState>) {
    let paths = crate::paths::RuntimePaths::from_dir(state.sessions.runtime_dir());
    // The platform's own long-lived program does the same job on both:
    // `ping` on Windows, `sleep` on Unix.
    #[cfg(windows)]
    let (program, args) = (
        "cmd.exe",
        vec![
            "/c".to_string(),
            "ping".to_string(),
            "-n".to_string(),
            "60".to_string(),
            "127.0.0.1".to_string(),
        ],
    );
    #[cfg(not(windows))]
    let (program, args) = ("/bin/sleep", vec!["60".to_string()]);
    crate::session::write_test_pty_command(
        &paths,
        &crate::session::PtyCommand::new(
            program,
            args,
            crate::test_dirs::test_temp_dir("devboule-term-write-pty"),
            Vec::new(),
        ),
    )
    .expect("spawn override written");
}

pub(super) fn pending_cards(state: &Arc<ServerState>, session: &str) -> Vec<String> {
    state
        .sessions
        .live_runtime(session, &owner())
        .expect("live session")
        .permission_broker()
        .expect("test broker")
        .test_pending_ids()
}

/// The terminal-write card, waited for: a write runs on another thread,
/// because the gate blocks until a person answers.
pub(super) fn wait_for_card(state: &Arc<ServerState>, session: &str) -> String {
    let start = Instant::now();
    loop {
        let mut ids = pending_cards(state, session);
        if let Some(id) = ids.pop() {
            return id;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "the write raised no card"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

pub(super) fn answer(
    state: &Arc<ServerState>,
    session: &str,
    card: &str,
    outcome: PermissionOutcome,
    option: &str,
) {
    state
        .sessions
        .live_runtime(session, &owner())
        .expect("live session")
        .permission_broker()
        .expect("test broker")
        .test_answer(card, outcome, option)
        .expect("answer the card");
}

/// Open one terminal-write group the way a person does, so the tests about
/// something else call the writes synchronously: raise it on a throwaway
/// thread and answer "for this session".
pub(super) fn allow_group(state: &Arc<ServerState>, session: &str, group: &str) {
    let thread_state = Arc::clone(state);
    let thread_session = session.to_string();
    let group = group.to_string();
    let handle = std::thread::spawn(move || {
        ensure_write_allowed(
            &thread_state,
            &thread_state.mcp,
            &thread_session,
            &owner(),
            &group,
            "testing the gate",
            &[("fact", "value")],
        )
    });
    let card = wait_for_card(state, session);
    answer(
        state,
        session,
        &card,
        PermissionOutcome::AllowOnce,
        "session",
    );
    assert!(handle.join().expect("gate thread").is_ok());
}

/// All three groups opened — one card per act — for the tests whose subject
/// is a write, not the consent. The consent tests open one group and card
/// the others with [`allow_group`] on its own.
pub(super) fn allow_terminal_writes(state: &Arc<ServerState>, session: &str) {
    for group in [
        TERMINAL_CREATE_GROUP,
        TERMINAL_KEYS_GROUP,
        TERMINAL_KILL_GROUP,
    ] {
        allow_group(state, session, group);
    }
}

/// A `tools/call` on its own thread, so a card can be answered while it
/// waits.
pub(super) fn call_on_a_thread(
    url: &str,
    token: &str,
    id: u64,
    name: &str,
    arguments: &str,
) -> std::thread::JoinHandle<Value> {
    let url = url.to_string();
    let token = token.to_string();
    let body = format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"{name}","arguments":{arguments}}}}}"#
    );
    std::thread::spawn(move || {
        let authorization = format!("Bearer {token}");
        response_json(&http_request(&url, Some(&authorization), &body))
    })
}

/// Join a call that must have passed the gate *without* raising a card: a
/// card that appears is answered (so the thread can finish) and then named
/// as the failure, so a regressed gate reddens this test instead of hanging
/// the run.
pub(super) fn join_without_a_card(
    state: &Arc<ServerState>,
    session: &str,
    handle: std::thread::JoinHandle<Value>,
) -> Value {
    let start = Instant::now();
    loop {
        let mut cards = pending_cards(state, session);
        if let Some(card) = cards.pop() {
            answer(state, session, &card, PermissionOutcome::Deny, "deny");
            let answer = handle.join().expect("call thread");
            panic!("the write raised a card again: {answer}");
        }
        if handle.is_finished() {
            return handle.join().expect("call thread");
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "the write answered neither a card nor a reply"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// The caller's live terminals as the roster answers them: the count several
/// terminal-write tests pin, read through the tool so the roster's own scope
/// is what is being counted.
pub(super) fn live_terminals(state: &Arc<ServerState>, token: &str) -> Vec<String> {
    let listed = call(&state.mcp.url, token, 99, MCP_LIST_TERMINALS_TOOL, "{}");
    listed["result"]["structuredContent"]["terminals"]
        .as_array()
        .expect("terminals")
        .iter()
        .map(|terminal| terminal["id"].as_str().expect("id").to_string())
        .collect()
}

/// One value a document must carry, or the test says which one is missing.
pub(super) fn field<'a>(document: &'a Value, name: &str) -> &'a Value {
    document
        .get(name)
        .unwrap_or_else(|| panic!("the reply carries {name}: {document}"))
}
