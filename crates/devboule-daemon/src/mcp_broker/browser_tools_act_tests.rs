//! What one `act` batch does: the steps it accepts, the frame the host
//! receives, and what an agent reads back.
//!
//! A step is checked against the row its own command declares, so the
//! vocabulary is one list and not a second spelling of ten commands' arguments.

use serde_json::{json, Value};

use super::browser_tools_harness::{host_document, panel, FakeHost};
use super::tools::browser_args::parse;
use super::tools::browser_commands::spec_for;

/// The sentence a refused batch answers, or a panic naming the batch that was
/// not refused.
fn refusal(arguments: &Value) -> String {
    match parse(
        spec_for("browser_act").expect("browser_act is served"),
        "browser_act",
        arguments,
    ) {
        Ok(_) => panic!("browser_act: {arguments} must be refused"),
        Err(sentence) => sentence,
    }
}

/// A batch of `steps`, on the tab the batch names.
fn batch(steps: Value) -> Value {
    json!({"browserId": "tab-1", "steps": steps})
}

/// The steps are read by an agent on every turn this tool is offered, so their
/// size is a cost: a shape per command, built out of the rows, and nothing
/// spelled a second time inside it. The bound is loose enough to survive a
/// field gaining a description and tight enough to catch a schema that started
/// carrying the whole lane twice.
#[test]
fn the_steps_an_agent_reads_stay_small() {
    let schema = super::tools::browser_commands::schema_for("browser_act").expect("browser_act");
    let printed = serde_json::to_string(&schema["properties"]["steps"]).expect("steps");
    assert!(
        printed.len() < 4_000,
        "the steps schema is {} characters: {printed}",
        printed.len()
    );
}

#[test]
fn a_batch_reaches_the_host_as_written_and_answers_unchanged() {
    let panel = panel("act");
    let host = FakeHost::register(&panel.state, 5);
    let steps = json!([
        {"command": "click", "ref": "e3"},
        {"command": "fill", "ref": "e4", "text": "hello"},
        {"command": "press", "key": "Enter"},
    ]);
    let result = json!({
        "steps": [{"command": "click", "ok": true}],
        "delta": {"navigated": true, "url": "https://example.test/next", "title": "Next"},
    });
    let (body, request) = panel.call(&host, "browser_act", batch(steps.clone()), result.clone());
    assert_eq!(request.command, "act");
    assert_eq!(
        request.args,
        batch(steps),
        "the daemon checks a step and sends the step as written"
    );
    assert_eq!(
        host_document(&body),
        result,
        "the host's answer passes through unchanged"
    );
}

#[test]
fn a_step_is_checked_against_the_command_it_names() {
    let cases = [
        (
            json!([{"command": "click"}]),
            "'ref' is required",
            "click without the ref it needs",
        ),
        (
            json!([{"command": "click", "ref": "3"}]),
            "must be a ref",
            "a ref that is not one",
        ),
        (
            json!([{"command": "navigate"}]),
            "exactly one of",
            "a navigate with neither a url nor an action",
        ),
        (
            json!([{"command": "click", "ref": "e1", "button": "sideways"}]),
            "must be one of",
            "a button outside the closed list",
        ),
        (
            json!([{"command": "scroll", "amount": 300}]),
            "exactly one of",
            "a scroll amount with nothing to scroll",
        ),
    ];
    for (steps, expected, what) in cases {
        let refused = refusal(&batch(steps.clone()));
        assert!(
            refused.contains(expected),
            "{what}: {refused} (want {expected})"
        );
    }
}

#[test]
fn a_step_runs_only_a_command_the_contract_lists_for_a_batch() {
    for command in [
        "act",
        "screenshot",
        "click_at",
        "read_text",
        "console_logs",
        "new_tab",
    ] {
        let refused = refusal(&batch(json!([{"command": command}])));
        assert!(
            refused.contains("not one of"),
            "{command} may not be a step: {refused}"
        );
    }
    let refused = refusal(&batch(json!([{"command": "nope"}])));
    assert!(refused.contains("not one of"), "{refused}");
}

#[test]
fn a_step_may_not_name_a_tab_of_its_own() {
    let refused = refusal(&batch(json!([{
        "command": "click",
        "ref": "e1",
        "browserId": "tab-9",
    }])));
    assert!(refused.contains("browserId"), "{refused}");
}

#[test]
fn a_batch_is_between_one_and_ten_steps() {
    let press = || json!({"command": "press", "key": "Tab"});
    let refused = refusal(&batch(json!([])));
    assert!(refused.contains("1 to 10 steps"), "{refused}");
    let refused = refusal(&batch(Value::Array(vec![press(); 11])));
    assert!(refused.contains("1 to 10 steps"), "{refused}");
    assert!(
        parse(
            spec_for("browser_act").expect("browser_act is served"),
            "browser_act",
            &batch(Value::Array(vec![press(); 10])),
        )
        .is_ok(),
        "ten steps are inside the contract"
    );
}

#[test]
fn a_step_is_an_object_naming_one_command() {
    for (steps, expected) in [
        (json!(["click"]), "must be an object"),
        (json!([{"ref": "e1"}]), "must name a 'command'"),
        (json!([{"command": 7}]), "must name 'command' as text"),
        (json!({"command": "click"}), "must be a list"),
    ] {
        let refused = refusal(&batch(steps.clone()));
        assert!(
            refused.contains(expected),
            "{steps} must be refused: {refused} (want {expected})"
        );
    }
}
