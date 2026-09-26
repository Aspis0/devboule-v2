use super::*;

use crate::server::ServerState;
use devboule_protocol::{OwnerId, PermissionOutcome, SessionEvent, WorkspaceIsolation};
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn owner() -> OwnerId {
    OwnerId::new("S-1-5-21-workspace-archive", "archive-client").expect("owner")
}

fn add_git_project(state: &ServerState, root: &std::path::Path) -> String {
    std::fs::create_dir_all(root).expect("project folder");
    let git = |args: &[&str]| {
        let status = std::process::Command::new("git")
            .args(args)
            .current_dir(root)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_AUTHOR_NAME", "test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?}");
    };
    git(&["init"]);
    std::fs::write(root.join("seed.txt"), "seed").expect("seed file");
    git(&["add", "seed.txt"]);
    git(&["commit", "-m", "seed"]);
    state
        .sessions
        .project_add(root.to_str().expect("project path"))
        .expect("project row")
        .id
}

fn setup(tag: &str) -> (Arc<ServerState>, String, String, std::path::PathBuf) {
    let state = ServerState::new(format!("archive-{tag}"));
    let dir = crate::test_dirs::test_temp_dir(&format!("devboule-archive-{tag}"));
    let root = dir.join("project");
    let project = add_git_project(&state, &root);
    let local = state
        .sessions
        .workspace_create(&project, WorkspaceIsolation::Local, None)
        .expect("local row");
    crate::session::insert_test_live_agent_in_workspace(&state.sessions, tag, owner(), &local.id);
    (state, project, local.id, dir)
}

fn add_worktree(state: &ServerState, project: &str, branch: &str) -> (String, std::path::PathBuf) {
    let workspace = state
        .sessions
        .workspace_create(
            project,
            WorkspaceIsolation::Worktree,
            Some(branch.to_string()),
        )
        .expect("worktree row");
    let path = std::path::PathBuf::from(&workspace.path);
    (workspace.id, path)
}

fn run_with_answer(
    state: &Arc<ServerState>,
    session: &str,
    workspace: &str,
    outcome: PermissionOutcome,
    choice: &str,
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
    let start = Instant::now();
    let card_id = loop {
        let mut ids = broker.test_pending_ids();
        if let Some(id) = ids.pop() {
            break id;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "archive card not raised"
        );
        std::thread::sleep(Duration::from_millis(10));
    };
    let request = broker.test_pending_request(&card_id).expect("pending card");
    let SessionEvent::PermissionRequest {
        title, description, ..
    } = request
    else {
        panic!("the first-use gate raises a permission request");
    };
    assert!(title.contains("archive workspace"), "{title}");
    let description = description.expect("card description");
    assert!(
        description.contains("permission to archive workspace"),
        "{description}"
    );
    assert!(description.contains("| path:"), "{description}");
    broker
        .test_answer(&card_id, outcome, choice)
        .expect("answer archive card");
    call.join().expect("archive call")
}

#[test]
fn archive_refuses_another_projects_workspace_and_the_callers_own() {
    let (state, project, own, _dir) = setup("scope");
    let other_dir = crate::test_dirs::test_temp_dir("devboule-archive-other-project");
    let other_root = other_dir.join("project");
    let other_project = add_git_project(&state, &other_root);
    let (other_workspace, other_path) =
        add_worktree(&state, &other_project, "archive-other-project");
    let (target, _) = add_worktree(&state, &project, "archive-own-project");

    for workspace in [&other_workspace, &own] {
        let result = archive_workspace(&state, &state.mcp, "scope", &owner(), workspace);
        assert!(matches!(result, Err(WorkspaceError::Refused(_))));
    }
    assert!(other_path.is_dir());
    assert!(state
        .sessions
        .workspace_records(&project)
        .unwrap()
        .iter()
        .any(|w| w.id == target));
}

#[test]
fn archive_refuses_local_workspace_with_daemon_sentence() {
    let (state, project, local, _dir) = setup("local");
    let (caller_workspace, _) = add_worktree(&state, &project, "archive-local-caller");
    crate::session::insert_test_live_agent_in_workspace(
        &state.sessions,
        "local-worktree-caller",
        owner(),
        &caller_workspace,
    );
    let result = archive_workspace(
        &state,
        &state.mcp,
        "local-worktree-caller",
        &owner(),
        &local,
    );
    let Err(WorkspaceError::Refused(message)) = result else {
        panic!("local workspace must be refused");
    };
    assert_eq!(
        message,
        "The local workspace is the project folder and is not removed as a worktree."
    );
}

