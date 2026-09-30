//! Order: one root's queue drains in arrival order — two writes, three
//! writes (writes never coalesce), two connections sharing the state's
//! queues, and a read behind a held write. A per-connection collector
//! records every reply the moment it is produced (worker-road arrivals and
//! inline returns alike), so a reorder always lands in the store rather
//! than being lost as a road timeout. Red causes, per test: the
//! read-behind-write test reds on a tree without the queue as an overtake
//! — the read needs no lock and runs past the held write; the three-writes
//! test reds there at the shape lookup (no queue to find). The two
//! write-after-write tests are green-side guards: on a tree without the
//! queue, `git_write_lock` still serializes a same-root write pair, so
//! their reds come from the queue reordering, not from the inline road.

use std::sync::atomic::Ordering;

use devboule_protocol::WorkspaceGitFileStatus;

use super::test_support::*;
use super::*;

fn stage(id: u64, workspace_id: &str, path: &str) -> ClientMessage {
    ClientMessage::WorkspaceGitStage {
        id,
        workspace_id: workspace_id.to_string(),
        paths: vec![path.to_string()],
        idempotency_key: None,
    }
}

#[test]
fn two_git_writes_on_one_workspace_keep_their_order() {
    let repo = TestRepo::new("git-off-loop-order");
    repo.write("b.txt", "second\n");
    let (path, state, workspace_id) = state_with_workspace("git-off-loop-order-state", &repo.root);
    let (entered_rx, release_tx) = arm_git_gate(&state, &workspace_id);
    let owner = test_owner();
    let conn = ConnHandle::new(93);
    let store = ReplyStore::new();
    let (stop, collector) = spawn_collector(&conn, store.clone());

    spawn_dispatch(
        &state,
        &owner,
        stage(1, &workspace_id, "b.txt"),
        &conn,
        store.clone(),
    );
    entered_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("the stage's git command was held");
    spawn_dispatch(
        &state,
        &owner,
        ClientMessage::WorkspaceGitCommit {
            id: 2,
            workspace_id: workspace_id.clone(),
            message: "order".to_string(),
            idempotency_key: None,
        },
        &conn,
        store.clone(),
    );
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        store.ids().is_empty(),
        "a later request overtook the held stage on the same workspace: reply order {:?}",
        store.ids()
    );
    release_tx.send(()).expect("release the stage");
    store.wait_len(2);
    stop.store(true, Ordering::SeqCst);
    let _ = collector.join();
    assert_eq!(
        store.ids(),
        vec![1, 2],
        "replies must be produced in send order: {:?}",
        store.ids()
    );
    match store.get(2) {
        DaemonMessage::WorkspaceGitWrite { error: None, .. } => {}
        other => panic!("the commit must land clean, got {other:?}"),
    }
    assert!(
        repo.commit_subjects().contains("order"),
        "the commit did not run after the stage"
    );
    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// Three write jobs of one root queue behind a held first and answer in
