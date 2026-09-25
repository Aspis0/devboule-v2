//! Pair the app-server's two compaction signals for the root thread.

use std::collections::HashSet;

use devboule_protocol::{NoticeSeverity, SessionEvent};
use serde_json::Value;

#[derive(Default)]
pub(crate) struct CodexCompactions {
    unpaired_items: usize,
    unpaired_notifications: usize,
    pending_items: HashSet<String>,
}

impl CodexCompactions {
    pub(crate) fn event(&mut self, value: &Value, root_thread_id: &str) -> Option<SessionEvent> {
        let method = value.get("method").and_then(Value::as_str)?;
        let params = value.get("params").unwrap_or(&Value::Null);
        match method {
            "thread/compacted" => {
                // Paseo's `ContextCompactedNotificationSchema` requires
                // `threadId` (`z.string()`), so a frame without one is an
                // invalid payload it warns and drops — never a root event.
                // An empty string is likewise not the root: the handler
                // compares strictly against the current thread (:6205-6216).
                if params.get("threadId").and_then(Value::as_str) != Some(root_thread_id) {
                    return None;
                }
                if self.unpaired_items > 0 {
                    self.unpaired_items -= 1;
                    return None;
                }
                // Paseo consumes one pending item here
                // (`consumePendingRootCompaction` :6211): without that, the
                // pending item survives into `turn_ended` and the same
                // compaction is reported twice.
                if let Some(id) = self.pending_items.iter().next().cloned() {
                    self.pending_items.remove(&id);
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
                    // A completion for an id with other items still pending
                    // is late, not new (Paseo `isLateCompletionForOlderItem`
                    // :5602-5610); with nothing pending it pairs below or
                    // opens a new count, exactly as Paseo does once its turn
                    // reset cleared the pending sets (`resetTurnTrackingState`
                    // :6037-6059). There is no stale set to keep: Paseo has
                    // none, and one would grow for the life of the session.
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
        // Paseo's `completePendingRootCompactions` (:6145): close every
        // loading row at the turn boundary, then clear the pairing state
        // (`resetTurnTrackingState` :6037) so the next turn starts unpaired.
        let mut events = Vec::with_capacity(self.pending_items.len());
        for _ in self.pending_items.drain() {
            events.push(notice("Context compacted.", NoticeSeverity::Info));
        }
        self.unpaired_items = 0;
        self.unpaired_notifications = 0;
        events
    }
}

/// Whether this frame belongs to the session's root thread, for the channels
/// whose schema leaves `threadId` optional (`item/*`, `turn/completed`).
/// Paseo routes those through `getSubAgentCallIdForThread` (:5477), where a
/// missing or empty id is the root thread (`if (!threadId ...) return null`).
/// `thread/compacted` is not one of those channels — its schema requires
/// `threadId` — so it does not use this predicate.
pub(crate) fn is_root_thread(params: &Value, root_thread_id: &str) -> bool {
    match params.get("threadId").and_then(Value::as_str) {
        None | Some("") => true,
        Some(thread_id) => thread_id == root_thread_id,
    }
}

fn notice(text: &str, severity: NoticeSeverity) -> SessionEvent {
    SessionEvent::SessionNotice {
        text: text.to_string(),
        severity,
    }
}
