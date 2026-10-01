//! The shared replay road: the journalled finish-withholding marker and
//! the one driver every replay seam calls.

use devboule_protocol::SessionEvent;
use serde_json::Value;

use super::ClaudeView;

/// The journal row that says the NEXT `result` envelope's finish was
/// withheld live (`claude_client`): replay reads it so a reattach derives
/// the same suppression instead of resurrecting the finish. An older daemon
/// that predates the marker ignores the unknown type and does resurrect the
/// finish — silent, crash-free, the safe direction.
pub(crate) const WITHHELD_FINISH_MARKER_TYPE: &str = "devboule_withheld_finish";

/// The marker row itself, journalled ahead of a result envelope whose finish
/// the live pass withheld. A daemon-owned type string, never a Claude frame.
pub(crate) fn withheld_finish_marker() -> serde_json::Value {
    serde_json::json!({"type": WITHHELD_FINISH_MARKER_TYPE})
}

/// One journalled envelope through the replay road shared by the rebuild
/// (`journal_replay`), the attach pull (`event_pull`) and the live restart
/// seed: the ACP view first, the Claude view when it models nothing. All
/// three derive the same events because all three call this — including the
/// rebind `reset()` and the turn-boundary clearing inside `ingest`.
pub(crate) fn drive_replay(view: &mut ClaudeView, value: &mut Value) -> Vec<SessionEvent> {
    crate::plan_text::bound_claude_envelope(value);
    let views = crate::acp_view::view_from_envelope(value, "");
    let views = if views.is_empty() {
        view.ingest(value)
    } else {
        views
    };
    // The one exception to replay-equals-live: plan usage is the account's
    // LIVE state, not transcript. The drop protects a reading the app
    // already has; where none exists yet the meter stays empty until the
    // provider's next live frame, and the attach seam re-delivers the
    // daemon's cached latest live frame (plan_usage_cache) to cover it.
    views
        .into_iter()
        .filter(|event| !matches!(event, SessionEvent::PlanUsage { .. }))
        .collect()
}

#[cfg(test)]
#[path = "claude_view_replay_tests.rs"]
mod tests;
