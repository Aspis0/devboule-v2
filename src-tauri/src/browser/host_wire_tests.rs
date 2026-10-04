//! The host loop over a real connection: register, answer a command that
//! arrives after an idle, answer a command it cannot run, and do it all again
//! on the next connection.
//!
//! It fails the way the live agent's calls did if the loop ever stops reading:
//! the answer never arrives, and the daemon's own reason for that refusal
//! (`browser_busy`, from a queue with no reader) is what the agent saw.

#![cfg(windows)]

mod wire_daemon;

use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;

use devboule_protocol::{BrowserError, BrowserErrorCode, BrowserExecuteRequest, BrowserOutcome};

use crate::browser::host::{serve_connection, Stop};
use crate::browser::registry::BrowserRegistry;
use crate::browser::test_support::registry_with;
use wire_daemon::{
    list_tabs, next_answer, request, request_without_a_workspace, WireDaemon, ANSWER, HOST_ID, IDLE,
};

/// Register on the daemon's newest connection and run the production loop on
/// it, answering `list_tabs` from `registry`.
fn host_on(daemon: &WireDaemon, registry: Arc<BrowserRegistry>) -> (String, Receiver<Stop>) {
    let client = Arc::new(daemon.client());
    let host_id = client
        .browser_host_register(&["list_tabs".to_owned()])
        .expect("the host registers");
    let (stop, stops) = channel();
    std::thread::Builder::new()
        .name("browser-host".into())
        .spawn(move || {
            let answer = move |request: &BrowserExecuteRequest| list_tabs(&registry, request);
            let _ = stop.send(serve_connection(&client, &answer));
        })
        .expect("host thread");
    (host_id, stops)
}

#[test]
fn a_host_answers_a_command_that_arrives_after_an_idle() {
    let daemon = WireDaemon::start("host-idle");
    let (host_id, stops) = host_on(&daemon, Arc::new(registry_with("tab-1")));
    assert_eq!(host_id, HOST_ID);

    // The daemon says nothing for a while, then sends a command. A loop that
    // gave up on an idle would have dropped its request queue, and this is
    // where the live agent's `browser_busy` came from.
    std::thread::sleep(IDLE);
    daemon.push(request("browser-1"));

    let (answered, answer) = next_answer(&daemon);
    assert_eq!(
        answered, "browser-1",
        "one answer per request, and this is it"
    );
    assert_eq!(answer["status"], "ok");
    let tabs = answer["result"]["tabs"].as_array().expect("a list of tabs");
    assert_eq!(tabs.len(), 1, "the registry's own tab: {answer}");
    assert_eq!(tabs[0]["browserId"], "tab-1");

    daemon.close_current();
    assert_eq!(stops.recv_timeout(ANSWER).ok(), Some(Stop::ConnectionEnded));
}

#[test]
fn a_host_that_lost_its_connection_stops_and_a_new_connection_serves_again() {
    let daemon = WireDaemon::start("host-reconnect");
    let registry = Arc::new(registry_with("tab-1"));

    let (first, first_stops) = host_on(&daemon, Arc::clone(&registry));
    assert_eq!(first, HOST_ID);

    // The connection ends: the client's half of the request queue goes with it,
    // which is the only thing that can end the loop.
    daemon.close_current();
    assert_eq!(
        first_stops.recv_timeout(ANSWER).ok(),
        Some(Stop::ConnectionEnded),
        "and the loop says why it stopped"
    );

    // A new connection registers and is served: the host is not one-shot.
    let (second, second_stops) = host_on(&daemon, Arc::clone(&registry));
    assert_eq!(second, HOST_ID);

    std::thread::sleep(IDLE);
    daemon.push(request("browser-2"));
    let (answered, answer) = next_answer(&daemon);
    assert_eq!(answered, "browser-2");
    assert_eq!(answer["status"], "ok");

    daemon.close_current();
    assert_eq!(
        second_stops.recv_timeout(ANSWER).ok(),
        Some(Stop::ConnectionEnded)
    );
}

/// A refused answer is still an answer, and it is the one the live agent saw.
#[test]
fn a_command_the_host_cannot_answer_is_answered_not_dropped() {
    let daemon = WireDaemon::start("host-refusal");
    let (_host_id, stops) = host_on(&daemon, Arc::new(BrowserRegistry::new()));

    // A caller with no workspace cannot open or list a tab, and the refusal
    // travels back over the wire instead of the command disappearing: the
    // daemon's caller is waiting on exactly one answer per request.
    daemon.push(request_without_a_workspace("browser-1"));
    let (_, answer) = next_answer(&daemon);
    assert_eq!(
        answer["status"], "err",
        "a refused command is still answered: {answer}"
    );
    assert_eq!(answer["code"], "browser_host_error");
    assert!(
        answer["message"]
            .as_str()
            .unwrap_or_default()
            .contains("workspace"),
        "and it says what is missing: {answer}"
    );

    daemon.close_current();
    assert_eq!(stops.recv_timeout(ANSWER).ok(), Some(Stop::ConnectionEnded));
}

/// The refusal a command the host cannot run carries, spelled the way the
/// daemon's own log spells it.
#[test]
fn an_answer_carries_the_code_the_wire_names() {
    let refused = BrowserOutcome::Err(BrowserError::daemon(
        BrowserErrorCode::TabNotFound,
        "No browser tab tab-9 in this workspace.",
    ));
    let json = serde_json::to_value(&refused).expect("the outcome is JSON");
    assert_eq!(json["status"], "err");
    assert_eq!(json["code"], "browser_tab_not_found");
    assert_eq!(
        crate::browser::commands::code_name(BrowserErrorCode::TabNotFound),
        "browser_tab_not_found",
        "the app's one line per command says the same name"
    );
}
