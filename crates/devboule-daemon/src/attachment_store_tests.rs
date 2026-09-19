//! Tests for the attachment store: deposit, resolution, retention and the read door.

use super::*;
use crate::raster_metadata::{clean_png, png_with_text_chunk, vector_input, vector_output};
use devboule_protocol::ATTACHMENT_MIME_TYPES;

struct TempDir(PathBuf);

impl TempDir {
    /// The store hardens every session folder it creates, and a hardened
    /// folder is one `Drop` below cannot remove — so these directories
    /// outlive the run that made them. A pid and a counter that restarts at 1
    /// name the same one again once Windows recycles that pid: then
    /// `create_dir_all` succeeds on the inherited folder and the first write
    /// into it fails with `Access is denied`, in whatever test drew the short
    /// straw. Name it with the clock so no run can inherit another's.
    fn new() -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|since| since.as_nanos())
            .unwrap_or_default();
        let dir = std::env::temp_dir().join(format!(
            "devboule-attachments-{}-{nonce}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        Self(dir)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn attachment(name: &str, mime_type: &str, data: &str) -> PromptAttachment {
    PromptAttachment {
        name: name.to_string(),
        mime_type: mime_type.to_string(),
        data: data.to_string(),
    }
}

fn encoded(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[test]
fn the_file_name_is_the_digest_of_the_bytes() {
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let session = store.session("s.a.1").expect("session");
    // A container the walk accepts and changes nothing in, so the digest is
    // over exactly these bytes.
    let bytes = clean_png(0x01);

    let path = session
        .materialize(&attachment("photo.png", "image/png", &encoded(&bytes)))
        .expect("materialized");

    assert_eq!(
        path.file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string),
        Some(format!("{}.png", sha256_hex(&bytes)))
    );
    assert_eq!(std::fs::read(&path).expect("read"), bytes);
    assert!(path.starts_with(&temp.0));
}

#[test]
fn the_users_name_never_reaches_the_path() {
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let session = store.session("s.a.1").expect("session");
    let bytes = clean_png(0x02);

    let path = session
        .materialize(&attachment(
            r"..\..\evil.png",
            "image/png",
            &encoded(&bytes),
        ))
        .expect("materialized");

    assert_eq!(path.parent(), Some(session.dir.as_path()));
    assert!(!path.to_string_lossy().contains("evil"));
    let siblings: Vec<_> = std::fs::read_dir(&temp.0)
        .expect("root list")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(siblings, vec!["attachments".to_string()]);
}

#[test]
fn identical_bytes_are_stored_once() {
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let session = store.session("s.a.1").expect("session");
    let image = clean_png(0x03);
    let first = attachment("one.png", "image/png", &encoded(&image));
    let second = attachment("two.png", "image/png", &encoded(&image));

    let first_path = session.materialize(&first).expect("first");
    let second_path = session.materialize(&second).expect("second");

    assert_eq!(first_path, second_path);
    let files: Vec<_> = std::fs::read_dir(first_path.parent().expect("parent"))
        .expect("session list")
        .flatten()
        .collect();
    assert_eq!(files.len(), 1, "the same image left a second file behind");
}

#[test]
fn different_bytes_get_different_files() {
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let session = store.session("s.a.1").expect("session");

    let first = session
        .materialize(&attachment(
            "a.png",
            "image/png",
            &encoded(&clean_png(0x04)),
        ))
        .expect("first");
    let second = session
        .materialize(&attachment(
            "b.png",
            "image/png",
            &encoded(&clean_png(0x05)),
        ))
        .expect("second");

    assert_ne!(first, second);
}

#[test]
fn a_stripped_file_is_named_after_the_bytes_that_were_written() {
    // The consequence this wiring carries, stated in the doc comment on
    // `materialize`: the name is the digest of what is on disk, not of what
    // arrived, so a client that predicts the path from its own digest is
    // wrong whenever a rule fired.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let session = store.session("s.a.1").expect("session");
    let sent = png_with_text_chunk();
    let kept = clean_png(0x01);
    assert_ne!(
        sha256_hex(&sent),
        sha256_hex(&kept),
        "the fixture must actually carry something that leaves"
    );

    let path = session
        .materialize(&attachment("photo.png", "image/png", &encoded(&sent)))
        .expect("materialized");

    assert_eq!(
        path.file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string),
        Some(format!("{}.png", sha256_hex(&kept)))
    );
    assert_eq!(std::fs::read(&path).expect("read"), kept);
    assert!(
        !path.to_string_lossy().contains(sha256_hex(&sent).as_str()),
        "the digest of the bytes the client sent is not the path"
    );
}

#[test]
fn an_image_the_walk_cannot_follow_is_refused_rather_than_written() {
    // Real PNG bytes, cut short inside the last chunk. The sniff agrees with
    // the label here, so this reaches the walk and fails there rather than
    // at the disagreement check below — which is the path this test is
    // about, and the one the fixture used to reach before the sniff existed.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let session = store.session("s.a.1").expect("session");
    let mut truncated = clean_png(0x13);
    truncated.truncate(truncated.len() - 6);

    let item = attachment("photo.png", "image/png", &encoded(&truncated));
    let error = session.materialize(&item).expect_err("refused");

    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(
        error.message.contains("could not be read"),
        "{}",
        error.message
    );
    assert!(!session.dir.exists(), "nothing may be created on a refusal");
}

/// The vector the JPEG-side tests use: real bytes carrying EXIF, taken from
/// the shared file rather than hand-rolled, so a test that asserts about
/// stripping is asserting about the same bytes the rule is pinned to.
const EXIF_JPEG_VECTOR: &str =
    "a jpeg whose APP1 holds EXIF, between a kept JFIF APP0 and a kept ICC APP2";

#[test]
fn a_jpeg_carrying_exif_declared_as_svg_is_refused() {
    // The regression test for the bypass. The strip used to be selected by
    // `mime_type` — a field the sender controls — so this exact file, a JPEG
    // whose APP1 holds GPS coordinates, took the `image/svg+xml`
    // write-through path and was stored untouched. Nothing may be written:
    // the bytes were never sanitised, and a path to them is a promise the
    // daemon cannot keep.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let session = store.session("s.a.1").expect("session");
    let jpeg = vector_input(EXIF_JPEG_VECTOR);

    let item = attachment("photo.jpg", "image/svg+xml", &encoded(&jpeg));
    let error = session.materialize(&item).expect_err("refused");

    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(
        error.message.contains("do not match the type"),
        "{}",
        error.message
    );
    assert!(!session.dir.exists(), "nothing may be created on a refusal");
}

#[test]
fn a_png_declared_as_a_jpeg_is_refused() {
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let session = store.session("s.a.1").expect("session");

    let item = attachment("photo.png", "image/jpeg", &encoded(&clean_png(0x14)));
    let error = session.materialize(&item).expect_err("refused");

    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(!session.dir.exists(), "nothing may be created on a refusal");
}

#[test]
fn a_declared_raster_whose_bytes_are_not_a_container_is_refused() {
    // The label says PNG and the bytes say nothing recognisable, so there is
    // no walk to run and no evidence to check the label against. A file that
    // cannot be walked is refused rather than stored, which is the rule the
    // whole pass rests on.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let session = store.session("s.a.1").expect("session");

    let item = attachment("photo.png", "image/png", &encoded(b"not a png at all"));
    let error = session.materialize(&item).expect_err("refused");

    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(!session.dir.exists(), "nothing may be created on a refusal");
}

#[test]
fn a_correctly_declared_jpeg_still_strips() {
    // The counterweight to the refusals: when the label and the bytes agree,
    // the walk runs and what lands on disk is the vector's authored output,
    // named after it. Without this, a "fix" that refused everything would
    // look like it passed the tests above.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let session = store.session("s.a.1").expect("session");
    let sent = vector_input(EXIF_JPEG_VECTOR);
    let kept = vector_output(EXIF_JPEG_VECTOR);
    assert_ne!(
        sha256_hex(&sent),
        sha256_hex(&kept),
        "the vector must actually lose something"
    );

    let item = attachment("photo.jpg", "image/jpeg", &encoded(&sent));
    let path = session.materialize(&item).expect("materialized");

    assert_eq!(
        path.file_name()
            .and_then(|name| name.to_str())
            .map(str::to_string),
        Some(format!("{}.jpg", sha256_hex(&kept)))
    );
    assert_eq!(std::fs::read(&path).expect("read"), kept);
}

#[test]
fn an_svg_is_written_as_it_arrived() {
    // The frontend sanitises SVG source and this side does not, so an SVG
    // that arrives from another device is stored unsanitised. Pinning that
    // keeps the gap visible: if this ever fails because the bytes changed,
    // a sanitiser was added and the doc comment on `materialize` is wrong.
    //
    // These bytes are neither container, which is what makes this the one
    // remaining write-through path: nothing here is examined, so the label
    // is the only thing that decides what is written — including its
    // extension.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let session = store.session("s.a.1").expect("session");
    let sent = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>";

    let path = session
        .materialize(&attachment("drawing.svg", "image/svg+xml", &encoded(sent)))
        .expect("materialized");

    assert_eq!(std::fs::read(&path).expect("read"), sent.to_vec());
}

#[test]
fn every_supported_type_has_an_extension() {
    // Pins the store's table to the wire's list: a type that validates but
    // has no extension would be refused after the validator accepted it.
    for mime_type in ATTACHMENT_MIME_TYPES {
        assert!(
            extension_for(mime_type).is_some(),
            "{mime_type} is accepted on the wire but has no extension"
        );
    }
}

