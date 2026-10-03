//! What one browser call does: the arguments the daemon accepts, the frame the
//! host receives, and what an agent reads back from an answer or a refusal.
//!
//! Driven through the loopback door a real agent uses, with a host registered on
//! the same broker the app registers on, so what is proved is the whole road and
//! not one function of it.

use serde_json::{json, Value};

use super::browser_tools_harness::*;
use super::tools::browser_args::parse;
use super::tools::browser_commands::{self, TOOLS};

/// The row one tool runs, so a call can be checked against it directly.
fn spec(tool: &str) -> &'static super::tools::browser_args::Spec {
    super::tools::browser_commands::spec_for(tool).expect("served tool")
}

/// The sentence a refused call answers, or a panic naming the call that was not
/// refused. `parse`'s own type has no `Debug`: a checked call carries the
/// arguments an agent typed, and no failure path needs them printed.
fn refusal(
    spec: &'static super::tools::browser_args::Spec,
    tool: &str,
    arguments: &serde_json::Value,
) -> String {
    match parse(spec, tool, arguments) {
        Ok(_) => panic!("{tool}: {arguments} must be refused"),
        Err(sentence) => sentence,
    }
}
#[test]
fn the_daemon_accepts_exactly_the_arguments_the_schema_offers() {
    for (tool, _) in TOOLS {
        let schema = browser_commands::schema_for(tool).expect("schema");
        for name in schema["properties"].as_object().expect("properties").keys() {
            let call = json!({ name.clone(): valid_value(name) });
            if let Err(refusal) = parse(spec(tool), tool, &call) {
                assert!(
                    !refusal.contains("is not an argument"),
                    "{tool}: the schema offers '{name}' and the daemon refuses it as unknown: {refusal}"
                );
            }
        }
        for smuggled in ["caller", "callerSessionId", "workspaceId", "screenshot"] {
            let refused = refusal(spec(tool), tool, &json!({ smuggled: "x" }));
            assert!(
                refused.contains(&format!("'{smuggled}' is not an argument")),
                "{tool}: {refused}"
            );
        }
    }
}

/// A value of the right shape for one argument name.
fn valid_value(name: &str) -> Value {
    match name {
        "browserId" => json!("tab-1"),
        "ref" | "scope" => json!("e123"),
        "clickCount" => json!(1),
        "amount" | "timeoutMs" => json!(100),
        "checked" => json!(true),
        "modifiers" => json!(["Control"]),
        "mode" => json!("interactive"),
        "action" => json!("back"),
        "button" => json!("left"),
        "direction" => json!("down"),
        _ => json!("x"),
    }
}

#[test]
fn a_shape_the_schema_forbids_is_refused_before_the_host_sees_it() {
    let cases = [
        (
            "browser_click",
            json!({"browserId": "tab-1", "ref": "123"}),
            "must be a ref",
        ),
        (
            "browser_click",
            json!({"browserId": "tab-1", "ref": "e12a"}),
            "must be a ref",
        ),
        (
            "browser_click",
            json!({"browserId": "", "ref": "e12"}),
            "must be a browserId",
        ),
        (
            "browser_check",
            json!({"browserId": "tab-1", "ref": "e12", "checked": "yes"}),
            "true or false",
        ),
        (
            "browser_wait_for",
            json!({"browserId": "t", "text": "hi", "timeoutMs": 12001}),
            "1 to 12000",
        ),
        (
            "browser_navigate",
            json!({"browserId": "t"}),
            "exactly one of",
        ),
        (
            "browser_navigate",
            json!({"browserId": "t", "url": "https://a.test", "action": "back"}),
            "exactly one of",
        ),
        (
            "browser_select",
            json!({"browserId": "t", "ref": "e1"}),
            "exactly one of",
        ),
        (
            "browser_select",
            json!({"browserId": "t", "ref": "e1", "value": "a", "label": "A"}),
            "exactly one of",
        ),
        (
            "browser_wait_for",
            json!({"browserId": "t", "ref": "e1"}),
            "exactly one of",
        ),
        (
            "browser_wait_for",
            json!({"browserId": "t", "ref": "e1", "state": "visible", "url": "https://a.test"}),
            "exactly one of",
        ),
        (
            "browser_find",
            json!({"browserId": "t"}),
            "'query' is required",
        ),
        (
            "browser_snapshot",
            json!({"mode": "full"}),
            "'browserId' is required",
        ),
        (
            "browser_click",
            json!({"browserId": "tab-1", "ref": "e1", "modifiers": ["Hyper"]}),
            "must be a list of",
        ),
    ];
    for (tool, arguments, expected) in cases {
        let refused = refusal(spec(tool), tool, &arguments);
        assert!(
            refused.contains(expected),
            "{tool}: {refused} (want {expected})"
        );
    }
}

