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
//! Three consequences are load-bearing and stated here rather than left to
//! a caller.
//!
//! **The daemon makes the copy — and opens the source exactly once.** An
//! app-side `join` + `copy` would follow a link swapped between the
//! listing and the preview, and so would a daemon that walked the path and
//! then reopened it by name: `std::fs::copy` follows links, which is
//! precisely what this module's walk refuses. So the stage opens the
//! source a single time, without resolving a link at the final component,
//! and proves **on that handle** what the walk proved on the path — not a
//! link, a regular file, still inside the workspace root — and copies from
//! the handle ([`verified_source`]). That closes the stat→open race the
//! shared module declares (`workspace_git_support::walk`): two seam tests
//! hold the swapper still and come back refused. On non-Windows the open
//! resolves links, so the binding there is the handle's identity against
//! the walk's own stat — the target's bytes are never read (its metadata
//! is), and a swap to a FIFO can still block that open: declared, and this
//! daemon starts on Windows (`crate::paths` asks for `LOCALAPPDATA`).
//!
//! **Every stage clears the folder first**, so at most one copy rests
//! there — the panel's revoke cannot be defeated by an older copy left
//! behind, and a file that changed between two previews gets a fresh name
//! (the copy is named from the file's own stat).
//!
//! **There is no TTL and no periodic sweep.** Nothing runs while the panel
//! sits idle: a copy rests until the next stage, the next unstage, or the
//! next start ([`sweep`]) — and at most one copy ever rests.
//!
//! What the app concedes is `<runtime dir>/previews/*` (`tauri.conf.json`,
//! `$CACHE/Devboule/previews/*`), which must name this module's folder byte
//! for byte — the trap being that Tauri's `$CACHE` resolves to
//! `%LOCALAPPDATA%` without the app identifier while `$APPLOCALDATA`
//! carries one, and the runtime dir has none (`crate::paths`). Beside that
//! static default the app concedes, at start, the `previews` folder of the
//! runtime dir this process actually resolves — so a
//! `DEVBOULE_RUNTIME_DIR` override stages into a conceded folder too
//! (`src-tauri/src/preview_scope.rs`). The daemon's start sweeps this
//! folder ([`sweep`]), so a copy a killed process left dies with the
//! restart.
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

use std::fs::{File, OpenOptions};
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use devboule_protocol::{
    DaemonMessage, ErrorCode, WireError, WorkspaceFilePreview, WorkspaceFilePreviewStatus,
};
use sha2::{Digest, Sha256};

use crate::workspace_file_read::{stamped, IMAGE_EXTENSIONS};
use crate::workspace_files::{names_git_metadata, DOES_NOT_EXIST, NOT_PART_OF_THE_TREE};
use crate::workspace_git_diff::NOT_A_FILE;
use crate::workspace_git_support::{confined, walk, Walked, LINK_FINAL, OUTSIDE_THE_WORKSPACE};
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

/// The sentence for a source that would not open — after the walk said it
/// exists and the extension gate said the panel draws it. Static, like
/// every sentence here: the OS error is dropped, because it carries a path.
const OPEN_FAILED: &str = "the file could not be opened for the preview";

/// The sentence for an open this code cannot vouch for: the handle's own
/// stat or its resolved path came back unusable, so "inside the workspace"
/// cannot be affirmed — and an unaffirmed location is a refusal, never a
/// copy.
const LOCATION_UNCONFIRMED: &str =
    "the file's location could not be confirmed inside the workspace";

/// Non-Windows only: what the handle holds is not the object the walk
/// stat'ed — a component under it was replaced between the two (a symlink
/// resolved through, a folder redirected). The identity of an open handle
/// cannot be forged by such a swap, and no path is re-examined to say it.
#[cfg(not(windows))]
const REPLACED_WHILE_STAGED: &str = "the file changed after it was checked; nothing was copied";

/// Windows only: `FILE_FLAG_OPEN_REPARSE_POINT` — the flag that opens the
/// final component as itself instead of resolving it, so a link planted
/// over the validated name yields a handle **to the link** (refused on its
/// attribute below) and its target is never touched, not even stat'ed.
/// Hardcoded the way this house hardcodes `0x400` in
/// `workspace_git_support::is_reparse_point`.
#[cfg(windows)]
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

