//! Turn completion: the `result` envelope's finish and usage, the context
//! reading, and the running-total cost latch measured against
//! `CostBaseline`.

use devboule_protocol::{SessionEvent, TurnUsage};
use serde_json::Value;

use super::ClaudeView;
use super::CostBaseline;

impl ClaudeView {
    pub(super) fn ingest_result(&mut self, envelope: &Value) -> Vec<SessionEvent> {
        self.question_tool_ids.clear();
        self.plan_tool_ids.clear();
        self.open_tools.clear();
        self.last_tool_start = None;
        // Unmatched task-tool inputs die with the turn: a result never
        // arrives after its turn's end. The list itself is session state.
        self.task_state.end_turn();
        // An error result's `stop_reason` is whatever the failing call was
        // mid-way through, not a reason, so the error markers decide.
        let stop_reason = if envelope.get("is_error").and_then(Value::as_bool) == Some(true) {
            if is_interrupted_result(envelope) {
                "interrupted".to_string()
            } else {
                "error".to_string()
            }
        } else {
            envelope
                .get("stop_reason")
                .and_then(Value::as_str)
                .unwrap_or("end_turn")
                .to_string()
        };
        let model_id = envelope
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| self.current_model.clone());
        let turn_cost = self.turn_cost(total_cost_from_result(envelope));
        let (mut usage, context_used) = match envelope.get("usage").and_then(usage_from_claude) {
            Some(parsed) => (Some(parsed.turn), parsed.context_used),
            // A cost with no counters is still the turn's billing: the
            // finish carries it alone rather than dropping it with the
            // absent usage object.
            None => (
                turn_cost.map(|cost_usd| TurnUsage {
                    input_tokens: None,
                    output_tokens: None,
                    total_tokens: None,
                    thought_tokens: None,
                    cache_read_tokens: None,
                    cache_write_tokens: None,
                    cost_usd: Some(cost_usd),
                }),
                None,
            ),
        };
        if let Some(usage) = usage.as_mut() {
            usage.cost_usd = turn_cost;
        }
        let suppress_finish = self.withheld_finish_pending;
        self.withheld_finish_pending = false;
        let mut events = if suppress_finish {
            Vec::new()
        } else {
            vec![SessionEvent::AgentFinished {
                stop_reason,
                model_id: model_id.clone(),
                usage,
            }]
        };
        if let Some(used_tokens) = context_used {
            let max_tokens = context_window_from_model_usage(envelope, model_id.as_deref());
            events.push(SessionEvent::ContextUsage {
                model_id,
                used_tokens,
                max_tokens,
                live: false,
            });
        }
        events
    }

    /// This turn's cost from the result's `total_cost_usd` — the CLI
    /// process's **running total** (a conversation reset zeroes it, per
    /// Anthropic's agent SDK), so the turn is billed with the delta against
    /// the last result. A total below the latch means the totals restarted
    /// and the turn's cost is the new total itself. Each root init zeroes
    /// the baseline with the new process.
    fn turn_cost(&mut self, total: Option<f64>) -> Option<f64> {
        let total = crate::usage_cost::finite_cost(total?)?;
        let previous = match self.cost_baseline {
            CostBaseline::Known(previous) => previous.unwrap_or(0.0),
            // An unreadable baseline cannot verify this delta; its own total
            // is the baseline every later delta needs, so latch it and
            // recover instead of staying unreadable forever.
            CostBaseline::Unknown => {
                self.cost_baseline = CostBaseline::Known(Some(total));
                return None;
            }
        };
        let turn = if total >= previous {
            total - previous
        } else {
            total
        };
        self.cost_baseline = CostBaseline::Known(Some(total));
        crate::usage_cost::finite_cost(turn)
    }
}

/// Whether a `result` envelope is an interrupted turn's, measured on the live
/// CLI (2.1.284): `is_error` true with `terminal_reason` `aborted_streaming`
/// — its `stop_reason` field carried the dead call's `tool_use`, so only the
/// terminal reason identifies the abort. An `is_error` result with any other
/// terminal reason is a genuine failure, not an interrupt.
pub(crate) fn is_interrupted_result(envelope: &Value) -> bool {
    envelope.get("is_error").and_then(Value::as_bool) == Some(true)
        && envelope
            .get("terminal_reason")
            .and_then(Value::as_str)
            .is_some_and(|reason| reason.starts_with("aborted"))
}

