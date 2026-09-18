//! Tests for `peer_roster.rs`, kept out of the production file:
//! the dial fixtures and journal readers are test-only weight.

use super::*;
use crate::peer_policy::{TransportBinding, CAP_VIEW};

fn remote_conn(paired_by_user: Option<String>) -> Arc<ConnHandle> {
    ConnHandle::with_peer_caps(
        0,
        None,
        Some(ConnPeer::Remote {
            device_id: "dev-far".to_string(),
            role: PeerRole::Daemon,
            paired_by_user,
            binding: TransportBinding::tailnet("nstable", "node", "user@example.com"),
        }),
        vec![CAP_VIEW.to_string()],
    )
}

/// The roster answers with the pairing user's live agents only: another
/// local user's agents are absent, and each entry is the narrow shape —
/// id, name, provider, model, state, depth — never a `Session`.
#[test]
fn the_roster_answers_the_pairing_user_only() {
    let state = ServerState::new("peer-roster-scope".into());
    let paired = OwnerId::new("S-1-5-21-paired", "claude").expect("owner");
    let other = OwnerId::new("S-1-5-21-other", "claude").expect("owner");
    crate::session::insert_test_live_agent(&state.sessions, "s.a.1", paired.clone());
    crate::session::insert_test_live_agent(&state.sessions, "s.a.2", paired.clone());
    crate::session::insert_test_live_agent(&state.sessions, "s.b.1", other.clone());
    let conn = remote_conn(Some(paired.user.clone()));

    let reply = peer_agents_reply(&state, 7, &conn, &other);
    match reply {
        DaemonMessage::PeerAgents { id, agents, .. } => {
            assert_eq!(id, 7);
            assert_eq!(
                agents
                    .iter()
                    .map(|a| a.session_id.as_str())
                    .collect::<Vec<_>>(),
                ["s.a.1", "s.a.2"],
                "the pairing user's agents, in id order: {agents:?}"
            );
            let first = &agents[0];
            assert_eq!(first.name, "Agent", "the title is the fallback name");
            assert_eq!(first.provider.as_deref(), Some("test-agent"));
            assert_eq!(first.model, None);
            assert_eq!(
                first.state,
                devboule_protocol::AgentTaskState::Submitted,
                "live with no running turn is submitted"
            );
            assert_eq!(first.depth, 0);
        }
        other => panic!("expected PeerAgents, got {other:?}"),
    }
}

/// A connection whose row recorded no pairing user answers — but as
/// `unscoped`, never as an empty roster: "this device cannot scope its
/// roster" and "that user has no live agents" are different facts, and
/// `paired_by_user` is always `None` on a platform without user ids.
#[test]
fn an_absent_pairing_user_answers_unscoped_not_empty() {
    let state = ServerState::new("peer-roster-absent".into());
    let paired = OwnerId::new("S-1-5-21-paired", "claude").expect("owner");
    crate::session::insert_test_live_agent(&state.sessions, "s.a.1", paired.clone());
    let conn = remote_conn(None);

    let reply = peer_agents_reply(&state, 8, &conn, &paired);
    let rendered = serde_json::to_value(&reply).expect("wire value");
    assert_eq!(
        rendered["scope"], "unscoped",
        "no pairing user is a scope verdict of its own: {rendered}"
    );
    assert_eq!(
        rendered["agents"].as_array().map(Vec::len),
        Some(0),
        "an unscoped answer carries no roster"
    );
    assert_eq!(rendered["id"], 8);
}

/// A local pipe answers scoped to its own user, and says so.
#[test]
fn a_local_connection_answers_its_own_user_scoped() {
    let state = ServerState::new("peer-roster-local".into());
    let local = OwnerId::new("S-1-5-21-local", "claude").expect("owner");
    crate::session::insert_test_live_agent(&state.sessions, "s.l.1", local.clone());
    let conn = ConnHandle::with_peer(0, None);

    let reply = peer_agents_reply(&state, 9, &conn, &local);
    let rendered = serde_json::to_value(&reply).expect("wire value");
    assert_eq!(rendered["scope"], "local_user", "{rendered}");
    assert_eq!(
        rendered["agents"].as_array().map(Vec::len),
        Some(1),
        "{rendered}"
    );
}

/// Serving the roster to a paired device is the one read that discloses
/// the pairing user's whole live surface, so it lands in the audit table
/// beside the peer denials: who read, from which device and role. The
/// outcome carries the scope verdict — `ok` for a served roster,
/// `unscoped` for the empty answer a device gives when its pairing recorded
/// no user — so a person reading the table later can tell "saw the roster"
/// from "was told nothing".
#[test]
fn a_roster_served_to_a_peer_is_audited_with_its_scope() {
    let state = ServerState::new("peer-roster-audit".into());
    let paired = OwnerId::new("S-1-5-21-paired", "claude").expect("owner");
    crate::session::insert_test_live_agent(&state.sessions, "s.a.1", paired.clone());

    let served = remote_conn(Some(paired.user.clone()));
    let reply = peer_agents_reply(&state, 10, &served, &paired);
    assert!(
        matches!(reply, DaemonMessage::PeerAgents { .. }),
        "{reply:?}"
    );

    let unscoped = remote_conn(None);
    let reply = peer_agents_reply(&state, 11, &unscoped, &paired);
    assert!(
        matches!(reply, DaemonMessage::PeerAgents { .. }),
        "{reply:?}"
    );

    let runtime_dir = state.sessions.runtime_dir().to_path_buf();
    let connection =
        rusqlite::Connection::open(runtime_dir.join("journal.db")).expect("journal db");
    let mut statement = connection
        .prepare("SELECT action, device_id, role, outcome FROM audit ORDER BY id")
        .expect("prepare");
    let rows: Vec<(String, String, String, String)> = statement
        .query_map([], |row| {
            Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
        })
        .expect("query")
        .map(Result::unwrap)
        .collect();
    assert!(
        rows.contains(&(
            "peer_agents_list".to_string(),
            "dev-far".to_string(),
            "daemon".to_string(),
            "ok".to_string()
        )),
        "the served roster read is audited ok: {rows:?}"
    );
    assert!(
        rows.contains(&(
            "peer_agents_list".to_string(),
            "dev-far".to_string(),
            "daemon".to_string(),
            "unscoped".to_string()
        )),
        "the unscoped answer is audited as unscoped, never as a served read: {rows:?}"
    );
}

/// A local pipe's read is nobody's disclosure but its own: no audit row,
/// exactly like every other local read.
#[test]
fn a_local_roster_read_is_not_audited() {
    let state = ServerState::new("peer-roster-local-audit".into());
    let local = OwnerId::new("S-1-5-21-local", "claude").expect("owner");
    crate::session::insert_test_live_agent(&state.sessions, "s.l.1", local.clone());
    let conn = ConnHandle::with_peer(0, None);

    let reply = peer_agents_reply(&state, 11, &conn, &local);
    assert!(
        matches!(reply, DaemonMessage::PeerAgents { .. }),
        "{reply:?}"
    );

    let runtime_dir = state.sessions.runtime_dir().to_path_buf();
    let connection =
        rusqlite::Connection::open(runtime_dir.join("journal.db")).expect("journal db");
    let mut statement = connection
        .prepare("SELECT action FROM audit")
        .expect("prepare");
    let rows: Vec<String> = statement
        .query_map([], |row| row.get(0))
        .expect("query")
        .map(Result::unwrap)
        .collect();
    assert!(
        !rows.iter().any(|action| action == "peer_agents_list"),
        "a local read writes no roster audit row: {rows:?}"
    );
}
