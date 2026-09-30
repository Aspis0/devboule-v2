//! Tests for the wire-only orphan resolution at transcript hydration
//! (`session_permission_recovery.rs`), every one driven through the real
//! attach road, because the claim is about the hydration a client's attach
//! takes after a daemon restart — not about a helper the road might bypass.
//!
//! The five claims: an orphaned request replays a synthetic resolution after
//! it while the journal gains no row; a resolved request gets nothing,
//! whether modern (attribution present) or pre-2026-09-14 (a real
//! `allow_once` ledger row, no attribution); two hydrations write nothing
//! and resolve each replay exactly once; a live session is untouched; and a
//! journal that refuses writes still hydrates.
//!
//! What the app receives is read from the runtime's `transcript_agent_reports`
//! — the exact store the pull serves in key order — while the journal replay
//! and the `permissions` table prove nothing durable moved.

use std::path::Path;

use rusqlite::Connection;

use super::tests::{ended_record, insert_live, test_owner, tmp_delete_registry};
use super::{
    compose_session_id, ConnHandle, Journal, OwnerId, SessionEvent, SessionOrigin, SessionRegistry,
};

fn request(card_id: &str) -> SessionEvent {
    SessionEvent::PermissionRequest {
        tool_call_id: card_id.to_string(),
        title: "Run command".to_string(),
        description: None,
        command: Some("dir".to_string()),
        args: None,
        cwd: None,
        env: None,
        options: Vec::new(),
        is_chooser: None,
        kind: None,
        plan: None,
        questions: None,
        origin: SessionOrigin::local(),
        create_agent: None,
    }
}

fn resolved(card_id: &str) -> SessionEvent {
    SessionEvent::PermissionResolved {
        tool_call_id: card_id.to_string(),
        selected_option_id: None,
        selected_option_kind: None,
        selected_option_name: None,
        answered_by: None,
    }
}

/// A 2026-09-04..14-era resolution: decided `allow_once`, journaled before
/// the attribution event existed, so no `PermissionAnswered` was ever
/// written for it.
fn resolved_allow(card_id: &str) -> SessionEvent {
    SessionEvent::PermissionResolved {
        tool_call_id: card_id.to_string(),
        selected_option_id: None,
        selected_option_kind: Some("allow_once".to_string()),
        selected_option_name: None,
        answered_by: None,
    }
}

fn answered(card_id: &str, outcome: &str) -> SessionEvent {
    SessionEvent::PermissionAnswered {
        card_id: card_id.to_string(),
        answered_by: None,
        outcome: outcome.to_string(),
    }
}

fn record_event(journal: &Journal, id: &str, seq: u64, event: &SessionEvent) {
    let record = crate::journal::agent_report_record(id.to_string(), 1, seq, event)
        .expect("the event is reportable");
    journal.append_blocking(record).expect("the row lands");
}

fn record_ledger(journal: &Journal, id: &str, card_id: &str, outcome: &str) {
    let payload = serde_json::to_vec(&request(card_id)).expect("the request serialises");
    journal
        .record_permission(id, card_id, outcome, &payload)
        .expect("the ledger row lands");
}

fn setup(journal: &Journal, owner: &OwnerId, id: &str) {
    journal
        .upsert_blocking(ended_record(id, &owner.user))
        .expect("session row");
}

/// What the app pull serves for one session: the runtime's agent-report map
/// in key order, the same order the wire carries.
fn runtime_events(registry: &SessionRegistry, id: &str) -> Vec<SessionEvent> {
    let map = registry.inner.lock().expect("registry");
    let entry = map.get(id).expect("the entry is cached");
    let runtime = entry.runtime();
    let stream = runtime.lock_stream().expect("stream");
    stream
        .transcript_agent_reports
        .values()
        .flat_map(|row| row.iter().cloned())
        .collect()
}

