//! The stored-reference tests, moved whole out of `session_tests.rs` lines
//! 2261-2567 (at `774478c`): a deposited reference reaching the provider as a
//! path line, a reference whose stored bytes disagree with the file refused, a
//! reference naming another session refused by the wire's own rule before the
//! store is asked, a reference whose digest was never deposited refused rather
//! than dropped, and inline attachments and references in one send keeping the
//! client's order. Every line below is byte-identical to its text there apart
//! from this header; `stored_path` is promoted to `pub(super)` for this move,
//! every other fixture already was, and `clean_png` is imported here rather
//! than through the provider.

use super::tests::{
    attach_live_agent_for_test, attachment, attachment_folder, attachment_message, files_under,
    insert_live_agent_with_writer, stored_path, test_owner, tmp_delete_registry, RecordingWriter,
};
use super::*;
use crate::raster_metadata::clean_png;

/// The prompt the plain-text writer received, as a string.
fn written_prompt(received: &Arc<Mutex<Vec<u8>>>) -> String {
    String::from_utf8(received.lock().expect("writer").clone()).expect("utf8")
}

#[test]
fn a_deposited_reference_reaches_the_provider_as_a_path_line() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-ref-deposited", "process-ref-deposited");
    let session_id = "ref-deposited";
    let received = Arc::new(Mutex::new(Vec::new()));
    let runtime = insert_live_agent_with_writer(
        &registry,
        session_id,
        owner.clone(),
        Box::new(RecordingWriter(Arc::clone(&received))),
    );
    let conn = attach_live_agent_for_test(&runtime, session_id, 61);
    let deck = clean_png(0x31);
    let request = attachment("deck.png", "image/png", &deck);

    let reference = registry
        .deposit(session_id, &owner, &conn, &request)
        .expect("the owner may deposit into their own session");

    registry
        .send_with_subscription(
            session_id,
            61,
            "read the deck",
            &[],
            std::slice::from_ref(&reference),
            &owner,
            &conn,
        )
        .expect("a send naming a reference that was really deposited");

    let stored = files_under(&attachment_folder(&registry, session_id));
    assert_eq!(stored.len(), 1, "the deposit wrote one file");
    assert_eq!(
        stored_path(&registry, session_id, &reference.digest),
        stored[0],
        "the reference resolves to the deposited file"
    );
    assert_eq!(
        written_prompt(&received),
        format!(
            "read the deck\n\n[Image available at: {}]",
            stored[0].display()
        ),
        "the provider is handed the stored file's path, not its bytes"
    );
    assert!(
        !written_prompt(&received).contains(&request.data),
        "a reference exists so the bytes do not travel in the frame"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_reference_whose_stored_bytes_disagree_with_the_file_is_refused() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-ref-size", "process-ref-size");
    let session_id = "ref-size";
    let received = Arc::new(Mutex::new(Vec::new()));
    let runtime = insert_live_agent_with_writer(
        &registry,
        session_id,
        owner.clone(),
        Box::new(RecordingWriter(Arc::clone(&received))),
    );
    let conn = attach_live_agent_for_test(&runtime, session_id, 62);

    let mut reference = registry
        .deposit(
            session_id,
            &owner,
            &conn,
            &attachment("deck.png", "image/png", &clean_png(0x32)),
        )
        .expect("deposit");
    let real_size = reference.stored_bytes;
    // The client's copy of the size is off by one: it is naming a file it
    // did not deposit, or a file that changed under it.
    reference.stored_bytes = real_size + 1;

    let error = registry
        .send_with_subscription(
            session_id,
            62,
            "read the deck",
            &[],
            std::slice::from_ref(&reference),
            &owner,
            &conn,
        )
        .expect_err("a size that disagrees with the file is a refusal, not a warning");

    let message = attachment_message(&error);
    assert!(message.contains(&reference.digest), "{message}");
    assert!(message.contains(&real_size.to_string()), "{message}");
    assert!(message.contains(&(real_size + 1).to_string()), "{message}");
    assert!(
        received.lock().expect("writer").is_empty(),
        "a refused reference must not leave a prompt half-sent"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// The wire's session rule comes before the store, which is the order the
/// whole path keeps. The discriminating half of the assertion is that the
/// store *could* have answered: the reference names a file that really is
/// on disk, in the other session's folder. If resolution ran first, this
/// request would be refused for a digest the store cannot find in the
/// request's session, and the session sentence would never be reached.
#[test]
fn a_reference_naming_another_session_is_refused_by_the_wires_own_rule() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-ref-foreign", "process-ref-foreign");
    let session_id = "ref-foreign";
    let received = Arc::new(Mutex::new(Vec::new()));
    let runtime = insert_live_agent_with_writer(
        &registry,
        session_id,
        owner.clone(),
        Box::new(RecordingWriter(Arc::clone(&received))),
    );
    let conn = attach_live_agent_for_test(&runtime, session_id, 63);
    let other_id = compose_session_id(&owner.session_token(), "ref02").expect("id");
    insert_live_agent_with_writer(
        &registry,
        &other_id,
        owner.clone(),
        Box::new(std::io::sink()),
    );
    let other = registry
        .deposit(
            &other_id,
            &owner,
            // A connection of its own: this deposit is not the request's,
            // and the request's session is the one that must stay empty.
            &ConnHandle::new(640),
            &attachment("deck.png", "image/png", &clean_png(0x33)),
        )
        .expect("the owner deposits into the other session too");
    assert_eq!(
        other.session_id, other_id,
        "the reference names the other session"
    );
    assert!(
        files_under(&attachment_folder(&registry, session_id)).is_empty(),
        "only the other session was deposited into, so the store has nothing to resolve \
         against the request's session"
    );

    let error = registry
        .send_with_subscription(
            session_id,
            63,
            "read the deck",
            &[],
            std::slice::from_ref(&other),
            &owner,
            &conn,
        )
        .expect_err("a reference to another session is refused");

    let message = attachment_message(&error);
    assert!(message.contains("belongs to session"), "{message}");
    assert!(
        message.contains(&other_id),
        "the refusal says which session the reference belongs to: {message}"
    );
    assert!(
        !message.contains("holds no attachment"),
        "the store's sentence means the store was asked first: {message}"
    );
    assert!(received.lock().expect("writer").is_empty());

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

/// The regression the deleted dispatch guard stood in for. Before the
/// resolution existed, a request naming references was refused whole; what
/// must not happen now is a send that answers `Ok` while the file it named
/// was quietly dropped out of the prompt, which is the vanishing deck the
/// whole feature exists to prevent.
#[test]
fn a_reference_whose_digest_was_never_deposited_is_refused_not_dropped() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-ref-phantom", "process-ref-phantom");
    let session_id = "ref-phantom";
    let received = Arc::new(Mutex::new(Vec::new()));
    let runtime = insert_live_agent_with_writer(
        &registry,
        session_id,
        owner.clone(),
        Box::new(RecordingWriter(Arc::clone(&received))),
    );
    let conn = attach_live_agent_for_test(&runtime, session_id, 64);
    let phantom = AttachmentReference {
        session_id: session_id.to_string(),
        digest: "a".repeat(64),
        stored_bytes: 4096,
    };

    let error = registry
        .send_with_subscription(
            session_id,
            64,
            "read the deck",
            &[],
            std::slice::from_ref(&phantom),
            &owner,
            &conn,
        )
        .expect_err("a digest with no file behind it is refused");

    let message = attachment_message(&error);
    assert!(
        message.contains("holds no attachment"),
        "the store's own sentence is the one that must come back: {message}"
    );
    assert!(
        received.lock().expect("writer").is_empty(),
        "nothing was sent, so nothing was silently missing from it"
    );
    assert!(
        files_under(&attachment_folder(&registry, session_id)).is_empty(),
        "a refused send writes nothing either"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn inline_attachments_and_references_in_one_send_keep_the_order_the_client_gave() {
    let (dir, registry, journal) = tmp_delete_registry();
    let owner = test_owner("S-1-5-21-ref-order", "process-ref-order");
    let session_id = "ref-order";
    let received = Arc::new(Mutex::new(Vec::new()));
    let runtime = insert_live_agent_with_writer(
        &registry,
        session_id,
        owner.clone(),
        Box::new(RecordingWriter(Arc::clone(&received))),
    );
    let conn = attach_live_agent_for_test(&runtime, session_id, 65);
    let inline_bytes = clean_png(0x34);
    // Two stored decks, deposited in the opposite order to the one the
    // request names them in: the prompt must follow the request, not the
    // store's write order.
    let second = registry
        .deposit(
            session_id,
            &owner,
            &conn,
            &attachment("second.png", "image/png", &clean_png(0x35)),
        )
        .expect("deposit the deck the request names second");
    let first = registry
        .deposit(
            session_id,
            &owner,
            &conn,
            &attachment("first.png", "image/png", &clean_png(0x36)),
        )
        .expect("deposit the deck the request names first");

    registry
        .send_with_subscription(
            session_id,
            65,
            "two files",
            &[attachment("inline.png", "image/png", &inline_bytes)],
            &[first.clone(), second.clone()],
            &owner,
            &conn,
        )
        .expect("one inline attachment and two references in one send");

    // `clean_png` carries no metadata the store strips, so the digest of
    // the bytes the client sent is the name the daemon stored them under.
    let inline_path = attachment_folder(&registry, session_id).join(format!(
        "{}.png",
        crate::attachment_store::sha256_hex(&inline_bytes)
    ));
    assert_eq!(
        written_prompt(&received),
        format!(
            "two files\n\n[Image available at: {}]\n\n[Image available at: {}]\n[Image available at: {}]",
            inline_path.display(),
            stored_path(&registry, session_id, &first.digest).display(),
            stored_path(&registry, session_id, &second.digest).display()
        ),
        "the inline attachment's line comes first and the references follow in the client's order"
    );

    journal.shutdown();
    let _ = std::fs::remove_dir_all(dir);
}
