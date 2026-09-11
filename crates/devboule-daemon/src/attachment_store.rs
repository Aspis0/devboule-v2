//! Where the bytes attached to a prompt live on disk.
//!
//! Files live under the daemon's runtime directory, one folder per session, and
//! never inside the workspace. A workspace is a git checkout the user reads from
//! `git status` and the Changes panel; an attachment written there would appear
//! as the user's own edit and could be committed by accident.
//!
//! Layout is `<runtime dir>/attachments/<session id>/<sha256>.<ext>`.
//!
//! The file name is the sha256 of the decoded bytes plus an extension taken
//! from the MIME type. The user's file name is not in the path, for two
//! reasons. It comes from outside — it may contain `..`, a path separator, or a
//! drive letter — and the digest contains none of those, so traversal is not
//! possible to express. And a content-addressed name makes re-materializing the
//! same image reuse the file: the same bytes are the same path, so a second
//! turn with the same picture, or a replay of the history that rebuilds it, does
//! not leave another copy behind.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use base64::Engine;
use devboule_protocol::{
    invalid_base64_message, unsupported_attachment_type_message, ErrorCode, PromptAttachment,
    WireError,
};
use sha2::{Digest, Sha256};

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
    pub(crate) fn materialize(&self, attachment: &PromptAttachment) -> Result<PathBuf, WireError> {
        let bytes = decode(&attachment.data)?;
        let extension = extension_for(&attachment.mime_type)
            .ok_or_else(|| unsupported_type(&attachment.mime_type))?;
        let path = self.dir.join(format!("{}.{extension}", sha256_hex(&bytes)));
        let _guard = self
            .write_lock
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if path.exists() {
            return Ok(path);
        }
        // A temp file plus a rename: the agent reads this path from another
        // process, and it must never observe a half-written image.
        crate::atomic::atomic_write(&path, &bytes).map_err(|error| {
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
        let bytes = b"the bytes of one small png";

        let path = session
            .materialize(&attachment("photo.png", "image/png", &encoded(bytes)))
            .expect("materialized");

        assert_eq!(
            path.file_name()
                .and_then(|name| name.to_str())
                .map(str::to_string),
            Some(format!("{}.png", sha256_hex(bytes)))
        );
        assert_eq!(std::fs::read(&path).expect("read"), bytes);
        assert!(path.starts_with(&temp.0));
    }

    #[test]
    fn the_users_name_never_reaches_the_path() {
        let temp = TempDir::new();
        let store = AttachmentStore::new(&temp.0);
        let session = store.session("s.a.1").expect("session");
        let bytes = b"payload";

        let path = session
            .materialize(&attachment(r"..\..\evil.png", "image/png", &encoded(bytes)))
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
        let first = attachment("one.png", "image/png", &encoded(b"same image"));
        let second = attachment("two.png", "image/png", &encoded(b"same image"));

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
            .materialize(&attachment("a.png", "image/png", &encoded(b"one")))
            .expect("first");
        let second = session
            .materialize(&attachment("b.png", "image/png", &encoded(b"two")))
            .expect("second");

        assert_ne!(first, second);
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
            .materialize(&attachment("a.png", "image/png", &encoded(b"old")))
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
            .materialize(&attachment("a.png", "image/png", &encoded(b"fresh")))
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
            .materialize(&attachment("a.png", "image/png", &encoded(b"clock skew")))
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
            .materialize(&attachment("b.png", "image/png", &encoded(b"kept")))
            .expect("materialized");
        store
            .session("s.a.1")
            .expect("session")
            .materialize(&attachment("a.png", "image/png", &encoded(b"closed")))
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
