//! Atomic replace of a file: write a temp, rename over the target, restore
//! from backup on failure.
//!
//! Adapted from v1 `fs_replace.rs` (temp + rename + backup + restore). The
//! AppContainer copy-fallback is deliberately absent: the daemon is not
//! sandboxed, and a non-atomic overwrite would defeat crash safety. A failed
//! rename is a failed write.
//!
//! SQLite WAL is the journal's crash path. This helper is for the rare
//! whole-file replace (tests, and a future compact-into-new-file).

use std::fs;
use std::io;
use std::path::Path;

/// Write `bytes` to `target` by staging a sibling temp file and renaming
/// over it. If `target` already exists it is copied to `target.bak` first
/// and restored if the rename fails.
pub fn atomic_write(target: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = target.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "atomic write target has no parent directory",
        )
    })?;
    fs::create_dir_all(parent)?;
    let temp = target.with_extension("tmp");
    let backup = target.with_extension("bak");
    fs::write(&temp, bytes)?;
    replace_with_backup(&temp, target, &backup)
}

fn replace_with_backup(temp: &Path, target: &Path, backup: &Path) -> io::Result<()> {
    let had_backup = target.exists();
    if had_backup {
        fs::copy(target, backup)?;
    }
    match replace_existing(temp, target) {
        Ok(()) => {
            let _ = fs::remove_file(backup);
            Ok(())
        }
        Err(error) => {
            if had_backup {
                if let Err(restore_err) = fs::copy(backup, target) {
                    return Err(io::Error::other(format!(
                        "replace failed ({error}); backup restoration also failed ({restore_err}); keeping backup at {}",
                        backup.display()
                    )));
                }
                let _ = fs::remove_file(backup);
            } else if target.exists() {
                let _ = fs::remove_file(target);
            }
            let _ = fs::remove_file(temp);
            Err(error)
        }
    }
}

#[cfg(windows)]
fn replace_existing(temp: &Path, target: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
    };

    let mut source: Vec<u16> = temp.as_os_str().encode_wide().collect();
    source.push(0);
    let mut dest: Vec<u16> = target.as_os_str().encode_wide().collect();
    dest.push(0);
    let ok = unsafe {
        MoveFileExW(
            source.as_ptr(),
            dest.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(windows))]
fn replace_existing(temp: &Path, target: &Path) -> io::Result<()> {
    fs::rename(temp, target)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    fn tmp_dir() -> std::path::PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        let dir = std::env::temp_dir().join(format!(
            "devboule-atomic-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[test]
    fn first_write_creates_the_file() {
        let dir = tmp_dir();
        let target = dir.join("note.txt");
        atomic_write(&target, b"hello").expect("write");
        assert_eq!(fs::read(&target).expect("read"), b"hello");
        assert!(!dir.join("note.bak").exists());
        assert!(!dir.join("note.tmp").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn replace_overwrites_and_does_not_leave_backup() {
        let dir = tmp_dir();
        let target = dir.join("note.txt");
        atomic_write(&target, b"old").expect("first");
        atomic_write(&target, b"new").expect("second");
        assert_eq!(fs::read(&target).expect("read"), b"new");
        assert!(!target.with_extension("bak").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn failed_replace_restores_the_backup() {
        let dir = tmp_dir();
        let target = dir.join("note.txt");
        fs::write(&target, b"keep-me").expect("seed");
        // Rename onto an existing directory fails on Windows; the backup
        // must be restored and the original content survive.
        let temp = target.with_extension("tmp");
        fs::write(&temp, b"replacement").expect("temp");
        let blocking_dir = dir.join("note.txt.block");
        fs::create_dir(&blocking_dir).expect("blocker");
        let result = replace_with_backup(&temp, &blocking_dir, &dir.join("note.bak"));
        assert!(result.is_err());
        assert_eq!(fs::read(&target).expect("original"), b"keep-me");
        let _ = fs::remove_dir_all(&dir);
    }
}
