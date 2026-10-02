//! The drain: the turn-end trigger, the idle-add trigger, the claim that
//! keeps one send on the wire per session however many frames arrive, and what
//! each of a send's three outcomes does to the row.
//!
//! The two lifecycle fences that close and stop put in front of a claimed write
//! are here too, with the write in flight.
//!
//! The send-now a client asks for by name is `session_queue_send_now_tests.rs`.

use super::session_queue::{queue_items_for_test, queue_revision_for_test};
use super::session_queue_fixtures::{
    attached, blocking_writer, end_turn, eventually, queue_registry, queued_agent,
    queued_agent_counting_kills, queued_agent_with_writer, queued_session, sent_text,
    QueuedSession,
};
use super::tests::test_owner;
use super::*;

/// One idle agent session: nothing has begun a turn, so an add drains at once,
/// which is today's app semantics.
fn idle_session(registry: &SessionRegistry, journal: &Arc<Journal>) -> QueuedSession {
    queued_session(
        registry,
        journal,
        "S-1-5-21-queue-drain",
        "process-drain",
        "d",
    )
}

/// The session's writer, so a test can hold the lock a send takes just before
/// it writes. A send parked on it has claimed its row and resolved its
/// session; nothing has been written yet.
fn session_writer(
    registry: &SessionRegistry,
    session_id: &str,
) -> Arc<Mutex<Box<dyn Write + Send>>> {
    let map = registry
        .inner
        .lock()
        .expect("registry lock for the writer under test");
    Arc::clone(
        &map.get(session_id)
            .expect("session")
            .as_peer_visible()
            .expect("a live session")
            .writer,
    )
}

