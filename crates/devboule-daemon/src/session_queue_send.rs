//! Send-now: take one item out of the queue and send it as an `interrupt` —
//! the queue's own synchronous send. The claim/restore shape here is the one
//! `session_queue_drain.rs` mirrors: the item leaves (and the snapshot says
//! so) before the send starts, and a failed send puts it back at the front
//! with its error, which is Paseo's rule.

use super::*;

impl SessionRegistry {
    /// Send one queued item immediately: the running turn is interrupted, the
    /// text goes as a new turn (a plain send when nothing runs), and the item
    /// is already out of the queue — on failure it returns to the front,
    /// carrying the error for the snapshot to show.
    pub(crate) fn queue_send_now(
        &self,
        session_id: &str,
        subscription_id: u64,
        item_id: &str,
        owner: &OwnerId,
        conn: &ConnHandle,
    ) -> Result<(), WireError> {
        let runtime = self.queue_target(session_id, owner, conn)?;
        check_attached(&runtime, conn, subscription_id)?;
        let taken = {
            let mut queues = self.queues.lock();
            self.ensure_queue_loaded(session_id, &mut queues)?;
            let state = queues
                .entry(session_id.to_string())
                .or_insert_with(|| QueueState::new(Vec::new()));
            if state.draining {
                return Err(WireError::new(
                    ErrorCode::InvalidRequest,
                    "A queued message is already being sent for this session.",
                ));
            }
            let position = state
                .position(item_id)
                .ok_or_else(|| no_such_item(item_id))?;
            state.draining = true;
            let taken = state.items.remove(position);
            let items = state.snapshot();
            self.persist_queue(session_id, &items);
            runtime.publish_queue_snapshot(items);
            taken
        };
        let references = taken.attachment_references.clone();
        let result = self.send_with_subscription_timeout(&SendRequest {
            session_id,
            subscription_id,
            text: &taken.text,
            attachments: &[],
            attachment_references: &references,
            owner,
            conn,
            mcp_timeout: crate::mcp_broker::ready_timeout(),
            active_turn_behavior: Some(ActiveTurnBehavior::Interrupt),
            require_attachment: true,
            // Interrupting is the act `SessionInterrupt` decides; the peer gate
            // has already charged send-now `admin` for it (peer_policy.rs), and
            // this is the same local-only rule every steer fallback keeps.
            interrupt_on_steer_refusal: session_origin_for(&conn.conn_peer).is_local(),
            message_slot: None,
            preset_preamble: None,
            author: UserMessageAuthor::Human,
            message_kind: UserMessageKind::Composer,
        });
        match result {
            Ok(_) => {
                self.edit_queue(session_id, &runtime, |state| {
                    state.draining = false;
                    Ok(())
                })?;
                // The rest of the queue belongs to the turn this send just
                // started: wake the drain so it arms the hook for that
                // turn's end.
                self.wake_queue_drain(session_id);
                Ok(())
            }
            Err(error) => {
                // Back to the front, with the error on the row — one edit, one
                // snapshot: the item's return and its error are one fact.
                let mut failed = taken;
                failed.error = Some(error.message.clone());
                self.edit_queue(session_id, &runtime, |state| {
                    state.draining = false;
                    state.items.insert(0, failed);
                    Ok(())
                })?;
                // A queue frame arrived (this one): the pause rule says the
                // drain tries again now — it fails the same way and parks,
                // bounded by exactly one retry per frame.
                self.wake_queue_drain(session_id);
                Err(error)
            }
        }
    }
}
