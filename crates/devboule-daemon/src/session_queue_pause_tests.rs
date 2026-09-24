//! The pause that outlives the daemon (fix pass 3, P1-6) and the ordering
//! of its durable writes (fix pass 4): `paused` is the fence that keeps a
//! failed send — or a close that could not discard its queue — from ever
//! sending on its own, so it must survive a restart, and a newer failure's
//! pause must never be erased by an older unpause. The restart tests cross
//! a real journal reopen; the race is built with the writer's gate.

use super::tests::{
    attach_live_agent_for_test, drain, ended_record, insert_live_agent_with_writer,
    install_end_turn_killer, test_owner, tmp_delete_registry, FailingWriter, RecordingWriter,
};
use super::{
    Arc, Barrier, ConnHandle, ConnPeer, Duration, ErrorCode, Instant, Journal, Mutex, Ordering,
    OwnerId, RuntimePaths, SessionEvent, SessionRegistry,
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

/// A writer that refuses while `fail` is set and accepts otherwise — the
/// timeout test parks the queue with it set and drains with it cleared.
struct FailSwitchWriter {
    fail: Arc<AtomicBool>,
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl std::io::Write for FailSwitchWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.fail.load(Ordering::Acquire) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "fail switch writer refused",
            ));
        }
        self.bytes.lock().expect("writer").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
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

/// Fix pass 5, P2: an edit that committed is never reported as failed. The
/// trigger refuses exactly the unpause write (the paused store passes), so
/// the frame answers with its edit done and the queue still paused — the
/// same answer send-now gives — instead of an error a client retry would
/// answer with a duplicate item. The edit clears the item's error, so the
/// wire snapshot after this failure shows an edited queue with no pause
/// marker; saying "paused" on the wire would need a protocol field, which
/// this pass reports instead of adding.
#[test]
fn a_committed_edit_is_answered_even_when_its_unpause_cannot_be_stored() {
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
    registry
        .queue_edit(SESSION, "queue-1", "doomed, edited", &owner, &conn)
        .expect("the committed edit is not reported as a failure");
    let items = registry.queue_items_for_test(SESSION);
    assert_eq!(items[0].text, "doomed, edited", "the edit is done");
    assert!(
        registry.queue_paused_for_test(SESSION),
        "the queue is still paused"
    );

    {
        let raw = rusqlite::Connection::open(dir.join("journal.db")).expect("raw handle");
        raw.execute_batch("DROP TRIGGER session_queue_unpause_fails;")
            .expect("the sabotage dropped");
        let paused: i64 = raw
            .query_row(
                "SELECT paused FROM session_queue_state WHERE session_id = ?1",
                [SESSION],
                |row| row.get(0),
            )
            .expect("the durable pause");
        assert_eq!(paused, 1, "the disk still parks the queue");
    }
    let _ = dir;
    let _ = journal;
}

/// Fix pass 5, P1: a fence that fails must stop the close before anything
/// irreversible. The session stays open — attach still answers for it — a
/// later resume sends nothing while its turn runs, and a retry of the close
/// once the store heals discards the queue for real.
#[test]
fn a_failed_fence_refuses_the_close_and_leaves_the_session_open() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("queue.user", "queue.client");
    seed_row(&journal, &owner);
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let runtime = insert_live_agent_with_writer(
        &registry,
        SESSION,
        owner.clone(),
        Box::new(RecordingWriter(Arc::clone(&bytes))),
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
                "CREATE TRIGGER session_queue_fence_fails
                 BEFORE UPDATE ON session_queue_state
                 BEGIN SELECT RAISE(ABORT, 'simulated disk failure'); END;",
            )
            .expect("the failing fence");
    }
    let no_peer: Option<ConnPeer> = None;
    let error = registry
        .close(SESSION, &owner, &no_peer)
        .expect_err("the fence failed; the close must be refused");
    assert_eq!(error.code, ErrorCode::Journal, "{}", error.message);

    // The session is still open: attach answers for it, which only a live
    // registry entry does.
    let live_conn = Arc::new(ConnHandle::new(7));
    registry
        .attach_with_subscription(SESSION, live_conn.id, None, &live_conn, &owner, false)
        .expect("the refused close left the session open");

    // A later resume sends nothing: the queue is the user's live queue and
    // its turn is still running.
    registry.queue_restore_and_drain(SESSION);
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        written(&bytes).is_empty(),
        "the refused close never sent the queue"
    );

    // Once the store heals, the same close succeeds and the queue is gone.
    {
        let raw = rusqlite::Connection::open(dir.join("journal.db")).expect("raw handle");
        raw.execute_batch("DROP TRIGGER session_queue_fence_fails;")
            .expect("the sabotage dropped");
    }
    registry
        .close(SESSION, &owner, &no_peer)
        .expect("the retried close discards the queue");
    assert!(registry.queue_items_for_test(SESSION).is_empty());
}

