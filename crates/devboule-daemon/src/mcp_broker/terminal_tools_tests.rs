//! End-to-end tests for the two terminal read tools: the scope both reads
//! share, the screen reply, and the two gates that can still take them away.

use super::dispatch::{enabled_tool_list, tool_call_refusal};
use super::tests::{http_request, owner, response_json};
use super::*;
use crate::provider_catalog::{ToolOverlay, MCP_CAPTURE_TERMINAL_TOOL, MCP_LIST_TERMINALS_TOOL};
use crate::session::ConnHandle;

/// One `tools/call` against the loopback broker, answered already parsed.
fn call(url: &str, token: &str, id: u64, name: &str, arguments: &str) -> Value {
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
fn refusal(body: &Value) -> &str {
    assert_eq!(
        body.pointer("/result/isError"),
        Some(&json!(true)),
        "a scope refusal is a tool error: {body}"
    );
    body.pointer("/result/content/0/text")
        .and_then(Value::as_str)
        .expect("the refusal sentence")
}

#[test]
fn list_terminals_serves_the_callers_workspace_and_nothing_else() {
    let state = ServerState::new("mcp-term-list".to_string());
    let stranger_owner = owner("mcp-term-list-other", "mcp-term-list-other-client");
    let owner = owner("mcp-term-list-user", "mcp-term-list-client");
    // The caller is an agent session in one workspace: the bearer, and the
    // row the whole scope is read from.
    crate::session::insert_test_live_agent_in_workspace(
        &state.sessions,
        "list-caller",
        owner.clone(),
        "ws-a",
    );
    crate::session::insert_test_terminal(
        &state.sessions,
        "term-live",
        owner.clone(),
        Some("ws-a".to_string()),
    );
    let exited = crate::session::insert_test_terminal(
        &state.sessions,
        "term-exited",
        owner.clone(),
        Some("ws-a".to_string()),
    );
    // Out of scope: an agent session in the same workspace, a terminal of
    // another workspace, and a terminal of another owner.
    crate::session::insert_test_live_agent_in_workspace(
        &state.sessions,
        "agent-here",
        owner.clone(),
        "ws-a",
    );
    crate::session::insert_test_terminal(
        &state.sessions,
        "term-other-workspace",
        owner.clone(),
        Some("ws-b".to_string()),
    );
    crate::session::insert_test_terminal(
        &state.sessions,
        "term-other-owner",
        stranger_owner,
        Some("ws-a".to_string()),
    );
    exited.mark_terminal_dead("test: the process is gone");

    let guard = state
        .mcp
        .register("list-caller", &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token("list-caller").expect("token");
    let server = state.mcp.start(&state).expect("MCP server");

    // Both tools are served, and the parameterless one states its own schema
    // rather than falling through to the default arm.
    let listed = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
    );
    let listed_body = response_json(&listed);
    let tools = listed_body["result"]["tools"].as_array().expect("tools");
    let served = tools
        .iter()
        .find(|tool| tool["name"] == MCP_LIST_TERMINALS_TOOL)
        .expect("the terminal roster is served");
    assert_eq!(
        served["inputSchema"],
        json!({"type": "object", "properties": {}, "additionalProperties": false})
    );

    let body = call(&state.mcp.url, &token, 2, MCP_LIST_TERMINALS_TOOL, "{}");
    assert_eq!(
        body.pointer("/result/isError"),
        Some(&json!(false)),
        "the read answers: {body}"
    );
    let terminals = body["result"]["structuredContent"]["terminals"]
        .as_array()
        .expect("terminals");
    let ids: Vec<&str> = terminals
        .iter()
        .map(|terminal| terminal["id"].as_str().expect("id"))
        .collect();
    assert_eq!(
        ids,
        vec!["term-exited", "term-live"],
        "the caller's own workspace's terminals only — no agent session, \
         no other workspace, no other owner"
    );
    let live = &terminals[1];
    assert_eq!(live["live"], json!(true));
    assert_eq!(live["title"], json!("Terminal"));
    assert_eq!(live["cwd"], json!("/tmp/devboule-terminal"));
    assert_eq!(live["createdBy"], json!(null));
    // An exited terminal keeps its place in the roster, honestly labelled.
    assert_eq!(terminals[0]["live"], json!(false));

    drop(guard);
    drop(server);
}

#[test]
fn a_session_without_a_workspace_reads_no_terminals() {
    // The scope is the caller's own workspace, so a caller that has none is
    // told the missing fact instead of being answered about every
    // workspace-less terminal of its user.
    let state = ServerState::new("mcp-term-nowrap".to_string());
    let owner = owner("mcp-term-nowrap-user", "mcp-term-nowrap-client");
    crate::session::insert_test_live_agent(&state.sessions, "nowrap-caller", owner.clone());
    crate::session::insert_test_terminal(&state.sessions, "term-loose", owner.clone(), None);

    let guard = state
        .mcp
        .register("nowrap-caller", &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token("nowrap-caller").expect("token");
    let server = state.mcp.start(&state).expect("MCP server");

    let listed = call(&state.mcp.url, &token, 1, MCP_LIST_TERMINALS_TOOL, "{}");
    let sentence = refusal(&listed);
    assert!(
        sentence.contains("no workspace"),
        "the refusal names the missing fact: {sentence}"
    );
    let captured = call(
        &state.mcp.url,
        &token,
        2,
        MCP_CAPTURE_TERMINAL_TOOL,
        r#"{"terminalId":"term-loose"}"#,
    );
    let sentence = refusal(&captured);
    assert!(
        sentence.contains("no workspace"),
        "the same refusal on the screen read: {sentence}"
    );

    drop(guard);
    drop(server);
}

#[test]
fn capture_terminal_answers_the_visible_grid_without_escape_sequences() {
    let state = ServerState::new("mcp-term-screen".to_string());
    let owner = owner("mcp-term-screen-user", "mcp-term-screen-client");
    crate::session::insert_test_live_agent_in_workspace(
        &state.sessions,
        "screen-caller",
        owner.clone(),
        "ws-a",
    );
    let runtime = crate::session::insert_test_terminal(
        &state.sessions,
        "term-screen",
        owner.clone(),
        Some("ws-a".to_string()),
    );
    runtime.publish_output("ready \x1b[31mred\x1b[0m\r\nsecond line");

    let guard = state
        .mcp
        .register("screen-caller", &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token("screen-caller").expect("token");
    let server = state.mcp.start(&state).expect("MCP server");

    let body = call(
        &state.mcp.url,
        &token,
        1,
        MCP_CAPTURE_TERMINAL_TOOL,
        r#"{"terminalId":"term-screen"}"#,
    );
    assert_eq!(
        body.pointer("/result/isError"),
        Some(&json!(false)),
        "the capture answers: {body}"
    );
    let document = &body["result"]["structuredContent"];
    assert_eq!(document["terminalId"], json!("term-screen"));
    let total = document["totalLines"].as_u64().expect("totalLines");
    let lines = document["lines"].as_array().expect("lines");
    assert_eq!(
        lines.len() as u64,
        total,
        "the default window is wider than the grid, so the whole grid answers"
    );
    assert_eq!(lines[0], json!("ready red"), "styled text, no escape bytes");
    assert_eq!(lines[1], json!("second line"));
    for line in lines {
        let line = line.as_str().expect("line");
        assert!(
            !line.contains('\x1b') && !line.contains('\r') && !line.contains('\n'),
            "plain text only: {line:?}"
        );
    }

    drop(guard);
    drop(server);
}

#[test]
fn capture_terminal_caps_the_window_from_the_bottom() {
    let state = ServerState::new("mcp-term-cap".to_string());
    let owner = owner("mcp-term-cap-user", "mcp-term-cap-client");
    crate::session::insert_test_live_agent_in_workspace(
        &state.sessions,
        "cap-caller",
        owner.clone(),
        "ws-a",
    );
    let runtime = crate::session::insert_test_terminal(
        &state.sessions,
        "term-tall",
        owner.clone(),
        Some("ws-a".to_string()),
    );
    // A 250-row window, so both caps can bind: the requested `lines`, and
    // the hard maximum underneath it. The resize is the wire's own road —
    // attach, claim the resize right, resize — because the screen behind it
    // is private to the session module.
    let conn = ConnHandle::new(7);
    state
        .sessions
        .attach_with_subscription("term-tall", 101, None, &conn, &owner, false)
        .expect("attach");
    state
        .sessions
        .claim_resize_with_subscription("term-tall", 101, &owner, &conn)
        .expect("resize claim");
    state
        .sessions
        .resize_with_subscription("term-tall", 101, 120, 250, &owner, &conn)
        .expect("resize");
    let output = (0..260)
        .map(|line| format!("L{line}"))
        .collect::<Vec<_>>()
        .join("\r\n");
    runtime.publish_output(&output);

    let guard = state
        .mcp
        .register("cap-caller", &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token("cap-caller").expect("token");
    let server = state.mcp.start(&state).expect("MCP server");

    let window = |id: u64, arguments: &str| {
        let body = call(
            &state.mcp.url,
            &token,
            id,
            MCP_CAPTURE_TERMINAL_TOOL,
            arguments,
        );
        assert_eq!(
            body.pointer("/result/isError"),
            Some(&json!(false)),
            "the capture answers: {body}"
        );
        let document = &body["result"]["structuredContent"];
        let lines: Vec<String> = document["lines"]
            .as_array()
            .expect("lines")
            .iter()
            .map(|line| line.as_str().expect("line").to_string())
            .collect();
        (lines, document["totalLines"].as_u64().expect("totalLines"))
    };

    // The default window is 40 lines, taken from the bottom of a 250-row
    // grid that shows L10..L259.
    let (lines, total) = window(1, r#"{"terminalId":"term-tall"}"#);
    assert_eq!(total, 250);
    assert_eq!(lines.len(), 40);
    assert_eq!(lines.first().map(String::as_str), Some("L220"));
    assert_eq!(lines.last().map(String::as_str), Some("L259"));

    let (lines, total) = window(2, r#"{"terminalId":"term-tall","lines":3}"#);
    assert_eq!(total, 250);
    assert_eq!(lines, vec!["L257", "L258", "L259"], "the bottom window");

    // Over the hard maximum: clamped to 200 rather than refused, and 0 is
    // clamped up to one line. Both are the range the schema states.
    let (lines, total) = window(3, r#"{"terminalId":"term-tall","lines":5000}"#);
    assert_eq!(total, 250);
    assert_eq!(lines.len(), 200);
    assert_eq!(lines.first().map(String::as_str), Some("L60"));
    let (lines, _) = window(4, r#"{"terminalId":"term-tall","lines":0}"#);
    assert_eq!(lines, vec!["L259"]);

    // The argument set is closed, out of the published schema itself.
    let unknown = call(
        &state.mcp.url,
        &token,
        5,
        MCP_CAPTURE_TERMINAL_TOOL,
        r#"{"terminalId":"term-tall","bogus":1}"#,
    );
    assert_eq!(unknown.pointer("/error/code"), Some(&json!(-32602)));
    let missing = call(
        &state.mcp.url,
        &token,
        6,
        MCP_CAPTURE_TERMINAL_TOOL,
        r#"{}"#,
    );
    assert_eq!(missing.pointer("/error/code"), Some(&json!(-32602)));

    drop(guard);
    drop(server);
}

#[test]
fn capture_terminal_refuses_every_id_outside_the_scope() {
    // Five ids, one sentence: an agent session, another owner's terminal,
    // another workspace's terminal, an exited terminal, and an id the
    // daemon does not know all read "No session with that id.", so the
    // answer never says which of them the id named.
    let state = ServerState::new("mcp-term-scope".to_string());
    let stranger_owner = owner("mcp-term-scope-other", "mcp-term-scope-other-client");
    let owner = owner("mcp-term-scope-user", "mcp-term-scope-client");
    crate::session::insert_test_live_agent_in_workspace(
        &state.sessions,
        "scope-caller",
        owner.clone(),
        "ws-a",
    );
    crate::session::insert_test_live_agent_in_workspace(
        &state.sessions,
        "scope-agent",
        owner.clone(),
        "ws-a",
    );
    crate::session::insert_test_terminal(
        &state.sessions,
        "term-elsewhere",
        owner.clone(),
        Some("ws-b".to_string()),
    );
    crate::session::insert_test_terminal(
        &state.sessions,
        "term-stranger",
        stranger_owner,
        Some("ws-a".to_string()),
    );
    let exited = crate::session::insert_test_terminal(
        &state.sessions,
        "term-gone",
        owner.clone(),
        Some("ws-a".to_string()),
    );
    exited.mark_terminal_dead("test: the process is gone");

    let guard = state
        .mcp
        .register("scope-caller", &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token("scope-caller").expect("token");
    let server = state.mcp.start(&state).expect("MCP server");

    for (id, target) in [
        (10, "scope-agent"),
        (11, "term-elsewhere"),
        (12, "term-stranger"),
        (13, "term-gone"),
        (14, "term-unknown"),
    ] {
        let body = call(
            &state.mcp.url,
            &token,
            id,
            MCP_CAPTURE_TERMINAL_TOOL,
            &format!(r#"{{"terminalId":"{target}"}}"#),
        );
        assert_eq!(
            refusal(&body),
            "No session with that id.",
            "{target} must be indistinguishable from an id the daemon does not know"
        );
    }

    drop(guard);
    drop(server);
}

#[test]
fn a_stored_policy_can_take_the_terminal_tools_away() {
    // The catalog promises it: both reads are disableable per provider, and
    // a profile's overlay removes them on the two functions the broker
    // calls — `enabled_tool_list` for tools/list, `tool_call_refusal` for
    // tools/call.
    let state = ServerState::new("mcp-term-policy".to_string());
    let owner = owner("mcp-term-policy-user", "mcp-term-policy-client");
    crate::session::insert_test_live_agent_in_workspace(
        &state.sessions,
        "policy-caller",
        owner.clone(),
        "ws-a",
    );
    let guard = state
        .mcp
        .register_with_provider(
            "policy-caller",
            &owner,
            &SessionKind::Acp,
            Some("claude"),
            AgentLineage::root(),
        )
        .expect("registration")
        .expect("MCP guard");
    state
        .tool_policy
        .set(
            "claude",
            Some(true),
            vec![
                MCP_LIST_TERMINALS_TOOL.to_string(),
                MCP_CAPTURE_TERMINAL_TOOL.to_string(),
            ],
        )
        .expect("policy");
    let token = state.mcp.test_token("policy-caller").expect("token");
    let server = state.mcp.start(&state).expect("MCP server");

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
    assert!(!names.contains(&MCP_LIST_TERMINALS_TOOL));
    assert!(!names.contains(&MCP_CAPTURE_TERMINAL_TOOL));
    for (id, name) in [(2, MCP_LIST_TERMINALS_TOOL), (3, MCP_CAPTURE_TERMINAL_TOOL)] {
        let body = call(&state.mcp.url, &token, id, name, "{}");
        assert_eq!(body.pointer("/error/code"), Some(&json!(-32601)), "{body}");
        assert_eq!(
            body.pointer("/error/message"),
            Some(&json!("Tool disabled by policy"))
        );
    }

    let overlay = ToolOverlay::from_profile_names(&[
        MCP_LIST_TERMINALS_TOOL.to_string(),
        MCP_CAPTURE_TERMINAL_TOOL.to_string(),
    ]);
    let served = enabled_tool_list(
        crate::provider_catalog::MCP_BROKER_TOOLS,
        None,
        overlay.clone(),
    );
    let names: Vec<&str> = served
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str))
        .collect();
    assert!(!names.contains(&MCP_LIST_TERMINALS_TOOL));
    assert!(!names.contains(&MCP_CAPTURE_TERMINAL_TOOL));
    assert!(
        names.contains(&crate::provider_catalog::MCP_ROSTER_TOOL),
        "the roster stays served: {names:?}"
    );
    for name in [MCP_LIST_TERMINALS_TOOL, MCP_CAPTURE_TERMINAL_TOOL] {
        assert_eq!(
            tool_call_refusal(None, &overlay, name),
            Some("Tool disabled by policy"),
            "{name}"
        );
        assert_eq!(tool_call_refusal(None, &ToolOverlay::NONE, name), None);
    }

    drop(guard);
    drop(server);
}
