//! Tests for one topic: turn completion — the finish event, the
//! running-total cost latch, and the usage and context reading.

use devboule_protocol::{SessionEvent, TurnUsage};
use serde_json::{json, Value};

use crate::claude_view::test_support::{init_frame, view};
use crate::claude_view::{ClaudeView, CostBaseline};

#[test]
fn result_frame_is_end_of_turn() {
    let mut mapper = view();
    let _ = mapper.ingest(&init_frame());
    let events = mapper.ingest(&json!({
        "type": "result",
        "subtype": "success",
        "stop_reason": "end_turn",
        "session_id": "cbe439d8-8e95-42c3-b6c7-40c7e5d3b3cd",
        "total_cost_usd": 0.093081,
        "num_turns": 2,
        "is_error": false,
        "usage": {
            "input_tokens": 4,
            "output_tokens": 230,
            "output_tokens_details": {"thinking_tokens": 0}
        }
    }));
    match events.as_slice() {
        [SessionEvent::AgentFinished {
            stop_reason,
            model_id,
            usage,
            ..
        }, SessionEvent::ContextUsage { .. }] => {
            assert_eq!(stop_reason, "end_turn");
            assert_eq!(model_id.as_deref(), Some("claude-opus-5[1m]"));
            let usage = usage.as_ref().expect("usage");
            assert_eq!(usage.input_tokens, Some(4));
            assert_eq!(usage.output_tokens, Some(230));
        }
        other => panic!("expected AgentFinished then ContextUsage, got {other:?}"),
    }
}

#[test]
fn result_cost_is_the_turn_s_delta_of_the_running_total() {
    // `total_cost_usd` is the CLI process's running total, not the
    // turn's: the frames below are two results of ONE session (E1.jsonl
    // lines 17 and 54), and only their difference is what the second
    // turn billed. Anthropic's agent SDK documents the same running
    // total — a conversation reset "zeroes the running totals reported
    // on subsequent ResultMessage objects".
    let frames: Vec<Value> = include_str!("../fixtures/wire/claude-e1-results.jsonl")
        .lines()
        .map(|line| serde_json::from_str(line).expect("measured result envelope"))
        .collect();
    let mut mapper = view();
    let _ = mapper.ingest(&init_frame());
    fn finish_usage(events: &[SessionEvent]) -> &TurnUsage {
        match events.first() {
            Some(SessionEvent::AgentFinished {
                usage: Some(usage), ..
            }) => usage,
            other => panic!("expected AgentFinished with usage, got {other:?}"),
        }
    }
    let first_events = mapper.ingest(&frames[0]);
    let first = finish_usage(&first_events);
    assert_eq!(first.cost_usd, Some(0.0723335));
    assert_eq!(first.cache_read_tokens, Some(16_519));
    assert_eq!(first.cache_write_tokens, Some(6_301));
    let second_events = mapper.ingest(&frames[1]);
    let second = finish_usage(&second_events);
    // Binary subtraction rounds, so the delta holds a tolerance.
    let second_cost = second.cost_usd.expect("second turn cost");
    assert!((second_cost - 0.01273).abs() < 1e-9, "{second_cost}");
    assert_eq!(second.cache_read_tokens, Some(22_820));
    assert_eq!(second.cache_write_tokens, Some(121));
    // A total below the latch is the documented reset — the running
    // totals restarted — and the turn's cost is the new total itself.
    let reset = mapper.ingest(&json!({
        "type": "result",
        "subtype": "success",
        "stop_reason": "end_turn",
        "session_id": "cbe439d8-8e95-42c3-b6c7-40c7e5d3b3cd",
        "total_cost_usd": 0.01,
        "is_error": false,
        "usage": {"input_tokens": 4, "output_tokens": 12}
    }));
    assert_eq!(finish_usage(&reset).cost_usd, Some(0.01));
    // A fresh init zeroes the baseline: the next result reports its
    // whole total as the turn's cost, not a delta of the old process.
    let _ = mapper.ingest(&init_frame());
    let fresh = mapper.ingest(&json!({
        "type": "result",
        "subtype": "success",
        "stop_reason": "end_turn",
        "session_id": "cbe439d8-8e95-42c3-b6c7-40c7e5d3b3cd",
        "total_cost_usd": 0.02,
        "is_error": false,
        "usage": {"input_tokens": 4, "output_tokens": 12}
    }));
    assert_eq!(finish_usage(&fresh).cost_usd, Some(0.02));
    // A result whose envelope names no total carries no cost.
    let bare = mapper.ingest(&json!({
        "type": "result",
        "subtype": "success",
        "stop_reason": "end_turn",
        "session_id": "cbe439d8-8e95-42c3-b6c7-40c7e5d3b3cd",
        "is_error": false,
        "usage": {"input_tokens": 4, "output_tokens": 12}
    }));
    assert_eq!(finish_usage(&bare).cost_usd, None);
}

