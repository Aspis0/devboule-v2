//! The attachment, deposit and structured-prompt tests, moved whole out of
//! `session_tests.rs` lines 2862-3745 (at `1c6b03e`): a deposit answered with
//! the reference of the file the store wrote, the unauthorised, oversized and
//! close-inside-a-deposit refusals, the count, per-file, total and text-cap
//! limits, the path line a fallback session writes and the one a terminal
//! session never writes, the structured prompt an inline image plans beside the
//! path line a refused or unknown session keeps, and the block's mime type.
//! Every line below is byte-identical to its text there apart from this header;
//! `agent_ready_for_attachment`, `attachment`, `attachment_folder`,
//! `attachment_message`, `files_under`, `insert_live` and
//! `insert_live_with_writer` are promoted to `pub(super)` for this move, the
//! other fixtures come from the provider's own imports, and `clean_png` and the
//! three attachment limits are imported here rather than through it.

use super::tests::{
    agent_ready_for_attachment, attach_live_agent_for_test, attachment, attachment_folder,
    attachment_message, files_under, insert_live, insert_live_agent_with_writer,
    insert_live_with_writer, test_owner, tmp_delete_registry, RecordingWriter,
};
use super::*;
use crate::raster_metadata::clean_png;
use devboule_protocol::{
    MAX_ATTACHMENTS_TOTAL_BYTES, MAX_ATTACHMENT_COUNT, MAX_ATTACHMENT_DATA_BYTES,
};

// --- prompt deposits ---------------------------------------------------

