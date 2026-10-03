//! What the broker serves of the browser lane: the table of names and commands,
//! the schema each one offers, and the sentence each description carries.
//!
//! Nothing here runs a command; the two tables the door reads live in
//! `tools::browser_commands` and `tools::browser_args`, and what is proved here
//! is that the broker, the peer door and an agent's reading all see the same set.

use serde_json::json;

use super::browser_tools_harness::panel;
use super::tests::{http_request, response_json};
use super::tools::browser_commands::{self, TOOLS};

/// The sentence an agent reads for one tool, from the catalog the broker
/// serves its `tools/list` from.
fn description_of(tool: &str) -> &'static str {
    crate::provider_catalog::MCP_BROKER_TOOLS
        .iter()
        .find(|(name, _)| *name == tool)
        .map(|(_, description)| *description)
        .unwrap_or_else(|| panic!("{tool} is served"))
}

#[test]
fn every_contract_command_is_served_under_its_browser_name() {
    let contract = [
        ("browser_new_tab", "new_tab"),
        ("browser_list_tabs", "list_tabs"),
        ("browser_close_tab", "close_tab"),
        ("browser_navigate", "navigate"),
        ("browser_snapshot", "snapshot"),
        ("browser_find", "find"),
        ("browser_click", "click"),
        ("browser_fill", "fill"),
        ("browser_type", "type"),
        ("browser_press", "press"),
        ("browser_select", "select"),
        ("browser_check", "check"),
        ("browser_hover", "hover"),
        ("browser_scroll", "scroll"),
        ("browser_wait_for", "wait_for"),
    ];
    assert_eq!(
        TOOLS,
        &contract[..],
        "the served table is the contract's 4b-1 wave"
    );
    for (tool, command) in contract {
        assert_eq!(browser_commands::command_for(tool), Some(command), "{tool}");
        assert!(browser_commands::serves(tool), "{tool} is served");
    }
    assert!(!browser_commands::serves("devboule_browser_click"));
    // 4b-2's commands are not served yet: a tool that always answers "no such
    // tab" is a tool an agent will try.
    assert_eq!(browser_commands::command_for("browser_act"), None);
    assert_eq!(browser_commands::command_for("browser_screenshot"), None);
    assert_eq!(browser_commands::command_for("browser_read_text"), None);
}

/// The prefix the peer door judges on lives in another module on purpose, so no
/// tool of this lane needs a second edit there. That is only true while the two
/// name the same set.
#[test]
fn the_browser_prefix_is_exactly_the_browser_table() {
    let mut by_prefix = crate::provider_catalog::MCP_BROKER_TOOLS
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| name.starts_with(crate::provider_catalog::BROWSER_TOOL_PREFIX))
        .collect::<Vec<_>>();
    let mut by_table = TOOLS.iter().map(|(tool, _)| *tool).collect::<Vec<_>>();
    by_prefix.sort_unstable();
    by_table.sort_unstable();
    assert_eq!(
        by_prefix, by_table,
        "the peer door's prefix and the broker's table must name one set"
    );
}

/// The sentence about a ref that is gone is repeated on purpose — each
/// description is read on its own, without the others — so this is what keeps
/// one of them from losing it.
#[test]
fn a_tool_that_takes_a_ref_says_what_to_do_when_it_is_gone() {
    let takes_ref = TOOLS
        .iter()
        .filter(|(tool, _)| {
            browser_commands::schema_for(tool)
                .is_some_and(|schema| schema.pointer("/properties/ref").is_some())
        })
        .map(|(tool, _)| *tool)
        .collect::<Vec<_>>();
    assert_eq!(takes_ref.len(), 9, "the ref-taking tools: {takes_ref:?}");
    for tool in takes_ref {
        let description = description_of(tool);
        assert!(
            description.contains("stale_ref") && description.contains("snapshot"),
            "{tool} must say what a gone ref answers and what to do about it: {description}"
        );
    }
}

#[test]
fn the_list_carries_the_contract_schema_for_every_browser_tool() {
    let panel = panel("schemas");
    let body = response_json(&http_request(
        panel.state.mcp.url(),
        Some(&format!("Bearer {}", panel.token())),
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
    ));
    let listed = body["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .map(|tool| {
            (
                tool["name"].as_str().expect("name").to_string(),
                tool["inputSchema"].clone(),
                tool["description"].as_str().unwrap_or_default().to_string(),
            )
        })
        .collect::<Vec<_>>();
    let schema_of = |tool: &str| {
        listed
            .iter()
            .find(|(name, _, _)| name == tool)
            .map(|(_, schema, _)| schema.clone())
            .unwrap_or_else(|| panic!("{tool} is served"))
    };

    for (tool, _) in TOOLS {
        let (_, schema, description) = listed
            .iter()
            .find(|(name, _, _)| name == tool)
            .unwrap_or_else(|| panic!("{tool} is not served"));
        assert_eq!(
            *schema,
            browser_commands::schema_for(tool).unwrap(),
            "{tool}"
        );
        assert_eq!(schema["type"], "object", "{tool}");
        assert_eq!(schema["additionalProperties"], json!(false), "{tool}");
        assert!(
            !description.is_empty(),
            "{tool} needs a description an agent can read"
        );
    }

    let snapshot = schema_of("browser_snapshot");
    assert_eq!(snapshot["required"], json!(["browserId"]));
    assert_eq!(snapshot["properties"]["scope"]["pattern"], "^e\\d+$");
    assert_eq!(
        snapshot["properties"]["mode"]["enum"],
        json!(["interactive", "full"])
    );
    assert_eq!(
        snapshot["properties"]["browserId"]["maxLength"],
        json!(crate::browser_affinity::MAX_BROWSER_ID_BYTES)
    );
    let click = schema_of("browser_click");
    assert_eq!(click["required"], json!(["browserId", "ref"]));
    assert_eq!(click["properties"]["clickCount"]["minimum"], json!(1));
    assert_eq!(
        click["properties"]["button"]["enum"],
        json!(["left", "middle", "right"])
    );
    assert_eq!(
        schema_of("browser_wait_for")["properties"]["timeoutMs"]["maximum"],
        json!(12_000)
    );
    assert_eq!(schema_of("browser_list_tabs")["properties"], json!({}));
}

/// One declaration, two readers. The daemon's closed argument set is exactly
/// the schema's property set: a name the schema offers is never "not an
/// argument", and a name it does not offer never reaches a host.
/// The tools are listed whether or not a host is registered. `tools/list` is a
/// pure walk of the catalog, with no conditional tool to copy, and a name an
/// agent cannot see is one it asks a person about instead of trying.

#[test]
fn the_browser_tools_are_listed_with_no_host_registered() {
    let panel = panel("listed");
    let body = response_json(&http_request(
        panel.state.mcp.url(),
        Some(&format!("Bearer {}", panel.token())),
        r#"{"jsonrpc":"2.0","id":6,"method":"tools/list"}"#,
    ));
    let names = body["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect::<Vec<_>>();
    for (tool, _) in TOOLS {
        assert!(names.contains(tool), "{tool} must be listed: {names:?}");
    }
}
