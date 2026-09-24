//! The pause that outlives the daemon (fix pass 3, P1-6): `paused` is the
//! fence that keeps a failed send — or a close that could not discard its
//! queue — from ever sending on its own, so it must survive a restart.
//! Both tests cross a real restart boundary: the journal shuts down, a
//! second daemon opens the same file, the session resumes — and nothing is
//! sent until a user act.

use super::tests::{
    insert_live_agent_with_writer, test_owner, tmp_delete_registry, FailingWriter, RecordingWriter,
};
use super::{
    Arc, ConnHandle, ConnPeer, Duration, Instant, Journal, Mutex, OwnerId, RuntimePaths,
    SessionRegistry,
};

const SESSION: &str = "s.q.pause";

fn wait_until(what: &str, mut condition: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if condition() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

/// A daemon over `dir`'s journal: live agent, recording writer, no attach,
/// idle — the world a resume lands in. The returned writer mirror is what
/// the test reads to prove nothing was sent on its own.
fn resumed_daemon(dir: &std::path::Path, owner: OwnerId) -> (SessionRegistry, Arc<Mutex<Vec<u8>>>) {
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).expect("journal"));
    let registry = SessionRegistry::new(RuntimePaths::from_dir(dir), Some(Arc::clone(&journal)));
    let bytes = Arc::new(Mutex::new(Vec::new()));
    insert_live_agent_with_writer(
        &registry,
        SESSION,
        owner,
        Box::new(RecordingWriter(Arc::clone(&bytes))),
    );
    (registry, bytes)
}

fn written(bytes: &Arc<Mutex<Vec<u8>>>) -> String {
    String::from_utf8_lossy(&bytes.lock().expect("writer")).to_string()
}

/// Case (a): a close whose `queue_clear` the journal refused. The close
/// reported the failure and paused the kept state in memory; the restarted
/// daemon must restore that pause from the journal, so the surviving rows
/// wait for a user act exactly as they did before the restart.
#[test]
fn a_close_failed_discard_stays_paused_across_a_restart() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("queue.user", "queue.client");
    let runtime = insert_live_agent_with_writer(
        &registry,
        SESSION,
        owner.clone(),
        Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
    );
    runtime.begin_turn();
    let conn = ConnHandle::new(1);
    registry
        .queue_add(SESSION, "kept one", &[], &[], &owner, &conn)
        .expect("add");
    registry
        .queue_add(SESSION, "kept two", &[], &[], &owner, &conn)
        .expect("add");

    {
        let sabotage = rusqlite::Connection::open(dir.join("journal.db")).expect("raw handle");
        sabotage
            .execute_batch(
                "CREATE TRIGGER session_queue_clear_fails BEFORE DELETE ON session_queue
                 BEGIN SELECT RAISE(ABORT, 'simulated disk failure'); END;",
            )
            .expect("the failing store");
    }
    let no_peer: Option<ConnPeer> = None;
    registry
        .close(SESSION, &owner, &no_peer)
        .expect_err("the close reports the discard it could not finish");
    assert!(
        registry.queue_paused_for_test(SESSION),
        "the kept state is paused in memory"
    );
    // The trigger has done its one job; the restart must find a healthy
    // store, or no later write could prove anything.
    {
        let raw = rusqlite::Connection::open(dir.join("journal.db")).expect("raw handle");
        raw.execute_batch("DROP TRIGGER session_queue_clear_fails;")
            .expect("the trigger dropped");
    }
    drop(registry);
    drop(journal);

    let (resumed, resumed_bytes) = resumed_daemon(&dir, owner.clone());
    resumed.queue_restore_and_drain(SESSION);
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        written(&resumed_bytes).is_empty(),
        "a queue the close failed to discard stays paused across the restart"
    );
    assert!(
        resumed.queue_paused_for_test(SESSION),
        "the restored state carries the pause"
    );

    // A user act is what ends the pause — and then the queue drains.
    resumed
        .queue_edit(
            SESSION,
            "queue-1",
            "kept one, edited",
            &owner,
            &ConnHandle::new(2),
        )
        .expect("the user's edit is the act that unpauses");
    wait_until("the edited item to drain", || {
        written(&resumed_bytes).contains("kept one, edited")
    });
}

/// Case (b): a queue paused by a failed send. The parked item carries its
/// error and is not in flight, so no other fence protects it — the durable
/// pause must, or the restart would send a message whose send already
/// failed once, without a new user act.
#[test]
fn a_failed_send_pause_survives_a_restart() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("queue.user", "queue.client");
    // The writer refuses everything and the session is idle: the add's own
    // wake drains, fails, and parks the item at the front.
    insert_live_agent_with_writer(&registry, SESSION, owner.clone(), Box::new(FailingWriter));
    let conn = ConnHandle::new(1);
    registry
        .queue_add(SESSION, "doomed message", &[], &[], &owner, &conn)
        .expect("add");
    wait_until("the failed send to park the item", || {
        registry
            .queue_items_for_test(SESSION)
            .first()
            .is_some_and(|item| item.error.is_some())
    });
    assert!(
        registry.queue_paused_for_test(SESSION),
        "the failed send pauses the queue"
    );
    drop(registry);
    drop(journal);

    let (resumed, resumed_bytes) = resumed_daemon(&dir, owner.clone());
    resumed.queue_restore_and_drain(SESSION);
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        written(&resumed_bytes).is_empty(),
        "a send that already failed once is not retried by a restart"
    );
    assert!(
        resumed.queue_paused_for_test(SESSION),
        "the restored state carries the pause"
    );

    resumed
        .queue_edit(
            SESSION,
            "queue-1",
            "doomed, edited",
            &owner,
            &ConnHandle::new(2),
        )
        .expect("the user's edit is the act that unpauses");
    wait_until("the edited item to drain", || {
        written(&resumed_bytes).contains("doomed, edited")
    });
}
