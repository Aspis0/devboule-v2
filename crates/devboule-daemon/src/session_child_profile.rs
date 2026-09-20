//! The move road's named phases, split out of
//! `SessionRegistry::set_agent_child_profile` (a rewrite, not a move: the
//! characterisation tests in `session_child_profile_tests.rs` are its
//! proof). The method stays in `session.rs` as the thin sequence: resolve
//! the caller's own live child, resolve the profile, refuse a child whose
//! manifest has not arrived, ask the mode, ask the model, record.

use super::*;

/// Whether the child's own provider has reported a session manifest. A move
/// is only judgeable against what the child advertises; the absence of a
/// manifest is its own refusal ("the daemon cannot say yet"), never "the
/// mode is unavailable".
pub(super) fn manifest_arrived(runtime: &SessionRuntime) -> bool {
    runtime.session_manifest().is_some()
}

/// Whether the model ask has anything to ask: a child already running the
/// profile's model with no thinking option to deliver asks nothing — there
/// is no ask to make. Every other combination is asked on the provider's
/// own wire.
pub(super) fn model_ask_needed(manifest: Option<&SessionEvent>, facts: &ChildProfileFacts) -> bool {
    let current_model_id = manifest.and_then(|event| match event {
        SessionEvent::SessionManifest {
            current_model_id, ..
        } => current_model_id.as_deref(),
        _ => None,
    });
    current_model_id != Some(facts.model.as_str()) || facts.thinking_option_id.is_some()
}

impl super::SessionRegistry {
    /// Checks 1 and 2 of the move road: the caller's row (its owner scopes
    /// the scan below; the registry is the only place "mine" is a fact), the
    /// refusal of the caller itself, and the resolution of `target` by id or
    /// display name among the caller's own live children — visible, and
    /// `created_by` equals the caller. A sibling, a grandchild, the caller
    /// itself, a human-started session and an invented or dead name each get
    /// the sentence that case earns; a name two live children share is
    /// refused ambiguous rather than resolved to one of them.
    ///
    /// The `matches` scan and the `not_child` probe share ONE `inner` guard
    /// on purpose: split into two locks, the map could change between them
    /// and flip the refusal between "exists but not your child" and "no such
    /// child". The caller-owner lookup is its own scope block so its guard
    /// dies before the scan locks again — `std::sync::Mutex` is not
    /// reentrant, and a guard held across the scan would self-deadlock.
    pub(super) fn resolve_own_live_child(
        &self,
        creator_session_id: &str,
        target: &str,
    ) -> Result<(Session, Arc<SessionRuntime>, OwnerId), String> {
        let caller_owner = {
            let map = self
                .inner
                .lock()
                .map_err(|_| "session state is unavailable".to_string())?;
            let entry = map.get(creator_session_id).ok_or_else(|| {
                "the calling session is not registered on this daemon".to_string()
            })?;
            entry.owner().clone()
        };
        if target == creator_session_id {
            return Err("a session is not its own child; name a session you created".to_string());
        }
        // The name a child is addressed by, the same one the roster shows:
        // the display name a creation gave it, or the title beneath it.
        let display = |session: &Session| {
            session
                .display_name
                .clone()
                .unwrap_or_else(|| session.title.clone())
        };
        // The scan is read-only; nothing is asked of any provider until a
        // profile and a mode have both been agreed.
        let (child_session, child_runtime, child_owner) = {
            let map = self
                .inner
                .lock()
                .map_err(|_| "session state is unavailable".to_string())?;
            let mut matches: Vec<(Session, Arc<SessionRuntime>, OwnerId)> = map
                .values()
                .filter(|entry| entry.owner().user == caller_owner.user)
                .filter_map(|entry| {
                    let live = entry.as_peer_visible()?;
                    Some((
                        live_session_view(live),
                        Arc::clone(&live.runtime),
                        entry.owner().clone(),
                    ))
                })
                .filter(|(session, _, _)| {
                    is_child_of(session.created_by.as_deref(), creator_session_id)
                })
                .filter(|(session, _, _)| session.id == target || display(session) == target)
                .collect();
            match matches.len() {
                1 => Ok(matches.pop().expect("exactly one match")),
                0 => {
                    // What this owner's own live roster distinguishes is
                    // distinguished: a live session of theirs that is not the
                    // caller's child is told what it is. Everything else — an
                    // invented name, a dead child, a stranger's session — is
                    // one refusal, because the daemon cannot and must not say
                    // which.
                    let not_child = map.values().any(|entry| {
                        entry.owner().user == caller_owner.user
                            && entry.as_peer_visible().is_some_and(|live| {
                                let session = live_session_view(live);
                                !is_child_of(session.created_by.as_deref(), creator_session_id)
                                    && (session.id == target || display(&session) == target)
                            })
                    });
                    if not_child {
                        Err(format!(
                            "'{target}' is not your child; only a session you created can be moved onto a profile"
                        ))
                    } else {
                        Err(format!(
                            "none of your live children is called '{target}'; devboule_list_agents names them"
                        ))
                    }
                }
                _ => Err(format!(
                    "more than one of your live children is called '{target}'; use the session id"
                )),
            }
        }?;
        Ok((child_session, child_runtime, child_owner))
    }

    /// The partial arm of check 5: the mode landed, the model ask did not.
    /// The ratchet still fires — the child has been able to run in that
    /// mode, and that cannot be un-lived — but **no** profile change is
    /// recorded, and the answer says exactly what stands.
    pub(super) fn record_partial_move(
        &self,
        child_session: &Session,
        facts: &ChildProfileFacts,
        model_refusal: &str,
    ) -> Result<(), String> {
        self.record_child_profile_move(
            &child_session.id,
            &child_session.kind,
            &facts.mode_id,
            None,
        );
        Err(format!(
            "the mode was switched to '{}', but the model ask was refused: {}. the child runs in mode '{}' on its previous model, and no profile change is recorded",
            facts.mode_id, model_refusal, facts.mode_id
        ))
    }
}
