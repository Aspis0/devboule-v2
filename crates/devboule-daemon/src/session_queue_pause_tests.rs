//! The pause that outlives the daemon (fix pass 3, P1-6) and the ordering
//! of its durable writes (fix pass 4): `paused` is the fence that keeps a
//! failed send — or a close that could not discard its queue — from ever
//! sending on its own, so it must survive a restart, and a newer failure's
//! pause must never be erased by an older unpause. The restart tests cross
//! a real journal reopen; the race is built with the writer's gate.

use super::tests::{
    attach_live_agent_for_test, ended_record, insert_live_agent_with_writer,
    install_end_turn_killer, test_owner, tmp_delete_registry, FailingWriter, RecordingWriter,
};
use super::{
    Arc, Barrier, ConnHandle, ConnPeer, Duration, ErrorCode, Instant, Journal, Mutex, Ordering,
    OwnerId, RuntimePaths, SessionRegistry,
};
use std::sync::atomic::AtomicBool;

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

/// The session row every queue write rides on: the journal refuses to store
/// queue text for a session it does not know (fix pass 4), and production
/// sessions always have their row before they can be queued for.
fn seed_row(journal: &Journal, owner: &OwnerId) {
    journal
        .create_session(ended_record(SESSION, &owner.user))
        .expect("the session row");
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
    seed_row(&journal, &owner);
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
    seed_row(&journal, &owner);
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

/// A writer that refuses while `fail` is set, and — when it would accept —
/// parks once inside `gate` so a test can run another actor mid-send, then
/// refuses after the gate releases it. The double fail-check is what lets
/// the drain's early failures stay fast while the send-now's write parks.
struct SwitchGateWriter {
    fail: Arc<AtomicBool>,
    entered: Arc<AtomicBool>,
    gate: Arc<Barrier>,
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl std::io::Write for SwitchGateWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.fail.load(Ordering::Acquire) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "switch gate writer refused",
            ));
        }
        if !self.entered.swap(true, Ordering::SeqCst) {
            self.gate.wait();
        }
        if self.fail.load(Ordering::Acquire) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "switch gate writer refused after the gate",
            ));
        }
        self.bytes.lock().expect("writer").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Fix pass 4, P1: the pause must be durable BEFORE the discard is
/// attempted, so a crash anywhere between the failed clear and the restart
/// leaves the surviving rows paused. The seam lets the restart happen with
/// the standalone pause write lost — exactly the crash window.
#[test]
fn a_failed_clear_is_fenced_before_anything_else() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("queue.user", "queue.client");
    seed_row(&journal, &owner);
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
            .expect("the failing clear");
    }
    let no_peer: Option<ConnPeer> = None;
    registry
        .close(SESSION, &owner, &no_peer)
        .expect_err("the close reports the discard it could not finish");

    {
        let raw = rusqlite::Connection::open(dir.join("journal.db")).expect("raw handle");
        raw.execute_batch("DROP TRIGGER session_queue_clear_fails;")
            .expect("the sabotage dropped");
    }
    drop(registry);
    drop(journal);

    let (resumed, resumed_bytes) = resumed_daemon(&dir, owner.clone());
    resumed.queue_restore_and_drain(SESSION);
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        written(&resumed_bytes).is_empty(),
        "a queue whose discard failed stays paused across the restart, \
         whatever happened to the pause write afterwards"
    );
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

