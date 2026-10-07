//! `devboule_ci_watch` over the real broker: the closed argument set, the
//! result envelope, and a verdict that reaches the caller's own session once.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use devboule_protocol::{OwnerId, PermissionOutcome, SessionKind};
use serde_json::{json, Value};

use crate::ci_test_support::{
    branch_head, check_run, check_runs, fail, github_origin, ok, ScriptedRunner, SHA,
};
use crate::mcp_broker::terminal_write_harness::project_workspace;
use crate::mcp_broker::tests::{http_request, owner, response_json};
use crate::mcp_broker::tools::first_use::CI_RETRY_GROUP;
use crate::provider_catalog::MCP_CI_WATCH_TOOL;
use crate::server::ServerState;

struct Fixture {
    state: Arc<ServerState>,
    runner: Arc<ScriptedRunner>,
    owner: OwnerId,
    token: String,
    received: Arc<Mutex<Vec<u8>>>,
    _guard: crate::mcp_broker::McpSessionGuard,
    _server: crate::mcp_broker::McpServerHandle,
}

fn fixture(tag: &str) -> Fixture {
    let runner = Arc::new(github_origin());
    let state = ServerState::with_ci_runner(format!("mcp-ci-{tag}"), runner.clone());
    let owner = owner(&format!("mcp-ci-user-{tag}"), "mcp-ci-client");
    let session = "ci.caller";
    let received = crate::session::insert_test_live_agent_with_recording_writer(
        &state.sessions,
        session,
        owner.clone(),
        SessionKind::Pi,
    );
    let guard = state
        .mcp
        .register(session, &owner, &SessionKind::Pi)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token(session).expect("token");
    let server = state.mcp.start(&state).expect("MCP server");
    Fixture {
        state,
        runner,
        owner,
        token,
        received,
        _guard: guard,
        _server: server,
    }
}

fn call(fixture: &Fixture, arguments: &str) -> Value {
    response_json(&http_request(
        &fixture.state.mcp.url,
        Some(&format!("Bearer {}", fixture.token)),
        &format!(
            r#"{{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{{"name":"{MCP_CI_WATCH_TOOL}","arguments":{arguments}}}}}"#
        ),
    ))
}

