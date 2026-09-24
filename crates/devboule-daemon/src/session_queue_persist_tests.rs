//! The queue across a daemon restart: items survive, a close discards them,
//! and a restored queue drains once its session is live and idle. The restart
//! is real here — the journal shuts down, a second daemon opens the same file
//! — because "survives a restart" is the claim under test.
//! Written first, per the Q1 brief's red-first rule.

use super::tests::{
    insert_live_agent_with_writer, test_owner, tmp_delete_registry, GatedWriter, RecordingWriter,
};
use super::{
    new_session_record, Arc, Barrier, ConnHandle, ConnPeer, Duration, Instant, Journal, Mutex,
    OwnerId, RuntimePaths, SessionEvent, SessionKind, SessionRegistry, SessionRuntime,
};
use std::sync::atomic::{AtomicBool, Ordering};

const SESSION: &str = "s.q.persist";

struct Handles {
    registry: SessionRegistry,
    owner: OwnerId,
    conn: Arc<ConnHandle>,
    runtime: Arc<SessionRuntime>,
    journal: Arc<Journal>,
    bytes: Arc<Mutex<Vec<u8>>>,
}

/// A daemon over `dir`'s journal: live agent, recording writer, no attach —
/// the tests attach (when they do) through the registry, the way the app
/// does. The turn is the test's dial: running so nothing drains while items
/// are queued, idle so a restored queue may drain.
fn daemon_over(dir: &std::path::Path, turn_running: bool) -> Handles {
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).expect("journal"));
    let registry = SessionRegistry::new(RuntimePaths::from_dir(dir), Some(Arc::clone(&journal)));
    let owner = test_owner("queue.user", "queue.client");
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let runtime = insert_live_agent_with_writer(
        &registry,
        SESSION,
        owner.clone(),
        Box::new(RecordingWriter(Arc::clone(&bytes))),
    );
    let conn = ConnHandle::new(1);
    if turn_running {
        runtime.begin_turn();
    }
    Handles {
        registry,
        owner,
        conn,
        runtime,
        journal,
        bytes,
    }
}

impl Handles {
    /// Attach the way the app does — through the registry, which is where
    /// the queue is handed to a late client. The direct test helper would
    /// bypass exactly that, which is how this assertion first failed.
    fn attach(&self) {
        self.registry
            .attach_with_subscription(SESSION, self.conn.id, None, &self.conn, &self.owner, false)
            .expect("attach through the registry");
    }
}

fn written(bytes: &Arc<Mutex<Vec<u8>>>) -> String {
    String::from_utf8_lossy(&bytes.lock().expect("writer")).to_string()
}

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

