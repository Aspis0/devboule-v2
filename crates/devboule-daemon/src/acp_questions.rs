//! grok's `_x.ai/ask_user_question`: the vendor question carrier on the ACP
//! road — parsing the model's items into a broker card and shaping the
//! person's answer back into grok's `{outcome, answers}` result.
//!
//! The request frame is confirmed from our own journal (grok 1.0.25/26):
//! `params` carries `{sessionId, toolCallId, questions[], mode}`. Newer grok
//! (1.0.40) sends the same fields directly under the method, without the
//! `params` envelope. Both shapes are accepted. The reply follows grok's own
//! response enum, measured live: `accepted` with answers keyed by full
//! question text — a free-text answer travels verbatim as its label — and
//! the `skip_interview` decline variant for a dismiss, a close, or a refusal.

use std::collections::BTreeMap;

use devboule_protocol::{
    PermissionOption, PermissionQuestion, PermissionQuestionOption, PermissionRequestKind,
    SessionEvent,
};
use serde_json::Value;

use super::codex_input_requests::generated_card_id;

/// One question grok asked the person: text plus the offered labels. There
/// is no per-question id on this wire — answers are keyed by the full
/// question text.
pub(super) struct GrokQuestion {
    pub(super) question: String,
    pub(super) options: Vec<GrokQuestionOption>,
    pub(super) multi_select: bool,
}

pub(super) struct GrokQuestionOption {
    pub(super) label: String,
    pub(super) description: Option<String>,
}

/// The payload of an `_x.ai/ask_user_question` frame: the enveloped `params`
/// when it carries the fields, otherwise the frame itself (the unenveloped
/// shape newer grok sends).
pub(super) fn grok_payload(value: &Value) -> &Value {
    match value.get("params") {
        Some(params)
            if params.is_object()
                && (params.get("questions").is_some() || params.get("toolCallId").is_some()) =>
        {
            params
        }
        _ => value,
    }
}

/// The model's questions out of the payload. An item without text is not a
/// question anyone could answer, so it is skipped; options without a label
/// go the same way. `multiSelect: null` (what grok actually sends for a
/// single pick) reads as false, like an absent flag.
pub(super) fn parse_grok_questions(payload: &Value) -> Vec<GrokQuestion> {
    let Some(items) = payload.get("questions").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut questions = Vec::with_capacity(items.len());
    for item in items {
        let text = item
            .get("question")
            .and_then(Value::as_str)
            .or_else(|| item.get("header").and_then(Value::as_str))
            .map(str::trim)
            .unwrap_or("");
        if text.is_empty() {
            continue;
        }
        let mut options = Vec::new();
        if let Some(raw) = item.get("options").and_then(Value::as_array) {
            for option in raw {
                let label = option
                    .get("label")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .unwrap_or("");
                if label.is_empty() {
                    continue;
                }
                options.push(GrokQuestionOption {
                    label: label.to_string(),
                    description: option
                        .get("description")
                        .and_then(Value::as_str)
                        .filter(|description| !description.trim().is_empty())
                        .map(str::to_string),
                });
            }
        }
        questions.push(GrokQuestion {
            question: text.to_string(),
            options,
            multi_select: item
                .get("multiSelect")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        });
    }
    questions
}

/// A grok `kind: "question"` card: the agent's own option labels, one broker
/// option per label with its position encoded for the reply, the question
/// text as title. Field bounds are enforced by the broker's own validation
/// at registration, so an oversize frame is refused there, not trimmed here.
pub(super) fn grok_question_event(
    tool_call_id: String,
    questions: &[GrokQuestion],
) -> SessionEvent {
    let mut options = Vec::new();
    for (question_index, question) in questions.iter().enumerate() {
        for (option_index, option) in question.options.iter().enumerate() {
            options.push(PermissionOption {
                option_id: format!("q{question_index}o{option_index}"),
                name: option.label.clone(),
                kind: "allow_once".to_string(),
            });
        }
    }
    let labels: Vec<&str> = questions[0]
        .options
        .iter()
        .map(|option| option.label.as_str())
        .collect();
    SessionEvent::PermissionRequest {
        tool_call_id,
        title: questions[0].question.clone(),
        description: if labels.is_empty() {
            None
        } else {
            Some(labels.join(" / "))
        },
        command: None,
        args: None,
        cwd: None,
        env: None,
        options,
        is_chooser: Some(false),
        kind: Some(PermissionRequestKind::Question),
        questions: Some(
            questions
                .iter()
                .map(|question| PermissionQuestion {
                    question: question.question.clone(),
                    header: None,
                    options: question
                        .options
                        .iter()
                        .map(|option| PermissionQuestionOption {
                            label: option.label.clone(),
                            description: option.description.clone(),
                        })
                        .collect(),
                    multi_select: question.multi_select,
                    allow_other: None,
                    secret: None,
                })
                .collect(),
        ),
        // A placeholder the daemon overwrites with the session's stored
        // origin before the request leaves for a subscriber.
        origin: devboule_protocol::SessionOrigin::unknown(),
        create_agent: None,
    }
}

