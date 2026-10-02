//! Delete outcomes: failed removal, force retry, and live-session refusal.

use std::path::PathBuf;

use devboule_protocol::WorkspaceIsolation;

use super::test_support::{temp_state, test_owner, TestRepo};

/// The remove runs before the row delete, so a failed removal keeps the row
/// and stays retryable. A worktree whose git link was deleted by hand stands
/// in for every removal that fails: the identity checks pass, the remove
/// dies on validation, and the worktree stays listed so the force-path
/// recovery refuses it too — the failure returns and the row survives.
#[test]
fn a_failed_checkout_removal_keeps_the_row_retryable() {
    let repo = TestRepo::new("git-off-loop-delete-row");
    let (path, state) = temp_state("git-off-loop-delete-row-state");
    let project = state
        .sessions
        .project_add(repo.root.to_str().expect("repo path"))
        .expect("project");
    let workspace = state
        .sessions
        .workspace_create(
            &project.id,
            WorkspaceIsolation::Worktree,
            Some("branch-one".to_string()),
        )
        .expect("worktree workspace");
    let checkout = PathBuf::from(&workspace.path);
    assert!(checkout.is_dir(), "the create made a real checkout");
    std::fs::remove_file(checkout.join(".git")).expect("remove the worktree's git link");
    assert!(
        state
            .sessions
            .workspace_delete(&workspace.id, true)
            .is_err(),
        "removing a worktree without its git link must report its failure"
    );
    assert_eq!(
        state
            .sessions
            .workspaces_list(&project.id)
            .expect("workspaces list")
            .len(),
        1,
        "a failed removal must keep the row: the retry road stays open"
    );
    drop(state);
    let _ = std::fs::remove_dir_all(&path);
}

/// The advertised retry works: a checkout that went dirty between the
/// panel's refresh and the delete refuses the non-forced removal with the
/// force prompt, keeps its row — and the forced retry then removes both the
/// checkout and the row.
#[test]
fn a_dirty_worktree_refuses_then_force_retry_removes_it() {
    let repo = TestRepo::new("git-off-loop-force-retry");
    let (path, state) = temp_state("git-off-loop-force-retry-state");
    let project = state
        .sessions
        .project_add(repo.root.to_str().expect("repo path"))
        .expect("project");
    let workspace = state
        .sessions
        .workspace_create(
            &project.id,
            WorkspaceIsolation::Worktree,
            Some("branch-one".to_string()),
        )
        .expect("worktree workspace");
    let checkout = PathBuf::from(&workspace.path);
    std::fs::write(checkout.join("late.txt"), "written after create\n")
        .expect("dirty the checkout");
    match state.sessions.workspace_delete(&workspace.id, false) {
        Err(error) => assert!(
            error.details.is_some(),
            "the non-forced refusal must carry the force prompt: {error:?}"
        ),
        Ok(()) => panic!("a dirty checkout must refuse the non-forced removal"),
    }
    assert_eq!(
        state
            .sessions
            .workspaces_list(&project.id)
            .expect("workspaces list")
            .len(),
        1,
        "the refusal keeps the row"
    );
    assert!(state.sessions.workspace_delete(&workspace.id, true).is_ok());
    assert!(
        state
            .sessions
            .workspaces_list(&project.id)
            .expect("workspaces list")
            .is_empty(),
        "the forced retry deletes the row"
    );
    assert!(!checkout.exists(), "the forced retry deletes the checkout");
    drop(state);
    let _ = std::fs::remove_dir_all(&path);
}

/// A live session refuses the delete where it executes — this direct call is
/// the same registry road the queue's worker reaches, and the gate has to sit
/// there, not at dispatch where the queue's delay would make any answer
/// stale. The refusal is pathless, keeps the checkout and the row, and the
/// same delete succeeds once the session is closed.
#[test]
fn a_live_session_refuses_the_delete_until_it_is_closed() {
    let repo = TestRepo::new("git-off-loop-delete-live-session");
    let (path, state) = temp_state("git-off-loop-delete-live-session-state");
    let project = state
        .sessions
        .project_add(repo.root.to_str().expect("repo path"))
        .expect("project");
    let workspace = state
        .sessions
        .workspace_create(
            &project.id,
            WorkspaceIsolation::Worktree,
            Some("branch-one".to_string()),
        )
        .expect("worktree workspace");
    let checkout = PathBuf::from(&workspace.path);
    let owner = test_owner();
    crate::session::insert_test_live_session_in_workspace(
        &state.sessions,
        "delete-live-agent",
        owner.clone(),
        devboule_protocol::SessionKind::Acp,
        &workspace.id,
    );

    let error = state
        .sessions
        .workspace_delete(&workspace.id, false)
        .expect_err("a live session must refuse the delete");
    assert!(
        error.message.contains("close them first"),
        "the refusal says what to do: {error:?}"
    );
    assert!(
        !error.message.contains('\\') && !error.message.contains('/'),
        "the refusal names no path: {error:?}"
    );
    assert!(checkout.is_dir(), "the refused delete keeps the checkout");
    assert_eq!(
        state
            .sessions
            .workspaces_list(&project.id)
            .expect("workspaces list")
            .len(),
        1,
        "the refused delete keeps the row"
    );

    assert!(
        state
            .sessions
            .close("delete-live-agent", &owner, &None)
            .expect("close the live session"),
        "the inserted session was live and is now closed"
    );
    state
        .sessions
        .workspace_delete(&workspace.id, false)
        .expect("the delete succeeds once no session is live");
    assert!(!checkout.exists(), "the delete removed the checkout");
    drop(state);
    let _ = std::fs::remove_dir_all(&path);
}

/// The app turns this refusal into its own sentence by matching the daemon's
/// exact wording (`src/lib/errorSentence.ts`); the wire has no kind for it.
#[test]
fn the_live_session_refusal_wording_is_the_one_the_app_maps() {
    let repo = TestRepo::new("git-off-loop-delete-live-wording");
    let (path, state) = temp_state("git-off-loop-delete-live-wording-state");
    let project = state
        .sessions
        .project_add(repo.root.to_str().expect("repo path"))
        .expect("project");
    let workspace = state
        .sessions
        .workspace_create(
            &project.id,
            WorkspaceIsolation::Worktree,
            Some("branch-one".to_string()),
        )
        .expect("worktree workspace");
    crate::session::insert_test_live_session_in_workspace(
        &state.sessions,
        "delete-wording-agent",
        test_owner(),
        devboule_protocol::SessionKind::Acp,
        &workspace.id,
    );

    let error = state
        .sessions
        .workspace_delete(&workspace.id, false)
        .expect_err("a live session must refuse the delete");
    assert_eq!(
        error.message,
        "sessions or terminals are still running in this workspace; close them first",
        "the app maps this exact text in src/lib/errorSentence.ts; change both together"
    );
    drop(state);
    let _ = std::fs::remove_dir_all(&path);
}
