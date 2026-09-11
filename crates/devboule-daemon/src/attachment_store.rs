//! Where the bytes attached to a prompt live on disk.
//!
//! Files live under the daemon's runtime directory, one folder per session, and
//! never inside the workspace. A workspace is a git checkout the user reads from
//! `git status` and the Changes panel; an attachment written there would appear
//! as the user's own edit and could be committed by accident.
//!
//! Layout is `<runtime dir>/attachments/<session id>/<sha256>.<ext>`.
//!
//! The file name is the sha256 of the bytes that were written, plus an
//! extension taken from the MIME type. For an SVG those are the decoded bytes;
//! for a JPEG or a PNG they are the decoded bytes with their identity metadata
//! removed ([`crate::raster_metadata`]), so the name answers for what is on
//! disk rather than for what arrived. The user's file name is not in the path,
//! for two reasons. It comes from outside — it may contain `..`, a path
//! separator, or a drive letter — and the digest contains none of those, so
//! traversal is not possible to express. And a content-addressed name makes
//! re-materializing the same image reuse the file: the same bytes are the same
//! path, so a second turn with the same picture, or a replay of the history
//! that rebuilds it, does not leave another copy behind.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use base64::Engine;
use devboule_protocol::{
    invalid_base64_message, unsupported_attachment_type_message, ErrorCode, PromptAttachment,
    WireError,
};
use sha2::{Digest, Sha256};

use crate::raster_metadata::{
    sniff_raster_mime, strip_raster_metadata, RasterMime, RasterStripError,
};

/// The runtime-dir subdirectory that holds every session's attachments.
const ATTACHMENTS_DIR: &str = "attachments";

/// How long a session's attachment folder may outlive its newest write.
///
/// Session close removes the folder outright, so this exists for the close that
/// never ran: a daemon killed with the power failed, a crash, a machine that
/// went down mid-session. Seven days with no write means no session is using it.
pub(crate) const ATTACHMENT_RETENTION: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The attachment folders of one runtime directory.
#[derive(Clone)]
pub(crate) struct AttachmentStore {
    root: PathBuf,
    /// Serializes writes across sessions. One process owns the runtime dir
    /// (single-instance lock), so this is enough to keep two client threads
    /// materializing the same image from racing over the same temp file.
    write_lock: Arc<Mutex<()>>,
}

impl AttachmentStore {
    pub(crate) fn new(runtime_dir: &Path) -> Self {
        Self {
            root: runtime_dir.join(ATTACHMENTS_DIR),
            write_lock: Arc::new(Mutex::new(())),
        }
    }

    /// The folder holding one session's attachments.
    ///
    /// `None` for `.` and `..`. Both pass `validate_session_id` — its alphabet
    /// is `[A-Za-z0-9._-]` — and both are path traversal when joined to a root.
    /// A real id is composed as `s.<owner>.<n>`, so refusing them loses nothing
    /// and removes the only way a session id could name a directory outside the
    /// store.
    pub(crate) fn session(&self, session_id: &str) -> Option<SessionAttachments> {
        if session_id == "." || session_id == ".." {
            return None;
        }
        Some(SessionAttachments {
            dir: self.root.join(session_id),
            write_lock: Arc::clone(&self.write_lock),
        })
    }

    /// Drop one session's folder. Absent is not an error: close may run for a
    /// session that never sent an attachment.
    pub(crate) fn remove_session(&self, session_id: &str) {
        if let Some(session) = self.session(session_id) {
            let _ = std::fs::remove_dir_all(&session.dir);
        }
    }

    /// Delete every session folder whose newest write is older than `max_age`.
    /// Returns how many were deleted.
    ///
    /// `now` is a parameter so the retention rule can be tested without moving
    /// real file timestamps around.
    pub(crate) fn sweep_older_than(&self, now: SystemTime, max_age: Duration) -> usize {
        let Ok(entries) = std::fs::read_dir(&self.root) else {
            // No store yet is the normal state of a fresh install.
            return 0;
        };
        let mut removed = 0;
        for entry in entries.flatten() {
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if !metadata.is_dir() {
                continue;
            }
            let Some(newest) = newest_write(&entry.path()) else {
                continue;
            };
            // A timestamp in the future is not evidence of age; keep the folder
            // and let a later sweep decide.
            let Ok(age) = now.duration_since(newest) else {
                continue;
            };
            if age > max_age && std::fs::remove_dir_all(entry.path()).is_ok() {
                removed += 1;
            }
        }
        removed
    }
}