/// grok's decline answer: the `skip_interview` response variant, on its
/// own. One constructor so every refusal path spells it the same way.
pub(super) fn grok_skip_interview() -> Value {
    serde_json::json!({ "outcome": "skip_interview" })
}

/// The reply to an `_x.ai/ask_user_question` request: `{outcome, answers}`
/// with **labels** on allow, grok's `skip_interview` decline variant on
/// anything else. An option pick decodes the position the card encoded; a
/// text answer is the value verbatim for one question, or a JSON
/// text-to-value map for several (multi-selects split back on commas, the
/// card's own join). Anything unmappable refuses with the decline variant
/// rather than answering what nobody chose.
pub(super) fn grok_question_result(params: &Value, result: &Value) -> Value {
    let outcome = result
        .pointer("/outcome/outcome")
        .and_then(Value::as_str)
        .unwrap_or("");
    if outcome == "selected" {
        let questions = parse_grok_questions(params);
        let option_id = result
            .pointer("/outcome/optionId")
            .and_then(Value::as_str)
            .unwrap_or("");
        let answer = result
            .pointer("/outcome/answer")
            .and_then(Value::as_str)
            .unwrap_or("");
        if let Some(answers) = grok_question_answers(&questions, option_id, answer) {
            let mut map = serde_json::Map::new();
            for (question, labels) in answers {
                map.insert(question, serde_json::json!(labels));
            }
            return serde_json::json!({ "outcome": "accepted", "answers": map });
        }
    }
    grok_skip_interview()
}

fn grok_question_answers(
    questions: &[GrokQuestion],
    option_id: &str,
    answer: &str,
) -> Option<BTreeMap<String, Vec<String>>> {
    if questions.is_empty() {
        return None;
    }
    if !answer.is_empty() {
        if questions.len() == 1 {
            return Some(BTreeMap::from([(
                questions[0].question.clone(),
                grok_answer_labels(&questions[0], answer),
            )]));
        }
        let parsed: BTreeMap<String, String> = serde_json::from_str(answer).ok()?;
        let mut answers = BTreeMap::new();
        for (text, value) in parsed {
            let question = questions.iter().find(|item| item.question == text)?;
            if value.trim().is_empty() {
                return None;
            }
            answers.insert(
                question.question.clone(),
                grok_answer_labels(question, &value),
            );
        }
        return if answers.is_empty() {
            None
        } else {
            Some(answers)
        };
    }
    // One pick answers one question: several questions are always answered
    // together through the text map above.
    if questions.len() != 1 {
        return None;
    }
    let rest = option_id.strip_prefix('q')?;
    let (question, option) = rest.split_once('o')?;
    let picked = questions
        .get(question.parse::<usize>().ok()?)?
        .options
        .get(option.parse::<usize>().ok()?)?;
    Some(BTreeMap::from([(
        questions[0].question.clone(),
        vec![picked.label.clone()],
    )]))
}

fn grok_answer_labels(question: &GrokQuestion, value: &str) -> Vec<String> {
    if !question.multi_select {
        return vec![value.to_string()];
    }
    let labels: Vec<String> = value
        .split(',')
        .map(str::trim)
        .filter(|label| !label.is_empty())
        .map(str::to_string)
        .collect();
    if labels.is_empty() {
        vec![value.trim().to_string()]
    } else {
        labels
    }
}

/// A card id the provider did not choose, in the same shape as the sibling
/// carriers: unique across spawns of one session, since the journal's
/// reused-id ledger is sticky per session.
pub(super) fn grok_card_id() -> String {
    generated_card_id("grok-question")
}
