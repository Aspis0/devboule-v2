//! The pi watchdog's clock: silence expiry, what feeds it, and the holds
//! that stop it. Every test arms the watch the way a send arms it — a
//! prompt through the writer — and fakes time with the watch's
//! deterministic hooks, never slept out. The ends the expiry shares with
//! the reader (a late `turn_end`, EOF, a rejected prompt) live in
//! `ends_tests`.

use super::test_support::{
    abort_frame, attached, confirm_card, deliver, drain, feed_line, finish, harness, is_any_finish,
    is_error_finish, is_watchdog_error, tool_execution_end, toolcall_start, touch,
};
use crate::session::permission_broker::PermissionBroker;
use devboule_protocol::{AgentActivityState, SessionEvent};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

fn broker() -> Arc<PermissionBroker> {
    PermissionBroker::for_test(Arc::new(|_, _| Ok(())))
}

#[test]
fn a_silent_turn_is_ended_by_the_watchdog_and_aborted_on_the_wire() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let broker = broker();
    let mut harness = harness(&broker);
    let (runtime, conn) = attached(&broker);
    runtime.begin_turn();
    deliver(&mut harness, "hello");
    feed_line(&mut harness, &runtime, touch());
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
    assert!(
        events.iter().any(is_error_finish),
        "the run finishes failed, through the settled finish path: {:?}",
        events.iter().map(|event| event.kind()).collect::<Vec<_>>()
    );
    assert_eq!(runtime.activity(), AgentActivityState::Idle);
    let abort = abort_frame(&mut harness);
    assert_eq!(
        abort.get("type").and_then(Value::as_str),
        Some("abort"),
        "the expiry asks pi to stop the abandoned turn: {abort}"
    );
    finish(&mut harness, &runtime);
}

#[test]
fn activity_keeps_the_watchdog_quiet() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let broker = broker();
    let mut harness = harness(&broker);
    let (runtime, conn) = attached(&broker);
    runtime.begin_turn();
    deliver(&mut harness, "hello");
    feed_line(&mut harness, &runtime, touch());
    let _ = drain(&conn);
    // Fresh activity, however stale it felt a moment ago: the tick finds
    // nothing owed and the run lives.
    harness.watch.tick_for_test();
    let alive = drain(&conn);
    assert!(
        !alive.iter().any(is_watchdog_error) && !alive.iter().any(is_any_finish),
        "a turn the provider is still talking about does not expire: {alive:?}"
    );
    // A full bound of real silence later, it does.
    harness
        .watch
        .backdate_activity_for_test(Duration::from_secs(1));
    harness.watch.tick_for_test();
    let events = drain(&conn);
    assert!(
        events.iter().any(is_watchdog_error) && events.iter().any(is_error_finish),
        "silence past the bound ends the run: {events:?}"
    );
    finish(&mut harness, &runtime);
}

#[test]
fn a_pending_card_holds_the_clock() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let broker = broker();
    let mut harness = harness(&broker);
    let (runtime, conn) = attached(&broker);
    runtime.begin_turn();
    deliver(&mut harness, "rm -rf /");
    feed_line(&mut harness, &runtime, confirm_card("perm-1"));
    let carded = drain(&conn);
    assert!(
        carded
            .iter()
            .any(|event| matches!(event, SessionEvent::PermissionRequest { .. })),
        "the card went up: {:?}",
        carded.iter().map(|event| event.kind()).collect::<Vec<_>>()
    );
    // The card sits longer than the bound: the hold re-stamps every tick,
    // so no expiry fires for silence from before the card opened.
    harness
        .watch
        .backdate_activity_for_test(Duration::from_secs(600));
    harness.watch.tick_for_test();
    let held = drain(&conn);
    assert!(
        !held.iter().any(is_watchdog_error) && !held.iter().any(is_any_finish),
        "a pending card stops the clock; the turn stays open: {held:?}"
    );
    // The answer resolves the card with no inbound line, and the clock
    // restarts from the release — the held ticks re-stamped it.
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
fn an_open_tool_call_inside_its_grace_holds_the_clock() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let broker = broker();
    let mut harness = harness(&broker);
    let (runtime, conn) = attached(&broker);
    runtime.begin_turn();
    deliver(&mut harness, "build it");
    feed_line(&mut harness, &runtime, toolcall_start("tool-1"));
    let _ = drain(&conn);
    // Freshly opened against the production grace: the provider working,
    // not silent.
    harness
        .watch
        .backdate_activity_for_test(Duration::from_secs(600));
    harness.watch.tick_for_test();
    let held = drain(&conn);
    assert!(
        !held.iter().any(is_watchdog_error) && !held.iter().any(is_any_finish),
        "an unanswered tool call inside its grace is the provider working: {held:?}"
    );
    // The execution end closes the hold; a full bound of silence after it
    // expires.
    feed_line(&mut harness, &runtime, tool_execution_end("tool-1"));
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
fn an_orphaned_tool_call_expires_past_the_grace() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let broker = broker();
    let mut harness = harness(&broker);
    let (runtime, conn) = attached(&broker);
    runtime.begin_turn();
    deliver(&mut harness, "build it");
    feed_line(&mut harness, &runtime, toolcall_start("tool-1"));
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
fn a_turn_end_in_time_finishes_once_and_disarms_the_watch() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let broker = broker();
    let mut harness = harness(&broker);
    let (runtime, conn) = attached(&broker);
    runtime.begin_turn();
    deliver(&mut harness, "quick");
    feed_line(&mut harness, &runtime, recorded_turn_end());
    let events = drain(&conn);
    assert_eq!(
        events.iter().filter(|event| is_any_finish(event)).count(),
        1,
        "exactly one finish — the turn's own: {:?}",
        events.iter().map(|event| event.kind()).collect::<Vec<_>>()
    );
    assert!(
        !events
            .iter()
            .any(|event| matches!(event, SessionEvent::AgentError { .. })),
        "a turn that ended in time publishes no silence error: {events:?}"
    );
    assert_eq!(runtime.activity(), AgentActivityState::Idle);
    // The turn is over and the watch with it: silence alone ends nothing.
    harness
        .watch
        .backdate_activity_for_test(Duration::from_secs(600));
    harness.watch.tick_for_test();
    let after = drain(&conn);
    assert!(
        !after.iter().any(is_any_finish) && !after.iter().any(is_watchdog_error),
        "an ended turn is not expired again: {after:?}"
    );
    finish(&mut harness, &runtime);
}

/// The recorded `turn_end` of `pi_view.rs`'s own fixture.
fn recorded_turn_end() -> Value {
    serde_json::from_str(
        r#"{"type":"turn_end","message":{"role":"assistant","content":[{"type":"text","text":"OK"}],"api":"openai-completions","provider":"openrouter","model":"z-ai/glm-5.3-flash","usage":{"input":25848,"output":3,"cacheRead":0,"cacheWrite":0,"reasoning":0,"totalTokens":25851},"stopReason":"stop"},"toolResults":[]}"#,
    )
    .expect("recorded turn_end frame")
}
