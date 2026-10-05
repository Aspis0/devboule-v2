//! One live agent on its own journal, and the two ways a test reads what a
//! client got: the events the connection drains, and the text of those events.
//!
//! Shared by the two attach-resume test files because they ask their questions
//! about the same session, and a difference between their fixtures would be a
//! difference nobody reads.

use std::path::PathBuf;
use std::sync::Arc;

use devboule_protocol::SessionEvent;

use crate::journal::Journal;

use super::session_queue_fixtures::{journal_row, queue_registry, queued_agent};
use super::tests::test_owner;
use super::{compose_session_id, ConnHandle, OwnerId, SessionRegistry, SessionRuntime};

/// A live agent session that has journalled nothing yet, its registry, and the
/// owner it belongs to.
pub(super) struct AttachFixture {
    pub(super) dir: PathBuf,
    pub(super) journal: Arc<Journal>,
    pub(super) registry: SessionRegistry,
    pub(super) owner: OwnerId,
    pub(super) id: String,
    pub(super) runtime: Arc<SessionRuntime>,
}

impl AttachFixture {
    pub(super) fn new(tag: &str) -> Self {
        let (dir, registry, journal) = queue_registry();
        let owner = test_owner("S-1-5-21-1", tag);
        let id = compose_session_id(&owner.session_token(), "r").expect("id");
        journal_row(&journal, &id, &owner);
        let runtime = queued_agent(&registry, &journal, &owner, &id).0;
        Self {
            dir,
            journal,
            registry,
            owner,
            id,
            runtime,
        }
    }

    /// A connection that agrees `session.queue` and, when asked, the
    /// resume-outcomes capability.
    pub(super) fn conn(&self, id: u64, resume_negotiated: bool) -> Arc<ConnHandle> {
        let conn = ConnHandle::new(id);
        conn.set_session_queue_negotiated(true);
        conn.set_resume_outcomes_negotiated(resume_negotiated);
        conn
    }

    /// Everything this connection is handed, in order.
    pub(super) fn drain(&self, conn: &ConnHandle) -> Vec<SessionEvent> {
        let mut events = Vec::new();
        loop {
            let batch = conn.pull_events();
            if batch.is_empty() {
                return events;
            }
            for event in &batch {
                conn.event_sent(event);
            }
            events.extend(batch.into_iter().map(|pending| pending.envelope.event));
        }
    }

    pub(super) fn shutdown(self) {
        self.journal.shutdown();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// One `agent_report`-journalled agent message.
pub(super) fn answer(text: &str) -> SessionEvent {
    SessionEvent::AgentMessage {
        message_id: Some(format!("m-{}", text)),
        text: text.to_string(),
        parent_tool_use_id: None,
        spawn_depth: None,

        images: Vec::new(),
    }
}

/// The conversation rows of a delivery, ignoring what crosses beside them.
pub(super) fn texts(events: &[SessionEvent]) -> Vec<String> {
    events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::AgentMessage { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}