fn count_resolved(events: &[SessionEvent], card_id: &str) -> usize {
    events
        .iter()
        .filter(|event| {
            matches!(event, SessionEvent::PermissionResolved { tool_call_id, .. }
                if tool_call_id == card_id)
        })
        .count()
}

fn count_answered(events: &[SessionEvent], card_id: &str) -> usize {
    events
        .iter()
        .filter(|event| {
            matches!(event, SessionEvent::PermissionAnswered { card_id: found, .. }
                if found == card_id)
        })
        .count()
}

fn resolved_kind(events: &[SessionEvent], card_id: &str) -> Option<Option<String>> {
    events.iter().find_map(|event| match event {
        SessionEvent::PermissionResolved {
            tool_call_id,
            selected_option_kind,
            ..
        } if tool_call_id == card_id => Some(selected_option_kind.clone()),
        _ => None,
    })
}

/// Journaled event rows for one session, after a flush so the writer's queue
/// is never the reason a count is short.
fn journal_len(journal: &Journal, id: &str) -> usize {
    journal.flush().expect("flush");
    journal.replay(id).expect("replay").events.len()
}

fn ledger_count(journal: &Journal, dir: &Path, id: &str, card_id: &str) -> i64 {
    journal.flush().expect("flush");
    Connection::open(dir.join("journal.db"))
        .expect("open journal")
        .query_row(
            "SELECT COUNT(*) FROM permissions WHERE session_id = ?1 AND request_id = ?2",
            [id, card_id],
            |row| row.get(0),
        )
        .expect("count ledger rows")
}

fn ledger_outcome(journal: &Journal, dir: &Path, id: &str, card_id: &str) -> String {
    journal.flush().expect("flush");
    Connection::open(dir.join("journal.db"))
        .expect("open journal")
        .query_row(
            "SELECT outcome FROM permissions WHERE session_id = ?1 AND request_id = ?2",
            [id, card_id],
            |row| row.get(0),
        )
        .expect("the ledger row exists")
}

