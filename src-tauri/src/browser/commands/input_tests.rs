//! What the commands that put input into a page answer with, and what they
//! say to the page. The page is a table of canned answers, so a shape that
//! changes without the contract changing fails here.

use super::super::*;
use crate::browser::test_support::{ax_fixture, box_model, function_answer, parked_tab, FakePage};
use devboule_protocol::BrowserErrorCode;
use serde_json::json;

fn args(pairs: Value) -> Value {
    let mut args = json!({ "browserId": "tab-1" });
    let object = args.as_object_mut().expect("args are an object");
    for (key, value) in pairs.as_object().expect("pairs are an object") {
        object.insert(key.clone(), value.clone());
    }
    args
}

/// A page that knows the sign-in form and where the controls are.
fn form_page() -> FakePage {
    FakePage::new()
        .answering("Accessibility.getFullAXTree", ax_fixture())
        .answering("DOM.getBoxModel", box_model())
        .answering("DOM.resolveNode", json!({ "object": { "objectId": "7" } }))
        .answering("Runtime.callFunctionOn", function_answer(json!("done")))
}

fn run(page: &FakePage, command: &str, args: Value) -> Result<Value, BrowserError> {
    let tab = parked_tab("tab-1");
    tauri::async_runtime::block_on(super::super::on_tab(
        &tab,
        page,
        command,
        &args,
        Deadline::in_(std::time::Duration::from_secs(10)),
    ))
}

#[test]
fn a_click_scrolls_the_node_into_view_and_dispatches_two_real_events() {
    let page = form_page();

    run(&page, "click", args(json!({ "ref": "e15" }))).expect("answered");

    assert_eq!(page.called("DOM.scrollIntoViewIfNeeded"), 1);
    assert_eq!(
        page.last_params("DOM.scrollIntoViewIfNeeded")
            .expect("scrolled")["backendNodeId"],
        15
    );
    let pressed = page
        .last_params("Input.dispatchMouseEvent")
        .expect("a click");
    assert_eq!(pressed["type"], "mouseReleased");
    // The fixture's quad is (30,15) (92,15) (92,38) (30,38), so its middle is
    // (61, 26.5) — a point inside the link, not on its border.
    assert_eq!(pressed["x"], 61.0, "the middle of the box, not its edge");
    assert_eq!(pressed["y"], 26.5);
    assert_eq!(pressed["clickCount"], 1);
}

#[test]
fn a_click_puts_a_parked_page_on_screen_before_it_measures_it() {
    let page = form_page();

    run(&page, "click", args(json!({ "ref": "e15" }))).expect("answered");

    let override_call = page
        .calls()
        .into_iter()
        .position(|(method, _)| method == "Emulation.setDeviceMetricsOverride")
        .expect("a parked page is measured on screen");
    let box_call = page
        .calls()
        .into_iter()
        .position(|(method, _)| method == "DOM.getBoxModel")
        .expect("the node was measured");
    assert!(
        override_call < box_call,
        "the override re-lays the page out, so it has to come first"
    );
    assert_eq!(
        page.last_params("Emulation.setDeviceMetricsOverride")
            .expect("sent")["width"]
            .as_f64(),
        Some(1280.0),
        "the fixture tab has never been presented, so the default is what it \
         is measured at"
    );
}

#[test]
fn every_action_answers_with_a_delta_and_never_with_a_whole_view() {
    for (command, pairs) in [
        ("click", json!({ "ref": "e15" })),
        ("hover", json!({ "ref": "e15" })),
        (
            "fill",
            json!({ "ref": "e13", "text": "other@example.test" }),
        ),
        ("type", json!({ "text": "hello" })),
        ("press", json!({ "key": "Enter" })),
        ("select", json!({ "ref": "e13", "value": "on" })),
        ("check", json!({ "ref": "e14", "checked": true })),
        ("scroll", json!({ "direction": "down" })),
    ] {
        let answered = run(&form_page(), command, args(pairs)).expect(command);
        let delta = answered.get("delta").unwrap_or_else(|| panic!("{command}"));
        assert!(delta.get("url").is_some(), "{command} answers a delta");
        // The actions that put input into a field send only what is worth
        // reading, so an empty list is not there to be read past.
        let into_a_field = matches!(command, "fill" | "type" | "press" | "select" | "check");
        assert_eq!(
            delta.get("added").is_some(),
            !into_a_field,
            "{command} lists what changed, or says only what matters"
        );
        assert!(answered.get("view").is_none(), "{command} sends no view");
    }
}

#[test]
fn a_fill_clears_through_the_page_and_types_as_input() {
    let page = form_page();

    run(
        &page,
        "fill",
        args(json!({ "ref": "e13", "text": "other@example.test" })),
    )
    .expect("answered");

    assert_eq!(
        page.last_params("DOM.focus").expect("focused")["backendNodeId"],
        13
    );
    let cleared = page
        .last_params("Runtime.callFunctionOn")
        .expect("the clear runs on the page");
    assert!(cleared["functionDeclaration"]
        .as_str()
        .expect("a function")
        .contains("setter.call(this, \"\")"));
    assert_eq!(
        page.last_params("Input.insertText")
            .expect("the text is typed")["text"],
        "other@example.test"
    );
}

