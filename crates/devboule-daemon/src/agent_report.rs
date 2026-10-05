//! Agent announcements: the report and peer types, and the announcement
//! contract — field caps, official sources, and named-pipe peer
//! verification. The per-life hook `seq` gate and the accepted headline
//! live in the child module `agent_report_state`.

use devboule_protocol::{AgentActivityState, ErrorCode, WireError};

#[path = "agent_report_state.rs"]
mod agent_report_state;

pub use agent_report_state::AgentReportState;
// Peer identity lives in transport, beside the readers that derive it;
// re-exported here so existing paths keep working.
pub use crate::transport::PeerIdentity;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentReport {
    pub source: String,
    pub agent: String,
    pub state: AgentActivityState,
    pub message: Option<String>,
    pub seq: Option<u64>,
    pub agent_session_id: Option<String>,
    pub agent_session_path: Option<String>,
    pub session_start_source: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AcceptedAgentReport {
    pub source: String,
    pub agent: String,
    pub state: AgentActivityState,
    pub message: Option<String>,
    pub seq: Option<u64>,
    pub agent_session_id: Option<String>,
    pub agent_session_path: Option<String>,
    pub session_start_source: Option<String>,
}

impl From<&AgentReport> for AcceptedAgentReport {
    fn from(report: &AgentReport) -> Self {
        Self {
            source: report.source.clone(),
            agent: report.agent.clone(),
            state: report.state,
            message: report.message.clone(),
            seq: report.seq,
            agent_session_id: report.agent_session_id.clone(),
            agent_session_path: report.agent_session_path.clone(),
            session_start_source: report.session_start_source.clone(),
        }
    }
}

/// Verify the named-pipe peer before trusting a frame that names a session.
///
/// A missing identity is a failed check, not an open door. The SID must
/// come from the OS (`GetNamedPipeClientProcessId` + `process_user_sid`),
/// never from the frame.
pub fn verify_announcement_peer(
    peer: Option<&PeerIdentity>,
    session_user: &str,
) -> Result<(), WireError> {
    let Some(peer) = peer else {
        return Err(unauthorized_peer(
            "Could not verify the announcing process identity.",
        ));
    };
    if peer.user != session_user {
        return Err(unauthorized_peer(
            "The announcing process is not the session owner.",
        ));
    }
    Ok(())
}

pub fn unauthorized_peer(message: impl Into<String>) -> WireError {
    WireError::new(ErrorCode::Unauthorized, message)
}

/// Per-field cap on announcement strings. 4 KiB is far above any legitimate
/// agent id, source, or session path and far below the 1 MiB frame cap.
pub const MAX_ANNOUNCEMENT_FIELD_BYTES: usize = 4096;
/// Distinct hook `source` values tracked per session. Official sources are
/// `devboule:<id>` for each known agent plus the test stub, so this cap is
/// the actual number of sources `validate_announcement` can admit.
pub const MAX_HOOK_SOURCES: usize = crate::provider_catalog::KNOWN_AGENTS.len() + 1;

/// Official sources have the form `devboule:<agent>` and must name the
/// same agent they claim. Adapted from herdr `is_official_agent_source`
/// (Apache-2.0, commit 3150bd9).
pub fn is_official_agent_source(source: &str, agent: &str) -> bool {
    if !is_known_announcing_agent(agent) {
        return false;
    }
    source == format!("devboule:{agent}")
}

fn is_known_announcing_agent(agent: &str) -> bool {
    agent == "stub"
        || crate::provider_catalog::KNOWN_AGENTS
            .iter()
            .any(|known| known.id == agent)
}

pub fn validate_announcement(report: &AgentReport) -> Result<(), WireError> {
    check_field("source", &report.source)?;
    check_field("agent", &report.agent)?;
    if let Some(value) = &report.message {
        check_field("message", value)?;
    }
    if let Some(value) = &report.agent_session_id {
        check_field("agentSessionId", value)?;
    }
    if let Some(value) = &report.agent_session_path {
        check_field("agentSessionPath", value)?;
    }
    if let Some(value) = &report.session_start_source {
        check_field("sessionStartSource", value)?;
    }
    if report.source.trim().is_empty() || report.agent.trim().is_empty() {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            "Agent announcement requires source and agent.",
        ));
    }
    if !is_official_agent_source(&report.source, &report.agent) {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            format!(
                "Agent announcement source '{}' is not an official Devboule source for agent '{}'.",
                report.source, report.agent
            ),
        ));
    }
    Ok(())
}

fn check_field(name: &str, value: &str) -> Result<(), WireError> {
    if value.len() > MAX_ANNOUNCEMENT_FIELD_BYTES {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            format!(
                "Agent announcement field '{name}' exceeds {MAX_ANNOUNCEMENT_FIELD_BYTES} bytes"
            ),
        ));
    }
    Ok(())
}

/// Used when this platform cannot identify a named-pipe peer. Must not be
/// reported as authorization failure: we did not decide the peer is the
/// wrong user, we could not tell who they are.
#[cfg_attr(windows, allow(dead_code))]
pub fn peer_identity_unavailable_on_platform() -> WireError {
    WireError::new(
        ErrorCode::Unimplemented,
        "Peer identity is not available on this platform.",
    )
}

/// herdr `normalize_session_start_source`: unknown values become `None`,
/// they do not reject the rest of the report.
pub fn normalize_session_start_source(value: Option<String>) -> Option<String> {
    match value.as_deref().map(str::trim) {
        Some(
            source @ ("startup" | "resume" | "clear" | "compact" | "branch" | "new" | "fork"
            | "select"),
        ) => Some(source.to_string()),
        _ => None,
    }
}

#[cfg(test)]
#[path = "agent_report_tests.rs"]
mod tests;
