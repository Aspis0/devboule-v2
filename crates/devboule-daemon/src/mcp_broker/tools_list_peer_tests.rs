//! What `tools/list` shows a caller: the person at this machine sees every
//! enabled tool; a paired device sees only the tools it may call under its
//! capabilities — the same door that refuses the call decides the listing.

use std::sync::Arc;

use serde_json::Value;

use super::peer_fail_closed_tests::{serve, CALLER};
use super::tests::{http_request, response_json};
use crate::peer_policy::{CAP_SEARCH, CAP_VIEW};
use crate::provider_catalog::{
    MCP_BROKER_TOOLS, MCP_BROWSER_FILL_LOGIN_TOOL, MCP_BROWSER_NAVIGATE_TOOL,
    MCP_CLEANUP_PROCESSES_TOOL, MCP_ORACLE_SEARCH_TOOL, MCP_PROCESS_OWNER_TOOL, MCP_ROSTER_TOOL,
    MCP_SESSION_PROCESSES_TOOL,
};
use crate::server::ServerState;
use devboule_protocol::PEER_CAPS;

fn listed(state: &Arc<ServerState>) -> Vec<String> {
    let token = state.mcp.test_token(CALLER).expect("token");
    let reply = response_json(&http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
    ));
    reply
        .pointer("/result/tools")
        .and_then(Value::as_array)
        .unwrap_or_else(|| panic!("a tool list: {reply}"))
        .iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str).map(str::to_string))
        .collect()
}

const LOCAL_ONLY: [&str; 4] = [
    MCP_PROCESS_OWNER_TOOL,
    MCP_SESSION_PROCESSES_TOOL,
    MCP_CLEANUP_PROCESSES_TOOL,
    MCP_BROWSER_FILL_LOGIN_TOOL,
];

#[test]
fn a_local_caller_sees_every_enabled_tool() {
    let state = ServerState::new("mcp-list-local".to_string());
    let (_guard, _server) = serve(&state, None);

    let names = listed(&state);

    assert_eq!(
        names.len(),
        MCP_BROKER_TOOLS.len(),
        "the whole catalog: {names:?}"
    );
    for tool in LOCAL_ONLY {
        assert!(
            names.iter().any(|name| name == tool),
            "{tool} is listed for the person here"
        );
    }
}

#[test]
fn a_paired_device_sees_only_what_its_capabilities_let_it_call() {
    let state = ServerState::new("mcp-list-peer-view".to_string());
    let (_guard, _server) = serve(&state, Some(&[CAP_VIEW]));

    let names = listed(&state);

    assert!(
        names.iter().any(|name| name == MCP_ROSTER_TOOL),
        "view reads the roster: {names:?}"
    );
    for hidden in [MCP_ORACLE_SEARCH_TOOL, MCP_BROWSER_NAVIGATE_TOOL] {
        assert!(
            !names.iter().any(|name| name == hidden),
            "{hidden} needs a grant it lacks"
        );
    }
    for tool in LOCAL_ONLY {
        assert!(
            !names.iter().any(|name| name == tool),
            "{tool} is never a paired device's"
        );
    }
    assert!(
        names.len() < MCP_BROKER_TOOLS.len() - LOCAL_ONLY.len(),
        "a one-capability device sees a strict subset: {names:?}"
    );
}

/// Every capability opens everything a paired device can ever call — and still
/// not the four tools that are this machine's alone.
#[test]
fn a_paired_device_holding_everything_still_does_not_see_the_local_only_tools() {
    let state = ServerState::new("mcp-list-peer-all".to_string());
    let (_guard, _server) = serve(&state, Some(&PEER_CAPS));

    let names = listed(&state);

    assert_eq!(
        names.len(),
        MCP_BROKER_TOOLS.len() - LOCAL_ONLY.len(),
        "the catalog minus the local-only tools: {names:?}"
    );
    for tool in LOCAL_ONLY {
        assert!(
            !names.iter().any(|name| name == tool),
            "{tool} is local only"
        );
    }
    assert!(names.iter().any(|name| name == MCP_ORACLE_SEARCH_TOOL));
}

#[test]
fn a_grant_adds_exactly_its_tools_to_the_listing() {
    let without = ServerState::new("mcp-list-peer-without".to_string());
    let (_guard, _server) = serve(&without, Some(&[CAP_VIEW]));
    let before = listed(&without);
    drop((_guard, _server));

    let with = ServerState::new("mcp-list-peer-with".to_string());
    let (_guard, _server) = serve(&with, Some(&[CAP_VIEW, CAP_SEARCH]));
    let after = listed(&with);

    let added: Vec<&String> = after.iter().filter(|name| !before.contains(name)).collect();
    assert_eq!(
        added,
        [&MCP_ORACLE_SEARCH_TOOL.to_string()],
        "search opens search and nothing else"
    );
}
