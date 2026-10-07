//! Which watches a pass serves: the call budget, the repository that fails
//! or hangs, and the watches a pass had to pass over.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::Duration;

use super::{Budget, Limits, Passes, Turn};
use crate::ci_summary::CiState;
use crate::ci_watch_store::{CiWatchRecord, Wake};

fn watch(id: &str, repo: &str) -> CiWatchRecord {
    CiWatchRecord {
        watch_id: id.to_string(),
        session_id: "session".to_string(),
        owner_user: "user".to_string(),
        owner_client: "client".to_string(),
        host: "github.com".to_string(),
        repo_owner: "acme".to_string(),
        repo: repo.to_string(),
        sha: "0".repeat(40),
        branch: None,
        created_at_ms: 0,
        state: CiState::Running,
        summary: None,
        wake_key: None,
        wake: Wake::NotDue,
        retry_approved: false,
        retry_count: 0,
        retry_issued: false,
        retried_runs: Vec::new(),
        retry_attempts: Vec::new(),
    }
}

fn limits(requests: usize) -> Limits {
    Limits {
        requests,
        workers: 4,
        repo_time: Duration::from_secs(30),
    }
}

/// Every turn costs one request; the ids served, in the order they ran.
fn costing_one(
    served: &Mutex<Vec<String>>,
) -> impl Fn(&CiWatchRecord, &Budget) -> Turn + Sync + '_ {
    move |record, budget| {
        if !budget.take(1) {
            return Turn::NoBudget;
        }
        served.lock().expect("served").push(record.watch_id.clone());
        Turn::Done
    }
}

#[test]
fn a_pass_stops_at_its_request_budget_and_the_rest_go_first_next_time() {
    let passes = Passes::default();
    let open: Vec<_> = ["a", "b", "c", "d", "e"]
        .iter()
        .map(|id| watch(id, "widgets"))
        .collect();
    let served = Mutex::new(Vec::new());

    passes.run(open.clone(), limits(3), &costing_one(&served));
    assert_eq!(
        served.lock().expect("served").len(),
        3,
        "three requests, three turns"
    );

    served.lock().expect("served").clear();
    passes.run(open, limits(3), &costing_one(&served));
    let second = served.lock().expect("served").clone();
    assert_eq!(second.len(), 3);
    assert!(
        second.contains(&"d".to_string()) && second.contains(&"e".to_string()),
        "the two watches the first pass could not afford come first: {second:?}"
    );
}

#[test]
fn a_failing_repository_skips_only_its_own_watches() {
    let passes = Passes::default();
    let open = vec![
        watch("a1", "broken"),
        watch("a2", "broken"),
        watch("b1", "healthy"),
    ];
    let served = Mutex::new(Vec::new());
    passes.run(open, limits(40), &|record, _budget| {
        served.lock().expect("served").push(record.watch_id.clone());
        if record.repo == "broken" {
            Turn::RepoFailed
        } else {
            Turn::Done
        }
    });
    let mut ran = served.lock().expect("served").clone();
    ran.sort();
    assert_eq!(
        ran,
        ["a1", "b1"],
        "the broken repository's second watch waits"
    );
}

#[test]
fn a_repository_that_hangs_does_not_hold_the_others_behind_it() {
    let passes = Passes::default();
    let open = vec![watch("slow", "stuck"), watch("fast", "healthy")];
    let (done, heard) = mpsc::channel();
    let done = Mutex::new(done);
    let heard = Mutex::new(heard);
    let stuck_heard_it = AtomicBool::new(false);
    passes.run(open, limits(40), &|record, _budget| {
        if record.repo == "stuck" {
            // Waits for the other repository's turn: serial work would sit
            // here until the timeout.
            let signalled = heard
                .lock()
                .expect("heard")
                .recv_timeout(Duration::from_secs(10))
                .is_ok();
            stuck_heard_it.store(signalled, Ordering::SeqCst);
        } else {
            done.lock().expect("done").send(()).expect("signal");
        }
        Turn::Done
    });
    assert!(
        stuck_heard_it.load(Ordering::SeqCst),
        "the healthy repository never got its turn"
    );
}

#[test]
fn a_repository_past_its_time_passes_its_remaining_watches_over() {
    let passes = Passes::default();
    let open = vec![watch("first", "slow"), watch("second", "slow")];
    let served = Mutex::new(Vec::new());
    let tight = Limits {
        requests: 40,
        workers: 1,
        repo_time: Duration::from_millis(30),
    };
    passes.run(open.clone(), tight, &|record, _budget| {
        served.lock().expect("served").push(record.watch_id.clone());
        std::thread::sleep(Duration::from_millis(80));
        Turn::Done
    });
    assert_eq!(*served.lock().expect("served"), ["first"]);

    served.lock().expect("served").clear();
    passes.run(open, tight, &|record, _budget| {
        served.lock().expect("served").push(record.watch_id.clone());
        std::thread::sleep(Duration::from_millis(80));
        Turn::Done
    });
    assert_eq!(
        *served.lock().expect("served"),
        ["second"],
        "the watch the time cost its turn is served first"
    );
}

#[test]
fn a_panicking_watch_does_not_end_the_pass() {
    let passes = Passes::default();
    let open = vec![watch("boom", "widgets"), watch("fine", "widgets")];
    let served = Mutex::new(Vec::new());
    passes.run(open, limits(40), &|record, _budget| {
        if record.watch_id == "boom" {
            panic!("a watch that panics");
        }
        served.lock().expect("served").push(record.watch_id.clone());
        Turn::Done
    });
    assert_eq!(*served.lock().expect("served"), ["fine"]);
}

/// One watch whose read keeps failing is retried first by store order; it must
/// not keep the repository's other watches from ever being read.
#[test]
fn a_watch_that_keeps_failing_does_not_starve_its_siblings() {
    let passes = Passes::default();
    let open = vec![
        watch("bad", "widgets"),
        watch("b", "widgets"),
        watch("c", "widgets"),
    ];
    let served = Mutex::new(Vec::new());
    let read = |record: &CiWatchRecord, _budget: &Budget| {
        served.lock().expect("served").push(record.watch_id.clone());
        if record.watch_id == "bad" {
            Turn::RepoFailed
        } else {
            Turn::Done
        }
    };

    passes.run(open.clone(), limits(40), &read);
    assert_eq!(
        *served.lock().expect("served"),
        ["bad"],
        "its turn ends the pass"
    );

    served.lock().expect("served").clear();
    passes.run(open, limits(40), &read);
    let second = served.lock().expect("served").clone();
    assert_eq!(
        second,
        ["b", "c", "bad"],
        "the siblings it held back are read first, the failing watch last"
    );
}

#[test]
fn pages_read_after_the_first_are_paid_for_afterwards() {
    let passes = Passes::default();
    let open = vec![watch("paged", "widgets"), watch("next", "widgets")];
    let served = Mutex::new(Vec::new());
    passes.run(open, limits(3), &|record, budget| {
        if !budget.take(1) {
            return Turn::NoBudget;
        }
        served.lock().expect("served").push(record.watch_id.clone());
        if record.watch_id == "paged" {
            budget.spend(2);
        }
        Turn::Done
    });
    assert_eq!(
        *served.lock().expect("served"),
        ["paged"],
        "three pages used the whole budget of three"
    );
}
