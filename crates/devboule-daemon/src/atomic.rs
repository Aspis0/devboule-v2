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
use std::fs::OpenOptions;
use std::io;
use std::io::Write;
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

/// Write `bytes` to `path` through a temp file that is created narrow and
/// stays narrow: `create_new` (after removing a leftover temp, so a run that
/// died between create and rename never wedges the name), `0o600` on unix,
/// the current-user DACL on Windows **before the first byte**, `write_all`,
/// `sync_all`, rename, temp removed on failure.
///
/// The ONE protected writer (P2): the MCP config, the pi bridge, the Codex
/// home and the tool policy all go through here, so the security order lives
/// in exactly one function and cannot drift. (Plain `atomic_write` above is
/// deliberately weaker — no mode, no DACL — for non-secret whole-file
/// replaces; secrets must never take that road.)
pub(crate) fn write_protected_bytes(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "protected write target has no parent directory",
        )
    })?;
    fs::create_dir_all(parent)?;
    let temp = path.with_extension("tmp");
    let result = (|| {
        // The temp name is this writer's own: remove a leftover first so a run
        // that died between create and rename never wedges the name (and
        // `create_new` refuses a file already at it). Removing it removes the
        // name, not whatever a symlink at it points at.
        let _ = fs::remove_file(&temp);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&temp)?;
        // Before the first byte: the secret is never on disk under a weaker
        // DACL. The helper lives in `security.rs`; off Windows there is none.
        #[cfg(windows)]
        crate::security::apply_current_user_dacl(&temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_dir() -> std::path::PathBuf {
        crate::test_dirs::test_temp_dir("devboule-atomic")
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

    /// P2: the DACL-before-bytes call lives in exactly one function. A second
    /// copy of the security order — the defect this unification removes — trips
    /// this test: reintroduce a DACL call in the broker or the policy writer
    /// and the count goes red. (The needle is concatenated so this very test
    /// does not match itself.)
    #[test]
    fn the_protected_write_order_lives_in_exactly_one_place() {
        let needle = ["apply_current_user_dacl", "(&"].concat();
        let mut sources = vec![
            include_str!("atomic.rs").to_string(),
            include_str!("tool_policy.rs").to_string(),
        ];
        sources.extend(crate::test_support::mcp_broker_sources());
        let calls: usize = sources
            .iter()
            .map(|source| source.matches(needle.as_str()).count())
            .sum();
        assert_eq!(
            calls, 1,
            "one protected writer: the DACL call must appear exactly once across atomic/broker/policy"
        );
    }
}
