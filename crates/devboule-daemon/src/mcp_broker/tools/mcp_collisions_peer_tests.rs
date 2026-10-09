//! The collision tool at the peer door: a paired device's agent asks, and the
//! answer is the door's, before any body runs.
//!
//! This drives the real road — the loopback broker, `handle_rpc`, the origin
//! the registry holds for the caller's session, `peer_policy::mcp_tool_wire`
//! — because the defect this pins is one a direct call to the body cannot see:
//! the body is local-only work that a peer would otherwise reach.

use std::sync::Arc;

use devboule_protocol::PeerRole;
use devboule_protocol::{SessionKind, SessionOrigin};

use serde_json::{json, Value};

use crate::mcp_broker::{McpServerHandle, McpSessionGuard};
use crate::peer_policy::{CAP_ADMIN, CAP_VIEW};
use crate::server::ServerState;

use super::super::super::tests::{http_request, owner, peer_row, response_json};

const CALLER: &str = "collision-peer-caller";
const TOOL: &str = crate::provider_catalog::MCP_FILE_COLLISIONS_TOOL;

/// One `tools/call` against the loopback broker for `CALLER`, parsed.
fn call(state: &Arc<ServerState>, token: &str, arguments: &str) -> Value {
    response_json(&http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        &format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"{TOOL}","arguments":{arguments}}}}}"#
        ),
    ))
}

/// A caller born on a paired device that holds `caps`, with the broker
/// registration and server the call needs — the two handles are what keep the
/// loopback listener alive for the test.
fn serve(
    state: &Arc<ServerState>,
    caps: &[&str],
    peer: bool,
) -> (McpSessionGuard, McpServerHandle) {
    let user = if peer {
        "S-1-5-21-collisions-peer"
    } else {
        "S-1-5-21-collisions-local"
    };
    let owner = owner(user, "collisions-peer-client");
    crate::session::insert_test_live_agent_in_workspace(
        &state.sessions,
        CALLER,
        owner.clone(),
        "ws-a",
    );
    if peer {
        state.sessions.set_test_origin(
            CALLER,
            SessionOrigin::peer("device-phone", PeerRole::Client),
        );
        state
            .peer_upsert(peer_row("device-phone", caps))
            .expect("peer row");
    }
    let guard = state
        .mcp
        .register(CALLER, &owner, &SessionKind::Acp)
        .expect("registration")
        .expect("MCP guard");
    let server = state.mcp.start(state).expect("MCP server");
    (guard, server)
}

#[test]
fn a_paired_device_is_refused_the_collision_report_without_admin() {
    let state = ServerState::new("mcp-collisions-peer-no-admin".to_string());
    let (_guard, _server) = serve(&state, &[CAP_VIEW], true);
    let token = state.mcp.test_token(CALLER).expect("token");
    let reply = call(&state, &token, r#"{"path":"src/main.rs"}"#);
    assert_eq!(
        reply.pointer("/error/code"),
        Some(&json!(-32601)),
        "the door refuses it, not the body: {reply}"
    );
    assert_eq!(
        reply.pointer("/error/message"),
        Some(&json!("capability 'admin' was not negotiated")),
        "the refusal names the capability that would open it: {reply}"
    );
    assert!(
        reply.get("result").is_none(),
        "a refused call runs no body, so it discloses no repository, no worktree and no session id: {reply}"
    );
}

#[test]
fn a_paired_device_holding_admin_reaches_the_body() {
    let state = ServerState::new("mcp-collisions-peer-admin".to_string());
    let (_guard, _server) = serve(&state, &[CAP_VIEW, CAP_ADMIN], true);
    let token = state.mcp.test_token(CALLER).expect("token");
    let reply = call(&state, &token, r#"{"path":"src/main.rs"}"#);
    assert!(
        reply.pointer("/error/message").is_none(),
        "the door let it through: {reply}"
    );
    // The body refuses it for its own reason (this session's workspace is not
    // a repository the registry knows), which is what proves the door is the
    // thing that changed.
    assert_eq!(
        reply.pointer("/result/isError"),
        Some(&json!(true)),
        "the body answered for itself: {reply}"
    );
    assert_eq!(
        reply.pointer("/result/structuredContent/ok"),
        Some(&json!(false)),
        "{reply}"
    );
}

/// The door is a no-op for the person at this machine, like every other tool.
#[test]
fn a_local_caller_reaches_the_collision_report_unjudged() {
    let state = ServerState::new("mcp-collisions-peer-local".to_string());
    let (_guard, _server) = serve(&state, &[], false);
    let token = state.mcp.test_token(CALLER).expect("token");
    let reply = call(&state, &token, r#"{"path":"src/main.rs"}"#);
    assert!(
        reply.pointer("/error/message").is_none(),
        "a local caller is never judged: {reply}"
    );
    assert_eq!(
        reply.pointer("/result/structuredContent/ok"),
        Some(&json!(false)),
        "the body answered for itself: {reply}"
    );
}
