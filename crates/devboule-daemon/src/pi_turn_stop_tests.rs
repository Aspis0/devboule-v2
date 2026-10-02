//! What a user's Stop does to a pi run's failure: the run in flight it
//! ended announces nothing; a Stop with no run in flight, or whose abort
//! never reached pi, ends nothing.

use super::super::local_command_test_support::{agent_start, idle_state, prompt_response};
use super::settled_support::{
    agent_settled, answered_agent_end, answered_turn_end, errors_of, failed_agent_end,
    failed_attempt, failed_turn_end, replay_errors, LiveRun,
};
use super::test_support::{abort_frame, deliver, drain, feed_line, finishes_of, try_next_echo};
use crate::session::SessionKiller;
use std::io::Write;
use std::time::Duration;

/// A Stop while the compaction an overflow started is summarising: the
/// abort cancels it, pi settles without an end that answers the interrupt,
/// and the run the user ended announces nothing.
#[test]
fn a_stop_during_the_compaction_announces_nothing() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let mut flow = LiveRun::start("s.pi.compact-stop");
    failed_attempt(&mut flow);
    feed_line(
        &mut flow.harness,
        &flow.runtime,
        serde_json::json!({"type": "compaction_start", "reason": "overflow"}),
    );
    // The user's Stop mid-summarisation: the abort reaches pi, and the
    // compaction ends cancelled.
    flow.harness.killer.interrupt();
    let _ = abort_frame(&mut flow.harness);
    feed_line(
        &mut flow.harness,
        &flow.runtime,
        serde_json::json!({"type": "compaction_end", "reason": "overflow", "aborted": true}),
    );
    feed_line(&mut flow.harness, &flow.runtime, agent_settled());
    let stopped = drain(&flow.conn);
    assert_eq!(
        errors_of(&stopped),
        Vec::<String>::new(),
        "the cancelled run announces no failure: {stopped:?}"
    );
    assert_eq!(
        replay_errors(flow),
        Vec::<String>::new(),
        "the restart replays no row either"
    );
}

/// A failing end without an abort mark that the reader dispatches after the
/// Stop: the run the user ended still announces nothing.
#[test]
fn a_failure_dispatched_after_a_stop_announces_nothing() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let mut flow = LiveRun::start("s.pi.stop-then-late-failure");
    flow.harness.killer.interrupt();
    let _ = abort_frame(&mut flow.harness);
    feed_line(&mut flow.harness, &flow.runtime, failed_agent_end(false));
    feed_line(&mut flow.harness, &flow.runtime, agent_settled());
    let stopped = drain(&flow.conn);
    assert_eq!(
        errors_of(&stopped),
        Vec::<String>::new(),
        "the stopped run announces no failure: {stopped:?}"
    );
    assert_eq!(
        replay_errors(flow),
        Vec::<String>::new(),
        "the restart replays no row either"
    );
}

/// A Stop that arrives after the run settled ended nothing: the failure the
/// settle announced stands, live and replayed.
#[test]
fn a_stop_after_the_settle_leaves_the_failure_row() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let mut flow = LiveRun::start("s.pi.stop-after-settle");
    feed_line(&mut flow.harness, &flow.runtime, failed_turn_end());
    feed_line(&mut flow.harness, &flow.runtime, failed_agent_end(false));
    feed_line(&mut flow.harness, &flow.runtime, agent_settled());
    let settled = drain(&flow.conn);
    assert_eq!(
        errors_of(&settled).len(),
        1,
        "the settled failure is one row: {settled:?}"
    );
    flow.harness.killer.interrupt();
    let _ = abort_frame(&mut flow.harness);
    let after_stop = drain(&flow.conn);
    assert_eq!(
        errors_of(&after_stop),
        Vec::<String>::new(),
        "the stop adds nothing and takes nothing back: {after_stop:?}"
    );
    assert_eq!(
        replay_errors(flow),
        errors_of(&settled),
        "the restart replays the failure the settle announced"
    );
}

/// A Stop belongs to the run it ended: the run that opens after it and
/// fails announces its own failure.
#[test]
fn a_failure_in_the_run_after_a_stop_is_announced() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let mut flow = LiveRun::start("s.pi.stop-then-failure");
    flow.harness.killer.interrupt();
    let _ = abort_frame(&mut flow.harness);
    feed_line(&mut flow.harness, &flow.runtime, agent_settled());
    // The next run ends with its failure before any turn boundary of its
    // own could answer the interrupt.
    feed_line(&mut flow.harness, &flow.runtime, agent_start());
    feed_line(&mut flow.harness, &flow.runtime, failed_agent_end(false));
    feed_line(&mut flow.harness, &flow.runtime, agent_settled());
    let next = drain(&flow.conn);
    assert_eq!(
        errors_of(&next).len(),
        1,
        "the run after the stop announces its failure: {next:?}"
    );
    assert_eq!(
        replay_errors(flow),
        errors_of(&next),
        "the restart replays that row"
    );
}

