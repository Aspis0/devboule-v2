//! Single-instance lock. Same primitive as v1's `fs2` exclusive lock: the OS
//! releases it when the process dies, so a leftover file is not a deadlock.
//!
//! The locked range is one byte *past* the record the file carries, not the
//! whole file. A whole-file lock would make the daemon's own identity
//! unreadable by every other process for as long as the daemon lived —
//! measured here, a second process' `ReadFile` fails with
//! `ERROR_LOCK_VIOLATION` (33) — and being able to read it without connecting
//! is the only reason to write it down.

use std::fs::{File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::Path;

use crate::daemon_record::RECORD_CAPACITY;
use crate::error::DaemonError;
use crate::paths::RuntimePaths;

#[cfg(windows)]
use std::os::windows::io::AsRawHandle;
#[cfg(windows)]
use windows_sys::Win32::Foundation::HANDLE;
#[cfg(windows)]
use windows_sys::Win32::Storage::FileSystem::{
    LockFileEx, LOCKFILE_EXCLUSIVE_LOCK, LOCKFILE_FAIL_IMMEDIATELY,
};
#[cfg(windows)]
use windows_sys::Win32::System::IO::OVERLAPPED;

pub struct SingleInstanceLock {
    file: File,
}

impl SingleInstanceLock {
    pub fn acquire(paths: &RuntimePaths) -> Result<Self, DaemonError> {
        paths.ensure_dir()?;
        Self::acquire_at(&paths.lock_file)
    }

    /// The same lock at an explicit path: the app's Oracle record lock sits
    /// next to `daemon.lock` and is not a `RuntimePaths` field. The caller
    /// owns the directory — this only creates the file it locks.
    pub fn acquire_at(path: &Path) -> Result<Self, DaemonError> {
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(path)?;
        if !try_lock_exclusive(&file)? {
            return Err(DaemonError::AlreadyRunning);
        }
        Ok(Self { file })
    }

    /// Replace the record the file carries. Truncate first so a shorter
    /// record cannot leave the tail of a longer one behind it.
    pub fn write_body(&mut self, body: &str) -> io::Result<()> {
        self.file.set_len(0)?;
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(body.as_bytes())?;
        self.file.flush()
    }
}

#[cfg(windows)]
fn try_lock_exclusive(file: &File) -> io::Result<bool> {
    unsafe {
        let mut overlapped: OVERLAPPED = std::mem::zeroed();
        // Not offset 0: see the module comment. One byte is all the mutex
        // needs, and everything before it stays readable from outside.
        overlapped.Anonymous.Anonymous.Offset = RECORD_CAPACITY as u32;
        let ok = LockFileEx(
            file.as_raw_handle() as HANDLE,
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            1,
            0,
            &mut overlapped,
        );
        if ok != 0 {
            return Ok(true);
        }
        let err = io::Error::last_os_error();
        if err.raw_os_error() == Some(33) || err.kind() == io::ErrorKind::WouldBlock {
            // ERROR_LOCK_VIOLATION (33): another process holds the lock.
            Ok(false)
        } else {
            Err(err)
        }
    }
}

#[cfg(not(windows))]
fn try_lock_exclusive(_file: &File) -> io::Result<bool> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "devboule-daemon M3a targets Windows only",
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::paths::RuntimePaths;

    fn unique_dir() -> (RuntimePaths, PathBufDrop) {
        let dir = crate::test_dirs::test_temp_dir("devboule lock");
        (RuntimePaths::from_dir(&dir), PathBufDrop(dir))
    }

    struct PathBufDrop(std::path::PathBuf);
    impl Drop for PathBufDrop {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn stale_lock_file_is_not_a_deadlock() {
        let (paths, _guard) = unique_dir();
        paths.ensure_dir().expect("dir");
        std::fs::write(&paths.lock_file, "pid=999999\ninstance=dead\n").expect("stale");
        let mut lock = SingleInstanceLock::acquire(&paths).expect("stale file must be lockable");
        let record = crate::daemon_record::DaemonRecord::starting(1, "live", &paths.pipe_name);
        lock.write_body(&record.body()).expect("write");
        lock.file.seek(SeekFrom::Start(0)).expect("rewind");
        let mut body = String::new();
        std::io::Read::read_to_string(&mut lock.file, &mut body).expect("read own lock");
        assert!(body.contains("instance=live"));
        assert!(
            body.len() < RECORD_CAPACITY as usize,
            "the record has to fit in the bytes the lock does not cover"
        );
    }

    #[test]
    fn second_lock_on_the_same_dir_fails() {
        let (paths, _guard) = unique_dir();
        let _first = SingleInstanceLock::acquire(&paths).expect("first");
        match SingleInstanceLock::acquire(&paths) {
            Err(DaemonError::AlreadyRunning) => {}
            Ok(_) => panic!("second lock succeeded"),
            Err(error) => panic!("expected AlreadyRunning, got {error}"),
        }
    }

    /// A daemon from the version whose lock covered the whole file still
    /// excludes this one. The byte this version locks is inside that range, so
    /// the two versions cannot both hold the daemon — which is what an app
    /// upgrade starts while its old daemon is still running.
    #[test]
    fn a_whole_file_lock_from_an_older_daemon_still_excludes_this_one() {
        let (paths, _guard) = unique_dir();
        paths.ensure_dir().expect("dir");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&paths.lock_file)
            .expect("open");
        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        let held = unsafe {
            LockFileEx(
                file.as_raw_handle() as HANDLE,
                LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                0,
                u32::MAX,
                u32::MAX,
                &mut overlapped,
            )
        };
        assert_ne!(
            held, 0,
            "the fixture could not take the old whole-file range"
        );
        match SingleInstanceLock::acquire(&paths) {
            Err(DaemonError::AlreadyRunning) => {}
            Ok(_) => panic!("the far byte is inside the old whole-file range"),
            Err(error) => panic!("expected AlreadyRunning, got {error}"),
        }
    }

    #[test]
    fn lock_path_with_spaces_works() {
        let (paths, _guard) = unique_dir();
        assert!(paths.dir.to_string_lossy().contains(' '));
        SingleInstanceLock::acquire(&paths).expect("spaces");
    }

    /// The offset is an agreement between two versions, not a detail of this
    /// one: this build leaves the record readable and takes the byte after it,
    /// and a later build that took a different byte would let both hold the
    /// lock at once — same pipe name, two servers, split clients. The byte is
    /// taken here by its literal number, so moving `RECORD_CAPACITY` reddens
    /// this test before it ships the split.
    #[test]
    fn the_lock_is_taken_at_the_literal_byte_4096() {
        let (paths, _guard) = unique_dir();
        paths.ensure_dir().expect("dir");
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&paths.lock_file)
            .expect("open");
        let mut overlapped: OVERLAPPED = unsafe { std::mem::zeroed() };
        overlapped.Anonymous.Anonymous.Offset = 4096;
        let held = unsafe {
            LockFileEx(
                file.as_raw_handle() as HANDLE,
                LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
                0,
                1,
                0,
                &mut overlapped,
            )
        };
        assert_ne!(held, 0, "the fixture could not take byte 4096");
        match SingleInstanceLock::acquire(&paths) {
            Err(DaemonError::AlreadyRunning) => {}
            Ok(_) => {
                panic!("this build did not take byte 4096: two versions would both hold the lock")
            }
            Err(error) => panic!("expected AlreadyRunning, got {error}"),
        }
    }
}
