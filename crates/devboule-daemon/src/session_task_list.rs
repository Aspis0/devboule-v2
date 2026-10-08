//! Derive and publish one session's background-task list.
//!
//! No stored state: children come from the registry and journal rows the
//! daemon already keeps, and the list is derived fresh on every read. A read
//! that cannot see the parent is refused; the internal refresh publishes to
//! the session's own observers, who passed its scope check at attach.

use devboule_protocol::{Session, SessionEvent, SessionTask};

use super::*;
use crate::session_tasks::{derive_tasks, HeldChild};
use std::sync::Arc;

impl super::SessionRegistry {
    /// Answer a `SessionTasksGet`: the parent must be this owner's, then the
    /// list is derived fresh from the roster and the journals.
    pub(crate) fn session_tasks(
        &self,
        session_id: &str,
        owner: &OwnerId,
    ) -> Result<Vec<SessionTask>, WireError> {
        let sessions = self.list(owner)?;
        if !sessions.iter().any(|session| session.id == session_id) {
            return Err(not_found());
        }
        let held = self.held_children(session_id, &sessions);
        self.derive_with_held(session_id, held, &[])
    }

    /// Re-derive and publish the list after a change. Best effort and silent:
    /// a session with no runtime has no observers, and a journal that cannot
    /// be read leaves the last list standing until the next change repairs it.
    ///
    /// `extra` carries the just-published rows with their publish time, which
    /// the keyed fold applies after the replay — the journal write may not be
    /// visible yet, and when it is, applying the row twice answers the same
    /// list.
    pub(crate) fn refresh_session_tasks(&self, session_id: &str, extra: &[(SessionEvent, u64)]) {
        // Sessions out under the lock, facts after it: the child journals
        // are disk reads, and holding the registry map across them would
        // nest two waits nobody ordered.
        let (runtime, children): (Arc<SessionRuntime>, Vec<Session>) = {
            let Ok(map) = self.inner.lock() else {
                return;
            };
            let Some(entry) = map.get(session_id) else {
                return;
            };
            if entry.is_configuring() {
                return;
            }
            let runtime = entry.runtime();
            let children = map
                .values()
                .map(|entry| entry.to_session())
                .filter(|session| session.created_by.as_deref() == Some(session_id))
                .collect();
            (runtime, children)
        };
        let held = self.held_children(session_id, &children);
        let Ok(tasks) = self.derive_with_held(session_id, held, extra) else {
            return;
        };
        runtime.publish_tasks_snapshot(tasks);
    }

    fn held_children(&self, session_id: &str, sessions: &[Session]) -> Vec<HeldChild> {
        sessions
            .iter()
            .filter(|session| session.created_by.as_deref() == Some(session_id))
            .map(|session| self.held_child(session))
            .collect()
    }

    fn held_child(&self, session: &Session) -> HeldChild {
        let title = session
            .display_name
            .clone()
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| session.title.clone());
        let (model, tool_call_count) = self.child_facts(&session.id);
        HeldChild {
            id: session.id.clone(),
            title,
            state: session.state.clone(),
            started_at_ms: session.created_at_ms,
            model,
            tool_call_count,
        }
    }

    /// The child's declared model and its counted tool calls, both read off
    /// its own journal: the last manifest names the model, every tool-call
    /// row counts. `None` means the journal could not be read; a readable
    /// journal answers a count, zero included.
    fn child_facts(&self, child_id: &str) -> (Option<String>, Option<u64>) {
        let Some(journal) = &self.journal else {
            return (None, None);
        };
        let Ok(replay) = journal.replay(child_id) else {
            return (None, None);
        };
        let mut model = None;
        let mut calls = 0u64;
        for event in &replay.events {
            match event {
                SessionEvent::SessionManifest {
                    current_model_id: Some(id),
                    ..
                } => model = Some(id.clone()),
                SessionEvent::AgentToolCall { .. } => calls += 1,
                _ => {}
            }
        }
        (model, Some(calls))
    }

    fn derive_with_held(
        &self,
        session_id: &str,
        held: Vec<HeldChild>,
        extra: &[(SessionEvent, u64)],
    ) -> Result<Vec<SessionTask>, WireError> {
        let mut rows: Vec<(SessionEvent, Option<u64>)> = Vec::new();
        if let Some(journal) = &self.journal {
            let replay = journal
                .replay(session_id)
                .map_err(|_| internal("Session state is unavailable."))?;
            rows.extend(replay.events.into_iter().zip(replay.event_ts_ms));
        }
        rows.extend(extra.iter().map(|(event, ts)| (event.clone(), Some(*ts))));
        Ok(derive_tasks(session_id, &held, &rows))
    }
}

#[cfg(test)]
#[path = "session_task_list_tests.rs"]
mod tests;
