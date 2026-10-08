//! The attach lookup for quota frames: a Pi session on an OpenCode model gets
//! the OpenCode Go frame, nothing else does, and a successful poll replaces the
//! reading with its newer stamp. These tests use the `opencode-go` slot only,
//! so they do not race the Claude and Codex tests that share the cache.

use std::sync::{Mutex, MutexGuard};

use devboule_protocol::{PlanWindow, SessionEvent, SessionKind};

use super::{cached_for_session, note_live};

/// One test at a time through the shared slot.
static SLOT: Mutex<()> = Mutex::new(());

fn slot() -> MutexGuard<'static, ()> {
    SLOT.lock().unwrap_or_else(|error| error.into_inner())
}

fn go_frame(observed_at_ms: i64, used: u64) -> SessionEvent {
    SessionEvent::PlanUsage {
        provider_id: "opencode-go".to_string(),
        plan_label: Some("OpenCode Go".to_string()),
        windows: vec![PlanWindow {
            duration_mins: 300,
            used_percent: Some(used),
            resets_at: None,
        }],
        credits: None,
        observed_at_ms: Some(observed_at_ms),
    }
}

#[test]
fn a_pi_session_on_an_opencode_model_gets_the_go_frame() {
    let _slot = slot();
    note_live(&go_frame(1_000, 10));
    assert_eq!(
        cached_for_session(SessionKind::Pi, Some("opencode")),
        Some(go_frame(1_000, 10))
    );
}

#[test]
fn a_pi_session_on_another_provider_or_with_none_gets_no_quota_frame() {
    let _slot = slot();
    note_live(&go_frame(1_000, 10));
    assert_eq!(cached_for_session(SessionKind::Pi, Some("anthropic")), None);
    assert_eq!(
        cached_for_session(SessionKind::Pi, Some("openrouter")),
        None
    );
    assert_eq!(cached_for_session(SessionKind::Pi, None), None);
}

#[test]
fn a_session_of_another_kind_is_not_served_the_go_frame() {
    let _slot = slot();
    note_live(&go_frame(1_000, 10));
    assert_eq!(cached_for_session(SessionKind::Acp, Some("opencode")), None);
    assert_eq!(
        cached_for_session(SessionKind::Terminal, Some("opencode")),
        None
    );
}

#[test]
fn an_identical_successful_poll_replaces_the_reading_with_its_newer_stamp() {
    let _slot = slot();
    note_live(&go_frame(2_000, 10));
    note_live(&go_frame(3_000, 10));
    assert_eq!(
        cached_for_session(SessionKind::Pi, Some("opencode")),
        Some(go_frame(3_000, 10))
    );
}
