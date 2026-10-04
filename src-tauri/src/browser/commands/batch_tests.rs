//! `act`: a batch runs the commands it was given, answers with the steps that
//! ran and one delta, and refuses a batch it cannot run whole.

use super::*;
use crate::browser::cdp::CdpError;
use crate::browser::commands::{BrowserError, Deadline};
use crate::browser::test_support::{ax_fixture, box_model, function_answer, parked_tab, FakePage};
use devboule_protocol::BrowserErrorCode;
use serde_json::json;

/// A page that answers a form and a navigation, and counts what it was asked.
fn form_page() -> FakePage {
    FakePage::new()
        .answering("Accessibility.getFullAXTree", ax_fixture())
        .answering("DOM.getBoxModel", box_model())
        .answering("DOM.getLayoutMetrics", viewport())
        .answering("DOM.resolveNode", json!({ "object": { "objectId": "7" } }))
        .answering("Runtime.callFunctionOn", function_answer(json!("done")))
}

fn viewport() -> Value {
    json!({ "cssLayoutViewport": { "pageX": 0, "pageY": 0, "clientWidth": 1024, "clientHeight": 768 } })
}

fn run(page: &FakePage, steps: Value) -> Result<Value, BrowserError> {
    let tab = parked_tab("tab-1");
    tauri::async_runtime::block_on(super::run(
        &tab,
        page,
        &json!({ "browserId": "tab-1", "steps": steps }),
        Deadline::in_(std::time::Duration::from_secs(10)),
    ))
}

fn steps(answered: &Value) -> &Vec<Value> {
    answered["steps"].as_array().expect("steps are a list")
}

#[test]
fn the_steps_run_in_order_and_the_batch_answers_with_one_delta() {
    let page = form_page();

    let answered = run(
        &page,
        json!([
            { "command": "fill", "ref": "e13", "text": "person@example.test" },
            { "command": "click", "ref": "e15" },
            { "command": "wait_for", "text": "remember" },
        ]),
    )
    .expect("the page answers");

    assert_eq!(
        steps(&answered)
            .iter()
            .map(|step| step["command"].as_str().unwrap_or_default())
            .collect::<Vec<_>>(),
        ["fill", "click", "wait_for"]
    );
    assert!(steps(&answered).iter().all(|step| step["ok"] == true));
    assert!(steps(&answered)
        .iter()
        .all(|step| step.get("error").is_none()));
    assert!(answered.get("delta").is_some(), "one delta for the batch");

    // The fill typed, then the click pressed: the order is the caller's.
    let typed = page
        .calls()
        .iter()
        .position(|(method, _)| method == "Input.insertText")
        .expect("the fill typed");
    let pressed = page
        .calls()
        .iter()
        .position(|(method, _)| method == "Input.dispatchMouseEvent")
        .expect("the click pressed");
    assert!(typed < pressed, "the fill went in before the click");
}

#[test]
fn a_step_that_fails_stops_the_batch_and_says_which_one() {
    let page = form_page();

    let answered = run(
        &page,
        json!([
            { "command": "click", "ref": "e15" },
            // A scroll in a direction that is not one: the shape is right, and
            // the command itself refuses it when it runs.
            { "command": "scroll", "direction": "sideways" },
            { "command": "click", "ref": "e15" },
        ]),
    )
    .expect("the batch answers even when a step does not");

    assert_eq!(steps(&answered).len(), 2, "the batch stopped");
    assert_eq!(steps(&answered)[0]["ok"], true);
    assert_eq!(steps(&answered)[1]["ok"], false);
    assert_eq!(steps(&answered)[1]["command"], "scroll");
    assert_eq!(
        steps(&answered)[1]["error"]["code"],
        "browser_host_error",
        "the refusal is in the protocol's own words"
    );
    assert_eq!(
        page.called("Input.dispatchMouseEvent"),
        2,
        "the click that ran, twice: pressed and released"
    );
    assert_eq!(
        page.called("Input.insertText"),
        0,
        "and nothing was typed into a step that never ran"
    );
}

#[test]
fn a_step_that_fails_on_a_dead_ref_says_stale_ref_the_way_the_command_would() {
    let page = FakePage::new()
        .answering("Accessibility.getFullAXTree", ax_fixture())
        .refusing("DOM.getBoxModel", crate::browser::cdp::CdpError::StaleRef);

    let answered = run(&page, json!([{ "command": "click", "ref": "e15" }])).expect("answered");

    let message = steps(&answered)[0]["error"]["message"]
        .as_str()
        .expect("a message")
        .to_owned();
    assert!(message.starts_with("stale_ref:"), "{message}");
}

