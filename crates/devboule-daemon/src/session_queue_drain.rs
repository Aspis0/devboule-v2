//! The drain: what turns a queued message into a send. One item at a time,
//! through the same path a `SessionSend` takes (journal, echo, attachment
//! resolution), started by a turn's end or — when the session is live and
//! idle — by the queue frame that put the item there.
//!
//! Delivery (fix pass 1): the claim keeps the row in the journal, flagged
//! `unconfirmed`, until the provider accepts the prompt — a crash in that
//! window restores the item paused instead of losing it, and nothing
//! resends it automatically ("never lost, never resent silently"). A send
//! that fails parks the item at the front with its error and pauses until
//! the user acts; a send whose writer turned out to hold a *new* turn puts
//! the item back untouched and waits for that turn instead.
//!
//! Locks: the resolve below takes the registry map on its own, the claim and
//! the settle take this module's map on their own, and the send takes neither
//! — so no order between them exists to get wrong (see `session_queue.rs`).

use super::session_messaging;
use super::{Arc, ConnHandle, SendRequest, SessionRegistry, UserMessageAuthor, UserMessageKind};

impl SessionRegistry {
    /// A queue frame arrived: the pause a failed drain set is over (the
    /// brief: draining resumes "until a queue frame arrives from a client"),
    /// and a live idle session sends now. Synchronous on purpose — the frame
    /// that changed the queue pays for the send it started, exactly as a
    /// `SessionSend` frame pays for its own, and a caller sees the queue act
    /// before its reply is written.
    ///
    /// One fence stays down: while the front item is `unconfirmed` the
    /// pause is not cleared, because a frame about *another* message is not
    /// permission to resend one whose outcome is unknown.
    pub(super) fn wake_queue_drain(&self, session_id: &str) {
        {
            let mut queues = self.queues.lock();
            if let Some(state) = queues.get_mut(session_id) {
                if !state.items.first().is_some_and(|item| item.unconfirmed) {
                    state.paused = false;
                }
            }
        }
        self.drain_step(session_id);
    }

