//! Every name the broker can serve has exactly one peer rule, walked from the
//! served set itself — the listed catalog, the browser lane's own table and every
//! route the dispatcher names — never a list typed beside it.

use super::tools::browser_commands;
use crate::peer_policy::mcp_tool_wire;
use crate::provider_catalog::MCP_BROKER_TOOLS;

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
