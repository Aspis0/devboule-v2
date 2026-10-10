//! The in-app editor's read and write road: Paseo's
//! `packages/server/src/server/file-explorer/service.ts`
//! (`readExplorerFile`, `getExplorerFileVersion`, `writeExplorerFile`,
//! `fileRevision`, `matchesExpectedRevision`, `isLikelyBinary`,
//! `isValidUtf8`, `MAX_EDITABLE_FILE_BYTES`), ported to this daemon line
//! by line (Apache-2.0, Copyright (c) 2025-present Mohamed Boudra).
//!
//! What is the same: the binary sniff (a NUL byte, or more than three in
//! ten suspicious control bytes, or bytes that are not UTF-8), the version as size plus stamp, the
//! expected-version check (a revision when one is named, the stamp
//! otherwise), and the atomic write — a temp file in the same folder,
//! `fsync`, `rename` — with a second stat before the rename so a change
//! inside the window answers `conflict` instead of overwriting. The mode
//! of the file being replaced is kept (Unix); a created file takes the
//! process umask default.
//!
//! What diverges, and why:
//!
//! - A missing file is not an error here. Paseo answers ENOENT and the
//!   editor stays shut; the open answers empty content with a `missing`
//!   version, and a write that names no expected version creates the file
//!   (owner decision). A write that names no version while anything is
//!   already there answers `conflict` with the ready version, so a retry
//!   after an outside creator lands on the conflict road, never on a
//!   silent overwrite.
//! - Parent folders are never made silently: when the parent is missing
//!   the write answers `the parent folder does not exist` and nothing is
//!   created.
//! - The confinement is this daemon's own two-layer rule (`confined` plus
//!   `walk` through [`crate::workspace_git_support`]), and the sentences
//!   shared with the other file roads stay shared: the workspace's own
//!   refusal words are reused where they say the same fact, while the
//!   editor-specific facts keep Paseo's exact strings (`Binary files
//!   cannot be edited`, `File is too large to edit`).
//! - The cap is 128 KiB, not Paseo's 1 MiB: a megabyte of content does
//!   not fit the 1 MiB frame once JSON-escaped (controls escape to six
//!   bytes), and an over-cap reply would take the connection down instead
//!   of answering. Bigger files stay on the windowed read-only road.
//! - The temp file marker names this daemon (`.devboule-<uuid>.tmp`);
//!   Paseo's is `.paseo-<uuid>.tmp`.
//! - There is no subscribe push on this wire: the editor polls
//!   [`reply_version`], which is the `fs.file.subscribe` +
//!   `fs.file.update` half mapped onto a request/reply pair.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use devboule_protocol::{
    DaemonMessage, WorkspaceEditableFile, WorkspaceFileContentStatus, WorkspaceFileVersion,
    WorkspaceFileWriteResult,
};

use crate::workspace_file_read::stamped;
use crate::workspace_files::{names_git_metadata, DOES_NOT_EXIST, NOT_PART_OF_THE_TREE};
use crate::workspace_git_diff::NOT_A_FILE;
use crate::workspace_git_support::{
    confined, crosses_a_link, walk, Walked, LINK_FINAL, OUTSIDE_THE_WORKSPACE,
};
use crate::ServerState;

/// Bytes of file this road hands back or takes. 128 KiB, like the
/// windowed read's own cap (`workspace_file_read.rs`): the frame is one
/// JSON line of at most [`devboule_protocol::MAX_FRAME_BYTES`] (1 MiB),
/// and serde_json escapes `"` and `\` as 2 bytes and every control byte
/// as 2-6 (`\n` is 2, the rest `\u00XX` is 6) — so 128 KiB of the worst
/// bytes is 768 KiB on the wire, always under the cap with margin to
/// spare for the envelope. A whole megabyte of content would not fit once
/// escaped, and an over-cap reply would take the connection down with it
/// instead of answering, so the cap lives in bytes here, not characters.
pub(crate) const MAX_EDITABLE_FILE_BYTES: u64 = 128 * 1024;

/// Paseo's `File is too large to edit`, kept verbatim: the sentence the
/// editor shows when either direction exceeds [`MAX_EDITABLE_FILE_BYTES`].
const TOO_LARGE: &str = "File is too large to edit";
/// Paseo's `Binary files cannot be edited`, kept verbatim: the sentence
/// the editor shows when the bytes on disk are not text.
const BINARY: &str = "Binary files cannot be edited";
/// The sentence a create answers with when the parent folder is not there.
/// Ours, in this daemon's own words: parents are never made silently.
const PARENT_MISSING: &str = "the parent folder does not exist";
/// The sentence a stat that fails for any reason but absence answers
/// with. Absence opens empty and creates; anything else (an ACL that
/// denies even the stat, an I/O error, an offline share) is a failed
/// check, never a missing file — offering those as empty editors would
/// let a create replace a file the daemon simply could not see.
const UNSTATABLE: &str = "the file could not be checked";

