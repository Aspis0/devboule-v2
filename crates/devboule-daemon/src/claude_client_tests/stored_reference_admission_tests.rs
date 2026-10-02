//! What admits a stored reference as a Claude image block: the bytes must
//! prove themselves (digest, signature) and fit the caps — verification and
//! budget, the two refusals that both end in a path line.

use super::super::{
    carried_mime_types, plan_claude_prompt, read_inline_candidate, MAX_INLINE_IMAGE_BYTES,
};
use super::{plan_attachment, reference_for, PlanTempDir};
use crate::attachment_store::AttachmentStore;
use crate::raster_metadata::{bulky_png, clean_png, vector_input};
use crate::session::session_prompt_planning::resolve_attachment_references;
use devboule_protocol::{
    AttachmentReference, MAX_ATTACHMENTS_TOTAL_BYTES, MAX_ATTACHMENT_COUNT,
    MAX_ATTACHMENT_DATA_BYTES,
};

/// The bytes are checked at read time, not trusted from the path: a file
/// swapped for same-size bytes between resolve and read keeps its path line
/// instead of travelling under the digest the deposit wrote.
#[test]
fn a_same_size_replaced_reference_is_not_inlined() {
    let temp = PlanTempDir::new("ref-swap");
    let store = AttachmentStore::new(&temp.0);
    let session_id = "claude-ref-swap";
    let stored = store
        .session(session_id)
        .expect("session")
        .materialize(&plan_attachment("photo.png", "image/png", &clean_png(0x0a)))
        .expect("stored");
    let reference = reference_for(session_id, &stored);
    let paths = resolve_attachment_references(&store, session_id, std::slice::from_ref(&reference))
        .expect("resolved");
    let swapped = clean_png(0x0b);
    assert_eq!(
        swapped.len() as u64,
        reference.stored_bytes,
        "the fixture must be the same size or resolution would refuse it"
    );
    std::fs::write(&stored, &swapped).expect("replace the stored file");
    let plan = plan_claude_prompt(&store, session_id, "look at this", &[], &paths)
        .expect("planned")
        .expect("a reference still plans a frame");
    assert!(
        carried_mime_types(Some(&plan)).is_empty(),
        "the replaced bytes do not travel: {:?}",
        carried_mime_types(Some(&plan))
    );
    assert_eq!(
        plan.fallback_text,
        format!(
            "look at this\n\n[Image available at: {}]",
            paths[0].display()
        ),
        "the replaced file keeps its path line"
    );
}

/// A file swapped for one over the cap after resolution is not read into a
/// block: the reference keeps its path line.
#[test]
fn a_reference_replaced_by_an_over_cap_file_is_not_inlined() {
    let temp = PlanTempDir::new("ref-swap-large");
    let store = AttachmentStore::new(&temp.0);
    let session_id = "claude-ref-swap-large";
    let stored = store
        .session(session_id)
        .expect("session")
        .materialize(&plan_attachment("photo.png", "image/png", &clean_png(0x0d)))
        .expect("stored");
    let reference = reference_for(session_id, &stored);
    let paths = resolve_attachment_references(&store, session_id, std::slice::from_ref(&reference))
        .expect("resolved");
    std::fs::write(&stored, vec![0u8; MAX_INLINE_IMAGE_BYTES + 1]).expect("replace the file");
    let plan = plan_claude_prompt(&store, session_id, "look at this", &[], &paths)
        .expect("planned")
        .expect("a reference still plans a frame");
    assert!(carried_mime_types(Some(&plan)).is_empty());
    assert_eq!(
        plan.fallback_text,
        format!(
            "look at this\n\n[Image available at: {}]",
            paths[0].display()
        )
    );
}

/// The raw cap is inclusive and the read never hands back more than it:
/// the cap itself is read, one byte more is refused, and so is a non-file.
#[test]
fn the_inline_read_stops_at_the_raw_cap() {
    let temp = PlanTempDir::new("ref-raw-cap");
    let at_cap = temp.0.join("at-cap.bin");
    let over_cap = temp.0.join("over-cap.bin");
    std::fs::write(&at_cap, vec![1u8; MAX_INLINE_IMAGE_BYTES]).expect("write at cap");
    std::fs::write(&over_cap, vec![1u8; MAX_INLINE_IMAGE_BYTES + 1]).expect("write over cap");
    assert_eq!(
        read_inline_candidate(&at_cap)
            .expect("read")
            .map(|bytes| bytes.len()),
        Some(MAX_INLINE_IMAGE_BYTES)
    );
    assert!(read_inline_candidate(&over_cap).expect("read").is_none());
}

