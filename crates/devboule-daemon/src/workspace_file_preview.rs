//! The Files panel's full-size preview: stage one workspace file into the
//! runtime directory's `previews` folder, and revoke by deleting the copy.
//!
//! Orchestration only, like [`crate::workspace_file_read`]: resolve the
//! folder from a workspace id — never a request field — confine the path
//! with the same two-layer rule the listing and the read use (lexical
//! components plus a walk that refuses links and junctions, both through
//! [`crate::workspace_git_support`], so the sentences are shared), and only
//! then copy. The copy exists because a concession to Tauri's asset protocol
//! cannot be withdrawn until the process restarts — the scope API has no
//! `disallow_file` — so revocation is expressed by the path instead: the
//! app concedes this one folder, the panel's copy is the only thing that
//! ever rests in it, and unstage deletes it.
//!
//! Two consequences are load-bearing and stated here rather than left to a
//! caller. The **daemon** makes the copy: an app-side `join` + `copy` would
//! follow a link swapped between the listing and the preview
//! (`std::fs::copy` follows links), which is exactly what this module's
//! walk refuses. And every stage clears the folder first, so at most one
//! copy rests there — the panel's revoke cannot be defeated by an older
//! copy left behind, and a file that changed between two previews gets a
//! fresh name (the copy is named from the file's own stat).
//!
//! What the app concedes is `<runtime dir>/previews/*` (`tauri.conf.json`,
//! `$CACHE/Devboule/previews/*`), which must name this module's folder byte
//! for byte — the trap being that Tauri's `$CACHE` resolves to
//! `%LOCALAPPDATA%` without the app identifier while `$APPLOCALDATA`
//! carries one, and the runtime dir has none (`crate::paths`). The daemon's
//! start sweeps this folder ([`sweep`]), so a copy a killed process left
//! dies with the restart.
//!
//! # The residual, stated plainly
//!
//! After a successful unstage **nothing this module staged is readable
//! through the asset protocol**: the scope still matches the path (a
//! concession cannot be taken back until the process ends), but the file
//! behind it is gone, and the protocol answers a missing file with 404 —
//! the revoke is the deletion, and it is complete when the deletion is.
//! What can outlive a revoke, in order of how much it matters:
//!
//! - **A copy whose stage landed after the last unstage.** The panel sends
//!   an unstage before every new load and on close, but a stage already in
//!   flight when that unstage is processed can still create its copy a
//!   moment later; the panel has then discarded the reply and no revoke
//!   follows until the next stage, the next selection change or the next
//!   start. Bounded by those three, never by a timer, and the copy is
//!   never displayed (its URL was thrown away with the reply).
//! - **A copy left by a process that dies mid-stage** — the half of a
//!   `reset_folder` + `copy` that completed. Removed by [`sweep`] at the
//!   next start, or by the next stage's own clear.
//! - **Bytes the webview already fetched.** The asset protocol sets no
//!   cache headers either way (tauri `protocol/asset.rs`), so whether
//!   WebView2 retains a decoded image after the copy is deleted is
//!   unverified; what is verified is the file side: the URL 404s from the
//!   moment the copy is gone.

use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use devboule_protocol::{
    DaemonMessage, ErrorCode, WireError, WorkspaceFilePreview, WorkspaceFilePreviewStatus,
};
use sha2::{Digest, Sha256};

use crate::workspace_file_read::{stamped, IMAGE_EXTENSIONS};
use crate::workspace_files::{names_git_metadata, DOES_NOT_EXIST, NOT_PART_OF_THE_TREE};
use crate::workspace_git_diff::NOT_A_FILE;
use crate::workspace_git_support::{confined, walk, Walked, OUTSIDE_THE_WORKSPACE};
use crate::ServerState;

/// The runtime-dir subfolder the app concedes to the asset protocol
/// (`tauri.conf.json`: `$CACHE/Devboule/previews/*`). That glob carries one
/// level (`*` with `require_literal_separator`), so copies rest directly in
/// this folder — a subfolder of it would be outside the scope and the
/// webview's read would be refused.
const PREVIEWS_DIR: &str = "previews";

/// The video half of what the panel draws as media, by spelling: what
/// WebView2 decodes in a `<video>` element. `mov`/`mkv`/`avi` are absent on
/// purpose — the panel shows them as the binary sentence the read already
/// gives, rather than staging a copy nothing here can play. The image half
/// is [`IMAGE_EXTENSIONS`]; the panel's mirror of both lists lives in
/// `src/features/workspace/previewMedia.ts`, and an extension joins both
/// mirrors or neither.
const VIDEO_EXTENSIONS: [&str; 3] = ["m4v", "mp4", "webm"];

