//! Stream tracking: the per-parent-stream block text the deltas
//! accumulate, and the message-id bookkeeping the delta and consolidated
//! frames share.

use devboule_protocol::SessionEvent;
use serde_json::Value;

use super::tasks::{parent_tool_use_id, spawn_depth};
use super::ClaudeView;

impl ClaudeView {
    pub(super) fn ingest_stream_event(&mut self, envelope: &Value) -> Vec<SessionEvent> {
        let event = match envelope.get("event") {
            Some(event) => event,
            None => return Vec::new(),
        };
        let is_subagent = envelope
            .get("parent_tool_use_id")
            .is_some_and(|value| !value.is_null());
        let parent_tool_use_id = parent_tool_use_id(envelope);
        let spawn_depth = spawn_depth(envelope);
        match event.get("type").and_then(Value::as_str) {
            Some("message_start") => {
                let message = event.get("message");
                let id = message
                    .and_then(|message| message.get("id"))
                    .and_then(Value::as_str);
                let model = message
                    .and_then(|message| message.get("model"))
                    .and_then(Value::as_str);
                self.note_message(
                    parent_tool_use_id.as_deref(),
                    id,
                    if is_subagent { None } else { model },
                );
                Vec::new()
            }
            Some("content_block_start") => {
                let index = event.get("index").and_then(Value::as_u64).unwrap_or(0);
                if let Some(kind) = event
                    .get("content_block")
                    .and_then(|block| block.get("type"))
                    .and_then(Value::as_str)
                {
                    self.note_stream_block(parent_tool_use_id.as_deref(), index, kind);
                }
                Vec::new()
            }
            Some("content_block_delta") => {
                let index = event.get("index").and_then(Value::as_u64).unwrap_or(0);
                let delta = match event.get("delta") {
                    Some(delta) => delta,
                    None => return Vec::new(),
                };
                match delta.get("type").and_then(Value::as_str) {
                    Some("text_delta") => {
                        let text = delta.get("text").and_then(Value::as_str).unwrap_or("");
                        if text.is_empty() {
                            return Vec::new();
                        }
                        self.add_streamed(parent_tool_use_id.as_deref(), index, "text", text);
                        vec![SessionEvent::AgentMessage {
                            message_id: self.current_message_id(parent_tool_use_id.as_deref()),
                            text: text.to_string(),
                            parent_tool_use_id: parent_tool_use_id.clone(),
                            spawn_depth,

                            images: Vec::new(),
                        }]
                    }
                    Some("thinking_delta") => {
                        let text = delta
                            .get("thinking")
                            .or_else(|| delta.get("text"))
                            .and_then(Value::as_str)
                            .unwrap_or("");
                        if text.is_empty() {
                            return Vec::new();
                        }
                        self.add_streamed(parent_tool_use_id.as_deref(), index, "thinking", text);
                        vec![SessionEvent::AgentThought {
                            message_id: self.current_message_id(parent_tool_use_id.as_deref()),
                            text: text.to_string(),
                            parent_tool_use_id: parent_tool_use_id.clone(),
                            spawn_depth,
                        }]
                    }
                    _ => Vec::new(),
                }
            }
            Some("content_block_stop") => {
                // The block is complete and its confirming envelope has been
                // answered (the consolidated envelope precedes the stop in
                // the field), so the accumulated text has no reader after
                // this. The entry dies here rather than at the next
                // message-id change: what the map bounds is the blocks still
                // in flight, not the session.
                let index = event.get("index").and_then(Value::as_u64).unwrap_or(0);
                self.streamed.remove(&(parent_tool_use_id, index));
                Vec::new()
            }
            _ => Vec::new(),
        }
    }

    pub(super) fn note_message(
        &mut self,
        parent_tool_use_id: Option<&str>,
        id: Option<&str>,
        model: Option<&str>,
    ) {
        if let Some(model) = model.filter(|model| !model.is_empty()) {
            self.current_model = Some(model.to_string());
        }
        let Some(id) = id.filter(|id| !id.is_empty()) else {
            return;
        };
        let stream = parent_tool_use_id.map(str::to_string);
        if self.current_message_ids.get(&stream).map(String::as_str) != Some(id) {
            self.streamed
                .retain(|(key, _), _| key.as_deref() != parent_tool_use_id);
            self.current_message_ids.insert(stream, id.to_string());
        }
    }

    pub(super) fn current_message_id(&self, parent_tool_use_id: Option<&str>) -> Option<String> {
        self.current_message_ids
            .get(&parent_tool_use_id.map(str::to_string))
            .cloned()
    }

    fn note_stream_block(&mut self, parent_tool_use_id: Option<&str>, index: u64, kind: &str) {
        let block = self
            .streamed
            .entry((parent_tool_use_id.map(str::to_string), index))
            .or_default();
        block.kind = Some(kind.to_string());
    }

    fn add_streamed(
        &mut self,
        parent_tool_use_id: Option<&str>,
        index: u64,
        kind: &str,
        fragment: &str,
    ) {
        let block = self
            .streamed
            .entry((parent_tool_use_id.map(str::to_string), index))
            .or_default();
        block.kind = Some(kind.to_string());
        block.text.push_str(fragment);
    }
}

#[cfg(test)]
#[path = "claude_view_stream_tests.rs"]
mod tests;
