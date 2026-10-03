//! The browser host over a real connection: admission by hello, the push and
//! the answer on the wire, a stranger's answer, a host that vanishes, and a
//! connection that is busy with other frames when its answer is processed.

use super::browser_host_harness::WireDaemon;
use super::*;
use devboule_protocol::{BrowserCaller, BrowserError, BrowserErrorCode, BrowserOutcome};
use serde_json::{json, Value};

fn send(framed: &Framed, request: ClientMessage) {
    framed.send(&request).expect("send");
}

fn recv(framed: &Framed) -> DaemonMessage {
    framed
        .recv_timeout::<DaemonMessage>(Duration::from_secs(10))
        .expect("a frame within the budget")
}

fn register(framed: &Framed, commands: &[&str]) -> String {
    send(
        framed,
        ClientMessage::BrowserHostRegister {
            id: 1,
            supported_commands: commands.iter().map(|name| (*name).to_string()).collect(),
        },
    );
    match recv(framed) {
        DaemonMessage::BrowserHostRegistered { host_id, .. } => host_id,
        other => panic!("expected a registration, got {other:?}"),
    }
}

fn start_call(
    daemon: &WireDaemon,
    command: &'static str,
    timeout: Duration,
) -> std::thread::JoinHandle<Result<Value, BrowserError>> {
    let state = Arc::clone(&daemon.state);
    std::thread::spawn(move || {
        state.browser.execute(
            &BrowserCaller {
                caller_session_id: "s.agent.1".to_string(),
                workspace_id: Some("w1".to_string()),
            },
            command,
            json!({"n": 1}),
            None,
            Some(timeout),
        )
    })
}

fn next_request(framed: &Framed) -> devboule_protocol::BrowserExecuteRequest {
    loop {
        if let DaemonMessage::BrowserExecuteRequest(request) = recv(framed) {
            return request;
        }
    }
}

fn answer(
    id: u64,
    request: &devboule_protocol::BrowserExecuteRequest,
    result: Value,
) -> ClientMessage {
    ClientMessage::BrowserExecuteResponse {
        id,
        request_id: request.request_id.clone(),
        host_id: request.host_id.clone(),
        outcome: BrowserOutcome::Ok { result },
    }
}

#[test]
fn a_connection_with_a_plain_hello_cannot_register() {
    let daemon = WireDaemon::start("browser-wire-plain");
    let plain = daemon.raw(false);
    send(
        &plain,
        ClientMessage::BrowserHostRegister {
            id: 1,
            supported_commands: vec!["navigate".to_string()],
        },
    );
    match recv(&plain) {
        DaemonMessage::Error(error) => {
            assert_eq!(error.code, ErrorCode::CapabilityNotSupported);
            assert_eq!(error.id, Some(1));
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    assert_eq!(daemon.state.browser.host_count(), 0);
}

#[test]
fn a_command_travels_to_the_host_and_its_answer_travels_back() {
    let daemon = WireDaemon::start("browser-wire-roundtrip");
    let host = daemon.raw(true);
    register(&host, &["navigate"]);
    let waiting = start_call(&daemon, "navigate", Duration::from_secs(10));

    let request = next_request(&host);
    assert_eq!(request.command, "navigate");
    assert_eq!(request.args, json!({"n": 1}));
    assert_eq!(request.caller.caller_session_id, "s.agent.1");
    assert_eq!(request.caller.workspace_id.as_deref(), Some("w1"));
    send(&host, answer(7, &request, json!({"title": "ok"})));
    assert_eq!(recv(&host), DaemonMessage::Ok { id: 7 });
    assert_eq!(
        waiting.join().expect("caller").expect("the answer"),
        json!({"title": "ok"})
    );
}

#[test]
fn another_connections_answer_is_ignored_and_the_real_one_still_lands() {
    let daemon = WireDaemon::start("browser-wire-foreign");
    let host = daemon.raw(true);
    register(&host, &["navigate"]);
    let stranger = daemon.raw(true);
    let waiting = start_call(&daemon, "navigate", Duration::from_secs(10));
    let request = next_request(&host);

    send(&stranger, answer(3, &request, json!({"forged": true})));
    assert_eq!(recv(&stranger), DaemonMessage::Ok { id: 3 });
    assert_eq!(daemon.state.browser.pending_len(), 1, "nothing resolved");

    send(&host, answer(4, &request, json!({"real": true})));
    assert_eq!(recv(&host), DaemonMessage::Ok { id: 4 });
    assert_eq!(
        waiting.join().expect("caller").expect("the host's answer"),
        json!({"real": true})
    );
}

#[test]
fn a_host_that_disconnects_mid_call_fails_it_as_no_host() {
    let daemon = WireDaemon::start("browser-wire-vanish");
    let keeper = daemon.raw(true);
    let host = daemon.raw(true);
    register(&host, &["navigate"]);
    let waiting = start_call(&daemon, "navigate", Duration::from_secs(10));
    next_request(&host);

    drop(host);
    let error = waiting
        .join()
        .expect("caller")
        .expect_err("the host is gone");
    assert_eq!(error.code, BrowserErrorCode::NoHost);
    assert!(error.retryable);
    assert_eq!(daemon.state.browser.pending_len(), 0);
    assert_eq!(daemon.state.browser.host_count(), 0);
    drop(keeper);
}

/// The answer is processed on the connection thread that is also reading
/// other frames: a burst of pings goes out ahead of it and more behind it,
/// and every reply, the answer's acknowledgement included, comes back.
#[test]
fn the_answer_is_processed_while_the_connection_is_busy_with_other_frames() {
    let daemon = WireDaemon::start("browser-wire-busy");
    let host = daemon.raw(true);
    register(&host, &["navigate"]);
    let waiting = start_call(&daemon, "navigate", Duration::from_secs(10));
    let request = next_request(&host);

    // The daemon flushes each reply to the pipe, so the host must read while
    // it writes or the test would deadlock on its own pipe buffer.
    let writer = {
        let host = host.clone();
        std::thread::spawn(move || {
            for id in 100..150 {
                send(&host, ClientMessage::Ping { id });
            }
            send(&host, answer(7, &request, json!({"busy": true})));
            for id in 150..200 {
                send(&host, ClientMessage::Ping { id });
            }
        })
    };

    let (mut pongs, mut acknowledged) = (0, false);
    while pongs < 100 || !acknowledged {
        match recv(&host) {
            DaemonMessage::Pong { .. } => pongs += 1,
            DaemonMessage::Ok { id: 7 } => acknowledged = true,
            other => panic!("unexpected frame while busy: {other:?}"),
        }
    }
    writer.join().expect("writer");
    assert_eq!(
        waiting.join().expect("caller").expect("the answer"),
        json!({"busy": true})
    );
}
