//! Tests for grok's `_x.ai/ask_user_question` cards: the card carries the
//! agent's own labels in both params shapes, and a pick, an "Other" answer
//! and a dismiss produce the exact reply bytes — read off a fake child's
//! stdout, through the production sender. Deleting the write fails the test
//! on timeout instead of passing silently.

use devboule_protocol::{PermissionOutcome, PermissionRequestKind, SessionEvent};

use super::super::acp_questions::{grok_question_result, parse_grok_questions};
use super::question_support::{
    asked, echo_harness, enveloped, fence_params, has_notice, live_turn, node_gated, unenveloped,
    FENCE, TOPPINGS,
};

#[test]
fn grok_question_raises_a_card_with_the_agents_labels() {
    if node_gated() {
        return;
    }
    let echo = echo_harness();
    live_turn(&echo.reader);
    echo.dispatch(&enveloped(0));
    let events = echo.conn.pull_events();
    match asked(&events, "call-fence-0") {
        SessionEvent::PermissionRequest {
            title,
            description,
            options,
            kind,
            questions,
            is_chooser,
            ..
        } => {
            assert_eq!(
                kind,
                Some(PermissionRequestKind::Question),
                "a model's question is never auto-answered"
            );
            assert_eq!(is_chooser, Some(false), "a question is not a chooser");
            assert_eq!(title, FENCE);
            assert_eq!(
                description.as_deref(),
                Some("Forest green (Recommended) / Barn red / Weathered grey")
            );
            assert_eq!(
                options
                    .iter()
                    .map(|option| option.option_id.as_str())
                    .collect::<Vec<_>>(),
                vec!["q0o0", "q0o1", "q0o2"]
            );
            assert!(
                options.iter().all(|option| option.kind == "allow_once"),
                "the agent's own labels, not a permission vocabulary"
            );
            let questions = questions.expect("question items");
            assert_eq!(questions.len(), 1);
            assert_eq!(questions[0].question, FENCE);
            assert_eq!(questions[0].header, None);
            assert!(!questions[0].multi_select, "null reads as false");
            assert_eq!(questions[0].options.len(), 3);
            assert_eq!(
                questions[0].options[0].description.as_deref(),
                Some("Blends in.")
            );
        }
        _ => panic!("expected a permission request"),
    }
    assert_eq!(echo.broker.pending_len(), 1);
    assert!(
        !has_notice(&echo.conn.pull_events()),
        "a question is asked, not refused"
    );
}

#[test]
fn unenveloped_params_raise_the_same_card() {
    if node_gated() {
        return;
    }
    let echo = echo_harness();
    live_turn(&echo.reader);
    echo.dispatch(&unenveloped(5));
    let events = echo.conn.pull_events();
    match asked(&events, "call-bare-5") {
        SessionEvent::PermissionRequest { title, kind, .. } => {
            assert_eq!(kind, Some(PermissionRequestKind::Question));
            assert_eq!(title, FENCE);
        }
        _ => panic!("expected a permission request"),
    }
    assert_eq!(echo.broker.pending_len(), 1);
}

#[test]
fn option_pick_answers_accepted_with_labels() {
    if node_gated() {
        return;
    }
    let mut echo = echo_harness();
    live_turn(&echo.reader);
    echo.dispatch(&enveloped(0));
    let _ = echo.conn.pull_events();
    echo.broker
        .respond_with_option(
            "call-fence-0",
            PermissionOutcome::AllowOnce,
            Some("q0o1".to_string()),
            None,
        )
        .expect("option pick");
    assert_eq!(
        echo.read_frame(),
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": 0,
            "result": {
                "outcome": "accepted",
                "answers": { FENCE: ["Barn red"] },
            },
        }),
        "grok is answered with labels, never option ids"
    );
}

#[test]
fn other_answer_travels_verbatim() {
    if node_gated() {
        return;
    }
    let mut echo = echo_harness();
    live_turn(&echo.reader);
    echo.dispatch(&enveloped(0));
    let _ = echo.conn.pull_events();
    echo.broker
        .respond_with_option(
            "call-fence-0",
            PermissionOutcome::AllowOnce,
            None,
            Some("Teal, obviously".to_string()),
        )
        .expect("Other answer");
    assert_eq!(
        echo.read_frame()["result"],
        serde_json::json!({
            "outcome": "accepted",
            "answers": { FENCE: ["Teal, obviously"] },
        })
    );
}

