//! The Codex image thread items as events, on real frames: a generated image
//! (saved path, base64, failed), a viewed image, and a tool result that
//! carries one.

use base64::Engine;
use devboule_protocol::SessionEvent;
use serde_json::{json, Value};

use super::CodexView;

const TINY_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

fn workspace(tag: &str) -> std::path::PathBuf {
    let dir = crate::test_dirs::test_temp_dir(&format!("devboule-codex-image-{tag}"));
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(TINY_PNG)
        .expect("the fixture decodes");
    std::fs::write(dir.join("tiny.png"), bytes).expect("the fixture lands");
    dir
}

fn view(tag: &str) -> (CodexView, std::path::PathBuf) {
    let workspace = workspace(tag);
    let store = crate::attachment_store::AttachmentStore::new(&workspace);
    let sink =
        crate::agent_image::AgentImageSink::new(store, format!("s.{tag}.1"), workspace.clone());
    (
        CodexView::new(Some(workspace.clone())).with_images(sink),
        workspace,
    )
}

fn completed(item: Value) -> Value {
    json!({"method": "item/completed", "params": {"item": item}})
}

#[test]
fn a_saved_generation_becomes_an_assistant_image() {
    let (mut view, workspace) = view("saved");
    let events = view.ingest(&completed(json!({
        "type": "imageGeneration",
        "id": "img-1",
        "status": "completed",
        "savedPath": workspace.join("tiny.png").to_string_lossy(),
        "revisedPrompt": "a small red square",
    })));
    match events.as_slice() {
        [SessionEvent::AgentMessage {
            message_id,
            text,
            images,
            ..
        }] => {
            assert_eq!(message_id.as_deref(), Some("img-1"));
            assert!(text.is_empty());
            assert_eq!(images.len(), 1);
            assert_eq!(images[0].session_id, "s.saved.1");
        }
        other => panic!("one assistant image expected: {other:?}"),
    }
}

#[test]
fn a_base64_generation_becomes_an_assistant_image() {
    let (mut view, _) = view("base64");
    let events = view.ingest(&completed(json!({
        "type": "imageGeneration",
        "id": "img-2",
        "status": "completed",
        "result": format!("data:image/png;base64,{TINY_PNG}"),
    })));
    match events.as_slice() {
        [event] => match event {
            SessionEvent::AgentMessage { images, .. } => {
                assert_eq!(images.len(), 1)
            }
            other => panic!("an assistant image expected: {other:?}"),
        },
        other => panic!("one event expected: {other:?}"),
    }
}

#[test]
fn an_in_progress_generation_says_nothing() {
    let (mut view, _) = view("progress");
    let events = view.ingest(&json!({
        "method": "item/started",
        "params": {"item": {"type": "imageGeneration", "id": "img-3", "status": "in_progress", "result": ""}}
    }));
    assert!(events.is_empty(), "{events:?}");
}

#[test]
fn a_failed_generation_is_a_tool_row_with_its_prompt() {
    let (mut view, _) = view("failed");
    let events = view.ingest(&completed(json!({
        "type": "imageGeneration",
        "id": "img-4",
        "status": "failed",
        "revisedPrompt": "a blue whale",
        "failure": {"type": "usageLimitExceeded", "limitId": "image_gen", "resetsAt": 42},
    })));
    use SessionEvent;
    let update = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::AgentToolUpdate {
                status,
                text,
                images,
                ..
            } => Some((status.clone(), text.clone(), images.clone())),
            _ => None,
        })
        .expect("the failed row's update");
    assert_eq!(update.0.as_deref(), Some("failed"));
    let text = update.1.expect("the prompt and reason");
    assert!(text.contains("a blue whale"), "{text}");
    assert!(
        text.contains("image generation limit exceeded (image_gen)"),
        "{text}"
    );
    assert!(update.2.is_empty());
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::AgentToolCall { tool_call_id, status, .. }
            if tool_call_id == "img-4" && status == "failed"
    )));
}

#[test]
fn a_viewed_image_becomes_an_assistant_image() {
    let (mut view, workspace) = view("viewed");
    let events = view.ingest(&completed(json!({
        "type": "imageView",
        "id": "img-5",
        "path": workspace.join("tiny.png").to_string_lossy(),
    })));
    match events.as_slice() {
        [event] => match event {
            SessionEvent::AgentMessage { images, .. } => {
                assert_eq!(images.len(), 1)
            }
            other => panic!("an assistant image expected: {other:?}"),
        },
        other => panic!("one event expected: {other:?}"),
    }
}

#[test]
fn a_tool_result_image_lands_on_its_row() {
    let (mut view, _) = view("tool-result");
    let events = view.ingest(&completed(json!({
        "type": "mcpToolCall",
        "id": "call-1",
        "server": "devboule",
        "tool": "browser_screenshot",
        "status": "completed",
        "arguments": {},
        "result": {"content": [
            {"type": "text", "text": "captured"},
            {"type": "image", "data": TINY_PNG, "mimeType": "image/png"},
        ]},
    })));
    use SessionEvent;
    match events.as_slice() {
        [SessionEvent::AgentToolUpdate { text, images, .. }] => {
            let text = text.clone().expect("the text part");
            assert!(text.contains("captured"), "{text}");
            assert!(text.contains("[image]"), "{text}");
            assert_eq!(images.len(), 1);
        }
        other => panic!("one tool update expected: {other:?}"),
    }
}
