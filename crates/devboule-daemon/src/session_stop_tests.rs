//! The stop doors: what a live row, a recovered transcript and an unknown id
//! answer, and the wire door's subscription gate before any mutation.

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use super::tests::{insert_live, test_owner, tmp_delete_registry};
use super::{
    compose_session_id, ConnHandle, ErrorCode, OwnerId, RegistryEntry, SessionKind,
    SessionRegistry, SessionState, SessionStateSnapshot, TranscriptIntegrity,
};
use crate::journal::{new_session_record, Journal};

/// The row a restart left behind, hydrated the way an attach hydrates it: a
/// `Live` journal row with no reaped exit is the recovered state, and the
/// attach is what puts the `Transcript` entry in the registry.
fn recovered_transcript(
    registry: &SessionRegistry,
    journal: &Journal,
    owner: &OwnerId,
) -> (String, Arc<ConnHandle>) {
    let session_id = compose_session_id(&owner.session_token(), "stoprec").expect("id");
    journal
        .upsert_blocking(new_session_record(
            session_id.clone(),
            owner.user.clone(),
            None,
            SessionKind::Acp,
            "Recovered agent",
        ))
        .expect("the left-over row");
    let conn = ConnHandle::new(9);
    registry
        .attach_with_subscription(&session_id, 901, None, &conn, owner, false)
        .expect("the recovered row hydrates on attach");
    (session_id, conn)
}

#[test]
fn a_stop_of_a_recovered_transcript_lands_twice_without_a_write() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-stop-recovered", "process-stop-recovered");
    let (session_id, conn) = recovered_transcript(&registry, &journal, &owner);
    let pushed: Arc<Mutex<Vec<SessionStateSnapshot>>> = Arc::new(Mutex::new(Vec::new()));
    let fired = Arc::clone(&pushed);
    registry.set_transition_sink(Arc::new(move |_owner, snapshots| {
        if let Some(snapshots) = snapshots {
            fired.lock().expect("push log").extend(snapshots);
        }
    }));

    registry
        .stop_with_subscription(&session_id, 901, &owner, &conn)
        .expect("the archive of a row with no process lands");
    registry
        .stop_with_subscription(&session_id, 901, &owner, &conn)
        .expect("a second archive is the same stop, not a refusal");

    assert!(
        pushed.lock().expect("push log").is_empty(),
        "a stop with nothing to stop pushes no row"
    );
    {
        let map = registry.inner.lock().expect("registry");
        let entry = map.get(&session_id).expect("the transcript stays");
        assert!(
            matches!(entry, RegistryEntry::Transcript(_)),
            "the stop keeps the row and its replay"
        );
        assert!(
            matches!(entry.metadata().state, SessionState::Recovered { .. }),
            "the stop leaves the row recovered: {:?}",
            entry.metadata().state
        );
    }

    journal.flush().expect("flush");
    journal.shutdown();
    let reopened = Journal::open(&dir.join("journal.db")).expect("cold reopen");
    let record = reopened
        .list()
        .expect("rows")
        .into_iter()
        .find(|row| row.id == session_id)
        .expect("the transcript survives the restart");
    let state = record.to_session().state;
    assert!(
        matches!(
            state,
            SessionState::Recovered {
                integrity: TranscriptIntegrity::Unverifiable { .. },
                ..
            }
        ),
        "the cold read still holds the unverifiable recovery: {state:?}"
    );
    reopened.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_refused_subscription_leaves_the_live_row_unpreserved() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-stop-subscription", "process-stop-subscription");
    let session_id = compose_session_id(&owner.session_token(), "stoplive").expect("id");
    insert_live(&registry, &session_id, owner.clone());
    let conn = ConnHandle::new(11);
    registry
        .attach_with_subscription(&session_id, 7, None, &conn, &owner, false)
        .expect("the live row attaches");

    let error = registry
        .stop_with_subscription(&session_id, 8, &owner, &conn)
        .expect_err("a subscription that does not observe the row is refused");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    let runtime = registry.runtime(&session_id).expect("the live runtime");
    assert!(
        !runtime.stop_requested(),
        "a refused stop must not record a stop request"
    );

    let map = registry.inner.lock().expect("registry");
    let live = map
        .get(&session_id)
        .and_then(RegistryEntry::as_peer_visible)
        .expect("the live row stays");
    assert!(
        !live.preserve_on_exit.load(Ordering::Acquire),
        "a refused stop must not mark the live row preserved"
    );
    drop(map);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// The request is on the runtime before the provider is signalled, so the
/// end the kill produces reads as a stop, whatever the provider says.
#[test]
fn a_stop_records_the_request_on_the_live_runtime() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-stop-records", "process-stop-records");
    let session_id = compose_session_id(&owner.session_token(), "stoprec").expect("id");
    insert_live(&registry, &session_id, owner.clone());
    let conn = ConnHandle::new(12);
    registry
        .attach_with_subscription(&session_id, 17, None, &conn, &owner, false)
        .expect("the live row attaches");
    let runtime = registry.runtime(&session_id).expect("the live runtime");
    assert!(!runtime.stop_requested());

    registry
        .stop_with_subscription(&session_id, 17, &owner, &conn)
        .expect("the stop lands");
    assert!(runtime.stop_requested());

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_stop_of_an_ended_row_records_no_request() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-stop-ended", "process-stop-ended");
    let session_id = compose_session_id(&owner.session_token(), "stopend").expect("id");
    insert_live(&registry, &session_id, owner.clone());
    let conn = ConnHandle::new(13);
    registry
        .attach_with_subscription(&session_id, 19, None, &conn, &owner, false)
        .expect("the live row attaches");
    let runtime = registry.runtime(&session_id).expect("the live runtime");
    runtime.finish(Some(0));

    registry
        .stop_with_subscription(&session_id, 19, &owner, &conn)
        .expect("a stop of a row that already ended is not an error");
    assert!(
        !runtime.stop_requested(),
        "a stop of an ended row must not be recorded as the cause of its end"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_stop_of_an_unknown_id_still_refuses() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-stop-unknown", "process-stop-unknown");
    let conn = ConnHandle::new(10);

    let error = registry
        .stop("s.unknown.1", &owner)
        .expect_err("no such session");
    assert_eq!(error.code, ErrorCode::SessionNotFound);
    let error = registry
        .stop_with_subscription("s.unknown.1", 1, &owner, &conn)
        .expect_err("no such session");
    assert_eq!(error.code, ErrorCode::SessionNotFound);
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}