/// Serializes stage against stage and unstage: the folder's "at most one
/// copy" invariant is written here, and two panel clicks are two threads
/// behind this bridge, so the clear + copy of one act must not interleave
/// with the clear of the next. A poisoned lock is still this lock — no
/// panic elsewhere must stop the panel from revoking.
static FOLDER_LOCK: Mutex<()> = Mutex::new(());

/// The seam the race tests drive: it runs between the walk's verdict and
/// the open of the source — the exact window `workspace_git_support::walk`
/// declares as the stat→open race ("no test holds a swapper still"). A
/// test installs the swap here; [`verified_source`] has to refuse what the
/// open then finds. Per thread because each `#[test]` runs on its own, so
/// one test's swapper can never fire inside another's.
#[cfg(test)]
pub(crate) mod between {
    use std::cell::RefCell;

    thread_local! {
        static AFTER_WALK: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
    }

    /// Install the swap for the next stage **on this thread**. It fires
    /// once — `run` takes it — so a test that never reaches the stage
    /// cannot leak its swapper into anything.
    pub(crate) fn install(after_walk: Box<dyn FnOnce()>) {
        AFTER_WALK.with(|slot| *slot.borrow_mut() = Some(after_walk));
    }

    pub(crate) fn run() {
        if let Some(swapper) = AFTER_WALK.with(|slot| slot.borrow_mut().take()) {
            swapper();
        }
    }
}

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

/// Open the source once and prove on the handle what the walk proved on
/// the path: not a link (the final component never resolved), a regular
/// file, still inside `root` — then hand back the handle to copy from.
/// Every error is a static sentence; the OS error itself is dropped,
/// because it carries a path.
///
/// The binding is per platform and it is the point of the function:
/// Windows asks the handle where it *really* lives
/// (`GetFinalPathNameByHandle` reports a component swapped for a junction
/// as the outside path the open resolved through); the rest resolves
/// links at open and compares the handle's own identity with the walk's
/// stat, which no swap can keep equal. Neither branch is a second look at
/// the path — the path already lied once, which is why everything here
/// reads the handle.
fn verified_source(
    root: &Path,
    target: &Path,
    walked: &std::fs::Metadata,
) -> Result<(File, std::fs::Metadata), &'static str> {
    let file = open_source(target).map_err(|_| open_failure_sentence(target))?;
    let metadata = match file.metadata() {
        Ok(metadata) => metadata,
        // A handle this code opened but cannot stat: choose the sentence
        // from the path instead — a refusal either way, and a link that
        // got here still gets the link's words.
        Err(_) => return Err(open_failure_sentence(target)),
    };
    if opened_final_is_link(&metadata) {
        return Err(LINK_FINAL);
    }
    if !metadata.is_file() {
        return Err(NOT_A_FILE);
    }
    confirm_binding(&file, root, walked)?;
    Ok((file, metadata))
}

/// The sentence for a source that would not open. The path is stat'ed
/// only to *choose* words for a stage that is already refused — it gates
/// nothing, and nothing is read through it.
fn open_failure_sentence(target: &Path) -> &'static str {
    match std::fs::symlink_metadata(target) {
        Ok(metadata) if opened_final_is_link(&metadata) => LINK_FINAL,
        Ok(_) => OPEN_FAILED,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => DOES_NOT_EXIST,
        Err(_) => LOCATION_UNCONFIRMED,
    }
}

/// One open of the source, without resolving a link at the final
/// component, so a link planted over the validated name is opened as the
/// link it is and its target is never touched.
#[cfg(windows)]
fn open_source(target: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(target)
}

/// One open of the source. Plain on non-Windows: links are resolved here,
/// which is what the identity half of [`confirm_binding`] then refuses —
/// the target may be *stat'ed* through that follow, never read: its bytes
/// are copied only from a handle that matched the walk's own stat.
#[cfg(not(windows))]
fn open_source(target: &Path) -> std::io::Result<File> {
    OpenOptions::new().read(true).open(target)
}

/// The house predicate for "this stat is a link", spelled without
/// `is_symlink()`: whether a junction answers to that label is a
/// measurement this house has seen flip (`workspace_git_support`), while
/// the attribute never has.
#[cfg(windows)]
fn opened_final_is_link(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0 // FILE_ATTRIBUTE_REPARSE_POINT
}

