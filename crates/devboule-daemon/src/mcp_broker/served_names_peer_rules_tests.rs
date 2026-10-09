//! Every name the broker can serve has exactly one peer rule, walked from the
//! served set itself — the listed catalog, the browser lane's own table and every
//! route the dispatcher names — never a list typed beside it.

use devboule_protocol::PEER_CAPS;

use super::tools::browser_commands;
use crate::peer_policy::{
    mcp_tool_denial, mcp_tool_locality, mcp_tool_wire, McpToolWire, CAP_BROWSER, UNLISTED_TOOL,
};

use crate::provider_catalog::{
    BROWSER_TOOL_PREFIX, MCP_BROKER_TOOLS, MCP_BROWSER_FILL_LOGIN_TOOL, MCP_LIST_PROFILES_TOOL,
};

const UNLISTED_NAME: &str = "devboule_a_tool_nobody_wrote";

/// The tool constants the dispatcher routes by, read from its own source: a
/// route added there is in this set whether or not anyone remembered the
/// catalog.
fn dispatched_names() -> Vec<String> {
    let dispatch = include_str!("dispatch.rs");
    let catalog = include_str!("../provider_catalog.rs");
    let mut names = Vec::new();
    for (at, _) in dispatch.match_indices("provider_catalog::MCP_") {
        let start = at + "provider_catalog::".len();
        let ident: String = dispatch[start..]
            .chars()
            .take_while(|cell| cell.is_ascii_uppercase() || cell.is_ascii_digit() || *cell == '_')
            .collect();
        if !ident.ends_with("_TOOL") {
            continue;
        }
        let declaration = format!("pub const {ident}: &str = \"");
        let value_start = catalog
            .find(&declaration)
            .unwrap_or_else(|| panic!("{ident} is routed but not declared in the catalog"))
            + declaration.len();
        let value = &catalog[value_start..];
        names.push(value[..value.find('"').expect("closing quote")].to_string());
    }
    names.sort();
    names.dedup();
    names
}

/// Every name the broker can serve: the listed catalog, the browser lane's own
/// table (`browser_commands::serves`), and every route the dispatcher names.
fn served_names() -> Vec<String> {
    let mut names: Vec<String> = MCP_BROKER_TOOLS
        .iter()
        .map(|(name, _)| (*name).to_string())
        .collect();
    names.extend(
        browser_commands::TOOLS
            .iter()
            .map(|(name, _)| (*name).to_string()),
    );
    names.extend(dispatched_names());
    names.sort();
    names.dedup();
    names
}

#[test]
fn every_served_mcp_name_has_peer_rule() {
    let served = served_names();
    assert!(
        served.len() >= 40,
        "the served set was read: {}",
        served.len()
    );
    for name in &served {
        assert!(
            mcp_tool_wire(name).is_some(),
            "{name} is served but has no peer rule"
        );
    }
    // Nothing is routed that the listing does not carry, so the catalog walk
    // above is the whole set.
    let listed: Vec<&str> = MCP_BROKER_TOOLS.iter().map(|(name, _)| *name).collect();
    let mut routed = dispatched_names();
    routed.extend(
        browser_commands::TOOLS
            .iter()
            .map(|(name, _)| (*name).to_string()),
    );
    for name in &routed {
        assert!(
            listed.contains(&name.as_str()),
            "{name} is routed but not in the tools/list catalog"
        );
    }
    assert!(
        mcp_tool_wire(UNLISTED_NAME).is_none(),
        "the negative control is not served"
    );
}

/// The dispatcher routes by catalog constants only: a name typed as a literal
/// there would be a route this walk cannot see.
#[test]
fn the_dispatcher_routes_no_tool_by_a_string_literal() {
    let dispatch = include_str!("dispatch.rs");
    let literals: Vec<&str> = dispatch
        .lines()
        .filter(|line| line.contains("\"devboule_") || line.contains("\"browser_"))
        .collect();
    assert!(
        literals.is_empty(),
        "literal tool names in dispatch: {literals:?}"
    );
}

/// A row is more than existing: it is one of the known kinds, says what it
/// needs, and the unjudged ones are this machine's alone — refused by the
/// locality check too — except the one list a human enabled for agents. The two
/// tables must agree, so a process tool cannot become peer-callable by an edit to
/// one of them.
#[test]
fn every_served_row_is_a_known_kind_and_the_unjudged_ones_are_local_only() {
    for name in served_names() {
        match mcp_tool_wire(&name).unwrap_or_else(|| panic!("{name} has no row")) {
            McpToolWire::Judged(requests) => {
                assert!(
                    !requests.is_empty(),
                    "{name}: a judged tool names a wire act"
                );
                assert!(
                    mcp_tool_locality(&name).is_none(),
                    "{name}: judged yet local only"
                );
            }
            McpToolWire::Requires(capability) => {
                assert!(
                    PEER_CAPS.contains(&capability),
                    "{name}: `{capability}` is not a capability a device can hold"
                );
                assert!(
                    mcp_tool_locality(&name).is_none(),
                    "{name}: gated yet local only"
                );
            }
            McpToolWire::Unjudged(reason) => {
                assert!(!reason.is_empty(), "{name}: an unjudged tool says why");
                assert!(
                    name == MCP_LIST_PROFILES_TOOL || mcp_tool_locality(&name).is_some(),
                    "{name}: unjudged and not refused by the locality check"
                );
            }
        }
        if mcp_tool_locality(&name).is_some() {
            assert!(
                matches!(mcp_tool_wire(&name), Some(McpToolWire::Unjudged(_))),
                "{name}: refused by the locality check but not an unjudged row"
            );
        }
    }
}

/// The browser rule covers the lane's own table and nothing by prefix: a listed
/// `browser_*` name the table does not hold gets no lane row, and the saved-login
/// tool has a row of its own.
#[test]
fn a_browser_name_outside_the_lanes_table_is_not_given_the_lane_row() {
    for (name, _) in MCP_BROKER_TOOLS
        .iter()
        .filter(|(name, _)| name.starts_with(BROWSER_TOOL_PREFIX))
    {
        match mcp_tool_wire(name) {
            Some(McpToolWire::Requires(capability)) => {
                assert!(
                    crate::mcp_broker::browser_lane_serves(name) && capability == CAP_BROWSER,
                    "{name} has the lane row without being in the lane's table"
                );
            }
            Some(McpToolWire::Unjudged(_)) => {
                assert_eq!(
                    *name, MCP_BROWSER_FILL_LOGIN_TOOL,
                    "{name}: only the saved login"
                );
            }
            other => panic!("{name}: a listed browser tool with no usable row: {other:?}"),
        }
    }
    let everything: Vec<String> = PEER_CAPS.iter().map(|cap| (*cap).to_string()).collect();
    for synthetic in ["browser_export_cookies", "browser_", "browser_navigate "] {
        assert!(
            mcp_tool_wire(synthetic).is_none(),
            "{synthetic:?} is not in the lane's table"
        );
        assert_eq!(
            mcp_tool_denial(&everything, synthetic),
            Some(UNLISTED_TOOL),
            "holding the browser grant is still refused {synthetic:?}"
        );
    }
    assert!(
        matches!(
            mcp_tool_wire(MCP_BROWSER_FILL_LOGIN_TOOL),
            Some(McpToolWire::Unjudged(_))
        ),
        "the saved login has its own row, not the lane's"
    );
}
