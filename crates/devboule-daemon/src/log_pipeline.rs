//! Moving bytes from the pipe to the log file without ever blocking the
//! writing thread — and the shutdown protocol around that.
//!
//! Two threads: a reader that only `ReadFile`s the pipe into a bounded queue
//! (128 chunks of up to 8 KiB, the log's whole memory ceiling), and a writer
//! that owns the file. A stalled or failing file write costs queue space,
//! never an `eprintln!`: a full queue drops chunks and counts them, and one
//! note per gap records the drops when the file is writable again. After
//! three consecutive read failures the reader hands stderr back instead of
//! spinning on a dead drain.
//!
//! The pipeline's pipe handles are **never closed while the process lives**:
//! a concurrent `eprintln!` may hold the stderr value and be mid-`WriteFile`,
//! and closing a handle under it turns a log write into a panic (a failed
//! stderr write panics in Rust). They die with the process, by design.
//! Shutdown restores stderr, disconnects the queue so the writer drains the
//! goodbye lines, and gives the writer a bounded drain whose past-deadline
//! arm drops the handle detached — a stalled writer delays the exit by at
//! most the bound, never past it. The reader is detached without waiting:
//! its handles are never closed, so it cannot finish, and waiting for it
//! would be budget spent, not work.

/// Restore stderr, disconnect the queue, and give the writer a bounded
/// moment to drain, so the goodbye lines land. Called from the tail of
/// `run_windows` and from `main`'s exit paths. No-op without a taken-over
/// stderr.
#[cfg(all(windows, feature = "server"))]
pub use imp::shutdown_log;

/// The surface `daemon_log`'s takeover drives: the sink, the threads, and
/// the queue. Everything else about the pipeline is its own.
#[cfg(all(windows, feature = "server"))]
pub(crate) use imp::{
    abandon, open_pipe, publish, reader_slot, request_reopen, spawn_reader_thread,
    spawn_writer_thread, writer_slot, SinkCore, QUEUE_CAPACITY,
};

#[cfg(all(windows, feature = "server"))]
#[cfg(test)]
#[path = "log_pipeline_tests.rs"]
mod tests;

