//! Derive a UI view from an inbound Claude stream-json envelope.
//!
//! The envelope is the source of truth. This module never mutates it: callers
//! journal the original object and, separately, publish the derived view.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use devboule_protocol::{SessionEvent, SessionModel, ToolLocation, TurnUsage};
use serde_json::Value;

/// Stateful mapper: stream-json emits `stream_event` deltas and then a
/// consolidated `assistant` message. Track streamed length per content block
/// so the consolidated text is forwarded only as the unstreamed remainder.
pub(crate) struct ClaudeView {
    streamed: HashMap<u64, usize>,
    current_model: Option<String>,
    last_manifest_model: Option<String>,
    current_message_id: Option<String>,
    peer_session_id: Option<String>,
    cwd: Option<PathBuf>,
}

impl ClaudeView {
    pub(crate) fn new(cwd: Option<PathBuf>) -> Self {
        Self {
            streamed: HashMap::new(),
            current_model: None,
            last_manifest_model: None,
            current_message_id: None,
            peer_session_id: None,
            cwd,
        }
    }

    pub(crate) fn peer_session_id(&self) -> Option<&str> {
        self.peer_session_id.as_deref()
    }

    /// Map one parsed envelope to zero or more view events. Unknown or
    /// journal-only frames yield an empty vec.
    pub(crate) fn ingest(&mut self, envelope: &Value) -> Vec<SessionEvent> {
        match envelope.get("type").and_then(Value::as_str) {
            Some("system") => self.ingest_system(envelope),
            Some("stream_event") => self.ingest_stream_event(envelope),
            Some("assistant") => self.ingest_assistant(envelope),
            Some("user") => self.ingest_user(envelope),
            Some("result") => self.ingest_result(envelope),
            _ => Vec::new(),
        }
    }

    fn ingest_system(&mut self, envelope: &Value) -> Vec<SessionEvent> {
        if envelope.get("subtype").and_then(Value::as_str) != Some("init") {
            return Vec::new();
        }
        if let Some(session_id) = envelope
            .get("session_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        {
            self.peer_session_id = Some(session_id.to_string());
        }
        let model = envelope
            .get("model")
            .and_then(Value::as_str)
            .filter(|model| !model.is_empty())
            .map(str::to_string);
        if let Some(model) = model.clone() {
            self.current_model = Some(model);
        }
        self.last_manifest_model = model.clone();
        let models = match &model {
            Some(model) => vec![SessionModel {
                model_id: model.clone(),
                name: model.clone(),
                description: None,
                context_tokens: None,
                current_effort: None,
                efforts: None,
            }],
            None => Vec::new(),
        };
        vec![SessionEvent::SessionManifest {
            provider_id: Some("claude".to_string()),
            current_model_id: model,
            models,
            modes: None,
        }]
    }