/// The sentence an app-file road answers with when the path is not
/// absolute (after `~` expansion). Ours: the human names a file on this
/// machine, never a workspace-relative spelling.
const NOT_ABSOLUTE: &str = "a file path must be absolute, or start with ~";

/// What a stat of the target says. `walk` answers `Missing` for *any*
/// `symlink_metadata` failure, so every missing arm re-asks here:
/// absence and inaccessibility take different roads from here on.
enum TargetState {
    Present(std::fs::Metadata),
    Absent,
    Unstatable,
}

fn classify_target(target: &Path) -> TargetState {
    match std::fs::symlink_metadata(target) {
        Ok(metadata) => TargetState::Present(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => TargetState::Absent,
        Err(_) => TargetState::Unstatable,
    }
}

/// Paseo's `fileRevision`: `dev:ino:size:mtimeNs` — except `std` exposes no
/// stable file id on every platform this daemon ships, so the revision is
/// `size:mtimeNs`. The stamp alone would alias a same-size rewrite inside
/// one millisecond tick; nanoseconds narrow that window, and the stat
/// before the rename closes what remains into a conflict rather than a
/// silent overwrite.
fn file_revision(size: u64, modified: SystemTime) -> String {
    let nanos: i128 = match modified.duration_since(UNIX_EPOCH) {
        Ok(after) => after.as_nanos() as i128,
        Err(before) => -(before.duration().as_nanos() as i128),
    };
    format!("{size}:{nanos}")
}

/// Paseo's `matchesExpectedRevision`: a named revision decides by string
/// equality, otherwise the millisecond stamp decides.
fn matches_expected(
    size: u64,
    modified: SystemTime,
    modified_ms: Option<i64>,
    expected_modified_at: Option<i64>,
    expected_revision: Option<&str>,
) -> bool {
    if let Some(expected) = expected_revision {
        return file_revision(size, modified) == expected;
    }
    match (modified_ms, expected_modified_at) {
        (Some(actual), Some(expected)) => actual == expected,
        _ => false,
    }
}

/// Paseo's `isLikelyBinary`: empty is text, a NUL byte is binary, and more
/// than three in ten suspicious control bytes is binary.
fn is_likely_binary(buffer: &[u8]) -> bool {
    if buffer.is_empty() {
        return false;
    }
    let mut suspicious = 0usize;
    for &byte in buffer {
        if byte == 0 {
            return true;
        }
        let is_control = byte < 32 && byte != 9 && byte != 10 && byte != 13;
        if is_control || byte == 127 {
            suspicious += 1;
        }
    }
    suspicious as f64 / buffer.len() as f64 > 0.3
}

/// Paseo's `isValidUtf8`: a fatal decoder decides, never a lossy one.
fn is_valid_utf8(buffer: &[u8]) -> bool {
    std::str::from_utf8(buffer).is_ok()
}

/// Resolve `workspace_id` through the registry — never a request field —
/// and open the file whole for the editor.
pub(crate) fn reply_open(
    state: &ServerState,
    id: u64,
    workspace_id: &str,
    path: &str,
) -> DaemonMessage {
    let file = match state.sessions.workspace_cwd(workspace_id) {
        Ok(root) => open_workspace_file(&root, workspace_id, path),
        // The sentence comes from the registry and names no path: safe to
        // echo on this frame, the way the windowed read does.
        Err(error) => return DaemonMessage::Error(error.with_id(id)),
    };
    DaemonMessage::WorkspaceFileOpened { id, file }
}

/// The editor's version poll for one workspace file.
pub(crate) fn reply_version(
    state: &ServerState,
    id: u64,
    workspace_id: &str,
    path: &str,
) -> DaemonMessage {
    let version = match state.sessions.workspace_cwd(workspace_id) {
        Ok(root) => version_workspace_file(&root, workspace_id, path),
        Err(error) => return DaemonMessage::Error(error.with_id(id)),
    };
    DaemonMessage::WorkspaceFileVersion { id, version }
}

/// The editor's write for one workspace file.
pub(crate) fn reply_write(
    state: &ServerState,
    id: u64,
    workspace_id: &str,
    path: &str,
    content: &str,
    expected_modified_at: Option<i64>,
    expected_revision: Option<&str>,
) -> DaemonMessage {
    let result = match state.sessions.workspace_cwd(workspace_id) {
        Ok(root) => write_workspace_file(
            &root,
            workspace_id,
            path,
            content.as_bytes(),
            expected_modified_at,
            expected_revision,
        ),
        Err(error) => return DaemonMessage::Error(error.with_id(id)),
    };
    DaemonMessage::WorkspaceFileWrite { id, result }
}

/// Open one file anywhere on this machine: the human's own file, links
/// followed, the real file opened. App-only one layer up (the peer gate
/// refuses these frames to every peer); this function judges paths, not
/// connections.
pub(crate) fn reply_app_open(state: &ServerState, id: u64, path: &str) -> DaemonMessage {
    let _ = state;
    DaemonMessage::AppFileOpened {
        id,
        file: open_app_file(path),
    }
}

/// The version poll for such a file.
pub(crate) fn reply_app_version(state: &ServerState, id: u64, path: &str) -> DaemonMessage {
    let _ = state;
    DaemonMessage::AppFileVersion {
        id,
        version: version_app_file(path),
    }
}

/// The write for such a file.
pub(crate) fn reply_app_write(
    state: &ServerState,
    id: u64,
    path: &str,
    content: &str,
    expected_modified_at: Option<i64>,
    expected_revision: Option<&str>,
) -> DaemonMessage {
    let _ = state;
    DaemonMessage::AppFileWrite {
        id,
        result: write_app_file(
            path,
            content.as_bytes(),
            expected_modified_at,
            expected_revision,
        ),
    }
}

/// One workspace file, opened whole: confinement first without a process,
/// the metadata-folder guard before the walk, then the walk that trusts
/// the filesystem — every component stat'ed without following, a link
/// refused with the shared sentence.
fn open_workspace_file(root: &Path, workspace_id: &str, requested: &str) -> WorkspaceEditableFile {
    let Some(target) = confined(root, requested) else {
        return refused(OUTSIDE_THE_WORKSPACE);
    };
    if names_git_metadata(requested) {
        return refused(NOT_PART_OF_THE_TREE);
    }
    match walk(root, requested) {
        Walked::Link(sentence) => refused(sentence),
        // A missing file opens empty — the first save creates it — but a
        // missing *parent* is an error: parents are never made silently.
        // And `Missing` is every stat failure, not just absence, so the
        // target is re-asked: a link the walk could not classify stays a
        // link, a file that raced into place opens, and only true absence
        // opens empty. Anything the stat cannot say is a failed check.
        Walked::Missing => match classify_target(&target) {
            TargetState::Present(metadata) if crosses_a_link(&metadata) => refused(LINK_FINAL),
            TargetState::Present(metadata) if metadata.is_file() => {
                open_resolved(&target, workspace_id, requested, &metadata)
            }
            TargetState::Present(_) => refused(NOT_A_FILE),
            TargetState::Absent => match parent_is_dir(&target) {
                Some(true) => missing(workspace_id, requested),
                _ => refused(PARENT_MISSING),
            },
            TargetState::Unstatable => refused(UNSTATABLE),
        },
        Walked::Inside(metadata) => {
            if !metadata.is_file() {
                return refused(NOT_A_FILE);
            }
            open_resolved(&target, workspace_id, requested, &metadata)
        }
    }
}

/// The version of one workspace file: stat only, never a byte read.
fn version_workspace_file(
    root: &Path,
    workspace_id: &str,
    requested: &str,
) -> WorkspaceFileVersion {
    let Some(target) = confined(root, requested) else {
        return WorkspaceFileVersion::Error {
            workspace_id: workspace_id.to_string(),
            path: requested.to_string(),
            error: OUTSIDE_THE_WORKSPACE.to_string(),
        };
    };
    if names_git_metadata(requested) {
        return WorkspaceFileVersion::Error {
            workspace_id: workspace_id.to_string(),
            path: requested.to_string(),
            error: NOT_PART_OF_THE_TREE.to_string(),
        };
    }
    match walk(root, requested) {
        Walked::Link(sentence) => WorkspaceFileVersion::Error {
            workspace_id: workspace_id.to_string(),
            path: requested.to_string(),
            error: sentence.to_string(),
        },
        Walked::Missing => match classify_target(&target) {
            // A link the walk could not classify is still a link, and a
            // file that raced into place polls ready — only true absence
            // polls missing, and an unstatable file reports the failed
            // check, never a missing version that would arm a create.
            TargetState::Present(metadata) if crosses_a_link(&metadata) => {
                WorkspaceFileVersion::Error {
                    workspace_id: workspace_id.to_string(),
                    path: requested.to_string(),
                    error: LINK_FINAL.to_string(),
                }
            }
            TargetState::Present(metadata) if metadata.is_file() => {
                ready_version(workspace_id, requested, &metadata)
            }
            TargetState::Present(_) => WorkspaceFileVersion::Error {
                workspace_id: workspace_id.to_string(),
                path: requested.to_string(),
                error: NOT_A_FILE.to_string(),
            },
            TargetState::Absent => WorkspaceFileVersion::Missing {
                workspace_id: workspace_id.to_string(),
                path: requested.to_string(),
            },
            TargetState::Unstatable => WorkspaceFileVersion::Error {
                workspace_id: workspace_id.to_string(),
                path: requested.to_string(),
                error: UNSTATABLE.to_string(),
            },
        },
        Walked::Inside(metadata) => {
            if !metadata.is_file() {
                return WorkspaceFileVersion::Error {
                    workspace_id: workspace_id.to_string(),
                    path: requested.to_string(),
                    error: NOT_A_FILE.to_string(),
                };
            }
            ready_version(workspace_id, requested, &metadata)
        }
    }
}

/// One workspace file written: Paseo's `writeExplorerFile` with the
/// missing-file arm turned from a conflict into a create.
#[allow(clippy::too_many_arguments)]
fn write_workspace_file(
    root: &Path,
    workspace_id: &str,
    requested: &str,
    content: &[u8],
    expected_modified_at: Option<i64>,
    expected_revision: Option<&str>,
) -> WorkspaceFileWriteResult {
    // Paseo encodes first and measures bytes: `content` is characters on
    // the wire, the cap is bytes on the disk.
    if content.len() as u64 > MAX_EDITABLE_FILE_BYTES {
        return error(TOO_LARGE);
    }
    let Some(target) = confined(root, requested) else {
        return error(OUTSIDE_THE_WORKSPACE);
    };
    if names_git_metadata(requested) {
        return error(NOT_PART_OF_THE_TREE);
    }
    match walk(root, requested) {
        Walked::Link(sentence) => error(sentence),
        // Like the open: re-ask the target instead of trusting `Missing`.
        // A file that raced into place takes the present road (an expected
        // write checks against it, a create conflicts with it); only true
        // absence creates, and an unstatable file errors.
        Walked::Missing => match classify_target(&target) {
            TargetState::Present(metadata) => write_present(
                &target,
                workspace_id,
                requested,
                content,
                expected_modified_at,
                expected_revision,
                &metadata,
            ),
            TargetState::Absent => {
                // A write that names an expected version cannot create: the
                // file it meant to replace is gone, so the answer is the
                // missing version — the editor's conflict road.
                if expected_modified_at.is_some() || expected_revision.is_some() {
                    return WorkspaceFileWriteResult::Conflict {
                        version: WorkspaceFileVersion::Missing {
                            workspace_id: workspace_id.to_string(),
                            path: requested.to_string(),
                        },
                    };
                }
                match parent_is_dir(&target) {
                    Some(true) => create_file(&target, content),
                    _ => error(PARENT_MISSING),
                }
            }
            TargetState::Unstatable => error(UNSTATABLE),
        },
        Walked::Inside(metadata) => write_present(
            &target,
            workspace_id,
            requested,
            content,
            expected_modified_at,
            expected_revision,
            &metadata,
        ),
    }
}

/// Write a file whose metadata is in hand: the binary and cap guards,
/// the expected-version check, and the atomic replace.
#[allow(clippy::too_many_arguments)]
fn write_present(
    target: &Path,
    workspace_id: &str,
    requested: &str,
    content: &[u8],
    expected_modified_at: Option<i64>,
    expected_revision: Option<&str>,
    metadata: &std::fs::Metadata,
) -> WorkspaceFileWriteResult {
    if !metadata.is_file() {
        return error(NOT_A_FILE);
    }
    if metadata.len() > MAX_EDITABLE_FILE_BYTES {
        return error(TOO_LARGE);
    }
    // Paseo refuses to replace bytes it would not open: binary on
    // disk stays binary, never overwritten with text by this road.
    let current = match std::fs::read(target) {
        Ok(bytes) => bytes,
        Err(_) => return error(DOES_NOT_EXIST),
    };
    if is_likely_binary(&current) || !is_valid_utf8(&current) {
        return error(BINARY);
    }
    let modified = metadata.modified().ok();
    let modified_ms = stamped(metadata);
    let matches = match modified {
        Some(time) => matches_expected(
            metadata.len(),
            time,
            modified_ms,
            expected_modified_at,
            expected_revision,
        ),
        None => false,
    };
    if !matches {
        return WorkspaceFileWriteResult::Conflict {
            version: ready_version(workspace_id, requested, metadata),
        };
    }
    #[cfg(unix)]
    let mode = {
        use std::os::unix::fs::PermissionsExt;
        Some(metadata.permissions().mode())
    };
    #[cfg(not(unix))]
    let mode: Option<u32> = None;
    replace_file(target, content, mode, || {
        // The TOCTOU window Paseo closes the same way: a change
        // between the check and the rename is a conflict, never an
        // overwrite.
        match std::fs::metadata(target) {
            Ok(latest) => {
                let latest_ms = stamped(&latest);
                let still_matches = match latest.modified().ok() {
                    Some(time) => matches_expected(
                        latest.len(),
                        time,
                        latest_ms,
                        expected_modified_at,
                        expected_revision,
                    ),
                    None => false,
                };
                if still_matches {
                    Recheck::Proceed
                } else {
                    Recheck::Conflict
                }
            }
            // Gone under us races the delete road: conflict with
            // the missing version. Unstatable is a failed check,
            // not an absence.
            Err(_) => match classify_target(target) {
                TargetState::Absent => Recheck::Conflict,
                _ => Recheck::Refuse(UNSTATABLE),
            },
        }
    })
}

/// The file's bytes, whole, with their BOM flag and version — or the
/// refusal's sentence. Over the cap, binary, or undecodable bytes never
/// open.
fn open_resolved(
    target: &Path,
    workspace_id: &str,
    requested: &str,
    metadata: &std::fs::Metadata,
) -> WorkspaceEditableFile {
    if metadata.len() > MAX_EDITABLE_FILE_BYTES {
        return refused(TOO_LARGE);
    }
    let bytes = match std::fs::read(target) {
        Ok(bytes) => bytes,
        Err(_) => return refused(DOES_NOT_EXIST),
    };
    // The file can grow between the stat and the read: the cap is
    // re-checked on the bytes in hand, the way the image road does.
    if bytes.len() as u64 > MAX_EDITABLE_FILE_BYTES {
        return refused(TOO_LARGE);
    }
    if is_likely_binary(&bytes) || !is_valid_utf8(&bytes) {
        return refused(BINARY);
    }
    // The BOM is the editor's to restore: it travels as the flag, never
    // inside the content — Paseo's `hasBom`, byte for byte.
    let text = String::from_utf8(bytes).expect("checked UTF-8");
    let (has_bom, content) = match text.strip_prefix('\u{FEFF}') {
        Some(stripped) => (true, stripped.to_owned()),
        None => (false, text),
    };
    WorkspaceEditableFile {
        status: WorkspaceFileContentStatus::Ok,
        content: Some(content),
        has_bom: Some(has_bom),
        version: Some(ready_version(workspace_id, requested, metadata)),
        size: Some(metadata.len()),
        error: None,
    }
}

/// The ready version of a file whose metadata is in hand. `modified`
/// without a stamp (a filesystem that gave none) still versions by its
/// revision string; a file with neither stamp nor readable clock keeps a
/// revision of size alone — and a write that names an expected value then
/// compares against exactly that.
fn ready_version(
    workspace_id: &str,
    requested: &str,
    metadata: &std::fs::Metadata,
) -> WorkspaceFileVersion {
    let size = metadata.len();
    let modified = metadata.modified().ok();
    let revision = modified
        .map(|time| file_revision(size, time))
        .unwrap_or_else(|| format!("{size}:none"));
    WorkspaceFileVersion::Ready {
        workspace_id: workspace_id.to_string(),
        path: requested.to_string(),
        size,
        modified_at: stamped(metadata).unwrap_or(0),
        revision: Some(revision),
    }
}

/// Whether the target's parent is a directory: `Some(true)` green-lights a
/// create, anything else refuses it. A missing parent and a parent that is
/// a file are the same refusal — neither can hold the new file.
fn parent_is_dir(target: &Path) -> Option<bool> {
    let parent = target.parent()?;
    match std::fs::metadata(parent) {
        Ok(metadata) => Some(metadata.is_dir()),
        Err(_) => None,
    }
}

/// Create a file that is not there: the same atomic road as a replace
/// (temp file in the same folder, `fsync`, `rename`), claimed exclusively
/// — a file that appears between the walk and the rename answers
/// `conflict`, never overwritten.
fn create_file(target: &Path, content: &[u8]) -> WorkspaceFileWriteResult {
    replace_file(target, content, None, || {
        // The create's own TOCTOU: still missing proceeds, appeared
        // conflicts, unstatable refuses — `exists` is false on every
        // error and would let a create replace a file the daemon
        // cannot see, so the stat is classified, not booleanised.
        match classify_target(target) {
            TargetState::Absent => Recheck::Proceed,
            TargetState::Present(_) => Recheck::Conflict,
            TargetState::Unstatable => Recheck::Refuse(UNSTATABLE),
        }
    })
}

/// What the pre-rename recheck decides: proceed with the rename, turn
/// the write into a conflict with the freshly stat'ed version, or refuse
/// with a sentence when the target cannot even be stat'ed.
enum Recheck {
    Proceed,
    Conflict,
    Refuse(&'static str),
}

/// Atomically replace (or create) `target` with `content`: Paseo's temp
/// file in the same folder, `sync`, `rename`. `recheck` runs after the
/// temp file is synced and before the rename; on `Conflict` the write
/// becomes [`conflict_from_target`]. The temp file is removed on every
/// exit.
fn replace_file(
    target: &Path,
    content: &[u8],
    mode: Option<u32>,
    recheck: impl FnOnce() -> Recheck,
) -> WorkspaceFileWriteResult {
    let dir = target.parent();
    let Some(dir) = dir else {
        return error(PARENT_MISSING);
    };
    let name = target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".to_string());
    let temporary = dir.join(format!(
        ".{name}.devboule-{}.tmp",
        uuid::Uuid::new_v4().as_simple()
    ));
    let write_temp = (|| -> std::io::Result<()> {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            // A replaced file keeps its mode; a created one takes the
            // process default (no mode set, so the umask governs).
            if let Some(mode) = mode {
                options.mode(mode & 0o7777);
            }
        }
        let _ = mode;
        let mut handle = options.open(&temporary)?;
        use std::io::Write;
        handle.write_all(content)?;
        handle.sync_all()?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Some(mode) = mode {
                handle.set_permissions(std::fs::Permissions::from_mode(mode & 0o7777))?;
            }
        }
        Ok(())
    })();
    if let Err(io_error) = write_temp {
        let _ = std::fs::remove_file(&temporary);
        return error(io_sentence(&io_error));
    }
    let proceed = recheck();
    match proceed {
        Recheck::Conflict => {
            let _ = std::fs::remove_file(&temporary);
            return conflict_from_target(target);
        }
        Recheck::Refuse(sentence) => {
            let _ = std::fs::remove_file(&temporary);
            return error(sentence);
        }
        Recheck::Proceed => {}
    }
    if std::fs::rename(&temporary, target).is_err() {
        let _ = std::fs::remove_file(&temporary);
        return error("the file could not be written");
    }
    let _ = std::fs::remove_file(&temporary);
    match std::fs::metadata(target) {
        Ok(stats) => {
            let size = stats.len();
            let modified_at = stamped(&stats).unwrap_or(0);
            let revision = stats
                .modified()
                .ok()
                .map(|time| file_revision(size, time))
                .unwrap_or_else(|| format!("{size}:none"));
            WorkspaceFileWriteResult::Written {
                modified_at,
                size,
                revision,
            }
        }
        Err(_) => error(DOES_NOT_EXIST),
    }
}