#[test]
fn the_tool_is_served_with_a_closed_schema() {
    let fixture = fixture("schema");
    let listed = response_json(&http_request(
        &fixture.state.mcp.url,
        Some(&format!("Bearer {}", fixture.token)),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
    ));
    let tool = listed["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|tool| tool["name"] == MCP_CI_WATCH_TOOL)
        .expect("the CI watch is served")
        .clone();
    assert_eq!(tool["inputSchema"]["additionalProperties"], json!(false));
    let properties = tool["inputSchema"]["properties"]
        .as_object()
        .expect("properties");
    for name in ["sha", "branch", "repo", "retryInfra"] {
        assert!(properties.contains_key(name), "{name} is accepted");
    }
    assert_eq!(properties.len(), 4, "and nothing else is");
    assert!(
        tool["inputSchema"].get("required").is_none(),
        "exactly one of sha and branch is a rule the schema cannot express"
    );
}

#[test]
fn malformed_arguments_are_refused_before_github_is_asked() {
    let fixture = fixture("args");
    for arguments in [
        "{}",
        r#"{"sha":"abc123"}"#,
        r#"{"sha":"0123456789abcdef0123456789abcdef0123456z"}"#,
        r#"{"branch":""}"#,
        r#"{"branch":"main..dev"}"#,
        r#"{"branch":"../commits"}"#,
        r#"{"branch":"feature branch"}"#,
        &format!(r#"{{"sha":"{SHA}","branch":"main"}}"#),
        &format!(r#"{{"sha":"{SHA}","repo":"not a repo"}}"#),
        &format!(r#"{{"sha":"{SHA}","repo":"evil.com/attacker/gadget"}}"#),
        &format!(r#"{{"sha":"{SHA}","repo":7}}"#),
        &format!(r#"{{"sha":"{SHA}","retryInfra":"yes"}}"#),
    ] {
        let body = call(&fixture, arguments);
        assert_eq!(
            body.pointer("/error/code"),
            Some(&json!(-32602)),
            "{arguments}: {body}"
        );
    }
    assert!(fixture.runner.calls().is_empty(), "GitHub was never asked");
}

#[test]
fn a_session_with_no_workspace_has_no_origin_host() {
    let fixture = fixture("norepo");
    // Without the repo the session names nothing; with one the host still
    // cannot come from the argument, so both spellings refuse the same way.
    for arguments in [
        format!(r#"{{"sha":"{SHA}"}}"#),
        format!(r#"{{"sha":"{SHA}","repo":"acme/widgets"}}"#),
    ] {
        let body = call(&fixture, &arguments);
        assert_eq!(
            body.pointer("/result/isError"),
            Some(&json!(true)),
            "{body}"
        );
        assert_eq!(
            body.pointer("/result/structuredContent/error/code"),
            Some(&json!("repo_not_github")),
            "{body}"
        );
    }
}

/// The login is a GitHub read, so the call does not make it: the watch is
/// registered at once and the owner learns the step to take from the wake.
#[test]
fn a_missing_login_is_reported_by_the_wake_not_by_the_call() {
    let fixture = workspace_fixture("login");
    fixture.runner.set(
        &format!("commits/{SHA}/check-runs"),
        fail(
            4,
            "To get started with GitHub CLI, please run:  gh auth login",
        ),
    );
    let body = call(
        &fixture,
        &format!(r#"{{"sha":"{SHA}","repo":"acme/widgets"}}"#),
    );
    assert_eq!(
        body.pointer("/result/isError"),
        Some(&json!(false)),
        "{body}"
    );
    let envelope = &body["result"]["structuredContent"];
    assert_eq!(envelope["ok"], json!(true));
    assert_eq!(envelope["data"]["state"], json!("queued"));

    // The daemon's own poll thread may get there first; either pass delivers.
    fixture.state.ci_watches.poll_once(&fixture.state.sessions);
    let delivered = wait_for_delivery(&fixture, WAKE_END);
    assert!(delivered.contains("github_auth_required"), "{delivered}");
    assert!(delivered.contains("gh auth login"), "{delivered}");
}

/// The last thing a daemon message says: the wake is whole once it is there.
const WAKE_END: &str = "</devboule-system>";

/// What the session's writer holds once `needle` is in it. The daemon's own
/// poll thread runs the same passes as the test's, so whichever of them makes
/// the wake may still be writing when the other returns.
fn wait_for_delivery(fixture: &Fixture, needle: &str) -> String {
    for _ in 0..100 {
        let received = fixture.received.lock().expect("received").clone();
        let text = String::from_utf8_lossy(&received).into_owned();
        if text.contains(needle) {
            return text;
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    String::from_utf8_lossy(&fixture.received.lock().expect("received")).into_owned()
}

fn workspace_fixture(tag: &str) -> Fixture {
    let runner = Arc::new(github_origin());
    let state = ServerState::with_ci_runner(format!("mcp-ci-{tag}"), runner.clone());
    let owner = owner(&format!("mcp-ci-user-{tag}"), "mcp-ci-client");
    let (workspace, _root) = project_workspace(&state, tag);
    let session = "ci.host";
    let received = crate::session::insert_test_agent_in_workspace_with_recording_writer(
        &state.sessions,
        session,
        owner.clone(),
        &workspace,
    );
    let guard = state
        .mcp
        .register(session, &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token(session).expect("token");
    let server = state.mcp.start(&state).expect("MCP server");
    Fixture {
        state,
        runner,
        owner,
        token,
        received,
        _guard: guard,
        _server: server,
    }
}

/// The session's current mode, recorded the way a provider handshake records
/// it: the write gate reads it at call time.
fn set_mode(fixture: &Fixture, session: &str, mode: &str) {
    fixture
        .state
        .sessions
        .live_runtime(session, &fixture.owner)
        .expect("live session")
        .store_session_manifest(devboule_protocol::SessionEvent::SessionManifest {
            provider_id: Some("acp".to_string()),
            current_model_id: None,
            models: Vec::new(),
            modes: Some(devboule_protocol::SessionModeStateView {
                current_mode_id: mode.to_string(),
                available_modes: Vec::new(),
            }),
        });
}

/// The audit row this tool wrote last: the journal thread writes it, so the
/// raw connection polls until it lands rather than guessing at a flush.
fn audit_outcome(fixture: &Fixture, session: &str) -> String {
    let path = fixture.state.paths.journal_file();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let connection = rusqlite::Connection::open(&path).expect("raw journal");
        let found = connection
            .query_row(
                "SELECT outcome FROM audit WHERE session_id = ?1 AND action = ?2 ORDER BY rowid DESC LIMIT 1",
                rusqlite::params![session, MCP_CI_WATCH_TOOL],
                |row| row.get(0),
            )
            .ok();
        if let Some(outcome) = found {
            return outcome;
        }
        assert!(
            Instant::now() < deadline,
            "the audit row lands within five seconds"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The pending card's id, waited for: the call that raises it blocks until a
/// person answers, so it runs on another thread.
fn wait_for_card(fixture: &Fixture, session: &str) -> String {
    let start = Instant::now();
    loop {
        let pending = fixture
            .state
            .sessions
            .live_runtime(session, &fixture.owner)
            .expect("live session")
            .permission_broker()
            .expect("test broker")
            .test_pending_ids();
        if let Some(card) = pending.into_iter().next() {
            return card;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "the retry card was never raised"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Answer the card the retry call raised, and hand back the tool's answer.
fn call_with_retry_card(
    fixture: &Fixture,
    session: &str,
    outcome: PermissionOutcome,
    option: &str,
) -> Value {
    let url = fixture.state.mcp.url.clone();
    let token = fixture.token.clone();
    let request = format!(
        r#"{{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{{"name":"{MCP_CI_WATCH_TOOL}","arguments":{{"sha":"{SHA}","repo":"acme/widgets","retryInfra":true}}}}}}"#
    );
    let calling = std::thread::spawn(move || {
        response_json(&http_request(
            &url,
            Some(&format!("Bearer {token}")),
            &request,
        ))
    });
    let card = wait_for_card(fixture, session);
    fixture
        .state
        .sessions
        .live_runtime(session, &fixture.owner)
        .expect("live session")
        .permission_broker()
        .expect("test broker")
        .test_answer(&card, outcome, option)
        .expect("answer the retry card");
    calling.join().expect("the call thread")
}

#[test]
fn agent_repo_arg_cannot_select_arbitrary_host() {
    let fixture = workspace_fixture("host");
    // The origin says github.com/acme/widgets; the argument names another
    // owner. The watch resolves on the origin's host, never the argument's.
    fixture.runner.set(
        &format!("commits/{SHA}/check-runs"),
        ok(&check_runs(&[check_run(31, "test", "queued", None)])),
    );
    let body = call(
        &fixture,
        &format!(r#"{{"sha":"{SHA}","repo":"attacker/gadget"}}"#),
    );
    assert_eq!(
        body.pointer("/result/isError"),
        Some(&json!(false)),
        "{body}"
    );
    let envelope = &body["result"]["structuredContent"];
    assert_eq!(envelope["data"]["repo"], json!("attacker/gadget"));
    fixture.state.ci_watches.poll_once(&fixture.state.sessions);
    assert!(
        fixture
            .runner
            .calls()
            .iter()
            .any(|call| call.contains("check-runs")),
        "the watch read checks"
    );
    for call in fixture.runner.calls() {
        if call.contains("--hostname") {
            assert!(
                call.contains("--hostname github.com"),
                "every GitHub call stays on the origin host: {call}"
            );
        }
    }
}

#[test]
fn the_verdict_reaches_the_caller_once() {
    let fixture = workspace_fixture("wake");
    fixture.runner.set(
        &format!("commits/{SHA}/check-runs"),
        ok(&check_runs(&[check_run(31, "test", "queued", None)])),
    );
    let body = call(
        &fixture,
        &format!(r#"{{"sha":"{SHA}","repo":"acme/widgets"}}"#),
    );
    assert_eq!(
        body.pointer("/result/isError"),
        Some(&json!(false)),
        "{body}"
    );
    let envelope = &body["result"]["structuredContent"];
    assert_eq!(envelope["ok"], json!(true));
    let data = &envelope["data"];
    assert_eq!(data["resolvedSha"], json!(SHA));
    assert_eq!(data["state"], json!("queued"));
    assert_eq!(data["wake"], json!("pending"));
    let watch_id = data["watchId"].as_str().expect("watch id").to_string();

    // The log first: the daemon's poll thread may read the finished run the
    // moment it is scripted, and must find its log.
    fixture.runner.set(
        "actions/jobs/31/logs",
        ok("error: assertion failed: left == right\n"),
    );
    fixture.runner.set(
        &format!("commits/{SHA}/check-runs"),
        ok(&check_runs(&[check_run(
            31,
            "test",
            "completed",
            Some("failure"),
        )])),
    );
    fixture.state.ci_watches.poll_once(&fixture.state.sessions);
    let delivered = wait_for_delivery(&fixture, WAKE_END);
    assert!(delivered.contains("kind: ci_verdict"), "{delivered}");
    assert!(delivered.contains("role: daemon"), "{delivered}");
    assert!(
        delivered.contains(&format!("eventId: {watch_id}:failed")),
        "{delivered}"
    );
    assert!(delivered.contains("assertion failed"), "{delivered}");

    let before = delivered.len();
    fixture.state.ci_watches.poll_once(&fixture.state.sessions);
    let after = fixture.received.lock().expect("received").len();
    assert_eq!(before, after, "the second pass wakes nobody");
}

#[test]
fn the_branch_head_is_resolved_at_call_time_and_reported() {
    let fixture = workspace_fixture("branch");
    fixture
        .runner
        .set("git/ref/heads/main", ok(&branch_head(SHA)));
    let body = call(&fixture, r#"{"branch":"main"}"#);
    assert_eq!(
        body.pointer("/result/isError"),
        Some(&json!(false)),
        "{body}"
    );
    let data = &body["result"]["structuredContent"]["data"];
    assert_eq!(data["resolvedSha"], json!(SHA));
    assert_eq!(data["branch"], json!("main"));
    assert_eq!(data["state"], json!("queued"));
    assert_eq!(data["retryCount"], json!(0));
    assert!(
        fixture
            .runner
            .calls()
            .iter()
            .any(|call| call.contains("git/ref/heads/main")),
        "the head is read on the call path, where the answer names it"
    );
}

#[test]
fn an_unknown_branch_is_a_missing_commit() {
    let fixture = workspace_fixture("no-branch");
    fixture
        .runner
        .set("git/ref", fail(1, "gh: Not Found (HTTP 404)"));
    let body = call(&fixture, r#"{"branch":"nope"}"#);
    assert_eq!(
        body.pointer("/result/isError"),
        Some(&json!(true)),
        "{body}"
    );
    assert_eq!(
        body.pointer("/result/structuredContent/error/code"),
        Some(&json!("sha_not_found")),
        "{body}"
    );
    assert!(fixture.state.ci_watches.open().is_empty());
}

/// Re-running failed jobs is a state-changing GitHub action, so a call that
/// asks for it waits on a card in a mode that asks — and the person's yes is
/// what the watch then holds.
#[test]
fn a_retry_asks_the_person_and_the_approval_rides_the_watch() {
    let fixture = workspace_fixture("retry-card");
    let body = call_with_retry_card(&fixture, "ci.host", PermissionOutcome::AllowOnce, "once");
    assert_eq!(
        body.pointer("/result/isError"),
        Some(&json!(false)),
        "{body}"
    );
    let data = &body["result"]["structuredContent"]["data"];
    assert_eq!(data["retryCount"], json!(0));
    let watch_id = data["watchId"].as_str().expect("watch id");
    assert_eq!(
        data["retryIssued"],
        json!(false),
        "nothing has been re-run yet"
    );
    let watch = fixture
        .state
        .ci_watches
        .get(watch_id)
        .expect("the watch the call started");
    assert!(watch.retry_approved, "the card's yes reached the watch");
    let outcome = audit_outcome(&fixture, "ci.host");
    assert!(
        outcome.contains("approved by person"),
        "the audit row names who approved: {outcome}"
    );
}

/// An automatic mode approves the retry card itself — and the audit row still
/// says the approval was the mode's, never a person's.
#[test]
fn a_mode_approved_retry_is_logged_as_the_mode() {
    let fixture = workspace_fixture("retry-mode");
    fixture
        .state
        .sessions
        .live_runtime("ci.host", &fixture.owner)
        .expect("live session")
        .set_agent_kind(SessionKind::Acp);
    set_mode(&fixture, "ci.host", "auto_accept");

    let body = call(
        &fixture,
        &format!(r#"{{"sha":"{SHA}","repo":"acme/widgets","retryInfra":true}}"#),
    );

    assert_eq!(
        body.pointer("/result/isError"),
        Some(&json!(false)),
        "an automatic mode raises no card: {body}"
    );
    let data = &body["result"]["structuredContent"]["data"];
    assert_eq!(data["retryCount"], json!(0));
    let watch_id = data["watchId"].as_str().expect("watch id");
    assert!(
        fixture
            .state
            .ci_watches
            .get(watch_id)
            .expect("the watch")
            .retry_approved
    );
    let outcome = audit_outcome(&fixture, "ci.host");
    assert!(
        outcome.contains("approved by automatic mode"),
        "the audit row names the approval: {outcome}"
    );
}

#[test]
fn a_denied_retry_starts_no_watch() {
    let fixture = workspace_fixture("retry-denied");
    let body = call_with_retry_card(&fixture, "ci.host", PermissionOutcome::Deny, "deny");
    assert_eq!(
        body.pointer("/result/isError"),
        Some(&json!(true)),
        "{body}"
    );
    assert!(
        body.pointer("/result/content/0/text")
            .and_then(Value::as_str)
            .is_some_and(|text| text.contains("permission refused")),
        "{body}"
    );
    assert!(
        fixture.state.ci_watches.open().is_empty(),
        "a refusal starts nothing"
    );
    assert!(
        fixture
            .state
            .mcp
            .first_use_mark("ci.host", CI_RETRY_GROUP)
            .is_none(),
        "and leaves the gate shut"
    );
}
