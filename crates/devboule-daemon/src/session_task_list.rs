//! Derive and publish one session's background-task list.
//!
//! No stored state: children come from the registry and journal rows the
//! daemon already keeps, and the list is derived fresh on every read. A read
//! that cannot see the parent is refused; the internal refresh publishes to
//! the session's own observers, who passed its scope check at attach.

use devboule_protocol::{Session, SessionEvent, SessionTask, TranscriptIntegrity};

use super::*;
use crate::session_tasks::{derive_tasks, HeldChild, TaskRow};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// At most one derive per session per window; a trigger inside the window
/// schedules one trailing refresh, so the last state is always published.
pub(crate) const TASKS_REFRESH_DEBOUNCE: Duration = Duration::from_millis(500);
/// Triggering rows stashed for the trailing run, at most: the journal is
/// the primary source, this covers the commit gap.
pub(crate) const TASKS_PENDING_EXTRA_MAX: usize = 64;

impl super::SessionRegistry {
    /// Answer a `SessionTasksGet`: the parent must be this owner's, then the
    /// list is derived fresh from the roster and the journals, capped for
    /// the frame like the event.
    pub(crate) fn session_tasks(
        &self,
        session_id: &str,
        owner: &OwnerId,
    ) -> Result<(Vec<SessionTask>, u32), WireError> {
        let sessions = self.list(owner)?;
        if !sessions.iter().any(|session| session.id == session_id) {
            return Err(not_found());
        }
        let held = self.held_children(session_id, &sessions);
        let parent_end = self.parent_end_ts(session_id, &sessions);
        let tasks = self.derive_with_held(session_id, held, parent_end, &[])?;
        Ok(crate::session_tasks::cap_published(tasks))
    }

    /// Re-derive and publish the list after a change. Best effort and silent:
    /// a session with no runtime has no observers, and a journal that cannot
    /// be read leaves the last list standing until the next change repairs it.
    ///
    /// `extra` carries the just-published rows with their publish time. The
    /// fold merges them by journal sequence behind the replay rows — the
    /// journal write may not be visible yet, and when it is, applying the
    /// row twice answers the same list.
    pub(crate) fn refresh_session_tasks(&self, session_id: &str, extra: &[(SessionEvent, u64)]) {
        // Sessions out under the lock, facts after it: the child journals
        // are disk reads, and holding the registry map across them would
        // nest two waits nobody ordered.
        let (runtime, children, parent_ended): (Arc<SessionRuntime>, Vec<Session>, bool) = {
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
            // The debounce lives on the runtime because it is per session:
            // a trigger inside the window stashes its rows and schedules
            // one trailing refresh instead of deriving, so a burst costs a
            // leading derive plus one trailing one — and the triggering row
            // is never lost to a journal that has not landed it yet.
            if !runtime.tasks_derive_due() {
                runtime.stash_tasks_extra(extra);
                runtime.schedule_tasks_trailing(self.clone(), session_id.to_string());
                return;
            }
            let parent_ended = !entry.to_session().state.is_live();
            let children = map
                .values()
                .map(|entry| entry.to_session())
                .filter(|session| session.created_by.as_deref() == Some(session_id))
                .collect();
            (runtime, children, parent_ended)
        };
        self.publish_session_tasks(&runtime, session_id, &children, parent_ended, extra);
    }

    /// The same derive-and-publish without the debounce: a session's own
    /// end publishes its cancellations now, not at the window's end.
    pub(crate) fn refresh_session_tasks_urgent(&self, session_id: &str) {
        let (runtime, children, parent_ended): (Arc<SessionRuntime>, Vec<Session>, bool) = {
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
            runtime.mark_tasks_derived();
            let parent_ended = !entry.to_session().state.is_live();
            let children = map
                .values()
                .map(|entry| entry.to_session())
                .filter(|session| session.created_by.as_deref() == Some(session_id))
                .collect();
            (runtime, children, parent_ended)
        };
        self.publish_session_tasks(&runtime, session_id, &children, parent_ended, &[]);
    }

    /// Derive with the stashed triggering rows drained in, cap for the
    /// frame, and publish when the revision is newer than the last send.
    fn publish_session_tasks(
        &self,
        runtime: &Arc<SessionRuntime>,
        session_id: &str,
        children: &[Session],
        parent_ended: bool,
        extra: &[(SessionEvent, u64)],
    ) {
        let parent_end = parent_ended.then(|| {
            runtime
                .tasks_ended_wall_ms()
                .unwrap_or_else(crate::agent_activity::wall_now_ms)
        });
        let held = self.held_children(session_id, children);
        let revision = runtime.next_tasks_revision();
        let mut stashed = runtime.take_tasks_pending();
        stashed.extend(extra.iter().cloned());
        let Ok(tasks) = self.derive_with_held(session_id, held, parent_end, &stashed) else {
            return;
        };
        let (tasks, omitted) = crate::session_tasks::cap_published(tasks);
        runtime.publish_tasks_snapshot(self.tasks_epoch.clone(), tasks, revision, omitted);
    }

