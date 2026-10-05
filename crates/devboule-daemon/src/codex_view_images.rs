//! Codex image items in and out. Before a frame is journaled its image
//! sources are replaced by stored markers, so the journal holds a reference
//! once; the prepared frame then becomes events, live and on replay alike.
//!
//! Behaviour follows Paseo `codex-app-server-agent.ts` (commit 4ed13fadb,
//! Apache-2.0): a successful generation and a viewed image are assistant
//! images, a failed generation is a tool row carrying its prompt, and an
//! in-progress item says nothing.

use devboule_protocol::SessionEvent;
use serde_json::{json, Value};

use crate::agent_image::{
    codex_image_source, image_block_source, AgentImageSink, AgentImageSource, StoredImage,
    MAX_IMAGES_PER_FRAME,
};
use crate::text_cap::capped;

/// Replace one frame's image sources with stored markers. A running item is
/// only marked pending: the store is written once, when the item completes.
pub(crate) fn prepare_images(value: &mut Value, sink: Option<&AgentImageSink>) {
    let Some(completed) = completed_item(value) else {
        return;
    };
    let Some(item) = value.pointer_mut("/params/item") else {
        return;
    };
    match item.get("type").and_then(Value::as_str) {
        Some("imageGeneration" | "imageView") => {
            let Some(source) = codex_image_source(item) else {
                return;
            };
            let stored = store(source, completed, sink);
            if let Some(object) = item.as_object_mut() {
                object.remove("savedPath");
                object.remove("saved_path");
                object.remove("result");
                object.remove("path");
                object.insert("devboule_image".to_string(), stored.to_value());
            }
        }
        Some("mcpToolCall") => prepare_result_blocks(item, completed, sink),
        _ => {}
    }
}

/// Whether this frame is an item's completion; `Some(false)` for its start,
/// `None` for a frame that carries no item lifecycle.
fn completed_item(value: &Value) -> Option<bool> {
    match value.get("method").and_then(Value::as_str) {
        Some("item/started") => Some(false),
        Some("item/completed") => Some(true),
        _ => None,
    }
}

fn store(source: AgentImageSource, completed: bool, sink: Option<&AgentImageSink>) -> StoredImage {
    if !completed {
        return StoredImage::Pending;
    }
    let Some(sink) = sink else {
        return StoredImage::Refused("this session has no store for images".to_string());
    };
    match sink.store(&source) {
        Ok(reference) => StoredImage::Reference(reference),
        Err(reason) => StoredImage::Refused(reason),
    }
}

fn prepare_result_blocks(item: &mut Value, completed: bool, sink: Option<&AgentImageSink>) {
    let Some(blocks) = item
        .pointer_mut("/result/content")
        .and_then(Value::as_array_mut)
    else {
        return;
    };
    let mut taken = 0;
    for block in blocks.iter_mut() {
        let Some(source) = image_block_source(block) else {
            continue;
        };
        if taken >= MAX_IMAGES_PER_FRAME {
            *block = marker(StoredImage::Refused(
                "this result carried more images than one frame stores".to_string(),
            ));
            continue;
        }
        taken += 1;
        *block = marker(store(source, completed, sink));
    }
}

fn marker(stored: StoredImage) -> Value {
    json!({"devboule_image": stored.to_value()})
}

/// One completed image item's events. A refused image is a notice, never a
/// row that claims an image it does not carry.
pub(super) fn image_item_events(id: &str, item: &Value, completed: bool) -> Vec<SessionEvent> {
    if !completed {
        return Vec::new();
    }
    let Some(stored) = item.get("devboule_image").and_then(StoredImage::from_value) else {
        return Vec::new();
    };
    match stored {
        StoredImage::Reference(reference) => vec![SessionEvent::AgentMessage {
            message_id: Some(id.to_string()),
            text: String::new(),
            parent_tool_use_id: None,
            spawn_depth: None,
            images: vec![reference],
        }],
        StoredImage::Refused(reason) => vec![refusal_notice(&reason)],
        StoredImage::Pending => Vec::new(),
    }
}

pub(super) fn refusal_notice(reason: &str) -> SessionEvent {
    SessionEvent::SessionNotice {
        text: capped(&format!("An agent image was not stored: {reason}")),
        severity: devboule_protocol::NoticeSeverity::Warning,
    }
}

/// The failed-generation row: the prompt that was asked for, then why it
/// failed. Paseo carries the same pair as `input.prompt` and `error`.
pub(super) fn failed_generation_events(id: &str, item: &Value) -> Vec<SessionEvent> {
    let failure = generation_failure(item);
    let prompt = item
        .get("revisedPrompt")
        .or_else(|| item.get("revised_prompt"))
        .and_then(Value::as_str)
        .filter(|prompt| !prompt.trim().is_empty());
    let text = match prompt {
        Some(prompt) => format!("{prompt}\n{failure}"),
        None => failure,
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

/// A failed generation is a row even though a running item is marked pending:
/// the item's status is what says the failure happened.
pub(super) fn is_failed_generation(item: &Value) -> bool {
    item.get("type").and_then(Value::as_str) == Some("imageGeneration")
        && item.get("status").and_then(Value::as_str) == Some("failed")
}

fn generation_failure(item: &Value) -> String {
    item.get("failure")
        .filter(|failure| !failure.is_null())
        .map(failure_text)
        .unwrap_or_else(|| "Image generation failed".to_string())
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
