//! Who may call the process tools: a paired device never, whatever it holds.
//!
//! The door dispatch consults is `mcp_peer_door` (dispatch.rs, the
//! `tools/call` path); the locality check it runs first is where these three
//! are refused, so no capability combination — empty or complete — opens
//! them, and the sentence is the locality sentence itself.

use serde_json::json;

use super::caller::{mcp_peer_door, McpCaller};
use crate::peer_policy::mcp_tool_locality;

use crate::provider_catalog::{
    MCP_CLEANUP_PROCESSES_TOOL, MCP_PROCESS_OWNER_TOOL, MCP_SESSION_PROCESSES_TOOL,
};

const PROCESS_TOOLS: [&str; 3] = [
    MCP_PROCESS_OWNER_TOOL,
    MCP_SESSION_PROCESSES_TOOL,
    MCP_CLEANUP_PROCESSES_TOOL,
];

fn peer(held: &[&str]) -> McpCaller {
    McpCaller::Peer {
        device_id: "dev-process".to_string(),
        caps: held.iter().map(|cap| (*cap).to_string()).collect(),
    }
}

/// Every capability a pairing can grant still refuses all three tools, and an
/// unpaired-strength set refuses them the same way: the refusal is the
/// locality sentence, not a capability name — no capability opens them.
#[test]
fn the_peer_door_refuses_every_process_tool_for_every_capability_set() {
    let everything = [
        "view",
        "send",
        "answer_permissions",
        "create_sessions",
        "roster",
        "search",
        "admin",
        "browser",
    ];
    let sentence =
        "the processes of this machine are never inspected or stopped from a paired device";
    for tool in PROCESS_TOOLS {
        for held in [&everything[..], &[][..]] {
            let refused = mcp_peer_door(&peer(held), Some(tool), &json!(1))
                .unwrap_or_else(|| panic!("{tool} must be refused for a paired device"));
            assert_eq!(
                refused.pointer("/error/message"),
                Some(&json!(sentence)),
                "the locality sentence, not a capability name: {refused}"
            );
            assert_eq!(refused.pointer("/error/code"), Some(&json!(-32601)));
        }
        assert_eq!(
            mcp_tool_locality(tool),
            Some(sentence),
            "{tool} is refused at the locality check itself"
        );
    }
}

/// A local caller is never judged at this door — the tools are ordinary for
/// the daemon's own sessions.
#[test]
fn a_local_caller_is_not_judged_at_the_peer_door() {
    for tool in PROCESS_TOOLS {
        assert!(mcp_peer_door(&McpCaller::Local, Some(tool), &json!(2)).is_none());
    }
}
