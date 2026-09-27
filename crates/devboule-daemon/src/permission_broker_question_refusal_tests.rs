//! A question grant the acceptance rule refuses, at the broker door: an
//! error, no `permissions` row, no provider frame, no tool row live or
//! replayed, and the card still open for a real answer.

use super::{
    journaled_with, permission_question, replayed_tool_rows, tool_rows, two_question_card,
    PermissionBroker, PermissionSender,
};
use crate::journal::Journal;
use devboule_protocol::{PermissionOutcome, SessionEvent};
use rusqlite::Connection;
use std::sync::{Arc, Mutex};

struct Door {
    broker: Arc<PermissionBroker>,
    journal: Arc<Journal>,
    session_id: String,
    path: std::path::PathBuf,
    frames: Arc<Mutex<Vec<serde_json::Value>>>,
    // Keeps the session alive while the broker publishes into it.
    _runtime: Arc<crate::session::SessionRuntime>,
}

fn door(label: &str, card: SessionEvent) -> Door {
    let frames: Arc<Mutex<Vec<serde_json::Value>>> = Arc::default();
    let captured = Arc::clone(&frames);
    let sender: Arc<PermissionSender> = Arc::new(move |_, frame| {
        captured.lock().expect("frames").push(frame);
        Ok(())
    });
    let (broker, journal, runtime, session_id, path) = journaled_with(label, label, sender);
    broker.register(400, card, &runtime).expect("register");
    Door {
        broker,
        journal,
        session_id,
        path,
        frames,
        _runtime: runtime,
    }
}

impl Door {
    fn refuse(
        &self,
        tool_call_id: &str,
        option_id: Option<&str>,
        answer: Option<&str>,
        reason: &str,
    ) {
        let error = self
            .broker
            .respond_with_option(
                tool_call_id,
                PermissionOutcome::AllowOnce,
                option_id.map(str::to_string),
                answer.map(str::to_string),
            )
            .expect_err("the door refuses");
        assert!(
            error.to_string().contains(reason),
            "pick {option_id:?} text {answer:?}: unexpected error {error}"
        );
        assert_eq!(self.broker.pending_len(), 1, "the card stays open");
    }

    fn assert_untouched(&self) {
        self.journal.flush().expect("journal flush");
        let conn = Connection::open(&self.path).expect("inspect journal");
        let permissions: u32 = conn
            .query_row(
                "SELECT COUNT(*) FROM permissions WHERE session_id = ?1",
                [&self.session_id],
                |row| row.get(0),
            )
            .expect("permissions count");
        assert_eq!(permissions, 0, "no permissions row");
        let (calls, updates) = tool_rows(&conn, &self.session_id);
        assert!(calls.is_empty() && updates.is_empty(), "no tool row");
        assert!(replayed_tool_rows(&conn, &self.session_id).is_empty());
        assert!(self.frames.lock().expect("frames").is_empty(), "no frame");
    }

    fn assert_one_answer_landed(&self) {
        assert_eq!(self.broker.pending_len(), 0);
        assert_eq!(self.frames.lock().expect("frames").len(), 1, "one frame");
        self.journal.flush().expect("journal flush");
        let conn = Connection::open(&self.path).expect("inspect journal");
        let permissions: u32 = conn
            .query_row(
                "SELECT COUNT(*) FROM permissions WHERE session_id = ?1",
                [&self.session_id],
                |row| row.get(0),
            )
            .expect("permissions count");
        assert_eq!(permissions, 1);
    }

    fn close(self) {
        self.journal.shutdown();
        let _ = std::fs::remove_file(&self.path);
    }
}

#[test]
fn empty_map_is_refused_then_a_real_map_lands() {
    let id = "question-refusal-empty-map";
    let door = door(id, two_question_card(id));
    door.refuse(id, None, Some("{}"), "must name at least one question");
    door.assert_untouched();
    let map = serde_json::json!({ "Which stain finish?": "Satin" }).to_string();
    door.broker
        .respond_with_option(id, PermissionOutcome::AllowOnce, None, Some(map))
        .expect("a real answer still lands");
    door.assert_one_answer_landed();
    door.close();
}

#[test]
fn every_multi_question_refusal_leaves_no_trace() {
    let id = "question-refusal-multi";
    let door = door(id, two_question_card(id));
    let blank = serde_json::json!({
        "Which colour should I paint the fence?": "  ",
        "Which stain finish?": "Satin",
    })
    .to_string();
    let unknown = serde_json::json!({
        "Which colour should I paint the fence?": "Green",
        "Something else?": "Satin",
    })
    .to_string();
    door.refuse(id, None, Some(&blank), "must not hold a blank value");
    door.refuse(
        id,
        None,
        Some(&unknown),
        "must not name an unknown question",
    );
    door.refuse(id, None, Some("Green"), "must map question text to answers");
    door.refuse(
        id,
        None,
        Some(r#"{"Which stain finish?": 1}"#),
        "must map question text to answers",
    );
    door.refuse(
        id,
        Some("q1o1"),
        None,
        "answered with a text map, not a pick",
    );
    door.refuse(
        id,
        None,
        None,
        "must name the picked option or carry its text",
    );
    door.assert_untouched();
    door.close();
}

#[test]
fn every_single_question_refusal_leaves_no_trace() {
    let id = "question-refusal-single";
    let door = door(id, permission_question(id));
    door.refuse(id, None, Some(""), "has an empty answer");
    door.refuse(id, None, Some("   "), "must not be blank");
    door.refuse(
        id,
        Some("q0o9"),
        None,
        "names no option the question offered",
    );
    door.refuse(
        id,
        Some("q1o0"),
        None,
        "names no option the question offered",
    );
    door.assert_untouched();
    door.broker
        .respond_with_option(
            id,
            PermissionOutcome::AllowOnce,
            Some("q0o0".to_string()),
            None,
        )
        .expect("a real pick still lands");
    door.assert_one_answer_landed();
    door.close();
}

#[test]
fn a_card_without_questions_refuses_every_grant_but_can_be_dismissed() {
    let id = "question-refusal-no-questions";
    let mut card = permission_question(id);
    if let SessionEvent::PermissionRequest {
        options, questions, ..
    } = &mut card
    {
        options.clear();
        *questions = None;
    }
    let door = door(id, card);
    door.refuse(id, None, Some("Green"), "no questions cannot be answered");
    door.assert_untouched();
    door.broker
        .respond_with_option(id, PermissionOutcome::Deny, None, None)
        .expect("the card can still be dismissed");
    door.assert_one_answer_landed();
    door.close();
}