/// One session's attachment folder.
pub(crate) struct SessionAttachments {
    dir: PathBuf,
    write_lock: Arc<Mutex<()>>,
}

impl SessionAttachments {
    /// Write one attachment to disk and return its absolute path.
    ///
    /// Re-materializing the same bytes returns the existing path and does not
    /// rewrite the file.
    ///
    /// A raster is walked and its identity metadata taken out
    /// ([`crate::raster_metadata`]) between the decode and the digest, so the
    /// bytes written are not always the bytes the client sent. The name is
    /// built from the bytes that were written, which makes the consequence
    /// worth stating plainly, because a content-addressed name reads like a
    /// promise about the input: **the file is named after the stripped bytes.**
    /// A client that predicts the attachment path from the digest of the bytes
    /// it sent is therefore wrong whenever anything was stripped. That is
    /// intended, not a defect. The daemon is not transparent about
    /// attachments; it rewrites them before forwarding, and the path is the
    /// digest of what is actually on disk.
    ///
    /// A raster this pass cannot walk is refused rather than stored. Handing
    /// the input back on a parse failure would re-admit exactly the bytes the
    /// caller asked to have removed and would report a strip that did nothing
    /// as a success.
    ///
    /// SVG is deliberately not stripped here, and that is a decision rather
    /// than an oversight. The frontend sanitises SVG source
    /// (`sanitizeSvgSource`) and this side does not, so an SVG arriving from
    /// another device — the very path this pass exists for — is written
    /// unsanitised. It is a known, named gap: the raster rule is a byte-level
    /// walk and the SVG rule is a source-level parse with its own vocabulary,
    /// and this pass does not guess at the second one. Closing it means
    /// porting that sanitiser, not extending this one.
    ///
    /// Which container the bytes are is decided by the bytes, not by the
    /// `mime_type` the client sent. The declared type is the sender's word and
    /// the bytes are the evidence, and the two have to agree: the strip used to
    /// be selected from the label alone, so a JPEG full of coordinates labelled
    /// `image/svg+xml` took the write-through path and reached the provider
    /// untouched. The label is a field the sender controls, and it must not be
    /// the thing that decides whether the guarantee runs.
    ///
    /// So a file whose bytes and label disagree about the container is refused,
    /// and so is a file declared a raster that does not open with that
    /// container's signature — the rule of the pass is that what cannot be
    /// walked is refused rather than handed back intact. The label still decides
    /// the *extension*, which is part of why a disagreement is a refusal and not
    /// a correction: there is no extension to write that would be honest about
    /// both what was declared and what the bytes are.
    pub(crate) fn materialize(&self, attachment: &PromptAttachment) -> Result<PathBuf, WireError> {
        let bytes = decode(&attachment.data)?;
        let extension = extension_for(&attachment.mime_type)
            .ok_or_else(|| unsupported_type(&attachment.mime_type))?;
        let stored = match (
            sniff_raster_mime(&bytes),
            RasterMime::from_mime_type(&attachment.mime_type),
        ) {
            // The label and the bytes agree, so the walk runs on what is really
            // there.
            (Some(sniffed), Some(declared)) if sniffed == declared => {
                strip_raster_metadata(&bytes, sniffed)
                    .map_err(unreadable_image)?
                    .bytes
            }
            // Neither the bytes nor the label say raster: the SVG path, and
            // every other type the extension table accepts. Written as it
            // arrived, which is the gap named above and not a new one.
            (None, None) => bytes,
            // Everything else is a disagreement. Either the bytes are a raster
            // the label misnames — including naming it a type that is not a
            // raster at all, which is the bypass this arm closes — or the label
            // says raster and the bytes do not open with the signature they
            // claim. Both are refused, and the alternative to refusing is
            // writing bytes through a guarantee that never examined them.
            _ => return Err(container_disagrees()),
        };
        let path = self
            .dir
            .join(format!("{}.{extension}", sha256_hex(&stored)));
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if path.exists() {
            return Ok(path);
        }
        // A temp file plus a rename: the agent reads this path from another
        // process, and it must never observe a half-written image.
        crate::atomic::atomic_write(&path, &stored).map_err(|error| {
            WireError::new(
                ErrorCode::Io,
                format!("Could not store an attached file: {error}"),
            )
        })?;
        Ok(path)
    }
}

