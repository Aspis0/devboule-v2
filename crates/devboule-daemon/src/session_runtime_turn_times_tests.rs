//! The roster's turn times: a running turn has a start and no end, and a
//! finished one has an end and no start.

use super::{SessionRuntime, TurnTimes};

#[test]
fn an_idle_session_has_no_turn_times() {
    assert_eq!(SessionRuntime::new().turn_times(), TurnTimes::default());
}

#[test]
fn a_running_turn_has_a_start_and_no_end_until_it_finishes() {
    let runtime = SessionRuntime::new();
    runtime.begin_turn();
    let running = runtime.turn_times();
    assert!(running.started_at_ms.is_some());
    assert_eq!(running.ended_at_ms, None);

    runtime.finish_turn();
    let finished = runtime.turn_times();
    assert_eq!(finished.started_at_ms, None);
    assert!(finished.ended_at_ms.is_some());
}

#[test]
fn a_second_begin_while_running_keeps_the_first_start() {
    let runtime = SessionRuntime::new();
    runtime.begin_turn();
    let first = runtime.turn_times().started_at_ms;
    runtime.begin_turn();
    assert_eq!(runtime.turn_times().started_at_ms, first);
}
