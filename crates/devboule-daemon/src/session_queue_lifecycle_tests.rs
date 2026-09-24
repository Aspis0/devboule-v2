//! Close and delete are the two ways a session leaves the daemon; both must
//! take its queue with them — in memory as well as in the journal — and a
//! later queue frame for that session must be refused rather than recreate
//! it (fix pass 1, P2-5 and P2-7). Fix pass 2 adds the failure half (P1-3):
//! a close whose discard the journal refuses is reported, keeps memory on
//! the durable side, and a resume of the surviving rows sends nothing on
//! its own — and the same drop for retention's row deletions (P2-7).

use super::tests::{
    ended_record, insert_live_agent_with_writer, insert_transcript, test_owner,
    tmp_delete_registry, RecordingWriter,
};
use super::{Arc, ConnHandle, Duration, ErrorCode, Journal, Mutex, OwnerId, RetentionPatch};

const SESSION: &str = "s.q.life";

/// The session row every queue write rides on (fix pass 4's existence
/// check): production sessions always have one before they can be queued
/// for, and the tests' registry-only shortcuts must say so too.
fn seed_row(journal: &Journal, owner: &OwnerId) {
    journal
        .create_session(ended_record(SESSION, &owner.user))
        .expect("the session row");
}

/// Fix pass 1, P2-7: deleting an ended session drops its queue state. The
/// journal half of the deletion is the journal's own `delete_session`; what
/// was missing was the in-memory map, whose resident text outlived the rows.
#[test]
fn deleting_a_session_drops_its_queue_and_a_later_frame_is_refused() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("queue.user", "queue.client");
    journal
        .create_session(ended_record(SESSION, &owner.user))
        .expect("the row delete will look for");
    let runtime =
        insert_live_agent_with_writer(&registry, SESSION, owner.clone(), Box::new(std::io::sink()));
    runtime.begin_turn();
    let conn = ConnHandle::new(1);
    registry
        .queue_add(SESSION, "queued before death", &[], &[], &owner, &conn)
        .expect("add");
    assert_eq!(registry.queue_items_for_test(SESSION).len(), 1);

    // The session ends: the live entry is replaced by its transcript, which
    // is the state `delete_session` accepts (a live one says close first).
    insert_transcript(&registry, SESSION, owner.clone());
    registry
        .delete_session(SESSION, &owner)
        .expect("delete the ended session");

    assert!(
        registry.queue_items_for_test(SESSION).is_empty(),
        "delete drops the in-memory queue with the rows"
    );
    let error = registry
        .queue_add(SESSION, "after the delete", &[], &[], &owner, &conn)
        .expect_err("a deleted session's queue is refused, not recreated");
    assert_eq!(error.code, ErrorCode::SessionNotFound);
    let _ = dir;
}

/// Fix pass 2, P1-3: the discard a close promises is delete-on-disk first.
/// When the journal refuses the clear, the close says so, memory keeps the
/// state (paused, so nothing auto-sends it) — memory and rows still agree —
/// and a resume of the surviving rows sends nothing until the user acts.
#[test]
fn a_close_that_cannot_discard_the_queue_is_reported_and_a_resume_sends_nothing() {
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
                "CREATE TRIGGER session_queue_clear_fails BEFORE DELETE ON session_queue
                 BEGIN SELECT RAISE(ABORT, 'simulated disk failure'); END;",
            )
            .expect("the failing store");
    }

    // The close reports the discard it could not complete.
    let no_peer: Option<super::ConnPeer> = None;
    let error = registry
        .close(SESSION, &owner, &no_peer)
        .expect_err("a close whose queue rows survive must say so");
    assert!(error.message.contains("queued"), "{}", error.message);

    // The rows are still there, and memory is still on their side — paused.
    {
        let raw = rusqlite::Connection::open(dir.join("journal.db")).expect("raw handle");
        let rows: i64 = raw
            .query_row(
                "SELECT COUNT(*) FROM session_queue WHERE session_id = ?1",
                [SESSION],
                |row| row.get(0),
            )
            .expect("count the surviving rows");
        assert_eq!(rows, 2, "the discard failed; the rows survived");
    }
    assert_eq!(
        registry.queue_items_for_test(SESSION).len(),
        2,
        "memory keeps the state the disk still holds"
    );
    assert!(
        registry.queue_paused_for_test(SESSION),
        "the kept state is parked, so nothing auto-sends it"
    );

    // The resume road: the session comes back live and idle. The surviving
    // rows are restored, but nothing sends on its own — the user acts first.
    let resumed_bytes = Arc::new(Mutex::new(Vec::new()));
    let resumed = insert_live_agent_with_writer(
        &registry,
        SESSION,
        owner.clone(),
        Box::new(RecordingWriter(Arc::clone(&resumed_bytes))),
    );
    let _ = resumed;
    let _ = &journal;
    registry.queue_restore_and_drain(SESSION);
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        String::from_utf8_lossy(&resumed_bytes.lock().expect("writer")).is_empty(),
        "a queue the user tried to discard never sends on its own"
    );
}

