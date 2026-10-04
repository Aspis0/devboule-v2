//! The reading commands' answers, and the refusals every command shares: what
//! a tab of another workspace is, what a command this app does not run is, and
//! what the registered list has to reach. The page is a table of canned
//! answers, so a shape that changes without the contract changing fails here.

use super::*;
use crate::browser::test_support::{ax_fixture, box_model, function_answer, parked_tab, FakePage};
use devboule_protocol::{BrowserCaller, BrowserErrorCode};
use serde_json::json;

fn caller() -> BrowserCaller {
    BrowserCaller {
        caller_session_id: "s-1".to_owned(),
        workspace_id: Some("ws-1".to_owned()),
    }
}

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
    tauri::async_runtime::block_on(on_tab(
        &tab,
        page,
        command,
        &args,
        Deadline::in_(TEST_BUDGET),
    ))
}

/// Long enough that no test is about time, short enough that a mistake in the
/// budget cannot hang the suite.
const TEST_BUDGET: std::time::Duration = std::time::Duration::from_secs(10);

fn refused_by(error: BrowserError, code: BrowserErrorCode) {
    assert_eq!(error.code, code, "wrong code: {}", error.message);
}

#[test]
fn a_snapshot_answers_with_the_url_the_title_and_the_lines() {
    let answered = run(&form_page(), "snapshot", args(json!({}))).expect("the page answers");

    assert_eq!(answered["url"], "https://example.test/sign-in");
    assert_eq!(answered["title"], "Sign in");
    assert_eq!(answered["truncated"], false);
    assert_eq!(answered["cursor"], Value::Null);
    let view = answered["view"].as_str().expect("a view is text");
    assert!(view.contains(r#"  - button "Sign in" [ref=e15]"#), "{view}");
    assert!(!view.contains("StaticText"), "the view drops page text");
}

#[test]
fn a_snapshot_can_be_asked_for_the_whole_tree_and_for_one_subtree() {
    let page = form_page();

    let full = run(&page, "snapshot", args(json!({ "mode": "full" }))).expect("answered");
    let full_view = full["view"].as_str().expect("a view is text");
    assert!(
        full_view.contains("RootWebArea"),
        "full keeps what interactive drops"
    );

    let scoped = run(&page, "snapshot", args(json!({ "scope": "e19" }))).expect("answered");
    let scoped_view = scoped["view"].as_str().expect("a view is text");
    assert!(
        scoped_view.starts_with(r#"- form "Sign in" [ref=e19]"#),
        "a scope is the node and what is under it: {scoped_view}"
    );
    assert!(
        scoped_view.contains(r#"  - textbox "Email""#),
        "{scoped_view}"
    );
    assert!(
        !scoped_view.contains("heading"),
        "the heading is not in the form"
    );
}

#[test]
fn a_snapshot_a_cursor_cannot_find_starts_over_rather_than_skipping_the_rest() {
    let page = form_page();

    let whole = run(&page, "snapshot", args(json!({}))).expect("answered");
    let restarted = run(&page, "snapshot", args(json!({ "cursor": "e999" }))).expect("answered");
    assert_eq!(
        restarted["view"], whole["view"],
        "a cursor from a page that has moved is not a position any more, so          the read starts over rather than skipping what is left"
    );
}

#[test]
fn a_find_answers_with_at_most_twenty_refs_and_what_each_one_is() {
    let answered = run(
        &form_page(),
        "find",
        json!({ "browserId": "tab-1", "query": "remember" }),
    )
    .expect("answered");

    let matches = answered["matches"].as_array().expect("matches are a list");
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0]["ref"], "e14");
    assert_eq!(matches[0]["role"], "checkbox");
    assert_eq!(matches[0]["name"], "Remember me");
    assert_eq!(matches[0]["context"], r#"in form "Sign in""#);
}

#[test]
fn a_wait_for_looks_until_the_deadline_and_says_whether_it_was_met() {
    let met = run(
        &form_page(),
        "wait_for",
        json!({ "browserId": "tab-1", "text": "remember", "timeoutMs": 1000 }),
    )
    .expect("answered");
    assert_eq!(met["met"], true);
    assert!(met.get("delta").is_some());

    let timed_out = run(
        &form_page(),
        "wait_for",
        json!({ "browserId": "tab-1", "text": "nothing says this", "timeoutMs": 300 }),
    )
    .expect("answered");
    assert_eq!(timed_out["met"], false, "a wait that runs out says so");
}

#[test]
fn a_wait_for_that_names_nothing_to_wait_for_is_a_refusal() {
    let error = run(&form_page(), "wait_for", json!({ "browserId": "tab-1" }))
        .expect_err("there is nothing to wait for");
    assert_eq!(error.code, BrowserErrorCode::HostError);
    assert!(
        error.message.contains("wait_for needs"),
        "{}",
        error.message
    );
}

#[test]
fn a_navigate_goes_somewhere_and_answers_with_where_it_landed() {
    let page = form_page();

    let answered = run(
        &page,
        "navigate",
        json!({ "browserId": "tab-1", "url": "https://example.test/home" }),
    )
    .expect("answered");

    assert_eq!(
        page.last_params("Page.navigate").expect("it navigated")["url"],
        "https://example.test/home"
    );
    assert_eq!(answered["url"], "https://example.test/sign-in");
    assert!(answered.get("delta").is_some());
}

#[test]
fn a_navigate_to_a_scheme_this_tab_never_loads_is_refused_before_the_page() {
    let page = form_page();

    let error = run(
        &page,
        "navigate",
        json!({ "browserId": "tab-1", "url": "file:///C:/Windows/System32" }),
    )
    .expect_err("a file is not a page here");
    assert_eq!(error.code, BrowserErrorCode::HostError);
    assert_eq!(page.called("Page.navigate"), 0);
}

#[test]
fn a_navigate_that_the_page_refuses_says_what_the_page_said() {
    let page = FakePage::new()
        .answering("Accessibility.getFullAXTree", ax_fixture())
        .answering(
            "Page.navigate",
            json!({ "errorText": "net::ERR_NAME_NOT_RESOLVED" }),
        );

    let error = run(
        &page,
        "navigate",
        json!({ "browserId": "tab-1", "url": "https://nowhere.test" }),
    )
    .expect_err("the page refused");
    assert!(
        error.message.contains("ERR_NAME_NOT_RESOLVED"),
        "{}",
        error.message
    );
}

#[test]
fn a_navigate_step_through_history_uses_the_pages_own_entries() {
    let page = FakePage::new()
        .answering("Accessibility.getFullAXTree", ax_fixture())
        .answering(
            "Page.getNavigationHistory",
            json!({
                "currentIndex": 1,
                "entries": [
                    { "id": 40, "url": "https://example.test/" },
                    { "id": 41, "url": "https://example.test/sign-in" }
                ]
            }),
        );

    run(
        &page,
        "navigate",
        json!({ "browserId": "tab-1", "action": "back" }),
    )
    .expect("answered");
    assert_eq!(
        page.last_params("Page.navigateToHistoryEntry")
            .expect("stepped")["entryId"],
        40
    );
}

#[test]
fn a_step_that_way_with_no_history_that_way_goes_nowhere() {
    let only_entry = FakePage::new()
        .answering("Accessibility.getFullAXTree", ax_fixture())
        .answering(
            "Page.getNavigationHistory",
            json!({ "currentIndex": 0, "entries": [{ "id": 40, "url": "https://example.test/" }] }),
        );

    for action in ["back", "forward"] {
        let error = run(
            &only_entry,
            "navigate",
            json!({ "browserId": "tab-1", "action": action }),
        )
        .expect_err(action);
        assert!(
            error.message.contains("nothing that way"),
            "{action}: {}",
            error.message
        );
    }
    assert_eq!(
        only_entry.called("Page.navigateToHistoryEntry"),
        0,
        "a back from the first entry is not a navigation to the first entry"
    );
}

#[test]
fn a_command_this_app_does_not_run_is_a_refusal_and_not_an_empty_answer() {
    let error = run(&form_page(), "screenshot", args(json!({}))).expect_err("not a 4b-1 command");
    assert_eq!(error.code, BrowserErrorCode::HostError);
    assert!(error.message.contains("screenshot"), "{}", error.message);
}

#[test]
fn a_tab_of_another_workspace_is_exactly_as_unknown_as_a_tab_that_is_not_there() {
    let registry = crate::browser::test_support::registry_with("tab-1");
    let other = BrowserCaller {
        caller_session_id: "s-2".to_owned(),
        workspace_id: Some("ws-2".to_owned()),
    };

    refused_by(
        resolve(&registry, &other, "tab-1").expect_err("another workspace's tab"),
        BrowserErrorCode::TabNotFound,
    );
    refused_by(
        resolve(&registry, &caller(), "tab-404").expect_err("no such tab"),
        BrowserErrorCode::TabNotFound,
    );
    assert!(
        resolve(&registry, &caller(), "tab-1").is_ok(),
        "and its own workspace's tab resolves"
    );
}

#[test]
fn a_command_without_a_browser_id_is_refused_rather_than_guessed_at() {
    let error = browser_id(&json!({ "url": "https://example.test" })).expect_err("no id");
    assert_eq!(error.code, BrowserErrorCode::HostError);
    assert!(error.message.contains("browserId"), "{}", error.message);
}

#[test]
fn the_registered_command_list_is_the_one_this_dispatch_runs() {
    // A name registered and not run would be a command the daemon routes and
    // this app refuses; a name run and not registered would never arrive.
    for command in COMMANDS {
        if matches!(command, "new_tab" | "list_tabs" | "close_tab") {
            continue;
        }
        let answered = run(&form_page(), command, json!({}));
        let message = match answered {
            Ok(_) => String::new(),
            Err(error) => error.message,
        };
        assert_ne!(
            message,
            format!("{command} is not a command this app runs."),
            "{command} is registered, so the dispatch reaches it"
        );
    }
    assert_eq!(COMMANDS.len(), 15, "4b-1 is fifteen commands");
}
