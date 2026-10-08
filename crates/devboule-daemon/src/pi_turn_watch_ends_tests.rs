//! The ends that share one finish with the watchdog: the late `turn_end`
//! after an expiry — live and replayed — a reader EOF with the turn
//! running, and pi's own refusal of a prompt. Every run ends exactly once,
//! whichever road takes it.

use super::super::local_command_test_support::recorded_turn_end;
use super::test_support::turn_end_with;
use super::test_support::{
    attached, broker, deliver, drain, feed_line, finish, harness, is_eof_error, tool_execution_end,
    toolcall_start, touch, try_next_echo,
};
use crate::journal::{new_session_record, Journal};
use crate::session::SessionRuntime;
use crate::test_support::{is_any_finish, is_error_finish, is_watchdog_error};
use devboule_protocol::{SessionEvent, SessionKind};
use std::sync::Arc;
use std::time::Duration;

/// One inbound frame before the clock is faked: the priming feed every live
/// reader gets, which binds the runtime the expiry publishes through.
fn prime(harness: &mut super::test_support::PiWatchHarness, runtime: &Arc<SessionRuntime>) {
    feed_line(harness, runtime, touch());
}

#[test]
fn a_late_turn_end_after_the_expiry_finishes_nothing() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let broker = broker();
    let mut harness = harness(&broker);
    let (runtime, conn) = attached(&broker);
    runtime.begin_turn();
    deliver(&mut harness, "hello");
    prime(&mut harness, &runtime);
    let _ = drain(&conn);
    // The watchdog ends the silent turn...
    harness
        .watch
        .backdate_activity_for_test(Duration::from_secs(60));
    harness.watch.tick_for_test();
    let expiry = drain(&conn);
    assert!(
        expiry.iter().any(is_watchdog_error) && expiry.iter().any(is_error_finish),
        "the expiry ran: {expiry:?}"
    );
    // ...and pi's late `turn_end` for the abandoned turn arrives: it adds
    // no finish and no error of its own.
    feed_line(&mut harness, &runtime, recorded_turn_end());
    let late = drain(&conn);
    assert!(
        !late.iter().any(is_any_finish),
        "the late turn_end finishes nothing: {late:?}"
    );
    assert!(
        !late.iter().any(is_watchdog_error),
        "the late turn_end publishes no second silence error: {late:?}"
    );
    finish(&mut harness, &runtime);
}

/// The tool call and result are published before expiry; the late `turn_end`
/// publishes no finish, while its durable context reading reaches the connection.
/// Replay withholds the twin finish through the marker row the
/// suppression journals (`replay_shows_one_finish_when_the_watchdog_ended_the_run`).
#[test]
fn a_withheld_turn_end_drops_its_finish_and_keeps_its_durable_context() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let broker = broker();
    let mut harness = harness(&broker);
    let (runtime, conn) = attached(&broker);
    runtime.begin_turn();
    deliver(&mut harness, "build it");
    prime(&mut harness, &runtime);
    // The tool call opens and closes inside the turn; both events land.
    feed_line(&mut harness, &runtime, toolcall_start("tool-1"));
    feed_line(&mut harness, &runtime, tool_execution_end("tool-1"));
    let tool_events = drain(&conn);
    assert!(
        tool_events.iter().any(|event| matches!(
            event,
            SessionEvent::AgentToolCall { tool_call_id, status, .. }
                if tool_call_id == "tool-1" && status == "pending"
        )),
        "the tool call is published: {tool_events:?}"
    );
    assert!(
        tool_events.iter().any(|event| matches!(
            event,
            SessionEvent::AgentToolUpdate { tool_call_id, status: Some(status), .. }
                if tool_call_id == "tool-1" && status == "completed"
        )),
        "the tool result is published: {tool_events:?}"
    );
    // The watchdog ends the silent turn...
    harness
        .watch
        .backdate_activity_for_test(Duration::from_secs(60));
    harness.watch.tick_for_test();
    let expiry = drain(&conn);
    assert!(
        expiry.iter().any(is_watchdog_error) && expiry.iter().any(is_error_finish),
        "the expiry ran: {expiry:?}"
    );
    // ...and pi's late end for the abandoned turn arrives — the aborted
    // answer to the expiry's own abort, carrying the run's usage and cost.
    // The filter drops the finish whole; the same row's durable context
    // reading survives.
    feed_line(
        &mut harness,
        &runtime,
        serde_json::from_str::<serde_json::Value>(
            r#"{"type":"turn_end","message":{"role":"assistant","content":[],"model":"pi-test","usage":{"input":11,"output":4,"cacheRead":7,"cacheWrite":2,"reasoning":0,"totalTokens":42,"cost":{"input":0.01,"output":0.02,"cacheRead":0,"cacheWrite":0,"total":0.5}},"stopReason":"aborted"},"toolResults":[]}"#,
        )
        .expect("costed turn_end frame"),
    );
    let late = drain(&conn);
    assert!(
        !late.iter().any(is_any_finish),
        "the withheld end finishes nothing — its cost-bearing usage with it: {late:?}"
    );
    assert_eq!(
        late,
        vec![SessionEvent::ContextUsage {
            model_id: Some("pi-test".to_string()),
            used_tokens: 42,
            max_tokens: None,
            live: false,
        }],
        "the durable context reading from the withheld row still lands"
    );
    finish(&mut harness, &runtime);
}

