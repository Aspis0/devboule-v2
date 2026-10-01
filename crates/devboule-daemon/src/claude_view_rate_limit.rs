//! The plan-usage view of Claude's `rate_limit_event`: the unified
//! windows the frame names, or the one top-level window an older CLI
//! sends.

use devboule_protocol::{PlanWindow, SessionEvent};
use serde_json::Value;

/// The windows Claude's frame can name, in the order the popover lists them.
/// A key outside this table is a window this view cannot label — skipped,
/// never shown with a guessed duration.
const NAMED_WINDOWS: [(&str, u64); 2] = [("five_hour", 300), ("seven_day", 10_080)];

/// The plan-usage view of one `rate_limit_event`: the unified windows the
/// frame carried, or the single top-level window an older CLI names instead.
/// `status`, `surpassedThreshold` and the `overage*` fields have no popover
/// field to feed and no protocol field to carry, so they stay out.
pub(super) fn rate_limit_plan_usage(envelope: &Value) -> Option<SessionEvent> {
    let info = envelope.get("rate_limit_info")?;
    let mut windows = unified_windows(info.get("unifiedWindows"));
    if info.get("unifiedWindows").is_none() {
        windows.extend(top_level_window(info));
    }
    // A frame that names no window this view can label publishes nothing,
    // never an empty `windows` list.
    (!windows.is_empty()).then_some(SessionEvent::PlanUsage {
        provider_id: "claude".to_string(),
        plan_label: None,
        windows,
        credits: None,
    })
}

fn unified_windows(windows: Option<&Value>) -> Vec<PlanWindow> {
    let Some(windows) = windows.and_then(Value::as_object) else {
        return Vec::new();
    };
    NAMED_WINDOWS
        .iter()
        .filter_map(|(key, duration_mins)| {
            windows
                .get(*key)
                .map(|window| claude_plan_window(window, *duration_mins))
        })
        .collect()
}

/// The one window an older CLI names at the top level instead of inside
/// `unifiedWindows`: type, utilization and reset must all be there, or the
/// frame names no complete window.
fn top_level_window(info: &Value) -> Option<PlanWindow> {
    let duration_mins = NAMED_WINDOWS
        .iter()
        .find(|(key, _)| info.get("rateLimitType").and_then(Value::as_str) == Some(key))
        .map(|(_, duration_mins)| *duration_mins)?;
    info.get("utilization")?;
    info.get("resetsAt")?;
    Some(claude_plan_window(info, duration_mins))
}

/// One window: the frame's 0-1 fraction becomes the popover's 0-100, rounded.
/// A fraction that is absent, non-numeric or negative leaves the percent
/// absent — never a stand-in 0 — while above 1 (overage) stays the real
/// number: the popover clamps its bar and keeps its text true. `resetsAt`
/// passes through in epoch seconds, the unit `PlanWindow` carries.
fn claude_plan_window(window: &Value, duration_mins: u64) -> PlanWindow {
    PlanWindow {
        duration_mins,
        used_percent: window
            .get("utilization")
            .and_then(Value::as_f64)
            .and_then(crate::usage_cost::finite_cost)
            .map(|fraction| (fraction * 100.0).round() as u64),
        resets_at: window.get("resetsAt").and_then(Value::as_i64),
    }
}

#[cfg(test)]
#[path = "claude_view_rate_limit_tests.rs"]
mod tests;
