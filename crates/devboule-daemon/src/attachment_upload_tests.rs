//! The chunked upload state: offsets, resume, dedupe, sanitizing and
//! cleanup, driven against a real store on a temp runtime directory.

use base64::Engine as _;

use devboule_protocol::{sanitize_attachment_name, MAX_UPLOAD_BYTES};

use super::AttachmentUploads;
use crate::attachment_store::AttachmentStore;

struct TempDir(std::path::PathBuf);

impl TempDir {
    fn new() -> Self {
        Self(crate::test_dirs::test_temp_dir("devboule-upload"))
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn fixture() -> (TempDir, AttachmentStore, AttachmentUploads) {
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    (temp, store, AttachmentUploads::default())
}

fn encoded(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn session_dir(store: &AttachmentStore, session_id: &str) -> std::path::PathBuf {
    store
        .prepare_upload_dir(session_id)
        .expect("the session folder")
}

fn part_files(store: &AttachmentStore, session_id: &str) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(session_dir(store, session_id)) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|extension| extension.to_str()) == Some("part"))
        .collect()
}

fn stored_files(store: &AttachmentStore, session_id: &str) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(session_dir(store, session_id)) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.extension().and_then(|extension| extension.to_str()) != Some("part"))
        .collect()
}

#[test]
fn a_file_uploaded_in_chunks_finishes_as_a_stored_file() {
    let (_temp, store, uploads) = fixture();
    let data: Vec<u8> = (0..300_000u32).map(|index| index as u8).collect();
    let session_id = "s.a.1";
    assert_eq!(
        uploads
            .begin(&store, session_id, "up-1", "report.log", data.len() as u64)
            .expect("open the upload"),
        0
    );
    let first = &data[..100_000];
    let second = &data[100_000..];
    assert_eq!(
        uploads
            .chunk(session_id, "up-1", 0, &encoded(first))
            .expect("first chunk"),
        first.len() as u64
    );
    assert_eq!(
        uploads
            .chunk(session_id, "up-1", first.len() as u64, &encoded(second))
            .expect("second chunk"),
        data.len() as u64
    );
    let (deposited, name) = uploads
        .finish(&store, session_id, "up-1")
        .expect("finish the upload");
    assert_eq!(name, "report.log");
    assert_eq!(deposited.digest, crate::attachment_store::sha256_hex(&data));
    assert_eq!(deposited.stored_bytes, data.len() as u64);
    assert_eq!(
        deposited
            .path
            .extension()
            .and_then(|extension| extension.to_str()),
        Some("log")
    );
    assert_eq!(std::fs::read(&deposited.path).expect("stored bytes"), data);
    let (resolved, size) = store
        .resolve(session_id, &deposited.digest, Some("log"))
        .expect("resolve by the name's extension");
    assert_eq!(resolved, deposited.path);
    assert_eq!(size, data.len() as u64);
    assert!(
        part_files(&store, session_id).is_empty(),
        "no part survives a finish"
    );
}

#[test]
fn a_lost_acknowledgement_resumes_and_a_stale_chunk_is_refused() {
    let (_temp, store, uploads) = fixture();
    let session_id = "s.a.2";
    let data = b"aaaaabbbbbcccccddddd";
    uploads
        .begin(&store, session_id, "up-2", "notes.txt", data.len() as u64)
        .expect("open");
    uploads
        .chunk(session_id, "up-2", 0, &encoded(&data[..5]))
        .expect("first chunk");
    // The same declaration under the same id is the reconnect: it answers the
    // offset instead of refusing or starting over.
    assert_eq!(
        uploads
            .begin(&store, session_id, "up-2", "notes.txt", data.len() as u64)
            .expect("adopt"),
        5
    );
    let stale = uploads
        .chunk(session_id, "up-2", 0, &encoded(&data[5..10]))
        .expect_err("a stale offset is refused, not appended blind");
    assert!(
        stale.message.contains('5') && stale.message.contains('0'),
        "{stale:?}"
    );
    assert_eq!(uploads.status(session_id, "up-2").expect("status"), 5);
    uploads
        .chunk(session_id, "up-2", 5, &encoded(&data[5..]))
        .expect("the rest");
    let (deposited, _) = uploads.finish(&store, session_id, "up-2").expect("finish");
    assert_eq!(std::fs::read(deposited.path).expect("bytes"), data);
}

#[test]
fn a_restarted_daemon_refuses_status_and_the_next_begin_starts_over() {
    let (_temp, store, uploads) = fixture();
    let session_id = "s.a.3";
    let data = b"abcdefgh";
    uploads
        .begin(&store, session_id, "up-3", "half.bin", data.len() as u64)
        .expect("open");
    uploads
        .chunk(session_id, "up-3", 0, &encoded(&data[..4]))
        .expect("half");
    assert_eq!(part_files(&store, session_id).len(), 1);
    // A restart loses the in-memory map, which is what `forget_session`
    // models; the status is the refusal the client restarts from.
    uploads.forget_session(session_id);
    let refused = uploads
        .status(session_id, "up-3")
        .expect_err("the survey is gone with the process");
    assert!(refused.message.contains("start it again"), "{refused:?}");
    assert_eq!(
        uploads
            .begin(&store, session_id, "up-3", "half.bin", data.len() as u64)
            .expect("a fresh begin under the same id"),
        0
    );
    uploads
        .chunk(session_id, "up-3", 0, &encoded(data))
        .expect("the whole file");
    let (deposited, _) = uploads.finish(&store, session_id, "up-3").expect("finish");
    assert_eq!(std::fs::read(deposited.path).expect("bytes"), data);
}