#[test]
fn every_stored_extension_names_its_mime_type() {
    // The read answer states the file's type from the store's table, so
    // the reverse map has to cover every extension the store writes —
    // and round-trip back to the type the forward map names.
    for extension in STORED_EXTENSIONS {
        let mime_type = mime_type_for_extension(extension).expect("a mime type");
        assert_eq!(
            extension_for(mime_type),
            Some(extension),
            "{extension} must round-trip through the type table"
        );
    }
    assert_eq!(mime_type_for_extension("tmp"), None);
    assert_eq!(mime_type_for_extension("bak"), None);
}

#[test]
fn a_session_id_that_is_a_parent_directory_is_refused() {
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    assert!(store.session("..").is_none());
    assert!(store.session(".").is_none());
    assert_eq!(
        store.remove_session(".."),
        Some(0),
        "an id with no folder dropped nothing"
    );
    assert!(temp.0.exists(), "the store must not walk out of its root");
}

#[test]
fn invalid_base64_is_refused_rather_than_written() {
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let session = store.session("s.a.1").expect("session");
    let error = session
        .materialize(&attachment("a.png", "image/png", "not base64!"))
        .expect_err("refused");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(error.message, invalid_base64_message());
    assert!(!session.dir.exists(), "nothing may be created on a refusal");
}

#[test]
fn an_unsupported_type_is_refused_rather_than_named_png() {
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let session = store.session("s.a.1").expect("session");
    let error = session
        .materialize(&attachment("a.gif", "image/gif", &encoded(b"gif")))
        .expect_err("refused");
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(error.message.contains("image/gif"), "{}", error.message);
}

#[test]
fn retention_deletes_a_folder_older_than_the_limit() {
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let session = store.session("s.a.1").expect("session");
    let path = session
        .materialize(&attachment(
            "a.png",
            "image/png",
            &encoded(&clean_png(0x06)),
        ))
        .expect("materialized");

    let later = SystemTime::now() + ATTACHMENT_RETENTION + Duration::from_secs(60);
    let reclaimed = store.sweep_older_than(later, ATTACHMENT_RETENTION);
    assert_eq!(reclaimed.len(), 1, "one folder was past the limit");
    assert_eq!(reclaimed[0].0, "s.a.1", "the report names the session");
    assert_eq!(
        reclaimed[0].1,
        Some(clean_png(0x06).len() as u64),
        "and what its removal took out of the total"
    );
    assert!(!path.exists(), "the file must go with its folder");
    assert!(!session.dir.exists());
}

#[test]
fn retention_keeps_a_folder_inside_the_limit() {
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let session = store.session("s.a.1").expect("session");
    let path = session
        .materialize(&attachment(
            "a.png",
            "image/png",
            &encoded(&clean_png(0x07)),
        ))
        .expect("materialized");

    let soon = SystemTime::now() + Duration::from_secs(60);
    assert!(store
        .sweep_older_than(soon, ATTACHMENT_RETENTION)
        .is_empty());
    assert!(path.exists());
}

#[test]
fn retention_keeps_a_folder_whose_timestamp_is_in_the_future() {
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let session = store.session("s.a.1").expect("session");
    let path = session
        .materialize(&attachment(
            "a.png",
            "image/png",
            &encoded(&clean_png(0x08)),
        ))
        .expect("materialized");
    let file = std::fs::File::options()
        .write(true)
        .open(&path)
        .expect("open");
    file.set_modified(SystemTime::now() + Duration::from_secs(86_400))
        .expect("set mtime");

    assert!(store
        .sweep_older_than(SystemTime::now(), ATTACHMENT_RETENTION)
        .is_empty());
    assert!(path.exists());
}

#[test]
fn sweeping_a_store_that_does_not_exist_yet_is_not_an_error() {
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0.join("absent"));
    assert!(store
        .sweep_older_than(SystemTime::now(), ATTACHMENT_RETENTION)
        .is_empty());
}

#[test]
fn closing_a_session_removes_only_that_session() {
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let kept = store.session("s.a.2").expect("session");
    let kept_path = kept
        .materialize(&attachment(
            "b.png",
            "image/png",
            &encoded(&clean_png(0x09)),
        ))
        .expect("materialized");
    store
        .session("s.a.1")
        .expect("session")
        .materialize(&attachment(
            "a.png",
            "image/png",
            &encoded(&clean_png(0x0a)),
        ))
        .expect("materialized");

    store.remove_session("s.a.1");

    assert!(!store.session("s.a.1").expect("session").dir.exists());
    assert!(kept_path.exists());
}

#[test]
fn an_under_lock_recheck_keeps_a_folder_that_became_fresh() {
    // The sweep decides on age without the lock and deletes under it, and
    // the decision is made again there. This pins the second decision: a
    // folder that was old enough when the unlocked filter ran, and had a
    // write land before the lock, is kept — deleting it would lose an
    // attachment the user made moments ago.
    //
    // The interleaving with a real writer is not reproduced, and cannot be
    // pinned here: the only synchronization point between the unlocked
    // filter and the removal is the lock wait itself, which std offers no
    // way to observe. A second thread told to "materialize while the sweep
    // is parked" would have to be held back by a sleep, which is a guess
    // rather than an assertion. What is pinned is the rule the removal
    // runs under the lock.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let session = store.session("s.a.1").expect("session");
    let path = session
        .materialize(&attachment(
            "a.png",
            "image/png",
            &encoded(&clean_png(0x0d)),
        ))
        .expect("materialized");

    // `now` is a parameter of the sweep, so the folder can be made old by
    // moving `now` forward instead of moving file timestamps back.
    let now = SystemTime::now();
    let sweep_now = now + ATTACHMENT_RETENTION + Duration::from_secs(120);

    // What the unlocked filter sees: a candidate.
    assert_eq!(
        is_older_than(&session.dir, sweep_now, ATTACHMENT_RETENTION),
        Some(true)
    );

    // A write lands in the folder while the sweep is on its way to the
    // lock, so its newest write moves in front of the limit.
    let file = std::fs::File::options()
        .write(true)
        .open(&path)
        .expect("open");
    file.set_modified(sweep_now - Duration::from_secs(30))
        .expect("set mtime");
    assert_eq!(
        is_older_than(&session.dir, sweep_now, ATTACHMENT_RETENTION),
        Some(false)
    );

    assert!(
        store
            .remove_if_still_older_than(&session.dir, sweep_now, ATTACHMENT_RETENTION)
            .is_none(),
        "a folder that is no longer old must not be removed under the lock"
    );
    assert!(path.exists(), "the write's folder must survive");
}

#[test]
fn an_under_lock_recheck_keeps_a_path_it_cannot_read() {
    // An unreadable folder is not an empty one. When `newest_write` cannot
    // read the path — here a file where a folder is expected, the portable
    // stand-in for any path that cannot be listed — the answer is "cannot
    // tell", and the removal refuses instead of deleting on a guess.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let stray = temp.0.join("not-a-folder");
    std::fs::write(&stray, b"x").expect("write");

    let now = SystemTime::now() + ATTACHMENT_RETENTION + Duration::from_secs(120);
    assert_eq!(is_older_than(&stray, now, ATTACHMENT_RETENTION), None);
    assert!(
        store
            .remove_if_still_older_than(&stray, now, ATTACHMENT_RETENTION)
            .is_none(),
        "a path whose age cannot be read must not be removed"
    );
    assert!(stray.exists(), "nothing may be deleted on a guess");
}

/// How long the tests below give a deleter that should be blocked before
/// they call it blocked. The value is generous on purpose: under the fix
/// the deleter cannot proceed while this thread holds the lock, so no
/// timeout can make the assertion fail, and a build without the lock
/// answers as soon as its thread is scheduled. See the comment on
/// `closing_a_session_waits_for_an_in_flight_write` for why the wait is a
/// timeout and not a sleep.
const BLOCKED_TIMEOUT: Duration = Duration::from_millis(500);

#[test]
fn closing_a_session_waits_for_an_in_flight_write() {
    // The refusal to race is asserted as serialization: hold the write
    // lock, ask another thread to close the session, and require that the
    // close does not finish while the lock is held. The wait is a channel
    // timeout, which is the shape of the statement and not a sleep — a
    // fixed build cannot satisfy `recv_timeout` here because the deleter
    // is parked on a mutex, while a build missing the lock sends `done` as
    // soon as the thread runs. The `started` message makes the assertion
    // about the close itself rather than about thread scheduling.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let session = store.session("s.a.1").expect("session");
    let path = session
        .materialize(&attachment(
            "a.png",
            "image/png",
            &encoded(&clean_png(0x0b)),
        ))
        .expect("materialized");

    let guard = store
        .write_lock
        .lock()
        .unwrap_or_else(|error| error.into_inner());

    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let closer = {
        let store = store.clone();
        std::thread::spawn(move || {
            started_tx.send(()).expect("signal start");
            store.remove_session("s.a.1");
            done_tx.send(()).expect("signal done");
        })
    };

    started_rx.recv().expect("the closer started");
    assert!(
        done_rx.recv_timeout(BLOCKED_TIMEOUT).is_err(),
        "the close finished while a writer held the lock"
    );
    assert!(path.exists(), "the folder was removed under the lock");

    drop(guard);
    done_rx
        .recv()
        .expect("the close finished once the lock was free");
    closer.join().expect("thread");
    assert!(!path.exists(), "the close must still remove the folder");
}

