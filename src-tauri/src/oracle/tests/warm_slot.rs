//! The warm single-flight behind the `warming` refusal (review F-1): the
//! slot must reopen when its guard drops — at the normal end of a load, and
//! when a thread panics while holding it. A flag left set answers `warming`
//! to every later query for the life of the process, and no retry repairs
//! it, because `try_begin` would see the stale flag and never start a new
//! warm.

use std::panic::AssertUnwindSafe;
use std::thread;

use crate::oracle::endpoint_query::WarmSlot;

/// The normal path: one warm at a time, and the flag false again once the
/// guard drops.
#[test]
fn the_slot_reopens_when_the_guard_drops() {
    let slot = WarmSlot::new();
    let guard = WarmSlot::try_begin(&slot).expect("the first warm starts");
    assert!(
        WarmSlot::try_begin(&slot).is_none(),
        "a second warm must not start while one holds the slot"
    );
    drop(guard);
    assert!(
        WarmSlot::try_begin(&slot).is_some(),
        "the normal end of a warm must put the flag back to false"
    );
}

/// The pin: a thread that panics while holding the guard must leave the slot
/// free. `catch_unwind` keeps the panic inside the test; the guard's `Drop`
/// does the reset. (The panic line the harness prints during this test is
/// this caught one.)
#[test]
fn a_thread_that_panics_holding_the_guard_leaves_the_slot_free() {
    let slot = WarmSlot::new();
    let worker = slot.clone();
    let joined = thread::spawn(move || {
        let guard = WarmSlot::try_begin(&worker).expect("the warm starts");
        let caught = std::panic::catch_unwind(AssertUnwindSafe(move || {
            let _guard = guard; // still holding the slot when it fires
            panic!("the load panicked while holding the guard");
        }));
        assert!(
            caught.is_err(),
            "the panic must be caught here, not escape the thread"
        );
    });
    joined.join().expect("the worker thread");
    assert!(
        WarmSlot::try_begin(&slot).is_some(),
        "a panicking warm must not leave the flag set: try_begin has to succeed again"
    );
}
