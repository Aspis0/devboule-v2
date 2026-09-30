//! Tests for the announcement contract: peer verification, official
//! sources, field validation, and the platform peer-identity error.

use devboule_protocol::{AgentActivityState, ErrorCode};

use super::{
    is_official_agent_source, peer_identity_unavailable_on_platform, validate_announcement,
    verify_announcement_peer, AgentReport, PeerIdentity, MAX_ANNOUNCEMENT_FIELD_BYTES,
};

pub(super) fn report(seq: Option<u64>, state: AgentActivityState) -> AgentReport {
    AgentReport {
        source: "devboule:stub".to_string(),
        agent: "stub".to_string(),
        state,
        message: None,
        seq,
        agent_session_id: Some("agent-1".to_string()),
        agent_session_path: None,
        session_start_source: None,
    }
}

#[test]
fn unverifiable_peer_is_rejected_and_does_not_apply() {
    let error = verify_announcement_peer(None, "S-1-5-21-1-2-3-1001")
        .expect_err("a missing peer identity is not a verified peer");
    assert_eq!(error.code, ErrorCode::Unauthorized);
}

#[test]
fn peer_sid_mismatch_is_rejected() {
    let peer = PeerIdentity {
        user: "S-1-5-21-1-2-3-1002".to_string(),
        pid: 4242,
    };
    let error = verify_announcement_peer(Some(&peer), "S-1-5-21-1-2-3-1001")
        .expect_err("a different user SID is not the session owner");
    assert_eq!(error.code, ErrorCode::Unauthorized);
}

#[test]
fn matching_peer_sid_is_accepted() {
    let peer = PeerIdentity {
        user: "S-1-5-21-1-2-3-1001".to_string(),
        pid: 7,
    };
    verify_announcement_peer(Some(&peer), "S-1-5-21-1-2-3-1001")
        .expect("same-user peer is the authorized announcer");
}

#[test]
fn malformed_source_is_not_official() {
    assert!(
        !is_official_agent_source("freeform", "stub"),
        "a free-text source must be rejected"
    );
    assert!(
        !is_official_agent_source("devboule:claude", "codex"),
        "source must name the same agent it claims"
    );
    assert!(
        !is_official_agent_source("herdr:stub", "stub"),
        "a herdr source is not a Devboule source"
    );
}

#[test]
fn validate_rejects_malformed_source() {
    let mut item = report(Some(1), AgentActivityState::Working);
    item.source = "freeform".to_string();
    let error = validate_announcement(&item).expect_err("malformed source");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
}

#[test]
fn official_devboule_source_matches_the_agent() {
    assert!(is_official_agent_source("devboule:stub", "stub"));
    assert!(is_official_agent_source("devboule:claude", "claude"));
}

#[test]
fn overlong_field_is_rejected_by_name() {
    let mut item = report(Some(1), AgentActivityState::Working);
    item.agent = "x".repeat(MAX_ANNOUNCEMENT_FIELD_BYTES + 1);
    let error = validate_announcement(&item).expect_err("overlong agent");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(
        error.message.contains("agent"),
        "error must name the field, got {}",
        error.message
    );
    assert!(
        error
            .message
            .contains(&MAX_ANNOUNCEMENT_FIELD_BYTES.to_string()),
        "error must name the limit, got {}",
        error.message
    );
}

#[test]
fn platform_without_peer_identity_is_not_unauthorized() {
    let error = peer_identity_unavailable_on_platform();
    assert_ne!(
        error.code,
        ErrorCode::Unauthorized,
        "missing platform support is not an authorization decision"
    );
    assert!(
        error.message.to_ascii_lowercase().contains("platform"),
        "error must name the platform cause, got {}",
        error.message
    );
}