/// The restart's card: a request the journal holds and nothing resolves
/// replays with a synthetic resolution after it — while the journal itself
/// gains no row, no ledger row, and no attribution: the resolution is wire
/// history, not a verdict anyone made.
/// Mutants: the resolution journaled (the journal grows and B1/B2 return);
/// the resolution before the request (an app reading in order resolves a
/// card it was never shown); a ledger row written (the delegation count
/// starts counting cards nobody answered).
#[test]
fn an_orphaned_request_replays_a_wire_only_resolution_after_it() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-zombie-1", "process-1111");
    let id = compose_session_id(&owner.session_token(), "zombie1").expect("id");
    setup(&journal, &owner, &id);
    record_event(&journal, &id, 1, &request("card-1"));
    let before = journal_len(&journal, &id);

    let conn = ConnHandle::new(1);
    registry
        .attach(&id, None, &conn, &owner, false)
        .expect("the hydration runs the wire-only pass");

    let events = runtime_events(&registry, &id);
    let request_at = events
        .iter()
        .position(|event| {
            matches!(event, SessionEvent::PermissionRequest { tool_call_id, .. }
                if tool_call_id == "card-1")
        })
        .expect("the request replays");
    let resolved_at = events
        .iter()
        .position(|event| {
            matches!(event, SessionEvent::PermissionResolved { tool_call_id, .. }
                if tool_call_id == "card-1")
        })
        .expect("the replay carries a resolution for the orphaned request");
    assert!(
        resolved_at > request_at,
        "the resolution follows the request: request at {request_at}, resolved at {resolved_at}"
    );
    assert_eq!(
        count_resolved(&events, "card-1"),
        1,
        "exactly one resolution, however many times the road runs"
    );
    assert_eq!(
        count_answered(&events, "card-1"),
        0,
        "no attribution: nobody answered"
    );
    assert_eq!(
        journal_len(&journal, &id),
        before,
        "hydration writes nothing to the journal"
    );
    assert_eq!(
        ledger_count(&journal, &dir, &id, "card-1"),
        0,
        "no ledger row for a card nobody answered"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A request the journal already resolved gains no synthetic — neither the
/// modern shape (attribution present) nor the 2026-09-04..14 shape (a real
/// `allow_once` ledger row, journaled before the attribution event
/// existed): the old verdict stands exactly as made, ledger included.
/// Mutants: the era read from the attribution instead of the resolution (a
/// second `cancelled` row falsifies the `allow_once` ledger row — B1); any
/// ledger write (the delegation count moves).
#[test]
fn an_already_resolved_request_gets_nothing_regardless_of_era() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-zombie-3", "process-1111");
    let id = compose_session_id(&owner.session_token(), "zombie3").expect("id");
    setup(&journal, &owner, &id);
    record_event(&journal, &id, 1, &request("old-1"));
    record_event(&journal, &id, 2, &resolved_allow("old-1"));
    record_ledger(&journal, &id, "old-1", "allow_once");
    record_event(&journal, &id, 3, &request("new-1"));
    record_event(&journal, &id, 4, &resolved("new-1"));
    record_event(&journal, &id, 5, &answered("new-1", "cancelled"));
    record_ledger(&journal, &id, "new-1", "cancelled");
    let before = journal_len(&journal, &id);

    let conn = ConnHandle::new(1);
    registry
        .attach(&id, None, &conn, &owner, false)
        .expect("the hydration runs");

    let events = runtime_events(&registry, &id);
    assert_eq!(
        count_resolved(&events, "old-1"),
        1,
        "the pre-attribution resolution gains no synthetic twin"
    );
    assert_eq!(
        resolved_kind(&events, "old-1"),
        Some(Some("allow_once".to_string())),
        "the old verdict reads exactly as journaled"
    );
    assert_eq!(
        count_answered(&events, "old-1"),
        0,
        "no attribution is invented for the old shape"
    );
    assert_eq!(
        count_resolved(&events, "new-1"),
        1,
        "the modern resolution gains no twin either"
    );
    assert_eq!(
        count_answered(&events, "new-1"),
        1,
        "the modern attribution is untouched"
    );
    assert_eq!(
        ledger_outcome(&journal, &dir, &id, "old-1"),
        "allow_once",
        "the ledger row keeps the verdict actually made"
    );
    assert_eq!(
        journal_len(&journal, &id),
        before,
        "hydration writes nothing to the journal"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Two hydrations — the second after the idle transcript was dropped, the
/// road a later attach really takes — leave the journal row count exactly
/// where it was, while each replay carries exactly one synthetic resolution:
/// wire-only state recomputed per hydration, never stored.
/// Mutant: the synthetic journaled (the second hydration doubles it, or the
/// journal grows at all).
#[test]
fn two_hydrations_write_nothing_and_resolve_each_replay_once() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-zombie-2", "process-1111");
    let id = compose_session_id(&owner.session_token(), "zombie2").expect("id");
    setup(&journal, &owner, &id);
    record_event(&journal, &id, 1, &request("card-2"));
    let before = journal_len(&journal, &id);

    let conn = ConnHandle::new(1);
    registry
        .attach(&id, None, &conn, &owner, false)
        .expect("first hydration");
    let first = runtime_events(&registry, &id);
    assert_eq!(
        count_resolved(&first, "card-2"),
        1,
        "one synthetic resolution in the first replay"
    );
    assert_eq!(count_answered(&first, "card-2"), 0, "no attribution");
    assert_eq!(
        journal_len(&journal, &id),
        before,
        "the first hydration writes nothing"
    );

    registry
        .detach(&id, &conn, &owner)
        .expect("the idle transcript is dropped");
    registry
        .attach(&id, None, &conn, &owner, false)
        .expect("second hydration");
    let second = runtime_events(&registry, &id);
    assert_eq!(
        count_resolved(&second, "card-2"),
        1,
        "one synthetic resolution in the second replay, recomputed"
    );
    assert_eq!(count_answered(&second, "card-2"), 0, "still no attribution");
    assert_eq!(
        journal_len(&journal, &id),
        before,
        "the second hydration writes nothing either"
    );
    assert_eq!(
        ledger_count(&journal, &dir, &id, "card-2"),
        0,
        "no ledger row across either hydration"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A live session is never touched: its attach answers through the existing
/// registry entry, hydration never runs, and the journaled row stays exactly
/// as it is — the control proves the guard, by resolving the same row in the
/// replay once the entry is a transcript.
/// Mutant: the patch moved before the live check (a live card resolves in
/// the replay while its broker still holds it).
#[test]
fn a_live_session_is_untouched_by_recovery() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-zombie-4", "process-1111");
    let id = compose_session_id(&owner.session_token(), "zombie4").expect("id");
    setup(&journal, &owner, &id);
    record_event(&journal, &id, 1, &request("card-4"));
    let before = journal_len(&journal, &id);
    insert_live(&registry, &id, owner.clone());

    let conn = ConnHandle::new(1);
    registry
        .attach(&id, None, &conn, &owner, false)
        .expect("the live entry attaches without hydrating");
    assert_eq!(
        journal_len(&journal, &id),
        before,
        "a live session's journal is untouched"
    );
    assert!(
        !registry
            .inner
            .lock()
            .expect("registry")
            .get(&id)
            .expect("the live entry")
            .runtime()
            .is_transcript(),
        "the live entry stays a live session, not a transcript"
    );

    registry
        .detach(&id, &conn, &owner)
        .expect("the live session detaches");
    let removed = registry.inner.lock().expect("registry").remove(&id);
    assert!(
        removed.is_some(),
        "the live entry is gone, as a restart would leave it"
    );
    registry
        .attach(&id, None, &conn, &owner, false)
        .expect("the same journal hydrates as a transcript");
    assert_eq!(
        count_resolved(&runtime_events(&registry, &id), "card-4"),
        1,
        "once the entry is a transcript, the same row resolves in the replay"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A journal that refuses writes still hydrates: with the write lock parked
/// by a second connection's uncommitted row, every write the old design
/// issued fails, while this one issues none — the transcript opens and the
/// orphan resolves in the replay. A dropped holder rolls back, so a panic
/// here never wedges the journal.
/// Mutant: any journal write on the hydration road (the attach fails — B2).
#[test]
fn a_journal_that_refuses_writes_still_hydrates() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-zombie-5", "process-1111");
    let id = compose_session_id(&owner.session_token(), "zombie5").expect("id");
    setup(&journal, &owner, &id);
    record_event(&journal, &id, 1, &request("card-5"));
    let before = journal_len(&journal, &id);

    let holder = Connection::open(dir.join("journal.db")).expect("second connection");
    holder
        .execute_batch("BEGIN IMMEDIATE")
        .expect("park the write lock");
    holder
        .execute(
            "INSERT INTO permissions (session_id, request_id, ts_ms, outcome, payload, checksum) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            ("holder-session", "holder-1", 0i64, "cancelled", vec![0u8], 0i64),
        )
        .expect("park an uncommitted row");

    let conn = ConnHandle::new(1);
    registry
        .attach(&id, None, &conn, &owner, false)
        .expect("hydration performs no write, so a read-only journal opens");
    let events = runtime_events(&registry, &id);
    assert_eq!(
        count_resolved(&events, "card-5"),
        1,
        "the orphan resolves in the replay even with writes refused"
    );

    holder
        .execute_batch("ROLLBACK")
        .expect("release the write lock");
    // Only now, with writes possible again, the count is readable — and it
    // proves the parked hydration wrote nothing past the lock.
    assert_eq!(
        journal_len(&journal, &id),
        before,
        "nothing was written past the parked lock"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