/// An empty editor for a file that is not there: empty content and a
/// `missing` version — the first save creates it.
fn missing(workspace_id: &str, requested: &str) -> WorkspaceEditableFile {
    WorkspaceEditableFile {
        status: WorkspaceFileContentStatus::Ok,
        content: Some(String::new()),
        has_bom: Some(false),
        version: Some(WorkspaceFileVersion::Missing {
            workspace_id: workspace_id.to_string(),
            path: requested.to_string(),
        }),
        size: Some(0),
        error: None,
    }
}

/// A refusal to open: the sentence says what stopped it, and — per the
/// carve-out the type documents — nothing else travels with it.
fn refused(sentence: &str) -> WorkspaceEditableFile {
    WorkspaceEditableFile {
        status: WorkspaceFileContentStatus::Refused,
        content: None,
        has_bom: None,
        version: None,
        size: None,
        error: Some(sentence.to_string()),
    }
}

/// A refused write: Paseo's `{ status: "error", error }`.
fn error(sentence: &str) -> WorkspaceFileWriteResult {
    WorkspaceFileWriteResult::Error {
        error: sentence.to_string(),
    }
}

/// The conflict when the recheck finds the target changed (or, for a
/// create, appeared): the fresh version, stat'ed at refusal time.
fn conflict_from_target(target: &Path) -> WorkspaceFileWriteResult {
    let version = match std::fs::metadata(target) {
        Ok(latest) => ready_version("", &target_name(target), &latest),
        Err(_) => WorkspaceFileVersion::Missing {
            workspace_id: String::new(),
            path: target_name(target),
        },
    };
    WorkspaceFileWriteResult::Conflict { version }
}