/// Fix pass 5, P1: a timed-out unpause store is not a failure — the command
/// is already in the journal and may still commit. Memory stays as the user
/// asked (unpaused), and when the late commit lands, memory and disk agree.
/// The journal loop is occupied, deterministically, by a giant append the
/// hook enqueues right before the store: the store sits behind it past the
/// shrunken rpc wait, then commits late.
#[test]
fn a_timed_out_unpause_store_keeps_memory_and_later_agrees_with_disk() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("queue.user", "queue.client");
    seed_row(&journal, &owner);
    // The writer refuses first — on an idle session the add's own wake
    // drains, fails, and stores the durable pause — and flips to accepting
    // when the test lets the parked item drain.
    let fail = Arc::new(AtomicBool::new(true));
    let bytes = Arc::new(Mutex::new(Vec::<u8>::new()));
    let runtime = insert_live_agent_with_writer(
        &registry,
        SESSION,
        owner.clone(),
        Box::new(FailSwitchWriter {
            fail: Arc::clone(&fail),
            bytes: Arc::clone(&bytes),
        }),
    );
    let conn = ConnHandle::new(1);
    registry
        .queue_add(SESSION, "parked", &[], &[], &owner, &conn)
        .expect("add");
    wait_until("the failed drain to park the queue", || {
        registry.queue_paused_for_test(SESSION)
    });
    // The turn holds the drain off from here on, so the only stores in
    // flight are this test's.
    runtime.begin_turn();

    // The unpause store will time out: the rpc wait shrinks, and the hook
    // parks a giant append in front of it — the loop grinds through the
    // payload for longer than the wait before the store is even touched.
    registry.set_rpc_wait_for_test(Duration::from_millis(100));
    registry.set_before_queue_unpause_store_hook(Arc::new({
        let journal = Arc::clone(&journal);
        move || {
            // Two heavy appends: the loop grinds through crc32, the blob
            // writes and the snapshot pass for far longer than the shrunken
            // wait before the unpause store is even dequeued.
            for seq in [100, 101] {
                assert!(
                    journal.try_append(crate::journal::EventRecord {
                        session_id: SESSION.to_string(),
                        generation: 1,
                        seq,
                        kind: crate::journal::EventKind::Output,
                        ts_ms: std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .expect("clock")
                            .as_millis() as u64,
                        payload: vec![0x20; 16 * 1024 * 1024],
                    }),
                    "the loop-occupying append is accepted"
                );
            }
        }
    }));

    registry
        .queue_edit(SESSION, "queue-1", "parked, edited", &owner, &conn)
        .expect("the edit is durable; the timeout is not reported as its failure");

    // The timeout did not undo the user's act in memory.
    assert!(
        !registry.queue_paused_for_test(SESSION),
        "a timeout is not a failure: memory stays as the user asked"
    );

    // The late commit: once the loop is through the giant append, the
    // queued store lands, and disk agrees with memory.
    wait_until("the late commit to land", || {
        let raw = rusqlite::Connection::open(dir.join("journal.db")).expect("raw handle");
        let paused: i64 = raw
            .query_row(
                "SELECT paused FROM session_queue_state WHERE session_id = ?1",
                [SESSION],
                |row| row.get(0),
            )
            .expect("the state row");
        paused == 0
    });
    assert!(
        !registry.queue_paused_for_test(SESSION),
        "memory and disk agree after the late commit"
    );

    // Fix pass 6, P2: the timed-out unpause still scheduled the drain — the
    // frame's turn ends and the edited item is sent, not stranded behind a
    // wait nobody retries.
    fail.store(false, Ordering::Release);
    runtime.publish_agent_event(
        SessionEvent::AgentFinished {
            stop_reason: "end_turn".to_string(),
            model_id: None,
            usage: None,
        },
        None,
    );
    wait_until("the edited item to drain after the timeout", || {
        written(&bytes).contains("parked, edited")
    });
    assert!(
        registry.queue_items_for_test(SESSION).is_empty(),
        "the drained item left the queue"
    );
}