#[test]
fn a_retention_sweep_waits_for_an_in_flight_write() {
    // The sweep takes the lock per removal. The observable half of that is
    // the same serialization the close shows: while this thread holds the
    // lock, a sweep that has decided to delete the folder cannot finish.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let session = store.session("s.a.1").expect("session");
    let path = session
        .materialize(&attachment(
            "a.png",
            "image/png",
            &encoded(&clean_png(0x0c)),
        ))
        .expect("materialized");

    let guard = store
        .write_lock
        .lock()
        .unwrap_or_else(|error| error.into_inner());

    let (started_tx, started_rx) = std::sync::mpsc::channel();
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let sweeper = {
        let store = store.clone();
        std::thread::spawn(move || {
            started_tx.send(()).expect("signal start");
            let later = SystemTime::now() + ATTACHMENT_RETENTION + Duration::from_secs(60);
            let removed = store.sweep_older_than(later, ATTACHMENT_RETENTION).len();
            done_tx.send(removed).expect("signal done");
        })
    };

    started_rx.recv().expect("the sweeper started");
    assert!(
        done_rx.recv_timeout(BLOCKED_TIMEOUT).is_err(),
        "the sweep finished while a writer held the lock"
    );
    assert!(path.exists(), "the folder was removed under the lock");

    drop(guard);
    assert_eq!(
        done_rx
            .recv()
            .expect("the sweep finished once the lock was free"),
        1,
        "the sweep still reports the folder it removed"
    );
    sweeper.join().expect("thread");
    assert!(!path.exists(), "the sweep must still remove the folder");
}

#[test]
fn the_fingerprint_digest_distinguishes_two_images_with_one_name() {
    let first = attachment("same.png", "image/png", &encoded(b"first image"));
    let second = attachment("same.png", "image/png", &encoded(b"second image"));
    assert_ne!(attachment_digest(&first), attachment_digest(&second));
    assert_eq!(attachment_digest(&first), attachment_digest(&first));
    assert_eq!(attachment_digest(&first).len(), 64);
}

#[test]
fn depositing_the_same_bytes_twice_in_one_session_counts_once() {
    // The within-session half of content addressing. The second deposit
    // finds the digest's file already there, so it creates no file and must
    // move no total: an increment charged beside the budget check rather
    // than after the write passes every other test in this file and fails
    // this one.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let bytes = clean_png(0x21);
    let item = attachment("photo.png", "image/png", &encoded(&bytes));

    let first = store.deposit("s.a.1", &item).expect("first deposit");
    let second = store.deposit("s.a.1", &item).expect("second deposit");

    assert_eq!(first.digest, second.digest);
    assert_eq!(first.path, second.path);
    assert_eq!(first.stored_bytes, bytes.len() as u64);
    assert_eq!(
        store.store_bytes(),
        Some(bytes.len() as u64),
        "one file, one increment"
    );
    assert_eq!(
        std::fs::read_dir(first.path.parent().expect("session folder"))
            .expect("read the session folder")
            .flatten()
            .count(),
        1,
        "the second deposit left a second file behind"
    );
}

#[test]
fn the_same_bytes_in_two_sessions_count_twice() {
    // The case an auditor got wrong. Content addressing is scoped to one
    // session folder, so the second session's digest names a second file and
    // a second contribution to the store's total. A cache keyed by digest
    // alone — or a lookup that searched outside the session's folder — would
    // report one copy here, and the store would be allowed twice the bytes
    // the limit is for.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let bytes = clean_png(0x22);
    let item = attachment("photo.png", "image/png", &encoded(&bytes));

    let first = store.deposit("s.a.1", &item).expect("first session");
    let second = store.deposit("s.a.2", &item).expect("second session");

    assert_eq!(first.digest, second.digest, "one image, one digest");
    assert_ne!(first.path, second.path, "two sessions, two files");
    assert!(second.path.exists());
    assert_eq!(store.store_bytes(), Some(2 * bytes.len() as u64));
    // Per session as well as in total, because the caller that reserves
    // bytes against a device attributes them one session at a time.
    assert_eq!(store.session_bytes("s.a.1"), Some(bytes.len() as u64));
    assert_eq!(store.session_bytes("s.a.2"), Some(bytes.len() as u64));
}

#[test]
fn closing_a_session_drops_only_its_own_bytes_from_the_store_total() {
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let kept = store
        .deposit(
            "s.a.2",
            &attachment("kept.png", "image/png", &encoded(&clean_png(0x23))),
        )
        .expect("kept");
    let gone = store
        .deposit(
            "s.a.1",
            &attachment("gone.png", "image/png", &encoded(&clean_png(0x24))),
        )
        .expect("gone");
    assert_eq!(
        store.store_bytes(),
        Some(kept.stored_bytes + gone.stored_bytes)
    );

    assert_eq!(
        store.remove_session("s.a.1"),
        Some(gone.stored_bytes),
        "the close reports what it dropped"
    );

    assert_eq!(
        store.store_bytes(),
        Some(kept.stored_bytes),
        "the close must drop exactly one session's bytes"
    );
    assert!(kept.path.exists(), "the sibling session's file was removed");
    assert!(!gone.path.exists());
}

#[test]
fn an_over_budget_deposit_writes_nothing() {
    // The budget is read from the tree, so the cheapest fixture that fills
    // it is a folder this process never wrote: the state D4 calls "a folder
    // present at startup", where the walk is the only thing that knows the
    // bytes are there. That assertion doubles as the proof that the walk
    // counts what it finds rather than only what a deposit recorded.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let full = store.session("s.a.1").expect("session");
    std::fs::create_dir_all(&full.dir).expect("session folder");
    std::fs::write(
        full.dir.join("photo.png"),
        vec![0u8; MAX_ATTACHMENT_OWNER_BYTES],
    )
    .expect("fill the store's budget");

    assert_eq!(
        store.store_bytes(),
        Some(MAX_ATTACHMENT_OWNER_BYTES as u64),
        "the walk must count a folder this process did not write"
    );

    let bytes = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>";
    let item = attachment("drawing.svg", "image/svg+xml", &encoded(bytes));
    let error = store.deposit("s.a.2", &item).expect_err("refused");

    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(
        error
            .message
            .contains(&MAX_ATTACHMENT_OWNER_BYTES.to_string()),
        "the refusal must name the limit it refused against: {}",
        error.message
    );
    assert!(
        !store.session("s.a.2").expect("session").dir.exists(),
        "nothing may be created on a refusal"
    );

    // The refusal is the budget and not the shape of the request: once the
    // folder is gone the same bytes are accepted, which is the close
    // dropping exactly one session's contribution.
    assert_eq!(
        store.remove_session("s.a.1"),
        Some(MAX_ATTACHMENT_OWNER_BYTES as u64),
        "the close reports the bytes the filled folder held"
    );
    assert_eq!(store.store_bytes(), Some(0));
    let accepted = store.deposit("s.a.2", &item).expect("accepted");
    assert!(accepted.path.exists());
    assert_eq!(accepted.stored_bytes, bytes.len() as u64);
    assert_eq!(store.store_bytes(), Some(bytes.len() as u64));
}

#[test]
fn a_deposited_markdown_artifact_resolves_by_digest() {
    // Three tables have to agree, and this proves it from outside the store:
    // `ATTACHMENT_MIME_TYPES` admits the type, `extension_for` names the
    // file, and `STORED_EXTENSIONS` is what `find_stored` reads when it
    // starts from the disk. `text/markdown` was added to the first two and
    // not the third, which left the finish report's artifact deposited,
    // charged to the budget, and impossible to resolve — the store's own
    // definition of waste (`discard_scratch`).
    //
    // Deposit and then resolve, never assert the table's length: a count
    // would have been green with the bug in place.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let report = b"# What the child did\n\nOne paragraph, and a fenced block.\n";
    let item = attachment("report.md", "text/markdown", &encoded(report));

    let stored = store.deposit("s.a.1", &item).expect("markdown deposits");

    // No hint is the path the send-side resolution takes — `session.rs`
    // resolves a reference with `None`, because a reference carries a digest
    // and a size and no MIME type — so it is the one that matters most.
    let (path, bytes) = store
        .resolve("s.a.1", &stored.digest, None)
        .expect("the listing finds a stored markdown file");
    assert_eq!(path, stored.path);
    assert_eq!(bytes, stored.stored_bytes);

    // Both spellings of a hint, because they fail separately: the MIME type
    // resolved even with the bug (`extension_for` knew it), the bare
    // extension did not (`STORED_EXTENSIONS` did not).
    for hint in ["text/markdown", "md"] {
        let (hinted, _) = store
            .resolve("s.a.1", &stored.digest, Some(hint))
            .unwrap_or_else(|error| panic!("hint {hint:?} did not resolve: {error:?}"));
        assert_eq!(hinted, stored.path, "hint {hint:?}");
    }

    // And it is not scratch. A later write into the same folder runs
    // `discard_scratch`, whose doc reasons about what "no `resolve` can ever
    // name" — a stored `.md` was exactly that while the table disagreed, so
    // the survivor is asserted rather than assumed.
    store
        .deposit(
            "s.a.1",
            &attachment("photo.png", "image/png", &encoded(&clean_png(0x33))),
        )
        .expect("a second deposit into the same folder");
    assert!(
        store.resolve("s.a.1", &stored.digest, None).is_ok(),
        "the markdown artifact did not survive a later write into its folder"
    );
}

#[test]
fn a_digest_that_is_not_a_digest_is_refused() {
    // The store is the second door. The wire checks a reference's digest
    // before anything here is reached, and this must not depend on that
    // check having run: every string below would name something once it were
    // joined to a session folder, and none of them may reach the filesystem
    // at all.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let refusals = [
        // A traversal, at the length the check above the join expects.
        "../".repeat(21) + "a",
        // Right length, wrong characters: not hex, and uppercase hex.
        "z".repeat(64),
        "A".repeat(64),
        // Right characters, wrong length: one short, one long.
        "a".repeat(63),
        "a".repeat(65),
        // A separator inside a string that is otherwise a digest.
        format!("/{}", "a".repeat(63)),
    ];

    for digest in &refusals {
        let error = store
            .resolve("s.a.1", digest, Some("png"))
            .expect_err("refused");
        assert_eq!(error.code, ErrorCode::InvalidRequest, "{digest}");
        assert_eq!(
            error.message,
            invalid_attachment_digest_message(),
            "{digest}"
        );
    }

    // Nothing was looked up, so nothing was created: not the session folder,
    // and not anything a traversal would have climbed to from it.
    assert!(
        !temp.0.join(ATTACHMENTS_DIR).exists(),
        "a refusal may not create the path it refused"
    );

    // A well-formed digest under a session id the store will not turn into
    // a folder is refused as well, before a name is built from either.
    assert!(store.resolve("..", &"a".repeat(64), None).is_err());
}

