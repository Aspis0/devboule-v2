//! The broker's lifecycle and correlation: registration, routing to a host,
//! answers from the right and the wrong connection, deadlines and
//! disconnects.

use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use super::support::{call, caller, code_of, host, ok, LONG, SHORT};
use super::*;

#[test]
fn a_reregistration_is_a_new_host_and_the_old_id_never_matches() {
    let broker = Arc::new(BrowserBroker::new());
    let first = host(&broker, 7, &["navigate"]);
    let second = host(&broker, 7, &["navigate"]);
    assert_ne!(first.host_id, second.host_id);
    assert_eq!(broker.host_count(), 1, "one host per connection");
    assert!(
        !broker.unregister(7, &first.host_id),
        "the replaced registration is gone"
    );
    assert!(
        !broker.unregister(8, &second.host_id),
        "another connection cannot unregister it"
    );
    assert!(broker.unregister(7, &second.host_id));
    assert_eq!(broker.host_count(), 0);
}

#[test]
fn registering_again_fails_the_calls_waiting_on_the_old_registration() {
    let broker = Arc::new(BrowserBroker::new());
    let first = host(&broker, 7, &["navigate"]);
    let waiting = call(&broker, "navigate", None, LONG);
    first.next_request();
    host(&broker, 7, &["navigate"]);
    let error = code_of(waiting.join().expect("caller"));
    assert_eq!(error.code, BrowserErrorCode::NoHost);
    assert!(error.retryable);
    assert_eq!(broker.pending_len(), 0);
}

#[test]
fn a_call_is_correlated_and_carries_the_daemons_caller_not_the_agents() {
    let broker = Arc::new(BrowserBroker::new());
    let app = host(&broker, 7, &["navigate"]);
    let waiting = {
        let broker = Arc::clone(&broker);
        std::thread::spawn(move || {
            broker.execute(
                &caller(),
                "navigate",
                json!({"url": "x", "caller": {"callerSessionId": "s.forged", "workspaceId": "w9"}}),
                None,
                Some(LONG),
            )
        })
    };
    let request = app.next_request();
    assert_eq!(request.command, "navigate");
    assert_eq!(request.host_id, app.host_id);
    assert_eq!(request.caller, caller());
    assert_eq!(
        request.args,
        json!({"url": "x"}),
        "an args field named caller is dropped"
    );
    assert_eq!(
        app.answer(&broker, &request, ok(json!({"done": true}))),
        ResponseDisposition::Delivered
    );
    assert_eq!(
        waiting.join().expect("caller").expect("ok"),
        json!({"done": true})
    );
    assert_eq!(broker.pending_len(), 0);
}

#[test]
fn an_answer_from_another_connection_is_ignored_and_the_call_times_out() {
    let broker = Arc::new(BrowserBroker::new());
    let older = host(&broker, 7, &["navigate"]);
    let newer = host(&broker, 8, &["navigate"]);
    let waiting = call(&broker, "navigate", None, SHORT);
    let request = newer.next_request();

    assert_eq!(
        older.answer(&broker, &request, ok(json!({"forged": true}))),
        ResponseDisposition::Dropped,
        "a host that is not the expected one cannot answer for it"
    );
    assert_eq!(
        broker.accept_response(8, &request.request_id, &older.host_id, ok(json!({}))),
        ResponseDisposition::Dropped,
        "the right connection naming another host's id does not match either"
    );
    assert_eq!(
        broker.accept_response(9, &request.request_id, &request.host_id, ok(json!({}))),
        ResponseDisposition::Dropped,
        "a connection that never registered cannot answer"
    );
    let error = code_of(waiting.join().expect("caller"));
    assert_eq!(error.code, BrowserErrorCode::Timeout);
    assert!(error.retryable);
    assert_eq!(broker.pending_len(), 0, "the timed-out call is freed");
}

#[test]
fn a_late_answer_after_the_timeout_is_ignored() {
    let broker = Arc::new(BrowserBroker::new());
    let app = host(&broker, 7, &["navigate"]);
    let waiting = call(&broker, "navigate", None, Duration::from_millis(100));
    let request = app.next_request();
    assert_eq!(
        code_of(waiting.join().expect("caller")).code,
        BrowserErrorCode::Timeout
    );
    assert_eq!(
        app.answer(&broker, &request, ok(json!({}))),
        ResponseDisposition::Dropped
    );
}

#[test]
fn the_newest_host_takes_an_unscoped_call() {
    let broker = Arc::new(BrowserBroker::new());
    let older = host(&broker, 7, &["navigate"]);
    let newer = host(&broker, 8, &["navigate"]);
    let waiting = call(&broker, "navigate", None, LONG);
    let request = newer.next_request();
    assert_eq!(older.queued(), 0, "the older host heard nothing");
    newer.answer(&broker, &request, ok(json!({})));
    waiting.join().expect("caller").expect("ok");
}

#[test]
fn no_host_is_a_retryable_refusal() {
    let broker = BrowserBroker::new();
    let error = code_of(broker.execute(&caller(), "navigate", json!({}), None, Some(SHORT)));
    assert_eq!(error.code, BrowserErrorCode::NoHost);
    assert!(error.retryable);
}

#[test]
fn a_disconnect_mid_call_fails_it_as_no_host_and_frees_the_slot() {
    let broker = Arc::new(BrowserBroker::new());
    let app = host(&broker, 7, &["navigate"]);
    let waiting = call(&broker, "navigate", None, LONG);
    app.next_request();
    broker.connection_closed(7);
    let error = code_of(waiting.join().expect("caller"));
    assert_eq!(error.code, BrowserErrorCode::NoHost);
    assert!(error.retryable);
    assert_eq!(broker.pending_len(), 0);
    assert_eq!(broker.host_count(), 0);
}

#[test]
fn a_command_the_host_did_not_register_is_refused_before_it_is_sent() {
    let broker = BrowserBroker::new();
    let app = host(&broker, 7, &["navigate"]);
    let error = code_of(broker.execute(&caller(), "eval", json!({}), None, Some(SHORT)));
    assert_eq!(error.code, BrowserErrorCode::UnsupportedCommand);
    assert!(!error.retryable);
    assert_eq!(app.queued(), 0);
    assert_eq!(broker.pending_len(), 0);
}