/// The whole road for one command: the frame the host receives carries the
/// caller's own row and the tab, and the host's result comes back unchanged.
#[test]
fn a_call_reaches_the_host_as_the_callers_own_command_and_answers_unchanged() {
    let panel = panel("dispatch");
    let host = FakeHost::register(&panel.state, 7);
    let result = json!({
        "url": "https://example.test/",
        "title": "Example",
        "view": "- button \"Go\" [ref=e4]",
        "truncated": false,
    });
    let (body, request) = panel.call(
        &host,
        "browser_snapshot",
        json!({"browserId": "tab-9", "mode": "full"}),
        result.clone(),
    );
    assert_eq!(request.command, "snapshot");
    assert_eq!(
        request.args,
        json!({"browserId": "tab-9", "mode": "full"}),
        "the host receives the arguments as written"
    );
    assert_eq!(request.caller.caller_session_id, SESSION);
    assert_eq!(
        request.caller.workspace_id.as_deref(),
        Some("w-browser"),
        "the tab belongs to the caller's own workspace, never to an argument"
    );
    assert_eq!(body["result"]["isError"], json!(false), "{body}");
    assert_eq!(body["error"], Value::Null, "{body}");
    assert_eq!(
        body["result"]["structuredContent"], result,
        "the host's result passes through unchanged"
    );
    assert_eq!(
        serde_json::from_str::<Value>(tool_text(&body)).expect("text is JSON"),
        result,
        "the text an agent reads is the same document"
    );
}

/// An `err` outcome is a tool error an agent can read, and it carries both the
/// code and the host's own sentence — which is where the stale-ref advice is.
#[test]
fn a_failed_command_is_a_tool_error_naming_the_code_and_the_reason() {
    let panel = panel("failure");
    let host = FakeHost::register(&panel.state, 3);
    let reply = panel.in_background("browser_click", json!({"browserId": "tab-1", "ref": "e9"}));
    let request = host.next();
    host.answer_error(
        &panel.state,
        &request,
        host_refusal("stale_ref: e9 is gone; take a new snapshot"),
    );
    let body = reply.join().expect("tool call");
    assert_eq!(body["result"]["isError"], json!(true), "{body}");
    assert_eq!(
        body["error"],
        Value::Null,
        "a refusal is not a transport error"
    );
    assert_eq!(
        tool_text(&body),
        "browser_host_error: stale_ref: e9 is gone; take a new snapshot"
    );
    assert_eq!(
        body["result"].get("structuredContent"),
        None,
        "a failed command answers no document: {body}"
    );
}

/// With no host registered there is nothing to route to, and the refusal says so
/// in the daemon's own words rather than pretending the page answered.
#[test]
fn no_registered_host_answers_with_the_daemons_own_refusal() {
    let panel = panel("no-host");
    let body = panel
        .in_background("browser_list_tabs", json!({}))
        .join()
        .expect("tool call");
    assert_eq!(body["result"]["isError"], json!(true), "{body}");
    assert!(
        tool_text(&body).starts_with("browser_no_host: "),
        "{}",
        tool_text(&body)
    );
}
