//! The broker's limits: the pending caps, the payload cap on results and
//! arguments, and the bound on a host's error text.

use std::sync::Arc;

use serde_json::json;

use super::support::{call, caller, code_of, host, ok, wait_for_pending, LONG, SHORT};
use super::*;
use crate::browser_registry::{MAX_PENDING_PER_HOST, MAX_PENDING_TOTAL};

#[test]
fn the_seventeenth_call_on_one_host_is_busy_and_nothing_is_queued() {
    let broker = Arc::new(BrowserBroker::new());
    let app = host(&broker, 7, &["navigate"]);
    let waiting: Vec<_> = (0..MAX_PENDING_PER_HOST)
        .map(|_| call(&broker, "navigate", None, LONG))
        .collect();
    wait_for_pending(&broker, MAX_PENDING_PER_HOST);

    let error = code_of(broker.execute(&caller(), "navigate", json!({}), None, Some(SHORT)));
    assert_eq!(error.code, BrowserErrorCode::Busy);
    assert!(error.retryable);
    assert_eq!(broker.pending_len(), MAX_PENDING_PER_HOST);
    assert_eq!(
        app.queued(),
        MAX_PENDING_PER_HOST,
        "the refused call was never sent"
    );

    broker.connection_closed(7);
    for handle in waiting {
        assert_eq!(
            code_of(handle.join().expect("caller")).code,
            BrowserErrorCode::NoHost
        );
    }
}

#[test]
fn the_sixty_fifth_call_overall_is_busy_even_on_an_idle_host() {
    let broker = Arc::new(BrowserBroker::new());
    let mut waiting = Vec::new();
    for conn_id in 1..=4 {
        let app = host(&broker, conn_id, &["navigate"]);
        let first = waiting.len();
        waiting.extend((0..MAX_PENDING_PER_HOST).map(|_| call(&broker, "navigate", None, LONG)));
        wait_for_pending(&broker, first + MAX_PENDING_PER_HOST);
        assert_eq!(app.queued(), MAX_PENDING_PER_HOST);
    }
    assert_eq!(broker.pending_len(), MAX_PENDING_TOTAL);
    let idle = host(&broker, 5, &["navigate"]);

    let error = code_of(broker.execute(&caller(), "navigate", json!({}), None, Some(SHORT)));
    assert_eq!(error.code, BrowserErrorCode::Busy);
    assert!(error.retryable);
    assert_eq!(broker.pending_len(), MAX_PENDING_TOTAL);
    assert_eq!(idle.queued(), 0, "nothing reached the idle host");

    for conn_id in 1..=5 {
        broker.connection_closed(conn_id);
    }
    for handle in waiting {
        assert_eq!(
            code_of(handle.join().expect("caller")).code,
            BrowserErrorCode::NoHost
        );
    }
}

#[test]
fn an_oversized_result_fails_the_call_and_the_host_is_told_why() {
    let broker = Arc::new(BrowserBroker::new());
    let app = host(&broker, 7, &["screenshot"]);
    let waiting = call(&broker, "screenshot", None, LONG);
    let request = app.next_request();
    let huge = json!({ "png": "x".repeat(MAX_BROWSER_PAYLOAD_BYTES) });
    let disposition = app.answer(&broker, &request, ok(huge));
    let ResponseDisposition::Refused(refusal) = disposition else {
        panic!("an oversized result must be refused, got {disposition:?}");
    };
    assert_eq!(refusal.code, BrowserErrorCode::ResultTooLarge);
    let error = code_of(waiting.join().expect("caller"));
    assert_eq!(error.code, BrowserErrorCode::ResultTooLarge);
    assert!(!error.retryable);
    assert_eq!(broker.pending_len(), 0);
}

#[test]
fn a_result_exactly_at_the_cap_is_delivered() {
    let broker = Arc::new(BrowserBroker::new());
    let app = host(&broker, 7, &["screenshot"]);
    let waiting = call(&broker, "screenshot", None, LONG);
    let request = app.next_request();
    let at_cap = json!({ "s": "x".repeat(MAX_BROWSER_PAYLOAD_BYTES - 8) });
    assert_eq!(compact_len(&at_cap), MAX_BROWSER_PAYLOAD_BYTES);
    assert_eq!(
        app.answer(&broker, &request, ok(at_cap)),
        ResponseDisposition::Delivered
    );
    waiting.join().expect("caller").expect("ok");
}

#[test]
fn oversized_arguments_are_refused_before_anything_is_sent() {
    let broker = BrowserBroker::new();
    let app = host(&broker, 7, &["navigate"]);
    let args = json!({ "text": "x".repeat(MAX_BROWSER_PAYLOAD_BYTES) });
    let error = code_of(broker.execute(&caller(), "navigate", args, None, Some(SHORT)));
    assert_eq!(error.code, BrowserErrorCode::ArgsTooLarge);
    assert!(!error.retryable);
    assert_eq!(app.queued(), 0);
    assert_eq!(broker.pending_len(), 0);
}

#[test]
fn a_hosts_error_text_is_cut_before_the_caller_sees_it() {
    let broker = Arc::new(BrowserBroker::new());
    let app = host(&broker, 7, &["navigate"]);
    let waiting = call(&broker, "navigate", None, LONG);
    let request = app.next_request();
    let verbose = BrowserOutcome::Err(BrowserError {
        code: BrowserErrorCode::HostError,
        message: "é".repeat(MAX_BROWSER_PAYLOAD_BYTES),
        retryable: false,
    });
    assert_eq!(
        app.answer(&broker, &request, verbose),
        ResponseDisposition::Delivered
    );
    let error = code_of(waiting.join().expect("caller"));
    assert_eq!(error.code, BrowserErrorCode::HostError);
    assert!(error.message.len() <= devboule_protocol::MAX_BROWSER_ERROR_MESSAGE_BYTES);
    assert!(error.message.starts_with('é'));
}