#[test]
fn a_child_init_does_not_reset_the_parent_s_cost_baseline() {
    // A subagent's in-band `init` is not a CLI process boundary: the
    // parent's running total keeps running across it, and the next
    // parent result must still cost its delta.
    let mut mapper = view();
    let _ = mapper.ingest(&init_frame());
    let first = mapper.ingest(&json!({
        "type": "result",
        "subtype": "success",
        "stop_reason": "end_turn",
        "session_id": "cbe439d8-8e95-42c3-b6c7-40c7e5d3b3cd",
        "total_cost_usd": 0.0723335,
        "is_error": false,
        "usage": {"input_tokens": 4, "output_tokens": 12}
    }));
    match first.as_slice() {
        [SessionEvent::AgentFinished {
            usage: Some(usage), ..
        }, ..] => assert_eq!(usage.cost_usd, Some(0.0723335)),
        other => panic!("expected the first finish, got {other:?}"),
    }
    let _ = mapper.ingest(&json!({
        "type": "system",
        "subtype": "init",
        "session_id": "child-of-cbe439d8",
        "spawn_depth": 1
    }));
    let second = mapper.ingest(&json!({
        "type": "result",
        "subtype": "success",
        "stop_reason": "end_turn",
        "session_id": "cbe439d8-8e95-42c3-b6c7-40c7e5d3b3cd",
        "total_cost_usd": 0.0850635,
        "is_error": false,
        "usage": {"input_tokens": 2, "output_tokens": 4}
    }));
    match second.as_slice() {
        [SessionEvent::AgentFinished {
            usage: Some(usage), ..
        }, ..] => {
            let cost = usage.cost_usd.expect("second cost");
            assert!((cost - 0.01273).abs() < 1e-9, "{cost}");
        }
        other => panic!("expected the second finish, got {other:?}"),
    }
    // An explicit depth 0 is the root marker as much as an absent key:
    // a process init that reports it still resets the latch.
    let _ = mapper.ingest(&json!({
        "type": "system",
        "subtype": "init",
        "session_id": "cbe439d8-8e95-42c3-b6c7-40c7e5d3b3cd",
        "spawn_depth": 0
    }));
    let third = mapper.ingest(&json!({
        "type": "result",
        "subtype": "success",
        "stop_reason": "end_turn",
        "session_id": "cbe439d8-8e95-42c3-b6c7-40c7e5d3b3cd",
        "total_cost_usd": 0.09,
        "is_error": false,
        "usage": {"input_tokens": 2, "output_tokens": 4}
    }));
    match third.as_slice() {
        [SessionEvent::AgentFinished {
            usage: Some(usage), ..
        }, ..] => assert_eq!(usage.cost_usd, Some(0.09)),
        other => panic!("expected the third finish, got {other:?}"),
    }
}

#[test]
fn an_unknown_baseline_costs_nothing_once_then_recovers() {
    // A failed lookback scan leaves the latch unreadable: the first
    // result shows no cost — and its own total is the baseline every
    // later delta needs, so the view recovers from the second result
    // on. (The init precedes the restore: a root init resets the latch
    // to a known zero, which would erase the state under test.)
    let mut view = ClaudeView::new(None);
    let _ = view.ingest(&init_frame());
    view.restore_cost_baseline(CostBaseline::Unknown);
    let first = view.ingest(&json!({
        "type": "result",
        "subtype": "success",
        "stop_reason": "end_turn",
        "session_id": "cbe439d8-8e95-42c3-b6c7-40c7e5d3b3cd",
        "total_cost_usd": 0.07,
        "is_error": false,
        "usage": {"input_tokens": 4, "output_tokens": 12}
    }));
    match first.as_slice() {
        [SessionEvent::AgentFinished {
            usage: Some(usage), ..
        }, ..] => assert_eq!(usage.cost_usd, None),
        other => panic!("expected the uncosted first finish, got {other:?}"),
    }
    let second = view.ingest(&json!({
        "type": "result",
        "subtype": "success",
        "stop_reason": "end_turn",
        "session_id": "cbe439d8-8e95-42c3-b6c7-40c7e5d3b3cd",
        "total_cost_usd": 0.09,
        "is_error": false,
        "usage": {"input_tokens": 2, "output_tokens": 4}
    }));
    match second.as_slice() {
        [SessionEvent::AgentFinished {
            usage: Some(usage), ..
        }, ..] => {
            let cost = usage.cost_usd.expect("the recovered delta");
            assert!((cost - 0.02).abs() < 1e-9, "{cost}");
        }
        other => panic!("expected the second finish, got {other:?}"),
    }
}

