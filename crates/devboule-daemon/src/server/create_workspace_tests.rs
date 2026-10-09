//! The wire door names a workspace: a create that carries none is refused
//! before anything is journaled or spawned, for an agent and for a terminal.

use std::collections::VecDeque;

use devboule_protocol::{ClientMessage, DaemonMessage, ErrorCode, OwnerId, SessionKind};

use super::dispatch;
use super::git_workers::test_support::wait_for_worker_reply;
use super::ServerState;
use crate::session::ConnHandle;

#[test]
fn a_create_without_a_workspace_is_refused_for_an_agent_and_a_terminal() {
    let state = ServerState::new("create-no-workspace".to_string());
    let owner =
        OwnerId::new("S-1-5-21-create-no-workspace", "create-no-workspace-client").expect("owner");
    let conn = ConnHandle::new(1);
    for kind in [SessionKind::Terminal, SessionKind::Acp] {
        let label = format!("{kind:?}");
        let inline = dispatch(
            &state,
            &owner,
            ClientMessage::SessionCreate {
                id: 7,
                workspace_id: None,
                kind,
                provider: None,
                mode: None,
                display_name: None,
                idempotency_key: None,
                cols: None,
                rows: None,
            },
            &conn,
            true,
            true,
            true,
            true,
        );
        let reply = inline.unwrap_or_else(|| {
            let mut backlog = VecDeque::new();
            wait_for_worker_reply(&conn, &mut backlog)
        });
        match reply {
            DaemonMessage::Error(error) => {
                assert!(
                    matches!(error.code, ErrorCode::InvalidRequest),
                    "{label}: {error:?}"
                );
                assert!(
                    error.message.contains("workspace"),
                    "the refusal must name the missing workspace: {}",
                    error.message
                );
            }
            other => panic!("{label} without a workspace must be refused: {other:?}"),
        }
    }
}
