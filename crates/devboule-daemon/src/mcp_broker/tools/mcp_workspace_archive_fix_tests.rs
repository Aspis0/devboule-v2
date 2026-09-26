use super::*;

use crate::mcp_broker::tools::first_use::{
    ensure_write_allowed, WORKSPACES_GROUP, WORKSPACE_ARCHIVE_GROUP,
};
use devboule_protocol::{PermissionOutcome, SessionEvent, SessionKind};
use std::time::{Duration, Instant};

fn archive_with_card_action(
    state: &Arc<ServerState>,
    session: &str,
    workspace: &str,
    action: impl FnOnce(),
) -> Result<serde_json::Value, WorkspaceError> {
    let call_state = Arc::clone(state);
    let session_id = session.to_string();
    let workspace_id = workspace.to_string();
    let call = std::thread::spawn(move || {
        archive_workspace(
            &call_state,
            &call_state.mcp,
            &session_id,
            &owner(),
            &workspace_id,
        )
    });
    let broker = state
        .sessions
        .live_runtime(session, &owner())
        .expect("session runtime")
        .permission_broker()
        .expect("permission broker");
    let card_id = pending_card(state, session);
    let request = broker.test_pending_request(&card_id).expect("card request");
    let SessionEvent::PermissionRequest { description, .. } = request else {
        panic!("archive asks for permission");
    };
    assert!(description
        .as_deref()
        .is_some_and(|text| text.contains("Allow workspace archiving for this session")));
    action();
    broker
        .test_answer(&card_id, PermissionOutcome::AllowOnce, "once")
        .expect("answer card");
    call.join().expect("archive call")
}

