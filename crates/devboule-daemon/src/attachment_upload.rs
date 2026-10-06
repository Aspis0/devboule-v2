//! The in-progress half of a chunked file upload: the staged part file, the
//! offset it stands at, and the declaration it was opened under.
//!
//! The bytes are the store's, not this module's: a part file lives in the
//! session's own attachment folder, under the same owner-only hardening, and
//! every byte appended to it is charged to the store's budget as it arrives —
//! `AttachmentStore::append_staged` keeps the check and the charge in one
//! critical section. What lives here is the one fact the disk does not carry —
//! where each part stands and what it promised — because a chunk that does not
//! start at that offset must be refused rather than appended blind.
//!
//! An upload survives a lost connection while the daemon lives: the client
//! names the same upload id and asks for its offset. After a daemon restart the
//! entry is gone and the restart is a refusal; the client aborts and opens a
//! fresh one. A part the restart left behind is charged to the session's budget
//! (the walk counts it), and the next `begin` under the same id truncates it
//! and gives those bytes back.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

use devboule_protocol::{
    invalid_base64_message, sanitize_attachment_name, validate_upload_begin, validate_upload_chunk,
    validate_upload_id, ErrorCode, WireError,
};

use crate::attachment_store::{AttachmentStore, Deposited};

/// How many uploads one session may have open at once.
///
/// The composer sends one file at a time, so this is the backstop against a
/// client that opens a hundred: each open upload is a part file the session's
/// budget already charges, and the cap keeps the map and the folder bounded.
const MAX_CONCURRENT_UPLOADS_PER_SESSION: usize = 4;

struct InProgress {
    session_id: String,
    /// The sanitized display name; the store's extension and the prompt line
    /// both come from it.
    name: String,
    total_bytes: u64,
    received: u64,
    staged: PathBuf,
    /// True from the moment `finish` starts until it settles. A second
    /// `finish`, a `chunk`, a `status`, an `abort` or a re-`begin` refuses
    /// while set, so a re-`begin` cannot truncate the file being admitted.
    finishing: bool,
}

/// One opened upload, copied out of the map so the admission runs without the
/// map lock held across a 50 MiB hash.
struct OpenedUpload {
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
                if open.finishing {
                    return Err(refusal(
                        "That upload is being stored; start a new one.".to_string(),
                    ));
                }
                return Ok(open.received);
            }
            return Err(refusal(
                "An upload with that id is already in progress for another file.".to_string(),
            ));
        }
        if state
            .values()
            .filter(|open| open.session_id == session_id)
            .count()
            >= MAX_CONCURRENT_UPLOADS_PER_SESSION
        {
            return Err(refusal(format!(
                "This session already has {MAX_CONCURRENT_UPLOADS_PER_SESSION} uploads in flight."
            )));
        }
        let staged = store.create_staged(session_id, upload_id)?;
        state.insert(
            upload_id.to_string(),
            InProgress {
                session_id: session_id.to_string(),
                name,
                total_bytes,
                received: 0,
                staged,
                finishing: false,
            },
        );
        Ok(0)
    }

    /// How many bytes the daemon holds for one upload.
    pub(crate) fn status(&self, session_id: &str, upload_id: &str) -> Result<u64, WireError> {
        let state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        match state.get(upload_id) {
            Some(open) if open.session_id == session_id && !open.finishing => Ok(open.received),
            _ => Err(not_in_progress()),
        }
    }

    /// Append one chunk at exactly the offset the upload stands at.
    pub(crate) fn chunk(
        &self,
        store: &AttachmentStore,
        session_id: &str,
        upload_id: &str,
        offset: u64,
        data: &str,
    ) -> Result<u64, WireError> {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        let open = match state.get_mut(upload_id) {
            Some(open) if open.session_id == session_id && !open.finishing => open,
            _ => return Err(not_in_progress()),
        };
        validate_upload_chunk(data, offset, open.received, open.total_bytes).map_err(refusal)?;
        let bytes = decode(data)?;
        store.append_staged(session_id, upload_id, &bytes)?;
        open.received += bytes.len() as u64;
        Ok(open.received)
    }

    /// Admit a fully received upload as a stored attachment, and report the
    /// sanitized name the reference carries.
    ///
    /// The length on disk is compared with the declaration before anything is
    /// admitted: the in-memory counter and the file can drift (a short write, a
    /// resumed part), and the store's digest names whatever bytes are really
    /// there. A mismatch discards the part and releases its bytes, the way
    /// every other refusal does.
    pub(crate) fn finish(
        &self,
        store: &AttachmentStore,
        session_id: &str,
        upload_id: &str,
    ) -> Result<(Deposited, String), WireError> {
        let open = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            match state.get_mut(upload_id) {
                Some(open) if open.session_id == session_id && !open.finishing => {
                    if open.received != open.total_bytes {
                        return Err(refusal(format!(
                            "The upload has {} of its {} bytes; it cannot finish yet.",
                            open.received, open.total_bytes
                        )));
                    }
                    open.finishing = true;
                    OpenedUpload {
                        name: open.name.clone(),
                        total_bytes: open.total_bytes,
                        received: open.received,
                        staged: open.staged.clone(),
                    }
                }
                _ => return Err(not_in_progress()),
            }
        };
        let outcome = match std::fs::metadata(&open.staged) {
            Ok(metadata) if metadata.len() == open.total_bytes => {
                store.admit_upload(session_id, &open.name, &open.staged)
            }
            Ok(metadata) => {
                let _ = std::fs::remove_file(&open.staged);
                store.release_staged(session_id, open.received);
                Err(refusal(format!(
                    "The staged upload has {} of its {} bytes; it cannot finish yet.",
                    metadata.len(),
                    open.total_bytes
                )))
            }
            Err(error) => {
                store.release_staged(session_id, open.received);
                Err(WireError::new(
                    ErrorCode::Io,
                    format!("Could not read a staged upload: {error}"),
                ))
            }
        };
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        state.remove(upload_id);
        drop(state);
        outcome.map(|deposited| (deposited, open.name))
    }

    /// Discard one upload and the bytes received so far. An id the daemon does
    /// not hold is `Ok`: there is nothing left to discard.
    pub(crate) fn abort(
        &self,
        store: &AttachmentStore,
        session_id: &str,
        upload_id: &str,
    ) -> Result<(), WireError> {
        let removed = {
            let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
            match state.get(upload_id) {
                Some(open) if open.session_id == session_id && !open.finishing => {
                    state.remove(upload_id)
                }
                Some(open) if open.session_id == session_id => {
                    return Err(refusal(
                        "That upload is being stored; it cannot be aborted.".to_string(),
                    ))
                }
                Some(_) => return Err(not_in_progress()),
                None => None,
            }
        };
        if let Some(open) = removed {
            let _ = std::fs::remove_file(&open.staged);
            store.release_staged(session_id, open.received);
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
#[path = "attachment_upload_budget_tests.rs"]
mod budget_tests;
#[cfg(test)]
#[path = "attachment_upload_tests.rs"]
mod tests;