    /// Clear a scheduled trailing refresh: the scheduled run starts now, so
    /// a newer trigger may schedule the next one.
    pub(crate) fn clear_tasks_trailing(&self, session_id: &str) {
        let Ok(map) = self.inner.lock() else {
            return;
        };
        if let Some(entry) = map.get(session_id) {
            entry.runtime().clear_tasks_trailing_flag();
        }
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
        // A terminal child without a recorded end is still bounded: now is
        // the honest fallback, and the finish row corrects it when it lands.
        let ended_at_ms = match session.state.is_live() {
            true => None,
            false => Some(
                self.runtime_tasks_end(&session.id)
                    .unwrap_or_else(crate::agent_activity::wall_now_ms),
            ),
        };
        let (model, tool_call_count) = self.child_facts(&session.id);
        HeldChild {
            id: session.id.clone(),
            title,
            state: session.state.clone(),
            started_at_ms: session.created_at_ms,
            ended_at_ms,
            model,
            tool_call_count,
        }
    }

    /// The wall time the session's process was observed dead, when the
    /// runtime recorded one.
    fn runtime_tasks_end(&self, session_id: &str) -> Option<u64> {
        let map = self.inner.lock().ok()?;
        let entry = map.get(session_id)?;
        if entry.is_configuring() {
            return None;
        }
        entry.runtime().tasks_ended_wall_ms()
    }

    /// The parent's end time when it has ended: the observed death, or now
    /// when nothing recorded it (a recovered transcript never saw its own
    /// end). Live parents have no end.
    fn parent_end_ts(&self, session_id: &str, sessions: &[Session]) -> Option<u64> {
        let parent = sessions.iter().find(|s| s.id == session_id)?;
        if parent.state.is_live() {
            return None;
        }
        Some(
            self.runtime_tasks_end(session_id)
                .unwrap_or_else(crate::agent_activity::wall_now_ms),
        )
    }

    /// The child's declared model and its counted tool calls, both read off
    /// its own journal: the last manifest names the model, every tool-call
    /// row counts. The count is exact only for a complete replay: anything
    /// else answers `None`, never a partial number.
    fn child_facts(&self, child_id: &str) -> (Option<String>, Option<u64>) {
        let Some(journal) = &self.journal else {
            return (None, None);
        };
        let Ok(replay) = journal.replay(child_id) else {
            return (None, None);
        };
        let complete = replay.integrity == TranscriptIntegrity::Complete;
        crate::session_tasks::summarize_child(&replay.events, complete)
    }

    fn derive_with_held(
        &self,
        session_id: &str,
        held: Vec<HeldChild>,
        parent_end: Option<u64>,
        extra: &[(SessionEvent, u64)],
    ) -> Result<Vec<SessionTask>, WireError> {
        let mut rows: Vec<TaskRow> = Vec::new();
        if let Some(journal) = &self.journal {
            let replay = journal
                .replay(session_id)
                .map_err(|_| internal("Session state is unavailable."))?;
            rows.extend(
                replay
                    .events
                    .into_iter()
                    .zip(replay.event_ts_ms)
                    .zip(replay.event_seqs)
                    .map(|((event, ts_ms), pos)| TaskRow {
                        event,
                        ts_ms,
                        pos: Some(pos),
                    }),
            );
        }
        // The publisher never learns its row's sequence, so a trigger
        // sorts after every positioned row: it fired after them.
        rows.extend(extra.iter().map(|(event, ts)| TaskRow {
            event: event.clone(),
            ts_ms: Some(*ts),
            pos: None,
        }));
        Ok(derive_tasks(session_id, &held, &rows, parent_end))
    }
}

/// Whether a refresh may derive now: outside the debounce window. Pure so
/// tests drive it without sleeping.
pub(crate) fn refresh_due(last: Option<Instant>, now: Instant) -> bool {
    last.is_none_or(|at| now.duration_since(at) >= TASKS_REFRESH_DEBOUNCE)
}

#[cfg(test)]
#[path = "session_task_list_tests.rs"]
mod tests;
