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
        self.deliver_daemon_notice(session_id, owner, text)
            .map(|_| ())
    }
}

/// The message a finished watch wakes its owner with.
pub(crate) fn wake_text(record: &CiWatchRecord) -> String {
    let summary = record.summary.as_deref().unwrap_or_default();
    format!(
        "<devboule-system>\norigin: local\nrole: daemon\nfrom_agent: devboule-ci-watch\n\
         kind: ci_verdict\ntimestamp: {}\neventId: {}\nwatchId: {}\nstate: {}\nrepo: {}\nsha: {}\n\
         summary:\n{}\n</devboule-system>",
        crate::ci_watch_store::now_ms(),
        record.wake_key.as_deref().unwrap_or_default(),
        record.watch_id,
        record.state.as_str(),
        record.slug(),
        record.sha,
        neutralise_envelope_text(summary.trim_end()),
    )
}

#[cfg(test)]
#[path = "ci_wake_tests.rs"]
mod tests;
