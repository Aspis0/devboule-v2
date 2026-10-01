//! The workspace creation gate: a session create parks by being counted,
//! never by holding a lock, so a create starting into one workspace must
//! not block a delete of another workspace, nor a create into a third,
//! and a delete or archive of the parked create's own workspace must refuse
//! instead of waiting. The parked create here holds the create road's own
//! guard rather than spawning a real session; how long the real road holds
//! it is `session_workspace_gate_road_tests`'s claim.

use std::sync::Arc;
use std::time::Duration;

use super::tests::tmp_delete_registry;
use super::*;

/// A registry whose journal holds `count` worktree workspaces on one
/// project. The project folder existed when it was added and is removed
/// here, so every delete below takes the detach road: no git, no real
/// checkout, only the gate and the journal.
fn registry_with_vanished_worktrees(
    count: usize,
) -> (
    std::path::PathBuf,
    Arc<SessionRegistry>,
    Arc<Journal>,
    Vec<String>,
) {
    let (dir, registry, journal) = tmp_delete_registry();
    let project_path = dir.join("Project");
    std::fs::create_dir(&project_path).expect("project folder");
    let project = crate::workspace::project_record(
        project_path.to_str().expect("project path is valid UTF-8"),
    )
    .expect("project record");
    let project = journal.project_add(project).expect("persist project");
    let ids = (0..count)
        .map(|index| {
            journal
                .workspace_create(crate::workspace::worktree_workspace_record(
                    &project,
                    &dir.join(format!("checkout-{index}")),
                    "main",
                ))
                .expect("persist worktree workspace")
                .id
        })
        .collect();
    std::fs::remove_dir(&project_path).expect("remove project folder");
    (dir, Arc::new(registry), journal, ids)
}

