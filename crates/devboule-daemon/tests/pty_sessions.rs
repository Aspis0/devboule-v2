//! Real ConPTY + daemon pipe tests. Ignored by default, same pattern as
//! the M2 tests that lived in `session_tests.rs`.
//!
//! Run: `cargo test -p devboule-daemon -- --ignored --nocapture`

#![cfg(windows)]

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use devboule_daemon::current_user_sid;
use devboule_daemon::{
    connect, spawn_daemon, write_test_pty_command, DaemonClient, EventHandler, PtyCommand,
    RuntimePaths, IDLE_SHUTDOWN_GRACE, RING_CAPACITY,
};
use devboule_protocol::{
    ClientHello, Cursor, ErrorCode, OwnerId, Persistence, PersistenceKind, ResumeResult,
    SessionEvent, SessionKind, SessionState,
};
use portable_pty::{CommandBuilder, PtySize};

fn daemon_bin() -> PathBuf {
    if let Some(path) = option_env!("CARGO_BIN_EXE_devboule_daemon") {
        return PathBuf::from(path);
    }
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path.push("target");
    path.push(if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    });
    path.push("devboule-daemon.exe");
    path
}

fn unique_paths() -> (RuntimePaths, PathBuf) {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let dir = std::env::temp_dir().join(format!(
        "devboule m3b {}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("runtime dir with spaces");
    (RuntimePaths::from_dir(&dir), dir)
}

struct Harness {
    paths: RuntimePaths,
    dir: PathBuf,
    child: Option<ChildGuard>,
}

impl Harness {
    fn spawn() -> Self {
        let (paths, dir) = unique_paths();
        let child = ChildGuard::spawn(&paths);
        let mut harness = Self {
            paths,
            dir,
            child: Some(child),
        };
        harness.wait_until_up();
        harness
    }

    fn wait_until_up(&mut self) {
        let hello = test_hello("wait");
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            if let Ok(client) = connect(&self.paths, hello.clone()) {
                drop(client);
                return;
            }
            if let Some(child) = &mut self.child {
                if let Ok(Some(status)) = child.try_wait() {
                    panic!("daemon exited before listen: {status}");
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("daemon did not accept within 5s at {}", self.dir.display());
    }

    fn client(&self, name: &str) -> DaemonClient {
        connect(&self.paths, test_hello(name)).expect("connect")
    }

    fn restart(&mut self) {
        drop(self.child.take());
        std::thread::sleep(Duration::from_millis(150));
        self.child = Some(ChildGuard::spawn(&self.paths));
        self.wait_until_up();
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        drop(self.child.take());
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

struct ChildGuard {
    child: Child,
}

impl ChildGuard {
    fn spawn(paths: &RuntimePaths) -> Self {
        Self {
            child: spawn_daemon(&daemon_bin(), paths).expect("spawn daemon"),
        }
    }

    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        self.child.try_wait()
    }

    fn kill_and_wait(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        self.kill_and_wait();
    }
}

fn test_hello(client: &str) -> ClientHello {
    let owner = OwnerId::new(
        current_user_sid().expect("sid"),
        format!("app-{client}-{}", std::process::id()),
    )
    .expect("owner");
    ClientHello::m3a(owner, "devboule-test")
}

fn collect_handler(received: Arc<Mutex<Vec<SessionEvent>>>) -> EventHandler {
    Arc::new(move |envelope| {
        received.lock().unwrap().push(envelope.event);
    })
}

fn queue_command(paths: &RuntimePaths, command: PtyCommand) {
    write_test_pty_command(paths, &command).expect("write test command");
}

fn cmd_echo(marker: &str) -> PtyCommand {
    PtyCommand::new(
        "cmd.exe",
        vec!["/c".to_string(), format!("echo {marker}")],
        std::env::current_dir().unwrap(),
        Vec::new(),
    )
}

fn cmd_keep() -> PtyCommand {
    PtyCommand::new(
        "cmd.exe",
        vec!["/k".to_string()],
        std::env::current_dir().unwrap(),
        Vec::new(),
    )
}

fn answer_dsr(client: &DaemonClient, id: &str) {
    let _ = client.session_send(id, "\x1b[1;1R");
}

fn start_dsr_pump(
    client: Arc<DaemonClient>,
    id: String,
) -> (Arc<AtomicBool>, std::thread::JoinHandle<()>) {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_for_thread = Arc::clone(&stop);
    let handle = std::thread::spawn(move || {
        while !stop_for_thread.load(Ordering::Acquire) {
            let _ = client.session_send(&id, "\x1b[1;1R");
            std::thread::sleep(Duration::from_millis(25));
        }
    });
    (stop, handle)
}

fn stop_dsr_pump(stop: Arc<AtomicBool>, handle: std::thread::JoinHandle<()>) {
    stop.store(true, Ordering::Release);
    let _ = handle.join();
}

fn wait_for_marker(received: &Mutex<Vec<SessionEvent>>, marker: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if received.lock().unwrap().iter().any(
            |event| matches!(event, SessionEvent::Output { data, .. } if data.contains(marker)),
        ) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[test]
#[ignore = "spawns a real Windows ConPTY; run locally with --ignored"]
fn real_pty_spawn_read_resize_and_teardown() {
    let harness = Harness::spawn();
    queue_command(&harness.paths, cmd_echo("DEVBOULE_PTY_OK"));
    let client = harness.client("echo");
    let session = client
        .session_create(None, SessionKind::Terminal, None)
        .expect("create");
    answer_dsr(&client, &session.id);
    let pump_client = Arc::new(harness.client("dsr"));
    let (stop_dsr, dsr_thread) = start_dsr_pump(Arc::clone(&pump_client), session.id.clone());
    let received = Arc::new(Mutex::new(Vec::new()));
    client
        .session_attach(&session.id, None, collect_handler(Arc::clone(&received)))
        .expect("attach");
    client.session_resize(&session.id, 100, 30).expect("resize");
    let saw_marker = wait_for_marker(&received, "DEVBOULE_PTY_OK", Duration::from_secs(10));
    client.session_close(&session.id).expect("close");
    stop_dsr_pump(stop_dsr, dsr_thread);
    assert!(saw_marker);
    assert!(client.sessions_list().expect("list").is_empty());
}

#[test]
#[ignore = "spawns a real Windows ConPTY; run locally with --ignored"]
fn real_pty_detach_keeps_session_buffers_output_and_close_reaps_child() {
    const MARKER: &str = "DEVBOULE_DETACH_BUFFER";
    let harness = Harness::spawn();
    queue_command(&harness.paths, cmd_keep());
    let client = harness.client("detach");
    let session = client
        .session_create(None, SessionKind::Terminal, None)
        .expect("create");
    answer_dsr(&client, &session.id);
    let pump_client = Arc::new(harness.client("dsr"));
    let (stop_dsr, dsr_thread) = start_dsr_pump(Arc::clone(&pump_client), session.id.clone());
    let received = Arc::new(Mutex::new(Vec::new()));
    client
        .session_attach(&session.id, None, collect_handler(Arc::clone(&received)))
        .expect("attach");
    let received_before_detach = received.lock().unwrap().len();

    client.session_detach(&session.id).expect("detach");
    assert_eq!(
        client.sessions_list().expect("list").len(),
        1,
        "detach must leave the session alive"
    );

    client
        .session_send(&session.id, &format!("echo {MARKER}\r\n"))
        .expect("send while detached");

    let replayed = Arc::new(Mutex::new(Vec::new()));
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut saw_marker = false;
    while Instant::now() < deadline {
        replayed.lock().unwrap().clear();
        client
            .session_attach(&session.id, None, collect_handler(Arc::clone(&replayed)))
            .expect("reattach");
        std::thread::sleep(Duration::from_millis(50));
        saw_marker = replayed.lock().unwrap().iter().any(
            |event| matches!(event, SessionEvent::Output { data, .. } if data.contains(MARKER)),
        );
        if saw_marker {
            break;
        }
        client.session_detach(&session.id).expect("detach to retry");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(saw_marker, "detached output was not retained in the ring");
    assert_eq!(received.lock().unwrap().len(), received_before_detach);

    let expected_replay: Vec<(u64, String)> = replayed
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Output { seq, data } => Some((*seq, data.clone())),
            SessionEvent::Exit { .. } | SessionEvent::Recovered { .. } => None,
        })
        .collect();
    client
        .session_detach(&session.id)
        .expect("detach before close");
    let second = Arc::new(Mutex::new(Vec::new()));
    client
        .session_attach(&session.id, None, collect_handler(Arc::clone(&second)))
        .expect("replay attach");
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let actual: Vec<(u64, String)> = second
            .lock()
            .unwrap()
            .iter()
            .filter_map(|event| match event {
                SessionEvent::Output { seq, data } => Some((*seq, data.clone())),
                SessionEvent::Exit { .. } | SessionEvent::Recovered { .. } => None,
            })
            .collect();
        if actual == expected_replay {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let actual: Vec<(u64, String)> = second
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Output { seq, data } => Some((*seq, data.clone())),
            SessionEvent::Exit { .. } | SessionEvent::Recovered { .. } => None,
        })
        .collect();
    assert!(
        actual.len() >= expected_replay.len(),
        "reattach replay was shorter than the ring captured while detached"
    );
    assert_eq!(
        &actual[..expected_replay.len()],
        &expected_replay[..],
        "reattach must replay previously retained chunks first"
    );

    client.session_close(&session.id).expect("close");
    stop_dsr_pump(stop_dsr, dsr_thread);
    assert!(client.sessions_list().expect("list").is_empty());
}

#[test]
#[ignore = "spawns a real Windows ConPTY; run locally with --ignored"]
fn output_flows_to_an_attached_client() {
    let harness = Harness::spawn();
    queue_command(&harness.paths, cmd_echo("DEVBOULE_ATTACHED"));
    let client = harness.client("flow");
    let session = client
        .session_create(None, SessionKind::Terminal, None)
        .expect("create");
    answer_dsr(&client, &session.id);
    let pump_client = Arc::new(harness.client("dsr"));
    let (stop_dsr, dsr_thread) = start_dsr_pump(Arc::clone(&pump_client), session.id.clone());
    let received = Arc::new(Mutex::new(Vec::new()));
    client
        .session_attach(&session.id, None, collect_handler(Arc::clone(&received)))
        .expect("attach");
    let saw = wait_for_marker(&received, "DEVBOULE_ATTACHED", Duration::from_secs(10));
    client.session_close(&session.id).expect("close");
    stop_dsr_pump(stop_dsr, dsr_thread);
    assert!(saw);
}

#[test]
#[ignore = "spawns a real Windows ConPTY; run locally with --ignored"]
fn detach_stops_delivery_without_killing_the_process() {
    let harness = Harness::spawn();
    queue_command(&harness.paths, cmd_keep());
    let client = harness.client("live");
    let session = client
        .session_create(None, SessionKind::Terminal, None)
        .expect("create");
    answer_dsr(&client, &session.id);
    let pump_client = Arc::new(harness.client("dsr"));
    let (stop_dsr, dsr_thread) = start_dsr_pump(Arc::clone(&pump_client), session.id.clone());
    let received = Arc::new(Mutex::new(Vec::new()));
    client
        .session_attach(&session.id, None, collect_handler(Arc::clone(&received)))
        .expect("attach");
    client.session_detach(&session.id).expect("detach");
    let after = received.lock().unwrap().len();
    client
        .session_send(&session.id, "echo still-alive\r\n")
        .expect("send");
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(received.lock().unwrap().len(), after);
    assert_eq!(client.sessions_list().expect("list").len(), 1);
    client.session_close(&session.id).expect("close");
    stop_dsr_pump(stop_dsr, dsr_thread);
}

#[test]
#[ignore = "spawns a real Windows ConPTY; run locally with --ignored"]
fn reattach_with_a_cursor_replays_only_after() {
    let harness = Harness::spawn();
    queue_command(&harness.paths, cmd_keep());
    let client = harness.client("cursor");
    let session = client
        .session_create(None, SessionKind::Terminal, None)
        .expect("create");
    answer_dsr(&client, &session.id);
    let pump_client = Arc::new(harness.client("dsr"));
    let (stop_dsr, dsr_thread) = start_dsr_pump(Arc::clone(&pump_client), session.id.clone());
    let first = Arc::new(Mutex::new(Vec::new()));
    client
        .session_attach(&session.id, None, collect_handler(Arc::clone(&first)))
        .expect("attach");
    client
        .session_send(&session.id, "echo DEVBOULE_CURSOR_ONE\r\n")
        .expect("send");
    assert!(wait_for_marker(
        &first,
        "DEVBOULE_CURSOR_ONE",
        Duration::from_secs(10)
    ));
    let last_seq = first
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Output { seq, .. } => Some(*seq),
            SessionEvent::Exit { .. } | SessionEvent::Recovered { .. } => None,
        })
        .max()
        .expect("seq");
    client.session_detach(&session.id).expect("detach");
    client
        .session_send(&session.id, "echo DEVBOULE_CURSOR_TWO\r\n")
        .expect("send");
    std::thread::sleep(Duration::from_millis(400));
    let second = Arc::new(Mutex::new(Vec::new()));
    client
        .session_attach(
            &session.id,
            Some(Cursor {
                generation: 1,
                seq: last_seq,
            }),
            collect_handler(Arc::clone(&second)),
        )
        .expect("reattach");
    std::thread::sleep(Duration::from_millis(200));
    let replayed = second.lock().unwrap().clone();
    assert!(
        replayed.iter().any(|event| {
            matches!(event, SessionEvent::Output { data, .. } if data.contains("DEVBOULE_CURSOR_TWO"))
        }),
        "cursor replay missed later output: {replayed:?}"
    );
    assert!(
        replayed.iter().all(|event| match event {
            SessionEvent::Output { seq, .. } => *seq > last_seq,
            SessionEvent::Exit { .. } | SessionEvent::Recovered { .. } => true,
        }),
        "cursor replay included seq <= last_seq"
    );
    client.session_close(&session.id).expect("close");
    stop_dsr_pump(stop_dsr, dsr_thread);
}

#[test]
#[ignore = "spawns a real Windows ConPTY; run locally with --ignored"]
fn two_clients_cannot_both_attach() {
    let harness = Harness::spawn();
    queue_command(&harness.paths, cmd_keep());
    let a = harness.client("one");
    let b = harness.client("two");
    let session = a
        .session_create(None, SessionKind::Terminal, None)
        .expect("create");
    answer_dsr(&a, &session.id);
    let pump_client = Arc::new(harness.client("dsr"));
    let (stop_dsr, dsr_thread) = start_dsr_pump(Arc::clone(&pump_client), session.id.clone());
    a.session_attach(
        &session.id,
        None,
        collect_handler(Arc::new(Mutex::new(Vec::new()))),
    )
    .expect("first attach");
    let err = b
        .session_attach(
            &session.id,
            None,
            collect_handler(Arc::new(Mutex::new(Vec::new()))),
        )
        .expect_err("second attach must fail");
    let message = err.to_string();
    assert!(
        message.contains("already attached"),
        "unexpected error: {message}"
    );
    a.session_close(&session.id).expect("close");
    stop_dsr_pump(stop_dsr, dsr_thread);
}

#[test]
#[ignore = "spawns a real Windows ConPTY; run locally with --ignored"]
fn killing_the_daemon_surfaces_exit_on_the_attached_client() {
    let mut harness = Harness::spawn();
    queue_command(&harness.paths, cmd_keep());
    let client = harness.client("kill");
    let session = client
        .session_create(None, SessionKind::Terminal, None)
        .expect("create");
    let received = Arc::new(Mutex::new(Vec::new()));
    client
        .session_attach(&session.id, None, collect_handler(Arc::clone(&received)))
        .expect("attach");
    let mut child = harness.child.take().expect("child");
    child.child.kill().expect("kill");
    child.child.wait().expect("reap");
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut saw_exit = false;
    while Instant::now() < deadline {
        if received
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event, SessionEvent::Exit { .. }))
        {
            saw_exit = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        saw_exit,
        "attached client did not see exit after daemon death"
    );
}

#[test]
#[ignore = "spawns a real Windows ConPTY; run locally with --ignored"]
fn session_process_exit_reports_through_the_envelope() {
    let harness = Harness::spawn();
    queue_command(&harness.paths, cmd_echo("DEVBOULE_EXIT"));
    let client = harness.client("exit");
    let session = client
        .session_create(None, SessionKind::Terminal, None)
        .expect("create");
    answer_dsr(&client, &session.id);
    let pump_client = Arc::new(harness.client("dsr"));
    let (stop_dsr, dsr_thread) = start_dsr_pump(Arc::clone(&pump_client), session.id.clone());
    let received = Arc::new(Mutex::new(Vec::new()));
    client
        .session_attach(&session.id, None, collect_handler(Arc::clone(&received)))
        .expect("attach");
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut saw_exit = false;
    while Instant::now() < deadline {
        if received
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event, SessionEvent::Exit { .. }))
        {
            saw_exit = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    stop_dsr_pump(stop_dsr, dsr_thread);
    assert!(saw_exit, "process exit did not arrive as an envelope");
}

#[test]
#[ignore = "spawns a real Windows ConPTY; run locally with --ignored"]
fn stale_generation_is_a_mismatch_not_a_silent_stream() {
    let harness = Harness::spawn();
    queue_command(&harness.paths, cmd_keep());
    let client = harness.client("gen");
    let session = client
        .session_create(None, SessionKind::Terminal, None)
        .expect("create");
    let err = client
        .session_attach(
            &session.id,
            Some(Cursor {
                generation: 99,
                seq: 0,
            }),
            collect_handler(Arc::new(Mutex::new(Vec::new()))),
        )
        .expect_err("stale generation");
    match err {
        devboule_daemon::DaemonError::Handshake(wire) => {
            assert_eq!(wire.code, ErrorCode::SessionGenerationMismatch);
        }
        other => panic!("expected generation mismatch, got {other:?}"),
    }
    client.session_close(&session.id).expect("close");
}

#[test]
#[ignore = "spawns a real Windows ConPTY; run locally with --ignored"]
fn detach_does_not_trip_idle_exit() {
    let harness = Harness::spawn();
    queue_command(&harness.paths, cmd_keep());
    let client = harness.client("idle-detach");
    let session = client
        .session_create(None, SessionKind::Terminal, None)
        .expect("create");
    client.session_detach(&session.id).expect("detach");
    drop(client);
    std::thread::sleep(IDLE_SHUTDOWN_GRACE + Duration::from_millis(300));
    let still = harness.client("after-grace");
    still
        .ping()
        .expect("daemon must still be up: detached session is live");
    still.session_close(&session.id).expect("close");
}

#[test]
#[ignore = "spawns a real Windows ConPTY; run locally with --ignored"]
fn real_pty_channel_flood_correctness() {
    const LINES: usize = 50_000;
    const PAYLOAD: &str = "DEVBOULE_LOAD_0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    const DONE: &str = "DEVBOULE_LOAD_DONE";
    let harness = Harness::spawn();
    queue_command(
        &harness.paths,
        PtyCommand::new(
            "pwsh.exe",
            vec![
                "-NoLogo".to_string(),
                "-NoProfile".to_string(),
                "-Command".to_string(),
                format!("$line = '{PAYLOAD}'; 1..{LINES} | ForEach-Object {{ $line }}; '{DONE}'"),
            ],
            std::env::current_dir().unwrap(),
            Vec::new(),
        ),
    );
    let client = harness.client("flood");
    let session = client
        .session_create(None, SessionKind::Terminal, None)
        .expect("create");
    answer_dsr(&client, &session.id);
    let pump_client = Arc::new(harness.client("dsr"));
    let (stop_dsr, dsr_thread) = start_dsr_pump(Arc::clone(&pump_client), session.id.clone());
    let observed = Arc::new(Mutex::new((0usize, 0usize, None::<u64>, false, false)));
    let observed_for_handler = Arc::clone(&observed);
    let writer = Arc::new(harness.client("dsr-inline"));
    let session_id = session.id.clone();
    let handler: EventHandler = Arc::new(move |envelope| {
        if let SessionEvent::Output { seq, data } = envelope.event {
            if data.contains("\x1b[6n") {
                let _ = writer.session_send(&session_id, "\x1b[1;1R");
            }
            let mut observed = observed_for_handler.lock().unwrap();
            let expected = observed.2.map_or(seq, |last| last + 1);
            if seq != expected {
                observed.3 = true;
            }
            if data.contains(DONE) {
                observed.4 = true;
            }
            observed.2 = Some(seq);
            observed.0 += data.len();
            observed.1 += 1;
        }
    });
    client
        .session_attach(&session.id, None, handler)
        .expect("attach");
    let start = Instant::now();
    let deadline = start + Duration::from_secs(60);
    while Instant::now() < deadline {
        if observed.lock().unwrap().4 {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let wall = start.elapsed();
    if !observed.lock().unwrap().4 {
        let _ = client.session_close(&session.id);
        stop_dsr_pump(stop_dsr, dsr_thread);
        panic!("load child did not emit its completion marker");
    }
    let (bytes, chunks, _, reordered, _) = *observed.lock().unwrap();
    let expected_bytes = LINES * PAYLOAD.len();
    let output_complete = bytes >= expected_bytes;
    let close_start = Instant::now();
    client.session_close(&session.id).expect("close");
    let teardown = close_start.elapsed();
    stop_dsr_pump(stop_dsr, dsr_thread);
    println!(
        "PTY_CORRECTNESS lines={LINES} expected_min_bytes={expected_bytes} bytes={bytes} chunks={chunks} wall_ms={} peak_ring_bytes=n/a(daemon) output_complete={output_complete} seq_reordered={reordered} child_reaped=n/a teardown_ms={} clean={}",
        wall.as_millis(),
        teardown.as_millis(),
        client.sessions_list().expect("list").is_empty(),
    );
    assert!(
        output_complete,
        "the generator did not deliver its expected flood"
    );
    assert!(!reordered, "output sequence was dropped or reordered");
}

fn summarize_chunk_sizes(chunk_sizes: &[usize]) -> (usize, f64, usize) {
    let mut sorted = chunk_sizes.to_vec();
    sorted.sort_unstable();
    let min = *sorted.first().expect("transport produced no chunks");
    let max = *sorted.last().unwrap();
    let middle = sorted.len() / 2;
    let median = if sorted.len().is_multiple_of(2) {
        (sorted[middle - 1] + sorted[middle]) as f64 / 2.0
    } else {
        sorted[middle] as f64
    };
    (min, median, max)
}

// Keep this benchmark human-shaped: 256 individual bytes at 8 characters per
// second makes an interval poll visible instead of hiding it in a flood.
const ECHO_SAMPLES: usize = 256;
const ECHO_CADENCE: Duration = Duration::from_millis(125);
const ECHO_TIMEOUT: Duration = Duration::from_secs(5);
const ECHO_KEYS: &[u8] = b"qwertyuiopasdfghjklzxcvbnm";
const MAX_ECHO_P95_MS: f64 = 10.0;

#[derive(Clone, Copy)]
struct EchoTiming {
    end_to_end: Duration,
    request_rtt: Duration,
    event_tail: Duration,
}

fn percentile(values: &[Duration], percentile: usize) -> f64 {
    assert!(!values.is_empty());
    let mut values: Vec<f64> = values.iter().map(Duration::as_secs_f64).collect();
    values.sort_by(f64::total_cmp);
    let rank = ((values.len() * percentile).div_ceil(100)).max(1) - 1;
    values[rank] * 1_000.0
}

fn print_distribution(label: &str, values: &[Duration]) {
    println!(
        "PTY_LATENCY distribution={label} samples={} cadence_ms={} min_ms={:.3} median_ms={:.3} p95_ms={:.3} max_ms={:.3}",
        values.len(),
        ECHO_CADENCE.as_millis(),
        percentile(values, 0),
        percentile(values, 50),
        percentile(values, 95),
        percentile(values, 100),
    );
}

fn wait_for_pty_quiet(rx: &mpsc::Receiver<(Instant, Vec<u8>)>) {
    let deadline = Instant::now() + Duration::from_millis(750);
    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(remaining.min(Duration::from_millis(25))) {
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    while rx.try_recv().is_ok() {}
}

fn direct_conpty_echo_timings() -> Vec<Duration> {
    println!("PTY_LATENCY phase=conpty_direct begin");
    let pty_system = portable_pty::native_pty_system();
    let pair = pty_system
        .openpty(PtySize {
            rows: 32,
            cols: 120,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("open direct ConPTY");
    let mut child = pair
        .slave
        .spawn_command({
            let mut command = CommandBuilder::new("cmd.exe");
            command.arg("/k");
            command
        })
        .expect("spawn direct ConPTY shell");
    let mut killer = child.clone_killer();
    let writer = Arc::new(Mutex::new(
        pair.master.take_writer().expect("direct ConPTY writer"),
    ));
    let reader = pair
        .master
        .try_clone_reader()
        .expect("direct ConPTY reader");
    let (tx, rx) = mpsc::channel::<(Instant, Vec<u8>)>();
    let reader_writer = Arc::clone(&writer);
    let reader_thread = std::thread::spawn(move || {
        let mut reader = reader;
        let mut buf = [0u8; 8192];
        loop {
            let n = match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => n,
                Err(_) => break,
            };
            if buf[..n].windows(4).any(|window| window == b"\x1b[6n") {
                if let Ok(mut writer) = reader_writer.lock() {
                    let _ = writer.write_all(b"\x1b[1;1R");
                    let _ = writer.flush();
                }
            }
            if tx.send((Instant::now(), buf[..n].to_vec())).is_err() {
                break;
            }
        }
    });
    wait_for_pty_quiet(&rx);

    let mut timings = Vec::with_capacity(ECHO_SAMPLES);
    let mut next_send = Instant::now();
    for index in 0..ECHO_SAMPLES {
        if index != 0 {
            let wait = next_send.saturating_duration_since(Instant::now());
            if !wait.is_zero() {
                std::thread::sleep(wait);
            }
        }
        let key = ECHO_KEYS[index % ECHO_KEYS.len()];
        let sent = Instant::now();
        {
            let mut writer = writer.lock().expect("direct ConPTY writer lock");
            writer.write_all(&[key]).expect("write direct ConPTY key");
            writer.flush().expect("flush direct ConPTY key");
        }
        let deadline = sent + ECHO_TIMEOUT;
        loop {
            assert!(
                Instant::now() < deadline,
                "direct ConPTY did not echo sample {index} {key:?}"
            );
            let remaining = deadline.saturating_duration_since(Instant::now());
            let (received, data) = rx.recv_timeout(remaining).unwrap_or_else(|error| {
                panic!("receive direct ConPTY output for sample {index}: {error:?}")
            });
            if received >= sent && data.contains(&key) {
                timings.push(received.saturating_duration_since(sent));
                break;
            }
        }
        next_send += ECHO_CADENCE;
    }

    let _ = killer.kill();
    drop(writer);
    drop(pair.master);
    let _ = child.wait();
    let join_deadline = Instant::now() + Duration::from_millis(500);
    while !reader_thread.is_finished() && Instant::now() < join_deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    if reader_thread.is_finished() {
        let _ = reader_thread.join();
    }
    println!(
        "PTY_LATENCY phase=conpty_direct end samples={}",
        timings.len()
    );
    timings
}

fn daemon_echo_timings() -> Vec<EchoTiming> {
    println!("PTY_LATENCY phase=daemon_echo begin");
    let harness = Harness::spawn();
    queue_command(&harness.paths, cmd_keep());
    let client = harness.client("latency");
    let session = client
        .session_create(None, SessionKind::Terminal, None)
        .expect("create latency session");
    let writer = Arc::new(harness.client("latency-dsr"));
    let (tx, rx) = mpsc::channel::<(Instant, String)>();
    let session_id = session.id.clone();
    let handler: EventHandler = Arc::new(move |envelope| {
        if let SessionEvent::Output { data, .. } = envelope.event {
            if data.contains("\x1b[6n") {
                let _ = writer.session_send(&session_id, "\x1b[1;1R");
            }
            let _ = tx.send((Instant::now(), data));
        }
    });
    client
        .session_attach(&session.id, None, handler)
        .expect("attach latency session");
    answer_dsr(&client, &session.id);
    std::thread::sleep(Duration::from_millis(750));
    while rx.try_recv().is_ok() {}

    let mut timings = Vec::with_capacity(ECHO_SAMPLES);
    let mut next_send = Instant::now();
    for index in 0..ECHO_SAMPLES {
        if index != 0 {
            let wait = next_send.saturating_duration_since(Instant::now());
            if !wait.is_zero() {
                std::thread::sleep(wait);
            }
        }
        let key = ECHO_KEYS[index % ECHO_KEYS.len()] as char;
        let sent = Instant::now();
        client
            .session_send(&session.id, &key.to_string())
            .expect("send latency key");
        let request_returned = Instant::now();
        let deadline = sent + ECHO_TIMEOUT;
        loop {
            assert!(
                Instant::now() < deadline,
                "daemon did not echo sample {index} {key:?}"
            );
            let remaining = deadline.saturating_duration_since(Instant::now());
            let (received, data) = rx.recv_timeout(remaining).unwrap_or_else(|error| {
                panic!(
                    "delivery stalled receiving daemon PTY output for sample {index} {key:?} after {:.3}s: {error:?}",
                    sent.elapsed().as_secs_f64()
                )
            });
            if received >= sent && data.contains(key) {
                timings.push(EchoTiming {
                    end_to_end: received.saturating_duration_since(sent),
                    request_rtt: request_returned.saturating_duration_since(sent),
                    event_tail: received.saturating_duration_since(request_returned),
                });
                break;
            }
        }
        next_send += ECHO_CADENCE;
    }

    client
        .session_close(&session.id)
        .expect("close latency session");
    println!(
        "PTY_LATENCY phase=daemon_echo end samples={}",
        timings.len()
    );
    timings
}

fn daemon_ping_timings() -> Vec<Duration> {
    println!("PTY_LATENCY phase=daemon_ping begin");
    let harness = Harness::spawn();
    let client = harness.client("latency-ping");
    let mut timings = Vec::with_capacity(64);
    let mut next_send = Instant::now();
    for index in 0..64 {
        if index != 0 {
            let wait = next_send.saturating_duration_since(Instant::now());
            if !wait.is_zero() {
                std::thread::sleep(wait);
            }
        }
        let sent = Instant::now();
        client.ping().expect("ping latency control");
        timings.push(sent.elapsed());
        next_send += ECHO_CADENCE;
    }
    println!(
        "PTY_LATENCY phase=daemon_ping end samples={}",
        timings.len()
    );
    timings
}

fn benchmark_file_command(file_path: &Path) -> PtyCommand {
    PtyCommand::new(
        "cmd.exe",
        vec!["/c".to_string(), format!("type {}", file_path.display())],
        std::env::current_dir().unwrap(),
        Vec::new(),
    )
}

#[test]
#[ignore = "spawns a real Windows ConPTY; run locally with --ignored"]
fn real_pty_channel_file_transport_ab_benchmark() {
    const DATA_LINES: usize = 200_000;
    const PAYLOAD: &str = "DEVBOULE_TRANSPORT_0123456789abcdefghijklmnopqrstuvwxyz0123456789";

    let file_path = std::env::temp_dir().join(format!(
        "devboule-pty-transport-{}-{}.txt",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    ));
    {
        let file = std::fs::File::create(&file_path).unwrap();
        let mut file = std::io::BufWriter::new(file);
        for _ in 0..DATA_LINES {
            file.write_all(PAYLOAD.as_bytes()).unwrap();
            file.write_all(b"\r\n").unwrap();
        }
        file.flush().unwrap();
    }
    let expected_file_bytes = std::fs::metadata(&file_path).unwrap().len() as usize;

    let harness = Harness::spawn();
    queue_command(&harness.paths, benchmark_file_command(&file_path));
    let client = harness.client("bench");
    let session = client
        .session_create(None, SessionKind::Terminal, None)
        .expect("create");
    let writer = Arc::new(harness.client("dsr-inline"));
    let session_id = session.id.clone();
    let observed = Arc::new(Mutex::new((
        0usize,
        Vec::<usize>::new(),
        None::<u64>,
        false,
    )));
    let observed_for_handler = Arc::clone(&observed);
    let start = Instant::now();
    let handler: EventHandler = Arc::new(move |envelope| {
        if let SessionEvent::Output { seq, data } = envelope.event {
            if data.contains("\x1b[6n") {
                let _ = writer.session_send(&session_id, "\x1b[1;1R");
            }
            let mut observed = observed_for_handler.lock().unwrap();
            let expected = observed.2.map_or(seq, |last| last + 1);
            if seq != expected {
                observed.3 = true;
            }
            observed.2 = Some(seq);
            observed.0 += data.len();
            observed.1.push(data.len());
        }
    });
    client
        .session_attach(&session.id, None, handler)
        .expect("attach");
    let pump_client = Arc::new(harness.client("dsr"));
    let (stop_dsr, dsr_thread) = start_dsr_pump(Arc::clone(&pump_client), session.id.clone());

    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        if observed.lock().unwrap().0 >= expected_file_bytes {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let complete = observed.lock().unwrap().0 >= expected_file_bytes;
    if !complete {
        let _ = client.session_close(&session.id);
        stop_dsr_pump(stop_dsr, dsr_thread);
        panic!(
            "daemon transport did not finish: bytes={} expected_file_bytes={expected_file_bytes}",
            observed.lock().unwrap().0
        );
    }
    let wall = start.elapsed();
    let (bytes, chunk_sizes, seq_reordered) = {
        let observed = observed.lock().unwrap();
        (observed.0, observed.1.clone(), observed.3)
    };
    let close_start = Instant::now();
    client.session_close(&session.id).expect("close");
    let teardown = close_start.elapsed();
    stop_dsr_pump(stop_dsr, dsr_thread);
    let (chunk_min, chunk_median, chunk_max) = summarize_chunk_sizes(&chunk_sizes);
    let mib_s = bytes as f64 / (1024.0 * 1024.0) / wall.as_secs_f64();
    let messages_per_s = chunk_sizes.len() as f64 / wall.as_secs_f64();
    println!(
        "PTY_AB scenario=daemon_pipe bytes={bytes} expected_file_bytes={expected_file_bytes} wall_ms={} mib_s={mib_s:.2} messages={} messages_per_s={messages_per_s:.2} chunk_min={chunk_min} chunk_median={chunk_median:.1} chunk_max={chunk_max} peak_ring_bytes<={RING_CAPACITY} seq_reordered={seq_reordered} teardown_ms={} clean={}",
        wall.as_millis(),
        chunk_sizes.len(),
        teardown.as_millis(),
        client.sessions_list().expect("list").is_empty(),
    );
    println!("PTY_AB comparison_m2_channel mib_s=0.52 messages_per_s=6951 chunk_median=67");
    assert!(bytes >= expected_file_bytes, "daemon output was truncated");
    assert!(!seq_reordered, "daemon output was dropped or reordered");
    let _ = std::fs::remove_file(&file_path);
}

#[test]
#[ignore = "spawns real Windows ConPTYs and measures a human-cadence run"]
fn real_pty_echo_latency_benchmark() {
    // The direct run is the ConPTY floor. Request RTT covers the client to
    // daemon request/reply path; event tail covers coalescing, connection
    // scheduling, event serialization, and the return pipe. The ping control
    // is the same request/reply framing without PTY output.
    let conpty = direct_conpty_echo_timings();
    print_distribution("conpty_direct", &conpty);

    let daemon = daemon_echo_timings();
    let end_to_end: Vec<_> = daemon.iter().map(|timing| timing.end_to_end).collect();
    let request_rtt: Vec<_> = daemon.iter().map(|timing| timing.request_rtt).collect();
    let event_tail: Vec<_> = daemon.iter().map(|timing| timing.event_tail).collect();
    print_distribution("daemon_end_to_end", &end_to_end);
    print_distribution("daemon_request_rtt", &request_rtt);
    print_distribution("daemon_event_tail", &event_tail);

    let ping = daemon_ping_timings();
    print_distribution("daemon_ping_control", &ping);
    println!(
        "PTY_LATENCY attribution conpty_floor_median_ms={:.3} request_path_median_ms={:.3} event_tail_median_ms={:.3} ping_control_median_ms={:.3} coalesce_flush_budget_ms=4 coalesce_eager_bytes=1 connection_wait=notification",
        percentile(&conpty, 50),
        percentile(&request_rtt, 50),
        percentile(&event_tail, 50),
        percentile(&ping, 50),
    );
    assert_eq!(conpty.len(), ECHO_SAMPLES);
    assert_eq!(daemon.len(), ECHO_SAMPLES);
    assert!(
        percentile(&end_to_end, 95) <= MAX_ECHO_P95_MS,
        "human-cadence echo p95 exceeded {MAX_ECHO_P95_MS:.1} ms"
    );
}

#[test]
#[ignore = "spawns a real Windows ConPTY; run locally with --ignored"]
fn pty_resume_is_not_supported() {
    let harness = Harness::spawn();
    let client = harness.client("resume");
    let result = client
        .session_resume(
            Persistence {
                kind: PersistenceKind::None,
            },
            None,
        )
        .expect("resume rpc");
    assert_eq!(result, ResumeResult::NotSupported);
}

#[test]
#[ignore = "spawns a real Windows ConPTY; run locally with --ignored"]
fn killed_daemon_replays_scrollback_as_recovered() {
    const MARKER: &str = "DEVBOULE_JOURNAL_ALIVE";
    let mut harness = Harness::spawn();
    queue_command(&harness.paths, cmd_keep());
    let client = harness.client("journal-kill");
    let session = client
        .session_create(None, SessionKind::Terminal, None)
        .expect("create");
    let session_id = session.id.clone();
    answer_dsr(&client, &session_id);
    let received = Arc::new(Mutex::new(Vec::new()));
    client
        .session_attach(&session_id, None, collect_handler(Arc::clone(&received)))
        .expect("attach");
    client
        .session_send(&session_id, &format!("echo {MARKER}\r\n"))
        .expect("send marker");
    assert!(wait_for_marker(&received, MARKER, Duration::from_secs(10)));
    std::thread::sleep(Duration::from_millis(400));
    drop(client);
    harness.restart();

    let client = harness.client("journal-replay");
    let listed = client.sessions_list().expect("list");
    let recovered = listed
        .iter()
        .find(|session| session.id == session_id)
        .expect("recovered session missing from list");
    assert!(
        matches!(recovered.state, SessionState::Recovered { .. }),
        "expected recovered, got {:?}",
        recovered.state
    );

    let replayed = Arc::new(Mutex::new(Vec::new()));
    client
        .session_attach(&session_id, None, collect_handler(Arc::clone(&replayed)))
        .expect("attach recovered");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let events = replayed.lock().unwrap().clone();
        let recovered_event = events
            .iter()
            .any(|event| matches!(event, SessionEvent::Recovered { .. }));
        let has_marker = events.iter().any(
            |event| matches!(event, SessionEvent::Output { data, .. } if data.contains(MARKER)),
        );
        if recovered_event && has_marker {
            break;
        }
        if Instant::now() > deadline {
            panic!("recovered replay missing marker or Recovered event: {events:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        replayed
            .lock()
            .unwrap()
            .iter()
            .all(|event| !matches!(event, SessionEvent::Exit { .. })),
        "killed session must not look like a clean Exit"
    );
    let err = client
        .session_send(&session_id, "echo no\r\n")
        .expect_err("send to recovered");
    assert!(
        err.to_string().to_ascii_lowercase().contains("gone"),
        "expected process-gone error, got {err}"
    );
}

#[test]
#[ignore = "spawns a real Windows ConPTY; run locally with --ignored"]
fn clean_exit_reopens_as_ended_not_recovered() {
    const MARKER: &str = "DEVBOULE_JOURNAL_ENDED";
    let mut harness = Harness::spawn();
    queue_command(&harness.paths, cmd_echo(MARKER));
    let client = harness.client("journal-end");
    let session = client
        .session_create(None, SessionKind::Terminal, None)
        .expect("create");
    let session_id = session.id.clone();
    let received = Arc::new(Mutex::new(Vec::new()));
    client
        .session_attach(&session_id, None, collect_handler(Arc::clone(&received)))
        .expect("attach");
    answer_dsr(&client, &session_id);
    assert!(wait_for_marker(&received, MARKER, Duration::from_secs(10)));
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if received
            .lock()
            .unwrap()
            .iter()
            .any(|event| matches!(event, SessionEvent::Exit { .. }))
        {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    std::thread::sleep(Duration::from_millis(400));
    let before = client.sessions_list().expect("list before restart");
    assert!(
        before.iter().any(|session| {
            session.id == session_id && matches!(session.state, SessionState::Ended { .. })
        }),
        "clean exit should be Ended before restart: {before:?}"
    );
    let _ = client.shutdown();
    drop(client);
    harness.restart();

    let client = harness.client("journal-ended-replay");
    let listed = client.sessions_list().expect("list");
    let ended = listed
        .iter()
        .find(|session| session.id == session_id)
        .expect("ended session missing");
    assert!(
        matches!(ended.state, SessionState::Ended { .. }),
        "expected ended, got {:?}",
        ended.state
    );
    let replayed = Arc::new(Mutex::new(Vec::new()));
    client
        .session_attach(&session_id, None, collect_handler(Arc::clone(&replayed)))
        .expect("attach ended");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let events = replayed.lock().unwrap().clone();
        let saw_exit = events
            .iter()
            .any(|event| matches!(event, SessionEvent::Exit { .. }));
        let saw_recovered = events
            .iter()
            .any(|event| matches!(event, SessionEvent::Recovered { .. }));
        if saw_exit {
            assert!(
                !saw_recovered,
                "clean exit must not emit Recovered: {events:?}"
            );
            break;
        }
        if Instant::now() > deadline {
            panic!("ended replay missing Exit: {events:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
#[ignore = "spawns a real Windows ConPTY; run locally with --ignored"]
fn journal_outlives_the_256kib_ring() {
    const LINES: usize = 4_000;
    const PAYLOAD: &str = "DEVBOULE_RING_XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX";
    const DONE: &str = "DEVBOULE_RING_DONE";
    let mut harness = Harness::spawn();
    queue_command(
        &harness.paths,
        PtyCommand::new(
            "pwsh.exe",
            vec![
                "-NoLogo".to_string(),
                "-NoProfile".to_string(),
                "-Command".to_string(),
                format!("$line = '{PAYLOAD}'; 1..{LINES} | ForEach-Object {{ $line }}; '{DONE}'"),
            ],
            std::env::current_dir().unwrap(),
            Vec::new(),
        ),
    );
    let client = harness.client("ring");
    let session = client
        .session_create(None, SessionKind::Terminal, None)
        .expect("create");
    let session_id = session.id.clone();
    answer_dsr(&client, &session_id);
    let pump_client = Arc::new(harness.client("dsr"));
    let (stop_dsr, dsr_thread) = start_dsr_pump(Arc::clone(&pump_client), session_id.clone());
    let received = Arc::new(Mutex::new(Vec::new()));
    client
        .session_attach(&session_id, None, collect_handler(Arc::clone(&received)))
        .expect("attach");
    assert!(wait_for_marker(&received, DONE, Duration::from_secs(30)));
    let live_bytes: usize = received
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Output { data, .. } => Some(data.len()),
            _ => None,
        })
        .sum();
    assert!(
        live_bytes > RING_CAPACITY,
        "live capture was only {live_bytes} bytes, need more than the ring"
    );
    std::thread::sleep(Duration::from_millis(500));
    stop_dsr_pump(stop_dsr, dsr_thread);
    drop(client);
    harness.restart();

    let client = harness.client("ring-replay");
    let replayed = Arc::new(Mutex::new(Vec::new()));
    client
        .session_attach(&session_id, None, collect_handler(Arc::clone(&replayed)))
        .expect("attach recovered");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if replayed.lock().unwrap().iter().any(|event| {
            matches!(
                event,
                SessionEvent::Recovered { .. } | SessionEvent::Exit { .. }
            )
        }) {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let events = replayed.lock().unwrap().clone();
    let mut seqs = Vec::new();
    let mut bytes = 0usize;
    for event in &events {
        match event {
            SessionEvent::Output { seq, data } => {
                seqs.push(*seq);
                bytes += data.len();
            }
            SessionEvent::Recovered { .. } | SessionEvent::Exit { .. } => {}
        }
    }
    assert!(
        bytes > RING_CAPACITY,
        "journal replay was only {bytes} bytes, ring is {RING_CAPACITY}"
    );
    for pair in seqs.windows(2) {
        assert_eq!(
            pair[1],
            pair[0] + 1,
            "journal replay seq gap or duplicate: {seqs:?}"
        );
    }
}

#[test]
#[ignore = "spawns a real Windows ConPTY; run locally with --ignored"]
fn stale_generation_on_recovered_session_is_a_mismatch() {
    const MARKER: &str = "DEVBOULE_JOURNAL_GEN";
    let mut harness = Harness::spawn();
    queue_command(&harness.paths, cmd_echo(MARKER));
    let client = harness.client("gen");
    let session = client
        .session_create(None, SessionKind::Terminal, None)
        .expect("create");
    let session_id = session.id.clone();
    let received = Arc::new(Mutex::new(Vec::new()));
    client
        .session_attach(&session_id, None, collect_handler(Arc::clone(&received)))
        .expect("attach");
    answer_dsr(&client, &session_id);
    assert!(wait_for_marker(&received, MARKER, Duration::from_secs(10)));
    std::thread::sleep(Duration::from_millis(400));
    drop(client);
    harness.restart();

    let client = harness.client("gen-mismatch");
    let err = client
        .session_attach(
            &session_id,
            Some(Cursor {
                generation: 99,
                seq: 0,
            }),
            collect_handler(Arc::new(Mutex::new(Vec::new()))),
        )
        .expect_err("stale generation");
    match err {
        devboule_daemon::DaemonError::Handshake(wire) => {
            assert_eq!(wire.code, ErrorCode::SessionGenerationMismatch);
        }
        other => panic!("expected generation mismatch, got {other}"),
    }
}

#[test]
#[ignore = "spawns a real Windows ConPTY; run locally with --ignored"]
fn journal_growth_after_13mb_flood() {
    const DATA_LINES: usize = 200_000;
    const PAYLOAD: &str = "DEVBOULE_TRANSPORT_0123456789abcdefghijklmnopqrstuvwxyz0123456789";
    let file_path = std::env::temp_dir().join(format!(
        "devboule-journal-growth-{}-{}.txt",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis()
    ));
    {
        let file = std::fs::File::create(&file_path).unwrap();
        let mut file = std::io::BufWriter::new(file);
        for _ in 0..DATA_LINES {
            file.write_all(PAYLOAD.as_bytes()).unwrap();
            file.write_all(b"\r\n").unwrap();
        }
        file.flush().unwrap();
    }
    let expected_file_bytes = std::fs::metadata(&file_path).unwrap().len() as usize;
    let mut harness = Harness::spawn();
    queue_command(&harness.paths, benchmark_file_command(&file_path));
    let client = harness.client("growth");
    let session = client
        .session_create(None, SessionKind::Terminal, None)
        .expect("create");
    let session_id = session.id.clone();
    let writer = Arc::new(harness.client("dsr-inline"));
    let observed = Arc::new(Mutex::new((0usize, 0usize)));
    let observed_for_handler = Arc::clone(&observed);
    let handler: EventHandler = Arc::new(move |envelope| {
        if let SessionEvent::Output { data, .. } = envelope.event {
            if data.contains("\x1b[6n") {
                let _ = writer.session_send(&session_id, "\x1b[1;1R");
            }
            let mut observed = observed_for_handler.lock().unwrap();
            observed.0 += data.len();
            observed.1 += 1;
        }
    });
    client
        .session_attach(&session.id, None, handler)
        .expect("attach");
    let pump_client = Arc::new(harness.client("dsr"));
    let (stop_dsr, dsr_thread) = start_dsr_pump(Arc::clone(&pump_client), session.id.clone());
    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        if observed.lock().unwrap().0 >= expected_file_bytes {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let (live_bytes, live_frames) = *observed.lock().unwrap();
    assert!(
        live_bytes >= expected_file_bytes,
        "flood truncated: {live_bytes} < {expected_file_bytes}"
    );
    std::thread::sleep(Duration::from_millis(800));
    stop_dsr_pump(stop_dsr, dsr_thread);
    drop(client);
    harness.restart();

    let journal_path = harness.paths.journal_file();
    let db_bytes = std::fs::metadata(&journal_path)
        .map(|meta| meta.len())
        .unwrap_or(0);
    let wal_bytes = std::fs::metadata(journal_path.with_extension("db-wal"))
        .map(|meta| meta.len())
        .unwrap_or(0);
    println!(
        "JOURNAL_GROWTH payload_bytes={expected_file_bytes} live_bytes={live_bytes} live_frames={live_frames} db_bytes={db_bytes} wal_bytes={wal_bytes} total_bytes={}",
        db_bytes + wal_bytes
    );

    let client = harness.client("growth-replay");
    let replayed = Arc::new(Mutex::new((0usize, Vec::<u64>::new(), false, false)));
    let replayed_for_handler = Arc::clone(&replayed);
    let handler: EventHandler = Arc::new(move |envelope| {
        let mut replayed = replayed_for_handler.lock().unwrap();
        match envelope.event {
            SessionEvent::Output { seq, data } => {
                replayed.0 += data.len();
                replayed.1.push(seq);
            }
            SessionEvent::Recovered { .. } => replayed.2 = true,
            SessionEvent::Exit { .. } => replayed.3 = true,
        }
    });
    client
        .session_attach(&session.id, None, handler)
        .expect("replay");
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline {
        let snap = replayed.lock().unwrap();
        if snap.2 || snap.3 {
            break;
        }
        drop(snap);
        std::thread::sleep(Duration::from_millis(20));
    }
    let (replay_bytes, seqs, recovered, exited) = {
        let snap = replayed.lock().unwrap();
        (snap.0, snap.1.clone(), snap.2, snap.3)
    };
    println!(
        "JOURNAL_GROWTH replay_bytes={replay_bytes} frames={} recovered={recovered} exited={exited} live_bytes={live_bytes}",
        seqs.len()
    );
    assert_eq!(
        replay_bytes,
        live_bytes,
        "journal silently lost {} live bytes (live={live_bytes} replay={replay_bytes})",
        live_bytes.saturating_sub(replay_bytes)
    );
    assert!(
        recovered || exited,
        "flood replay must end honestly as Recovered or Exit"
    );
    assert!(
        !(recovered && exited),
        "Recovered and Exit must not both fire"
    );
    assert!(
        replay_bytes > RING_CAPACITY,
        "journal replay {replay_bytes} did not outlive the ring"
    );
    for pair in seqs.windows(2) {
        assert_eq!(
            pair[1],
            pair[0] + 1,
            "journal replay seq gap or duplicate around {} -> {}",
            pair[0],
            pair[1]
        );
    }
    let _ = std::fs::remove_file(&file_path);
}
