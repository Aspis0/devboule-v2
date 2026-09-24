//! The frame budget on every publish path (fix pass 2, P1-2): the whole
//! queue travels as one `MAX_FRAME_BYTES` frame, the add/edit budget keeps
//! the headroom a later failure spends (an item's error text, bounded), and
//! every publish passes one choke point — so no path can hand the
//! connection a frame the connection must refuse.

use super::tests::{
    attach_live_agent_for_test, drain, insert_live_agent_with_writer, install_end_turn_killer,
    test_owner, tmp_delete_registry, RecordingWriter,
};
use super::{
    new_session_record, Arc, ConnHandle, Duration, Instant, Mutex, OwnerId, PersistStatus,
    RuntimePaths, SessionEvent, SessionKind, SessionRegistry, SessionRuntime, MAX_WRITE_BYTES,
};
use devboule_protocol::{DaemonMessage, QueuedMessage, SessionEventEnvelope, MAX_FRAME_BYTES};

const SESSION: &str = "s.q.frame";

/// The session row every queue store rides on (fix pass 4's existence
/// check): a production session always has its row before it can be queued
/// for, and these harnesses say so too.
fn ended_row(owner: &str) -> crate::journal::SessionRecord {
    let mut record = new_session_record(SESSION, owner, None, SessionKind::Acp, "queue harness");
    record.status = PersistStatus::Ended;
    record
}

/// The budget the fix promises: the frame cap less what a later failure may
/// add — a bounded error at its JSON-escaped worst, the field's own quotes,
/// and the claim's flag flip.
const ADD_BUDGET: usize = MAX_FRAME_BYTES - 4096;

/// The bound the fix promises for the error a failed send leaves on its
/// item.
const BOUNDED_ERROR: usize = 512;

/// A writer that refuses everything with a refusal far longer than the
/// whole frame cap — the realistic shape of a provider's failure text, and
/// the thing the bounded error and the reserved headroom stand against.
struct LongRefusalWriter;

impl std::io::Write for LongRefusalWriter {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::new(
            std::io::ErrorKind::BrokenPipe,
            "x".repeat(4 * MAX_FRAME_BYTES),
        ))
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The frame the connection would write for this snapshot — the same
/// conservative mirror the budget check computes (one exact field, every
/// unknown field at its longest), kept local so the assertions measure what
/// would actually travel.
fn frame_bytes(session_id: &str, items: &[QueuedMessage]) -> usize {
    let message = DaemonMessage::SubscriptionEvent {
        subscription_id: u64::MAX,
        envelope: SessionEventEnvelope {
            session_id: session_id.to_string(),
            generation: u64::MAX,
            transcript_seq: Some(u64::MAX),
            event: SessionEvent::QueueSnapshot {
                items: items.to_vec(),
            },
        },
    };
    serde_json::to_vec(&message)
        .expect("serialize the frame")
        .len()
}