/// A create parked into one workspace must not stall a delete of a
/// different workspace: the delete returns — and succeeds — within 1 s of
/// the park, instead of waiting for the create to finish.
#[test]
fn a_delete_of_another_workspace_returns_while_a_create_is_parked() {
    let (dir, registry, journal, ids) = registry_with_vanished_worktrees(2);
    let parked = registry
        .workspace_creation_guard(Some(&ids[0]))
        .expect("the parked create's gate check")
        .expect("the parked create holds the gate");

    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let registry_for_delete = Arc::clone(&registry);
    let other = ids[1].clone();
    let delete = std::thread::spawn(move || {
        let _ = done_tx.send(registry_for_delete.workspace_delete(&other, false));
    });
    let result = done_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("the delete of another workspace must return within 1s of a parked create");
    result.expect("the delete of another workspace must succeed");
    drop(parked);
    delete.join().expect("delete thread");
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// One pending delete must not stall a *new* create anywhere: on the
/// read-lock gate the delete's write permit queued behind the parked
/// create's read hold, and a fresh reader queued behind the waiting
/// writer. With per-workspace bookkeeping the third create and the
/// pending delete are strangers.
#[test]
fn a_create_into_a_third_workspace_completes_while_a_delete_is_pending() {
    let (dir, registry, journal, ids) = registry_with_vanished_worktrees(3);

    let (parked_tx, parked_rx) = std::sync::mpsc::channel();
    let (release_parked_tx, release_parked_rx) = std::sync::mpsc::channel();
    let registry_for_parked = Arc::clone(&registry);
    let parked_workspace = ids[0].clone();
    let parked = std::thread::spawn(move || {
        let _guard = registry_for_parked
            .workspace_creation_guard(Some(&parked_workspace))
            .expect("the parked create's gate check")
            .expect("the parked create holds the gate");
        let _ = parked_tx.send(());
        let _ = release_parked_rx.recv();
    });
    parked_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("the parked create parks");

    let (delete_holding_tx, delete_holding_rx) = std::sync::mpsc::channel();
    let (release_delete_tx, release_delete_rx) = std::sync::mpsc::channel();
    let registry_for_delete = Arc::clone(&registry);
    let delete_workspace = ids[1].clone();
    let delete = std::thread::spawn(move || {
        registry_for_delete.hold_workspace_delete_reservation(&delete_workspace, || {
            let _ = delete_holding_tx.send(());
            let _ = release_delete_rx.recv();
        });
    });
    // On the fixed gate the reservation holds at once; on the read-lock
    // gate it waits behind the parked create. Either way the wait only
    // gives the delete time to reach the gate before the third create is
    // asked to proceed.
    let _ = delete_holding_rx.recv_timeout(Duration::from_secs(2));
    std::thread::sleep(Duration::from_millis(100));

    let (third_tx, third_rx) = std::sync::mpsc::channel();
    let registry_for_third = Arc::clone(&registry);
    let third_workspace = ids[2].clone();
    let third = std::thread::spawn(move || {
        let _guard = registry_for_third
            .workspace_creation_guard(Some(&third_workspace))
            .expect("the third create's gate check")
            .expect("the third create holds the gate");
        let _ = third_tx.send(());
    });
    third_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("a create into a third workspace must not wait behind a pending delete");
    third.join().expect("third create thread");

    let _ = release_delete_tx.send(());
    delete.join().expect("delete thread");
    let _ = release_parked_tx.send(());
    parked.join().expect("parked thread");
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A delete of the parked create's own workspace must refuse promptly —
/// a session is starting there; try again — instead of waiting out the
/// create, and a refused delete must leave no mark behind: creation and
/// deletion both work once the parked create is gone.
#[test]
fn a_delete_of_the_parking_workspace_refuses_instead_of_waiting() {
    let (dir, registry, journal, ids) = registry_with_vanished_worktrees(1);
    let workspace = ids[0].clone();
    let parked = registry
        .workspace_creation_guard(Some(&workspace))
        .expect("the parked create's gate check")
        .expect("the parked create holds the gate");

    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let registry_for_delete = Arc::clone(&registry);
    let delete_workspace = workspace.clone();
    let delete = std::thread::spawn(move || {
        let _ = done_tx.send(registry_for_delete.workspace_delete(&delete_workspace, false));
    });
    let result = done_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("the delete must refuse within 1s, not wait for the parked create");
    let error = result.expect_err("the delete must refuse while a create is starting");
    assert!(
        error.message.contains("starting") && error.message.contains("try again"),
        "the refusal names the starting session: {}",
        error.message
    );
    assert!(
        !registry.workspace_is_marked_archiving(&workspace),
        "a refused delete leaves no archiving mark behind"
    );

    drop(parked);
    assert!(
        registry
            .workspace_creation_guard(Some(&workspace))
            .expect("the retry create's gate check")
            .is_some(),
        "creation is allowed again once the parked create is gone"
    );
    assert!(
        registry.workspace_delete(&workspace, false).is_ok(),
        "the delete completes once no create is starting"
    );
    delete.join().expect("delete thread");
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The archive road's twin of the refusal above: a mark asked for while a
/// create is starting refuses, and leaves no mark behind to refuse the next
/// create.
#[test]
fn an_archive_mark_refuses_while_a_create_is_starting_and_leaves_no_mark() {
    let (dir, registry, journal) = tmp_delete_registry();
    let workspace = "w.gate-archive";
    let parked = registry
        .workspace_creation_guard(Some(workspace))
        .expect("the parked create's gate check")
        .expect("the parked create holds the gate");

    let Err(error) = registry.mark_workspace_archiving(workspace) else {
        panic!("the archive mark must refuse while a create is starting");
    };
    assert_eq!(error.message, SESSION_STARTING_MESSAGE);
    assert!(
        !registry.workspace_is_marked_archiving(workspace),
        "a refused archive leaves no archiving mark behind"
    );

    drop(parked);
    let archiving = registry
        .mark_workspace_archiving(workspace)
        .expect("the mark takes once no create is starting");
    drop(archiving);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The same invariant from the mark's side: while a delete or an archive
/// holds the mark, a create is refused without being counted, so the
/// refused create cannot turn the next delete or archive away.
#[test]
fn a_marked_workspace_refuses_a_create_without_counting_it() {
    let (dir, registry, journal) = tmp_delete_registry();
    let workspace = "w.gate-marked";
    registry.hold_workspace_delete_reservation(workspace, || {
        let Err(error) = registry.workspace_creation_guard(Some(workspace)) else {
            panic!("a create into a workspace being deleted must refuse");
        };
        assert_eq!(error.message, "Workspace is being archived.");
    });
    let archiving = registry
        .mark_workspace_archiving(workspace)
        .expect("a create refused under the delete's mark left no count");
    let Err(error) = registry.workspace_creation_guard(Some(workspace)) else {
        panic!("a create into a workspace being archived must refuse");
    };
    assert_eq!(error.message, "Workspace is being archived.");
    drop(archiving);
    registry.hold_workspace_delete_reservation(workspace, || {});
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
