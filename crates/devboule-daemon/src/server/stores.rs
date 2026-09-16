//! Stores domain — pass-3a split of `server.rs`: the four inline store
//! domains that `dispatch_immediate` used to carry as arm bodies (tool
//! policy, agent profiles, delegation). Each function below is one moved arm
//! body, verbatim but for indentation; the routing arms stay in `dispatch.rs`.
//! The `ProviderVocabularyGet` arm stays a routing call for the same reason:
//! it already delegates straight out of the file.

use super::*;

pub(super) fn tool_policy_get(
    state: &Arc<ServerState>,
    id: u64,
    _passed: &GatePassed,
) -> DaemonMessage {
    DaemonMessage::ToolPolicy {
        id,
        policies: state.tool_policy.entries(),
    }
}

pub(super) fn tool_policy_set(
    state: &Arc<ServerState>,
    id: u64,
    provider_id: String,
    enabled: Option<bool>,
    disabled_tools: Vec<String>,
    _passed: &GatePassed,
) -> DaemonMessage {
    match state.tool_policy.set(&provider_id, enabled, disabled_tools) {
        Ok(()) => DaemonMessage::ToolPolicySetOk { id },
        // A request over a cap, or one naming a provider the daemon
        // publishes no tools for, is the caller's mistake and is reported
        // as one: retrying it would fail the same way. A write failure is
        // the daemon's, and the store kept the policy it already had.
        Err(error) => {
            let code = match error {
                crate::tool_policy::PolicyError::InvalidRequest(_) => ErrorCode::InvalidRequest,
                crate::tool_policy::PolicyError::Io(_) => ErrorCode::Io,
            };
            DaemonMessage::Error(
                WireError::new(
                    code,
                    format!("Could not save the tool policy for '{provider_id}': {error}"),
                )
                .with_id(id),
            )
        }
    }
}

pub(super) fn agent_profiles_get(
    state: &Arc<ServerState>,
    id: u64,
    _passed: &GatePassed,
) -> DaemonMessage {
    DaemonMessage::AgentProfiles {
        id,
        document: state.agent_profiles.document(),
    }
}

pub(super) fn agent_profiles_set(
    state: &Arc<ServerState>,
    id: u64,
    document: devboule_protocol::AgentProfilesDocument,
    _passed: &GatePassed,
) -> DaemonMessage {
    match state.agent_profiles.set(document) {
        Ok(()) => DaemonMessage::AgentProfilesSetOk { id },
        // A document over a cap, naming a provider the catalog does not
        // publish, or repeating an id is the caller's mistake and is
        // reported as one: retrying it would fail the same way. A write
        // failure is the daemon's, and the store kept the document it
        // already had.
        Err(error) => {
            let code = match error {
                crate::agent_profiles::ProfilesError::InvalidRequest(_) => {
                    ErrorCode::InvalidRequest
                }
                crate::agent_profiles::ProfilesError::Io(_) => ErrorCode::Io,
            };
            DaemonMessage::Error(
                WireError::new(code, format!("Could not save the agent profiles: {error}"))
                    .with_id(id),
            )
        }
    }
}

pub(super) fn delegation_get(
    state: &Arc<ServerState>,
    id: u64,
    _passed: &GatePassed,
) -> DaemonMessage {
    // The one read: the store answers with the switch and where the
    // answer came from, and nothing else in the daemon consults it
    // (the nothing-reads-it test holds that line).
    let (enabled, source) = state.delegation.get();
    DaemonMessage::DelegationState {
        id,
        enabled,
        source,
    }
}

pub(super) fn delegation_set(
    state: &Arc<ServerState>,
    id: u64,
    enabled: bool,
    _passed: &GatePassed,
) -> DaemonMessage {
    // The reply carries what the daemon stored, not an echo of the
    // request (`NOTE-a-write-that-does-not-say-what-it-stored.md`),
    // and every session-watching connection is pushed the same pair:
    // the setting is global and read once at mount by the app, so a
    // write from any surface must reach every client or a stale OFF
    // hides the control that stops delegation.
    match state.delegation.set(enabled) {
        Ok((enabled, source)) => {
            // The setting change is an audited act (§4.3): the actor
            // here is always the person at this machine — the peer
            // gate refuses the pair before this arm ever runs — and
            // the row names the act, not the value.
            if let Ok(identity) = state.device_identity() {
                state.audit(AuditRecord {
                    device_id: identity.device_id.clone(),
                    role: "local".to_string(),
                    claimed_origin: None,
                    action: "DelegationSet".to_string(),
                    session_id: None,
                    outcome: "ok".to_string(),
                });
            }
            state.broadcast_delegation(enabled, source);
            // The delegation facts ride the roster snapshot rows: the
            // cache would otherwise serve the pre-flip value until a
            // transition happened to rebuild it, so drop it and
            // re-push every watcher's roster — a changed row is what
            // makes the roster broadcast fire.
            {
                let owners: Vec<OwnerId> = state
                    .session_watchers
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .values()
                    .map(|watch| watch.owner.clone())
                    .collect::<std::collections::HashSet<_>>()
                    .into_iter()
                    .collect();
                state.sessions.invalidate_state_roster_cache();
                for owner in owners {
                    state.broadcast_session_state(&owner);
                }
            }
            DaemonMessage::DelegationSetOk {
                id,
                enabled,
                source,
            }
        }
        // There is no invalid request to a one-boolean store: the
        // only failure is the write's, and the store kept the value
        // it already had.
        Err(error) => DaemonMessage::Error(
            WireError::new(
                ErrorCode::Io,
                format!("Could not save the delegation setting: {error}"),
            )
            .with_id(id),
        ),
    }
}
