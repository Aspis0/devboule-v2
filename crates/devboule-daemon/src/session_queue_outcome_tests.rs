//! What each outcome of a claimed row's send does to it: a refusal that never
//! reached the provider puts the row back with its reason, and a write that
//! failed after it began drops it.
//!
//! The drain's triggers and its claim are `session_queue_drain_tests.rs`.

use super::session_queue::queue_items_for_test;
use super::session_queue_fixtures::{attached, journal_row, queue_registry, snapshots};
use super::tests::test_owner;
use super::*;

#[test]
fn a_refused_head_is_not_sent_again_until_a_client_acts() {
    let (dir, registry, journal) = queue_registry();
    let owner = test_owner("S-1-5-21-queue-park", "process-park");
    let id = compose_session_id(&owner.session_token(), "p").expect("id");
    journal_row(&journal, &id, &owner);
    super::tests::insert_live_agent(&registry, &id, owner.clone());
    let conn = attached(&registry, &id, 4, &owner);
    let _ = conn.pull_events();
    // A stored attachment the store has never heard of: the send refuses it
    // while resolving references, which is before a single byte is written.
    let dangling = devboule_protocol::AttachmentReference {
        session_id: id.clone(),
        digest: "b".repeat(64),
        stored_bytes: 12,
        name: String::new(),
    };
    registry
        .queue_add(
            &id,
            "op-1",
            "doomed",
            &[],
            std::slice::from_ref(&dangling),
            &owner,
            &conn,
        )
        .expect("the add is accepted; the reference is well formed");

    // Any wake that found the queue still parked must leave it parked: no
    // timer, no automatic resend, no second attempt from an unrelated frame.
    registry.resume_queue_after_frame(&id);
    let items = queue_items_for_test(&registry, &id);
    assert_eq!(items.len(), 1, "the refused row is still there");
    let reason = items[0]
        .error
        .as_deref()
        .expect("a refused send leaves its reason on the row");
    assert!(!reason.is_empty(), "the reason is not an empty string");
    assert!(
        reason.len() <= super::session_queue::MAX_QUEUE_ERROR_BYTES,
        "the stored reason is bounded: {reason:?}"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// A write that failed after it began may already be with the agent, so the row
/// is dropped rather than offered again — and the snapshot that drops it names
/// it, so the client does not watch a message of its own disappear.
#[test]
fn a_write_that_failed_after_it_began_drops_the_row_and_says_so() {
    let (dir, registry, journal) = queue_registry();
    let owner = test_owner("S-1-5-21-queue-uncertain", "process-uncertain");
    let id = compose_session_id(&owner.session_token(), "u").expect("id");
    // The default insert's writer refuses every write, and the refusal comes
    // from the write itself: the write began, so the daemon cannot know whether
    // the agent read any of it.
    journal_row(&journal, &id, &owner);
    super::tests::insert_live_agent(&registry, &id, owner.clone());
    let conn = attached(&registry, &id, 4, &owner);
    let _ = conn.pull_events();

    registry
        .queue_add(
            &id,
            "op-1",
            "may or may not have landed",
            &[],
            &[],
            &owner,
            &conn,
        )
        .expect("add");

    assert!(
        queue_items_for_test(&registry, &id).is_empty(),
        "a maybe-delivered row is not put back: a resend could say it twice"
    );
    let published = snapshots(&conn);
    let last = published.last().expect("the drop published a snapshot");
    assert_eq!(
        last.dropped,
        vec!["queue-1".to_string()],
        "the snapshot names the row it dropped"
    );
    assert!(
        last.items.is_empty(),
        "and the row is gone from the list, with no text left behind"
    );

    // Nothing resends it on its own: the next wake finds an empty queue.
    registry.resume_queue_after_frame(&id);
    assert!(queue_items_for_test(&registry, &id).is_empty());

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}
