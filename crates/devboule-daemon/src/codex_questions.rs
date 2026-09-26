//! Codex `requestUserInput` questions: parsing the model's items into broker
//! cards and shaping the answers map back.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use devboule_protocol::{
    NoticeSeverity, PermissionOption, PermissionQuestion, PermissionQuestionOption,
    PermissionRequestKind, SessionEvent,
};
use serde_json::Value;

use super::codex_input_requests::{
    generated_card_id, send_result, CodexInputDeps, CodexPendingKind, CodexPendingResponse,
};
use super::permission_broker::PermissionResponseError;
use super::SessionRuntime;

/// One question a Codex agent asked the person, as `requestUserInput`
/// carries it: the `id` keys the answers map, `header` is the short label.
pub(super) struct CodexQuestion {
    pub(super) id: String,
    pub(super) header: String,
    pub(super) question: String,
    pub(super) options: Vec<CodexQuestionOption>,
    pub(super) multi_select: bool,
    pub(super) allow_other: bool,
    pub(super) secret: bool,
}

pub(super) struct CodexQuestionOption {
    pub(super) label: String,
    pub(super) description: Option<String>,
}

/// The model's questions out of a `requestUserInput` params object. An item
/// without an id, header or text is not a question anyone could answer, so
/// it is skipped; options without a label go the same way.
pub(super) fn parse_codex_questions(params: &Value) -> Vec<CodexQuestion> {
    let Some(items) = params.get("questions").and_then(Value::as_array) else {
        return Vec::new();
    };
    let mut questions = Vec::with_capacity(items.len());
    for item in items {
        let id = item
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        let header = item
            .get("header")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        let text = item
            .get("question")
            .and_then(Value::as_str)
            .map(str::trim)
            .unwrap_or("");
        if id.is_empty() || header.is_empty() || text.is_empty() {
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
                options.push(CodexQuestionOption {
                    label: label.to_string(),
                    description: option
                        .get("description")
                        .and_then(Value::as_str)
                        .filter(|description| !description.trim().is_empty())
                        .map(str::to_string),
                });
            }
        }
        questions.push(CodexQuestion {
            id: id.to_string(),
            header: header.to_string(),
            question: text.to_string(),
            options,
            multi_select: item
                .get("multiSelect")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            allow_other: item
                .get("isOther")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            secret: item
                .get("isSecret")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        });
    }
    questions
}

/// The reply to a `requestUserInput` request: `{answers: {id: {answers}}}`
/// on allow, `{answers: {}}` on anything else. An option pick decodes the
/// position the card encoded; a text answer is the value verbatim for one
/// question, or a JSON text-to-value map for several (multi-selects split
/// back on commas, the card's own join). Anything unmappable refuses with
/// the empty map rather than answering what nobody chose.
pub(super) fn codex_question_result(params: &Value, result: &Value) -> Value {
    let outcome = result
        .pointer("/outcome/outcome")
        .and_then(Value::as_str)
        .unwrap_or("");
    if outcome == "selected" {
        let questions = parse_codex_questions(params);
        let option_id = result
            .pointer("/outcome/optionId")
            .and_then(Value::as_str)
            .unwrap_or("");
        let answer = result
            .pointer("/outcome/answer")
            .and_then(Value::as_str)
            .unwrap_or("");
        if let Some(answers) = codex_question_answers(&questions, option_id, answer) {
            let mut map = serde_json::Map::new();
            for (id, labels) in answers {
                map.insert(id, serde_json::json!({ "answers": labels }));
            }
            return serde_json::json!({ "answers": map });
        }
    }
    serde_json::json!({ "answers": {} })
}

