//! The line a browser tool call shows: every command of the lane, and the two
//! rules that keep a row a row (a value is cut, a url is a host).

use serde_json::json;

use super::browser_tool_title;

/// The row for one call, or a panic naming the call that has no row.
fn row(tool: &str, input: serde_json::Value) -> String {
    browser_tool_title(tool, &input).unwrap_or_else(|| panic!("{tool} has a row"))
}

/// One call of every command the broker serves, with the line each one reads as.
#[test]
fn a_row_names_the_command_and_the_argument_it_was_given() {
    let cases = [
        (
            "browser_new_tab",
            json!({"url": "https://news.ycombinator.com/newest"}),
            "new tab news.ycombinator.com",
        ),
        ("browser_list_tabs", json!({}), "list tabs"),
        (
            "browser_close_tab",
            json!({"browserId": "tab-1"}),
            "close tab",
        ),
        (
            "browser_navigate",
            json!({"action": "back"}),
            "navigate back",
        ),
        (
            "browser_navigate",
            json!({"url": "https://example.com/a"}),
            "navigate example.com",
        ),
        ("browser_snapshot", json!({"scope": "e2"}), "snapshot e2"),
        ("browser_snapshot", json!({}), "snapshot"),
        (
            "browser_find",
            json!({"query": "sign in"}),
            "find \"sign in\"",
        ),
        ("browser_click", json!({"ref": "e33"}), "click e33"),
        (
            "browser_fill",
            json!({"ref": "e3", "text": "WebView2"}),
            "fill e3 (8 chars)",
        ),
        ("browser_type", json!({"text": "hello"}), "type (5 chars)"),
        (
            "browser_type",
            json!({"ref": "e4", "text": "hello"}),
            "type e4 (5 chars)",
        ),
        (
            "browser_press",
            json!({"key": "Control+A"}),
            "press Control+A",
        ),
        (
            "browser_select",
            json!({"ref": "e2", "label": "Dark"}),
            "select e2 (4 chars)",
        ),
        (
            "browser_check",
            json!({"ref": "e7", "checked": true}),
            "check e7 true",
        ),
        ("browser_hover", json!({"ref": "e9"}), "hover e9"),
        (
            "browser_scroll",
            json!({"direction": "down", "amount": 300}),
            "scroll down 300",
        ),
        (
            "browser_wait_for",
            json!({"ref": "e1", "state": "visible"}),
            "wait for e1 visible",
        ),
        (
            "browser_act",
            json!({"steps": [{"command": "click"}, {"command": "press"}]}),
            "act 2 steps",
        ),
        ("browser_screenshot", json!({}), "screenshot"),
        (
            "browser_screenshot",
            json!({"zoom": 2}),
            "screenshot zoom 2",
        ),
        (
            "browser_click_at",
            json!({"x": 120, "y": 80}),
            "click at 120,80",
        ),
        ("browser_read_text", json!({}), "read text"),
        ("browser_read_text", json!({"scope": "e4"}), "read text e4"),
        (
            "browser_console_logs",
            json!({"level": "error"}),
            "console logs error",
        ),
        ("browser_console_logs", json!({}), "console logs"),
    ];
    for (tool, input, expected) in cases {
        assert_eq!(row(tool, input), expected, "{tool}");
    }
}

/// A tab id names nothing a reader can use, so no row of the lane carries one.
#[test]
fn no_row_shows_the_tab_id() {
    for (tool, input) in [
        ("browser_click", json!({"browserId": "tab-1", "ref": "e33"})),
        ("browser_close_tab", json!({"browserId": "tab-1"})),
        (
            "browser_click_at",
            json!({"browserId": "tab-1", "x": 1, "y": 2}),
        ),
    ] {
        assert!(!row(tool, input).contains("tab-1"), "{tool}");
    }
}

/// A row is journaled and outlives the call, so it never shows what was typed
/// into a page: a password an agent filled would be legible in the transcript
/// forever. The row says how much there was, which is what a reader needs.
#[test]
fn a_row_never_shows_typed_text() {
    let typed = "correct horse battery staple";
    for (name, input) in [
        ("browser_fill", json!({"ref": "e3", "text": typed})),
        ("browser_type", json!({"text": typed})),
        ("browser_select", json!({"ref": "e2", "value": typed})),
        ("browser_select", json!({"ref": "e2", "label": typed})),
    ] {
        let line = row(name, input.clone());
        assert!(!line.contains("horse"), "{name}: {line}");
        assert!(!line.contains("battery"), "{name}: {line}");
        assert!(line.contains("(28 chars)"), "{name}: {line}");
    }
    assert_eq!(
        row("browser_fill", json!({"ref": "e3", "text": "a"})),
        "fill e3 (1 char)"
    );
    assert_eq!(
        row("browser_press", json!({"key": "Control+A"})),
        "press Control+A",
        "a key name is the act, not something typed into a page"
    );
}

#[test]
fn a_query_is_folded_onto_one_line_and_cut() {
    assert_eq!(
        row("browser_find", json!({"query": "first\nsecond\tthird"})),
        "find \"first second third\"",
        "a value that would break the row onto two lines is folded onto one"
    );
    let long = format!("{} {}", "word ".repeat(30), "end");
    let line = row("browser_find", json!({"query": long}));
    let prefix = "find \"".chars().count();
    assert!(line.chars().count() <= prefix + 40 + 2, "{line}");
    assert!(line.contains('…'), "{line}");
}

#[test]
fn a_url_is_a_host_and_never_a_credential_or_a_path() {
    assert_eq!(
        row(
            "browser_navigate",
            json!({"url": "https://user:secret@example.com:8443/a/b?q=1#top"}),
        ),
        "navigate example.com:8443",
        "the row names the host, and nothing a person typed into the app"
    );
    assert_eq!(
        row("browser_new_tab", json!({"url": "not a url"})),
        "new tab",
        "an address with no host to name shows no detail"
    );
}

#[test]
fn a_tool_outside_the_lane_keeps_the_title_its_own_view_gave_it() {
    assert_eq!(
        browser_tool_title("Read", &json!({"file_path": "src/lib.rs"})),
        None
    );
    assert_eq!(browser_tool_title("browser_nope", &json!({})), None);
}

/// A provider calls a broker tool by the name it was given, and Claude gives a
/// broker tool the MCP server's name in front of it
/// (`mcp__devboule__browser_click`). The row is the same call either way.
#[test]
fn a_provider_prefix_does_not_hide_the_lane() {
    let cases = [
        ("mcp__devboule__browser_click", "click e33"),
        ("browser_click", "click e33"),
        ("devboule_browser_new_tab", "new tab news.ycombinator.com"),
    ];
    for (name, expected) in cases {
        assert_eq!(
            row(
                name,
                json!({"ref": "e33", "url": "https://news.ycombinator.com/newest"})
            ),
            expected,
            "{name}"
        );
    }
    // Another MCP server's own tool that happens to carry our names is not ours.
    assert_eq!(
        browser_tool_title("mcp__probe__browser_click", &json!({})),
        None
    );
    assert_eq!(
        browser_tool_title("devboule_browser_nope", &json!({})),
        None
    );
}
