//! GIF and WebP on the Claude plan: stored or fresh, they ride as image blocks
//! carrying the stripped bytes, exactly as a PNG does, and the caps that send a
//! PNG to a path line send them there too.

use super::super::{carried_mime_types, frame_user_message_with_images, plan_claude_prompt};
use super::{plan_attachment, reference_for, PlanTempDir};
use crate::attachment_store::AttachmentStore;
use crate::raster_metadata::clean_png;
use crate::raster_metadata::container_fixtures::{
    bulky_gif, bulky_webp, gif_comment, gif_frame, gif_from, gif_graphic_control, gif_xmp,
    webp_chunk, webp_extended_header, webp_from, webp_lossless,
};
use crate::session::session_prompt_planning::resolve_attachment_references;
use base64::Engine;
use serde_json::Value;

const EXIF: u8 = 0x08;
const SVG: &[u8] = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>";

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

fn base64_of(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

/// Stores each item in the session and answers the paths a reference resolves to.
fn store_references(
    store: &AttachmentStore,
    session_id: &str,
    items: &[(&str, &str, &[u8])],
) -> Vec<crate::session::ResolvedReference> {
    let references: Vec<_> = items
        .iter()
        .map(|(name, mime_type, bytes)| {
            let stored = store
                .session(session_id)
                .expect("session")
                .materialize(&plan_attachment(name, mime_type, bytes))
                .expect("stored");
            reference_for(session_id, &stored)
        })
        .collect();
    resolve_attachment_references(store, session_id, &references).expect("resolved")
}

fn frame_content(plan: &super::super::ClaudePromptPlan) -> Vec<Value> {
    let bytes = frame_user_message_with_images(&plan.fallback_text, &plan.images).expect("frame");
    let value: Value =
        serde_json::from_str(std::str::from_utf8(&bytes).expect("utf8").trim_end()).expect("json");
    value["message"]["content"]
        .as_array()
        .expect("content array")
        .clone()
}

/// The deposit strips on the way in, so the block carries what the store kept,
/// labelled with the container the bytes proved, and names no local path.
#[test]
fn a_stored_gif_reference_plans_a_block_with_the_stripped_bytes() {
    let temp = PlanTempDir::new("ref-gif");
    let store = AttachmentStore::new(&temp.0);
    let session_id = "claude-ref-gif";
    let (sent, kept) = gif_with_metadata();
    assert_ne!(sent, kept, "the fixture must carry something that leaves");
    let paths = store_references(&store, session_id, &[("loop.gif", "image/gif", &sent[..])]);

    let plan = plan_claude_prompt(&store, session_id, "look at this", &[], &paths)
        .expect("planned")
        .expect("a stored gif plans a frame");

    assert_eq!(carried_mime_types(Some(&plan)), vec!["image/gif"]);
    assert_eq!(plan.fallback_text, "look at this", "no path line");
    assert_eq!(plan.images[0].data_base64, base64_of(&kept));
    let content = frame_content(&plan);
    assert_eq!(content.len(), 2);
    assert_eq!(content[1]["source"]["media_type"], "image/gif");
    assert_eq!(content[1]["type"], "image");
}

#[test]
fn a_stored_webp_reference_plans_a_block_with_the_stripped_bytes() {
    let temp = PlanTempDir::new("ref-webp");
    let store = AttachmentStore::new(&temp.0);
    let session_id = "claude-ref-webp";
    let (sent, kept) = webp_with_metadata();
    assert_ne!(sent, kept, "the fixture must carry something that leaves");
    let paths = store_references(
        &store,
        session_id,
        &[("photo.webp", "image/webp", &sent[..])],
    );

    let plan = plan_claude_prompt(&store, session_id, "look at this", &[], &paths)
        .expect("planned")
        .expect("a stored webp plans a frame");

    assert_eq!(carried_mime_types(Some(&plan)), vec!["image/webp"]);
    assert_eq!(plan.fallback_text, "look at this", "no path line");
    assert_eq!(plan.images[0].data_base64, base64_of(&kept));
    assert_eq!(
        frame_content(&plan)[1]["source"]["media_type"],
        "image/webp"
    );
}

/// A fresh attachment takes the same road: the label was checked against the
/// bytes at materialize, and the block carries the stripped file.
#[test]
fn fresh_gif_and_webp_attachments_become_blocks_with_the_stripped_bytes() {
    let temp = PlanTempDir::new("fresh");
    let store = AttachmentStore::new(&temp.0);
    let session_id = "claude-fresh";
    let (gif, kept_gif) = gif_with_metadata();
    let (webp, kept_webp) = webp_with_metadata();
    let attachments = [
        plan_attachment("loop.gif", "image/gif", &gif),
        plan_attachment("photo.webp", "image/webp", &webp),
    ];

    let plan = plan_claude_prompt(&store, session_id, "two pictures", &attachments, &[])
        .expect("planned")
        .expect("fresh images plan a frame");

    assert_eq!(
        carried_mime_types(Some(&plan)),
        vec!["image/gif", "image/webp"]
    );
    assert_eq!(plan.fallback_text, "two pictures", "no path line");
    assert_eq!(plan.images[0].data_base64, base64_of(&kept_gif));
    assert_eq!(plan.images[1].data_base64, base64_of(&kept_webp));
}

/// A PNG, a GIF and an SVG: two blocks in reference order, one path line.
#[test]
fn a_png_gif_and_svg_mix_gives_two_blocks_and_one_path_line() {
    let temp = PlanTempDir::new("ref-mix");
    let store = AttachmentStore::new(&temp.0);
    let session_id = "claude-ref-mix";
    let (gif, _) = gif_with_metadata();
    let png = clean_png(0x41);
    let paths = store_references(
        &store,
        session_id,
        &[
            ("photo.png", "image/png", &png[..]),
            ("loop.gif", "image/gif", &gif[..]),
            ("drawing.svg", "image/svg+xml", SVG),
        ],
    );

    let plan = plan_claude_prompt(&store, session_id, "three files", &[], &paths)
        .expect("planned")
        .expect("the rasters plan a frame");

    assert_eq!(
        carried_mime_types(Some(&plan)),
        vec!["image/png", "image/gif"]
    );
    assert_eq!(
        plan.fallback_text,
        format!(
            "three files\n\n[Image available at: {}]",
            paths[2].path.display()
        ),
        "only the SVG keeps a line"
    );
    let content = frame_content(&plan);
    assert_eq!(content.len(), 3, "one text block and two image blocks");
    assert_eq!(content[1]["source"]["media_type"], "image/png");
    assert_eq!(content[2]["source"]["media_type"], "image/gif");
}

/// The per-image cap binds a GIF and a WebP as it binds a PNG: the block is
/// never built and the path line names the file.
#[test]
fn a_gif_or_webp_over_the_per_image_cap_keeps_its_path_line() {
    let temp = PlanTempDir::new("ref-over-cap");
    let store = AttachmentStore::new(&temp.0);
    let session_id = "claude-ref-over-cap";
    let paths = store_references(
        &store,
        session_id,
        &[
            ("huge.gif", "image/gif", &bulky_gif(9, 200 * 1024)[..]),
            ("huge.webp", "image/webp", &bulky_webp(9, 200 * 1024)[..]),
        ],
    );

    let plan = plan_claude_prompt(&store, session_id, "two huge files", &[], &paths)
        .expect("planned")
        .expect("references plan a frame");

    assert!(
        carried_mime_types(Some(&plan)).is_empty(),
        "an over-cap file does not travel"
    );
    assert_eq!(
        plan.fallback_text,
        format!(
            "two huge files\n\n[Image available at: {}]\n[Image available at: {}]",
            paths[0].path.display(),
            paths[1].path.display()
        ),
        "each over-cap reference keeps its path line"
    );
}

/// A GIF whose bytes are swapped after resolution is judged on what is read, not
/// on the name, exactly as a PNG is.
#[test]
fn a_gif_named_file_with_other_bytes_is_not_inlined() {
    let temp = PlanTempDir::new("ref-gif-mislabeled");
    let store = AttachmentStore::new(&temp.0);
    let session_id = "claude-ref-gif-mislabeled";
    let (gif, _) = gif_with_metadata();
    let stored = store
        .session(session_id)
        .expect("session")
        .materialize(&plan_attachment("loop.gif", "image/gif", &gif))
        .expect("stored");
    let digest = crate::attachment_store::sha256_hex(SVG);
    let mislabeled = stored
        .parent()
        .expect("session folder")
        .join(format!("{digest}.gif"));
    std::fs::write(&mislabeled, SVG).expect("write the mislabeled file");
    let reference = devboule_protocol::AttachmentReference {
        session_id: session_id.to_string(),
        digest,
        stored_bytes: SVG.len() as u64,
        name: String::new(),
    };
    let paths = resolve_attachment_references(&store, session_id, std::slice::from_ref(&reference))
        .expect("resolved");

    let plan = plan_claude_prompt(&store, session_id, "look at this", &[], &paths)
        .expect("planned")
        .expect("a reference still plans a frame");

    assert!(carried_mime_types(Some(&plan)).is_empty());
    assert_eq!(
        plan.fallback_text,
        format!(
            "look at this\n\n[Image available at: {}]",
            mislabeled.display()
        )
    );
}
