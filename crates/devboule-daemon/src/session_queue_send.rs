//! Send-now: take one item out of the queue and send it as an `interrupt` —
//! the queue's own synchronous send. The claim keeps the row (flagged in
//! flight) until the provider accepts, so a crash cannot lose it; a failed
//! attempt restores the item at the front with its error and parks the queue
//! — no automatic retry, the next attempt is a user act (fix pass 1, P1-4
//! and P1-5).

use super::{
    check_attached, no_such_item, not_found, session_origin_for, ActiveTurnBehavior, ConnHandle,
    ErrorCode, OwnerId, SendRequest, SessionRegistry, UserMessageAuthor, UserMessageKind,
    WireError,
};

impl SessionRegistry {
    /// Send one queued item immediately: the running turn is interrupted, the
    /// text goes as a new turn (a plain send when nothing runs), and on
    /// failure the item returns to the front carrying its error — Paseo's
    /// rule — while the queue stays paused for the user's next move.
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
            // The resolve above can be overtaken by a close or a delete;
            // the claim must not recreate what they discarded (P2-5).
            if !self.queue_session_live(session_id) {
                return Err(not_found());
            }
            self.ensure_queue_loaded(session_id, &mut queues)?;
            let state = queues
                .get_mut(session_id)
                .expect("ensure_queue_loaded inserts the state");
            if state.closing {
                return Err(WireError::new(
                    ErrorCode::SessionNotFound,
                    "This session is closing; its queue no longer accepts messages.",
                ));
            }
            if state.draining {
                return Err(WireError::new(
                    ErrorCode::InvalidRequest,
                    "A queued message is already being sent for this session.",
                ));
            }
            let position = state
                .position(item_id)
                .ok_or_else(|| no_such_item(item_id))?;
            let mut trial = state.clone();
            trial.draining = true;
            trial.items[position].unconfirmed = true;
            // The claim is written before it is taken: an unwritable store
            // refuses the claim outright rather than sending a message the
            // journal could not restore (P1-3, P1-4).
            self.store_queue_state(session_id, &trial.items, trial.next_id, trial.paused)?;
            let taken = trial.items[position].clone();
            let items = trial.snapshot();
            *state = trial;
            super::session_queue::deliver_queue_snapshot(session_id, &runtime, items);
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
            // The interrupt waited for its turn; if another turn started
            // after that wait, this text must not enter it either (the same
            // admission the drain gives).
            require_no_turn_running: true,
        });
        match result {
            Ok(_) => {
                // The provider accepted: the row leaves — unless the client
                // removed or rewrote it mid-flight, which wins. A store that
                // refuses this clear leaves the row flagged (the honest
                // "unknown") and parks the queue; the send itself succeeded,
                // so the caller is still told that.
                let cleared = self.edit_queue(session_id, &runtime, |state| {
                    state.draining = false;
                    if let Some(position) = state.position(&taken.item_id) {
                        if state.items[position].unconfirmed {
                            state.items.remove(position);
                        }
                    }
                    Ok(())
                });
                match cleared {
                    Ok(()) => {
                        // The rest of the queue belongs to the turn this send
                        // just started: wake the drain so it arms the hook for
                        // that turn's end. A wake whose unpause cannot be
                        // stored is logged, never reported as a failed send —
                        // the send and the item's removal are durable, and a
                        // retry would send a delivered message twice; the
                        // parked rest waits for the next client frame.
                        if let Err(error) = self.wake_queue_drain(session_id) {
                            eprintln!(
                                "send-now could not resume the queue for {session_id}: {}",
                                error.message
                            );
                        }
                        Ok(())
                    }
                    Err(cleanup) => {
                        eprintln!(
                            "send-now could not record its success for {session_id}: {}",
                            cleanup.message
                        );
                        self.park_queue_after_failed_write(session_id);
                        Ok(())
                    }
                }
            }
            Err(error) => {
                let failed_id = taken.item_id.clone();
                // The settle door, not the add budget: the error this writes
                // is the failure the add budget reserved room for, so the
                // trial is judged against the frame cap itself (P1-2).
                let restored = self.settle_queue_send(session_id, &runtime, |state| {
                    state.draining = false;
                    // The failed attempt parks the queue, and this call is
                    // deliberately the last thing it does: no wake, no
                    // retry — the next attempt is a user act (P1-5).
                    state.paused = true;
                    if let Some(position) = state.position(&failed_id) {
                        let mut item = state.items.remove(position);
                        item.unconfirmed = false;
                        item.error =
                            Some(super::session_queue::bounded_queue_error(&error.message));
                        state.items.insert(0, item);
                    }
                    Ok(())
                });
                if let Err(cleanup) = restored {
                    eprintln!(
                        "send-now failure could not be recorded for {session_id}: {}",
                        cleanup.message
                    );
                    self.park_queue_after_failed_write(session_id);
                }
                Err(error)
            }
        }
    }

    /// A write-through failed after the claim already ended: keep memory on
    /// the durable side of the line — items untouched, `draining` cleared
    /// (it is not durable), and the queue parked so nothing auto-sends a
    /// state the store and the daemon disagree about.
    fn park_queue_after_failed_write(&self, session_id: &str) {
        let mut queues = self.queues.lock();
        if let Some(state) = queues.get_mut(session_id) {
            state.draining = false;
            state.paused = true;
        }
    }
}
