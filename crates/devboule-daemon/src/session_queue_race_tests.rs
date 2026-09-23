//! The races the Q1 brief names: a turn ending while a client changes the
//! queue, a client reaching for the item the drain is taking, two clients
//! changing the queue at once — plus send-now with no turn running, which is
//! the plain send the interrupt behaviour promises.
//!
//! These pin invariants the drain was written with, so they were green on
//! their first run; the red log for them (`q1-races-red.txt`) comes from the
//! claim-ordering mutation described in the coder report, which is what the
//! second test fails without.

use super::tests::{
    attach_live_agent_for_test, drain, insert_live_agent_with_writer, test_owner,
    tmp_delete_registry, RecordingWriter,
};
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

const SESSION: &str = "s.q.race";

struct Harness {
    registry: SessionRegistry,
    owner: OwnerId,
    conn: Arc<ConnHandle>,
    runtime: Arc<SessionRuntime>,
    bytes: Arc<Mutex<Vec<u8>>>,
}

fn harness() -> Harness {
    let (_dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("queue.user", "queue.client");
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let runtime = insert_live_agent_with_writer(
        &registry,
        SESSION,
        owner.clone(),
        Box::new(RecordingWriter(Arc::clone(&bytes))),
    );
    let conn = attach_live_agent_for_test(&runtime, SESSION, 1);
    Harness {
        registry,
        owner,
        conn,
        runtime,
        bytes,
    }
}

impl Harness {
    fn add(&self, text: &str) {
        self.registry
            .queue_add(SESSION, text, &[], &[], &self.owner, &self.conn)
            .expect("add");
    }

    fn written(&self) -> String {
        String::from_utf8_lossy(&self.bytes.lock().expect("writer")).to_string()
    }

    fn finish_turn(&self) {
        self.runtime.publish_agent_event(
            SessionEvent::AgentFinished {
                stop_reason: "end_turn".to_string(),
                model_id: None,
                usage: None,
            },
            None,
        );
    }

    /// End turns until the queue has drained — the test standing in for the
    /// person who keeps the conversation going. Each pass either observes an
    /// empty queue or ends whatever turn the drain is waiting on, so the
    /// deadline can only trip on a queue that truly stopped.
    fn drive_to_empty(&self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if self.registry.queue_items_for_test(SESSION).is_empty() {
                return;
            }
            if self.runtime.is_turn_active(self.runtime.turn_counter()) {
                std::thread::sleep(Duration::from_millis(50));
                if !self.registry.queue_items_for_test(SESSION).is_empty() {
                    self.finish_turn();
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!(
            "the queue never emptied: {:?}",
            self.registry.queue_items_for_test(SESSION)
        );
    }
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

/// A writer that parks its first write until the test releases the gate —
/// the window in which "the item the drain is taking" is observably in
/// flight and out of the queue at the same time.
struct GatedWriter {
    entered: Arc<AtomicBool>,
    gate: Arc<Barrier>,
    bytes: Arc<Mutex<Vec<u8>>>,
}

impl Write for GatedWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if !self.entered.swap(true, Ordering::SeqCst) {
            self.gate.wait();
        }
        self.bytes.lock().expect("writer").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_turn_ending_while_a_client_adds_loses_neither_message() {
    let h = harness();
    h.add("first message");
    wait_until("the first item to drain", || {
        h.written().contains("first message")
    });
    h.add("second message");
    assert_eq!(h.registry.queue_items_for_test(SESSION).len(), 1);

    // The race: the turn ends (which fires the drain's hook) at the same
    // moment a client appends. Neither may be lost, and FIFO must hold.
    let racing_add = {
        let registry = h.registry.clone();
        let owner = h.owner.clone();
        let conn = Arc::clone(&h.conn);
        std::thread::spawn(move || {
            registry
                .queue_add(SESSION, "third message", &[], &[], &owner, &conn)
                .expect("the racing add");
        })
    };
    h.finish_turn();
    racing_add.join().expect("the racing add thread");
    h.drive_to_empty();

    let text = h.written();
    let first = text.find("first message").expect("first");
    let second = text.find("second message").expect("second");
    let third = text.find("third message").expect("third");
    assert!(
        first < second && second < third,
        "FIFO across the race: {text:?}"
    );
}

#[test]
fn a_client_reaching_for_the_item_the_drain_is_taking_finds_it_already_gone() {
    let (_dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("queue.user", "queue.client");
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
    let conn = attach_live_agent_for_test(&runtime, SESSION, 1);

    // The add's synchronous wake carries the drain, so it must run on its
    // own thread: the writer parks it mid-send, which is the window.
    let adding = {
        let registry = registry.clone();
        let owner = owner.clone();
        let conn = Arc::clone(&conn);
        std::thread::spawn(move || {
            registry
                .queue_add(SESSION, "the claimed message", &[], &[], &owner, &conn)
                .expect("add")
        })
    };
    wait_until("the drain to reach the writer", || {
        entered.load(Ordering::SeqCst)
    });

    // Mid-send: the claim already took the item out and published that, so
    // both mutations find no such item — deterministically, not by timing.
    let edit = registry
        .queue_edit(SESSION, "queue-1", "edited mid-flight", &owner, &conn)
        .expect_err("the item is not in the queue any more");
    assert_eq!(edit.code, ErrorCode::InvalidRequest);
    let remove = registry
        .queue_remove(SESSION, "queue-1", &owner, &conn)
        .expect_err("the item is not in the queue any more");
    assert_eq!(remove.code, ErrorCode::InvalidRequest);
    assert!(registry.queue_items_for_test(SESSION).is_empty());

    gate.wait();
    adding
        .join()
        .expect("the add thread finishes the drain's send");
    wait_until("the claimed item to land", || {
        String::from_utf8_lossy(&bytes.lock().expect("writer")).contains("the claimed message")
    });
    assert!(registry.queue_items_for_test(SESSION).is_empty());
}

#[test]
fn two_clients_changing_the_queue_at_once_lose_nothing() {
    let h = harness();
    h.runtime.begin_turn(); // hold the drain; this test is about the edits
    let threads: Vec<_> = ["left", "right"]
        .into_iter()
        .map(|side| {
            let registry = h.registry.clone();
            let owner = h.owner.clone();
            let conn = Arc::clone(&h.conn);
            std::thread::spawn(move || {
                for index in 0..10 {
                    registry
                        .queue_add(SESSION, &format!("{side} {index}"), &[], &[], &owner, &conn)
                        .expect("concurrent add");
                }
            })
        })
        .collect();
    for thread in threads {
        thread.join().expect("a concurrent client");
    }

    let items = h.registry.queue_items_for_test(SESSION);
    assert_eq!(items.len(), 20, "no add was lost to an interleaving");
    let unique: std::collections::HashSet<&str> =
        items.iter().map(|item| item.item_id.as_str()).collect();
    assert_eq!(unique.len(), 20, "every item kept its own id");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        h.registry.queue_items_for_test(SESSION).len(),
        20,
        "and the running turn held the drain throughout"
    );
    // The last snapshot is the whole queue — twenty rows, one per add.
    let snapshots: Vec<_> = drain(&h.conn)
        .into_iter()
        .filter_map(|event| match event {
            SessionEvent::QueueSnapshot { items } => Some(items),
            _ => None,
        })
        .collect();
    assert_eq!(
        snapshots.last().map(Vec::len),
        Some(20),
        "the last published snapshot is the settled queue"
    );
}

#[test]
fn send_now_with_no_turn_running_is_a_plain_send() {
    let h = harness();
    h.runtime.begin_turn();
    h.add("urgent message");
    assert_eq!(h.registry.queue_items_for_test(SESSION).len(), 1);
    // Park the drain, then end the turn: the hook fires into the pause, so
    // the item stays queued with the session idle — the state only a client's
    // own action could also produce.
    h.registry.queue_park_for_test(SESSION);
    h.finish_turn();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(h.registry.queue_items_for_test(SESSION).len(), 1);

    h.registry
        .queue_send_now(SESSION, h.conn.id, "queue-1", &h.owner, &h.conn)
        .expect("send-now with nothing running");

    assert!(
        h.written().contains("urgent message"),
        "the plain send went out"
    );
    assert!(h.registry.queue_items_for_test(SESSION).is_empty());
    assert!(
        !drain(&h.conn).iter().any(|event| matches!(
            event,
            SessionEvent::SessionNotice { text, .. } if text.contains("interrupted")
        )),
        "nothing was running, so nothing was interrupted and no notice is invented"
    );
}
