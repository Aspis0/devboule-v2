//! The browser-host client half against a real daemon: that it refuses to
//! speak without the capability, that a command reaches the receiver off the
//! reader thread, that a host may make its own RPCs while it handles one, and
//! that a host that stops draining its queue is refused promptly, not timed out.

use std::time::Duration;

use devboule_protocol::{
    BrowserCaller, BrowserError, BrowserErrorCode, BrowserOutcome, ErrorCode, ErrorDetails,
    MAX_BROWSER_PAYLOAD_BYTES,
};
use serde_json::{json, Value};

use crate::server::browser_host_harness::WireDaemon;

fn caller() -> BrowserCaller {
    BrowserCaller {
        caller_session_id: "s.agent.1".to_string(),
        workspace_id: Some("w1".to_string()),
    }
}

fn start_call(
    daemon: &WireDaemon,
    command: &'static str,
) -> std::thread::JoinHandle<Result<Value, BrowserError>> {
    start_call_for(daemon, command, Duration::from_secs(10))
}

fn start_call_for(
    daemon: &WireDaemon,
    command: &'static str,
    timeout: Duration,
) -> std::thread::JoinHandle<Result<Value, BrowserError>> {
    let state = std::sync::Arc::clone(&daemon.state);
    std::thread::spawn(move || {
        state
            .browser
            .execute(&caller(), command, json!({}), None, Some(timeout))
    })
}

#[test]
fn a_client_that_did_not_negotiate_browser_host_sends_nothing() {
    let daemon = WireDaemon::start("browser-client-plain");
    let client = daemon.client(false);
    let error = client
        .browser_host_register(&["navigate".to_string()])
        .expect_err("refused before it leaves");
    assert!(
        error.to_string().contains("browser.host"),
        "the refusal names the capability: {error}"
    );
    assert!(client
        .browser_host_unregister("1.1")
        .expect_err("refused")
        .to_string()
        .contains("browser.host"));
    assert!(client
        .browser_respond("browser-1", "1.1", BrowserOutcome::Ok { result: json!({}) })
        .expect_err("refused")
        .to_string()
        .contains("browser.host"));
    assert_eq!(daemon.state.browser.host_count(), 0);
}

/// The worker that receives the command makes two RPCs of its own before it
/// answers. Were the command run on the reader thread, the ping's reply could
/// never be read and this would stall until the RPC timeout.
#[test]
fn a_host_makes_its_own_rpcs_while_it_handles_a_command() {
    let daemon = WireDaemon::start("browser-client-handler");
    let client = std::sync::Arc::new(daemon.client(true));
    let host_id = client
        .browser_host_register(&["navigate".to_string()])
        .expect("register");
    let inbox = client.take_browser_requests().expect("the queue, once");
    assert!(client.take_browser_requests().is_none(), "handed out once");

    let worker_client = std::sync::Arc::clone(&client);
    let worker = std::thread::spawn(move || {
        let request = inbox
            .recv_timeout(Duration::from_secs(10))
            .expect("the daemon's command");
        worker_client.ping().expect("an RPC inside the handler");
        worker_client
            .browser_respond(
                &request.request_id,
                &request.host_id,
                BrowserOutcome::Ok {
                    result: json!({"command": request.command}),
                },
            )
            .expect("the answer");
        request
    });
    let waiting = start_call(&daemon, "navigate");

    let request = worker.join().expect("worker");
    assert_eq!(request.host_id, host_id);
    assert_eq!(request.caller, caller());
    assert_eq!(
        waiting.join().expect("caller").expect("the answer"),
        json!({"command": "navigate"})
    );
}

#[test]
fn unregistering_fails_the_waiting_call_as_no_host() {
    let daemon = WireDaemon::start("browser-client-unregister");
    let client = daemon.client(true);
    let host_id = client
        .browser_host_register(&["navigate".to_string()])
        .expect("register");
    let inbox = client.take_browser_requests().expect("queue");
    let waiting = start_call(&daemon, "navigate");
    inbox
        .recv_timeout(Duration::from_secs(10))
        .expect("the daemon's command");

    client
        .browser_host_unregister(&host_id)
        .expect("unregister");
    let error = waiting.join().expect("caller").expect_err("no host");
    assert_eq!(error.code, BrowserErrorCode::NoHost);
    assert!(error.retryable);
}

