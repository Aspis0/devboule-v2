//! How a CI verdict reaches the session that asked for it.
//!
//! One phrase: deliver the verdict as the daemon's own message. It goes
//! through the session registry's daemon-notice road — never as the user and
//! never as another agent — wrapped in the same `<devboule-system>` frame the
//! daemon's other reports use, with the idempotency key in it so the receiver
//! can tell a repeat from a new event.

use devboule_protocol::OwnerId;

use crate::ci_watch_store::CiWatchRecord;
use crate::session::SendError;
use crate::session::{neutralise_envelope_text, SessionRegistry};
use crate::untrusted_frame::Source;
use crate::visible_text::escape_for_model;

/// What the watch service needs from the sessions: whether the owner is
/// there, and a way to hand it a message. The delivery keeps the session
/// layer's split: a refusal never wrote anything and may be tried again,
/// an uncertain send may already be out and must not be repeated.
pub(crate) trait WakeSink: Send + Sync {
    fn is_live(&self, session_id: &str, owner: &OwnerId) -> bool;
    fn deliver(&self, session_id: &str, owner: &OwnerId, text: &str) -> Result<(), SendError>;
}

impl WakeSink for SessionRegistry {
    fn is_live(&self, session_id: &str, owner: &OwnerId) -> bool {
        self.live_agent_entries(owner)
            .is_ok_and(|entries| entries.iter().any(|entry| entry.session.id == session_id))
    }

    fn deliver(&self, session_id: &str, owner: &OwnerId, text: &str) -> Result<(), SendError> {
        self.note_data_read(session_id, owner, "ci".to_string());
        self.deliver_daemon_notice(session_id, owner, text)
            .map(|_| ())
    }
}

/// The message a finished watch wakes its owner with.
///
/// The summary is text GitHub's checks and logs wrote, so it is framed as
/// untrusted data with the host, repository, commit and watch the daemon holds;
/// the daemon's own lines (`kind`, `state`, `repo`, `sha`) are unchanged and
/// start their lines, where the app's parsers look for them.
pub(crate) fn wake_text(record: &CiWatchRecord) -> String {
    let summary = record.summary.as_deref().unwrap_or_default();
    let provenance = Source::CiRun {
        repo: &format!("{}/{}", record.host, record.slug()),
        sha: &record.sha,
        watch: &record.watch_id,
    }
    .header_lines();
    let lines = [
        "<devboule-system>".to_string(),
        "origin: local".to_string(),
        "role: daemon".to_string(),
        "from_agent: devboule-ci-watch".to_string(),
        "kind: ci_verdict".to_string(),
        provenance,
        format!("timestamp: {}", crate::ci_watch_store::now_ms()),
        format!(
            "eventId: {}",
            record.wake_key.as_deref().unwrap_or_default()
        ),
        format!("watchId: {}", record.watch_id),
        format!("state: {}", record.state.as_str()),
        format!("repo: {}", record.slug()),
        format!("sha: {}", record.sha),
        "summary:".to_string(),
        neutralise_envelope_text(&escape_for_model(summary.trim_end())),
        "</devboule-system>".to_string(),
    ];
    lines.join("\n")
}

#[cfg(test)]
#[path = "ci_wake_tests.rs"]
mod tests;
