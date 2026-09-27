//! Mapping cases: picks, text maps, secrets, and the row format.

use super::*;
use devboule_protocol::{PermissionQuestionOption, SessionOrigin};

fn request_with(questions: Vec<devboule_protocol::PermissionQuestion>) -> SessionEvent {
    SessionEvent::PermissionRequest {
        tool_call_id: "q-1".to_string(),
        title: questions
            .first()
            .map(|item| item.question.clone())
            .unwrap_or_default(),
        description: None,
        command: None,
        args: None,
        cwd: None,
        env: None,
        options: Vec::new(),
        is_chooser: None,
        kind: Some(PermissionRequestKind::Question),
        questions: Some(questions),
        origin: SessionOrigin::local(),
        create_agent: None,
    }
}

fn one_question() -> Vec<devboule_protocol::PermissionQuestion> {
    vec![devboule_protocol::PermissionQuestion {
        question: "Which colour?".to_string(),
        header: None,
        options: vec![
            PermissionQuestionOption {
                label: "Green".to_string(),
                description: None,
            },
            PermissionQuestionOption {
                label: "Red".to_string(),
                description: None,
            },
        ],
        multi_select: false,
        allow_other: Some(true),
        secret: None,
    }]
}

#[test]
fn pick_names_the_offered_label() {
    let rows = answered_questions(
        &request_with(one_question()),
        &serde_json::json!({ "outcome": { "outcome": "selected", "optionId": "q0o1" } }),
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].question, "Which colour?");
    assert_eq!(rows[0].answer, "Red");
}

#[test]
fn dismissal_adds_no_row() {
    let rows = answered_questions(
        &request_with(one_question()),
        &serde_json::json!({ "outcome": { "outcome": "cancelled" } }),
    );
    assert!(rows.is_empty());
}

#[test]
fn secret_answer_stays_hidden() {
    let mut items = one_question();
    items[0].secret = Some(true);
    let rows = answered_questions(
        &request_with(items),
        &serde_json::json!({ "outcome": { "outcome": "selected", "answer": "s3cr3t words" } }),
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].answer, HIDDEN_ANSWER);
    assert!(!rows[0].answer.contains("s3cr3t"));
}

#[test]
fn format_pins_labels_order_and_multiline_answers() {
    assert_eq!(
        format_question_answer("Which colour?", "Red"),
        "Question: Which colour?\nAnswer: Red"
    );
    assert_eq!(
        format_question_answer("Why?", "first line\nsecond line"),
        "Question: Why?\nAnswer: first line\nsecond line"
    );
    let rows = vec![
        AnsweredQuestion {
            question: "First?".to_string(),
            answer: "One".to_string(),
        },
        AnsweredQuestion {
            question: "Second?".to_string(),
            answer: "Two".to_string(),
        },
    ];
    assert_eq!(
        format_tool_output(&rows),
        "Question: First?\nAnswer: One\nQuestion: Second?\nAnswer: Two"
    );
}

#[test]
fn card_title_names_only_what_the_row_shows() {
    let empty: Vec<AnsweredQuestion> = Vec::new();
    assert_eq!(format_card_title(&empty), "Question");
    let one = vec![AnsweredQuestion {
        question: "Which colour?".to_string(),
        answer: "Green".to_string(),
    }];
    assert_eq!(format_card_title(&one), "Which colour?");
    let two = vec![
        AnsweredQuestion {
            question: "Which colour?".to_string(),
            answer: "Green".to_string(),
        },
        AnsweredQuestion {
            question: "Which finish?".to_string(),
            answer: "Satin".to_string(),
        },
    ];
    assert_eq!(format_card_title(&two), "Which colour? (+1 more)");
    let three = vec![
        AnsweredQuestion {
            question: "One?".to_string(),
            answer: "1".to_string(),
        },
        AnsweredQuestion {
            question: "Two?".to_string(),
            answer: "2".to_string(),
        },
        AnsweredQuestion {
            question: "Three?".to_string(),
            answer: "3".to_string(),
        },
    ];
    assert_eq!(format_card_title(&three), "One? (+2 more)");
}

fn two_questions() -> Vec<devboule_protocol::PermissionQuestion> {
    vec![
        devboule_protocol::PermissionQuestion {
            question: "Which colour?".to_string(),
            header: None,
            options: vec![
                PermissionQuestionOption {
                    label: "Green".to_string(),
                    description: None,
                },
                PermissionQuestionOption {
                    label: "Red".to_string(),
                    description: None,
                },
            ],
            multi_select: false,
            allow_other: Some(true),
            secret: None,
        },
        devboule_protocol::PermissionQuestion {
            question: "Which finish?".to_string(),
            header: None,
            options: vec![
                PermissionQuestionOption {
                    label: "Matte".to_string(),
                    description: None,
                },
                PermissionQuestionOption {
                    label: "Satin".to_string(),
                    description: None,
                },
            ],
            multi_select: false,
            allow_other: Some(true),
            secret: None,
        },
    ]
}

#[test]
fn pick_on_a_multi_question_card_matches_the_decline() {
    let rows = answered_questions(
        &request_with(two_questions()),
        &serde_json::json!({ "outcome": { "outcome": "selected", "optionId": "q1o1" } }),
    );
    assert!(
        rows.is_empty(),
        "the shapers decline a multi pick, so no row"
    );
}

#[test]
fn pick_out_of_range_names_nothing() {
    for option_id in ["q0o9", "q9o0", "q0", "pick", ""] {
        let rows = answered_questions(
            &request_with(one_question()),
            &serde_json::json!({ "outcome": { "outcome": "selected", "optionId": option_id } }),
        );
        assert!(rows.is_empty(), "option id {option_id} must yield no row");
    }
}

#[test]
fn partial_map_answers_its_subset() {
    let map = serde_json::json!({ "Which finish?": "Satin" }).to_string();
    let rows = answered_questions(
        &request_with(two_questions()),
        &serde_json::json!({ "outcome": { "outcome": "selected", "answer": map } }),
    );
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].question, "Which finish?");
    assert_eq!(rows[0].answer, "Satin");
}

#[test]
fn duplicate_texts_yield_one_row_for_the_shared_value() {
    let mut items = two_questions();
    items[1].question = "Which colour?".to_string();
    let map = serde_json::json!({ "Which colour?": "Green" }).to_string();
    let rows = answered_questions(
        &request_with(items),
        &serde_json::json!({ "outcome": { "outcome": "selected", "answer": map } }),
    );
    assert_eq!(rows.len(), 1, "one map key names one row");
    assert_eq!(rows[0].question, "Which colour?");
    assert_eq!(rows[0].answer, "Green");
}
