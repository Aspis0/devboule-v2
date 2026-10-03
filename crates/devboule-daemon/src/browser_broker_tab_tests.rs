//! Tab ownership through the broker: what teaches it a tab's owner, what makes
//! it forget one, and that a call for a tab never moves to another host. The
//! map itself is tested in `browser_affinity_tests.rs`.

use std::sync::Arc;

use serde_json::json;

use super::support::{call_as, caller, caller_in, code_of, host, ok, FakeHost, LONG, SHORT};
use super::*;

const COMMANDS: &[&str] = &["new_tab", "navigate", "close_tab", "list_tabs"];

/// Run `command` as `workspace` and answer it from `host` with `result`.
fn run(
    broker: &Arc<BrowserBroker>,
    host: &FakeHost,
    workspace: Option<&str>,
    command: &'static str,
    browser_id: Option<&'static str>,
    result: serde_json::Value,
) -> Result<serde_json::Value, BrowserError> {
    let waiting = call_as(broker, caller_in(workspace), command, browser_id, LONG);
    let request = host.next_request();
    host.answer(broker, &request, ok(result));
    waiting.join().expect("caller")
}

fn navigate_error(broker: &BrowserBroker, workspace: Option<&str>, tab: &str) -> BrowserError {
    code_of(broker.execute(
        &caller_in(workspace),
        "navigate",
        json!({}),
        Some(tab),
        Some(SHORT),
    ))
}

#[test]
fn a_tab_keeps_its_host_and_never_moves_to_another() {
    let broker = Arc::new(BrowserBroker::new());
    let owner = host(&broker, 7, COMMANDS);
    run(
        &broker,
        &owner,
        Some("w1"),
        "new_tab",
        None,
        json!({"browserId": "tab-1"}),
    )
    .expect("opened");

    let newer = host(&broker, 8, COMMANDS);
    run(
        &broker,
        &owner,
        Some("w1"),
        "navigate",
        Some("tab-1"),
        json!({}),
    )
    .expect("navigated");
    assert_eq!(
        newer.queued(),
        0,
        "a registration after the tab changes nothing"
    );

    broker.connection_closed(7);
    let error = navigate_error(&broker, Some("w1"), "tab-1");
    assert_eq!(error.code, BrowserErrorCode::OwnerUnavailable);
    assert!(!error.retryable);
    assert_eq!(newer.queued(), 0, "the call was not rerouted");

    // A tab nobody claimed is not stranded: it goes to the newest host, which
    // answers whether it knows the tab.
    let waiting = call_as(&broker, caller(), "navigate", Some("tab-unseen"), LONG);
    let request = newer.next_request();
    newer.answer(
        &broker,
        &request,
        BrowserOutcome::Err(BrowserError {
            code: BrowserErrorCode::TabNotFound,
            message: "no such tab".to_string(),
            retryable: false,
        }),
    );
    assert_eq!(
        code_of(waiting.join().expect("caller")).code,
        BrowserErrorCode::TabNotFound
    );
}

#[test]
fn only_a_tab_creating_command_teaches_an_owner() {
    let broker = Arc::new(BrowserBroker::new());
    let first = host(&broker, 7, COMMANDS);
    run(
        &broker,
        &first,
        Some("w1"),
        "list_tabs",
        None,
        json!({"browserId": "tab-x"}),
    )
    .expect("answered");
    run(
        &broker,
        &first,
        Some("w1"),
        "navigate",
        Some("tab-y"),
        json!({"browserId": "tab-y"}),
    )
    .expect("answered");

    let second = host(&broker, 8, COMMANDS);
    broker.connection_closed(7);
    // Nothing was learned, so the tabs are unknown and go to the newest host
    // instead of being reported as owned by the host that left.
    for tab in ["tab-x", "tab-y"] {
        let waiting = call_as(&broker, caller(), "navigate", Some(tab), LONG);
        let request = second.next_request();
        second.answer(&broker, &request, ok(json!({})));
        waiting
            .join()
            .expect("caller")
            .expect("routed to the newest host");
    }
}