/// Fix pass 2, P2-7 (the residual): retention deletes rows without the
/// registry, so the queue state it leaves behind is dropped the next time
/// the roster is re-read — the same drop `delete_session` performs.
#[test]
fn retention_deleting_a_session_drops_its_queue_state_too() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("queue.user", "queue.client");
    let mut record = ended_record(SESSION, &owner.user);
    record.updated_at_ms = 1_000_000_000; // years before any sweep's cutoff
    journal
        .create_session(record)
        .expect("the row retention will delete");
    // A live row the sweep's pending global pass appends through — the
    // sweep runs inside the append path, not inside the settings call.
    let writer = format!("{SESSION}.writer");
    journal
        .create_session(ended_record(&writer, &owner.user))
        .expect("the row the append needs");
    let runtime =
        insert_live_agent_with_writer(&registry, SESSION, owner.clone(), Box::new(std::io::sink()));
    runtime.begin_turn();
    let conn = ConnHandle::new(1);
    registry
        .queue_add(SESSION, "queued before death", &[], &[], &owner, &conn)
        .expect("add");
    assert!(
        !registry.queue_items_for_test(SESSION).is_empty(),
        "the state is loaded before retention runs"
    );
    insert_transcript(&registry, SESSION, owner.clone());

    // Age the row LAST: every journal write above (the snapshot's own
    // append among them) re-stamps the session's updated_at_ms, and what
    // retention judges is the row as its last activity left it.
    {
        let raw = rusqlite::Connection::open(dir.join("journal.db")).expect("raw handle");
        raw.execute(
            "UPDATE sessions SET updated_at_ms = 1000000000 WHERE id = ?1",
            [SESSION],
        )
        .expect("age the row");
    }

    registry
        .journal_retention_set(RetentionPatch {
            max_age_ms: Some(60_000),
            max_bytes: None,
            max_sessions: None,
            session_max_bytes: None,
        })
        .expect("age retention is set");
    // The global sweep runs inside an append's commit, when the accumulated
    // appended bytes cross the sweep threshold — so append past it, and the
    // sweep deletes the aged row in the same transaction.
    assert!(
        journal.try_append(crate::journal::EventRecord {
            session_id: format!("{SESSION}.writer"),
            generation: 1,
            seq: 1,
            kind: crate::journal::EventKind::Output,
            // Now, not an age: the append's own UPDATE stamps this onto the
            // writer row, and a stale one would age the writer into the
            // very sweep this append runs.
            ts_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_millis() as u64,
            payload: vec![0x20; 1024 * 1024 + 1],
        }),
        "the sweep-driving append is accepted"
    );

    // The roster re-read proves what the journal still holds, and the queue
    // prune rides that re-read. The append and the sweep are async to this
    // thread, so the drop is awaited, not assumed.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline && !registry.queue_items_for_test(SESSION).is_empty()
    {
        registry.state_snapshots(&owner);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        registry.queue_items_for_test(SESSION).is_empty(),
        "retention drops the in-memory queue with the rows"
    );
    let _ = dir;
}

/// Fix pass 3, P2-6: the prune must not depend on someone asking for a
/// roster. With no state watcher attached, `broadcast_session_state`
/// returns before any roster read, so only the retention path itself
/// reaches the queue map — the sweep reports its deletions and the states
/// leave memory on that report alone.
#[test]
fn retention_prunes_the_queue_without_a_roster_read() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("queue.user", "queue.client");
    let mut record = ended_record(SESSION, &owner.user);
    record.updated_at_ms = 1_000_000_000;
    journal
        .create_session(record)
        .expect("the row retention will delete");
    let writer = format!("{SESSION}.writer");
    journal
        .create_session(ended_record(&writer, &owner.user))
        .expect("the row the append needs");
    let runtime =
        insert_live_agent_with_writer(&registry, SESSION, owner.clone(), Box::new(std::io::sink()));
    runtime.begin_turn();
    let conn = ConnHandle::new(1);
    registry
        .queue_add(SESSION, "queued before death", &[], &[], &owner, &conn)
        .expect("add");
    assert!(
        !registry.queue_items_for_test(SESSION).is_empty(),
        "the state is loaded before retention runs"
    );
    insert_transcript(&registry, SESSION, owner.clone());
    {
        let raw = rusqlite::Connection::open(dir.join("journal.db")).expect("raw handle");
        raw.execute(
            "UPDATE sessions SET updated_at_ms = 1000000000 WHERE id = ?1",
            [SESSION],
        )
        .expect("age the row");
    }

    registry
        .journal_retention_set(RetentionPatch {
            max_age_ms: Some(60_000),
            max_bytes: None,
            max_sessions: None,
            session_max_bytes: None,
        })
        .expect("age retention is set");
    assert!(
        journal.try_append(crate::journal::EventRecord {
            session_id: format!("{SESSION}.writer"),
            generation: 1,
            seq: 1,
            kind: crate::journal::EventKind::Output,
            ts_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_millis() as u64,
            payload: vec![0x20; 1024 * 1024 + 1],
        }),
        "the sweep-driving append is accepted"
    );

    // No state_snapshots, no roster read, no watcher: the drop is awaited
    // from the retention path's own report and nothing else. The reads here
    // are the queue map only, never the journal roster.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while std::time::Instant::now() < deadline && !registry.queue_items_for_test(SESSION).is_empty()
    {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        registry.queue_items_for_test(SESSION).is_empty(),
        "retention prunes the queue without waiting for a roster read"
    );
    let _ = dir;
}
