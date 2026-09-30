//! Join and keys: which requests share a job — a queued read joins only an
//! identical read that is this root's last job, never one parked behind a
//! write, or an identical read already on the waitlist — and which share a
//! queue: one key producer, canonicalized the same way for both path
//! namespaces.

use std::sync::atomic::Ordering;

use devboule_protocol::{DaemonMessage, WorkspaceGitFileStatus};

use super::super::git_queue::{Job, ReadKey, Sink};
use super::test_support::*;
use super::*;

/// While a status runs, a second status queues and a third *joins it*: one
/// queued read for two requests — the join is visible in the queue shape —
/// and every request is answered from that job's result with its own id.
#[test]
fn a_queued_poll_joins_the_same_poll_and_every_request_is_answered() {
    let repo = TestRepo::new("git-off-loop-coalesce");
    let (path, state, workspace_id) =
        state_with_workspace("git-off-loop-coalesce-state", &repo.root);
    let root_key = resolved_root(&state, &workspace_id);
    let (entered_rx, release_tx) = arm_git_gate(&state, &workspace_id);
    let owner = test_owner();
    let conn = ConnHandle::new(99);
    let store = ReplyStore::new();
    let (stop, collector) = spawn_collector(&conn, store.clone());
    let status = |id: u64| ClientMessage::WorkspaceGitStatus {
        id,
        workspace_id: workspace_id.clone(),
    };
    assert!(dispatch(&state, &owner, status(1), &conn, true, true, true, true).is_none());
    entered_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("the first status's git command was held");
    assert!(
        dispatch(&state, &owner, status(2), &conn, true, true, true, true).is_none(),
        "the second status queues behind the running one"
    );
    assert!(
        dispatch(&state, &owner, status(3), &conn, true, true, true, true).is_none(),
        "the third status joins the queued one"
    );
    assert_eq!(
        queue_shape(&state, &root_key),
        (1, 1, 0),
        "the third status must join the queued poll, not add a job"
    );
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        conn.outbound.pull_replies().is_empty(),
        "a coalesced request answered before its job ran"
    );
    release_tx.send(()).expect("release the first status");
    store.wait_len(3);
    stop.store(true, Ordering::SeqCst);
    let _ = collector.join();
    assert_eq!(
        store.ids(),
        vec![1, 2, 3],
        "every request gets its own reply"
    );
    for id in [1, 2, 3] {
        match store.get(id) {
            DaemonMessage::WorkspaceGit { status, .. } => assert!(status.is_git),
            other => panic!("wrong reply shape: {other:?}"),
        }
    }
    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// A poll issued **after** a write must never join a poll queued **before**
