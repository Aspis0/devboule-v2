//! The peer door fails closed: a tool name with no peer rule is refused for a
//! paired device before anything routes it, and the rules that already existed
//! keep their answers for both roles and every capability set.

use std::sync::Arc;
use std::time::{Duration, Instant};

use devboule_protocol::{SessionKind, SessionOrigin, PEER_CAPS};
use serde_json::{json, Value};

use super::caller::{mcp_peer_door, McpCaller};
use super::{McpServerHandle, McpSessionGuard};
use crate::peer_policy::{
    mcp_refusal_message, mcp_tool_denial, mcp_tool_locality, CAP_ADMIN, CAP_BROWSER, CAP_SEARCH,
    UNLISTED_TOOL,
};
use devboule_protocol::PeerRole;

use crate::provider_catalog::{
    MCP_BROWSER_FILL_LOGIN_TOOL, MCP_CI_WATCH_TOOL, MCP_LIST_PROFILES_TOOL, MCP_ORACLE_SEARCH_TOOL,
    MCP_PROCESS_OWNER_TOOL, MCP_SEND_MESSAGE_TOOL,
};
use crate::server::ServerState;

use super::tests::{http_request, owner, peer_row, response_json};

const ROLES: [PeerRole; 2] = [PeerRole::Client, PeerRole::Daemon];
pub(super) const CALLER: &str = "fail-closed-peer-caller";
const UNLISTED_NAME: &str = "devboule_a_tool_nobody_wrote";

fn caps(held: &[&str]) -> Vec<String> {
    held.iter().map(|cap| (*cap).to_string()).collect()
}

fn every_cap() -> Vec<String> {
    caps(&PEER_CAPS)
}

fn peer(held: Vec<String>) -> McpCaller {
    McpCaller::Peer {
        device_id: "dev-fail-closed".to_string(),
        caps: held,
    }
}

/// Every capability set worth walking: none, each one alone, all of them.
fn capability_sets() -> Vec<Vec<String>> {
    let mut sets = vec![Vec::new()];
    sets.extend(PEER_CAPS.iter().map(|cap| caps(&[cap])));
    sets.push(every_cap());
    sets
}

#[test]
fn peer_unknown_tool_denied_at_the_helper_for_every_role_and_set() {
    for role in ROLES {
        for held in capability_sets() {
            assert_eq!(
                mcp_tool_denial(&held, UNLISTED_NAME),
                Some(UNLISTED_TOOL),
                "{role:?} holding {held:?} must not pass a name with no rule"
            );
            let refused = mcp_peer_door(&peer(held.clone()), Some(UNLISTED_NAME), &json!(1))
                .expect("the door refuses it");
            assert_eq!(
                refused.pointer("/error/message"),
                Some(&json!(mcp_refusal_message(UNLISTED_TOOL))),
                "{role:?} holding {held:?}: {refused}"
            );
            assert_eq!(refused.pointer("/error/code"), Some(&json!(-32601)));
        }
    }
    let sentence = mcp_refusal_message(UNLISTED_TOOL);
    assert!(
        !sentence.contains("Unknown tool") && !sentence.contains(UNLISTED_NAME),
        "the sentence is the door's own and never echoes the caller's name: {sentence}"
    );
}

// ── the real road: loopback broker, handle_rpc, the registry's origin ──

fn call(state: &Arc<ServerState>, token: &str, tool: &str) -> Value {
    response_json(&http_request(
        &state.mcp.url,
        Some(&format!("Bearer {token}")),
        &format!(
            r#"{{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{{"name":"{tool}","arguments":{{}}}}}}"#
        ),
    ))
}