/// The container comes from the bytes' signature, never from the name: a
/// `.png` file whose bytes are SVG (digest-consistent, so hashing alone
/// cannot see it) keeps its path line rather than travelling as image/png.
#[test]
fn a_png_named_file_with_svg_bytes_is_not_inlined() {
    let temp = PlanTempDir::new("ref-mislabeled");
    let store = AttachmentStore::new(&temp.0);
    let session_id = "claude-ref-mislabeled";
    let png = store
        .session(session_id)
        .expect("session")
        .materialize(&plan_attachment("photo.png", "image/png", &clean_png(0x0c)))
        .expect("stored");
    let svg_bytes = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>";
    let digest = crate::attachment_store::sha256_hex(svg_bytes);
    let mislabeled = png
        .parent()
        .expect("session folder")
        .join(format!("{digest}.png"));
    std::fs::write(&mislabeled, svg_bytes).expect("write the mislabeled file");
    let reference = AttachmentReference {
        session_id: session_id.to_string(),
        digest,
        stored_bytes: svg_bytes.len() as u64,
    };
    let paths = resolve_attachment_references(&store, session_id, std::slice::from_ref(&reference))
        .expect("resolved");
    assert_eq!(paths[0], mislabeled);
    let plan = plan_claude_prompt(&store, session_id, "look at this", &[], &paths)
        .expect("planned")
        .expect("a reference still plans a frame");
    assert!(
        carried_mime_types(Some(&plan)).is_empty(),
        "svg bytes under a .png name do not travel as image/png: {:?}",
        carried_mime_types(Some(&plan))
    );
    assert_eq!(
        plan.fallback_text,
        format!(
            "look at this\n\n[Image available at: {}]",
            mislabeled.display()
        ),
        "the mislabeled file keeps its path line"
    );
}

/// The per-image cap binds a reference whose file alone is over it: the
/// block is never built, and the path line names the file truthfully.
#[test]
fn a_reference_over_the_per_image_cap_keeps_its_path_line() {
    let temp = PlanTempDir::new("ref-per-image");
    let store = AttachmentStore::new(&temp.0);
    let session_id = "claude-ref-per-image";
    let stored = store
        .session(session_id)
        .expect("session")
        .materialize(&plan_attachment(
            "huge.png",
            "image/png",
            &bulky_png(9, 200 * 1024),
        ))
        .expect("stored");
    let reference = reference_for(session_id, &stored);
    let paths = resolve_attachment_references(&store, session_id, std::slice::from_ref(&reference))
        .expect("resolved");
    {
        use base64::Engine;
        let stored_bytes = std::fs::read(&paths[0]).expect("stored bytes");
        assert!(
            base64::engine::general_purpose::STANDARD
                .encode(&stored_bytes)
                .len()
                > MAX_ATTACHMENT_DATA_BYTES,
            "the fixture must really be over the per-image cap"
        );
    }
    let plan = plan_claude_prompt(&store, session_id, "one huge file", &[], &paths)
        .expect("planned")
        .expect("a reference plans a frame");
    assert!(
        carried_mime_types(Some(&plan)).is_empty(),
        "the over-cap file does not travel"
    );
    assert_eq!(
        plan.fallback_text,
        format!(
            "one huge file\n\n[Image available at: {}]",
            paths[0].display()
        ),
        "the over-cap reference keeps its path line"
    );
}

