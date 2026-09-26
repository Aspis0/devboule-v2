//! End-to-end tests for the two terminal read tools: the scope both reads
//! share, the screen reply, and the two gates that can still take them away.

use super::dispatch::{enabled_tool_list, tool_call_refusal};
use super::tests::{http_request, owner, peer_row, response_json};
use super::*;
use crate::peer_policy::PeerRole;
use crate::provider_catalog::{ToolOverlay, MCP_CAPTURE_TERMINAL_TOOL, MCP_LIST_TERMINALS_TOOL};
use crate::session::ConnHandle;
use devboule_protocol::SessionOrigin;

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
        vec!["term-live"],
        "the caller's own workspace's running terminals only — no agent session, \
         no other workspace, no other owner, and no terminal whose process has \
         gone: capture refuses those, so the roster must not name them either"
    );
    let live = &terminals[0];
    assert_eq!(live["title"], json!("Terminal"));
    assert_eq!(live["cwd"], json!("/tmp/devboule-terminal"));
    assert_eq!(live["createdBy"], json!(null));
    assert!(
        live.get("live").is_none(),
        "only running terminals are listed, so the roster carries no live flag: {live}"
    );

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
    assert_eq!(
        document["truncated"],
        json!(false),
        "nothing was cut: the whole grid fits the window"
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
        (
            lines,
            document["totalLines"].as_u64().expect("totalLines"),
            document["truncated"].as_bool().expect("truncated"),
        )
    };

    // The default window is 40 lines, taken from the bottom of a 250-row
    // grid that shows L10..L259.
    let (lines, total, truncated) = window(1, r#"{"terminalId":"term-tall"}"#);
    assert_eq!(total, 250);
    assert_eq!(lines.len(), 40);
    assert_eq!(lines.first().map(String::as_str), Some("L220"));
    assert_eq!(lines.last().map(String::as_str), Some("L259"));
    assert!(
        truncated,
        "the grid is taller than the window: {total} rows"
    );

    let (lines, total, truncated) = window(2, r#"{"terminalId":"term-tall","lines":3}"#);
    assert_eq!(total, 250);
    assert_eq!(lines, vec!["L257", "L258", "L259"], "the bottom window");
    assert!(truncated, "three of {total} rows is a cut");

    // The top of the published range answers; there is no silent rewrite.
    let (lines, total, truncated) = window(3, r#"{"terminalId":"term-tall","lines":200}"#);
    assert_eq!(total, 250);
    assert_eq!(lines.len(), 200);
    assert_eq!(lines.first().map(String::as_str), Some("L60"));
    assert!(truncated);

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

#[test]
fn capture_terminal_refuses_lines_outside_the_published_range() {
    // The schema's minimum and maximum and the parser are one rule: an
    // integer outside the range is refused with that range, and a value that
    // is not an integer says so. Neither is silently rewritten into something
    // the caller did not ask for.
    let state = ServerState::new("mcp-term-lines".to_string());
    let owner = owner("mcp-term-lines-user", "mcp-term-lines-client");
    crate::session::insert_test_live_agent_in_workspace(
        &state.sessions,
        "lines-caller",
        owner.clone(),
        "ws-a",
    );
    crate::session::insert_test_terminal(
        &state.sessions,
        "term-lines",
        owner.clone(),
        Some("ws-a".to_string()),
    );

    let guard = state
        .mcp
        .register("lines-caller", &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token("lines-caller").expect("token");
    let server = state.mcp.start(&state).expect("MCP server");

    // The published bounds and the refusal carry the same two numbers.
    let listed = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
    );
    let listed_body = response_json(&listed);
    let served = listed_body["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .find(|tool| tool["name"] == MCP_CAPTURE_TERMINAL_TOOL)
        .expect("the screen read is served");
    assert_eq!(
        served["inputSchema"]["properties"]["lines"]["minimum"],
        json!(1)
    );
    assert_eq!(
        served["inputSchema"]["properties"]["lines"]["maximum"],
        json!(200)
    );

    for (id, arguments, sentence) in [
        (
            2,
            r#"{"terminalId":"term-lines","lines":0}"#,
            "lines must be between 1 and 200",
        ),
        (
            3,
            r#"{"terminalId":"term-lines","lines":-1}"#,
            "lines must be between 1 and 200",
        ),
        (
            4,
            r#"{"terminalId":"term-lines","lines":5000}"#,
            "lines must be between 1 and 200",
        ),
        (
            5,
            r#"{"terminalId":"term-lines","lines":3.5}"#,
            "lines must be an integer",
        ),
        (
            6,
            r#"{"terminalId":"term-lines","lines":"3"}"#,
            "lines must be an integer",
        ),
    ] {
        let body = call(
            &state.mcp.url,
            &token,
            id,
            MCP_CAPTURE_TERMINAL_TOOL,
            arguments,
        );
        assert_eq!(
            body.pointer("/error/code"),
            Some(&json!(-32602)),
            "{arguments}: {body}"
        );
        assert_eq!(
            body.pointer("/error/message"),
            Some(&json!(sentence)),
            "{arguments}: {body}"
        );
    }

    // Both ends of the range the schema states still answer.
    for (id, arguments) in [
        (7, r#"{"terminalId":"term-lines","lines":1}"#),
        (8, r#"{"terminalId":"term-lines","lines":200}"#),
    ] {
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
            "{arguments}: {body}"
        );
    }

    drop(guard);
    drop(server);
}

#[test]
fn a_daemon_origin_caller_reads_only_terminals_of_its_own_origin() {
    // The origin rule, not the owner name: every session below shares one
    // owner user, so the owner filter alone would hand a device paired as
    // `Daemon` the person's own terminal. The door decides, and it grants as
    // well as refuses — the terminal that device created here stays readable.
    let state = ServerState::new("mcp-term-daemon".to_string());
    let owner = owner("mcp-term-daemon-user", "mcp-term-daemon-client");
    crate::session::insert_test_live_agent_in_workspace(
        &state.sessions,
        "daemon-caller",
        owner.clone(),
        "ws-a",
    );
    state.sessions.set_test_origin(
        "daemon-caller",
        SessionOrigin::peer("device-d", PeerRole::Daemon),
    );
    crate::session::insert_test_terminal(
        &state.sessions,
        "term-human",
        owner.clone(),
        Some("ws-a".to_string()),
    );
    crate::session::insert_test_terminal(
        &state.sessions,
        "term-own",
        owner.clone(),
        Some("ws-a".to_string()),
    );
    state.sessions.set_test_origin(
        "term-own",
        SessionOrigin::peer("device-d", PeerRole::Daemon),
    );
    // The door lets the tool through on the way in: both reads are judged
    // against this device's own capabilities first.
    state
        .peer_upsert(peer_row("device-d", &["view", "admin"]))
        .expect("store a peer");

    let guard = state
        .mcp
        .register("daemon-caller", &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token("daemon-caller").expect("token");
    let server = state.mcp.start(&state).expect("MCP server");

    let listed = call(&state.mcp.url, &token, 1, MCP_LIST_TERMINALS_TOOL, "{}");
    assert_eq!(
        listed.pointer("/result/isError"),
        Some(&json!(false)),
        "{listed}"
    );
    let terminals = listed["result"]["structuredContent"]["terminals"]
        .as_array()
        .expect("terminals");
    let ids: Vec<&str> = terminals
        .iter()
        .map(|terminal| terminal["id"].as_str().expect("id"))
        .collect();
    assert_eq!(
        ids,
        vec!["term-own"],
        "only the terminals this device created here — never the person's: {ids:?}"
    );

    // The person's terminal is as unknown to the screen read as an id the
    // daemon never saw, and the device's own terminal still answers.
    let human = call(
        &state.mcp.url,
        &token,
        2,
        MCP_CAPTURE_TERMINAL_TOOL,
        r#"{"terminalId":"term-human"}"#,
    );
    assert_eq!(refusal(&human), "No session with that id.");
    let own = call(
        &state.mcp.url,
        &token,
        3,
        MCP_CAPTURE_TERMINAL_TOOL,
        r#"{"terminalId":"term-own"}"#,
    );
    assert_eq!(
        own.pointer("/result/isError"),
        Some(&json!(false)),
        "its own terminal stays readable: {own}"
    );

    drop(guard);
    drop(server);
}

#[test]
fn a_peer_caller_without_a_paired_user_is_refused() {
    // A `Client` peer speaks for the user that paired it, and a row that
    // never recorded that user speaks for nobody: the caller's own row is
    // refused at the door, so every read behind it is out of reach.
    let state = ServerState::new("mcp-term-peer".to_string());
    let owner = owner("mcp-term-peer-user", "mcp-term-peer-client");
    crate::session::insert_test_live_agent_in_workspace(
        &state.sessions,
        "peer-caller",
        owner.clone(),
        "ws-a",
    );
    state.sessions.set_test_origin(
        "peer-caller",
        SessionOrigin::peer("device-p", PeerRole::Client),
    );
    crate::session::insert_test_terminal(
        &state.sessions,
        "term-peer",
        owner.clone(),
        Some("ws-a".to_string()),
    );
    state
        .peer_upsert(peer_row("device-p", &["view", "admin"]))
        .expect("store a peer");

    let guard = state
        .mcp
        .register("peer-caller", &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token("peer-caller").expect("token");
    let server = state.mcp.start(&state).expect("MCP server");

    let listed = call(&state.mcp.url, &token, 1, MCP_LIST_TERMINALS_TOOL, "{}");
    assert_eq!(
        listed.pointer("/result/isError"),
        Some(&json!(true)),
        "the roster is refused before it names anything: {listed}"
    );
    let captured = call(
        &state.mcp.url,
        &token,
        2,
        MCP_CAPTURE_TERMINAL_TOOL,
        r#"{"terminalId":"term-peer"}"#,
    );
    assert_eq!(
        captured.pointer("/result/isError"),
        Some(&json!(true)),
        "and so is the screen read: {captured}"
    );

    drop(guard);
    drop(server);
}

#[test]
fn capture_terminal_cuts_a_row_to_the_renderer_bound() {
    // A window can be resized to thousands of columns, so the renderer caps
    // a row instead of trusting the terminal's size: what one reply costs is
    // decided by the request, never by the window.
    let state = ServerState::new("mcp-term-wide".to_string());
    let owner = owner("mcp-term-wide-user", "mcp-term-wide-client");
    crate::session::insert_test_live_agent_in_workspace(
        &state.sessions,
        "wide-caller",
        owner.clone(),
        "ws-a",
    );
    let runtime = crate::session::insert_test_terminal(
        &state.sessions,
        "term-wide",
        owner.clone(),
        Some("ws-a".to_string()),
    );
    let conn = ConnHandle::new(8);
    state
        .sessions
        .attach_with_subscription("term-wide", 102, None, &conn, &owner, false)
        .expect("attach");
    state
        .sessions
        .claim_resize_with_subscription("term-wide", 102, &owner, &conn)
        .expect("resize claim");
    state
        .sessions
        .resize_with_subscription("term-wide", 102, 2000, 10, &owner, &conn)
        .expect("resize");
    runtime.publish_output(&"x".repeat(2000));

    let guard = state
        .mcp
        .register("wide-caller", &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token("wide-caller").expect("token");
    let server = state.mcp.start(&state).expect("MCP server");

    let body = call(
        &state.mcp.url,
        &token,
        1,
        MCP_CAPTURE_TERMINAL_TOOL,
        r#"{"terminalId":"term-wide","lines":10}"#,
    );
    assert_eq!(
        body.pointer("/result/isError"),
        Some(&json!(false)),
        "{body}"
    );
    let document = &body["result"]["structuredContent"];
    assert_eq!(document["totalLines"], json!(10));
    let rows = document["lines"].as_array().expect("lines");
    let row = rows[0].as_str().expect("row");
    assert_eq!(
        row.len(),
        1024,
        "one row stops at the renderer's bound instead of shipping 2000 columns"
    );
    assert!(
        row.chars().all(|character| character == 'x'),
        "the row is what the terminal held, cut and not rewritten"
    );

    drop(guard);
    drop(server);
}

#[test]
fn every_terminal_read_is_audited() {
    // A read of somebody's screen leaves a row either way — which session
    // read which terminal, and whether it was granted — so the journal can
    // answer "who saw it" after the fact.
    let state = ServerState::new("mcp-term-audit".to_string());
    let owner = owner("mcp-term-audit-user", "mcp-term-audit-client");
    crate::session::insert_test_live_agent_in_workspace(
        &state.sessions,
        "audit-caller",
        owner.clone(),
        "ws-a",
    );
    crate::session::insert_test_terminal(
        &state.sessions,
        "term-audit",
        owner.clone(),
        Some("ws-a".to_string()),
    );

    let guard = state
        .mcp
        .register("audit-caller", &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token("audit-caller").expect("token");
    let server = state.mcp.start(&state).expect("MCP server");

    let granted = call(
        &state.mcp.url,
        &token,
        1,
        MCP_CAPTURE_TERMINAL_TOOL,
        r#"{"terminalId":"term-audit"}"#,
    );
    assert_eq!(
        granted.pointer("/result/isError"),
        Some(&json!(false)),
        "{granted}"
    );
    let listed = call(&state.mcp.url, &token, 2, MCP_LIST_TERMINALS_TOOL, "{}");
    assert_eq!(
        listed.pointer("/result/isError"),
        Some(&json!(false)),
        "{listed}"
    );
    let refused = call(
        &state.mcp.url,
        &token,
        3,
        MCP_CAPTURE_TERMINAL_TOOL,
        r#"{"terminalId":"term-audit-gone"}"#,
    );
    assert_eq!(
        refused.pointer("/result/isError"),
        Some(&json!(true)),
        "{refused}"
    );

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
    let caller = Some("audit-caller".to_string());
    for (action, outcome) in [
        (MCP_CAPTURE_TERMINAL_TOOL, "ok"),
        (MCP_LIST_TERMINALS_TOOL, "ok"),
        (MCP_CAPTURE_TERMINAL_TOOL, "denied"),
    ] {
        assert!(
            rows.contains(&(action.to_string(), caller.clone(), outcome.to_string())),
            "{action} as {outcome} is audited with its actor session: {rows:?}"
        );
    }

    drop(guard);
    drop(server);
}
