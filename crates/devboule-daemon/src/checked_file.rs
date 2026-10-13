//! The open a checked workspace file gets. A walk stats each component of a
//! path without following it, and then something opens the name, so the
//! name can be swapped between the two. Opening the final component as
//! itself and then proving, on the handle, where it lives (Windows) or what
//! it is (elsewhere) closes that window; the bytes are read from that handle
//! and nowhere else, under a cap.

use std::fs::{File, Metadata, OpenOptions};
use std::io::Read;
use std::path::Path;

use crate::workspace_git_diff::NOT_A_FILE;
use crate::workspace_git_support::{LINK_FINAL, OUTSIDE_THE_WORKSPACE};

/// Why a checked path does not bind to a file that can be read.
#[derive(Debug)]
pub(crate) enum Refusal {
    /// The open failed. The caller words it: only it knows what it was for.
    Unopened,
    /// The final component is a link, opened as the link itself.
    Link,
    /// The handle is not a regular file.
    NotRegular,
    /// Windows: the handle really lives outside the root.
    #[cfg(windows)]
    Outside,
    /// The handle's location cannot be named, so it cannot be affirmed.
    Unconfirmed,
    /// Elsewhere: the handle is not the object the walk stat'ed.
    #[cfg(not(windows))]
    Replaced,
}

pub(crate) const LOCATION_UNCONFIRMED: &str =
    "the file's location could not be confirmed inside the workspace";

const UNOPENED: &str = "the file could not be opened";

#[cfg(not(windows))]
const REPLACED: &str = "the file changed after it was checked; nothing was read";

impl Refusal {
    /// The static sentence a caller that has no words of its own shows.
    pub(crate) fn sentence(&self) -> &'static str {
        match self {
            Self::Unopened => UNOPENED,
            Self::Link => LINK_FINAL,
            Self::NotRegular => NOT_A_FILE,
            #[cfg(windows)]
            Self::Outside => OUTSIDE_THE_WORKSPACE,
            Self::Unconfirmed => LOCATION_UNCONFIRMED,
            #[cfg(not(windows))]
            Self::Replaced => REPLACED,
        }
    }
}

/// Open `target`, a path the walk stat'ed as `walked` under `root`, and hand
/// back the handle with its own metadata once it is proven to be that file:
/// not a link, a regular file, and still inside `root`.
pub(crate) fn open_checked(
    root: &Path,
    target: &Path,
    walked: &Metadata,
) -> Result<(File, Metadata), Refusal> {
    // The swap a race test plants must land between the walk's verdict and
    // this open.
    #[cfg(test)]
    between::run();
    let file = open_no_follow(target).map_err(|_| Refusal::Unopened)?;
    let metadata = file.metadata().map_err(|_| Refusal::Unopened)?;
    if is_link(&metadata) {
        return Err(Refusal::Link);
    }
    if !metadata.is_file() {
        return Err(Refusal::NotRegular);
    }
    confirm_binding(&file, root, walked)?;
    Ok((file, metadata))
}

/// Read at most `cap` bytes from `reader`, or `None` when there are more.
/// Reading `cap + 1` bounds the allocation whatever the file does after its
/// check, and the extra byte is what tells "exactly at the cap" from "over".
pub(crate) fn read_capped(reader: &mut impl Read, cap: u64) -> std::io::Result<Option<Vec<u8>>> {
    let mut bytes = Vec::new();
    reader.take(cap + 1).read_to_end(&mut bytes)?;
    Ok((bytes.len() as u64 <= cap).then_some(bytes))
}

/// The house predicate for "this stat is a link", spelled without
/// `is_symlink()`: whether a junction answers to that label is a measurement
/// this house has seen flip (`workspace_git_support`), while the attribute
/// never has.
#[cfg(windows)]
pub(crate) fn is_link(metadata: &Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    metadata.file_attributes() & 0x400 != 0 // FILE_ATTRIBUTE_REPARSE_POINT
}

#[cfg(not(windows))]
pub(crate) fn is_link(metadata: &Metadata) -> bool {
    metadata.file_type().is_symlink()
}