/// The sentence for a file whose extension is in neither list: a static
/// claim about the preview, no path and no OS error, like every sentence
/// this panel shows. `svg` lands here with the text formats on purpose —
/// it reads better as text, and the read hands it back as text.
const NOT_SHOWABLE: &str = "this file's type is not shown in the preview";

/// The sentence for a copy or a removal the filesystem refused. Static, like
/// [`crate::workspace_file_read`]'s read-failure sentence: no OS error
/// string travels, because those carry paths.
const COPY_FAILED: &str = "the preview copy could not be written";
const REMOVE_FAILED: &str = "the preview copy could not be removed";

/// Serializes stage against stage and unstage: the folder's "at most one
/// copy" invariant is written here, and two panel clicks are two threads
/// behind this bridge, so the clear + copy of one act must not interleave
/// with the clear of the next. A poisoned lock is still this lock — no
/// panic elsewhere must stop the panel from revoking.
static FOLDER_LOCK: Mutex<()> = Mutex::new(());

/// The [`ClientMessage::WorkspaceFilePreviewStage`] arm: confine, guard,
/// copy, answer with the copy's path.
pub(crate) fn reply_stage(
    state: &ServerState,
    id: u64,
    workspace_id: &str,
    path: &str,
) -> DaemonMessage {
    let staged = match state.sessions.workspace_cwd(workspace_id) {
        Ok(root) => copy_of(&root, path, &previews_of(state.sessions.runtime_dir())),
        // The sentence comes from the registry and names no path (see
        // `workspace_cwd`): it is safe to echo on this frame.
        Err(error) => refused(error.message),
    };
    DaemonMessage::WorkspaceFilePreviewStaged { id, staged }
}

/// The [`ClientMessage::WorkspaceFilePreviewUnstage`] arm: delete every
/// copy. The reply is the daemon's own `Ok` — the panel asks for no path
/// back, because the only path it would name is one that must no longer
/// exist.
pub(crate) fn reply_unstage(state: &ServerState, id: u64) -> DaemonMessage {
    if clear(&previews_of(state.sessions.runtime_dir())) {
        DaemonMessage::Ok { id }
    } else {
        DaemonMessage::Error(WireError::new(ErrorCode::Io, REMOVE_FAILED).with_id(id))
    }
}

/// The daemon's start: drop every copy a previous process left. A failed
/// removal is not reported — nothing waits on this sweep, the next stage's
/// own clear retries it, and a start must not fail over a locked file.
pub(crate) fn sweep(runtime_dir: &Path) {
    let _ = clear(&previews_of(runtime_dir));
}

/// This daemon's staging folder, under the runtime directory the process
/// took its lock file from (`crate::paths`) — the same directory the app's
/// own `RuntimePaths::from_env` resolves, which is what makes the static
/// scope in `tauri.conf.json` cover it.
fn previews_of(runtime_dir: &Path) -> PathBuf {
    runtime_dir.join(PREVIEWS_DIR)
}

/// One file's stage, over a real root. Private on purpose: callers outside
/// this module arrive through [`reply_stage`], and the test module is a
/// child.
fn copy_of(root: &Path, requested: &str, previews: &Path) -> WorkspaceFilePreview {
    // The folder itself — in any spelling (``, `.`, `./`) — is a folder:
    // not an escape, and nothing to copy. Checked ahead of the confinement,
    // which would call the empty spelling an escape.
    if Path::new(requested)
        .components()
        .all(|component| matches!(component, Component::CurDir))
    {
        return refused(NOT_A_FILE);
    }
    // Confinement first and without a process: nothing below touches a path
    // this check has not put inside `root`.
    let Some(target) = confined(root, requested) else {
        return refused(OUTSIDE_THE_WORKSPACE);
    };
    // The listing's own guard, before anything else looks at the spelling:
    // no part of the repository's metadata is ever copied out, in any
    // spelling Win32 resolves — and before the extension gate below, so the
    // sentence stays the tree's own even for a metadata entry whose name
    // happens to end in a shown extension.
    if names_git_metadata(requested) {
        return refused(NOT_PART_OF_THE_TREE);
    }
    // Then the walk: the confinement above trusts the spelling, this trusts
    // the filesystem — every component stat'ed without following, a link
    // refused with the shared sentence, a component with no stat ending the
    // walk in the shared "does not exist". Exactly the read's order, so a
    // path refused here is refused with the sentence the panel already
    // shows for it.
    match walk(root, requested) {
        Walked::Link(sentence) => refused(sentence),
        Walked::Missing => refused(DOES_NOT_EXIST),
        Walked::Inside(metadata) => {
            if !metadata.is_file() {
                return refused(NOT_A_FILE);
            }
            // The one gate beyond the read's, last — after every check the
            // read makes, so its sentences stay identical: the stage exists
            // for files the panel draws as media, and this folder is the
            // one the webview can read without this module's confinement —
            // so a file whose type is never drawn cannot be copied into it,
            // even though the read would answer it under its 128 KiB cap.
            let Some(extension) = showable_extension(requested) else {
                return refused(NOT_SHOWABLE);
            };
            stage_file(root, &target, requested, &extension, previews, &metadata)
        }
    }
}

