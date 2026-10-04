//! `read_text`: the page's own words, capped, continued and scoped.

use crate::browser::commands::{on_tab, BrowserError, Deadline};
use crate::browser::test_support::{ax_fixture, parked_tab, FakePage};
use crate::browser::view::VIEW_BUDGET;
use serde_json::json;

/// The answer `Runtime.callFunctionOn` gives when the page-side function
/// returned text: the same shape the spike recorded from this runtime for
/// `Runtime.callFunctionOn` on a resolved node.
fn said(text: &str) -> serde_json::Value {
    json!({ "result": { "type": "string", "value": text } })
}

fn page_answering(text: &str) -> FakePage {
    FakePage::new()
        .answering("Accessibility.getFullAXTree", ax_fixture())
        .answering("DOM.resolveNode", json!({ "object": { "objectId": "9" } }))
        .answering("Runtime.callFunctionOn", said(text))
        .answering("Runtime.evaluate", said(text))
}

fn read(page: &FakePage, args: serde_json::Value) -> Result<serde_json::Value, BrowserError> {
    let tab = parked_tab("tab-1");
    tauri::async_runtime::block_on(on_tab(
        &tab,
        page,
        "read_text",
        &args,
        Deadline::in_(std::time::Duration::from_secs(10)),
    ))
}

#[test]
fn the_page_is_read_by_one_function_where_it_stands() {
    let page = page_answering("# Quarterly report\nTotals\n- three links");

    let answered = read(&page, json!({ "browserId": "tab-1" })).expect("answered");

    assert_eq!(answered["url"], "https://example.test/sign-in");
    assert_eq!(answered["title"], "Sign in");
    assert_eq!(
        answered["text"],
        "# Quarterly report\nTotals\n- three links"
    );
    assert_eq!(answered["truncated"], false);
    assert_eq!(answered["cursor"], serde_json::Value::Null);

    let evaluated = page
        .calls()
        .into_iter()
        .find(|(method, _)| method == "Runtime.evaluate")
        .expect("the whole document is read where the page stands");
    assert!(
        evaluated.1["expression"]
            .as_str()
            .unwrap_or_default()
            .starts_with("(function (budget)"),
        "the one function this app wrote, called on the body"
    );
    assert_eq!(
        page.called("Accessibility.getFullAXTree"),
        0,
        "the tree is not read"
    );
    assert!(
        !evaluated.1["expression"]
            .as_str()
            .unwrap_or_default()
            .contains("outerHTML"),
        "no markup comes back into this app: the page's own words, as text"
    );
}

#[test]
fn a_scope_is_read_on_that_node_rather_than_on_the_document() {
    let page = page_answering("- one row");

    let answered = read(&page, json!({ "browserId": "tab-1", "scope": "e19" })).expect("answered");

    assert_eq!(answered["text"], "- one row");
    assert_eq!(
        page.last_params("DOM.resolveNode").expect("resolved")["backendNodeId"],
        19
    );
    assert_eq!(
        page.last_params("Runtime.callFunctionOn").expect("ran")["objectId"],
        "9",
        "on the object that node resolved to"
    );
    assert_eq!(
        page.called("Runtime.evaluate"),
        0,
        "the document is not read too"
    );
}

#[test]
fn a_scope_the_page_has_is_a_dead_ref_and_not_the_whole_page() {
    let page = FakePage::new()
        .answering("Accessibility.getFullAXTree", ax_fixture())
        .refusing("DOM.resolveNode", crate::browser::cdp::CdpError::StaleRef);

    let error = read(&page, json!({ "browserId": "tab-1", "scope": "e404" }))
        .expect_err("the page never had it");

    assert!(error.message.starts_with("stale_ref:"), "{}", error.message);
    assert_eq!(page.called("Runtime.callFunctionOn"), 0);
}

#[test]
fn text_longer_than_an_answer_is_cut_and_the_cursor_continues_it() {
    // Tokens that never repeat, so a continuation that started at the top
    // again would be visibly the same text twice.
    let whole: String = (0..6_000).map(|at| format!("<{at}> ")).collect();
    let page = page_answering(&whole);

    let first = read(&page, json!({ "browserId": "tab-1" })).expect("answered");

    let sent = first["text"].as_str().expect("text");
    assert_eq!(sent.chars().count(), VIEW_BUDGET);
    assert_eq!(first["truncated"], true);
    assert_eq!(first["cursor"], VIEW_BUDGET.to_string());

    let cursor = first["cursor"].as_str().expect("a cursor").to_owned();
    let second = read(&page, json!({ "browserId": "tab-1", "cursor": cursor })).expect("answered");
    let rest = whole.chars().skip(VIEW_BUDGET).collect::<String>();
    let expected: String = rest.chars().take(VIEW_BUDGET).collect();
    assert_eq!(second["text"], expected);
    assert_ne!(
        second["text"], first["text"],
        "and the second call is not the first one again"
    );
}

#[test]
fn a_cursor_that_is_not_a_position_is_refused() {
    let page = page_answering("some words");

    let error =
        read(&page, json!({ "browserId": "tab-1", "cursor": "next" })).expect_err("not a position");

    assert!(
        error.message.contains("not a position"),
        "{}",
        error.message
    );
    assert_eq!(page.called("Runtime.evaluate"), 0);
}
