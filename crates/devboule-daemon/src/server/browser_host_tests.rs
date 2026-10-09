//! Who may be a browser host, and what the three frames answer: the door
//! (`browser.host` negotiated, never a peer), the registration lifecycle, and
//! the acknowledgements that must not tell a stranger what is pending.

use super::*;
use crate::peer_policy::PeerScope;
use crate::peer_policy::{TransportBinding, CAP_ADMIN};
use devboule_protocol::{
    BrowserCaller, BrowserError, BrowserErrorCode, BrowserOutcome, ErrorDetails,
    MAX_BROWSER_PAYLOAD_BYTES,
};
use serde_json::json;

fn local_conn(id: u64, negotiated: bool) -> Arc<ConnHandle> {
    let conn = ConnHandle::new(id);
    conn.set_browser_host_negotiated(negotiated);
    conn
}

fn peer_conn() -> Arc<ConnHandle> {
    let conn = ConnHandle::with_peer_caps(
        9,
        None,
        Some(ConnPeer::Remote {
            device_id: "dev-peer-1".to_string(),
            scope: PeerScope::PairedUser,
            paired_by_user: Some("local-user".to_string()),
            binding: TransportBinding::tailnet("nstable", "node", "user@example.com"),
        }),
        devboule_protocol::PEER_CAPS
            .iter()
            .map(|cap| (*cap).to_string())
            .collect(),
        QuitIntent::default(),
    );
    // Even a peer that somehow agreed the name is refused by the gate.
    conn.set_browser_host_negotiated(true);
    conn
}

fn send(state: &Arc<ServerState>, conn: &Arc<ConnHandle>, request: ClientMessage) -> DaemonMessage {
    let owner = OwnerId::new("local-user", "process-1").expect("owner");
    dispatch(state, &owner, request, conn, true, true, true, true).expect("an immediate reply")
}

fn register(commands: &[&str]) -> ClientMessage {
    ClientMessage::BrowserHostRegister {
        id: 1,
        supported_commands: commands.iter().map(|name| (*name).to_string()).collect(),
    }
}

