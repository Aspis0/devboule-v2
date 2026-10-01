//! The poll against what happens around it: a tick stalled on the catalog
//! must not pin the window's stop roads, a switch committing during the
//! round trip must not publish, and a kill leaves nothing that polls again.

use super::test_support::{
    broker, context_usages, drain, harness, node_skip, prime, release_hold, stage_hold,
    stage_stats, stats_requests, wait_held,
};
use serde_json::json;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::session::SessionKiller;

#[test]
fn a_tick_stalled_on_the_catalog_does_not_block_the_window_stop() {
    if node_skip() {
        eprintln!("node is required for the pi usage-poller tests");
        return;
    }
    let broker = broker();
    let harness = harness(&broker);
    prime(&harness);
    stage_stats(
        &harness,
        json!({"contextUsage": {"tokens": 1000, "contextWindow": 4000}}),
    );
    harness.poller.run_opened();
    harness.poller.arm_next_poll_now_for_test();
    // The catalog is the test's to hold: a due tick blocks inside its
    // model read, before any request can go out.
    let catalog = harness.catalog.lock().expect("catalog");
    let ticking = Arc::clone(&harness.poller);
    let tick = std::thread::spawn(move || ticking.tick_for_test());
    // A tick that held the window across its catalog read would park
    // holding it: would-block proves it parked, and the stop below is then
    // blocked until the catalog frees. A tick that holds nothing leaves
    // the lock free and the bound elapses — the passing shape.
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline && harness.poller.window.try_lock().is_ok() {
        std::thread::sleep(Duration::from_millis(2));
    }

    // The bound only covers scheduling: a stop blocked behind the tick
    // waits until the catalog below is freed, past any bound.
    let stopping = Arc::clone(&harness.poller);
    let (done, done_rx) = std::sync::mpsc::channel();
    let stopper = std::thread::spawn(move || {
        stopping.stop_window();
        let _ = done.send(());
    });
    done_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("stop_window to return while a tick is stalled on the catalog");
    drop(catalog);
    tick.join()
        .expect("the tick finishes once the catalog frees");
    stopper.join().expect("the stop thread ends");
    assert!(
        !harness.poller.window_open_for_test(),
        "the stop closed the window"
    );
    // Exactly one request: the stalled tick sent none, and the counter can
    // count — a missing request log reads as 0 and fails here.
    harness.poller.run_opened();
    harness.poller.arm_next_poll_now_for_test();
    harness.poller.tick_for_test();
    assert_eq!(
        stats_requests(&harness),
        1,
        "the poller still polls after the incident"
    );
}

#[test]
fn a_switch_during_the_round_trip_publishes_nothing() {
    if node_skip() {
        eprintln!("node is required for the pi usage-poller tests");
        return;
    }
    let broker = broker();
    let harness = harness(&broker);
    prime(&harness);
    stage_stats(
        &harness,
        json!({"contextUsage": {"tokens": 1000, "contextWindow": 4000}}),
    );
    stage_hold(&harness);
    harness.poller.run_opened();
    harness.poller.arm_next_poll_now_for_test();
    // One tick, on its own thread, held inside the round trip.
    let ticking = Arc::clone(&harness.poller);
    let tick = std::thread::spawn(move || ticking.tick_for_test());
    wait_held(&harness);
    // The switch commits while the reply is held: what lands is the old
    // model's count.
    if let Ok(mut catalog) = harness.catalog.lock() {
        catalog.current_model_id = Some("other".to_string());
    }
    release_hold(&harness);
    tick.join().expect("the tick finishes once the reply lands");
    let quiet = drain(&harness.conn);
    assert!(
        context_usages(&quiet).is_empty(),
        "a reading the switch outdates publishes nowhere: {quiet:?}"
    );
}

#[test]
fn a_killed_poller_never_polls_again() {
    if node_skip() {
        eprintln!("node is required for the pi usage-poller tests");
        return;
    }
    let mut harness = harness(&broker());
    harness.poller.run_opened();
    assert!(
        harness.poller.window_open_for_test(),
        "the run opens the window"
    );
    harness.killer.kill();
    assert!(
        !harness.poller.window_open_for_test(),
        "the kill closes the window"
    );
    // A closed window is not enough: a later agent_start must reopen
    // nothing.
    harness.poller.run_opened();
    assert!(
        !harness.poller.window_open_for_test(),
        "a killed poller never polls again"
    );
}