#[test]
fn archive_rejects_force_as_unknown() {
    let result = ArchiveRequest::parse(&json!({"workspaceId":"w", "force":true}));
    assert!(
        matches!(result, Err(WorkspaceError::Invalid(message)) if message == "unknown parameter 'force'")
    );
}

#[test]
fn archive_dirty_worktree_is_refused_and_kept() {
    let (state, project, _own, _dir) = setup("dirty");
    let (target, path) = add_worktree(&state, &project, "archive-dirty");
    std::fs::write(path.join("uncommitted.txt"), "keep me").expect("dirty file");
    let result = run_with_answer(
        &state,
        "dirty",
        &target,
        PermissionOutcome::AllowOnce,
        "once",
    );
    assert!(matches!(result, Err(WorkspaceError::Refused(message)) if message.contains("dirty")));
    assert!(path.is_dir());
    assert!(state
        .sessions
        .workspace_records(&project)
        .unwrap()
        .iter()
        .any(|w| w.id == target));
}

#[test]
fn archive_allows_once_then_asks_again_and_removes_worktrees() {
    let (state, project, _own, _dir) = setup("once");
    let (first, first_path) = add_worktree(&state, &project, "archive-once-first");
    let (second, second_path) = add_worktree(&state, &project, "archive-once-second");
    assert!(run_with_answer(&state, "once", &first, PermissionOutcome::AllowOnce, "once").is_ok());
    assert!(!first_path.exists());
    assert!(run_with_answer(
        &state,
        "once",
        &second,
        PermissionOutcome::AllowOnce,
        "once"
    )
    .is_ok());
    assert!(!second_path.exists());
    let records = state.sessions.workspace_records(&project).unwrap();
    assert!(!records.iter().any(|w| w.id == first || w.id == second));
}

#[test]
fn archive_session_approval_covers_next_workspace_and_deny_keeps_target() {
    let (state, project, _own, _dir) = setup("session");
    let (first, first_path) = add_worktree(&state, &project, "archive-session-first");
    let (second, second_path) = add_worktree(&state, &project, "archive-session-second");
    assert!(run_with_answer(
        &state,
        "session",
        &first,
        PermissionOutcome::AllowOnce,
        "session"
    )
    .is_ok());
    assert!(!first_path.exists());
    assert!(archive_workspace(&state, &state.mcp, "session", &owner(), &second).is_ok());
    assert!(!second_path.exists());

    let (denied_state, denied_project, _denied_own, _denied_dir) = setup("deny");
    let (denied_target, denied_path) =
        add_worktree(&denied_state, &denied_project, "archive-denied");
    assert!(matches!(
        run_with_answer(&denied_state, "deny", &denied_target, PermissionOutcome::Deny, "deny"),
        Err(WorkspaceError::Refused(message)) if message == "permission refused"
    ));
    assert!(denied_path.is_dir());
    assert!(denied_state
        .sessions
        .workspace_records(&denied_project)
        .unwrap()
        .iter()
        .any(|w| w.id == denied_target));
}

#[test]
fn archive_tool_is_hidden_from_design_and_peer_door_requires_admin() {
    use crate::provider_catalog::{ToolOverlay, MCP_ARCHIVE_WORKSPACE_TOOL};
    assert!(!ToolOverlay::DESIGN.allows(MCP_ARCHIVE_WORKSPACE_TOOL));
    for role in [
        devboule_protocol::PeerRole::Client,
        devboule_protocol::PeerRole::Daemon,
    ] {
        assert_eq!(
            crate::peer_policy::mcp_tool_denial(role, &[], MCP_ARCHIVE_WORKSPACE_TOOL),
            Some(crate::peer_policy::CAP_ADMIN)
        );
        assert_eq!(
            crate::peer_policy::mcp_tool_denial(
                role,
                &[crate::peer_policy::CAP_ADMIN.to_string()],
                MCP_ARCHIVE_WORKSPACE_TOOL
            ),
            None
        );
    }
    let wire = crate::peer_policy::mcp_tool_wire(MCP_ARCHIVE_WORKSPACE_TOOL).expect("peer arm");
    let crate::peer_policy::McpToolWire::Judged(frames) = wire else {
        panic!("archive maps to a judged wire frame");
    };
    assert!(matches!(
        frames.as_slice(),
        [devboule_protocol::ClientMessage::WorkspaceDelete { force: false, .. }]
    ));
}
