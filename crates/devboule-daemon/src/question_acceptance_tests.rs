//! Every refusal the acceptance rule knows, and the shapes it accepts.

use super::*;
use devboule_protocol::PermissionQuestionOption;

fn question(text: &str, labels: &[&str]) -> PermissionQuestion {
    PermissionQuestion {
        question: text.to_string(),
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
    }
}

fn one() -> Vec<PermissionQuestion> {
    vec![question("Which colour?", &["Green", "Red"])]
}

fn two() -> Vec<PermissionQuestion> {
    vec![
        question("Which colour?", &["Green", "Red"]),
        question("Which finish?", &["Matte", "Satin"]),
    ]
}

fn map(entries: &[(&str, &str)]) -> String {
    let map: BTreeMap<&str, &str> = entries.iter().copied().collect();
    serde_json::to_string(&map).expect("map")
}

#[test]
fn empty_map_is_refused() {
    assert_eq!(
        validate_answer(&two(), None, Some("{}")),
        Err("a multi-question answer must name at least one question")
    );
}

#[test]
fn every_text_refusal_names_its_reason() {
    let cases = [
        (one(), "", "a question's text answer must not be blank"),
        (one(), "  \n", "a question's text answer must not be blank"),
        (
            two(),
            &map(&[("Which colour?", " "), ("Which finish?", "Satin")]),
            "a multi-question answer must not hold a blank value",
        ),
        (
            two(),
            &map(&[("Which colour?", "Green"), ("Something else?", "Satin")]),
            "a multi-question answer must not name an unknown question",
        ),
        (
            two(),
            "Green",
            "a multi-question answer must map question text to answers",
        ),
        (
            two(),
            r#"{"Which colour?": 1}"#,
            "a multi-question answer must map question text to answers",
        ),
        (
            Vec::new(),
            "Green",
            "a question card with no questions cannot be answered",
        ),
    ];
    for (questions, answer, reason) in cases {
        assert_eq!(
            validate_answer(&questions, None, Some(answer)),
            Err(reason),
            "answer {answer:?}"
        );
    }
}

#[test]
fn every_pick_refusal_names_its_reason() {
    let cases = [
        (
            one(),
            "q0o2",
            "the pick names no option the question offered",
        ),
        (
            one(),
            "q1o0",
            "the pick names no option the question offered",
        ),
        (one(), "q0", "the pick names no option the question offered"),
        (
            one(),
            "allow",
            "the pick names no option the question offered",
        ),
        (
            two(),
            "q1o0",
            "a multi-question card is answered with a text map, not a pick",
        ),
        (
            Vec::new(),
            "q0o0",
            "a question card with no questions cannot be answered",
        ),
    ];
    for (questions, option_id, reason) in cases {
        assert_eq!(
            validate_answer(&questions, Some(option_id), None),
            Err(reason),
            "pick {option_id}"
        );
    }
    assert_eq!(
        validate_answer(&one(), None, None),
        Err("a question grant must name the picked option or carry its text")
    );
}

#[test]
fn accepted_shapes_pass() {
    assert_eq!(validate_answer(&one(), None, Some("Teal")), Ok(()));
    assert_eq!(validate_answer(&one(), Some("q0o1"), None), Ok(()));
    let both = map(&[("Which colour?", "Green"), ("Which finish?", "Satin")]);
    assert_eq!(validate_answer(&two(), None, Some(&both)), Ok(()));
    let partial = map(&[("Which finish?", "Satin")]);
    assert_eq!(validate_answer(&two(), None, Some(&partial)), Ok(()));
}