/// A Stop while nothing runs ends nothing: the next run's failure shows,
/// even when it arrives with no opening of its own.
#[test]
fn an_idle_stop_leaves_the_next_run_s_failure_row() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let mut flow = LiveRun::start("s.pi.idle-stop");
    settle_answered(&mut flow);
    flow.harness.killer.interrupt();
    let _ = abort_frame(&mut flow.harness);
    send_next_prompt(&mut flow);
    feed_line(&mut flow.harness, &flow.runtime, failed_agent_end(false));
    feed_line(&mut flow.harness, &flow.runtime, agent_settled());
    let next = drain(&flow.conn);
    assert_eq!(
        errors_of(&next).len(),
        1,
        "the run after an idle stop announces its failure: {next:?}"
    );
    assert_eq!(
        replay_errors(flow),
        errors_of(&next),
        "the restart replays that row"
    );
}

/// A Stop while the prompt is out and pi's opening has not reached the
/// reader yet: the opening dispatched after it belongs to the run the user
/// ended.
#[test]
fn a_stop_before_the_run_s_opening_is_dispatched_announces_nothing() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let mut flow = LiveRun::start("s.pi.stop-before-start");
    settle_answered(&mut flow);
    send_next_prompt(&mut flow);
    flow.harness.killer.interrupt();
    let _ = abort_frame(&mut flow.harness);
    feed_line(&mut flow.harness, &flow.runtime, agent_start());
    feed_line(&mut flow.harness, &flow.runtime, failed_agent_end(false));
    feed_line(&mut flow.harness, &flow.runtime, agent_settled());
    let stopped = drain(&flow.conn);
    assert_eq!(
        errors_of(&stopped),
        Vec::<String>::new(),
        "the stopped run announces no failure: {stopped:?}"
    );
    assert_eq!(
        replay_errors(flow),
        Vec::<String>::new(),
        "the restart replays no row either"
    );
}

/// A Stop whose abort never reached pi ended nothing: the run goes on, and
/// its failure shows.
#[test]
fn a_stop_whose_abort_is_not_written_leaves_the_failure_row() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let mut flow = LiveRun::start("s.pi.abort-unsent");
    // pi's stdin is closed, so the abort frame cannot be written.
    *flow.harness.killer.stdin.lock().expect("pi stdin") = None;
    flow.harness.killer.interrupt();
    feed_line(&mut flow.harness, &flow.runtime, failed_turn_end());
    feed_line(&mut flow.harness, &flow.runtime, failed_agent_end(false));
    feed_line(&mut flow.harness, &flow.runtime, agent_settled());
    let settled = drain(&flow.conn);
    assert_eq!(
        errors_of(&settled).len(),
        1,
        "the run the abort never reached announces its failure: {settled:?}"
    );
    assert_eq!(
        replay_errors(flow),
        errors_of(&settled),
        "the restart replays that row"
    );
}

/// A Stop after a command pi handled itself ends nothing: the command
/// opened no run, and the next run's failure shows.
#[test]
fn a_stop_after_a_locally_handled_command_leaves_the_next_run_s_failure_row() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let mut flow = LiveRun::start("s.pi.local-then-stop");
    settle_answered(&mut flow);
    run_local_command(&mut flow);
    flow.harness.killer.interrupt();
    let _ = abort_frame(&mut flow.harness);
    send_next_prompt(&mut flow);
    feed_line(&mut flow.harness, &flow.runtime, agent_start());
    feed_line(&mut flow.harness, &flow.runtime, failed_agent_end(false));
    feed_line(&mut flow.harness, &flow.runtime, agent_settled());
    let next = drain(&flow.conn);
    assert_eq!(
        errors_of(&next).len(),
        1,
        "the run after the stop announces its failure: {next:?}"
    );
    assert_eq!(
        replay_errors(flow),
        errors_of(&next),
        "the restart replays that row"
    );
}

/// The opening run answers and settles, leaving the session idle.
fn settle_answered(flow: &mut LiveRun) {
    feed_line(&mut flow.harness, &flow.runtime, answered_turn_end());
    feed_line(&mut flow.harness, &flow.runtime, answered_agent_end());
    feed_line(&mut flow.harness, &flow.runtime, agent_settled());
    let _ = drain(&flow.conn);
}

/// The next prompt goes out; nothing of its run has reached the reader.
fn send_next_prompt(flow: &mut LiveRun) {
    flow.runtime.begin_turn();
    deliver(&mut flow.harness, "again");
}

/// A slash command pi handles itself: it answers the prompt and its
/// `get_state` says nothing runs, so no run opens and none settles.
fn run_local_command(flow: &mut LiveRun) {
    flow.runtime.begin_turn();
    flow.harness
        .writer
        .write_all(b"/goal-list")
        .expect("buffer the command");
    flow.harness
        .writer
        .flush()
        .expect("the command goes to the wire");
    let prompt = next_frame_id(flow);
    feed_line(&mut flow.harness, &flow.runtime, prompt_response(&prompt));
    let probe = next_frame_id(flow);
    let state: serde_json::Value = serde_json::from_str(idle_state()).expect("idle state");
    feed_line(
        &mut flow.harness,
        &flow.runtime,
        serde_json::json!({
            "id": probe, "type": "response", "command": "get_state", "success": true,
            "data": state,
        }),
    );
    let ended = drain(&flow.conn);
    assert_eq!(
        finishes_of(&ended),
        vec!["completed".to_string()],
        "the command ended its run locally: {ended:?}"
    );
}

/// The id of the next frame the daemon wrote to pi.
fn next_frame_id(flow: &mut LiveRun) -> String {
    try_next_echo(&mut flow.harness, Duration::from_secs(5)).expect("the frame pi was sent")["id"]
        .as_str()
        .expect("a framed id")
        .to_string()
}
