//! Close and delete are the two ways a session leaves the daemon; both must
//! take its queue with them — in memory as well as in the journal — and a
//! later queue frame for that session must be refused rather than recreate
//! it (fix pass 1, P2-5 and P2-7).

use super::tests::{
    ended_record, insert_live_agent_with_writer, insert_transcript, test_owner, tmp_delete_registry,
};
use super::{ConnHandle, ErrorCode};

const SESSION: &str = "s.q.life";

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
