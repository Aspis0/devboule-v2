//! The registry roads behind the agent-command tools: `devboule_cancel_agent`,
//! `devboule_list_pending_permissions` and `devboule_get_agent_status`.
//!
//! Scope is the creator's session id every time — the broker's bearer hands it
//! over, and no tool argument can claim one. Everything here is a read or a
//! soft interrupt: nothing kills, nothing answers a card, nothing spends a slot.

use super::*;

/// How long cancel waits for the turn it caught to be no longer active:
/// two seconds is Paseo's own `INTERRUPT_SESSION_TIMEOUT_MS`, the same
/// question over the same provider round-trip. The bound is where parity
/// ends: Paseo force-cancels an acknowledged run that overruns it (and
/// throws when nothing acknowledged); this road never force-finishes a turn
/// it does not own, so an overrun answers `TurnStillRunning` with the turn
/// still running — the caller decides.
const CANCEL_TURN_END_TIMEOUT: Duration = Duration::from_secs(2);

/// One reply lists at most this many cards: a bound on what a single call can
/// pull into a coordinator's context, while the status snapshot keeps counting
/// exactly. The register road already caps each child at
/// `MAX_PENDING_ACP_PERMISSIONS`, so two saturated children fill the list.
const MAX_LISTED_PENDING_CARDS: usize = 64;

/// What one cancel attempt found: the tool's `success` and its sentence are
/// both read off this (`D1`) — only `Interrupted` is `success: true`.
#[derive(Debug)]
pub(crate) enum CancelOutcome {
    /// The turn the call caught was running and is gone within the bound —
    /// the send ended it or it ended before the send fired; the reply claims
    /// the turn's state, never the cause.
    Interrupted,
    /// No turn was running: nothing was sent and nothing was touched.
    NotRunning,
    /// The road fired but the turn was still running at the deadline — an
    /// interrupt no provider acknowledged.
    TurnStillRunning,
}

/// The card's `kind` in the reply's own spelling: the frame's words, read
/// from the type (`serde(rename_all = "lowercase")`) so the reply and the
/// wire can never spell it differently — `tool` for an ordinary permission
/// and for every request that carries no field, `question` for a model's
/// question. Anything else that reaches the table is named by its event kind.
fn card_kind(request: &SessionEvent) -> String {
    match request {
        SessionEvent::PermissionRequest { kind, .. } => {
            let kind = kind.unwrap_or_default();
            serde_json::to_value(kind)
                .ok()
                .and_then(|value| value.as_str().map(str::to_string))
                .unwrap_or_else(|| "tool".to_string())
        }
        other => crate::agent_activity::event_kind(other).to_string(),
    }
}

/// One card of the pending list: the same five fields wherever a card appears.
/// Framed the way the push envelope frames it — the title through
/// `single_line_header`, the excerpt through the cap and the neutraliser —
/// because tool output gets re-read as text wherever it lands: a fence or a
/// fake header line inside a child's words must not parse as structure.
pub(super) fn card_value(
    agent_id: &str,
    card_id: String,
    request: &SessionEvent,
) -> serde_json::Value {
    match request {
        SessionEvent::PermissionRequest {
            title,
            description,
            command,
            ..
        } => {
            let excerpt =
                session_envelopes::card_excerpt(description.as_deref(), command.as_deref(), title);
            serde_json::json!({
                "agentId": agent_id,
                "cardId": card_id,
                "title": session_envelopes::neutralise_envelope_text(
                    &session_envelopes::single_line_header(title),
                ),
                "kind": card_kind(request),
                "excerpt": session_envelopes::neutralise_envelope_text(
                    &session_envelopes::cap_excerpt_scalars(&excerpt),
                ),
            })
        }
        // A card that is not a permission request cannot be dropped: it sits
        // in the same table `pending_len` counts, and a list that hid it would
        // contradict its own count. Its event kind is all there is to say
        // about it, so it says it. (The register road refuses non-permission
        // events today — this arm is what a future card kind inherits rather
        // than a path that runs.)
        _ => serde_json::json!({
            "agentId": agent_id,
            "cardId": card_id,
            "title": card_kind(request),
            "kind": card_kind(request),
            "excerpt": "",
        }),
    }
}

