//! Archive marks: ownership, release after refusal, and mutual exclusion.

use devboule_protocol::WorkspaceIsolation;

use super::test_support::{temp_state, test_owner, TestRepo};

/// A refused delete releases its hold on the archiving mark. A leaked mark
/// would refuse every later session create, resume and terminal open into
/// this workspace forever with "Workspace is being archived." — a silent
/// wedge with no self-healing path and no log line.
#[test]
fn a_refused_delete_releases_its_mark() {
    let repo = TestRepo::new("git-off-loop-delete-mark-release");
    let (path, state) = temp_state("git-off-loop-delete-mark-release-state");
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
    let owner = test_owner();
    crate::session::insert_test_live_session_in_workspace(
        &state.sessions,
        "mark-release-agent",
        owner,
        devboule_protocol::SessionKind::Acp,
        &workspace.id,
    );
    state
        .sessions
        .workspace_delete(&workspace.id, false)
        .expect_err("the live session refuses the delete");

    assert!(
        !state.sessions.workspace_is_marked_archiving(&workspace.id),
        "the refusal must release the mark"
    );
    assert!(
        state
            .sessions
            .workspace_creation_guard(Some(&workspace.id))
            .is_ok(),
        "session creation must not be refused after the release"
    );
    drop(state);
    let _ = std::fs::remove_dir_all(&path);
}

/// The archive's mark is not the delete's to remove: a delete running under
/// the archive flow's held mark finds it present, owns nothing, and its
/// reservation's Drop must leave the archive's mark exactly where it was.
#[test]
fn a_delete_under_the_archives_mark_leaves_the_mark_alone() {
    let repo = TestRepo::new("git-off-loop-delete-mark-owned");
    let (path, state) = temp_state("git-off-loop-delete-mark-owned-state");
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
    let owner = test_owner();
    crate::session::insert_test_live_session_in_workspace(
        &state.sessions,
        "mark-owned-agent",
        owner,
        devboule_protocol::SessionKind::Acp,
        &workspace.id,
    );
    let archiving = state
        .sessions
        .mark_workspace_archiving(&workspace.id)
        .expect("the archive holds the mark");
    state
        .sessions
        .workspace_delete(&workspace.id, false)
        .expect_err("the live session refuses the delete");

    assert!(
        state.sessions.workspace_is_marked_archiving(&workspace.id),
        "the delete's reservation must not remove the archive's mark"
    );
    drop(archiving);
    assert!(
        !state.sessions.workspace_is_marked_archiving(&workspace.id),
        "the archive's own guard is the one that removes it"
    );
    drop(state);
    let _ = std::fs::remove_dir_all(&path);
}

/// The delete's own mark is a real mark: while a delete holds it, the
/// archive road is refused instead of double-deleting, and it is gone the
/// moment the reservation drops.
#[test]
fn a_deletes_mark_refuses_the_archive_road_until_it_drops() {
    let repo = TestRepo::new("git-off-loop-delete-mark-serial");
    let (path, state) = temp_state("git-off-loop-delete-mark-serial-state");
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

    state
        .sessions
        .hold_workspace_delete_reservation(&workspace.id, || {
            assert!(
                state
                    .sessions
                    .mark_workspace_archiving(&workspace.id)
                    .is_err(),
                "the archive road must be refused while a delete holds the mark"
            );
            assert!(
                state.sessions.workspace_is_marked_archiving(&workspace.id),
                "the mark is held for the reservation's whole life"
            );
        });
    assert!(
        !state.sessions.workspace_is_marked_archiving(&workspace.id),
        "the reservation's drop releases the mark"
    );
    assert!(state
        .sessions
        .mark_workspace_archiving(&workspace.id)
        .is_ok());
    drop(state);
    let _ = std::fs::remove_dir_all(&path);
}
