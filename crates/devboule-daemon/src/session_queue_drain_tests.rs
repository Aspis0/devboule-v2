//! The drain: FIFO, one at a time, and only at a turn's end; a failed send
//! parking the item at the front with its error until a client's next frame;
//! and send-now leaving the rest of the queue for the turn it started.
//! Written first, per the Q1 brief's red-first rule.

use super::tests::{
    attach_live_agent_for_test, drain, insert_live_agent_with_writer, install_end_turn_killer,
    test_owner, tmp_delete_registry,
};
use super::{
    new_session_record, Arc, Barrier, ConnHandle, Duration, Instant, Mutex, OwnerId, PersistStatus,
    SessionEvent, SessionKind, SessionRegistry, SessionRuntime, Write,
};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const SESSION: &str = "s.q.2";

/// The session row every queue store rides on (fix pass 4's existence
/// check): a production session always has its row before it can be queued
/// for, and these harnesses say so too.
fn ended_row(owner: &str) -> crate::journal::SessionRecord {
    let mut record = new_session_record(SESSION, owner, None, SessionKind::Acp, "queue harness");
    record.status = PersistStatus::Ended;
    record
}

/// A writer that records what it accepts and refuses every write while
/// `fail` is set — one switch, so a test can park a drain and release it.
/// `attempts` counts every call, refused ones included: P1-5's test pins
/// "no automatic retry" by the attempt count, not by the end state.
struct SwitchWriter {
    bytes: Arc<Mutex<Vec<u8>>>,
    fail: Arc<AtomicBool>,
    attempts: Arc<AtomicU64>,
}

impl Write for SwitchWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.attempts.fetch_add(1, Ordering::AcqRel);
        if self.fail.load(Ordering::Acquire) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "switch writer refused",
            ));
        }
        self.bytes.lock().expect("writer").extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

struct Harness {
    registry: SessionRegistry,
    owner: OwnerId,
    conn: Arc<ConnHandle>,
    runtime: Arc<SessionRuntime>,
    bytes: Arc<Mutex<Vec<u8>>>,
    fail: Arc<AtomicBool>,
    attempts: Arc<AtomicU64>,
}

