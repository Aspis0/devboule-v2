//! What an accepted close or stop does to a queue: it fences it, clears it,
//! and a stop's fence lasts only until the session is resumed — for a queue
//! that held nothing, for a refused second stop, and for a send that settles
//! after the session stopped being sendable.
//!
//! The two lifecycle doors are deliberately covered separately, because they
//! leave different things behind. A close removes the session from the
//! registry, so nothing can resolve it again and the queue entry is released
//! with it. A stop preserves the session — a resume finds it — so the fence is
//! the only thing standing between a stopped session and a queue until that
//! resume: that is the path the empty-queue fence, the late settle and the
//! reopening are proved on.

use super::session_idle_close_tests::{idle_state, linked_child, linked_creator, shut_down};
use super::session_queue::{queue_is_held_for_test, queue_items_for_test, queue_revision_for_test};
use super::session_queue_fixtures::{
    attached, blocking_writer, end_turn, eventually, queue_registry, queued_agent,
    queued_agent_with_writer, snapshots,
};
use super::session_queue_operations::PayloadFingerprint;
use super::tests::test_owner;
use super::*;

fn owner() -> OwnerId {
    test_owner("S-1-5-21-queue-life", "process-life")
}

/// A busy agent session with two queued rows and one attached client.
fn queued_session(
    registry: &SessionRegistry,
    journal: &Arc<Journal>,
) -> (OwnerId, String, Arc<SessionRuntime>, Arc<ConnHandle>) {
    let owner = owner();
    let id = compose_session_id(&owner.session_token(), "l").expect("id");
    let (runtime, _sent) = queued_agent(registry, journal, &owner, &id);
    let conn = attached(registry, &id, 4, &owner);
    runtime.begin_turn();
    for (index, text) in ["first", "second"].iter().enumerate() {
        registry
            .queue_add(&id, &format!("op-{index}"), text, &[], &[], &owner, &conn)
            .expect("add");
    }
    let _ = conn.pull_events();
    (owner, id, runtime, conn)
}

