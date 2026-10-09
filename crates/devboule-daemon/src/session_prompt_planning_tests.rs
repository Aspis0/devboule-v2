//! The ACP `prompt` array for an image-only message: the text block is left
//! out, because a provider refuses an empty text block.

use super::{AcpImageBlock, StructuredPromptPlan};

fn image() -> AcpImageBlock {
    AcpImageBlock {
        mime_type: "image/png".to_string(),
        data_base64: "aGk=".to_string(),
    }
}

#[test]
fn an_image_only_plan_sends_its_images_and_no_empty_text_block() {
    let plan = StructuredPromptPlan {
        fallback_text: String::new(),
        images: vec![image()],
    };
    let blocks = plan.content_blocks();
    assert_eq!(blocks.len(), 1, "{blocks:?}");
    assert_eq!(blocks[0]["type"], "image");
}

#[test]
fn a_plan_with_text_keeps_the_text_block_first() {
    let plan = StructuredPromptPlan {
        fallback_text: "look".to_string(),
        images: vec![image()],
    };
    let blocks = plan.content_blocks();
    assert_eq!(blocks.len(), 2, "{blocks:?}");
    assert_eq!(blocks[0]["type"], "text");
    assert_eq!(blocks[0]["text"], "look");
    assert_eq!(blocks[1]["type"], "image");
}
