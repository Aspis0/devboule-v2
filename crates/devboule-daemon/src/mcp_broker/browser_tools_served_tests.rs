//! What the broker serves of the browser lane: the table of names and commands,
//! the schema each one offers, and the sentence each description carries.
//!
//! Nothing here runs a command; the two tables the door reads live in
//! `tools::browser_commands` and `tools::browser_args`, and what is proved here
//! is that the broker, the peer door and an agent's reading all see the same set.

use serde_json::{json, Value};

use super::browser_tools_harness::{design_panel, panel, tools_call, FakeHost};
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
        ("browser_act", "act"),
        ("browser_screenshot", "screenshot"),
        ("browser_click_at", "click_at"),
        ("browser_read_text", "read_text"),
        ("browser_console_logs", "console_logs"),
        ("browser_fill_login", "fill_login"),
    ];
    assert_eq!(
        TOOLS,
        &contract[..],
        "the served table is the contract's two waves and the saved login"
    );
    for (tool, command) in contract {
        assert_eq!(browser_commands::command_for(tool), Some(command), "{tool}");
        assert!(browser_commands::serves(tool), "{tool} is served");
    }
    assert!(!browser_commands::serves("devboule_browser_click"));
    assert_eq!(
        browser_commands::command_for("browser_screenshot_all"),
        None
    );
}

/// The second wave's bounds are stated where an agent reads them: the batch
/// length a step list may have, the zoom and the four clip numbers, and the
/// closed console levels. A step's own arguments are stated too, because an
/// agent that cannot see them guesses, and is then refused for guessing.
#[test]
fn the_second_waves_schema_states_the_bounds_the_daemon_enforces() {
    let act = browser_commands::schema_for("browser_act").expect("browser_act is served");
    let steps = &act["properties"]["steps"];
    assert_eq!(steps["type"], "array", "{steps}");
    assert_eq!(steps["minItems"], json!(1), "{steps}");
    assert_eq!(steps["maxItems"], json!(10), "{steps}");
    let entries = steps["items"]["oneOf"]
        .as_array()
        .expect("one step shape per command");
    assert_eq!(
        Value::Array(
            entries
                .iter()
                .map(|entry| entry["properties"]["command"]["const"].clone())
                .collect::<Vec<_>>()
        ),
        json!([
            "click", "fill", "type", "press", "select", "check", "hover", "scroll", "wait_for",
            "navigate"
        ]),
        "the contract's ten, and never an act inside one"
    );
    let entry = |command: &str| {
        entries
            .iter()
            .find(|entry| entry["properties"]["command"]["const"] == json!(command))
            .unwrap_or_else(|| panic!("{command} has a step shape: {steps}"))
            .clone()
    };
    // The fill a live agent got wrong: the schema has to say `text`.
    let fill = entry("fill");
    assert_eq!(
        fill["required"],
        json!(["command", "ref", "text"]),
        "{fill}"
    );
    assert_eq!(fill["additionalProperties"], json!(false), "{fill}");
    assert_eq!(fill["properties"]["ref"]["pattern"], "^e\\d+$", "{fill}");
    assert_eq!(
        entry("click")["properties"]["clickCount"],
        browser_commands::schema_for("browser_click").expect("click")["properties"]["clickCount"],
        "a step takes the arguments the command itself takes"
    );
    for entry in entries {
        assert!(
            entry["properties"].get("browserId").is_none(),
            "no step names a tab: {entry}"
        );
    }

    let screenshot = browser_commands::schema_for("browser_screenshot").expect("screenshot");
    assert_eq!(screenshot["properties"]["zoom"]["type"], "number");
    assert_eq!(screenshot["properties"]["zoom"]["minimum"], json!(1.0));
    assert_eq!(screenshot["properties"]["zoom"]["maximum"], json!(3.0));
    let clip = &screenshot["properties"]["clip"]["properties"];
    for corner in ["x", "y", "width", "height"] {
        assert_eq!(clip[corner]["minimum"], json!(0.0), "{corner}");
    }

    let logs = browser_commands::schema_for("browser_console_logs").expect("console_logs");
    assert_eq!(
        logs["properties"]["level"]["enum"],
        json!(["error", "warning", "all"])
    );
    let read_text = browser_commands::schema_for("browser_read_text").expect("read_text");
    assert_eq!(read_text["required"], json!(["browserId"]));
    assert_eq!(read_text["properties"]["scope"]["pattern"], "^e\\d+$");
    let click_at = browser_commands::schema_for("browser_click_at").expect("click_at");
    assert_eq!(click_at["required"], json!(["browserId", "x", "y"]));
    assert_eq!(click_at["properties"]["clickCount"]["maximum"], json!(3));

    let login = browser_commands::schema_for("browser_fill_login").expect("fill_login");
    assert_eq!(login["required"], json!(["browserId"]), "{login}");
    assert_eq!(
        login["additionalProperties"],
        json!(false),
        "the entry a person chooses is not an argument an agent may send: {login}"
    );
    for field in ["usernameRef", "passwordRef"] {
        assert_eq!(login["properties"][field]["pattern"], "^e\\d+$", "{field}");
    }
}

/// The saved login is not a batch step: a batch would carry it inside another
/// call's arguments, and a person approving a batch approves every step in it
/// without naming which login any of them used.
#[test]
fn a_batch_step_may_not_be_a_saved_login_fill() {
    let step = json!({
        "command": "fill_login",
        "passwordRef": "e14",
    });
    let refused = crate::mcp_broker::tools::browser_steps::check_steps(
        "browser_act",
        "steps",
        &json!([step]),
    )
    .expect_err("a batch step may not fill a login");
    assert!(
        refused.contains("fill_login"),
        "the refusal names the command: {refused}"
    );
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
    assert_eq!(
        snapshot["properties"]["browserId"]["pattern"],
        json!("^[\\x21-\\x7E]+$"),
        "the schema's alphabet is the one the daemon enforces"
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

/// A design child is a child of someone's session, and the lane is this
/// machine's pages in the logins of the person at the keyboard, so the overlay
/// denies the whole family — neither the list an agent reads nor the call it
/// makes. Driven on a session registered with the real overlay, through the
/// real door, because a local caller is otherwise unjudged.
#[test]
fn a_design_child_is_served_none_of_the_lane_and_reaches_no_host() {
    let panel = design_panel("design");
    let host = FakeHost::register(&panel.state, 5);
    let body = response_json(&http_request(
        panel.state.mcp.url(),
        Some(&format!("Bearer {}", panel.token())),
        r#"{"jsonrpc":"2.0","id":7,"method":"tools/list"}"#,
    ));
    let names = body["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect::<Vec<_>>();
    for (tool, _) in TOOLS {
        assert!(
            !names.contains(tool),
            "{tool} must not be listed: {names:?}"
        );
    }

    let refused = response_json(&http_request(
        panel.state.mcp.url(),
        Some(&format!("Bearer {}", panel.token())),
        &tools_call("browser_click", json!({"browserId": "tab-1", "ref": "e1"})),
    ));
    assert_eq!(
        refused.pointer("/error/code"),
        Some(&json!(-32601)),
        "{refused}"
    );
    assert_eq!(
        refused.pointer("/error/message"),
        Some(&json!("Tool disabled by policy")),
        "{refused}"
    );
    assert!(
        host.pending().is_empty(),
        "the overlay refuses before a host is asked"
    );
}
