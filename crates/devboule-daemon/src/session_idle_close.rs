//! The idle-close timer for coordinator-created children.
//!
//! The candidates are exactly the link table's children: a session a human
//! started has no link and is never visited here, which is the owner's rule
//! by construction rather than by a check. "Idle" is four conditions held at
//! one sweep — no turn running, no pending card, nothing queued or on its way
//! to the child, nobody viewing it — and the minutes come from the child's
//! profile read live at every sweep (D5), so a settings edit reaches children
//! already running and `Some(0)` is the timer off.
//!
//! The act decides and takes the child out of the map in one critical
//! section — the lock an agent message takes to find its target and arm its
//! brake, and the lock a prompt's delivery takes to resolve the writer it
//! marks itself with — so no send is writing to a child that is about to go.
//! Only then does it say "closed: idle": a notice on the child's own
//! transcript, a dedicated envelope to its creator (falling back to the
//! creator's own transcript when the prompt route is shut), and the close's
//! tail — the creator's report, the journal's closed mark, the teardown and
//! the slot release. A message to a closed child never reopens it: a sender
//! delivery lost the child in between, is refused — with
//! [`SessionRegistry::closed_child_refusal`]'s sentence once the row is
//! marked closed, whose "closed: idle" half comes from the in-memory reason
//! map below (D6: a restart closes every session anyway, so nothing about
//! the reason outlives the process).

use super::*;

use devboule_protocol::NoticeSeverity;

/// How many idle-closed ids the reason map keeps. A sender's question about a
/// child comes from its own recent history, so a bounded window answers every
/// plausible one, and the map cannot grow with the daemon's lifetime.
const IDLE_CLOSED_KEPT: usize = 128;

impl SessionRegistry {
    /// Sweep every commissioned child for idleness and close the ones whose
    /// profile's minutes have run out. Driven once a minute by the
    /// server-owned thread that already runs the quiet sweep; tests call it
    /// with injected instants. Each close releases its child's idle-shutdown
    /// slot here, beside the close it belongs to, so the pairing cannot be
    /// lost by a caller.
    pub(crate) fn sweep_idle_close_children(
        &self,
        state: &Arc<ServerState>,
        now: Instant,
    ) -> usize {
        let children: Vec<String> = self
            .creations
            .lock()
            .map(|table| table.children.keys().cloned().collect())
            .unwrap_or_default();
        let mut closed = 0;
        for child in &children {
            if self.sweep_one_idle_child(state, child, now) {
                closed += 1;
            }
        }
        closed
    }

    /// One child's turn in the sweep: arm or clear its timer, and act when
    /// the profile's minutes have run out with the conditions still held.
    fn sweep_one_idle_child(&self, state: &Arc<ServerState>, child: &str, now: Instant) -> bool {
        let Some(view) = self.child_view(child) else {
            return false;
        };
        let (session, runtime, owner) = &view;
        let is_live = matches!(
            session.state,
            SessionState::Live { .. } | SessionState::Silent { .. }
        );
        let minutes = self.idle_close_minutes_now(session.profile_id.as_deref());
        // The switch is read at the same moment as the clock: "never" clears
        // an armed spell before it can run.
        if minutes == 0 {
            self.set_idle_close_since(child, None);
            return false;
        }
        let idle = is_live
            && !runtime.is_running_turn()
            && !runtime.permission_pending()
            && !self.message_in_flight_to(child)
            // A prompt written straight to the provider arms no brake — the
            // wire road passes `message_slot: None` — so the delivery marks
            // itself instead, under this same lock.
            && !runtime.delivery_in_flight()
            && !self.child_is_viewed(child, &owner.user, runtime);
        let Some((since, creator)) = self.arm_idle_close(child, idle, now) else {
            return false;
        };
        if now.saturating_duration_since(since) < Duration::from_secs(u64::from(minutes) * 60) {
            return false;
        }
        #[cfg(test)]
        self.fire_idle_close_before_act_hook();
        // The timer's field is read before the section, not inside it: the
        // creation table is never taken under the map lock. A focus that is
        // still held is caught by the viewer read inside the section; what
        // this catches is a spell already cleared — a viewer who focused
        // and looked away.
        if self.idle_close_since(child) != Some(since) {
            return false;
        }
        // One critical section decides and takes the child out of the map.
        // Everything is re-read under the session map lock — the lock an
        // agent message holds while it arms its brake, and the lock a
        // prompt's delivery holds while it resolves the writer it marks
        // itself with — and the removal happens in that same hold. So a send
        // is either marked before this read (the child stays) or finds no
        // such child after it: never a write into a child that is already
        // going. The minutes come with them, so ticking the timer off wins
        // over a spell that has already run out.
        enum Act {
            Blocked,
            Refused,
            Taken(u32, Option<RegistryEntry>),
        }
        let act = {
            let Ok(mut map) = self.inner.lock() else {
                return false;
            };
            let minutes = self.idle_close_minutes_now(session.profile_id.as_deref());
            let blocked = minutes == 0
                || runtime.is_running_turn()
                || runtime.permission_pending()
                || self.message_in_flight_to(child)
                || runtime.delivery_in_flight()
                || self.child_is_viewed(child, &owner.user, runtime);
            if blocked {
                Act::Blocked
            } else {
                match self.take_session_for_close(&mut map, child, owner, &None) {
                    Ok(removed) => Act::Taken(minutes, removed),
                    Err(_) => Act::Refused,
                }
            }
        };
        match act {
            Act::Refused => false,
            Act::Blocked => {
                self.set_idle_close_since(child, None);
                false
            }
            Act::Taken(minutes, removed) => {
                self.close_removed_child(state, child, &view, &creator, minutes, removed)
            }
        }
    }