/// A live caller registered with the loopback broker: born on a paired device
/// holding `held`, or the person at this machine for `None`. The two handles
/// keep the registration and the listener alive for the test.
pub(super) fn serve(
    state: &Arc<ServerState>,
    held: Option<&[&str]>,
) -> (McpSessionGuard, McpServerHandle) {
    let owner = owner("S-1-5-21-fail-closed", "fail-closed-client");
    crate::session::insert_test_live_agent_in_workspace(
        &state.sessions,
        CALLER,
        owner.clone(),
        "ws-a",
    );
    if let Some(held) = held {
        state.sessions.set_test_origin(
            CALLER,
            SessionOrigin::peer("device-phone", PeerRole::Client),
        );
        state
            .peer_upsert(peer_row("device-phone", held))
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

fn audit_actions(state: &Arc<ServerState>) -> Vec<String> {
    let path = state.paths.journal_file();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let connection = rusqlite::Connection::open(&path).expect("raw journal");
        let mut statement = connection
            .prepare("SELECT action FROM audit WHERE session_id = ?1")
            .expect("audit query");
        let actions: Vec<String> = statement
            .query_map([CALLER], |row| row.get(0))
            .expect("audit rows")
            .filter_map(Result::ok)
            .collect();
        if !actions.is_empty() || Instant::now() >= deadline {
            return actions;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// A device holding every capability is still refused a name nobody wrote, by
/// the door and not by the router's `Unknown tool`; the person at this machine
/// is not judged, and the refusal row never records the text the caller sent.
#[test]
fn peer_unknown_tool_denied_before_dispatch() {
    let state = ServerState::new("mcp-fail-closed-peer".to_string());
    let (_guard, _server) = serve(&state, Some(&PEER_CAPS));
    let token = state.mcp.test_token(CALLER).expect("token");

    let reply = call(&state, &token, UNLISTED_NAME);

    assert_eq!(
        reply.pointer("/error/code"),
        Some(&json!(-32601)),
        "{reply}"
    );
    assert_eq!(
        reply.pointer("/error/message"),
        Some(&json!(mcp_refusal_message(UNLISTED_TOOL))),
        "the door's sentence, not the router's: {reply}"
    );
    assert!(reply.get("result").is_none(), "{reply}");
    let actions = audit_actions(&state);
    assert!(
        actions.iter().any(|action| action == UNLISTED_TOOL),
        "the refusal is recorded under the stable label: {actions:?}"
    );
    assert!(
        !actions.iter().any(|action| action == UNLISTED_NAME),
        "a caller-chosen name is never written to the audit: {actions:?}"
    );
}

#[test]
fn a_local_caller_still_reaches_the_routers_own_unknown_tool_answer() {
    let state = ServerState::new("mcp-fail-closed-local".to_string());
    let (_guard, _server) = serve(&state, None);
    let token = state.mcp.test_token(CALLER).expect("token");

    let reply = call(&state, &token, UNLISTED_NAME);

    assert_eq!(
        reply.pointer("/error/message"),
        Some(&json!("Unknown tool")),
        "the person at this machine is not judged at the peer door: {reply}"
    );
}

// ── the rules that already existed keep their answers ──

#[test]
fn ci_watch_peer_requires_admin() {
    for role in ROLES {
        for held in capability_sets() {
            let expected = (!held.iter().any(|cap| cap == CAP_ADMIN)).then_some(CAP_ADMIN);
            assert_eq!(
                mcp_tool_denial(&held, MCP_CI_WATCH_TOOL),
                expected,
                "{role:?} holding {held:?} on the CI watch"
            );
        }
    }
}

#[test]
fn browser_fill_login_local_only() {
    for role in ROLES {
        for held in capability_sets() {
            let refused = mcp_peer_door(
                &peer(held.clone()),
                Some(MCP_BROWSER_FILL_LOGIN_TOOL),
                &json!(1),
            )
            .unwrap_or_else(|| panic!("{role:?} holding {held:?} must never use a saved login"));
            assert_eq!(
                refused.pointer("/error/message"),
                Some(&json!(
                    "a saved login of this machine is never used from a paired device"
                )),
                "the locality sentence, whatever the device holds: {refused}"
            );
        }
    }
    assert!(
        mcp_peer_door(
            &McpCaller::Local,
            Some(MCP_BROWSER_FILL_LOGIN_TOOL),
            &json!(2)
        )
        .is_none(),
        "the person at this machine reaches it"
    );
}

/// One tool of each rule kind, both roles, every capability set: the kinds are
/// judged on the wire (`send`), unjudged (the ticked profile list), a bare
/// capability (`search`, `browser`), local only (the process table), and — the
/// kind this slice adds — no rule at all.
#[test]
fn each_kind_of_peer_rule_answers_for_both_roles_and_every_capability_set() {
    for role in ROLES {
        for held in capability_sets() {
            let has = |cap: &str| held.iter().any(|name| name == cap);
            let ctx = format!("{role:?} holding {held:?}");

            let judged = mcp_tool_denial(&held, MCP_SEND_MESSAGE_TOOL);
            assert_eq!(judged.is_none(), has("send"), "{ctx}: judged on the wire");

            assert_eq!(
                mcp_tool_denial(&held, MCP_LIST_PROFILES_TOOL),
                None,
                "{ctx}: the ticked list is unjudged"
            );

            assert_eq!(
                mcp_tool_denial(&held, MCP_ORACLE_SEARCH_TOOL),
                (!has(CAP_SEARCH)).then_some(CAP_SEARCH),
                "{ctx}: a bare capability"
            );
            assert_eq!(
                mcp_tool_denial(&held, crate::provider_catalog::MCP_BROWSER_NAVIGATE_TOOL),
                (!has(CAP_BROWSER)).then_some(CAP_BROWSER),
                "{ctx}: the browser lane"
            );

            assert!(
                mcp_tool_locality(MCP_PROCESS_OWNER_TOOL).is_some(),
                "{ctx}: local only"
            );
            assert!(
                mcp_peer_door(&peer(held.clone()), Some(MCP_PROCESS_OWNER_TOOL), &json!(3))
                    .is_some(),
                "{ctx}: the door refuses the process table"
            );

            assert_eq!(
                mcp_tool_denial(&held, UNLISTED_NAME),
                Some(UNLISTED_TOOL),
                "{ctx}: no rule at all"
            );
        }
    }
}
