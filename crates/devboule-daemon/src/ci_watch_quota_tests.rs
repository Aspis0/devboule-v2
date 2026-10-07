//! Who may add a watch: a session and a repository each have a bound, and a
//! flood from one session cannot push another session's watches out.

use super::{make_room, Refusal, MAX_OPEN_PER_REPO, MAX_PER_SESSION};
use crate::ci_summary::CiState;
use crate::ci_watch_store::{new_watch_id, now_ms, CiWatchRecord, CiWatchStore, InsertError, Wake};

fn record(session: &str, repo: &str, number: usize) -> CiWatchRecord {
    CiWatchRecord {
        watch_id: new_watch_id(),
        session_id: session.to_string(),
        owner_user: "user".to_string(),
        owner_client: "client".to_string(),
        host: "github.com".to_string(),
        repo_owner: "acme".to_string(),
        repo: repo.to_string(),
        sha: format!("{number:040x}"),
        branch: None,
        created_at_ms: now_ms() + number as u64,
        state: CiState::Queued,
        summary: None,
        wake_key: None,
        wake: Wake::NotDue,
        retry_approved: false,
        retry_count: 0,
        retried_runs: Vec::new(),
    }
}

fn settled(mut watch: CiWatchRecord, wake: Wake) -> CiWatchRecord {
    watch.state = CiState::Failed;
    watch.wake = wake;
    watch
}

#[test]
fn a_session_flooding_bogus_commits_cannot_push_out_another_sessions_watches() {
    let dir = crate::test_dirs::test_temp_dir("ci-quota-flood");
    let store = CiWatchStore::load(&dir);
    let honest: Vec<_> = (0..3).map(|n| record("honest", "widgets", n)).collect();
    for watch in &honest {
        store.insert(watch.clone()).expect("an honest watch");
    }

    let mut accepted = 0;
    for number in 0..500 {
        match store.insert(record("flooder", "widgets", 1000 + number)) {
            Ok(()) => accepted += 1,
            Err(InsertError::Quota(Refusal::Session)) => {}
            Err(other) => panic!("unexpected refusal: {other:?}"),
        }
    }
    assert_eq!(accepted, MAX_PER_SESSION, "the flooder holds its own bound");
    for watch in &honest {
        assert!(
            store.get(&watch.watch_id).is_some(),
            "the other session's watch is still there"
        );
    }
    store
        .insert(record("honest", "widgets", 9))
        .expect("the honest session can still start a watch");
}

#[test]
fn a_session_makes_room_from_its_own_settled_history_only() {
    let mut records: Vec<_> = (0..MAX_PER_SESSION)
        .map(|n| record("mine", "widgets", n))
        .collect();
    records[0] = settled(records[0].clone(), Wake::Delivered);
    let theirs = settled(record("theirs", "widgets", 99), Wake::Delivered);
    records.push(theirs.clone());
    let delivered_id = records[0].watch_id.clone();

    make_room(&mut records, &record("mine", "widgets", 100)).expect("room is made");
    assert!(
        records.iter().all(|r| r.watch_id != delivered_id),
        "its own delivered watch went"
    );
    assert!(
        records.iter().any(|r| r.watch_id == theirs.watch_id),
        "another session's history stays"
    );
}

#[test]
fn a_verdict_still_owed_is_never_dropped_to_make_room() {
    let mut records: Vec<_> = (0..MAX_PER_SESSION)
        .map(|n| settled(record("mine", "widgets", n), Wake::Pending))
        .collect();
    let refused = make_room(&mut records, &record("mine", "widgets", 100));
    assert_eq!(refused, Err(Refusal::Session));
    assert_eq!(records.len(), MAX_PER_SESSION, "nothing was forgotten");
}

#[test]
fn a_repository_bounds_its_unfinished_watches_whoever_asked() {
    let mut records: Vec<_> = (0..MAX_OPEN_PER_REPO)
        .map(|n| record(&format!("session-{n}"), "Widgets", n))
        .collect();
    let refused = make_room(&mut records, &record("newcomer", "widgets", 500));
    assert_eq!(
        refused,
        Err(Refusal::Repo),
        "the spelling of the name does not make it another repository"
    );
    make_room(&mut records, &record("newcomer", "other", 501)).expect("another repository");
}