#[test]
fn the_next_turns_own_turn_end_still_finishes_after_a_late_one() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let broker = broker();
    let mut harness = harness(&broker);
    let (runtime, conn) = attached(&broker);
    runtime.begin_turn();
    deliver(&mut harness, "hello");
    prime(&mut harness, &runtime);
    let _ = drain(&conn);
    harness
        .watch
        .backdate_activity_for_test(Duration::from_secs(60));
    harness.watch.tick_for_test();
    let _ = drain(&conn);
    // A new prompt arms the next turn before pi's late end arrives.
    runtime.begin_turn();
    deliver(&mut harness, "again");
    // The late end: pi's aborted answer to the expiry's own abort, and it
    // belongs to the abandoned turn, not to this one.
    feed_line(&mut harness, &runtime, turn_end_with("aborted"));
    let late = drain(&conn);
    assert!(
        !late.iter().any(is_any_finish),
        "the stale turn_end does not finish the new turn: {late:?}"
    );
    // The new turn's own end still finishes it, once.
    feed_line(&mut harness, &runtime, recorded_turn_end());
    let events = drain(&conn);
    assert_eq!(
        events.iter().filter(|event| is_any_finish(event)).count(),
        1,
        "the new turn ends once, on its own turn_end: {events:?}"
    );
    finish(&mut harness, &runtime);
}

#[test]
fn replay_shows_one_finish_when_the_watchdog_ended_the_run() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let (dir, path) = crate::journal::tmp_journal();
    let journal = Arc::new(Journal::open(&path).expect("open"));
    let session_id = "s.pi.watch.replay";
    journal
        .create_session(new_session_record(
            session_id,
            "owner",
            None,
            SessionKind::Pi,
            "pi watch",
        ))
        .expect("birth");
    let runtime = Arc::new(SessionRuntime::with_journal(
        session_id.to_string(),
        Some(Arc::clone(&journal)),
    ));
    runtime.set_agent_kind(SessionKind::Pi);
    let broker = broker();
    let mut harness = harness(&broker);
    runtime.begin_turn();
    deliver(&mut harness, "hello");
    prime(&mut harness, &runtime);
    harness
        .watch
        .backdate_activity_for_test(Duration::from_secs(60));
    harness.watch.tick_for_test();
    // pi's late `turn_end` for the abandoned turn: suppressed live, and
    // journalled beside the marker replay reads.
    feed_line(&mut harness, &runtime, recorded_turn_end());
    journal.flush().expect("flush");
    drop(harness);
    drop(runtime);
    journal.shutdown();

    let replay = Journal::open(&path)
        .expect("reopen")
        .replay(session_id)
        .expect("replay");
    let finishes = replay
        .events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::AgentFinished { stop_reason, .. } => Some(stop_reason.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        finishes,
        ["error"],
        "the restart's replay shows the watchdog's one finish, not the late \
         turn_end's second one: {finishes:?}"
    );
    assert!(
        replay
            .events
            .iter()
            .any(|event| matches!(event, SessionEvent::AgentError { .. })),
        "the replay carries the silence error too"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_eof_with_the_turn_running_finishes_exactly_once() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let broker = broker();
    let mut harness = harness(&broker);
    let (runtime, conn) = attached(&broker);
    runtime.begin_turn();
    deliver(&mut harness, "hello");
    prime(&mut harness, &runtime);
    let _ = drain(&conn);
    // The child's output ends mid-turn: the run ends with it, on the
    // journaled road, instead of sitting on Working forever.
    finish(&mut harness, &runtime);
    let events = drain(&conn);
    assert!(
        events.iter().any(is_eof_error),
        "the EOF is announced: {:?}",
        events.iter().map(|event| event.kind()).collect::<Vec<_>>()
    );
    assert_eq!(
        events.iter().filter(|event| is_any_finish(event)).count(),
        1,
        "exactly one finish: {events:?}"
    );
    assert!(events.iter().any(is_error_finish), "the finish is an error");
}

