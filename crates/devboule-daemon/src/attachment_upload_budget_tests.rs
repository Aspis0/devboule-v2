//! The staged-file half of the upload tests: the bytes in flight against
//! the store budget, the concurrency cap, the redirection and length
//! refusals, and the delete that gives the bytes back.

use devboule_protocol::MAX_ATTACHMENT_OWNER_BYTES;

use super::tests::{encoded, fixture, part_files};

#[test]
fn staged_bytes_are_charged_to_the_store_and_released_on_abort() {
    let (_temp, store, uploads) = fixture();
    let session_id = "s.budget.1";
    uploads
        .begin(&store, session_id, "up-b1", "a.bin", 6)
        .expect("open");
    uploads
        .chunk(&store, session_id, "up-b1", 0, &encoded(b"abc"))
        .expect("chunk");
    assert_eq!(
        store.store_bytes(),
        Some(3),
        "bytes in flight are the store's bytes"
    );
    assert_eq!(store.session_bytes(session_id), Some(3));
    uploads.abort(&store, session_id, "up-b1").expect("abort");
    assert_eq!(store.store_bytes(), Some(0), "the abort gives them back");
}

#[test]
fn a_chunk_over_the_owner_budget_is_refused_before_it_is_written() {
    let (_temp, store, uploads) = fixture();
    // A sparse filler is the cheapest fixture that fills the budget: the walk
    // counts its length, and no page is ever written.
    let filler = store
        .prepare_upload_dir("s.budget.2")
        .expect("filler folder");
    std::fs::File::create(filler.join("filler.bin"))
        .expect("filler")
        .set_len(MAX_ATTACHMENT_OWNER_BYTES as u64)
        .expect("sparse filler");

    let session_id = "s.budget.3";
    uploads
        .begin(&store, session_id, "up-b2", "a.bin", 4)
        .expect("open");
    let error = uploads
        .chunk(&store, session_id, "up-b2", 0, &encoded(b"abcd"))
        .expect_err("the owner budget is full");
    assert!(
        error
            .message
            .contains(&MAX_ATTACHMENT_OWNER_BYTES.to_string()),
        "{error:?}"
    );
    assert_eq!(
        uploads.status(session_id, "up-b2").expect("status"),
        0,
        "a refused chunk writes nothing"
    );
}

#[test]
fn a_session_may_hold_at_most_four_uploads_at_once() {
    let (_temp, store, uploads) = fixture();
    let session_id = "s.stage.conc";
    for index in 1..=4 {
        uploads
            .begin(&store, session_id, &format!("up-c{index}"), "a.bin", 4)
            .expect("one of four");
    }
    let error = uploads
        .begin(&store, session_id, "up-c5", "a.bin", 4)
        .expect_err("the fifth is refused");
    assert!(error.message.contains("4 uploads"), "{error:?}");
}

#[test]
fn a_name_that_is_not_a_file_is_refused_before_the_stage_write() {
    let (_temp, store, uploads) = fixture();
    let session_id = "s.stage.dir";
    let dir = store.prepare_upload_dir(session_id).expect("folder");
    std::fs::create_dir(dir.join("upload-up-d1.part")).expect("a directory at the part name");
    let error = uploads
        .begin(&store, session_id, "up-d1", "a.bin", 4)
        .expect_err("a directory is not a part file");
    assert!(error.message.contains("is not a file"), "{error:?}");
}

#[test]
fn a_staged_length_that_disagrees_with_the_declaration_is_refused() {
    let (_temp, store, uploads) = fixture();
    let session_id = "s.stage.short";
    uploads
        .begin(&store, session_id, "up-e1", "a.bin", 4)
        .expect("open");
    uploads
        .chunk(&store, session_id, "up-e1", 0, &encoded(b"abcd"))
        .expect("chunk");
    let dir = store.prepare_upload_dir(session_id).expect("folder");
    std::fs::OpenOptions::new()
        .write(true)
        .open(dir.join("upload-up-e1.part"))
        .expect("open part")
        .set_len(2)
        .expect("truncate");

    let error = uploads
        .finish(&store, session_id, "up-e1")
        .expect_err("a staged file the declaration does not describe is refused");
    assert!(error.message.contains("2 of its 4"), "{error:?}");
    assert_eq!(
        store.store_bytes(),
        Some(0),
        "the charged bytes are released"
    );
    assert_eq!(
        uploads
            .begin(&store, session_id, "up-e1", "a.bin", 4)
            .expect("a fresh begin under the same id"),
        0
    );
}

