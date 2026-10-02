//! The drain: the send the daemon makes for a queue at the end of a turn, the
//! claim that holds its row, and the settle that records what that send cost.
//!
//! Both go through the ordinary send path, and both hold the queue's
//! `draining` claim across their send. That claim is what makes the queue send
//! once: whoever takes it holds the row, so a concurrent edit, remove, move,
//! send-now or a second turn end cannot send the same item again.
//!
//! Admission is atomic against an ordinary send in the same place every other
//! send is: `require_no_turn_running` is judged under the session writer lock,
//! which `begin_turn` also takes, so a turn that started while this one waited
//! is observed rather than raced past. The queue's own fence is judged under the
//! same lock, one line above it. Neither a stop nor a close waits for that lock
//! (a write blocked on a hung child pipe would hang them), so a write that had
//! already passed the check goes into the process being killed: the declared
//! remainder, stated at the fence check and in `ARCHITECTURE.md`. A drain that
//! loses the turn race puts the item back untouched and waits for the turn that
//! won — the race is not the message's fault, and recording it as a failure
//! would park the queue over a message that never failed.
//!
//! A failure is read two ways, and the difference is the whole exactly-once
//! rule. A refusal that never reached the provider puts the row back with its
//! reason, because the text is certainly not with the agent. A write that
//! failed *after it began* drops the row instead: the prompt may already be in
//! the agent's context, and putting it back would offer the user a resend that
//! says the same thing twice. The snapshot that carries the drop names the row
//! and why, so nothing vanishes silently.

use std::sync::Arc;

use devboule_protocol::{ActiveTurnBehavior, QueuedMessage};

use super::session_messaging::SendError;
use super::session_queue::bounded_queue_error;
use super::{
    session_origin_for, ConnHandle, OwnerId, SendRequest, SessionRegistry, SessionRuntime,
    SteerOrigin, UserMessageAuthor, UserMessageKind, WireError,
};

/// The sentence a send that promised to start from idle answers with when a
/// turn began between its look and the writer. Re-exported from the send path
/// so the drain recognises the one race by the exact string that raised it.
pub(crate) use super::session_messaging::TURN_STARTED_WHILE_WAITING;

/// The connection a drain's own send runs under: no subscription, no peer, and
/// an id nothing outside this path knows. It exists because the send path asks
/// for one; `require_attachment: false` is what keeps it from being checked
/// against a subscription.
fn drain_conn() -> Arc<ConnHandle> {
    ConnHandle::with_peer(0, None)
}

/// One row the queue has handed to the wire, with the session it belongs to.
pub(super) struct QueueClaim {
    pub(super) item: QueuedMessage,
    pub(super) runtime: Arc<SessionRuntime>,
    /// The session's own owner, which is who a drain speaks for: it sends on
    /// the daemon's behalf, with no client behind it.
    pub(super) owner: OwnerId,
}

impl SessionRegistry {
    /// A queue frame arrived: resume the drain.
    ///
    /// Synchronous on purpose — the frame that changed the queue pays for the
    /// send it started, exactly as a `SessionSend` frame pays for its own, so
    /// a caller sees the queue act before its reply is written.
    pub(crate) fn resume_queue_after_frame(&self, session_id: &str) {
        self.drain_queue(session_id);
    }

    /// One drain pass: claim item 0 when no turn runs, send it outside every
    /// lock, settle the outcome, and loop while it may still send. If a turn
    /// runs, one turn-end hook is armed instead — that hook is the drain's only
    /// trigger from the turn side, and it re-enters here on a thread of its own.
    pub(crate) fn drain_queue(&self, session_id: &str) {
        loop {
            let Some(claim) = self.claim_queue_head(session_id) else {
                return;
            };
            let conn = drain_conn();
            match self.send_claim(&claim, None, &claim.owner, &conn, 0) {
                QueueSendOutcome::Sent => {}
                // The admission race: not a failure. The loop re-decides and
                // arms the hook for the turn that won.
                QueueSendOutcome::AdmissionLost => {}
                QueueSendOutcome::Refused(_) | QueueSendOutcome::Dropped(_) => return,
            }
        }
    }

