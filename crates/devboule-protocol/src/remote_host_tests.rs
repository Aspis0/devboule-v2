//! Tests for the remote-host wire surface (`remote_host.rs`): that the three
//! lists are the whole vocabulary, that each one produces the request the far
//! daemon already serves, and that a reply of another shape is never carried
//! through as if it were the answer.

use super::*;
use crate::{DaemonMessage, ErrorCode, WireError};

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
