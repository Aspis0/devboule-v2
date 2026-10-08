//! One poll's decision, without a thread or a network: what each attempt came
//! to, the wait that follows it, and the demand rules. The clock, the key-source
//! fingerprint and the demand are passed in, so no test reads real time.

use std::cell::Cell;
use std::sync::Arc;
use std::time::Duration;

use devboule_protocol::{PlanWindow, SessionEvent};

use super::{attempt_for, poll_once, Attempt, Schedule};
use crate::egress_client::OutboundError;
use crate::quota_key::ApiKey;
use crate::quota_live;
use crate::quota_source::QuotaError;
use crate::session::SessionRuntime;

const MINUTE: i64 = 60_000;
const FINGERPRINT: u64 = 7;

fn frame(observed_at_ms: i64) -> SessionEvent {
    SessionEvent::PlanUsage {
        provider_id: "opencode-go".to_string(),
        plan_label: Some("OpenCode Go".to_string()),
        windows: vec![PlanWindow {
            duration_mins: 300,
            used_percent: Some(3),
            resets_at: None,
        }],
        credits: None,
        observed_at_ms: Some(observed_at_ms),
    }
}

#[test]
fn no_key_means_no_request_and_no_reading() {
    let asked = Cell::new(false);
    let (attempt, reading) = attempt_for(None, 1, |_, _| {
        asked.set(true);
        Ok(frame(1))
    });
    assert_eq!(attempt, Attempt::NoKey);
    assert!(reading.is_none());
    assert!(!asked.get());
}

#[test]
fn a_reading_for_a_key_is_kept_with_the_poll_time() {
    let key = ApiKey::from_text("fixture-key").expect("a key");
    let (attempt, reading) = attempt_for(Some(&key), 42, |_, observed| Ok(frame(observed)));
    assert_eq!(attempt, Attempt::Reading);
    assert_eq!(reading, Some(frame(42)));
}

#[test]
fn a_failed_poll_stores_no_reading_and_names_its_kind() {
    let key = ApiKey::from_text("fixture-key").expect("a key");
    for (error, expected) in [
        (QuotaError::Rejected, Attempt::Refused),
        (QuotaError::Malformed, Attempt::Malformed),
        (QuotaError::Status(502), Attempt::Transient),
        (
            QuotaError::Throttled {
                retry_after_secs: Some(45),
            },
            Attempt::Throttled {
                retry_after_secs: Some(45),
            },
        ),
    ] {
        let (attempt, reading) = attempt_for(Some(&key), 1, |_, _| Err(error));
        assert_eq!(attempt, expected);
        assert!(reading.is_none());
    }
}

#[test]
fn outbound_failures_map_to_the_transient_or_malformed_kind() {
    assert_eq!(
        Attempt::from(QuotaError::Outbound(OutboundError::Timeout)),
        Attempt::Transient
    );
    assert_eq!(
        Attempt::from(QuotaError::Outbound(OutboundError::Cut)),
        Attempt::Transient
    );
    assert_eq!(
        Attempt::from(QuotaError::Outbound(OutboundError::Transport)),
        Attempt::Transient
    );
    assert_eq!(
        Attempt::from(QuotaError::Outbound(OutboundError::TooLarge)),
        Attempt::Malformed
    );
    assert_eq!(
        Attempt::from(QuotaError::Outbound(OutboundError::Refused("policy".into()))),
        Attempt::Malformed
    );
}

#[test]
fn a_reading_is_due_again_after_a_minute() {
    let mut schedule = Schedule::new();
    assert!(schedule.due(0, FINGERPRINT, false));
    schedule.record(Attempt::Reading, 0, FINGERPRINT);
    assert!(!schedule.due(MINUTE - 1, FINGERPRINT, false));
    assert!(schedule.due(MINUTE, FINGERPRINT, false));
}

#[test]
fn transient_failures_double_the_wait_up_to_thirty_minutes() {
    let mut schedule = Schedule::new();
    let mut now = 0;
    let mut waits = Vec::new();
    for _ in 0..6 {
        schedule.record(Attempt::Transient, now, FINGERPRINT);
        waits.push(schedule.next_ms - now);
        now = schedule.next_ms;
    }
    assert_eq!(
        waits,
        vec![
            2 * MINUTE,
            4 * MINUTE,
            8 * MINUTE,
            16 * MINUTE,
            30 * MINUTE,
            30 * MINUTE
        ]
    );
}

#[test]
fn a_named_retry_after_is_honoured_within_the_bounds() {
    for (seconds, expected) in [
        (Some(45), MINUTE),
        (Some(600), 10 * MINUTE),
        (Some(3600), 30 * MINUTE),
    ] {
        let mut schedule = Schedule::new();
        schedule.record(
            Attempt::Throttled {
                retry_after_secs: seconds,
            },
            0,
            FINGERPRINT,
        );
        assert_eq!(schedule.next_ms, expected, "{seconds:?}");
    }
    let mut unnamed = Schedule::new();
    unnamed.record(
        Attempt::Throttled {
            retry_after_secs: None,
        },
        0,
        FINGERPRINT,
    );
    assert_eq!(unnamed.next_ms, 2 * MINUTE, "an unnamed 429 starts the backoff");
}