#[test]
fn a_digest_with_no_file_behind_it_is_refused() {
    // A refusal and not an empty success: a missing file is not a stored
    // attachment of zero bytes, and a caller handed `Ok` with a path would
    // find that out at the provider instead, one layer further from the
    // cause than the request that named it.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let absent = sha256_hex(b"never deposited");

    let error = store
        .resolve("s.a.1", &absent, Some("png"))
        .expect_err("refused");

    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_ne!(
        error.message,
        invalid_attachment_digest_message(),
        "the digest is well formed; the file is what is missing"
    );
}

#[test]
fn a_resolved_file_reports_the_size_the_store_wrote() {
    // The size a resolve hands back is read from the file, so it is the
    // stored size and not the size that was sent: this fixture carries a text
    // chunk the strip removes, which is exactly the disagreement D2 turns
    // into a refusal when the client's own number is the one that travelled.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let sent = png_with_text_chunk();
    let kept = clean_png(0x01);
    let deposited = store
        .deposit(
            "s.a.1",
            &attachment("photo.png", "image/png", &encoded(&sent)),
        )
        .expect("deposited");

    assert_eq!(deposited.stored_bytes, kept.len() as u64);
    assert_ne!(
        deposited.stored_bytes,
        sent.len() as u64,
        "the fixture must lose bytes to the strip"
    );

    let (path, stored_bytes) = store
        .resolve("s.a.1", &deposited.digest, Some("png"))
        .expect("resolved");
    assert_eq!(path, deposited.path);
    assert_eq!(stored_bytes, kept.len() as u64);
    assert_eq!(
        stored_bytes,
        std::fs::metadata(&path).expect("metadata").len(),
        "the size must be the file's own"
    );

    // The hint is a shortcut and not the answer: with none, the folder's
    // listing finds the same file, which is the path a caller holding only a
    // session and a digest takes.
    let (listed, listed_bytes) = store
        .resolve("s.a.1", &deposited.digest, None)
        .expect("resolved without a hint");
    assert_eq!(listed, path);
    assert_eq!(listed_bytes, stored_bytes);

    // A hint for the wrong type falls back to the listing rather than
    // refusing, or answering about a file that is not there.
    let (wrong_hint, _) = store
        .resolve("s.a.1", &deposited.digest, Some("image/svg+xml"))
        .expect("resolved with the wrong hint");
    assert_eq!(wrong_hint, path);

    // A digest resolves only in the session it was deposited to, so the same
    // digest asked about from another session is a refusal.
    assert!(store
        .resolve("s.a.2", &deposited.digest, Some("png"))
        .is_err());
}

#[test]
fn the_inline_path_writes_for_a_legacy_id_and_counts_the_bytes() {
    // The M2 form has no middle segment to read, and nothing reads one any
    // more. The inline path keeps writing for it, and its bytes are counted
    // like any other file's: the write goes through `write_locked`, which
    // charges the folder it landed in. Before that, an inline attachment was
    // bytes the store held that no number anywhere knew about.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let session = store.session("session-123-1").expect("session");
    let bytes = clean_png(0x35);
    let path = session
        .materialize(&attachment("photo.png", "image/png", &encoded(&bytes)))
        .expect("materialized");

    assert!(path.exists(), "the inline path must still store the file");
    assert_eq!(
        store.session_bytes("session-123-1"),
        Some(bytes.len() as u64),
        "the file the inline path wrote is counted"
    );
    assert_eq!(
        store.store_bytes(),
        Some(bytes.len() as u64),
        "and it is part of the store's total"
    );
}

#[test]
fn a_folder_that_cannot_be_listed_has_no_size() {
    // A file where a folder is expected is this file's stand-in for a path
    // that cannot be listed (`an_under_lock_recheck_keeps_a_path_it_cannot_read`),
    // and it is a real failure rather than an injected one. The point is the
    // difference between "could not read" and "nothing there": both are a
    // failed listing, and only one of them is a zero.
    let temp = TempDir::new();
    let stray = temp.0.join("not-a-folder");
    std::fs::write(&stray, b"x").expect("write");

    assert_eq!(
        folder_bytes(&stray),
        None,
        "a listing that failed is not an empty folder"
    );
    assert_eq!(
        folder_bytes(&temp.0.join("absent")),
        Some(0),
        "a folder that is gone holds nothing, which is a number"
    );
}

#[test]
fn a_root_that_cannot_be_listed_makes_every_total_unknown() {
    // The same failure as the folder above, one level up — and this one is
    // produced for real rather than injected: `attachments` exists and is not
    // a folder, so the walk cannot learn which sessions exist and no total is
    // knowable. Every budget question answers unknown, which is the honest
    // answer when a store cannot read its own root, and the deposit is
    // refused before it writes, so an unreadable root does not become the
    // hole instead.
    let temp = TempDir::new();
    let state_file = temp.0.join(ATTACHMENTS_DIR);
    std::fs::write(&state_file, b"not a folder").expect("write");

    let store = AttachmentStore::new(&temp.0);
    assert_eq!(
        store.store_bytes(),
        None,
        "a root that cannot be listed is not an empty store"
    );
    let error = store
        .deposit(
            "s.a.1",
            &attachment("photo.png", "image/png", &encoded(&clean_png(0x36))),
        )
        .expect_err("refused");
    assert_eq!(error.code, ErrorCode::Io);
    assert!(
        error.message.contains("could not be read"),
        "the refusal must name the real cause: {}",
        error.message
    );
    assert_eq!(
        std::fs::read(&state_file).expect("read"),
        b"not a folder".to_vec(),
        "nothing may be written where the store root belongs"
    );
}

/// Put one session into the state a folder the walk could not read leaves it
/// in, without needing an unreadable folder to exist.
///
/// [`SessionBytes::Unknown`] is reachable only from a `read_dir` that failed,
/// and there is no portable way to make one fail in a test: on Windows it
/// takes an ACL or a held exclusive handle on the directory, neither of which
/// a test may assume it is allowed to create, and on a POSIX box a
/// `chmod 000` on the folder does nothing when the suite runs as root. So the
/// test writes the state the walk would have reached — through the store's own
/// lock, after a first seed, so every later `seed_locked` is a no-op and the
/// entry stands — and asserts the consequence, which is the part the store is
/// responsible for. What this does *not* pin is `seed_locked` declining to set
/// `seeded`: that needs the real failure, and the comment above the assertion
/// in `an_unknown_folder_refuses_every_deposit` says so.
fn make_unknown(store: &AttachmentStore, session_id: &str) {
    let mut state = store
        .write_lock
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    AttachmentStore::seed_locked(&store.root, &mut state);
    state
        .sessions
        .insert(session_id.to_string(), SessionBytes::Unknown);
}

/// Clear `seeded`, which is the state a walk that could not read a folder
/// leaves behind (`seed_locked`): the next budget question walks again and
/// rebuilds the map from the tree.
///
/// Same caveat as `make_unknown` — the real state comes from a failed
/// `read_dir`, which a test cannot produce portably — so the test sets the
/// flag the walk would have left and asserts what the store does with it.
fn make_unseeded(store: &AttachmentStore) {
    let mut state = store
        .write_lock
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    state.seeded = false;
}

#[test]
fn an_unknown_folder_refuses_every_deposit() {
    // One folder the walk could not read makes the total unknown, and an
    // unknown total is a refusal rather than a number rounded down: the bytes
    // in that folder are exactly the budget a zero would hand back. The
    // refusal is store-wide because the budget is — every deposit is checked
    // against the same number, so while that number is unknowable every
    // deposit is refused, including deposits into sessions whose own folders
    // read perfectly well. That is what one budget instead of one per
    // connection costs, and the rebuilt picture is what lifts it.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let balanced = store
        .deposit(
            "s.b.1",
            &attachment("b.png", "image/png", &encoded(&clean_png(0x31))),
        )
        .expect("an unrelated session's deposit");

    make_unknown(&store, "s.a.1");

    assert_eq!(
        store.store_bytes(),
        None,
        "an unreadable folder is not a zero"
    );
    for session_id in ["s.a.1", "s.b.2"] {
        let error = store
            .deposit(
                session_id,
                &attachment("a.png", "image/png", &encoded(&clean_png(0x32))),
            )
            .expect_err("refused");
        assert_eq!(error.code, ErrorCode::Io, "{session_id}");
        assert!(
            error.message.contains("could not be read"),
            "the refusal must name the real cause: {}",
            error.message
        );
        assert!(
            !store.session(session_id).expect("session").dir.exists(),
            "nothing may be created on a refusal: {session_id}"
        );
    }

    // And it lifts the way the design says it does: a picture the walk has
    // rebuilt has no unknown in it, and what was refused is accepted.
    make_unseeded(&store);
    assert_eq!(store.store_bytes(), Some(balanced.stored_bytes));
    let accepted = store
        .deposit(
            "s.a.1",
            &attachment("a.png", "image/png", &encoded(&clean_png(0x32))),
        )
        .expect("accepted once the picture is rebuilt");
    assert!(accepted.path.exists());
}