#[test]
fn two_workspaces_using_one_browser_id_route_independently() {
    let broker = Arc::new(BrowserBroker::new());
    let first = host(&broker, 7, COMMANDS);
    let second = host(&broker, 8, COMMANDS);
    // The second host is the newest, so it opens w2's tab; the first opens
    // w1's by being the only one asked while the second is not yet there.
    run(
        &broker,
        &second,
        Some("w2"),
        "new_tab",
        None,
        json!({"browserId": "tab-1"}),
    )
    .expect("w2 opened");
    broker.connection_closed(8);
    run(
        &broker,
        &first,
        Some("w1"),
        "new_tab",
        None,
        json!({"browserId": "tab-1"}),
    )
    .expect("w1 opened");
    let second = host(&broker, 9, COMMANDS);

    run(
        &broker,
        &first,
        Some("w1"),
        "navigate",
        Some("tab-1"),
        json!({}),
    )
    .expect("w1's tab");
    assert_eq!(second.queued(), 0);
    let stranded = navigate_error(&broker, Some("w2"), "tab-1");
    assert_eq!(
        stranded.code,
        BrowserErrorCode::OwnerUnavailable,
        "w2's tab belonged to the host that left, whatever w1's same id says"
    );
}

#[test]
fn a_conflicting_second_claim_does_not_move_the_route() {
    let broker = Arc::new(BrowserBroker::new());
    let first = host(&broker, 7, COMMANDS);
    run(
        &broker,
        &first,
        Some("w1"),
        "new_tab",
        None,
        json!({"browserId": "tab-1"}),
    )
    .expect("opened");
    let second = host(&broker, 8, COMMANDS);
    run(
        &broker,
        &second,
        Some("w1"),
        "new_tab",
        None,
        json!({"browserId": "tab-1"}),
    )
    .expect("the second host answered, its claim was ignored");

    run(
        &broker,
        &first,
        Some("w1"),
        "navigate",
        Some("tab-1"),
        json!({}),
    )
    .expect("still the first host's");
    assert_eq!(second.queued(), 0);
}

#[test]
fn closing_a_tab_makes_the_daemon_forget_its_owner() {
    let broker = Arc::new(BrowserBroker::new());
    let owner = host(&broker, 7, COMMANDS);
    run(
        &broker,
        &owner,
        Some("w1"),
        "new_tab",
        None,
        json!({"browserId": "tab-1"}),
    )
    .expect("opened");
    run(
        &broker,
        &owner,
        Some("w1"),
        "close_tab",
        Some("tab-1"),
        json!({}),
    )
    .expect("closed");

    // The owner leaves; a forgotten tab is unknown, not stranded.
    let newer = host(&broker, 8, COMMANDS);
    broker.connection_closed(7);
    let waiting = call_as(&broker, caller(), "navigate", Some("tab-1"), LONG);
    let request = newer.next_request();
    newer.answer(&broker, &request, ok(json!({})));
    waiting.join().expect("caller").expect("routed afresh");
}

#[test]
fn a_failed_close_keeps_the_owner() {
    let broker = Arc::new(BrowserBroker::new());
    let owner = host(&broker, 7, COMMANDS);
    run(
        &broker,
        &owner,
        Some("w1"),
        "new_tab",
        None,
        json!({"browserId": "tab-1"}),
    )
    .expect("opened");
    let waiting = call_as(&broker, caller(), "close_tab", Some("tab-1"), LONG);
    let request = owner.next_request();
    owner.answer(
        &broker,
        &request,
        BrowserOutcome::Err(BrowserError::daemon(BrowserErrorCode::HostError, "busy")),
    );
    code_of(waiting.join().expect("caller"));

    broker.connection_closed(7);
    assert_eq!(
        navigate_error(&broker, Some("w1"), "tab-1").code,
        BrowserErrorCode::OwnerUnavailable
    );
}
