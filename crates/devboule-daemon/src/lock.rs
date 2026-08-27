//! Single-instance lock. Same primitive as v1's `fs2` exclusive lock: the OS
//! releases it when the process dies, so a leftover file is not a deadlock.

use std::fs::{File, OpenOptions};
use std::io::{self, Seek, SeekFrom, Write};

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
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(&paths.lock_file)?;
        if !try_lock_exclusive(&file)? {
            return Err(DaemonError::AlreadyRunning);
        }
        Ok(Self { file })
    }

    pub fn write_identity(
        &mut self,
        pid: u32,
        instance_id: &str,
        pipe_name: &str,
    ) -> io::Result<()> {
        self.file.set_len(0)?;
        self.file.seek(SeekFrom::Start(0))?;
        writeln!(self.file, "pid={pid}")?;
        writeln!(self.file, "instance={instance_id}")?;
        writeln!(self.file, "pipe={pipe_name}")?;
        self.file.flush()
    }
}

#[cfg(windows)]
fn try_lock_exclusive(file: &File) -> io::Result<bool> {
    unsafe {
        let mut overlapped: OVERLAPPED = std::mem::zeroed();
        let ok = LockFileEx(
            file.as_raw_handle() as HANDLE,
            LOCKFILE_EXCLUSIVE_LOCK | LOCKFILE_FAIL_IMMEDIATELY,
            0,
            u32::MAX,
            u32::MAX,
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
    use std::sync::atomic::{AtomicU64, Ordering};

    fn unique_dir() -> (RuntimePaths, PathBufDrop) {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        let dir = std::env::temp_dir().join(format!(
            "devboule lock {}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
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
        lock.write_identity(1, "live", &paths.pipe_name)
            .expect("write");
        lock.file.seek(SeekFrom::Start(0)).expect("rewind");
        let mut body = String::new();
        std::io::Read::read_to_string(&mut lock.file, &mut body).expect("read own lock");
        assert!(body.contains("instance=live"));
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

    #[test]
    fn lock_path_with_spaces_works() {
        let (paths, _guard) = unique_dir();
        assert!(paths.dir.to_string_lossy().contains(' '));
        SingleInstanceLock::acquire(&paths).expect("spaces");
    }
}
