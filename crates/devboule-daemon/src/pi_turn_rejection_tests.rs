//! The roads that share one run's finish when a prompt goes wrong: a
//! rejection answers only the prompt we wrote and still own; the aborted
//! turn's end answers the interrupt that caused it, never the replacement;
//! and a turn pi started by itself is begun and watched like any other.

use super::super::local_command_test_support::{agent_start, recorded_turn_end};
use super::test_support::{
    abort_frame, attached, broker, deliver, drain, feed_line, finish, finishes_of, harness,
    rejects, touch, turn_end_with,
};
use crate::session::SessionKiller;
use crate::test_support::{is_error_finish, is_watchdog_error};
use devboule_protocol::{AgentActivityState, SessionEvent};
use std::time::Duration;

/// The review's `/help`-mid-turn shape: the steer road refuses a slash
/// input, the interrupt goes out, and the same text is delivered as a plain
/// prompt pi refuses in its preflight — "Agent is already processing". The
/// refusal shows its error and ends nothing: the streaming turn keeps
/// running, and pi's own end for the interrupted turn finishes the run,
/// exactly once.
#[test]
fn a_rejection_mid_turn_shows_its_error_and_leaves_pi_s_turn_its_finish() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let broker = broker();
    let mut harness = harness(&broker);
    let (runtime, conn) = attached(&broker);
    runtime.begin_turn();
    deliver(&mut harness, "hello");
    feed_line(
        &mut harness,
        &runtime,
        serde_json::json!({"type":"response","id":"p-1","command":"prompt","success":true}),
    );
    feed_line(&mut harness, &runtime, agent_start());
    let _ = drain(&conn);
    // The steer road refused `/help`; the interrupt-and-replace went out.
    harness.killer.interrupt();
    let _ = abort_frame(&mut harness);
    deliver(&mut harness, "/help");
    // The abort frame took an id from the same counter (a-2), so the
    // replacement prompt went out as p-3.
    feed_line(
        &mut harness,
        &runtime,
        rejects(
            "p-3",
            "Agent is already processing. Specify streamingBehavior to queue it.",
        ),
    );
    let after = drain(&conn);
    assert!(
        after.iter().any(|event| matches!(
            event,
            SessionEvent::AgentError { message } if message.contains("Agent is already processing")
        )),
        "the refusal shows pi's own error text: {after:?}"
    );
    assert_eq!(
        finishes_of(&after),
        Vec::<String>::new(),
        "a mid-turn rejection ends nothing: {after:?}"
    );
    assert!(
        matches!(runtime.activity(), AgentActivityState::Working),
        "the streaming turn is still working, got {:?}",
        runtime.activity()
    );
    // pi's own end for the interrupted turn finishes the run, once.
    feed_line(&mut harness, &runtime, turn_end_with("aborted"));
    let events = drain(&conn);
    assert_eq!(
        finishes_of(&events),
        ["aborted"],
        "one finish, pi's own: {events:?}"
    );
    finish(&mut harness, &runtime);
}

/// A rejection answers the prompt we wrote and have not seen answered: a
/// stale or foreign id — an earlier prompt's late refusal, an echo, a
/// duplicate — is ignored for the run and logged, while the current
/// prompt's own refusal still ends an idle run.
#[test]
fn a_rejection_for_an_untracked_prompt_id_answers_no_run() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let broker = broker();
    let mut harness = harness(&broker);
    let (runtime, conn) = attached(&broker);
    runtime.begin_turn();
    deliver(&mut harness, "first");
    feed_line(
        &mut harness,
        &runtime,
        serde_json::json!({"type":"response","id":"p-1","command":"prompt","success":true}),
    );
    deliver(&mut harness, "second");
    let _ = drain(&conn);
    // p-1's refusal arrives late: not the current prompt, answers nothing.
    feed_line(&mut harness, &runtime, rejects("p-1", "No model selected"));
    let stale = drain(&conn);
    assert!(
        !stale
            .iter()
            .any(|event| matches!(event, SessionEvent::AgentError { .. })),
        "a stale id publishes no error: {stale:?}"
    );
    assert_eq!(
        finishes_of(&stale),
        Vec::<String>::new(),
        "a stale id finishes nothing: {stale:?}"
    );
    // The current prompt's own refusal ends the run, once.
    feed_line(&mut harness, &runtime, rejects("p-2", "No model selected"));
    let events = drain(&conn);
    assert!(
        events.iter().any(|event| matches!(
            event,
            SessionEvent::AgentError { message } if message.contains("No model selected")
        )),
        "the current refusal shows pi's text: {events:?}"
    );
    assert_eq!(
        finishes_of(&events),
        ["error"],
        "one error finish: {events:?}"
    );
    finish(&mut harness, &runtime);
}

