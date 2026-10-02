//! A pi run's failure: the sentence its own messages carry, and the hold
//! that keeps an attempt's failure until the run's own ending decides.

use serde_json::Value;

/// The failure a run would report: held from the attempt that hit it,
/// because pi emits an `agent_end` per attempt and runs on after a retry or
/// a compaction. The run's settle decides — the daemon's own roads can end
/// a turn without one — and a Stop that ended the run drops it there.
#[derive(Default)]
pub(super) struct PendingFailure {
    held: Option<String>,
    /// A prompt we wrote, until pi opens its run, refuses it, or is found to
    /// have handled it without one. A prompt an extension's input hook
    /// swallows is never found: it stays out until a run opens or settles.
    prompt_out: bool,
    /// A run pi opened that has not settled.
    run_open: bool,
    stopped: bool,
}

impl PendingFailure {
    pub(super) fn prompt_sent(&mut self) {
        self.prompt_out = true;
    }

    /// pi answered the prompt without a run of its own — a refusal, or a
    /// command it handled itself — so no run of it will settle: a Stop aimed
    /// at it must not outlive it into the next run's settle.
    pub(super) fn prompt_answered(&mut self) {
        self.prompt_out = false;
        if !self.run_open {
            self.stopped = false;
        }
    }

    /// A run's opening holds nothing an earlier run held. A Stop stands: the
    /// opening can reach the reader after the Stop that ended its run.
    pub(super) fn run_opened(&mut self) {
        self.prompt_out = false;
        self.run_open = true;
        self.held = None;
    }

    /// One attempt's ending. An attempt pi will retry is not the run's
    /// outcome: it holds nothing, and clears what an earlier attempt held.
    pub(super) fn hold(&mut self, value: &Value) {
        self.held = if value.get("willRetry").and_then(Value::as_bool) == Some(true) {
            None
        } else {
            value
                .get("messages")
                .and_then(Value::as_array)
                .and_then(|messages| failed_run_message(messages))
        };
    }

    /// The run's own ending: what it holds is its outcome unless a Stop
    /// ended it. Nothing of the run, its Stop included, outlives this.
    pub(super) fn settle(&mut self) -> Option<String> {
        let ended = std::mem::take(self);
        if ended.stopped {
            None
        } else {
            ended.held
        }
    }

    /// A user's Stop marks the run in flight, so its settle announces no
    /// failure — including one the reader dispatches after the Stop. With
    /// nothing in flight it marks nothing; the answer is whether it marked.
    /// Translated from Paseo's `interruptingTurn`
    /// (`pi/agent.ts:1280-1282, 1549-1615, 2463-2475`).
    pub(super) fn stop(&mut self) -> bool {
        let in_flight = self.prompt_out || self.run_open;
        if in_flight {
            self.stopped = true;
        }
        in_flight
    }

    /// The abort never reached pi: the run goes on, and its failure is its
    /// own to announce.
    pub(super) fn unstop(&mut self) {
        self.stopped = false;
    }
}

/// The failure sentence one run's own messages carry: the last assistant
/// message's `errorMessage` and details, at Paseo's 500-UTF-16-unit partial
/// bound (`formatPiErrorMessage`/`latestPiErrorMessage`, `pi/agent.ts:799-819`).
fn failed_run_message(messages: &[Value]) -> Option<String> {
    let message = messages
        .iter()
        .rev()
        .find(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))?;
    // A run ended by Stop, or cancelled when another command replaced it, is
    // no failure to announce.
    if message
        .get("stopReason")
        .and_then(Value::as_str)
        .is_some_and(|reason| reason.eq_ignore_ascii_case("aborted"))
    {
        return None;
    }
    let headline = message.get("errorMessage").and_then(Value::as_str)?.trim();
    if headline.is_empty() {
        return None;
    }
    let field = |key: &str| {
        message
            .get(key)
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
    };
    let mut details = Vec::new();
    if let Some(stop_reason) = field("stopReason") {
        details.push(format!("stopReason={stop_reason}"));
    }
    if let (Some(provider), Some(model)) = (field("provider"), field("model")) {
        details.push(format!("model={provider}/{model}"));
    }
    if let Some(response_model) = field("responseModel") {
        details.push(format!("responseModel={response_model}"));
    }
    if let Some(response_id) = field("responseId") {
        details.push(format!("responseId={response_id}"));
    }
    if let Some(partial) = failure_partial_text(message) {
        details.push(format!(
            "partial={}",
            Value::String(utf16_prefix(&partial, 500))
        ));
    }
    if details.is_empty() {
        return Some(headline.to_string());
    }
    Some(format!("{headline} ({})", details.join(", ")))
}

/// The first `max_units` UTF-16 code units of `value`. A cut between a
/// surrogate pair keeps the lead unit, which no `String` can hold: the lead
/// goes with the cut.
fn utf16_prefix(value: &str, max_units: usize) -> String {
    let mut units: Vec<u16> = value.encode_utf16().take(max_units + 1).collect();
    if units.len() > max_units {
        units.truncate(max_units);
        if units
            .last()
            .is_some_and(|unit| (0xD800..0xDC00).contains(unit))
        {
            units.pop();
        }
    }
    String::from_utf16_lossy(&units)
}

/// The partial answer one failure sentence quotes: the message's `text`
/// and `thinking` parts in order, joined by a blank line and trimmed;
/// `None` when no part carries words. A typed part with no string keeps its
/// empty slot, so the separators a `flatMap` + `join` renders stay.
fn failure_partial_text(message: &Value) -> Option<String> {
    let parts = message.get("content").and_then(Value::as_array)?;
    let joined = parts
        .iter()
        .filter_map(|part| match part.get("type").and_then(Value::as_str) {
            Some("text") => Some(part.get("text").and_then(Value::as_str).unwrap_or_default()),
            Some("thinking") => Some(
                part.get("thinking")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            ),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n\n");
    let trimmed = joined.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

#[cfg(test)]
#[path = "pi_run_failure_tests.rs"]
mod tests;