/// The target's own name for a sentence-free version: no path travels in
/// a refusal, and these versions name only the entry.
fn target_name(target: &Path) -> String {
    target
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// An I/O sentence that names no path: paths carry spellings the wire
/// must not echo, so the OS string stays out of every reply.
fn io_sentence(_error: &std::io::Error) -> &'static str {
    "the file could not be written"
}

// ---------------------------------------------------------------------------
// The human's own files: absolute or `~` paths on this machine.
// ---------------------------------------------------------------------------

/// Paseo's `expandUserPath`: `~` and `~/…` grow the home folder, anything
/// else resolves as spelled. Trimmed first, the way the shell reads it.
fn expand_user_path(value: &str) -> PathBuf {
    let trimmed = value.trim();
    if trimmed == "~" || trimmed.starts_with("~/") || trimmed.starts_with("~\\") {
        if let Some(home) = home_dir() {
            if trimmed.len() == 1 {
                return home;
            }
            return home.join(&trimmed[2..]);
        }
    }
    PathBuf::from(trimmed)
}

/// The sentence a dangling link answers with on the human's road. The
/// link names a file that is not there, and creating in its place would
/// silently swap the link for a regular file.
const LINK_TARGET_MISSING: &str = "the link target is missing";

/// The real file behind an app-road `target`: links are followed to
/// their target and every later step stages beside *that* file, so a
/// write replaces the target's bytes and never the link — and a link
/// whose directory lives on another volume keeps the content's home.
/// A dangling link fails here instead of becoming a create; a loop (or
/// any other follow failure) is a failed check.
fn canonical_app_target(target: &Path) -> Result<PathBuf, &'static str> {
    match std::fs::canonicalize(target) {
        Ok(real) => Ok(real),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Err(LINK_TARGET_MISSING),
        Err(_) => Err(UNSTATABLE),
    }
}
/// The home folder: `HOME`, else `USERPROFILE` — what Paseo's
/// `os.homedir()` reads on each platform, without the Windows fallbacks
/// Node carries (those need the registry; an unset home here is a refusal,
/// never a guess).
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Resolve a human path: expand `~`, require absolute, open the real file
/// — links are followed, never refused. A relative spelling is the
/// caller's mistake and is refused with these words rather than joined to
/// a folder the caller never named.
fn resolve_app_target(requested: &str) -> Result<PathBuf, &'static str> {
    let expanded = expand_user_path(requested);
    if !expanded.is_absolute() {
        return Err(NOT_ABSOLUTE);
    }
    Ok(expanded)
}