/// The write itself, under the folder lock: the folder emptied, the copy
/// made, the copy's path answered. A refusal here — a folder that could not
/// be cleared, a copy that failed — leaves no half-folder pretending to be
/// a stage: either the folder holds exactly this copy or the reply is the
/// sentence above.
fn stage_file(
    root: &Path,
    target: &Path,
    requested: &str,
    extension: &str,
    previews: &Path,
    metadata: &std::fs::Metadata,
) -> WorkspaceFilePreview {
    let _guard = FOLDER_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if !reset_folder(previews) {
        return refused(COPY_FAILED);
    }
    let destination = previews.join(copy_name(root, requested, extension, metadata));
    if std::fs::copy(target, &destination).is_err() {
        return refused(COPY_FAILED);
    }
    staged_copy(&destination, metadata)
}

/// The folder empty and creatable, or `false`. "Already absent" is a
/// success — the normal first stage of a process — while any other removal
/// failure is a refusal: copying beside a stale copy would put two files in
/// a folder whose whole meaning is that the one under the panel's URL is
/// the only one.
fn reset_folder(previews: &Path) -> bool {
    match std::fs::remove_dir_all(previews) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return false,
    }
    std::fs::create_dir_all(previews).is_ok()
}

/// Delete the folder and everything staged in it. `true` when nothing of it
/// is left — including when there was nothing to delete: revoking twice, or
/// revoking before any stage, is not a failure.
fn clear(previews: &Path) -> bool {
    let _guard = FOLDER_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    match std::fs::remove_dir_all(previews) {
        Ok(()) => true,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => false,
    }
}

/// The copy's file name: `<digest>.<extension>`, the digest taken over the
/// file's identity — root, requested spelling, size, mtime — never over a
/// spelling the caller chose alone. Three properties the panel leans on:
/// the name can only contain hex and the (validated, lower-cased) shown
/// extension, so traversal is not expressible; two previews of the same
/// unchanged file get the same name and therefore the same asset URL (which
/// the webview may cache); and any edit — new size or mtime — gets a new
/// name, so a stale cached response can never stand in for a changed file.
fn copy_name(
    root: &Path,
    requested: &str,
    extension: &str,
    metadata: &std::fs::Metadata,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(root.to_string_lossy().as_bytes());
    hasher.update([0]);
    hasher.update(requested.as_bytes());
    hasher.update([0]);
    hasher.update(metadata.len().to_le_bytes());
    hasher.update(stamped(metadata).unwrap_or(0).to_le_bytes());
    let stem: String = hasher
        .finalize()
        .iter()
        .take(16)
        .map(|byte| format!("{byte:02x}"))
        .collect();
    format!("{stem}.{extension}")
}

/// The requested spelling's extension when the panel draws files of it as
/// media — lower-cased, which is both how the lists are spelled and the
/// extension the copy keeps, so the asset protocol's own type sniff reads
/// the same word the `<img>`/`<video>`/`<embed>` element expects. `None`
/// for no extension, and for everything outside the lists.
fn showable_extension(requested: &str) -> Option<String> {
    let extension = Path::new(requested)
        .extension()?
        .to_string_lossy()
        .to_ascii_lowercase();
    let showable = IMAGE_EXTENSIONS.contains(&extension.as_str())
        || VIDEO_EXTENSIONS.contains(&extension.as_str())
        || extension == "pdf";
    showable.then_some(extension)
}

/// A stage that answered with a copy: the path the app concedes, and the
/// source file's own stat — the header shows the same numbers the read's
/// `ok` would have shown.
fn staged_copy(path: &Path, metadata: &std::fs::Metadata) -> WorkspaceFilePreview {
    WorkspaceFilePreview {
        status: WorkspaceFilePreviewStatus::Ok,
        path: Some(path.to_string_lossy().into_owned()),
        size: Some(metadata.len()),
        modified_at: stamped(metadata),
        error: None,
    }
}

/// A refusal to stage: the sentence says what stopped it, and — per the
/// carve-outs on the type — nothing else travels with it.
fn refused(sentence: impl Into<String>) -> WorkspaceFilePreview {
    WorkspaceFilePreview {
        status: WorkspaceFilePreviewStatus::Refused,
        path: None,
        size: None,
        modified_at: None,
        error: Some(sentence.into()),
    }
}

#[cfg(test)]
#[path = "workspace_file_preview_tests.rs"]
mod tests;
