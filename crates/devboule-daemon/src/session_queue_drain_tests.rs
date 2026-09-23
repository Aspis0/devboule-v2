//! The drain: FIFO, one at a time, and only at a turn's end; a failed send
//! parking the item at the front with its error until a client's next frame;
//! and send-now leaving the rest of the queue for the turn it started.
//! Written first, per the Q1 brief's red-first rule.

use super::tests::{
    attach_live_agent_for_test, drain, insert_live_agent_with_writer, install_end_turn_killer,
    test_owner, tmp_delete_registry,
};
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};

const SESSION: &str = "s.q.2";

/// A writer that records what it accepts and refuses every write while
/// `fail` is set — one switch, so a test can park a drain and release it.
struct SwitchWriter {
    bytes: Arc<Mutex<Vec<u8>>>,
    fail: Arc<AtomicBool>,
}

impl Write for SwitchWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
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
}

fn harness(fail: bool) -> Harness {
    let (_dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("queue.user", "queue.client");
    let bytes = Arc::new(Mutex::new(Vec::new()));
    let flag = Arc::new(AtomicBool::new(fail));
    let runtime = insert_live_agent_with_writer(
        &registry,
        SESSION,
        owner.clone(),
        Box::new(SwitchWriter {
            bytes: Arc::clone(&bytes),
            fail: Arc::clone(&flag),
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