    /// The profile's minutes right now: read live at every use (D5), never
    /// copied at a child's birth, defaulting when there is no store, no
    /// profile id, or no such profile in the document.
    fn idle_close_minutes_now(&self, profile_id: Option<&str>) -> u32 {
        self.agent_profiles
            .get()
            .map(|store| store.idle_close_minutes(profile_id))
            .unwrap_or(crate::agent_profiles::DEFAULT_IDLE_CLOSE_MINUTES)
    }

    /// The act on a child the section already took out of the map: the
    /// child's transcript told, the creator told, then the close's tail with
    /// the slot release that belongs to it. Publishing *after* the removal
    /// is what keeps both messages true — nothing can take the child back —
    /// and the runtime is still alive until the tail tears it down.
    fn close_removed_child(
        &self,
        state: &Arc<ServerState>,
        child: &str,
        view: &(Session, Arc<SessionRuntime>, OwnerId),
        creator: &str,
        minutes: u32,
        removed: Option<RegistryEntry>,
    ) -> bool {
        let (session, runtime, owner) = view;
        // A child this sweep did not take is narrated by the road that took
        // it: only a removal this section made is talked about here, and the
        // entry leaving the map in that removal makes it one shot.
        if removed.is_some() {
            let _ = runtime.publish_daemon_event(SessionEvent::SessionNotice {
                text: format!("closed: idle after {minutes} minutes"),
                severity: NoticeSeverity::Info,
            });
            let display_name = session
                .display_name
                .clone()
                .unwrap_or_else(|| session.title.clone());
            let envelope =
                agent_idle_closed_envelope(&session.id, &display_name, minutes, &session.origin);
            // Zero wait on a creator whose broker has not come up: this runs
            // on the shared sweep thread, which owes every other child its
            // cadence, so `MCP_READY_TIMEOUT` (15 s) must never be spent here.
            if self
                .deliver_notice_to_creator(creator, owner, &envelope, Duration::ZERO)
                .is_err()
            {
                // The prompt route is shut and the child is already gone, so
                // the fact would otherwise reach nobody: the creator's own
                // transcript carries it as a daemon notice, which needs no
                // broker to land.
                if let Some(creator_runtime) = self.live_runtime(creator, owner) {
                    let _ = creator_runtime.publish_daemon_event(SessionEvent::SessionNotice {
                        text: format!(
                            "its child '{}' was closed: idle after {minutes} minutes",
                            neutralise_envelope_text(&single_line_header(&display_name))
                        ),
                        severity: NoticeSeverity::Info,
                    });
                }
            }
        }
        let closed = matches!(self.finish_close(child, removed, owner), Ok(true));
        if closed {
            self.record_idle_close(child);
            state.session_finished();
        }
        closed
    }

