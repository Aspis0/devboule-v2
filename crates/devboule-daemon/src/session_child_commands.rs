//! The registry roads behind the agent-command tools: `devboule_cancel_agent`,
//! `devboule_list_pending_permissions` and `devboule_get_agent_status`.
//!
//! Scope is the creator's session id every time — the broker's bearer hands it
//! over, and no tool argument can claim one. Everything here is a read or a
//! soft interrupt: nothing kills, nothing answers a card, nothing spends a slot.

use super::*;

/// One card of the pending list and of the status snapshot, as both tools
/// serve it: the same five fields wherever a card appears, so an agent that
/// read one shape has read both.
fn card_value(
    agent_id: &str,
    card_id: String,
    request: &SessionEvent,
) -> Option<serde_json::Value> {
    let SessionEvent::PermissionRequest {
        title,
        description,
        command,
        ..
    } = request
    else {
        // The pending table only ever holds a permission request; a card that
        // is something else has no title to show a reader, so it shows nothing.
        return None;
    };
    Some(serde_json::json!({
        "agentId": agent_id,
        "cardId": card_id,
        "title": title,
        "kind": "tool",
        "excerpt": session_envelopes::card_excerpt(
            description.as_deref(),
            command.as_deref(),
            title,
        ),
    }))
}

impl super::SessionRegistry {
    /// `devboule_cancel_agent`: interrupt the current turn of one of the
    /// caller's own live children and keep the child. `Ok(true)` when a turn
    /// was running, `Ok(false)` when it was not — Paseo's `success`, where
    /// false is a fact and not an error, and nothing is touched in that case.
    pub fn interrupt_agent_child(
        &self,
        creator_session_id: &str,
        target: &str,
    ) -> Result<bool, WireError> {
        let (child_id, owner) = self.resolve_own_child(creator_session_id, target)?;
        let Some((_, runtime, _)) = self.child_view(&child_id) else {
            return Err(WireError::new(
                ErrorCode::SessionNotFound,
                format!("none of your live children is called '{target}'"),
            ));
        };
        if !runtime.is_running_turn() {
            return Ok(false);
        }
        self.interrupt(&child_id, &owner)?;
        // Paseo's cancel command resolves the agent's parked cards itself
        // rather than trusting the interrupt to do it; a provider's killer
        // drains this same table on interrupt, so the drain here is the
        // command's own promise, taken over every killer.
        if let Some(broker) = runtime.permission_broker() {
            broker.cancel_pending();
        }
        Ok(true)
    }

    /// `devboule_list_pending_permissions`: every card the caller's own live
    /// children are parked on, one flat list, ordered by child and card id.
    ///
    /// The delegation switch gates answering, never seeing: this list is read
    /// while the switch is off, and answering still runs its own checks at the
    /// answer tool. Cards of children this caller did not create are not in
    /// the scan.
    pub fn list_child_permission_cards(
        &self,
        creator_session_id: &str,
        owner: &OwnerId,
    ) -> Result<Vec<serde_json::Value>, WireError> {
        let mut cards = Vec::new();
        for entry in self.live_agent_entries(owner)? {
            if !is_child_of(entry.session.created_by.as_deref(), creator_session_id) {
                continue;
            }
            let Some(broker) = entry.runtime.permission_broker() else {
                continue;
            };
            for (card_id, request) in broker.pending_cards() {
                if let Some(card) = card_value(&entry.session.id, card_id, &request) {
                    cards.push(card);
                }
            }
        }
        Ok(cards)
    }

    /// `devboule_get_agent_status`: one child's snapshot. A live child
    /// answers from its runtime; a child that is not live falls back to its
    /// stored row, which carries no cards and no runtime facts (D4) — model
    /// and mode are not persisted, so a closed child answers `null` for them
    /// rather than a guess.
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
                let pending = runtime.permission_broker().map(|broker| {
                    broker
                        .pending_cards()
                        .into_iter()
                        .filter_map(|(card_id, request)| card_value(&session.id, card_id, &request))
                        .collect::<Vec<_>>()
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
                    "pendingPermissions": pending.unwrap_or_default(),
                }))
            }
            // A live miss only falls through to the row when the miss is a
            // "no such child" — an ambiguous name or the caller itself are
            // verdicts the stored scan must not soften.
            Err(error) if error.code == ErrorCode::SessionNotFound => {
                self.stored_child_status(creator_session_id, target)
            }
            Err(error) => Err(error),
        }
    }

    /// The stored half of the status snapshot: the caller's own children
    /// among the journal's rows, live or closed, answered from the row alone.
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
        let rows = match &self.journal {
            Some(journal) => journal
                .list_all()
                .map_err(|_| internal("Session history is unavailable."))?,
            // No journal: no stored rows to fall back to, and that is the
            // honest answer rather than a read that could not happen.
            None => Vec::new(),
        };
        let matches = rows
            .into_iter()
            .filter(|row| row.owner == caller_owner.user)
            .filter(|row| is_child_of(row.created_by.as_deref(), creator_session_id))
            // The same two doors the live resolution takes: the id, or the
            // name the human reads (display name, title beneath it).
            .filter(|row| {
                row.id == target || row.display_name.as_deref().unwrap_or(&row.title) == target
            })
            .collect::<Vec<_>>();
        match matches.len() {
            1 => {
                let row = matches.into_iter().next().expect("exactly one match");
                let session = row.to_session();
                Ok(serde_json::json!({
                    "agentId": session.id,
                    "state": session.state.task_state(false).as_str(),
                    "provider": session.provider,
                    "model": null,
                    "mode": null,
                    "profileId": session.profile_id,
                    "createdBy": session.created_by,
                    "depth": row.depth,
                    "idleMs": crate::agent_activity::wall_now_ms()
                        .saturating_sub(row.updated_at_ms),
                    "pendingPermissions": [],
                }))
            }
            0 => Err(WireError::new(
                ErrorCode::SessionNotFound,
                format!("none of your children is called '{target}'"),
            )),
            _ => Err(WireError::new(
                ErrorCode::InvalidRequest,
                format!("more than one of your children is called '{target}'; use the session id"),
            )),
        }
    }
}