    fn ingest_stream_event(&mut self, envelope: &Value) -> Vec<SessionEvent> {
        let event = match envelope.get("event") {
            Some(event) => event,
            None => return Vec::new(),
        };
        let is_subagent = envelope
            .get("parent_tool_use_id")
            .is_some_and(|value| !value.is_null());
        match event.get("type").and_then(Value::as_str) {
            Some("message_start") => {
                let message = event.get("message");
                let id = message
                    .and_then(|message| message.get("id"))
                    .and_then(Value::as_str);
                let model = message
                    .and_then(|message| message.get("model"))
                    .and_then(Value::as_str);
                if !is_subagent {
                    self.note_message(id, model);
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
                        self.add_streamed(index, text.len());
                        vec![SessionEvent::AgentMessage {
                            message_id: self.current_message_id.clone(),
                            text: text.to_string(),
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
                        self.add_streamed(index, text.len());
                        vec![SessionEvent::AgentThought {
                            message_id: self.current_message_id.clone(),
                            text: text.to_string(),
                        }]
                    }
                    _ => Vec::new(),
                }
            }
            _ => Vec::new(),
        }
    }

    fn ingest_assistant(&mut self, envelope: &Value) -> Vec<SessionEvent> {
        let message = match envelope.get("message") {
            Some(message) => message,
            None => return Vec::new(),
        };
        let is_subagent = envelope
            .get("parent_tool_use_id")
            .is_some_and(|value| !value.is_null());
        let model = if is_subagent {
            None
        } else {
            message.get("model").and_then(Value::as_str)
        };
        let model_changed = model.is_some_and(|model| {
            !model.is_empty()
                && self.last_manifest_model.is_some()
                && self.last_manifest_model.as_deref() != Some(model)
        });
        if !is_subagent {
            self.note_message(message.get("id").and_then(Value::as_str), model);
        }
        let mut events = Vec::new();
        if model_changed {
            self.last_manifest_model = self.current_model.clone();
            if let Some(model) = self.current_model.clone() {
                events.push(SessionEvent::SessionManifest {
                    provider_id: Some("claude".to_string()),
                    current_model_id: Some(model.clone()),
                    models: vec![SessionModel {
                        model_id: model.clone(),
                        name: model,
                        description: None,
                        context_tokens: None,
                        current_effort: None,
                        efforts: None,
                    }],
                    modes: None,
                });
            }
        }
        let Some(content) = message.get("content").and_then(Value::as_array) else {
            return events;
        };
        for (index, block) in content.iter().enumerate() {
            let index = index as u64;
            match block.get("type").and_then(Value::as_str) {
                Some("text") => {
                    let text = block.get("text").and_then(Value::as_str).unwrap_or("");
                    if let Some(text) = self.take_remainder(index, text) {
                        events.push(SessionEvent::AgentMessage {
                            message_id: self.current_message_id.clone(),
                            text,
                        });
                    }
                }
                Some("thinking") => {
                    let text = block.get("thinking").and_then(Value::as_str).unwrap_or("");
                    if let Some(text) = self.take_remainder(index, text) {
                        events.push(SessionEvent::AgentThought {
                            message_id: self.current_message_id.clone(),
                            text,
                        });
                    }
                }
                Some("tool_use") => {
                    if let Some(event) = tool_call_from_block(block, self.cwd.as_deref()) {
                        events.push(event);
                    }
                }
                _ => {}
            }
        }
        events
    }

    fn ingest_user(&mut self, envelope: &Value) -> Vec<SessionEvent> {
        let Some(content) = envelope
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(Value::as_array)
        else {
            return Vec::new();
        };
        content.iter().filter_map(tool_update_from_result).collect()
    }

    fn ingest_result(&mut self, envelope: &Value) -> Vec<SessionEvent> {
        let stop_reason = envelope
            .get("stop_reason")
            .and_then(Value::as_str)
            .unwrap_or("end_turn")
            .to_string();
        let model_id = envelope
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| self.current_model.clone());
        let usage = envelope.get("usage").and_then(usage_from_claude);
        vec![SessionEvent::AgentFinished {
            stop_reason,
            model_id,
            usage,
        }]
    }

    fn note_message(&mut self, id: Option<&str>, model: Option<&str>) {
        if let Some(model) = model.filter(|model| !model.is_empty()) {
            self.current_model = Some(model.to_string());
        }
        let Some(id) = id.filter(|id| !id.is_empty()) else {
            return;
        };
        if self.current_message_id.as_deref() != Some(id) {
            self.streamed.clear();
            self.current_message_id = Some(id.to_string());
        }
    }

    fn add_streamed(&mut self, index: u64, added: usize) {
        *self.streamed.entry(index).or_insert(0) += added;
    }

    fn take_remainder(&mut self, index: u64, full: &str) -> Option<String> {
        let streamed = self.streamed.get(&index).copied().unwrap_or(0);
        let emit = if streamed == 0 {
            full.to_string()
        } else if full.len() >= streamed && full.is_char_boundary(streamed) {
            full[streamed..].to_string()
        } else {
            full.to_string()
        };
        self.streamed.insert(index, full.len().max(streamed));
        if emit.is_empty() {
            None
        } else {
            Some(emit)
        }
    }
}

fn tool_kind(name: &str) -> &'static str {
    match name {
        "Read" => "read",
        "Edit" | "Write" | "NotebookEdit" => "edit",
        "Bash" | "PowerShell" => "execute",
        "Glob" | "Grep" => "search",
        "WebFetch" | "WebSearch" => "fetch",
        "Task" => "think",
        _ => "other",
    }
}

fn tool_title(name: &str, input: &Value) -> String {
    if let Some(description) = input
        .get("description")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        return format!("{name} {description}");
    }
    if let Some(command) = input
        .get("command")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        let command = if command.len() > 80 {
            format!("{}...", command.chars().take(80).collect::<String>())
        } else {
            command.to_string()
        };
        return format!("{name} {command}");
    }
    if let Some(path) = input
        .get("file_path")
        .or_else(|| input.get("path"))
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        return format!("{name} {path}");
    }
    name.to_string()
}