fn codex_question_answers(
    questions: &[CodexQuestion],
    option_id: &str,
    answer: &str,
) -> Option<std::collections::BTreeMap<String, Vec<String>>> {
    if questions.is_empty() {
        return None;
    }
    if !answer.is_empty() {
        if questions.len() == 1 {
            return Some(std::collections::BTreeMap::from([(
                questions[0].id.clone(),
                codex_answer_labels(&questions[0], answer),
            )]));
        }
        let parsed: std::collections::BTreeMap<String, String> =
            serde_json::from_str(answer).ok()?;
        let mut answers = std::collections::BTreeMap::new();
        for (text, value) in parsed {
            let question = questions.iter().find(|item| item.question == text)?;
            if value.trim().is_empty() {
                return None;
            }
            answers.insert(question.id.clone(), codex_answer_labels(question, &value));
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
    Some(std::collections::BTreeMap::from([(
        questions[0].id.clone(),
        vec![picked.label.clone()],
    )]))
}

fn codex_answer_labels(question: &CodexQuestion, value: &str) -> Vec<String> {
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

/// A Codex `requestUserInput` (current or legacy spelling): the model's
/// questions become a `kind: "question"` card answered with Paseo's
/// `{answers: {id: {answers}}}` shape. Nothing parseable means nothing
/// the person could answer, so the id is answered now, not carded.
pub(super) fn dispatch_question(
    deps: &CodexInputDeps,
    value: &Value,
    runtime: &Arc<SessionRuntime>,
    seq: Option<u64>,
) {
    let params = value.get("params").cloned().unwrap_or(Value::Null);
    let questions = parse_codex_questions(&params);
    if questions.is_empty() {
        if let Some(id) = value.get("id") {
            let _ = send_result(&deps.stdin, id, serde_json::json!({ "answers": {} }));
        }
        return;
    }
    let broker_id = deps.next_id.fetch_add(1, Ordering::Relaxed);
    let tool_call_id = params
        .get("itemId")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| generated_card_id("codex-question"));
    let labels: Vec<&str> = questions[0]
        .options
        .iter()
        .map(|option| option.label.as_str())
        .collect();
    // One broker option per offered label, its id encoding the position
    // the reply decodes; a free-text-only question offers none, and the
    // card answers through the text door instead.
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
    let event = SessionEvent::PermissionRequest {
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
        is_chooser: None,
        kind: Some(PermissionRequestKind::Question),
        questions: Some(
            questions
                .iter()
                .map(|question| PermissionQuestion {
                    question: question.question.clone(),
                    header: Some(question.header.clone()),
                    options: question
                        .options
                        .iter()
                        .map(|option| PermissionQuestionOption {
                            label: option.label.clone(),
                            description: option.description.clone(),
                        })
                        .collect(),
                    multi_select: question.multi_select,
                    allow_other: Some(question.allow_other),
                    secret: Some(question.secret),
                })
                .collect(),
        ),
        // A placeholder the daemon overwrites with the session's stored
        // origin before the request leaves for a subscriber.
        origin: devboule_protocol::SessionOrigin::unknown(),
        create_agent: None,
    };
    if let Ok(mut ids) = deps.response_ids.lock() {
        if let Some(id) = value.get("id") {
            ids.insert(
                broker_id,
                CodexPendingResponse {
                    id: id.clone(),
                    kind: CodexPendingKind::Question,
                    params: params.clone(),
                },
            );
        }
    }
    if let Err(error) = deps
        .permission_broker
        .register(broker_id, event.clone(), runtime)
    {
        deps.response_ids
            .lock()
            .ok()
            .map(|mut ids| ids.remove(&broker_id));
        if let Some(id) = value.get("id") {
            let _ = send_result(&deps.stdin, id, serde_json::json!({ "answers": {} }));
        }
        if !matches!(error, PermissionResponseError::AlreadyRecorded) {
            // The repeat-refusal's one plain notice is already up; only
            // the empty answers go back for that case.
            let _ = runtime.publish_session_notice(
                format!("Could not queue Codex question: {error}"),
                NoticeSeverity::Warning,
            );
        }
        return;
    }
    let _ = runtime.publish_agent_event_with_seq(event, None, seq);
}
