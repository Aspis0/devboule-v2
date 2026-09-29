//! The pipeline: what happens between the pipe and the file. Queue bounds,
//! drop accounting, the rotation request, the stalled writer, and the
//! bounded shutdown join. The log's decision and its cap live in
//! `daemon_log_tests.rs`.

use super::imp::*;
use std::fs::{File, OpenOptions};
use std::sync::atomic::AtomicIsize;
use std::sync::mpsc::{sync_channel, SyncSender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::Storage::FileSystem::{WriteFile, PIPE_ACCESS_INBOUND};
use windows_sys::Win32::System::Pipes::CreateNamedPipeW;

fn test_core(feed: SyncSender<Msg>) -> Arc<SinkCore> {
    Arc::new(SinkCore {
        feed: Mutex::new(Some(feed)),
        original_stderr: AtomicIsize::new(0),
    })
}

#[test]
fn a_full_queue_drops_and_carries_the_count() {
    let (feed, take) = sync_channel::<Msg>(1);
    let core = test_core(feed);
    let mut dropped: u64 = 0;

    enqueue(&core, &mut dropped, b"one");
    enqueue(&core, &mut dropped, b"two");
    enqueue(&core, &mut dropped, b"three");

    assert_eq!(
        dropped, 2,
        "chunks past the queue are dropped, never waited on"
    );
    match take.recv().expect("the first chunk is queued") {
        Msg::Bytes(bytes, drops) => {
            assert_eq!(bytes, b"one");
            assert_eq!(drops, 0, "no drop had happened before this chunk");
        }
        other => panic!("expected bytes, got {other:?}"),
    }
}

#[test]
fn the_drop_note_is_written_once_per_gap() {
    let dir = crate::test_dirs::test_temp_dir("devboule-drop-note");
    let log_path = dir.join(crate::daemon_log::log_file_name());
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
        .expect("open");
    let mut log = crate::daemon_log::LogFile::appending(Some(file));
    let mut reported: u64 = 0;

    write_drop_note(&mut log, &mut reported, 2);
    write_drop_note(&mut log, &mut reported, 2);
    write_drop_note(&mut log, &mut reported, 5);

    let bytes = std::fs::read(&log_path).expect("read back");
    let text = String::from_utf8_lossy(&bytes);
    assert_eq!(
        text.matches("dropped").count(),
        2,
        "one note per gap, not per chunk: {text:?}"
    );
}

#[test]
fn a_reopen_rotates_only_once_the_lock_request_arrives() {
    let dir = crate::test_dirs::test_temp_dir("devboule-reopen");
    let log_path = dir.join(crate::daemon_log::log_file_name());
    let previous = vec![b'x'; (crate::daemon_log::log_cap_bytes() + 1) as usize];
    std::fs::write(&log_path, &previous).expect("seed the over-cap log");
    let file = OpenOptions::new()
        .append(true)
        .open(&log_path)
        .expect("open the pre-lock log");
    let (feed, take) = sync_channel::<Msg>(4);

    let writer = spawn_writer_thread(take, Some(file)).expect("the writer thread starts");
    feed.send(Msg::Reopen(dir.clone()))
        .expect("queue the rotation");
    feed.send(Msg::Bytes(b"after the lock\n".to_vec(), 0))
        .expect("queue the line");
    drop(feed);
    writer.join().expect("the writer finishes");

    assert_eq!(
        std::fs::read(dir.join(crate::daemon_log::rotated_file_name())).expect("rotated aside"),
        previous,
        "the over-cap pre-lock log moves whole"
    );
    let bytes = std::fs::read(&log_path).expect("fresh log");
    let text = String::from_utf8_lossy(&bytes);
    assert!(
        text.contains("after the lock"),
        "the line lands in the fresh log: {text:?}"
    );
    assert!(
        !text.contains("xxxxxxxxxx"),
        "the fresh log does not repeat the old bytes: {text:?}"
    );
}

#[test]
fn the_final_tally_waits_boundedly_for_a_full_queue() {
    let (feed, take) = sync_channel::<Msg>(1);
    let core = test_core(feed);
    let mut dropped: u64 = 0;
    enqueue(&core, &mut dropped, b"one");
    enqueue(&core, &mut dropped, b"two");

    let started = Instant::now();
    send_final_tally(&core, dropped);
    let elapsed = started.elapsed();

    assert!(
        elapsed >= TALLY_WAIT,
        "the tally must give a stalled writer its moment: {elapsed:?}"
    );
    assert!(
        elapsed < TALLY_WAIT * 4,
        "the tally's wait must be bounded: {elapsed:?}"
    );
    drop(take);
}

/// A named pipe nobody reads plays the stalled file: the writer's writes
/// block once its buffer fills, which is exactly the wedge the reader/writer
/// split exists to contain.
fn stalled_pipe_file(tag: &str) -> File {
    let name = format!(r"\\.\pipe\u4-stall-{tag}");
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let server = unsafe {
        CreateNamedPipeW(
            wide.as_ptr(),
            PIPE_ACCESS_INBOUND,
            0, // byte mode, wait
            1,
            4096,
            4096,
            0,
            std::ptr::null(),
        )
    };
    assert!(server != INVALID_HANDLE_VALUE, "the stall server opens");
    OpenOptions::new()
        .write(true)
        .open(&name)
        .expect("the client connects to the listening instance")
}

#[test]
fn a_stalled_file_writer_never_blocks_the_pipe_reader() {
    let stalled = stalled_pipe_file(&format!("{}-{}", std::process::id(), line!()));
    let (pipe_read, pipe_write) = match open_pipe() {
        Some((read, write)) => (read, write),
        None => panic!("the anonymous pipe opens"),
    };
    let (feed, take) = sync_channel::<Msg>(4);
    let core = test_core(feed);
    let reader =
        spawn_reader_thread(Arc::clone(&core), pipe_read).expect("the reader thread starts");
    let writer = spawn_writer_thread(take, Some(stalled)).expect("the writer thread starts");

    // Play `eprintln!`: push far more than the pipe and the queue can hold.
    // The producer must never block, because the reader keeps draining and
    // drops what the stalled writer cannot take.
    let line = [b'x'; 1023];
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut written: u64 = 0;
    while written < 4 * 1024 * 1024 {
        assert!(Instant::now() < deadline, "the reader stopped draining");
        let mut wrote: u32 = 0;
        let ok = unsafe {
            WriteFile(
                pipe_write,
                line.as_ptr(),
                line.len() as u32,
                &mut wrote,
                std::ptr::null_mut(),
            )
        };
        assert_ne!(ok, 0, "the producer's write must succeed");
        assert_eq!(
            wrote as usize,
            line.len(),
            "a short write would mean a full pipe"
        );
        written += wrote as u64;
    }

    unsafe { CloseHandle(pipe_write) };
    let _ = core
        .feed
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .take();
    let deadline = Instant::now() + SHUTDOWN_JOIN;
    while !reader.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(reader.is_finished(), "a closed pipe ends the reader");
    // The writer stays blocked on the stalled pipe on purpose: it is the
    // state `join_bounded` must contain, which the next test pins.
    drop(writer);
}

/// The bounded join's deadline arm drops the handle detached: a stalled
/// writer must cost the shutdown at most the bound, never a `join` that
/// waits for it. On a tree whose past-deadline arm joins anyway, this test
/// fails after the stalled thread's sleep — bounded, with a message.
#[test]
fn join_bounded_returns_at_the_bound_without_waiting_for_the_thread() {
    let stalled = Mutex::new(Some(std::thread::spawn(|| {
        std::thread::sleep(Duration::from_secs(30))
    })));
    let started = Instant::now();
    join_bounded(&stalled);
    let elapsed = started.elapsed();
    assert!(
        elapsed >= SHUTDOWN_JOIN && elapsed < Duration::from_secs(5),
        "the bounded join must return at the bound, not wait for the thread: {elapsed:?}"
    );

    let finished = Mutex::new(Some(std::thread::spawn(|| ())));
    std::thread::sleep(Duration::from_millis(50));
    join_bounded(&finished);
    assert!(
        finished.lock().unwrap().is_none(),
        "a finished thread is joined and its slot emptied"
    );
}
