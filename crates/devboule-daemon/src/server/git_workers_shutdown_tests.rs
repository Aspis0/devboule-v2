//! Shutdown behavior: the accepted arm drains its queued work — a write or
//! a delete — into the journal before the close; the refused arm leaves
//! the journal untouched.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use devboule_protocol::{ClientMessage, WorkspaceIsolation};

use super::super::{dispatch, ConnHandle, DaemonMessage};
use super::test_support::{
    arm_git_gate, spawn_collector, state_with_workspace, temp_state, test_owner, ReplyStore,
    TestRepo,
};

/// The real Shutdown, end to end: a commit parked behind a held read must
/// still RUN — its drain pops it after the shutdown flag is already up —
/// and the shutdown-path drain (the one `lifecycle` runs before its
/// flush) must not return until it has. The Shutdown arm itself must not
/// block the connection loop: its reply arrives while the read is still
/// held in the git runner.
#[test]
fn a_real_shutdown_drains_the_queued_write() {
    let repo = TestRepo::new("git-off-loop-shutdown-drain");
    repo.write("b.txt", "second\n");
    repo.run(&["add", "b.txt"]);
    let (path, state, workspace_id) =
        state_with_workspace("git-off-loop-shutdown-drain-state", &repo.root);
    let (entered_rx, release_tx) = arm_git_gate(&state, &workspace_id);
    let owner = test_owner();
    let conn = ConnHandle::new(106);
    let store = ReplyStore::new();
    let (stop, collector) = spawn_collector(&conn, store.clone());

    // A read starts and is held inside `git status`; a commit queues behind it.
    assert!(dispatch(
        &state,
        &owner,
        ClientMessage::WorkspaceGitStatus {
            id: 1,
            workspace_id: workspace_id.clone(),
        },
        &conn,
        true,
        true,
        true,
        true,
    )
    .is_none());
    entered_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("the read's git command was held");
    assert!(dispatch(
        &state,
        &owner,
        ClientMessage::WorkspaceGitCommit {
            id: 2,
            workspace_id: workspace_id.clone(),
            message: "shutdown".to_string(),
            idempotency_key: None,
        },
        &conn,
        true,
        true,
        true,
        true,
    )
    .is_none());

    // The real Shutdown: accepted and answered at once — nothing drains on
    // the loop thread. The reply rides its own channel so a drain hiding in
    // the arm shows up as this receive's timeout.
    let (tx, rx) = std::sync::mpsc::channel();
    let shutdown_state = Arc::clone(&state);
    let shutdown_conn = Arc::clone(&conn);
    let shutdown_owner = owner.clone();
    std::thread::spawn(move || {
        let reply = dispatch(
            &shutdown_state,
            &shutdown_owner,
            ClientMessage::Shutdown { id: 99 },
            &shutdown_conn,
            true,
            true,
            true,
            true,
        );
        let _ = tx.send(reply);
    });
    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Some(DaemonMessage::Shutdown { accepted: true, .. })) => {}
        other => panic!("the Shutdown arm must answer without draining on the loop: {other:?}"),
    }

    // The shutdown-path drain must wait for the queued write.
    let drained = Arc::new(Mutex::new(false));
    let waiter_state = Arc::clone(&state);
    let waiter_flag = Arc::clone(&drained);
    let waiter = std::thread::spawn(move || {
        // The house budget, not production's 10 s — ~3× the overrun the
        // recallA2 gate measured: what this asserts is that the drain
        // finishes once the queued write has run, not how fast. This wait
        // is a model of the shutdown drain, not the shipped path, which
        // also cancels on failure and closes the journal.
        let finished_clean = waiter_state
            .git_jobs
            .wait_for_write_jobs(Duration::from_secs(30));
        *waiter_flag
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = finished_clean;
    });
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        !*drained.lock().unwrap_or_else(|error| error.into_inner()),
        "the drain returned while the write was still queued behind the held read"
    );
    release_tx.send(()).expect("release the held read");
    waiter.join().expect("drain thread");
    assert!(
        *drained.lock().unwrap_or_else(|error| error.into_inner()),
        "the drain must finish once the queued write has run"
    );

    store.wait_len(2);
    stop.store(true, Ordering::SeqCst);
    let _ = collector.join();
    assert_eq!(store.ids(), vec![1, 2]);
    match store.get(2) {
        DaemonMessage::WorkspaceGitWrite { error: None, .. } => {}
        other => {
            panic!("the queued write must run even though the shutdown flag is up, got {other:?}")
        }
    }
    assert!(
        repo.commit_subjects().contains("shutdown"),
        "the queued commit must really run"
    );
    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// The R1 guarantee, both halves: a `WorkspaceDelete` queued behind a held