#[test]
fn an_eof_after_the_expiry_publishes_no_second_finish() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let broker = broker();
    let mut harness = harness(&broker);
    let (runtime, conn) = attached(&broker);
    runtime.begin_turn();
    deliver(&mut harness, "hello");
    prime(&mut harness, &runtime);
    let _ = drain(&conn);
    harness
        .watch
        .backdate_activity_for_test(Duration::from_secs(60));
    harness.watch.tick_for_test();
    let expiry = drain(&conn);
    assert!(
        expiry.iter().any(is_error_finish),
        "the expiry ran: {expiry:?}"
    );
    // The output then ends anyway: the watchdog already ended the run, and
    // the EOF adds nothing.
    finish(&mut harness, &runtime);
    let after = drain(&conn);
    assert!(
        !after.iter().any(is_any_finish),
        "one finish, even when both roads fire: {after:?}"
    );
    finish(&mut harness, &runtime);
}

#[test]
fn a_rejected_plain_prompt_ends_the_run_with_pi_s_error_text() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let broker = broker();
    let mut harness = harness(&broker);
    let (runtime, conn) = attached(&broker);
    runtime.begin_turn();
    deliver(&mut harness, "hello");
    let _ = drain(&conn);
    // pi refuses the prompt in its preflight — no model selected — and no
    // `turn_end` will ever come.
    feed_line(
        &mut harness,
        &runtime,
        serde_json::json!({
            "type": "response",
            "id": "p-1",
            "command": "prompt",
            "success": false,
            "error": "No model selected",
        }),
    );
    let events = drain(&conn);
    assert!(
        events.iter().any(|event| matches!(
            event,
            SessionEvent::AgentError { message }
                if message.contains("rejected the prompt") && message.contains("No model selected")
        )),
        "the refusal carries pi's own error text: {events:?}"
    );
    assert_eq!(
        events.iter().filter(|event| is_any_finish(event)).count(),
        1,
        "the run finishes once: {events:?}"
    );
    assert!(events.iter().any(is_error_finish), "the finish is an error");
    finish(&mut harness, &runtime);
}

#[test]
fn a_rejected_slash_prompt_publishes_its_window_and_no_probe() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let broker = broker();
    let mut harness = harness(&broker);
    let (runtime, conn) = attached(&broker);
    runtime.begin_turn();
    deliver(&mut harness, "/goal-list");
    feed_line(
        &mut harness,
        &runtime,
        super::super::local_command_test_support::notify_of("No open goals."),
    );
    // The notify earns its production denial answer on the wire; consume it,
    // so the only frame left to read would be a probe.
    try_next_echo(&mut harness, Duration::from_secs(5)).expect("the notify denial goes out");
    feed_line(
        &mut harness,
        &runtime,
        serde_json::json!({
            "type": "response",
            "id": "p-1",
            "command": "prompt",
            "success": false,
            "error": "No model selected",
        }),
    );
    let events = drain(&conn);
    assert_eq!(
        events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::SessionNotice { text, .. } => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>(),
        ["No open goals."],
        "the captured window output is still published: {events:?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            SessionEvent::AgentError { message }
                if message.contains("rejected the prompt")
        )) && events.iter().any(is_error_finish),
        "the rejected slash prompt's run ends too: {events:?}"
    );
    // A rejected local command gets no `get_state` probe: nothing further
    // is written to the wire.
    assert!(
        try_next_echo(&mut harness, Duration::from_millis(300)).is_none(),
        "no probe follows a rejected prompt"
    );
    finish(&mut harness, &runtime);
}
