//! Tests for one topic: the Claude turn watchdog's holds — silence expiry,
//! pending cards, and the grace-bounded tool hold. Every test arms the watch
//! the way spawn does, through the shared harness, and fakes time with the
//! watch's deterministic hooks, never slept out. Suppression, replay and
//! killer tests live in `claude_turn_watch_suppression_tests.rs`.

use super::turn_watch_test_support::{
    attached, deliver, drain, feed_line, finish, harness, tool_frame, touch,
};
use super::*;
use crate::session::turn_watch::watchdog_message;
use crate::test_support::{is_any_finish, is_error_finish, is_watchdog_error};
use devboule_protocol::{AgentActivityState, SessionEvent};
use std::time::Duration;

#[test]
fn a_silent_turn_is_ended_by_the_watchdog() {
    let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let mut harness = harness(&broker, false);
    let (runtime, conn) = attached(&broker);
    runtime.begin_turn();
    deliver(&mut harness, "hello");
    touch(&mut harness, &runtime);
    let _ = drain(&conn);
    harness
        .watch
        .backdate_activity_for_test(Duration::from_secs(60));
    harness.watch.tick_for_test();
    let events = drain(&conn);
    assert!(
        events.iter().any(is_watchdog_error),
        "the silence is announced as an error notice: {:?}",
        events.iter().map(|event| event.kind()).collect::<Vec<_>>()
    );
    let message = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::AgentError { message } if is_watchdog_error(event) => Some(message),
            _ => None,
        })
        .expect("the watchdog notice the predicate matched");
    assert_eq!(
        message,
        &watchdog_message("Claude", Duration::from_millis(300)),
        "the notice is the shared production sentence"
    );
    assert!(
        events.iter().any(is_error_finish),
        "the run finishes failed, through the settled finish path: {:?}",
        events.iter().map(|event| event.kind()).collect::<Vec<_>>()
    );
    assert_eq!(
        runtime.activity(),
        AgentActivityState::Idle,
        "activity returns to idle once the watchdog ends the turn"
    );
    finish(&mut harness, &runtime);
}

#[test]
fn an_answered_card_restarts_the_silence_clock() {
    let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let mut harness = harness(&broker, false);
    let (runtime, conn) = attached(&broker);
    runtime.begin_turn();
    deliver(&mut harness, "rm -rf /");
    feed_line(
        &mut harness.reader,
        &runtime,
        serde_json::json!({
            "type": "control_request",
            "request_id": "e73c118e-6742-481e-b60a-e8486a9bde4e",
            "request": {
                "subtype": "can_use_tool",
                "tool_name": "Bash",
                "display_name": "Bash",
                "input": {"command": "sleep 45", "description": "A long command"},
                "tool_use_id": "toolu_01FgWLJkmeyU9wAGYkx3YFXu",
            }
        }),
    );
    let _ = drain(&conn);
    // The card sits longer than the bound: the hold re-stamps every tick, so
    // no expiry fires for silence from before the card opened.
    harness
        .watch
        .backdate_activity_for_test(Duration::from_secs(600));
    harness.watch.tick_for_test();
    let held = drain(&conn);
    assert!(
        !held.iter().any(is_watchdog_error) && !held.iter().any(is_any_finish),
        "a pending card stops the clock; the turn stays open: {held:?}"
    );
    // The answer resolves the last card with no inbound line — the reader's
    // activity hook never runs — so only the hold's re-stamp stands between
    // the release and the next tick.
    harness.reader.permission_broker.cancel_pending();
    harness.watch.tick_for_test();
    let released = drain(&conn);
    assert!(
        !released.iter().any(|event| matches!(
            event,
            SessionEvent::AgentError { .. } | SessionEvent::AgentFinished { .. }
        )),
        "answering restarts the clock instead of ending the run: {released:?}"
    );
    // A full bound of silence AFTER the answer is still an expiry.
    harness
        .watch
        .backdate_activity_for_test(Duration::from_secs(1));
    harness.watch.tick_for_test();
    let events = drain(&conn);
    assert!(
        events.iter().any(is_watchdog_error) && events.iter().any(is_error_finish),
        "silence past the bound after the answer ends the run: {events:?}"
    );
    finish(&mut harness, &runtime);
}