    /// One drain pass, looping while it may still send: claim item 0 when no
    /// turn runs, send it outside every lock, settle the outcome, repeat. If
    /// a turn runs, arm one `on_turn_end` hook instead — that hook is the
    /// drain's only trigger from the turn side, and it re-enters here on a
    /// thread of its own.
    pub(super) fn drain_step(&self, session_id: &str) {
        loop {
            // Resolve the session first, on its own: live, an agent, still
            // there. A session that died or closed leaves its items where
            // they are — a restore finds them, a close already forgot them.
            let (runtime, owner) = {
                let Ok(map) = self.inner.lock() else {
                    return;
                };
                let Some(entry) = map.get(session_id) else {
                    return;
                };
                let Some(session) = entry.as_peer_visible() else {
                    return;
                };
                if !session.metadata.kind.is_agent() {
                    return;
                }
                (Arc::clone(&session.runtime), entry.owner().clone())
            };
            let claimed = {
                let mut queues = self.queues.lock();
                // The resolve above can be overtaken by a close or a delete;
                // the claim must not recreate what they discarded (P2-5).
                if !self.queue_session_live(session_id) {
                    return;
                }
                if self.ensure_queue_loaded(session_id, &mut queues).is_err() {
                    return;
                }
                let Some(state) = queues.get_mut(session_id) else {
                    return;
                };
                if state.draining || state.paused || state.items.is_empty() {
                    return;
                }
                if state.items[0].unconfirmed {
                    // An item whose outcome this daemon never confirmed is
                    // never claimed automatically: the UI asks, and edit,
                    // remove or send-now is how it leaves.
                    return;
                }
                let turn_id = runtime.turn_counter();
                if runtime.is_turn_active(turn_id) {
                    if state.hook_armed {
                        return;
                    }
                    // Registered against the turn that is running *now*, in
                    // the same critical section that turn's end takes: `None`
                    // means it ended between the look and the arm, so the
                    // loop re-decides instead of arming a hook nothing will
                    // fire (or worse, one that waits for a later turn).
                    let registry = self.clone();
                    let target = session_id.to_string();
                    let armed = runtime.on_turn_end_if_active(turn_id, move || {
                        let attempt = std::thread::Builder::new()
                            .name("session-queue-drain".to_string())
                            .spawn({
                                let registry = registry.clone();
                                let target = target.clone();
                                move || registry.drain_after_hook(&target)
                            });
                        if let Err(error) = attempt {
                            // A thread that cannot start is a machine that is
                            // shutting down; run the pass here rather than
                            // strand the queue until the next frame — this
                            // callback is the provider's reader thread, so the
                            // inline pass is the rarer failure's price.
                            eprintln!("session queue drain thread could not start: {error}");
                            registry.drain_after_hook(&target);
                        }
                    });
                    if armed.is_none() {
                        continue;
                    }
                    state.hook_armed = true;
                    return;
                }
                // Live and idle: the claim. The row stays in the journal,
                // flagged in flight, until the provider accepts the prompt —
                // a crash between here and the settle restores it paused
                // instead of losing it (P1-4). A store that refuses the flag
                // refuses the claim: an unrecorded claim is one this daemon
                // could not promise to restore, so nothing may be sent (P1-3).
                state.hook_armed = false;
                state.draining = true;
                state.items[0].unconfirmed = true;
                let items = state.snapshot();
                if self
                    .persist_queue(session_id, &items, state.next_id)
                    .is_err()
                {
                    state.draining = false;
                    state.items[0].unconfirmed = false;
                    state.paused = true;
                    return;
                }
                runtime.publish_queue_snapshot(items);
                state.items[0].clone()
            };
            let internal_conn = ConnHandle::with_peer(0, None);
            let references = claimed.attachment_references.clone();
            let result = self.send_with_subscription_timeout(&SendRequest {
                session_id,
                subscription_id: 0,
                text: &claimed.text,
                attachments: &[],
                attachment_references: &references,
                owner: &owner,
                conn: &internal_conn,
                mcp_timeout: crate::mcp_broker::ready_timeout(),
                // The turn this starts is what the *next* item waits for;
                // nothing steers here — a drain is a fresh prompt by
                // construction, the turn it follows has already ended.
                active_turn_behavior: None,
                // The daemon sends this, not a client: no subscription to be
                // attached through, exactly like an agent-to-agent delivery.
                require_attachment: false,
                interrupt_on_steer_refusal: false,
                message_slot: None,
                preset_preamble: None,
                author: UserMessageAuthor::Human,
                message_kind: UserMessageKind::Composer,
                // The drain's admission, checked where it is atomic: the
                // idle look above and this write are one step against a
                // regular `SessionSend`, whose `begin_turn` runs under this
                // same writer lock (P1-1).
                require_no_turn_running: true,
            });
            {
                let mut queues = self.queues.lock();
                // `None` means the session's queue is gone — a close or a
                // delete landed while the send ran. The items went with it,
                // and this claim goes with them: nothing is resurrected.
                let Some(state) = queues.get_mut(session_id) else {
                    return;
                };
                state.draining = false;
                if !self.queue_session_live(session_id) {
                    // Discarded mid-send: the forget owns the rest. The flag
                    // stays only until that forget removes the state whole.
                    return;
                }
                let mut trial = state.clone();
                let admission_lost = matches!(
                    &result,
                    Err(error) if error.message == session_messaging::TURN_STARTED_WHILE_WAITING
                );
                if let Err(error) = &result {
                    trial.paused = !admission_lost;
                    if let Some(position) = trial.position(&claimed.item_id) {
                        let mut item = trial.items.remove(position);
                        if admission_lost {
                            // Back to the front untouched: no error, not
                            // paused — the race was not the message's fault,
                            // and the loop below re-decides (it arms the hook
                            // for the turn that won).
                            item.unconfirmed = false;
                            trial.items.insert(0, item);
                        } else {
                            item.unconfirmed = false;
                            item.error = Some(error.message.clone());
                            trial.items.insert(0, item);
                        }
                    }
                } else if let Some(position) = trial.position(&claimed.item_id) {
                    // Accepted: the row leaves — unless the client removed or
                    // rewrote it mid-flight, which wins.
                    if trial.items[position].unconfirmed {
                        trial.items.remove(position);
                    }
                }
                if self
                    .persist_queue(session_id, &trial.items, trial.next_id)
                    .is_err()
                {
                    // Nothing committed: memory stays on the durable side of
                    // the line (the flagged row) and the queue parks so a
                    // broken store cannot invite a resend.
                    state.paused = true;
                    return;
                }
                let items = trial.snapshot();
                state.items = trial.items;
                state.paused = trial.paused;
                runtime.publish_queue_snapshot(items);
            }
            match &result {
                Ok(_) => {}
                // The admission race: not a failure — loop, and the pass
                // arms the hook for the turn that began meanwhile.
                Err(error) if error.message == session_messaging::TURN_STARTED_WHILE_WAITING => {}
                // A real failure parks the queue until a client's frame.
                Err(_) => return,
            }
        }
    }

    /// The armed hook fired: the arm is consumed here, before anything else
    /// — a wake that finds a stale `hook_armed` arms nothing for the new
    /// turn and would strand the queue behind it (the send-now race: the
    /// hook fires while `draining` holds the claim, so the pass below may
    /// return early and the clear must not wait for it) — then take the pass.
    fn drain_after_hook(&self, session_id: &str) {
        {
            let mut queues = self.queues.lock();
            if let Some(state) = queues.get_mut(session_id) {
                state.hook_armed = false;
            }
        }
        self.drain_step(session_id);
    }

    /// A session came back (resume): load its queue and — when it is live
    /// and idle — send it. The restored items go through the same wake every
    /// client frame uses, so pause, FIFO, one at a time, and waiting for the
    /// turn's end are one code path for restored and fresh items alike; a
    /// restored in-flight item keeps its fence (the wake will not unpause
    /// behind an `unconfirmed` front), so nothing resends on its own.
    pub(super) fn queue_restore_and_drain(&self, session_id: &str) {
        let has_items = {
            let mut queues = self.queues.lock();
            if self.ensure_queue_loaded(session_id, &mut queues).is_err() {
                return;
            }
            queues
                .get(session_id)
                .is_some_and(|state| !state.items.is_empty())
        };
        if has_items {
            self.wake_queue_drain(session_id);
        }
    }
}
