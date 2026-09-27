//! The attribution record's kind and the answer that outlives its row.

use super::{permission_path, permission_question, PermissionBroker, PermissionSender};
use crate::journal::Journal;
use crate::session::SessionRuntime;
use devboule_protocol::{PermissionOutcome, SessionEvent, SessionKind};
use rusqlite::Connection;
use std::sync::{Arc, Mutex};

fn journaled(
    label: &str,
    session_suffix: &str,
) -> (
    Arc<PermissionBroker>,
    Arc<Journal>,
    Arc<SessionRuntime>,
    String,
    std::path::PathBuf,
) {
    let path = permission_path(label);
    let _ = std::fs::remove_file(&path);
    let journal = Arc::new(Journal::open(&path).expect("journal"));
    let sender: Arc<PermissionSender> = Arc::new(|_, _| Ok(()));
    let broker = PermissionBroker::with_sender(sender);
    let session_id = format!("s.permission.{session_suffix}");
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
    (broker, journal, runtime, session_id, path)
}

fn tool_rows(conn: &Connection, session_id: &str) -> (Vec<SessionEvent>, Vec<SessionEvent>) {
    let mut stmt = conn
        .prepare("SELECT payload FROM events WHERE session_id = ?1 AND kind = 'agent_report'")
        .expect("events query");
    let rows = stmt
        .query_map([session_id], |row| row.get::<_, Vec<u8>>(0))
        .expect("agent_report rows");
    let mut calls = Vec::new();
    let mut updates = Vec::new();
    for payload in rows {
        let event: SessionEvent =
            serde_json::from_slice(&payload.expect("payload")).expect("agent_report event");
        match event {
            SessionEvent::AgentToolCall { .. } => calls.push(event),
            SessionEvent::AgentToolUpdate { .. } => updates.push(event),
            _ => {}
        }
    }
    (calls, updates)
}

/// `PermissionAnswered` journals as `agent_report`, never as `output`:
/// both replay readers return it, and no terminal scrollback can carry its
/// raw JSON. What this pins is the row's kind and the absence of an output
/// twin — a Terminal session replays `output` rows into its scrollback and
/// `agent_report` rows into its transcript ledger, so the kind is the fix.
#[test]
fn permission_answered_survives_as_agent_report_not_output() {
    let (broker, journal, runtime, session_id, path) =
        journaled("transcript-answered", "question-transcript-answered");
    let _runtime = Arc::clone(&runtime);
    broker
        .register(
            305,
            permission_question("question-transcript-answered"),
            &runtime,
        )
        .expect("register");
    broker
        .respond_with_option(
            "question-transcript-answered",
            PermissionOutcome::AllowOnce,
            Some("q0o1".to_string()),
            None,
        )
        .expect("option pick");
    journal.flush().expect("journal flush");
    let conn = Connection::open(&path).expect("inspect journal");
    let mut answered_reports = 0;
    {
        let mut stmt = conn
            .prepare("SELECT kind, payload FROM events")
            .expect("events");
        let rows = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, Vec<u8>>(1)?))
            })
            .expect("event rows");
        for row in rows {
            let (kind, payload) = row.expect("event row");
            if kind == "output" {
                assert!(
                    !String::from_utf8_lossy(&payload).contains("permission_answered"),
                    "no output row carries the attribution record"
                );
            }
            if kind == "agent_report" {
                if let Ok(SessionEvent::PermissionAnswered {
                    card_id, outcome, ..
                }) = serde_json::from_slice::<SessionEvent>(&payload)
                {
                    assert_eq!(card_id, "question-transcript-answered");
                    assert_eq!(outcome, "allow_once");
                    answered_reports += 1;
                }
            }
        }
    }
    assert_eq!(answered_reports, 1, "the attribution record replays");
    let _ = session_id;
    drop(conn);
    journal.shutdown();
    let _ = std::fs::remove_file(path);
}