/// send order — and the two later writes are two queued jobs, never merged:
/// writes do not coalesce. On a tree without the queue this red is the
/// missing queue itself (the shape lookup below finds none), not an
/// overtake — the overtake reds are the two tests around this one.
#[test]
fn three_writes_run_in_the_order_they_were_sent() {
    let repo = TestRepo::new("git-off-loop-fifo");
    repo.write("b1.txt", "one\n");
    repo.write("b2.txt", "two\n");
    let (path, state, workspace_id) = state_with_workspace("git-off-loop-fifo-state", &repo.root);
    let (entered_rx, release_tx) = arm_git_gate(&state, &workspace_id);
    let owner = test_owner();
    let conn = ConnHandle::new(95);
    let store = ReplyStore::new();
    let (stop, collector) = spawn_collector(&conn, store.clone());

    let first_thread = spawn_dispatch(
        &state,
        &owner,
        stage(1, &workspace_id, "b1.txt"),
        &conn,
        store.clone(),
    );
    entered_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("the first stage's git command was held");
    let root_key = resolved_root(&state, &workspace_id);
    spawn_dispatch(
        &state,
        &owner,
        ClientMessage::WorkspaceGitCommit {
            id: 2,
            workspace_id: workspace_id.clone(),
            message: "fifo".to_string(),
            idempotency_key: None,
        },
        &conn,
        store.clone(),
    );
    wait_queue_len(&state, &root_key, 1);
    spawn_dispatch(
        &state,
        &owner,
        stage(3, &workspace_id, "b2.txt"),
        &conn,
        store.clone(),
    );
    wait_queue_len(&state, &root_key, 2);
    assert_eq!(
        queue_shape(&state, &root_key),
        (2, 0, 0),
        "two later writes must queue as two jobs — writes never coalesce"
    );
    release_tx.send(()).expect("release the first stage");
    first_thread.join().expect("first stage thread");
    store.wait_len(3);
    stop.store(true, Ordering::SeqCst);
    let _ = collector.join();
    assert_eq!(
        store.ids(),
        vec![1, 2, 3],
        "replies must be produced in send order: {:?}",
        store.ids()
    );
    assert!(repo.commit_subjects().contains("fifo"));
    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// The brief's forbidden overtake, in its own words: a later read must not
/// answer before an earlier write on the same workspace. The stage is held
/// in the git runner, holding the root's write lock with it; a read needs
/// no lock, so on a tree without the queue it runs straight past and its
/// reply is already in the store during the held window — the red the
/// assertion quotes. On the queued tree the read waits its turn and, when
/// it runs, sees the staged index.
#[test]
fn a_later_read_does_not_overtake_the_held_write() {
    let repo = TestRepo::new("git-off-loop-read-after-write");
    repo.write("b.txt", "second\n");
    let (path, state, workspace_id) =
        state_with_workspace("git-off-loop-read-after-write-state", &repo.root);
    let (entered_rx, release_tx) = arm_git_gate(&state, &workspace_id);
    let owner = test_owner();
    let conn = ConnHandle::new(104);
    let store = ReplyStore::new();
    let (stop, collector) = spawn_collector(&conn, store.clone());

    spawn_dispatch(
        &state,
        &owner,
        stage(1, &workspace_id, "b.txt"),
        &conn,
        store.clone(),
    );
    entered_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("the stage's git command was held");
    spawn_dispatch(
        &state,
        &owner,
        ClientMessage::WorkspaceGitStatus {
            id: 2,
            workspace_id: workspace_id.clone(),
        },
        &conn,
        store.clone(),
    );
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        store.ids().is_empty(),
        "a later read overtook the held write on the same workspace: reply order {:?}",
        store.ids()
    );
    release_tx.send(()).expect("release the stage");
    store.wait_len(2);
    stop.store(true, Ordering::SeqCst);
    let _ = collector.join();
    assert_eq!(
        store.ids(),
        vec![1, 2],
        "replies must be produced in send order: {:?}",
        store.ids()
    );
    match store.get(1) {
        DaemonMessage::WorkspaceGitWrite { error: None, .. } => {}
        other => panic!("the stage must land clean, got {other:?}"),
    }
    match store.get(2) {
        DaemonMessage::WorkspaceGit { status, .. } => {
            let row = status
                .rows
                .iter()
                .find(|row| row.path == "b.txt")
                .expect("a row for the staged file");
            assert_eq!(
                row.status,
                WorkspaceGitFileStatus::Added,
                "the read queued behind the write must see the staged index"
            );
        }
        other => panic!("wrong reply shape: {other:?}"),
    }
    drop(state);
    let _ = std::fs::remove_dir_all(path);
}

/// The queues live on the state, not the connection: two connections on one
/// workspace serialize behind the same held command, in send order.
#[test]
fn two_connections_serialise_on_one_root() {
    let repo = TestRepo::new("git-off-loop-two-conns");
    repo.write("b.txt", "second\n");
    let (path, state, workspace_id) =
        state_with_workspace("git-off-loop-two-conns-state", &repo.root);
    let (entered_rx, release_tx) = arm_git_gate(&state, &workspace_id);
    let owner = test_owner();
    let conn_a = ConnHandle::new(96);
    let conn_b = ConnHandle::new(97);
    let store = ReplyStore::new();
    let (stop_a, collector_a) = spawn_collector(&conn_a, store.clone());
    let (stop_b, collector_b) = spawn_collector(&conn_b, store.clone());

    let stage_thread = spawn_dispatch(
        &state,
        &owner,
        stage(1, &workspace_id, "b.txt"),
        &conn_a,
        store.clone(),
    );
    entered_rx
        .recv_timeout(Duration::from_secs(30))
        .expect("the stage's git command was held");
    // Via the second connection, on its own thread for the same reason as
    // the sibling test: this is the enqueue order, and an inline tree
    // records the overtake in the store.
    spawn_dispatch(
        &state,
        &owner,
        ClientMessage::WorkspaceGitCommit {
            id: 2,
            workspace_id: workspace_id.clone(),
            message: "shared queue".to_string(),
            idempotency_key: None,
        },
        &conn_b,
        store.clone(),
    );
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        store.ids().is_empty(),
        "a second connection overtook the held stage on the same root: reply order {:?}",
        store.ids()
    );
    release_tx.send(()).expect("release the stage");
    stage_thread.join().expect("stage thread");
    store.wait_len(2);
    stop_a.store(true, Ordering::SeqCst);
    stop_b.store(true, Ordering::SeqCst);
    let _ = collector_a.join();
    let _ = collector_b.join();
    assert_eq!(
        store.ids(),
        vec![1, 2],
        "replies must be produced in send order across connections: {:?}",
        store.ids()
    );
    assert!(repo.commit_subjects().contains("shared queue"));
    drop(state);
    let _ = std::fs::remove_dir_all(path);
}