#[test]
fn an_add_while_idle_sends_the_row_at_once() {
    let (dir, registry, journal) = queue_registry();
    let fixture = idle_session(&registry, &journal);

    registry
        .queue_add(
            &fixture.id,
            "op-1",
            "while idle",
            &[],
            &[],
            &fixture.owner,
            &ConnHandle::new(9),
        )
        .expect("add");

    assert_eq!(
        sent_text(&fixture.sent),
        "while idle",
        "today's app semantics: an idle session sends the row the add put there"
    );
    assert!(queue_items_for_test(&registry, &fixture.id).is_empty());

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_turn_end_sends_item_zero_exactly_once_with_two_clients_attached() {
    let (dir, registry, journal) = queue_registry();
    let first_client = idle_session(&registry, &journal);
    let owner = &first_client.owner;
    let id = &first_client.id;
    let second = attached(&registry, id, 5, owner);
    first_client.runtime.begin_turn();
    registry
        .queue_add(id, "op-1", "first", &[], &[], owner, &first_client.conn)
        .expect("add");
    registry
        .queue_add(id, "op-2", "second", &[], &[], owner, &second)
        .expect("add");
    let _ = first_client.conn.pull_events();
    let _ = second.pull_events();
    assert_eq!(
        sent_text(&first_client.sent),
        "",
        "a running turn holds the queue"
    );

    end_turn(&first_client.runtime);

    eventually("the drain to send the front row", || {
        sent_text(&first_client.sent) == "first"
    });
    assert_eq!(
        sent_text(&first_client.sent),
        "first",
        "the front row went out once and nothing followed it"
    );
    assert_eq!(
        queue_items_for_test(&registry, id)
            .iter()
            .map(|item| item.text.clone())
            .collect::<Vec<_>>(),
        vec!["second".to_string()],
        "the rest of the queue waits for the turn this send started"
    );
    // Both clients were sent the queue as it moved, not only the one that
    // pressed.
    for conn in [&first_client.conn, &second] {
        let revisions: Vec<u64> = super::session_queue_fixtures::snapshots(conn)
            .into_iter()
            .map(|snapshot| snapshot.revision)
            .collect();
        assert!(
            !revisions.is_empty(),
            "every attached client sees the drain's snapshots"
        );
        assert!(
            revisions.windows(2).all(|pair| pair[0] < pair[1]),
            "revisions never go backwards: {revisions:?}"
        );
    }

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// The turn-end arm is what keeps a busy turn from collecting one hook per
/// mutation; this is the observable half of that: many mutations in one turn,
/// one pass when it ends.
#[test]
fn several_mutations_in_one_turn_arm_one_drain_and_send_one_row() {
    let (dir, registry, journal) = queue_registry();
    let fixture = idle_session(&registry, &journal);
    fixture.runtime.begin_turn();
    for index in 0..5 {
        registry
            .queue_add(
                &fixture.id,
                &format!("op-{index}"),
                &format!("row {index}"),
                &[],
                &[],
                &fixture.owner,
                &fixture.conn,
            )
            .expect("add");
    }
    let _ = fixture.conn.pull_events();
    assert_eq!(
        fixture.runtime.turn_end_hook_count(),
        1,
        "five mutations in one turn arm one hook, not one each"
    );

    end_turn(&fixture.runtime);

    eventually("the drain to send the front row", || {
        sent_text(&fixture.sent).starts_with("row 0")
    });
    std::thread::sleep(Duration::from_millis(300));

    assert_eq!(
        sent_text(&fixture.sent),
        "row 0",
        "one pass sent one row, whatever the number of arms it started from"
    );
    assert_eq!(
        queue_items_for_test(&registry, &fixture.id).len(),
        4,
        "the rest waits for the turn this send started"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_queue_frame_while_the_drain_sends_does_not_start_a_second_send() {
    let (dir, registry, journal) = queue_registry();
    let owner = test_owner("S-1-5-21-queue-frame", "process-frame");
    let id = compose_session_id(&owner.session_token(), "f").expect("id");
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

    end_turn(&runtime);
    eventually("the drain to reach the writer", || {
        seen.load(Ordering::Acquire)
    });

    // A move wakes the drain, and the drain is already sending: the second
    // row must wait for the first send to settle.
    registry
        .queue_move(&id, "op-move", "queue-2", 0, &owner, &conn)
        .expect("the move itself is accepted");

    release.send(()).expect("release the blocked write");
    eventually("the drain's write to finish", || {
        written.lock().expect("written lock").len() >= 5
    });
    std::thread::sleep(Duration::from_millis(300));

    assert_eq!(
        String::from_utf8(written.lock().expect("written lock").clone()).expect("utf8"),
        "first",
        "one send per session, however many frames arrive while it is on the wire"
    );
    assert_eq!(
        queue_items_for_test(&registry, &id)
            .iter()
            .map(|item| item.text.clone())
            .collect::<Vec<_>>(),
        vec!["second".to_string()],
        "the move did what it asked and nothing more"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_claimed_row_is_not_in_the_queue_while_its_send_is_in_flight() {
    let (dir, registry, journal) = queue_registry();
    let owner = test_owner("S-1-5-21-queue-gone", "process-gone");
    let id = compose_session_id(&owner.session_token(), "g").expect("id");
    let (writer, release) = blocking_writer();
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

    end_turn(&runtime);
    eventually("the drain to reach the writer", || {
        seen.load(Ordering::Acquire)
    });

    // The row a send is holding is out of the list, so an edit or a remove
    // aimed at it has nothing to act on and cannot make it be sent twice.
    for outcome in [
        registry.queue_edit(&id, "op-edit", "queue-1", "rewritten", &owner, &conn),
        registry.queue_remove(&id, "op-remove", "queue-1", &owner, &conn),
    ] {
        assert_eq!(
            outcome
                .expect_err("the claimed row is not in the queue")
                .code,
            ErrorCode::InvalidRequest
        );
    }
    assert_eq!(
        queue_items_for_test(&registry, &id)
            .iter()
            .map(|item| item.text.clone())
            .collect::<Vec<_>>(),
        vec!["second".to_string()],
        "neither refusal touched the queue"
    );

    release.send(()).expect("release the blocked write");
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// A stop with a write blocked inside the transport, driven through whichever
/// door the caller gives. A stop is the escape hatch for a hung agent, so it
/// may not wait for the very write it is there to interrupt: it fences, clears
/// and reaches the killer while that write is still blocked. Bounded by a
/// timeout rather than judged by a sleep.
fn a_stop_returns_while_a_write_is_blocked(
    tag: &str,
    stop: fn(&SessionRegistry, &str, &OwnerId, &ConnHandle) -> Result<(), WireError>,
) {
    let (dir, registry, journal) = queue_registry();
    let owner = test_owner(&format!("S-1-5-21-queue-stop-{tag}"), "process-stopfence");
    let id = compose_session_id(&owner.session_token(), "s").expect("id");
    let (writer, release) = blocking_writer();
    let written = Arc::clone(&writer.bytes);
    let seen = Arc::clone(&writer.first_write_seen);
    let (runtime, kills) =
        queued_agent_counting_kills(&registry, &journal, &owner, &id, Box::new(writer));
    let conn = attached(&registry, &id, 4, &owner);
    runtime.begin_turn();
    registry
        .queue_add(&id, "op-1", "first", &[], &[], &owner, &conn)
        .expect("add");
    let _ = conn.pull_events();

    end_turn(&runtime);
    eventually("the drain to reach the writer", || {
        seen.load(Ordering::Acquire)
    });

    let (done, stopped) = std::sync::mpsc::channel();
    let stop_thread = {
        let (registry, id, owner, conn) = (
            registry.clone(),
            id.clone(),
            owner.clone(),
            Arc::clone(&conn),
        );
        std::thread::spawn(move || {
            let _ = done.send(stop(&registry, &id, &owner, &conn));
        })
    };
    stopped
        .recv_timeout(Duration::from_secs(2))
        .expect("the stop returned while the write was still blocked")
        .expect("stop");
    assert_eq!(
        kills.load(Ordering::SeqCst),
        1,
        "and it reached the killer, which is what releases a wedged write"
    );
    assert_eq!(
        registry
            .queue_add(&id, "op-2", "after the stop", &[], &[], &owner, &conn)
            .expect_err("a stopped session's queue takes nothing")
            .code,
        ErrorCode::SessionNotFound
    );
    let after_the_stop = queue_revision_for_test(&registry, &id);

    release.send(()).expect("release the blocked write");
    stop_thread.join().expect("the stop thread");
    eventually("the late settle to put the claim down", || {
        registry
            .queues
            .lock()
            .get(&id)
            .is_some_and(|state| !state.draining)
    });

    assert_eq!(
        sent_text(&written),
        "first",
        "the write that had passed its check went out once: the declared remainder"
    );
    assert_eq!(
        queue_revision_for_test(&registry, &id),
        after_the_stop,
        "and its settle republished nothing against the fenced queue"
    );
    assert!(
        queue_items_for_test(&registry, &id).is_empty(),
        "nor put a row back"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_stop_with_a_subscription_returns_while_a_write_is_blocked() {
    a_stop_returns_while_a_write_is_blocked("sub", |registry, id, owner, conn| {
        registry.stop_with_subscription(id, 4, owner, conn)
    });
}

#[test]
fn a_plain_stop_returns_while_a_write_is_blocked() {
    a_stop_returns_while_a_write_is_blocked("plain", |registry, id, owner, _conn| {
        registry.stop(id, owner)
    });
}

/// A close that lands while the write is already inside the transport cannot
/// take it back: the close does not wait on the session writer, because a write
/// blocked on a hung child pipe would hang the close with it. The prompt goes
/// into a process that is on its way out — the declared remainder — and what
/// must hold afterwards is that the settle brings nothing back, republishes
/// nothing and reaches for no forgotten queue.
#[test]
fn a_close_that_lands_mid_write_restores_nothing_and_republishes_nothing() {
    let (dir, registry, journal) = queue_registry();
    let owner = test_owner("S-1-5-21-queue-midwrite", "process-midwrite");
    let id = compose_session_id(&owner.session_token(), "m").expect("id");
    let (writer, release) = blocking_writer();
    let written = Arc::clone(&writer.bytes);
    let seen = Arc::clone(&writer.first_write_seen);
    let runtime = queued_agent_with_writer(&registry, &journal, &owner, &id, Box::new(writer));
    let conn = attached(&registry, &id, 4, &owner);
    runtime.begin_turn();
    registry
        .queue_add(&id, "op-1", "first", &[], &[], &owner, &conn)
        .expect("add");
    let _ = conn.pull_events();

    end_turn(&runtime);
    eventually("the drain to reach the writer", || {
        seen.load(Ordering::Acquire)
    });
    registry.close(&id, &owner, &None).expect("close");

    release.send(()).expect("release the blocked write");
    std::thread::sleep(Duration::from_millis(300));

    assert_eq!(
        sent_text(&written),
        "first",
        "the bytes that were already inside the transport went out, once"
    );
    assert_eq!(
        queue_revision_for_test(&registry, &id),
        0,
        "and the settle reached no queue: the close forgot it"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// A close that lands while a drain has claimed a row and is waiting to write:
/// the queue is fenced before the session leaves the registry, and the write is
/// re-checked under the session writer lock, so the prompt never reaches a
/// session that is on its way out.
#[test]
fn a_close_that_lands_before_a_claimed_write_stops_it() {
    let (dir, registry, journal) = queue_registry();
    let owner = test_owner("S-1-5-21-queue-claim-close", "process-claimclose");
    let id = compose_session_id(&owner.session_token(), "x").expect("id");
    let (runtime, sent) = queued_agent(&registry, &journal, &owner, &id);
    let conn = attached(&registry, &id, 4, &owner);
    runtime.begin_turn();
    registry
        .queue_add(&id, "op-1", "first", &[], &[], &owner, &conn)
        .expect("add");
    let _ = conn.pull_events();

    // Hold the writer the send takes just before it writes. The drain claims
    // its row and parks here, with nothing on the wire.
    let writer = session_writer(&registry, &id);
    let held = writer.lock().expect("the writer under test");
    end_turn(&runtime);
    eventually("the drain to claim its row", || {
        queue_items_for_test(&registry, &id).is_empty()
    });
    assert_eq!(sent_text(&sent), "", "the claimed row is not written yet");

    registry.close(&id, &owner, &None).expect("close");
    let after_the_close = queue_revision_for_test(&registry, &id);
    drop(held);
    std::thread::sleep(Duration::from_millis(300));

    assert_eq!(
        sent_text(&sent),
        "",
        "a queue fenced by the close must not write into the session it fenced"
    );
    assert_eq!(
        queue_revision_for_test(&registry, &id),
        after_the_close,
        "and the late settle publishes nothing"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}
