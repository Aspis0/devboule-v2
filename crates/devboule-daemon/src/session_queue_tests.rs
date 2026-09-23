//! The queue's frame ops — add, edit, remove, move, send-now — and the typed
//! refusals around them: a terminal, an unknown item, an unknown session, an
//! over-cap add. Every op's visible output is the snapshot it publishes; the
//! queue's own state is read through the test getter. Written first, per the
//! Q1 brief's red-first rule.
//!
//! Every harness keeps a turn running: the queue exists while the agent
//! works, and a running turn is what holds the drain (pinned separately in
//! `session_queue_drain_tests.rs`) away from these assertions.

use super::tests::{
    attach_live_agent_for_test, attachment, drain, files_under,
    insert_live_agent_with_kind_and_writer, insert_live_agent_with_writer, install_end_turn_killer,
    test_owner, tmp_delete_registry, FailingWriter, RecordingWriter,
};
use super::*;
use crate::raster_metadata::clean_png;
use devboule_protocol::QueuedMessage;

const SESSION: &str = "s.q.1";

/// A live attached agent with a recording writer and a running turn — the
/// world every queue op is pinned in.
fn harness() -> (
    SessionRegistry,
    OwnerId,
    Arc<ConnHandle>,
    Arc<Mutex<Vec<u8>>>,
) {
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
    // Send-now interrupts: the fake provider has to end its turn like one.
    install_end_turn_killer(&registry, SESSION, &runtime, &bytes);
    runtime.begin_turn();
    (registry, owner, conn, bytes)
}

/// A live attached agent whose writer refuses everything, turn running.
fn failing_harness() -> (SessionRegistry, OwnerId, Arc<ConnHandle>) {
    let (_dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("queue.user", "queue.client");
    let runtime =
        insert_live_agent_with_writer(&registry, SESSION, owner.clone(), Box::new(FailingWriter));
    let conn = attach_live_agent_for_test(&runtime, SESSION, 1);
    // Send-now interrupts here too; its bytes go nowhere (the writer refuses
    // them), so the probe's writer mirror is a dummy it can still read.
    install_end_turn_killer(
        &registry,
        SESSION,
        &runtime,
        &Arc::new(Mutex::new(Vec::new())),
    );
    runtime.begin_turn();
    (registry, owner, conn)
}

fn last_snapshot(events: &[SessionEvent]) -> Vec<QueuedMessage> {
    events
        .iter()
        .rev()
        .find_map(|event| match event {
            SessionEvent::QueueSnapshot { items } => Some(items.clone()),
            _ => None,
        })
        .expect("every queue change publishes a snapshot")
}

#[test]
fn add_edit_move_remove_change_the_queue_and_each_publishes_a_snapshot() {
    let (registry, owner, conn, _bytes) = harness();
    registry
        .queue_add(SESSION, "first", &[], &[], &owner, &conn)
        .expect("add");
    registry
        .queue_add(SESSION, "second", &[], &[], &owner, &conn)
        .expect("add");
    let items = registry.queue_items_for_test(SESSION);
    assert_eq!(
        items
            .iter()
            .map(|item| item.text.as_str())
            .collect::<Vec<_>>(),
        ["first", "second"]
    );
    let first_id = items[0].item_id.clone();
    let second_id = items[1].item_id.clone();

    registry
        .queue_move(SESSION, &first_id, 1, &owner, &conn)
        .expect("move");
    let moved = registry.queue_items_for_test(SESSION);
    assert_eq!(moved[0].item_id, second_id, "the move took effect");

    registry
        .queue_edit(SESSION, &second_id, "second, edited", &owner, &conn)
        .expect("edit");
    registry
        .queue_remove(SESSION, &first_id, &owner, &conn)
        .expect("remove");

    let final_items = registry.queue_items_for_test(SESSION);
    assert_eq!(final_items.len(), 1);
    assert_eq!(final_items[0].item_id, second_id);
    assert_eq!(final_items[0].text, "second, edited");

    // The snapshot is the whole queue, and the last one is the state above.
    assert_eq!(last_snapshot(&drain(&conn)), final_items);
}

#[test]
fn an_add_with_inline_attachments_queues_them_as_deposited_references() {
    let (registry, owner, conn, _bytes) = harness();
    registry
        .queue_add(
            SESSION,
            "look at this",
            &[attachment("page.png", "image/png", &clean_png(0x0b))],
            &[],
            &owner,
            &conn,
        )
        .expect("add");
    let items = registry.queue_items_for_test(SESSION);
    assert_eq!(items.len(), 1);
    let references = &items[0].attachment_references;
    assert_eq!(references.len(), 1, "the bytes were deposited, not queued");
    assert_eq!(references[0].session_id, SESSION);
    assert_eq!(references[0].digest.len(), 64);
    assert!(
        files_under(&super::tests::attachment_folder(&registry, SESSION))
            .iter()
            .any(|path| path.to_string_lossy().contains(&references[0].digest))
    );
    // The snapshot carries the reference, never the bytes.
    assert_eq!(last_snapshot(&drain(&conn)), items);
}

#[test]
fn empty_text_a_missing_item_and_an_out_of_range_move_are_refused() {
    let (registry, owner, conn, _bytes) = harness();
    let error = registry
        .queue_add(SESSION, "   ", &[], &[], &owner, &conn)
        .expect_err("blank text");
    assert_eq!(error.code, ErrorCode::InvalidRequest);

    registry
        .queue_add(SESSION, "kept", &[], &[], &owner, &conn)
        .expect("add");
    let item_id = registry.queue_items_for_test(SESSION)[0].item_id.clone();

    let error = registry
        .queue_edit(SESSION, "queue-999", "nope", &owner, &conn)
        .expect_err("missing item");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(error.message.contains("queue-999"), "{}", error.message);

    let error = registry
        .queue_remove(SESSION, "queue-999", &owner, &conn)
        .expect_err("missing item");
    assert_eq!(error.code, ErrorCode::InvalidRequest);

    let error = registry
        .queue_move(SESSION, &item_id, 7, &owner, &conn)
        .expect_err("out of range");
    assert_eq!(error.code, ErrorCode::InvalidRequest);

    // The refusals changed nothing.
    let items = registry.queue_items_for_test(SESSION);
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].text, "kept");
}