#[test]
fn the_queue_survives_a_restart_and_drains_once_its_session_is_live_and_idle() {
    let (dir, first_registry, first_journal) = tmp_delete_registry();
    drop(first_registry);
    let first = daemon_over(&dir, true);
    // The row `insert_live_agent` never writes: without it the journal
    // refuses the queue_snapshot publishes, and the attach half of "the
    // queue reaches a late client" would have nothing to replay.
    first
        .journal
        .create_session(new_session_record(
            SESSION,
            &first.owner.user,
            None,
            SessionKind::Acp,
            "Agent",
        ))
        .expect("journal row");
    first
        .registry
        .queue_add(SESSION, "kept one", &[], &[], &first.owner, &first.conn)
        .expect("add");
    first
        .registry
        .queue_add(SESSION, "kept two", &[], &[], &first.owner, &first.conn)
        .expect("add");
    assert_eq!(first.registry.queue_items_for_test(SESSION).len(), 2);
    // The barrier before the restart: `try_append` is asynchronous, so the
    // snapshots this queue published are committed only when the writer says
    // they are — without this, the second daemon could read a queue whose
    // rows were still in the first writer's channel.
    first.journal.flush().expect("queue snapshots flushed");
    drop(first_journal);
    drop(first);

    // The restart: the first daemon's journal is gone, a second daemon opens
    // the same file, and its session comes back live with no turn — "a
    // restored queue drains once its session is live and idle".
    let second = daemon_over(&dir, false);
    // A resume opens the NEXT generation (`session_resume.rs`: `record
    // .generation.saturating_add(1)`), and this helper inserts at generation
    // 1 like a first spawn: without the same bump, this runtime's seqs would
    // fight the rows the first daemon wrote for the same (generation, seq).
    second.runtime.set_generation(2);
    second.attach();
    assert!(
        written(&second.bytes).is_empty(),
        "the second daemon starts with an empty writer"
    );
    // What the second daemon's own attach was handed before anything was
    // asked of it: the whole queue as the first daemon left it — the "queue
    // in whatever the app receives on attach" half of Q1. The rows the first
    // daemon journalled can only ever replay *one* item here (the fresh
    // runtime's watermark starts below them and extends only to its own
    // current seq), so a snapshot with BOTH items can only come from the
    // attach hand-over itself.
    let mut saw_full_queue = false;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !saw_full_queue {
        for event in super::tests::drain(&second.conn) {
            if let SessionEvent::QueueSnapshot { items } = event {
                saw_full_queue = items.len() == 2;
            }
        }
        if !saw_full_queue {
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    assert!(saw_full_queue, "a late attach is handed the whole queue");
    second.registry.queue_restore_and_drain(SESSION);

    wait_until("the first restored item", || {
        written(&second.bytes).contains("kept one")
    });
    // One at a time applies to restored items too: the second waits for the
    // turn the first drain started to end, which is this test's job to end.
    second.runtime.publish_agent_event(
        SessionEvent::AgentFinished {
            stop_reason: "end_turn".to_string(),
            model_id: None,
            usage: None,
        },
        None,
    );
    wait_until("the second restored item", || {
        written(&second.bytes).contains("kept two")
    });
    let text = written(&second.bytes);
    assert!(
        text.find("kept one").expect("one") < text.find("kept two").expect("two"),
        "restoration keeps the order: {text:?}"
    );
    wait_until("the restored queue to empty", || {
        second.registry.queue_items_for_test(SESSION).is_empty()
    });
    assert!(
        second.runtime.is_turn_active(second.runtime.turn_counter()),
        "the last drain's own turn is running"
    );
}

#[test]
fn closing_a_session_with_items_queued_discards_them_before_and_after_a_restart() {
    let (dir, first_registry, first_journal) = tmp_delete_registry();
    drop(first_registry);
    let first = daemon_over(&dir, true);
    first
        .journal
        .create_session(new_session_record(
            SESSION,
            &first.owner.user,
            None,
            SessionKind::Acp,
            "Agent",
        ))
        .expect("journal row");
    first
        .registry
        .queue_add(
            SESSION,
            "about to close",
            &[],
            &[],
            &first.owner,
            &first.conn,
        )
        .expect("add");
    first
        .registry
        .queue_add(
            SESSION,
            "and this one too",
            &[],
            &[],
            &first.owner,
            &first.conn,
        )
        .expect("add");
    assert_eq!(first.registry.queue_items_for_test(SESSION).len(), 2);

    let no_peer: Option<ConnPeer> = None;
    let removed = first
        .registry
        .close(SESSION, &first.owner, &no_peer)
        .expect("close");
    assert!(removed, "a live entry closes");
    assert!(
        first.registry.queue_items_for_test(SESSION).is_empty(),
        "closing discards what was queued, and says so in memory at once"
    );
    drop(first_journal);
    drop(first);

    // Same rule after a restart: the rows went with the close, so a fresh
    // daemon finds nothing to restore and nothing to drain.
    let second = daemon_over(&dir, false);
    second.registry.queue_restore_and_drain(SESSION);
    assert!(second.registry.queue_items_for_test(SESSION).is_empty());
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        written(&second.bytes).is_empty(),
        "a closed session's queue must not come back from the journal"
    );
}

/// Fix pass 1, P1-4 (the delivery decision: never lost, never resent
/// silently): a claim that was in flight when the daemon died comes back at
/// the front of the restored queue, paused, and nothing resends it — the
/// journal keeps the row until the provider accepted the prompt.
#[test]
fn a_crash_mid_claim_comes_back_paused_and_is_never_resent() {
    let (dir, first_registry, first_journal) = tmp_delete_registry();
    drop(first_registry);
    let journal = Arc::new(Journal::open(&dir.join("journal.db")).expect("journal"));
    let registry = SessionRegistry::new(RuntimePaths::from_dir(&dir), Some(Arc::clone(&journal)));
    let owner = test_owner("queue.user", "queue.client");
    journal
        .create_session(new_session_record(
            SESSION,
            &owner.user,
            None,
            SessionKind::Acp,
            "the crash claim's session",
        ))
        .expect("the session row the claim's store rides on");
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let entered = Arc::new(AtomicBool::new(false));
    let gate = Arc::new(Barrier::new(2));
    let runtime = insert_live_agent_with_writer(
        &registry,
        SESSION,
        owner.clone(),
        Box::new(GatedWriter {
            entered: Arc::clone(&entered),
            gate: Arc::clone(&gate),
            bytes: Arc::clone(&bytes),
        }),
    );
    let conn = ConnHandle::new(1);
    let _ = runtime;
    drop(first_journal);

    // The session is idle, so this add's wake drains immediately and the
    // claim parks inside the writer — the crash window.
    let adding = {
        let registry = registry.clone();
        let owner = owner.clone();
        let conn = Arc::clone(&conn);
        std::thread::spawn(move || {
            registry
                .queue_add(SESSION, "in flight when it died", &[], &[], &owner, &conn)
                .expect("the add reaches the writer before any crash")
        })
    };
    let deadline = Instant::now() + Duration::from_secs(10);
    while !entered.load(Ordering::SeqCst) {
        assert!(
            Instant::now() < deadline,
            "the drain never reached the writer"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        registry.queue_draining_for_test(SESSION),
        "the claim is in flight"
    );

    // The crash: this registry is abandoned with the send parked, and a
    // second daemon opens the same journal — the resume road's world.
    let second = daemon_over(&dir, false);
    second.registry.queue_restore_and_drain(SESSION);

    let items = second.registry.queue_items_for_test(SESSION);
    assert_eq!(
        items.len(),
        1,
        "a claimed item is never lost: the journal still holds it"
    );
    assert_eq!(items[0].text, "in flight when it died");
    assert!(
        items[0].unconfirmed,
        "flagged for the UI: it may not have been sent"
    );
    assert!(
        second.registry.queue_paused_for_test(SESSION),
        "it comes back paused"
    );
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        written(&second.bytes).is_empty(),
        "nothing resends it automatically"
    );

    // Release the dead daemon's writer only after the restart's assertions;
    // nothing below this line asserts.
    gate.wait();
    let _ = adding.join();
}
