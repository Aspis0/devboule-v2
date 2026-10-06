//! The value lane: a repository sweep, a job that answers a value rather
//! than a wire frame. It must not spend a read permit the workspace reads
//! need, and it must not hold this root's drain thread while it waits for its
//! own — a sweep queued behind a full value lane waits in the queue, so a
//! write behind it still runs.

use std::sync::atomic::Ordering;

use devboule_protocol::{ClientMessage, DaemonMessage};

use super::super::git_queue::{MAX_IN_FLIGHT_READ_JOBS, MAX_IN_FLIGHT_VALUE_JOBS};
use super::test_support::*;
use super::*;

/// Cross-root: four slow reads hold every read permit; a sweep on a fifth
/// root still runs — and two sweeps run together, which is the lane's whole
/// ceiling.
#[test]
fn a_value_job_runs_while_the_read_lane_is_full() {
    let mut repos = Vec::new();
    let (path, state) = temp_state("git-off-loop-value-lane-state");
    let mut foreign_ids = Vec::new();
    for index in 0..MAX_IN_FLIGHT_READ_JOBS {
        let repo = TestRepo::new(&format!("git-off-loop-value-foreign-{index}"));
        foreign_ids.push(add_workspace(&state, &repo.root));
        repos.push(repo);
    }
    let gates: Vec<_> = foreign_ids
        .iter()
        .map(|workspace_id| arm_git_gate(&state, workspace_id))
        .collect();
    let owner = test_owner();
    let conn = ConnHandle::new(101);
    let store = ReplyStore::new();
    let (stop, collector) = spawn_collector(&conn, store.clone());
    for (index, workspace_id) in foreign_ids.iter().enumerate() {
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

    let mut sweep_roots = Vec::new();
    for index in 0..MAX_IN_FLIGHT_VALUE_JOBS {
        let repo = TestRepo::new(&format!("git-off-loop-value-sweep-{index}"));
        sweep_roots.push(repo.root.clone());
        repos.push(repo);
    }
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let mut releases = Vec::new();
    let mut running = Vec::new();
    for root in sweep_roots {
        let sweep_state = Arc::clone(&state);
        let started_tx = started_tx.clone();
        // One release channel per sweep: a receiver is not cloneable, and each
        // sweep must be let go on its own.
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        releases.push(release_tx);
        running.push(std::thread::spawn(move || {
            sweep_state.read_git_value(&root, Duration::from_secs(30), move || {
                let _ = started_tx.send(());
                // Hold the lane until the test lets go, so a second sweep that
                // could not take it would still be waiting here.
                let _ = release_rx.recv_timeout(Duration::from_secs(30));
                "answered"
            })
        }));
    }
    drop(started_tx);
    for _ in 0..MAX_IN_FLIGHT_VALUE_JOBS {
        started_rx
            .recv_timeout(Duration::from_secs(30))
            .expect("a value job must not wait on a read permit");
    }
    for (_, gate_release) in &gates {
        gate_release.send(()).expect("release a held status");
    }
    store.wait_each_reply(MAX_IN_FLIGHT_READ_JOBS);
    for release in &releases {
        let _ = release.send(());
    }
    stop.store(true, Ordering::SeqCst);
    let _ = collector.join();
    for (index, sweep) in running.into_iter().enumerate() {
        assert_eq!(
            sweep.join().expect("sweep thread"),
            Ok("answered"),
            "sweep {index} never answered"
        );
    }
    drop(repos);
    drop(state);
    let _ = std::fs::remove_dir_all(path);
}
/// The case the lane's waiting is about: the value lane is full, so a sweep
/// on a third root waits in its queue — and the commit behind it runs anyway.
/// If a waiting sweep were picked up and blocked the drain on its permit, this
/// commit would sit behind it for as long as the two sweeps hold theirs.
#[test]
fn a_write_runs_ahead_of_a_sweep_waiting_for_its_permit() {
    let mut repos = Vec::new();
    let (path, state) = temp_state("git-off-loop-value-blocked-state");

    // Two roots whose sweeps hold the whole lane, and a third root whose sweep
    // is left waiting: one root has one drain thread, so only roots that
    // differ can run at once.
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let mut releases = Vec::new();
    let mut running = Vec::new();
    let mut spawn_sweep = |state: &Arc<ServerState>, root: std::path::PathBuf| {
        let sweep_state = Arc::clone(state);
        let started_tx = started_tx.clone();
        let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
        releases.push(release_tx);
        running.push(std::thread::spawn(move || {
            sweep_state.read_git_value(&root, Duration::from_secs(60), move || {
                let _ = started_tx.send(());
                let _ = release_rx.recv_timeout(Duration::from_secs(60));
                "answered"
            })
        }));
    };
    for index in 0..MAX_IN_FLIGHT_VALUE_JOBS {
        let repo = TestRepo::new(&format!("git-off-loop-value-blocked-{index}"));
        spawn_sweep(&state, repo.root.clone());
        repos.push(repo);
    }
    let waiting = TestRepo::new("git-off-loop-value-blocked-waiting");
    let workspace_id = add_workspace(&state, &waiting.root);
    spawn_sweep(&state, waiting.root.clone());
    repos.push(waiting);
    // The root whose sweep is waiting: the commit is queued behind it.
    let waiting_index = MAX_IN_FLIGHT_VALUE_JOBS;
    drop(started_tx);
    for _ in 0..MAX_IN_FLIGHT_VALUE_JOBS {
        started_rx
            .recv_timeout(Duration::from_secs(30))
            .expect("the first sweeps must take the lane");
    }
    assert!(
        started_rx.try_recv().is_err(),
        "a sweep past the lane's ceiling must wait in the queue, not start"
    );

    let owner = test_owner();
    let conn = ConnHandle::new(102);
    let store = ReplyStore::new();
    let (stop, collector) = spawn_collector(&conn, store.clone());
    repos[waiting_index].write("b.txt", "second\n");
    repos[waiting_index].run(&["add", "b.txt"]);
    assert!(dispatch(
        &state,
        &owner,
        ClientMessage::WorkspaceGitCommit {
            id: 1,
            workspace_id,
            message: "behind a waiting sweep".to_string(),
            idempotency_key: None,
        },
        &conn,
        true,
        true,
        true,
        true,
    )
    .is_none());
    store.wait_each_reply(1);
    match store.get(1) {
        DaemonMessage::WorkspaceGitWrite { error: None, .. } => {}
        other => panic!("the commit must land while a sweep waits, got {other:?}"),
    }
    assert!(
        repos[waiting_index]
            .commit_subjects()
            .contains("behind a waiting sweep"),
        "the commit must really run"
    );
    assert!(
        started_rx.try_recv().is_err(),
        "the waiting sweep must still be waiting: it never got the lane"
    );

    for release in &releases {
        let _ = release.send(());
    }
    stop.store(true, Ordering::SeqCst);
    let _ = collector.join();
    for sweep in running {
        assert!(
            sweep.join().expect("sweep thread").is_ok(),
            "a sweep never answered"
        );
    }
    drop(repos);
    drop(state);
    let _ = std::fs::remove_dir_all(path);
}
