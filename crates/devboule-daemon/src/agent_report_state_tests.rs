//! Tests for the per-life hook `seq` gate and the accepted-state cache.

use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

use devboule_protocol::{AgentActivityState, ErrorCode};

use super::{reject_log_due, AgentReportState, SharedAgentReportState, REJECT_LOG_INTERVAL};
use crate::agent_report::{tests::report, AgentReport, MAX_HOOK_SOURCES};

#[test]
fn out_of_order_seq_must_not_regress_state() {
    let mut state = AgentReportState::default();
    assert!(state
        .apply(report(Some(5), AgentActivityState::Working))
        .expect("apply"));
    assert_eq!(
        state.last().map(|last| last.state),
        Some(AgentActivityState::Working)
    );
    assert!(!state
        .apply(report(Some(3), AgentActivityState::Idle))
        .expect("stale"));
    assert_eq!(
        state.last().map(|last| last.state),
        Some(AgentActivityState::Working),
        "seq 3 arrived after seq 5 and must not replace Working with Idle"
    );
    assert_eq!(state.last().and_then(|last| last.seq), Some(5));
}

#[test]
fn duplicate_seq_must_not_replace_state() {
    let mut state = AgentReportState::default();
    assert!(state
        .apply(report(Some(2), AgentActivityState::Working))
        .expect("apply"));
    assert!(!state
        .apply(report(Some(2), AgentActivityState::Blocked))
        .expect("duplicate"));
    assert_eq!(
        state.last().map(|last| last.state),
        Some(AgentActivityState::Working)
    );
}

#[test]
fn later_seq_is_accepted() {
    let mut state = AgentReportState::default();
    assert!(state
        .apply(report(Some(1), AgentActivityState::Idle))
        .expect("apply"));
    assert!(state
        .apply(report(Some(2), AgentActivityState::Working))
        .expect("apply"));
    assert_eq!(
        state.last().map(|last| last.state),
        Some(AgentActivityState::Working)
    );
    assert_eq!(state.last().and_then(|last| last.seq), Some(2));
}

#[test]
fn a_missing_seq_does_not_block_a_later_zero() {
    let mut state = AgentReportState::default();
    assert!(state
        .apply(report(None, AgentActivityState::Idle))
        .expect("unsequenced"));
    assert!(
        state
            .apply(report(Some(0), AgentActivityState::Working))
            .expect("zero after none"),
        "seq 0 must be distinct from a missing seq"
    );
    assert_eq!(
        state.last().map(|last| last.state),
        Some(AgentActivityState::Working)
    );
    assert_eq!(state.last().and_then(|last| last.seq), Some(0));
}

fn claude_report(
    identity: Option<&str>,
    seq: Option<u64>,
    state: AgentActivityState,
) -> AgentReport {
    let mut item = report(seq, state);
    item.source = "devboule:claude".to_string();
    item.agent = "claude".to_string();
    item.agent_session_id = identity.map(str::to_string);
    item
}

#[test]
fn the_least_recently_accepted_identity_is_evicted_and_returns_fresh() {
    let mut state = AgentReportState::default();
    for identity in ["life-1", "life-2", "life-3", "life-4"] {
        assert!(state
            .apply(claude_report(
                Some(identity),
                Some(5),
                AgentActivityState::Working
            ))
            .expect("apply"));
    }
    assert!(
        state
            .apply(claude_report(
                Some("life-5"),
                Some(5),
                AgentActivityState::Working
            ))
            .expect("fifth identity"),
        "a fifth identity fits by evicting the least recently accepted one"
    );
    assert!(
        !state
            .apply(claude_report(
                Some("life-2"),
                Some(1),
                AgentActivityState::Idle
            ))
            .expect("stale seq applied as false"),
        "the identities still tracked keep their counters, and a rejected \
         report does not refresh its recency"
    );
    assert!(
        state
            .apply(claude_report(
                Some("life-1"),
                Some(1),
                AgentActivityState::Idle
            ))
            .expect("evicted identity applies"),
        "an evicted identity counts from fresh — eviction forgets, it never silences"
    );
}

#[test]
fn an_identityless_sender_keeps_one_counter() {
    let mut state = AgentReportState::default();
    assert!(state
        .apply(claude_report(None, Some(5), AgentActivityState::Working))
        .expect("apply"));
    assert!(
        !state
            .apply(claude_report(None, Some(3), AgentActivityState::Idle))
            .expect("stale seq applied as false"),
        "a sender that announces no identity keeps the one counter per source it always had"
    );
    assert_eq!(state.last().and_then(|last| last.seq), Some(5));
}

