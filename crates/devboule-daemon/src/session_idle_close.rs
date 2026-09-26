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
//! The close says "closed: idle" twice: a notice on the child's own transcript
//! while its journal row is still in the map, then a dedicated envelope to its
//! creator, and only then the audited `close` the app's own close uses. A
//! message to a closed child never reopens it — the send's miss answers with
//! [`SessionRegistry::closed_child_refusal`], whose "closed: idle" half comes
//! from the in-memory reason map below (D6: a restart closes every session
//! anyway, so nothing about the reason outlives the process).

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
        let minutes = self
            .agent_profiles
            .get()
            .map(|store| store.idle_close_minutes(session.profile_id.as_deref()))
            .unwrap_or(crate::agent_profiles::DEFAULT_IDLE_CLOSE_MINUTES);
        // The switch is read at the same moment as the clock: "never" clears
        // an armed spell, so a child can never be closed on minutes that were
        // true before the edit landed.
        if minutes == 0 {
            self.set_idle_close_since(child, None);
            return false;
        }
        let idle = is_live
            && !runtime.is_running_turn()
            && !runtime.permission_pending()
            && !self.message_in_flight_to(child)
            && !self.child_is_viewed(child, &owner.user, runtime);
        let Some((since, creator)) = self.arm_idle_close(child, idle, now) else {
            return false;
        };
        if now.saturating_duration_since(since) < Duration::from_secs(u64::from(minutes) * 60) {
            return false;
        }
        // What can move between the read above and the act is re-read here,
        // in the same order the conditions were weighed: a turn or a card
        // that appeared in the meantime ends the spell, and the field itself
        // is re-read last because focusing the child clears it
        // (`set_presence`) without any of the four reads above knowing.
        if runtime.is_running_turn() || runtime.permission_pending() {
            self.set_idle_close_since(child, None);
            return false;
        }
        if self.idle_close_since(child) != Some(since) {
            return false;
        }
        self.close_idle_child(state, child, &view, &creator, minutes)
    }

    /// The close act, in the order it has to happen: the child's transcript
    /// told while its row is still in the map, the creator told, then the
    /// audited close with the slot release that belongs to it.
    fn close_idle_child(
        &self,
        state: &Arc<ServerState>,
        child: &str,
        view: &(Session, Arc<SessionRuntime>, OwnerId),
        creator: &str,
        minutes: u32,
    ) -> bool {
        let (session, runtime, owner) = view;
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
        let _ = self.deliver_notice_to_creator(creator, owner, &envelope);
        match self.close(child, owner, &None) {
            Ok(true) => {
                self.record_idle_close(child);
                state.session_finished();
                true
            }
            Ok(false) | Err(_) => false,
        }
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
