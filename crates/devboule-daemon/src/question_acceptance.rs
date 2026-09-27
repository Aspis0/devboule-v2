//! Whether a grant may complete a question card. The broker asks before it
//! takes the card, so a refused answer leaves no `permissions` row, no
//! provider frame and no tool row, and the card stays open. The reply
//! shapers and the transcript mapping format only what this accepted.

use std::collections::BTreeMap;

use devboule_protocol::PermissionQuestion;

/// One card answered by a pick (`q{question}o{option}`, the position the
/// card encoded) or by text: the value verbatim for a single question, a
/// JSON text-to-value map for several. With both, text wins, as it does in
/// every shaper; the broker refuses that pair before asking.
pub(super) fn validate_answer(
    questions: &[PermissionQuestion],
    option_id: Option<&str>,
    answer: Option<&str>,
) -> Result<(), &'static str> {
    let [first, rest @ ..] = questions else {
        return Err("a question card with no questions cannot be answered");
    };
    match (answer, option_id) {
        (Some(answer), _) if rest.is_empty() => {
            if is_blank(answer) {
                return Err("a question's text answer must not be blank");
            }
            Ok(())
        }
        (Some(answer), _) => validate_map(questions, answer),
        (None, Some(_)) if !rest.is_empty() => {
            Err("a multi-question card is answered with a text map, not a pick")
        }
        (None, Some(option_id)) => {
            if picked_index(option_id).is_some_and(|index| index < first.options.len()) {
                Ok(())
            } else {
                Err("the pick names no option the question offered")
            }
        }
        (None, None) => Err("a question grant must name the picked option or carry its text"),
    }
}

fn validate_map(questions: &[PermissionQuestion], answer: &str) -> Result<(), &'static str> {
    let parsed: BTreeMap<String, String> = serde_json::from_str(answer)
        .map_err(|_| "a multi-question answer must map question text to answers")?;
    if parsed.is_empty() {
        return Err("a multi-question answer must name at least one question");
    }
    for (key, value) in &parsed {
        if !questions.iter().any(|question| &question.question == key) {
            return Err("a multi-question answer must not name an unknown question");
        }
        if is_blank(value) {
            return Err("a multi-question answer must not hold a blank value");
        }
    }
    Ok(())
}

/// The option position of a pick on the card's only question, decoded the
/// way every shaper decodes it.
fn picked_index(option_id: &str) -> Option<usize> {
    let (asked, offered) = option_id.strip_prefix('q')?.split_once('o')?;
    if asked.parse::<usize>().ok()? != 0 {
        return None;
    }
    offered.parse::<usize>().ok()
}

/// Whitespace-only counts as no answer.
fn is_blank(value: &str) -> bool {
    value.trim().is_empty()
}

#[cfg(test)]
#[path = "question_acceptance_tests.rs"]
mod tests;
