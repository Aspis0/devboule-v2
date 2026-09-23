//! The delete act's own file: the roads that remove what the walk has
//! vouched. Split from [`crate::workspace_file_mutations`] by
//! responsibility — the platform anchoring is the delete's alone, the way
//! each test file sits beside its subject.
//!
//! **What is anchored where.** The walk's verdict is about the past:
//! between it and the act, any component of the path can be swapped. Where
//! the platform allows, the act does not trust the name again. Windows:
//! a file is opened once — the final component as itself, `DELETE` access
//! — its handle is asked where it really lives, and the disposition is
//! marked **on the handle**, so what dies is the object the open named,
//! whatever the path spells afterwards. A folder is anchored by a **verdict
//! on the handle** (a recursive removal cannot run on one), with the
//! window that remains declared on [`delete_folder`]. Off Windows the
//! walk's verdict is the only guard — declared with those roads, and this
//! daemon starts on Windows (`crate::paths` asks for `LOCALAPPDATA`).
//!
//! **The link rule, stated precisely.** A link named as the act's target
//! is refused — the walk's sentence, the one rule this tree keeps where
//! Paseo's delete unlinks the link instead. A link **inside** a folder
//! being removed goes with the folder: unlinked as an entry, never
//! followed (`remove_dir_all` traverses no reparse point), so the rule
//! holds one level deeper than the walk can see.

use std::path::Path;

use super::vouched_entry;
use crate::workspace_git_support::OUTSIDE_THE_WORKSPACE;

/// Bare constant on arms no test can force open (declared, like its two
/// siblings `RENAME_FAILED`/`COPY_FAILED` in the parent): there is no
/// deterministic way to make a removal of a path this suite owns fail — a
/// vanished entry dies at the walk instead. Static and pathless, like
/// every sentence here.
const DELETE_FAILED: &str = "the entry could not be deleted";

/// The open could not be vouched: the handle's own location came back
/// unusable, so "inside the workspace" cannot be affirmed — and an act
/// that loses data never runs on an unaffirmed location. Static and
/// pathless, like every sentence here.
const LOCATION_UNCONFIRMED: &str =
    "the entry's location could not be confirmed inside the workspace";

/// Windows only, the numbers the delete's opens need, hardcoded the way
/// the house hardcodes `0x400` in `workspace_git_support` and the same
/// flag in `workspace_file_preview`: `DELETE` access (nothing here reads
/// or writes what it removes), the full share mode (another process's
/// handle is not a reason to fail before the verdict),
/// `FILE_FLAG_OPEN_REPARSE_POINT` (the final component is opened as
/// itself, so a link planted over the validated name is disposed of as
/// the link it is and its target is never touched), and
/// `FILE_FLAG_BACKUP_SEMANTICS` (what lets a folder be opened at all).
#[cfg(windows)]
const FILE_DELETE_ACCESS: u32 = 0x0001_0000;
#[cfg(windows)]
const FILE_SHARE_ALL: u32 = 0x0000_0007;
#[cfg(windows)]
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
#[cfg(windows)]
const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

/// Delete one entry, the act that loses data and never gets a second
/// chance. The vouching is the listing's own door — root refused, `.git`
/// refused in any spelling, a link as the act's target refused with the
/// walk's sentence — and the act itself is [`delete_vouched`], split from
/// this vouching so a race test can hold the swapper still between the
/// two, the way [`super::claim_then_move`] is the rename's window made
/// callable.
pub(super) fn deleted(root: &Path, requested: &str) -> Result<(), String> {
    let (target, metadata) = vouched_entry(root, requested)?;
    delete_vouched(root, target, metadata)
}

/// The act itself, on an already-vouched entry: the branch reads the stat
/// the walk vouched (a folder goes whole, one `remove_dir_all`; anything
/// else goes by [`delete_file`]), and the road is per platform.
fn delete_vouched(
    root: &Path,
    target: std::path::PathBuf,
    metadata: std::fs::Metadata,
) -> Result<(), String> {
    if metadata.is_dir() {
        delete_folder(root, &target)
    } else {
        delete_file(root, &target)
    }
}

/// The file road, Windows: the act is anchored on the handle, the same
/// shape the preview's copy takes
/// (`workspace_file_preview::verified_source`). Open the final component
/// as itself with `DELETE` access, ask the handle where it really lives
/// ([`final_path_inside`]), and only then mark the disposition **on the
/// handle** — between that verdict and the close there is no name left to
/// re-resolve, so no swap of any component after the open can redirect
/// what dies.
#[cfg(windows)]
fn delete_file(root: &Path, target: &Path) -> Result<(), String> {
    use std::os::windows::fs::OpenOptionsExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FileDispositionInfo, SetFileInformationByHandle, FILE_DISPOSITION_INFO,
    };

    let file = std::fs::OpenOptions::new()
        .access_mode(FILE_DELETE_ACCESS)
        .share_mode(FILE_SHARE_ALL)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(target)
        .map_err(|_| DELETE_FAILED.to_string())?;
    final_path_inside(root, &file)?;
    // A read-only entry refuses the disposition exactly as it refuses
    // `DeleteFileW` — the same refusal the name road gave, now worded by
    // `DELETE_FAILED`.
    let info = FILE_DISPOSITION_INFO { DeleteFile: true };
    let marked = unsafe {
        SetFileInformationByHandle(
            file.as_raw_handle(),
            FileDispositionInfo,
            std::ptr::from_ref(&info).cast(),
            std::mem::size_of::<FILE_DISPOSITION_INFO>() as u32,
        )
    };
    if marked == 0 {
        return Err(DELETE_FAILED.to_string());
    }
    // The disposition completes at the close: the handle is the act's last
    // word, not the path.
    drop(file);
    Ok(())
}

