//! Tests for Codex `requestUserInput` questions, current and legacy spelling:
//! card shape, option picks, text answers, dismissal, and the parser.

use devboule_protocol::SessionEvent;

use super::super::codex_questions::{codex_question_result, parse_codex_questions};
use super::input_test_support::{
    asked_question, fence_question_params, question_harness, question_harness_with_journal,
    single_question_params, user_input_line,
};

#[test]
fn request_user_input_builds_a_question_card() {
    let (broker, _, runtime, conn, mut reader) = question_harness();
    reader.dispatch_value(
        user_input_line("item/tool/requestUserInput", fence_question_params()),
        &runtime,
    );
    let events = conn.pull_events();
    match asked_question(&events) {
        SessionEvent::PermissionRequest {
            title,
            description,
            options,
            kind,
            questions,
            ..
        } => {
            assert_eq!(
                kind,
                Some(devboule_protocol::PermissionRequestKind::Question)
            );
            assert_eq!(title, "Which colour should I paint the fence?");
            assert_eq!(
                description.as_deref(),
                Some("Forest green (Recommended) / Barn red")
            );
            assert_eq!(
                options
                    .iter()
                    .map(|option| option.option_id.as_str())
                    .collect::<Vec<_>>(),
                vec!["q0o0", "q0o1", "q1o0", "q1o1"]
            );
            let questions = questions.expect("question items");
            assert_eq!(questions.len(), 2);
            assert_eq!(
                questions[0].question,
                "Which colour should I paint the fence?"
            );
            assert_eq!(questions[0].header.as_deref(), Some("Fence colour"));
            assert!(!questions[0].multi_select);
            // Absent wire flags arrive as explicit false, not absent.
            assert_eq!(questions[0].allow_other, Some(false));
            assert_eq!(questions[0].secret, Some(false));
            assert_eq!(
                questions[0].options[0].description.as_deref(),
                Some("Blends in.")
            );
            assert!(questions[1].multi_select);
        }
        _ => panic!("expected a permission request"),
    }
    assert_eq!(broker.pending_len(), 1);
    // A question is asked, not refused: no notice goes up.
    assert!(!conn
        .pull_events()
        .iter()
        .any(|event| matches!(event.envelope.event, SessionEvent::SessionNotice { .. })));
}

#[test]
fn question_option_pick_answers_per_id_shape() {
    let (broker, captured, runtime, conn, mut reader) = question_harness();
    reader.dispatch_value(
        user_input_line("item/tool/requestUserInput", single_question_params()),
        &runtime,
    );
    let _ = conn.pull_events();
    broker
        .respond_with_option(
            "item-1",
            devboule_protocol::PermissionOutcome::AllowOnce,
            Some("q0o1".to_string()),
            None,
        )
        .expect("option pick");
    let frames = captured.lock().expect("captured");
    assert_eq!(frames.len(), 1);
    assert_eq!(
        frames[0]["result"],
        serde_json::json!({ "answers": { "q1": { "answers": ["Barn red"] } } })
    );
}

#[test]
fn question_other_answer_maps_by_question_id() {
    let (broker, captured, runtime, conn, mut reader) = question_harness();
    reader.dispatch_value(
        user_input_line("item/tool/requestUserInput", single_question_params()),
        &runtime,
    );
    let _ = conn.pull_events();
    broker
        .respond_with_option(
            "item-1",
            devboule_protocol::PermissionOutcome::AllowOnce,
            None,
            Some("Teal, obviously".to_string()),
        )
        .expect("Other answer");
    let frames = captured.lock().expect("captured");
    // One question answered through the text door: the per-id shape
    // with the person's words verbatim. (A two-question card answers
    // through the JSON text map instead — covered below.)
    assert_eq!(
        frames[0]["result"],
        serde_json::json!({ "answers": { "q1": { "answers": ["Teal, obviously"] } } })
    );
}

#[test]
fn question_multi_answer_maps_each_id() {
    // The card joins a multi-select the provider's own way and answers both
    // questions at once through the JSON text map, keyed by question text.
    let answer = serde_json::json!({
        "Which colour should I paint the fence?": "Barn red",
        "Which toppings?": "Cheese, Pepperoni"
    })
    .to_string();
    let params = fence_question_params();
    let result = serde_json::json!({
        "outcome": { "outcome": "selected", "answer": answer }
    });
    assert_eq!(
        codex_question_result(&params, &result),
        serde_json::json!({ "answers": {
            "q1": { "answers": ["Barn red"] },
            "q2": { "answers": ["Cheese", "Pepperoni"] }
        } })
    );
}