#[test]
fn the_reject_log_line_is_rate_bound_per_key() {
    let now = Instant::now();
    assert!(
        reject_log_due(None, now),
        "the first reject for a key always earns its line"
    );
    assert!(
        !reject_log_due(Some(now), now),
        "an immediate repeat stays quiet"
    );
    assert!(!reject_log_due(
        Some(now),
        now + REJECT_LOG_INTERVAL - Duration::from_secs(1)
    ));
    assert!(reject_log_due(Some(now), now + REJECT_LOG_INTERVAL));
}

#[test]
fn the_reject_log_throttle_survives_a_reset() {
    let mut state = AgentReportState::default();
    let key = ("devboule:claude".to_string(), Some("life-a".to_string()));
    assert!(state
        .apply(claude_report(
            Some("life-a"),
            Some(5),
            AgentActivityState::Working
        ))
        .expect("apply"));
    assert!(
        !state
            .apply(claude_report(
                Some("life-a"),
                Some(3),
                AgentActivityState::Idle
            ))
            .expect("stale seq applied as false"),
        "the first reject earns its line"
    );
    let armed = state
        .sequences
        .get(&key)
        .and_then(|gate| gate.last_reject_log)
        .expect("the first reject was logged");
    let mut reset = claude_report(Some("life-a"), Some(1), AgentActivityState::Idle);
    reset.session_start_source = Some("startup".to_string());
    assert!(state.apply(reset).expect("the reset accepts"));
    assert_eq!(
        state
            .sequences
            .get(&key)
            .and_then(|gate| gate.last_reject_log),
        Some(armed),
        "the throttle rides the reset: the bound is per key per minute across re-opens"
    );
    assert!(
        !state
            .apply(claude_report(
                Some("life-a"),
                Some(1),
                AgentActivityState::Blocked
            ))
            .expect("stale seq applied as false"),
        "an immediate reject after the reset stays within the interval"
    );
    assert_eq!(
        state
            .sequences
            .get(&key)
            .and_then(|gate| gate.last_reject_log),
        Some(armed),
        "the reset/reject alternation does not log again inside the interval"
    );
}

#[test]
fn u64_max_seq_is_rejected() {
    let mut state = AgentReportState::default();
    let error = state
        .apply(report(Some(u64::MAX), AgentActivityState::Working))
        .expect_err("MAX is not a usable hook seq");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(state.last().is_none());
}

#[test]
fn concurrent_duplicate_seq_applies_exactly_once() {
    let state = Arc::new(SharedAgentReportState::default());
    let barrier = Arc::new(Barrier::new(2));
    let mut joins = Vec::new();
    for state_value in [AgentActivityState::Working, AgentActivityState::Blocked] {
        let state = Arc::clone(&state);
        let barrier = Arc::clone(&barrier);
        joins.push(thread::spawn(move || {
            barrier.wait();
            state.apply(report(Some(1), state_value))
        }));
    }
    let applied: Vec<bool> = joins
        .into_iter()
        .map(|join| join.join().expect("worker").expect("apply"))
        .collect();
    let wins = applied.iter().filter(|applied| **applied).count();
    assert_eq!(
        wins, 1,
        "duplicate seq=1 must apply exactly once, got {applied:?}"
    );
    let last = state.last().expect("one report");
    assert_eq!(last.seq, Some(1));
    assert!(
        last.state == AgentActivityState::Working || last.state == AgentActivityState::Blocked,
        "winner must be one of the two whole reports, got {:?}",
        last.state
    );
}

#[test]
fn too_many_distinct_sources_are_rejected() {
    let mut state = AgentReportState::default();
    for index in 0..MAX_HOOK_SOURCES {
        let mut item = report(Some(1), AgentActivityState::Working);
        item.source = format!("src-{index}");
        assert!(
            state.apply(item).expect("within cap"),
            "source {index} should fit"
        );
    }
    let mut extra = report(Some(1), AgentActivityState::Idle);
    extra.source = "src-overflow".to_string();
    let error = state
        .apply(extra)
        .expect_err("a 33rd distinct source must not grow the map");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(
        error.message.contains(&MAX_HOOK_SOURCES.to_string()),
        "error must name the limit, got {}",
        error.message
    );
}
