//! The Claude send seam for images that travelled as stored references: the
//! composer deposits and names the deposit, so the frame reaching Claude must
//! carry the image block itself — a path line here sends the agent to Read the
//! user's own attachment through a permission card.

use super::super::{ClaudeModeGate, ClaudeModeGateRef, ClaudeModeGateState, ClaudeStaticPrompt};
use super::test_abort_gate;
use crate::raster_metadata::{clean_png, png_with_text_chunk};
use crate::session::tests::{
    attach_live_agent_for_test, attachment, insert_live_agent_with_kind_writer_and_sink,
    test_owner, tmp_delete_registry, RecordingWriter,
};
use devboule_protocol::{SessionEvent, SessionKind};
use serde_json::Value;
use std::sync::{Arc, Mutex};

/// One deposited PNG, sent by reference: the frame carries the text block and
/// one image block with the stored (stripped) bytes, the plain writer is not
/// typed into, and the transcript keeps the user's words with the reference —
/// never the base64.
#[test]
fn a_claude_send_inlines_a_stored_png_reference() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-claude-ref", "process-claude-ref");
    let received = Arc::new(Mutex::new(Vec::new()));
    let gate: ClaudeModeGateRef = Arc::new(Mutex::new(ClaudeModeGate {
        state: ClaudeModeGateState::AwaitingResponse {
            request_id: "initial-permission-mode-0".to_string(),
        },
        pending_frames: Vec::new(),
    }));
    let route = ClaudeStaticPrompt::new(
        Arc::new(Mutex::new(None)),
        Some(Arc::clone(&gate)),
        test_abort_gate(),
        None,
    );
    let runtime = insert_live_agent_with_kind_writer_and_sink(
        &registry,
        "claude-ref",
        owner.clone(),
        SessionKind::Claude,
        Box::new(RecordingWriter(Arc::clone(&received))),
        None,
        Some(Arc::new(route)),
    );
    let conn = attach_live_agent_for_test(&runtime, "claude-ref", 81);
    // The wire carried a tEXt chunk; the store kept the stripped file, and the
    // block must carry what the store kept.
    let sent = png_with_text_chunk();
    let kept = clean_png(0x01);
    assert_ne!(
        sent, kept,
        "the fixture must actually carry something that leaves"
    );
    let reference = registry
        .deposit(
            "claude-ref",
            &owner,
            &conn,
            &attachment("photo.png", "image/png", &sent),
        )
        .expect("the owner may deposit into their own session");
    registry
        .send_with_subscription(
            "claude-ref",
            81,
            "look at this",
            &[],
            std::slice::from_ref(&reference),
            &owner,
            &conn,
        )
        .expect("send with one deposited reference");

    let frames = gate.lock().expect("gate").pending_frames.clone();
    assert_eq!(
        frames.len(),
        1,
        "the prompt went out through the Claude route, not the plain writer"
    );
    let value: Value = serde_json::from_slice(&frames[0]).expect("json");
    let content = value["message"]["content"]
        .as_array()
        .expect("content array");
    assert_eq!(content.len(), 2, "{content:?}");
    let text = content[0]["text"].as_str().expect("text block");
    assert_eq!(text, "look at this");
    assert!(
        !text.contains("[Image available at: "),
        "no path line for an inlined reference: {text}"
    );
    assert_eq!(content[1]["source"]["media_type"], "image/png");
    use base64::Engine;
    assert_eq!(
        content[1]["source"]["data"].as_str(),
        Some(
            base64::engine::general_purpose::STANDARD
                .encode(&kept)
                .as_str()
        ),
        "the block carries the stored, stripped bytes"
    );
    assert!(
        received.lock().expect("writer").is_empty(),
        "the plain writer typed nothing"
    );
    let recorded = conn
        .pull_events()
        .into_iter()
        .map(|event| event.envelope.event)
        .find_map(|event| match event {
            SessionEvent::AgentUserMessage { text, images, .. } => Some((text, images)),
            _ => None,
        })
        .expect("the user message is published, and that is what the journal records");
    assert_eq!(recorded.0, "look at this");
    assert_eq!(
        recorded.1,
        vec![reference],
        "the transcript keeps the reference beside the raw text"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// A PNG and an SVG by reference in one send: the PNG becomes the block, the
/// SVG keeps its `[Image available at: …]` line, in the same frame.
#[test]
fn a_claude_send_splits_a_png_and_an_svg_reference() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-claude-split", "process-claude-split");
    let received = Arc::new(Mutex::new(Vec::new()));
    let gate: ClaudeModeGateRef = Arc::new(Mutex::new(ClaudeModeGate {
        state: ClaudeModeGateState::AwaitingResponse {
            request_id: "initial-permission-mode-0".to_string(),
        },
        pending_frames: Vec::new(),
    }));
    let route = ClaudeStaticPrompt::new(
        Arc::new(Mutex::new(None)),
        Some(Arc::clone(&gate)),
        test_abort_gate(),
        None,
    );
    let runtime = insert_live_agent_with_kind_writer_and_sink(
        &registry,
        "claude-ref-split",
        owner.clone(),
        SessionKind::Claude,
        Box::new(RecordingWriter(Arc::clone(&received))),
        None,
        Some(Arc::new(route)),
    );
    let conn = attach_live_agent_for_test(&runtime, "claude-ref-split", 82);
    let png = registry
        .deposit(
            "claude-ref-split",
            &owner,
            &conn,
            &attachment("photo.png", "image/png", &clean_png(0x21)),
        )
        .expect("deposit the png");
    let svg = registry
        .deposit(
            "claude-ref-split",
            &owner,
            &conn,
            &attachment(
                "drawing.svg",
                "image/svg+xml",
                b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
            ),
        )
        .expect("deposit the svg");
    registry
        .send_with_subscription(
            "claude-ref-split",
            82,
            "logo and photo",
            &[],
            &[png, svg.clone()],
            &owner,
            &conn,
        )
        .expect("send with two deposited references");

    let frames = gate.lock().expect("gate").pending_frames.clone();
    assert_eq!(
        frames.len(),
        1,
        "the mixed prompt went out through the Claude route"
    );
    let value: Value = serde_json::from_slice(&frames[0]).expect("json");
    let content = value["message"]["content"]
        .as_array()
        .expect("content array");
    assert_eq!(content.len(), 2, "{content:?}");
    let text = content[0]["text"].as_str().expect("text block");
    assert_eq!(
        text.matches("[Image available at: ").count(),
        1,
        "exactly the SVG keeps a path line: {text}"
    );
    assert!(
        !text.contains(".png]"),
        "the inlined png leaves no path line: {text}"
    );
    let svg_path = registry
        .attachments
        .resolve("claude-ref-split", &svg.digest, None)
        .expect("the deposited svg resolves")
        .0;
    assert!(
        text.ends_with(&format!("[Image available at: {}]", svg_path.display())),
        "the svg line comes last: {text}"
    );
    assert_eq!(content[1]["source"]["media_type"], "image/png");
    assert!(
        received.lock().expect("writer").is_empty(),
        "the plain writer typed nothing"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}
