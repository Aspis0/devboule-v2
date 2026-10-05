//! The Unix announcement gate: peer identity is the kernel uid the transport
//! minted for a local socket, so a report from the session's owner is accepted
//! (the platform refusal that stood here is gone) and another user's is not.

use devboule_protocol::{AgentActivityState, ErrorCode};

use crate::agent_report::{AgentReport, PeerIdentity};

use super::tests::{insert_live_agent, test_epoch, test_owner};
use super::*;

fn report(seq: u64) -> AgentReport {
    AgentReport {
        source: "devboule:stub".to_string(),
        agent: "stub".to_string(),
        state: AgentActivityState::Working,
        message: None,
        seq: Some(seq),
        agent_session_id: None,
        agent_session_path: None,
        session_start_source: None,
    }
}

#[test]
fn a_report_from_the_sessions_owner_is_accepted_on_unix() {
    let dir = crate::test_dirs::test_temp_dir("devboule-unix-announce");
    let registry = SessionRegistry::new(RuntimePaths::from_dir(&dir), None, test_epoch());
    let uid = crate::transport::local_uid().to_string();
    let session = "s.unix.announce".to_string();
    let _runtime = insert_live_agent(&registry, &session, test_owner(&uid, "unix-announce"));

    let owner = PeerIdentity {
        user: uid,
        pid: std::process::id(),
    };
    assert!(
        registry
            .report_agent(&session, report(1), Some(&owner))
            .expect("the owner's report is accepted"),
        "the first report is accepted"
    );

    let stranger = PeerIdentity {
        user: "another-user".to_string(),
        pid: std::process::id(),
    };
    let refused = registry
        .report_agent(&session, report(2), Some(&stranger))
        .expect_err("another user's report is refused");
    assert_eq!(refused.code, ErrorCode::Unauthorized);

    let _ = std::fs::remove_dir_all(&dir);
}
