//! The Codex image thread items as transcript events.
//!
//! Behaviour follows Paseo `codex-app-server-agent.ts` (commit 4ed13fadb,
//! Apache-2.0): a successful generation and a viewed image are assistant
//! images; a failed generation is a tool row that carries its prompt; an
//! in-progress item says nothing.

use devboule_protocol::SessionEvent;
use serde_json::Value;

use crate::agent_image::{codex_image_source, AgentImageSink};
use crate::text_cap::capped;

pub(super) fn image_generation_events(
    id: &str,
    item: &Value,
    completed: bool,
    images: Option<&AgentImageSink>,
) -> Vec<SessionEvent> {
    if !completed {
        return Vec::new();
    }
    if let Some(failure) = generation_failure(item) {
        return failed_generation_events(id, item, &failure);
    }
    let Some(source) = codex_image_source(item) else {
        return Vec::new();
    };
    let Some(image) = images.and_then(|sink| sink.store(&source)) else {
        return Vec::new();
    };
    vec![assistant_image(id, image)]
}

pub(super) fn image_view_events(
    id: &str,
    item: &Value,
    images: Option<&AgentImageSink>,
) -> Vec<SessionEvent> {
    let Some(source) = codex_image_source(item) else {
        return Vec::new();
    };
    let Some(image) = images.and_then(|sink| sink.store(&source)) else {
        return Vec::new();
    };
    vec![assistant_image(id, image)]
}

fn assistant_image(id: &str, image: devboule_protocol::AttachmentReference) -> SessionEvent {
    SessionEvent::AgentMessage {
        message_id: Some(id.to_string()),
        text: String::new(),
        parent_tool_use_id: None,
        spawn_depth: None,
        images: vec![image],
    }
}

/// The failure a failed item carries, defaulted the way Paseo defaults a
/// failure with no body.
fn generation_failure(item: &Value) -> Option<String> {
    if item.get("status").and_then(Value::as_str) != Some("failed") {
        return None;
    }
    let detail = item
        .get("failure")
        .filter(|failure| !failure.is_null())
        .map(failure_text)
        .unwrap_or_else(|| "Image generation failed".to_string());
    Some(detail)
}

fn failure_text(failure: &Value) -> String {
    match failure.get("type").and_then(Value::as_str) {
        Some("usageLimitExceeded") => {
            let limit = failure
                .get("limitId")
                .and_then(Value::as_str)
                .unwrap_or("image generation");
            format!("image generation limit exceeded ({limit})")
        }
        _ => "Image generation failed".to_string(),
    }
}

/// The failed-generation row: the prompt that was asked for, then why it
/// failed. Paseo carries the same pair as `input.prompt` and `error`.
fn failed_generation_events(id: &str, item: &Value, failure: &str) -> Vec<SessionEvent> {
    let prompt = item
        .get("revisedPrompt")
        .or_else(|| item.get("revised_prompt"))
        .and_then(Value::as_str)
        .filter(|prompt| !prompt.trim().is_empty());
    let text = match prompt {
        Some(prompt) => format!("{prompt}\n{failure}"),
        None => failure.to_string(),
    };
    vec![
        SessionEvent::AgentToolCall {
            tool_call_id: id.to_string(),
            title: "image_generation".to_string(),
            status: "failed".to_string(),
            kind: Some("image_generation".to_string()),
            locations: None,
            subagent_type: None,
            parent_tool_use_id: None,
            spawn_depth: None,
            command: None,
            exit_code: None,
        },
        SessionEvent::AgentToolUpdate {
            tool_call_id: id.to_string(),
            status: Some("failed".to_string()),
            text: Some(capped(&text)),
            title: Some("image_generation".to_string()),
            kind: Some("image_generation".to_string()),
            locations: None,
            parent_tool_use_id: None,
            spawn_depth: None,
            command: None,
            exit_code: None,
            replace: false,
            images: Vec::new(),
        },
    ]
}
