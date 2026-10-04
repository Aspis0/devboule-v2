//! The answer `console_logs` gives: the ring, filtered the way the caller
//! asked. The command touches no page — what the page said is already here.

use super::*;
use crate::browser::console::{self, Wanted};
use serde_json::json;

/// A tab with three entries said against it: a log, an error and a warning.
/// Each caller names its own tab, because these tests run at the same time and
/// one ring is shared by every command that names it.
fn said_against(id: &str) -> String {
    console::open(id);
    console::record(
        id,
        "Runtime.consoleAPICalled",
        &json!({
            "type": "log", "args": [{ "type": "string", "value": "loading" }],
            "timestamp": 10.0
        })
        .to_string(),
    );
    console::record(
        id,
        "Runtime.consoleAPICalled",
        &json!({
            "type": "error", "args": [{ "type": "string", "value": "no route" }],
            "timestamp": 20.0
        })
        .to_string(),
    );
    console::record(
        id,
        "Runtime.consoleAPICalled",
        &json!({
            "type": "warning", "args": [{ "type": "string", "value": "slow route" }],
            "timestamp": 30.0
        })
        .to_string(),
    );
    id.to_owned()
}

#[test]
fn warnings_and_errors_are_the_answer_unless_the_caller_asks_for_more() {
    let id = said_against("tab-logs-default");

    let warned = logs(&id, &json!({ "browserId": id })).expect("answered");
    assert_eq!(
        warned["entries"]
            .as_array()
            .expect("entries")
            .iter()
            .map(|entry| entry["text"].as_str().unwrap_or_default())
            .collect::<Vec<_>>(),
        ["no route", "slow route"],
        "the default is a warning, and an error is one too"
    );
    assert_eq!(warned["dropped"], 0);
    assert_eq!(warned["entries"][0]["level"], "error");
    assert_eq!(warned["entries"][0]["timeMs"], 20.0);

    let everything = logs(&id, &json!({ "browserId": id, "level": "all" })).expect("answered");
    assert_eq!(everything["entries"].as_array().map(Vec::len), Some(3));

    let errors = logs(&id, &json!({ "browserId": id, "level": "error" })).expect("answered");
    assert_eq!(errors["entries"].as_array().map(Vec::len), Some(1));
}

#[test]
fn a_level_this_host_does_not_know_is_refused_by_name() {
    let id = said_against("tab-logs-level");

    let error = logs(&id, &json!({ "browserId": id, "level": "loud" })).expect_err("not a level");

    assert!(
        error.message.contains("loud is not a level"),
        "{}",
        error.message
    );
}

#[test]
fn since_ms_asks_for_the_last_of_the_page_own_clock() {
    let id = said_against("tab-logs-since");

    let recent = logs(
        &id,
        &json!({ "browserId": id, "level": "all", "sinceMs": 5.0 }),
    )
    .expect("answered");

    assert_eq!(
        recent["entries"]
            .as_array()
            .expect("entries")
            .iter()
            .map(|entry| entry["text"].as_str().unwrap_or_default())
            .collect::<Vec<_>>(),
        ["slow route"],
        "the newest of the three is at 30, so 5 ms back is 25"
    );
}

#[test]
fn a_tab_that_said_nothing_answers_with_no_entries() {
    let answered = logs(
        "tab-never-spoke",
        &json!({ "browserId": "tab-never-spoke" }),
    )
    .expect("answered");

    assert_eq!(answered["entries"], json!([]));
    assert_eq!(answered["dropped"], 0);
}

#[test]
fn the_words_of_a_level_are_the_ones_the_contract_names() {
    assert_eq!(Wanted::parse(None), Some(Wanted::Warning));
    assert_eq!(Wanted::parse(Some("error")), Some(Wanted::Error));
    assert_eq!(Wanted::parse(Some("all")), Some(Wanted::All));
    assert_eq!(Wanted::parse(Some("info")), None);
}
