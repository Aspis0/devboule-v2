//! The agent/tools boundary for the in-app file editor: no MCP road
//! reaches the file-edit service, and the served surface is exactly the
//! catalog. The first test pins the call graph (no shipped broker file
//! names the service module); the second pins the served surface over the
//! real HTTP road (tools/list answers exactly the catalog names). A tool
//! that wrote file bytes under any other name would still need a handler,
//! and every handler lives under the scanned tree — so the pair, not a
//! name-substring scan, is what proves agents gain no file writes.

use super::dispatch::enabled_tool_list;
use super::tests::{http_request, owner, response_json};
use super::*;
use crate::provider_catalog::{ToolOverlay, MCP_BROKER_TOOLS};
use devboule_protocol::SessionKind;
use serde_json::Value;

/// Every shipped broker file, walked at test time: none may name the
/// file-edit service. Test files are skipped (a test may exercise the
/// boundary the shipped code must not cross); everything else is the
/// broker's real call graph, and a handler that reached the service
/// would have to name it.
#[test]
fn no_mcp_road_reaches_the_file_edit_service() {
    fn visit(dir: &std::path::Path, hits: &mut Vec<String>) {
        let entries = std::fs::read_dir(dir).expect("broker tree");
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                visit(&path, hits);
                continue;
            }
            if path.extension().and_then(|ext| ext.to_str()) != Some("rs") {
                continue;
            }
            let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
            if name.contains("tests") || name == "file_edit_boundary_tests.rs" {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("read");
            if source.contains("workspace_file_edit") {
                hits.push(path.display().to_string());
            }
        }
    }

    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/mcp_broker");
    let mut hits = Vec::new();
    visit(&root, &mut hits);
    assert!(
        hits.is_empty(),
        "the broker must not reach the file-edit service: {hits:?}"
    );
}

/// The served surface over the real HTTP road is exactly the catalog: no
/// tool is served that the catalog does not name, and the broker serves
/// every name the catalog carries (under an open policy). Combined with
/// the call-graph pin above, a file-writing tool cannot exist under any
/// name — served or not, it would need a handler, and handlers live in
/// the scanned tree.
#[test]
fn tools_list_serves_exactly_the_catalog() {
    let state = ServerState::new("mcp-file-boundary".to_string());
    let owner = owner("mcp-file-boundary-user", "mcp-file-boundary-client");
    crate::session::insert_test_live_agent(&state.sessions, "boundary-caller", owner.clone());
    let guard = state
        .mcp
        .register_with_provider(
            "boundary-caller",
            &owner,
            &SessionKind::Acp,
            Some("claude"),
            AgentLineage::root(),
        )
        .expect("registration")
        .expect("MCP guard");
    let token = state.mcp.test_token("boundary-caller").expect("token");
    let server = state.mcp.start(&state).expect("MCP server");

    let listed = http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
    );
    let body = response_json(&listed);
    let mut served: Vec<&str> = body["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect();
    served.sort_unstable();
    let mut catalogued: Vec<&str> = MCP_BROKER_TOOLS.iter().map(|(name, _)| *name).collect();
    catalogued.sort_unstable();
    assert_eq!(
        served, catalogued,
        "tools/list must serve exactly the catalog"
    );

    // The unit-level builder agrees with the road: under an open policy
    // nothing is added or dropped between the catalog and the body.
    let built = enabled_tool_list(MCP_BROKER_TOOLS, None, ToolOverlay::NONE);
    let mut built_names: Vec<&str> = built
        .iter()
        .filter_map(|tool: &Value| tool.get("name").and_then(Value::as_str))
        .collect();
    built_names.sort_unstable();
    assert_eq!(built_names, catalogued);

    drop(server);
    drop(guard);
}
