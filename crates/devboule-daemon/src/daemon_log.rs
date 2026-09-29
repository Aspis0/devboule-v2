//! The daemon's own log: where stderr goes, and what lands in the file.
//!
//! The app spawns the daemon with stderr on `Stdio::null()` (`spawn.rs`), so
//! every `eprintln!` — journal write failures, quarantines, heartbeat
//! failures — went nowhere. When stderr is going nowhere (the NUL device or
//! an absent handle — never a console, an explicit file redirect, or a pipe
//! someone else is draining), the daemon points stderr at an anonymous pipe
//! and hands the bytes to the pipeline (`log_pipeline`) which appends them
//! to `daemon.log` in the runtime dir, next to `device.json`. The pipe is
//! the sink precisely so a failed stderr write — a panic in Rust — is
//! impossible, and the pipeline (a reader that never touches the file, a
//! writer that owns it) is what keeps an `eprintln!` from ever blocking on
//! the log's account. The best-effort shape — a failed write is dropped and
//! the reader keeps going — is translated from Paseo's #5445 daemon-log fix.
//!
//! Growth: two files, `daemon.log` and `daemon.log.1`, and that is the whole
//! retention — nothing ever deletes them. Once the single-instance lock says
//! this daemon is the one, a log over the 5 MiB cap is moved whole to
//! `daemon.log.1` (replacing the previous rotation), immediately — the
//! request goes into the writer's queue, so a daemon that never prints a
//! line still rotates. Mid-run the writer stops at the cap with one notice.
//! The bound holds for files this code wrote; a `daemon.log` that arrived
//! from anywhere else is moved as it is, and the live file may overshoot the
//! cap by one read chunk plus this notice before the check trips.
//!
//! Lines from concurrent `eprintln!` threads can interleave mid-line (a
//! byte-mode pipe preserves `WriteFile` boundaries, not lines), the same
//! tearing `rpc_trace` accepts across processes. The cap notice is
//! per-process and cannot use `rpc_trace`'s cross-process tail read: the
//! writer holds one append handle instead of re-opening per line.
//!
//! Why the log could not be opened (a directory on the path, a sharing
//! violation, permissions) is recorded once — including a failed reopen
//! after rotation — and reported through [`startup_error`], which `Status`
//! surfaces as `logError` and the diagnostics report carries next to
//! `journalError`; the daemon falls back to its null stderr and keeps
//! serving. A thread that cannot be spawned is the same kind of failure: the
//! error is recorded, stderr goes back to the launcher's sink, and the daemon
//! runs — unless the launcher gave no stderr at all, in which case stderr
//! stays on the undrained pipe and lines can block: there was nothing to
//! fall back to. Pipe handles are never closed while the process lives
//! (see `log_pipeline`); they die with it. Panics: a panic message follows stderr like any other write, but only
//! the clean-shutdown flush reaches the file; a process that dies by panic
//! or `process::exit` elsewhere can lose its last lines. No panic hook is
//! installed.

