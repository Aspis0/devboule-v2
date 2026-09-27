//! A secret answer's boundary: the provider's frame and every forbidden store.

use super::super::event_pull::ConnHandle;
use super::{permission_path, PermissionBroker, PermissionSender};
use crate::journal::Journal;
use crate::session::SessionRuntime;
use devboule_protocol::{
    PermissionOutcome, PermissionQuestion, PermissionQuestionOption, PermissionRequestKind,
    SessionEvent, SessionKind, SessionOrigin,
};
use rusqlite::Connection;
use std::sync::{Arc, Mutex};

/// A secret answer reaches the provider's reply frame and no forbidden
/// store. This harness has no provider client, so no inbound echo exists;
/// in a live Claude session the journalled `tool_result` line does carry
/// the answer — the journal is an allowed store for the words, and what
/// this pins is every store the owner's rule forbids: `events`,
/// `permissions`, `audit`, and the live notice and notification lane.
#[test]
fn secret_answer_reaches_the_provider_but_no_forbidden_store() {
    let secret = "the cellar combination is 40-11-07";
    let path = permission_path("transcript-secret");
    let _ = std::fs::remove_file(&path);
    let journal = Arc::new(Journal::open(&path).expect("journal"));
    let sent = Arc::new(Mutex::new(Vec::new()));
    let sent_for_sender = Arc::clone(&sent);
    let sender: Arc<PermissionSender> = Arc::new(move |id, result| {
        sent_for_sender
            .lock()
            .expect("sent lock")
            .push((id, result));
        Ok(())
    });
    let broker = PermissionBroker::with_sender(sender);
    let session_id = "s.permission.question-transcript-secret".to_string();
    journal
        .upsert_blocking(crate::journal::new_session_record(
            session_id.clone(),
            "owner",
            None,
            SessionKind::Acp,
            "question transcript test",
        ))
        .expect("session row");
    let runtime = SessionRuntime::for_acp(
        session_id.clone(),
        Some(Arc::clone(&journal)),
        Arc::clone(&broker),
    );
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach");
    conn.track_with_agent_replay(
        session_id.as_str(),
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    let request = SessionEvent::PermissionRequest {
        tool_call_id: "question-transcript-secret".to_string(),
        title: "Which colour should I paint the fence?".to_string(),
        description: None,
        command: None,
        args: None,
        cwd: None,
        env: None,
        options: Vec::new(),
        is_chooser: None,
        kind: Some(PermissionRequestKind::Question),
        plan: None,
        questions: Some(vec![PermissionQuestion {
            question: "Which colour should I paint the fence?".to_string(),
            header: None,
            options: vec![PermissionQuestionOption {
                label: "Green".to_string(),
                description: None,
            }],
            multi_select: false,
            allow_other: Some(true),
            secret: Some(true),
        }]),
        origin: SessionOrigin::local(),
        create_agent: None,
    };
    broker.register(304, request, &runtime).expect("register");
    broker
        .respond_with_option(
            "question-transcript-secret",
            PermissionOutcome::AllowOnce,
            None,
            Some(secret.to_string()),
        )
        .expect("secret answer");
    assert_eq!(
        sent.lock().expect("sent lock")[0].1["outcome"]["answer"],
        secret,
        "the provider still gets its answer"
    );
    journal.flush().expect("journal flush");
    for event in conn.pull_events() {
        let text = serde_json::to_string(&event.envelope.event).expect("live event text");
        assert!(
            !text.contains(secret),
            "no live notice or notification carries the secret answer"
        );
    }
    let conn = Connection::open(&path).expect("inspect journal");
    let mut transcript = 0;
    {
        let mut event_stmt = conn
            .prepare("SELECT kind, payload FROM events")
            .expect("events");
        let event_rows = event_stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .expect("event rows");
        for row in event_rows {
            let (kind, payload) = row.expect("event row");
            let text = String::from_utf8_lossy(&payload);
            assert!(
                !text.contains(secret),
                "no {kind} row holds the secret answer"
            );
            if kind == "agent_report" {
                if let Ok(SessionEvent::AgentToolUpdate { text, .. }) =
                    serde_json::from_slice::<SessionEvent>(&payload)
                {
                    if text.as_deref().unwrap_or_default().contains("Which colour") {
                        transcript += 1;
                        assert!(text.as_deref().unwrap_or_default().contains("(hidden)"));
                    }
                }
            }
        }
    }
    assert_eq!(transcript, 1, "the question still leaves its row");
    let payload: Vec<u8> = conn
        .query_row(
            "SELECT payload FROM permissions WHERE session_id = ?1 AND request_id = ?2",
            [&session_id, "question-transcript-secret"],
            |row| row.get(0),
        )
        .expect("permission row");
    let payload = String::from_utf8_lossy(&payload);
    assert!(!payload.contains(secret));
    assert!(payload.contains("Which colour should I paint the fence?"));
    {
        let mut audit_stmt = conn
            .prepare(
                "SELECT device_id, role, claimed_origin, action, session_id, outcome FROM audit",
            )
            .expect("audit");
        let audit_rows = audit_stmt
            .query_map([], |row| {
                Ok(format!(
                    "{}|{}|{}|{}|{}|{}",
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?.unwrap_or_default(),
                    row.get::<_, String>(5)?,
                ))
            })
            .expect("audit rows");
        for row in audit_rows {
            assert!(
                !row.expect("audit row").contains(secret),
                "no audit row holds the secret answer"
            );
        }
    }
    drop(conn);
    journal.shutdown();
    let _ = std::fs::remove_file(path);
}
