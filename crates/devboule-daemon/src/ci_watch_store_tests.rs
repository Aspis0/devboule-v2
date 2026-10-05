//! The persistent watches: what survives a restart, and the one-wake rule.

use super::{new_watch_id, now_ms, CiWatchRecord, CiWatchStore, Wake};
use crate::ci_summary::CiState;

fn record(session: &str, sha: &str) -> CiWatchRecord {
    CiWatchRecord {
        watch_id: new_watch_id(),
        session_id: session.to_string(),
        owner_user: "user".to_string(),
        owner_client: "client".to_string(),
        host: "github.com".to_string(),
        repo_owner: "acme".to_string(),
        repo: "widgets".to_string(),
        sha: sha.to_string(),
        created_at_ms: now_ms(),
        state: CiState::Queued,
        summary: None,
        wake_key: None,
        wake: Wake::NotDue,
    }
}

fn dir(tag: &str) -> std::path::PathBuf {
    crate::test_dirs::test_temp_dir(&format!("ci-store-{tag}"))
}

#[test]
fn a_finished_watch_survives_a_restart_with_its_wake_state() {
    let dir = dir("restart");
    let store = CiWatchStore::load(&dir);
    let watch = record("s1", "aaaa");
    store.insert(watch.clone()).expect("insert");
    assert!(store
        .complete(&watch.watch_id, CiState::Failed, "text".to_string())
        .expect("complete"));

    let reloaded = CiWatchStore::load(&dir);
    let found = reloaded.get(&watch.watch_id).expect("kept");
    assert_eq!(found.state, CiState::Failed);
    assert_eq!(found.wake, Wake::Pending);
    assert_eq!(found.wake_key, Some(format!("{}:failed", watch.watch_id)));
    assert_eq!(reloaded.pending_wakes().len(), 1);
    assert!(reloaded.open().is_empty());
}

#[test]
fn a_verdict_is_recorded_once_and_a_wake_is_claimed_once() {
    let store = CiWatchStore::load(&dir("once"));
    let watch = record("s1", "aaaa");
    store.insert(watch.clone()).expect("insert");
    assert!(store
        .complete(&watch.watch_id, CiState::Passed, "first".to_string())
        .expect("first"));
    assert!(
        !store
            .complete(&watch.watch_id, CiState::Failed, "second".to_string())
            .expect("second"),
        "a finished watch keeps its first verdict"
    );
    assert_eq!(
        store.get(&watch.watch_id).expect("kept").summary.as_deref(),
        Some("first")
    );

    assert!(store.claim_wake(&watch.watch_id).is_some());
    assert!(
        store.claim_wake(&watch.watch_id).is_none(),
        "the claim is taken once"
    );
    store.finish_wake(&watch.watch_id, true).expect("settle");
    assert!(
        store.claim_wake(&watch.watch_id).is_none(),
        "a delivered wake is never claimed again"
    );
}

#[test]
fn a_claim_found_after_a_restart_is_not_sent_again_but_a_given_back_one_is() {
    let dir = dir("claim");
    let store = CiWatchStore::load(&dir);
    let sent = record("s1", "aaaa");
    let returned = record("s2", "bbbb");
    for watch in [&sent, &returned] {
        store.insert(watch.clone()).expect("insert");
        store
            .complete(&watch.watch_id, CiState::Failed, "t".to_string())
            .expect("complete");
        assert!(store.claim_wake(&watch.watch_id).is_some());
    }
    store
        .finish_wake(&returned.watch_id, false)
        .expect("given back");

    let reloaded = CiWatchStore::load(&dir);
    let pending: Vec<String> = reloaded
        .pending_wakes()
        .into_iter()
        .map(|watch| watch.watch_id)
        .collect();
    assert_eq!(
        pending,
        vec![returned.watch_id],
        "only the claim that did not send is owed"
    );
    assert_eq!(
        reloaded.get(&sent.watch_id).expect("kept").wake,
        Wake::Sending
    );
}

#[test]
fn asking_twice_for_one_commit_finds_the_first_watch() {
    let store = CiWatchStore::load(&dir("find"));
    let watch = record("s1", "aaaa");
    store.insert(watch.clone()).expect("insert");
    assert_eq!(
        store
            .find("s1", "acme/widgets", "aaaa")
            .map(|found| found.watch_id),
        Some(watch.watch_id)
    );
    assert!(
        store.find("s2", "acme/widgets", "aaaa").is_none(),
        "another session has its own"
    );
    assert!(store.find("s1", "acme/widgets", "bbbb").is_none());
}

#[test]
fn an_unreadable_file_is_moved_aside_and_the_store_starts_empty() {
    let dir = dir("corrupt");
    std::fs::write(dir.join("ci_watches.json"), b"{ not json").expect("write");
    let store = CiWatchStore::load(&dir);
    assert!(store.open().is_empty());
    assert!(
        dir.join("ci_watches.corrupt").exists(),
        "the evidence is kept"
    );
}