/// The folder road, Windows. A recursive removal cannot run on a handle —
/// the children are enumerated by name, and std offers no handle-relative
/// traversal — so the anchor here is a **verdict on the handle**: open the
/// folder *following* whatever the name now spells (a component swapped
/// for a junction lands this open outside), ask where the handle really
/// lives, and refuse anywhere but inside the workspace. Only then is the
/// tree removed under the walked spelling.
///
/// **The window that remains, stated precisely:** between that verdict and
/// `remove_dir_all`'s own open of the tree, an ancestor component can
/// still be swapped, and the removal would follow the swap — the handle's
/// verdict vouches the object, not every future resolution of the name.
/// Closing it entirely would need a handle-relative recursive delete this
/// platform's std does not offer; the file road above needs no such
/// window, and this one is bounded by the gap between two syscalls.
/// Declared, not hidden.
#[cfg(windows)]
fn delete_folder(root: &Path, target: &Path) -> Result<(), String> {
    use std::os::windows::fs::OpenOptionsExt;

    // Desired access 0: this handle exists to be located, not to touch
    // anything; the backup-semantics flag is what lets a folder open.
    let folder = std::fs::OpenOptions::new()
        .access_mode(0)
        .share_mode(FILE_SHARE_ALL)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(target)
        .map_err(|_| DELETE_FAILED.to_string())?;
    final_path_inside(root, &folder)?;
    // The handle's job is done: the removal below re-opens the tree by
    // name, and this code holding a handle across that would only add one
    // more thing the act has to work against.
    drop(folder);
    std::fs::remove_dir_all(target).map_err(|_| DELETE_FAILED.to_string())
}

/// Where the handle *really* lives — the same question the preview asks
/// its own handle (`workspace_file_preview::confirm_binding`, mirrored
/// here rather than shared: the sentences differ, and the extraction pays
/// when a third caller appears). A component swapped for a junction
/// reports the outside path the open resolved through; a location that
/// cannot even be named is a refusal, never a deletion.
#[cfg(windows)]
fn final_path_inside(root: &Path, file: &std::fs::File) -> Result<(), String> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{GetFinalPathNameByHandleW, VOLUME_NAME_DOS};

    let root = std::fs::canonicalize(root).map_err(|_| LOCATION_UNCONFIRMED.to_string())?;
    let mut buffer = vec![0u16; 1024];
    let length = loop {
        let written = unsafe {
            GetFinalPathNameByHandleW(
                file.as_raw_handle(),
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                VOLUME_NAME_DOS,
            )
        };
        if written == 0 {
            return Err(LOCATION_UNCONFIRMED.to_string());
        }
        // Success excludes the terminating null (it fits); too small
        // returns the size *including* it — exactly the length to retry
        // with, and `+ 1` where they are equal so the loop cannot stall.
        if (written as usize) < buffer.len() {
            break written as usize;
        }
        if written > 32_768 {
            return Err(LOCATION_UNCONFIRMED.to_string());
        }
        buffer.resize((written as usize).max(buffer.len() + 1), 0);
    };
    let resolved = std::path::PathBuf::from(OsString::from_wide(&buffer[..length]));
    if resolved.starts_with(&root) {
        Ok(())
    } else {
        Err(OUTSIDE_THE_WORKSPACE.to_string())
    }
}

/// The file road off Windows: the name is removed directly. The handle
/// anchor is a Windows mechanism, and off Windows the walk's verdict is
/// the only guard between it and the act — declared on this module's
/// header, in the same breath as the preview's non-Windows binding.
#[cfg(not(windows))]
fn delete_file(_root: &Path, target: &Path) -> Result<(), String> {
    std::fs::remove_file(target).map_err(|_| DELETE_FAILED.to_string())
}

/// The folder road off Windows: the same declared limit, for the
/// recursive act.
#[cfg(not(windows))]
fn delete_folder(_root: &Path, target: &Path) -> Result<(), String> {
    std::fs::remove_dir_all(target).map_err(|_| DELETE_FAILED.to_string())
}

#[cfg(test)]
#[path = "workspace_file_delete_tests.rs"]
mod tests;
