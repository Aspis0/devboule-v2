//! Pair the app-server's two compaction signals for the root thread.

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
                if !is_root_thread(params, root_thread_id) {
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
                if !is_root_thread(params, root_thread_id) {
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
            events.push(notice("Context compacted.", NoticeSeverity::Info));
        }
        self.unpaired_items = 0;
        self.unpaired_notifications = 0;
        events
    }
}

pub(crate) fn is_root_thread(params: &Value, root_thread_id: &str) -> bool {
    params
        .get("threadId")
        .and_then(Value::as_str)
        .is_none_or(|thread_id| thread_id == root_thread_id)
}

fn notice(text: &str, severity: NoticeSeverity) -> SessionEvent {
    SessionEvent::SessionNotice {
        text: text.to_string(),
        severity,
    }
}