#[test]
fn the_queue_refuses_an_add_past_the_cap() {
    let (registry, owner, conn, _bytes) = harness();
    for index in 0..MAX_QUEUE_ITEMS {
        registry
            .queue_add(SESSION, &format!("item {index}"), &[], &[], &owner, &conn)
            .expect("add under the cap");
    }
    let error = registry
        .queue_add(SESSION, "one too many", &[], &[], &owner, &conn)
        .expect_err("over the cap");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(error.message.contains(&MAX_QUEUE_ITEMS.to_string()));
    assert_eq!(
        registry.queue_items_for_test(SESSION).len(),
        MAX_QUEUE_ITEMS
    );
}

#[test]
fn every_queue_frame_is_refused_on_a_terminal_session() {
    let (_dir, registry, _journal) = tmp_delete_registry();
    let owner = test_owner("queue.user", "queue.client");
    let runtime = insert_live_agent_with_kind_and_writer(
        &registry,
        "s.q.term",
        owner.clone(),
        SessionKind::Terminal,
        Box::new(std::io::sink()),
    );
    let conn = attach_live_agent_for_test(&runtime, "s.q.term", 1);
    for attempt in 0..5 {
        let result = match attempt {
            0 => registry.queue_add("s.q.term", "text", &[], &[], &owner, &conn),
            1 => registry.queue_edit("s.q.term", "queue-1", "text", &owner, &conn),
            2 => registry.queue_remove("s.q.term", "queue-1", &owner, &conn),
            3 => registry.queue_move("s.q.term", "queue-1", 0, &owner, &conn),
            _ => registry.queue_send_now("s.q.term", 1, "queue-1", &owner, &conn),
        };
        let error = result.expect_err("a terminal has no queue");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(
            error.message.contains("Only agent sessions"),
            "{}",
            error.message
        );
    }
}

#[test]
fn a_queue_frame_names_the_session_it_cannot_find() {
    let (registry, owner, conn, _bytes) = harness();
    let error = registry
        .queue_add("s.q.gone", "text", &[], &[], &owner, &conn)
        .expect_err("unknown session");
    assert_eq!(error.code, ErrorCode::SessionNotFound);
}

#[test]
fn send_now_takes_the_item_out_and_sends_it_through_the_ordinary_path() {
    let (registry, owner, conn, bytes) = harness();
    registry
        .queue_add(SESSION, "send me now", &[], &[], &owner, &conn)
        .expect("add");
    let item_id = registry.queue_items_for_test(SESSION)[0].item_id.clone();

    registry
        .queue_send_now(SESSION, conn.id, &item_id, &owner, &conn)
        .expect("send-now");

    assert!(registry.queue_items_for_test(SESSION).is_empty());
    assert!(
        String::from_utf8_lossy(&bytes.lock().expect("writer")).contains("send me now"),
        "the text reached the provider's writer"
    );
    // The change is published like any other.
    assert!(last_snapshot(&drain(&conn)).is_empty());
}

#[test]
fn send_now_failure_returns_the_item_to_the_front_with_its_error() {
    let (registry, owner, conn) = failing_harness();
    registry
        .queue_add(SESSION, "later", &[], &[], &owner, &conn)
        .expect("add");
    registry
        .queue_add(SESSION, "after", &[], &[], &owner, &conn)
        .expect("add");
    let front = registry.queue_items_for_test(SESSION)[0].item_id.clone();

    let error = registry
        .queue_send_now(SESSION, conn.id, &front, &owner, &conn)
        .expect_err("the writer refuses");
    assert_eq!(error.code, ErrorCode::Io);

    let items = registry.queue_items_for_test(SESSION);
    assert_eq!(items.len(), 2, "nothing was lost");
    assert_eq!(items[0].item_id, front, "back at the front, Paseo's rule");
    assert!(items[0].error.is_some(), "the error rides the snapshot");
    assert_eq!(last_snapshot(&drain(&conn)), items);
}