    /// Arm the timer while this child is idle and clear it when it is not,
    /// answering the instant its idle spell began (`None` when it did not).
    fn arm_idle_close(&self, child: &str, idle: bool, now: Instant) -> Option<(Instant, String)> {
        let mut table = self
            .creations
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let link = table.children.get_mut(child)?;
        if !idle {
            link.idle_close_since = None;
            return None;
        }
        let since = *link.idle_close_since.get_or_insert(now);
        Some((since, link.creator.clone()))
    }

    /// Set one child's timer — armed by the sweep, cleared by the sweep and
    /// by `set_presence` when a viewer focuses the child. A child that is not
    /// in the link table is left alone: nothing else closes it.
    pub(super) fn set_idle_close_since(&self, child: &str, since: Option<Instant>) {
        if let Ok(mut table) = self.creations.lock() {
            if let Some(link) = table.children.get_mut(child) {
                link.idle_close_since = since;
            }
        }
    }

    /// The armed instant, for the re-read the act makes.
    fn idle_close_since(&self, child: &str) -> Option<Instant> {
        let table = self.creations.lock().ok()?;
        table.children.get(child)?.idle_close_since
    }

    /// Nothing is queued or on its way to this child: every admitted send
    /// whose boundary it has not reached yet, whichever sender admitted it
    /// (`MessageBrake` is the only queue a target can be sitting in — a send
    /// to an idle child becomes its next prompt inline, it is never parked).
    fn message_in_flight_to(&self, child: &str) -> bool {
        self.message_brakes.lock().is_ok_and(|brakes| {
            brakes.values().any(|brake| {
                brake
                    .outstanding
                    .iter()
                    .any(|message| message.to_session == child)
            })
        })
    }

    /// Is anybody on this child right now: the attention suppression's own
    /// predicate — a connection of this owner's, app visible, focused on the
    /// child — or anyone attached to read its transcript.
    fn child_is_viewed(&self, child: &str, user: &str, runtime: &Arc<SessionRuntime>) -> bool {
        let focused = self.presence.lock().is_ok_and(|connections| {
            connections.values().any(|connection| {
                connection.user == user
                    && connection.app_visible
                    && connection.focused_session_id.as_deref() == Some(child)
            })
        });
        focused
            || runtime
                .stream
                .lock()
                .is_ok_and(|stream| !stream.observers.is_empty())
    }

    /// Remember that this child was closed *for idleness*, so the send miss
    /// can name the reason (D6). Bounded: the oldest reason goes first, and
    /// the id it belonged to simply falls back to plain "closed" afterwards.
    fn record_idle_close(&self, child: &str) {
        if let Ok(mut table) = self.creations.lock() {
            while table.idle_closed.len() >= IDLE_CLOSED_KEPT {
                table.idle_closed.pop_front();
            }
            table.idle_closed.push_back(child.to_string());
        }
    }

    /// The sentence `devboule_send_message` answers with when its live target
    /// lookup missed and the name is one of the caller's own children, closed.
    /// `None` keeps the broker's plain "target agent not found": a name that
    /// is no closed child of this caller says nothing about what exists. The
    /// targets accepted here are the live lookup's own — id, or a title one
    /// closed child carries — so a name that found the child alive finds its
    /// row dead.
    pub(crate) fn closed_child_refusal(
        &self,
        owner: &OwnerId,
        creator_session_id: &str,
        target: &str,
    ) -> Option<String> {
        let journal = self.journal.as_ref()?;
        let row = journal
            .closed_child_record(target, &owner.user, creator_session_id)
            .ok()
            .flatten()?;
        let name = row
            .display_name
            .clone()
            .unwrap_or_else(|| row.title.clone());
        let reason = if self.was_idle_closed(&row.id) {
            "closed: idle"
        } else {
            "closed"
        };
        Some(format!(
            "your child '{name}' is {reason}; closed sessions do not reopen — create a new one"
        ))
    }

    /// Whether this id is in the in-memory idle-reason map.
    fn was_idle_closed(&self, child: &str) -> bool {
        self.creations
            .lock()
            .ok()
            .is_some_and(|table| table.idle_closed.iter().any(|id| id == child))
    }
}