/// it: the stage is queued between two statuses, the third status adds its
/// own job behind the write, and its answer is computed after the stage —
/// the row for the staged file reads staged, with its line counted.
#[test]
fn a_poll_issued_after_a_write_is_answered_after_the_write() {
    let repo = TestRepo::new("git-off-loop-stale");
    repo.write("b.txt", "second\n");
    let (path, state, workspace_id) = state_with_workspace("git-off-loop-stale-state", &repo.root);
    let root_key = resolved_root(&state, &workspace_id);
    let (entered_rx, release_tx) = arm_git_gate(&state, &workspace_id);
    let owner = test_owner();
    let conn = ConnHandle::new(101);
    let store = ReplyStore::new();
    let (stop, collector) = spawn_collector(&conn, store.clone());
    let status = |id: u64| ClientMessage::WorkspaceGitStatus {
        id,
        workspace_id: workspace_id.clone(),
    };
    assert!(dispatch(&state, &owner, status(1), &conn, true, true, true, true).is_none());
    entered_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("the first status's git command was held");
    assert!(dispatch(&state, &owner, status(2), &conn, true, true, true, true).is_none());
    assert!(dispatch(
        &state,
        &owner,
        ClientMessage::WorkspaceGitStage {
            id: 3,
            workspace_id: workspace_id.clone(),
            paths: vec!["b.txt".to_string()],
            idempotency_key: None,
        },
        &conn,
        true,
        true,
        true,
        true,
    )
    .is_none());
    assert!(dispatch(&state, &owner, status(4), &conn, true, true, true, true).is_none());
    assert_eq!(
        queue_shape(&state, &root_key),
        (3, 2, 0),
        "the poll after the write must not join the poll before it"
    );
    release_tx.send(()).expect("release the first status");
    store.wait_len(4);
    stop.store(true, Ordering::SeqCst);
    let _ = collector.join();
    assert_eq!(store.ids(), vec![1, 2, 3, 4]);
    let row = |id: u64| match store.get(id) {
        DaemonMessage::WorkspaceGit { status, .. } => status
            .rows
            .iter()
            .find(|row| row.path == "b.txt")
            .expect("a row for the staged file")
            .clone(),
        other => panic!("wrong reply shape: {other:?}"),
    };
    let before = row(2);
    assert_eq!(
        before.status,
        WorkspaceGitFileStatus::Untracked,
        "the poll queued before the stage saw the file untracked"
    );
    assert_eq!(before.additions, 1, "an untracked row counts its own lines");
    let after = row(4);
    assert_eq!(
        after.status,
        WorkspaceGitFileStatus::Added,
        "the poll after the stage must see the staged index"
    );
    assert_eq!(after.additions, 1);
    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// One folder, two spellings, two key producers: a project add and the git
/// frames of that folder's workspace must land on one queue.
#[test]
fn project_add_keys_like_the_workspace_frames() {
    let dir = crate::test_dirs::test_temp_dir("git-off-loop-canonical-dir");
    let (marker, state, workspace_id) = state_with_workspace("git-off-loop-canonical", &dir);
    let add_key = queue_key(
        &state,
        &ClientMessage::ProjectAdd {
            id: 1,
            path: dir.to_string_lossy().into_owned(),
        },
    );
    let workspace_key = queue_key(
        &state,
        &ClientMessage::WorkspaceGitStatus {
            id: 2,
            workspace_id: workspace_id.clone(),
        },
    );
    assert_eq!(
        add_key, workspace_key,
        "a project add and its folder's workspace frames must share one queue"
    );
    assert_ne!(add_key, "workspace:unkeyed");
    assert!(
        !add_key.starts_with("project-add-unresolvable"),
        "an existing folder must canonicalize: {add_key}"
    );
    drop(state);
    let _ = std::fs::remove_dir_all(&marker);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Every arm `is_git_backed` admits must resolve a real queue key. The two
/// lists are hand-kept; this is the test that keeps them honest — a new arm
/// admitted without a key would serialize against everything on the fallback
/// queue, silently.
#[test]
fn every_admitted_arm_resolves_a_real_queue_key() {
    let marker = crate::test_dirs::test_temp_dir("git-off-loop-keys-marker");
    let (path, state, workspace_id) = state_with_workspace("git-off-loop-keys", &marker);
    let requests = vec![
        ClientMessage::WorkspaceGitStatus {
            id: 1,
            workspace_id: workspace_id.clone(),
        },
        ClientMessage::WorkspaceGitDiff {
            id: 2,
            workspace_id: workspace_id.clone(),
            path: "a.txt".to_string(),
        },
        ClientMessage::WorkspaceGitLog {
            id: 3,
            workspace_id: workspace_id.clone(),
        },
        ClientMessage::WorkspaceGitStage {
            id: 4,
            workspace_id: workspace_id.clone(),
            paths: vec!["a.txt".to_string()],
            idempotency_key: None,
        },
        ClientMessage::WorkspaceGitUnstage {
            id: 5,
            workspace_id: workspace_id.clone(),
            paths: vec!["a.txt".to_string()],
            idempotency_key: None,
        },
        ClientMessage::WorkspaceGitDiscard {
            id: 6,
            workspace_id: workspace_id.clone(),
            paths: vec!["a.txt".to_string()],
            idempotency_key: None,
        },
        ClientMessage::WorkspaceGitCommit {
            id: 7,
            workspace_id: workspace_id.clone(),
            message: "keys".to_string(),
            idempotency_key: None,
        },
        ClientMessage::WorkspaceFileRename {
            id: 8,
            workspace_id: workspace_id.clone(),
            path: "a.txt".to_string(),
            name: "b.txt".to_string(),
            idempotency_key: None,
        },
        ClientMessage::WorkspaceDelete {
            id: 9,
            workspace_id: workspace_id.clone(),
            force: false,
        },
        ClientMessage::WorkspaceCreate {
            id: 10,
            project_id: "p.keys".to_string(),
            isolation: WorkspaceIsolation::Worktree,
            branch: None,
        },
        ClientMessage::ProjectAdd {
            id: 11,
            path: marker.to_string_lossy().into_owned(),
        },
    ];
    for request in &requests {
        assert_ne!(
            queue_key(&state, request),
            "workspace:unkeyed",
            "admitted arm without a queue key: {}",
            request.name()
        );
    }
    drop(state);
    let _ = std::fs::remove_dir_all(&path);
    let _ = std::fs::remove_dir_all(&marker);
}

/// Delivery is per sink: a panicking sink must not cost the sinks queued
/// behind it — a healthy sink before it and a joiner after it both still
/// get the job's one result.
#[test]
fn a_panicking_sink_does_not_drop_the_sinks_behind_it() {
    let (path, state) = temp_state("git-off-loop-sink-state");
    let hits: Arc<Mutex<Vec<&'static str>>> = Arc::new(Mutex::new(Vec::new()));
    let healthy = |label: &'static str| {
        let hits = Arc::clone(&hits);
        Sink::new(Box::new(move |_reply| {
            hits.lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(label);
        }))
    };

    // A holder read keeps the root's drain busy; the probe job queues
    // behind it, and the joiner joins the queued probe, landing behind the
    // panicking sink in the delivery order.
    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::sync_channel(1);
    let holder = Job::Read {
        key: ReadKey::new(70, None),
        compute: Box::new(move || {
            let _ = started_tx.send(());
            let _ = release_rx.recv_timeout(Duration::from_secs(10));
            DaemonMessage::Error(WireError::new(ErrorCode::Io, "the holder reply"))
        }),
        sinks: Arc::new(Mutex::new(vec![healthy("holder")])),
    };
    assert!(state
        .git_jobs
        .enqueue_job("sink-root".to_string(), holder)
        .is_ok());
    started_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("the holder started");
    let probe = Job::Read {
        key: ReadKey::new(71, None),
        compute: Box::new(|| {
            DaemonMessage::Error(WireError::new(ErrorCode::Io, "the probe reply"))
        }),
        sinks: Arc::new(Mutex::new(vec![
            healthy("first"),
            Sink::new(Box::new(|_reply| panic!("a sink panics mid-delivery"))),
            healthy("second"),
        ])),
    };
    assert!(state
        .git_jobs
        .enqueue_job("sink-root".to_string(), probe)
        .is_ok());
    let joiner = Job::Read {
        key: ReadKey::new(71, None),
        compute: Box::new(|| {
            DaemonMessage::Error(WireError::new(
                ErrorCode::Io,
                "a joined read never computes",
            ))
        }),
        sinks: Arc::new(Mutex::new(vec![healthy("joiner")])),
    };
    assert!(state
        .git_jobs
        .enqueue_job("sink-root".to_string(), joiner)
        .is_ok());
    release_tx.send(()).expect("release the holder");
    let deadline = Instant::now() + Duration::from_secs(5);
    while hits.lock().unwrap_or_else(|error| error.into_inner()).len() < 4
        && Instant::now() < deadline
    {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        *hits.lock().unwrap_or_else(|error| error.into_inner()),
        vec!["holder", "first", "second", "joiner"],
        "every sink but the panicking one is delivered, in order"
    );
    drop(state);
    let _ = std::fs::remove_dir_all(&path);
}