#[cfg(all(windows, feature = "server"))]
mod imp {
    use std::fs::File;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicIsize, Ordering};
    use std::sync::mpsc::{Receiver, SyncSender, TrySendError};
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{GetLastError, ERROR_BROKEN_PIPE, HANDLE};
    use windows_sys::Win32::Storage::FileSystem::ReadFile;
    use windows_sys::Win32::System::Console::{SetStdHandle, STD_ERROR_HANDLE};
    use windows_sys::Win32::System::Pipes::CreatePipe;

    const PIPE_BUFFER_BYTES: u32 = 64 * 1024;
    /// The queue between reader and writer: 128 chunks of up to 8 KiB, the
    /// most the log may ever hold in memory.
    pub(crate) const QUEUE_CAPACITY: usize = 128;
    const READ_CHUNK: usize = 8 * 1024;
    /// A pipe that keeps failing reads would otherwise fill and block every
    /// `eprintln!` in the process; after this many, stderr goes back.
    const MAX_CONSECUTIVE_READ_ERRORS: u32 = 3;
    /// How long shutdown waits for the writer to flush; only the writer is
    /// joined, the reader is detached.
    pub(super) const SHUTDOWN_JOIN: Duration = Duration::from_millis(250);
    /// How long the reader's final drop tally waits for a full queue before
    /// giving up and leaving the count an honest lower bound.
    pub(super) const TALLY_WAIT: Duration = Duration::from_millis(250);

    /// One item of work for the writer thread. The daemon log names it
    /// once, to build the queue the takeover hands the writer.
    #[derive(Debug)]
    pub(crate) enum Msg {
        /// stderr bytes, carrying the reader's dropped-chunk count as of the
        /// bytes before this one.
        Bytes(Vec<u8>, u64),
        /// The single-instance lock is ours: rotate and open fresh.
        Reopen(PathBuf),
        /// The reader's final count, so a note is still written if the file
        /// is writable again.
        Dropped(u64),
    }

    /// The state the threads, `rotate_after_lock` and shutdown share. Lives
    /// in a static so the exit paths can reach it without threading handles
    /// through `main`.
    pub(crate) struct SinkCore {
        /// The queue's sending end; `None` once shutdown disconnected it.
        pub(crate) feed: Mutex<Option<SyncSender<Msg>>>,
        /// HANDLE as `isize`; 0 means there was no stderr to restore.
        pub(crate) original_stderr: AtomicIsize,
    }

    static SINK: OnceLock<Arc<SinkCore>> = OnceLock::new();
    static READER_THREAD: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);
    static WRITER_THREAD: Mutex<Option<std::thread::JoinHandle<()>>> = Mutex::new(None);

    /// Publish the sink before the threads start: `request_reopen` and
    /// shutdown must find it the moment the first line can be printed.
    pub(crate) fn publish(core: Arc<SinkCore>) -> Result<(), Arc<SinkCore>> {
        SINK.set(core)
    }

    pub(crate) fn writer_slot() -> &'static Mutex<Option<std::thread::JoinHandle<()>>> {
        &WRITER_THREAD
    }

    pub(crate) fn reader_slot() -> &'static Mutex<Option<std::thread::JoinHandle<()>>> {
        &READER_THREAD
    }

    /// Queue a rotation request. `Err(dir)` means the queue is full — the
    /// caller may retry; a disconnected channel means shutdown won, and the
    /// request is dropped with everything else.
    pub(crate) fn request_reopen(dir: PathBuf) -> Result<(), PathBuf> {
        let Some(core) = SINK.get() else {
            return Ok(());
        };
        let guard = core.feed.lock().unwrap_or_else(|err| err.into_inner());
        let Some(feed) = guard.as_ref() else {
            return Ok(());
        };
        match feed.try_send(Msg::Reopen(dir)) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(msg)) => match msg {
                Msg::Reopen(dir) => Err(dir),
                other => unreachable!("only a rotation is sent here, got {other:?}"),
            },
            Err(TrySendError::Disconnected(_)) => Ok(()),
        }
    }

    pub(crate) fn restore_stderr(core: &SinkCore) {
        let original = core.original_stderr.swap(0, Ordering::SeqCst);
        // A zero means there was no valid stderr to begin with; restoring to
        // NULL would turn every later eprintln! into a panic.
        if original != 0 {
            unsafe { SetStdHandle(STD_ERROR_HANDLE, original as _) };
        }
    }

    /// Disconnect the queue: the writer drains what is queued and exits.
    pub(crate) fn disconnect(core: &SinkCore) {
        let _ = core
            .feed
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .take();
    }

    pub fn shutdown_log() {
        let Some(core) = SINK.get() else {
            return;
        };
        // Restore first: anything printed from here on goes to the launcher's
        // stderr, not into the pipeline we are closing.
        restore_stderr(core);
        disconnect(core);
        join_bounded(&WRITER_THREAD);
        detach_reader();
    }

    /// The reader is detached, not joined: its handles are never closed, so
    /// `ERROR_BROKEN_PIPE` can never release it from `ReadFile`, and waiting
    /// for it would be shutdown budget spent, not work.
    fn detach_reader() {
        reader_slot()
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .take();
    }

    /// Join a thread only while it is finished; past the bound the handle is
    /// dropped detached, never joined — `join` blocks until the thread exits,
    /// and a stalled log writer must not hold the daemon's exit.
    pub(super) fn join_bounded(slot: &Mutex<Option<std::thread::JoinHandle<()>>>) {
        let deadline = Instant::now() + SHUTDOWN_JOIN;
        loop {
            let finished = {
                let guard = slot.lock().unwrap_or_else(|err| err.into_inner());
                match guard.as_ref() {
                    None => return,
                    Some(handle) => handle.is_finished(),
                }
            };
            if finished {
                if let Some(handle) = slot.lock().unwrap_or_else(|err| err.into_inner()).take() {
                    let _ = handle.join();
                }
                return;
            }
            if Instant::now() >= deadline {
                slot.lock().unwrap_or_else(|err| err.into_inner()).take();
                return;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    pub(crate) fn open_pipe() -> Option<(PipeRead, HANDLE)> {
        let mut read_end: HANDLE = std::ptr::null_mut();
        let mut write_end: HANDLE = std::ptr::null_mut();
        let ok = unsafe {
            CreatePipe(
                &mut read_end,
                &mut write_end,
                std::ptr::null(),
                PIPE_BUFFER_BYTES,
            )
        };
        (ok != 0).then_some((PipeRead(read_end), write_end))
    }

    /// A pipe read handle moved into the reader thread. A handle is an
    /// integer, not a pointer into process memory: handing it to another
    /// thread of the same process is sound, and the newtype keeps it from
    /// being copied anywhere else.
    pub(crate) struct PipeRead(pub(crate) HANDLE);
    unsafe impl Send for PipeRead {}

    /// A thread that could not be spawned is a log failure, not a daemon
    /// failure: record it, hand stderr back, and let whatever started drain.
    /// Nothing is closed — the handles die with the process.
    pub(crate) fn abandon(core: &Arc<SinkCore>, reason: String) {
        crate::daemon_log::record_error(reason);
        restore_stderr(core);
        disconnect(core);
        join_bounded(&WRITER_THREAD);
        detach_reader();
    }

    pub(crate) fn spawn_reader_thread(
        core: Arc<SinkCore>,
        read_end: PipeRead,
    ) -> std::io::Result<std::thread::JoinHandle<()>> {
        std::thread::Builder::new()
            .name("daemon-log-read".into())
            .spawn(move || reader_loop(core, read_end))
    }

    pub(crate) fn spawn_writer_thread(
        take: Receiver<Msg>,
        log: Option<File>,
    ) -> std::io::Result<std::thread::JoinHandle<()>> {
        std::thread::Builder::new()
            .name("daemon-log-write".into())
            .spawn(move || writer_loop(take, crate::daemon_log::LogFile::appending(log)))
    }

    /// Move bytes from the pipe into the queue. The reader never touches the
    /// file and never waits on it, so a stalled or failed write costs queue
    /// space and dropped chunks, never a blocked `eprintln!`.
    fn reader_loop(core: Arc<SinkCore>, read_end: PipeRead) {
        let mut chunk = [0u8; READ_CHUNK];
        let mut dropped: u64 = 0;
        let mut consecutive_errors: u32 = 0;
        loop {
            let mut read: u32 = 0;
            let ok = unsafe {
                ReadFile(
                    read_end.0,
                    chunk.as_mut_ptr(),
                    chunk.len() as u32,
                    &mut read,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                if unsafe { GetLastError() } == ERROR_BROKEN_PIPE {
                    break;
                }
                consecutive_errors += 1;
                if consecutive_errors >= MAX_CONSECUTIVE_READ_ERRORS {
                    // A pipe that never yields bytes again fills up and
                    // blocks every eprintln! in the process: hand stderr
                    // back instead of spinning on a dead drain.
                    restore_stderr(&core);
                    break;
                }
                std::thread::sleep(Duration::from_millis(100));
                continue;
            }
            consecutive_errors = 0;
            if read == 0 {
                continue;
            }
            enqueue(&core, &mut dropped, &chunk[..read as usize]);
        }
        send_final_tally(&core, dropped);
        // The read end is deliberately never closed: a concurrent eprintln!
        // may hold the write end mid-write, and breaking the pipe under it
        // turns a log write into a panic. The handle dies with the process.
    }

    /// Queue one chunk; on a full queue drop it and count the drop.
    pub(super) fn enqueue(core: &SinkCore, dropped: &mut u64, bytes: &[u8]) {
        let guard = core.feed.lock().unwrap_or_else(|err| err.into_inner());
        let Some(feed) = guard.as_ref() else {
            return;
        };
        match feed.try_send(Msg::Bytes(bytes.to_vec(), *dropped)) {
            Ok(()) => {}
            Err(TrySendError::Full(_)) => *dropped += 1,
            Err(TrySendError::Disconnected(_)) => {}
        }
    }

    /// The reader's exit tally. A full queue gets a bounded moment to free
    /// up; a writer still stalled keeps the log's number an honest lower
    /// bound by omission.
    pub(super) fn send_final_tally(core: &SinkCore, dropped: u64) {
        let deadline = Instant::now() + TALLY_WAIT;
        loop {
            let guard = core.feed.lock().unwrap_or_else(|err| err.into_inner());
            match guard.as_ref() {
                None => return,
                Some(feed) => match feed.try_send(Msg::Dropped(dropped)) {
                    Ok(()) | Err(TrySendError::Disconnected(_)) => return,
                    Err(TrySendError::Full(_)) => {
                        if Instant::now() >= deadline {
                            return;
                        }
                    }
                },
            }
            drop(guard);
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn writer_loop(take: Receiver<Msg>, mut log: crate::daemon_log::LogFile) {
        let mut reported_drops: u64 = 0;
        while let Ok(msg) = take.recv() {
            match msg {
                Msg::Reopen(dir) => log = crate::daemon_log::reopen_log(&dir),
                Msg::Bytes(bytes, drops) => {
                    write_drop_note(&mut log, &mut reported_drops, drops);
                    log.append(&bytes);
                }
                Msg::Dropped(drops) => write_drop_note(&mut log, &mut reported_drops, drops),
            }
        }
    }

    pub(super) fn write_drop_note(
        log: &mut crate::daemon_log::LogFile,
        reported: &mut u64,
        drops: u64,
    ) {
        if drops <= *reported {
            return;
        }
        *reported = drops;
        log.note(&format!(
            "daemon log: {drops} stderr chunk(s) (up to {READ_CHUNK} bytes each) dropped \
             while the log writer could not keep up"
        ));
    }
}
