//! Tests for `mcp_project_graph.rs`, kept out of the production file: the
//! fixture graphs and the loopback HTTP plumbing are test-only weight.
//!
//! Every fixture is a real `ckg.sqlite` written by `CkgStore` and read back
//! through the production path — session row → workspace → journal → the
//! engine's own queries. No mock of the engine.

use super::*;
use crate::provider_catalog::{MCP_IMPORTERS_TOOL, MCP_IMPORTS_TOOL, MCP_NEIGHBORHOOD_TOOL};
use crate::server::ServerState;
use devboule_protocol::{SessionKind, WorkspaceIsolation};
use oracle_core::{CkgEdgeRow, CkgNodeRow, CkgStore};
use std::io::{Read, Write};
use std::net::{Shutdown, TcpStream};
use std::path::Path;
use std::sync::Arc;

fn file_node(id: &str) -> CkgNodeRow {
    CkgNodeRow {
        id: id.to_string(),
        kind: "FILE".to_string(),
        name: None,
        file: id.to_string(),
        start_line: Some(1),
        end_line: Some(10),
        lang: Some("rust".to_string()),
    }
}

fn symbol_node(id: &str, file: &str) -> CkgNodeRow {
    CkgNodeRow {
        id: id.to_string(),
        kind: "function".to_string(),
        name: Some("f".to_string()),
        file: file.to_string(),
        start_line: Some(1),
        end_line: Some(5),
        lang: Some("rust".to_string()),
    }
}

fn import_edge(from: &str, to: &str) -> CkgEdgeRow {
    CkgEdgeRow {
        src: from.to_string(),
        dst: to.to_string(),
        kind: "IMPORT".to_string(),
        src_file: from.to_string(),
    }
}

fn contain_edge(from: &str, to: &str) -> CkgEdgeRow {
    CkgEdgeRow {
        src: from.to_string(),
        dst: to.to_string(),
        kind: "CONTAIN".to_string(),
        src_file: from.to_string(),
    }
}

fn write_graph(root: &Path, nodes: &[CkgNodeRow], edges: &[CkgEdgeRow]) {
    let path = OracleDataPaths::from_root_without_env(root).ckg;
    let store = CkgStore::new(&path).expect("ckg store");
    store.replace_all(nodes, edges).expect("graph rows");
}

fn owner() -> OwnerId {
    OwnerId::new("S-1-5-21-project-graph", "claude").expect("owner")
}

fn temp_dir(tag: &str) -> std::path::PathBuf {
    crate::test_dirs::test_temp_dir(&format!("devboule-project-graph-{tag}"))
}

/// One project folder with its own graph, added to `state` as a local
/// workspace: the caller's own project, as production builds it.
fn add_workspace(state: &Arc<ServerState>, root: &Path) -> String {
    let project = state
        .sessions
        .project_add(root.to_str().expect("project path"))
        .expect("project row");
    state
        .sessions
        .workspace_create(&project.id, WorkspaceIsolation::Local, None)
        .expect("workspace row")
        .id
}

/// The project the tests read: `src/a.rs` imports `src/b.rs`, which imports
/// `src/c.rs`; `src/a.rs` also contains one symbol.
fn workspace_a() -> (std::path::PathBuf, Arc<ServerState>) {
    let dir = temp_dir("a");
    let root = dir.join("ProjectA");
    std::fs::create_dir_all(&root).expect("project folder");
    write_graph(
        &root,
        &[
            file_node("src/a.rs"),
            file_node("src/b.rs"),
            file_node("src/c.rs"),
            symbol_node("src/a.rs#1-5-0", "src/a.rs"),
        ],
        &[
            import_edge("src/a.rs", "src/b.rs"),
            import_edge("src/b.rs", "src/c.rs"),
            contain_edge("src/a.rs", "src/a.rs#1-5-0"),
        ],
    );
    let state = ServerState::new("project-graph-a".to_string());
    let workspace = add_workspace(&state, &root);
    crate::session::insert_test_live_agent_in_workspace(
        &state.sessions,
        "graph-a",
        owner(),
        &workspace,
    );
    (dir, state)
}