#[test]
fn a_stored_attachment_can_be_deleted_and_releases_its_bytes() {
    let (_temp, store, uploads) = fixture();
    let session_id = "s.del.1";
    uploads
        .begin(&store, session_id, "up-f1", "a.log", 4)
        .expect("open");
    uploads
        .chunk(&store, session_id, "up-f1", 0, &encoded(b"abcd"))
        .expect("chunk");
    let (deposited, _) = uploads.finish(&store, session_id, "up-f1").expect("finish");
    assert_eq!(store.store_bytes(), Some(4));

    store
        .remove_stored(session_id, &deposited.digest, Some("log"))
        .expect("delete");
    assert!(!deposited.path.exists());
    assert_eq!(store.store_bytes(), Some(0));
    store
        .remove_stored(session_id, &deposited.digest, Some("log"))
        .expect("a file already gone is an Ok");
}

#[test]
fn a_send_over_a_charge_that_would_pass_the_budget_is_refused_before_admit() {
    let (_temp, store, uploads) = fixture();
    let filler = store
        .prepare_upload_dir("s.budget.4")
        .expect("filler folder");
    std::fs::File::create(filler.join("filler.bin"))
        .expect("filler")
        .set_len(MAX_ATTACHMENT_OWNER_BYTES as u64 - 2)
        .expect("sparse filler");
    let session_id = "s.budget.5";
    uploads
        .begin(&store, session_id, "up-g1", "a.bin", 4)
        .expect("open");
    let error = uploads
        .chunk(&store, session_id, "up-g1", 0, &encoded(b"abcd"))
        .expect_err("2 bytes of room are not 4");
    assert!(
        error
            .message
            .contains(&MAX_ATTACHMENT_OWNER_BYTES.to_string()),
        "{error:?}"
    );
}

#[test]
fn an_abort_of_an_id_not_yet_open_refuses_the_begin_that_raced_it() {
    let (_temp, store, uploads) = fixture();
    let session_id = "s.tomb.1";
    uploads
        .abort(&store, session_id, "up-t1")
        .expect("an abort of an id that is not open is an Ok");
    // The abort cannot see the file a `begin` is about to create, so the
    // tombstone is what keeps that begin from leaving a charged part.
    let error = uploads
        .begin(&store, session_id, "up-t1", "a.bin", 4)
        .expect_err("the late begin is refused");
    assert!(error.message.contains("cancelled"), "{error:?}");
    assert!(
        part_files(&store, session_id).is_empty(),
        "the refused begin leaves no part"
    );
    assert_eq!(store.store_bytes(), Some(0));
}

#[test]
fn a_tombstone_from_one_session_does_not_refuse_another() {
    let (_temp, store, uploads) = fixture();
    uploads
        .abort(&store, "s.tomb.a", "up-t2")
        .expect("an abort in one session");
    assert_eq!(
        uploads
            .begin(&store, "s.tomb.b", "up-t2", "a.bin", 4)
            .expect("the other session's id is free"),
        0
    );
}

#[test]
fn an_abort_of_an_open_id_leaves_no_tombstone() {
    let (_temp, store, uploads) = fixture();
    let session_id = "s.tomb.3";
    uploads
        .begin(&store, session_id, "up-t3", "a.bin", 4)
        .expect("open");
    uploads.abort(&store, session_id, "up-t3").expect("abort");
    assert_eq!(
        uploads
            .begin(&store, session_id, "up-t3", "a.bin", 4)
            .expect("an id whose upload was aborted is free to open again"),
        0
    );
}