fn open_app_file(requested: &str) -> WorkspaceEditableFile {
    let target = match resolve_app_target(requested) {
        Ok(target) => target,
        Err(sentence) => return refused(sentence),
    };
    // Classified, not booleanised: absence opens empty, anything else
    // the stat cannot say is a failed check.
    match classify_target(&target) {
        TargetState::Absent => match parent_is_dir(&target) {
            Some(true) => missing("", &target.to_string_lossy()),
            _ => refused(PARENT_MISSING),
        },
        TargetState::Unstatable => refused(UNSTATABLE),
        // Followed to the real file: the read, the stat and the version
        // all describe the target, while the reply keeps the spelling
        // the human opened.
        TargetState::Present(_) => {
            let spelling = target.to_string_lossy().into_owned();
            let real = match canonical_app_target(&target) {
                Ok(real) => real,
                Err(sentence) => return refused(sentence),
            };
            match std::fs::metadata(&real) {
                Err(_) => refused(UNSTATABLE),
                Ok(metadata) => {
                    if !metadata.is_file() {
                        return refused(NOT_A_FILE);
                    }
                    open_resolved(&real, "", &spelling, &metadata)
                }
            }
        }
    }
}

fn version_app_file(requested: &str) -> WorkspaceFileVersion {
    let target = match resolve_app_target(requested) {
        Ok(target) => target,
        Err(sentence) => {
            return WorkspaceFileVersion::Error {
                workspace_id: String::new(),
                path: requested.to_string(),
                error: sentence.to_string(),
            };
        }
    };
    let spelling = target.to_string_lossy().into_owned();
    match classify_target(&target) {
        TargetState::Absent => WorkspaceFileVersion::Missing {
            workspace_id: String::new(),
            path: spelling,
        },
        TargetState::Unstatable => WorkspaceFileVersion::Error {
            workspace_id: String::new(),
            path: spelling,
            error: UNSTATABLE.to_string(),
        },
        TargetState::Present(_) => {
            let real = match canonical_app_target(&target) {
                Ok(real) => real,
                Err(sentence) => {
                    return WorkspaceFileVersion::Error {
                        workspace_id: String::new(),
                        path: target.to_string_lossy().into_owned(),
                        error: sentence.to_string(),
                    };
                }
            };
            match std::fs::metadata(&real) {
                Err(_) => WorkspaceFileVersion::Error {
                    workspace_id: String::new(),
                    path: target.to_string_lossy().into_owned(),
                    error: UNSTATABLE.to_string(),
                },
                Ok(metadata) => {
                    if !metadata.is_file() {
                        return WorkspaceFileVersion::Error {
                            workspace_id: String::new(),
                            path: spelling,
                            error: NOT_A_FILE.to_string(),
                        };
                    }
                    ready_version("", &spelling, &metadata)
                }
            }
        }
    }
}

