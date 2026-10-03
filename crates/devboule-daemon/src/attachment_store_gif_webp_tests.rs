//! The store's trust boundary for GIF and WebP: the strip runs on what the bytes
//! are, the label has to agree, and the stored file resolves by digest.

use super::tests::{attachment, encoded, TempDir};
use super::*;
use crate::raster_metadata::container_fixtures::{
    clean_gif, clean_webp, gif_comment, gif_frame, gif_from, gif_graphic_control, gif_xmp,
    webp_chunk, webp_extended_header, webp_from, webp_lossless,
};

const EXIF: u8 = 0x08;

fn gif_with_metadata() -> (Vec<u8>, Vec<u8>) {
    let sent = gif_from(&[
        gif_graphic_control(),
        gif_comment(b"shot on a phone"),
        gif_xmp(),
        gif_frame(1),
    ]);
    let kept = gif_from(&[gif_graphic_control(), gif_frame(1)]);
    (sent, kept)
}

fn webp_with_metadata() -> (Vec<u8>, Vec<u8>) {
    let sent = webp_from(&[
        webp_extended_header(EXIF),
        webp_lossless(1),
        webp_chunk(b"EXIF", b"Exif\0\0gps"),
    ]);
    let kept = webp_from(&[webp_extended_header(0), webp_lossless(1)]);
    (sent, kept)
}

fn assert_stored_as(path: &Path, kept: &[u8], extension: &str) {
    assert_eq!(
        path.file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string),
        Some(format!("{}.{extension}", sha256_hex(kept)))
    );
    assert_eq!(std::fs::read(path).expect("read"), kept);
}

#[test]
fn a_gif_is_stripped_before_it_is_written() {
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let session = store.session("s.a.1").expect("session");
    let (sent, kept) = gif_with_metadata();
    assert_ne!(sent, kept, "the fixture must carry something that leaves");

    let path = session
        .materialize(&attachment("loop.gif", "image/gif", &encoded(&sent)))
        .expect("materialized");

    assert_stored_as(&path, &kept, "gif");
}

#[test]
fn a_webp_is_stripped_before_it_is_written() {
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let session = store.session("s.a.1").expect("session");
    let (sent, kept) = webp_with_metadata();
    assert_ne!(sent, kept, "the fixture must carry something that leaves");

    let path = session
        .materialize(&attachment("photo.webp", "image/webp", &encoded(&sent)))
        .expect("materialized");

    assert_stored_as(&path, &kept, "webp");
}

#[test]
fn a_clean_gif_and_a_clean_webp_are_written_as_they_arrived() {
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let session = store.session("s.a.1").expect("session");

    let gif = session
        .materialize(&attachment("a.gif", "image/gif", &encoded(&clean_gif(3))))
        .expect("gif");
    let webp = session
        .materialize(&attachment(
            "a.webp",
            "image/webp",
            &encoded(&clean_webp(3)),
        ))
        .expect("webp");

    assert_stored_as(&gif, &clean_gif(3), "gif");
    assert_stored_as(&webp, &clean_webp(3), "webp");
}

#[test]
fn a_declared_type_the_bytes_contradict_is_refused() {
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let session = store.session("s.a.1").expect("session");
    let (gif, _) = gif_with_metadata();
    let (webp, _) = webp_with_metadata();
    let cases = [
        ("image/webp", gif.clone()),
        ("image/gif", webp.clone()),
        ("image/png", gif.clone()),
        ("image/jpeg", webp.clone()),
        // The bypass shape: a raster declared as a type that skips the walk.
        ("image/svg+xml", gif),
        ("text/markdown", webp),
        ("image/gif", b"not a gif at all".to_vec()),
        ("image/webp", b"not a webp at all".to_vec()),
    ];

    for (declared, bytes) in cases {
        let item = attachment("x", declared, &encoded(&bytes));
        let error = session.materialize(&item).expect_err("refused");
        assert_eq!(error.code, ErrorCode::InvalidRequest, "{declared}");
        assert!(
            error.message.contains("do not match the type"),
            "{declared}: {}",
            error.message
        );
    }
    assert!(!session.dir.exists(), "nothing may be created on a refusal");
}

#[test]
fn a_gif_or_webp_the_walk_cannot_follow_is_refused_rather_than_written() {
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let session = store.session("s.a.1").expect("session");
    let mut gif = clean_gif(1);
    gif.pop();
    let mut webp = clean_webp(1);
    webp.pop();

    for (declared, bytes) in [("image/gif", gif), ("image/webp", webp)] {
        let error = session
            .materialize(&attachment("x", declared, &encoded(&bytes)))
            .expect_err("refused");
        assert_eq!(error.code, ErrorCode::InvalidRequest, "{declared}");
        assert!(
            error.message.contains("could not be read"),
            "{declared}: {}",
            error.message
        );
    }
    assert!(!session.dir.exists(), "nothing may be created on a refusal");
}

#[test]
fn a_deposited_gif_and_webp_resolve_by_digest_with_and_without_a_hint() {
    // `find_stored` reads `STORED_EXTENSIONS` when it starts from the disk, so a
    // type missing from that table deposits fine and can never be found again.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let (gif, kept_gif) = gif_with_metadata();
    let (webp, kept_webp) = webp_with_metadata();

    for (name, declared, sent, kept, extension) in [
        ("a.gif", "image/gif", gif, kept_gif, "gif"),
        ("a.webp", "image/webp", webp, kept_webp, "webp"),
    ] {
        let stored = store
            .deposit("s.a.1", &attachment(name, declared, &encoded(&sent)))
            .expect("deposits");
        assert_eq!(stored.stored_bytes, kept.len() as u64, "{declared}");
        for hint in [None, Some(declared), Some(extension)] {
            let (path, bytes) = store
                .resolve("s.a.1", &stored.digest, hint)
                .unwrap_or_else(|error| panic!("{declared} hint {hint:?}: {error:?}"));
            assert_eq!(path, stored.path);
            assert_eq!(bytes, stored.stored_bytes);
        }
        assert_eq!(
            mime_type_for_extension(extension),
            Some(declared),
            "the read answer states the stored type"
        );
    }
}