/// The digest that stands for one attachment in an idempotency fingerprint.
///
/// The sha256 of the decoded bytes, not the bytes: the fingerprint lives in an
/// in-memory table for every key the daemon has seen and must not weigh as much
/// as the images. `data` that does not decode is hashed as text instead, which
/// keeps the value total; such a request is refused before it can ever be
/// remembered under a key, so the fallback only has to be deterministic.
pub(crate) fn attachment_digest(attachment: &PromptAttachment) -> String {
    match decode(&attachment.data) {
        Ok(bytes) => sha256_hex(&bytes),
        Err(_) => sha256_hex(attachment.data.as_bytes()),
    }
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn decode(data: &str) -> Result<Vec<u8>, WireError> {
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|_| WireError::new(ErrorCode::InvalidRequest, invalid_base64_message()))
}

fn unsupported_type(mime_type: &str) -> WireError {
    WireError::new(
        ErrorCode::InvalidRequest,
        unsupported_attachment_type_message(mime_type),
    )
}

/// The refusal for a raster the walk could not follow.
///
/// The sentence the walk produced travels: it names the byte offset or the
/// structure that did not add up, which is what makes a refusal actionable, and
/// it is derived from the file's own bytes rather than from anything the user
/// wrote. It is not the whole story the daemon could tell — the reader here is
/// a client showing one line — so the framing is the daemon's and the detail is
/// the walk's.
fn unreadable_image(reason: RasterStripError) -> WireError {
    WireError::new(
        ErrorCode::InvalidRequest,
        format!("An attachment's image could not be read: {reason}"),
    )
}

/// The refusal for a file whose bytes and declared type disagree.
///
/// The declared type is not echoed back. It is the field this refusal is about,
/// it arrives from the wire with nothing bounding its length, and the sender
/// already knows what they sent — so repeating it would put a string the sender
/// chose into a sentence the daemon writes. What is left is the part that can be
/// acted on: the contents are not what they were declared to be. The walk's byte
/// offsets are for a file the client is not being asked to inspect, which is why
/// this reads as a sentence about the attachment rather than about its bytes.
fn container_disagrees() -> WireError {
    WireError::new(
        ErrorCode::InvalidRequest,
        "An attachment's contents do not match the type it was sent as.".to_string(),
    )
}

fn extension_for(mime_type: &str) -> Option<&'static str> {
    match mime_type {
        "image/png" => Some("png"),
        "image/jpeg" => Some("jpg"),
        "image/svg+xml" => Some("svg"),
        _ => None,
    }
}

/// The newest modification time inside one session folder, the folder itself
/// included.
///
/// The folder's own timestamp is not enough: on a POSIX filesystem rewriting an
/// existing file does not touch its directory, so a session that only re-sent
/// files it had already stored would look idle.
fn newest_write(dir: &Path) -> Option<SystemTime> {
    let mut newest = std::fs::metadata(dir).ok()?.modified().ok()?;
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        if let Ok(modified) = entry.metadata().and_then(|metadata| metadata.modified()) {
            newest = newest.max(modified);
        }
    }
    Some(newest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raster_metadata::{clean_png, png_with_text_chunk, vector_input, vector_output};
    use devboule_protocol::ATTACHMENT_MIME_TYPES;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
            let dir = std::env::temp_dir().join(format!(
                "devboule-attachments-{}-{}",
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
    fn a_session_id_that_is_a_parent_directory_is_refused() {
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        assert!(store.session("..").is_none());
        assert!(store.session(".").is_none());
        store.remove_session("..");
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
        assert_eq!(store.sweep_older_than(later, ATTACHMENT_RETENTION), 1);
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
        assert_eq!(store.sweep_older_than(soon, ATTACHMENT_RETENTION), 0);
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

        assert_eq!(
            store.sweep_older_than(SystemTime::now(), ATTACHMENT_RETENTION),
            0
        );
        assert!(path.exists());
    }

    #[test]
    fn sweeping_a_store_that_does_not_exist_yet_is_not_an_error() {
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0.join("absent"));
        assert_eq!(
            store.sweep_older_than(SystemTime::now(), ATTACHMENT_RETENTION),
            0
        );
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
    fn the_fingerprint_digest_distinguishes_two_images_with_one_name() {
        let first = attachment("same.png", "image/png", &encoded(b"first image"));
        let second = attachment("same.png", "image/png", &encoded(b"second image"));
        assert_ne!(attachment_digest(&first), attachment_digest(&second));
        assert_eq!(attachment_digest(&first), attachment_digest(&first));
        assert_eq!(attachment_digest(&first).len(), 64);
    }
}
