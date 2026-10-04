//! What a snapshot continues from and is scoped to, and what a find reads off
//! the page before it answers. The page is a table of canned answers.

use super::super::*;
use crate::browser::test_support::{ax_fixture, ax_node, parked_tab, FakePage};
use devboule_protocol::BrowserErrorCode;
use serde_json::{json, Value};

fn run(page: &FakePage, command: &str, args: Value) -> Result<Value, BrowserError> {
    let tab = parked_tab("tab-1");
    let mut args = args;
    args["browserId"] = json!("tab-1");
    tauri::async_runtime::block_on(on_tab(
        &tab,
        page,
        command,
        &args,
        Deadline::in_(std::time::Duration::from_secs(10)),
    ))
}

fn form_page() -> FakePage {
    FakePage::new().answering("Accessibility.getFullAXTree", ax_fixture())
}

/// `count` buttons named alike, in one page: more lines than one view carries.
fn many_buttons(count: u64) -> Value {
    let ids: Vec<String> = (1..=count).map(|id| id.to_string()).collect();
    let children: Vec<&str> = ids.iter().map(String::as_str).collect();
    let mut nodes = vec![ax_node("0", 900, "RootWebArea", "", &children)];
    nodes.extend((1..=count).map(|id| ax_node(&id.to_string(), id, "button", "Save", &[])));
    json!({ "nodes": nodes })
}

#[test]
fn following_a_cursor_loses_no_line_and_repeats_none() {
    let page = FakePage::new().answering("Accessibility.getFullAXTree", many_buttons(1_000));

    let mut seen: Vec<String> = Vec::new();
    let mut cursor: Option<String> = None;
    let mut reads = 0;
    loop {
        let mut args = json!({});
        if let Some(cursor) = &cursor {
            args["cursor"] = json!(cursor);
        }
        let answered = run(&page, "snapshot", args).expect("answered");
        reads += 1;
        seen.extend(
            answered["view"]
                .as_str()
                .expect("a view is text")
                .lines()
                .map(str::to_owned),
        );
        match answered["cursor"].as_str() {
            Some(next) => cursor = Some(next.to_owned()),
            None => break,
        }
        assert!(reads < 20, "a cursor that never ends");
    }

    assert!(reads > 1, "the page is longer than one view");
    let expected: Vec<String> = (1..=1_000)
        .map(|id| format!(r#"- button "Save" [ref=e{id}]"#))
        .collect();
    assert_eq!(seen, expected, "every button once, in order");
}

#[test]
fn a_continuation_starts_at_the_node_the_last_read_stopped_before() {
    let page = FakePage::new().answering("Accessibility.getFullAXTree", many_buttons(1_000));

    let first = run(&page, "snapshot", json!({})).expect("answered");
    let cursor = first["cursor"].as_str().expect("the page does not fit");
    let second = run(&page, "snapshot", json!({ "cursor": cursor })).expect("answered");

    let opening = second["view"]
        .as_str()
        .and_then(|view| view.lines().next())
        .expect("a line");
    assert!(
        opening.ends_with(&format!("[ref={cursor}]")),
        "the cursor names the first line of the next read: {opening} / {cursor}"
    );
}

#[test]
fn a_ref_from_the_full_view_is_a_live_scope_in_the_interactive_one() {
    let page = form_page();

    // The root web area is in the full view and is no line of the interactive
    // one, and the scope is still a live node of the page.
    let scoped = run(&page, "snapshot", json!({ "scope": "e10" })).expect("a live node");
    let whole = run(&page, "snapshot", json!({})).expect("answered");
    assert_eq!(scoped["view"], whole["view"]);

    // A paragraph holds nothing interactive: a scope with an empty view, and not
    // an error.
    let paragraph = run(&page, "snapshot", json!({ "scope": "e16" })).expect("a live node");
    assert_eq!(paragraph["view"], "");
}

#[test]
fn a_scope_on_an_unnamed_node_still_holds_what_is_under_it() {
    let page = form_page();

    let scoped =
        run(&page, "snapshot", json!({ "scope": "e10", "mode": "full" })).expect("answered");

    let view = scoped["view"].as_str().expect("a view is text");
    assert!(view.contains("RootWebArea"), "{view}");
    assert!(view.contains(r#"button "Sign in""#), "{view}");
}

#[test]
fn a_scope_the_page_does_not_have_is_a_dead_ref() {
    let error = run(&form_page(), "snapshot", json!({ "scope": "e999" }))
        .expect_err("there is no such node");

    assert_eq!(error.code, BrowserErrorCode::HostError);
    assert!(error.message.starts_with("stale_ref:"), "{}", error.message);
}

/// A page with one field that has no accessible name and one that has.
fn two_fields() -> FakePage {
    let tree = json!({ "nodes": [
        ax_node("0", 900, "RootWebArea", "", &["1", "2"]),
        ax_node("1", 41, "combobox", "", &[]),
        ax_node("2", 42, "textbox", "Email", &[]),
    ]});
    FakePage::new()
        .answering("Accessibility.getFullAXTree", tree)
        .answering(
            "DOM.describeNode",
            json!({ "node": { "nodeName": "INPUT",
                "attributes": ["class", "wide", "type", "search", "name", "q", "id", "box"] } }),
        )
}

#[test]
fn a_question_about_a_field_reads_the_markup_of_the_pages_fields() {
    let page = two_fields();

    let answered = run(&page, "find", json!({ "query": "search box" })).expect("answered");

    let matches = answered["matches"].as_array().expect("a list");
    assert_eq!(matches[0]["ref"], "e41");
    assert_eq!(matches[0]["name"], "");
    assert_eq!(
        page.called("DOM.describeNode"),
        2,
        "one read per field, and nothing else on the page"
    );
    assert_eq!(
        page.first_params("DOM.describeNode").expect("asked")["backendNodeId"],
        41
    );
}

#[test]
fn a_question_that_is_not_about_a_field_reads_no_markup() {
    let page = two_fields();

    run(&page, "find", json!({ "query": "email" })).expect("answered");
    run(&page, "find", json!({ "query": "save button" })).expect("answered");

    assert_eq!(page.called("DOM.describeNode"), 0);
}

#[test]
fn a_field_whose_markup_cannot_be_read_is_still_answered() {
    let page = FakePage::new()
        .answering(
            "Accessibility.getFullAXTree",
            json!({ "nodes": [
                ax_node("0", 900, "RootWebArea", "", &["1"]),
                ax_node("1", 41, "searchbox", "Search", &[]),
            ]}),
        )
        .refusing("DOM.describeNode", crate::browser::cdp::CdpError::StaleRef);

    let answered = run(&page, "find", json!({ "query": "search box" })).expect("answered");

    assert_eq!(answered["matches"][0]["ref"], "e41");
}

#[test]
fn only_the_first_twenty_fields_are_read() {
    let ids: Vec<String> = (1..=30).map(|id| id.to_string()).collect();
    let children: Vec<&str> = ids.iter().map(String::as_str).collect();
    let mut nodes = vec![ax_node("0", 900, "RootWebArea", "", &children)];
    nodes.extend((1..=30).map(|id| ax_node(&id.to_string(), id, "textbox", "Field", &[])));
    let page = FakePage::new().answering("Accessibility.getFullAXTree", json!({ "nodes": nodes }));

    run(&page, "find", json!({ "query": "text field" })).expect("answered");

    assert_eq!(page.called("DOM.describeNode"), 20);
}