#[test]
fn question_dismissal_answers_empty() {
    let (broker, captured, runtime, conn, mut reader) = question_harness();
    reader.dispatch_value(
        user_input_line("item/tool/requestUserInput", fence_question_params()),
        &runtime,
    );
    let _ = conn.pull_events();
    broker
        .respond_with_option(
            "item-1",
            devboule_protocol::PermissionOutcome::Deny,
            None,
            None,
        )
        .expect("dismissal");
    let frames = captured.lock().expect("captured");
    assert_eq!(frames[0]["result"], serde_json::json!({ "answers": {} }));
}

#[test]
fn legacy_request_user_input_matches() {
    let (broker, captured, runtime, conn, mut reader) = question_harness();
    reader.dispatch_value(
        user_input_line("tool/requestUserInput", single_question_params()),
        &runtime,
    );
    let events = conn.pull_events();
    assert!(matches!(
        asked_question(&events),
        SessionEvent::PermissionRequest { .. }
    ));
    broker
        .respond_with_option(
            "item-1",
            devboule_protocol::PermissionOutcome::AllowOnce,
            Some("q0o1".to_string()),
            None,
        )
        .expect("option pick");
    let frames = captured.lock().expect("captured");
    assert_eq!(
        frames[0]["result"],
        serde_json::json!({ "answers": { "q1": { "answers": ["Barn red"] } } })
    );
}

#[test]
fn unparseable_question_answers_empty_at_once() {
    // Nothing the person could answer: the id is answered now, not carded.
    let (broker, _, runtime, conn, mut reader) = question_harness();
    reader.dispatch_value(
        user_input_line(
            "item/tool/requestUserInput",
            serde_json::json!({
                "itemId": "item-empty",
                "threadId": "thread-1",
                "turnId": "turn-1",
                "questions": [{"header": "No text"}]
            }),
        ),
        &runtime,
    );
    assert_eq!(broker.pending_len(), 0);
    assert!(!conn
        .pull_events()
        .iter()
        .any(|event| matches!(event.envelope.event, SessionEvent::PermissionRequest { .. })));
    // And the empty map is the shape a dismissal carries.
    assert_eq!(
        codex_question_result(
            &serde_json::json!({}),
            &serde_json::json!({ "outcome": { "outcome": "cancelled" } })
        ),
        serde_json::json!({ "answers": {} })
    );
}

#[test]
fn parse_codex_questions_skips_what_nobody_could_answer() {
    let questions = parse_codex_questions(&serde_json::json!({
        "questions": [
            {"id": "ok", "header": "H", "question": "Q?",
             "options": [{"label": "A", "description": "The A."}, {"nope": 1}, {"label": " "}],
             "multiSelect": true, "isOther": true, "isSecret": true},
            {"header": "No id"},
            {"id": "x", "question": "No header"},
            "a string", 42, null
        ]
    }));
    assert_eq!(questions.len(), 1);
    assert_eq!(questions[0].id, "ok");
    assert!(questions[0].multi_select);
    assert_eq!(questions[0].options.len(), 1);
    assert_eq!(questions[0].options[0].label, "A");
    assert_eq!(
        questions[0].options[0].description.as_deref(),
        Some("The A.")
    );
    assert!(questions[0].allow_other);
    assert!(questions[0].secret);
}

