//! Bound: the read cap across roots is reached and never exceeded (polled,
//! not slept-to), and a burst of distinct diffs parks at the per-root caps —
//! queued at one, waitlisted at the other — with every request answered and
//! none refused. The waitlist admits newest-first — the read the user just
//! asked for is the one whose latency they feel — and at its cap the newest
//! read supersedes the oldest, whose sinks are told so, instead of anyone
//! being refused.

use std::sync::atomic::Ordering;

use devboule_protocol::DaemonMessage;

use super::super::git_queue::{MAX_IN_FLIGHT_READ_JOBS, WAITING_READ_CAP};
use super::test_support::*;
use super::*;

/// Five workspaces, five held commands: the read lane fills to its cap and
/// the fifth status — over the cap — never starts while the four gates stay
/// held. Polling, not sleeping: on a loaded box the count is waited for, and
/// the never-exceeds half keeps sampling under load.
#[test]
fn the_read_cap_bounds_in_flight_git_jobs() {
    const WORKSPACES: usize = 5;
    let mut repos = Vec::new();
    let (path, state) = temp_state("git-off-loop-cap-state");
    let mut workspace_ids = Vec::new();
    for index in 0..WORKSPACES {
        let repo = TestRepo::new(&format!("git-off-loop-cap-{index}"));
        workspace_ids.push(add_workspace(&state, &repo.root));
        repos.push(repo);
    }
    let gates: Vec<_> = workspace_ids
        .iter()
        .map(|workspace_id| arm_git_gate(&state, workspace_id))
        .collect();
    let owner = test_owner();
    let conn = ConnHandle::new(98);
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
    let held = wait_until_taken(&gates, MAX_IN_FLIGHT_READ_JOBS);
    for _ in 0..4 {
        std::thread::sleep(Duration::from_millis(100));
        for (index, taken) in held.iter().enumerate() {
            if !taken {
                assert!(
                    gates[index].0.try_recv().is_err(),
                    "a fifth read started while the read lane was full"
                );
            }
        }
    }
    for (_, release) in &gates {
        release.send(()).expect("release a held status");
    }
    store.wait_len(WORKSPACES);
    stop.store(true, Ordering::SeqCst);
    let _ = collector.join();
    let mut ids = store.ids();
    ids.sort_unstable();
    assert_eq!(
        ids,
        vec![1, 2, 3, 4, 5],
        "every status answers exactly once"
    );
    for id in 1..=5 {
        match store.get(id) {
            DaemonMessage::WorkspaceGit { status, .. } => assert!(status.is_git),
            other => panic!("wrong reply shape: {other:?}"),
        }
    }
    drop(repos);
    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// Distinct diff paths cannot coalesce, so a burst of them is exactly where
/// a queue would grow without bound. Ten distinct diffs on one held root:
/// four park in the queue and five on the waitlist, and the burst stops
/// there. Every request is answered with its own path and no request is
/// refused; on a tree whose waitlist is unbounded the shape assert below
/// fails, because the burst rides straight through it.
#[test]
fn a_diff_burst_stays_bounded_and_answers_its_own_path() {
    let repo = TestRepo::new("git-off-loop-burst");
    let (path, state, workspace_id) = state_with_workspace("git-off-loop-burst-state", &repo.root);
    let root_key = resolved_root(&state, &workspace_id);
    let (entered_rx, release_tx) = arm_git_gate(&state, &workspace_id);
    let owner = test_owner();
    let conn = ConnHandle::new(102);
    let store = ReplyStore::new();
    let (stop, collector) = spawn_collector(&conn, store.clone());
    let diff = |id: u64, path: &str| ClientMessage::WorkspaceGitDiff {
        id,
        workspace_id: workspace_id.clone(),
        path: path.to_string(),
    };
    assert!(dispatch(
        &state,
        &owner,
        diff(1, "f1.txt"),
        &conn,
        true,
        true,
        true,
        true
    )
    .is_none());
    entered_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("the first diff's git command was held");
    for id in 2..=10u64 {
        let file = format!("f{id}.txt");
        repo.write(&file, format!("file {id}\n"));
        assert!(dispatch(
            &state,
            &owner,
            diff(id, &file),
            &conn,
            true,
            true,
            true,
            true
        )
        .is_none());
    }
    assert_eq!(
        queue_shape(&state, &root_key),
        (4, 4, 5),
        "the burst must park at the per-root caps: four queued reads, five waitlisted"
    );
    release_tx.send(()).expect("release the first diff");
    store.wait_each_reply(10);
    stop.store(true, Ordering::SeqCst);
    let _ = collector.join();
    let mut ids = store.ids();
    ids.sort_unstable();
    assert_eq!(
        ids,
        vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
        "every request answered exactly once"
    );
    for id in 1..=10u64 {
        match store.get(id) {
            DaemonMessage::WorkspaceGitFile { file, .. } => {
                assert_eq!(
                    file.path,
                    format!("f{id}.txt"),
                    "a diff must answer its own path"
                );
            }
            other => panic!("a burst inside the caps must answer, not refuse: {other:?}"),
        }
    }
    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// A client flooding distinct reads past both caps is never refused: the
/// newest read supersedes the oldest waiting one — the user has clicked
/// past it, and its sinks are answered with a superseded error the
/// frontend's generation check buries — and the waitlist stays at its cap.
/// Seventeen reads are parked (one running, four queued, twelve waiting);
/// the eighteenth through twentieth each evict the oldest waiting read.
#[test]
fn a_read_flood_supersedes_the_oldest_waiting_read_at_the_cap() {
    let repo = TestRepo::new("git-off-loop-flood");
    let (path, state, workspace_id) = state_with_workspace("git-off-loop-flood-state", &repo.root);
    let root_key = resolved_root(&state, &workspace_id);
    let (entered_rx, release_tx) = arm_git_gate(&state, &workspace_id);
    let owner = test_owner();
    let conn = ConnHandle::new(104);
    let store = ReplyStore::new();
    let (stop, collector) = spawn_collector(&conn, store.clone());
    let diff = |id: u64| ClientMessage::WorkspaceGitDiff {
        id,
        workspace_id: workspace_id.clone(),
        path: format!("f{id}.txt"),
    };
    assert!(dispatch(&state, &owner, diff(1), &conn, true, true, true, true).is_none());
    entered_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("the first diff's git command was held");
    for id in 2..=20u64 {
        repo.write(&format!("f{id}.txt"), format!("file {id}\n"));
        assert!(dispatch(&state, &owner, diff(id), &conn, true, true, true, true).is_none());
    }
    assert_eq!(
        queue_shape(&state, &root_key),
        (4, 4, WAITING_READ_CAP),
        "the flood must park at the caps, not grow past them"
    );
    store.wait_len(3);
    assert_eq!(
        store.ids(),
        vec![6, 7, 8],
        "the oldest waiting reads are superseded, oldest first, while the others still wait"
    );
    for id in [6, 7, 8] {
        match store.get(id) {
            DaemonMessage::Error(error) => {
                assert_eq!(error.id, Some(id), "the superseded read keeps its own id");
                assert!(
                    error.message.contains("superseded by newer reads"),
                    "the answer says it was superseded, got: {}",
                    error.message
                );
            }
            other => panic!("wrong supersession shape: {other:?}"),
        }
    }
    release_tx.send(()).expect("release the first diff");
    // The gate sample: 10 admitted children drained within 30 s (≈3 s each)
    // with 7 still outstanding; 17 × 3 s ≈ 51 s, doubled for a worse run.
    // The bound only has to catch a lost reply, not pace the drain.
    store.wait_len_within(20, Duration::from_secs(6) * (20 - 3));
    stop.store(true, Ordering::SeqCst);
    let _ = collector.join();
    let mut ids = store.ids();
    ids.sort_unstable();
    assert_eq!(
        ids,
        (1..=20).collect::<Vec<u64>>(),
        "every request answered exactly once — superseded reads included"
    );
    for id in (1..=20u64).filter(|id| !matches!(id, 6..=8)) {
        match store.get(id) {
            DaemonMessage::WorkspaceGitFile { file, .. } => {
                assert_eq!(file.path, format!("f{id}.txt"));
            }
            other => panic!("an admitted read must answer its own path: {other:?}"),
        }
    }
    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// The waitlist admits newest-first: the read the user just asked for is
/// the one whose latency they feel, so after a click-through the newest
/// waitlisted diff is served before the older ones, deterministically.
#[test]
fn the_newest_waitlisted_read_is_admitted_first() {
    let repo = TestRepo::new("git-off-loop-newest");
    let (path, state, workspace_id) = state_with_workspace("git-off-loop-newest-state", &repo.root);
    let (entered_rx, release_tx) = arm_git_gate(&state, &workspace_id);
    let owner = test_owner();
    let conn = ConnHandle::new(106);
    let store = ReplyStore::new();
    let (stop, collector) = spawn_collector(&conn, store.clone());
    let diff = |id: u64| ClientMessage::WorkspaceGitDiff {
        id,
        workspace_id: workspace_id.clone(),
        path: format!("f{id}.txt"),
    };
    assert!(dispatch(&state, &owner, diff(1), &conn, true, true, true, true).is_none());
    entered_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("the first diff's git command was held");
    for id in 2..=5u64 {
        repo.write(&format!("f{id}.txt"), format!("file {id}\n"));
        assert!(dispatch(&state, &owner, diff(id), &conn, true, true, true, true).is_none());
    }
    // Two reads beyond the queue cap: 6 arrived first, 7 is the newest.
    for id in 6..=7u64 {
        repo.write(&format!("f{id}.txt"), format!("file {id}\n"));
        assert!(dispatch(&state, &owner, diff(id), &conn, true, true, true, true).is_none());
    }
    release_tx.send(()).expect("release the first diff");
    store.wait_each_reply(7);
    stop.store(true, Ordering::SeqCst);
    let _ = collector.join();
    assert_eq!(
        store.ids(),
        vec![1, 2, 3, 4, 5, 7, 6],
        "the newest waitlisted read is admitted before the older one"
    );
    for id in 1..=7u64 {
        match store.get(id) {
            DaemonMessage::WorkspaceGitFile { file, .. } => {
                assert_eq!(file.path, format!("f{id}.txt"));
            }
            other => panic!("wrong reply shape: {other:?}"),
        }
    }
    drop(state);
    let _ = std::fs::remove_dir_all(path);
}