/// A deposit by the session's owner answers the reference of the file the
/// store wrote, with the digest and the size that file really has.
#[test]
fn an_owners_deposit_answers_the_reference_of_the_file_on_disk() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-deposit-owner", "process-deposit");
    let id = compose_session_id(&owner.session_token(), "depo01").expect("id");
    insert_live(&registry, &id, owner.clone());
    let conn = ConnHandle::new(4);
    let image = clean_png(0x0b);

    let reference = registry
        .deposit(
            &id,
            &owner,
            &conn,
            &attachment("photo.png", "image/png", &image),
        )
        .expect("the owner may deposit into their own session");

    let files = files_under(&attachment_folder(&registry, &id));
    assert_eq!(files.len(), 1, "one deposit, one file");
    assert_eq!(reference.session_id, id, "the reference names the session");
    assert_eq!(
        files[0].file_stem().and_then(|value| value.to_str()),
        Some(reference.digest.as_str()),
        "the digest is the name of the file on disk"
    );
    assert_eq!(
        reference.stored_bytes,
        std::fs::metadata(&files[0])
            .expect("stat the stored file")
            .len(),
        "stored_bytes is the file's own size, not the request's"
    );
    // The store's own digest, computed here from the bytes that were sent:
    // `clean_png` carries no metadata to strip, so the two agree and the
    // assertion above is about the stored bytes rather than about a name
    // that happens to be some digest.
    assert_eq!(
        reference.digest,
        crate::attachment_store::sha256_hex(&image)
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// A deposit by another user is refused and nothing reaches the disk: the
/// refusal is the ownership one, before the store is asked, so the session's
/// folder is not created at all.
#[test]
fn a_deposit_by_another_user_is_unauthorized_and_writes_nothing() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-deposit-theirs", "process-theirs");
    let other = test_owner("S-1-5-21-deposit-other", "process-other");
    let id = compose_session_id(&owner.session_token(), "depo02").expect("id");
    insert_live(&registry, &id, owner.clone());
    let conn = ConnHandle::new(4);

    let error = registry
        .deposit(
            &id,
            &other,
            &conn,
            &attachment("photo.png", "image/png", &clean_png(0x0b)),
        )
        .expect_err("another user may not deposit into this session");
    assert_eq!(error.code, ErrorCode::Unauthorized, "{error:?}");

    let folder = attachment_folder(&registry, &id);
    assert!(
        !folder.exists(),
        "a refused deposit must not create the session's folder: {:?}",
        files_under(&folder)
    );
    assert!(
        files_under(&registry.runtime_dir().join("attachments")).is_empty(),
        "nothing under the store's root belongs to a refused deposit"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// DEP-06: the wire's limits refuse before the store is called, so a frame
/// the protocol rejects costs no decode and no file.
///
/// The discriminator has to be the size cap and not the type: the store
/// refuses an unsupported type and a bad base64 with the *same* sentences
/// the wire does (it calls `unsupported_attachment_type_message` and
/// `invalid_base64_message` too), so an `image/bmp` or a `"!!!"` attachment
/// would read identically whichever layer refused it. An `image/svg+xml`
/// past the per-file cap decodes, is not a raster, and is written as it
/// arrived — so a `deposit` that reached the store first would answer `Ok`
/// and leave a file here. The sentence and the empty folder together are
/// what make the order observable from outside.
#[test]
fn an_oversized_deposit_is_refused_by_the_wire_before_the_store_writes_anything() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-deposit-size", "process-deposit");
    let id = compose_session_id(&owner.session_token(), "depo03").expect("id");
    insert_live(&registry, &id, owner.clone());
    let conn = ConnHandle::new(4);
    // Bypassing `attachment()` on purpose, like the total-limit send test:
    // it encodes, and what the cap counts is the encoded length.
    let over = PromptAttachment {
        name: "big.svg".to_string(),
        mime_type: "image/svg+xml".to_string(),
        data: "A".repeat(MAX_ATTACHMENT_DATA_BYTES + 4),
    };

    let error = registry
        .deposit(&id, &owner, &conn, &over)
        .expect_err("an attachment over the per-file cap is refused");
    assert_eq!(error.code, ErrorCode::InvalidRequest, "{error:?}");
    assert!(
        error
            .message
            .contains(&MAX_ATTACHMENT_DATA_BYTES.to_string()),
        "{}",
        error.message
    );
    assert!(
        files_under(&attachment_folder(&registry, &id)).is_empty(),
        "a refused deposit leaves no file, which is the half the store could not answer"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// A close that lands between the ownership check and the store write must
/// not leave a folder behind. The error alone would not say so — the orphan
/// is the half only the filesystem can answer, so both halves are asserted.
#[test]
fn a_close_inside_a_deposit_is_refused_and_leaves_no_orphan_folder() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-deposit-close", "process-deposit");
    let id = compose_session_id(&owner.session_token(), "depo04").expect("id");
    insert_live(&registry, &id, owner.clone());
    let conn = ConnHandle::new(4);
    // The window is real, and it is the store write the hook lands in: the
    // ownership check has passed, nothing has been written yet, and no
    // registry lock is held, so a close can take it.
    let closing = registry.clone();
    let closing_id = id.clone();
    let closing_owner = owner.clone();
    registry.set_deposit_after_ownership_hook(Arc::new(move || {
        closing
            .close(&closing_id, &closing_owner, &None)
            .expect("the close wins the race");
    }));

    let error = registry
        .deposit(
            &id,
            &owner,
            &conn,
            &attachment("photo.png", "image/png", &clean_png(0x0b)),
        )
        .expect_err("a deposit into a session that closed under it is refused");
    assert_eq!(error.code, ErrorCode::SessionNotFound, "{error:?}");

    // The half that matters: the file written after the close is gone with
    // the session, not left for the retention sweep to find.
    let folder = attachment_folder(&registry, &id);
    assert!(
        !folder.exists(),
        "the write that lost the race must be undone: {:?}",
        files_under(&folder)
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_fallback_session_writes_an_attachment_path_line() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-attach-path", "process-attach");
    let received = Arc::new(Mutex::new(Vec::new()));
    let runtime = insert_live_agent_with_writer(
        &registry,
        "attach-path",
        owner.clone(),
        Box::new(RecordingWriter(Arc::clone(&received))),
    );
    // The sibling is `None` here — the fallback world — so this pins the
    // honest path line, not a block. The structured tests below pin the
    // block world on a session with a sink.
    let conn = attach_live_agent_for_test(&runtime, "attach-path", 41);
    // A container the daemon's walk accepts and changes nothing in, so the
    // name and the bytes asserted below are the ones the client sent.
    let image = clean_png(0x0b);

    registry
        .send_with_subscription(
            "attach-path",
            41,
            "describe this",
            &[attachment("photo.png", "image/png", &image)],
            &[],
            &owner,
            &conn,
        )
        .expect("send with one attachment");

    let files: Vec<PathBuf> = std::fs::read_dir(attachment_folder(&registry, "attach-path"))
        .expect("session folder")
        .flatten()
        .map(|entry| entry.path())
        .collect();
    assert_eq!(files.len(), 1, "one attachment, one file");
    let path = &files[0];
    assert_eq!(
        path.extension().and_then(|value| value.to_str()),
        Some("png")
    );
    let digest = crate::attachment_store::sha256_hex(&image);
    assert_eq!(
        path.file_stem().and_then(|value| value.to_str()),
        Some(digest.as_str())
    );
    assert_eq!(std::fs::read(path).expect("read"), image);

    let written = String::from_utf8(received.lock().expect("writer").clone()).expect("utf8");
    assert_eq!(
        written,
        format!("describe this\n\n[Image available at: {}]", path.display())
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_fallback_session_separates_attachment_lines_by_a_blank_line() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-attach-two", "process-attach");
    let received = Arc::new(Mutex::new(Vec::new()));
    let runtime = insert_live_agent_with_writer(
        &registry,
        "attach-two",
        owner.clone(),
        Box::new(RecordingWriter(Arc::clone(&received))),
    );
    let conn = attach_live_agent_for_test(&runtime, "attach-two", 42);

    registry
        .send_with_subscription(
            "attach-two",
            42,
            "two files",
            &[
                attachment("a.png", "image/png", &clean_png(0x0c)),
                attachment("b.svg", "image/svg+xml", b"<svg/>"),
            ],
            &[],
            &owner,
            &conn,
        )
        .expect("send with two attachments");

    let written = String::from_utf8(received.lock().expect("writer").clone()).expect("utf8");
    let lines: Vec<&str> = written.split('\n').collect();
    assert_eq!(lines[0], "two files");
    assert_eq!(lines[1], "", "the block is separated from the prompt");
    assert!(lines[2].starts_with("[Image available at: "), "{written}");
    assert!(lines[2].ends_with(".png]"), "{written}");
    assert!(lines[3].starts_with("[Image available at: "), "{written}");
    assert!(lines[3].ends_with(".svg]"), "{written}");
    assert_eq!(
        lines.len(),
        4,
        "one line per attachment, no extras: {written}"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_fallback_session_delivers_svg_as_a_file() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-attach-svg", "process-attach");
    let runtime = insert_live_agent_with_writer(
        &registry,
        "attach-svg",
        owner.clone(),
        Box::new(std::io::sink()),
    );
    let conn = attach_live_agent_for_test(&runtime, "attach-svg", 43);
    let source = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>\n";

    registry
        .send_with_subscription(
            "attach-svg",
            43,
            "logo",
            &[attachment("logo.svg", "image/svg+xml", source)],
            &[],
            &owner,
            &conn,
        )
        .expect("send with an svg");

    let files: Vec<PathBuf> = std::fs::read_dir(attachment_folder(&registry, "attach-svg"))
        .expect("session folder")
        .flatten()
        .map(|entry| entry.path())
        .collect();
    assert_eq!(files.len(), 1);
    assert_eq!(
        files[0].extension().and_then(|value| value.to_str()),
        Some("svg")
    );
    assert_eq!(std::fs::read(&files[0]).expect("read"), source);

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn too_many_attachments_are_refused_by_the_count_limit() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-attach-count", "process-attach");
    let conn = agent_ready_for_attachment(&registry, "attach-count", &owner, 44);
    let many = vec![attachment("a.png", "image/png", b"x"); MAX_ATTACHMENT_COUNT + 1];

    let error = registry
        .send_with_subscription("attach-count", 44, "hello", &many, &[], &owner, &conn)
        .expect_err("a fifth file is refused");
    assert!(
        attachment_message(&error).contains(&MAX_ATTACHMENT_COUNT.to_string()),
        "{}",
        error.message
    );
    assert!(
        !attachment_folder(&registry, "attach-count").exists(),
        "a refused request writes nothing"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_unsupported_attachment_type_is_refused() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-attach-type", "process-attach");
    let conn = agent_ready_for_attachment(&registry, "attach-type", &owner, 45);

    let error = registry
        .send_with_subscription(
            "attach-type",
            45,
            "hello",
            &[attachment("pic.bmp", "image/bmp", b"bmp")],
            &[],
            &owner,
            &conn,
        )
        .expect_err("a bmp is refused");
    assert!(
        attachment_message(&error).contains("image/bmp"),
        "{}",
        error.message
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_oversized_attachment_is_refused_by_the_per_file_limit() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-attach-size", "process-attach");
    let conn = agent_ready_for_attachment(&registry, "attach-size", &owner, 46);
    let huge = "A".repeat(MAX_ATTACHMENT_DATA_BYTES + 4);

    let error = registry
        .send_with_subscription(
            "attach-size",
            46,
            "hello",
            &[attachment("big.png", "image/png", huge.as_bytes())],
            &[],
            &owner,
            &conn,
        )
        .expect_err("oversized data is refused");
    assert!(
        attachment_message(&error).contains(&MAX_ATTACHMENT_DATA_BYTES.to_string()),
        "{}",
        error.message
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_attachment_that_is_not_base64_is_refused() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-attach-b64", "process-attach");
    let conn = agent_ready_for_attachment(&registry, "attach-b64", &owner, 47);
    let mut not_base64 = attachment("a.png", "image/png", b"fine");
    not_base64.data = "not base64!".to_string();

    let error = registry
        .send_with_subscription("attach-b64", 47, "hello", &[not_base64], &[], &owner, &conn)
        .expect_err("invalid base64 is refused");
    assert_eq!(
        attachment_message(&error),
        format!(
            "Attachment 1 ('a.png'): {}",
            devboule_protocol::invalid_base64_message()
        )
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn attachments_over_the_total_limit_are_refused() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-attach-total", "process-attach");
    let conn = agent_ready_for_attachment(&registry, "attach-total", &owner, 48);
    // Four items each just under the per-item cap, so only the total is
    // wrong. Bypassing `attachment()` on purpose: it encodes, and what the
    // limits count is the encoded length.
    let each = "A".repeat(MAX_ATTACHMENT_DATA_BYTES - 4);
    let one = PromptAttachment {
        name: "a.png".to_string(),
        mime_type: "image/png".to_string(),
        data: each.clone(),
    };
    assert!(each.len() * MAX_ATTACHMENT_COUNT > MAX_ATTACHMENTS_TOTAL_BYTES);
    let four = vec![one; MAX_ATTACHMENT_COUNT];

    let error = registry
        .send_with_subscription("attach-total", 48, "hello", &four, &[], &owner, &conn)
        .expect_err("a total over the cap is refused");
    assert!(
        attachment_message(&error).contains(&MAX_ATTACHMENTS_TOTAL_BYTES.to_string()),
        "{}",
        error.message
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_fallback_session_measures_the_text_cap_before_appending_lines() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-attach-cap", "process-attach");
    let received = Arc::new(Mutex::new(Vec::new()));
    let runtime = insert_live_agent_with_writer(
        &registry,
        "attach-cap",
        owner.clone(),
        Box::new(RecordingWriter(Arc::clone(&received))),
    );
    let conn = attach_live_agent_for_test(&runtime, "attach-cap", 49);
    let files = vec![attachment("a.png", "image/png", &clean_png(0x0d))];

    // A text exactly at the cap, plus the lines this function adds: the
    // cap governs the user's text, and the lines are not charged to it.
    let at_cap = "x".repeat(MAX_WRITE_BYTES);
    registry
        .send_with_subscription("attach-cap", 49, &at_cap, &files, &[], &owner, &conn)
        .expect("a prompt at the cap is still sent");
    let written = received.lock().expect("writer").clone();
    assert!(written.len() > MAX_WRITE_BYTES, "the lines were appended");
    assert!(written.starts_with(at_cap.as_bytes()));
    received.lock().expect("writer").clear();

    // One byte over the cap is still refused, and nothing is written or
    // materialized on the way to that refusal. The bytes are distinct from
    // the first send's: that file already exists, so the name that must not
    // exist is what a materialize-before-the-cap-check regression creates.
    let over = "x".repeat(MAX_WRITE_BYTES + 1);
    let unreached_bytes = clean_png(0x0e);
    let unreached = vec![attachment("b.png", "image/png", &unreached_bytes)];
    let error = registry
        .send_with_subscription("attach-cap", 49, &over, &unreached, &[], &owner, &conn)
        .expect_err("an oversized text is refused");
    assert_eq!(attachment_message(&error), "Session input is too large.");
    assert!(received.lock().expect("writer").is_empty());
    let refused_file = attachment_folder(&registry, "attach-cap").join(format!(
        "{}.png",
        crate::attachment_store::sha256_hex(&unreached_bytes)
    ));
    assert!(
        !refused_file.exists(),
        "a refused prompt must not materialize its attachment"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_fallback_session_journals_the_users_text_and_never_the_bytes() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-attach-journal", "process-attach");
    let conn = agent_ready_for_attachment(&registry, "attach-journal", &owner, 50);
    let image = attachment("photo.png", "image/png", &clean_png(0x0f));
    let encoded = image.data.clone();
    assert!(
        encoded.len() > 8,
        "the fixture must be findable in a transcript"
    );

    registry
        .send_with_subscription(
            "attach-journal",
            50,
            "look at this",
            &[image],
            &[],
            &owner,
            &conn,
        )
        .expect("send");

    let events: Vec<SessionEvent> = conn
        .pull_events()
        .into_iter()
        .map(|event| event.envelope.event)
        .collect();
    let recorded = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::AgentUserMessage { text, images, .. } => {
                Some((text.clone(), images.clone()))
            }
            _ => None,
        })
        .expect("the user message is published, and that is what is journaled");
    assert_eq!(
        recorded.0, "look at this",
        "the echo is the user's own text, not the provider-composed prompt"
    );
    assert!(
        !recorded.0.contains("[Image available at: "),
        "no provider-composed line reaches the transcript: {recorded:?}"
    );
    assert!(
        recorded.1.is_empty(),
        "inline attachments travel no references: {recorded:?}"
    );
    assert!(
        !recorded.0.contains(&encoded),
        "the base64 must never reach the transcript"
    );
    assert!(
        recorded.0.len() < MAX_WRITE_BYTES,
        "the transcript row stays the size it was before attachments"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_send_without_attachments_is_byte_identical_to_before() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-attach-none", "process-attach");
    let received = Arc::new(Mutex::new(Vec::new()));
    let runtime = insert_live_agent_with_writer(
        &registry,
        "attach-none",
        owner.clone(),
        Box::new(RecordingWriter(Arc::clone(&received))),
    );
    let conn = attach_live_agent_for_test(&runtime, "attach-none", 51);

    registry
        .send_with_subscription("attach-none", 51, "plain prompt", &[], &[], &owner, &conn)
        .expect("send");

    assert_eq!(received.lock().expect("writer").as_slice(), b"plain prompt");
    assert!(
        !attachment_folder(&registry, "attach-none").exists(),
        "no attachment means no folder"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_terminal_session_refuses_attachments_before_writing_anything() {
    // Terminals have no sibling (`image_sink: None`) and fail before it:
    // the PTY refusal above runs before any materialize or any write.
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-attach-terminal", "process-attach");
    let received = Arc::new(Mutex::new(Vec::new()));
    insert_live_with_writer(
        &registry,
        "attach-terminal",
        owner.clone(),
        Box::new(RecordingWriter(Arc::clone(&received))),
    );
    let conn = ConnHandle::new(52);
    registry
        .attach("attach-terminal", None, &conn, &owner, false)
        .expect("terminal attaches");

    let error = registry
        .send_with_subscription(
            "attach-terminal",
            52,
            "hello",
            &[attachment("photo.png", "image/png", &clean_png(0x10))],
            &[],
            &owner,
            &conn,
        )
        .expect_err("a terminal does not accept attachments");
    assert_eq!(
        attachment_message(&error),
        "This session does not accept attachments."
    );
    assert!(
        received.lock().expect("writer").is_empty(),
        "an appended line would be typed into the PTY"
    );
    assert!(
        !attachment_folder(&registry, "attach-terminal").exists(),
        "nothing is materialized for a session that cannot read it"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

// --- structured prompts (ACP image blocks) -----------------------------
//
// The decision — which attachments become blocks, what text the journal
// records — is `plan_structured_prompt`, a pure function of the request's
// `(text, attachments)`, so the block-shape tests pin it against the
// attachment store directly, without spawning a child. The wire shape of
// one block is pinned against the exact JSON the ACP read-side test
// already expects
// (`{"type":"image","mimeType":"image/png","data":"<base64>"}`).
// The journal on the structured route is pinned below by reading the
// published `AgentUserMessage` — the same way `a_fallback_session_journals`
// pins the fallback route — through a sink double that stands in for the
// child. A test that saw the plan but not the journal call would still be
// an argument from reading the code, and the journal is the one place a
// leak would be permanent.

#[test]
fn a_supported_session_plans_an_image_block_and_no_path_line() {
    // Supported: the raster becomes one image block; the text block is
    // the bare user text, with no path line.
    let (dir, _registry, journal) = tmp_delete_registry();
    let session_id = "attach-block";
    assert_eq!(
        ImageDelivery::from_negotiated(crate::acp_view::PromptCapabilityState::Supported),
        ImageDelivery::NegotiatedImageBlock,
    );
    // A container the walk accepts but changes: what the block carries
    // must be the stripped bytes, never the wire bytes.
    let sent = crate::raster_metadata::png_with_text_chunk();
    let kept = clean_png(0x01);
    assert_ne!(
        sent, kept,
        "the fixture must actually carry something that leaves"
    );
    let store = AttachmentStore::new(&dir);
    let plan = plan_structured_prompt(
        &store,
        session_id,
        "describe this",
        &[attachment("photo.png", "image/png", &sent)],
    )
    .expect("planned")
    .expect("a raster plans a structured prompt");
    assert_eq!(
        plan.fallback_text, "describe this",
        "no fallback path means the bare text"
    );
    assert_eq!(plan.images.len(), 1);
    assert_eq!(plan.images[0].mime_type, "image/png");
    {
        use base64::Engine;
        assert_eq!(
            plan.images[0].data_base64,
            base64::engine::general_purpose::STANDARD.encode(&kept),
            "the block carries the stripped bytes"
        );
    }
    let block = plan.images[0].to_content_block();
    assert_eq!(
        block.get("type").and_then(|value| value.as_str()),
        Some("image")
    );
    assert_eq!(
        block.get("mimeType").and_then(|value| value.as_str()),
        Some("image/png")
    );
    assert!(
        block.get("data").and_then(|value| value.as_str()).is_some(),
        "the ACP image block shape the read-side test pins"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_refused_session_keeps_the_path_line_and_builds_no_block() {
    // Refused (`false` in the handshake): the safe answer is the path
    // line, exactly as today, and no block is built.
    assert_eq!(
        ImageDelivery::from_negotiated(crate::acp_view::PromptCapabilityState::Unsupported),
        ImageDelivery::PathLine,
    );
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-attach-refused", "process-attach");
    let received = Arc::new(Mutex::new(Vec::new()));
    // No sibling installed: the fallback world, like a session whose
    // handshake refused images.
    let runtime = insert_live_agent_with_writer(
        &registry,
        "attach-refused",
        owner.clone(),
        Box::new(RecordingWriter(Arc::clone(&received))),
    );
    let conn = attach_live_agent_for_test(&runtime, "attach-refused", 61);
    let image = clean_png(0x11);
    registry
        .send_with_subscription(
            "attach-refused",
            61,
            "describe this",
            &[attachment("photo.png", "image/png", &image)],
            &[],
            &owner,
            &conn,
        )
        .expect("send");
    let written = String::from_utf8(received.lock().expect("writer").clone()).expect("utf8");
    assert!(
        written.starts_with("describe this\n\n[Image available at: "),
        "{written}"
    );
    assert!(!written.contains("\"type\":\"image\""), "{written}");
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_unknown_session_keeps_the_path_line_and_builds_no_block() {
    // Absent (the agent said nothing, or a malformed value): silence is
    // not consent, so the path line is the safe answer. Unknown never
    // means yes.
    assert_eq!(
        ImageDelivery::from_negotiated(crate::acp_view::PromptCapabilityState::Absent),
        ImageDelivery::PathLine,
    );
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-attach-unknown", "process-attach");
    let received = Arc::new(Mutex::new(Vec::new()));
    let runtime = insert_live_agent_with_writer(
        &registry,
        "attach-unknown",
        owner.clone(),
        Box::new(RecordingWriter(Arc::clone(&received))),
    );
    let conn = attach_live_agent_for_test(&runtime, "attach-unknown", 62);
    registry
        .send_with_subscription(
            "attach-unknown",
            62,
            "describe this",
            &[attachment("photo.png", "image/png", &clean_png(0x12))],
            &[],
            &owner,
            &conn,
        )
        .expect("send");
    let written = String::from_utf8(received.lock().expect("writer").clone()).expect("utf8");
    assert!(
        written.starts_with("describe this\n\n[Image available at: "),
        "{written}"
    );
    assert!(!written.contains("\"type\":\"image\""), "{written}");
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_svg_keeps_its_path_line_beside_image_blocks() {
    // SVG never becomes a block — no provider accepts it inline — so a
    // mixed prompt carries both: the raster as a block, the SVG as a
    // path line in the text block.
    let (dir, _registry, journal) = tmp_delete_registry();
    let session_id = "attach-mixed";
    let store = AttachmentStore::new(&dir);
    let source = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>";
    let plan = plan_structured_prompt(
        &store,
        session_id,
        "logo and photo",
        &[
            attachment("photo.png", "image/png", &clean_png(0x13)),
            attachment("drawing.svg", "image/svg+xml", source),
        ],
    )
    .expect("planned")
    .expect("a mixed prompt plans a structured prompt");
    assert_eq!(plan.images.len(), 1, "only the raster becomes a block");
    assert_eq!(plan.images[0].mime_type, "image/png");
    assert!(
        plan.fallback_text
            .starts_with("logo and photo\n\n[Image available at: "),
        "{}",
        plan.fallback_text
    );
    assert!(
        plan.fallback_text.ends_with(".svg]"),
        "{}",
        plan.fallback_text
    );
    assert!(
        !plan.fallback_text.contains(".png]"),
        "the raster left no path line: {}",
        plan.fallback_text
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_gif_keeps_its_path_line_beside_image_blocks() {
    // Only Claude is known to take a GIF inline, so the structured ACP plan
    // sends the PNG as a block and the GIF as a path line, and a GIF-only
    // prompt stays on the legacy write like an SVG-only one.
    use crate::raster_metadata::container_fixtures::clean_gif;
    let (dir, _registry, journal) = tmp_delete_registry();
    let store = AttachmentStore::new(&dir);
    let plan = plan_structured_prompt(
        &store,
        "attach-gif-mixed",
        "photo and loop",
        &[
            attachment("photo.png", "image/png", &clean_png(0x15)),
            attachment("loop.gif", "image/gif", &clean_gif(1)),
        ],
    )
    .expect("planned")
    .expect("the png plans a structured prompt");
    assert_eq!(plan.images.len(), 1, "only the png becomes a block");
    assert_eq!(plan.images[0].mime_type, "image/png");
    assert!(
        plan.fallback_text
            .starts_with("photo and loop\n\n[Image available at: "),
        "{}",
        plan.fallback_text
    );
    assert!(
        plan.fallback_text.ends_with(".gif]"),
        "{}",
        plan.fallback_text
    );
    let gif_only = plan_structured_prompt(
        &store,
        "attach-gif-only",
        "loop",
        &[attachment("loop.gif", "image/gif", &clean_gif(2))],
    )
    .expect("planned");
    assert!(gif_only.is_none(), "a GIF-only prompt plans no block");
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_svg_only_prompt_plans_no_structured_prompt() {
    // An SVG-only prompt on a capable session has nothing to send inline:
    // the plan is `None`, so the send path takes the legacy write —
    // materialized once, never twice.
    let (dir, _registry, journal) = tmp_delete_registry();
    let store = AttachmentStore::new(&dir);
    let source = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>";
    let plan = plan_structured_prompt(
        &store,
        "attach-svg-only",
        "logo",
        &[attachment("drawing.svg", "image/svg+xml", source)],
    )
    .expect("planned");
    assert!(
        plan.is_none(),
        "an SVG-only prompt stays on the legacy path-line write"
    );
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_block_mime_type_matches_what_was_stored() {
    // A JPEG stays a JPEG on the wire: the label `materialize` checked
    // against the sniffed container is the label the block carries.
    let (dir, _registry, journal) = tmp_delete_registry();
    let store = AttachmentStore::new(&dir);
    const EXIF_JPEG_VECTOR: &str =
        "a jpeg whose APP1 holds EXIF, between a kept JFIF APP0 and a kept ICC APP2";
    let sent = crate::raster_metadata::vector_input(EXIF_JPEG_VECTOR);
    let kept = crate::raster_metadata::vector_output(EXIF_JPEG_VECTOR);
    let plan = plan_structured_prompt(
        &store,
        "attach-mime",
        "describe this",
        &[attachment("photo.jpg", "image/jpeg", &sent)],
    )
    .expect("planned")
    .expect("a raster plans a structured prompt");
    assert_eq!(plan.fallback_text, "describe this");
    assert_eq!(plan.images.len(), 1);
    assert_eq!(plan.images[0].mime_type, "image/jpeg");
    {
        use base64::Engine;
        assert_eq!(
            plan.images[0].data_base64,
            base64::engine::general_purpose::STANDARD.encode(&kept),
            "stripped JPEG bytes, JPEG label"
        );
    }
    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

// --- chat images: deposited references on the user message ---

/// A composer send that names deposited references echoes those references on
/// its `AgentUserMessage`, so replay can find the stored bytes. The prompt
/// text carries a path line, never the base64.
#[test]
fn a_send_with_references_echoes_them_on_the_user_message_without_bytes() {
    use base64::Engine;
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-chat-images", "process-chat-images");
    let received = Arc::new(Mutex::new(Vec::new()));
    let runtime = insert_live_agent_with_writer(
        &registry,
        "chat-images",
        owner.clone(),
        Box::new(RecordingWriter(Arc::clone(&received))),
    );
    let conn = attach_live_agent_for_test(&runtime, "chat-images", 61);
    let image = clean_png(0x0b);
    let reference = registry
        .deposit(
            "chat-images",
            &owner,
            &conn,
            &attachment("photo.png", "image/png", &image),
        )
        .expect("the owner may deposit into their own session");

    registry
        .send_with_subscription(
            "chat-images",
            61,
            "look at this",
            &[],
            std::slice::from_ref(&reference),
            &owner,
            &conn,
        )
        .expect("send with one deposited reference");

    let encoded = base64::engine::general_purpose::STANDARD.encode(&image);
    let echoed = conn
        .pull_events()
        .into_iter()
        .map(|event| event.envelope.event)
        .find_map(|event| match event {
            SessionEvent::AgentUserMessage { text, images, .. } => Some((text, images)),
            _ => None,
        })
        .expect("the user message is published, and that is what is journaled");
    assert_eq!(
        echoed.1,
        vec![reference],
        "the echo carries the deposited reference: {echoed:?}"
    );
    assert!(
        !echoed.0.contains(&encoded),
        "the base64 must never reach the transcript"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// A recovered transcript replays the references, so thumbnails resolve after
/// a restart. Rows written before references existed replay with none.
#[test]
fn image_references_survive_journal_replay_for_recovered_transcripts() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-chat-replay", "process-chat-replay");
    // The journal row the live registry never writes itself: replay reads
    // the row, so the test births it the way `Journal::create_session`
    // callers do and lets the send append its event rows under it.
    journal
        .create_session(crate::journal::new_session_record(
            "chat-replay",
            "S-1-5-21-chat-replay",
            None,
            devboule_protocol::SessionKind::Acp,
            "Chat images",
        ))
        .expect("birth the journal row");
    let received = Arc::new(Mutex::new(Vec::new()));
    let runtime = insert_live_agent_with_writer(
        &registry,
        "chat-replay",
        owner.clone(),
        Box::new(RecordingWriter(Arc::clone(&received))),
    );
    let conn = attach_live_agent_for_test(&runtime, "chat-replay", 62);
    let image = clean_png(0x0b);
    let reference = registry
        .deposit(
            "chat-replay",
            &owner,
            &conn,
            &attachment("photo.png", "image/png", &image),
        )
        .expect("deposit");
    registry
        .send_with_subscription(
            "chat-replay",
            62,
            "look at this",
            &[],
            std::slice::from_ref(&reference),
            &owner,
            &conn,
        )
        .expect("send");

    let replay = journal.replay("chat-replay").expect("replay");
    let replayed = replay
        .events
        .iter()
        .find_map(|event| match event {
            SessionEvent::AgentUserMessage { text, images, .. } => Some((text, images)),
            _ => None,
        })
        .expect("the journaled user message replays");
    assert_eq!(
        *replayed.1,
        vec![reference],
        "replay keeps the deposited reference: {replayed:?}"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// A send with deposited references journals and replays the user's own text
/// with those references: the provider-composed path lines travel to the
/// provider only, never into the transcript the user sees.
#[test]
fn a_send_with_references_journals_the_users_text_without_path_lines() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-chat-clean", "process-chat-clean");
    // The journal row the live registry never writes itself: birth it first
    // so the send's event rows land in the journal and replay serves them.
    journal
        .create_session(crate::journal::new_session_record(
            "chat-clean",
            "S-1-5-21-chat-clean",
            None,
            devboule_protocol::SessionKind::Acp,
            "Chat clean",
        ))
        .expect("birth the journal row");
    let received = Arc::new(Mutex::new(Vec::new()));
    let runtime = insert_live_agent_with_writer(
        &registry,
        "chat-clean",
        owner.clone(),
        Box::new(RecordingWriter(Arc::clone(&received))),
    );
    let conn = attach_live_agent_for_test(&runtime, "chat-clean", 63);
    let first = registry
        .deposit(
            "chat-clean",
            &owner,
            &conn,
            &attachment("first.png", "image/png", &clean_png(0x0b)),
        )
        .expect("deposit");
    let second = registry
        .deposit(
            "chat-clean",
            &owner,
            &conn,
            &attachment("second.png", "image/png", &clean_png(0x0c)),
        )
        .expect("deposit");

    registry
        .send_with_subscription(
            "chat-clean",
            63,
            "look at these",
            &[],
            &[first.clone(), second.clone()],
            &owner,
            &conn,
        )
        .expect("send with two deposited references");

    // The provider still gets the composed text with one path line per file.
    let written = String::from_utf8(received.lock().expect("writer").clone()).expect("utf8");
    assert_eq!(
        written
            .split('\n')
            .filter(|line| line.starts_with("[Image available at: "))
            .count(),
        2,
        "the provider keeps both path lines: {written}"
    );
    let user_turn = conn
        .pull_events()
        .into_iter()
        .map(|event| event.envelope.event)
        .find_map(|event| match event {
            SessionEvent::AgentUserMessage { text, images, .. } => Some((text, images)),
            _ => None,
        })
        .expect("the user message is published");
    assert_eq!(
        user_turn.0, "look at these",
        "the echo is the user's text: {:?}",
        user_turn.0
    );
    assert!(
        !user_turn.0.contains("[Image available at: "),
        "no provider-composed line reaches the transcript: {:?}",
        user_turn.0
    );
    assert_eq!(user_turn.1, vec![first.clone(), second.clone()]);

    // Replay serves the same clean row: the event and its references
    // survive the journal. What the replacement agent is handed is proven
    // separately, by the recovery-context tests.
    let replayed = journal
        .replay("chat-clean")
        .expect("replay")
        .events
        .into_iter()
        .find_map(|event| match event {
            SessionEvent::AgentUserMessage { text, images, .. } => Some((text, images)),
            _ => None,
        })
        .expect("the journaled user message replays");
    assert_eq!(replayed.0, "look at these");
    assert!(!replayed.0.contains("[Image available at: "));
    assert_eq!(replayed.1, vec![first, second]);

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// A chunked file upload by the session's owner answers a reference whose
/// name is the sanitized display name, and the prompt the send plan builds
/// carries Paseo's named block with the path the agent reads.
#[test]
fn an_owners_chunked_upload_reaches_the_prompt_as_a_named_block() {
    use base64::Engine as _;
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-upload-owner", "process-upload");
    let id = compose_session_id(&owner.session_token(), "uplo01").expect("id");
    insert_live(&registry, &id, owner.clone());
    let conn = ConnHandle::new(4);
    let bytes: Vec<u8> = b"%PDF-1.4 quarterly numbers".to_vec();
    let total = bytes.len() as u64;
    let encoded = |slice: &[u8]| base64::engine::general_purpose::STANDARD.encode(slice);
    let split = bytes.len() / 2;

    assert_eq!(
        registry
            .begin_upload(
                &id,
                &owner,
                &conn,
                "up-own-1",
                "quarterly report.pdf",
                total
            )
            .expect("the owner may open an upload"),
        0
    );
    assert_eq!(
        registry
            .upload_chunk(&id, &owner, &conn, "up-own-1", 0, &encoded(&bytes[..split]))
            .expect("first chunk"),
        split as u64
    );
    assert_eq!(
        registry
            .upload_chunk(
                &id,
                &owner,
                &conn,
                "up-own-1",
                split as u64,
                &encoded(&bytes[split..])
            )
            .expect("second chunk"),
        total
    );
    let reference = registry
        .finish_upload(&id, &owner, &conn, "up-own-1")
        .expect("the finish answers the reference");
    assert_eq!(reference.session_id, id);
    assert_eq!(reference.name, "quarterly report.pdf");
    assert_eq!(reference.stored_bytes, total);
    assert_eq!(
        reference.digest,
        crate::attachment_store::sha256_hex(&bytes)
    );

    let resolved =
        resolve_attachment_references(&registry.attachments, &id, std::slice::from_ref(&reference))
            .expect("the reference resolves");
    let mut prompt = String::from("summarise the attached file");
    push_reference_path_lines(&mut prompt, &resolved);
    assert!(
        prompt.starts_with(
            "summarise the attached file\n\nUploaded file: quarterly report.pdf\nPath: "
        ),
        "{prompt}"
    );
    assert!(prompt.contains("\nMIME: application/pdf\n"), "{prompt}");
    assert!(
        prompt.ends_with(&format!("Size: {total} bytes")),
        "{prompt}"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// An upload by another user is refused at the ownership door, before any
/// part file exists, and an upload for a session that is gone is refused
/// too.
#[test]
fn an_upload_by_another_user_is_unauthorized_and_stages_nothing() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-upload-theirs", "process-upload-theirs");
    let other = test_owner("S-1-5-21-upload-other", "process-upload-other");
    let id = compose_session_id(&owner.session_token(), "uplo02").expect("id");
    insert_live(&registry, &id, owner.clone());
    let conn = ConnHandle::new(4);

    let error = registry
        .begin_upload(&id, &other, &conn, "up-their-1", "x.bin", 4)
        .expect_err("another user may not open an upload");
    assert_eq!(error.code, ErrorCode::Unauthorized, "{error:?}");
    assert!(
        !attachment_folder(&registry, &id).exists(),
        "a refused upload must not create the session's folder"
    );

    let error = registry
        .begin_upload("s.no.1", &owner, &conn, "up-gone-1", "x.bin", 4)
        .expect_err("an unknown session is refused");
    assert_eq!(error.code, ErrorCode::SessionNotFound, "{error:?}");

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}