#[test]
fn the_declaration_is_bounded_before_anything_is_staged() {
    let (_temp, store, uploads) = fixture();
    let session_id = "s.a.4";
    for (upload_id, total) in [("up-4", 0u64), ("up-5", MAX_UPLOAD_BYTES + 1)] {
        let error = uploads
            .begin(&store, session_id, upload_id, "big.bin", total)
            .expect_err("refused");
        assert_eq!(error.code, devboule_protocol::ErrorCode::InvalidRequest);
    }
    let error = uploads
        .begin(&store, session_id, "../up", "x.bin", 1)
        .expect_err("the id is a token");
    assert!(error.message.contains("upload id"), "{error:?}");
    assert!(part_files(&store, session_id).is_empty());
}

#[test]
fn a_chunk_past_the_declared_total_is_refused() {
    let (_temp, store, uploads) = fixture();
    let session_id = "s.a.5";
    uploads
        .begin(&store, session_id, "up-6", "short.bin", 4)
        .expect("open");
    let error = uploads
        .chunk(session_id, "up-6", 0, &encoded(b"12345"))
        .expect_err("past the declared size");
    assert!(error.message.contains('4'), "{error:?}");
    assert_eq!(uploads.status(session_id, "up-6").expect("status"), 0);
}

#[test]
fn the_display_name_is_sanitized_before_it_reaches_a_reference() {
    let (_temp, store, uploads) = fixture();
    let session_id = "s.a.6";
    for (upload_id, name, expected) in [
        ("up-7", "..\\..\\CON.txt", "_CON.txt"),
        ("up-8", "nul", "_nul"),
        ("up-9", "rápport-é.pdf", "rápport-é.pdf"),
    ] {
        uploads
            .begin(&store, session_id, upload_id, name, 3)
            .expect("open");
        uploads
            .chunk(session_id, upload_id, 0, &encoded(b"abc"))
            .expect("chunk");
        let (_, answered) = uploads
            .finish(&store, session_id, upload_id)
            .expect("finish");
        assert_eq!(answered, expected);
        assert_eq!(sanitize_attachment_name(name), expected);
    }
}

#[test]
fn the_same_bytes_twice_are_one_stored_file() {
    let (_temp, store, uploads) = fixture();
    let session_id = "s.a.7";
    let data = b"the same bytes";
    let mut digests = Vec::new();
    for (upload_id, name) in [("up-10", "first.log"), ("up-11", "second.log")] {
        uploads
            .begin(&store, session_id, upload_id, name, data.len() as u64)
            .expect("open");
        uploads
            .chunk(session_id, upload_id, 0, &encoded(data))
            .expect("chunk");
        let (deposited, _) = uploads
            .finish(&store, session_id, upload_id)
            .expect("finish");
        digests.push(deposited.digest);
    }
    assert_eq!(digests[0], digests[1]);
    assert_eq!(
        stored_files(&store, session_id).len(),
        1,
        "one file, one digest"
    );
}

#[test]
fn abort_removes_the_staged_bytes_and_a_close_drops_the_state() {
    let (temp, store, uploads) = fixture();
    let session_id = "s.a.8";
    uploads
        .begin(&store, session_id, "up-12", "gone.bin", 8)
        .expect("open");
    uploads
        .chunk(session_id, "up-12", 0, &encoded(b"abcd"))
        .expect("half");
    assert_eq!(part_files(&store, session_id).len(), 1);
    uploads.abort(session_id, "up-12").expect("abort");
    assert!(part_files(&store, session_id).is_empty());
    assert!(uploads.status(session_id, "up-12").is_err());
    // An abort of an id the daemon no longer holds is an `Ok`: there is
    // nothing left to discard.
    uploads.abort(session_id, "up-12").expect("abort twice");

    uploads
        .begin(&store, session_id, "up-13", "gone2.bin", 8)
        .expect("open again");
    uploads.forget_session(session_id);
    assert!(uploads.status(session_id, "up-13").is_err());
    assert_eq!(
        store.remove_session(session_id),
        Some(0),
        "the part file's folder"
    );
    assert!(!temp.0.join("attachments").join(session_id).exists());
}

#[test]
fn a_chunk_for_another_session_is_refused() {
    let (_temp, store, uploads) = fixture();
    let session_id = "s.a.9";
    uploads
        .begin(&store, session_id, "up-14", "mine.bin", 4)
        .expect("open");
    let error = uploads
        .chunk("s.a.10", "up-14", 0, &encoded(b"abcd"))
        .expect_err("another session's frame");
    assert!(error.message.contains("start it again"), "{error:?}");
    assert_eq!(uploads.status(session_id, "up-14").expect("untouched"), 0);
}