#[test]
fn an_orphaned_tool_call_expires_past_the_grace() {
    let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let mut harness = harness(&broker, false);
    let (runtime, conn) = attached(&broker);
    runtime.begin_turn();
    deliver(&mut harness, "build it");
    feed_line(
        &mut harness.reader,
        &runtime,
        tool_frame(None, "toolu_orphan"),
    );
    let _ = drain(&conn);
    // The call opened and was never answered: shrink the grace and age the
    // start past it, and the hold lapses back into ordinary silence.
    harness
        .watch
        .set_tool_grace_for_test(Duration::from_secs(5));
    harness
        .watch
        .backdate_tool_start_for_test(Duration::from_secs(60));
    harness
        .watch
        .backdate_activity_for_test(Duration::from_secs(600));
    harness.watch.tick_for_test();
    let events = drain(&conn);
    assert!(
        events.iter().any(is_watchdog_error) && events.iter().any(is_error_finish),
        "an orphaned call only postpones the expiry: {events:?}"
    );
    finish(&mut harness, &runtime);
}

#[test]
fn a_sidechain_tool_call_expires_past_the_grace_like_any_call() {
    let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let mut harness = harness(&broker, false);
    let (runtime, conn) = attached(&broker);
    runtime.begin_turn();
    deliver(&mut harness, "run the subagent");
    // An inner call of a sidechain turn: it opens and stamps like any
    // `tool_use`, and the same grace bounds it — inner frames hold nothing
    // indefinitely.
    feed_line(
        &mut harness.reader,
        &runtime,
        tool_frame(Some("toolu_side_outer"), "toolu_side_inner"),
    );
    let _ = drain(&conn);
    harness
        .watch
        .set_tool_grace_for_test(Duration::from_secs(5));
    harness
        .watch
        .backdate_tool_start_for_test(Duration::from_secs(60));
    harness
        .watch
        .backdate_activity_for_test(Duration::from_secs(600));
    harness.watch.tick_for_test();
    let events = drain(&conn);
    assert!(
        events.iter().any(is_watchdog_error) && events.iter().any(is_error_finish),
        "a sidechain call holds nothing past the grace: {events:?}"
    );
    finish(&mut harness, &runtime);
}

#[test]
fn a_tool_in_flight_inside_the_grace_holds_the_watchdog_off() {
    let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let mut harness = harness(&broker, false);
    let (runtime, conn) = attached(&broker);
    runtime.begin_turn();
    deliver(&mut harness, "build it");
    feed_line(
        &mut harness.reader,
        &runtime,
        tool_frame(None, "toolu_grace"),
    );
    let _ = drain(&conn);
    // Freshly opened against the production grace: working, not silent.
    harness
        .watch
        .backdate_activity_for_test(Duration::from_secs(600));
    harness.watch.tick_for_test();
    let held = drain(&conn);
    assert!(
        !held.iter().any(is_watchdog_error) && !held.iter().any(is_any_finish),
        "an unanswered tool call inside its grace is the provider working: {held:?}"
    );
    // The result closes the call; a full bound of silence after it expires.
    feed_line(
        &mut harness.reader,
        &runtime,
        serde_json::json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [{"type": "tool_result", "tool_use_id": "toolu_grace", "content": "done"}],
            },
        }),
    );
    harness
        .watch
        .backdate_activity_for_test(Duration::from_secs(1));
    harness.watch.tick_for_test();
    let events = drain(&conn);
    assert!(
        events.iter().any(is_watchdog_error) && events.iter().any(is_error_finish),
        "pure silence past the bound still ends the run: {events:?}"
    );
    finish(&mut harness, &runtime);
}

#[test]
fn a_result_that_arrives_in_time_keeps_the_turn_its_own() {
    let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let mut harness = harness(&broker, false);
    let (runtime, conn) = attached(&broker);
    runtime.begin_turn();
    deliver(&mut harness, "quick");
    feed_line(
        &mut harness.reader,
        &runtime,
        serde_json::json!({
            "type": "result",
            "subtype": "success",
            "stop_reason": "end_turn",
        }),
    );
    harness
        .watch
        .backdate_activity_for_test(Duration::from_secs(60));
    harness.watch.tick_for_test();
    let events = drain(&conn);
    assert_eq!(
        events.iter().filter(|event| is_any_finish(event)).count(),
        1,
        "exactly one finish — the result's, not the watchdog's: {events:?}"
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SessionEvent::AgentError { .. })),
        "a turn that ended in time publishes no silence error: {events:?}"
    );
    assert_eq!(runtime.activity(), AgentActivityState::Idle);
    finish(&mut harness, &runtime);
}