#[test]
fn a_batch_is_refused_whole_before_the_first_step_runs() {
    let cases = [
        (
            json!([{ "command": "click", "ref": "e15" }, { "command": "act", "steps": [] }]),
            "cannot be an act",
        ),
        (
            json!([{ "command": "drag", "ref": "e1", "to": "e2" }]),
            "drag is not a command act can run",
        ),
        (
            json!([{ "command": "snapshot" }]),
            "snapshot is not a command act can run",
        ),
    ];

    for (steps_asked, because) in cases {
        let page = form_page();

        let error = run(&page, steps_asked).expect_err(because);

        assert_eq!(error.code, BrowserErrorCode::HostError, "{because}");
        assert!(
            error.message.contains(because),
            "{because}: {}",
            error.message
        );
        assert!(
            page.calls().is_empty(),
            "{because}: nothing ran before the refusal: {:?}",
            page.calls()
        );
    }
}

#[test]
fn a_batch_of_no_steps_is_refused_rather_than_a_delta_of_nothing() {
    let page = form_page();

    let error = run(&page, json!([])).expect_err("nothing to run");

    assert!(error.message.contains("from 1 to"), "{}", error.message);
    assert_eq!(page.called("Accessibility.getFullAXTree"), 0);
}

#[test]
fn ten_steps_run_and_the_eleventh_does_not() {
    let one = json!({ "command": "click", "ref": "e15" });
    let mut many = vec![one.clone(); MAX_STEPS + 1];

    let page = form_page();
    let error = run(&page, json!(many.clone())).expect_err("eleven is one too many");
    assert!(error.message.contains("11"), "{}", error.message);

    many.truncate(MAX_STEPS);
    let page = form_page();
    let answered = run(&page, json!(many)).expect("ten is the contract's cap");
    assert_eq!(steps(&answered).len(), MAX_STEPS);
}

/// A step that runs out of time has already changed the page, and the batch
/// still answers with what it did: a caller that gets a bare error cannot tell
/// an action that never happened from one that did.
#[test]
fn a_batch_that_runs_out_of_time_still_answers_with_the_steps_that_ran() {
    // The typing is where the budget runs out, and the page never settles
    // after it. Said by call order, so the test does not depend on how fast
    // the machine reaches the typing.
    let page = form_page()
        .refusing("Input.insertText", CdpError::OutOfTime)
        .refusing_after(
            "Input.insertText",
            "Accessibility.getFullAXTree",
            CdpError::OutOfTime,
        );
    let asked = json!([
        { "command": "fill", "ref": "e13", "text": "person@example.test" },
        { "command": "click", "ref": "e15" },
        { "command": "click", "ref": "e15" },
    ]);

    let answered =
        run(&page, asked).expect("the batch answers even when the page runs out of time");

    let ran = steps(&answered);
    assert_eq!(ran[0]["ok"], false, "the fill ran out of time: {answered}");
    assert_eq!(
        ran[0]["error"]["code"], "browser_timeout",
        "a budget that ran out is a timeout, not a page refusal: {answered}"
    );
    assert_eq!(
        ran.len(),
        1,
        "and the batch stopped at the step that timed out"
    );
    assert_eq!(answered["settled"], false, "the page never settled");
    assert!(
        answered["delta"].is_null(),
        "and no delta could be computed: {answered}"
    );
    assert_eq!(
        page.called("Input.insertText"),
        1,
        "what did happen is what the report says"
    );
}

/// The timeout a step carries is the protocol's own code, not a host refusal:
/// the page had no say in it.
#[test]
fn a_step_that_times_out_says_so_in_the_protocols_own_words() {
    let page = FakePage::new()
        .answering("Accessibility.getFullAXTree", ax_fixture())
        .refusing("DOM.getBoxModel", CdpError::OutOfTime);

    let answered = run(&page, json!([{ "command": "click", "ref": "e15" }])).expect("answered");

    assert_eq!(
        steps(&answered)[0]["error"]["code"],
        "browser_timeout",
        "{answered}"
    );
}

/// Every step's arguments are checked before the first one runs: a batch that
/// cannot run whole is refused whole, rather than doing its first step and
/// failing on the second.
#[test]
fn a_batch_is_refused_whole_when_a_later_steps_arguments_are_wrong() {
    for (asked, because) in [
        (
            json!([
                { "command": "click", "ref": "e15" },
                { "command": "fill", "ref": "e13" },
            ]),
            "fill needs a text",
        ),
        (
            json!([
                { "command": "fill", "ref": "e13", "text": "person@example.test" },
                { "command": "click" },
            ]),
            "click needs a ref",
        ),
        (
            json!([
                { "command": "navigate", "url": "https://example.test/" },
                { "command": "wait_for", "timeoutMs": 1 },
            ]),
            "wait_for needs something to wait for",
        ),
    ] {
        let page = form_page();

        let error = run(&page, asked.clone()).expect_err(because);

        assert_eq!(error.code, BrowserErrorCode::HostError, "{because}");
        assert!(
            page.calls().is_empty(),
            "{because}: the first step ran anyway: {:?}",
            page.calls()
        );
    }
}
