//! Who may call a browser tool at all: the paired device's own switch.
//!
//! The rule is `search`'s, taken one step further. A paired device's agent reads
//! and clicks this machine's pages in the logins of the person at the keyboard,
//! so the lane rides its own capability, `browser`, granted per device from the
//! Devices panel and off for every pairing until then. Nothing about it is
//! reachable through `admin`, whose switch means settings, projects and
//! shutdown, and a local agent is never judged.

use serde_json::json;

use super::caller::{mcp_peer_door, McpCaller};
use super::tools::browser_commands::TOOLS;
use crate::peer_policy::{PeerRole, CAP_BROWSER};

fn peer(held: &[&str]) -> McpCaller {
    McpCaller::Peer {
        device_id: "dev-browser".to_string(),
        role: PeerRole::Client,
        caps: held.iter().map(|cap| (*cap).to_string()).collect(),
    }
}

/// A peer holding everything except the browser grant is refused, and the
/// refusal names the capability that is missing rather than saying "denied".
#[test]
fn a_peer_is_refused_every_browser_tool_until_it_holds_browser() {
    let every_other = [
        "view",
        "send",
        "answer_permissions",
        "create_sessions",
        "roster",
        "search",
        "admin",
    ];
    for (tool, _) in TOOLS {
        let refused = mcp_peer_door(&peer(&every_other), Some(tool), &json!(1))
            .unwrap_or_else(|| panic!("{tool} must be refused without `browser`"));
        assert_eq!(
            refused.pointer("/error/message"),
            Some(&json!(format!(
                "capability '{CAP_BROWSER}' was not negotiated"
            ))),
            "the refusal names the missing capability: {refused}"
        );
        assert_eq!(refused.pointer("/error/code"), Some(&json!(-32601)));
    }
}

/// The grant opens the whole lane, and only the lane: `admin` is not a
/// substitute for it, and it does not open anything else.
#[test]
fn the_browser_grant_opens_the_lane_and_nothing_else() {
    let mut held = vec![
        "view",
        "send",
        "answer_permissions",
        "create_sessions",
        "roster",
        "search",
    ];
    for (tool, _) in TOOLS {
        assert!(
            mcp_peer_door(&peer(&held), Some(tool), &json!(2)).is_some(),
            "{tool} stays refused for a peer holding only the act capabilities"
        );
        held.push(CAP_BROWSER);
        assert!(
            mcp_peer_door(&peer(&held), Some(tool), &json!(3)).is_none(),
            "{tool} passes for a peer holding `browser`"
        );
        held.pop();
    }
    assert_eq!(TOOLS.len(), 20, "every tool of the lane was walked");
}

/// `admin` alone is not the lane: a device the owner gave the whole surface
/// still has to be given the pages.
#[test]
fn admin_alone_does_not_reach_the_browser_tools() {
    let admin_only = ["view", "send", "admin"];
    assert!(mcp_peer_door(&peer(&admin_only), Some("browser_click"), &json!(4)).is_some());
    let mut with_browser = admin_only.to_vec();
    with_browser.push(CAP_BROWSER);
    assert!(mcp_peer_door(&peer(&with_browser), Some("browser_click"), &json!(5)).is_none());
}

/// A local agent passes unjudged, like every other served tool.
#[test]
fn a_local_caller_reaches_every_browser_tool() {
    for (tool, _) in TOOLS {
        assert!(
            mcp_peer_door(&McpCaller::Local, Some(tool), &json!(6)).is_none(),
            "{tool} is never judged for a session on this machine"
        );
    }
}

/// The switch is the only route in: a pairing is born without the grant, and
/// the wire refuses any other name.
#[test]
fn no_pairing_is_born_with_the_browser_grant() {
    assert!(
        !devboule_protocol::PEER_DEFAULT_CAPS.contains(&CAP_BROWSER),
        "a new device may steer this machine, not read the person's pages"
    );
    assert!(
        devboule_protocol::PEER_CAPS.contains(&CAP_BROWSER),
        "the name must be one a device can be granted"
    );
}