fn refusal(reply: DaemonMessage) -> WireError {
    match reply {
        DaemonMessage::Error(error) => error,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn registered_host(reply: DaemonMessage) -> String {
    match reply {
        DaemonMessage::BrowserHostRegistered { host_id, .. } => host_id,
        other => panic!("expected a registration, got {other:?}"),
    }
}

#[test]
fn a_connection_that_did_not_negotiate_browser_host_is_refused_all_three_frames() {
    let state = ServerState::new("browser-host-no-cap".into());
    let conn = local_conn(1, false);
    for request in [
        register(&["navigate"]),
        ClientMessage::BrowserHostUnregister {
            id: 2,
            host_id: "1.1".to_string(),
        },
        ClientMessage::BrowserExecuteResponse {
            id: 3,
            request_id: "browser-1".to_string(),
            host_id: "1.1".to_string(),
            outcome: BrowserOutcome::Ok { result: json!({}) },
        },
    ] {
        let id = request.request_id();
        let error = refusal(send(&state, &conn, request));
        assert_eq!(error.code, ErrorCode::CapabilityNotSupported);
        assert_eq!(error.id, id);
        assert!(error.message.contains("browser.host"), "{error:?}");
    }
    assert_eq!(state.browser.host_count(), 0);
}

#[test]
fn a_peer_connection_is_refused_by_the_gate_even_holding_every_capability() {
    let state = ServerState::new("browser-host-peer".into());
    let conn = peer_conn();
    assert!(devboule_protocol::PEER_CAPS.contains(&CAP_ADMIN));
    for request in [
        register(&["navigate"]),
        ClientMessage::BrowserHostUnregister {
            id: 2,
            host_id: "9.1".to_string(),
        },
        ClientMessage::BrowserExecuteResponse {
            id: 3,
            request_id: "browser-1".to_string(),
            host_id: "9.1".to_string(),
            outcome: BrowserOutcome::Ok { result: json!({}) },
        },
    ] {
        let id = request.request_id();
        let error = refusal(send(&state, &conn, request));
        assert_eq!(error.code, ErrorCode::CapabilityNotSupported);
        assert_eq!(error.id, id);
        assert!(error.message.contains("browser.host"), "{error:?}");
    }
    assert_eq!(
        state.browser.host_count(),
        0,
        "no peer frame reached the broker"
    );
}

#[test]
fn a_local_app_registers_and_leaves_and_each_registration_is_a_new_id() {
    let state = ServerState::new("browser-host-lifecycle".into());
    let conn = local_conn(1, true);
    let first = registered_host(send(&state, &conn, register(&["navigate"])));
    let second = registered_host(send(&state, &conn, register(&["navigate"])));
    assert_ne!(first, second);

    let stale = ClientMessage::BrowserHostUnregister {
        id: 5,
        host_id: first,
    };
    let error = refusal(send(&state, &conn, stale));
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(error.id, Some(5));

    let leaving = ClientMessage::BrowserHostUnregister {
        id: 6,
        host_id: second,
    };
    assert_eq!(send(&state, &conn, leaving), DaemonMessage::Ok { id: 6 });
    assert_eq!(state.browser.host_count(), 0);
}

#[test]
fn a_registration_must_be_a_list_of_command_names() {
    let state = ServerState::new("browser-host-names".into());
    let conn = local_conn(1, true);
    for bad in [
        vec![""],
        vec!["has space"],
        vec!["navigate;rm"],
        vec!["ünicode"],
    ] {
        let error = refusal(send(&state, &conn, register(&bad)));
        assert_eq!(error.code, ErrorCode::InvalidRequest, "{bad:?}");
    }
    let too_many: Vec<String> = (0..65).map(|n| format!("command_{n}")).collect();
    let error = refusal(send(
        &state,
        &conn,
        ClientMessage::BrowserHostRegister {
            id: 1,
            supported_commands: too_many,
        },
    ));
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(state.browser.host_count(), 0);
}

fn pending_call(
    state: &Arc<ServerState>,
    timeout: Duration,
) -> std::thread::JoinHandle<Result<serde_json::Value, BrowserError>> {
    let state = Arc::clone(state);
    std::thread::spawn(move || {
        state.browser.execute(
            &BrowserCaller {
                caller_session_id: "s.agent.1".to_string(),
                workspace_id: None,
            },
            "screenshot",
            json!({}),
            None,
            Some(timeout),
        )
    })
}

fn wait_for_pending(state: &ServerState) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while state.browser.pending_len() == 0 {
        assert!(Instant::now() < deadline, "the call never became pending");
        std::thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn a_late_or_foreign_answer_is_acknowledged_like_any_other() {
    let state = ServerState::new("browser-host-ack".into());
    let host = local_conn(1, true);
    let host_id = registered_host(send(&state, &host, register(&["screenshot"])));
    let stranger = local_conn(2, true);

    let answer = |id: u64, host_id: &str, request_id: &str| ClientMessage::BrowserExecuteResponse {
        id,
        request_id: request_id.to_string(),
        host_id: host_id.to_string(),
        outcome: BrowserOutcome::Ok { result: json!({}) },
    };
    assert_eq!(
        send(&state, &host, answer(10, &host_id, "browser-404")),
        DaemonMessage::Ok { id: 10 },
        "an unknown request is a quiet acknowledgement"
    );
    let waiting = pending_call(&state, Duration::from_secs(10));
    wait_for_pending(&state);
    assert_eq!(
        send(&state, &stranger, answer(11, &host_id, "browser-1")),
        DaemonMessage::Ok { id: 11 },
        "a stranger's answer looks like any other to the stranger"
    );
    assert_eq!(state.browser.pending_len(), 1, "and it resolved nothing");
    assert_eq!(
        send(&state, &host, answer(12, &host_id, "browser-1")),
        DaemonMessage::Ok { id: 12 }
    );
    waiting.join().expect("caller").expect("the host's answer");
}

#[test]
fn an_oversized_result_is_refused_to_the_host_with_the_browser_code() {
    let state = ServerState::new("browser-host-too-large".into());
    let host = local_conn(1, true);
    let host_id = registered_host(send(&state, &host, register(&["screenshot"])));
    let waiting = pending_call(&state, Duration::from_secs(10));
    wait_for_pending(&state);

    let huge = ClientMessage::BrowserExecuteResponse {
        id: 20,
        request_id: "browser-1".to_string(),
        host_id,
        outcome: BrowserOutcome::Ok {
            result: json!({ "png": "x".repeat(MAX_BROWSER_PAYLOAD_BYTES) }),
        },
    };
    let error = refusal(send(&state, &host, huge));
    assert_eq!(error.id, Some(20));
    assert_eq!(
        error.details,
        Some(ErrorDetails::BrowserRefused {
            code: BrowserErrorCode::ResultTooLarge
        })
    );
    let failed = waiting.join().expect("caller").expect_err("refused");
    assert_eq!(failed.code, BrowserErrorCode::ResultTooLarge);
}
