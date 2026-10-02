//! What a pi run's settled ending announces: the one failure row a
//! finally-failed run gets, nothing for a run a continuation rescued, and
//! the Failed strip the row leaves behind.

use super::super::local_command_test_support::agent_start;
use super::settled_support::{
    agent_settled, answered_agent_end, answered_turn_end, errors_of, failed_agent_end,
    failed_attempt, failed_turn_end, replay_errors, LiveRun,
};
use super::test_support::{drain, feed_line};
use devboule_protocol::AttentionReason;

/// A failure pi retried is silent: an attempt pi will run again holds
/// nothing, and only the run's own ending carries the row, live and
/// replayed identically.
#[test]
fn a_retried_attempt_is_silent_and_the_settled_failure_is_one_row() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let mut flow = LiveRun::start("s.pi.retry");
    // The first attempt fails and pi says it will run it again.
    feed_line(&mut flow.harness, &flow.runtime, failed_turn_end());
    feed_line(&mut flow.harness, &flow.runtime, failed_agent_end(true));
    let retried = drain(&flow.conn);
    assert_eq!(
        errors_of(&retried),
        Vec::<String>::new(),
        "a retried attempt announces nothing: {retried:?}"
    );
    // The retry fails the same way; that failure is the run's, and the row
    // it owes waits for the run's own ending.
    feed_line(&mut flow.harness, &flow.runtime, agent_start());
    feed_line(&mut flow.harness, &flow.runtime, failed_turn_end());
    feed_line(&mut flow.harness, &flow.runtime, failed_agent_end(false));
    let attempt = drain(&flow.conn);
    assert_eq!(
        errors_of(&attempt),
        Vec::<String>::new(),
        "the last attempt is not the run's ending: {attempt:?}"
    );
    feed_line(&mut flow.harness, &flow.runtime, agent_settled());
    let settled = drain(&flow.conn);
    let settled_errors = errors_of(&settled);
    assert_eq!(
        settled_errors.len(),
        1,
        "the settled failure is exactly one row: {settled:?}"
    );
    assert_eq!(
        replay_errors(flow),
        settled_errors,
        "the restart replays the live rows verbatim"
    );
}

/// A run that settles on an attempt pi had staked for a retry: that attempt
/// held nothing, so the settle announces nothing, live or replayed.
#[test]
fn a_settle_after_an_attempt_staked_for_retry_announces_nothing() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let mut flow = LiveRun::start("s.pi.retry-settled");
    feed_line(&mut flow.harness, &flow.runtime, failed_turn_end());
    feed_line(&mut flow.harness, &flow.runtime, failed_agent_end(true));
    feed_line(&mut flow.harness, &flow.runtime, agent_settled());
    let settled = drain(&flow.conn);
    assert_eq!(
        errors_of(&settled),
        Vec::<String>::new(),
        "an attempt staked for retry announces nothing: {settled:?}"
    );
    assert_eq!(
        replay_errors(flow),
        Vec::<String>::new(),
        "the restart replays no row either"
    );
}

/// A run pi recovers by compacting a context overflow: the failed attempt
/// is not the run's outcome, the continuation answers, and no strip ever
/// says the run failed.
#[test]
fn a_compaction_continued_run_announces_no_failure() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let mut flow = LiveRun::start("s.pi.compaction");
    failed_attempt(&mut flow);
    // The overflow continues through compaction — on by default — and pi
    // runs the prompt again on the compacted context.
    feed_line(
        &mut flow.harness,
        &flow.runtime,
        serde_json::json!({"type": "compaction_start", "reason": "overflow"}),
    );
    feed_line(
        &mut flow.harness,
        &flow.runtime,
        serde_json::json!({"type": "compaction_end", "reason": "overflow"}),
    );
    feed_line(&mut flow.harness, &flow.runtime, agent_start());
    feed_line(&mut flow.harness, &flow.runtime, answered_turn_end());
    feed_line(&mut flow.harness, &flow.runtime, answered_agent_end());
    feed_line(&mut flow.harness, &flow.runtime, agent_settled());
    let recovered = drain(&flow.conn);
    assert_eq!(
        errors_of(&recovered),
        Vec::<String>::new(),
        "the run recovered: it announces no failure: {recovered:?}"
    );
    assert_eq!(
        replay_errors(flow),
        Vec::<String>::new(),
        "the restart replays no row either"
    );
}

/// The other continuation pi runs after a failed attempt: a message queued
/// while it failed. Same shape — the continuation's own ending clears the
/// failure — so nothing is announced.
#[test]
fn a_queued_continuation_announces_no_failure() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let mut flow = LiveRun::start("s.pi.queued");
    failed_attempt(&mut flow);
    // The message the user queued while the attempt failed: pi starts the
    // run again for it and answers.
    feed_line(&mut flow.harness, &flow.runtime, agent_start());
    feed_line(
        &mut flow.harness,
        &flow.runtime,
        serde_json::json!({
            "type": "message_end",
            "message": {"role": "user", "content": [{"type": "text", "text": "one more thing"}]},
        }),
    );
    feed_line(&mut flow.harness, &flow.runtime, answered_turn_end());
    feed_line(&mut flow.harness, &flow.runtime, answered_agent_end());
    feed_line(&mut flow.harness, &flow.runtime, agent_settled());
    let serviced = drain(&flow.conn);
    assert_eq!(
        errors_of(&serviced),
        Vec::<String>::new(),
        "the queued message was answered: no failure is announced: {serviced:?}"
    );
    assert_eq!(
        replay_errors(flow),
        Vec::<String>::new(),
        "the restart replays no row either"
    );
}

/// The settled failure lands after the turn ended, and the strip has to
/// come out at Failed: the finish raises `Finished` while the run is
/// open, and the error row that follows outranks it.
#[test]
fn a_settled_failure_leaves_the_strip_failed_rather_than_done() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let mut flow = LiveRun::start("s.pi.strip");
    feed_line(&mut flow.harness, &flow.runtime, failed_turn_end());
    let finished = drain(&flow.conn);
    assert_eq!(
        super::test_support::finishes_of(&finished),
        ["error"],
        "the failed attempt finishes the turn: {finished:?}"
    );
    assert_eq!(
        flow.runtime.attention().map(|attention| attention.reason),
        Some(AttentionReason::Finished),
        "the strip holds the turn's own end while the run is open"
    );
    // The run's ending carries the failure, and it lands after the finish:
    // the row must leave the strip at Failed, not at the finish it followed.
    feed_line(&mut flow.harness, &flow.runtime, failed_agent_end(false));
    assert_eq!(
        flow.runtime.attention().map(|attention| attention.reason),
        Some(AttentionReason::Finished),
        "the attempt's ending is not the run's: the strip still holds the finish"
    );
    feed_line(&mut flow.harness, &flow.runtime, agent_settled());
    let settled = drain(&flow.conn);
    assert_eq!(
        errors_of(&settled).len(),
        1,
        "the settled failure is one row: {settled:?}"
    );
    assert_eq!(
        flow.runtime.attention().map(|attention| attention.reason),
        Some(AttentionReason::Error),
        "the settled failure outranks the finish it followed: the strip reads Failed"
    );
    let (dir, _session_id, _path) = flow.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}
