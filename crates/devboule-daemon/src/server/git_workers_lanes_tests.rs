//! Lanes: which waits cross roots and which never do. A read's permit wait
//! is its own problem — four foreign reads hold the read lane and a write
//! still runs, on another root because the lanes are global, and on the
//! *same* root because the drain takes a read's permit before it starts
//! the read.

use std::sync::atomic::Ordering;

use devboule_protocol::DaemonMessage;

use super::super::git_queue::MAX_IN_FLIGHT_READ_JOBS;
use super::test_support::*;
use super::*;

/// Cross-root: four slow reads hold every read permit; a commit on a fifth
/// root runs at once — writes take the write lane and never wait behind the
/// read sweep.
#[test]
fn a_write_runs_while_the_read_lane_is_saturated() {
    let mut repos = Vec::new();
    let (path, state) = temp_state("git-off-loop-lane-state");
    let mut workspace_ids = Vec::new();
    for index in 0..MAX_IN_FLIGHT_READ_JOBS {
        let repo = TestRepo::new(&format!("git-off-loop-lane-{index}"));
        workspace_ids.push(add_workspace(&state, &repo.root));
        repos.push(repo);
    }
    let write_repo = TestRepo::new("git-off-loop-lane-write");
    write_repo.write("b.txt", "second\n");
    write_repo.run(&["add", "b.txt"]);
    let write_workspace = add_workspace(&state, &write_repo.root);
    let gates: Vec<_> = workspace_ids
        .iter()
        .map(|workspace_id| arm_git_gate(&state, workspace_id))
        .collect();
    let owner = test_owner();
    let conn = ConnHandle::new(100);
    let store = ReplyStore::new();
    let (stop, collector) = spawn_collector(&conn, store.clone());
    for (index, workspace_id) in workspace_ids.iter().enumerate() {
        assert!(dispatch(
            &state,
            &owner,
            ClientMessage::WorkspaceGitStatus {
                id: index as u64 + 1,
                workspace_id: workspace_id.clone(),
            },
            &conn,
            true,
            true,
            true,
            true,
        )
        .is_none());
    }
    wait_until_taken(&gates, MAX_IN_FLIGHT_READ_JOBS);
    let (write_entered, write_release) = arm_git_gate(&state, &write_workspace);
    assert!(dispatch(
        &state,
        &owner,
        ClientMessage::WorkspaceGitCommit {
            id: 10,
            workspace_id: write_workspace.clone(),
            message: "lane".to_string(),
            idempotency_key: None,
        },
        &conn,
        true,
        true,
        true,
        true,
    )
    .is_none());
    write_entered
        .recv_timeout(Duration::from_secs(30))
        .expect("the commit must run on the write lane while the read lane is full");
    write_release.send(()).expect("release the commit");
    for (_, release) in &gates {
        release.send(()).expect("release a held status");
    }
    store.wait_each_reply(5);
    stop.store(true, Ordering::SeqCst);
    let _ = collector.join();
    for id in 1..=4 {
        match store.get(id) {
            DaemonMessage::WorkspaceGit { status, .. } => assert!(status.is_git),
            other => panic!("wrong reply shape: {other:?}"),
        }
    }
    match store.get(10) {
        DaemonMessage::WorkspaceGitWrite { error: None, .. } => {}
        other => panic!("expected the commit's clean reply, got {other:?}"),
    }
    assert!(write_repo.commit_subjects().contains("lane"));
    drop(repos);
    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// Same root — the case the panel actually exercises: a read waiting on
/// the read lane must not pin its own root's write. Four foreign reads
/// hold every permit; a status on root R has no permit to take, and a
/// commit on R runs anyway, ahead of the read it is queued behind. The
/// read itself still runs last — behind the write, never in front of it.
#[test]
fn a_write_runs_while_its_own_root_read_waits_for_a_permit() {
    let mut repos = Vec::new();
    let (path, state) = temp_state("git-off-loop-sameroot-state");
    let mut foreign_ids = Vec::new();
    for index in 0..MAX_IN_FLIGHT_READ_JOBS {
        let repo = TestRepo::new(&format!("git-off-loop-sameroot-foreign-{index}"));
        foreign_ids.push(add_workspace(&state, &repo.root));
        repos.push(repo);
    }
    let write_repo = TestRepo::new("git-off-loop-sameroot-write");
    write_repo.write("b.txt", "second\n");
    write_repo.run(&["add", "b.txt"]);
    let root_workspace = add_workspace(&state, &write_repo.root);
    let foreign_gates: Vec<_> = foreign_ids
        .iter()
        .map(|workspace_id| arm_git_gate(&state, workspace_id))
        .collect();
    let owner = test_owner();
    let conn = ConnHandle::new(103);
    let store = ReplyStore::new();
    let (stop, collector) = spawn_collector(&conn, store.clone());
    let status = |id: u64, workspace_id: &str| ClientMessage::WorkspaceGitStatus {
        id,
        workspace_id: workspace_id.to_string(),
    };
    for (index, workspace_id) in foreign_ids.iter().enumerate() {
        assert!(dispatch(
            &state,
            &owner,
            status(index as u64 + 1, workspace_id),
            &conn,
            true,
            true,
            true,
            true,
        )
        .is_none());
    }
    wait_until_taken(&foreign_gates, MAX_IN_FLIGHT_READ_JOBS);
    // Root R's read has no permit to take: its drain waits for one instead
    // of running it — and must not let that wait pin the write behind it.
    assert!(dispatch(
        &state,
        &owner,
        status(5, &root_workspace),
        &conn,
        true,
        true,
        true,
        true,
    )
    .is_none());
    let (write_entered, write_release) = arm_git_gate(&state, &root_workspace);
    assert!(dispatch(
        &state,
        &owner,
        ClientMessage::WorkspaceGitCommit {
            id: 6,
            workspace_id: root_workspace.clone(),
            message: "same-root".to_string(),
            idempotency_key: None,
        },
        &conn,
        true,
        true,
        true,
        true,
    )
    .is_none());
    write_entered
        .recv_timeout(Duration::from_secs(30))
        .expect("the commit must run while its own root's read waits for a read permit");
    assert!(
        store.ids().is_empty(),
        "nothing answered yet: the read still waits, the write sits in the gate: {:?}",
        store.ids()
    );
    write_release.send(()).expect("release the commit");
    store.wait_len(1);
    assert_eq!(
        store.ids(),
        vec![6],
        "the commit answers while the read still waits for a permit"
    );
    for (_, release) in &foreign_gates {
        release.send(()).expect("release a foreign status");
    }
    store.wait_each_reply(6);
    stop.store(true, Ordering::SeqCst);
    let _ = collector.join();
    let mut ids = store.ids();
    ids.sort_unstable();
    assert_eq!(
        ids,
        vec![1, 2, 3, 4, 5, 6],
        "every request answered exactly once"
    );
    match store.get(6) {
        DaemonMessage::WorkspaceGitWrite { error: None, .. } => {}
        other => panic!("the commit must land clean, got {other:?}"),
    }
    assert!(
        write_repo.commit_subjects().contains("same-root"),
        "the same-root commit must really run"
    );
    for id in [1, 2, 3, 4, 5] {
        match store.get(id) {
            DaemonMessage::WorkspaceGit { status, .. } => assert!(status.is_git),
            other => panic!("wrong reply shape: {other:?}"),
        }
    }
    drop(repos);
    drop(state);
    let _ = std::fs::remove_dir_all(path);
}
