//! The daemon's latest plan-usage frame per quota provider id.
//!
//! Fed by the live reader roads (`claude_client`, `codex_client`) as they
//! publish a `PlanUsage`, and by the OpenCode Go quota poll. The replay roads
//! never touch this. Read once per attach at the replay/live seam in
//! `event_pull`, where a fresh viewer is handed the latest frame — replay no
//! longer re-emits plan usage, so without this an attach that saw no frame yet
//! would show an empty meter.
//!
//! Memory-only, like the app-side store it feeds: a daemon restart empties
//! it until the provider's next live frame. It is never persisted and never
//! journaled.
//!
//! Keyed by provider, not by account: sessions record no account identity,
//! so two accounts of one provider share a slot here, as they already do in
//! the app-side store.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use devboule_protocol::{SessionEvent, SessionKind};

static LATEST: OnceLock<Mutex<HashMap<String, SessionEvent>>> = OnceLock::new();

fn latest() -> &'static Mutex<HashMap<String, SessionEvent>> {
    LATEST.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The providers whose plan usage this cache keeps, by session kind. A kind
/// that has no plan usage maps to none. A Pi session draws on the quota of the
/// provider its model is served by, so it is not keyed here: see
/// [`cached_for_session`].
fn provider_id(kind: SessionKind) -> Option<&'static str> {
    match kind {
        SessionKind::Claude => Some("claude"),
        SessionKind::Codex => Some("codex"),
        SessionKind::Pi | SessionKind::Acp | SessionKind::Terminal => None,
    }
}

/// The quota provider a Pi session's model provider draws on, when one is
/// known. OpenCode's Go plan is the only one so far.
fn quota_provider_for_model(model_provider: &str) -> Option<&'static str> {
    match model_provider {
        "opencode" => Some("opencode-go"),
        _ => None,
    }
}

/// Remember the latest plan-usage frame for its provider: a live frame or a
/// quota poll's reading. Anything else is not this cache's concern and is
/// ignored.
pub(crate) fn note_live(event: &SessionEvent) {
    let SessionEvent::PlanUsage { provider_id, .. } = event else {
        return;
    };
    latest()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(provider_id.clone(), event.clone());
}

/// The provider's latest frame, for a viewer at the attach seam. A read
/// copies; the entry stays for the next viewer.
pub(crate) fn cached_for_kind(kind: SessionKind) -> Option<SessionEvent> {
    let provider_id = provider_id(kind)?;
    cached(provider_id)
}

/// The frame a viewer of this session gets at attach: a Pi session whose model
/// is served by a quota provider gets that provider's frame, and any other
/// session gets what [`cached_for_kind`] gives it.
pub(crate) fn cached_for_session(
    kind: SessionKind,
    model_provider: Option<&str>,
) -> Option<SessionEvent> {
    match kind {
        SessionKind::Pi => cached(quota_provider_for_model(model_provider?)?),
        other => cached_for_kind(other),
    }
}

fn cached(provider_id: &str) -> Option<SessionEvent> {
    latest()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(provider_id)
        .cloned()
}

#[cfg(test)]
#[path = "plan_usage_cache_tests.rs"]
mod tests;

#[cfg(test)]
pub(crate) fn reset() {
    latest()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clear();
}
