//! Tests for the owed-release guard: exactly one release, carrying the
//! caller's answer when it is explicit and the incomplete answer on `Drop`.

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Counts releases and how many of them said the operation finished. Atomics
/// rather than a channel because one of these releases runs inside an unwind,
/// where a blocking send would be a hazard of its own.
#[derive(Default)]
struct Releases {
    count: AtomicUsize,
    completed: AtomicUsize,
}

impl Releases {
    fn record(&self, completed: bool) {
        self.count.fetch_add(1, Ordering::SeqCst);
        if completed {
            self.completed.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// `(releases, of which completed)`.
    fn seen(&self) -> (usize, usize) {
        (
            self.count.load(Ordering::SeqCst),
            self.completed.load(Ordering::SeqCst),
        )
    }
}

#[test]
fn an_explicit_release_runs_once_with_the_callers_answer() {
    let releases = Arc::new(Releases::default());
    let seen = Arc::clone(&releases);
    ReleaseGuard::armed(move |completed: bool| seen.record(completed)).release(true);
    assert_eq!(
        releases.seen(),
        (1, 1),
        "one release, and it carries the caller's answer"
    );
}

#[test]
fn a_drop_with_no_explicit_release_reports_the_operation_incomplete() {
    let releases = Arc::new(Releases::default());
    let seen = Arc::clone(&releases);
    {
        let _guard = ReleaseGuard::armed(move |completed: bool| seen.record(completed));
    }
    assert_eq!(
        releases.seen(),
        (1, 0),
        "one release, and an operation that never returned is not a finished one"
    );
}

/// The case the type exists for: the covered code panics between the arm and
/// the explicit release, and the reservation must come back anyway.
#[test]
fn a_panic_between_the_arm_and_the_release_still_releases() {
    let releases = Arc::new(Releases::default());
    let seen = Arc::clone(&releases);
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _guard = ReleaseGuard::armed(move |completed: bool| seen.record(completed));
        panic!("the covered operation panicked");
    }));
    assert!(outcome.is_err(), "the fixture must have panicked");
    assert_eq!(
        releases.seen(),
        (1, 0),
        "the reservation comes back on the way out"
    );
}