/// The aggregate is shared with every other block in the request: two of the
/// three ~100 KiB references fit inside it, the third keeps its path line.
#[test]
fn stored_references_inline_only_inside_the_shared_aggregate_cap() {
    let temp = PlanTempDir::new("ref-budget");
    let store = AttachmentStore::new(&temp.0);
    let session_id = "claude-ref-budget";
    let mut references = Vec::new();
    for seed in 0..3u8 {
        let stored = store
            .session(session_id)
            .expect("session")
            .materialize(&plan_attachment(
                &format!("page-{seed}.png"),
                "image/png",
                &bulky_png(seed, 100 * 1024),
            ))
            .expect("stored");
        references.push(reference_for(session_id, &stored));
    }
    let paths = resolve_attachment_references(&store, session_id, &references).expect("resolved");
    let plan = plan_claude_prompt(&store, session_id, "three big files", &[], &paths)
        .expect("planned")
        .expect("references plan a frame");
    assert_eq!(
        carried_mime_types(Some(&plan)),
        vec!["image/png", "image/png"],
        "the first two fit, the third steps over the aggregate"
    );
    assert_eq!(
        plan.fallback_text,
        format!(
            "three big files\n\n[Image available at: {}]",
            paths[2].display()
        ),
        "the reference over the budget keeps its path line"
    );
    let inline: usize = plan
        .images
        .iter()
        .map(|image| image.data_base64.len())
        .sum();
    assert!(
        inline <= MAX_ATTACHMENTS_TOTAL_BYTES,
        "the inlined bytes stay inside the wire's aggregate: {inline}"
    );
}

/// The count cap binds references the way it binds fresh attachments: the
/// fifth small PNG keeps its path line while four ride as blocks.
#[test]
fn stored_references_stop_at_the_attachment_count_cap() {
    let temp = PlanTempDir::new("ref-count");
    let store = AttachmentStore::new(&temp.0);
    let session_id = "claude-ref-count";
    let mut references = Vec::new();
    for seed in 0..5u8 {
        let stored = store
            .session(session_id)
            .expect("session")
            .materialize(&plan_attachment(
                &format!("tile-{seed}.png"),
                "image/png",
                &clean_png(seed),
            ))
            .expect("stored");
        references.push(reference_for(session_id, &stored));
    }
    let paths = resolve_attachment_references(&store, session_id, &references).expect("resolved");
    let plan = plan_claude_prompt(&store, session_id, "five tiles", &[], &paths)
        .expect("planned")
        .expect("references plan a frame");
    assert_eq!(
        carried_mime_types(Some(&plan)),
        vec!["image/png"; MAX_ATTACHMENT_COUNT],
        "the fifth is over the count cap"
    );
    assert_eq!(
        plan.fallback_text,
        format!("five tiles\n\n[Image available at: {}]", paths[4].display()),
        "the reference past the count cap keeps its path line"
    );
}

/// One budget, both kinds of input: a fresh image and the references are
/// charged together, so the second reference steps over the aggregate and
/// keeps its path line while the fresh block and the first reference ride.
#[test]
fn the_aggregate_budget_is_shared_by_a_fresh_image_and_references() {
    let temp = PlanTempDir::new("ref-shared");
    let store = AttachmentStore::new(&temp.0);
    let session_id = "claude-ref-shared";
    let mut references = Vec::new();
    for seed in 1..3u8 {
        let stored = store
            .session(session_id)
            .expect("session")
            .materialize(&plan_attachment(
                &format!("page-{seed}.png"),
                "image/png",
                // Each file's base64 is exactly the per-image cap, so the
                // fresh image tips the pair over the aggregate on the second.
                &bulky_png(seed, 147_399),
            ))
            .expect("stored");
        references.push(reference_for(session_id, &stored));
    }
    let paths = resolve_attachment_references(&store, session_id, &references).expect("resolved");
    let fresh = plan_attachment(
        "photo.jpg",
        "image/jpeg",
        &vector_input("a jpeg whose APP1 holds EXIF, between a kept JFIF APP0 and a kept ICC APP2"),
    );
    let plan = plan_claude_prompt(&store, session_id, "fresh and stored", &[fresh], &paths)
        .expect("planned")
        .expect("the fresh image plans a frame");
    assert_eq!(
        carried_mime_types(Some(&plan)),
        vec!["image/jpeg", "image/png"],
        "the fresh block first, then the reference that still fits"
    );
    assert_eq!(
        plan.fallback_text,
        format!(
            "fresh and stored\n\n[Image available at: {}]",
            paths[1].display()
        ),
        "the second reference steps over the shared aggregate"
    );
}
