//! Tests for one topic: a Claude `rate_limit_event` maps to the plan-usage
//! windows the popover shows. The view's other suites live beside their
//! subjects in the `claude_view_*.rs` files.

use devboule_protocol::{PlanWindow, SessionEvent};
use serde_json::{json, Value};

use crate::claude_view::ClaudeView;

fn view() -> ClaudeView {
    ClaudeView::new(None)
}

/// The (duration, percent, reset) triple of each window, for asserting a
/// whole window list in one equality.
fn window_shapes(windows: &[PlanWindow]) -> Vec<(u64, Option<u64>, Option<i64>)> {
    windows
        .iter()
        .map(|window| (window.duration_mins, window.used_percent, window.resets_at))
        .collect()
}

#[test]
fn the_measured_rate_limit_shapes_map_to_their_plan_windows() {
    let envelopes: Vec<Value> = include_str!("../fixtures/wire/claude-rate-limits-synthetic.jsonl")
        .lines()
        .map(|line| serde_json::from_str(line).expect("measured envelope"))
        .collect();
    let expected = [
        vec![
            (300, Some(33), Some(1_790_632_800)),
            (10_080, Some(76), Some(1_790_748_000)),
        ],
        vec![
            (300, Some(30), Some(1_790_614_800)),
            (10_080, Some(70), Some(1_790_748_000)),
        ],
        vec![
            (300, Some(3), Some(1_789_035_000)),
            (10_080, Some(29), Some(1_789_538_400)),
        ],
    ];
    for (envelope, expected) in envelopes.iter().zip(expected) {
        let mut mapper = view();
        let events = mapper.ingest(envelope);
        match events.as_slice() {
            [SessionEvent::PlanUsage {
                provider_id,
                plan_label,
                windows,
                credits,
            }] => {
                assert_eq!(provider_id, "claude");
                assert_eq!(plan_label, &None);
                assert_eq!(credits, &None);
                // `resets_at` passes the frame's epoch seconds through
                // unchanged: the unit `PlanWindow` carries and the popover's
                // countdown already multiplies back to ms.
                assert_eq!(window_shapes(windows), expected);
            }
            other => panic!("expected one PlanUsage, got {other:?}"),
        }
    }
}

#[test]
fn unknown_unified_windows_are_skipped_not_named() {
    let mut mapper = view();
    let events = mapper.ingest(&json!({
        "type": "rate_limit_event",
        "rate_limit_info": {
            "status": "allowed_warning",
            "unifiedWindows": {
                "five_hour": {"utilization": 0.03, "resetsAt": 1_789_035_000},
                "seven_day_opus": {"utilization": 0.5, "resetsAt": 1_790_748_000}
            }
        }
    }));
    match events.as_slice() {
        [SessionEvent::PlanUsage { windows, .. }] => {
            assert_eq!(
                window_shapes(windows),
                vec![(300, Some(3), Some(1_789_035_000))]
            );
        }
        other => panic!("expected the known window alone, got {other:?}"),
    }
    // An all-unknown frame names no window this view can label: no event at
    // all, never an empty `windows` list.
    let mut mapper = view();
    assert!(mapper
        .ingest(&json!({
            "type": "rate_limit_event",
            "rate_limit_info": {
                "status": "allowed",
                "unifiedWindows": {
                    "seven_day_opus": {"utilization": 0.5, "resetsAt": 1_790_748_000}
                }
            }
        }))
        .is_empty());
}

#[test]
fn a_window_without_utilization_keeps_no_percent() {
    let mut mapper = view();
    let events = mapper.ingest(&json!({
        "type": "rate_limit_event",
        "rate_limit_info": {
            "status": "allowed",
            "unifiedWindows": {"five_hour": {"resetsAt": 1_790_614_800}}
        }
    }));
    match events.as_slice() {
        [SessionEvent::PlanUsage { windows, .. }] => {
            assert_eq!(
                window_shapes(windows),
                vec![(300, None, Some(1_790_614_800))]
            );
        }
        other => panic!("expected the named window, got {other:?}"),
    }
}

#[test]
fn a_negative_or_non_numeric_utilization_is_no_percent_at_all() {
    // A negative fraction must not survive the saturating cast as a
    // stand-in 0 %, and a non-number is the frame not saying. The window
    // itself and its reset still land.
    for utilization in [json!(-0.5), json!("0.5")] {
        let mut mapper = view();
        let events = mapper.ingest(&json!({
            "type": "rate_limit_event",
            "rate_limit_info": {
                "status": "allowed",
                "unifiedWindows": {"five_hour": {"utilization": utilization, "resetsAt": 1_790_614_800}}
            }
        }));
        match events.as_slice() {
            [SessionEvent::PlanUsage { windows, .. }] => {
                assert_eq!(
                    window_shapes(windows),
                    vec![(300, None, Some(1_790_614_800))],
                    "utilization {utilization}"
                );
            }
            other => panic!("expected the named window, got {other:?}"),
        }
    }
}

#[test]
fn an_overage_utilization_keeps_its_real_percent() {
    // Above 1 is overage: the number the provider sent is the number shown.
    // The popover clamps its bar; the text stays true.
    let mut mapper = view();
    let events = mapper.ingest(&json!({
        "type": "rate_limit_event",
        "rate_limit_info": {
            "status": "allowed_warning",
            "unifiedWindows": {"five_hour": {"utilization": 1.05, "resetsAt": 1_790_614_800}}
        }
    }));
    match events.as_slice() {
        [SessionEvent::PlanUsage { windows, .. }] => {
            assert_eq!(
                window_shapes(windows),
                vec![(300, Some(105), Some(1_790_614_800))]
            );
        }
        other => panic!("expected the named window, got {other:?}"),
    }
}

#[test]
fn an_older_frame_without_unified_windows_names_one_top_level_window() {
    let mut mapper = view();
    let events = mapper.ingest(&json!({
        "type": "rate_limit_event",
        "rate_limit_info": {
            "status": "allowed",
            "rateLimitType": "five_hour",
            "utilization": 0.5,
            "resetsAt": 1_790_614_800
        }
    }));
    match events.as_slice() {
        [SessionEvent::PlanUsage { windows, .. }] => {
            assert_eq!(
                window_shapes(windows),
                vec![(300, Some(50), Some(1_790_614_800))]
            );
        }
        other => panic!("expected one window, got {other:?}"),
    }
    // A frame missing any of the three names no complete window: nothing.
    let mut mapper = view();
    assert!(mapper
        .ingest(&json!({
            "type": "rate_limit_event",
            "rate_limit_info": {
                "status": "allowed",
                "rateLimitType": "five_hour",
                "utilization": 0.5
            }
        }))
        .is_empty());
}

#[test]
fn a_bare_rate_limit_frame_names_no_window() {
    // The hand-trimmed shape an older CLI sends: no unifiedWindows and no
    // top-level trio — no view.
    let mut mapper = view();
    assert!(mapper
        .ingest(&json!({
            "type": "rate_limit_event",
            "rate_limit_info": {"status": "allowed"},
            "session_id": "00000000-0000-4000-8000-0000000000c1"
        }))
        .is_empty());
}