/// A live attached agent whose writer refuses with a long text, turn
/// running so the queue built below stays parked while it is sized.
fn failing_harness() -> (
    SessionRegistry,
    OwnerId,
    Arc<ConnHandle>,
    Arc<SessionRuntime>,
) {
    let (_dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("queue.user", "queue.client");
    _journal
        .create_session(ended_row(&owner.user))
        .expect("the session row");
    let runtime = insert_live_agent_with_writer(
        &registry,
        SESSION,
        owner.clone(),
        Box::new(LongRefusalWriter),
    );
    let conn = attach_live_agent_for_test(&runtime, SESSION, 1);
    install_end_turn_killer(
        &registry,
        SESSION,
        &runtime,
        &Arc::new(Mutex::new(Vec::new())),
    );
    runtime.begin_turn();
    (registry, owner, conn, runtime)
}

/// Queue items until the frame lands exactly on `target`: full messages
/// while a whole one still fits, then one sized to the byte (its id is
/// minted as `queue-{n+1}`, exactly what the probe assumed).
fn fill_to_exact(
    registry: &SessionRegistry,
    owner: &OwnerId,
    conn: &Arc<ConnHandle>,
    target: usize,
) {
    let full = "a".repeat(MAX_WRITE_BYTES);
    registry
        .queue_add(SESSION, &full, &[], &[], owner, conn)
        .expect("the first full message fits");
    let mut items = registry.queue_items_for_test(SESSION);
    loop {
        // Probe with an EMPTY text: the loop must stop while one more full
        // message could still land on the target, so the sized remainder
        // below is at most one message's worth.
        let mut probe = items.last().expect("at least one item").clone();
        probe.item_id = format!("queue-{}", items.len() + 1);
        probe.text = String::new();
        let mut probe_items = items.clone();
        probe_items.push(probe);
        if frame_bytes(SESSION, &probe_items) + MAX_WRITE_BYTES > target {
            break;
        }
        registry
            .queue_add(SESSION, &full, &[], &[], owner, conn)
            .expect("a message that fits");
        items = registry.queue_items_for_test(SESSION);
    }
    let mut sized = items.last().expect("at least one item").clone();
    sized.item_id = format!("queue-{}", items.len() + 1);
    sized.text = String::new();
    let mut probe_items = items.clone();
    probe_items.push(sized.clone());
    let base = frame_bytes(SESSION, &probe_items);
    assert!(
        target - base <= MAX_WRITE_BYTES,
        "the sized remainder must be one message's worth"
    );
    sized.text = "a".repeat(target - base);
    let mut probe_items = items.clone();
    probe_items.push(sized.clone());
    assert_eq!(
        frame_bytes(SESSION, &probe_items),
        target,
        "the boundary state is sized to the byte"
    );
    registry
        .queue_add(SESSION, &sized.text, &[], &[], owner, conn)
        .expect("exactly on the budget the queue still takes it");
    assert_eq!(
        frame_bytes(SESSION, &registry.queue_items_for_test(SESSION)),
        target,
        "the live queue sits on the boundary"
    );
}

fn snapshots(events: &[SessionEvent]) -> Vec<Vec<QueuedMessage>> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::QueueSnapshot { items } => Some(items.clone()),
            _ => None,
        })
        .collect()
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