/// Fix pass 4, P1: a client frame's unpause racing a failing send-now on a
/// non-front item. The unpause is issued while the send is parked; when the
/// send then fails, the failure's pause is the newest state and must win —
/// after a restart the queue is paused and the failed item is not retried
/// without a user act. Built with the writer's gate, not sleeps.
#[test]
fn a_frame_unpause_cannot_erase_a_newer_send_failure_pause() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("queue.user", "queue.client");
    seed_row(&journal, &owner);
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let fail = Arc::new(AtomicBool::new(true));
    let entered = Arc::new(AtomicBool::new(false));
    let gate = Arc::new(Barrier::new(2));
    let runtime = insert_live_agent_with_writer(
        &registry,
        SESSION,
        owner.clone(),
        Box::new(SwitchGateWriter {
            fail: Arc::clone(&fail),
            entered: Arc::clone(&entered),
            gate: Arc::clone(&gate),
            bytes: Arc::clone(&bytes),
        }),
    );
    let conn = attach_live_agent_for_test(&runtime, SESSION, 1);
    install_end_turn_killer(
        &registry,
        SESSION,
        &runtime,
        &Arc::new(Mutex::new(Vec::new())),
    );
    // The failed drain parks "first" at the front with its error and pauses
    // the queue; "second" queues behind it.
    registry
        .queue_add(SESSION, "first", &[], &[], &owner, &conn)
        .expect("add");
    wait_until("the failed drain to park the front", || {
        registry
            .queue_items_for_test(SESSION)
            .first()
            .is_some_and(|item| item.error.is_some())
    });
    registry
        .queue_add(SESSION, "second", &[], &[], &owner, &conn)
        .expect("add");
    let second = registry.queue_items_for_test(SESSION)[1].item_id.clone();
    assert_eq!(second, "queue-2");

    // Send-now on the non-front item: its claim lands, its write parks in
    // the gate.
    fail.store(false, Ordering::Release);
    let sending = {
        let registry = registry.clone();
        let owner = owner.clone();
        let conn = conn.clone();
        let second = second.clone();
        std::thread::spawn(move || {
            registry
                .queue_send_now(SESSION, conn.id, &second, &owner, &conn)
                .expect_err("the send-now fails after the gate")
        })
    };
    wait_until("the send-now's write to park", || {
        registry.queue_draining_for_test(SESSION)
    });

    // The client frame runs mid-send: it mutates the queue and its wake
    // stores the unpaused state under the queue lock, while the send is
    // still parked. Only then does the send fail — its settle holds the
    // lock after the wake did, so the failure's pause is the newest state
    // the journal ever sees.
    registry
        .queue_add(SESSION, "third", &[], &[], &owner, &conn)
        .expect("the frame lands while the send is in flight");
    fail.store(true, Ordering::Release);
    gate.wait();
    wait_until("the send-now's failure to settle", || {
        registry
            .queue_items_for_test(SESSION)
            .first()
            .is_some_and(|item| item.item_id == "queue-2" && item.error.is_some())
            && !registry.queue_draining_for_test(SESSION)
    });
    sending.join().expect("the send-now thread");

    // The restart: whatever the interleaving did to memory, the durable
    // state must be the newest one — paused, with the failed item at the
    // front, retried by nobody.
    drop(registry);
    drop(journal);

    fail.store(false, Ordering::Release);
    let (resumed, resumed_bytes) = resumed_daemon(&dir, owner.clone());
    resumed.queue_restore_and_drain(SESSION);
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        written(&resumed_bytes).is_empty(),
        "the failed send's pause is the newest state; a restart must not retry it"
    );
    resumed
        .queue_edit(
            SESSION,
            "queue-2",
            "second, edited",
            &owner,
            &ConnHandle::new(2),
        )
        .expect("the user's edit is the act that unpauses");
    wait_until("the edited item to drain", || {
        written(&resumed_bytes).contains("second, edited")
    });
}

/// Fix pass 4, P2: a frame whose unpause cannot be made durable must not
/// answer success — the caller would walk away from a queue that a crash
/// re-parks. The trigger refuses exactly the unpause write (the paused
/// store passes), so the refusal is the unpause's alone.
#[test]
fn a_frame_whose_unpause_cannot_be_stored_is_refused() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("queue.user", "queue.client");
    seed_row(&journal, &owner);
    insert_live_agent_with_writer(&registry, SESSION, owner.clone(), Box::new(FailingWriter));
    let conn = ConnHandle::new(1);
    registry
        .queue_add(SESSION, "doomed", &[], &[], &owner, &conn)
        .expect("add");
    wait_until("the failed drain to park the queue", || {
        registry.queue_paused_for_test(SESSION)
    });

    {
        let sabotage = rusqlite::Connection::open(dir.join("journal.db")).expect("raw handle");
        sabotage
            .execute_batch(
                "CREATE TRIGGER session_queue_unpause_fails
                 BEFORE UPDATE ON session_queue_state WHEN NEW.paused = 0
                 BEGIN SELECT RAISE(ABORT, 'simulated disk failure'); END;",
            )
            .expect("the failing unpause");
    }
    let error = registry
        .queue_edit(SESSION, "queue-1", "doomed, edited", &owner, &conn)
        .expect_err("the frame must not report success for an unstorable state");
    assert_eq!(error.code, ErrorCode::Journal, "{}", error.message);
    assert!(
        registry.queue_paused_for_test(SESSION),
        "the queue stays paused on both sides of the refusal"
    );

    {
        let raw = rusqlite::Connection::open(dir.join("journal.db")).expect("raw handle");
        raw.execute_batch("DROP TRIGGER session_queue_unpause_fails;")
            .expect("the sabotage dropped");
    }
    registry
        .queue_edit(SESSION, "queue-1", "doomed, edited", &owner, &conn)
        .expect("the retry lands once the store heals");
    let _ = dir;
    let _ = journal;
}
