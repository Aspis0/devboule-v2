//! The queue's four edits and the walls they run into: what a row becomes,
//! what an edit keeps, and which refusals commit nothing.
//!
//! Every test here holds a turn open on the session, because an add into an
//! idle session drains at once (that is today's app semantics, and
//! `session_queue_drain_tests.rs` owns it). Holding the turn keeps the queue
//! where these assertions can read it. What a snapshot says is
//! `session_queue_snapshot_tests.rs`.

use super::session_queue::{queue_items_for_test, MAX_QUEUE_ITEMS};
use super::session_queue_fixtures::{attached, queue_registry, queued_agent, snapshots};
use super::tests::{insert_live, test_owner};
use super::*;

fn owner() -> OwnerId {
    test_owner("S-1-5-21-queue-owner", "process-queue")
}

/// A busy agent session with one attached client: the turn is open, so nothing
/// drains while these tests edit.
fn busy_session(
    registry: &SessionRegistry,
    journal: &Arc<Journal>,
    conn_id: u64,
) -> (OwnerId, String, Arc<SessionRuntime>, Arc<ConnHandle>) {
    let owner = owner();
    let id = compose_session_id(&owner.session_token(), "q").expect("id");
    let (runtime, _sent) = queued_agent(registry, journal, &owner, &id);
    runtime.begin_turn();
    let conn = attached(registry, &id, conn_id, &owner);
    let _ = conn.pull_events();
    (owner, id, runtime, conn)
}