fn tool_locations(name: &str, input: &Value, cwd: Option<&Path>) -> Option<Vec<ToolLocation>> {
    let path = match name {
        "Read" | "Edit" | "Write" | "NotebookEdit" => input.get("file_path"),
        "Glob" | "Grep" => input.get("path"),
        _ => None,
    }
    .and_then(Value::as_str)
    .filter(|path| !path.is_empty())?;
    let line = input
        .get("offset")
        .or_else(|| input.get("line"))
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok());
    Some(vec![ToolLocation {
        path: relativize_tool_path(path, cwd),
        line,
    }])
}

fn tool_call_from_block(block: &Value, cwd: Option<&Path>) -> Option<SessionEvent> {
    let tool_call_id = block.get("id").and_then(Value::as_str)?.to_string();
    let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
    let input = block.get("input").unwrap_or(&Value::Null);
    Some(SessionEvent::AgentToolCall {
        tool_call_id,
        title: tool_title(name, input),
        status: "pending".to_string(),
        kind: Some(tool_kind(name).to_string()),
        locations: tool_locations(name, input, cwd),
    })
}

fn tool_result_text(content: &Value) -> String {
    if let Some(text) = content.as_str() {
        return text.to_string();
    }
    if let Some(blocks) = content.as_array() {
        return blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("");
    }
    String::new()
}

fn tool_update_from_result(block: &Value) -> Option<SessionEvent> {
    if block.get("type").and_then(Value::as_str) != Some("tool_result") {
        return None;
    }
    let tool_call_id = block
        .get("tool_use_id")
        .and_then(Value::as_str)?
        .to_string();
    let failed = block
        .get("is_error")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let text = block.get("content").map(tool_result_text);
    Some(SessionEvent::AgentToolUpdate {
        tool_call_id,
        status: Some(if failed {
            "failed".to_string()
        } else {
            "completed".to_string()
        }),
        text,
        kind: None,
        locations: None,
    })
}

fn usage_from_claude(usage: &Value) -> Option<TurnUsage> {
    let input_tokens = usage.get("input_tokens").and_then(Value::as_u64);
    let output_tokens = usage.get("output_tokens").and_then(Value::as_u64);
    let thought_tokens = usage
        .get("output_tokens_details")
        .and_then(|details| details.get("thinking_tokens"))
        .and_then(Value::as_u64);
    if input_tokens.is_none() && output_tokens.is_none() && thought_tokens.is_none() {
        return None;
    }
    Some(TurnUsage {
        input_tokens,
        output_tokens,
        total_tokens: None,
        thought_tokens,
    })
}

/// Relativize `path` against `cwd`. Paths that are not under cwd are
/// returned unchanged (absolute, as received).
pub(crate) fn relativize_tool_path(path: &str, cwd: Option<&Path>) -> String {
    let Some(cwd) = cwd else {
        return path.to_string();
    };
    let given = Path::new(path);
    if !given.is_absolute() {
        return path.to_string();
    }
    if let Ok(stripped) = given.strip_prefix(cwd) {
        if stripped.as_os_str().is_empty() {
            return path.to_string();
        }
        return stripped.to_string_lossy().into_owned();
    }
    #[cfg(windows)]
    {
        if let Some(relative) = relativize_windows_case_insensitive(given, cwd) {
            return relative;
        }
    }
    path.to_string()
}

#[cfg(windows)]
fn relativize_windows_case_insensitive(path: &Path, cwd: &Path) -> Option<String> {
    use std::path::Component;
    let path_components: Vec<Component<'_>> = path.components().collect();
    let cwd_components: Vec<Component<'_>> = cwd.components().collect();
    if path_components.len() <= cwd_components.len() {
        return None;
    }
    for (left, right) in path_components.iter().zip(cwd_components.iter()) {
        if !windows_components_eq_ignore_case(left, right) {
            return None;
        }
    }
    let remainder: PathBuf = path_components[cwd_components.len()..].iter().collect();
    if remainder.as_os_str().is_empty() {
        return None;
    }
    Some(remainder.to_string_lossy().into_owned())
}