fn replayed_tool_rows(
    journal: &crate::journal::Journal,
    path: &std::path::Path,
) -> Vec<SessionEvent> {
    journal.flush().expect("journal flush");
    let conn = rusqlite::Connection::open(path).expect("inspect journal");
    crate::journal::replay_session(&conn, "s.codex.questions")
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

/// Codex, end to end: an answered question leaves exactly one tool row
/// with the question's words and the picked label, rebuilt by the history
/// reader, and the provider's own reply carries the answers map.
#[test]
fn answered_question_leaves_one_transcript_row() {
    let ((broker, captured, runtime, conn, mut reader), journal, path) =
        question_harness_with_journal("codex-transcript-pick");
    reader.dispatch_value(
        user_input_line("item/tool/requestUserInput", single_question_params()),
        &runtime,
    );
    let _ = conn.pull_events();
    broker
        .respond_with_option(
            "item-1",
            devboule_protocol::PermissionOutcome::AllowOnce,
            Some("q0o0".to_string()),
            None,
        )
        .expect("option pick");
    let frames = captured.lock().expect("captured");
    assert_eq!(frames.len(), 1);
    assert_eq!(
        frames[0]["result"],
        serde_json::json!({ "answers": { "q1": { "answers": ["Forest green (Recommended)"] } } })
    );
    // The live attach lane carries the same pair: the event_pull reader
    // path, not just the journal rows.
    let live: Vec<SessionEvent> = conn
        .pull_events()
        .into_iter()
        .filter_map(|event| match event.envelope.event {
            SessionEvent::AgentToolCall { .. } | SessionEvent::AgentToolUpdate { .. } => {
                Some(event.envelope.event)
            }
            _ => None,
        })
        .collect();
    assert_eq!(live.len(), 2);
    assert!(live.iter().all(|event| match event {
        SessionEvent::AgentToolCall { tool_call_id, .. }
        | SessionEvent::AgentToolUpdate { tool_call_id, .. } => tool_call_id == "item-1",
        _ => false,
    }));
    let texts = replayed_tool_rows(&journal, &path);
    assert_eq!(texts.len(), 2, "one answered card leaves one row");
    match &texts[0] {
        SessionEvent::AgentToolCall {
            tool_call_id,
            title,
            ..
        } => {
            assert_eq!(tool_call_id, "item-1");
            assert_eq!(title, "Which colour should I paint the fence?");
        }
        _ => panic!("expected the tool call"),
    }
    match &texts[1] {
        SessionEvent::AgentToolUpdate {
            tool_call_id, text, ..
        } => {
            assert_eq!(tool_call_id, "item-1");
            let text = text.as_deref().unwrap_or_default();
            assert!(text.contains("Which colour should I paint the fence?"));
            assert!(text.contains("Forest green (Recommended)"));
        }
        _ => panic!("expected the tool update"),
    }
    journal.shutdown();
    let _ = std::fs::remove_file(path);
}

#[test]
fn dismissed_question_leaves_no_transcript_row() {
    let ((broker, _, runtime, conn, mut reader), journal, path) =
        question_harness_with_journal("codex-transcript-dismiss");
    reader.dispatch_value(
        user_input_line("item/tool/requestUserInput", single_question_params()),
        &runtime,
    );
    let _ = conn.pull_events();
    broker
        .respond_with_option(
            "item-1",
            devboule_protocol::PermissionOutcome::Deny,
            None,
            None,
        )
        .expect("dismissal");
    assert!(replayed_tool_rows(&journal, &path).is_empty());
    journal.shutdown();
    let _ = std::fs::remove_file(path);
}

/// A secret question answered by option pick: the transcript row hides the
/// label, and so does the resolved event — the option id and kind travel
/// (positions, not words), the name stays out.
#[test]
fn secret_option_pick_hides_the_label_everywhere() {
    let ((broker, _, runtime, conn, mut reader), journal, path) =
        question_harness_with_journal("codex-transcript-secret-pick");
    let mut params = single_question_params();
    params["questions"][0]["isSecret"] = serde_json::json!(true);
    reader.dispatch_value(
        user_input_line("item/tool/requestUserInput", params),
        &runtime,
    );
    let _ = conn.pull_events();
    broker
        .respond_with_option(
            "item-1",
            devboule_protocol::PermissionOutcome::AllowOnce,
            Some("q0o0".to_string()),
            None,
        )
        .expect("option pick");
    let resolved = conn
        .pull_events()
        .into_iter()
        .find_map(|event| match event.envelope.event {
            SessionEvent::PermissionResolved {
                tool_call_id,
                selected_option_name,
                ..
            } if tool_call_id == "item-1" => Some(selected_option_name),
            _ => None,
        })
        .expect("resolved event");
    assert_eq!(resolved, None);
    let rows = replayed_tool_rows(&journal, &path);
    assert_eq!(rows.len(), 2);
    match &rows[1] {
        SessionEvent::AgentToolUpdate { text, .. } => {
            let text = text.as_deref().unwrap_or_default();
            assert!(text.contains("(hidden)"));
            assert!(!text.contains("Forest green"));
        }
        _ => panic!("expected the tool update"),
    }
    journal.shutdown();
    let _ = std::fs::remove_file(path);
}
