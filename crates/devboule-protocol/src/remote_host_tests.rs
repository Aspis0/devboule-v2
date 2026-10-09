//! Tests for the remote-host wire surface (`remote_host.rs`): that the three
//! lists are the whole vocabulary, that each one produces the request the far
//! daemon already serves, that a reply of another shape is never carried
//! through as if it were the answer, and that the operate frames (Slice 4)
//! round-trip with the fields the link maps to the peer's own session
//! frames.

use super::*;
use crate::{DaemonMessage, ErrorCode, SessionKind, WireError};

#[test]
fn each_list_puts_the_far_daemons_own_frame_on_the_wire() {
    assert_eq!(
        RemoteHostList::Projects.peer_request(7),
        ClientMessage::ProjectsList { id: 7 }
    );
    assert_eq!(
        RemoteHostList::Workspaces {
            project_id: "p1".to_string()
        }
        .peer_request(8),
        ClientMessage::WorkspacesList {
            id: 8,
            project_id: "p1".to_string(),
        }
    );
    assert_eq!(
        RemoteHostList::Sessions.peer_request(9),
        ClientMessage::SessionsList { id: 9 }
    );
}

/// The allowlist is a closed type, so it is pinned two ways: the three
/// variants serialize as their own names, and a fourth name is not a variant
/// this crate can deserialize into the link's vocabulary at all.
#[test]
fn the_list_vocabulary_is_exactly_three_reads() {
    let read = |json: &str| -> Option<RemoteHostList> { serde_json::from_str(json).ok() };
    assert_eq!(
        read(r#"{"kind":"projects"}"#),
        Some(RemoteHostList::Projects)
    );
    assert_eq!(
        read(r#"{"kind":"workspaces","projectId":"p1"}"#),
        Some(RemoteHostList::Workspaces {
            project_id: "p1".to_string()
        })
    );
    assert_eq!(
        read(r#"{"kind":"sessions"}"#),
        Some(RemoteHostList::Sessions)
    );
    for fourth in [
        r#"{"kind":"agents"}"#,
        r#"{"kind":"ProjectsList"}"#,
        r#"{"kind":"journal"}"#,
    ] {
        assert!(
            read(fourth).is_none(),
            "{fourth} is not one of the three reads, so it must not deserialize"
        );
    }
}

#[test]
fn a_reply_of_another_shape_is_not_carried_through() {
    let sessions = RemoteHostList::Sessions;
    let wrong = DaemonMessage::Projects {
        id: 1,
        projects: Vec::new(),
    };
    assert!(
        sessions.body_from_reply(&wrong).is_none(),
        "an answer to a different list must not be dressed as this one"
    );
    assert!(sessions
        .body_from_reply(&DaemonMessage::Error(WireError::new(
            ErrorCode::Unauthorized,
            "no"
        )))
        .is_none());
    let right = DaemonMessage::Sessions {
        id: 1,
        sessions: Vec::new(),
    };
    assert_eq!(
        sessions.body_from_reply(&right),
        Some(RemoteHostListBody::Sessions { rows: Vec::new() })
    );
}

#[test]
fn a_body_carries_the_rows_and_nothing_else() {
    let json =
        serde_json::to_value(RemoteHostListBody::Workspaces { rows: Vec::new() }).expect("json");
    assert_eq!(json, serde_json::json!({"list": "workspaces", "rows": []}));
}

#[test]
fn the_status_push_is_camel_cased_for_the_webview() {
    let status = RemoteHostStatus {
        device_id: "b".to_string(),
        state: RemoteHostState::NeedsPairing,
        last_failure: Some("This device is no longer paired with this daemon.".to_string()),
        revision: None,
    };
    assert_eq!(
        serde_json::to_value(&status).expect("json"),
        serde_json::json!({
            "deviceId": "b",
            "state": "needs_pairing",
            "lastFailure": "This device is no longer paired with this daemon.",
        })
    );
    assert_eq!(
        serde_json::to_value(RemoteHostState::IdentityMissing).expect("json"),
        serde_json::json!("identity_missing")
    );
}

#[test]
fn the_status_push_carries_the_host_revision_when_it_has_one() {
    let status = RemoteHostStatus {
        device_id: "b".to_string(),
        state: RemoteHostState::Online,
        last_failure: None,
        revision: Some(7),
    };
    assert_eq!(
        serde_json::to_value(&status).expect("json"),
        serde_json::json!({
            "deviceId": "b",
            "state": "online",
            "revision": 7,
        })
    );
}

#[test]
fn the_workspace_change_push_names_the_host_and_its_revision() {
    let message = DaemonMessage::HostWorkspaceChanged {
        device_id: "a".to_string(),
        revision: 12,
    };
    assert_eq!(
        serde_json::to_value(&message).expect("json"),
        serde_json::json!({
            "type": "host_workspace_changed",
            "deviceId": "a",
            "revision": 12,
        })
    );
    let back: DaemonMessage = serde_json::from_value(serde_json::json!({
        "type": "host_workspace_changed",
        "deviceId": "a",
        "revision": 12,
    }))
    .expect("back");
    assert_eq!(back, message);
}

#[test]
fn a_peer_hello_keeps_its_presence_and_service_in_step() {
    let owner = OwnerId::new("peer_a", "devboule-daemon").expect("owner");
    let hosting = ClientHello::peer(owner.clone(), "devboule-daemon", true);
    assert_eq!(hosting.workspace_host, Some(true));
    assert!(
        hosting
            .capabilities
            .iter()
            .any(|capability| capability.as_str() == crate::caps::HOSTED_WORKSPACES),
        "a host advertises the service it speaks"
    );

    let client = ClientHello::peer(owner, "devboule-daemon", false);
    assert_eq!(client.workspace_host, Some(false));
    assert!(
        !client
            .capabilities
            .iter()
            .any(|capability| capability.as_str() == crate::caps::HOSTED_WORKSPACES),
        "a client does not advertise a service it does not provide"
    );
}

#[test]
fn the_remote_attach_frames_round_trip_with_their_fields() {
    let attach = ClientMessage::RemoteHostAttach {
        id: 7,
        device_id: "b".to_string(),
        session_id: "session-one".to_string(),
        subscription_id: 3,
    };
    assert_eq!(attach.name(), "RemoteHostAttach");
    let json = serde_json::to_value(&attach).expect("json");
    assert_eq!(json["type"], "remote_host_attach");
    assert_eq!(json["deviceId"], "b");
    assert_eq!(json["sessionId"], "session-one");
    assert_eq!(json["subscriptionId"], 3);
    let back: ClientMessage = serde_json::from_value(json).expect("back");
    assert_eq!(back, attach);

    let detach = ClientMessage::RemoteHostDetach {
        id: 8,
        device_id: "b".to_string(),
        session_id: "session-one".to_string(),
        subscription_id: 3,
    };
    assert_eq!(detach.name(), "RemoteHostDetach");
    let back: ClientMessage =
        serde_json::from_value(serde_json::to_value(&detach).expect("json")).expect("back");
    assert_eq!(back, detach);
}

#[test]
fn a_relayed_remote_event_names_its_host_session_and_subscription() {
    let message = DaemonMessage::RemoteHostEvent {
        device_id: "device-b".to_string(),
        session_id: "session-one".to_string(),
        subscription_id: 9,
        envelope: SessionEventEnvelope {
            session_id: "session-one".to_string(),
            generation: 1,
            transcript_seq: None,
            event: SessionEvent::Output {
                seq: 1,
                data: "hello".to_string(),
            },
        },
    };
    let json = serde_json::to_value(&message).expect("json");
    assert_eq!(json["type"], "remote_host_event");
    assert_eq!(json["deviceId"], "device-b");
    assert_eq!(json["sessionId"], "session-one");
    assert_eq!(json["subscriptionId"], 9);
    assert_eq!(json["envelope"]["event"]["data"], "hello");
    let back: DaemonMessage = serde_json::from_value(json).expect("back");
    assert_eq!(back, message);
}

/// Slice 4: every host-targeted operate frame round-trips with the fields
/// the link maps to the peer's own session frame. One frame per operation —
/// there is no generic tunnel, so each operation names itself here.
#[test]
fn the_remote_operate_frames_round_trip_with_their_fields() {
    let create = ClientMessage::RemoteHostCreate {
        id: 11,
        device_id: "b".to_string(),
        workspace_id: Some("w1".to_string()),
        kind: SessionKind::Terminal,
        provider: None,
        mode: None,
        display_name: Some("B shell".to_string()),
        idempotency_key: Some("key-1".to_string()),
        cols: Some(80),
        rows: Some(24),
    };
    assert_eq!(create.name(), "RemoteHostCreate");
    assert!(create.is_state_changing());
    let json = serde_json::to_value(&create).expect("json");
    assert_eq!(json["type"], "remote_host_create");
    assert_eq!(json["deviceId"], "b");
    assert_eq!(json["workspaceId"], "w1");
    assert_eq!(json["idempotencyKey"], "key-1");
    let back: ClientMessage = serde_json::from_value(json).expect("back");
    assert_eq!(back, create);

    let send = ClientMessage::RemoteHostSend {
        id: 12,
        device_id: "b".to_string(),
        session_id: "s1".to_string(),
        subscription_id: 3,
        text: "hello".to_string(),
        attachments: Vec::new(),
        active_turn_behavior: None,
        idempotency_key: Some("key-2".to_string()),
        attachment_references: Vec::new(),
    };
    assert_eq!(send.name(), "RemoteHostSend");
    assert!(send.is_state_changing());
    let back: ClientMessage =
        serde_json::from_value(serde_json::to_value(&send).expect("json")).expect("back");
    assert_eq!(back, send);

    let resize = ClientMessage::RemoteHostResize {
        id: 13,
        device_id: "b".to_string(),
        session_id: "s1".to_string(),
        subscription_id: 3,
        cols: 80,
        rows: 24,
    };
    assert_eq!(resize.name(), "RemoteHostResize");
    let back: ClientMessage =
        serde_json::from_value(serde_json::to_value(&resize).expect("json")).expect("back");
    assert_eq!(back, resize);

    let claim = ClientMessage::RemoteHostClaim {
        id: 14,
        device_id: "b".to_string(),
        session_id: "s1".to_string(),
        subscription_id: 3,
    };
    assert_eq!(claim.name(), "RemoteHostClaim");
    let back: ClientMessage =
        serde_json::from_value(serde_json::to_value(&claim).expect("json")).expect("back");
    assert_eq!(back, claim);

    let interrupt = ClientMessage::RemoteHostInterrupt {
        id: 15,
        device_id: "b".to_string(),
        session_id: "s1".to_string(),
        subscription_id: 3,
    };
    assert_eq!(interrupt.name(), "RemoteHostInterrupt");
    let back: ClientMessage =
        serde_json::from_value(serde_json::to_value(&interrupt).expect("json")).expect("back");
    assert_eq!(back, interrupt);

    let respond = ClientMessage::RemoteHostPermissionRespond {
        id: 16,
        device_id: "b".to_string(),
        session_id: "s1".to_string(),
        subscription_id: 3,
        request_id: "card-1".to_string(),
        outcome: crate::PermissionOutcome::AllowOnce,
        option_id: None,
        answer: None,
        idempotency_key: Some("key-3".to_string()),
    };
    assert_eq!(respond.name(), "RemoteHostPermissionRespond");
    let back: ClientMessage =
        serde_json::from_value(serde_json::to_value(&respond).expect("json")).expect("back");
    assert_eq!(back, respond);

    let close = ClientMessage::RemoteHostClose {
        id: 17,
        device_id: "b".to_string(),
        session_id: "s1".to_string(),
        idempotency_key: Some("key-4".to_string()),
    };
    assert_eq!(close.name(), "RemoteHostClose");
    let back: ClientMessage =
        serde_json::from_value(serde_json::to_value(&close).expect("json")).expect("back");
    assert_eq!(back, close);

    let stop = ClientMessage::RemoteHostStop {
        id: 18,
        device_id: "b".to_string(),
        session_id: "s1".to_string(),
        subscription_id: 3,
    };
    assert_eq!(stop.name(), "RemoteHostStop");
    let back: ClientMessage =
        serde_json::from_value(serde_json::to_value(&stop).expect("json")).expect("back");
    assert_eq!(back, stop);

    let set_model = ClientMessage::RemoteHostSetModel {
        id: 19,
        device_id: "b".to_string(),
        session_id: "s1".to_string(),
        model_id: Some("model-x".to_string()),
        effort: None,
    };
    assert_eq!(set_model.name(), "RemoteHostSetModel");
    let back: ClientMessage =
        serde_json::from_value(serde_json::to_value(&set_model).expect("json")).expect("back");
    assert_eq!(back, set_model);

    let set_mode = ClientMessage::RemoteHostSetMode {
        id: 20,
        device_id: "b".to_string(),
        session_id: "s1".to_string(),
        mode_id: "default".to_string(),
    };
    assert_eq!(set_mode.name(), "RemoteHostSetMode");
    let back: ClientMessage =
        serde_json::from_value(serde_json::to_value(&set_mode).expect("json")).expect("back");
    assert_eq!(back, set_mode);

    let providers = ClientMessage::RemoteHostProviders {
        id: 21,
        device_id: "b".to_string(),
    };
    assert_eq!(providers.name(), "RemoteHostProviders");
    assert!(!providers.is_state_changing());
    let json = serde_json::to_value(&providers).expect("json");
    assert_eq!(json["type"], "remote_host_providers");
    let back: ClientMessage = serde_json::from_value(json).expect("back");
    assert_eq!(back, providers);
}
