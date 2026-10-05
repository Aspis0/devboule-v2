//! The provider-image parser and its refusals: which wire strings are bytes,
//! which are paths or URLs, and what a deposit refuses.

use base64::Engine;
use serde_json::json;

use super::*;

/// A 1×1 transparent PNG, the smallest container the sniffing accepts.
pub(super) const TINY_PNG: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==";

pub(super) fn tiny_png_bytes() -> Vec<u8> {
    base64::engine::general_purpose::STANDARD
        .decode(TINY_PNG)
        .expect("the fixture decodes")
}

pub(super) fn workspace(tag: &str) -> std::path::PathBuf {
    let dir = crate::test_dirs::test_temp_dir(&format!("devboule-agent-image-{tag}"));
    std::fs::write(dir.join("tiny.png"), tiny_png_bytes()).expect("the fixture lands");
    dir
}

pub(super) fn sink(tag: &str, workspace: &Path) -> AgentImageSink {
    let store = crate::attachment_store::AttachmentStore::new(workspace);
    AgentImageSink::new(
        store,
        format!("s.agent.image.{tag}"),
        workspace.to_path_buf(),
    )
}

#[test]
fn a_saved_path_wins_over_the_result() {
    let item = json!({
        "type": "imageGeneration",
        "id": "img-1",
        "status": "completed",
        "savedPath": "/tmp/from-saved-path.png",
        "result": "https://example.invalid/from-result.png",
    });
    match codex_image_source(&item) {
        Some(AgentImageSource::Path(path)) => {
            assert_eq!(path, PathBuf::from("/tmp/from-saved-path.png"))
        }
        other => panic!("savedPath must win: {other:?}"),
    }
}

#[test]
fn a_result_string_is_bytes_or_a_url() {
    let data_url =
        json!({"id": "i", "type": "imageGeneration", "result": "data:image/png;base64,QUJD"});
    match codex_image_source(&data_url) {
        Some(AgentImageSource::Base64 { mime_type, data }) => {
            assert_eq!(mime_type.as_deref(), Some("image/png"));
            assert_eq!(data, "QUJD");
        }
        other => panic!("a data URL is bytes: {other:?}"),
    }

    // Long and base64-shaped: bytes, with no declared type to trust.
    let bare = "A".repeat(65);
    match codex_image_source(&json!({"id": "i", "type": "imageGeneration", "result": bare})) {
        Some(AgentImageSource::Base64 { mime_type, .. }) => assert!(mime_type.is_none()),
        other => panic!("bare base64 is bytes: {other:?}"),
    }

    // Too short to be base64: a URL the app can fetch.
    match codex_image_source(
        &json!({"id": "i", "type": "imageGeneration", "result": "https://example.invalid/a.png"}),
    ) {
        Some(AgentImageSource::Url(url)) => assert_eq!(url, "https://example.invalid/a.png"),
        other => panic!("a URL stays a URL: {other:?}"),
    }
}

#[test]
fn an_object_result_carries_path_url_or_data() {
    match codex_image_source(
        &json!({"id": "i", "type": "imageView", "result": {"path": "/tmp/a.png"}}),
    ) {
        Some(AgentImageSource::Path(path)) => assert_eq!(path, PathBuf::from("/tmp/a.png")),
        other => panic!("an object path is a path: {other:?}"),
    }
    match codex_image_source(
        &json!({"id": "i", "type": "imageView", "result": {"data": "QUJD", "mimeType": "image/png"}}),
    ) {
        Some(AgentImageSource::Base64 { mime_type, .. }) => {
            assert_eq!(mime_type.as_deref(), Some("image/png"))
        }
        other => panic!("an object data is bytes: {other:?}"),
    }
}

#[test]
fn a_content_block_is_mcp_or_claude_shaped() {
    match image_block_source(&json!({"type": "image", "data": "QUJD", "mimeType": "image/png"})) {
        Some(AgentImageSource::Base64 { mime_type, .. }) => {
            assert_eq!(mime_type.as_deref(), Some("image/png"))
        }
        other => panic!("the MCP block is bytes: {other:?}"),
    }
    match image_block_source(&json!({
        "type": "image",
        "source": {"type": "base64", "media_type": "image/jpeg", "data": "QUJD"},
    })) {
        Some(AgentImageSource::Base64 { mime_type, data }) => {
            assert_eq!(mime_type.as_deref(), Some("image/jpeg"));
            assert_eq!(data, "QUJD");
        }
        other => panic!("the Claude block is bytes: {other:?}"),
    }
    assert!(image_block_source(&json!({"type": "text", "text": "no"})).is_none());
}

#[test]
fn a_stored_image_answers_a_reference_the_store_resolves() {
    let workspace = workspace("store");
    let sink = sink("store", &workspace);
    let reference = sink
        .store(&AgentImageSource::Path(workspace.join("tiny.png")))
        .expect("the workspace image is stored");
    assert_eq!(reference.session_id, "s.agent.image.store");
    assert!(reference.stored_bytes > 0);

    let (path, size) = sink
        .store
        .resolve(&reference.session_id, &reference.digest, None)
        .expect("the stored reference resolves");
    assert_eq!(size, reference.stored_bytes);
    assert!(path.is_file());
}

#[test]
fn refusals_are_none_not_errors() {
    let workspace = workspace("refusals");
    let sink = sink("refusals", &workspace);

    // Outside the workspace and the temp dir: the helper's own parent is the
    // system temp root, and its parent is outside it.
    let temp_root = crate::test_dirs::test_temp_dir("devboule-agent-image-root")
        .parent()
        .expect("the temp root is the helper's parent")
        .to_path_buf();
    let outside = temp_root
        .parent()
        .expect("the temp root has a parent")
        .join("devboule-agent-image-outside.png");
    std::fs::write(&outside, tiny_png_bytes()).expect("the outside fixture lands");
    assert!(sink
        .store(&AgentImageSource::Path(outside.clone()))
        .is_none());
    let _ = std::fs::remove_file(outside);

    // The label disagrees with the bytes.
    assert!(sink
        .store(&AgentImageSource::Base64 {
            mime_type: Some("image/jpeg".to_string()),
            data: TINY_PNG.to_string(),
        })
        .is_none());

    // Bytes that are no image at all.
    assert!(sink
        .store(&AgentImageSource::Base64 {
            mime_type: None,
            data: base64::engine::general_purpose::STANDARD.encode(b"not an image"),
        })
        .is_none());

    // A payload over the ceiling, refused before it is decoded.
    let oversized =
        base64::engine::general_purpose::STANDARD.encode(vec![0u8; MAX_AGENT_IMAGE_BYTES + 1]);
    assert!(sink
        .store(&AgentImageSource::Base64 {
            mime_type: Some("image/png".to_string()),
            data: oversized,
        })
        .is_none());

    // The store's own wire refusals still stand: a session id it cannot name.
    let wrong = AgentImageSink::new(
        crate::attachment_store::AttachmentStore::new(&workspace),
        "../escape".to_string(),
        workspace.clone(),
    );
    assert!(wrong
        .store(&AgentImageSource::Path(workspace.join("tiny.png")))
        .is_none());
}