/// Non-Windows spelling of the same predicate, on a stat — where a symlink
/// is a symlink and the label has no junction to disagree about.
#[cfg(not(windows))]
fn opened_final_is_link(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

/// The binding, on Windows: where the handle *really* lives. The walk's
/// stat carries no identity stable Rust can read (`file_index` is still
/// unstable), so the proof is the handle's own resolved path —
/// `GetFinalPathNameByHandle` reports what the open actually traversed,
/// which makes a component swapped for a junction show up as the outside
/// path it leads to, and a handle whose location cannot even be named is
/// a refusal rather than a copy.
#[cfg(windows)]
fn confirm_binding(
    file: &File,
    root: &Path,
    _walked: &std::fs::Metadata,
) -> Result<(), &'static str> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{GetFinalPathNameByHandleW, VOLUME_NAME_DOS};

    let root = std::fs::canonicalize(root).map_err(|_| LOCATION_UNCONFIRMED)?;
    let mut buffer = vec![0u16; 1024];
    let length = loop {
        let written = unsafe {
            GetFinalPathNameByHandleW(
                file.as_raw_handle() as _,
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                VOLUME_NAME_DOS,
            )
        };
        if written == 0 {
            return Err(LOCATION_UNCONFIRMED);
        }
        // Success excludes the terminating null (it fits); too small
        // returns the size *including* it — exactly the length to retry
        // with, and `+ 1` where they are equal so the loop cannot stall.
        if (written as usize) < buffer.len() {
            break written as usize;
        }
        if written > 32_768 {
            return Err(LOCATION_UNCONFIRMED);
        }
        buffer.resize((written as usize).max(buffer.len() + 1), 0);
    };
    let resolved = PathBuf::from(OsString::from_wide(&buffer[..length]));
    if resolved.starts_with(&root) {
        Ok(())
    } else {
        Err(OUTSIDE_THE_WORKSPACE)
    }
}

/// The binding, on non-Windows: the handle's own identity (`dev`, `ino` —
/// both readable from the walk's lstat there) against the identity the
/// walk stat'ed. No swap of any component keeps them equal, and no path
/// is re-examined — the handle answers.
#[cfg(not(windows))]
fn confirm_binding(
    file: &File,
    _root: &Path,
    walked: &std::fs::Metadata,
) -> Result<(), &'static str> {
    use std::os::unix::fs::MetadataExt;
    let opened = file.metadata().map_err(|_| LOCATION_UNCONFIRMED)?;
    if (opened.dev(), opened.ino()) == (walked.dev(), walked.ino()) {
        Ok(())
    } else {
        Err(REPLACED_WHILE_STAGED)
    }
}

/// The write itself: open-once and verify first (a refusal here happens
/// before the folder is touched, so a swap cannot even destroy the
/// previous copy), then the folder lock, the reset, and a streaming copy
/// **from the handle** — the name of the source is never reopened.
/// Either the folder ends up holding exactly this copy or the reply is a
/// sentence; a copy that fails halfway has its half removed again.
fn stage_file(
    root: &Path,
    target: &Path,
    requested: &str,
    extension: &str,
    previews: &Path,
    walked: &std::fs::Metadata,
) -> WorkspaceFilePreview {
    // The seam, first thing after the walk's verdict: the swap a race
    // test plants must land between that verdict and the open below.
    #[cfg(test)]
    between::run();
    let (mut source, source_metadata) = match verified_source(root, target, walked) {
        Ok(verified) => verified,
        Err(sentence) => return refused(sentence),
    };
    let _guard = FOLDER_LOCK
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if !reset_folder(previews) {
        return refused(COPY_FAILED);
    }
    let destination = previews.join(copy_name(root, requested, extension, &source_metadata));
    // `create_new`: the folder was just reset, so a name that already
    // exists is a bug rather than a file to overwrite.
    let mut written = match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&destination)
    {
        Ok(written) => written,
        Err(_) => return refused(COPY_FAILED),
    };
    if std::io::copy(&mut source, &mut written).is_err() {
        drop(written);
        let _ = std::fs::remove_file(&destination);
        return refused(COPY_FAILED);
    }
    staged_copy(&destination, &source_metadata)
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
/// The converse is the declared limit of a stat-named snapshot: bytes that
/// change while size and mtime stay identical keep the name, and with it
/// the URL — the name answers for the stat, never for the content, and
/// this stage is a copy of the bytes at one moment, never a re-read.
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
