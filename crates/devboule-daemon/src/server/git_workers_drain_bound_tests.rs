//! Drain bounds: expired drains cancel queued writes and return on time.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use devboule_protocol::ClientMessage;

use super::super::{dispatch, ConnHandle, DaemonMessage};
use super::test_support::{
    arm_git_gate, spawn_collector, state_with_workspace, test_owner, ReplyStore, TestRepo,
};

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
