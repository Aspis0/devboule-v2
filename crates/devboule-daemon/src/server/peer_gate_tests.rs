//! Tests for the peer gate's session-target ordering and capability door.

use super::*;
use crate::peer_policy::{TransportBinding, CAP_SEND, CAP_VIEW};

fn remote_conn(caps: &[&str]) -> Arc<ConnHandle> {
    ConnHandle::with_peer_caps(
        7,
        None,
        Some(ConnPeer::Remote {
            device_id: "dev-peer-1".to_string(),
            role: PeerRole::Daemon,
            paired_by_user: Some("local-user".to_string()),
            binding: TransportBinding::tailnet("nstable", "node", "user@example.com"),
        }),
        caps.iter().map(|cap| (*cap).to_string()).collect(),
    )
}

fn request(to_session: &str) -> ClientMessage {
    ClientMessage::AgentMessageSend {
        id: 1,
        from_session: "s.far.source".to_string(),
        to_session: to_session.to_string(),
        text: "hello".to_string(),
        idempotency_key: None,
    }
}

#[test]
fn a_daemon_send_reaches_the_registry_for_a_local_target() {
    let state = ServerState::new("peer-gate-local-target".into());
    let target_owner = OwnerId::new("local-user", "local-process").expect("owner");
    crate::session::insert_test_live_agent(&state.sessions, "s.local.target", target_owner);
    let conn = remote_conn(&[CAP_VIEW, CAP_SEND]);
    let owner = OwnerId::new("peer_dev-peer-1", "daemon").expect("peer owner");

    assert!(
        peer_refusal_before_mode(&state, &owner, &request("s.local.target"), &conn.conn_peer)
            .is_none(),
        "the daemon peer's source namespace must not scope a local target"
    );
}

#[test]
fn a_third_device_target_is_refused_like_an_unknown_id_before_mode_lookup() {
    let state = ServerState::new("peer-gate-third-target".into());
    let owner = OwnerId::new("peer_dev-peer-1", "daemon").expect("peer owner");
    let runtime = crate::session::insert_test_live_agent_with_kind(
        &state.sessions,
        "s.third.target",
        owner.clone(),
        SessionKind::Claude,
    );
    runtime.store_session_manifest(devboule_protocol::SessionEvent::SessionManifest {
        provider_id: Some("claude".to_string()),
        current_model_id: None,
        models: Vec::new(),
        modes: Some(devboule_protocol::SessionModeStateView {
            current_mode_id: "bypassPermissions".to_string(),
            available_modes: Vec::new(),
        }),
    });
    state.sessions.set_test_origin(
        "s.third.target",
        devboule_protocol::SessionOrigin::peer("dev-tablet", PeerRole::Daemon),
    );
    let conn = remote_conn(&[CAP_VIEW, CAP_SEND]);

    let unknown = peer_refusal_before_mode(
        &state,
        &owner,
        &request("s.unknown.target"),
        &conn.conn_peer,
    )
    .expect("an unknown target is refused at the gate");
    let relay =
        peer_refusal_before_mode(&state, &owner, &request("s.third.target"), &conn.conn_peer)
            .expect("a third-device target is refused at the gate");
    assert_eq!(
        relay, unknown,
        "relay and unknown targets are indistinguishable"
    );
    assert!(
        peer_mode_refusal_for_conn(&state, &request("s.third.target"), &conn.conn_peer,).is_none(),
        "a gate-denied relay must not disclose its prompt-skipping mode"
    );
}

#[test]
fn send_without_the_peer_capability_stops_at_the_gate() {
    let state = ServerState::new("peer-gate-no-send".into());
    let owner = OwnerId::new("peer_dev-peer-1", "daemon").expect("peer owner");
    let conn = remote_conn(&[CAP_VIEW]);

    let result = run_gate(&state, &owner, &request("s.local.target"), &conn);
    let reply = match result {
        Ok(_) => panic!("view alone does not open agent messaging"),
        Err(reply) => reply,
    };
    match *reply {
        DaemonMessage::Error(error) => {
            assert_eq!(error.code, ErrorCode::CapabilityNotSupported);
            assert!(error.message.contains(CAP_SEND), "{error:?}");
        }
        other => panic!("expected a capability refusal, got {other:?}"),
    }
}