fn pending_card(state: &Arc<ServerState>, session: &str) -> String {
    let runtime = state
        .sessions
        .live_runtime(session, &owner())
        .expect("session runtime");
    let broker = runtime.permission_broker().expect("permission broker");
    let start = Instant::now();
    loop {
        if let Some(id) = broker.test_pending_ids().pop() {
            return id;
        }
        assert!(start.elapsed() < Duration::from_secs(10), "card not raised");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn archive_closes_agents_and_terminals_before_removing_workspace() {
    let (state, project, _own, _dir) = setup("cascade");
    let (target, path) = add_worktree(&state, &project, "archive-cascade");
    crate::session::insert_test_live_session_in_workspace(
        &state.sessions,
        "archive-cascade-agent",
        owner(),
        SessionKind::Acp,
        &target,
    );
    crate::session::insert_test_live_session_in_workspace(
        &state.sessions,
        "archive-cascade-terminal",
        owner(),
        SessionKind::Terminal,
        &target,
    );
    for id in ["archive-cascade-agent", "archive-cascade-terminal"] {
        assert!(state.sessions.session_journal_has_record(id));
    }
    let result = run_with_answer(
        &state,
        "cascade",
        &target,
        PermissionOutcome::AllowOnce,
        "once",
    )
    .expect("archive succeeds");
    assert_eq!(result["closedSessionIds"].as_array().unwrap().len(), 2);
    assert!(!path.exists());
    assert!(state
        .sessions
        .live_sessions_in_workspace(&target)
        .unwrap()
        .is_empty());
    for id in ["archive-cascade-agent", "archive-cascade-terminal"] {
        assert!(state.sessions.session_journal_has_record(id));
    }
}

#[test]
fn failed_session_close_keeps_the_checkout() {
    let (state, project, _own, _dir) = setup("close-fail");
    let (target, path) = add_worktree(&state, &project, "archive-close-fail");
    crate::session::insert_test_live_session_in_workspace(
        &state.sessions,
        "invalid session id",
        owner(),
        SessionKind::Acp,
        &target,
    );
    let result = run_with_answer(
        &state,
        "close-fail",
        &target,
        PermissionOutcome::AllowOnce,
        "once",
    );
    assert!(
        matches!(result, Err(WorkspaceError::Refused(message)) if message.contains("Could not close session"))
    );
    assert!(path.is_dir());
}

#[test]
fn archive_card_names_sessions_and_returns_the_closed_ids() {
    let (state, project, _own, _dir) = setup("card-names");
    let (target, _) = add_worktree(&state, &project, "archive-card-names");
    crate::session::insert_test_live_session_in_workspace(
        &state.sessions,
        "archive-card-agent",
        owner(),
        SessionKind::Acp,
        &target,
    );
    let result = run_with_answer(
        &state,
        "card-names",
        &target,
        PermissionOutcome::AllowOnce,
        "once",
    )
    .expect("archive succeeds");
    assert_eq!(result["closedSessionIds"][0], "archive-card-agent");
}

#[test]
fn local_refusal_does_not_raise_a_card_or_open_a_gate() {
    let (state, project, local, _dir) = setup("local-no-write");
    let (caller_workspace, _) = add_worktree(&state, &project, "archive-local-no-write-caller");
    crate::session::insert_test_live_agent_in_workspace(
        &state.sessions,
        "local-no-write-caller",
        owner(),
        &caller_workspace,
    );
    let runtime = state
        .sessions
        .live_runtime("local-no-write-caller", &owner())
        .expect("caller runtime");
    let permission_broker = runtime.permission_broker().expect("permission broker");
    let delete_calls = state.sessions.workspace_delete_call_count();
    assert!(matches!(
        archive_workspace(
            &state,
            &state.mcp,
            "local-no-write-caller",
            &owner(),
            &local
        ),
        Err(WorkspaceError::Refused(_))
    ));
    assert!(permission_broker.test_pending_ids().is_empty());
    assert_eq!(
        state.sessions.workspace_delete_call_count(),
        delete_calls,
        "local refusal must not reach the delete path"
    );
    assert_eq!(
        state
            .mcp
            .first_use_mark("local-no-write-caller", WORKSPACE_ARCHIVE_GROUP),
        None
    );
}

#[test]
fn workspace_create_and_archive_session_approvals_are_independent() {
    let (state, project, _own, _dir) = setup("separate-gates");
    let (target, _) = add_worktree(&state, &project, "archive-separate-gates");
    let create_state = Arc::clone(&state);
    let create = std::thread::spawn(move || {
        ensure_write_allowed(
            &create_state,
            &create_state.mcp,
            "separate-gates",
            &owner(),
            WORKSPACES_GROUP,
            "create workspace",
            &[],
        )
    });
    answer_group_card(&state, "separate-gates", "session");
    create.join().unwrap().unwrap();
    assert_eq!(
        state.mcp.first_use_mark("separate-gates", WORKSPACES_GROUP),
        Some(crate::mcp_broker::tools::first_use::GateMark::Open)
    );
    assert_eq!(
        state
            .mcp
            .first_use_mark("separate-gates", WORKSPACE_ARCHIVE_GROUP),
        None
    );

    assert!(archive_with_card_action(&state, "separate-gates", &target, || {}).is_ok());
    assert_eq!(
        state
            .mcp
            .first_use_mark("separate-gates", WORKSPACE_ARCHIVE_GROUP),
        None
    );
}

#[test]
fn archive_session_approval_does_not_authorise_workspace_creation() {
    let (state, project, _own, _dir) = setup("archive-gate-only");
    let (target, _) = add_worktree(&state, &project, "archive-gate-only");
    let call_state = Arc::clone(&state);
    let target_id = target.clone();
    let archive = std::thread::spawn(move || {
        archive_workspace(
            &call_state,
            &call_state.mcp,
            "archive-gate-only",
            &owner(),
            &target_id,
        )
    });
    answer_group_card(&state, "archive-gate-only", "session");
    archive.join().unwrap().unwrap();
    assert_eq!(
        state
            .mcp
            .first_use_mark("archive-gate-only", WORKSPACE_ARCHIVE_GROUP),
        Some(crate::mcp_broker::tools::first_use::GateMark::Open)
    );
    assert_eq!(
        state
            .mcp
            .first_use_mark("archive-gate-only", WORKSPACES_GROUP),
        None
    );
    let create_state = Arc::clone(&state);
    let create = std::thread::spawn(move || {
        ensure_write_allowed(
            &create_state,
            &create_state.mcp,
            "archive-gate-only",
            &owner(),
            WORKSPACES_GROUP,
            "create workspace",
            &[],
        )
    });
    answer_group_card(&state, "archive-gate-only", "once");
    create.join().unwrap().unwrap();
    assert_eq!(
        state
            .mcp
            .first_use_mark("archive-gate-only", WORKSPACES_GROUP),
        None
    );
}

#[test]
fn changed_scope_after_consent_is_refused_before_archive_removal() {
    let (state, project, _own, _dir) = setup("scope-recheck");
    let (target, path) = add_worktree(&state, &project, "archive-scope-recheck");
    let result = archive_with_card_action(&state, "scope-recheck", &target, || {
        state
            .sessions
            .set_live_workspace_for_test("scope-recheck", "changed-project-workspace");
    });
    assert!(
        matches!(result, Err(WorkspaceError::Refused(message)) if message.contains("gone from the journal"))
    );
    assert!(path.is_dir());
}

fn answer_group_card(state: &Arc<ServerState>, session: &str, choice: &str) {
    let runtime = state
        .sessions
        .live_runtime(session, &owner())
        .expect("session runtime");
    let broker = runtime.permission_broker().expect("permission broker");
    let card_id = pending_card(state, session);
    broker
        .test_answer(&card_id, PermissionOutcome::AllowOnce, choice)
        .expect("answer session gate");
}