/// The add budget (with P1-2's reserve): a queue that still leaves room for
/// a later failure's error is accepted to the byte, one byte more is
/// refused by name, and the same wall holds for an edit.
#[test]
fn the_add_budget_reserves_room_for_a_later_error() {
    let (_dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("queue.user", "queue.client");
    _journal
        .create_session(ended_row(&owner.user))
        .expect("the session row");
    let runtime = insert_live_agent_with_writer(
        &registry,
        SESSION,
        owner.clone(),
        Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
    );
    let conn = attach_live_agent_for_test(&runtime, SESSION, 1);
    runtime.begin_turn();

    fill_to_exact(&registry, &owner, &conn, ADD_BUDGET);
    let items = registry.queue_items_for_test(SESSION);

    // One byte more on an add: refused, and the refusal names the frame.
    let error = registry
        .queue_add(
            SESSION,
            &"a".repeat(items.last().expect("last").text.len() + 1),
            &[],
            &[],
            &owner,
            &conn,
        )
        .expect_err("one byte over the budget is not a queueable message");
    assert!(error.message.contains("frame"), "{}", error.message);

    // The same wall for an edit: growing the last message by one byte past
    // the budget is refused the same way, and changes nothing.
    let last = items.last().expect("at least one item");
    let mut bigger = last.text.clone();
    bigger.push('a');
    let error = registry
        .queue_edit(SESSION, &last.item_id, &bigger, &owner, &conn)
        .expect_err("an edit cannot outgrow the budget either");
    assert!(error.message.contains("frame"), "{}", error.message);
    assert_eq!(
        registry.queue_items_for_test(SESSION),
        items,
        "the refused edit changed nothing"
    );
}

/// A drain that fails on a boundary-sized queue writes the item's error
/// inside the reserved room: the snapshot it publishes still fits one
/// frame, and the error it stores is the bounded text, never the
/// provider's whole refusal.
#[test]
fn a_failed_drain_at_the_boundary_still_publishes_inside_the_frame() {
    let (registry, owner, conn, runtime) = failing_harness();
    fill_to_exact(&registry, &owner, &conn, ADD_BUDGET);

    runtime.publish_agent_event(
        SessionEvent::AgentFinished {
            stop_reason: "end_turn".to_string(),
            model_id: None,
            usage: None,
        },
        None,
    );
    wait_until("the drain's failure to land on the item", || {
        registry
            .queue_items_for_test(SESSION)
            .first()
            .is_some_and(|item| item.error.is_some())
    });
    assert!(
        registry.queue_paused_for_test(SESSION),
        "a failed send pauses"
    );

    let items = registry.queue_items_for_test(SESSION);
    let error_text = items[0].error.as_deref().expect("the error rode the item");
    assert!(
        error_text.len() <= BOUNDED_ERROR,
        "the stored error is bounded, got {} bytes",
        error_text.len()
    );
    for snapshot in snapshots(&drain(&conn)) {
        assert!(
            frame_bytes(SESSION, &snapshot) <= MAX_FRAME_BYTES,
            "every published snapshot must fit the frame cap"
        );
    }
}

/// Send-now failing on a boundary-sized queue is the same story from the
/// synchronous side: the restored item carries the bounded error and the
/// failure snapshot fits the frame.
#[test]
fn a_failed_send_now_at_the_boundary_still_publishes_inside_the_frame() {
    let (registry, owner, conn, _runtime) = failing_harness();
    fill_to_exact(&registry, &owner, &conn, ADD_BUDGET);
    let front = registry.queue_items_for_test(SESSION)[0].item_id.clone();

    registry
        .queue_send_now(SESSION, conn.id, &front, &owner, &conn)
        .expect_err("the writer refuses");

    let items = registry.queue_items_for_test(SESSION);
    assert_eq!(items[0].item_id, front, "back at the front, Paseo's rule");
    let error_text = items[0].error.as_deref().expect("the error rode the item");
    assert!(
        error_text.len() <= BOUNDED_ERROR,
        "the stored error is bounded, got {} bytes",
        error_text.len()
    );
    for snapshot in snapshots(&drain(&conn)) {
        assert!(
            frame_bytes(SESSION, &snapshot) <= MAX_FRAME_BYTES,
            "every published snapshot must fit the frame cap"
        );
    }
}

/// A journal may hold a queue over the budget — rows written before the
/// reserve existed. Attach must still not deliver what the connection
/// would refuse: the hand-over skips the publish (memory and rows keep the
/// queue), and the subscriber stays connected.
#[test]
fn an_over_budget_journal_is_not_delivered_on_attach() {
    let (dir, registry, journal) = tmp_delete_registry();
    drop(registry);
    {
        let raw = rusqlite::Connection::open(dir.join("journal.db")).expect("raw handle");
        raw.execute(
            "INSERT INTO session_queue
             (session_id, position, item_id, text, attachment_references, error, unconfirmed)
             VALUES (?1, 0, 'queue-1', ?2, '[]', NULL, 0)",
            rusqlite::params![SESSION, "b".repeat(MAX_FRAME_BYTES + 1)],
        )
        .expect("the over-budget row");
    }
    let restarted = SessionRegistry::new(RuntimePaths::from_dir(&dir), Some(journal));
    let owner = test_owner("queue.user", "queue.client");
    insert_live_agent_with_writer(
        &restarted,
        SESSION,
        owner.clone(),
        Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
    );
    let conn = Arc::new(ConnHandle::new(1));
    restarted
        .attach_with_subscription(SESSION, conn.id, None, &conn, &owner, false)
        .expect("attach through the registry");

    for snapshot in snapshots(&drain(&conn)) {
        assert!(
            frame_bytes(SESSION, &snapshot) <= MAX_FRAME_BYTES,
            "attach must not deliver a frame the connection refuses"
        );
    }
    assert_eq!(
        restarted.queue_items_for_test(SESSION).len(),
        1,
        "the queue itself is untouched: only the delivery was skipped"
    );
}
