//! Codex image frames through the two halves of the path: a frame is prepared
//! (its sources replaced by stored markers, so the journal holds a reference)
//! and the prepared frame becomes events — live and on replay alike.

use base64::Engine;
use devboule_protocol::SessionEvent;
use serde_json::{json, Value};

use crate::agent_image::{AgentImageSink, StoredImage};

use super::CodexView;

const TINY_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

fn roots(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let workspace = crate::test_dirs::test_temp_dir(&format!("devboule-codex-image-{tag}"));
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(TINY_PNG)
        .expect("the fixture decodes");
    std::fs::write(workspace.join("tiny.png"), bytes).expect("the fixture lands");
    let images = crate::test_dirs::test_temp_dir(&format!("devboule-codex-image-{tag}-out"));
    (workspace, images)
}

fn sink(tag: &str, workspace: &std::path::Path, images: &std::path::Path) -> AgentImageSink {
    let store = crate::attachment_store::AttachmentStore::new(workspace);
    AgentImageSink::new(
        store,
        format!("s.{tag}.1"),
        workspace.to_path_buf(),
        images.to_path_buf(),
    )
}

fn codex(tag: &str) -> (CodexView, std::path::PathBuf, AgentImageSink) {
    let (workspace, images) = roots(tag);
    let sink = sink(tag, &workspace, &images);
    (CodexView::new(Some(workspace.clone())), workspace, sink)
}

fn completed(item: Value) -> Value {
    json!({"method": "item/completed", "params": {"item": item}})
}

/// The journal's own round trip: what a prepared frame looks like after being
/// written and read back.
fn journaled(value: &Value) -> Value {
    let line = serde_json::to_string(value).expect("the frame serializes");
    serde_json::from_str(&line).expect("the frame reads back")
}

fn image_of(event: &SessionEvent) -> Option<&devboule_protocol::AttachmentReference> {
    match event {
        SessionEvent::AgentMessage { images, .. } => images.first(),
        _ => None,
    }
}

#[test]
fn a_saved_generation_becomes_an_assistant_image_live_and_on_replay() {
    let (mut view, workspace, sink) = codex("saved");
    let mut frame = completed(json!({
        "type": "imageGeneration",
        "id": "img-1",
        "status": "completed",
        "savedPath": workspace.join("tiny.png").to_string_lossy(),
        "revisedPrompt": "a small red square",
    }));
    crate::codex_view::images::prepare_images(&mut frame, Some(&sink));
    // The journaled frame carries the reference, never the bytes.
    let prepared = journaled(&frame);
    assert!(prepared.to_string().contains("reference"));

    let live = view.ingest(&prepared);
    let reference = live
        .iter()
        .find_map(image_of)
        .expect("the live frame shows the image")
        .clone();

    // Replay: the same journaled frame through the replay road, no sink.
    let (mut replay, _, _) = codex("saved-replay");
    let replayed = replay.ingest_replay(&journaled(&prepared));
    let replayed_reference = replayed
        .iter()
        .find_map(image_of)
        .expect("replay shows the same image");
    assert_eq!(&reference, replayed_reference);
    match replayed.as_slice() {
        [SessionEvent::AgentMessage {
            message_id, text, ..
        }] => {
            assert_eq!(message_id.as_deref(), Some("img-1"));
            assert!(text.is_empty());
        }
        other => panic!("one assistant image expected: {other:?}"),
    }
}

#[test]
fn a_base64_generation_is_stored_once_and_the_journal_holds_no_base64() {
    let (mut view, _, sink) = codex("base64");
    let item = json!({
        "type": "imageGeneration",
        "id": "img-2",
        "status": "completed",
        "result": format!("data:image/png;base64,{TINY_PNG}"),
    });
    let mut started = json!({"method": "item/started", "params": {"item": item.clone()}});
    crate::codex_view::images::prepare_images(&mut started, Some(&sink));
    assert!(!started.to_string().contains(TINY_PNG));
    assert!(view.ingest(&started).is_empty());
    assert_eq!(
        started
            .pointer("/params/item/devboule_image")
            .and_then(StoredImage::from_value),
        Some(StoredImage::Pending)
    );

    let mut frame = completed(item);
    crate::codex_view::images::prepare_images(&mut frame, Some(&sink));
    assert!(!frame.to_string().contains(TINY_PNG));
    let events = view.ingest(&journaled(&frame));
    assert_eq!(events.iter().filter_map(image_of).count(), 1);
}

