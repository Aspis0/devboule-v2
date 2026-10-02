//! The registry's half of the shared queue: the session a queue frame acts on,
//! the one mutation door every edit goes through, the attach hand-over, the
//! lifecycle fence, and the single door a snapshot leaves through.
//!
//! The state itself is `session_queue.rs`; the edits are
//! `session_queue_ops.rs`, the operation identity `session_queue_operations.rs`
//! and the sends `session_queue_drain.rs`.

use std::collections::HashMap;
use std::sync::{Arc, MutexGuard};

use devboule_protocol::WireError;

use super::session_queue::{operation_conflict, QueueSnapshot, QueueState};
use super::session_queue_operations::{
    checked_operation_id, Fingerprint, OperationOutcome, OperationSeen,
};
use super::{not_found, ErrorCode, SessionRegistry, SessionRuntime};

/// What one mutating queue frame did with its operation id.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct QueueMutation {
    /// True when the daemon answered this id before and is answering again,
    /// rather than applying the frame a second time.
    pub(crate) replayed: bool,
}

impl SessionRegistry {
    /// The session a queue frame acts on: it exists, the caller may reach it,
    /// and it is an agent. A terminal has no queue, and the refusal is typed so
    /// a client can tell it apart from a missing session.
    pub(crate) fn queue_target(
        &self,
        session_id: &str,
        owner: &super::OwnerId,
        conn: &super::ConnHandle,
    ) -> Result<Arc<SessionRuntime>, WireError> {
        let map = self
            .inner
            .lock()
            .map_err(|_| super::internal("Session state is unavailable."))?;
        let entry = super::peer_entry(&map, session_id, owner, &conn.conn_peer)?;
        let session = entry.as_peer_visible().ok_or_else(super::process_gone)?;
        if !session.metadata.kind.is_agent() {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "Only agent sessions have a message queue.",
            ));
        }
        Ok(Arc::clone(&session.runtime))
    }

    /// The session's queue, handed out only while the registry still holds the
    /// very runtime the caller resolved: a frame resolves its target early and
    /// writes late, and a close or delete that forgot the queue in between must
    /// not see it recreated for a session nothing can drain.
    ///
    /// Lock order: the registry's map, then the queue map — the order a close
    /// and a delete fence in. The map guard is released once the queue guard is
    /// held, so a close either ran first and is answered here, or waits for the
    /// caller's write and then fences and clears it.
    fn registered_queue(
        &self,
        session_id: &str,
        runtime: &Arc<SessionRuntime>,
    ) -> Result<MutexGuard<'_, HashMap<String, QueueState>>, WireError> {
        let map = self
            .inner
            .lock()
            .map_err(|_| super::internal("Session state is unavailable."))?;
        let registered = map
            .get(session_id)
            .is_some_and(|entry| Arc::ptr_eq(&entry.runtime(), runtime));
        if !registered {
            return Err(not_found());
        }
        Ok(self.queues.entry(session_id))
    }

    /// One accepted mutation: it runs against a trial copy, so a refusal
    /// commits nothing, and the snapshot goes out under this lock — the change
    /// and the state every client then reads are one step, never two. The
    /// drain is woken once the lock is released, because it takes it again.
    ///
    /// The operation id is answered here too, under the same lock: a repeat
    /// with the same payload skips the change and publishes nothing, and a
    /// repeat with a different one is refused before the change runs. An id
    /// this queue has already answered keeps that answer whatever the queue
    /// looks like now, so its check comes before the fence. An edit's answer is
    /// always an accepted one, so an outcome that is not that — which only a
    /// send-now records, under an id this frame's fingerprint cannot match — is
    /// answered as the conflict it cannot be.
    pub(crate) fn mutate_queue(
        &self,
        session_id: &str,
        runtime: &Arc<SessionRuntime>,
        client_operation_id: &str,
        fingerprint: Fingerprint,
        change: impl FnOnce(&mut QueueState) -> Result<(), WireError>,
    ) -> Result<QueueMutation, WireError> {
        checked_operation_id(client_operation_id)?;
        let mut queues = self.registered_queue(session_id, runtime)?;
        let state = queues
            .get_mut(session_id)
            .expect("entry inserts the session's queue");
        match state.seen_operation(client_operation_id, fingerprint) {
            OperationSeen::Answered(OperationOutcome::Accepted) => {
                return Ok(QueueMutation { replayed: true })
            }
            OperationSeen::Answered(_) => return Err(operation_conflict(client_operation_id)),
            OperationSeen::Conflict => return Err(operation_conflict(client_operation_id)),
            OperationSeen::Fresh => {}
        }
        if state.fenced {
            return Err(super::session_queue::queue_fenced());
        }
        let mut trial = state.clone();
        change(&mut trial)?;
        let snapshot = trial.publish();
        trial.remember_operation(client_operation_id, fingerprint, OperationOutcome::Accepted);
        *state = trial;
        drop(queues);
        self.deliver(runtime, snapshot);
        self.resume_queue_after_frame(session_id);
        Ok(QueueMutation { replayed: false })
    }

    /// The attach hand-over: the subscriber that has just registered its
    /// subscription gets the queue as it stands, empty included.
    ///
    /// Published after `try_attach_with_subscription` has inserted the
    /// observer, so a mutation racing the attach reaches the new subscriber
    /// either as the live snapshot or inside the one read here — never between a
    /// read and the registration that was supposed to deliver it. It goes to
    /// every observer rather than only the new one, because a whole snapshot is
    /// the same value to each of them and the revision is what orders them.
    ///
    /// A terminal has no queue, so its attach publishes nothing.
    pub(crate) fn publish_queue_attach_snapshot(
        &self,
        runtime: &Arc<SessionRuntime>,
        session_id: &str,
    ) {
        if !self.queue_session_is_agent(session_id) {
            return;
        }
        self.deliver(runtime, self.queues.read(session_id));
    }

    /// Fence one session's queue and take what it held, returning the cleared
    /// state so the caller can publish it.
    ///
    /// The state is created if this session has never been queued in, and the
    /// fence is set on it whatever the list held, because the thing being
    /// fenced is the session: from here on no frame, no drain and no send-now
    /// may reopen or send anything, until a resume reopens it.
    ///
    /// Separate from [`Self::publish_queue_snapshot`] because a close must run
    /// this inside the registry's own critical section — before it takes the
    /// session out, so a drain that already resolved its target cannot claim a
    /// row the removal is about to make unreachable. Publishing waits for that
    /// section to end, because reaching into a runtime under the registry's
    /// map lock would nest two locks nobody ordered.
    pub(crate) fn fence_queue(&self, session_id: &str) -> QueueSnapshot {
        let mut queues = self.queues.entry(session_id);
        let state = queues
            .get_mut(session_id)
            .expect("entry inserts the session's queue");
        state.fenced = true;
        state.paused = true;
        state.items.clear();
        state.publish()
    }

    /// The same fence for a caller that is not removing the session itself.
    ///
    /// `runtime` is `None` when the caller has already torn the session down;
    /// then no subscriber can be told anything, and the clear is silent.
    pub(crate) fn fence_and_clear_queue(
        &self,
        session_id: &str,
        runtime: Option<&Arc<SessionRuntime>>,
    ) {
        let snapshot = self.fence_queue(session_id);
        if let Some(runtime) = runtime {
            self.deliver(runtime, snapshot);
        }
    }

    /// The one door a queue snapshot leaves through.
    ///
    /// Ephemeral by construction: it is enqueued to the session's attached
    /// observers and goes nowhere else. Nothing is journaled, nothing is added
    /// to the connection's backlog for a later attach to replay, and nothing is
    /// broadcast to a connection that has not attached — an observer exists only
    /// for a subscription that passed the session's own scope check, so
    /// "attached and authorized" is what this reaches. The epoch is stamped
    /// here, from the one place it is defined, so no caller can publish a
    /// snapshot that claims to come from another daemon process.
    pub(crate) fn publish_queue_snapshot(
        &self,
        runtime: &Arc<SessionRuntime>,
        snapshot: QueueSnapshot,
    ) {
        self.deliver(runtime, snapshot);
    }

    fn deliver(&self, runtime: &Arc<SessionRuntime>, snapshot: QueueSnapshot) {
        let dropped = snapshot.dropped.into_iter().collect();
        runtime.publish_queue_snapshot(
            self.queues.epoch().to_string(),
            snapshot.revision,
            snapshot.items,
            dropped,
        );
    }

    /// Reopen a queue that a stop fenced, so a resumed session can queue
    /// again.
    ///
    /// What the stop cleared stays cleared: what the user had queued before it
    /// is not restored, because it was written for the session that stopped.
    /// The revision only moves on — a client that gated on it must read this as
    /// a newer queue, not as the one it already applied.
    pub(crate) fn reopen_queue(&self, session_id: &str, runtime: &Arc<SessionRuntime>) {
        let snapshot = {
            let mut queues = self.queues.entry(session_id);
            let Some(state) = queues.get_mut(session_id) else {
                return;
            };
            state.fenced = false;
            state.paused = false;
            state.publish()
        };
        self.deliver(runtime, snapshot);
    }

    /// Drop a session's queue with no snapshot, for the callers that have
    /// already told every subscriber or have no subscriber left.
    pub(crate) fn forget_queue(&self, session_id: &str) {
        self.queues.forget(session_id);
    }

    /// Whether this session is an agent, which is the only kind with a queue.
    fn queue_session_is_agent(&self, session_id: &str) -> bool {
        self.inner
            .lock()
            .map(|map| {
                map.get(session_id)
                    .is_some_and(|entry| entry.metadata().kind.is_agent())
            })
            .unwrap_or(false)
    }
}
