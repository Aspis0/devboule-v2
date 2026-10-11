//! The transcript search is a session read: a connection that did not
//! negotiate sessions, or the journal it reads, is refused before any scan
//! starts, the same way the other session reads are refused.

use super::*;
use devboule_protocol::ErrorCode;

#[test]
fn a_connection_without_the_sessions_capability_is_refused_the_search() {
    let state = Arc::new(ServerState::new("dispatch-search-gate".into()));
    let owner = OwnerId::new("local-user", "local-process").expect("owner");
    let conn = ConnHandle::new(1);
    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::SessionsSearch {
            id: 5,
            query: "kalsa".to_string(),
        },
        &conn,
        false,
        true,
        true,
        true,
    );
    match reply {
        Some(DaemonMessage::Error(error)) => {
            assert_eq!(error.code, ErrorCode::CapabilityNotSupported);
            assert_eq!(error.id, Some(5));
        }
        other => panic!("the search must be refused without the capability: {other:?}"),
    }
}

#[test]
fn a_connection_without_the_journal_is_refused_the_search() {
    let state = Arc::new(ServerState::new("dispatch-search-journal".into()));
    let owner = OwnerId::new("local-user", "local-process").expect("owner");
    let conn = ConnHandle::new(2);
    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::SessionsSearch {
            id: 6,
            query: "kalsa".to_string(),
        },
        &conn,
        true,
        false,
        true,
        true,
    );
    assert!(
        matches!(reply, Some(DaemonMessage::Error(_))),
        "the search needs the journal: {reply:?}"
    );
}