#[test]
fn a_url_generation_is_a_notice_never_a_fetch() {
    let (mut view, _, sink) = codex("url");
    let mut frame = completed(json!({
        "type": "imageGeneration",
        "id": "img-3",
        "status": "completed",
        "result": "http://127.0.0.1:1/tiny.png",
    }));
    crate::codex_view::images::prepare_images(&mut frame, Some(&sink));
    let events = view.ingest(&frame);
    match events.as_slice() {
        [SessionEvent::SessionNotice { text, .. }] => {
            assert!(text.contains("remote image not fetched"), "{text}")
        }
        other => panic!("one notice expected: {other:?}"),
    }
}

#[test]
fn a_failed_generation_is_a_tool_row_with_its_prompt() {
    let (mut view, _, sink) = codex("failed");
    let mut frame = completed(json!({
        "type": "imageGeneration",
        "id": "img-4",
        "status": "failed",
        "revisedPrompt": "a blue whale",
        "failure": {"type": "usageLimitExceeded", "limitId": "image_gen", "resetsAt": 42},
    }));
    crate::codex_view::images::prepare_images(&mut frame, Some(&sink));
    let events = view.ingest(&frame);
    let update = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::AgentToolUpdate { status, text, .. } => {
                Some((status.clone(), text.clone()))
            }
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
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::AgentToolCall { tool_call_id, status, .. }
            if tool_call_id == "img-4" && status == "failed"
    )));
}

#[test]
fn an_in_progress_generation_says_nothing_and_keeps_no_base64() {
    let (mut view, _, sink) = codex("progress");
    let mut frame = json!({
        "method": "item/started",
        "params": {"item": {
            "type": "imageGeneration", "id": "img-5", "status": "in_progress",
            "result": format!("data:image/png;base64,{TINY_PNG}"),
        }}
    });
    crate::codex_view::images::prepare_images(&mut frame, Some(&sink));
    assert!(!frame.to_string().contains(TINY_PNG));
    assert!(view.ingest(&frame).is_empty());
}

#[test]
fn a_viewed_image_becomes_an_assistant_image_and_a_started_one_says_nothing() {
    let (mut view, workspace, sink) = codex("viewed");
    let item = json!({
        "type": "imageView",
        "id": "img-6",
        "path": workspace.join("tiny.png").to_string_lossy(),
    });
    let mut started = json!({"method": "item/started", "params": {"item": item.clone()}});
    crate::codex_view::images::prepare_images(&mut started, Some(&sink));
    assert!(view.ingest(&started).is_empty());

    let mut frame = completed(item);
    crate::codex_view::images::prepare_images(&mut frame, Some(&sink));
    let events = view.ingest(&journaled(&frame));
    assert_eq!(events.iter().filter_map(image_of).count(), 1);
}

#[test]
fn a_tool_result_image_lands_on_its_row_and_a_refusal_is_named() {
    let (mut view, _, sink) = codex("tool-result");
    let mut frame = completed(json!({
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
    }));
    crate::codex_view::images::prepare_images(&mut frame, Some(&sink));
    assert!(!frame.to_string().contains(TINY_PNG));
    let events = view.ingest(&journaled(&frame));
    match events.as_slice() {
        [SessionEvent::AgentToolUpdate { text, images, .. }] => {
            let text = text.clone().expect("the text part");
            assert_eq!(text, "captured[image]");
            assert_eq!(images.len(), 1);
        }
        other => panic!("one tool update expected: {other:?}"),
    }

    // A refused payload: no image, the block is named as not stored, and the
    // reason reaches the transcript as a notice.
    let mut refused = completed(json!({
        "type": "mcpToolCall",
        "id": "call-2",
        "server": "devboule",
        "tool": "browser_screenshot",
        "status": "completed",
        "arguments": {},
        "result": {"content": [
            {"type": "image", "data": TINY_PNG, "mimeType": "image/jpeg"},
        ]},
    }));
    crate::codex_view::images::prepare_images(&mut refused, Some(&sink));
    let events = view.ingest(&refused);
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::SessionNotice { text, .. } if text.contains("1 image not shown")
    )));
    match events.first() {
        Some(SessionEvent::AgentToolUpdate { text, images, .. }) => {
            assert_eq!(text.as_deref(), Some("[image not stored]"));
            assert!(images.is_empty());
        }
        other => panic!("the row first, then the notice: {other:?}"),
    }
}