impl super::SessionRegistry {
    /// `devboule_cancel_agent`: interrupt the current turn of one of the
    /// caller's own live children and keep the child. The answer is measured,
    /// not asserted — [`CancelOutcome::Interrupted`] only when a turn was
    /// running and is seen ended within the bound; [`CancelOutcome::NotRunning`]
    /// when there was nothing to interrupt; [`CancelOutcome::TurnStillRunning`]
    /// when the road fired but the turn did not stop in time. Nothing here
    /// fences the child's next turn: a later message restarting it is the
    /// point of keeping the child.
    pub(crate) fn interrupt_agent_child(
        &self,
        creator_session_id: &str,
        target: &str,
    ) -> Result<CancelOutcome, WireError> {
        self.interrupt_agent_child_within(creator_session_id, target, CANCEL_TURN_END_TIMEOUT)
    }

    /// The road above with the wait spelled out, so a test can drive the
    /// timeout arm in milliseconds instead of paying the production bound.
    pub(super) fn interrupt_agent_child_within(
        &self,
        creator_session_id: &str,
        target: &str,
        timeout: Duration,
    ) -> Result<CancelOutcome, WireError> {
        let (child_id, owner) = self.resolve_own_child(creator_session_id, target)?;
        let Some((_, runtime, _)) = self.child_view(&child_id) else {
            return Err(WireError::new(
                ErrorCode::SessionNotFound,
                format!("none of your live children is called '{target}'"),
            ));
        };
        // The turn's identity, captured before anything is sent: the reply
        // and the wait are both about *this* turn. A turn that ends between
        // this check and the send gets a cancel into an idle turn or a new
        // one — the same session-scoped fact the tool's description states.
        let turn_id = runtime.turn_counter();
        if !runtime.is_turn_active(turn_id) {
            return Ok(CancelOutcome::NotRunning);
        }
        self.interrupt(&child_id, &owner)?;
        // One send, then the wait is for that id: `success: true` means the
        // caught turn is gone — the reply claims the turn's state, never the
        // cause; the wait itself sends nothing more.
        let deadline = Instant::now() + timeout;
        loop {
            if !runtime.is_turn_active(turn_id) {
                return Ok(CancelOutcome::Interrupted);
            }
            if Instant::now() >= deadline {
                return Ok(CancelOutcome::TurnStillRunning);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// `devboule_list_pending_permissions`: every card the caller's own live
    /// children are parked on, one flat list ordered by child and card id,
    /// capped at [`MAX_LISTED_PENDING_CARDS`] — and whether the cap cut, which
    /// the reply must say so a coordinator never mistakes a truncated list for
    /// everything that is parked.
    ///
    /// The delegation switch gates answering, never seeing: this list is read
    /// while the switch is off, and answering still runs its own checks at the
    /// answer tool. Cards of children this caller did not create are not in
    /// the scan, and a caller with no registry row is refused the way the
    /// other two roads refuse it.
    pub fn list_child_permission_cards(
        &self,
        creator_session_id: &str,
    ) -> Result<(Vec<serde_json::Value>, bool), WireError> {
        let caller_owner = {
            let map = self
                .inner
                .lock()
                .map_err(|_| internal("Session state is unavailable."))?;
            let entry = map.get(creator_session_id).ok_or_else(|| {
                WireError::new(
                    ErrorCode::InvalidRequest,
                    "the calling session is not registered on this daemon",
                )
            })?;
            entry.owner().clone()
        };
        let mut cards = Vec::new();
        let mut truncated = false;
        for entry in self.live_agent_entries(&caller_owner)? {
            if !is_child_of(entry.session.created_by.as_deref(), creator_session_id) {
                continue;
            }
            let Some(broker) = entry.runtime.permission_broker() else {
                continue;
            };
            // The cap is asked before anything is cloned: `remaining` is all
            // this child may contribute, `pending_cards(remaining)` clones
            // only that many, and a child holding more than fits marks the
            // cut — its count is O(1), and a card nobody serves is never
            // copied out from under the broker's lock.
            let remaining = MAX_LISTED_PENDING_CARDS.saturating_sub(cards.len());
            if broker.pending_len() > remaining {
                truncated = true;
            }
            if remaining == 0 {
                continue;
            }
            for (card_id, request) in broker.pending_cards(remaining) {
                cards.push(card_value(&entry.session.id, card_id, &request));
            }
        }
        Ok((cards, truncated))
    }

    /// `devboule_get_agent_status`: one child's snapshot. A live child
    /// answers from its runtime; a child that is not live falls back to its
    /// stored row (D4) — a count where the live arm counts, and `null` for
    /// model and mode because they are not persisted, never a guess.
    ///
    /// Anything that is not the caller's own child — a sibling, a stranger's
    /// session, an invented id, a row this caller did not create — ends in
    /// one refusal that says none of that, so the answer never leaks whether
    /// the id exists.
    pub fn agent_status_snapshot(
        &self,
        state: &Arc<ServerState>,
        creator_session_id: &str,
        target: &str,
    ) -> Result<serde_json::Value, WireError> {
        match self.resolve_own_child(creator_session_id, target) {
            Ok((child_id, _)) => {
                let Some((session, runtime, _)) = self.child_view(&child_id) else {
                    return Err(WireError::new(
                        ErrorCode::SessionNotFound,
                        format!("none of your children is called '{target}'"),
                    ));
                };
                let manifest = runtime.session_manifest();
                let manifest_provider = manifest.as_ref().and_then(|event| match event {
                    SessionEvent::SessionManifest { provider_id, .. } => provider_id.clone(),
                    _ => None,
                });
                let model = manifest.as_ref().and_then(|event| match event {
                    SessionEvent::SessionManifest {
                        current_model_id, ..
                    } => current_model_id.clone(),
                    _ => None,
                });
                Ok(serde_json::json!({
                    "agentId": session.id,
                    "state": roster_task_state(&session, &runtime).as_str(),
                    "provider": session.provider.clone().or(manifest_provider),
                    "model": model,
                    "mode": runtime.current_mode_id(),
                    "profileId": session.profile_id,
                    "createdBy": session.created_by,
                    "depth": state.mcp.depth_of(&session.id),
                    "idleMs": runtime
                        .activity_idle_at(Instant::now())
                        .map(|idle| idle.as_millis().try_into().unwrap_or(u64::MAX)),
                    // A count, and only a count: card details exist on
                    // `devboule_list_pending_permissions` alone, whose door
                    // prices them at `answer_permissions` — a `view` peer
                    // must not read a card through this snapshot.
                    "pendingPermissions": runtime
                        .permission_broker()
                        .map(|broker| broker.pending_len())
                        .unwrap_or(0),
                }))
            }
            // A live miss only falls through to the row when the miss is a
            // "no such child" — an ambiguous name or the caller itself are
            // verdicts the stored read must not soften.
            Err(error) if error.code == ErrorCode::SessionNotFound => {
                self.stored_child_status(creator_session_id, target)
            }
            Err(error) => Err(error),
        }
    }

    /// The stored half of the status snapshot: one row, by id, filtered by
    /// owner and creator in SQL — never a scan of anyone's history, and never
    /// a display name two rows could share (Paseo addresses its stored rows
    /// by id too).
    fn stored_child_status(
        &self,
        creator_session_id: &str,
        target: &str,
    ) -> Result<serde_json::Value, WireError> {
        let caller_owner = {
            let map = self.inner.lock().map_err(|_| {
                WireError::new(ErrorCode::Internal, "Session state is unavailable.")
            })?;
            let entry = map.get(creator_session_id).ok_or_else(|| {
                WireError::new(
                    ErrorCode::InvalidRequest,
                    "the calling session is not registered on this daemon",
                )
            })?;
            entry.owner().clone()
        };
        let row = match &self.journal {
            Some(journal) => {
                journal.owned_child_record(target, &caller_owner.user, creator_session_id)?
            }
            // No journal: no stored rows to fall back to, and that is the
            // honest answer rather than a read that could not happen.
            None => None,
        };
        let Some(row) = row else {
            return Err(WireError::new(
                ErrorCode::SessionNotFound,
                format!("none of your children is called '{target}'"),
            ));
        };
        let session = row.to_session();
        // A closed row reads `closed`: the row knows it was closed and not
        // why (the close reason is C1b's), so borrowing the
        // Ended-without-a-code → `failed` word would escalate a clean close
        // into a crash a supervisor then alerts on.
        let state = if row.closed {
            "closed"
        } else {
            session.state.task_state(false).as_str()
        };
        Ok(serde_json::json!({
            "agentId": session.id,
            "state": state,
            "provider": session.provider,
            "model": null,
            "mode": null,
            "profileId": session.profile_id,
            "createdBy": session.created_by,
            "depth": row.depth,
            "idleMs": crate::agent_activity::wall_now_ms()
                .saturating_sub(row.updated_at_ms),
            "pendingPermissions": 0,
        }))
    }
}
