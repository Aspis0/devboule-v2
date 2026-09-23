//! The content of one workspace file, for the Files panel's preview.
//! Orchestration only: resolve the folder from a workspace id — never a
//! request field — confine the requested path to it with the same two-layer
//! rule the listing uses (lexical components plus a walk that refuses links
//! and junctions, both through [`crate::workspace_git_support`], so the
//! sentences are shared and not copied), `stat` before reading so a file
//! over the cap is refused whole, and classify the bytes for the reply.
//! Nothing in this module writes.

use std::path::{Component, Path};
use std::time::UNIX_EPOCH;

use base64::Engine;
use devboule_protocol::{
    DaemonMessage, WorkspaceFileContent, WorkspaceFileContentKind, WorkspaceFileContentStatus,
};

use crate::workspace_files::{names_git_metadata, DOES_NOT_EXIST, NOT_PART_OF_THE_TREE};
use crate::workspace_git_diff::NOT_A_FILE;
use crate::workspace_git_support::{confined, walk, Walked, OUTSIDE_THE_WORKSPACE};
use crate::ServerState;

/// Bytes of one file this reply hands back. The frame is one JSON line of
/// at most [`devboule_protocol::MAX_FRAME_BYTES`] (1 MiB), and the worst
/// JSON escape is 6 bytes per control byte: 128 KiB × 6 = 768 KiB always
/// fits. Checked with the stat before any read — refused whole with the
/// measure, never cut short — and re-checked on the bytes actually read,
/// for the file that grows between the two.
const FILE_READ_MAX_BYTES: u64 = 128 * 1024;

/// The one sentence of this module that no other panel shows yet: the file
/// could not be opened or read. Static, like the rest — no OS error string
/// travels, because those carry paths.
const READ_FAILED: &str = "the file could not be read";

/// Extensions handed back as image content — recognized by spelling alone,
/// the way Paseo does it (`service.ts:91-95`), because an image's bytes are
/// binary and would otherwise never be shown. `svg` is absent on purpose:
/// it is text and reads better as text. Shared with
/// [`crate::workspace_file_preview`], whose stage gate needs this list for
/// the same reason; the panel's mirror of every extension the preview draws
/// lives in `src/features/workspace/previewMedia.ts` — an image extension
/// joins that mirror and this list or neither.
pub(crate) const IMAGE_EXTENSIONS: [&str; 9] = [
    "avif", "bmp", "gif", "ico", "jpeg", "jpg", "png", "tiff", "webp",
];

/// Resolve `workspace_id` through the registry — never a request field.
pub(crate) fn reply(state: &ServerState, id: u64, workspace_id: &str, path: &str) -> DaemonMessage {
    let file = match state.sessions.workspace_cwd(workspace_id) {
        Ok(root) => content_of(&root, path),
        // The sentence comes from the registry and names no path (see
        // `workspace_cwd`): it is safe to echo on this frame.
        Err(error) => refused(error.message),
    };
    DaemonMessage::WorkspaceFileContent { id, file }
}