#[test]
fn a_refused_key_waits_an_hour_unless_the_key_source_changes() {
    let mut schedule = Schedule::new();
    schedule.record(Attempt::Refused, 0, FINGERPRINT);
    assert!(!schedule.due(59 * MINUTE, FINGERPRINT, true));
    assert!(schedule.due(60 * MINUTE, FINGERPRINT, false));
    assert!(
        schedule.due(MINUTE, FINGERPRINT + 1, false),
        "a changed key source is tried at once"
    );
}

#[test]
fn a_malformed_reply_waits_half_an_hour() {
    let mut schedule = Schedule::new();
    schedule.record(Attempt::Malformed, 0, FINGERPRINT);
    assert!(!schedule.due(30 * MINUTE - 1, FINGERPRINT, false));
    assert!(schedule.due(30 * MINUTE, FINGERPRINT, false));
}

#[test]
fn a_demand_polls_early_only_when_nothing_holds_the_schedule() {
    let mut schedule = Schedule::new();
    schedule.record(Attempt::Reading, 0, FINGERPRINT);
    assert!(
        !schedule.due(10_000, FINGERPRINT, true),
        "inside the demand gap"
    );
    assert!(
        schedule.due(15_000, FINGERPRINT, true),
        "past the gap a demand polls early"
    );

    let mut held = Schedule::new();
    held.record(Attempt::Transient, 0, FINGERPRINT);
    assert!(
        !held.due(20_000, FINGERPRINT, true),
        "a held schedule ignores demand"
    );
}

#[test]
fn a_missing_key_holds_nothing_and_is_checked_again_in_a_minute() {
    let mut schedule = Schedule::new();
    schedule.record(Attempt::NoKey, 0, FINGERPRINT);
    assert_eq!(schedule.next_ms, MINUTE);
    assert!(
        schedule.due(15_000, FINGERPRINT, true),
        "no request is made, so a demand may check early"
    );
}

#[test]
fn the_thread_sleeps_at_most_a_minute_and_at_least_a_second() {
    let fresh = Schedule::new();
    assert_eq!(fresh.sleep_for(0), Duration::from_secs(1));

    let mut reading = Schedule::new();
    reading.record(Attempt::Reading, 0, FINGERPRINT);
    assert_eq!(reading.sleep_for(0), Duration::from_secs(60));

    let mut refused = Schedule::new();
    refused.record(Attempt::Refused, 0, FINGERPRINT);
    assert_eq!(refused.sleep_for(0), Duration::from_secs(60));
}

fn manifest(provider_id: Option<&str>, model_provider: Option<&str>) -> SessionEvent {
    SessionEvent::SessionManifest {
        provider_id: provider_id.map(str::to_string),
        current_model_id: None,
        current_model_provider_id: model_provider.map(str::to_string),
        models: Vec::new(),
        modes: None,
    }
}

/// The demand window is one shared static, so these tests take turns on it.
static DEMAND: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn a_manifest_for_another_provider_asks_for_no_quota_poll() {
    let _turn = DEMAND.lock().unwrap_or_else(|error| error.into_inner());
    super::DEMAND_UNTIL_MS.store(0, std::sync::atomic::Ordering::SeqCst);
    super::note_manifest(&manifest(Some("pi"), Some("anthropic")));
    super::note_manifest(&manifest(Some("pi"), None));
    super::note_manifest(&manifest(Some("claude"), Some("opencode")));
    assert_eq!(
        super::DEMAND_UNTIL_MS.load(std::sync::atomic::Ordering::SeqCst),
        0
    );
}

#[test]
fn a_pi_manifest_on_an_opencode_model_asks_for_polls_for_the_window() {
    let _turn = DEMAND.lock().unwrap_or_else(|error| error.into_inner());
    super::DEMAND_UNTIL_MS.store(0, std::sync::atomic::Ordering::SeqCst);
    super::note_manifest(&manifest(Some("pi"), Some("opencode")));
    let until = super::DEMAND_UNTIL_MS.load(std::sync::atomic::Ordering::SeqCst);
    assert!(until > super::now_ms());
    assert!(until <= super::now_ms() + super::DEMAND_WINDOW_MS);
}

#[test]
fn a_failed_poll_prunes_the_sessions_that_ended() {
    let kept = Arc::new(SessionRuntime::new());
    let ended = Arc::new(SessionRuntime::new());
    quota_live::watch(&kept);
    quota_live::watch(&ended);
    let ended_entry = Arc::downgrade(&ended);
    drop(ended);

    let key = ApiKey::from_text("fixture-key").expect("a key");
    let attempt = poll_once(Some(&key), 1, |_, _| Err(QuotaError::Status(502)));

    assert_eq!(attempt, Attempt::Transient);
    assert!(!quota_live::holds(&ended_entry));
    assert!(quota_live::holds(&Arc::downgrade(&kept)));
}

#[test]
fn a_poll_with_no_key_prunes_too() {
    let ended = Arc::new(SessionRuntime::new());
    quota_live::watch(&ended);
    let ended_entry = Arc::downgrade(&ended);
    drop(ended);

    assert_eq!(poll_once(None, 1, |_, _| Ok(frame(1))), Attempt::NoKey);
    assert!(!quota_live::holds(&ended_entry));
}
