//! Pair the app-server's two compaction signals and close incomplete rows at turn end.

use std::collections::HashSet;

use devboule_protocol::{NoticeSeverity, SessionEvent};
use serde_json::Value;

#[derive(Default)]
pub(crate) struct CodexCompactions {
    unpaired_items: usize,
    unpaired_notifications: usize,
    pending_items: HashSet<String>,
    stale_items: HashSet<String>,
}

impl CodexCompactions {
    pub(crate) fn event(&mut self, value: &Value, root_thread_id: &str) -> Option<SessionEvent> {
        let method = value.get("method").and_then(Value::as_str)?;
        let params = value.get("params").unwrap_or(&Value::Null);
        match method {
            "thread/compacted" => {
                if params.get("threadId").and_then(Value::as_str) != Some(root_thread_id) {
                    return None;
                }
                if self.unpaired_items > 0 {
                    self.unpaired_items -= 1;
                    return None;
                }
                self.unpaired_notifications += 1;
                Some(notice("Context compacted.", NoticeSeverity::Info))
            }
            "item/started" | "item/completed" => {
                if params.get("threadId").and_then(Value::as_str) != Some(root_thread_id) {
                    return None;
                }
                let item = params.get("item");
                if item
                    .and_then(|item| item.get("type"))
                    .and_then(Value::as_str)
                    != Some("contextCompaction")
                {
                    return None;
                }
                let item_id = item.and_then(|item| item.get("id")).and_then(Value::as_str);
                if method == "item/started" {
                    if let Some(id) = item_id {
                        self.pending_items.insert(id.to_string());
                    }
                    return Some(notice("Compacting the context.", NoticeSeverity::Info));
                }
                if let Some(id) = item_id {
                    if self.stale_items.remove(id) {
                        return None;
                    }
                    if !self.pending_items.remove(id) && !self.pending_items.is_empty() {
                        return None;
                    }
                }
                if self.unpaired_notifications > 0 {
                    self.unpaired_notifications -= 1;
                    return None;
                }
                self.unpaired_items += 1;
                Some(notice("Context compacted.", NoticeSeverity::Info))
            }
            _ => None,
        }
    }

    pub(crate) fn turn_ended(&mut self) -> Vec<SessionEvent> {
        let mut events = Vec::with_capacity(self.pending_items.len());
        for id in self.pending_items.drain() {
            self.stale_items.insert(id);
            events.push(notice(
                "Context compaction did not complete.",
                NoticeSeverity::Warning,
            ));
        }
        self.unpaired_items = 0;
        self.unpaired_notifications = 0;
        events
    }
}

fn notice(text: &str, severity: NoticeSeverity) -> SessionEvent {
    SessionEvent::SessionNotice {
        text: text.to_string(),
        severity,
    }
}