#[test]
fn a_select_picks_the_option_and_fires_what_a_choice_fires() {
    let page = form_page();

    run(
        &page,
        "select",
        args(json!({ "ref": "e13", "label": "Second" })),
    )
    .expect("answered");

    let chosen = page
        .last_params("Runtime.callFunctionOn")
        .expect("the choice runs on the page");
    assert_eq!(chosen["arguments"][0]["label"], "Second");
    assert!(chosen["functionDeclaration"]
        .as_str()
        .expect("a function")
        .contains("dispatchEvent"));
}

#[test]
fn a_select_with_no_such_option_is_a_refusal_and_not_an_empty_answer() {
    let page = FakePage::new()
        .answering("Accessibility.getFullAXTree", ax_fixture())
        .answering("DOM.resolveNode", json!({ "object": { "objectId": "7" } }))
        .answering(
            "Runtime.callFunctionOn",
            json!({ "result": { "value": null } }),
        );

    let error = run(
        &page,
        "select",
        args(json!({ "ref": "e13", "value": "zzz" })),
    )
    .expect_err("an option that is not there is a failure");
    assert_eq!(error.code, BrowserErrorCode::HostError);
    assert!(
        error.message.contains("no such option"),
        "{}",
        error.message
    );
}

#[test]
fn a_check_only_touches_the_control_when_the_state_is_not_what_was_asked_for() {
    let page = form_page();
    // The fixture's checkbox is unchecked.
    run(
        &page,
        "check",
        args(json!({ "ref": "e14", "checked": false })),
    )
    .expect("answered");
    assert_eq!(
        page.called("Input.dispatchMouseEvent"),
        0,
        "clicking a checked box to ask for checked unchecks it"
    );

    let other = form_page();
    run(
        &other,
        "check",
        args(json!({ "ref": "e14", "checked": true })),
    )
    .expect("answered");
    assert_eq!(
        other.called("Input.dispatchMouseEvent"),
        2,
        "press and release"
    );
}

#[test]
fn a_scroll_by_direction_sends_a_wheel_and_a_scroll_by_ref_scrolls_the_node() {
    let page = form_page();
    run(
        &page,
        "scroll",
        args(json!({ "direction": "down", "amount": 2 })),
    )
    .expect("answered");
    let wheel = page
        .last_params("Input.dispatchMouseEvent")
        .expect("a wheel");
    assert_eq!(wheel["type"], "mouseWheel");
    assert_eq!(wheel["deltaY"], 240.0, "two notches");

    let other = form_page();
    run(&other, "scroll", args(json!({ "ref": "e15" }))).expect("answered");
    assert_eq!(other.called("DOM.scrollIntoViewIfNeeded"), 1);
    assert_eq!(other.called("Input.dispatchMouseEvent"), 0, "not a wheel");
}

#[test]
fn a_dead_ref_says_stale_ref_and_names_the_snap_shot_to_take() {
    let page = FakePage::new()
        .answering("Accessibility.getFullAXTree", ax_fixture())
        .refusing("DOM.scrollIntoViewIfNeeded", CdpError::StaleRef);

    let error = run(&page, "click", args(json!({ "ref": "e15" }))).expect_err("the node is gone");
    assert_eq!(error.code, BrowserErrorCode::HostError);
    assert!(
        error.message.starts_with("stale_ref:"),
        "the caller has one move only, and it is named: {}",
        error.message
    );
}

#[test]
fn a_ref_that_is_not_a_ref_is_refused_before_the_page_is_touched() {
    let page = form_page();

    let error =
        run(&page, "click", args(json!({ "ref": "Sign in" }))).expect_err("a name is not a ref");
    assert!(error.message.starts_with("stale_ref:"), "{}", error.message);
    assert_eq!(
        page.called("DOM.scrollIntoViewIfNeeded"),
        0,
        "a ref that never existed is refused without a round trip"
    );
}

#[test]
fn a_script_that_threw_on_the_node_is_a_failure_and_the_text_is_not_typed() {
    // A function that throws is a successful protocol answer with the failure
    // beside an absent value.
    let page = FakePage::new()
        .answering("Accessibility.getFullAXTree", ax_fixture())
        .answering("DOM.getBoxModel", box_model())
        .answering("DOM.resolveNode", json!({ "object": { "objectId": "7" } }))
        .answering(
            "Runtime.callFunctionOn",
            json!({
                "result": { "type": "object", "subtype": "error" },
                "exceptionDetails": {
                    "exceptionId": 1,
                    "text": "Uncaught",
                    "lineNumber": 3,
                    "columnNumber": 10,
                    "exception": { "type": "object", "description": "TypeError: no value setter" }
                }
            }),
        );

    let error = run(
        &page,
        "fill",
        args(json!({ "ref": "e13", "text": "hello" })),
    )
    .expect_err("the clear threw");

    assert_eq!(error.code, BrowserErrorCode::HostError);
    assert!(
        error.message.contains("TypeError: no value setter"),
        "{}",
        error.message
    );
    assert_eq!(
        page.called("Input.insertText"),
        0,
        "typing into a field the clear never emptied would append to it"
    );
}
