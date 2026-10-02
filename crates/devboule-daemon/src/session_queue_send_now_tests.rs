//! Send-now: the send a client asks for by name, its claim against the drain,
//! and what a refusal does to the row and to a retried press.
//!
//! The turn-end drain is `session_queue_drain_tests.rs`.

use super::session_queue::queue_items_for_test;
use super::session_queue_fixtures::{
    attached, blocking_writer, end_turn, eventually, queue_registry, queued_agent,
    queued_agent_with_writer, queued_session, sent_text, snapshots, ClosedStdinWriter,
    FailingWriter, QueuedSession,
};
use super::tests::{remote_conn, test_owner};
use super::*;

/// One busy agent session with one attached client and a writer that records
/// what the daemon sent it. The turn is open, so an add stays where a send-now
/// can find it.
fn busy_session(registry: &SessionRegistry, journal: &Arc<Journal>) -> QueuedSession {
    let session = queued_session(
        registry,
        journal,
        "S-1-5-21-queue-sendnow",
        "process-sendnow",
        "s",
    );
    session.runtime.begin_turn();
    let _ = session.conn.pull_events();
    session
}

#[test]
fn a_send_now_claims_the_row_and_sends_it_ahead_of_the_queue() {
    let (dir, registry, journal) = queue_registry();
    let fixture = busy_session(&registry, &journal);
    registry
        .queue_add(
            &fixture.id,
            "op-1",
            "first",
            &[],
            &[],
            &fixture.owner,
            &fixture.conn,
        )
        .expect("add");
    registry
        .queue_add(
            &fixture.id,
            "op-2",
            "second",
            &[],
            &[],
            &fixture.owner,
            &fixture.conn,
        )
        .expect("add");
    let _ = fixture.conn.pull_events();

    registry
        .queue_send_now(
            &fixture.id,
            "op-3",
            4,
            "queue-2",
            &fixture.owner,
            &fixture.conn,
        )
        .expect("send now");

    assert_eq!(
        sent_text(&fixture.sent),
        "second",
        "the named row went out ahead of the one in front of it"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_send_now_and_a_turn_end_racing_the_same_row_send_it_once() {
    let (dir, registry, journal) = queue_registry();
    let fixture = busy_session(&registry, &journal);
    registry
        .queue_add(
            &fixture.id,
            "op-1",
            "only",
            &[],
            &[],
            &fixture.owner,
            &fixture.conn,
        )
        .expect("add");
    let _ = fixture.conn.pull_events();

    // The turn ends while send-now is in flight. Whichever takes the claim
    // sends the row; the other must find it gone.
    let turn_end = {
        let runtime = Arc::clone(&fixture.runtime);
        std::thread::spawn(move || end_turn(&runtime))
    };
    let send_now = registry.queue_send_now(
        &fixture.id,
        "op-2",
        4,
        "queue-1",
        &fixture.owner,
        &fixture.conn,
    );
    turn_end.join().expect("the turn ended");
    eventually("one send to land", || {
        sent_text(&fixture.sent).contains("only")
    });
    std::thread::sleep(Duration::from_millis(200));

    assert_eq!(
        sent_text(&fixture.sent).matches("only").count(),
        1,
        "the row went out exactly once"
    );
    match send_now {
        Ok(_) => {}
        Err(error) => assert_eq!(
            error.code,
            ErrorCode::InvalidRequest,
            "a refused send-now is the claim being taken, not a fault: {error:?}"
        ),
    }

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_send_now_while_the_drain_sends_is_refused_and_nothing_goes_out_twice() {
    let (dir, registry, journal) = queue_registry();
    let owner = test_owner("S-1-5-21-queue-claim", "process-claim");
    let id = compose_session_id(&owner.session_token(), "c").expect("id");
    // The writer parks inside its first write, so the drain's claim is held
    // across a window this test can act in.
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

    let error = registry
        .queue_send_now(&id, "op-send", 4, "queue-2", &owner, &conn)
        .expect_err("one send on the wire per session");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(
        error.message.contains("already being sent"),
        "the refusal says why: {error:?}"
    );

    release.send(()).expect("release the blocked write");
    eventually("the drain's write to finish", || {
        written.lock().expect("written lock").len() >= 5
    });
    std::thread::sleep(Duration::from_millis(300));

    assert_eq!(
        String::from_utf8(written.lock().expect("written lock").clone()).expect("utf8"),
        "first",
        "the claimed row went out once and nothing followed it"
    );
    assert_eq!(
        queue_items_for_test(&registry, &id)
            .iter()
            .map(|item| item.text.clone())
            .collect::<Vec<_>>(),
        vec!["second".to_string()],
        "the refused send-now took its row out of nothing and put nothing back"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// A write that fails after it began drops the row rather than putting it back
/// — and the press is told, because it asked for a send.
#[test]
fn a_send_now_whose_write_failed_drops_the_row_and_reports_it() {
    let (dir, registry, journal) = queue_registry();
    let owner = test_owner("S-1-5-21-queue-sendnow-drop", "process-sendnowdrop");
    let id = compose_session_id(&owner.session_token(), "w").expect("id");
    // The writer refuses every write, and the refusal comes from the write.
    let runtime =
        queued_agent_with_writer(&registry, &journal, &owner, &id, Box::new(FailingWriter));
    runtime.begin_turn();
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

    let error = registry
        .queue_send_now(&id, "op-2", 4, "queue-1", &owner, &conn)
        .expect_err("the write failed");

    assert_eq!(
        error.code,
        ErrorCode::Io,
        "the press is told why: {error:?}"
    );
    assert!(
        queue_items_for_test(&registry, &id).is_empty(),
        "a maybe-delivered row is not queued again for the user to resend"
    );
    let published = super::session_queue_fixtures::snapshots(&conn);
    assert_eq!(
        published.last().expect("a snapshot").dropped,
        vec!["queue-1".to_string()],
        "and the snapshot says which row went"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// A transport that was closed before the write began never took a byte, so the
/// row goes back and the press is answered with the refusal. A retry of the
/// same operation id is answered the same way: it must not report a send that
/// never happened, and it must not send the row either.
#[test]
fn a_send_now_into_a_closed_transport_puts_the_row_back_and_replays_its_refusal() {
    let (dir, registry, journal) = queue_registry();
    let owner = test_owner("S-1-5-21-queue-sendnow-closed", "process-sendnowclosed");
    let id = compose_session_id(&owner.session_token(), "c").expect("id");
    let runtime = queued_agent_with_writer(
        &registry,
        &journal,
        &owner,
        &id,
        Box::new(ClosedStdinWriter::default()),
    );
    runtime.begin_turn();
    let conn = attached(&registry, &id, 4, &owner);
    let _ = conn.pull_events();
    registry
        .queue_add(&id, "op-1", "never went out", &[], &[], &owner, &conn)
        .expect("add");

    let first = registry
        .queue_send_now(&id, "op-2", 4, "queue-1", &owner, &conn)
        .expect_err("the child's stdin was already closed");
    assert_eq!(
        first.code,
        ErrorCode::Io,
        "the press is told why: {first:?}"
    );

    let retry = registry
        .queue_send_now(&id, "op-2", 4, "queue-1", &owner, &conn)
        .expect_err("the retry is answered with the outcome it is waiting for");
    assert_eq!(retry.code, first.code);
    assert_eq!(
        retry.message, first.message,
        "a retry must answer with the refusal the press got, never a success"
    );

    let items = queue_items_for_test(&registry, &id);
    assert_eq!(
        items.len(),
        1,
        "the row is still queued: nothing was delivered to resend"
    );
    assert_eq!(items[0].item_id, "queue-1");
    assert!(
        items[0].error.is_some(),
        "and it carries the refusal on its own row: {:?}",
        items[0].error
    );
    assert!(
        snapshots(&conn)
            .iter()
            .all(|snapshot| snapshot.dropped.is_empty()),
        "a row that was never written is not dropped as delivery-unknown"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// Two presses, one operation id, one send: while the first is on the wire the
/// second is told it is in flight — retryable, with the same id — rather than
/// given a second claim or a second row out, and a different payload under that
/// id is the conflict it has always been.
#[test]
fn a_send_now_retried_while_it_is_on_the_wire_is_told_it_is_in_flight() {
    let (dir, registry, journal) = queue_registry();
    let owner = test_owner("S-1-5-21-queue-sendnow-twice", "process-sendnowtwice");
    let id = compose_session_id(&owner.session_token(), "t").expect("id");
    let (writer, release) = blocking_writer();
    let written = Arc::clone(&writer.bytes);
    let seen = Arc::clone(&writer.first_write_seen);
    let runtime = queued_agent_with_writer(&registry, &journal, &owner, &id, Box::new(writer));
    let conn = attached(&registry, &id, 4, &owner);
    runtime.begin_turn();
    registry
        .queue_add(&id, "op-1", "only", &[], &[], &owner, &conn)
        .expect("add");
    let _ = conn.pull_events();

    let press = {
        let registry = registry.clone();
        let (id, owner, conn) = (id.clone(), owner.clone(), Arc::clone(&conn));
        std::thread::spawn(move || {
            registry.queue_send_now(&id, "op-2", 4, "queue-1", &owner, &conn)
        })
    };
    eventually("the press to reach the writer", || {
        seen.load(Ordering::Acquire)
    });

    let error = registry
        .queue_send_now(&id, "op-2", 4, "queue-1", &owner, &conn)
        .expect_err("the press this id names is still on the wire");
    assert_eq!(
        error.code,
        ErrorCode::OperationInFlight,
        "the retry has no effect of its own and is told to ask again: {error:?}"
    );
    let other = registry
        .queue_send_now(&id, "op-2", 4, "queue-9", &owner, &conn)
        .expect_err("the same id for another row is not a retry");
    assert_eq!(
        other.code,
        ErrorCode::OperationConflict,
        "a different payload under a taken id stays a conflict: {other:?}"
    );

    release.send(()).expect("release the blocked write");
    press
        .join()
        .expect("the press thread")
        .expect("the press went out");

    assert_eq!(
        sent_text(&written).matches("only").count(),
        1,
        "one row, one send, however many times the press was repeated"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// A stop that lands while a press is on the wire clears the queue but keeps
/// the operation's answer: the press finishes against a fenced queue, and a
/// retry after the resume replays what it did instead of being told the press
/// is still being carried out.
#[test]
fn a_stop_during_a_send_now_leaves_its_outcome_to_replay_after_a_resume() {
    let (dir, registry, journal) = queue_registry();
    let owner = test_owner("S-1-5-21-queue-sendnow-stop", "process-sendnowstop");
    let id = compose_session_id(&owner.session_token(), "p").expect("id");
    let (writer, release) = blocking_writer();
    let written = Arc::clone(&writer.bytes);
    let seen = Arc::clone(&writer.first_write_seen);
    let runtime = queued_agent_with_writer(&registry, &journal, &owner, &id, Box::new(writer));
    let conn = attached(&registry, &id, 4, &owner);
    runtime.begin_turn();
    registry
        .queue_add(&id, "op-1", "only", &[], &[], &owner, &conn)
        .expect("add");
    let _ = conn.pull_events();

    let press = {
        let registry = registry.clone();
        let (id, owner, conn) = (id.clone(), owner.clone(), Arc::clone(&conn));
        std::thread::spawn(move || {
            registry.queue_send_now(&id, "op-2", 4, "queue-1", &owner, &conn)
        })
    };
    eventually("the press to reach the writer", || {
        seen.load(Ordering::Acquire)
    });
    registry
        .stop_with_subscription(&id, 4, &owner, &conn)
        .expect("stop");

    release.send(()).expect("release the blocked write");
    let first = press.join().expect("the press thread");
    assert!(
        first.is_ok(),
        "the write was already inside the transport, so the press went out: {first:?}"
    );
    registry.resume_session_queue(&id);

    let retry = registry
        .queue_send_now(&id, "op-2", 4, "queue-1", &owner, &conn)
        .expect("the retry is answered with the outcome the press had");
    assert!(
        retry.replayed,
        "the retry replays the recorded answer rather than sending again"
    );
    assert_eq!(
        sent_text(&written).matches("only").count(),
        1,
        "one row, one send, across the stop and the resume"
    );
    assert!(queue_items_for_test(&registry, &id).is_empty());

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_send_now_for_a_row_that_is_gone_is_refused() {
    let (dir, registry, journal) = queue_registry();
    let fixture = busy_session(&registry, &journal);
    registry
        .queue_add(
            &fixture.id,
            "op-1",
            "first",
            &[],
            &[],
            &fixture.owner,
            &fixture.conn,
        )
        .expect("add");
    let _ = fixture.conn.pull_events();

    let error = registry
        .queue_send_now(
            &fixture.id,
            "op-2",
            4,
            "queue-9",
            &fixture.owner,
            &fixture.conn,
        )
        .expect_err("no such row");

    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(
        queue_items_for_test(&registry, &fixture.id).len(),
        1,
        "the refused press took nothing out"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_send_now_needs_the_subscription_a_send_needs() {
    let (dir, registry, journal) = queue_registry();
    let fixture = busy_session(&registry, &journal);
    registry
        .queue_add(
            &fixture.id,
            "op-1",
            "first",
            &[],
            &[],
            &fixture.owner,
            &fixture.conn,
        )
        .expect("add");
    let _ = fixture.conn.pull_events();

    let error = registry
        .queue_send_now(
            &fixture.id,
            "op-2",
            77,
            "queue-1",
            &fixture.owner,
            &fixture.conn,
        )
        .expect_err("a subscription this connection does not hold");

    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(
        queue_items_for_test(&registry, &fixture.id).len(),
        1,
        "the refused press took nothing out"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// A paired device may send into a session it may reach, but interrupting a
/// running turn is the act `SessionInterrupt` decides and no capability opens
/// it — so send-now from a device is refused before the row is claimed.
#[test]
fn a_paired_device_may_queue_but_may_not_interrupt_through_send_now() {
    let (dir, registry, journal) = queue_registry();
    let owner = test_owner("S-1-5-21-queue-sendnow-peer", "process-sendnowpeer");
    let id = compose_session_id(&owner.session_token(), "i").expect("id");
    let (runtime, _sent) = queued_agent(&registry, &journal, &owner, &id);
    runtime.begin_turn();
    let phone = remote_conn(PeerRole::Client, Some(owner.user.as_str()));
    registry
        .attach_with_subscription(&id, phone.id, None, &phone, &owner, false)
        .expect("the device attaches");
    registry
        .queue_add(&id, "op-1", "queued from a phone", &[], &[], &owner, &phone)
        .expect("a send-capable device may queue");
    let _ = phone.pull_events();

    let error = registry
        .queue_send_now(&id, "op-2", phone.id, "queue-1", &owner, &phone)
        .expect_err("a device may not interrupt");

    assert_eq!(error.code, ErrorCode::Unauthorized);
    assert_eq!(
        queue_items_for_test(&registry, &id).len(),
        1,
        "the refused press took the row out of nothing"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}
