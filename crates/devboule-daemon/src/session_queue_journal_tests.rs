//! The queue is the daemon's memory and nothing else: after every kind of
//! mutation the journal must hold no row that names it, and a replay must not
//! produce one.
//!
//! This is the difference from a queue that is journaled, and it is why a
//! client's draft text never reaches a transcript, a replay, or another
//! machine's history.

use super::session_queue_fixtures::{attached, queue_registry, queued_agent, snapshots};
use super::tests::test_owner;
use super::*;
use rusqlite::Connection;

/// Every event row this journal holds for one session, as its lowercased kind.
fn event_kinds(path: &Path, session_id: &str) -> Vec<String> {
    let conn = Connection::open(path).expect("open the journal file");
    let mut statement = conn
        .prepare("SELECT kind FROM events WHERE session_id = ?1 ORDER BY generation, seq")
        .expect("prepare");
    let rows = statement
        .query_map([session_id], |row| row.get::<_, String>(0))
        .expect("query");
    rows.map(|row| row.expect("row").to_lowercase()).collect()
}

#[test]
fn no_queue_mutation_leaves_a_row_in_the_events_table() {
    let (dir, registry, journal) = queue_registry();
    let owner = test_owner("S-1-5-21-queue-journal", "process-journal");
    let id = compose_session_id(&owner.session_token(), "j").expect("id");
    let (runtime, _sent) = queued_agent(&registry, &journal, &owner, &id);
    runtime.begin_turn();
    let conn = attached(&registry, &id, 4, &owner);
    let _ = conn.pull_events();

    registry
        .queue_add(&id, "op-1", "a draft nobody sent", &[], &[], &owner, &conn)
        .expect("add");
    registry
        .queue_add(&id, "op-2", "another draft", &[], &[], &owner, &conn)
        .expect("add");
    registry
        .queue_edit(&id, "op-3", "queue-2", "an edited draft", &owner, &conn)
        .expect("edit");
    registry
        .queue_move(&id, "op-4", "queue-2", 0, &owner, &conn)
        .expect("move");
    registry
        .queue_remove(&id, "op-5", "queue-1", &owner, &conn)
        .expect("remove");
    let _ = registry.queue_send_now(&id, "op-6", 4, "queue-2", &owner, &conn);

    journal.shutdown();
    let kinds = event_kinds(&dir.join("journal.db"), &id);
    assert!(
        kinds.iter().all(|kind| !kind.contains("queue")),
        "no journaled event may name the queue: {kinds:?}"
    );
    // The send still journals the prompt it delivered — that is what a send
    // journals, and it is what makes the transcript real. What must be absent
    // is the queue's own bookkeeping.
    assert!(
        !kinds.is_empty(),
        "the send itself still journals: {kinds:?}"
    );

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_replay_over_a_session_brings_no_queue_snapshot_back() {
    let (dir, registry, journal) = queue_registry();
    let owner = test_owner("S-1-5-21-queue-replay", "process-replay");
    let id = compose_session_id(&owner.session_token(), "r").expect("id");
    let (runtime, _sent) = queued_agent(&registry, &journal, &owner, &id);
    runtime.begin_turn();
    let conn = attached(&registry, &id, 4, &owner);
    let _ = conn.pull_events();

    registry
        .queue_add(
            &id,
            "op-1",
            "queued before a restart",
            &[],
            &[],
            &owner,
            &conn,
        )
        .expect("add");
    registry
        .queue_add(&id, "op-2", "and another", &[], &[], &owner, &conn)
        .expect("add");

    let replay = journal.replay(&id).expect("replay");
    assert!(
        replay
            .events
            .iter()
            .all(|event| !matches!(event, SessionEvent::QueueSnapshot { .. })),
        "a replay must not resurrect a snapshot: {:?}",
        replay.events.len()
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_detached_client_rejoining_gets_one_snapshot_and_not_a_replay_of_the_last() {
    let (dir, registry, journal) = queue_registry();
    let owner = test_owner("S-1-5-21-queue-detach", "process-detach");
    let id = compose_session_id(&owner.session_token(), "d").expect("id");
    let (runtime, _sent) = queued_agent(&registry, &journal, &owner, &id);
    runtime.begin_turn();
    let first = attached(&registry, &id, 4, &owner);
    registry
        .queue_add(
            &id,
            "op-1",
            "queued while nobody watched",
            &[],
            &[],
            &owner,
            &first,
        )
        .expect("add");
    let _ = first.pull_events();

    runtime.detach_subscription(4, 4);
    let second = attached(&registry, &id, 5, &owner);

    let published = snapshots(&second);
    assert_eq!(
        published.len(),
        1,
        "one snapshot on attach: the queue itself, not the detach backlog"
    );
    assert_eq!(
        published[0].revision, 1,
        "at the revision the queue stands at"
    );
    assert_eq!(published[0].items.len(), 1);
    assert_eq!(published[0].items[0].text, "queued while nobody watched");

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}