/// Fix pass 6, P1: the timeout rule on every queue store. A `queue_add`
/// whose store sits behind a slow journal write times out — the client is
/// not told it failed, memory keeps the queued message, the snapshot says
/// so, the late commit lands, and a simulated restart resumes exactly what
/// the client saw.
#[test]
fn a_timed_out_add_commits_late_and_the_client_was_not_told_it_failed() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("queue.user", "queue.client");
    seed_row(&journal, &owner);
    let bytes = Arc::new(Mutex::new(Vec::<u8>::new()));
    let runtime = insert_live_agent_with_writer(
        &registry,
        SESSION,
        owner.clone(),
        Box::new(RecordingWriter(Arc::clone(&bytes))),
    );
    let conn = attach_live_agent_for_test(&runtime, SESSION, 1);
    runtime.begin_turn();

    // A first add loads the queue state into memory, so the timed-out add
    // below goes straight to its store instead of timing out on the load.
    registry
        .queue_add(SESSION, "first", &[], &[], &owner, &conn)
        .expect("the pre-load add");

    // The occupancy lever, applied to an add: the giant append grinds the
    // loop for longer than the shrunken rpc wait, and the add's store queues
    // behind it.
    registry.set_rpc_wait_for_test(Duration::from_millis(100));
    assert!(
        journal.try_append(crate::journal::EventRecord {
            session_id: SESSION.to_string(),
            generation: 1,
            seq: 100,
            kind: crate::journal::EventKind::Output,
            ts_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_millis() as u64,
            payload: vec![0x20; 16 * 1024 * 1024],
        }),
        "the loop-occupying append is accepted"
    );

    registry
        .queue_add(SESSION, "late commit", &[], &[], &owner, &conn)
        .expect("the client is not told a timed-out add failed");
    let items = registry.queue_items_for_test(SESSION);
    assert_eq!(items.len(), 2, "memory keeps the queued message");
    assert_eq!(items[1].text, "late commit");

    // The snapshot was published with the message the client asked for.
    let snapshot = drain(&conn)
        .into_iter()
        .rev()
        .find_map(|event| match event {
            SessionEvent::QueueSnapshot { items } => Some(items),
            _ => None,
        })
        .expect("the add published its snapshot");
    assert_eq!(snapshot.len(), 2);
    assert_eq!(snapshot[1].text, "late commit");

    // The late commit lands, and a simulated restart resumes exactly what
    // the client saw: the same single message.
    wait_until("the add's late commit to land", || {
        let raw = rusqlite::Connection::open(dir.join("journal.db")).expect("raw handle");
        let rows: i64 = raw
            .query_row(
                "SELECT COUNT(*) FROM session_queue WHERE session_id = ?1",
                [SESSION],
                |row| row.get(0),
            )
            .expect("count the rows");
        rows == 2
    });
    drop(registry);
    drop(journal);
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).expect("journal"));
    let registry = SessionRegistry::new(RuntimePaths::from_dir(&dir), Some(Arc::clone(&journal)));
    insert_live_agent_with_writer(&registry, SESSION, owner.clone(), Box::new(FailingWriter));
    registry.queue_restore_and_drain(SESSION);
    let items = registry.queue_items_for_test(SESSION);
    assert_eq!(
        items.len(),
        2,
        "the resumed queue matches the client's view"
    );
    assert_eq!(items[0].text, "first");
    assert_eq!(items[1].text, "late commit");
}
