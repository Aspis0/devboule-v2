//! `devboule_ci_watch` over the real broker: the closed argument set, the
//! result envelope, and a verdict that reaches the caller's own session once.

use std::sync::{Arc, Mutex};

use devboule_protocol::SessionKind;
use serde_json::{json, Value};

use crate::ci_test_support::{
    check_run, check_runs, fail, github_with_commit, ok, ScriptedRunner, SHA,
};
use crate::mcp_broker::terminal_write_harness::project_workspace;
use crate::mcp_broker::tests::{http_request, owner, response_json};
use crate::provider_catalog::MCP_CI_WATCH_TOOL;
use crate::server::ServerState;

struct Fixture {
    state: Arc<ServerState>,
    runner: Arc<ScriptedRunner>,
    token: String,
    received: Arc<Mutex<Vec<u8>>>,
    _guard: crate::mcp_broker::McpSessionGuard,
    _server: crate::mcp_broker::McpServerHandle,
}

fn fixture(tag: &str) -> Fixture {
    let runner = Arc::new(github_with_commit());
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
    assert_eq!(tool["inputSchema"]["required"], json!(["sha"]));
}

#[test]
fn malformed_arguments_are_refused_before_github_is_asked() {
    let fixture = fixture("args");
    for arguments in [
        "{}",
        r#"{"sha":"abc123"}"#,
        r#"{"sha":"0123456789abcdef0123456789abcdef0123456z"}"#,
        &format!(r#"{{"sha":"{SHA}","branch":"main"}}"#),
        &format!(r#"{{"sha":"{SHA}","repo":"not a repo"}}"#),
        &format!(r#"{{"sha":"{SHA}","repo":"evil.com/attacker/gadget"}}"#),
        &format!(r#"{{"sha":"{SHA}","repo":7}}"#),
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

#[test]
fn a_missing_login_is_a_tool_error_with_the_step_to_take() {
    let fixture = workspace_fixture("login");
    fixture.runner.set(
        &format!("git/commits/{SHA}"),
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
        Some(&json!(true)),
        "{body}"
    );
    let envelope = &body["result"]["structuredContent"];
    assert_eq!(envelope["ok"], json!(false));
    assert!(envelope["hostId"]
        .as_str()
        .is_some_and(|host| !host.is_empty()));
    assert_eq!(envelope["error"]["code"], json!("github_auth_required"));
    assert_eq!(envelope["error"]["retryable"], json!(false));
    assert!(envelope["error"]["message"]
        .as_str()
        .is_some_and(|message| message.contains("gh auth login")));
}

fn workspace_fixture(tag: &str) -> Fixture {
    let runner = Arc::new(github_with_commit());
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
        token,
        received,
        _guard: guard,
        _server: server,
    }
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

    fixture.runner.set(
        &format!("commits/{SHA}/check-runs"),
        ok(&check_runs(&[check_run(
            31,
            "test",
            "completed",
            Some("failure"),
        )])),
    );
    fixture.runner.set(
        "actions/jobs/31/logs",
        ok("error: assertion failed: left == right\n"),
    );
    fixture.state.ci_watches.poll_once(&fixture.state.sessions);
    let delivered = String::from_utf8(fixture.received.lock().expect("received").clone())
        .expect("utf8 delivery");
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
