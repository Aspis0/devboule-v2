//! An answered model question as a tool row: the journalled call and update
//! and the history reader's rebuild, per answer shape.

use super::{permission_path, permission_question, PermissionBroker, PermissionSender};
use crate::journal::Journal;
use crate::session::SessionRuntime;
use devboule_protocol::{
    PermissionOutcome, PermissionQuestion, PermissionQuestionOption, PermissionRequestKind,
    SessionEvent, SessionKind, SessionOrigin,
};
use rusqlite::Connection;
use std::sync::Arc;

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
    journaled_with(label, session_suffix, Arc::new(|_, _| Ok(())))
}

fn journaled_with(
    label: &str,
    session_suffix: &str,
    sender: Arc<PermissionSender>,
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

/// This card's tool row, as journalled: the call and its update.
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

/// The same pair rebuilt through the history replay reader.
fn replayed_tool_rows(conn: &Connection, session_id: &str) -> Vec<SessionEvent> {
    crate::journal::replay_session(conn, session_id)
        .expect("history replay")
        .events
        .into_iter()
        .filter(|event| {
            matches!(
                event,
                SessionEvent::AgentToolCall { .. } | SessionEvent::AgentToolUpdate { .. }
            )
        })
        .collect()
}

#[test]
fn answered_option_yields_one_tool_row_with_question_and_label() {
    let (broker, journal, runtime, session_id, path) =
        journaled("transcript-pick", "question-transcript-pick");
    let _runtime = Arc::clone(&runtime);
    broker
        .register(
            300,
            permission_question("question-transcript-pick"),
            &runtime,
        )
        .expect("register");
    broker
        .respond_with_option(
            "question-transcript-pick",
            PermissionOutcome::AllowOnce,
            Some("q0o0".to_string()),
            None,
        )
        .expect("option pick");
    journal.flush().expect("journal flush");
    let conn = Connection::open(&path).expect("inspect journal");
    let (calls, updates) = tool_rows(&conn, &session_id);
    assert_eq!(calls.len(), 1, "one answered card leaves one call");
    assert_eq!(updates.len(), 1, "one answered card leaves one update");
    match &calls[0] {
        SessionEvent::AgentToolCall {
            tool_call_id,
            title,
            status,
            kind,
            ..
        } => {
            assert_eq!(tool_call_id, "question-transcript-pick");
            assert_eq!(title, "Which colour should I paint the fence?");
            assert_eq!(status, "completed");
            assert_eq!(kind.as_deref(), Some("question"));
        }
        _ => panic!("expected the tool call"),
    }
    match &updates[0] {
        SessionEvent::AgentToolUpdate {
            tool_call_id,
            status,
            text,
            title,
            ..
        } => {
            assert_eq!(tool_call_id, "question-transcript-pick");
            assert_eq!(status.as_deref(), Some("completed"));
            assert_eq!(
                title.as_deref(),
                Some("Which colour should I paint the fence?")
            );
            assert_eq!(
                text.as_deref(),
                Some(
                    "Question: Which colour should I paint the fence?\nAnswer: Forest green (Recommended)"
                )
            );
        }
        _ => panic!("expected the tool update"),
    }
    let replayed = replayed_tool_rows(&conn, &session_id);
    assert_eq!(replayed.len(), 2, "the history reader rebuilds the pair");
    assert!(matches!(replayed[0], SessionEvent::AgentToolCall { .. }));
    assert!(matches!(replayed[1], SessionEvent::AgentToolUpdate { .. }));
    drop(conn);
    journal.shutdown();
    let _ = std::fs::remove_file(path);
}

#[test]
fn free_text_answer_yields_one_row_with_the_words() {
    let answer = "a free-text shade of blue";
    let (broker, journal, runtime, session_id, path) =
        journaled("transcript-text", "question-transcript-text");
    let _runtime = Arc::clone(&runtime);
    broker
        .register(
            301,
            permission_question("question-transcript-text"),
            &runtime,
        )
        .expect("register");
    broker
        .respond_with_option(
            "question-transcript-text",
            PermissionOutcome::AllowOnce,
            None,
            Some(answer.to_string()),
        )
        .expect("free-text answer");
    journal.flush().expect("journal flush");
    let conn = Connection::open(&path).expect("inspect journal");
    let (calls, updates) = tool_rows(&conn, &session_id);
    assert_eq!(calls.len(), 1);
    assert_eq!(updates.len(), 1);
    match &updates[0] {
        SessionEvent::AgentToolUpdate { text, .. } => {
            assert!(text.as_deref().unwrap_or_default().contains(answer));
        }
        _ => panic!("expected the tool update"),
    }
    let (outcome, payload): (String, Vec<u8>) = conn
        .query_row(
            "SELECT outcome, payload FROM permissions WHERE session_id = ?1 AND request_id = ?2",
            [&session_id, "question-transcript-text"],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("permission row");
    assert_eq!(outcome, "allow_once");
    assert!(!String::from_utf8_lossy(&payload).contains(answer));
    drop(conn);
    journal.shutdown();
    let _ = std::fs::remove_file(path);
}

#[test]
fn dismissed_question_adds_no_tool_row() {
    let (broker, journal, runtime, session_id, path) =
        journaled("transcript-dismiss", "question-transcript-dismiss");
    let _runtime = Arc::clone(&runtime);
    broker
        .register(
            302,
            permission_question("question-transcript-dismiss"),
            &runtime,
        )
        .expect("register");
    broker
        .respond_with_option(
            "question-transcript-dismiss",
            PermissionOutcome::Deny,
            None,
            None,
        )
        .expect("dismissal is an answer");
    journal.flush().expect("journal flush");
    let conn = Connection::open(&path).expect("inspect journal");
    let (calls, updates) = tool_rows(&conn, &session_id);
    assert!(calls.is_empty() && updates.is_empty());
    assert!(replayed_tool_rows(&conn, &session_id).is_empty());
    drop(conn);
    journal.shutdown();
    let _ = std::fs::remove_file(path);
}

/// A partial map titles only what it answered: the unanswered question is
/// not named by the collapsed line.
#[test]
fn partial_answer_titles_only_answered() {
    let (broker, journal, runtime, session_id, path) =
        journaled("transcript-partial", "question-transcript-partial");
    let _runtime = Arc::clone(&runtime);
    broker
        .register(
            312,
            two_question_card("question-transcript-partial"),
            &runtime,
        )
        .expect("register");
    let map = serde_json::json!({ "Which stain finish?": "Satin" }).to_string();
    broker
        .respond_with_option(
            "question-transcript-partial",
            PermissionOutcome::AllowOnce,
            None,
            Some(map),
        )
        .expect("partial answer");
    journal.flush().expect("journal flush");
    let conn = Connection::open(&path).expect("inspect journal");
    let (calls, updates) = tool_rows(&conn, &session_id);
    assert_eq!(calls.len(), 1);
    assert_eq!(updates.len(), 1);
    match &calls[0] {
        SessionEvent::AgentToolCall { title, .. } => {
            assert_eq!(title, "Which stain finish?");
        }
        _ => panic!("expected the tool call"),
    }
    match &updates[0] {
        SessionEvent::AgentToolUpdate { text, .. } => {
            assert_eq!(
                text.as_deref(),
                Some("Question: Which stain finish?\nAnswer: Satin")
            );
        }
        _ => panic!("expected the tool update"),
    }
    drop(conn);
    journal.shutdown();
    let _ = std::fs::remove_file(path);
}

fn two_question_card(tool_call_id: &str) -> SessionEvent {
    let item = |question: &str, labels: &[&str]| PermissionQuestion {
        question: question.to_string(),
        header: None,
        options: labels
            .iter()
            .map(|label| PermissionQuestionOption {
                label: label.to_string(),
                description: None,
            })
            .collect(),
        multi_select: false,
        allow_other: Some(true),
        secret: None,
    };
    SessionEvent::PermissionRequest {
        tool_call_id: tool_call_id.to_string(),
        title: "Which colour should I paint the fence?".to_string(),
        description: None,
        command: None,
        args: None,
        cwd: None,
        env: None,
        options: vec![
            devboule_protocol::PermissionOption {
                option_id: "q0o0".to_string(),
                name: "Green".to_string(),
                kind: "allow_once".to_string(),
            },
            devboule_protocol::PermissionOption {
                option_id: "q0o1".to_string(),
                name: "Red".to_string(),
                kind: "allow_once".to_string(),
            },
            devboule_protocol::PermissionOption {
                option_id: "q1o0".to_string(),
                name: "Matte".to_string(),
                kind: "allow_once".to_string(),
            },
            devboule_protocol::PermissionOption {
                option_id: "q1o1".to_string(),
                name: "Satin".to_string(),
                kind: "allow_once".to_string(),
            },
        ],
        is_chooser: None,
        kind: Some(PermissionRequestKind::Question),
        questions: Some(vec![
            item("Which colour should I paint the fence?", &["Green", "Red"]),
            item("Which stain finish?", &["Matte", "Satin"]),
        ]),
        origin: SessionOrigin::local(),
        create_agent: None,
    }
}

#[test]
fn multi_question_card_yields_one_row_with_both_answers_in_order() {
    let (broker, journal, runtime, session_id, path) =
        journaled("transcript-multi", "question-transcript-multi");
    let _runtime = Arc::clone(&runtime);
    broker
        .register(
            303,
            two_question_card("question-transcript-multi"),
            &runtime,
        )
        .expect("register");
    let map = serde_json::json!({
        "Which colour should I paint the fence?": "Green",
        "Which stain finish?": "Satin",
    })
    .to_string();
    broker
        .respond_with_option(
            "question-transcript-multi",
            PermissionOutcome::AllowOnce,
            None,
            Some(map),
        )
        .expect("multi answer");
    journal.flush().expect("journal flush");
    let conn = Connection::open(&path).expect("inspect journal");
    let (calls, updates) = tool_rows(&conn, &session_id);
    assert_eq!(calls.len(), 1, "one answered card leaves one row");
    assert_eq!(updates.len(), 1);
    match &calls[0] {
        SessionEvent::AgentToolCall {
            tool_call_id,
            title,
            ..
        } => {
            assert_eq!(tool_call_id, "question-transcript-multi");
            assert_eq!(title, "Which colour should I paint the fence? (+1 more)");
        }
        _ => panic!("expected the tool call"),
    }
    match &updates[0] {
        SessionEvent::AgentToolUpdate { text, .. } => {
            assert_eq!(
                text.as_deref(),
                Some(
                    "Question: Which colour should I paint the fence?\nAnswer: Green\nQuestion: Which stain finish?\nAnswer: Satin"
                )
            );
        }
        _ => panic!("expected the tool update"),
    }
    drop(conn);
    journal.shutdown();
    let _ = std::fs::remove_file(path);
}

#[path = "permission_broker_question_refusal_tests.rs"]
mod refusal_tests;
