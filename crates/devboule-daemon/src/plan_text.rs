use serde_json::Value;

use devboule_protocol::SessionEvent;

pub(super) const MAX_PLAN_BYTES: usize = 64 * 1024;

const TRUNCATION_MARKER: &str = "\n\n[Plan truncated.]";

pub(super) fn bound_plan_text(text: &str) -> String {
    if text.len() <= MAX_PLAN_BYTES {
        return text.to_string();
    }

    let mut end = MAX_PLAN_BYTES - TRUNCATION_MARKER.len();
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}{TRUNCATION_MARKER}", &text[..end])
}

pub(super) fn bound_permission_request(event: &mut SessionEvent) {
    if let SessionEvent::PermissionRequest {
        plan: Some(plan), ..
    } = event
    {
        *plan = bound_plan_text(plan);
    }
}

pub(super) fn bound_claude_envelope(value: &mut Value) {
    match value.get("type").and_then(Value::as_str) {
        Some("assistant") => {
            if let Some(blocks) = value
                .pointer_mut("/message/content")
                .and_then(Value::as_array_mut)
            {
                for block in blocks {
                    let is_plan = block.get("type").and_then(Value::as_str) == Some("tool_use")
                        && block.get("name").and_then(Value::as_str) == Some("ExitPlanMode");
                    if is_plan {
                        if let Some(Value::String(text)) = block.pointer_mut("/input/plan") {
                            *text = bound_plan_text(text);
                        }
                    }
                }
            }
        }
        Some("control_request")
            if value.pointer("/request/tool_name").and_then(Value::as_str)
                == Some("ExitPlanMode") =>
        {
            if let Some(Value::String(text)) = value.pointer_mut("/request/input/plan") {
                *text = bound_plan_text(text);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::{bound_claude_envelope, bound_plan_text, Value, MAX_PLAN_BYTES, TRUNCATION_MARKER};

    #[test]
    fn plan_text_is_bounded_at_a_utf8_boundary_with_a_marker() {
        let text = format!("ab{}", "🧭".repeat(MAX_PLAN_BYTES));
        let bounded = bound_plan_text(&text);
        let cutoff = bounded.len() - TRUNCATION_MARKER.len();

        assert_eq!(bounded.len(), MAX_PLAN_BYTES - 3);
        assert!(text.is_char_boundary(cutoff));
        assert!(bounded.ends_with("[Plan truncated.]"));
    }

    #[test]
    fn plan_envelopes_share_the_permission_text_cap() {
        let text = format!("ab{}", "🧭".repeat(MAX_PLAN_BYTES));
        for mut envelope in [
            serde_json::json!({
                "type": "assistant",
                "message": {"content": [{"type": "tool_use", "name": "ExitPlanMode", "input": {"plan": text.clone()}}]}
            }),
            serde_json::json!({
                "type": "control_request",
                "request": {"tool_name": "ExitPlanMode", "input": {"plan": text}}
            }),
        ] {
            bound_claude_envelope(&mut envelope);
            let plan = envelope
                .pointer("/message/content/0/input/plan")
                .or_else(|| envelope.pointer("/request/input/plan"))
                .and_then(Value::as_str)
                .expect("bounded plan");
            assert_eq!(plan.len(), MAX_PLAN_BYTES - 3);
            assert!(plan.ends_with("[Plan truncated.]"));
        }
    }

    #[test]
    fn unrelated_tool_inputs_are_left_unchanged() {
        let text = "🧭".repeat(MAX_PLAN_BYTES);
        for mut envelope in [
            serde_json::json!({
                "type": "assistant",
                "message": {"content": [{"type": "tool_use", "name": "Write", "input": {"plan": text}}]}
            }),
            serde_json::json!({
                "type": "control_request",
                "request": {"tool_name": "OtherTool", "input": {"plan": text}}
            }),
            serde_json::json!({"type": "assistant", "message": {"content": null}}),
        ] {
            let original = envelope.clone();
            bound_claude_envelope(&mut envelope);
            assert_eq!(envelope, original);
        }
    }
}