/// The interrupt-and-replace flow: pi's aborted end for the interrupted
/// turn arrives after the replacement was delivered — it finishes nothing,
/// and the replacement keeps both its run and its watchdog.
#[test]
fn an_aborted_end_after_a_replacement_delivered_finishes_nothing() {
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
    harness.killer.interrupt();
    let _ = abort_frame(&mut harness);
    deliver(&mut harness, "again");
    feed_line(&mut harness, &runtime, turn_end_with("aborted"));
    let aborted = drain(&conn);
    assert_eq!(
        finishes_of(&aborted),
        Vec::<String>::new(),
        "the aborted turn's end finishes nothing: {aborted:?}"
    );
    assert!(
        !aborted.iter().any(is_watchdog_error),
        "the stale end publishes no silence error either: {aborted:?}"
    );
    // The replacement's own end finishes the run, once.
    feed_line(&mut harness, &runtime, recorded_turn_end());
    let events = drain(&conn);
    assert_eq!(
        finishes_of(&events),
        ["stop"],
        "the replacement finishes once, on its own end: {events:?}"
    );
    finish(&mut harness, &runtime);
}

/// The clock never stopped for the withheld end: the replacement turn the
/// abort gate preserved is one the watchdog is still timing.
#[test]
fn the_watchdog_is_still_armed_after_a_withheld_aborted_end() {
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
    harness.killer.interrupt();
    let _ = abort_frame(&mut harness);
    deliver(&mut harness, "again");
    feed_line(&mut harness, &runtime, turn_end_with("aborted"));
    let _ = drain(&conn);
    // A full bound of silence on the replacement still expires.
    harness
        .watch
        .backdate_activity_for_test(Duration::from_secs(1));
    harness.watch.tick_for_test();
    let events = drain(&conn);
    assert!(
        events.iter().any(is_watchdog_error) && events.iter().any(is_error_finish),
        "the replacement turn is still watched: {events:?}"
    );
    finish(&mut harness, &runtime);
}

/// The expiry's expectation must not outlive its own turn: when pi never
/// answers the abort, the next prompt's own end still finishes the run.
#[test]
fn an_expiry_whose_late_end_never_comes_leaves_the_next_turn_alone() {
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
    let expiry = drain(&conn);
    assert!(
        expiry.iter().any(is_watchdog_error) && expiry.iter().any(is_error_finish),
        "the expiry ran: {expiry:?}"
    );
    // pi never sends the aborted end. The next prompt's turn runs and ends.
    runtime.begin_turn();
    deliver(&mut harness, "again");
    feed_line(&mut harness, &runtime, recorded_turn_end());
    let events = drain(&conn);
    assert_eq!(
        finishes_of(&events),
        ["stop"],
        "the next turn's own end finishes it, once: {events:?}"
    );
    finish(&mut harness, &runtime);
}

/// A turn pi starts with no prompt from us — an extension's sendMessage or
/// a boundary continuation — is begun and finished like any other.
#[test]
fn an_extension_turn_is_begun_and_finished() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let broker = broker();
    let mut harness = harness(&broker);
    let (runtime, conn) = attached(&broker);
    feed_line(&mut harness, &runtime, agent_start());
    assert!(
        matches!(runtime.activity(), AgentActivityState::Working),
        "a turn pi started by itself begins the run, got {:?}",
        runtime.activity()
    );
    feed_line(&mut harness, &runtime, recorded_turn_end());
    let events = drain(&conn);
    assert_eq!(
        finishes_of(&events),
        ["stop"],
        "pi's own turn finishes once: {events:?}"
    );
    assert!(matches!(runtime.activity(), AgentActivityState::Idle));
    finish(&mut harness, &runtime);
}

#[test]
fn an_extension_turns_silence_is_watched() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let broker = broker();
    let mut harness = harness(&broker);
    let (runtime, conn) = attached(&broker);
    feed_line(&mut harness, &runtime, agent_start());
    let _ = drain(&conn);
    harness
        .watch
        .backdate_activity_for_test(Duration::from_secs(1));
    harness.watch.tick_for_test();
    let events = drain(&conn);
    assert!(
        events.iter().any(is_watchdog_error) && events.iter().any(is_error_finish),
        "a turn pi started by itself is watched: {events:?}"
    );
    finish(&mut harness, &runtime);
}