#[test]
fn dismiss_answers_skip_interview() {
    if node_gated() {
        return;
    }
    let mut echo = echo_harness();
    live_turn(&echo.reader);
    echo.dispatch(&enveloped(0));
    let _ = echo.conn.pull_events();
    echo.broker
        .respond_with_option("call-fence-0", PermissionOutcome::Deny, None, None)
        .expect("dismissal");
    assert_eq!(
        echo.read_frame()["result"],
        serde_json::json!({ "outcome": "skip_interview" }),
        "a dismiss is grok's decline variant, not a cancelled outcome"
    );
}

#[test]
fn multi_question_answers_map_each_text() {
    if node_gated() {
        return;
    }
    let mut echo = echo_harness();
    live_turn(&echo.reader);
    echo.dispatch(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "_x.ai/ask_user_question",
        "params": {
            "sessionId": super::question_support::SESSION,
            "toolCallId": "call-multi-1",
            "questions": [
                {"question": FENCE, "options": [{"label": "Barn red"}]},
                {"question": TOPPINGS, "multiSelect": true,
                 "options": [{"label": "Cheese"}, {"label": "Pepperoni"}]},
            ],
            "mode": "default",
        },
    }));
    let events = echo.conn.pull_events();
    match asked(&events, "call-multi-1") {
        SessionEvent::PermissionRequest { questions, .. } => {
            assert_eq!(questions.expect("items").len(), 2);
        }
        _ => panic!("expected a permission request"),
    }
    // The card joins a multi-select the provider's own way and answers both
    // questions at once through the JSON text map, keyed by question text.
    let answer =
        serde_json::json!({ FENCE: "Barn red", TOPPINGS: "Cheese, Pepperoni" }).to_string();
    echo.broker
        .respond_with_option(
            "call-multi-1",
            PermissionOutcome::AllowOnce,
            None,
            Some(answer),
        )
        .expect("multi answer");
    assert_eq!(
        echo.read_frame()["result"],
        serde_json::json!({
            "outcome": "accepted",
            "answers": { FENCE: ["Barn red"], TOPPINGS: ["Cheese", "Pepperoni"] },
        })
    );
}

#[test]
fn result_mapping_refuses_what_nobody_chose() {
    let params = fence_params();
    // A refusal with an option attached is still a refusal.
    assert_eq!(
        grok_question_result(
            &params,
            &serde_json::json!({ "outcome": { "outcome": "selected", "optionId": "deny" } })
        ),
        serde_json::json!({ "outcome": "skip_interview" })
    );
    // A bare pick cannot answer several questions at once.
    let multi = serde_json::json!({
        "questions": [{"question": FENCE}, {"question": TOPPINGS}],
    });
    assert_eq!(
        grok_question_result(
            &multi,
            &serde_json::json!({ "outcome": { "outcome": "selected", "optionId": "q0o0" } })
        ),
        serde_json::json!({ "outcome": "skip_interview" })
    );
    // And the decline variant is the shape a dismissal carries.
    assert_eq!(
        grok_question_result(
            &serde_json::json!({}),
            &serde_json::json!({ "outcome": { "outcome": "cancelled" } })
        ),
        serde_json::json!({ "outcome": "skip_interview" })
    );
}

#[test]
fn parse_grok_questions_skips_what_nobody_could_answer() {
    let questions = parse_grok_questions(&serde_json::json!({
        "questions": [
            {"question": " Q? ",
             "options": [{"label": "A", "description": "The A."}, {"nope": 1}, {"label": " "}],
             "multiSelect": true},
            {"header": "Alias text", "options": [{"label": "B"}]},
            {"question": "No options gets a text-only card"},
            {"header": "   "},
            "a string", 42, null
        ]
    }));
    assert_eq!(questions.len(), 3);
    assert_eq!(questions[0].question, "Q?");
    assert!(questions[0].multi_select);
    assert_eq!(questions[0].options.len(), 1);
    assert_eq!(questions[0].options[0].label, "A");
    assert_eq!(
        questions[0].options[0].description.as_deref(),
        Some("The A.")
    );
    assert_eq!(questions[1].question, "Alias text");
    assert!(!questions[1].multi_select);
    assert!(questions[2].options.is_empty());
}