    /// Take the front row out of the queue and hold it under the queue's
    /// `draining` claim, or answer `None` when nothing may be claimed now.
    fn claim_queue_head(&self, session_id: &str) -> Option<QueueClaim> {
        let (runtime, owner) = self.queue_send_target(session_id)?;
        let mut queues = self.queues.entry(session_id);
        let state = queues.get_mut(session_id)?;
        if state.fenced || state.draining || state.paused || state.items.is_empty() {
            return None;
        }
        let turn_id = runtime.turn_counter();
        if runtime.is_turn_active(turn_id) {
            if state.hook_armed {
                return None;
            }
            // Registered against the turn that is running *now*, in the same
            // critical section that turn's end takes: `None` means it ended
            // between the look and the arm, so the pass re-decides instead of
            // arming a hook nothing will fire.
            runtime.on_turn_end_if_active(turn_id, {
                let registry = self.clone();
                let target = session_id.to_string();
                move || registry.queue_drain_on_own_thread(&target)
            })?;
            state.hook_armed = true;
            return None;
        }
        // Live and idle: the claim. The row leaves the list and the claim goes
        // with it, so a concurrent frame cannot see it, move it or remove it,
        // and the snapshot this publishes is the queue as everyone must now see
        // it — without the row a send is holding.
        state.hook_armed = false;
        state.draining = true;
        let item = state.items.remove(0);
        let snapshot = state.publish();
        drop(queues);
        self.publish_queue_snapshot(&runtime, snapshot);
        Some(QueueClaim {
            item,
            runtime,
            owner,
        })
    }

    /// The armed hook fired. The arm is consumed first — a wake that finds a
    /// stale `hook_armed` arms nothing for the next turn and would strand the
    /// queue behind it — and then the pass runs.
    fn queue_drain_on_own_thread(&self, session_id: &str) {
        {
            let mut queues = self.queues.lock();
            if let Some(state) = queues.get_mut(session_id) {
                state.hook_armed = false;
            }
        }
        // A thread of its own because this hook runs on whichever thread ended
        // the turn — usually the provider's reader. The pass writes to the
        // session, and a slow write there must not stall that reader. A machine
        // too tired to start it runs the pass here rather than stranding the
        // queue until the next client frame.
        let registry = self.clone();
        let target = session_id.to_string();
        if std::thread::Builder::new()
            .name("session-queue-drain".to_string())
            .spawn(move || registry.drain_queue(&target))
            .is_err()
        {
            self.drain_queue(session_id);
        }
    }

    /// One claimed row on the wire, then the settle that records its outcome.
    pub(super) fn send_claim(
        &self,
        claim: &QueueClaim,
        behavior: Option<ActiveTurnBehavior>,
        owner: &OwnerId,
        conn: &ConnHandle,
        subscription_id: u64,
    ) -> QueueSendOutcome {
        let session_id = claim.runtime.session_id.as_str();
        let references = claim.item.attachment_references.clone();
        let result = self.send_with_subscription_timeout(&SendRequest {
            session_id,
            subscription_id,
            text: &claim.item.text,
            attachments: &[],
            attachment_references: &references,
            owner,
            conn,
            mcp_timeout: crate::mcp_broker::ready_timeout(),
            active_turn_behavior: behavior,
            require_attachment: false,
            // Interrupting is the act `SessionInterrupt` decides, so it is the
            // local-only road; the send path refuses it for a paired device
            // before this runs.
            interrupt_on_steer_refusal: session_origin_for(&conn.conn_peer).is_local(),
            message_slot: None,
            preset_preamble: None,
            spawn_prompt: None,
            author: UserMessageAuthor::Human,
            message_kind: UserMessageKind::Composer,
            steer_origin: SteerOrigin::Person,
            // The admission, checked where it is atomic against an ordinary
            // send: this send promised to start from idle, so a turn that began
            // while it waited must not receive its text.
            require_no_turn_running: true,
            // And the queue's fence, checked under the same writer lock one
            // line earlier in that path: a stop or close that fenced this queue
            // before that check refuses the write.
            require_queue_unfenced: true,
        });
        let outcome = match result {
            Ok(_) => QueueSendOutcome::Sent,
            Err(SendError::Refused(error)) if error.message == TURN_STARTED_WHILE_WAITING => {
                QueueSendOutcome::AdmissionLost
            }
            Err(SendError::Refused(error)) => QueueSendOutcome::Refused(error),
            Err(SendError::Uncertain(error)) => QueueSendOutcome::Dropped(error),
        };
        self.settle_claim(claim, &outcome);
        outcome
    }

