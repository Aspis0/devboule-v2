//! The in-progress half of a chunked file upload: the staged part file, the
//! offset it stands at, and the declaration it was opened under.
//!
//! The bytes are the store's, not this module's: a part file lives in the
//! session's own attachment folder, under the same owner-only hardening, so a
//! close's `remove_dir_all` takes a half-upload with it. What lives here is the
//! one fact the disk does not carry — where each part stands and what it
//! promised — because a chunk that does not start at that offset must be
//! refused rather than appended blind.
//!
//! An upload survives a lost connection while the daemon lives: the client
//! names the same upload id and asks for its offset. After a daemon restart the
//! entry is gone and the restart is a refusal; the client aborts and opens a
//! fresh one. A part the restart left behind is charged to the session's budget
//! until the folder is closed or the retention sweep takes it, and the next
//! `begin` under the same id truncates it.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use devboule_protocol::{
    invalid_base64_message, sanitize_attachment_name, validate_upload_begin, validate_upload_chunk,
    validate_upload_id, ErrorCode, WireError,
};

use crate::attachment_store::AttachmentStore;

/// The suffix every staged part file carries. Not a stored extension
/// (`STORED_EXTENSIONS`), so no `resolve` can ever name one.
const PART_SUFFIX: &str = ".part";

struct InProgress {
    session_id: String,
    /// The sanitized display name; the store's extension and the prompt line
    /// both come from it.
    name: String,
    total_bytes: u64,
    received: u64,
    staged: PathBuf,
}

#[derive(Default, Clone)]
pub(crate) struct AttachmentUploads {
    state: std::sync::Arc<Mutex<HashMap<String, InProgress>>>,
}

impl AttachmentUploads {
    /// Open one upload, or adopt the one already in progress under the same id
    /// and the same declaration and answer its offset.
    pub(crate) fn begin(
        &self,
        store: &AttachmentStore,
        session_id: &str,
        upload_id: &str,
        name: &str,
        total_bytes: u64,
    ) -> Result<u64, WireError> {
        validate_upload_id(upload_id).map_err(refusal)?;
        validate_upload_begin(total_bytes).map_err(refusal)?;
        let name = sanitize_attachment_name(name);
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(open) = state.get(upload_id) {
            if open.session_id == session_id && open.name == name && open.total_bytes == total_bytes
            {
                return Ok(open.received);
            }
            return Err(refusal(
                "An upload with that id is already in progress for another file.".to_string(),
            ));
        }
        let dir = store.prepare_upload_dir(session_id)?;
        let staged = dir.join(format!("upload-{upload_id}{PART_SUFFIX}"));
        // Truncate: the id is the client's, and a part left by a restarted
        // daemon is not resumable (the map is empty), so this upload starts at
        // zero where the old one stood.
        std::fs::File::create(&staged).map_err(|error| {
            WireError::new(
                ErrorCode::Io,
                format!("Could not stage an uploaded file: {error}"),
            )
        })?;
        state.insert(
            upload_id.to_string(),
            InProgress {
                session_id: session_id.to_string(),
                name,
                total_bytes,
                received: 0,
                staged,
            },
        );
        Ok(0)
    }

    /// How many bytes the daemon holds for one upload.
    pub(crate) fn status(&self, session_id: &str, upload_id: &str) -> Result<u64, WireError> {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        match state.get(upload_id) {
            Some(open) if open.session_id == session_id => Ok(open.received),
            _ => Err(not_in_progress()),
        }
    }

    /// Append one chunk at exactly the offset the upload stands at.
    pub(crate) fn chunk(
        &self,
        session_id: &str,
        upload_id: &str,
        offset: u64,
        data: &str,
    ) -> Result<u64, WireError> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let open = match state.get_mut(upload_id) {
            Some(open) if open.session_id == session_id => open,
            _ => return Err(not_in_progress()),
        };
        validate_upload_chunk(data, offset, open.received, open.total_bytes).map_err(refusal)?;
        let bytes = decode(data)?;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&open.staged)
            .map_err(|error| {
                WireError::new(
                    ErrorCode::Io,
                    format!("Could not stage an uploaded file: {error}"),
                )
            })?;
        use std::io::Write as _;
        file.write_all(&bytes).map_err(|error| {
            WireError::new(
                ErrorCode::Io,
                format!("Could not stage an uploaded file: {error}"),
            )
        })?;
        open.received += bytes.len() as u64;
        Ok(open.received)
    }

    /// Admit a fully received upload as a stored attachment, and report the
    /// sanitized name the reference carries.
    pub(crate) fn finish(
        &self,
        store: &AttachmentStore,
        session_id: &str,
        upload_id: &str,
    ) -> Result<(crate::attachment_store::Deposited, String), WireError> {
        let open = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            match state.get(upload_id) {
                Some(open) if open.session_id == session_id => {
                    if open.received != open.total_bytes {
                        return Err(refusal(format!(
                            "The upload has {} of its {} bytes; it cannot finish yet.",
                            open.received, open.total_bytes
                        )));
                    }
                    state
                        .remove(upload_id)
                        .expect("the entry was just looked up")
                }
                _ => return Err(not_in_progress()),
            }
        };
        let deposited = store.admit_upload(session_id, &open.name, &open.staged)?;
        Ok((deposited, open.name))
    }

    /// Discard one upload and the bytes received so far. An id the daemon does
    /// not hold is `Ok`: there is nothing left to discard.
    pub(crate) fn abort(&self, session_id: &str, upload_id: &str) -> Result<(), WireError> {
        let removed = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            match state.get(upload_id) {
                Some(open) if open.session_id == session_id => state.remove(upload_id),
                Some(_) => return Err(not_in_progress()),
                None => None,
            }
        };
        if let Some(open) = removed {
            let _ = std::fs::remove_file(&open.staged);
        }
        Ok(())
    }

    /// Drop every in-progress upload of one session.
    ///
    /// The part files live in the session's folder, so the close that removes
    /// that folder is already taking them; this is the map's half, so a later
    /// frame for the dead session is refused rather than handed a path that is
    /// gone.
    pub(crate) fn forget_session(&self, session_id: &str) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.retain(|_, open| open.session_id != session_id);
    }
}

fn not_in_progress() -> WireError {
    WireError::new(
        ErrorCode::InvalidRequest,
        "That upload is not in progress; start it again.".to_string(),
    )
}

fn refusal(message: String) -> WireError {
    WireError::new(ErrorCode::InvalidRequest, message)
}

fn decode(data: &str) -> Result<Vec<u8>, WireError> {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD
        .decode(data)
        .map_err(|_| refusal(invalid_base64_message()))
}

#[cfg(test)]
#[path = "attachment_upload_tests.rs"]
mod tests;
