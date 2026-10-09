//! The Codex `turn/start` input for an image-only message: the `localImage`
//! entries alone, with no empty text entry in front of them.

use super::super::turn_start_params_with_images;
use std::path::PathBuf;

#[test]
fn an_image_only_turn_sends_its_local_images_and_no_empty_text_entry() {
    let params = turn_start_params_with_images(
        "thread-1",
        "",
        &[PathBuf::from("/tmp/photo.png")],
        None,
        None,
        None,
        None,
    );
    let input = params["input"].as_array().expect("input array");
    assert_eq!(input.len(), 1, "{input:?}");
    assert_eq!(input[0]["type"], "localImage");
}
