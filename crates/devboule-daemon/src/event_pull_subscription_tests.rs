//! The subscription table: subscription identity, the per-connection cap, a
//! poisoned stream, and the exit wake.

use super::super::*;
use super::*;

use super::test_support::{attach_tracked, drain};

#[test]
fn delivered_exit_removes_the_runtime_observer() {
    let runtime = Arc::new(SessionRuntime::new());
    let conn = ConnHandle::new(1);
    attach_tracked(&runtime, &conn);

    runtime.finish(Some(0));
    let events = drain(&conn);

    assert!(events
        .iter()
        .any(|event| matches!(event, SessionEvent::Exit { .. })));
    assert!(runtime.stream.lock().unwrap().observers.is_empty());
}

#[test]
fn duplicate_subscription_id_does_not_replace_another_session() {
    let conn = ConnHandle::new(1);
    let first = Arc::new(SessionRuntime::new());
    let second = Arc::new(SessionRuntime::new());

    conn.track_with_subscription(7, Arc::clone(&first), false, None, 1, None)
        .expect("first subscription");
    let error = conn
        .track_with_subscription(7, Arc::clone(&second), false, None, 1, None)
        .expect_err("duplicate subscription id must be rejected");
    assert_eq!(error.code, ErrorCode::InvalidRequest);

    let attached = conn.attached.lock().unwrap();
    assert!(Arc::ptr_eq(
        &attached.get(&7).expect("subscription").runtime,
        &first
    ));
}

#[test]
fn a_connection_holds_at_most_sixty_four_subscriptions() {
    // §8 R4's per-connection brake. A peer that attaches the same session
    // sixty-five times is either broken or probing; either way the
    // sixty-fifth is refused rather than booked.
    let conn = ConnHandle::new(1);
    let runtime = Arc::new(SessionRuntime::new());
    for subscription_id in 1..=MAX_SUBSCRIPTIONS as u64 {
        conn.track_with_subscription(subscription_id, Arc::clone(&runtime), false, None, 1, None)
            .expect("a subscription inside the cap is accepted");
    }
    let error = conn
        .track_with_subscription(
            MAX_SUBSCRIPTIONS as u64 + 1,
            Arc::clone(&runtime),
            false,
            None,
            1,
            None,
        )
        .expect_err("the sixty-fifth subscription on one connection is refused");
    assert_eq!(error.code, ErrorCode::CapabilityNotSupported);
    assert_eq!(
        conn.attached.lock().unwrap().len(),
        MAX_SUBSCRIPTIONS,
        "the refused subscription must not be booked"
    );
}

#[test]
fn poisoned_stream_is_dead_and_not_reused() {
    let runtime = Arc::new(SessionRuntime::new());
    let conn = ConnHandle::new(1);
    attach_tracked(&runtime, &conn);
    let poisoned = Arc::clone(&runtime);
    let panic = std::thread::spawn(move || {
        let _stream = poisoned.stream.lock().expect("stream lock");
        panic!("simulate a terminal-state panic");
    });
    assert!(panic.join().is_err());

    runtime.publish_output("must not be applied");
    let events = drain(&conn);
    assert!(runtime.terminal_dead.load(Ordering::Acquire));
    assert!(matches!(
        events.as_slice(),
        [
            SessionEvent::JournalDegraded {
                dropped_frames: 0,
                dropped_bytes: 0,
            },
            SessionEvent::Exit { code: None }
        ]
    ));
    assert!(matches!(
        runtime.try_attach_with_replay(None, &conn, false),
        Err(error) if error == process_gone()
    ));
}

#[test]
fn next_exit_wake_is_zero_once_the_drain_has_elapsed() {
    let runtime = Arc::new(SessionRuntime::new());
    let conn = ConnHandle::new(1);
    let outcome = runtime.try_attach_with_replay(None, &conn, false).unwrap();
    conn.track_with_agent_replay(
        "s.a.1",
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    assert_eq!(conn.next_exit_wake(), None);
    runtime.mark_exited(Some(0));
    let wake = conn.next_exit_wake().expect("drain timer");
    assert!(wake <= EXIT_DRAIN);
    std::thread::sleep(EXIT_DRAIN + Duration::from_millis(10));
    assert_eq!(conn.next_exit_wake(), Some(Duration::ZERO));
    let events = conn.pull_events();
    assert!(
        events
            .iter()
            .any(|envelope| matches!(envelope.envelope.event, SessionEvent::Exit { .. })),
        "zero wake must let the writer emit Exit, got {events:?}"
    );
    for event in &events {
        conn.event_sent(event);
    }
}
