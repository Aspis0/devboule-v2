//! One poll's decision, without a thread or a network: no key asks nothing, a
//! reading is kept, and a failed poll keeps no reading.

use std::cell::Cell;

use devboule_protocol::{PlanWindow, SessionEvent};

use super::reading_for;
use crate::quota_key::ApiKey;
use crate::quota_source::QuotaError;

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
    let outcome = reading_for(None, 1, |_, _| {
        asked.set(true);
        Ok(frame(1))
    });
    assert!(outcome.is_none());
    assert!(!asked.get());
}

#[test]
fn a_reading_for_a_key_is_kept_with_the_poll_time() {
    let key = ApiKey::from_text("fixture-key").expect("a key");
    let outcome = reading_for(Some(&key), 42, |_, observed| Ok(frame(observed)));
    assert_eq!(outcome, Some(frame(42)));
}

#[test]
fn a_failed_poll_stores_no_reading() {
    let key = ApiKey::from_text("fixture-key").expect("a key");
    for error in [
        QuotaError::Rejected,
        QuotaError::Malformed,
        QuotaError::Status(502),
    ] {
        let outcome = reading_for(Some(&key), 1, |_, _| Err(error));
        assert!(outcome.is_none());
    }
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
