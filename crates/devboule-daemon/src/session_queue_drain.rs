//! The drain: what turns a queued message into a send. One item at a time,
//! through the same path a `SessionSend` takes (journal, echo, attachment
//! resolution), started by a turn's end or — when the session is live and
//! idle — by the queue frame that put the item there. A failed send parks
//! the item at the front with its error and pauses until a client's next
//! queue frame arrives, which is the Q1 brief's rule.
//!
//! Locks: the resolve below takes the registry map on its own, the claim and
//! the settle take this module's map on their own, and the send takes neither
//! — so no order between them exists to get wrong (see `session_queue.rs`).

use super::*;

impl SessionRegistry {
    /// A queue frame arrived: the pause a failed drain set is over (the
    /// brief: draining resumes "until a queue frame arrives from a client"),
    /// and a live idle session sends now. Synchronous on purpose — the frame
    /// that changed the queue pays for the send it started, exactly as a
    /// `SessionSend` frame pays for its own, and a caller sees the queue act
    /// before its reply is written.
    pub(super) fn wake_queue_drain(&self, session_id: &str) {
        {
            let mut queues = self.queues.lock();
            if let Some(state) = queues.get_mut(session_id) {
                state.paused = false;
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
            let claim = {
                let mut queues = self.queues.lock();
                if self.ensure_queue_loaded(session_id, &mut queues).is_err() {
                    return;
                }
                let Some(state) = queues.get_mut(session_id) else {
                    return;
                };
                if state.draining || state.paused || state.items.is_empty() {
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
                // Live and idle: the claim. The item leaves — and the
                // snapshot says so — before the send starts, so a client
                // racing the drain finds it already gone, and `draining`
                // (set here, cleared in the settle) is what makes "one drain
                // in flight per session" true across threads.
                state.hook_armed = false;
                state.draining = true;
                let item = state.items.remove(0);
                let claimed_items = state.snapshot();
                self.persist_queue(session_id, &claimed_items);
                runtime.publish_queue_snapshot(claimed_items);
                item
            };
            let internal_conn = ConnHandle::with_peer(0, None);
            let references = claim.attachment_references.clone();
            let result = self.send_with_subscription_timeout(&SendRequest {
                session_id,
                subscription_id: 0,
                text: &claim.text,
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
            });
            let failed = result.as_ref().err().map(|error| error.message.clone());
            {
                let mut queues = self.queues.lock();
                // `None` means the session's queue is gone — a close landed
                // while the send ran. The items went with it, and this claim
                // goes with them: nothing is resurrected.
                let Some(state) = queues.get_mut(session_id) else {
                    return;
                };
                state.draining = false;
                if let Some(message) = failed.as_ref() {
                    state.paused = true;
                    let mut parked = claim;
                    parked.error = Some(message.clone());
                    state.items.insert(0, parked);
                }
                let settled = state.snapshot();
                self.persist_queue(session_id, &settled);
                runtime.publish_queue_snapshot(settled);
            }
            if failed.is_some() {
                return;
            }
            // Success: loop. The send began a turn, so the next pass arms the
            // hook that waits for *that* turn's end — unless the queue is
            // empty now, in which case it returns at the top.
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
    /// turn's end are one code path for restored and fresh items alike.
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