/// read runs to its journal row when the real Shutdown arm and the shutdown
/// path run around it — the path being production's own
/// `drain_writes_and_close_journal` (drain, bounded, then the terminal
/// close), so the tested order is the shipped order. The arm only
/// checkpoints the journal — if it closed the writer first, the delete's
/// first journal RPC would fail with stopped and both the checkout and the
/// row would survive.
#[test]
fn a_shutdown_delete_lands_its_row_and_removes_its_checkout() {
    let repo = TestRepo::new("git-off-loop-shutdown-delete");
    let (path, state) = temp_state("git-off-loop-shutdown-delete-state");
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
    let workspace_id = workspace.id;
    let (entered_rx, release_tx) = arm_git_gate(&state, &workspace_id);
    let owner = test_owner();
    let conn = ConnHandle::new(109);

    // The read holds the root's drain; the delete queues behind it.
    assert!(dispatch(
        &state,
        &owner,
        ClientMessage::WorkspaceGitStatus {
            id: 1,
            workspace_id: workspace_id.clone(),
        },
        &conn,
        true,
        true,
        true,
        true,
    )
    .is_none());
    entered_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("the read's git command was held");
    assert!(dispatch(
        &state,
        &owner,
        ClientMessage::WorkspaceDelete {
            id: 2,
            workspace_id: workspace_id.clone(),
            force: false,
        },
        &conn,
        true,
        true,
        true,
        true,
    )
    .is_none());

    // The real Shutdown arm — accepted, checkpointing once, never closing.
    let (tx, rx) = std::sync::mpsc::channel();
    let shutdown_state = Arc::clone(&state);
    let shutdown_conn = Arc::clone(&conn);
    let shutdown_owner = owner.clone();
    std::thread::spawn(move || {
        let reply = dispatch(
            &shutdown_state,
            &shutdown_owner,
            ClientMessage::Shutdown { id: 99 },
            &shutdown_conn,
            true,
            true,
            true,
            true,
        );
        let _ = tx.send(reply);
    });
    match rx.recv_timeout(Duration::from_secs(5)) {
        Ok(Some(DaemonMessage::Shutdown { accepted: true, .. })) => {}
        other => panic!("the Shutdown arm must accept without closing the journal: {other:?}"),
    }
    assert_eq!(
        state.sessions.journal_checkpoint_call_count(),
        1,
        "an accepted quit checkpoints exactly once and never closes the writer"
    );

    // The delete finishes behind the released read. Its row is asserted
    // through the live writer — nothing has closed it, and nothing else is
    // in flight once the delete itself answered, so this read is settled,
    // not raced. After it, the production shutdown sequence (drain, then
    // the terminal close) runs to its end on this thread.
    release_tx.send(()).expect("release the held read");
    // The bound only catches a lost or failed delete: the row lands behind a
    // real `git worktree remove`, and the bound is not a speed assertion.
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        match state.sessions.workspaces_list(&project.id) {
            Ok(workspaces) if workspaces.is_empty() => break,
            Ok(workspaces) => assert!(
                Instant::now() < deadline,
                "the queued delete must land its journal row: {} row(s) left",
                workspaces.len()
            ),
            Err(error) => {
                panic!("the writer must stay alive until the shutdown path closes it: {error:?}")
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let drained = super::super::lifecycle::drain_writes_and_close_journal(&state);
    assert!(
        drained,
        "the delete must finish inside the drain bound once the read is released"
    );
    assert!(
        !checkout.exists(),
        "the queued delete must remove the checkout even though shutdown started first"
    );
    drop(state);
    let _ = std::fs::remove_dir_all(&path);
}

/// A refused quit — another local app still connected — replies without
/// touching the journal at all: no checkpoint round trip on the loop that
/// writes replies, for a quit the daemon is refusing anyway.
#[test]
fn a_refused_quit_never_touches_the_journal() {
    let (path, state) = temp_state("git-off-loop-refused-quit-state");
    // Two local clients: the quit is refused, the daemon keeps running.
    state
        .lifecycle
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .local_clients = 2;
    let owner = test_owner();
    let conn = ConnHandle::new(110);
    let reply = dispatch(
        &state,
        &owner,
        ClientMessage::Shutdown { id: 5 },
        &conn,
        true,
        true,
        true,
        true,
    );
    match reply {
        Some(DaemonMessage::Shutdown {
            accepted: false,
            reason: Some(_),
            ..
        }) => {}
        other => panic!("a second local client must make the quit refused: {other:?}"),
    }
    assert_eq!(
        state.sessions.journal_checkpoint_call_count(),
        0,
        "a refused quit must not checkpoint the journal"
    );
    drop(state);
    let _ = std::fs::remove_dir_all(&path);
}
