#![cfg(windows)]
//! The hook seq lifecycle on the production announce path: `report_agent`
//! (registry lookup, peer verification, validation) driving one source
//! through a terminal restart with a fresh identity, a resume that keeps
//! its identity and arrives as `startup`/`resume` with a lower seq, two
//! interleaved lives, the reset's closed allowlist, and the headline as
//! the most recently accepted report. Replay rebuilds history rows, never
//! the gate.

use super::tests::{insert_live_agent, test_owner};
use super::*;

use crate::agent_report::{AgentReport, PeerIdentity};

/// One session's announce path: the registry entry, the peer identity the
/// wire checks, and the runtime whose headline the tests read.
struct Announce {
    dir: std::path::PathBuf,
    registry: SessionRegistry,
    runtime: Arc<SessionRuntime>,
    peer_user: String,
    session: String,
}

impl Announce {
    fn new(tag: &str) -> Self {
        let dir = crate::test_dirs::test_temp_dir("devboule-hook-lifecycle");
        let registry = SessionRegistry::new(RuntimePaths::from_dir(&dir), None);
        let peer_user = crate::security::current_user_sid().expect("current user SID");
        let session = format!("s.{tag}.1");
        let runtime = insert_live_agent(&registry, &session, test_owner(&peer_user, tag));
        Self {
            dir,
            registry,
            runtime,
            peer_user,
            session,
        }
    }

    fn send(&self, report: AgentReport) -> bool {
        self.registry
            .report_agent(
                &self.session,
                report,
                Some(&PeerIdentity {
                    user: self.peer_user.clone(),
                    pid: std::process::id(),
                }),
            )
            .expect("report_agent")
    }

    fn headline(&self) -> Option<(AgentActivityState, Option<u64>)> {
        self.runtime.hook_activity()
    }
}

