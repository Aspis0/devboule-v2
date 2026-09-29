//! The daemon's latest LIVE plan-usage frame per provider id.
//!
//! Fed only by the live reader roads (`claude_client`, `codex_client`) as
//! they publish a `PlanUsage`; the replay roads never touch this. Read once
//! per attach at the replay/live seam in `event_pull`, where a fresh viewer
//! is handed the provider's latest live frame — replay no longer re-emits
//! plan usage, so without this an attach that saw no live frame yet would
//! show an empty meter.
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
/// that has no plan usage maps to none.
fn provider_id(kind: SessionKind) -> Option<&'static str> {
    match kind {
        SessionKind::Claude => Some("claude"),
        SessionKind::Codex => Some("codex"),
        SessionKind::Pi | SessionKind::Acp | SessionKind::Terminal => None,
    }
}

/// Remember a LIVE plan-usage frame for its provider. Anything else is not
/// this cache's concern and is ignored.
pub(crate) fn note_live(event: &SessionEvent) {
    let SessionEvent::PlanUsage { provider_id, .. } = event else {
        return;
    };
    latest()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .insert(provider_id.clone(), event.clone());
}

/// The provider's latest live frame, for a viewer at the attach seam. A read
/// copies; the entry stays for the next viewer.
pub(crate) fn cached_for_kind(kind: SessionKind) -> Option<SessionEvent> {
    let provider_id = provider_id(kind)?;
    latest()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .get(provider_id)
        .cloned()
}

#[cfg(test)]
pub(crate) fn reset() {
    latest()
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clear();
}
