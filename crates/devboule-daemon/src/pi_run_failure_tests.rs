//! Tests for the failure predicate: which message fails a run and the
//! sentence it reads as.

use super::failed_run_message;

/// The failure sentence one attempt's messages carry — the predicate the
/// reader holds from an `agent_end` until the run settles.
fn sentence_of(messages: serde_json::Value) -> Option<String> {
    failed_run_message(messages.as_array().expect("messages"))
}

/// The failed assistant message of the recorded shape: the predicate's
/// subject.
fn failed_message() -> serde_json::Value {
    serde_json::json!({
        "role": "assistant",
        "content": [{"type": "text", "text": "I was about to finish"}],
        "provider": "openrouter",
        "model": "z-ai/glm-5.3-flash",
        "responseId": "gen-1788993862",
        "responseModel": "glm-5.3-flash",
        "errorMessage": "Request timed out.",
        "stopReason": "error",
    })
}

fn failure_sentence() -> String {
    "Request timed out. (stopReason=error, model=openrouter/z-ai/glm-5.3-flash, responseModel=glm-5.3-flash, responseId=gen-1788993862, partial=\"I was about to finish\")".to_string()
}

#[test]
fn the_runs_last_assistant_message_is_the_one_judged() {
    // The message list's tail is a tool result, not the failure: the scan
    // has to reach the last assistant message behind it.
    let mut tailed = vec![failed_message()];
    tailed.push(serde_json::json!({
        "role": "toolResult",
        "content": [{"type": "text", "text": "t"}],
    }));
    assert_eq!(
        sentence_of(serde_json::Value::Array(tailed)),
        Some(failure_sentence())
    );
    // A continuation's own assistant message becomes the last one, so the
    // failure it does not carry is what clears the attempt's.
    assert_eq!(
        sentence_of(serde_json::json!([
            failed_message(),
            {"role": "assistant", "content": [{"type": "text", "text": "the answer"}], "stopReason": "stop"},
        ])),
        None
    );
}

#[test]
fn a_stop_reason_error_alone_is_no_failure() {
    assert_eq!(
        sentence_of(
            serde_json::json!([{"role": "assistant", "content": [], "stopReason": "error"}])
        ),
        None
    );
    assert_eq!(
        sentence_of(serde_json::json!([
            {"role": "assistant", "content": [], "errorMessage": "   ", "stopReason": "error"},
        ])),
        None,
        "a whitespace-only errorMessage is no failure"
    );
    assert_eq!(
        sentence_of(
            serde_json::json!([{"role": "assistant", "content": [], "stopReason": "stop"}])
        ),
        None
    );
    assert_eq!(
        failed_run_message(&[]),
        None,
        "a frame with no message list fails nothing"
    );
}

#[test]
fn an_aborted_message_is_no_failure() {
    // The terminal response of an interrupted or replaced run keeps the
    // provider's own error text beside the `aborted` stop.
    assert_eq!(
        sentence_of(serde_json::json!([{
            "role": "assistant",
            "content": [{"type": "text", "text": "half an answer"}],
            "errorMessage": "OpenAI Responses stream ended before a terminal response event",
            "stopReason": "aborted",
        }])),
        None
    );
}

#[test]
fn only_an_assistant_message_can_fail_the_turn() {
    // The role is part of the predicate: only an assistant message can
    // carry the run's failure.
    assert_eq!(
        sentence_of(serde_json::json!([{
            "role": "user",
            "content": [],
            "errorMessage": "Request timed out.",
            "stopReason": "error",
        }])),
        None
    );
}

fn sentence_of_partial(partial: &str) -> String {
    sentence_of(serde_json::json!([{
        "role": "assistant",
        "content": [{"type": "text", "text": partial}],
        "errorMessage": "Request timed out.",
        "stopReason": "error",
    }]))
    .expect("a failure sentence")
}

#[test]
fn the_partial_quote_stops_at_five_hundred_utf16_units() {
    let tail = |body: &str| format!("partial=\"{body}\")");
    let one_byte = sentence_of_partial(&"a".repeat(501));
    assert!(one_byte.ends_with(&tail(&"a".repeat(500))), "{one_byte}");
    // An astral character is two units: 251 of them cut at the 500th
    // unit, a pair boundary.
    let astral = sentence_of_partial(&"🦀".repeat(251));
    assert!(astral.ends_with(&tail(&"🦀".repeat(250))), "{astral}");
    // One BMP unit in front shifts the cut between a pair: the lead unit
    // goes, so the quote never ends in half a character.
    let shifted = sentence_of_partial(&format!("a{}", "🦀".repeat(251)));
    assert!(
        shifted.ends_with(&tail(&format!("a{}", "🦀".repeat(249)))),
        "{shifted}"
    );
}

#[test]
fn a_typed_part_with_no_words_keeps_its_blank_line() {
    // A part of a quoted type with no string keeps its slot, so the
    // separators match a `flatMap` + `join` over the parts.
    assert_eq!(
        sentence_of(serde_json::json!([{
            "role": "assistant",
            "content": [
                {"type": "text", "text": "first"},
                {"type": "text"},
                {"type": "thinking", "thinking": "last"},
            ],
            "errorMessage": "Request timed out.",
            "stopReason": "error",
        }])),
        Some(r#"Request timed out. (stopReason=error, partial="first\n\n\n\nlast")"#.to_string())
    );
}