impl Drop for Announce {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn report(identity: Option<&str>, seq: Option<u64>, state: AgentActivityState) -> AgentReport {
    AgentReport {
        source: "devboule:claude".to_string(),
        agent: "claude".to_string(),
        state,
        message: None,
        seq,
        agent_session_id: identity.map(str::to_string),
        agent_session_path: None,
        session_start_source: None,
    }
}

fn started(
    identity: Option<&str>,
    seq: Option<u64>,
    state: AgentActivityState,
    start: &str,
) -> AgentReport {
    let mut item = report(identity, seq, state);
    item.session_start_source = Some(start.to_string());
    item
}

#[test]
fn a_restart_with_a_fresh_identity_announces_its_own_first_seq() {
    let announce = Announce::new("restart");
    assert!(announce.send(report(Some("life-a"), Some(5), AgentActivityState::Working)));
    assert!(
        announce.send(report(Some("life-b"), Some(1), AgentActivityState::Working)),
        "a restarted agent's fresh identity counts itself: its seq 1 is its own \
         counter, not a gate held by the previous identity's seq 5"
    );
    assert_eq!(
        announce.headline(),
        Some((AgentActivityState::Working, Some(1)))
    );
}

#[test]
fn a_resume_with_the_same_identity_resets_its_seq() {
    let announce = Announce::new("resume");
    assert!(announce.send(report(
        Some("agent-x"),
        Some(7),
        AgentActivityState::Working
    )));
    assert!(
        announce.send(started(
            Some("agent-x"),
            Some(1),
            AgentActivityState::Idle,
            "resume"
        )),
        "--resume keeps the identity but the new process counts from 1: its \
         resume report with a lower seq must reset that identity's own key"
    );
    assert_eq!(
        announce.headline(),
        Some((AgentActivityState::Idle, Some(1)))
    );
}

#[test]
fn two_live_identities_interleave_without_silencing_each_other() {
    let announce = Announce::new("interleave");
    assert!(announce.send(started(
        Some("first"),
        Some(1),
        AgentActivityState::Working,
        "startup"
    )));
    assert!(announce.send(started(
        Some("second"),
        Some(1),
        AgentActivityState::Idle,
        "startup"
    )));
    assert_eq!(
        announce.headline(),
        Some((AgentActivityState::Idle, Some(1)))
    );
    assert!(
        announce.send(report(Some("first"), Some(2), AgentActivityState::Blocked)),
        "the first identity's own counter keeps advancing beside the second"
    );
    assert_eq!(
        announce.headline(),
        Some((AgentActivityState::Blocked, Some(2))),
        "the headline is the most recently accepted report — the event stream \
         and the tool answer move together"
    );
    assert!(announce.send(report(Some("second"), Some(2), AgentActivityState::Working)));
    assert_eq!(
        announce.headline(),
        Some((AgentActivityState::Working, Some(2)))
    );
}

#[test]
fn the_same_identity_without_a_session_start_stays_monotonic() {
    let announce = Announce::new("monotonic");
    assert!(announce.send(report(
        Some("agent-y"),
        Some(5),
        AgentActivityState::Working
    )));
    assert!(
        !announce.send(report(Some("agent-y"), Some(3), AgentActivityState::Idle)),
        "one identity, one gate: without a new-life start source a late lower seq stays rejected"
    );
    assert_eq!(
        announce.headline(),
        Some((AgentActivityState::Working, Some(5)))
    );
}

#[test]
fn compact_never_resets_a_live_key() {
    let announce = Announce::new("compact");
    assert!(announce.send(report(
        Some("agent-c"),
        Some(5),
        AgentActivityState::Working
    )));
    assert!(
        announce.send(started(
            Some("agent-c"),
            Some(6),
            AgentActivityState::Idle,
            "compact"
        )),
        "a compact with a higher seq is a normal accept"
    );
    assert!(
        !announce.send(report(Some("agent-c"), Some(3), AgentActivityState::Idle)),
        "compact is not a new life: the counter it left stands, so a late lower \
         seq stays rejected"
    );
    assert_eq!(
        announce.headline(),
        Some((AgentActivityState::Idle, Some(6)))
    );
}

#[test]
fn clear_never_resets_a_live_key() {
    let announce = Announce::new("clear");
    assert!(announce.send(report(
        Some("agent-l"),
        Some(5),
        AgentActivityState::Working
    )));
    assert!(
        !announce.send(started(
            Some("agent-l"),
            Some(3),
            AgentActivityState::Idle,
            "clear"
        )),
        "clear is same-process: its lower seq stays gated by the live key"
    );
    assert_eq!(
        announce.headline(),
        Some((AgentActivityState::Working, Some(5)))
    );
}

#[test]
fn select_never_resets_a_live_key() {
    let announce = Announce::new("select");
    assert!(announce.send(report(
        Some("agent-m"),
        Some(5),
        AgentActivityState::Working
    )));
    assert!(
        !announce.send(started(
            Some("agent-m"),
            Some(3),
            AgentActivityState::Idle,
            "select"
        )),
        "select is same-process: its lower seq stays gated by the live key"
    );
    assert_eq!(
        announce.headline(),
        Some((AgentActivityState::Working, Some(5)))
    );
}

#[test]
fn startup_on_every_report_keeps_the_stale_seq_gated() {
    let announce = Announce::new("alwaystart");
    assert!(announce.send(started(
        Some("agent-s"),
        Some(5),
        AgentActivityState::Working,
        "startup"
    )));
    assert!(
        announce.send(started(
            Some("agent-s"),
            Some(6),
            AgentActivityState::Working,
            "startup"
        )),
        "a higher seq never needs the reset"
    );
    assert!(
        !announce.send(report(Some("agent-s"), Some(3), AgentActivityState::Idle)),
        "the start source alone resets nothing: a stale seq without a new-life \
         report stays rejected"
    );
    assert_eq!(
        announce.headline(),
        Some((AgentActivityState::Working, Some(6)))
    );
}

#[test]
fn an_identityless_report_counts_itself_and_takes_the_headline() {
    let announce = Announce::new("carry");
    assert!(announce.send(started(
        Some("agent-z"),
        Some(1),
        AgentActivityState::Working,
        "startup"
    )));
    assert!(
        announce.send(report(None, Some(1), AgentActivityState::Blocked)),
        "an identity-less report keys on (source, None) and counts itself"
    );
    assert_eq!(
        announce.headline(),
        Some((AgentActivityState::Blocked, Some(1))),
        "the headline follows the most recently accepted report, identity or not"
    );
    assert!(announce.send(report(
        Some("agent-z"),
        Some(2),
        AgentActivityState::Working
    )));
    assert_eq!(
        announce.headline(),
        Some((AgentActivityState::Working, Some(2)))
    );
}

#[test]
fn a_late_report_takes_the_headline_until_the_live_agents_next_report() {
    let announce = Announce::new("late");
    assert!(announce.send(started(
        Some("first"),
        Some(1),
        AgentActivityState::Working,
        "startup"
    )));
    assert!(announce.send(report(Some("first"), Some(2), AgentActivityState::Working)));
    assert!(announce.send(started(
        Some("second"),
        Some(1),
        AgentActivityState::Idle,
        "startup"
    )));
    assert_eq!(
        announce.headline(),
        Some((AgentActivityState::Idle, Some(1)))
    );
    assert!(
        announce.send(report(Some("first"), Some(3), AgentActivityState::Blocked)),
        "the earlier life's counter still advances"
    );
    assert_eq!(
        announce.headline(),
        Some((AgentActivityState::Blocked, Some(3))),
        "a late old-life report shows until the live agent's next report — the \
         row heals itself and can never be pinned"
    );
    assert!(announce.send(report(Some("second"), Some(2), AgentActivityState::Working)));
    assert_eq!(
        announce.headline(),
        Some((AgentActivityState::Working, Some(2))),
        "the live agent's next report takes the headline back"
    );
}

#[test]
fn a_replayed_transcript_keeps_the_hook_headline_clear() {
    let replay = crate::journal::Replay {
        generation: 1,
        last_seq: 4,
        integrity: devboule_protocol::TranscriptIntegrity::Complete,
        event_seqs: vec![(1, 2), (1, 3)],
        event_ts_ms: vec![None; 2],
        events: vec![
            SessionEvent::AgentReported {
                seq: 2,
                source: "devboule:claude".to_string(),
                agent: "claude".to_string(),
                state: AgentActivityState::Working,
                message: None,
                report_seq: Some(5),
                agent_session_id: None,
                agent_session_path: None,
                session_start_source: None,
            },
            SessionEvent::AgentReported {
                seq: 3,
                source: "devboule:claude".to_string(),
                agent: "claude".to_string(),
                state: AgentActivityState::Idle,
                message: None,
                report_seq: Some(1),
                agent_session_id: None,
                agent_session_path: None,
                session_start_source: None,
            },
        ],
    };
    let runtime = SessionRuntime::from_replay("hook-replay".to_string(), None, replay);
    assert_eq!(
        runtime.hook_activity(),
        None,
        "replay rebuilds history rows, not the gate: the headline cannot freeze where live cleared it"
    );
}