/// A second project with a disjoint graph: its files do not exist in the
/// first one's graph, which is what makes the scoping assertion mean
/// something.
fn workspace_b(state: &Arc<ServerState>) -> std::path::PathBuf {
    let dir = temp_dir("b");
    let root = dir.join("ProjectB");
    std::fs::create_dir_all(&root).expect("project folder");
    write_graph(
        &root,
        &[file_node("lib/x.rs"), file_node("lib/y.rs")],
        &[import_edge("lib/x.rs", "lib/y.rs")],
    );
    let workspace = add_workspace(state, &root);
    crate::session::insert_test_live_agent_in_workspace(
        &state.sessions,
        "graph-b",
        owner(),
        &workspace,
    );
    dir
}

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

/// The three tools over the real wire, against a real graph: the exact
/// documents, the closed schemas, and the two argument refusals the broker
/// owns.
#[test]
fn the_three_tools_answer_the_graph_of_the_callers_own_workspace() {
    let (_dir, state) = workspace_a();
    let guard = state
        .mcp
        .register("graph-a", &owner(), &SessionKind::Acp)
        .expect("registration")
        .expect("caller MCP guard");
    let token = state.mcp.test_token("graph-a").expect("token");
    let server = state.mcp.start(&state).expect("MCP server");

    let listed = http_post(
        state.mcp.url(),
        &token,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
    );
    let tools = listed["result"]["tools"].as_array().expect("tools");
    for (name, required) in [
        (MCP_NEIGHBORHOOD_TOOL, json!(["node"])),
        (MCP_IMPORTS_TOOL, json!(["file"])),
        (MCP_IMPORTERS_TOOL, json!(["file"])),
    ] {
        let tool = tools
            .iter()
            .find(|tool| tool["name"] == name)
            .unwrap_or_else(|| panic!("{name} is served: {tools:?}"));
        assert_eq!(tool["inputSchema"]["required"], required, "{name}");
        assert_eq!(
            tool["inputSchema"]["additionalProperties"], false,
            "{name} schema is closed"
        );
    }

    let neighborhood = http_call(
        state.mcp.url(),
        &token,
        MCP_NEIGHBORHOOD_TOOL,
        json!({"node": "src/a.rs", "depth": 2}),
    );
    assert_eq!(neighborhood["result"]["isError"], false);
    assert_eq!(
        neighborhood["result"]["structuredContent"],
        json!({
            "node": "src/a.rs",
            "depth": 2,
            "kind": null,
            "neighbors": [
                {"node": "src/a.rs#1-5-0", "depth": 1},
                {"node": "src/b.rs", "depth": 1},
                {"node": "src/c.rs", "depth": 2},
            ],
        }),
        "one node per shortest depth, symbol included"
    );

    let imports = http_call(
        state.mcp.url(),
        &token,
        MCP_IMPORTS_TOOL,
        json!({"file": "src/a.rs"}),
    );
    assert_eq!(imports["result"]["isError"], false);
    assert_eq!(
        imports["result"]["structuredContent"],
        json!({"file": "src/a.rs", "imports": [{"from": "src/a.rs", "to": "src/b.rs"}]})
    );

    let importers = http_call(
        state.mcp.url(),
        &token,
        MCP_IMPORTERS_TOOL,
        json!({"file": "src/c.rs"}),
    );
    assert_eq!(importers["result"]["isError"], false);
    assert_eq!(
        importers["result"]["structuredContent"],
        json!({"file": "src/c.rs", "importers": [{"from": "src/b.rs", "to": "src/c.rs"}]})
    );

    // A malformed request is the broker's own parameter refusal, not a tool
    // error: `-32602`, with the parameter named.
    for (tool, arguments, sentence) in [
        (
            MCP_IMPORTS_TOOL,
            json!({"file": "src/a.rs", "depth": 1}),
            "unknown parameter 'depth'",
        ),
        (MCP_NEIGHBORHOOD_TOOL, json!({}), "node is required"),
        (
            MCP_NEIGHBORHOOD_TOOL,
            json!({"node": "src/a.rs", "depth": 9}),
            "depth must be between 1 and 4",
        ),
        (
            MCP_NEIGHBORHOOD_TOOL,
            json!({"node": "src/a.rs", "kind": "CALLS"}),
            "kind must be IMPORT or CONTAIN",
        ),
    ] {
        let refused = http_call(state.mcp.url(), &token, tool, arguments.clone());
        assert_eq!(
            refused["error"]["code"], -32602,
            "{tool} {arguments}: {refused}"
        );
        assert!(
            refused["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains(sentence)),
            "{tool} {arguments}: {refused}"
        );
    }

    drop(guard);
    drop(server);
}

/// The scoping proof: the same argument, two sessions, two answers. Session B
/// asks about B's node and gets B's neighbour; session A asks about the very
/// same node and gets nothing, because A's graph does not contain it. If the
/// path were read from an argument, or fixed, the two answers would agree.
#[test]
fn the_graph_is_the_callers_own_workspace_and_not_another() {
    let (_dir_a, state) = workspace_a();
    let _dir_b = workspace_b(&state);

    let a_sees_own = imports(&state, "graph-a", &owner(), &json!({"file": "src/a.rs"}))
        .expect("A reads its own graph");
    assert_eq!(
        a_sees_own,
        json!({"file": "src/a.rs", "imports": [{"from": "src/a.rs", "to": "src/b.rs"}]})
    );

    let b_sees_own = neighborhood(
        &state,
        "graph-b",
        &owner(),
        &json!({"node": "lib/x.rs", "depth": 2}),
    )
    .expect("B reads its own graph");
    assert_eq!(
        b_sees_own,
        json!({
            "node": "lib/x.rs",
            "depth": 2,
            "kind": null,
            "neighbors": [{"node": "lib/y.rs", "depth": 1}],
        })
    );

    // The same argument, the other session: A must not see B's nodes.
    let a_on_b_node = neighborhood(
        &state,
        "graph-a",
        &owner(),
        &json!({"node": "lib/x.rs", "depth": 2}),
    )
    .expect("A answers about a node it does not have");
    assert_eq!(
        a_on_b_node["neighbors"],
        json!([]),
        "A's graph has no lib/x.rs, so A sees nothing: {a_on_b_node}"
    );
    let a_on_b_file = imports(&state, "graph-a", &owner(), &json!({"file": "lib/x.rs"}))
        .expect("A answers about a file it does not have");
    assert_eq!(a_on_b_file["imports"], json!([]));
    let b_on_a_node = importers(&state, "graph-b", &owner(), &json!({"file": "src/b.rs"}))
        .expect("B answers about a file it does not have");
    assert_eq!(b_on_a_node["importers"], json!([]));
}

/// Fail-closed, both cases: a session with no workspace, and a workspace whose
/// graph has never been built. Neither is an empty answer, which would read as
/// "this node has no neighbours".
#[test]
fn a_missing_workspace_or_graph_is_refused_and_never_answered_with_an_empty_graph() {
    let state = ServerState::new("project-graph-absent".to_string());
    crate::session::insert_test_live_agent(&state.sessions, "graph-none", owner());

    let no_workspace = imports(&state, "graph-none", &owner(), &json!({"file": "src/a.rs"}))
        .expect_err("a session with no workspace is refused");
    match no_workspace {
        GraphError::Refused(message) => assert!(
            message.contains("no workspace"),
            "the refusal names the missing fact: {message}"
        ),
        GraphError::Invalid(message) => panic!("not a parameter refusal: {message}"),
    }

    let dir = temp_dir("empty");
    let root = dir.join("Unindexed");
    std::fs::create_dir_all(&root).expect("project folder");
    let workspace = add_workspace(&state, &root);
    crate::session::insert_test_live_agent_in_workspace(
        &state.sessions,
        "graph-empty",
        owner(),
        &workspace,
    );
    let no_graph = neighborhood(
        &state,
        "graph-empty",
        &owner(),
        &json!({"node": "src/a.rs"}),
    )
    .expect_err("a workspace with no graph is refused");
    match no_graph {
        GraphError::Refused(message) => {
            assert!(
                message.contains("no project graph yet"),
                "the refusal says which fact is missing: {message}"
            );
            assert!(
                message.contains("ckg.sqlite"),
                "the refusal names the missing file: {message}"
            );
        }
        GraphError::Invalid(message) => panic!("not a parameter refusal: {message}"),
    }

    // And the ownership rule: another user's session is the same refusal as an
    // unknown one, so a bearer cannot be used to read a row it does not own.
    let stranger = OwnerId::new("S-1-5-21-stranger", "claude").expect("owner");
    assert!(imports(
        &state,
        "graph-empty",
        &stranger,
        &json!({"file": "src/a.rs"})
    )
    .is_err());
}