fn write_app_file(
    requested: &str,
    content: &[u8],
    expected_modified_at: Option<i64>,
    expected_revision: Option<&str>,
) -> WorkspaceFileWriteResult {
    if content.len() as u64 > MAX_EDITABLE_FILE_BYTES {
        return error(TOO_LARGE);
    }
    let target = match resolve_app_target(requested) {
        Ok(target) => target,
        Err(sentence) => return error(sentence),
    };
    let spelling = target.to_string_lossy().into_owned();
    match classify_target(&target) {
        TargetState::Absent => {
            if expected_modified_at.is_some() || expected_revision.is_some() {
                return WorkspaceFileWriteResult::Conflict {
                    version: WorkspaceFileVersion::Missing {
                        workspace_id: String::new(),
                        path: spelling,
                    },
                };
            }
            match parent_is_dir(&target) {
                Some(true) => create_file(&target, content),
                _ => error(PARENT_MISSING),
            }
        }
        TargetState::Unstatable => error(UNSTATABLE),
        // Followed to the real file before anything stages: the temp
        // sits beside the target and the rename replaces its bytes, so
        // a link is never swapped for a regular file. `spelling` above
        // stays the requested one; only the IO moves.
        TargetState::Present(_) => {
            let target = match canonical_app_target(&target) {
                Ok(real) => real,
                Err(sentence) => return error(sentence),
            };
            match std::fs::metadata(&target) {
                Err(_) => error(UNSTATABLE),
                Ok(metadata) => {
                    if !metadata.is_file() {
                        return error(NOT_A_FILE);
                    }
                    if metadata.len() > MAX_EDITABLE_FILE_BYTES {
                        return error(TOO_LARGE);
                    }
                    let current = match std::fs::read(&target) {
                        Ok(bytes) => bytes,
                        Err(_) => return error(DOES_NOT_EXIST),
                    };
                    if is_likely_binary(&current) || !is_valid_utf8(&current) {
                        return error(BINARY);
                    }
                    let modified = metadata.modified().ok();
                    let modified_ms = stamped(&metadata);
                    let matches = match modified {
                        Some(time) => matches_expected(
                            metadata.len(),
                            time,
                            modified_ms,
                            expected_modified_at,
                            expected_revision,
                        ),
                        None => false,
                    };
                    if !matches {
                        return WorkspaceFileWriteResult::Conflict {
                            version: ready_version("", &spelling, &metadata),
                        };
                    }
                    #[cfg(unix)]
                    let mode = {
                        use std::os::unix::fs::PermissionsExt;
                        Some(metadata.permissions().mode())
                    };
                    #[cfg(not(unix))]
                    let mode: Option<u32> = None;
                    replace_file(&target, content, mode, || {
                        match std::fs::metadata(&target) {
                            Ok(latest) => {
                                let latest_ms = stamped(&latest);
                                let still_matches = match latest.modified().ok() {
                                    Some(time) => matches_expected(
                                        latest.len(),
                                        time,
                                        latest_ms,
                                        expected_modified_at,
                                        expected_revision,
                                    ),
                                    None => false,
                                };
                                if still_matches {
                                    Recheck::Proceed
                                } else {
                                    Recheck::Conflict
                                }
                            }
                            Err(_) => match classify_target(&target) {
                                TargetState::Absent => Recheck::Conflict,
                                _ => Recheck::Refuse(UNSTATABLE),
                            },
                        }
                    })
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "workspace_file_edit_tests.rs"]
mod tests;
