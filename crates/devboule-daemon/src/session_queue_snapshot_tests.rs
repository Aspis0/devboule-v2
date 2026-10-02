//! What a client is told: the whole queue on every change, the revision that
//! orders two of them, the epoch that says which daemon counted them, and the
//! connection that must never be told at all.
//!
//! The edits themselves are `session_queue_mutation_tests.rs`.

use super::session_queue::queue_revision_for_test;
use super::session_queue_fixtures::{
    attached, attached_without_queue_capability, queue_registry, queued_agent, snapshots,
};
use super::tests::{insert_live, test_owner};
use super::*;

fn owner() -> OwnerId {
    test_owner("S-1-5-21-queue-snap", "process-snap")
}

/// A busy agent session with one attached client: the turn is open, so nothing
/// drains while these tests read snapshots.
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
fn every_accepted_mutation_moves_the_revision_on_and_never_back() {
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

    let revisions: Vec<u64> = snapshots(&conn)
        .into_iter()
        .map(|snapshot| snapshot.revision)
        .collect();
    assert_eq!(
        revisions,
        vec![1, 2, 3],
        "one revision per accepted mutation, in order"
    );
    assert_eq!(queue_revision_for_test(&registry, &id), 3);

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn every_snapshot_carries_the_daemon_that_counted_its_revision() {
    let (dir, registry, journal) = queue_registry();
    let (owner, id, _runtime, conn) = busy_session(&registry, &journal, 4);

    registry
        .queue_add(&id, "op-1", "first", &[], &[], &owner, &conn)
        .expect("add");

    // The epoch is this daemon's instance id, so a client that kept a revision
    // across a restart can tell the two counters apart instead of dropping
    // every snapshot after it.
    let epoch = registry.queues.epoch().to_string();
    assert!(
        epoch.len() == 32
            && epoch
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)),
        "the epoch is a daemon instance id, 32 lowercase hex characters: {epoch:?}"
    );
    let envelope_epoch = conn
        .pull_events()
        .into_iter()
        .find_map(|pending| match pending.envelope.event {
            SessionEvent::QueueSnapshot { epoch, .. } => Some(epoch),
            _ => None,
        })
        .expect("the add published one snapshot");
    assert_eq!(envelope_epoch, epoch);

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn two_clients_attached_both_see_the_other_one_s_add() {
    let (dir, registry, journal) = queue_registry();
    let (owner, id, _runtime, first) = busy_session(&registry, &journal, 4);
    let second = attached(&registry, &id, 5, &owner);
    // The attach snapshot both clients get, then the live add.
    let _ = first.pull_events();
    let _ = second.pull_events();

    registry
        .queue_add(&id, "op-1", "from the phone", &[], &[], &owner, &second)
        .expect("add");

    for (label, conn) in [("the writer", &first), ("the reader", &second)] {
        let published = snapshots(conn);
        assert_eq!(published.len(), 1, "{label} must get the whole snapshot");
        assert_eq!(published[0].items[0].text, "from the phone");
        assert_eq!(
            published[0].revision, 1,
            "{label} compares the same revision"
        );
    }

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_client_attaching_gets_the_queue_as_it_stands() {
    let (dir, registry, journal) = queue_registry();
    let owner = owner();
    let id = compose_session_id(&owner.session_token(), "q").expect("id");
    let (runtime, _sent) = queued_agent(&registry, &journal, &owner, &id);
    runtime.begin_turn();
    registry
        .queue_add(
            &id,
            "op-1",
            "queued while nobody watched",
            &[],
            &[],
            &owner,
            &ConnHandle::new(9),
        )
        .expect("add");

    let conn = attached(&registry, &id, 4, &owner);

    let published = snapshots(&conn);
    assert_eq!(published.len(), 1, "attach delivers the current queue once");
    assert_eq!(published[0].revision, 1, "the revision it stands at");
    assert_eq!(published[0].items[0].text, "queued while nobody watched");

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn attaching_to_a_session_with_no_queue_delivers_the_empty_list() {
    let (dir, registry, journal) = queue_registry();
    let owner = owner();
    let id = compose_session_id(&owner.session_token(), "q").expect("id");
    queued_agent(&registry, &journal, &owner, &id);

    let conn = attached(&registry, &id, 4, &owner);

    let published = snapshots(&conn);
    assert_eq!(published.len(), 1, "an empty queue is still a queue");
    assert_eq!(published[0].revision, 0);
    assert!(published[0].items.is_empty());

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// A client that never offered `session.queue` cannot read a protocol-22 event,
/// so this daemon must not put one on that connection — not on attach, and not
/// on a mutation either.
#[test]
fn a_client_that_did_not_negotiate_the_queue_is_sent_no_snapshot() {
    let (dir, registry, journal) = queue_registry();
    let owner = owner();
    let id = compose_session_id(&owner.session_token(), "q").expect("id");
    let (runtime, _sent) = queued_agent(&registry, &journal, &owner, &id);
    runtime.begin_turn();
    let capable = attached(&registry, &id, 4, &owner);
    let older = attached_without_queue_capability(&registry, &id, 5, &owner);
    let _ = capable.pull_events();
    let _ = older.pull_events();

    registry
        .queue_add(
            &id,
            "op-1",
            "for the client that can read it",
            &[],
            &[],
            &owner,
            &capable,
        )
        .expect("add");

    assert_eq!(
        snapshots(&capable).len(),
        1,
        "the connection that negotiated the capability sees the queue"
    );
    assert!(
        older.pull_events().is_empty(),
        "a connection that did not negotiate it is never sent the event"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_ordinary_snapshot_carries_no_dropped_row() {
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
    registry
        .queue_move(&id, "op-4", "queue-2", 0, &owner, &conn)
        .expect("move");

    for snapshot in snapshots(&conn) {
        assert!(
            snapshot.dropped.is_empty(),
            "a drain or a remove drops nothing: {:?}",
            snapshot.dropped
        );
    }

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_terminals_attach_publishes_no_queue_at_all() {
    let (dir, registry, journal) = queue_registry();
    let owner = owner();
    let id = compose_session_id(&owner.session_token(), "t").expect("id");
    insert_live(&registry, &id, owner.clone());

    let conn = attached(&registry, &id, 4, &owner);

    assert!(
        snapshots(&conn).is_empty(),
        "a terminal has no queue to hand over"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}