#[cfg(windows)]
fn windows_components_eq_ignore_case(
    left: &std::path::Component<'_>,
    right: &std::path::Component<'_>,
) -> bool {
    use std::path::Component;
    match (*left, *right) {
        (Component::Prefix(a), Component::Prefix(b)) => {
            a.as_os_str().eq_ignore_ascii_case(b.as_os_str())
        }
        (Component::RootDir, Component::RootDir)
        | (Component::CurDir, Component::CurDir)
        | (Component::ParentDir, Component::ParentDir) => true,
        (Component::Normal(a), Component::Normal(b)) => a.eq_ignore_ascii_case(b),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn view() -> ClaudeView {
        ClaudeView::new(Some(PathBuf::from(
            r"C:\Users\gualt\AppData\Local\Temp\devboule-claude-perm2-allow-host-8r8qc09c",
        )))
    }

    // Reconstructed from recon/probes/claude-perm-probe2-allow-host.txt
    // (CLI 2.1.260, 2026-09-05). Field names are the measured snake_case.
    fn init_frame() -> Value {
        json!({
            "type": "system",
            "subtype": "init",
            "cwd": r"C:\Users\gualt\AppData\Local\Temp\devboule-claude-perm2-allow-host-8r8qc09c",
            "session_id": "cbe439d8-8e95-42c3-b6c7-40c7e5d3b3cd",
            "tools": ["Task", "Bash", "Read", "Edit", "Write"],
            "model": "claude-opus-5[1m]",
            "permissionMode": "default",
            "claude_code_version": "2.1.260"
        })
    }

    #[test]
    fn system_init_becomes_session_manifest_and_stores_session_id() {
        let mut mapper = view();
        let envelope = init_frame();
        let events = mapper.ingest(&envelope);
        assert_eq!(
            mapper.peer_session_id(),
            Some("cbe439d8-8e95-42c3-b6c7-40c7e5d3b3cd")
        );
        match events.as_slice() {
            [SessionEvent::SessionManifest {
                provider_id,
                current_model_id,
                models,
                modes,
            }] => {
                assert_eq!(provider_id.as_deref(), Some("claude"));
                assert_eq!(current_model_id.as_deref(), Some("claude-opus-5[1m]"));
                assert_eq!(models.len(), 1);
                assert_eq!(models[0].model_id, "claude-opus-5[1m]");
                assert_eq!(models[0].name, "claude-opus-5[1m]");
                assert!(modes.is_none());
            }
            other => panic!("expected SessionManifest, got {other:?}"),
        }
        assert_eq!(
            envelope["subtype"], "init",
            "derivation must not consume the envelope"
        );
    }

    #[test]
    fn assistant_model_change_reemits_a_session_manifest() {
        let mut mapper = view();
        let _ = mapper.ingest(&init_frame());
        let events = mapper.ingest(&json!({
            "type": "assistant",
            "message": {
                "model": "claude-sonnet-5",
                "id": "msg-model-change",
                "role": "assistant",
                "content": []
            }
        }));
        assert!(matches!(
            events.as_slice(),
            [SessionEvent::SessionManifest {
                current_model_id,
                ..
            }] if current_model_id.as_deref() == Some("claude-sonnet-5")
        ));
    }

    #[test]
    fn subagent_model_changes_do_not_flap_session_manifests() {
        let mut mapper = view();
        let _ = mapper.ingest(&init_frame());
        let frames = [
            json!({
                "type": "assistant",
                "message": {
                    "model": "claude-opus-5",
                    "id": "msg-top-level-1",
                    "role": "assistant",
                    "content": []
                },
                "parent_tool_use_id": null
            }),
            json!({
                "type": "assistant",
                "message": {
                    "model": "claude-haiku-4-5",
                    "id": "msg-subagent",
                    "role": "assistant",
                    "content": []
                },
                "parent_tool_use_id": "toolu_x"
            }),
            json!({
                "type": "assistant",
                "message": {
                    "model": "claude-opus-5",
                    "id": "msg-top-level-2",
                    "role": "assistant",
                    "content": []
                },
                "parent_tool_use_id": null
            }),
        ];
        let manifest_count = frames
            .iter()
            .flat_map(|frame| mapper.ingest(frame))
            .filter(|event| matches!(event, SessionEvent::SessionManifest { .. }))
            .count();
        assert_eq!(manifest_count, 1, "subagent model flapped the manifest");
    }

    #[test]
    fn stream_event_text_delta_becomes_agent_message() {
        // recon/probes/claude-streaming-shape.txt
        let mut mapper = view();
        let events = mapper.ingest(&json!({
            "type": "stream_event",
            "event": {
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "text_delta", "text": "1"}
            },
            "session_id": "3989672c-b6c4-4f26-886c-fb8dc3446418"
        }));
        assert_eq!(
            events,
            vec![SessionEvent::AgentMessage {
                message_id: None,
                text: "1".to_string(),
            }]
        );
    }

    #[test]
    fn stream_event_thinking_delta_becomes_agent_thought() {
        let mut mapper = view();
        let events = mapper.ingest(&json!({
            "type": "stream_event",
            "event": {
                "type": "content_block_delta",
                "index": 0,
                "delta": {"type": "thinking_delta", "thinking": "Let me think."}
            }
        }));
        assert_eq!(
            events,
            vec![SessionEvent::AgentThought {
                message_id: None,
                text: "Let me think.".to_string(),
            }]
        );
    }

    #[test]
    fn consolidated_assistant_after_deltas_emits_only_the_remainder() {
        // recon/probes/claude-streaming-shape.txt: deltas then a full assistant.
        let mut mapper = view();
        let _ = mapper.ingest(&json!({
            "type": "stream_event",
            "event": {"type": "message_start", "message": {"id": "msg_011CeiQ5WgY6ewJ4proDpmny", "model": "claude-opus-5"}}
        }));
        let _ = mapper.ingest(&json!({
            "type": "stream_event",
            "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "1\n2\n3\n4\n5"}}
        }));
        let _ = mapper.ingest(&json!({
            "type": "stream_event",
            "event": {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "\n6\n7\n8\n9"}}
        }));
        let remainder = mapper.ingest(&json!({
            "type": "assistant",
            "message": {
                "model": "claude-opus-5",
                "id": "msg_011CeiQ5WgY6ewJ4proDpmny",
                "role": "assistant",
                "content": [{"type": "text", "text": "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n13\n14\n15"}]
            }
        }));
        assert_eq!(
            remainder,
            vec![SessionEvent::AgentMessage {
                message_id: Some("msg_011CeiQ5WgY6ewJ4proDpmny".to_string()),
                text: "\n10\n11\n12\n13\n14\n15".to_string(),
            }]
        );
    }

    #[test]
    fn assistant_tool_use_maps_kind_and_relativized_locations() {
        // recon/probes/claude-perm-probe2-allow-host.txt
        let mut mapper = view();
        let events = mapper.ingest(&json!({
            "type": "assistant",
            "message": {
                "model": "claude-opus-5",
                "id": "msg_011CekBDWjVAzk4UYDqwDeNo",
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": "toolu_01SPEx5ftKiRM6gUm1VBwYKz",
                    "name": "Bash",
                    "input": {
                        "command": r"cmd /c del /q C:\Windows\Temp\devboule-nonexistent.txt",
                        "description": "Delete a nonexistent temp file"
                    }
                }]
            }
        }));
        match events.as_slice() {
            [SessionEvent::AgentToolCall {
                tool_call_id,
                title,
                status,
                kind,
                locations,
            }] => {
                assert_eq!(tool_call_id, "toolu_01SPEx5ftKiRM6gUm1VBwYKz");
                assert_eq!(title, "Bash Delete a nonexistent temp file");
                assert_eq!(status, "pending");
                assert_eq!(kind.as_deref(), Some("execute"));
                assert!(locations.is_none());
            }
            other => panic!("expected AgentToolCall, got {other:?}"),
        }
    }

    #[test]
    fn read_tool_use_is_kind_read_with_relativized_path() {
        let mut mapper = view();
        let cwd = mapper.cwd.clone().expect("test cwd");
        let file = cwd.join("src").join("lib.rs");
        let events = mapper.ingest(&json!({
            "type": "assistant",
            "message": {
                "id": "msg_read",
                "role": "assistant",
                "content": [{
                    "type": "tool_use",
                    "id": "toolu_read",
                    "name": "Read",
                    "input": {"file_path": file.to_string_lossy()}
                }]
            }
        }));
        match events.as_slice() {
            [SessionEvent::AgentToolCall {
                kind,
                locations,
                title,
                ..
            }] => {
                assert_eq!(kind.as_deref(), Some("read"));
                let locations = locations.as_ref().expect("locations");
                assert_eq!(locations.len(), 1);
                assert_eq!(
                    locations[0].path,
                    PathBuf::from("src").join("lib.rs").to_string_lossy()
                );
                assert!(title.contains("Read"));
            }
            other => panic!("expected Read tool call, got {other:?}"),
        }
    }

    #[test]
    fn user_tool_result_success_and_error() {
        let mut mapper = view();
        let ok = mapper.ingest(&json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [{
                    "tool_use_id": "toolu_ok",
                    "type": "tool_result",
                    "content": "devboule-perm-probe",
                    "is_error": false
                }]
            }
        }));
        assert_eq!(
            ok,
            vec![SessionEvent::AgentToolUpdate {
                tool_call_id: "toolu_ok".to_string(),
                status: Some("completed".to_string()),
                text: Some("devboule-perm-probe".to_string()),
                kind: None,
                locations: None,
            }]
        );
        let err = mapper.ingest(&json!({
            "type": "user",
            "message": {
                "role": "user",
                "content": [{
                    "type": "tool_result",
                    "content": "The user declined this command in the probe.",
                    "is_error": true,
                    "tool_use_id": "toolu_01SPEx5ftKiRM6gUm1VBwYKz"
                }]
            }
        }));
        assert_eq!(
            err,
            vec![SessionEvent::AgentToolUpdate {
                tool_call_id: "toolu_01SPEx5ftKiRM6gUm1VBwYKz".to_string(),
                status: Some("failed".to_string()),
                text: Some("The user declined this command in the probe.".to_string()),
                kind: None,
                locations: None,
            }]
        );
    }

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
            }] => {
                assert_eq!(stop_reason, "end_turn");
                assert_eq!(model_id.as_deref(), Some("claude-opus-5[1m]"));
                let usage = usage.as_ref().expect("usage");
                assert_eq!(usage.input_tokens, Some(4));
                assert_eq!(usage.output_tokens, Some(230));
            }
            other => panic!("expected AgentFinished, got {other:?}"),
        }
    }

    #[test]
    fn rate_limit_and_system_status_have_no_view() {
        let mut mapper = view();
        let rate = mapper.ingest(&json!({
            "type": "rate_limit_event",
            "rate_limit_info": {"status": "allowed"},
            "session_id": "cbe439d8-8e95-42c3-b6c7-40c7e5d3b3cd"
        }));
        assert!(rate.is_empty());
        let status = mapper.ingest(&json!({
            "type": "system",
            "subtype": "status",
            "status": "requesting",
            "session_id": "cbe439d8-8e95-42c3-b6c7-40c7e5d3b3cd"
        }));
        assert!(status.is_empty());
        let thinking_tokens = mapper.ingest(&json!({
            "type": "system",
            "subtype": "thinking_tokens",
            "estimated_tokens": 100
        }));
        assert!(thinking_tokens.is_empty());
    }

    #[test]
    fn tool_kind_mapping_covers_the_named_claude_tools() {
        let cases = [
            ("Read", "read"),
            ("Edit", "edit"),
            ("Write", "edit"),
            ("NotebookEdit", "edit"),
            ("Bash", "execute"),
            ("PowerShell", "execute"),
            ("Glob", "search"),
            ("Grep", "search"),
            ("WebFetch", "fetch"),
            ("WebSearch", "fetch"),
            ("Task", "think"),
            ("Skill", "other"),
        ];
        for (name, expected) in cases {
            let mut mapper = view();
            let events = mapper.ingest(&json!({
                "type": "assistant",
                "message": {
                    "id": "m",
                    "role": "assistant",
                    "content": [{"type": "tool_use", "id": "t", "name": name, "input": {}}]
                }
            }));
            match events.as_slice() {
                [SessionEvent::AgentToolCall { kind, .. }] => {
                    assert_eq!(kind.as_deref(), Some(expected), "tool {name}");
                }
                other => panic!("tool {name}: {other:?}"),
            }
        }
    }

    #[test]
    fn path_outside_cwd_is_forwarded_absolute() {
        let relativized = relativize_tool_path(
            r"C:\Windows\Temp\secret.txt",
            Some(Path::new(r"C:\Users\gualt\work")),
        );
        assert_eq!(relativized, r"C:\Windows\Temp\secret.txt");
    }

    #[test]
    fn path_under_cwd_is_relativized() {
        let relativized = relativize_tool_path(
            r"C:\Users\gualt\work\src\lib.rs",
            Some(Path::new(r"C:\Users\gualt\work")),
        );
        assert_eq!(
            relativized,
            PathBuf::from("src").join("lib.rs").to_string_lossy()
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_path_differing_only_by_case_is_relativized() {
        let relativized = relativize_tool_path(r"c:\work\src\Lib.rs", Some(Path::new(r"C:\Work")));
        assert_eq!(
            relativized,
            PathBuf::from("src").join("Lib.rs").to_string_lossy(),
            "Windows prefix match is case-insensitive; remainder keeps original casing"
        );
    }
}
