use super::*;

use crate::mcp_broker::caller::McpCaller;
use devboule_protocol::SessionKind;

fn audit_rows(state: &ServerState) -> Vec<(String, String)> {
    let connection = rusqlite::Connection::open(state.sessions.runtime_dir().join("journal.db"))
        .expect("journal db");
    let mut statement = connection
        .prepare("SELECT action, outcome FROM audit ORDER BY id")
        .expect("audit query");
    statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .expect("audit rows")
        .collect::<Result<Vec<_>, _>>()
        .expect("read audit rows")
}

fn peer_caller() -> McpCaller {
    McpCaller::Peer {
        scope: crate::peer_policy::PeerScope::PairedUser,
        device_id: "audit-test-peer".to_string(),
        caps: Vec::new(),
    }
}

#[test]
fn partial_archive_audits_refusal_without_workspace_archive_close_rows() {
    let (state, project, _own, _dir) = setup("archive-audit-partial");
    let (target, _) = add_worktree(&state, &project, "archive-audit-partial");
    crate::session::insert_test_live_session_in_workspace(
        &state.sessions,
        "archive-audit-partial-a",
        owner(),
        SessionKind::Acp,
        &target,
    );
    crate::session::insert_test_live_session_in_workspace(
        &state.sessions,
        "invalid session id",
        owner(),
        SessionKind::Acp,
        &target,
    );
    let call_state = Arc::clone(&state);
    let workspace_id = target.clone();
    let archive = std::thread::spawn(move || {
        let caller = peer_caller();
        let result = super::super::super::archive_workspace_audited(
            &call_state,
            &call_state.mcp,
            Some(&caller),
            "archive-audit-partial",
            &owner(),
            &workspace_id,
        );
        if let Err(error) = &result {
            super::super::super::audit_archive(
                &call_state,
                &caller,
                "archive-audit-partial",
                super::super::super::archive_audit_outcome(error),
            );
        }
        result
    });
    let broker = state
        .sessions
        .live_runtime("archive-audit-partial", &owner())
        .expect("caller runtime")
        .permission_broker()
        .expect("permission broker");
    let card_id = pending_card(&state, "archive-audit-partial");
    broker
        .test_answer(&card_id, PermissionOutcome::AllowOnce, "once")
        .expect("approve archive");
    let result = archive.join().expect("archive call");
    assert!(matches!(
        result,
        Err(WorkspaceError::Refused(message))
            if message.contains("archive-audit-partial-a")
    ));

    let rows = audit_rows(&state);
    assert!(rows.contains(&(
        "devboule_archive_workspace".to_string(),
        "denied".to_string()
    )));
    assert!(!rows.contains(&("SessionClose".to_string(), "workspace_archive".to_string())));
}

/// A session starting in the workspace refuses the archive after its card
/// was approved; the audit must file that retryable refusal on its own,
/// not as a denial.
#[test]
fn an_archive_refused_by_a_starting_session_is_not_audited_as_denied() {
    let (state, project, _own, _dir) = setup("archive-audit-starting");
    let (target, path) = add_worktree(&state, &project, "archive-audit-starting");
    let starting = state
        .sessions
        .workspace_creation_guard(Some(&target))
        .expect("the starting create's gate check")
        .expect("the starting create holds the gate");
    let call_state = Arc::clone(&state);
    let workspace_id = target.clone();
    let archive = std::thread::spawn(move || {
        let caller = peer_caller();
        let result = super::super::super::archive_workspace_audited(
            &call_state,
            &call_state.mcp,
            Some(&caller),
            "archive-audit-starting",
            &owner(),
            &workspace_id,
        );
        if let Err(error) = &result {
            super::super::super::audit_archive(
                &call_state,
                &caller,
                "archive-audit-starting",
                super::super::super::archive_audit_outcome(error),
            );
        }
        result
    });
    let broker = state
        .sessions
        .live_runtime("archive-audit-starting", &owner())
        .expect("caller runtime")
        .permission_broker()
        .expect("permission broker");
    let card_id = pending_card(&state, "archive-audit-starting");
    broker
        .test_answer(&card_id, PermissionOutcome::AllowOnce, "once")
        .expect("approve archive");
    let result = archive.join().expect("archive call");
    drop(starting);
    assert!(matches!(
        result,
        Err(WorkspaceError::Refused(message))
            if message == crate::session::SESSION_STARTING_MESSAGE
    ));
    assert!(path.exists(), "a refused archive removes nothing");

    let rows = audit_rows(&state);
    assert!(rows.contains(&(
        "devboule_archive_workspace".to_string(),
        "session_starting".to_string()
    )));
    assert!(!rows.contains(&(
        "devboule_archive_workspace".to_string(),
        "denied".to_string()
    )));
}
