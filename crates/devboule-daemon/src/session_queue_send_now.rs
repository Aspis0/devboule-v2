//! Send-now: the send a client asks for by name, and the refusals that can
//! answer it before a byte is claimed.
//!
//! The row it claims goes out through the ordinary send path, with the
//! interrupting behaviour and the same authority a send has; what happens to
//! the row afterwards is the drain's settlement
//! (`session_queue_drain.rs`), and the turn-end drain is there too.
//!
//! The operation id is remembered once the outcome is known, never before, and
//! it is remembered with that outcome: a client that lost the reply to a
//! refusal is told the refusal again, not that its message went out.

use devboule_protocol::{ActiveTurnBehavior, ErrorCode};

use super::session_messaging::{
    interrupt_refused_for_peer, may_interrupt_for, TURN_STARTED_WHILE_WAITING,
};
use super::session_queue::{
    no_such_item, operation_conflict, operation_in_flight, queue_draining, queue_fenced,
};
use super::session_queue_drain::{QueueClaim, QueueSendOutcome};
use super::session_queue_lifecycle::QueueMutation;
use super::session_queue_operations::{
    checked_operation_id, OperationOutcome, OperationSeen, PayloadFingerprint,
};
use super::{check_attached, not_found, ConnHandle, OwnerId, SessionRegistry, WireError};

impl SessionRegistry {
    /// Send-now: claim the named row and send it as an `interrupt` — the
    /// running turn is stopped and the text goes as a new turn — the same act
    /// the app's own send-now performs. On a definite failure the row returns
    /// to the front carrying the reason, and the queue waits for the user's next
    /// move rather than retrying on its own; on a write that may have landed
    /// the row is dropped instead.
    pub(crate) fn queue_send_now(
        &self,
        session_id: &str,
        client_operation_id: &str,
        subscription_id: u64,
        item_id: &str,
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<QueueMutation, WireError> {
        let fingerprint = PayloadFingerprint::new("queue_send_now")
            .field(item_id)
            .finish();
        checked_operation_id(client_operation_id)?;
        let runtime = self.queue_target(session_id, owner, conn)?;
        check_attached(&runtime, conn, subscription_id)?;
        // Send-now is a send that interrupts, and the send path refuses an
        // interrupt from a paired device wherever it is asked for. Asked here,
        // the answer comes before the claim, so a press that could never send
        // takes nothing out of the queue.
        if !may_interrupt_for(conn) {
            return Err(interrupt_refused_for_peer());
        }
        let item = {
            let mut queues = self.queues.entry(session_id);
            let state = queues.get_mut(session_id).ok_or_else(not_found)?;
            match state.seen_operation(client_operation_id, fingerprint) {
                // The answer this press already got, refusal and all: a
                // second claim would send the same row twice, and a success
                // would be a lie if the first one failed.
                OperationSeen::Answered(outcome) => return outcome.replay(),
                OperationSeen::Conflict => {
                    return Err(operation_conflict(client_operation_id));
                }
                OperationSeen::Fresh => {}
            }
            // This press is on the wire, so its answer is not yet one anything
            // can be told. A repeat of it is told to ask again rather than
            // given a second claim; another payload under its id is a conflict.
            if let Some(sending) = state.sending(client_operation_id) {
                return Err(if sending == fingerprint {
                    operation_in_flight(client_operation_id)
                } else {
                    operation_conflict(client_operation_id)
                });
            }
            if state.fenced {
                return Err(queue_fenced());
            }
            // One send on the wire per session: whoever holds the claim holds
            // the row, so a second press and a turn end both find it gone.
            if state.draining {
                return Err(queue_draining());
            }
            let position = state
                .position(item_id)
                .ok_or_else(|| no_such_item(item_id))?;
            let item = state.items.remove(position);
            // The press is the user restating intent, so whatever a failed send
            // parked, this row is the next thing out.
            state.draining = true;
            state.paused = false;
            state.sending_operation = Some((client_operation_id.to_string(), fingerprint));
            let snapshot = state.publish();
            drop(queues);
            self.publish_queue_snapshot(&runtime, snapshot);
            item
        };
        let claim = QueueClaim {
            item,
            runtime,
            owner: owner.clone(),
        };
        let outcome = self.send_claim(
            &claim,
            Some(ActiveTurnBehavior::Interrupt),
            owner,
            conn,
            subscription_id,
        );
        // The outcome is what this operation id now means, and it is only
        // known here. A stop fences the queue and leaves its ring, so the
        // outcome is recorded either way and a retry after the resume replays
        // it; only a queue a close or a delete forgot has nothing to record in.
        let answered = match outcome {
            QueueSendOutcome::Sent => {
                // The rest of the queue belongs to the turn this send just
                // started, so the drain arms for that turn's end.
                self.resume_queue_after_frame(session_id);
                OperationOutcome::Accepted
            }
            QueueSendOutcome::AdmissionLost => OperationOutcome::Refused(WireError::new(
                ErrorCode::InvalidRequest,
                TURN_STARTED_WHILE_WAITING,
            )),
            // The press is answered with the write's own failure. The row is
            // gone either way, and the snapshot that dropped it says so.
            QueueSendOutcome::Refused(error) => OperationOutcome::Refused(error),
            QueueSendOutcome::Dropped(error) => OperationOutcome::Dropped(error),
        };
        if let Some(state) = self.queues.lock().get_mut(session_id) {
            state.remember_outcome(client_operation_id, fingerprint, answered.clone());
        }
        match answered {
            OperationOutcome::Accepted => Ok(QueueMutation { replayed: false }),
            failed => failed.replay(),
        }
    }
}