#[test]
fn an_add_appends_the_row_and_publishes_the_whole_queue() {
    let (dir, registry, journal) = queue_registry();
    let (owner, id, _runtime, conn) = busy_session(&registry, &journal, 4);

    registry
        .queue_add(&id, "op-1", "  first  ", &[], &[], &owner, &conn)
        .expect("add");

    let published = snapshots(&conn);
    assert_eq!(published.len(), 1, "one snapshot per accepted mutation");
    assert_eq!(published[0].revision, 1, "the first snapshot is revision 1");
    assert_eq!(published[0].items.len(), 1);
    assert_eq!(
        published[0].items[0].text, "first",
        "the text is trimmed on the way in"
    );
    assert_eq!(
        published[0].items[0].item_id, "queue-1",
        "ids are the daemon's own"
    );
    assert!(
        published[0].items[0].error.is_none(),
        "a fresh row carries no failure"
    );
    assert_eq!(
        queue_items_for_test(&registry, &id).len(),
        1,
        "the daemon's own view agrees with the snapshot"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_add_of_nothing_but_whitespace_is_refused_and_publishes_nothing() {
    let (dir, registry, journal) = queue_registry();
    let (owner, id, _runtime, conn) = busy_session(&registry, &journal, 4);

    let error = registry
        .queue_add(&id, "op-1", "   ", &[], &[], &owner, &conn)
        .expect_err("an empty message is not a message");

    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(
        snapshots(&conn).is_empty(),
        "a refusal publishes no snapshot"
    );
    assert!(queue_items_for_test(&registry, &id).is_empty());

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_edit_keeps_the_row_in_place_and_clears_its_failure() {
    let (dir, registry, journal) = queue_registry();
    let (owner, id, _runtime, conn) = busy_session(&registry, &journal, 4);
    registry
        .queue_add(&id, "op-1", "first", &[], &[], &owner, &conn)
        .expect("add");
    registry
        .queue_add(&id, "op-2", "second", &[], &[], &owner, &conn)
        .expect("add");

    registry
        .queue_edit(&id, "op-3", "queue-2", "  rewritten  ", &owner, &conn)
        .expect("edit");

    let published = snapshots(&conn);
    let edited = published.last().expect("the edit published");
    assert_eq!(
        edited.items[0].item_id, "queue-1",
        "the other row kept its place"
    );
    assert_eq!(edited.items[1].item_id, "queue-2");
    assert_eq!(edited.items[1].text, "rewritten");
    assert!(edited.items[1].error.is_none());

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_remove_takes_the_row_out_and_an_unknown_id_is_refused() {
    let (dir, registry, journal) = queue_registry();
    let (owner, id, _runtime, conn) = busy_session(&registry, &journal, 4);
    registry
        .queue_add(&id, "op-1", "first", &[], &[], &owner, &conn)
        .expect("add");
    registry
        .queue_add(&id, "op-2", "second", &[], &[], &owner, &conn)
        .expect("add");

    registry
        .queue_remove(&id, "op-3", "queue-1", &owner, &conn)
        .expect("remove");
    assert_eq!(
        super::session_queue_fixtures::latest_ids(&conn),
        vec!["queue-2".to_string()]
    );

    let error = registry
        .queue_remove(&id, "op-4", "queue-1", &owner, &conn)
        .expect_err("a row that is gone is gone");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(
        error.message.contains("queue-1"),
        "the refusal names the row: {error:?}"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_move_places_the_row_where_the_client_asked() {
    let (dir, registry, journal) = queue_registry();
    let (owner, id, _runtime, conn) = busy_session(&registry, &journal, 4);
    for (index, text) in ["first", "second", "third"].iter().enumerate() {
        registry
            .queue_add(&id, &format!("op-{index}"), text, &[], &[], &owner, &conn)
            .expect("add");
    }

    registry
        .queue_move(&id, "op-move", "queue-3", 0, &owner, &conn)
        .expect("move to the front");
    assert_eq!(
        super::session_queue_fixtures::latest_ids(&conn),
        vec![
            "queue-3".to_string(),
            "queue-1".to_string(),
            "queue-2".to_string()
        ]
    );

    let error = registry
        .queue_move(&id, "op-past", "queue-3", 9, &owner, &conn)
        .expect_err("past the end is not a position");
    assert_eq!(error.code, ErrorCode::InvalidRequest);

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_queue_is_capped_by_row_count_and_by_text() {
    let (dir, registry, journal) = queue_registry();
    let (owner, id, _runtime, conn) = busy_session(&registry, &journal, 4);

    for index in 0..MAX_QUEUE_ITEMS {
        registry
            .queue_add(
                &id,
                &format!("op-{index}"),
                &format!("row {index}"),
                &[],
                &[],
                &owner,
                &conn,
            )
            .expect("add");
    }
    let error = registry
        .queue_add(&id, "op-full", "one too many", &[], &[], &owner, &conn)
        .expect_err("the row cap holds");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(queue_items_for_test(&registry, &id).len(), MAX_QUEUE_ITEMS);

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn one_queue_longer_than_the_budget_is_refused_and_nothing_changes() {
    let (dir, registry, journal) = queue_registry();
    let (owner, id, _runtime, conn) = busy_session(&registry, &journal, 4);
    let long = "x".repeat(super::session_queue::MAX_QUEUE_TEXT_BYTES + 1);

    let error = registry
        .queue_add(&id, "op-1", &long, &[], &[], &owner, &conn)
        .expect_err("the text budget holds");

    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(
        queue_items_for_test(&registry, &id).is_empty(),
        "a refused add commits nothing"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_terminal_has_no_queue_and_is_refused_by_name() {
    let (dir, registry, journal) = queue_registry();
    let owner = owner();
    let id = compose_session_id(&owner.session_token(), "t").expect("id");
    insert_live(&registry, &id, owner.clone());
    let conn = attached(&registry, &id, 4, &owner);

    let error = registry
        .queue_add(&id, "op-1", "hi", &[], &[], &owner, &conn)
        .expect_err("a terminal has no queue");

    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(error.message.contains("agent"), "{error:?}");

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_second_add_from_another_connection_does_not_wedge_the_first_one() {
    // Two clients on one queue must not serialize into a stall: the second add
    // is answered while the first connection still reads its snapshots.
    let (dir, registry, journal) = queue_registry();
    let (owner, id, _runtime, first) = busy_session(&registry, &journal, 4);
    let second = attached(&registry, &id, 5, &owner);
    let _ = first.pull_events();
    let _ = second.pull_events();

    registry
        .queue_add(&id, "op-1", "one", &[], &[], &owner, &first)
        .expect("add");
    registry
        .queue_add(&id, "op-2", "two", &[], &[], &owner, &second)
        .expect("add");

    assert_eq!(
        super::session_queue_fixtures::latest_ids(&first),
        vec!["queue-1".to_string(), "queue-2".to_string()],
        "one queue, both clients, one order"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}
/// A refused add must leave nothing behind, and the only thing an add writes
/// outside the queue is the attachment it deposits.
#[test]
fn a_refused_add_stores_no_attachment() {
    let (dir, registry, journal) = queue_registry();
    let (owner, id, _runtime, conn) = busy_session(&registry, &journal, 4);
    for index in 0..super::session_queue::MAX_QUEUE_ITEMS {
        registry
            .queue_add(
                &id,
                &format!("op-{index}"),
                &format!("row {index}"),
                &[],
                &[],
                &owner,
                &conn,
            )
            .expect("add");
    }
    let notes = devboule_protocol::PromptAttachment {
        name: "notes.md".to_string(),
        mime_type: "text/markdown".to_string(),
        data: "IyBub3Rlcw==".to_string(),
    };

    let error = registry
        .queue_add(
            &id,
            "op-full",
            "one row too many",
            &[notes],
            &[],
            &owner,
            &conn,
        )
        .expect_err("the row cap holds");

    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(
        registry.attachments.session_bytes(&id),
        Some(0),
        "the attachment of a refused add is never written to the store"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_accepted_add_stores_its_attachment_once() {
    let (dir, registry, journal) = queue_registry();
    let (owner, id, _runtime, conn) = busy_session(&registry, &journal, 4);
    let notes = devboule_protocol::PromptAttachment {
        name: "notes.md".to_string(),
        mime_type: "text/markdown".to_string(),
        data: "IyBub3Rlcw==".to_string(),
    };

    registry
        .queue_add(&id, "op-1", "read these", &[notes], &[], &owner, &conn)
        .expect("add");

    let items = queue_items_for_test(&registry, &id);
    assert_eq!(items.len(), 1);
    assert_eq!(
        items[0].attachment_references.len(),
        1,
        "the row carries the stored bytes by reference, never inline"
    );
    assert!(
        registry
            .attachments
            .session_bytes(&id)
            .expect("this session's store folder")
            > 0,
        "and the bytes really are in the store"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}
