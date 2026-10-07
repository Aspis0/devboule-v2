//! The persistent watches: what survives a restart, and the one-wake rule.

use super::{new_watch_id, now_ms, Admit, CiWatchRecord, CiWatchStore, InsertError, Wake};
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
        branch: None,
        created_at_ms: now_ms(),
        state: CiState::Queued,
        summary: None,
        wake_key: None,
        wake: Wake::NotDue,
        retry_approved: false,
        retry_count: 0,
        retry_issued: false,
        retried_runs: Vec::new(),
        retry_evidence: Vec::new(),
    }
}

/// The `index`th of many watches that fit the per-session and per-repository
/// bounds: one session each, fifty to a repository.
fn spread(index: usize) -> CiWatchRecord {
    let mut watch = record(&format!("s{index}"), &format!("{index:040}"));
    watch.repo = format!("w{}", index / 50);
    watch
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
            .find("s1", "acme/widgets", "aaaa", None)
            .map(|found| found.watch_id),
        Some(watch.watch_id)
    );
    assert!(
        store.find("s2", "acme/widgets", "aaaa", None).is_none(),
        "another session has its own"
    );
    assert!(store.find("s1", "acme/widgets", "bbbb", None).is_none());
    assert!(
        store
            .find("s1", "acme/widgets", "aaaa", Some("main"))
            .is_none(),
        "a branch watch is its own watch"
    );
}

/// The key is looked up and the watch stored under one lock: a second call
/// for the same key answers the first watch and stores nothing.
#[test]
fn a_keyed_insert_answers_the_watch_that_is_already_there() {
    let store = CiWatchStore::load(&dir("admit"));
    let first = record("s1", "aaaa");
    let inserted = store.find_or_insert(first.clone()).expect("first");
    assert!(matches!(inserted, Admit::Inserted(_)));

    let again = store.find_or_insert(record("s1", "aaaa")).expect("second");
    let Admit::Existing(existing) = again else {
        panic!("the key already had a watch");
    };
    assert_eq!(existing.watch_id, first.watch_id);
    assert_eq!(store.open().len(), 1, "and only one watch exists");

    let mut branch = record("s1", "aaaa");
    branch.branch = Some("main".to_string());
    assert!(matches!(
        store.find_or_insert(branch).expect("branch"),
        Admit::Inserted(_)
    ));
    assert_eq!(store.open().len(), 2);
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

fn aged(mut watch: CiWatchRecord, age_ms: u64) -> CiWatchRecord {
    watch.created_at_ms = now_ms().saturating_sub(age_ms);
    watch
}

const WEEK_MS: u64 = 7 * 24 * 60 * 60 * 1000;

#[test]
fn terminal_watches_age_out_whatever_their_wake_did() {
    let dir = dir("ageout");
    let store = CiWatchStore::load(&dir);
    let mut old_delivered = record("s1", "aaaa");
    old_delivered.state = CiState::Failed;
    old_delivered.wake = Wake::Delivered;
    let mut old_owed = record("s2", "bbbb");
    old_owed.state = CiState::Failed;
    old_owed.wake = Wake::Pending;
    let mut old_sending = record("s3", "cccc");
    old_sending.state = CiState::Failed;
    old_sending.wake = Wake::Sending;
    for watch in [&old_delivered, &old_owed, &old_sending] {
        store
            .insert(aged(watch.clone(), WEEK_MS + 1))
            .expect("seed");
    }
    let mut fresh = record("s4", "dddd");
    fresh.state = CiState::Failed;
    fresh.wake = Wake::Pending;
    store.insert(fresh.clone()).expect("seed");
    let open = record("s5", "eeee");
    store.insert(open.clone()).expect("seed");

    store.insert(record("s6", "ffff")).expect("insert");
    assert!(store.get(&old_delivered.watch_id).is_none());
    assert!(
        store.get(&old_owed.watch_id).is_none(),
        "an undeliverable wake is kept a bounded time, then dropped"
    );
    assert!(store.get(&old_sending.watch_id).is_none());
    assert!(store.get(&fresh.watch_id).is_some(), "recent stays");
    assert!(store.get(&open.watch_id).is_some(), "open stays");
    let sixth = record("s6", "ffff");
    let sixth_id = sixth.watch_id.clone();
    store.insert(sixth).expect("insert");
    assert!(store.get(&sixth_id).is_some(), "the new watch lands");
}

#[test]
fn a_full_store_evicts_finished_history_before_refusing() {
    let dir = dir("evict");
    let store = CiWatchStore::load(&dir);
    for index in 0..500 {
        let mut watch = spread(index);
        if index == 0 {
            watch.state = CiState::Failed;
            watch.wake = Wake::Delivered;
        }
        store.insert(watch).expect("seed");
    }
    let evicted = record("s", "ffff");
    store.insert(evicted.clone()).expect("room is made");
    assert_eq!(
        store
            .find("s0", "acme/w0", &format!("{:040}", 0), None)
            .map(|found| found.watch_id),
        None,
        "the oldest finished watch went"
    );
    assert!(store.get(&evicted.watch_id).is_some());
}

#[test]
fn a_full_store_of_open_watches_still_refuses() {
    let dir = dir("full");
    let store = CiWatchStore::load(&dir);
    for index in 0..500 {
        store.insert(spread(index)).expect("seed");
    }
    let refused = store.insert(record("s", "ffff"));
    assert!(
        matches!(refused, Err(InsertError::Full)),
        "with nothing finished there is nothing to evict: {refused:?}"
    );
}
