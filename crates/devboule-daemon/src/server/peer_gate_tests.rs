//! Tests for the peer gate's session-target ordering and capability door.

use super::*;
use crate::peer_policy::PeerScope;
use crate::peer_policy::{TransportBinding, CAP_SEND, CAP_VIEW};
use devboule_protocol::PeerRole;

fn remote_conn(caps: &[&str]) -> Arc<ConnHandle> {
    ConnHandle::with_peer_caps(
        7,
        None,
        Some(ConnPeer::Remote {
            device_id: "dev-peer-1".to_string(),
            scope: PeerScope::PeerDevice,
            paired_by_user: Some("local-user".to_string()),
            binding: TransportBinding::tailnet("nstable", "node", "user@example.com"),
        }),
        caps.iter().map(|cap| (*cap).to_string()).collect(),
        QuitIntent::default(),
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

/// The pairing memory's rule, pinned: a client device — the owner's own phone
/// or laptop — may send into the local sessions of the person who paired it,
/// under the `send` grant. Child scopes and machine records must never take
/// that away, because that send is the point of pairing a device.
#[test]
fn a_client_scoped_send_reaches_the_paired_users_local_session() {
    let state = ServerState::new("peer-gate-client-local-target".into());
    let target_owner = OwnerId::new("local-user", "local-process").expect("owner");
    crate::session::insert_test_live_agent(&state.sessions, "s.local.target", target_owner);
    let conn = ConnHandle::with_peer_caps(
        7,
        None,
        Some(ConnPeer::Remote {
            device_id: "dev-phone".to_string(),
            scope: PeerScope::PairedUser,
            paired_by_user: Some("local-user".to_string()),
            binding: TransportBinding::tailnet("nstable", "node", "user@example.com"),
        }),
        [CAP_VIEW.to_string(), CAP_SEND.to_string()].to_vec(),
        QuitIntent::default(),
    );
    let owner = OwnerId::new("local-user", "paired-device").expect("peer owner");

    assert!(
        peer_refusal_before_mode(&state, &owner, &request("s.local.target"), &conn.conn_peer)
            .is_none(),
        "the paired user's own device must reach that user's local sessions"
    );

    // And only that user's: another account's local session stays out of a
    // client device's reach, so the allowance is consent to one person's
    // sessions, not to the machine.
    let other_owner = OwnerId::new("other-user", "local-process").expect("owner");
    crate::session::insert_test_live_agent(&state.sessions, "s.other.target", other_owner);
    assert!(
        peer_refusal_before_mode(&state, &owner, &request("s.other.target"), &conn.conn_peer)
            .is_some(),
        "another account's session is not the paired user's consent"
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
        current_model_provider_id: None,
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

/// The open root is local-only: its reply would be this machine's absolute
/// folder, and the editor launch it feeds exists only in the desktop
/// process — so no capability, `admin` among them, opens it for a peer.
/// The refusal is the capability refusal every gate denial is, and it
/// carries no path: the frame never reaches dispatch, so nothing about the
/// workspace's location can travel back.
#[test]
fn the_open_root_is_refused_to_a_peer_holding_every_capability() {
    let state = ServerState::new("peer-gate-open-root".into());
    let owner = OwnerId::new("peer_dev-peer-1", "daemon").expect("peer owner");
    let conn = remote_conn(&devboule_protocol::PEER_CAPS);
    let request = ClientMessage::WorkspaceOpenRoot {
        id: 9,
        workspace_id: "ws.1".to_string(),
    };

    let reply = match run_gate(&state, &owner, &request, &conn) {
        Ok(_) => panic!("the open root must never reach dispatch for a peer"),
        Err(reply) => reply,
    };
    match *reply {
        DaemonMessage::Error(error) => {
            assert_eq!(error.code, ErrorCode::CapabilityNotSupported);
            assert_eq!(error.id, Some(9), "the refusal answers the request");
            assert!(
                error.message.contains("workspace.open"),
                "the refusal names the act no capability holds: {error:?}"
            );
            assert!(
                !error.message.contains(['/', '\\']),
                "a gate refusal must carry no path: {error:?}"
            );
        }
        other => panic!("expected a capability refusal, got {other:?}"),
    }
}