/// The running `total_cost_usd` a `result` envelope carried, if it says —
/// the value a mid-generation replay seeds its cost latch from.
pub(crate) fn total_cost_from_result(envelope: &Value) -> Option<f64> {
    if envelope.get("type").and_then(Value::as_str) != Some("result") {
        return None;
    }
    envelope.get("total_cost_usd").and_then(Value::as_f64)
}

/// What one Claude `usage` object says about a finished turn: the counters
/// the transcript line renders — the top-level object, which is what the CLI
/// bills for the turn — and the context total the meter shows.
///
/// The meter's number is the four-counter sum over the **last** entry of
/// `usage.iterations[]`, falling back to the top-level object only when the
/// frame carries no iterations. The distinction is the
/// whole point: one turn can make several API
/// calls, each re-sending the conversation, so the top level is the turn's
/// billing while the last iteration is what sits in the context window. Claude
/// bills the cache separately, so `input_tokens` alone would understate either
/// number badly — the cache counters and the response are part of the context.
struct ClaudeUsage {
    turn: TurnUsage,
    context_used: Option<u64>,
}

/// The meter's reading off one `usage` object: the four-counter sum over the
/// last iteration entry, or the top-level object when the frame carries no
/// iterations. `None` when the sum is 0 — the `total > 0` gate — so an
/// all-zero frame claims no reading instead of claiming an empty window.
fn context_used_from_claude(usage: &Value) -> Option<u64> {
    let source = usage
        .get("iterations")
        .and_then(Value::as_array)
        .and_then(|entries| entries.iter().rev().find(|entry| entry.is_object()))
        .unwrap_or(usage);
    // Only counters the frame sent add up; an absent one contributes nothing
    // rather than a stand-in the provider never sent.
    let total = source
        .get("input_tokens")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        + source
            .get("cache_creation_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0)
        + source
            .get("cache_read_input_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0)
        + source
            .get("output_tokens")
            .and_then(Value::as_u64)
            .unwrap_or(0);
    (total > 0).then_some(total)
}

/// Another model's entry is never this reading's denominator: no key match
/// means no window, never a borrowed one.
fn context_window_from_model_usage(envelope: &Value, model_id: Option<&str>) -> Option<u64> {
    let model_id = model_id?;
    envelope
        .get("modelUsage")?
        .get(model_id)?
        .get("contextWindow")
        .and_then(Value::as_u64)
        .filter(|window| *window > 0)
}

fn usage_from_claude(usage: &Value) -> Option<ClaudeUsage> {
    let input_tokens = usage.get("input_tokens").and_then(Value::as_u64);
    let output_tokens = usage.get("output_tokens").and_then(Value::as_u64);
    let cache_read = usage.get("cache_read_input_tokens").and_then(Value::as_u64);
    let cache_creation = usage
        .get("cache_creation_input_tokens")
        .and_then(Value::as_u64);
    let thought_tokens = usage
        .get("output_tokens_details")
        .and_then(|details| details.get("thinking_tokens"))
        .and_then(Value::as_u64);
    if input_tokens.is_none()
        && output_tokens.is_none()
        && thought_tokens.is_none()
        && cache_read.is_none()
        && cache_creation.is_none()
    {
        return None;
    }
    Some(ClaudeUsage {
        turn: TurnUsage {
            input_tokens,
            output_tokens,
            total_tokens: None,
            thought_tokens,
            // Claude counts input, cache reads and cache writes separately —
            // the context total above is their four-way sum.
            cache_read_tokens: cache_read,
            cache_write_tokens: cache_creation,
            // The envelope's `total_cost_usd` is filled in by `turn_cost`:
            // the field is the process's running total, and only its delta
            // is the turn's cost.
            cost_usd: None,
        },
        context_used: context_used_from_claude(usage),
    })
}

#[cfg(test)]
#[path = "claude_view_result_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "claude_view_result_cost_only_tests.rs"]
mod cost_only_tests;