#[test]
fn an_oversized_answer_comes_back_as_the_daemons_typed_refusal() {
    let daemon = WireDaemon::start("browser-client-too-large");
    let client = daemon.client(true);
    client
        .browser_host_register(&["screenshot".to_string()])
        .expect("register");
    let inbox = client.take_browser_requests().expect("queue");
    let waiting = start_call(&daemon, "screenshot");
    let request = inbox
        .recv_timeout(Duration::from_secs(10))
        .expect("the daemon's command");

    let refusal = client
        .browser_respond(
            &request.request_id,
            &request.host_id,
            BrowserOutcome::Ok {
                result: json!({ "png": "x".repeat(MAX_BROWSER_PAYLOAD_BYTES) }),
            },
        )
        .expect_err("refused");
    let crate::DaemonError::Handshake(error) = refusal else {
        panic!("expected the daemon's error, got {refusal:?}");
    };
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(
        error.details,
        Some(ErrorDetails::BrowserRefused {
            code: BrowserErrorCode::ResultTooLarge
        })
    );
    assert_eq!(
        waiting.join().expect("caller").expect_err("refused").code,
        BrowserErrorCode::ResultTooLarge
    );
}

#[test]
fn a_hosts_long_error_text_is_cut_and_the_connection_survives() {
    let daemon = WireDaemon::start("browser-client-long-error");
    let client = daemon.client(true);
    client
        .browser_host_register(&["navigate".to_string()])
        .expect("register");
    let inbox = client.take_browser_requests().expect("queue");
    let waiting = start_call(&daemon, "navigate");
    let request = inbox
        .recv_timeout(Duration::from_secs(10))
        .expect("the daemon's command");

    // Larger than a frame: written whole it would have cost the connection.
    let verbose = BrowserError {
        code: BrowserErrorCode::HostError,
        message: "x".repeat(2 * devboule_protocol::MAX_FRAME_BYTES),
        retryable: false,
    };
    client
        .browser_respond(
            &request.request_id,
            &request.host_id,
            BrowserOutcome::Err(verbose),
        )
        .expect("a bounded answer still goes through");
    let error = waiting
        .join()
        .expect("caller")
        .expect_err("the host's failure");
    assert_eq!(error.code, BrowserErrorCode::HostError);
    assert!(error.message.len() <= devboule_protocol::MAX_BROWSER_ERROR_MESSAGE_BYTES);
    client.ping().expect("the connection is still up");
}

/// Sixty-four commands that timed out unanswered fill the queue of a host that
/// is not draining it. The next command is refused `browser_busy` at once
/// instead of waiting out its own deadline, and once the host drains the queue
/// it is admitted again.
#[test]
fn a_full_queue_refuses_at_once_and_admits_again_once_drained() {
    let daemon = WireDaemon::start("browser-client-full-queue");
    let client = daemon.client(true);
    client
        .browser_host_register(&["navigate".to_string()])
        .expect("register");
    let inbox = client.take_browser_requests().expect("queue");

    for _ in 0..4 {
        let round: Vec<_> = (0..16)
            .map(|_| start_call_for(&daemon, "navigate", Duration::from_millis(200)))
            .collect();
        for call in round {
            let error = call.join().expect("caller").expect_err("unanswered");
            assert_eq!(error.code, BrowserErrorCode::Timeout);
        }
    }

    let started = std::time::Instant::now();
    let refused = start_call(&daemon, "navigate")
        .join()
        .expect("caller")
        .expect_err("the queue is full");
    assert_eq!(refused.code, BrowserErrorCode::Busy);
    assert!(refused.retryable);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the refusal came at once, not at the 10 s deadline: {:?}",
        started.elapsed()
    );

    let stale = std::iter::from_fn(|| inbox.try_recv().ok()).count();
    assert_eq!(stale, 64, "the queue held the timed-out commands");
    let waiting = start_call(&daemon, "navigate");
    let request = inbox
        .recv_timeout(Duration::from_secs(10))
        .expect("admitted again");
    client
        .browser_respond(
            &request.request_id,
            &request.host_id,
            BrowserOutcome::Ok {
                result: json!({"back": true}),
            },
        )
        .expect("the answer");
    assert_eq!(
        waiting.join().expect("caller").expect("answered"),
        json!({"back": true})
    );
}
