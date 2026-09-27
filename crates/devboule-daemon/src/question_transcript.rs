//! An answered model question as a tool row's text, formatted from the same
//! accepted answer the provider reply shapers format, so the row matches
//! what the model was told. Redaction happens here, before any
//! transcript text is built; the provider's own reply frame is shaped
//! elsewhere from the unredacted result.

use std::collections::BTreeMap;

use devboule_protocol::{PermissionRequestKind, SessionEvent};
use serde_json::Value;

/// Marker shown where a secret answer would go.
const HIDDEN_ANSWER: &str = "(hidden)";

/// The tool row's kind: excluded from tool-call grouping, so the record is
/// never buried in a run. No existing icon fits a question; the row keeps
/// the default.
pub(super) const QUESTION_ROW_KIND: &str = "question";

/// The tool row's title for an answered card: the first answered question,
/// with a count when more were answered — the collapsed line names only
/// what the row shows.
pub(super) fn format_card_title(answered: &[AnsweredQuestion]) -> String {
    match answered {
        [] => "Question".to_string(),
        [only] => only.question.clone(),
        [first, rest @ ..] => format!("{} (+{} more)", first.question, rest.len()),
    }
}

/// One answered question, ready to format: the question's words and the
/// answer as shown (redacted when the question asked for secrecy).
pub(super) struct AnsweredQuestion {
    pub(super) question: String,
    pub(super) answer: String,
}

/// The transcript entries for an answered question card. The broker only
/// completes what `question_acceptance` accepted, so this formats: a subset
/// answers its subset, and duplicate texts share one value under their
/// first card match. Anything but a granted `question` yields nothing — a
/// dismissal or a tool adds no row of ours.
pub(super) fn answered_questions(request: &SessionEvent, result: &Value) -> Vec<AnsweredQuestion> {
    let SessionEvent::PermissionRequest {
        kind, questions, ..
    } = request
    else {
        return Vec::new();
    };
    if !matches!(kind, Some(PermissionRequestKind::Question)) {
        return Vec::new();
    }
    let Some(questions) = questions.as_deref() else {
        return Vec::new();
    };
    if questions.is_empty() {
        return Vec::new();
    }
    if result.pointer("/outcome/outcome").and_then(Value::as_str) != Some("selected") {
        return Vec::new();
    }
    let option_id = result
        .pointer("/outcome/optionId")
        .and_then(Value::as_str)
        .unwrap_or("");
    let answer = result
        .pointer("/outcome/answer")
        .and_then(Value::as_str)
        .unwrap_or("");
    if !answer.is_empty() {
        answers_from_text(questions, answer)
    } else if !option_id.is_empty() {
        answers_from_pick(questions, option_id)
    } else {
        Vec::new()
    }
}

/// The display answer for one question: the words unless secrecy hides them.
fn shown_answer(secret: bool, words: &str) -> String {
    if secret {
        HIDDEN_ANSWER.to_string()
    } else {
        words.to_string()
    }
}

fn answers_from_text(
    questions: &[devboule_protocol::PermissionQuestion],
    answer: &str,
) -> Vec<AnsweredQuestion> {
    if questions.len() == 1 {
        let question = &questions[0];
        return vec![AnsweredQuestion {
            question: question.question.clone(),
            answer: shown_answer(question.secret == Some(true), answer),
        }];
    }
    let parsed: BTreeMap<String, String> = match serde_json::from_str(answer) {
        Ok(parsed) => parsed,
        Err(_) => return Vec::new(),
    };
    // Duplicate texts share the map's one value, named by its first card
    // match like the shapers' keying.
    let mut rows: Vec<AnsweredQuestion> = parsed
        .iter()
        .filter_map(|(key, value)| {
            let question = questions
                .iter()
                .find(|question| &question.question == key)?;
            Some(AnsweredQuestion {
                question: question.question.clone(),
                answer: shown_answer(question.secret == Some(true), value),
            })
        })
        .collect();
    rows.sort_by_key(|row| {
        questions
            .iter()
            .position(|question| question.question == row.question)
            .unwrap_or(usize::MAX)
    });
    rows
}

fn answers_from_pick(
    questions: &[devboule_protocol::PermissionQuestion],
    option_id: &str,
) -> Vec<AnsweredQuestion> {
    // Picks travel one question at a time on every provider road; a pick on
    // a multi-question card is declined there, so it yields no row here.
    if questions.len() != 1 {
        return Vec::new();
    }
    let rest = match option_id.strip_prefix('q') {
        Some(rest) => rest,
        None => return Vec::new(),
    };
    let (asked, offered) = match rest.split_once('o') {
        Some(pair) => pair,
        None => return Vec::new(),
    };
    // Both halves index the same question: the text it asked and the label
    // it offered travel together, so a relaxed guard cannot pair one
    // question's words with another's label.
    let question = asked
        .parse::<usize>()
        .ok()
        .and_then(|index| questions.get(index));
    let picked = offered
        .parse::<usize>()
        .ok()
        .and_then(|index| question?.options.get(index));
    let (Some(question), Some(picked)) = (question, picked) else {
        return Vec::new();
    };
    vec![AnsweredQuestion {
        question: question.question.clone(),
        answer: shown_answer(question.secret == Some(true), &picked.label),
    }]
}

/// One block of a tool row's output: the question's words and the answer.
pub(super) fn format_question_answer(question: &str, answer: &str) -> String {
    format!("Question: {question}\nAnswer: {answer}")
}

/// The output text for an answered card's tool row: every answered question
/// in card order, each as its own labelled block.
pub(super) fn format_tool_output(answered: &[AnsweredQuestion]) -> String {
    answered
        .iter()
        .map(|row| format_question_answer(&row.question, &row.answer))
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
#[path = "question_transcript_tests.rs"]
mod tests;