/// The content of one file of one workspace. Private on purpose: callers
/// outside this module arrive through [`reply`], and the test module is a
/// child.
fn content_of(root: &Path, requested: &str) -> WorkspaceFileContent {
    // The folder itself — in any spelling (``, `.`, `./`) — is a folder:
    // not an escape, and no file content to hand back. Checked ahead of the
    // confinement, which would call the empty spelling an escape.
    if Path::new(requested)
        .components()
        .all(|component| matches!(component, Component::CurDir))
    {
        return refused(NOT_A_FILE);
    }
    // Confinement first and without a process: nothing below opens a path
    // this check has not put inside `root`.
    let Some(target) = confined(root, requested) else {
        return refused(OUTSIDE_THE_WORKSPACE);
    };
    // The listing's own guard, before the walk: no stat of the repository's
    // metadata folder happens at all, in any spelling Win32 resolves.
    if names_git_metadata(requested) {
        return refused(NOT_PART_OF_THE_TREE);
    }
    // Then the walk: the confinement above trusts the spelling, this trusts
    // the filesystem — every component stat'ed without following, a link
    // refused with the shared sentence, a component with no stat ending the
    // walk in the shared "does not exist".
    match walk(root, requested) {
        Walked::Link(sentence) => refused(sentence),
        Walked::Missing => refused(DOES_NOT_EXIST),
        Walked::Inside(metadata) => {
            if !metadata.is_file() {
                return refused(NOT_A_FILE);
            }
            if metadata.len() > FILE_READ_MAX_BYTES {
                return withheld(metadata.len(), stamped(&metadata));
            }
            let bytes = match std::fs::read(&target) {
                Ok(bytes) => bytes,
                Err(_) => return refused(READ_FAILED),
            };
            // The file can grow between the stat and the read: the cap is
            // re-checked on the bytes in hand, whose length is also the size
            // this reply claims from here on — the stamp stays the stat's.
            let size = bytes.len() as u64;
            if size > FILE_READ_MAX_BYTES {
                return withheld(size, stamped(&metadata));
            }
            if is_image(requested) {
                let base64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                return answered(WorkspaceFileContentKind::Image, base64, size, &metadata);
            }
            if bytes.contains(&0) {
                return binary(size, &metadata);
            }
            match String::from_utf8(bytes) {
                Ok(text) => answered(WorkspaceFileContentKind::Text, text, size, &metadata),
                // The UTF-8 half of the sniff: not decodable, so binary —
                // and binary on this wire means no content, never a lossy
                // re-decode of bytes the caller asked to see.
                Err(_) => binary(size, &metadata),
            }
        }
    }
}

/// A file whose bytes came back, with the stat's own numbers.
fn answered(
    kind: WorkspaceFileContentKind,
    content: String,
    size: u64,
    metadata: &std::fs::Metadata,
) -> WorkspaceFileContent {
    WorkspaceFileContent {
        status: WorkspaceFileContentStatus::Ok,
        kind: Some(kind),
        content: Some(content),
        size: Some(size),
        modified_at: stamped(metadata),
        error: None,
    }
}

/// A file read and classified as bytes with no text inside: the stat's
/// numbers travel, the content does not.
fn binary(size: u64, metadata: &std::fs::Metadata) -> WorkspaceFileContent {
    WorkspaceFileContent {
        status: WorkspaceFileContentStatus::Binary,
        kind: Some(WorkspaceFileContentKind::Binary),
        content: None,
        size: Some(size),
        modified_at: stamped(metadata),
        error: None,
    }
}

/// Content withheld for the cap: the measure is in the sentence, the real
/// size in `size` — refused whole, never cut short, like the diff's own.
fn withheld(size: u64, modified_at: Option<i64>) -> WorkspaceFileContent {
    WorkspaceFileContent {
        status: WorkspaceFileContentStatus::TooLarge,
        kind: None,
        content: None,
        size: Some(size),
        modified_at,
        error: Some(format!(
            "the file is larger than the {FILE_READ_MAX_BYTES}-byte content cap; its content is \
             not handed back"
        )),
    }
}

/// A refusal to answer: the sentence says what stopped it, and — per the
/// carve-outs on the type — nothing else travels with it.
fn refused(sentence: impl Into<String>) -> WorkspaceFileContent {
    WorkspaceFileContent {
        status: WorkspaceFileContentStatus::Refused,
        kind: None,
        content: None,
        size: None,
        modified_at: None,
        error: Some(sentence.into()),
    }
}

/// The stat's mtime, milliseconds since the epoch; `None` when the
/// filesystem gave no stamp. Shared with the preview's stage, which sends
/// the same number for the same file's own stat.
pub(crate) fn stamped(metadata: &std::fs::Metadata) -> Option<i64> {
    let time = metadata.modified().ok()?;
    Some(match time.duration_since(UNIX_EPOCH) {
        Ok(after) => after.as_millis() as i64,
        Err(before) => -(before.duration().as_millis() as i64),
    })
}

/// Whether the requested spelling names an image by its extension — the
/// decision Paseo makes before it sniffs, for the same reason.
fn is_image(requested: &str) -> bool {
    Path::new(requested).extension().is_some_and(|extension| {
        let extension = extension.to_string_lossy().to_ascii_lowercase();
        IMAGE_EXTENSIONS.contains(&extension.as_str())
    })
}

#[cfg(test)]
#[path = "workspace_file_read_tests.rs"]
mod tests;