#[test]
fn two_sessions_with_unrelated_id_shapes_count_toward_one_total() {
    // Why there is no key. One id carries a middle segment that looks like an
    // owner and is a connection token; another is the M2 form, which has no
    // middle segment at all. Both are folders in one store, and both count
    // against one budget: a total keyed on that segment would have put these
    // in different buckets, which is how one user's twenty megabytes became
    // two budgets for two clients.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let bytes = clean_png(0x41);
    let item = attachment("photo.png", "image/png", &encoded(&bytes));

    let token = store
        .deposit("s.a.1", &item)
        .expect("a session with a token");
    let other = store
        .deposit("s.b.1", &item)
        .expect("a session with a different token");
    let legacy = store
        .deposit("session-123-1", &item)
        .expect("a session with no token at all");

    assert_eq!(token.digest, other.digest, "one image, one digest");
    assert_eq!(legacy.digest, token.digest);
    assert_eq!(
        store.store_bytes(),
        Some(3 * bytes.len() as u64),
        "three folders, three copies, one budget"
    );
    assert_eq!(
        store.session_bytes("session-123-1"),
        Some(bytes.len() as u64)
    );
    assert!(legacy.path.exists());
}

#[test]
fn session_bytes_answers_for_one_session_or_not_at_all() {
    // The reader a caller attributes bytes with when it keeps a counter per
    // device rather than one for the store.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let bytes = clean_png(0x42);
    let deposited = store
        .deposit(
            "s.a.1",
            &attachment("photo.png", "image/png", &encoded(&bytes)),
        )
        .expect("deposited");

    assert_eq!(store.session_bytes("s.a.1"), Some(deposited.stored_bytes));
    assert_eq!(
        store.session_bytes("s.a.2"),
        Some(0),
        "a session with no folder holds nothing, which is a number"
    );
    assert_eq!(
        store.session_bytes(".."),
        None,
        "an id the store will not turn into a folder is not an empty session"
    );

    make_unknown(&store, "s.a.2");
    assert_eq!(
        store.session_bytes("s.a.2"),
        None,
        "a folder that could not be read is unknown, not zero"
    );
    assert_eq!(
        store.session_bytes("s.a.1"),
        Some(deposited.stored_bytes),
        "and one unknown folder does not taint a session that was read"
    );
}

#[test]
fn closing_a_session_reports_what_it_dropped() {
    // The number a caller subtracts from what it had reserved.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let first = store
        .deposit(
            "s.a.1",
            &attachment("a.png", "image/png", &encoded(&clean_png(0x43))),
        )
        .expect("first");
    let second = store
        .deposit(
            "s.a.2",
            &attachment("b.png", "image/png", &encoded(&clean_png(0x44))),
        )
        .expect("second");

    assert_eq!(
        store.remove_session("s.a.1"),
        Some(first.stored_bytes),
        "the bytes the folder held"
    );
    assert_eq!(store.store_bytes(), Some(second.stored_bytes));
    assert!(second.path.exists(), "the other session's folder stays");

    // A second close, a session that never existed, and an id with no folder
    // all dropped nothing, and they answer `Some(0)` rather than `None`:
    // `None` means "do not release, the store cannot say", so a caller that
    // read it here would hold a reservation it should have released.
    assert_eq!(store.remove_session("s.a.1"), Some(0));
    assert_eq!(store.remove_session("s.never.9"), Some(0));
    assert_eq!(store.remove_session(".."), Some(0));

    // Unknown is the one answer a caller must not act on.
    make_unknown(&store, "s.a.3");
    assert_eq!(
        store.remove_session("s.a.3"),
        None,
        "a folder the store could not count reports no number"
    );
    assert_eq!(store.store_bytes(), Some(second.stored_bytes));
}

#[test]
fn a_sweep_reports_what_it_reclaimed() {
    // The sweep takes bytes out of the store with no caller asking, and it is
    // the path a per-device counter cannot see unless the store says what
    // went: a folder deleted here is a reservation elsewhere that nothing
    // released. So every removal travels with the session whose folder it was
    // and the bytes it reclaimed.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let swept = store
        .deposit(
            "s.a.1",
            &attachment("a.png", "image/png", &encoded(&clean_png(0x45))),
        )
        .expect("swept");
    let kept = store
        .deposit(
            "s.a.2",
            &attachment("b.png", "image/png", &encoded(&clean_png(0x46))),
        )
        .expect("kept");

    // The sweep's clock is `later`, so the kept folder has to be newer than
    // the limit as measured against it: touching the file it holds puts it
    // there, the same trick successive retention tests use.
    let file = std::fs::File::options()
        .write(true)
        .open(&kept.path)
        .expect("open");
    file.set_modified(SystemTime::now() + ATTACHMENT_RETENTION + Duration::from_secs(30))
        .expect("set mtime");

    let later = SystemTime::now() + ATTACHMENT_RETENTION + Duration::from_secs(60);
    let reclaimed = store.sweep_older_than(later, ATTACHMENT_RETENTION);

    assert_eq!(reclaimed.len(), 1, "one folder was past the limit");
    assert_eq!(reclaimed[0].0, "s.a.1", "the report names the session");
    assert_eq!(
        reclaimed[0].1,
        Some(swept.stored_bytes),
        "and the bytes it reclaimed"
    );
    assert!(kept.path.exists(), "the fresh folder stays");
    assert_eq!(
        store.store_bytes(),
        Some(kept.stored_bytes),
        "what the sweep reclaimed has left the total"
    );
}

/// Every id the daemon composes, run through the store's own rule.
///
/// A rule that closes a hole by refusing the normal case is worse than the
/// hole, so "the normal case" is not a shape this file gets to imagine. The
/// ids below come from the two functions that mint them
/// ([`devboule_protocol::compose_session_id`] with the unique component the
/// real minter formats — `crate::session::session_unique_for_test`, the
/// test-only face of `session.rs`'s composition — and the M2 in-process
/// form `session-{pid}-{n}`); a hand-written `s.a.1` would prove nothing
/// about either.
///
/// The second half pins the assumption the `.`/`..` clause rests on: the
/// protocol accepts both, so the store is the only thing between them and a
/// join. If `validate_session_id` ever starts refusing them, this test says
/// so rather than leaving an extra clause nobody can justify.
#[test]
fn every_id_the_daemon_composes_is_a_folder_name() {
    use devboule_protocol::{compose_session_id, OwnerId};

    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let owners = [
        (
            "S-1-5-21-3806748775-377643871-1481430023-4003354170",
            "app-4242",
        ),
        ("S-1-5-21-1-2-3-1001", "process-1234"),
        ("peer_dev-1", "daemon"),
        ("peer_dev-1", "client"),
        ("S-1-5-21-1", "client"),
        (
            "S-1-5-21-1",
            "a-client-label-that-is-far-too-long-for-the-token",
        ),
        ("S-1-5-21-1", "...."),
    ];
    let mut minted = Vec::new();
    for (user, client) in owners {
        let owner = OwnerId::new(user, client).expect("owner");
        for (counter, nonce) in [(0u64, 0u64), (1, 0x9f2c_1a7b_3e5d_6048), (0xffff_ffff, 1)] {
            let unique = crate::session::session_unique_for_test(nonce, counter);
            let id =
                compose_session_id(&owner.session_token(), &unique).expect("the daemon mints this");
            minted.push(id);
        }
    }
    for id in ["session-123-1", "session-1-1", "s.a.1"] {
        minted.push(id.to_string());
    }

    for id in &minted {
        assert!(
            store.session(id).is_some(),
            "the store refuses an id the daemon composes: {id:?}"
        );
    }
    assert_eq!(minted.len(), 24, "the shapes above: {:?}", minted);

    // The spellings, written out so the shapes are readable without running
    // anything: `session_token` is the client label cut to sixteen
    // characters of `[A-Za-z0-9_-]`, `p`-prefixed for a remote owner, and
    // `client` when nothing survives the filter. The unique is the mint's
    // counter-plus-nonce shape with a fixed nonce.
    let compose = |user: &str, client: &str| {
        let owner = OwnerId::new(user, client).expect("owner");
        compose_session_id(&owner.session_token(), "00000001-9f2c1a7b3e5d6048")
            .expect("the daemon mints this")
    };
    assert_eq!(
        compose("S-1-5-21-1-2-3-1001", "process-1234"),
        "s.process-1234.00000001-9f2c1a7b3e5d6048"
    );
    assert_eq!(
        compose("peer_dev-1", "daemon"),
        "s.pdaemon.00000001-9f2c1a7b3e5d6048"
    );
    assert_eq!(
        compose("S-1-5-21-1", "...."),
        "s.client.00000001-9f2c1a7b3e5d6048"
    );
    assert_eq!(
        compose(
            "S-1-5-21-1",
            "a-client-label-that-is-far-too-long-for-the-token"
        ),
        "s.a-client-label-t.00000001-9f2c1a7b3e5d6048"
    );

    // Not assumed: this is why the clause above exists.
    for id in [".", ".."] {
        assert!(
            validate_session_id(id).is_ok(),
            "{id:?} is no longer an identifier, so the clause is now the protocol's"
        );
        assert!(store.session(id).is_none(), "{id:?} named a folder");
    }
}

