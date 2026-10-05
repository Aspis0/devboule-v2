//! The Claude plan for stored references: which references become image
//! blocks and which keep a path line in the text — the routing and grouping
//! half the send seam above depends on.

use super::super::{carried_mime_types, frame_user_message_with_images, plan_claude_prompt};
use super::{plan_attachment, reference_for, PlanTempDir};
use crate::attachment_store::AttachmentStore;
use crate::raster_metadata::{clean_png, png_with_text_chunk};
use crate::session::session_prompt_planning::resolve_attachment_references;
use serde_json::Value;

/// The deposit strips on the way in, so the block must carry what the store
/// kept — the same bytes a fresh attachment's block carries — and the frame
/// must name no local path.
#[test]
fn a_stored_png_reference_plans_a_block_with_the_stripped_bytes() {
    let temp = PlanTempDir::new("ref-png");
    let store = AttachmentStore::new(&temp.0);
    let session_id = "claude-ref-png";
    let sent = png_with_text_chunk();
    let kept = clean_png(0x01);
    assert_ne!(
        sent, kept,
        "the fixture must actually carry something that leaves"
    );
    let stored = store
        .session(session_id)
        .expect("session")
        .materialize(&plan_attachment("photo.png", "image/png", &sent))
        .expect("stored");
    let reference = reference_for(session_id, &stored);
    let paths = resolve_attachment_references(&store, session_id, std::slice::from_ref(&reference))
        .expect("resolved");
    let plan = plan_claude_prompt(&store, session_id, "look at this", &[], &paths)
        .expect("planned")
        .expect("a stored raster plans a frame");
    assert_eq!(carried_mime_types(Some(&plan)), vec!["image/png"]);
    assert_eq!(
        plan.fallback_text, "look at this",
        "no path line: {}",
        plan.fallback_text
    );
    {
        use base64::Engine;
        assert_eq!(
            plan.images[0].data_base64,
            base64::engine::general_purpose::STANDARD.encode(&kept),
            "the block carries the stored, stripped bytes"
        );
    }
    let bytes = frame_user_message_with_images(&plan.fallback_text, &plan.images).expect("frame");
    let frame = std::str::from_utf8(&bytes).expect("utf8");
    assert!(
        !frame.contains("[Image available at"),
        "the inlined frame carries no local path: {frame}"
    );
}

/// One send, two references: the raster becomes the block, the SVG keeps its
/// path line in the same frame's text.
#[test]
fn an_svg_reference_keeps_its_path_line_beside_an_inlined_png_reference() {
    let temp = PlanTempDir::new("ref-split");
    let store = AttachmentStore::new(&temp.0);
    let session_id = "claude-ref-split";
    let png = store
        .session(session_id)
        .expect("session")
        .materialize(&plan_attachment("photo.png", "image/png", &clean_png(0x24)))
        .expect("stored");
    let svg = store
        .session(session_id)
        .expect("session")
        .materialize(&plan_attachment(
            "drawing.svg",
            "image/svg+xml",
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
        ))
        .expect("stored");
    let references = [
        reference_for(session_id, &png),
        reference_for(session_id, &svg),
    ];
    let paths = resolve_attachment_references(&store, session_id, &references).expect("resolved");
    let plan = plan_claude_prompt(&store, session_id, "logo and photo", &[], &paths)
        .expect("planned")
        .expect("the raster reference plans a frame");
    assert_eq!(carried_mime_types(Some(&plan)), vec!["image/png"]);
    assert_eq!(
        plan.fallback_text,
        format!(
            "logo and photo\n\n[Image available at: {}]",
            paths[1].path.display()
        ),
        "only the SVG keeps a line, and it comes last"
    );
    let bytes = frame_user_message_with_images(&plan.fallback_text, &plan.images).expect("frame");
    let value: Value =
        serde_json::from_str(std::str::from_utf8(&bytes).expect("utf8").trim_end()).expect("json");
    let content = value["message"]["content"]
        .as_array()
        .expect("content array");
    assert_eq!(content.len(), 2, "one text block and one image block");
    assert_eq!(content[1]["source"]["media_type"], "image/png");
}

/// The frame is text then images: every fallback line goes in the text block
/// and every image block follows it, so cross-kind list order is not kept.
#[test]
fn an_svg_then_png_reference_order_groups_the_same_way_as_png_then_svg() {
    let temp = PlanTempDir::new("ref-order");
    let store = AttachmentStore::new(&temp.0);
    let session_id = "claude-ref-order";
    let svg = store
        .session(session_id)
        .expect("session")
        .materialize(&plan_attachment(
            "drawing.svg",
            "image/svg+xml",
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>",
        ))
        .expect("stored");
    let png = store
        .session(session_id)
        .expect("session")
        .materialize(&plan_attachment("photo.png", "image/png", &clean_png(0x31)))
        .expect("stored");
    let references = [
        reference_for(session_id, &svg),
        reference_for(session_id, &png),
    ];
    let paths = resolve_attachment_references(&store, session_id, &references).expect("resolved");
    let plan = plan_claude_prompt(&store, session_id, "logo and photo", &[], &paths)
        .expect("planned")
        .expect("the raster reference plans a frame");
    assert_eq!(carried_mime_types(Some(&plan)), vec!["image/png"]);
    assert_eq!(
        plan.fallback_text,
        format!(
            "logo and photo\n\n[Image available at: {}]",
            paths[0].path.display()
        ),
        "the SVG line sits in the text even though it was listed first"
    );
    let bytes = frame_user_message_with_images(&plan.fallback_text, &plan.images).expect("frame");
    let value: Value =
        serde_json::from_str(std::str::from_utf8(&bytes).expect("utf8").trim_end()).expect("json");
    let content = value["message"]["content"]
        .as_array()
        .expect("content array");
    assert_eq!(content.len(), 2, "text block first, image block after it");
    assert!(
        content[0]["text"].as_str().expect("text").ends_with("]"),
        "the text block carries the fallback line"
    );
    assert_eq!(content[1]["source"]["media_type"], "image/png");
}
