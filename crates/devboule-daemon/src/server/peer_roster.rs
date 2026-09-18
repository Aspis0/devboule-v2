//! The responder half of the peer agent roster: answer `PeerAgentsList`
//! with the agents this daemon is running **now**, scoped to the user who
//! approved the pairing.
//!
//! The scope anchor is `paired_by_user` — written by this daemon at pairing
//! time, never received from the peer and never trusted from a frame — so
//! the far machine decides whose roster it exposes using a fact it wrote.
//! A connection with no pairing user answers `unscoped`, its own third
//! state: not "no agents". Entries carry the narrow `PeerAgent` shape,
//! never protocol `Session`: a filesystem path has nothing to do with
//! naming an agent.

use devboule_protocol::{PeerAgent, PeerRosterScope, SessionEvent};

use super::*;

/// The live roster for the connection's pairing user, as
/// `DaemonMessage::PeerAgents`. A read, and a liveness snapshot: nothing is
/// stored, and a session id in it is unique only within this daemon.
pub(super) fn peer_agents_reply(
    state: &Arc<ServerState>,
    id: u64,
    conn: &Arc<ConnHandle>,
    owner: &OwnerId,
) -> DaemonMessage {
    // The roster belongs to the user who approved the pairing — a fact this
    // daemon wrote into its own `peers` row at pairing time and never
    // received from the peer. No pairing user on the connection, no roster
    // and no guess: `unscoped` is its own answer, never an empty "no agents"
    // — the row may predate user recording, or the platform may have no user
    // ids at all. A local pipe reads its own user, and says so.
    let (scope, scope_user, caller_device, caller_role) = match &conn.conn_peer {
        Some(ConnPeer::Remote {
            paired_by_user: Some(user),
            device_id,
            role,
            ..
        }) => (
            PeerRosterScope::PairingUser,
            Some(user.clone()),
            Some(device_id.clone()),
            Some(*role),
        ),
        Some(ConnPeer::Remote {
            paired_by_user: None,
            device_id,
            role,
            ..
        }) => (
            PeerRosterScope::Unscoped,
            None,
            Some(device_id.clone()),
            Some(*role),
        ),
        _ => (
            PeerRosterScope::LocalUser,
            Some(owner.user.clone()),
            None,
            None,
        ),
    };
    // The one read that discloses the pairing user's whole live surface is a
    // fact the audit table keeps, beside the peer denials: who read, from
    // which device and role. Every remote attempt lands exactly one row,
    // written where the outcome is known — never before the fallible work
    // below, where a failure would leave a row claiming a disclosure that
    // did not happen. The outcome is the scope verdict, not a bare `ok`:
    // an `unscoped` answer disclosed nothing, and `error` disclosed nothing
    // for a different reason. The action is the request's own name, the
    // same spelling the gate writes for its refusals, so one query reads
    // the whole pair. A local pipe's read is its own business, like every
    // other local read.
    let audit = |outcome: &str| {
        if let (Some(device_id), Some(role)) = (&caller_device, &caller_role) {
            state.audit(AuditRecord {
                device_id: device_id.clone(),
                role: role.as_str().to_string(),
                claimed_origin: None,
                action: "PeerAgentsList".to_string(),
                session_id: None,
                outcome: outcome.to_string(),
            });
        }
    };
    let Some(scope_user) = scope_user else {
        audit(crate::mcp_peer_agents::PEER_AGENTS_UNSCOPED);
        return DaemonMessage::PeerAgents {
            id,
            agents: Vec::new(),
            scope: PeerRosterScope::Unscoped,
        };
    };
    let scope_owner = match OwnerId::new(scope_user, "roster") {
        Ok(scope_owner) => scope_owner,
        Err(error) => {
            audit(crate::mcp_peer_agents::PEER_AGENTS_ERROR);
            return DaemonMessage::Error(WireError::new(ErrorCode::Internal, error).with_id(id));
        }
    };
    let entries = match state.sessions.live_agent_entries(&scope_owner) {
        Ok(entries) => entries,
        Err(error) => {
            audit(crate::mcp_peer_agents::PEER_AGENTS_ERROR);
            return DaemonMessage::Error(error.with_id(id));
        }
    };
    audit(crate::mcp_peer_agents::PEER_AGENTS_OK);
    let agents = entries
        .iter()
        .map(|entry| peer_agent_value(state, entry))
        .collect();
    DaemonMessage::PeerAgents { id, agents, scope }
}

/// One narrow roster entry: the fields that name and place the agent,
/// nothing path-like. The source is `live_agent_entries` — live `Live` and
/// `Silent` MCP-capable entries only, never journal rows — and the depth is
/// read from the same map the local roster reads.
fn peer_agent_value(state: &Arc<ServerState>, entry: &crate::session::LiveAgentEntry) -> PeerAgent {
    let manifest = entry.runtime.session_manifest();
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
    PeerAgent {
        session_id: entry.session.id.clone(),
        name: entry
            .session
            .display_name
            .clone()
            .unwrap_or_else(|| entry.session.title.clone()),
        provider: entry.session.provider.clone().or(manifest_provider),
        model,
        state: crate::session::roster_task_state(&entry.session, &entry.runtime),
        depth: state.mcp.depth_of(&entry.session.id),
    }
}

#[cfg(test)]
#[path = "peer_roster_tests.rs"]
mod tests;