/// Make `link` a link onto `target`, with the strongest link this machine
/// will make.
///
/// A symlink first, because it is the spelling that covers both kinds of
/// target — a file and a folder — and it is what a hostile process would
/// plant when it can. `CreateSymbolicLink` needs SeCreateSymbolicLinkPrivilege
/// or developer mode, so the fallback is the junction `mklink /J` any
/// unprivileged user can create (`acp_host.rs` makes the same choice, for the
/// same reason); a junction only accepts a folder, so a file target on a
/// machine that refused the symlink has no link at all. Both are reparse
/// points and on POSIX both are a symlink, which is the property these tests
/// are about.
///
/// `false` when no mechanism worked — and a caller must fail on it rather
/// than return: a test that skipped itself would report the property green
/// without having exercised it, which is worse than no test.
fn link_onto(link: &Path, target: &Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::{symlink_dir, symlink_file};
        let linked = if target.is_dir() {
            symlink_dir(target, link).is_ok()
        } else {
            symlink_file(target, link).is_ok()
        };
        if linked {
            return true;
        }
    }
    junction_onto(link, target)
}

/// Make `link` a junction onto `target` on Windows, and the symlink POSIX
/// has instead of one.
///
/// The unprivileged Windows spelling: `CreateSymbolicLink` wants a privilege
/// or developer mode, and `mklink /J` wants neither, so a junction is what a
/// hostile same-user process can plant on any machine. [`link_onto`] reaches
/// for this only as a fallback, because a junction takes a folder and not a
/// file; the tests that plant a folder redirect use it directly, so the
/// spelling that needs no privilege is the one they exercise.
fn junction_onto(link: &Path, target: &Path) -> bool {
    #[cfg(windows)]
    {
        std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .status()
            .is_ok_and(|status| status.success())
    }
    #[cfg(not(windows))]
    {
        std::os::unix::fs::symlink(target, link).is_ok()
    }
}

/// A link whose target has been taken away: a name that is there and cannot
/// be followed, which is the portable spelling of an entry whose metadata
/// cannot be read.
fn dangling_link(link: &Path, target: &Path) -> bool {
    std::fs::create_dir_all(target).expect("target folder");
    if !link_onto(link, target) {
        return false;
    }
    std::fs::remove_dir(target).expect("take the target away");
    true
}

#[test]
fn a_session_id_that_names_a_path_is_refused_rather_than_joined() {
    // The absolute case is the one that does the damage, and it is real
    // rather than hypothetical: `Path::join` with a rooted path *replaces*
    // the base, so `root.join(id)` is not "climb out of the store", it is
    // "the store is now that folder". The folder used here is this test's
    // own — a system directory would turn a bug in the store into a bug in
    // the machine — and the assertion is that it survives the call.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let outside = TempDir::new();
    let marker = outside.0.join("keep-me.png");
    std::fs::write(&marker, b"x").expect("marker");
    let absolute = outside.0.to_string_lossy().into_owned();

    let refused = [
        "",
        ".",
        "..",
        absolute.as_str(),
        "\\\\server\\share\\x",
        "C:x",
        "a/../..",
        "a\\..\\..",
        "s.a.1/nested",
        "file:stream",
        "nul",
        "CON.txt",
        "s.a.1.",
    ];
    for id in refused {
        assert!(store.session(id).is_none(), "id {id:?} named a folder");
        assert_eq!(
            store.remove_session(id),
            Some(0),
            "id {id:?} dropped bytes it could not have held"
        );
    }

    assert!(
        marker.exists(),
        "a refused id deleted a folder outside the store"
    );
    assert_eq!(
        std::fs::read_dir(&outside.0).expect("outside").count(),
        1,
        "a refused id touched a folder that is not the store"
    );
    assert_eq!(
        std::fs::read_dir(&temp.0).expect("the runtime dir").count(),
        0,
        "a refused id made something in the store's runtime directory"
    );
}

#[test]
fn the_inline_path_asks_the_budget_before_it_writes() {
    // `materialize` is the inline path: the attachment of a `SessionSend`
    // that carries its bytes in the prompt. It used to charge the store's
    // total without ever asking it — `write_locked` adds, and only `deposit`
    // compared — so this path could spend a limit it never consulted, one
    // image at a time. The fixture is the deposit test's: a folder this
    // process never wrote, filled to the byte, so the walk is the only thing
    // that knows the store is full.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let full = store.session("s.a.1").expect("session");
    std::fs::create_dir_all(&full.dir).expect("session folder");
    std::fs::write(
        full.dir.join("photo.png"),
        vec![0u8; MAX_ATTACHMENT_OWNER_BYTES],
    )
    .expect("fill the store's budget");
    assert_eq!(
        store.store_bytes(),
        Some(MAX_ATTACHMENT_OWNER_BYTES as u64),
        "the store is full before the inline write is tried"
    );

    let bytes = clean_png(0x61);
    let session = store.session("s.a.2").expect("session");
    let error = session
        .materialize(&attachment("photo.png", "image/png", &encoded(&bytes)))
        .expect_err("refused");

    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert!(
        error
            .message
            .contains(&MAX_ATTACHMENT_OWNER_BYTES.to_string()),
        "the refusal must name the limit it refused against: {}",
        error.message
    );
    assert!(
        !session.dir.exists(),
        "nothing may be created on a refusal: the inline path wrote anyway"
    );
    assert_eq!(
        store.store_bytes(),
        Some(MAX_ATTACHMENT_OWNER_BYTES as u64),
        "a refused inline write must not move the total"
    );
}

#[test]
fn two_writers_cannot_take_the_total_past_the_limit() {
    // Two threads, one lock, one slot: the store has room for exactly one of
    // the two payloads, so whichever thread wins, the total must never
    // exceed the limit and exactly one write must be refused. The two
    // threads take different paths into the store (`deposit` and the inline
    // `materialize`) because those are the two doors, and a budget held at
    // one door is not a budget. The observer thread reads the total while
    // they run: with the check and the write in one critical section no
    // reading can see the store over its limit, and a build that checked
    // outside the lock (or not at all) can. No sleeps — the loop yields, and
    // the number it keeps is the answer.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let bytes = clean_png(0x62);
    let full = store.session("s.z.1").expect("session");
    std::fs::create_dir_all(&full.dir).expect("session folder");
    std::fs::write(
        full.dir.join("photo.png"),
        vec![0u8; MAX_ATTACHMENT_OWNER_BYTES - bytes.len()],
    )
    .expect("leave room for exactly one");

    let running = Arc::new(std::sync::atomic::AtomicBool::new(true));
    let observed = {
        let store = store.clone();
        let running = Arc::clone(&running);
        std::thread::spawn(move || {
            let mut highest = 0u64;
            while running.load(std::sync::atomic::Ordering::Relaxed) {
                if let Some(total) = store.store_bytes() {
                    highest = highest.max(total);
                }
                std::thread::yield_now();
            }
            if let Some(total) = store.store_bytes() {
                highest = highest.max(total);
            }
            highest
        })
    };

    let via_deposit = {
        let store = store.clone();
        let item = attachment("photo.png", "image/png", &encoded(&bytes));
        std::thread::spawn(move || store.deposit("s.a.1", &item).is_ok())
    };
    let via_inline = {
        let store = store.clone();
        let item = attachment("photo.png", "image/png", &encoded(&bytes));
        std::thread::spawn(move || {
            store
                .session("s.a.2")
                .expect("session")
                .materialize(&item)
                .is_ok()
        })
    };
    let accepted = [via_deposit, via_inline]
        .into_iter()
        .map(|writer| writer.join().expect("thread"))
        .filter(|accepted| *accepted)
        .count();
    running.store(false, std::sync::atomic::Ordering::Relaxed);
    let highest = observed.join().expect("observer");

    assert_eq!(accepted, 1, "one slot, two writers");
    assert_eq!(
        store.store_bytes(),
        Some(MAX_ATTACHMENT_OWNER_BYTES as u64),
        "the accepted write must fill the store exactly"
    );
    assert!(
        highest <= MAX_ATTACHMENT_OWNER_BYTES as u64,
        "the total reached {highest} under a limit of {MAX_ATTACHMENT_OWNER_BYTES}"
    );
}

#[test]
fn a_write_through_a_junction_is_refused_rather_than_followed() {
    // The session folder's own name is checked with `symlink_metadata`
    // before anything is created or written, and both halves of the failure
    // are asserted: the refusal, and the fact that the tree the junction
    // names is still empty. A store that followed it would report a path it
    // does not hold, counted in a folder that holds nothing.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let elsewhere = TempDir::new();
    let session = store.session("s.a.1").expect("session");
    std::fs::create_dir_all(&store.root).expect("store root");
    assert!(
        junction_onto(&session.dir, &elsewhere.0),
        "could not create the junction this test is about"
    );

    let bytes = clean_png(0x63);
    let error = session
        .materialize(&attachment("photo.png", "image/png", &encoded(&bytes)))
        .expect_err("the write followed the junction");

    assert_eq!(error.code, ErrorCode::Io);
    assert!(
        error.message.contains("reparse point"),
        "the refusal must name the cause: {}",
        error.message
    );
    assert_eq!(
        std::fs::read_dir(&elsewhere.0).expect("elsewhere").count(),
        0,
        "the write landed in the tree the junction names"
    );
    assert!(
        is_redirect(&session.dir),
        "the junction must still be the session folder"
    );
}

#[test]
fn a_folder_a_sweep_cannot_date_is_kept_rather_than_deleted() {
    // `newest_write` used to skip an entry it could not read — `flatten` on
    // the listing, `if let Ok` on the metadata — so a folder whose only
    // unreadable entry was its newest write reported the newest write it
    // could read, looked idle, and was swept: the deletion the retention
    // rule exists to avoid. `folder_bytes` already answers this way for the
    // number it computes, and here the consequence is a file that survives
    // rather than a refusal.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let session = store.session("s.a.1").expect("session");
    let stored = session
        .materialize(&attachment(
            "a.png",
            "image/png",
            &encoded(&clean_png(0x64)),
        ))
        .expect("materialized");
    assert!(
        dangling_link(&session.dir.join("unreadable.png"), &temp.0.join("gone")),
        "could not create the unreadable entry this test is about"
    );

    let later = SystemTime::now() + ATTACHMENT_RETENTION + Duration::from_secs(60);
    assert_eq!(
        is_older_than(&session.dir, later, ATTACHMENT_RETENTION),
        None,
        "an entry that cannot be read is a folder that cannot be dated"
    );
    let reclaimed = store.sweep_older_than(later, ATTACHMENT_RETENTION);
    assert!(
        reclaimed.is_empty(),
        "the sweep deleted a folder it could not date: {reclaimed:?}"
    );
    assert!(
        stored.exists(),
        "the folder went with the timestamp the sweep could not read"
    );
}

