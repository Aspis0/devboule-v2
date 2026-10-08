//! The end shapes of a pi run that asked for a tool: an iteration's own
//! `turn_end` is not the run's end, and the run still ends exactly once —
//! at the closing end when one comes, at pi's own close when none does —
//! with every iteration's usage on the one finish.

use super::settled_support::{agent_settled, answered_agent_end, errors_of, LiveRun};
use super::test_support::{abort_frame, deliver, drain, feed_line, finishes_of, turn_end_with};
use crate::session::SessionKiller;
use devboule_protocol::SessionEvent;
use std::time::Duration;

/// One `turn_end` of a pi run that asked for a tool: the iteration stops
/// with `toolUse` and pi streams on once the tool results are back. Its
/// usage belongs to the iteration, not to the whole run.
fn tool_iteration_turn_end() -> serde_json::Value {
    serde_json::json!({
        "type": "turn_end",
        "message": {
            "role": "assistant",
            "content": [
                {"type": "toolCall", "id": "call-1", "name": "bash", "arguments": {}}
            ],
            "model": "pi-test",
            "usage": {
                "input": 10,
                "output": 4,
                "reasoning": 2,
                "cacheRead": 3,
                "cacheWrite": 5,
                "totalTokens": 14,
                "cost": {"total": 0.5}
            },
            "stopReason": "toolUse",
        },
        "toolResults": [
            {"role": "toolResult", "toolCallId": "call-1", "toolName": "bash", "content": "ok"}
        ],
    })
}

/// The run's closing end: the model's answer, with its own usage.
fn closing_turn_end() -> serde_json::Value {
    serde_json::json!({
        "type": "turn_end",
        "message": {
            "role": "assistant",
            "content": [{"type": "text", "text": "done"}],
            "model": "pi-test",
            "usage": {"input": 1, "output": 2, "totalTokens": 3, "cost": {"total": 0.25}},
            "stopReason": "stop",
        },
        "toolResults": [],
    })
}

/// A tool iteration's `turn_end` is not the run's end: pi keeps streaming
/// the same run while the tool results go back to the model. The turn stays
/// running, which is what admits a steer into that run, and the run's own
/// end still finishes it once. Mutant: the iteration's end finishing the
/// turn — a later steer is refused admission and goes out as a plain prompt
/// pi rejects with "Agent is already processing".
#[test]
fn a_tool_iterations_turn_end_leaves_the_run_running() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let mut flow = LiveRun::start("s.pi.tool.open");
    feed_line(&mut flow.harness, &flow.runtime, tool_iteration_turn_end());
    let iteration = drain(&flow.conn);
    assert_eq!(
        finishes_of(&iteration),
        Vec::<String>::new(),
        "a tool iteration finishes nothing: {iteration:?}"
    );
    assert!(
        flow.runtime.is_running_turn(),
        "the run keeps streaming after the tool iteration"
    );
    assert!(
        flow.runtime
            .with_active_turn(flow.runtime.turn_counter(), |_| ())
            .is_some(),
        "the steer road is still admitted while the run streams"
    );
    feed_line(&mut flow.harness, &flow.runtime, closing_turn_end());
    let ended = drain(&flow.conn);
    assert_eq!(
        finishes_of(&ended),
        ["stop"],
        "the run's own end finishes it once: {ended:?}"
    );
    assert!(!flow.runtime.is_running_turn(), "the run is over");
}

/// A tool iteration leaves the run open, and pi's own close — with no
/// closing `turn_end` at all — ends it: once, and the `agent_settled` that
/// follows adds no second finish.
#[test]
fn a_tool_iteration_followed_by_pi_s_close_ends_the_run_once() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let mut flow = LiveRun::start("s.pi.tool.close");
    feed_line(&mut flow.harness, &flow.runtime, tool_iteration_turn_end());
    let iteration = drain(&flow.conn);
    assert_eq!(
        finishes_of(&iteration),
        Vec::<String>::new(),
        "the iteration is not the run's end: {iteration:?}"
    );
    feed_line(&mut flow.harness, &flow.runtime, answered_agent_end());
    let closed = drain(&flow.conn);
    assert_eq!(
        finishes_of(&closed),
        ["stop"],
        "pi's close ends the run; no closing turn_end came: {closed:?}"
    );
    assert!(!flow.runtime.is_running_turn(), "the run is over");
    feed_line(&mut flow.harness, &flow.runtime, agent_settled());
    let settled = drain(&flow.conn);
    assert_eq!(
        finishes_of(&settled),
        Vec::<String>::new(),
        "the run already ended: {settled:?}"
    );
}

