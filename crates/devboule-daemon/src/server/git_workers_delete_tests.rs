//! Delete and shutdown: the delete's journal tail and the drain that
//! precedes the flush. The remove runs before the row delete, so a failed
//! removal keeps its row and the advertised forced retry works; shutdown
//! drains accepted write jobs — queued ones included — before it flushes
//! the journal, bounded.

use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use super::test_support::*;
use super::*;

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
        let finished_clean = waiter_state
            .git_jobs
            .wait_for_write_jobs(GIT_WRITE_DRAIN_BOUND);
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
    let deadline = Instant::now() + Duration::from_secs(5);
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

/// Writes still queued when the drain bound runs out are answered
/// ShuttingDown instead of run: the flush they were meant to precede is
/// next, and a delete started after it could strand a half-removed checkout.
/// The commit here sits behind a held read, so it is still queued when the
/// bound expires.
#[test]
fn writes_queued_past_the_drain_bound_are_answered_shutting_down() {
    let repo = TestRepo::new("git-off-loop-drain-cancel");
    repo.write("b.txt", "second\n");
    repo.run(&["add", "b.txt"]);
    let (path, state, workspace_id) =
        state_with_workspace("git-off-loop-drain-cancel-state", &repo.root);
    let (entered_rx, release_tx) = arm_git_gate(&state, &workspace_id);
    let owner = test_owner();
    let conn = ConnHandle::new(108);
    let store = ReplyStore::new();
    let (stop, collector) = spawn_collector(&conn, store.clone());
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
            message: "late".to_string(),
            idempotency_key: None,
        },
        &conn,
        true,
        true,
        true,
        true,
    )
    .is_none());
    assert!(
        !state
            .git_jobs
            .wait_for_write_jobs(Duration::from_millis(300)),
        "the drain must time out with the commit still queued"
    );
    state.git_jobs.cancel_queued_writes();
    release_tx.send(()).expect("release the held read");
    store.wait_len(2);
    stop.store(true, Ordering::SeqCst);
    let _ = collector.join();
    assert_eq!(store.ids(), vec![1, 2]);
    match store.get(2) {
        DaemonMessage::Error(error) => assert!(
            error.message.contains("shutting down"),
            "a write past the drain bound must be answered ShuttingDown, got: {}",
            error.message
        ),
        other => panic!("a write past the drain bound must not run: {other:?}"),
    }
    assert!(
        !repo.commit_subjects().contains("late"),
        "the cancelled commit must not run"
    );
    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// The drain is bounded: a write held in the git runner makes
/// `wait_for_write_jobs` give up within its bound, not hang.
#[test]
fn the_write_drain_is_bounded() {
    let repo = TestRepo::new("git-off-loop-drain-bound");
    repo.write("b.txt", "second\n");
    let (path, state, workspace_id) =
        state_with_workspace("git-off-loop-drain-bound-state", &repo.root);
    // Never released: the commit's `git commit` sits in the runner for the
    // whole test, so its write stays outstanding.
    let (entered_rx, _release_tx) = arm_git_gate(&state, &workspace_id);
    let owner = test_owner();
    let conn = ConnHandle::new(107);
    assert!(dispatch(
        &state,
        &owner,
        ClientMessage::WorkspaceGitCommit {
            id: 1,
            workspace_id: workspace_id.clone(),
            message: "stuck".to_string(),
            idempotency_key: None,
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
        .expect("the stuck write started (its accepted count is up)");
    let started = Instant::now();
    assert!(
        !state
            .git_jobs
            .wait_for_write_jobs(Duration::from_millis(300)),
        "a stuck write must time the drain out"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the drain bound must actually bound: {:?}",
        started.elapsed()
    );
    drop(state);
    let _ = std::fs::remove_dir_all(path);
}