#[test]
fn scratch_a_crash_left_behind_does_not_hold_budget() {
    // A run that died between `atomic_write`'s temp file and its rename
    // leaves `<digest>.tmp` in the session folder. Every walk counts it (it
    // is a file on the disk) and no `resolve` can return it (`find_stored`
    // takes a stored extension), so it is budget nothing can spend and
    // nothing can name — for the life of the folder, unless the store takes
    // it away. Two of the places it does are asserted: a write into the
    // folder, under the lock, and the store's open.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let stored = store
        .deposit(
            "s.a.1",
            &attachment("a.png", "image/png", &encoded(&clean_png(0x65))),
        )
        .expect("deposited");
    let digest = sha256_hex(b"a write that never renamed");
    let scratch = stored
        .path
        .parent()
        .expect("session folder")
        .join(format!("{digest}.tmp"));
    std::fs::write(&scratch, vec![0u8; 4096]).expect("scratch");

    // Rebuild the picture rather than trust the cache, which is what the
    // walk does on its own after a process restart.
    make_unseeded(&store);
    assert_eq!(
        store.store_bytes(),
        Some(stored.stored_bytes + 4096),
        "the walk counts what is on the disk, scratch and all"
    );
    assert!(
        store.resolve("s.a.1", &digest, None).is_err(),
        "and nothing can hand it back, which is why it must not stay counted"
    );

    // A write into the folder removes it under the lock and takes the bytes
    // off the total in the same critical section.
    let second = store
        .deposit(
            "s.a.1",
            &attachment("b.png", "image/png", &encoded(&clean_png(0x66))),
        )
        .expect("deposited");
    assert!(
        !scratch.exists(),
        "the write left the scratch in the folder"
    );
    assert_eq!(
        store.store_bytes(),
        Some(stored.stored_bytes + second.stored_bytes),
        "the scratch left the total with the file"
    );

    // And the store's open does it for a folder nothing writes into again.
    std::fs::write(&scratch, vec![0u8; 4096]).expect("scratch again");
    let reopened = AttachmentStore::new(&temp.0);
    assert!(!scratch.exists(), "the open left the scratch in the folder");
    assert_eq!(
        reopened.store_bytes(),
        Some(stored.stored_bytes + second.stored_bytes),
        "a reopened store counts the folder without the scratch"
    );
}

#[test]
fn a_folder_where_the_digest_names_a_file_refuses_the_deposit() {
    // `exists` follows nothing and is true of a folder, so the check it
    // replaced answered "already stored, success" for a directory sitting at
    // a digest's name: `deposit` reported a stored attachment that is not a
    // file and `stored_size` sized the directory. Success has to mean the
    // store holds *this* file. No privilege is needed to plant a folder,
    // which is why this is the regression that runs everywhere.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let image = attachment("photo.png", "image/png", &encoded(&clean_png(0x70)));
    let stored = store.deposit("s.a.1", &image).expect("deposited");

    std::fs::remove_file(&stored.path).expect("take the stored file away");
    std::fs::create_dir_all(&stored.path).expect("a folder at the digest name");

    let error = store
        .deposit("s.a.1", &image)
        .expect_err("a folder at the digest name was reported as a stored file");
    assert_eq!(error.code, ErrorCode::Io);
    assert!(
        error.message.contains("is not a stored file"),
        "the refusal must name what is there: {}",
        error.message
    );
    assert!(stored.path.is_dir(), "the deposit wrote over the folder");
    assert_eq!(
        std::fs::read_dir(&stored.path).expect("the folder").count(),
        0,
        "the store put something in a folder it had just refused"
    );
}

#[test]
fn a_link_at_the_digest_name_is_refused_rather_than_written_through() {
    // The same hole with a link, and the sharper half of it: `exists`
    // follows, so the deposit answered success, `stored_size` read the size
    // of what the link names, and this session was reported as holding
    // somebody else's file.
    //
    // A junction onto a folder and not a symlink onto a file, deliberately,
    // and it is the stronger of the two rather than the weaker: `mklink /J`
    // needs no privilege, so it is the spelling a hostile same-user process
    // can actually plant on Windows, while `CreateSymbolicLink` is refused
    // wherever developer mode is off — this repository's own
    // `acp_host.rs` plants a junction for exactly that reason. A test that
    // needed the privilege would go red on a machine setting rather than on
    // the property it guards (`ci.yml` runs this suite on `windows-latest`),
    // and a false red on a security fix is how the fix gets reverted.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let image = attachment("photo.png", "image/png", &encoded(&clean_png(0x71)));
    let stored = store.deposit("s.a.1", &image).expect("deposited");
    let elsewhere = TempDir::new();
    std::fs::remove_file(&stored.path).expect("take the stored file away");
    assert!(
        junction_onto(&stored.path, &elsewhere.0),
        "the platform refused to make the junction this test is about"
    );

    let error = store
        .deposit("s.a.1", &image)
        .expect_err("a link at the digest name was reported as a stored file");
    assert_eq!(error.code, ErrorCode::Io);
    assert!(
        error.message.contains("reparse point"),
        "the refusal must name the cause: {}",
        error.message
    );
    // The tree the junction names is untouched, which is what "nothing was
    // written through the name" means here — and it is the same fact the
    // pre-fix behaviour gets wrong: the deposit created no file, so it had
    // no bytes, and the folder is where that shows.
    assert_eq!(
        std::fs::read_dir(&elsewhere.0)
            .expect("the folder the junction names")
            .count(),
        0,
        "the deposit wrote through the junction"
    );
    assert!(
        is_redirect(&stored.path),
        "the name must still be the link the test planted"
    );
}

#[test]
fn a_resolve_does_not_follow_a_link_at_the_digest_name() {
    // `resolve` hands a provider a path to read, and `is_file` follows the
    // name: a name that is a reparse point answered for what it pointed at,
    // so the store could report a path outside itself as a stored
    // attachment. Both places that asked are asserted — the hint, which
    // builds the name and was the shortcut to the wrong answer, and the
    // listing, which is what answers when there is no hint — and both must
    // refuse a name that is a reparse point whatever it points at.
    //
    // A junction onto a folder for the same reason the deposit test above
    // uses one: `mklink /J` needs no privilege, so it is what a hostile
    // same-user process can plant on any Windows machine, and the suite has
    // to hold on a runner where developer mode is off (`ci.yml` runs it on
    // `windows-latest`).
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let image = attachment("photo.png", "image/png", &encoded(&clean_png(0x72)));
    let stored = store.deposit("s.a.1", &image).expect("deposited");
    let elsewhere = TempDir::new();
    std::fs::remove_file(&stored.path).expect("take the stored file away");
    assert!(
        junction_onto(&stored.path, &elsewhere.0),
        "the platform refused to make the junction this test is about"
    );

    let hinted = store
        .resolve("s.a.1", &stored.digest, Some("png"))
        .expect_err("resolve handed back a path through a link");
    assert_eq!(hinted.code, ErrorCode::InvalidRequest);
    assert!(
        store.resolve("s.a.1", &stored.digest, None).is_err(),
        "the listing handed back a path through a link"
    );
}

#[test]
fn a_resolve_refuses_a_session_folder_that_is_a_link() {
    // The entry check in `find_stored` does not cover this one: the folder
    // itself is the redirect, so the path it hands back is a name inside the
    // store that resolves outside it — through a link, to a file the store
    // never wrote, sized by `stored_size` and reported as a stored
    // attachment. The folder is refused before the listing runs, and both
    // spellings of the lookup are asserted: the hint, and the listing.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let session = store.session("s.a.1").expect("session");
    let elsewhere = TempDir::new();
    let digest = sha256_hex(b"a file this store never wrote");
    std::fs::write(elsewhere.0.join(format!("{digest}.png")), vec![0x5a; 64])
        .expect("the file behind the link");
    std::fs::create_dir_all(&store.root).expect("store root");
    assert!(
        junction_onto(&session.dir, &elsewhere.0),
        "the platform refused to make the junction this test is about"
    );

    let hinted = store
        .resolve("s.a.1", &digest, Some("png"))
        .expect_err("resolve read a file through a session folder that is a link");
    assert_eq!(hinted.code, ErrorCode::Io);
    assert!(
        hinted.message.contains("reparse point"),
        "the refusal must name the cause: {}",
        hinted.message
    );
    assert!(
        store.resolve("s.a.1", &digest, None).is_err(),
        "the listing read a file through a session folder that is a link"
    );
}

