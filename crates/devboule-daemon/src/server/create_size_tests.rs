//! The create frame's optional geometry at the wire boundary: which pairs
//! count as an ask, and that a framed ask reaches the PTY it names.

use std::collections::VecDeque;
use std::sync::Arc;

use devboule_protocol::{ClientMessage, DaemonMessage, OwnerId, SessionKind, WorkspaceIsolation};

use super::dispatch;
use super::git_workers::test_support::wait_for_worker_reply;
use super::sessions::create_size;
use super::ServerState;
use crate::session::{test_terminal_geometry, ConnHandle};

/// A project and its workspace, added over the same wire the create uses: a
/// session names a workspace or is refused, so the create needs one first.
fn scratch_workspace(state: &Arc<ServerState>, owner: &OwnerId, conn: &Arc<ConnHandle>) -> String {
    let folder = state.sessions.runtime_dir().join("Project");
    std::fs::create_dir_all(&folder).expect("project folder");
    let project = match wire_reply(
        state,
        owner,
        conn,
        ClientMessage::ProjectAdd {
            id: 2,
            path: folder.to_string_lossy().into_owned(),
        },
    ) {
        DaemonMessage::Project { project, .. } => project,
        other => panic!("the project must be added: {other:?}"),
    };
    match wire_reply(
        state,
        owner,
        conn,
        ClientMessage::WorkspaceCreate {
            id: 3,
            project_id: project.id,
            isolation: WorkspaceIsolation::Local,
            branch: None,
        },
    ) {
        DaemonMessage::Workspace { workspace, .. } => workspace.id,
        other => panic!("the workspace must be created: {other:?}"),
    }
}

fn wire_reply(
    state: &Arc<ServerState>,
    owner: &OwnerId,
    conn: &Arc<ConnHandle>,
    message: ClientMessage,
) -> DaemonMessage {
    let inline = dispatch(state, owner, message, conn, true, true, true, true);
    inline.unwrap_or_else(|| wait_for_worker_reply(conn, &mut VecDeque::new()))
}

/// Only a complete pair inside the bounds is a size. A column below the
/// screen's floor, zero, a half, an oversized axis, or an axis-legal pair
/// with too many cells all ask for nothing, which the spawn road answers
/// with the default — nothing is clamped into a grid the client never asked
/// for.
#[test]
fn create_size_judges_the_pair_once() {
    assert_eq!(create_size(Some(93), Some(28)), Some((93, 28)));
    assert_eq!(create_size(None, None), None);
    assert_eq!(create_size(None, Some(28)), None, "a half pair is no ask");
    assert_eq!(create_size(Some(93), None), None);
    assert_eq!(create_size(Some(0), Some(28)), None, "zero is no ask");
    assert_eq!(
        create_size(Some(1), Some(28)),
        None,
        "one column is below the screen's floor"
    );
    assert_eq!(create_size(Some(2), Some(1)), Some((2, 1)), "the floor");
    assert_eq!(create_size(Some(93), Some(0)), None);
    assert_eq!(create_size(Some(1_001), Some(28)), None, "oversized axis");
    assert_eq!(create_size(Some(93), Some(1_001)), None);
    assert_eq!(create_size(Some(1_000), Some(251)), None, "too many cells");
    assert_eq!(create_size(Some(1_000), Some(250)), Some((1_000, 250)));
}

/// Mutant: `dispatch_session` handing the spawn `None` instead of the
/// judged pair — the terminal opens at 120×32 and this fails.
#[test]
fn a_framed_create_size_reaches_the_pty() {
    let state = ServerState::new("create-size-wire".to_string());
    let owner = OwnerId::new("S-1-5-21-create-size-wire", "create-size-client").expect("owner");
    let conn = ConnHandle::new(1);
    let workspace_id = scratch_workspace(&state, &owner, &conn);
    let inline = dispatch(
        &state,
        &owner,
        ClientMessage::SessionCreate {
            id: 1,
            workspace_id: Some(workspace_id),
            kind: SessionKind::Terminal,
            provider: None,
            mode: None,
            display_name: None,
            idempotency_key: None,
            cols: Some(93),
            rows: Some(28),
        },
        &conn,
        true,
        true,
        true,
        true,
    );
    assert!(inline.is_none(), "a create answers from its worker");
    let mut reply_backlog = VecDeque::new();
    let reply = wait_for_worker_reply(&conn, &mut reply_backlog);
    let session = match reply {
        DaemonMessage::Session { session, .. } => session,
        other => panic!("the create must answer a session: {other:?}"),
    };
    let (pty, screen) = test_terminal_geometry(&state.sessions, &session.id);
    assert_eq!(pty, (93, 28), "the PTY at the framed ask");
    assert_eq!(screen, (93, 28), "the screen born at the same grid");
    let _ = state.sessions.close(&session.id, &owner, &None);
}
