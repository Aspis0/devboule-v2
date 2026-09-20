//! The delegated answer's named phases, split out of
//! `SessionRegistry::answer_child_permission` (a rewrite, not a move: the
//! characterisation tests in `session_child_permission_tests.rs` are its
//! proof). The method stays in `session.rs` as the thin sequence: resolve
//! the caller, find the card's holder among that owner's sessions, refuse
//! an ambiguous id before any broker check, then hand the closures to
//! `answer_delegated_on` and clear the child's attention on `Ok`.

use super::*;

/// Check 5's closure body: a creator whose stored origin is a paired device
/// answers only what the peer gate allows — judged with the same
/// `peer_allows` function the dispatcher uses, on the same wire message the
/// broker door names for this tool (`SessionPermissionRespond`), never a
/// copy of its conclusions. A local creator is the person at this machine's
/// own agent and never reaches for a device; a peer-shaped row without a
/// device or role is an unknown, and the unknown never renders as the
/// benign one. `device_caps` is consulted only after the shape is known, so
/// a device is asked for its caps exactly when the gate needs them.
pub(super) fn child_answer_caps_refusal(
    origin: &SessionOrigin,
    device_caps: &dyn Fn(&str) -> Vec<String>,
) -> Result<(), String> {
    if origin.kind != SessionOriginKind::Peer {
        return Ok(());
    }
    let (Some(device_id), Some(role)) = (origin.device_id.as_deref(), origin.role) else {
        return Err("the calling session's origin is unknown; the card stays pending".to_string());
    };
    let caps = device_caps(device_id);
    let request = devboule_protocol::ClientMessage::SessionPermissionRespond {
        id: 0,
        session_id: String::new(),
        subscription_id: 0,
        request_id: String::new(),
        outcome: devboule_protocol::PermissionOutcome::Deny,
        option_id: None,
        idempotency_key: None,
    };
    if let crate::peer_policy::PeerDecision::Deny(reason) =
        crate::peer_policy::peer_allows(role, &caps, &request)
    {
        return Err(format!(
            "{}; the card stays pending",
            crate::peer_policy::capability_refusal_message(reason)
        ));
    }
    Ok(())
}

impl super::SessionRegistry {
    /// P1: the caller's own row — its owner scopes the card scan, its origin
    /// decides whether the capability check applies. A **bare** `map.get` on
    /// purpose: the MCP registration guarantees the caller is live, so an
    /// absent row is a refusal, not a panic, and any row — a transcript
    /// among them — may carry the caller's identity. Unifying this read
    /// with the scan's live-only probe below would change who is allowed
    /// to answer.
    pub(super) fn caller_identity(
        &self,
        creator_session_id: &str,
    ) -> Result<(String, SessionOrigin), String> {
        let map = self
            .inner
            .lock()
            .map_err(|_| "session state is unavailable".to_string())?;
        let entry = map
            .get(creator_session_id)
            .ok_or_else(|| "the calling session is not registered on this daemon".to_string())?;
        Ok((entry.owner().user.clone(), entry.to_session().origin))
    }

    /// Check 3's closure body: a resolved card is a row in the ledger the
    /// replay reads back. A journal that cannot answer reads `false` — the
    /// sentence becomes "unknown", which is inert in both cases. No
    /// registry lock: the ledger is the journal's, not the map's.
    pub(super) fn permission_already_recorded(&self, request_id: &str) -> bool {
        self.journal
            .as_ref()
            .map(|journal| journal.permission_was_recorded(request_id).unwrap_or(false))
            .unwrap_or(false)
    }

    /// Check 4's closure body: the card's session is a **live child of the
    /// caller** — the view exists, and `created_by` equals the bearer's
    /// session. A sibling, a grandchild, a human-started session or a dead
    /// one fails here without learning which session owns the card. The
    /// resolved id is what the answer remembers for its attention tail.
    pub(super) fn child_answer_target(
        &self,
        card_session: &str,
        creator_session_id: &str,
    ) -> Result<String, String> {
        let Some((session, _runtime, _owner)) = self.child_view(card_session) else {
            return Err(format!(
                "permission card {card_session} is not pending on one of your live sessions"
            ));
        };
        if !is_child_of(session.created_by.as_deref(), creator_session_id) {
            return Err(format!(
                "permission card {card_session} belongs to a session that is not your child; it stays pending for whoever may answer it"
            ));
        }
        Ok(card_session.to_string())
    }

    /// P6: the child may have been waiting in attention for this answer —
    /// the card that just resolved was the reason it was raised. Called
    /// only after `answer_delegated_on` returned `Ok`; a refused answer
    /// must leave the attention up. Holds no registry lock of its own:
    /// `child_view` takes and drops it, and the transition's journal read
    /// runs inside that helper's own hold.
    pub(super) fn clear_child_attention_after_answer(&self, child: &str) {
        if let Some((_session, runtime, owner)) = self.child_view(child) {
            if runtime.clear_attention() {
                self.notify_session_transition(&owner, child);
            }
        }
    }

    /// P2: locate the broker that holds the card. The scan is read-only:
    /// locating is not answering, and every check still runs in the chain.
    ///
    /// Owner-scoped, so a card that exists on one of the owner's sessions
    /// but not on a child's stays "found" and the chain's child check
    /// answers it with the not-your-child sentence — a state distinct
    /// from "unknown card" (§1.5's three states). But the id is
    /// provider-chosen and carries no session qualifier, so the holder
    /// that answers must be the caller's own child: a child holder is
    /// preferred over a non-child one, and more than one child holding
    /// the same id is refused ambiguous rather than answered against
    /// whichever session the map yields first. That preference is safe
    /// only because the ambiguity count is refused before the chain runs;
    /// do not reorder the two.
    pub(super) fn find_card_holder(
        &self,
        owner_user: &str,
        creator_session_id: &str,
        card_id: &str,
    ) -> Result<
        (
            Option<std::sync::Arc<permission_broker::PermissionBroker>>,
            usize,
        ),
        String,
    > {
        let map = self
            .inner
            .lock()
            .map_err(|_| "session state is unavailable".to_string())?;
        let mut found: Option<std::sync::Arc<permission_broker::PermissionBroker>> = None;
        let mut found_is_child = false;
        let mut child_holders: usize = 0;
        for entry in map
            .values()
            .filter(|entry| entry.owner().user == owner_user)
        {
            let Some(broker) = entry.runtime().permission_broker() else {
                continue;
            };
            if !matches!(
                broker.peek_delegated(card_id),
                permission_broker::DelegatedPeek::Found { .. }
            ) {
                continue;
            }
            // The child probe is `as_peer_visible()` — live rows only —
            // while `caller_identity` answers for any row; the asymmetry
            // is the spec, not an accident to unify.
            let is_child = entry.as_peer_visible().is_some_and(|live| {
                is_child_of(live.metadata.created_by.as_deref(), creator_session_id)
            });
            if is_child {
                child_holders += 1;
            }
            if found.is_none() || (is_child && !found_is_child) {
                found = Some(std::sync::Arc::clone(&broker));
                found_is_child = is_child;
            }
        }
        Ok((found, child_holders))
    }
}