/// Why the daemon's own log could not be opened at startup, for
/// `Status.logError`. `None` when the log is healthy — or when this daemon
/// does not own its stderr (a terminal, an explicit redirect), where nothing
/// was taken over and there is nothing to report. Read at request time, so
/// a late failure (a reopen after rotation) is still visible.
#[cfg(feature = "server")]
pub fn startup_error() -> Option<String> {
    #[cfg(windows)]
    {
        imp::startup_error()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Point stderr at the daemon log if it is going nowhere. Called once, at the
/// very top of the binary's `main`, before anything can print.
#[cfg(all(windows, feature = "server"))]
pub use imp::take_over_stderr;

/// Rotate an over-cap log aside, once the single-instance lock is held: a
/// losing second daemon must never move the running daemon's log. Called
/// from `run_windows` right after `SingleInstanceLock::acquire`.
#[cfg(all(windows, feature = "server"))]
pub use imp::rotate_after_lock;

/// The file end of the sink, driven by the pipeline's writer thread: the
/// writer swaps in a fresh log on rotation and appends through these.
/// `record_error` is the pipeline's way in for a failure only it sees.
#[cfg(all(windows, feature = "server"))]
pub(crate) use imp::{record_error, reopen_log, LogFile};

/// Test-facing spellings of the file names and the cap, so the pipeline's
/// tests pin real values instead of copies.
#[cfg(all(windows, feature = "server"))]
#[cfg(test)]
pub(crate) use imp::{log_cap_bytes, log_file_name, rotated_file_name};

#[cfg(all(windows, feature = "server"))]
#[cfg(test)]
#[path = "daemon_log_tests.rs"]
mod tests;

#[cfg(all(windows, feature = "server"))]
mod imp {
    use std::fs::{File, OpenOptions};
    use std::io::Write as _;
    use std::path::Path;
    use std::sync::atomic::AtomicIsize;
    use std::sync::mpsc::sync_channel;
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::Storage::FileSystem::{
        GetFileType, FILE_TYPE, FILE_TYPE_CHAR, FILE_TYPE_DISK, FILE_TYPE_PIPE,
    };
    use windows_sys::Win32::System::Console::{
        GetConsoleMode, GetStdHandle, SetStdHandle, CONSOLE_MODE, STD_ERROR_HANDLE,
    };

    use crate::log_pipeline::{self, SinkCore, QUEUE_CAPACITY};
    use crate::paths::RuntimePaths;

    pub(super) const LOG_FILE_NAME: &str = "daemon.log";
    pub(super) const ROTATED_FILE_NAME: &str = "daemon.log.1";
    pub(super) const LOG_CAP_BYTES: u64 = 5 * 1024 * 1024;
    /// How long the rotation request retries a full queue before giving up.
    /// Startup waits with it — at most this long, on the main thread, before
    /// anything is served; a stalled log writer at startup is pathological,
    /// and if the retry gives up the cap still bounds the file.
    const ROTATE_RETRY: Duration = Duration::from_millis(250);

    static STARTUP_ERROR: OnceLock<String> = OnceLock::new();

    /// Test-facing spellings of the file names and the cap, so the tests
    /// assert against the real values instead of copies.
    #[cfg(test)]
    pub(crate) fn log_file_name() -> &'static str {
        LOG_FILE_NAME
    }

    #[cfg(test)]
    pub(crate) fn rotated_file_name() -> &'static str {
        ROTATED_FILE_NAME
    }

    #[cfg(test)]
    pub(crate) fn log_cap_bytes() -> u64 {
        LOG_CAP_BYTES
    }

    pub fn startup_error() -> Option<String> {
        STARTUP_ERROR.get().cloned()
    }

    pub(crate) fn record_error(reason: String) {
        let _ = STARTUP_ERROR.set(reason);
    }

    pub fn take_over_stderr() {
        let Ok(paths) = RuntimePaths::from_env() else {
            return;
        };
        if !stderr_is_going_nowhere() {
            return;
        }
        // No rotation here: the single-instance lock is not held yet, and a
        // losing second daemon must never move the running daemon's log.
        // `rotate_after_lock` does it, through the pipeline's queue.
        let log_path = paths.dir.join(LOG_FILE_NAME);
        let file = match OpenOptions::new().create(true).append(true).open(&log_path) {
            Ok(file) => file,
            Err(error) => {
                // No path in the reason: the wire field is ungated.
                record_error(format!("daemon.log: {error}"));
                return;
            }
        };
        let (reader_end, write_end) = match log_pipeline::open_pipe() {
            Some(ends) => ends,
            None => {
                record_error(
                    "daemon.log: the stderr pipe could not be created; the log stays off"
                        .to_string(),
                );
                return;
            }
        };
        let probe = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
        let original = if probe.is_null() || probe == INVALID_HANDLE_VALUE {
            0
        } else {
            probe as isize
        };
        if unsafe { SetStdHandle(STD_ERROR_HANDLE, write_end) } == 0 {
            record_error(
                "daemon.log: stderr could not be redirected; the log stays off".to_string(),
            );
            // stderr never pointed at the pipe, so both ends are ours alone.
            unsafe {
                CloseHandle(reader_end.0);
                CloseHandle(write_end);
            }
            return;
        }
        // The pipe handles are ours never to close from here on: a
        // concurrent eprintln! may hold the write end mid-write, and they
        // die with the process (see the pipeline doc).
        let core = Arc::new(SinkCore {
            feed: Mutex::new(None),
            original_stderr: AtomicIsize::new(original),
        });
        let (feed, take) = sync_channel(QUEUE_CAPACITY);
        *core.feed.lock().unwrap_or_else(|err| err.into_inner()) = Some(feed);
        // Published before the threads start: rotate_after_lock and shutdown
        // must find the sink the moment the first line can be printed.
        if log_pipeline::publish(Arc::clone(&core)).is_err() {
            return;
        }
        match log_pipeline::spawn_writer_thread(take, Some(file)) {
            Ok(writer) => {
                *log_pipeline::writer_slot()
                    .lock()
                    .unwrap_or_else(|err| err.into_inner()) = Some(writer);
            }
            Err(error) => {
                log_pipeline::abandon(
                    &core,
                    format!("daemon.log: the log writer thread could not be started ({error})"),
                );
                return;
            }
        }
        match log_pipeline::spawn_reader_thread(core.clone(), reader_end) {
            Ok(reader) => {
                *log_pipeline::reader_slot()
                    .lock()
                    .unwrap_or_else(|err| err.into_inner()) = Some(reader);
            }
            Err(error) => {
                log_pipeline::abandon(
                    &core,
                    format!("daemon.log: the log reader thread could not be started ({error})"),
                );
            }
        }
    }

    /// Rotate an over-cap log aside, now that the single-instance lock is
    /// ours. The request goes straight into the writer's queue, so the
    /// rotation happens now — even for a daemon that never prints a line. A
    /// stalled writer may delay it up to [`ROTATE_RETRY`]; giving up leaves
    /// the cap-and-stop bound in place and startup unblocked.
    pub fn rotate_after_lock(runtime_dir: &Path) {
        let mut unsent = Some(runtime_dir.to_path_buf());
        let deadline = Instant::now() + ROTATE_RETRY;
        while let Some(dir) = unsent {
            match log_pipeline::request_reopen(dir) {
                Ok(()) => return,
                Err(back) => {
                    unsent = Some(back);
                    if Instant::now() >= deadline {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        }
    }

    /// The append-only end of the log: everything is best effort, and past
    /// the cap the log declares itself stopped and writes nothing further —
    /// notes included, except the cap's own notice, which `append` writes as
    /// it trips.
    pub(crate) struct LogFile {
        file: Option<File>,
        capped: bool,
    }

    impl LogFile {
        /// The constructor the pipeline's writer and the tests start from;
        /// `None` is the log that was never opened, whose writes are
        /// silently dropped.
        pub(crate) fn appending(file: Option<File>) -> LogFile {
            LogFile {
                file,
                capped: false,
            }
        }

        pub(crate) fn append(&mut self, bytes: &[u8]) {
            let Some(file) = self.file.as_mut() else {
                return;
            };
            if self.capped {
                return;
            }
            let size = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
            if size >= LOG_CAP_BYTES {
                let _ = writeln!(
                    file,
                    "daemon log: the cap of {LOG_CAP_BYTES} bytes is reached; \
                     stderr lines are dropped until restart"
                );
                self.capped = true;
                return;
            }
            let _ = file.write_all(bytes);
        }

        /// Subject to the cap like everything else: a note must never raise
        /// the high-water mark past it.
        pub(crate) fn note(&mut self, text: &str) {
            if self.capped {
                return;
            }
            let Some(file) = self.file.as_mut() else {
                return;
            };
            let size = file.metadata().map(|metadata| metadata.len()).unwrap_or(0);
            if size >= LOG_CAP_BYTES {
                return;
            }
            let _ = writeln!(file, "{text}");
        }
    }

    /// Close the current log, move an over-cap one aside now that the lock
    /// is ours, and open fresh. Failure keeps writing where it can and is
    /// recorded, so a late `Status` still sees it.
    pub(crate) fn reopen_log(runtime_dir: &Path) -> LogFile {
        let log_path = runtime_dir.join(LOG_FILE_NAME);
        if rotate_at_startup(&log_path) {
            if let Ok(mut file) = OpenOptions::new().append(true).open(&log_path) {
                let _ = writeln!(
                    file,
                    "daemon log: an over-cap {LOG_FILE_NAME} was moved to {ROTATED_FILE_NAME}"
                );
            }
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_path)
            .ok();
        if file.is_none() {
            record_error("daemon.log: could not reopen after rotation".to_string());
        }
        LogFile::appending(file)
    }

    /// Move an over-cap log aside whole, replacing the previous rotation.
    /// A failed rotation is left alone: the cap below still bounds the file.
    pub(super) fn rotate_at_startup(log_path: &Path) -> bool {
        let Ok(metadata) = std::fs::metadata(log_path) else {
            return false;
        };
        if metadata.len() <= LOG_CAP_BYTES {
            return false;
        }
        std::fs::rename(log_path, log_path.with_file_name(ROTATED_FILE_NAME)).is_ok()
    }

    /// Take over stderr only when nothing is reading it: `Stdio::null()` (a
    /// character device that is not a console) or an absent handle. A
    /// console keeps the terminal, an explicit file keeps its file, a pipe
    /// keeps its reader.
    fn stderr_is_going_nowhere() -> bool {
        let stderr = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
        if stderr.is_null() || stderr == INVALID_HANDLE_VALUE {
            return true;
        }
        let file_type = unsafe { GetFileType(stderr) };
        let is_console = file_type == FILE_TYPE_CHAR
            && unsafe {
                let mut mode: CONSOLE_MODE = 0;
                GetConsoleMode(stderr, &mut mode) != 0
            };
        stderr_going_nowhere(file_type, is_console)
    }

    pub(super) fn stderr_going_nowhere(file_type: FILE_TYPE, is_console: bool) -> bool {
        match file_type {
            FILE_TYPE_DISK | FILE_TYPE_PIPE => false,
            FILE_TYPE_CHAR => !is_console,
            _ => true,
        }
    }
}