#[test]
fn an_accepted_close_clears_the_queue_and_publishes_the_empty_list() {
    let (dir, registry, journal) = queue_registry();
    let (owner, id, _runtime, conn) = queued_session(&registry, &journal);

    assert!(registry.close(&id, &owner, &None).expect("close"));

    assert!(!queue_is_held_for_test(&registry, &id));
    let cleared = snapshots(&conn)
        .pop()
        .expect("the close publishes the cleared queue");
    assert!(
        cleared.items.is_empty(),
        "the last snapshot a client holds is empty"
    );
    assert!(
        cleared.revision > 0,
        "the cleared snapshot carries the revision it cleared at"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_accepted_close_leaves_nothing_a_later_frame_can_reopen() {
    let (dir, registry, journal) = queue_registry();
    let (owner, id, _runtime, conn) = queued_session(&registry, &journal);
    registry.close(&id, &owner, &None).expect("close");

    let error = registry
        .queue_add(&id, "op-1", "after the close", &[], &[], &owner, &conn)
        .expect_err("a closed session does not queue");

    assert_eq!(
        error.code,
        ErrorCode::SessionNotFound,
        "the session itself is what is gone"
    );
    assert!(
        !queue_is_held_for_test(&registry, &id),
        "the refused frame must not have recreated the queue"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_stop_fences_the_queue_even_when_it_held_nothing() {
    let (dir, registry, journal) = queue_registry();
    let owner = owner();
    let id = compose_session_id(&owner.session_token(), "e").expect("id");
    queued_agent(&registry, &journal, &owner, &id);
    let conn = attached(&registry, &id, 4, &owner);
    let _ = conn.pull_events();

    registry
        .stop_with_subscription(&id, 4, &owner, &conn)
        .expect("stop");

    let error = registry
        .queue_add(&id, "op-1", "after the stop", &[], &[], &owner, &conn)
        .expect_err("an empty queue is fenced too");
    assert_eq!(
        error.code,
        ErrorCode::SessionNotFound,
        "the fence is what refuses, whatever the list held"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_stop_fences_the_queue_and_clears_what_it_held() {
    let (dir, registry, journal) = queue_registry();
    let (owner, id, _runtime, conn) = queued_session(&registry, &journal);

    registry
        .stop_with_subscription(&id, 4, &owner, &conn)
        .expect("stop");

    assert!(
        queue_items_for_test(&registry, &id).is_empty(),
        "a stop clears what the user had queued before it"
    );
    let cleared = snapshots(&conn)
        .pop()
        .expect("the stop publishes the cleared queue");
    assert!(cleared.items.is_empty());

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// The fence belongs to the stopped session, not to the queue forever: the
/// resume that makes the session sendable again makes its queue usable again,
/// and the revision moves on so a client does not read the reopened queue as
/// the cleared one it already applied.
#[test]
fn a_resume_reopens_the_queue_a_stop_fenced() {
    let (dir, registry, journal) = queue_registry();
    let (owner, id, _runtime, conn) = queued_session(&registry, &journal);
    registry
        .stop_with_subscription(&id, 4, &owner, &conn)
        .expect("stop");
    let after_the_stop = queue_revision_for_test(&registry, &id);

    registry.resume_session_queue(&id);
    let reopened = snapshots(&conn)
        .pop()
        .expect("the reopen publishes the queue it reopened");
    assert!(
        reopened.items.is_empty(),
        "what was queued before the stop does not come back"
    );
    assert!(
        reopened.revision > after_the_stop,
        "the revision only moves on: {} then {}",
        after_the_stop,
        reopened.revision
    );

    registry
        .queue_add(
            &id,
            "op-reopen",
            "after the resume",
            &[],
            &[],
            &owner,
            &conn,
        )
        .expect("a resumed session queues again");
    assert_eq!(queue_items_for_test(&registry, &id).len(), 1);

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_refused_second_stop_cannot_clear_the_first_stops_fence() {
    let (dir, registry, journal) = queue_registry();
    let (owner, id, _runtime, conn) = queued_session(&registry, &journal);
    let stranger = test_owner("S-1-5-21-queue-other", "process-other");

    registry
        .stop_with_subscription(&id, 4, &owner, &conn)
        .expect("the first stop is accepted");
    let refused = registry.stop_with_subscription(&id, 4, &stranger, &conn);
    assert_eq!(
        refused
            .expect_err("a stranger may not stop this session")
            .code,
        ErrorCode::Unauthorized
    );

    // The first stop's fence is still the thing refusing, and the queue it
    // cleared is still cleared.
    let error = registry
        .queue_add(&id, "op-later", "later", &[], &[], &owner, &conn)
        .expect_err("still fenced");
    assert_eq!(error.code, ErrorCode::SessionNotFound);
    assert!(queue_items_for_test(&registry, &id).is_empty());

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_queue_that_nothing_ever_queued_is_released_by_a_close() {
    let (dir, registry, journal) = queue_registry();
    let owner = owner();
    let id = compose_session_id(&owner.session_token(), "n").expect("id");
    queued_agent(&registry, &journal, &owner, &id);
    let conn = attached(&registry, &id, 4, &owner);
    let _ = conn.pull_events();

    registry.close(&id, &owner, &None).expect("close");

    assert!(
        !queue_is_held_for_test(&registry, &id),
        "a close that found no queue still leaves none behind"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_send_that_settles_after_a_stop_neither_sends_nor_republishes() {
    let (dir, registry, journal) = queue_registry();
    let owner = owner();
    let id = compose_session_id(&owner.session_token(), "s").expect("id");
    // The writer parks inside its first write until this test lets it go, so
    // the stop lands while the drain's send is genuinely on the wire.
    let (writer, release) = blocking_writer();
    let written = Arc::clone(&writer.bytes);
    let seen = Arc::clone(&writer.first_write_seen);
    let runtime = queued_agent_with_writer(&registry, &journal, &owner, &id, Box::new(writer));
    let conn = attached(&registry, &id, 4, &owner);
    runtime.begin_turn();
    for (index, text) in ["first", "second"].iter().enumerate() {
        registry
            .queue_add(&id, &format!("op-{index}"), text, &[], &[], &owner, &conn)
            .expect("add");
    }
    let _ = conn.pull_events();

    // The turn's end is the drain's trigger; the drain then claims the front
    // row and blocks inside its write.
    end_turn(&runtime);
    eventually("the drain to reach the writer", || {
        seen.load(Ordering::Acquire)
    });
    assert_eq!(
        queue_items_for_test(&registry, &id)
            .iter()
            .map(|item| item.text.clone())
            .collect::<Vec<_>>(),
        vec!["second".to_string()],
        "the claimed row is out of the list while its send is in flight"
    );

    registry
        .stop_with_subscription(&id, 4, &owner, &conn)
        .expect("stop");
    let after_the_stop = queue_revision_for_test(&registry, &id);
    release.send(()).expect("release the blocked write");
    eventually("the drain to settle after the stop", || {
        written.lock().expect("written lock").len() >= 5
    });
    // Give a settle that would have spoken a moment longer to speak.
    std::thread::sleep(Duration::from_millis(300));

    assert!(
        queue_items_for_test(&registry, &id).is_empty(),
        "a late settle must not put the claimed row back"
    );
    assert_eq!(
        String::from_utf8(written.lock().expect("written lock").clone()).expect("utf8"),
        "first",
        "and it must not send the next row on the way out"
    );
    assert_eq!(
        queue_revision_for_test(&registry, &id),
        after_the_stop,
        "a fenced settle publishes nothing at all"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// The add resolves its session first and writes its row later, with the
/// attachment deposits in between, so a close can land between the two.
#[test]
fn an_add_resolved_before_a_close_cannot_recreate_the_forgotten_queue() {
    let (dir, registry, journal) = queue_registry();
    let owner = owner();
    let id = compose_session_id(&owner.session_token(), "g").expect("id");
    queued_agent(&registry, &journal, &owner, &id);
    let conn = attached(&registry, &id, 4, &owner);
    let target = registry
        .queue_target(&id, &owner, &conn)
        .expect("the add resolves its session");
    registry.close(&id, &owner, &None).expect("close");

    let mut ran = false;
    let fingerprint = PayloadFingerprint::new("queue_add").field("late").finish();
    let error = registry
        .mutate_queue(&id, &target, "op-late", fingerprint, |_state| {
            ran = true;
            Ok(())
        })
        .expect_err("the session this add resolved is gone");

    assert_eq!(
        error.code,
        ErrorCode::SessionNotFound,
        "the same answer a frame for a closed session gets"
    );
    assert!(!ran, "a refused add changes nothing");
    assert!(
        !queue_is_held_for_test(&registry, &id),
        "and it must not bring back the queue the close forgot"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_idle_close_releases_the_queue_like_any_other_close() {
    let (state, dir) = idle_state("queue-forget");
    let registry = &state.sessions;
    let owner = owner();
    let creator = "idle-queue-creator";
    linked_creator(registry, creator, &owner);
    let child = linked_child(registry, "idle-queue-child", &owner, creator);

    let start = Instant::now();
    assert_eq!(registry.sweep_idle_close_children(&state, start), 0);
    assert_eq!(
        registry.sweep_idle_close_children(&state, start + Duration::from_secs(30 * 60)),
        1,
        "the sweep closed the child"
    );

    assert!(
        !queue_is_held_for_test(registry, &child),
        "the closed child's queue must not outlive it"
    );
    shut_down(&state, &dir);
}
