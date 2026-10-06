//! What a paired send leaves behind and what it is refused for: the mode that
//! does not act, and the audit row for each decision.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::json;

use super::always_card_road_tests::{
    answer, far_end, live_session, route, send_call, the_card, SESSION,
};
use crate::server::ServerState;

/// A plan or read-only session does not act: the paired send is refused with
/// the mode's own sentence before any card is raised and before anything is
/// dialled.
#[test]
fn a_plan_mode_session_cannot_send_to_a_paired_device() {
    let state = ServerState::new("always-card-plan".to_string());
    let runtime = live_session(&state, "plan");
    let requests = far_end(&state);

    let reply = route(&state, send_call("hello")).join().expect("the call");

    assert_eq!(
        reply.pointer("/result/isError"),
        Some(&json!(true)),
        "{reply}"
    );
    assert!(
        reply.to_string().contains("plan mode"),
        "the mode's sentence: {reply}"
    );
    assert!(
        runtime
            .permission_broker()
            .expect("the test broker")
            .test_pending_ids()
            .is_empty(),
        "no card for a session that does not act"
    );
    assert!(
        requests.recv_timeout(Duration::from_millis(500)).is_err(),
        "nothing was dialled at all, not even the roster read"
    );
}

/// The journal's rows for the session's calls, polled until the writer lands one.
fn audit_rows(state: &Arc<ServerState>, wanted: usize) -> Vec<(String, String)> {
    let path = state.paths.journal_file();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let connection = rusqlite::Connection::open(&path).expect("raw journal");
        let mut statement = connection
            .prepare("SELECT action, outcome FROM audit WHERE session_id = ?1 ORDER BY rowid")
            .expect("audit query");
        let rows: Vec<(String, String)> = statement
            .query_map([SESSION], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("audit rows")
            .filter_map(Result::ok)
            .collect();
        if rows.len() >= wanted || Instant::now() >= deadline {
            return rows;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Approving and refusing both leave a row naming the device, the session and
/// the length — and neither carries what was said.
#[test]
fn a_paired_send_writes_an_audit_row_for_each_decision_without_the_text() {
    let state = ServerState::new("always-card-audit".to_string());
    let runtime = live_session(&state, "bypassPermissions");
    let _requests = far_end(&state);

    let approved = route(&state, send_call("a private instruction"));
    let (id, _) = the_card(&runtime);
    answer(&runtime, &id, "once");
    approved.join().expect("the approved call");
    let refused = route(&state, send_call("another one"));
    let (id, _) = the_card(&runtime);
    answer(&runtime, &id, "deny");
    refused.join().expect("the refused call");

    let rows = audit_rows(&state, 2);
    let tool = crate::provider_catalog::MCP_SEND_MESSAGE_TOOL;
    assert_eq!(
        rows,
        [
            (
                tool.to_string(),
                "approved; device dev-far; session s.far.target; 21 characters".to_string()
            ),
            (
                tool.to_string(),
                "denied; device dev-far; session s.far.target; 11 characters".to_string()
            ),
        ]
    );
    let everything = format!("{rows:?}");
    assert!(!everything.contains("private") && !everything.contains("another"));
}
