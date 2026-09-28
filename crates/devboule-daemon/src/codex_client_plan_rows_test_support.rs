//! The plan rows the app builds from the daemon's events, so a test can
//! assert on what the person sees rather than on the event list.

use std::collections::BTreeMap;

use devboule_protocol::SessionEvent;

/// A plan row's own events. A still-pending card is not one: it is not a
/// journal row, and an attach sends it after the replay, as every pending card.
pub(super) fn is_plan_row_event(event: &SessionEvent) -> bool {
    match event {
        SessionEvent::AgentToolCall { kind, .. } | SessionEvent::AgentToolUpdate { kind, .. } => {
            kind.as_deref() == Some("plan")
        }
        _ => false,
    }
}

/// One plan row as the app builds it.
#[derive(Debug, Default)]
pub(super) struct PlanRow {
    pub(super) status: String,
    pub(super) title: String,
    pub(super) text: String,
}

/// The plan rows the app builds from these events, keyed by tool call id: a
/// repeated call rewrites only the status, an update keeps the status it does
/// not name and appends its text (`agentSession.ts` `appendTool`/`updateTool`).
pub(super) fn plan_rows(events: &[SessionEvent]) -> BTreeMap<String, PlanRow> {
    let mut rows = BTreeMap::new();
    for event in events {
        match event {
            SessionEvent::AgentToolCall {
                tool_call_id,
                title,
                status,
                kind: Some(kind),
                ..
            } if kind == "plan" => {
                let row = rows.entry(tool_call_id.clone()).or_insert_with(|| PlanRow {
                    title: title.clone(),
                    ..PlanRow::default()
                });
                row.status = status.clone();
            }
            SessionEvent::AgentToolUpdate {
                tool_call_id,
                status,
                text,
                title,
                kind: Some(kind),
                ..
            } if kind == "plan" => {
                let row = rows.entry(tool_call_id.clone()).or_insert_with(|| PlanRow {
                    status: "running".to_string(),
                    ..PlanRow::default()
                });
                if let Some(status) = status {
                    row.status = status.clone();
                }
                if let Some(title) = title.as_ref().filter(|title| !title.is_empty()) {
                    row.title = title.clone();
                }
                if let Some(text) = text.as_ref().filter(|text| !text.is_empty()) {
                    if !row.text.is_empty() {
                        row.text.push('\n');
                    }
                    row.text.push_str(text);
                }
            }
            _ => {}
        }
    }
    rows
}