/// A row the stream refuses still leaves the answer intact: the provider
/// frame went out and the decision is recorded, while the transcript row
/// is a wordless log line instead of a blocked or undone answer.
#[test]
fn failed_row_publish_keeps_the_answer() {
    let (broker, journal, runtime, session_id, path) =
        journaled("transcript-closed", "question-transcript-closed");
    let _runtime = Arc::clone(&runtime);
    broker
        .register(
            306,
            permission_question("question-transcript-closed"),
            &runtime,
        )
        .expect("register");
    runtime.close_output();
    broker
        .respond_with_option(
            "question-transcript-closed",
            PermissionOutcome::AllowOnce,
            Some("q0o0".to_string()),
            None,
        )
        .expect("option pick");
    journal.flush().expect("journal flush");
    let conn = Connection::open(&path).expect("inspect journal");
    let outcome: String = conn
        .query_row(
            "SELECT outcome FROM permissions WHERE session_id = ?1 AND request_id = ?2",
            [&session_id, "question-transcript-closed"],
            |row| row.get(0),
        )
        .expect("permission row");
    assert_eq!(outcome, "allow_once", "the answer stands without its row");
    let (calls, updates) = tool_rows(&conn, &session_id);
    assert!(calls.is_empty() && updates.is_empty());
    drop(conn);
    journal.shutdown();
    let _ = std::fs::remove_file(path);
}

/// Test broker only: with the journal dead the answer still completes —
/// the frame goes out, the degradation is marked, nothing is journalled.
/// Production cancels instead (next test); the `for_test` broker is what
/// lets this path return `Ok` at all.
#[test]
fn dead_journal_test_broker_completes_and_journals_nothing() {
    let path = permission_path("transcript-dead-journal");
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
    let broker = PermissionBroker::for_test(sender);
    let session_id = "s.permission.question-transcript-dead".to_string();
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
    broker
        .register(
            307,
            permission_question("question-transcript-dead"),
            &runtime,
        )
        .expect("register");
    journal.shutdown();
    assert!(!runtime.journal_degraded());
    broker
        .respond_with_option(
            "question-transcript-dead",
            PermissionOutcome::AllowOnce,
            Some("q0o0".to_string()),
            None,
        )
        .expect("option pick");
    assert_eq!(sent.lock().expect("sent lock").len(), 1);
    assert!(runtime.journal_degraded(), "the drop is marked, not silent");
    let conn = Connection::open(&path).expect("inspect journal");
    let (calls, updates) = tool_rows(&conn, &session_id);
    assert!(calls.is_empty() && updates.is_empty());
    drop(conn);
    let _ = std::fs::remove_file(path);
}

/// Production broker with a dead journal: the decision cannot record, so
/// the answer is cancelled before any row — the provider is told
/// `cancelled`, nothing is journalled, and the call reports the loss.
#[test]
fn dead_journal_production_broker_cancels_the_answer() {
    let path = permission_path("transcript-dead-production");
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
    let session_id = "s.permission.question-transcript-dead-prod".to_string();
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
    broker
        .register(
            308,
            permission_question("question-transcript-dead-prod"),
            &runtime,
        )
        .expect("register");
    journal.shutdown();
    let error = broker
        .respond_with_option(
            "question-transcript-dead-prod",
            PermissionOutcome::AllowOnce,
            Some("q0o0".to_string()),
            None,
        )
        .expect_err("an unjournalled decision cancels");
    assert!(
        error.to_string().contains("was not journaled"),
        "unexpected error: {error}"
    );
    assert_eq!(
        sent.lock().expect("sent lock")[0].1["outcome"]["outcome"],
        "cancelled"
    );
    let conn = Connection::open(&path).expect("inspect journal");
    let decisions: u32 = conn
        .query_row(
            "SELECT COUNT(*) FROM permissions WHERE session_id = ?1",
            [&session_id],
            |row| row.get(0),
        )
        .expect("permissions count");
    assert_eq!(decisions, 0);
    let (calls, updates) = tool_rows(&conn, &session_id);
    assert!(calls.is_empty() && updates.is_empty());
    drop(conn);
    let _ = std::fs::remove_file(path);
}