/// The other order: the closing `turn_end` lands first, so pi's later close
/// adds nothing to the one finish.
#[test]
fn a_tool_iteration_then_a_closing_turn_end_still_ends_at_that_end() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let mut flow = LiveRun::start("s.pi.tool.closing");
    feed_line(&mut flow.harness, &flow.runtime, tool_iteration_turn_end());
    let _ = drain(&flow.conn);
    feed_line(&mut flow.harness, &flow.runtime, closing_turn_end());
    let ended = drain(&flow.conn);
    assert_eq!(
        finishes_of(&ended),
        ["stop"],
        "the closing end is the run's end: {ended:?}"
    );
    assert!(!flow.runtime.is_running_turn(), "the run is over");
    feed_line(&mut flow.harness, &flow.runtime, answered_agent_end());
    feed_line(&mut flow.harness, &flow.runtime, agent_settled());
    let after = drain(&flow.conn);
    assert_eq!(
        finishes_of(&after),
        Vec::<String>::new(),
        "pi's close adds nothing to an ended run: {after:?}"
    );
}

/// A person's Stop during a tool iteration: the abort answers no
/// `turn_end`, and the run still ends — once, and with no failure row for
/// the run the person ended.
#[test]
fn an_interrupt_during_a_tool_iteration_ends_the_run() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let mut flow = LiveRun::start("s.pi.tool.stop");
    feed_line(&mut flow.harness, &flow.runtime, tool_iteration_turn_end());
    let _ = drain(&flow.conn);
    flow.harness.killer.interrupt();
    let _ = abort_frame(&mut flow.harness);
    feed_line(&mut flow.harness, &flow.runtime, answered_agent_end());
    feed_line(&mut flow.harness, &flow.runtime, agent_settled());
    let ended = drain(&flow.conn);
    assert_eq!(
        finishes_of(&ended),
        ["stop"],
        "the run the person stopped ends once: {ended:?}"
    );
    assert!(!flow.runtime.is_running_turn(), "the turn is over");
    assert_eq!(
        errors_of(&ended),
        Vec::<String>::new(),
        "a run the person ended announces no failure: {ended:?}"
    );
}

/// The expiry's owed mark is spent by the first end of any kind: a tool
/// iteration that reports after the expiry cannot leave the mark armed
/// against the next run's own aborted end.
#[test]
fn a_late_tool_iteration_after_the_expiry_does_not_trap_the_next_run() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let mut flow = LiveRun::start("s.pi.tool.expiry");
    flow.harness
        .watch
        .backdate_activity_for_test(Duration::from_secs(60));
    flow.harness.watch.tick_for_test();
    let expiry = drain(&flow.conn);
    assert_eq!(
        finishes_of(&expiry),
        ["error"],
        "the expiry ended the run: {expiry:?}"
    );
    // The long tool's iteration reports after the run was ended: it is an
    // end of the dead run, and the mark it meets is spent on it.
    feed_line(&mut flow.harness, &flow.runtime, tool_iteration_turn_end());
    let late = drain(&flow.conn);
    assert_eq!(
        finishes_of(&late),
        Vec::<String>::new(),
        "the late iteration finishes nothing: {late:?}"
    );
    // The next run, stopped by pi's own aborted answer, ends on that answer.
    flow.runtime.begin_turn();
    deliver(&mut flow.harness, "again");
    feed_line(&mut flow.harness, &flow.runtime, turn_end_with("aborted"));
    let ended = drain(&flow.conn);
    assert_eq!(
        finishes_of(&ended),
        ["aborted"],
        "the next run's own end is not the dead run's stale one: {ended:?}"
    );
    assert!(!flow.runtime.is_running_turn(), "the next run is over");
}

/// A suppressed iteration's usage is not lost: the run's one finish reports
/// the whole run, the closing end plus every tool iteration.
#[test]
fn a_withheld_iteration_s_usage_rides_the_finish_that_ends_the_run() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let mut flow = LiveRun::start("s.pi.tool.usage");
    feed_line(&mut flow.harness, &flow.runtime, tool_iteration_turn_end());
    let _ = drain(&flow.conn);
    feed_line(&mut flow.harness, &flow.runtime, closing_turn_end());
    let ended = drain(&flow.conn);
    let usage = ended
        .iter()
        .find_map(|event| match event {
            SessionEvent::AgentFinished { usage, .. } => usage.clone(),
            _ => None,
        })
        .expect("the run's one finish");
    assert_eq!(usage.input_tokens, Some(11), "10 + 1");
    assert_eq!(usage.output_tokens, Some(6), "4 + 2");
    assert_eq!(usage.total_tokens, Some(17), "14 + 3");
    assert_eq!(usage.thought_tokens, Some(2), "the iteration's reasoning");
    assert_eq!(usage.cache_read_tokens, Some(3));
    assert_eq!(usage.cache_write_tokens, Some(5));
    assert_eq!(usage.cost_usd, Some(0.75), "0.5 + 0.25");
}

/// A stop reason this daemon does not know is a run end, never an
/// iteration: a future pi spelling must fail toward today's behaviour, not
/// toward a turn nothing ever ends.
#[test]
fn an_unknown_stop_reason_is_a_run_end() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let mut flow = LiveRun::start("s.pi.stop.unknown");
    feed_line(
        &mut flow.harness,
        &flow.runtime,
        turn_end_with("someFutureIteration"),
    );
    let ended = drain(&flow.conn);
    assert_eq!(
        finishes_of(&ended),
        ["someFutureIteration"],
        "an unknown end still ends the run: {ended:?}"
    );
    assert!(!flow.runtime.is_running_turn(), "the run is over");
}
