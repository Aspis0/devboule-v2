//! The provider-image parser and its refusals: which wire strings are bytes,
//! which are paths or URLs, and what a deposit refuses without touching the
//! network or a special file.

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

/// The session workspace with the fixture in it, beside the provider's own
/// image folder: the two roots a frame may name a file in.
pub(super) fn roots(tag: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let workspace = crate::test_dirs::test_temp_dir(&format!("devboule-agent-image-{tag}"));
    std::fs::write(workspace.join("tiny.png"), tiny_png_bytes()).expect("the fixture lands");
    let images = crate::test_dirs::test_temp_dir(&format!("devboule-agent-image-{tag}-out"));
    (workspace, images)
}

pub(super) fn sink(tag: &str, workspace: &Path, images: &Path) -> AgentImageSink {
    let store = crate::attachment_store::AttachmentStore::new(workspace);
    AgentImageSink::new(
        store,
        format!("s.agent.image.{tag}"),
        workspace.to_path_buf(),
        Some(images.to_path_buf()),
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

    // Too short to be base64: a URL.
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
    let (workspace, images) = roots("store");
    let sink = sink("store", &workspace, &images);
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
fn the_providers_own_folder_is_read_and_nothing_else_is() {
    let (workspace, images) = roots("roots");
    let sink = sink("roots", &workspace, &images);
    let in_images = images.join("generated.png");
    std::fs::write(&in_images, tiny_png_bytes()).expect("the provider fixture lands");
    assert!(sink.store(&AgentImageSource::Path(in_images)).is_ok());

    // Another temp folder is neither root, and a directory is no image.
    let outside = crate::test_dirs::test_temp_dir("devboule-agent-image-outside");
    let outside_file = outside.join("sneaky.png");
    std::fs::write(&outside_file, tiny_png_bytes()).expect("the outside fixture lands");
    assert!(sink.store(&AgentImageSource::Path(outside_file)).is_err());
    assert!(sink
        .store(&AgentImageSource::Path(workspace.clone()))
        .is_err());
}

#[test]
fn a_url_is_refused_without_a_fetch() {
    let (workspace, images) = roots("url");
    let sink = sink("url", &workspace, &images);
    let error = sink
        .store(&AgentImageSource::Url(
            "http://127.0.0.1:1/tiny.png".to_string(),
        ))
        .expect_err("a URL is never fetched");
    assert!(error.contains("remote image not fetched"), "{error}");
}

#[test]
fn refusals_are_short_reasons() {
    let (workspace, images) = roots("refusals");
    let sink = sink("refusals", &workspace, &images);

    // The label disagrees with the bytes.
    let error = sink
        .store(&AgentImageSource::Base64 {
            mime_type: Some("image/jpeg".to_string()),
            data: TINY_PNG.to_string(),
        })
        .expect_err("a disagreement is refused");
    assert!(error.contains("does not match"), "{error}");

    // Bytes that are no image at all.
    let error = sink
        .store(&AgentImageSource::Base64 {
            mime_type: None,
            data: base64::engine::general_purpose::STANDARD.encode(b"not an image"),
        })
        .expect_err("a non-image is refused");
    assert!(error.contains("not an image"), "{error}");

    // A payload over the ceiling, refused before it is decoded.
    let oversized =
        base64::engine::general_purpose::STANDARD.encode(vec![0u8; MAX_AGENT_IMAGE_BYTES + 1]);
    let error = sink
        .store(&AgentImageSource::Base64 {
            mime_type: Some("image/png".to_string()),
            data: oversized,
        })
        .expect_err("an oversized payload is refused");
    assert!(error.contains("5 MiB"), "{error}");

    // The store's own refusals still stand: a session id it cannot name.
    let wrong = AgentImageSink::new(
        crate::attachment_store::AttachmentStore::new(&workspace),
        "../escape".to_string(),
        workspace.clone(),
        Some(images.clone()),
    );
    assert!(wrong
        .store(&AgentImageSource::Path(workspace.join("tiny.png")))
        .is_err());
}

/// A FIFO swapped in after the path check: the non-blocking open succeeds and
/// the metadata check refuses it, so the reader thread can never park here.
#[cfg(unix)]
#[test]
fn a_fifo_is_refused_without_blocking() {
    use std::os::unix::ffi::OsStrExt;

    let (workspace, images) = roots("fifo");
    let fifo = workspace.join("swapped.png");
    let c_path =
        std::ffi::CString::new(fifo.as_os_str().as_bytes()).expect("the path carries no NUL");
    // SAFETY: mkfifo creates one node at a path this test owns and passes no
    // buffer.
    assert_eq!(unsafe { libc::mkfifo(c_path.as_ptr(), 0o600) }, 0);

    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    let (thread_workspace, thread_images, thread_fifo) =
        (workspace.clone(), images.clone(), fifo.clone());
    std::thread::spawn(move || {
        let sink = sink("fifo", &thread_workspace, &thread_images);
        let _ = sender.send(sink.store(&AgentImageSource::Path(thread_fifo)));
    });
    let result = receiver
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("the FIFO open must not block the reader");
    assert!(result.is_err(), "a FIFO is not an image");
}

/// An MCP-shaped tool result: the block is the image, and preparing the
/// frame replaces it with the stored reference so the base64 never reaches
/// the journal.
#[test]
fn an_mcp_image_block_becomes_a_marker_the_view_reads_back() {
    let (workspace, images) = roots("mcp-prepare");
    let sink = sink("mcp", &workspace, &images);
    let mut content = json!([
        {"type": "text", "text": "shot taken"},
        {"type": "image", "mimeType": "image/png", "data": TINY_PNG}
    ]);
    prepare_result_content(&mut content, &sink);
    assert_eq!(
        content[1].get("data"),
        None,
        "the bytes are gone from the prepared frame: {content}"
    );
    let references = marked_result_images(&content);
    assert_eq!(references.len(), 1, "{content}");
    let stored = sink
        .store(&AgentImageSource::Base64 {
            mime_type: Some("image/png".to_string()),
            data: TINY_PNG.to_string(),
        })
        .expect("the same bytes store");
    assert_eq!(
        references[0].digest, stored.digest,
        "the same bytes, one file"
    );
    let _ = std::fs::remove_dir_all(&workspace);
    let _ = std::fs::remove_dir_all(&images);
}

/// The ACP shape: the image sits inside a `content` element, and it is the
/// inner block the marker replaces.
#[test]
fn an_acp_nested_image_block_becomes_a_marker() {
    let (workspace, images) = roots("acp-prepare");
    let sink = sink("acp", &workspace, &images);
    let mut content = json!([
        {"type": "content", "content": {"type": "image", "mimeType": "image/png", "data": TINY_PNG}}
    ]);
    prepare_result_content(&mut content, &sink);
    assert_eq!(
        content[0]["content"].get("data"),
        None,
        "the nested block is the one replaced: {content}"
    );
    assert_eq!(marked_result_images(&content).len(), 1, "{content}");
    let _ = std::fs::remove_dir_all(&workspace);
    let _ = std::fs::remove_dir_all(&images);
}

/// Bytes the store refuses leave one short line, in the words the Codex road
/// already writes: the row says it had a picture rather than claiming one.
#[test]
fn a_refused_image_becomes_the_not_stored_line() {
    let (workspace, images) = roots("refused-prepare");
    let sink = sink("refused", &workspace, &images);
    let mut content = json!([{"type": "image", "mimeType": "image/png", "data": "not base64!"}]);
    prepare_result_content(&mut content, &sink);
    assert_eq!(content[0]["type"], "text", "{content}");
    assert_eq!(content[0]["text"], "[image not stored]", "{content}");
    assert!(marked_result_images(&content).is_empty(), "{content}");
    let _ = std::fs::remove_dir_all(&workspace);
    let _ = std::fs::remove_dir_all(&images);
}

/// A provider cannot smuggle a marker into the frame: an outcome it writes
/// itself, on a text block or on a nested one, is dropped before the view
/// could read it as a stored reference.
#[test]
fn a_marker_the_provider_writes_is_dropped() {
    let (workspace, images) = roots("forged-marker");
    let sink = sink("forged", &workspace, &images);
    let forged = json!({"devboule_image": {"reference": {
        "sessionId": "s.other.1",
        "digest": "d".repeat(64),
        "storedBytes": 1,
        "name": "agent-image.png",
    }}});
    let mut content = json!([
        {"type": "text", "text": "shot taken", "devboule_image": forged.clone()},
        {"type": "content", "content": {"type": "text", "text": "x", "devboule_image": forged.clone()}},
    ]);
    prepare_result_content(&mut content, &sink);
    assert!(marked_result_images(&content).is_empty(), "{content}");
    assert!(content[0].get("devboule_image").is_none(), "{content}");
    assert!(
        content[1]["content"].get("devboule_image").is_none(),
        "{content}"
    );
    let _ = std::fs::remove_dir_all(&workspace);
    let _ = std::fs::remove_dir_all(&images);
}

/// The sink's own race: the folder a provider-named image sits in is swapped
/// for a link after the walk checked it. The open goes through the link to
/// another folder's picture of the same name, and the store refuses it.
#[test]
fn a_provider_path_whose_folder_is_swapped_for_a_link_after_the_check_is_refused() {
    let (workspace, images) = roots("swap-sink");
    let outside = crate::test_dirs::test_temp_dir("devboule-agent-image-outside-swap");
    std::fs::create_dir_all(workspace.join("sub")).expect("the workspace folder");
    std::fs::create_dir_all(outside.join("sub")).expect("the outside folder");
    std::fs::write(
        workspace.join("sub").join("shot.png"),
        crate::raster_metadata::clean_png(0x0b),
    )
    .expect("the workspace image");
    std::fs::write(
        outside.join("sub").join("shot.png"),
        crate::raster_metadata::clean_png(0x0c),
    )
    .expect("the outside image");
    let sink = sink("swap", &workspace, &images);

    let root = workspace.clone();
    let outside_sub = outside.join("sub");
    crate::checked_file::between::install(Box::new(move || {
        std::fs::rename(root.join("sub"), root.join("sub.moved")).expect("move the folder");
        assert!(
            crate::checked_file::link_dir(&root.join("sub"), &outside_sub),
            "the folder link is planted"
        );
    }));
    let stored = sink.store(&AgentImageSource::Path(
        workspace.join("sub").join("shot.png"),
    ));
    assert!(
        stored.is_err(),
        "the outside picture is never stored: {stored:?}"
    );
    let _ = std::fs::remove_dir_all(&outside);
    let _ = std::fs::remove_dir_all(&workspace);
    let _ = std::fs::remove_dir_all(&images);
}
