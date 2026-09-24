//! Writes that must survive: a queue operation is acknowledged only when its
//! journal write was (fix pass 1, P1-3), and an item id is never reused
//! across a restart (P2-6). Both are durability properties of the same
//! seam — the write-through in `session_queue.rs`.

use super::tests::{
    attach_live_agent_for_test, drain, insert_live_agent_with_writer, test_owner,
    tmp_delete_registry, RecordingWriter,
};
use super::{
    Arc, ConnHandle, Duration, ErrorCode, Mutex, RuntimePaths, SessionEvent, SessionRegistry,
};

const SESSION: &str = "s.q.dur";

/// Fix pass 1, P1-3: a store that refuses the write turns the op into an
/// error, changes nothing in memory, and publishes no snapshot claiming the
/// change happened. The trigger aborts the insert and rolls the transaction
/// back the way a full disk would, while leaving the rows readable.
#[test]
fn a_journal_write_failure_is_returned_and_changes_nothing() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("queue.user", "queue.client");
    let runtime = insert_live_agent_with_writer(
        &registry,
        SESSION,
        owner.clone(),
        Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
    );
    let conn = attach_live_agent_for_test(&runtime, SESSION, 1);
    runtime.begin_turn();
    registry
        .queue_add(SESSION, "first message", &[], &[], &owner, &conn)
        .expect("the store still works");

    {
        let sabotage = rusqlite::Connection::open(dir.join("journal.db")).expect("second handle");
        sabotage
            .busy_timeout(Duration::from_secs(5))
            .expect("busy timeout");
        sabotage
            .execute_batch(
                "CREATE TRIGGER session_queue_insert_fails BEFORE INSERT ON session_queue
                 BEGIN SELECT RAISE(ABORT, 'simulated disk failure'); END;",
            )
            .expect("the failing store");
    }

    registry
        .queue_add(SESSION, "second message", &[], &[], &owner, &conn)
        .expect_err("an unrecorded queue write must not be acknowledged");
    assert_eq!(
        registry.queue_items_for_test(SESSION).len(),
        1,
        "the in-memory queue still equals the durable one"
    );
    let last = drain(&conn)
        .into_iter()
        .rev()
        .find_map(|event| match event {
            SessionEvent::QueueSnapshot { items } => Some(items),
            _ => None,
        })
        .expect("the first add's snapshot");
    assert_eq!(last.len(), 1, "no snapshot claims the failed add happened");

    // A second daemon over the same journal reads what is durable — the
    // load path is untouched by the trigger — and fails the same way.
    let registry2 = SessionRegistry::new(RuntimePaths::from_dir(&dir), Some(Arc::clone(&journal)));
    let owner2 = test_owner("queue.user", "queue.client");
    insert_live_agent_with_writer(
        &registry2,
        SESSION,
        owner2.clone(),
        Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
    );
    registry2
        .queue_add(
            SESSION,
            "probe after restart",
            &[],
            &[],
            &owner2,
            &ConnHandle::new(1),
        )
        .expect_err("the store still refuses");
    assert_eq!(
        registry2.queue_items_for_test(SESSION).len(),
        1,
        "the reloaded queue equals the durable rows and stayed that way"
    );
}

/// Fix pass 1, P2-6: ids climb across a restart, so a stale edit aimed at a
/// removed id can never land on a different message.
#[test]
fn an_item_id_is_never_reused_across_a_restart() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("queue.user", "queue.client");
    let runtime = insert_live_agent_with_writer(
        &registry,
        SESSION,
        owner.clone(),
        Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
    );
    let conn = ConnHandle::new(1);
    runtime.begin_turn();
    registry
        .queue_add(SESSION, "one", &[], &[], &owner, &conn)
        .expect("add");
    registry
        .queue_add(SESSION, "two", &[], &[], &owner, &conn)
        .expect("add");
    let removed = registry.queue_items_for_test(SESSION)[1].item_id.clone();
    assert_eq!(removed, "queue-2");
    registry
        .queue_remove(SESSION, &removed, &owner, &conn)
        .expect("remove");

    // The restart: a fresh registry (and a fresh queue map) over the same
    // journal, with the session live again — the resume road's world.
    let registry2 = SessionRegistry::new(RuntimePaths::from_dir(&dir), Some(Arc::clone(&journal)));
    let runtime2 = insert_live_agent_with_writer(
        &registry2,
        SESSION,
        owner.clone(),
        Box::new(RecordingWriter(Arc::new(Mutex::new(Vec::new())))),
    );
    runtime2.begin_turn();
    registry2
        .queue_add(SESSION, "three", &[], &[], &owner, &conn)
        .expect("add after the restart");

    let items = registry2.queue_items_for_test(SESSION);
    assert_eq!(items.len(), 2);
    assert_eq!(
        items[1].item_id, "queue-3",
        "ids climb across a restart; the issued queue-2 must never come back"
    );
    let stale = registry2
        .queue_edit(SESSION, &removed, "hijack", &owner, &conn)
        .expect_err("a stale id names no live message");
    assert_eq!(stale.code, ErrorCode::InvalidRequest);
    assert_eq!(items[1].text, "three", "the live message is untouched");
}