    /// Put the claim down and record what it cost.
    ///
    /// A queue that no longer exists — a close took it — answers nothing: its
    /// rows went with it and this settle must not bring one back. A settled
    /// send is the only thing that clears the `draining` claim.
    fn settle_claim(&self, claim: &QueueClaim, outcome: &QueueSendOutcome) {
        let session_id = claim.runtime.session_id.as_str();
        let mut queues = self.queues.lock();
        let Some(state) = queues.get_mut(session_id) else {
            return;
        };
        state.draining = false;
        if state.fenced {
            // The session stopped being sendable while the send was on the
            // wire: the clear owns the list, and a late settle neither
            // repopulates it nor publishes it again — not the row this send
            // was refused or dropped for, and not a snapshot of a queue whose
            // session has gone. The row that was already inside the transport
            // when the fence went up is the one thing this cannot take back.
            return;
        }
        let snapshot = match outcome {
            // The row was taken out when it was claimed, so an accepted send
            // has nothing to remove. A client that edited or moved the list
            // meanwhile keeps what it did.
            QueueSendOutcome::Sent => state.publish(),
            QueueSendOutcome::AdmissionLost => {
                // Back to the front untouched: no error, and not parked.
                state.items.insert(0, claim.item.clone());
                state.publish()
            }
            QueueSendOutcome::Refused(error) => {
                let mut item = claim.item.clone();
                item.error = Some(bounded_queue_error(&error.message));
                state.items.insert(0, item);
                state.paused = true;
                state.publish()
            }
            QueueSendOutcome::Dropped(error) => {
                let item_id = claim.item.item_id.clone();
                eprintln!(
                    "queued message {item_id} of session {session_id} was dropped after a write \
                     that may have reached the provider: {}",
                    error.message
                );
                state.publish_with_drop(item_id)
            }
        };
        drop(queues);
        self.publish_queue_snapshot(&claim.runtime, snapshot);
    }

    /// The live agent session a drain acts on, and the owner it speaks for.
    /// Resolved straight from the registry map: a drain is the daemon's own act,
    /// not a client's, so no peer scope is consulted here — the client's scope
    /// was decided at its own frame, in `queue_target`.
    fn queue_send_target(&self, session_id: &str) -> Option<(Arc<SessionRuntime>, OwnerId)> {
        let map = self.inner.lock().ok()?;
        let entry = map.get(session_id)?;
        let session = entry.as_peer_visible()?;
        if !session.metadata.kind.is_agent() {
            return None;
        }
        Some((Arc::clone(&session.runtime), entry.owner().clone()))
    }
}

/// What a claimed row's send came back with.
pub(super) enum QueueSendOutcome {
    Sent,
    /// A turn began between the drain's idle look and its write. Not a failure:
    /// the item goes back untouched and the pass arms for the turn that won.
    AdmissionLost,
    /// The send was refused before any byte reached the provider, with the
    /// reason the row should carry.
    Refused(WireError),
    /// The write began and failed, so the prompt may be with the agent. The row
    /// is dropped rather than offered again, and the snapshot says so.
    Dropped(WireError),
}
