//! An image-only prompt on the Claude plan: the API refuses a text block whose
//! text is empty, so the frame carries the image block and no text entry.

use super::super::{frame_user_message_with_images, plan_claude_prompt};
use super::{plan_attachment, PlanTempDir};
use crate::attachment_store::AttachmentStore;
use crate::raster_metadata::clean_png;
use serde_json::Value;

#[test]
fn an_image_only_prompt_frames_the_image_and_no_empty_text_block() {
    let temp = PlanTempDir::new("image-only");
    let store = AttachmentStore::new(&temp.0);
    let plan = plan_claude_prompt(
        &store,
        "claude-plan-image-only",
        "",
        &[plan_attachment("photo.png", "image/png", &clean_png(0x01))],
        &[],
    )
    .expect("materialized")
    .expect("a raster plans a block");
    assert_eq!(plan.images.len(), 1);
    let bytes = frame_user_message_with_images(&plan.fallback_text, &plan.images).expect("frame");
    let value: Value =
        serde_json::from_str(std::str::from_utf8(&bytes).expect("utf8").trim_end()).expect("json");
    let content = value["message"]["content"].as_array().expect("array");
    assert_eq!(content.len(), 1, "{content:?}");
    assert_eq!(content[0]["type"], "image");
}