#[test]
fn result_context_usage_sums_the_last_iteration_of_each_measured_turn() {
    // Source: reports/foundations/wire/E1.jsonl lines 17 and 54 — two
    // `result` envelopes of ONE session (44e75940…), copied verbatim into
    // fixtures/wire/claude-e1-results.jsonl. Every captured frame has
    // exactly one iteration, equal to the top level, so
    // these two pin the summands — the cache counters included — while
    // the constructed turn below pins WHICH entry is read.
    let frames: Vec<Value> = include_str!("../fixtures/wire/claude-e1-results.jsonl")
        .lines()
        .map(|line| serde_json::from_str(line).expect("measured result envelope"))
        .collect();
    assert_eq!(frames.len(), 2, "E1.jsonl lines 17 and 54");
    let mut mapper = view();
    let _ = mapper.ingest(&init_frame());
    // 2 + 6301 + 16519 + 4, then 2 + 121 + 22820 + 4 — input alone is 2,
    // so a mapper that forgot the cache counters fails right here.
    for (frame, expected) in frames.iter().zip([22_826_u64, 22_947]) {
        let events = mapper.ingest(frame);
        match events.as_slice() {
            [SessionEvent::AgentFinished {
                model_id, usage, ..
            }, SessionEvent::ContextUsage {
                model_id: context_model,
                used_tokens,
                max_tokens,
                live,
            }] => {
                // The transcript line keeps the top-level counters.
                assert_eq!(usage.as_ref().expect("usage").input_tokens, Some(2));
                // The result frame carries no `model`; the id is the init
                // frame's, and the reading rides the same session.
                assert_eq!(model_id.as_deref(), Some("claude-opus-5[1m]"));
                assert_eq!(context_model.as_deref(), model_id.as_deref());
                assert_eq!(*used_tokens, expected);
                assert_eq!(*max_tokens, None);
                assert!(!live);
            }
            other => panic!("expected AgentFinished then ContextUsage, got {other:?}"),
        }
    }
}

#[test]
fn a_multi_iteration_turn_reads_the_last_iteration_not_the_turn_total() {
    // No capture in reports/foundations/wire has usage.iterations longer
    // than one, so the turn below is CONSTRUCTED from the
    // two measured iterations of E1:17/:54: the entries are verbatim and
    // the top level is their element-wise sum — the shape of a turn that
    // made two API calls, where the top level is the turn's billing and
    // the last entry is what remains in the context window. Reading the
    // top level fails the equality below.
    let frames: Vec<Value> = include_str!("../fixtures/wire/claude-e1-results.jsonl")
        .lines()
        .map(|line| serde_json::from_str(line).expect("measured result envelope"))
        .collect();
    const COUNTERS: [&str; 4] = [
        "input_tokens",
        "cache_creation_input_tokens",
        "cache_read_input_tokens",
        "output_tokens",
    ];
    let mut turn = frames[1].clone();
    let first_iteration = frames[0]["usage"]["iterations"][0].clone();
    let last_iteration = frames[1]["usage"]["iterations"][0].clone();
    turn["usage"]["iterations"] = json!([first_iteration, last_iteration]);
    for counter in COUNTERS {
        let top = turn["usage"][counter].as_u64().expect("measured top");
        let first = frames[0]["usage"][counter]
            .as_u64()
            .expect("measured first");
        turn["usage"][counter] = json!(top + first);
    }
    let top_total: u64 = COUNTERS
        .iter()
        .map(|&counter| turn["usage"][counter].as_u64().expect("measured top"))
        .sum();
    let last_total: u64 = COUNTERS
        .iter()
        .map(|&counter| {
            last_iteration[counter]
                .as_u64()
                .expect("measured iteration")
        })
        .sum();
    assert_ne!(
        top_total, last_total,
        "the premise: a two-call turn's billing differs from its last iteration"
    );

    let mut mapper = view();
    let _ = mapper.ingest(&init_frame());
    let events = mapper.ingest(&turn);
    match events.as_slice() {
        [SessionEvent::AgentFinished { .. }, SessionEvent::ContextUsage { used_tokens, .. }] => {
            assert_eq!(
                *used_tokens, last_total,
                "the meter reads the last iteration, never the turn's top-level billing ({top_total})"
            );
        }
        other => panic!("expected AgentFinished then ContextUsage, got {other:?}"),
    }
}

#[test]
fn measured_aborted_result_maps_to_interrupted() {
    let frame: Value =
        serde_json::from_str(include_str!("../../../fixtures/claude-aborted-result.json"))
            .expect("fixture");
    let events = view().ingest(&frame);
    let stop_reason = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::AgentFinished { stop_reason, .. } => Some(stop_reason.clone()),
            _ => None,
        })
        .expect("finish event");
    assert_eq!(stop_reason, "interrupted");
}

#[test]
fn synthetic_error_result_maps_to_error_not_completed() {
    let events = view().ingest(&json!({
        "type": "result",
        "subtype": "error_during_execution",
        "is_error": true,
        "terminal_reason": "api_error",
        "stop_reason": "stop_sequence",
    }));
    let stop_reason = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::AgentFinished { stop_reason, .. } => Some(stop_reason.clone()),
            _ => None,
        })
        .expect("finish event");
    assert_eq!(stop_reason, "error");
}