#[test]
fn a_store_root_that_is_a_link_refuses_every_answer() {
    // The root is the one folder every path in this store starts at, so a
    // link at its name is not one redirect among many: it is the whole store
    // being somebody else's tree. The open used to sweep the scratch out of
    // it — `read_dir` through the junction, `*.tmp` deleted in another tree —
    // the sweep dated and deleted folders in it, the walk counted its files
    // as this store's budget, and deposits wrote into it. All four are
    // asserted, plus the two deletions the tree on the other side must not
    // suffer.
    let temp = TempDir::new();
    let elsewhere = TempDir::new();
    let victim = elsewhere.0.join("victim.tmp");
    std::fs::write(&victim, vec![0u8; 4096]).expect("scratch the open must not sweep");
    let foreign = elsewhere.0.join("s.other.1");
    std::fs::create_dir_all(&foreign).expect("a folder the sweep must not delete");
    let root = temp.0.join(ATTACHMENTS_DIR);
    assert!(
        link_onto(&root, &elsewhere.0),
        "the platform refused to make the link this test is about"
    );

    let store = AttachmentStore::new(&temp.0);

    assert!(
        store.session("s.a.1").is_none(),
        "a store whose root is a link handed back a session folder"
    );
    assert_eq!(
        store.store_bytes(),
        None,
        "the total counted the files of the tree the root names"
    );
    assert_eq!(
        store.session_bytes("s.a.1"),
        None,
        "a session in a store with no root of its own is not a session of zero bytes"
    );
    assert_eq!(
        store.remove_session("s.a.1"),
        Some(0),
        "a close reported dropping bytes it never held"
    );
    let later = SystemTime::now() + ATTACHMENT_RETENTION + Duration::from_secs(60);
    assert!(
        store
            .sweep_older_than(later, ATTACHMENT_RETENTION)
            .is_empty(),
        "the sweep found candidates under a root that is a link"
    );
    let error = store
        .deposit(
            "s.a.1",
            &attachment("photo.png", "image/png", &encoded(&clean_png(0x73))),
        )
        .expect_err("a deposit into a store whose root is a link");
    assert_eq!(error.code, ErrorCode::SessionNotFound);

    assert!(
        victim.exists(),
        "the open swept scratch out of the tree the root names"
    );
    assert!(
        foreign.is_dir(),
        "the sweep deleted a folder in the tree the root names"
    );
}

#[test]
fn a_sweep_leaves_a_session_folder_that_is_a_link_alone() {
    // A junction in the root, which is the spelling an unprivileged process
    // can plant on Windows — and a pin rather than a regression test, said
    // plainly because it was measured: the sweep's `is_dir` filter already
    // passes a redirect over today (`DirEntry::metadata` answers for the
    // reparse point, so a junction to a folder is not a directory either),
    // and the explicit check above it is what keeps that true without
    // resting a deletion on how the toolchain classifies a mount point. The
    // store's own folder beside the junction is the control: the sweep still
    // runs, it just does not run into a link.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let elsewhere = TempDir::new();
    let kept = elsewhere.0.join("kept.png");
    std::fs::write(&kept, vec![0x5a; 512]).expect("the file behind the link");
    std::fs::create_dir_all(store.root.join("s.other.1")).expect("a real session folder");
    let link = store.root.join("s.link.1");
    assert!(
        junction_onto(&link, &elsewhere.0),
        "the platform refused to make the junction this test is about"
    );

    let later = SystemTime::now() + ATTACHMENT_RETENTION + Duration::from_secs(60);
    let reclaimed = store.sweep_older_than(later, ATTACHMENT_RETENTION);

    assert!(
        reclaimed.iter().all(|(id, _)| id != "s.link.1"),
        "the sweep dated and deleted a name that is a link: {reclaimed:?}"
    );
    assert!(is_redirect(&link), "the name the sweep removed was a link");
    assert!(kept.exists(), "the sweep deleted through the link");
}

#[test]
fn two_ids_that_differ_only_in_case_are_not_one_folder() {
    // Windows resolves folder names case-insensitively while the cache is
    // keyed by the id as written, so `s.a.1` and `S.A.1` were one folder
    // under two keys: the budget charged it twice, and closing either
    // session deleted the other's files. The upper-case spelling is refused
    // by name, which is the fix — folding it instead would merge two
    // distinct sessions on a case-sensitive filesystem.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    assert!(
        store.session("s.a.1").is_some(),
        "the lower-case id is a name"
    );
    assert!(
        store.session("S.A.1").is_none(),
        "an id differing from a lower-case one only in case named the same folder"
    );
    let stored = store
        .deposit(
            "s.a.1",
            &attachment("a.png", "image/png", &encoded(&clean_png(0x74))),
        )
        .expect("deposited");
    assert_eq!(
        store.session_bytes("S.A.1"),
        None,
        "the upper-case spelling answered as a session holding zero bytes"
    );
    assert_eq!(
        store.remove_session("S.A.1"),
        Some(0),
        "a close by the upper-case spelling reported dropping bytes"
    );
    assert!(
        stored.path.exists(),
        "a close by the upper-case spelling deleted the lower-case session's file"
    );
    assert_eq!(
        store.session_bytes("s.a.1"),
        Some(stored.stored_bytes),
        "the lower-case session is the one that still holds the bytes"
    );
}

#[cfg(all(windows, feature = "server"))]
#[test]
fn a_session_folder_that_predates_the_store_is_private_at_open() {
    // The DACL used to be applied only by `prepare_session_dir`, which is the
    // first *write*, and `resolve` reads a folder before any write has been
    // through it: a folder an earlier build left behind kept whatever its
    // parent granted, which on a default profile includes other accounts on
    // the machine. A folder created here has exactly that inherited DACL, and
    // the assertion is on the state right after the open — no write, no
    // resolve, nothing but `AttachmentStore::new` — so the fix is what
    // narrows it and not a deposit that happens to run first.
    let temp = TempDir::new();
    let dir = temp.0.join(ATTACHMENTS_DIR).join("s.a.1");
    std::fs::create_dir_all(&dir).expect("a session folder from an earlier build");
    let sid = crate::security::current_user_sid().expect("sid");
    assert!(
        !crate::security::dacl_is_current_user_only(
            &crate::security::dacl_sddl_for_path(&dir).expect("dacl"),
            &sid
        ),
        "the folder has to start out inheriting a wider DACL for this test to mean anything"
    );

    let _store = AttachmentStore::new(&temp.0);

    for folder in [temp.0.join(ATTACHMENTS_DIR), dir] {
        let sddl = crate::security::dacl_sddl_for_path(&folder).expect("dacl");
        assert!(
            crate::security::dacl_is_current_user_only(&sddl, &sid),
            "the open must narrow {} before anything writes into it: {sddl}",
            folder.display()
        );
    }
}

/// Ask [`harden`] to fail on one folder name, for the next store a test
/// opens on this thread.
///
/// The seam exists because the decision to pin is what the *walk* does with a
/// failure, and `restrict_to_current_user` is a no-op on POSIX: a test that
/// could only fail a real DACL write would not run on every platform this
/// suite runs on, and the failure path would be the one thing never
/// exercised.
fn fail_hardening_of(name: &str) {
    HARDEN_FAILURE.with(|blocked| *blocked.borrow_mut() = Some(name.into()));
}

/// Put [`harden`] back to refusing nothing, so the failure cannot leak into
/// the store another test opens on this thread.
fn stop_failing_hardening() {
    HARDEN_FAILURE.with(|blocked| *blocked.borrow_mut() = None);
}

#[test]
fn a_folder_the_open_cannot_harden_makes_the_store_unavailable() {
    // DEP-17: a DACL that could not be applied used to be swallowed and the
    // store opened anyway — `session` handed the folder back, `resolve` read
    // attachments out of it, and the folder kept whatever its parent granted,
    // which is the whole reason the walk hardens it. The failure is planted
    // through `harden`'s seam, so this holds on a platform where the call
    // underneath is a no-op, and the folder predates the store, which is the
    // case the walk hardens rather than the one a write creates later.
    let temp = TempDir::new();
    let dir = temp.0.join(ATTACHMENTS_DIR).join("s.a.1");
    std::fs::create_dir_all(&dir).expect("a session folder from an earlier build");
    fail_hardening_of("s.a.1");

    let store = AttachmentStore::new(&temp.0);
    stop_failing_hardening();

    assert!(
        store.session("s.a.1").is_none(),
        "a store opened with a folder it could not narrow handed the folder back"
    );
    assert_eq!(
        store.store_bytes(),
        None,
        "and the bytes of that folder are not a number this store may count"
    );
    assert!(
        store
            .sweep_older_than(
                SystemTime::now() + ATTACHMENT_RETENTION + Duration::from_secs(60),
                ATTACHMENT_RETENTION
            )
            .is_empty(),
        "and nothing in it may be swept"
    );

    // The same tree opens and answers normally when the failure is not asked
    // for, which is what keeps the assertions above about the failure rather
    // than about the seam being stuck.
    let reopened = AttachmentStore::new(&temp.0);
    assert!(
        reopened.session("s.a.1").is_some(),
        "the store must open when every folder it hardens is hardened"
    );
    assert_eq!(
        reopened.store_bytes(),
        Some(0),
        "and a store that opened may count what it holds"
    );
}

#[test]
fn a_junction_in_the_root_is_not_part_of_the_stores_bytes() {
    // The walk that builds the budget counts the folders in the root, and a
    // junction there is a folder in somebody else's tree: charging what is
    // behind it would hand this store a total counting files it does not
    // hold, and that total is what the limit is enforced against. Asserted on
    // the number and not on which check skips the entry: the redirect check
    // and the `is_dir` filter below it are measured to agree on today's
    // toolchain (see `seed_locked` and the sweep), and the property is what
    // has to hold if that measurement ever stops being true.
    let temp = TempDir::new();
    let store = AttachmentStore::new(&temp.0);
    let elsewhere = TempDir::new();
    std::fs::write(elsewhere.0.join("outside.png"), vec![0x5a; 4096]).expect("outside");
    std::fs::create_dir_all(store.root.join("s.real.1")).expect("a real session folder");
    assert!(
        junction_onto(&store.root.join("s.link.1"), &elsewhere.0),
        "the platform refused to make the junction this test is about"
    );

    assert_eq!(
        store.store_bytes(),
        Some(0),
        "the walk charged this store for the tree the junction names"
    );
}
