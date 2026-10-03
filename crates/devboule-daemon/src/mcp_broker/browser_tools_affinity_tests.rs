//! Tab ownership as the tools reach it: an id learned from `browser_new_tab`
//! and forgotten by `browser_close_tab`, both through the loopback door.
//!
//! The broker's own unit tests prove the map; what is proved here is that the
//! tools pass the two facts the map needs — the bare command name, and the tab
//! the call is about — so the learning and the forgetting happen for a real
//! agent and not only for a direct `execute`.

use std::collections::BTreeSet;

use serde_json::json;

use super::browser_tools_harness::*;
use super::tests::{http_request, response_json};
use super::tools::browser_commands::TOOLS;

#[test]
fn a_tab_stays_with_the_host_that_opened_it_after_another_one_registered() {
    let panel = panel("learn");
    let first = FakeHost::register(&panel.state, 11);
    let (_, opened) = panel.call(
        &first,
        "browser_new_tab",
        json!({"url": "https://example.test/one"}),
        json!({"browserId": "tab-one", "url": "https://example.test/one", "title": "One"}),
    );
    assert_eq!(opened.command, "new_tab");
    assert_eq!(opened.caller.caller_session_id, SESSION);

    // A second window's host registers afterwards. An unscoped call would now go
    // to it; a call for `tab-one` must not — reading it off `first` is what
    // proves the route, since the wrong host would answer nothing.
    let second = FakeHost::register(&panel.state, 12);
    let (_, click) = panel.call(
        &first,
        "browser_click",
        json!({"browserId": "tab-one", "ref": "e3"}),
        json!({"delta": {"navigated": false, "url": "https://example.test/one"}}),
    );
    assert_eq!(click.command, "click");
    assert_eq!(
        click.args["browserId"],
        json!("tab-one"),
        "the host receives the tab inside args as well as beside them"
    );
    drop(second);
}

#[test]
fn closing_a_tab_through_the_tool_forgets_it_and_a_later_call_reaches_the_newest_host() {
    let panel = panel("unlearn");
    let first = FakeHost::register(&panel.state, 21);
    panel.call(
        &first,
        "browser_new_tab",
        json!({"url": "https://example.test/one"}),
        json!({"browserId": "tab-two", "url": "https://example.test/one", "title": "One"}),
    );
    let second = FakeHost::register(&panel.state, 22);

    // While the tab is owned, the close still goes to its owner.
    let (_, closed) = panel.call(
        &first,
        "browser_close_tab",
        json!({"browserId": "tab-two"}),
        json!({"closed": true}),
    );
    assert_eq!(closed.command, "close_tab");

    // After it, the id names nothing the broker knows, so the call is unscoped
    // again and the newest host answers it — with `browser_tab_not_found` if the
    // tab was never its own.
    let (_, after) = panel.call(
        &second,
        "browser_click",
        json!({"browserId": "tab-two", "ref": "e3"}),
        json!({"delta": {"navigated": false, "url": "https://example.test/"}}),
    );
    assert_eq!(after.command, "click");
}

#[test]
fn a_close_that_failed_keeps_the_tab_with_its_owner() {
    let panel = panel("keep");
    let first = FakeHost::register(&panel.state, 31);
    panel.call(
        &first,
        "browser_new_tab",
        json!({"url": "https://example.test/one"}),
        json!({"browserId": "tab-three", "url": "https://example.test/one", "title": "One"}),
    );
    let second = FakeHost::register(&panel.state, 32);

    let reply = panel.in_background("browser_close_tab", json!({"browserId": "tab-three"}));
    let request = first.next();
    first.answer_error(
        &panel.state,
        &request,
        host_refusal("the tab is already closed"),
    );
    assert_eq!(
        reply.join().expect("call")["result"]["isError"],
        json!(true),
        "the refusal reaches the agent"
    );

    // The owner kept it, so the next call for it still goes there.
    let (_, click) = panel.call(
        &first,
        "browser_click",
        json!({"browserId": "tab-three", "ref": "e1"}),
        json!({"delta": {}}),
    );
    assert_eq!(click.command, "click");
    drop(second);
}

/// The map is keyed by the caller's workspace as well as the tab id, so two
/// workspaces of one daemon may use the same id without meeting: the second
/// session's call for it is an unscoped call, and it goes to the newest host
/// rather than to the one that opened it for the first.
#[test]
fn two_workspaces_use_one_browser_id_without_meeting() {
    let mut panel = panel_in("scopes", "w-one");
    let other_token = panel.join_workspace("other", "w-two");
    let first = FakeHost::register(&panel.state, 41);
    panel.call(
        &first,
        "browser_new_tab",
        json!({"url": "https://example.test/mine"}),
        json!({"browserId": "shared-id", "url": "https://example.test/mine", "title": "Mine"}),
    );
    let second = FakeHost::register(&panel.state, 42);

    // The other workspace calls with the same id and is answered by the host
    // that never opened it.
    let url = panel.state.mcp.url().to_string();
    let bearer = format!("Bearer {other_token}");
    let body = tools_call("browser_snapshot", json!({"browserId": "shared-id"}));
    let reply =
        std::thread::spawn(move || response_json(&http_request(&url, Some(&bearer), &body)));
    let request = second.next();
    assert_eq!(request.command, "snapshot");
    assert_eq!(
        request.caller.workspace_id.as_deref(),
        Some("w-two"),
        "the scope is the caller's own row, whatever id it passed"
    );
    second.answer_ok(
        &panel.state,
        &request,
        json!({"url": "https://example.test/", "view": "", "truncated": false}),
    );
    assert_eq!(
        reply.join().expect("call")["result"]["isError"],
        json!(false)
    );
}

/// The tool names the bare commands a host registers, and the registration list
/// is spelled from this table — so a name a host cannot answer is never served.
#[test]
fn every_served_tool_names_a_bare_command() {
    let commands = TOOLS
        .iter()
        .map(|(_, command)| *command)
        .collect::<BTreeSet<_>>();
    assert_eq!(commands.len(), TOOLS.len(), "one command per tool");
    for command in &commands {
        assert!(
            command
                .chars()
                .all(|letter| letter.is_ascii_lowercase() || letter == '_'),
            "{command} is a bare command name"
        );
    }
}
