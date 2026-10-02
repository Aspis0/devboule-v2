//! Operation identity: what a retried queue frame does, what a reused id is,
//! and how many ids one queue remembers.
//!
//! A client that queues a message and never sees the reply has to be able to
//! ask again; these are the answers that make that safe.

use sha2::{Digest, Sha256};

use super::session_queue::queue_items_for_test;
use super::session_queue_fixtures::{
    queue_registry, queued_session, sent_text, snapshots, QueuedSession,
};
use super::session_queue_operations::PayloadFingerprint;
use super::*;

/// A busy agent session with one attached client: the turn is open, so nothing
/// drains while these tests queue.
fn busy_session(registry: &SessionRegistry, journal: &Arc<Journal>) -> QueuedSession {
    let session = queued_session(registry, journal, "S-1-5-21-queue-op", "process-op", "o");
    session.runtime.begin_turn();
    let _ = session.conn.pull_events();
    session
}

#[test]
fn a_retried_add_queues_the_message_once_and_says_it_replayed() {
    let (dir, registry, journal) = queue_registry();
    let fixture = busy_session(&registry, &journal);
    let (owner, id) = (&fixture.owner, &fixture.id);
    let conn = &fixture.conn;

    let first = registry
        .queue_add(id, "op-1", "hello", &[], &[], owner, conn)
        .expect("the first add is accepted");
    assert!(!first.replayed, "the first answer is not a replay");
    let second = registry
        .queue_add(id, "op-1", "hello", &[], &[], owner, conn)
        .expect("the retry is answered again");

    assert!(
        second.replayed,
        "a retry that lost its reply is answered from the record, not run twice"
    );
    assert_eq!(
        queue_items_for_test(&registry, id).len(),
        1,
        "one press, one row: the whole point of the id"
    );
    assert_eq!(
        snapshots(conn).len(),
        1,
        "a replay applies no change, so it publishes nothing"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn one_operation_id_with_two_messages_is_refused() {
    let (dir, registry, journal) = queue_registry();
    let fixture = busy_session(&registry, &journal);
    let (owner, id) = (&fixture.owner, &fixture.id);
    let conn = &fixture.conn;
    registry
        .queue_add(id, "op-1", "hello", &[], &[], owner, conn)
        .expect("add");

    let error = registry
        .queue_add(id, "op-1", "something else", &[], &[], owner, conn)
        .expect_err("the same id cannot mean two messages");

    assert_eq!(error.code, ErrorCode::OperationConflict);
    assert!(
        error.message.contains("op-1"),
        "the refusal names the id the client chose: {error:?}"
    );
    assert_eq!(
        queue_items_for_test(&registry, id)
            .iter()
            .map(|item| item.text.clone())
            .collect::<Vec<_>>(),
        vec!["hello".to_string()],
        "and the conflicting message is not queued either"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// The fingerprint is what makes an id mean one message, so it covers every
/// semantic field of the frame — the bytes of an inline attachment included,
/// which two different payloads share nothing else about. The declared type is
/// hashed too; a change of that one is refused earlier still, by the store's
/// own container walk, so the bytes are the field a client can get wrong.
#[test]
fn one_operation_id_with_two_sets_of_attachment_bytes_is_refused() {
    let (dir, registry, journal) = queue_registry();
    let fixture = busy_session(&registry, &journal);
    let (owner, id) = (&fixture.owner, &fixture.id);
    let conn = &fixture.conn;
    let notes = |data: &str| devboule_protocol::PromptAttachment {
        name: "notes.md".to_string(),
        mime_type: "text/markdown".to_string(),
        data: data.to_string(),
    };
    registry
        .queue_add(
            id,
            "op-1",
            "read this",
            &[notes("IyBub3Rlcw==")],
            &[],
            owner,
            conn,
        )
        .expect("add");

    let error = registry
        .queue_add(
            id,
            "op-1",
            "read this",
            &[notes("IyBvdGhlciBub3Rlcw==")],
            &[],
            owner,
            conn,
        )
        .expect_err("the same id cannot mean two payloads");

    assert_eq!(
        error.code,
        ErrorCode::OperationConflict,
        "same text, same name, different bytes: {error:?}"
    );

    let replay = registry
        .queue_add(
            id,
            "op-1",
            "read this",
            &[notes("IyBub3Rlcw==")],
            &[],
            owner,
            conn,
        )
        .expect("the very same payload is a replay");
    assert!(
        replay.replayed,
        "hashing the bytes must not turn a genuine retry into a conflict"
    );
    assert_eq!(
        queue_items_for_test(&registry, id).len(),
        1,
        "one press, one row"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_send_now_retry_does_not_send_the_row_twice() {
    let (dir, registry, journal) = queue_registry();
    let fixture = busy_session(&registry, &journal);
    let (owner, id) = (&fixture.owner, &fixture.id);
    let conn = &fixture.conn;
    registry
        .queue_add(id, "op-1", "only", &[], &[], owner, conn)
        .expect("add");
    let _ = conn.pull_events();

    registry
        .queue_send_now(id, "op-2", 4, "queue-1", owner, conn)
        .expect("send now");
    let retry = registry
        .queue_send_now(id, "op-2", 4, "queue-1", owner, conn)
        .expect("the retry is answered");

    assert!(
        retry.replayed,
        "the row is gone, so the retry sends nothing"
    );
    assert_eq!(
        sent_text(&fixture.sent).matches("only").count(),
        1,
        "the message reached the agent once"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_empty_or_overlong_operation_id_is_refused() {
    let (dir, registry, journal) = queue_registry();
    let fixture = busy_session(&registry, &journal);
    let (owner, id) = (&fixture.owner, &fixture.id);
    let conn = &fixture.conn;

    for refused in ["", &"a".repeat(129), "has a space"] {
        let error = registry
            .queue_add(id, refused, "hello", &[], &[], owner, conn)
            .expect_err("an id that is not one");
        assert_eq!(
            error.code,
            ErrorCode::InvalidRequest,
            "{refused:?} is not an operation id: {error:?}"
        );
    }
    assert!(
        queue_items_for_test(&registry, id).is_empty(),
        "and nothing was queued under any of them"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// The ring is what bounds this dedupe. It is per session, it is capped, and
/// the oldest id is the one that goes.
#[test]
fn the_ring_of_answered_operations_is_bounded() {
    let (dir, registry, journal) = queue_registry();
    let fixture = busy_session(&registry, &journal);
    let (owner, id) = (&fixture.owner, &fixture.id);
    let conn = &fixture.conn;
    let capacity = super::session_queue_operations::OPERATION_RING_CAPACITY;

    // One row at a time, so the queue's own row cap is not what stops the
    // walk: each add is answered by the remove behind it, and the ring keeps
    // the ids of both.
    for index in 0..capacity + 8 {
        registry
            .queue_add(
                id,
                &format!("op-add-{index}"),
                &format!("row {index}"),
                &[],
                &[],
                owner,
                conn,
            )
            .expect("add");
        let minted = queue_items_for_test(&registry, id)
            .first()
            .expect("the add put a row there")
            .item_id
            .clone();
        registry
            .queue_remove(id, &format!("op-remove-{index}"), &minted, owner, conn)
            .expect("remove");
    }
    let remembered = registry
        .queues
        .lock()
        .get(id.as_str())
        .expect("queue")
        .operations
        .len();
    assert_eq!(
        remembered, capacity,
        "one session remembers {capacity} ids and no more"
    );

    // The oldest id has fallen out of the ring, so a reuse of it is a new
    // operation rather than a silent replay.
    let after_the_ring_turned = registry
        .queue_add(id, "op-add-0", "a different row", &[], &[], owner, conn)
        .expect("a forgotten id is new again");
    assert!(
        !after_the_ring_turned.replayed,
        "the ring is bounded, so its oldest entries stop deduplicating"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// The fingerprint is the whole digest of its length-framed fields, not a cut of
/// it: two payloads that share a 64-bit prefix must still differ, and a field
/// boundary moved by one byte is a different payload.
#[test]
fn a_fingerprint_is_the_whole_sha256_of_its_framed_fields() {
    let mut hasher = Sha256::new();
    for field in ["queue_x", "ab", "c"] {
        hasher.update((field.len() as u64).to_le_bytes());
        hasher.update(field.as_bytes());
    }
    let expected: [u8; 32] = hasher.finalize().into();

    let fingerprint = PayloadFingerprint::new("queue_x")
        .field("ab")
        .field("c")
        .finish();
    let shifted = PayloadFingerprint::new("queue_x")
        .field("a")
        .field("bc")
        .finish();

    assert_eq!(fingerprint, expected);
    assert_ne!(
        shifted, expected,
        "the boundary between fields is part of it"
    );
}
