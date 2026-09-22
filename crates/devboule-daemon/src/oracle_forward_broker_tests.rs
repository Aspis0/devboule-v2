//! The tool over the real broker wire: the dispatch arm and the `tools/list`
//! schema, driven through a real MCP listener — the counts that normally
//! cover this table are dynamic comparisons against `MCP_BROKER_TOOLS`, so
//! they pass even if both the arm and the branch below are deleted.

use super::tests::{owner, state_with_session, APP, SESSION};
use super::*;
use crate::provider_catalog::{MCP_BROKER_TOOLS, MCP_ORACLE_SEARCH_TOOL};
use devboule_protocol::SessionKind;
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};

fn endpoint(url: &str) -> String {
    url.strip_prefix("http://")
        .expect("loopback URL")
        .split('/')
        .next()
        .expect("loopback endpoint")
        .to_string()
}

fn http_post(url: &str, token: &str, body: &str) -> Value {
    let mut stream = TcpStream::connect(endpoint(url)).expect("MCP listener");
    let request = format!(
        "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\nAuthorization: Bearer {token}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).expect("MCP request");
    stream.shutdown(Shutdown::Write).expect("request shutdown");
    let mut response = Vec::new();
    stream.read_to_end(&mut response).expect("MCP response");
    let text = String::from_utf8(response).expect("HTTP response");
    serde_json::from_str(text.split_once("\r\n\r\n").expect("HTTP body").1).expect("JSON response")
}

fn http_call(url: &str, token: &str, tool: &str, arguments: Value) -> Value {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {"name": tool, "arguments": arguments},
    })
    .to_string();
    http_post(url, token, &body)
}

/// The call with the app closed: `isError: true` and the app phrase — not
/// `-32601 Unknown tool`, which is what the chain answers when the dispatch
/// arm is gone.
#[test]
fn the_tool_over_the_wire_without_an_app_reads_the_app_phrase() {
    let (_root, state) = state_with_session("t13");
    let guard = state
        .mcp
        .register(SESSION, &owner(), &SessionKind::Acp)
        .expect("registration")
        .expect("caller MCP guard");
    let token = state.mcp.test_token(SESSION).expect("token");
    let server = state.mcp.start(&state).expect("MCP server");

    let reply = http_call(
        state.mcp.url(),
        &token,
        MCP_ORACLE_SEARCH_TOOL,
        json!({"query": "where is main"}),
    );
    assert_eq!(
        reply["result"]["isError"], true,
        "the closed-app answer is a tool error: {reply}"
    );
    assert_eq!(
        reply["result"]["content"][0]["text"], APP,
        "the wire carries the app phrase: {reply}"
    );

    drop(guard);
    drop(server);
}

/// The schema branch: name, catalog description verbatim, the closed
/// document with `query` required and `limit` bounded 1..10 — not the
/// parameterless default arm the chain falls through to.
#[test]
fn the_tool_is_listed_with_its_closed_schema() {
    let (_root, state) = state_with_session("t14");
    let guard = state
        .mcp
        .register(SESSION, &owner(), &SessionKind::Acp)
        .expect("registration")
        .expect("caller MCP guard");
    let token = state.mcp.test_token(SESSION).expect("token");
    let server = state.mcp.start(&state).expect("MCP server");

    let listed = http_post(
        state.mcp.url(),
        &token,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
    );
    let tools = listed["result"]["tools"].as_array().expect("tools");
    let tool = tools
        .iter()
        .find(|tool| tool["name"] == MCP_ORACLE_SEARCH_TOOL)
        .unwrap_or_else(|| panic!("{MCP_ORACLE_SEARCH_TOOL} is served: {tools:?}"));
    let (_, description) = MCP_BROKER_TOOLS
        .iter()
        .find(|(name, _)| *name == MCP_ORACLE_SEARCH_TOOL)
        .expect("catalog row");
    assert_eq!(tool["description"], *description, "the wire's description");
    assert_eq!(tool["inputSchema"]["required"], json!(["query"]));
    assert_eq!(tool["inputSchema"]["properties"]["limit"]["minimum"], 1);
    assert_eq!(tool["inputSchema"]["properties"]["limit"]["maximum"], 10);
    assert_eq!(tool["inputSchema"]["additionalProperties"], false);

    drop(guard);
    drop(server);
}