/// Windows: `FILE_FLAG_OPEN_REPARSE_POINT` opens the final component as
/// itself, so a link planted over the checked name yields a handle to the
/// link (refused on its attribute) and its target is never touched. Hardcoded
/// the way this house hardcodes `0x400` in `workspace_git_support`.
#[cfg(windows)]
const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;

#[cfg(windows)]
fn open_no_follow(target: &Path) -> std::io::Result<File> {
    use std::os::windows::fs::OpenOptionsExt;
    OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(target)
}

/// Elsewhere: `O_NOFOLLOW` refuses a link at the final component. `O_NONBLOCK`
/// keeps a FIFO swapped in after the check from blocking the open; the type
/// check then refuses it.
#[cfg(not(windows))]
fn open_no_follow(target: &Path) -> std::io::Result<File> {
    use std::os::unix::fs::OpenOptionsExt;
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(target)
}

/// Windows: ask the handle where it really lives. `GetFinalPathNameByHandle`
/// reports what the open traversed, so a directory component swapped for a
/// junction shows as the outside path it leads to. The walk's stat has no
/// identity stable Rust can read, so the handle's own path is the proof, and
/// a handle whose location cannot be named is a refusal, never a read.
#[cfg(windows)]
fn confirm_binding(file: &File, root: &Path, _walked: &Metadata) -> Result<(), Refusal> {
    use std::ffi::OsString;
    use std::os::windows::ffi::OsStringExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{GetFinalPathNameByHandleW, VOLUME_NAME_DOS};

    let root = std::fs::canonicalize(root).map_err(|_| Refusal::Unconfirmed)?;
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
            return Err(Refusal::Unconfirmed);
        }
        // Success excludes the terminating null (it fits); too small
        // returns the size *including* it — exactly the length to retry
        // with, and `+ 1` where they are equal so the loop cannot stall.
        if (written as usize) < buffer.len() {
            break written as usize;
        }
        if written > 32_768 {
            return Err(Refusal::Unconfirmed);
        }
        buffer.resize((written as usize).max(buffer.len() + 1), 0);
    };
    let resolved = std::path::PathBuf::from(OsString::from_wide(&buffer[..length]));
    if resolved.starts_with(&root) {
        Ok(())
    } else {
        Err(Refusal::Outside)
    }
}

/// Elsewhere: the handle's own identity (`dev`, `ino`) against the identity
/// the walk stat'ed. No swap of any component keeps them equal.
#[cfg(not(windows))]
fn confirm_binding(file: &File, _root: &Path, walked: &Metadata) -> Result<(), Refusal> {
    use std::os::unix::fs::MetadataExt;
    let opened = file.metadata().map_err(|_| Refusal::Unconfirmed)?;
    if (opened.dev(), opened.ino()) == (walked.dev(), walked.ino()) {
        Ok(())
    } else {
        Err(Refusal::Replaced)
    }
}

/// The seam the race tests drive: it runs between the walk's verdict and the
/// open. A test installs its swap here; the open has to refuse what it finds.
/// Per thread, and it fires once, so a test cannot leak its swapper.
#[cfg(test)]
pub(crate) mod between {
    use std::cell::RefCell;

    thread_local! {
        static AFTER_WALK: RefCell<Option<Box<dyn FnOnce()>>> = RefCell::new(None);
    }

    pub(crate) fn install(after_walk: Box<dyn FnOnce()>) {
        AFTER_WALK.with(|slot| *slot.borrow_mut() = Some(after_walk));
    }

    pub(crate) fn run() {
        if let Some(swapper) = AFTER_WALK.with(|slot| slot.borrow_mut().take()) {
            swapper();
        }
    }
}

/// A folder link at `link` to `target`: a junction on Windows, which needs no
/// privilege, and a symlink elsewhere. `false` when the platform refused.
#[cfg(test)]
pub(crate) fn link_dir(link: &Path, target: &Path) -> bool {
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

#[cfg(test)]
#[path = "checked_file_tests.rs"]
mod tests;