fn harness(fail: bool) -> Harness {
    let (_dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("queue.user", "queue.client");
    _journal
        .create_session(ended_row(&owner.user))
        .expect("the session row");
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let flag = Arc::new(AtomicBool::new(fail));
    let attempts = Arc::new(AtomicU64::new(0));
    let runtime = insert_live_agent_with_writer(
        &registry,
        SESSION,
        owner.clone(),
        Box::new(SwitchWriter {
            bytes: Arc::clone(&bytes),
            fail: Arc::clone(&flag),
            attempts: Arc::clone(&attempts),
        }),
    );
    let conn = attach_live_agent_for_test(&runtime, SESSION, 1);
    // Send-now interrupts: the fake provider has to end its turn like one.
    install_end_turn_killer(&registry, SESSION, &runtime, &bytes);
    Harness {
        registry,
        owner,
        conn,
        runtime,
        bytes,
        fail: flag,
        attempts,
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
fn the_drain_sends_fifo_one_at_a_time_and_only_at_a_turn_end() {
    let h = harness(false);
    h.add("first message");
    // Live and idle: the first item goes out as a plain turn by itself.
    wait_until("the first item to drain", || {
        h.written().contains("first message")
    });
    assert!(h.runtime.is_turn_active(h.runtime.turn_counter()));

    h.add("second message");
    assert_eq!(
        h.registry.queue_items_for_test(SESSION).len(),
        1,
        "the second item waits its turn"
    );
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        !h.written().contains("second message"),
        "one at a time: nothing new sends while the turn runs"
    );

    // Only the turn's end starts the next send.
    h.finish_turn();
    wait_until("the second item to drain", || {
        h.written().contains("second message")
    });
    let text = h.written();
    assert!(
        text.find("first message").expect("first") < text.find("second message").expect("second"),
        "FIFO: {text:?}"
    );
    assert!(h.registry.queue_items_for_test(SESSION).is_empty());
}

#[test]
fn send_now_leaves_the_rest_of_the_queue_for_the_turn_it_started() {
    let h = harness(false);
    h.add("alpha message");
    wait_until("the first item to drain", || {
        h.written().contains("alpha message")
    });
    h.add("beta message");
    h.add("gamma message");
    let beta = h.registry.queue_items_for_test(SESSION)[0].item_id.clone();
    h.registry
        .queue_send_now(SESSION, h.conn.id, &beta, &h.owner, &h.conn)
        .expect("send-now");

    let remaining = h.registry.queue_items_for_test(SESSION);
    assert_eq!(remaining.len(), 1, "only gamma is left");
    assert_eq!(remaining[0].text, "gamma message");
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        !h.written().contains("gamma message"),
        "the rest waits for the running turn to end"
    );

    h.finish_turn();
    wait_until("gamma to drain", || h.written().contains("gamma message"));
    assert!(h.registry.queue_items_for_test(SESSION).is_empty());
}

#[test]
fn a_failed_drain_send_parks_the_item_at_the_front_until_a_client_sends_a_frame() {
    let h = harness(true);

    // The write is refused, so this add wakes a drain that fails: the item
    // stays at the front with its error, and the snapshot says so.
    h.add("doomed message");
    let items = h.registry.queue_items_for_test(SESSION);
    assert_eq!(items.len(), 1, "the item was not lost");
    assert!(items[0].error.is_some(), "the error rides the item");
    assert!(
        h.registry.queue_paused_for_test(SESSION),
        "a failed send pauses"
    );
    assert_eq!(
        last_snapshot(&drain(&h.conn)),
        items,
        "the snapshot carries it"
    );

    // Another frame whose drain fails the same way: the frame resumes
    // draining, and the refusal parks it again.
    h.add("second message");
    assert!(h.registry.queue_paused_for_test(SESSION));

    // The writer recovers; the next client frame resumes the drain, which
    // takes the front item first, in order, one turn at a time.
    h.fail.store(false, Ordering::Release);
    h.add("third message");
    wait_until("the parked item to drain", || {
        h.written().contains("doomed message")
    });
    assert!(
        !h.registry
            .queue_items_for_test(SESSION)
            .iter()
            .any(|item| item.text == "doomed message"),
        "the sent item left the queue; its error went with it"
    );
    h.finish_turn();
    wait_until("the second message to drain", || {
        h.written().contains("second message")
    });
    h.finish_turn();
    wait_until("the third message to drain", || {
        h.written().contains("third message")
    });
    wait_until("the queue to empty", || {
        h.registry.queue_items_for_test(SESSION).is_empty()
    });
    assert!(!h.registry.queue_paused_for_test(SESSION));
}

fn last_snapshot(events: &[SessionEvent]) -> Vec<devboule_protocol::QueuedMessage> {
    events
        .iter()
        .rev()
        .find_map(|event| match event {
            SessionEvent::QueueSnapshot { items } => Some(items.clone()),
            _ => None,
        })
        .expect("the failed drain published its snapshot")
}

/// Fix pass 1, P1-1: the drain's idle check and its write must be one step
/// against a regular `SessionSend`. The seam parks that send — holding the
/// writer — between its write and its `begin_turn`, so when the drain finally
/// gets the writer a turn has begun meanwhile; the queued prompt must go back
/// to the front untouched instead of entering that turn.
#[test]
fn a_regular_send_that_starts_a_turn_beats_the_drains_idle_look() {
    let h = harness(false);
    h.runtime.begin_turn(); // T1: the add arms the hook instead of sending
    let parked = Arc::new(AtomicBool::new(false));
    let gate = Arc::new(Barrier::new(2));
    {
        let parked_flag = Arc::clone(&parked);
        let gate = Arc::clone(&gate);
        h.registry.set_send_after_write_hook(Arc::new(move || {
            parked_flag.store(true, Ordering::SeqCst);
            gate.wait();
        }));
    }
    h.add("queued text");
    assert_eq!(h.registry.queue_items_for_test(SESSION).len(), 1);

    // A regular send writes first and parks between the write and
    // `begin_turn`, still holding the writer.
    let regular = {
        let registry = h.registry.clone();
        let owner = h.owner.clone();
        let conn = Arc::clone(&h.conn);
        std::thread::spawn(move || {
            registry
                .send_with_subscription(SESSION, conn.id, "regular text", &[], &[], &owner, &conn)
                .expect("the regular send")
        })
    };
    wait_until("the regular send to hold the writer", || {
        parked.load(Ordering::SeqCst)
    });

    // The turn ends while the writer is held: the drain claims its item and
    // blocks on the writer the regular send still owns.
    h.finish_turn();
    wait_until("the drain to claim its item", || {
        h.registry.queue_draining_for_test(SESSION)
    });

    // Release: the regular send begins its turn under the writer it holds,
    // and only then does the drain acquire it.
    gate.wait();
    regular.join().expect("the regular send thread");
    wait_until("the drain to settle", || {
        !h.registry.queue_draining_for_test(SESSION)
    });

    let text = h.written();
    assert!(text.contains("regular text"));
    assert!(
        !text.contains("queued text"),
        "the queued prompt must not enter a turn that began while it waited: {text:?}"
    );
    let items = h.registry.queue_items_for_test(SESSION);
    assert_eq!(items.len(), 1, "the claim went back to the front");
    assert!(
        items[0].error.is_none(),
        "an admission race is not a failure of the message"
    );
    assert!(!h.registry.queue_paused_for_test(SESSION));
}

/// Fix pass 1, P1-5: a failed send-now restores the item, records the error
/// and pauses — and does **not** retry itself. The proof is the attempt
/// count: one write call, refused, and nothing else touches the writer until
/// a user acts again.
#[test]
fn a_failed_send_now_does_not_retry_itself() {
    let h = harness(true); // every write is refused
    h.runtime.begin_turn(); // the add only arms; send-now does the sending
    h.add("send now");
    let item_id = h.registry.queue_items_for_test(SESSION)[0].item_id.clone();

    h.registry
        .queue_send_now(SESSION, h.conn.id, &item_id, &h.owner, &h.conn)
        .expect_err("the writer refuses");

    assert_eq!(
        h.attempts.load(Ordering::Acquire),
        1,
        "no automatic retry: the next attempt is a user act"
    );
    assert!(h.registry.queue_paused_for_test(SESSION));
    let items = h.registry.queue_items_for_test(SESSION);
    assert_eq!(items.len(), 1, "the item is back");
    assert!(items[0].error.is_some(), "and carries its error");
}
