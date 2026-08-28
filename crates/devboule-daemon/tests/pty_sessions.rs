//! Real ConPTY + daemon pipe tests. Ignored by default, same pattern as
//! the M2 tests that lived in `session_tests.rs`.
//!
//! Since M3.5 the daemon answers terminal queries (DSR/CPR) itself and is
//! the single responder; these tests deliberately do NOT answer `ESC[6n`.
//! A test only passes here if the daemon-side reply path works.
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
use devboule_daemon::Screen;
use devboule_daemon::{
    connect, spawn_daemon, write_test_pty_command, DaemonClient, EventHandler, PtyCommand,
    RuntimePaths, IDLE_SHUTDOWN_GRACE, PENDING_OUTPUT_BUDGET_BYTES,
};
use devboule_protocol::{
    ClientHello, ClientMessage, Cursor, CursorShape, ErrorCode, OwnerId, Persistence,
    PersistenceKind, ResumeResult, SessionEvent, SessionKind, SessionState,
};
use portable_pty::{CommandBuilder, PtySize};
use windows_sys::Win32::Foundation::{CloseHandle, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{
    GetProcessHandleCount, OpenProcess, WaitForSingleObject, PROCESS_QUERY_INFORMATION,
    PROCESS_SYNCHRONIZE,
};

/// The historical 256 KiB live ring. Journal tests still use its size as
/// the "more bytes than any screen window" floor a transcript must exceed.
const LEGACY_RING_CAPACITY: usize = 256 * 1024;

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

fn cmd_gated_exit(ready_file: &Path) -> PtyCommand {
    let ready_file = ready_file.display().to_string().replace('\'', "''");
    powershell_command(format!(
        "while (-not (Test-Path -LiteralPath '{ready_file}')) {{ Start-Sleep -Milliseconds 25 }}; exit 0"
    ))
}

fn cmd_spawn_long_lived_child_with_pid_file(marker: &str, pid_file: &Path) -> PtyCommand {
    let pid_file = pid_file.display().to_string().replace('\'', "''");
    // Start-Process always dispatches through ShellExecuteExW, which is licensed to
    // pop its own modal error dialog when a launch fails. An explicit
    // ProcessStartInfo with UseShellExecute = $false is the only PowerShell path
    // guaranteed to reach CreateProcessW, where a failed spawn is an exception.
    // The stability gate makes "PID published" imply "grandchild is alive", so a
    // teardown waiting on the PID file can never race an in-flight launch.
    let script = format!(
        "Start-Sleep -Milliseconds 250; \
         $psi = New-Object System.Diagnostics.ProcessStartInfo; \
         $psi.FileName = Join-Path $env:SystemRoot 'System32\\PING.EXE'; \
         $psi.Arguments = '-t 127.0.0.1'; \
         $psi.UseShellExecute = $false; \
         $psi.CreateNoWindow = $true; \
         $p = [System.Diagnostics.Process]::Start($psi); \
         Start-Sleep -Milliseconds 300; \
         if ($p.HasExited) {{ throw 'grandchild exited during startup stability gate' }}; \
         Set-Content -LiteralPath '{pid_file}' -Value $p.Id; \
         Write-Output ('{marker}' + $p.Id); \
         Wait-Process -Id $p.Id"
    );
    powershell_command(script)
}

fn powershell_command(script: String) -> PtyCommand {
    PtyCommand::new(
        "powershell.exe",
        vec![
            "-NoLogo".to_string(),
            "-NoProfile".to_string(),
            "-NonInteractive".to_string(),
            "-Command".to_string(),
            script,
        ],
        std::env::current_dir().unwrap(),
        Vec::new(),
    )
}

/// A marker counts as observed when it reached the client as live output OR
/// when it is part of the screen state the client received: content applied
/// before a snapshot boundary legitimately shows up inside the snapshot's
/// rendered ANSI instead of an Output frame.
fn event_carries_marker(event: &SessionEvent, marker: &str) -> bool {
    match event {
        SessionEvent::Output { data, .. } | SessionEvent::Snapshot { data, .. } => {
            data.contains(marker)
        }
        SessionEvent::Exit { .. }
        | SessionEvent::Recovered { .. }
        | SessionEvent::JournalDegraded
        | SessionEvent::OutputGap { .. } => false,
    }
}

fn apply_snapshot_state(screen: &mut Screen, event: &SessionEvent) {
    let SessionEvent::Snapshot {
        data,
        cursor,
        bracketed_paste,
        line_wrap,
        title,
        ..
    } = event
    else {
        return;
    };
    screen.process(data.as_bytes());
    let shape = match cursor.shape {
        CursorShape::Block => 1,
        CursorShape::Underline => 3,
        CursorShape::Bar => 5,
    } + u16::from(!cursor.blinking);
    let state = format!(
        "\x1b[{};{}H\x1b[?25{}\x1b[{shape} q\x1b[?2004{}\x1b[?7{}{}",
        cursor.row + 1,
        cursor.col + 1,
        if cursor.visible { 'h' } else { 'l' },
        if *bracketed_paste { 'h' } else { 'l' },
        if *line_wrap { 'h' } else { 'l' },
        title
            .as_deref()
            .map(|title| format!("\x1b]2;{}\x1b\\", title))
            .unwrap_or_default(),
    );
    screen.process(state.as_bytes());
}

fn wait_for_marker(received: &Mutex<Vec<SessionEvent>>, marker: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if received
            .lock()
            .unwrap()
            .iter()
            .any(|event| event_carries_marker(event, marker))
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    false
}

#[derive(Clone)]
struct BenchmarkDiagnostics {
    attach_succeeded: bool,
    output_events: u64,
    last_output_at_ms: Option<u128>,
    exit_seen: bool,
    exit_code: Option<u32>,
    last_error: Option<String>,
    last_error_at_ms: Option<u128>,
    cleanup_error: Option<String>,
}

impl BenchmarkDiagnostics {
    fn new() -> Self {
        Self {
            attach_succeeded: false,
            output_events: 0,
            last_output_at_ms: None,
            exit_seen: false,
            exit_code: None,
            last_error: None,
            last_error_at_ms: None,
            cleanup_error: None,
        }
    }

    fn record_error(
        &mut self,
        phase: &str,
        error: &devboule_daemon::DaemonError,
        started: Instant,
    ) {
        self.last_error = Some(format!("{phase}: {error}"));
        self.last_error_at_ms = Some(started.elapsed().as_millis());
    }

    fn record_output(&mut self, started: Instant) {
        self.output_events += 1;
        self.last_output_at_ms = Some(started.elapsed().as_millis());
    }

    fn record_exit(&mut self, code: Option<u32>) {
        self.exit_seen = true;
        self.exit_code = code;
    }

    fn record_cleanup_error(&mut self, phase: &str, error: &devboule_daemon::DaemonError) {
        self.cleanup_error = Some(format!("{phase}: {error}"));
    }
}

fn wait_for_pid_file(path: &Path, timeout: Duration) -> u32 {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Ok(contents) = std::fs::read_to_string(path) {
            if let Ok(pid) = contents.trim().parse() {
                return pid;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("did not receive process id file {}", path.display());
}

fn process_is_alive(pid: u32) -> bool {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        return false;
    }
    let state = unsafe { WaitForSingleObject(handle, 0) } == WAIT_TIMEOUT;
    unsafe { CloseHandle(handle) };
    state
}

fn wait_for_process_exit(pid: u32, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if !process_is_alive(pid) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(!process_is_alive(pid), "process {pid} is still alive");
}

fn daemon_handle_count(pid: u32) -> u32 {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_INFORMATION, 0, pid) };
    assert!(
        !handle.is_null(),
        "could not open daemon {pid} for handle count"
    );
    let mut count = 0;
    let result = unsafe { GetProcessHandleCount(handle, &mut count) };
    unsafe { CloseHandle(handle) };
    assert_ne!(result, 0, "GetProcessHandleCount failed for daemon {pid}");
    count
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
    let received = Arc::new(Mutex::new(Vec::new()));
    client
        .session_attach(&session.id, None, collect_handler(Arc::clone(&received)))
        .expect("attach");
    client.session_resize(&session.id, 100, 30).expect("resize");
    let saw_marker = wait_for_marker(&received, "DEVBOULE_PTY_OK", Duration::from_secs(10));
    client.session_close(&session.id).expect("close");
    assert!(saw_marker);
    assert!(client.sessions_list().expect("list").is_empty());
}

#[test]
#[ignore = "spawns a real Windows ConPTY; run locally with --ignored"]
fn real_pty_detach_keeps_screen_state_and_close_reaps_child() {
    const MARKER: &str = "DEVBOULE_DETACH_BUFFER";
    let harness = Harness::spawn();
    queue_command(&harness.paths, cmd_keep());
    let client = harness.client("detach");
    let session = client
        .session_create(None, SessionKind::Terminal, None)
        .expect("create");
    let received = Arc::new(Mutex::new(Vec::new()));
    client
        .session_attach(&session.id, None, collect_handler(Arc::clone(&received)))
        .expect("attach");
    // Attach always enqueues a snapshot; let it arrive before sampling so
    // the assertion below cannot count an in-flight pre-detach event.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && received.lock().unwrap().is_empty() {
        std::thread::sleep(Duration::from_millis(10));
    }
    let received_before_detach = received.lock().unwrap().len();

    client.session_detach(&session.id).expect("detach");
    assert_eq!(
        client.sessions_list().expect("list").len(),
        1,
        "detach must leave the session alive"
    );

    // Output produced while detached goes to the journal and the emulator.
    // The reattaching client synchronises through a screen snapshot, so the
    // detached command's text must be part of the snapshot's ANSI.
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
        saw_marker = replayed
            .lock()
            .unwrap()
            .iter()
            .any(|event| event_carries_marker(event, MARKER));
        if saw_marker {
            break;
        }
        client.session_detach(&session.id).expect("detach to retry");
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        saw_marker,
        "output from the detached window was not in the reattached screen state"
    );
    assert_eq!(
        received.lock().unwrap().len(),
        received_before_detach,
        "a detached client must receive nothing further"
    );

    // A second attach must also be snapshot-first, with live output strictly
    // after the snapshot boundary and no gaps anywhere.
    client
        .session_detach(&session.id)
        .expect("detach before close");
    let second = Arc::new(Mutex::new(Vec::new()));
    client
        .session_attach(&session.id, None, collect_handler(Arc::clone(&second)))
        .expect("second attach");
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline {
        let events = second.lock().unwrap();
        if events
            .iter()
            .any(|event| matches!(event, SessionEvent::Snapshot { .. }))
        {
            break;
        }
        drop(events);
        std::thread::sleep(Duration::from_millis(20));
    }
    let events = second.lock().unwrap().clone();
    let first = events
        .first()
        .expect("attach must deliver the snapshot first");
    let as_of_seq = match first {
        SessionEvent::Snapshot { as_of_seq, .. } => *as_of_seq,
        other => panic!("first event must be a snapshot, got {other:?}"),
    };
    for event in &events {
        match event {
            SessionEvent::Output { seq, .. } => {
                assert!(*seq > as_of_seq, "live output before the snapshot boundary");
            }
            SessionEvent::OutputGap { .. } => {
                panic!("live attach must never declare a gap: {events:?}")
            }
            _ => {}
        }
    }

    client.session_close(&session.id).expect("close");
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
    let received = Arc::new(Mutex::new(Vec::new()));
    client
        .session_attach(&session.id, None, collect_handler(Arc::clone(&received)))
        .expect("attach");
    let saw = wait_for_marker(&received, "DEVBOULE_ATTACHED", Duration::from_secs(10));
    client.session_close(&session.id).expect("close");
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
}

#[test]
#[ignore = "spawns a real Windows ConPTY; run locally with --ignored"]
fn reattach_with_a_cursor_synchronises_screen_state() {
    let harness = Harness::spawn();
    queue_command(&harness.paths, cmd_keep());
    let client = harness.client("cursor");
    let session = client
        .session_create(None, SessionKind::Terminal, None)
        .expect("create");
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
            SessionEvent::Exit { .. }
            | SessionEvent::Recovered { .. }
            | SessionEvent::JournalDegraded
            | SessionEvent::OutputGap { .. }
            | SessionEvent::Snapshot { .. } => None,
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
    // Wait for the marker to reach the client's view; a fixed sleep would
    // race shell scheduling (especially on a loaded machine).
    assert!(wait_for_marker(
        &second,
        "DEVBOULE_CURSOR_TWO",
        Duration::from_secs(10)
    ));
    let replayed = second.lock().unwrap().clone();
    // A live attach is snapshot-first: the first event carries the emulator
    // boundary, which must be at or past everything the client saw before.
    let as_of_seq = match replayed.first().expect("reattach delivered nothing") {
        SessionEvent::Snapshot { as_of_seq, .. } => *as_of_seq,
        other => panic!("first event must be the snapshot, got {other:?}"),
    };
    assert!(
        as_of_seq >= last_seq,
        "snapshot boundary {as_of_seq} behind the client cursor {last_seq}"
    );
    assert!(
        replayed.iter().all(|event| match event {
            SessionEvent::Output { seq, .. } => *seq > as_of_seq,
            SessionEvent::OutputGap { .. } => false,
            _ => true,
        }),
        "live output after the snapshot must be strictly newer and gap-free: {replayed:?}"
    );
    // Cursor or no cursor, the client's view must show the output produced
    // while it was away: either inside the snapshot's screen state, or as a
    // later live event.
    assert!(
        replayed
            .iter()
            .any(|event| event_carries_marker(event, "DEVBOULE_CURSOR_TWO")),
        "reattached view missed output produced after the cursor: {replayed:?}"
    );
    client.session_close(&session.id).expect("close");
}

#[test]
#[ignore = "spawns a real Windows ConPTY; run locally with --ignored"]
fn detach_reattach_cursor_never_delivers_a_sequence_twice() {
    const LAST_MARKER: &str = "DEVBOULE_DUP_2499";

    let harness = Harness::spawn();
    queue_command(&harness.paths, cmd_keep());
    let client = Arc::new(harness.client("cursor-duplicate"));
    let session = client
        .session_create(None, SessionKind::Terminal, None)
        .expect("create");

    let delivered = Arc::new(Mutex::new(Vec::<u64>::new()));
    let first_received = Arc::new(Mutex::new(Vec::<SessionEvent>::new()));
    let first_received_for_handler = Arc::clone(&first_received);
    let delivered_for_first_handler = Arc::clone(&delivered);
    client
        .session_attach(
            &session.id,
            None,
            Arc::new(move |envelope| {
                if let SessionEvent::Output { seq, .. } = envelope.event.clone() {
                    delivered_for_first_handler.lock().unwrap().push(seq);
                }
                first_received_for_handler
                    .lock()
                    .unwrap()
                    .push(envelope.event);
            }),
        )
        .expect("attach");

    // With M3.5 the first event of an attach is the screen snapshot, which
    // carries no sequence. Sample the cursor only after at least one real
    // output frame was delivered.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && delivered.lock().unwrap().is_empty() {
        std::thread::sleep(Duration::from_millis(10));
    }
    client.session_detach(&session.id).expect("initial detach");
    // Detach is an ordinary request. Fairness may deliver one more output
    // before its reply, so the cursor must be sampled after the reply rather
    // than before sending the request. That is the last sequence the client
    // has actually observed for the first attachment.
    let (cursor_seq, delivered_before_reattach) = {
        let delivered = delivered.lock().unwrap();
        (
            delivered.last().copied().expect("initial output sequence"),
            delivered.len(),
        )
    };

    let flood = (0..2500)
        .map(|index| format!("echo DEVBOULE_DUP_{index:04}\r\n"))
        .collect::<String>();
    client
        .session_send(&session.id, &flood)
        .expect("send flood");
    // Let the detached process accumulate output before the reattach. The
    // final marker below is still required after every attach, so this
    // pause cannot make the test accept a partial flood.
    std::thread::sleep(Duration::from_millis(750));

    let second_received = Arc::new(Mutex::new(Vec::<SessionEvent>::new()));
    let second_received_for_handler = Arc::clone(&second_received);
    let delivered_for_second_handler = Arc::clone(&delivered);
    let second_started = Arc::new(AtomicBool::new(false));
    let second_started_for_handler = Arc::clone(&second_started);
    client
        .session_attach(
            &session.id,
            Some(Cursor {
                generation: 1,
                seq: cursor_seq,
            }),
            Arc::new(move |envelope| {
                if let SessionEvent::Output { seq, .. } = envelope.event.clone() {
                    delivered_for_second_handler.lock().unwrap().push(seq);
                }
                second_received_for_handler
                    .lock()
                    .unwrap()
                    .push(envelope.event);
                second_started_for_handler.store(true, Ordering::Release);
            }),
        )
        .expect("reattach");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !second_started.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        second_started.load(Ordering::Acquire),
        "first replay did not arrive"
    );

    let ping_client = Arc::clone(&client);
    let ping_stop = Arc::new(AtomicBool::new(false));
    let ping_stop_for_thread = Arc::clone(&ping_stop);
    let ping_thread = std::thread::spawn(move || {
        while !ping_stop_for_thread.load(Ordering::Acquire) {
            let _ = ping_client.write_frame(&ClientMessage::Ping { id: 10_000_000 });
        }
    });
    std::thread::sleep(Duration::from_millis(25));
    ping_stop.store(true, Ordering::Release);
    let _ = ping_thread.join();
    client.session_detach(&session.id).expect("second detach");
    let cursor_after_first_replay = delivered
        .lock()
        .unwrap()
        .last()
        .copied()
        .expect("first replay sequence");

    let third_received = Arc::new(Mutex::new(Vec::<SessionEvent>::new()));
    let third_received_for_handler = Arc::clone(&third_received);
    let delivered_for_third_handler = Arc::clone(&delivered);
    client
        .session_attach(
            &session.id,
            Some(Cursor {
                generation: 1,
                seq: cursor_after_first_replay,
            }),
            Arc::new(move |envelope| {
                if let SessionEvent::Output { seq, .. } = envelope.event.clone() {
                    delivered_for_third_handler.lock().unwrap().push(seq);
                }
                third_received_for_handler
                    .lock()
                    .unwrap()
                    .push(envelope.event);
            }),
        )
        .expect("third attach");
    assert!(wait_for_marker(
        &third_received,
        LAST_MARKER,
        Duration::from_secs(10)
    ));

    let delivered = delivered.lock().unwrap().clone();
    assert!(
        delivered[delivered_before_reattach..]
            .iter()
            .all(|seq| *seq > cursor_seq),
        "reattach replay included a sequence already observed before reattach: {delivered:?}"
    );
    let mut unique = delivered.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        delivered.len(),
        "a sequence was delivered twice across detach/reattach: {delivered:?}"
    );
    client.session_close(&session.id).expect("close");
}

#[test]
#[ignore = "spawns a real Windows ConPTY; run locally with --ignored"]
fn shutdown_drain_never_delivers_a_pending_sequence_twice() {
    const LAST_MARKER: &str = "DEVBOULE_SHUTDOWN_DUP_9999";

    let harness = Harness::spawn();
    queue_command(&harness.paths, cmd_keep());
    let client = Arc::new(harness.client("shutdown-duplicate"));
    let session = client
        .session_create(None, SessionKind::Terminal, None)
        .expect("create");

    let baseline = Arc::new(Mutex::new(Vec::<u64>::new()));
    let baseline_for_handler = Arc::clone(&baseline);
    client
        .session_attach(
            &session.id,
            None,
            Arc::new(move |envelope| {
                if let SessionEvent::Output { seq, .. } = envelope.event {
                    baseline_for_handler.lock().unwrap().push(seq);
                }
            }),
        )
        .expect("attach");
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && baseline.lock().unwrap().is_empty() {
        std::thread::sleep(Duration::from_millis(10));
    }
    let cursor_seq = baseline
        .lock()
        .unwrap()
        .last()
        .copied()
        .expect("initial output sequence");
    client.session_detach(&session.id).expect("initial detach");

    let ready_file = harness.dir.join("shutdown-flood.ready");
    let ready_path = ready_file.display();
    let flood = format!(
        "for /L %i in (0,1,9999) do @echo DEVBOULE_SHUTDOWN_DUP_%i\r\necho READY>\"{ready_path}\"\r\n"
    );
    client
        .session_send(&session.id, &flood)
        .expect("send flood");
    let ready_deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < ready_deadline && !ready_file.exists() {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(ready_file.exists(), "flood command did not finish");
    // Ensure the replay is large enough that the fairness turn leaves output
    // in pending_events when the Shutdown request reaches the writer loop.
    std::thread::sleep(Duration::from_millis(750));

    let received = Arc::new(Mutex::new(Vec::<SessionEvent>::new()));
    let received_for_handler = Arc::clone(&received);
    let ping_sent = Arc::new(AtomicBool::new(false));
    let ping_sent_for_handler = Arc::clone(&ping_sent);
    let ping_client = Arc::clone(&client);
    client
        .session_attach(
            &session.id,
            Some(Cursor {
                generation: 1,
                seq: cursor_seq,
            }),
            Arc::new(move |envelope| {
                received_for_handler.lock().unwrap().push(envelope.event);
                if !ping_sent_for_handler.swap(true, Ordering::AcqRel) {
                    ping_client
                        .write_frame(&ClientMessage::Ping { id: 20_000_000 })
                        .expect("fairness ping");
                    // Keep the client reader from draining the pipe while the
                    // server reaches the already-queued Shutdown request.
                    std::thread::sleep(Duration::from_millis(100));
                }
            }),
        )
        .expect("reattach");

    let first_event_deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < first_event_deadline && !ping_sent.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        ping_sent.load(Ordering::Acquire),
        "first replay did not arrive"
    );

    // Force one ordinary request iteration to take the fairness turn. It
    // writes one event and replies, leaving the rest of this multi-frame
    // replay queued for Shutdown's pre-dispatch drain.
    client.shutdown().expect("shutdown");
    let saw_marker = wait_for_marker(&received, LAST_MARKER, Duration::from_secs(10));
    assert!(
        saw_marker,
        "final marker missing after Shutdown drain; received {} events",
        received.lock().unwrap().len()
    );

    let output_seqs: Vec<u64> = received
        .lock()
        .unwrap()
        .iter()
        .filter_map(|event| match event {
            SessionEvent::Output { seq, .. } => Some(*seq),
            SessionEvent::Exit { .. }
            | SessionEvent::Recovered { .. }
            | SessionEvent::JournalDegraded
            | SessionEvent::OutputGap { .. }
            | SessionEvent::Snapshot { .. } => None,
        })
        .collect();
    let mut unique = output_seqs.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(
        unique.len(),
        output_seqs.len(),
        "Shutdown duplicated a pending sequence: {output_seqs:?}"
    );
    assert!(output_seqs.iter().all(|seq| *seq > cursor_seq));
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
    let ready_file = harness.dir.join("normal-exit.ready");
    queue_command(&harness.paths, cmd_gated_exit(&ready_file));
    let client = harness.client("exit");
    let session = client
        .session_create(None, SessionKind::Terminal, None)
        .expect("create");
    let received = Arc::new(Mutex::new(Vec::new()));
    client
        .session_attach(&session.id, None, collect_handler(Arc::clone(&received)))
        .expect("attach");
    std::fs::write(&ready_file, b"ready").expect("release normal exit");
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
    assert!(saw_exit, "process exit did not arrive as an envelope");
    let listed = client.sessions_list().expect("list ended session");
    assert!(listed.iter().any(|listed| {
        listed.id == session.id && matches!(listed.state, SessionState::Ended { code: Some(0), .. })
    }));
}

#[test]
#[ignore = "spawns a real Windows ConPTY and a real child process; run locally with --ignored"]
fn closing_session_kills_its_grandchild_at_the_os() {
    let harness = Harness::spawn();
    const MARKER: &str = "DEVBOULE_SESSION_TREE_PID=";
    let pid_file = harness.dir.join("session-close.pid");
    queue_command(
        &harness.paths,
        cmd_spawn_long_lived_child_with_pid_file(MARKER, &pid_file),
    );
    let client = harness.client("tree-close");
    let session = client
        .session_create(None, SessionKind::Terminal, None)
        .expect("create tree session");
    let _received = Arc::new(Mutex::new(Vec::new()));
    client
        .session_attach(&session.id, None, collect_handler(Arc::clone(&_received)))
        .expect("attach tree session");
    let pid = wait_for_pid_file(&pid_file, Duration::from_secs(10));
    assert!(process_is_alive(pid), "grandchild {pid} never became live");

    client
        .session_close(&session.id)
        .expect("close tree session");
    wait_for_process_exit(pid, Duration::from_secs(5));
    println!("JOB_TREE session_close grandchild_pid={pid} os_alive=false");
}

#[test]
#[ignore = "spawns a real Windows ConPTY and a real child process; run locally with --ignored"]
fn killing_daemon_kills_every_session_tree_at_the_os() {
    let mut harness = Harness::spawn();
    const MARKER: &str = "DEVBOULE_DAEMON_TREE_PID=";
    let pid_file = harness.dir.join("daemon-kill.pid");
    queue_command(
        &harness.paths,
        cmd_spawn_long_lived_child_with_pid_file(MARKER, &pid_file),
    );
    let client = harness.client("tree-daemon-kill");
    let session = client
        .session_create(None, SessionKind::Terminal, None)
        .expect("create daemon tree session");
    let _received = Arc::new(Mutex::new(Vec::new()));
    client
        .session_attach(&session.id, None, collect_handler(Arc::clone(&_received)))
        .expect("attach daemon tree session");
    let pid = wait_for_pid_file(&pid_file, Duration::from_secs(10));
    assert!(process_is_alive(pid), "grandchild {pid} never became live");

    let mut daemon = harness.child.take().expect("daemon child");
    daemon.child.kill().expect("kill daemon without cleanup");
    daemon.child.wait().expect("reap daemon");
    wait_for_process_exit(pid, Duration::from_secs(5));
    drop(client);
    println!("JOB_TREE daemon_kill grandchild_pid={pid} os_alive=false");
}

#[test]
#[ignore = "spawns two real Windows ConPTYs and child processes; run locally with --ignored"]
fn closing_one_session_does_not_kill_the_other_session_tree() {
    let harness = Harness::spawn();
    const FIRST_MARKER: &str = "DEVBOULE_FIRST_TREE_PID=";
    const SECOND_MARKER: &str = "DEVBOULE_SECOND_TREE_PID=";
    let client = harness.client("tree-isolation");

    let first_pid_file = harness.dir.join("isolation-first.pid");
    queue_command(
        &harness.paths,
        cmd_spawn_long_lived_child_with_pid_file(FIRST_MARKER, &first_pid_file),
    );
    let first = client
        .session_create(None, SessionKind::Terminal, None)
        .expect("create first tree session");
    let _first_received = Arc::new(Mutex::new(Vec::new()));
    client
        .session_attach(
            &first.id,
            None,
            collect_handler(Arc::clone(&_first_received)),
        )
        .expect("attach first tree session");
    let first_pid = wait_for_pid_file(&first_pid_file, Duration::from_secs(10));

    let second_pid_file = harness.dir.join("isolation-second.pid");
    queue_command(
        &harness.paths,
        cmd_spawn_long_lived_child_with_pid_file(SECOND_MARKER, &second_pid_file),
    );
    let second = client
        .session_create(None, SessionKind::Terminal, None)
        .expect("create second tree session");
    let _second_received = Arc::new(Mutex::new(Vec::new()));
    client
        .session_attach(
            &second.id,
            None,
            collect_handler(Arc::clone(&_second_received)),
        )
        .expect("attach second tree session");
    let second_pid = wait_for_pid_file(&second_pid_file, Duration::from_secs(10));
    assert!(process_is_alive(first_pid));
    assert!(process_is_alive(second_pid));

    client
        .session_close(&first.id)
        .expect("close first tree session");
    wait_for_process_exit(first_pid, Duration::from_secs(5));
    assert!(
        process_is_alive(second_pid),
        "closing first session killed second grandchild {second_pid}"
    );

    client
        .session_close(&second.id)
        .expect("close second tree session");
    wait_for_process_exit(second_pid, Duration::from_secs(5));
    println!(
        "JOB_TREE isolation first_pid={first_pid} second_pid={second_pid} first_dead=true second_alive_after_first_close=true"
    );
}

#[test]
#[ignore = "spawns many real Windows ConPTY sessions; run locally with --ignored"]
fn opening_and_closing_sessions_does_not_leak_daemon_handles() {
    let harness = Harness::spawn();
    let client = harness.client("tree-handles");
    let daemon_pid = client.status().expect("status").pid;
    std::thread::sleep(Duration::from_millis(100));
    let baseline = daemon_handle_count(daemon_pid);
    let mut peak = baseline;

    for index in 0..32 {
        queue_command(&harness.paths, cmd_keep());
        let session = client
            .session_create(None, SessionKind::Terminal, None)
            .unwrap_or_else(|error| panic!("create session {index}: {error}"));
        client
            .session_close(&session.id)
            .unwrap_or_else(|error| panic!("close session {index}: {error}"));
        peak = peak.max(daemon_handle_count(daemon_pid));
    }
    std::thread::sleep(Duration::from_millis(250));
    let final_count = daemon_handle_count(daemon_pid);
    println!(
        "JOB_TREE handles daemon_pid={daemon_pid} baseline={baseline} peak={peak} final={final_count}"
    );
    assert!(
        final_count <= baseline + 8,
        "daemon handles accumulated across session close: baseline={baseline} final={final_count} peak={peak}"
    );
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
    // (bytes, chunks, last_seq, reordered, done, max snapshot boundary)
    let observed = Arc::new(Mutex::new((
        0usize,
        0usize,
        None::<u64>,
        false,
        false,
        0u64,
    )));
    let observed_for_handler = Arc::clone(&observed);
    let output_gap = Arc::new(Mutex::new(None::<(u64, u64, u64, u64)>));
    let output_gap_for_handler = Arc::clone(&output_gap);
    let handler: EventHandler = Arc::new(move |envelope| match envelope.event {
        SessionEvent::Output { seq, data } => {
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
        SessionEvent::OutputGap {
            from_seq,
            to_seq,
            dropped_bytes,
            dropped_frames,
        } => {
            *output_gap_for_handler.lock().unwrap() =
                Some((from_seq, to_seq, dropped_bytes, dropped_frames));
        }
        // A snapshot at N means the unsent suffix up to N was legitimately
        // coalesced for this viewer; the flood reached the client's view.
        SessionEvent::Snapshot {
            as_of_seq, data, ..
        } => {
            let mut observed = observed_for_handler.lock().unwrap();
            observed.5 = observed.5.max(as_of_seq);
            if data.contains(DONE) {
                observed.4 = true;
            }
        }
        SessionEvent::Exit { .. }
        | SessionEvent::Recovered { .. }
        | SessionEvent::JournalDegraded => {}
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
        panic!("load child did not emit its completion marker");
    }
    let (bytes, chunks, _, reordered, _, max_snapshot) = *observed.lock().unwrap();
    let expected_bytes = LINES * PAYLOAD.len();
    // Byte-complete delivery is the fast-client case. When snapshot
    // coalescing activated, the view was resynchronised instead; the flood
    // is then proven complete by the DONE marker inside the snapshot.
    let output_complete = if max_snapshot == 0 {
        bytes >= expected_bytes
    } else {
        true
    };
    let output_gap = *output_gap.lock().unwrap();
    let status = client.status().expect("status");
    let frames_per_s = chunks as f64 / wall.as_secs_f64();
    let close_start = Instant::now();
    client.session_close(&session.id).expect("close");
    let teardown = close_start.elapsed();
    println!(
        "PTY_CORRECTNESS lines={LINES} expected_min_bytes={expected_bytes} bytes={bytes} chunks={chunks} frames_per_s={frames_per_s:.2} wall_ms={} peak_ring_bytes={} ring_evicted_bytes={} ring_dropped_frames={} output_gap={output_gap:?} output_complete={output_complete} seq_reordered={reordered} child_reaped=n/a teardown_ms={} clean={}",
        wall.as_millis(),
        status.peak_ring_bytes,
        status.ring_evicted_bytes,
        status.ring_dropped_frames,
        teardown.as_millis(),
        client.sessions_list().expect("list").is_empty(),
    );
    assert!(
        output_complete,
        "the generator did not deliver its expected flood"
    );
    assert!(!reordered, "output sequence was dropped or reordered");
    assert!(
        output_gap.is_none(),
        "output loss was declared: {output_gap:?}"
    );
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
    let (tx, rx) = mpsc::channel::<(Instant, String)>();
    let handler: EventHandler = Arc::new(move |envelope| {
        if let SessionEvent::Output { data, .. } = envelope.event {
            // The daemon answers terminal queries itself; every Output event
            // here is echo content.
            let _ = tx.send((Instant::now(), data));
        }
    });
    client
        .session_attach(&session.id, None, handler)
        .expect("attach latency session");
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

fn normalize_journal_growth_output(data: &str) -> String {
    // ConPTY may issue a device-status query while the shell starts. It is
    // transport noise, not part of the file transcript; apply this exact
    // filter to both the live and replay captures.
    data.replace("\x1b[6n", "")
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
    let diagnostics = Arc::new(Mutex::new(BenchmarkDiagnostics::new()));
    let observed = Arc::new(Mutex::new((
        0usize,
        Vec::<usize>::new(),
        None::<u64>,
        false,
    )));
    let observed_for_handler = Arc::clone(&observed);
    let diagnostics_for_handler = Arc::clone(&diagnostics);
    let start = Instant::now();
    let handler: EventHandler = Arc::new(move |envelope| match envelope.event {
        SessionEvent::Output { seq, data } => {
            diagnostics_for_handler.lock().unwrap().record_output(start);
            let mut observed = observed_for_handler.lock().unwrap();
            let expected = observed.2.map_or(seq, |last| last + 1);
            if seq != expected {
                observed.3 = true;
            }
            observed.2 = Some(seq);
            observed.0 += data.len();
            observed.1.push(data.len());
        }
        SessionEvent::Exit { code } => {
            diagnostics_for_handler.lock().unwrap().record_exit(code);
        }
        SessionEvent::Recovered { .. }
        | SessionEvent::JournalDegraded
        | SessionEvent::OutputGap { .. }
        | SessionEvent::Snapshot { .. } => {}
    });
    if let Err(error) = client.session_attach(&session.id, None, handler) {
        let mut diagnostics = diagnostics.lock().unwrap();
        diagnostics.record_error("attach", &error, start);
        panic!(
            "daemon transport attach failed: elapsed_ms={} attach_succeeded={} last_error={} last_error_at_ms={}",
            start.elapsed().as_millis(),
            diagnostics.attach_succeeded,
            diagnostics.last_error.as_deref().unwrap_or("none"),
            diagnostics
                .last_error_at_ms
                .map_or_else(|| "none".to_string(), |value| value.to_string()),
        );
    }
    diagnostics.lock().unwrap().attach_succeeded = true;

    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        if observed.lock().unwrap().0 >= expected_file_bytes {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let complete = observed.lock().unwrap().0 >= expected_file_bytes;
    if !complete {
        let wait_elapsed_ms = start.elapsed().as_millis();
        let transport_diagnostics = diagnostics.lock().unwrap().clone();
        if let Err(error) = client.session_close(&session.id) {
            diagnostics
                .lock()
                .unwrap()
                .record_cleanup_error("close during failure cleanup", &error);
        }
        let total_elapsed_ms = start.elapsed().as_millis();
        let observed_bytes = observed.lock().unwrap().0;
        let cleanup_error = diagnostics.lock().unwrap().cleanup_error.clone();
        panic!(
            "daemon transport did not finish: bytes={observed_bytes} expected_file_bytes={expected_file_bytes} elapsed_ms={wait_elapsed_ms} cleanup_elapsed_ms={} total_elapsed_ms={total_elapsed_ms} attach_succeeded={} output_events={} last_output_at_ms={} exit_seen={} exit_code={} last_error={} last_error_at_ms={} cleanup_error={}",
            total_elapsed_ms.saturating_sub(wait_elapsed_ms),
            transport_diagnostics.attach_succeeded,
            transport_diagnostics.output_events,
            transport_diagnostics
                .last_output_at_ms
                .map_or_else(|| "none".to_string(), |value| value.to_string()),
            transport_diagnostics.exit_seen,
            transport_diagnostics
                .exit_code
                .map_or_else(|| "none".to_string(), |value| value.to_string()),
            transport_diagnostics.last_error.as_deref().unwrap_or("none"),
            transport_diagnostics
                .last_error_at_ms
                .map_or_else(|| "none".to_string(), |value| value.to_string()),
            cleanup_error.as_deref().unwrap_or("none"),
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
    let (chunk_min, chunk_median, chunk_max) = summarize_chunk_sizes(&chunk_sizes);
    let mib_s = bytes as f64 / (1024.0 * 1024.0) / wall.as_secs_f64();
    let messages_per_s = chunk_sizes.len() as f64 / wall.as_secs_f64();
    println!(
        "PTY_AB scenario=daemon_pipe bytes={bytes} expected_file_bytes={expected_file_bytes} wall_ms={} mib_s={mib_s:.2} messages={} messages_per_s={messages_per_s:.2} chunk_min={chunk_min} chunk_median={chunk_median:.1} chunk_max={chunk_max} pending_budget_bytes<={PENDING_OUTPUT_BUDGET_BYTES} seq_reordered={seq_reordered} teardown_ms={} clean={}",
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
        live_bytes > LEGACY_RING_CAPACITY,
        "live capture was only {live_bytes} bytes, need more than a screen window"
    );
    std::thread::sleep(Duration::from_millis(500));
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
            SessionEvent::Recovered { .. }
            | SessionEvent::Exit { .. }
            | SessionEvent::JournalDegraded
            | SessionEvent::OutputGap { .. }
            | SessionEvent::Snapshot { .. } => {}
        }
    }
    assert!(
        bytes > LEGACY_RING_CAPACITY,
        "journal replay was only {bytes} bytes, floor is {LEGACY_RING_CAPACITY}"
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
    let observed = Arc::new(Mutex::new(Vec::<(u64, String)>::new()));
    let observed_for_handler = Arc::clone(&observed);
    let snapshot_count = Arc::new(AtomicU64::new(0));
    let snapshot_count_for_handler = Arc::clone(&snapshot_count);
    let exit_seen = Arc::new(AtomicBool::new(false));
    let exit_seen_for_handler = Arc::clone(&exit_seen);
    let handler: EventHandler = Arc::new(move |envelope| match envelope.event {
        SessionEvent::Output { seq, data } => {
            observed_for_handler.lock().unwrap().push((seq, data));
        }
        // Snapshot coalescing for this viewer legitimately removes unsent
        // Output frames from the live view; the journal stays complete.
        SessionEvent::Snapshot { .. } => {
            snapshot_count_for_handler.fetch_add(1, Ordering::Release);
        }
        SessionEvent::Exit { .. } => {
            exit_seen_for_handler.store(true, Ordering::Release);
        }
        _ => {}
    });
    client
        .session_attach(&session.id, None, handler)
        .expect("attach");
    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        let live_bytes: usize = observed
            .lock()
            .unwrap()
            .iter()
            .map(|(_, data)| data.len())
            .sum();
        if live_bytes >= expected_file_bytes {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let live_bytes = observed
        .lock()
        .unwrap()
        .iter()
        .map(|(_, data)| data.len())
        .sum::<usize>();
    assert!(
        live_bytes >= expected_file_bytes,
        "flood truncated: {live_bytes} < {expected_file_bytes}"
    );
    // WHAT THIS TEST VERIFIES, stated plainly: durability of the COMMITTED
    // journal stream against an abrupt daemon kill, plus the honesty of the
    // flags a reopened transcript carries (assertions below). It is NOT a
    // graceful-shutdown test; the kill stays.
    //
    // The blind 800 ms sleep this replaces was the measured hole behind the
    // adversarial finding: the live view bypasses the journal, so "the client
    // saw everything" says nothing about the writer; on a 4-vCPU runner the
    // writer was still 1.9 MiB behind when TerminateProcess took the bounded
    // queue with it, and the old test then asserted a completeness nobody
    // had checked. The deterministic sync below makes the completeness claim
    // checkable instead of lucky.
    //
    // Three gates, in order:
    // 1. Exit delivered to the client (process end observed);
    // 2. a quiescence window with no new live bytes. Exit can be emitted
    //    from the process-exited path while ConPTY is still draining
    //    (EXIT_DRAIN = 200 ms), and drain frames are ordinary journal
    //    appends, so the window must outlive the drain;
    // 3. committed == accepted, read from the Status journal stats: after
    //    EOF nothing new is accepted, so equality means the queue is empty
    //    and the kill cannot lose an accepted frame.
    let exit_deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < exit_deadline && !exit_seen.load(Ordering::Acquire) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        exit_seen.load(Ordering::Acquire),
        "flood child never EOFed; the pre-kill durability claim cannot be made"
    );
    const QUIESCE: Duration = Duration::from_millis(600);
    let live_byte_count = || {
        observed
            .lock()
            .unwrap()
            .iter()
            .map(|(_, data)| data.len())
            .sum::<usize>()
    };
    let mut last_bytes = live_byte_count();
    let mut last_activity = Instant::now();
    let stats_before_kill = loop {
        assert!(
            last_activity.elapsed() < Duration::from_secs(60),
            "live output never went quiet; cannot distinguish drain from a stuck stream"
        );
        std::thread::sleep(Duration::from_millis(20));
        let bytes = live_byte_count();
        if bytes != last_bytes {
            last_bytes = bytes;
            last_activity = Instant::now();
            continue;
        }
        if last_activity.elapsed() < QUIESCE {
            continue;
        }
        if let Ok(body) = client.status() {
            if let Some(stats) = body.journal_stats {
                if stats.committed_frames == stats.accepted_frames {
                    // Re-check quiescence: a frame accepted between the
                    // stats read and now means the drain was still live.
                    if live_byte_count() == last_bytes {
                        break stats;
                    }
                    last_bytes = live_byte_count();
                    last_activity = Instant::now();
                }
            }
        }
    };
    assert!(
        stats_before_kill.accepted_frames > 0,
        "no output frame was ever accepted; the stats are not measuring this session"
    );
    let failed_before_kill = stats_before_kill.failed_frames;
    let live_events = observed.lock().unwrap().clone();
    let live_bytes: usize = live_events.iter().map(|(_, data)| data.len()).sum();
    let live_frames = live_events.len();
    drop(client);
    harness.restart();

    let live_seqs: Vec<u64> = live_events.iter().map(|(seq, _)| *seq).collect();
    let live_raw: String = live_events.iter().map(|(_, data)| data.as_str()).collect();
    let live_payload = normalize_journal_growth_output(&live_raw);
    let expected_first_seq = *live_seqs.first().expect("live output sequence");
    let expected_last_seq = *live_seqs.last().expect("live output sequence");

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
    struct ReplayCapture {
        events: Vec<(u64, String)>,
        recovered: bool,
        exited: bool,
        truncated: bool,
    }
    let replayed = Arc::new(Mutex::new(ReplayCapture {
        events: Vec::new(),
        recovered: false,
        exited: false,
        truncated: false,
    }));
    let replayed_for_handler = Arc::clone(&replayed);
    let handler: EventHandler = Arc::new(move |envelope| {
        let mut replayed = replayed_for_handler.lock().unwrap();
        match envelope.event {
            SessionEvent::Output { seq, data } => {
                replayed.events.push((seq, data));
            }
            // The truncation flag IS part of the recovered contract: it is
            // the only declared-loss signal the transcript carries, so the
            // handler must capture it, not just the fact of recovery.
            SessionEvent::Recovered { truncated } => {
                replayed.recovered = true;
                replayed.truncated = truncated;
            }
            SessionEvent::Exit { .. } => replayed.exited = true,
            SessionEvent::JournalDegraded => {}
            SessionEvent::OutputGap { .. } => {}
            SessionEvent::Snapshot { .. } => {}
        }
    });
    client
        .session_attach(&session.id, None, handler)
        .expect("replay");
    const REPLAY_TIMEOUT_SECONDS: u64 = 30;
    let deadline = Instant::now() + Duration::from_secs(REPLAY_TIMEOUT_SECONDS);
    let mut replay_complete = false;
    while Instant::now() < deadline {
        let snap = replayed.lock().unwrap();
        if snap.recovered || snap.exited {
            replay_complete = true;
            break;
        }
        drop(snap);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        replay_complete,
        "replay did not complete within {REPLAY_TIMEOUT_SECONDS}s"
    );
    let (replay_events, recovered, exited, replay_truncated) = {
        let snap = replayed.lock().unwrap();
        (
            snap.events.clone(),
            snap.recovered,
            snap.exited,
            snap.truncated,
        )
    };
    let replay_bytes: usize = replay_events.iter().map(|(_, data)| data.len()).sum();
    let replay_seqs: Vec<u64> = replay_events.iter().map(|(seq, _)| *seq).collect();
    let replay_raw: String = replay_events
        .iter()
        .map(|(_, data)| data.as_str())
        .collect();
    let replay_payload = normalize_journal_growth_output(&replay_raw);
    println!(
        "JOURNAL_GROWTH replay_bytes={replay_bytes} frames={} recovered={recovered} exited={exited} truncated={replay_truncated} failed_before_kill={failed_before_kill} live_bytes={live_bytes}",
        replay_events.len()
    );
    // THE CROSS nothing verified before: measured missing bytes against the
    // honest flags. The pre-kill sync makes both branches decidable.
    //
    // failed_frames == 0 at kill time: the journal never dropped a frame
    // knowingly, and the sync proved everything accepted was committed, so
    // the replay must cover the whole flood and match the live capture
    // sequence for sequence — and truncated must stay DOWN, because
    // claiming a loss nobody observed is the wolf that teaches people to
    // ignore the flag.
    //
    // failed_frames > 0: the journal dropped frames knowing it, so the
    // reopened transcript MUST declare truncation. Short coverage is then
    // a declared loss, which is the honest product, not a test failure.
    if failed_before_kill == 0 {
        assert!(
            replay_bytes >= expected_file_bytes,
            "journal replay lost payload bytes with no observed failure: replay={replay_bytes} expected={expected_file_bytes} missing={}",
            expected_file_bytes.saturating_sub(replay_bytes)
        );
        assert!(
            !replay_truncated,
            "no loss was observed before the kill but the reopened transcript claims truncation"
        );
        if snapshot_count.load(Ordering::Acquire) == 0 {
            assert_eq!(
                live_payload,
                replay_payload,
                "normalized journal replay differs: live_bytes={} replay_bytes={} first_difference={:?}",
                live_payload.len(),
                replay_payload.len(),
                live_payload
                    .as_bytes()
                    .iter()
                    .zip(replay_payload.as_bytes())
                    .position(|(live, replay)| live != replay)
                    .or_else(|| {
                        (live_payload.len() != replay_payload.len())
                            .then_some(live_payload.len().min(replay_payload.len()))
                    })
            );
        } else {
            // The live view was resynchronised with snapshots: every frame the
            // view DID receive must match the journal record for that sequence;
            // journal completeness is asserted above.
            let journal_by_seq: std::collections::HashMap<u64, String> =
                replay_events.iter().cloned().collect();
            for (seq, data) in &live_events {
                assert_eq!(
                    journal_by_seq.get(seq).map(String::as_str),
                    Some(data.as_str()),
                    "live frame {seq} differs from the journal record"
                );
            }
            println!(
                "JOURNAL_GROWTH snapshot_coalescing={} live_frames={} of journal_frames={}",
                snapshot_count.load(Ordering::Acquire),
                live_events.len(),
                replay_events.len()
            );
        }
        assert_eq!(
            replay_seqs.first(),
            Some(&expected_first_seq),
            "journal replay start sequence changed"
        );
        assert_eq!(
            replay_seqs.last(),
            Some(&expected_last_seq),
            "journal replay end sequence changed"
        );
        assert_eq!(
            replay_seqs, live_seqs,
            "journal replay sequence coverage differs from the live capture"
        );
        for pair in live_seqs.windows(2) {
            assert_eq!(
                pair[1],
                pair[0] + 1,
                "journal replay seq gap or duplicate around {} -> {}",
                pair[0],
                pair[1]
            );
        }
    } else {
        println!(
            "JOURNAL_GROWTH declared_loss observed_failures={failed_before_kill} replay_bytes={replay_bytes} expected={expected_file_bytes}"
        );
        assert!(
            replay_truncated,
            "{failed_before_kill} journal frames were dropped knowing it but the reopened transcript does not declare truncation"
        );
    }
    assert!(
        recovered || exited,
        "flood replay must end honestly as Recovered or Exit"
    );
    assert!(
        !(recovered && exited),
        "Recovered and Exit must not both fire"
    );
    assert!(
        replay_bytes > LEGACY_RING_CAPACITY,
        "journal replay {replay_bytes} did not outlive the live capture window"
    );
    let _ = std::fs::remove_file(&file_path);
}

/// The M3.5 acceptance properties under a real ConPTY flood, on ONE session:
/// 1. attach during the flood is snapshot-first, and across attach/detach
///    cycles no output sequence is delivered twice and none is silently
///    skipped (every sequence either arrives or is subsumed by a snapshot);
/// 2. the client's reconstructed screen — snapshot plus subsequent live
///    events — equals a fresh emulator fed the whole byte stream.
#[test]
#[ignore = "spawns a real Windows ConPTY; run locally with --ignored"]
fn attach_during_flood_delivers_every_sequence_once() {
    const LINES: usize = 40_000;
    const PAYLOAD: &str = "DEVBOULE_ATTACH_FLOOD_0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    const DONE: &str = "DEVBOULE_ATTACH_FLOOD_DONE";
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
    let client = harness.client("attach-flood");
    let session = client
        .session_create(None, SessionKind::Terminal, None)
        .expect("create");

    // All events across every attach epoch, in arrival order.
    let received = Arc::new(Mutex::new(Vec::<SessionEvent>::new()));
    let done = Arc::new(AtomicBool::new(false));
    // Every attach epoch gets a fresh handler that BOTH collects events and
    // watches for the completion marker, because attach replaces the
    // subscription for the session.
    let attach_epoch = || {
        let received_for_handler = Arc::clone(&received);
        let done_for_handler = Arc::clone(&done);
        let handler: EventHandler = Arc::new(move |envelope| {
            if event_carries_marker(&envelope.event, DONE) {
                done_for_handler.store(true, Ordering::Release);
            }
            received_for_handler.lock().unwrap().push(envelope.event);
        });
        client
            .session_attach(&session.id, None, handler)
            .expect("attach during flood");
    };

    attach_epoch();
    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline && !done.load(Ordering::Acquire) {
        // Repeatedly detach and reattach mid-flood; each reattach must begin
        // with a snapshot and continue the stream without holes.
        client.session_detach(&session.id).expect("detach");
        std::thread::sleep(Duration::from_millis(30));
        attach_epoch();
        std::thread::sleep(Duration::from_millis(60));
    }
    assert!(
        done.load(Ordering::Acquire),
        "flood did not finish while cycling attachments"
    );
    // Resynchronise one last time so the view covers everything the child
    // produced up to EOF, including any post-DONE drain tail.
    client.session_detach(&session.id).expect("final detach");
    std::thread::sleep(Duration::from_millis(500));
    attach_epoch();
    std::thread::sleep(Duration::from_millis(500));
    client
        .session_detach(&session.id)
        .expect("detach before compare");
    let received = received.lock().unwrap().clone();

    let mut seen = std::collections::HashSet::new();
    let mut covered_to = 0u64;
    let mut screen: Option<Screen> = None;
    for event in &received {
        match event {
            SessionEvent::Snapshot { as_of_seq, .. } => {
                assert!(
                    *as_of_seq >= covered_to,
                    "snapshot boundary {as_of_seq} behind covered {covered_to}"
                );
                covered_to = *as_of_seq;
                let term = screen.get_or_insert_with(|| Screen::new(120, 32));
                apply_snapshot_state(term, event);
            }
            SessionEvent::Output { seq, data } => {
                assert!(seen.insert(*seq), "sequence {seq} was delivered twice");
                assert_eq!(
                    *seq,
                    covered_to + 1,
                    "live stream skipped or reordered at {seq}"
                );
                covered_to = *seq;
                screen
                    .get_or_insert_with(|| Screen::new(120, 32))
                    .process(data.as_bytes());
            }
            SessionEvent::OutputGap {
                from_seq, to_seq, ..
            } => panic!(
                "a slow-viewer replacement must be a snapshot, not a gap {from_seq}..{to_seq}"
            ),
            SessionEvent::Exit { .. } => {}
            SessionEvent::Recovered { .. } | SessionEvent::JournalDegraded => {}
        }
    }

    // The reference: a fresh emulator fed the DURABLE byte stream from the
    // journal. Output published during detached windows is never delivered
    // as live Output frames (the reattach snapshot subsumed it), so the
    // captured Outputs alone are not the whole stream; the journal is.
    let journal =
        devboule_daemon::Journal::open(&harness.paths.journal_file()).expect("open journal");
    let replay = journal.replay(&session.id, 0).expect("journal replay");
    let mut ordered: Vec<(u64, String)> = replay
        .events
        .into_iter()
        .filter_map(|event| match event {
            SessionEvent::Output { seq, data } => Some((seq, data)),
            _ => None,
        })
        .collect();
    ordered.sort_unstable_by_key(|(seq, _)| *seq);
    for pair in ordered.windows(2) {
        assert_eq!(
            pair[1].0,
            pair[0].0 + 1,
            "journal stream has a hole: no sequence may be silently skipped"
        );
    }
    assert_eq!(
        ordered.last().map(|(seq, _)| *seq),
        Some(covered_to),
        "the client view must cover the entire journaled stream"
    );
    let mut reference = Screen::new(120, 32);
    for (_, data) in &ordered {
        reference.process(data.as_bytes());
    }
    let expected_bytes: usize = ordered.iter().map(|(_, data)| data.len()).sum();
    let screen = screen.expect("at least one snapshot must have been delivered");
    assert_eq!(
        screen.snapshot(),
        reference.snapshot(),
        "snapshot + subsequent events differ from a fresh emulator fed the journaled stream"
    );
    println!(
        "ATTACH_FLOOD journal_frames={} bytes={expected_bytes} covered_to={covered_to} unique={} snapshots={}",
        ordered.len(),
        seen.len(),
        received
            .iter()
            .filter(|event| matches!(event, SessionEvent::Snapshot { .. }))
            .count(),
    );
    drop(journal);
    client.session_close(&session.id).expect("close");
    assert!(client.sessions_list().expect("list").is_empty());
}

/// THE acceptance test for this milestone: while one session floods output
/// through the pipe, control traffic (ping here; resize below) on the same
/// connection is answered within a declared bound.
///
/// Bound: 1,000 ms. Healthy RTT on this pipe is single-digit milliseconds
/// (see real_pty_echo_latency_benchmark's ping control). The defect this
/// milestone removes surfaced as `TimedOut("waiting for a daemon reply")`
/// against the client's 30,000 ms RPC timeout. A 1,000 ms bound sits two
/// orders of magnitude above healthy latency and thirty times below the
/// observed failure, so it can only fail when control traffic is actually
/// starved.
#[test]
#[ignore = "spawns a real Windows ConPTY; run locally with --ignored"]
fn control_traffic_is_answered_within_bound_during_flood() {
    const CONTROL_BOUND: Duration = Duration::from_millis(1_000);
    const LINES: usize = 100_000;
    const PAYLOAD: &str = "DEVBOULE_CONTROL_FLOOD_0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    const DONE: &str = "DEVBOULE_CONTROL_FLOOD_DONE";

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
    let client = Arc::new(harness.client("control"));
    let session = client
        .session_create(None, SessionKind::Terminal, None)
        .expect("create");

    let done = Arc::new(AtomicBool::new(false));
    let gaps = Arc::new(AtomicU64::new(0));
    let done_for_handler = Arc::clone(&done);
    let gaps_for_handler = Arc::clone(&gaps);
    let handler: EventHandler = Arc::new(move |envelope| match envelope.event {
        SessionEvent::Output { data, .. } => {
            if data.contains(DONE) {
                done_for_handler.store(true, Ordering::Release);
            }
        }
        SessionEvent::OutputGap { .. } => {
            gaps_for_handler.fetch_add(1, Ordering::Release);
        }
        _ => {}
    });
    client
        .session_attach(&session.id, None, handler)
        .expect("attach");

    // Control traffic on the SAME connection whose writer is busy draining
    // the flood — exactly the path that used to starve. Ping and resize
    // alternate: ping exercises the dispatch turn, resize additionally
    // serialises against emulator parsing under the session state lock.
    let mut rtts: Vec<(Duration, &'static str)> = Vec::new();
    let mut widest = true;
    let deadline = Instant::now() + Duration::from_secs(120);
    while Instant::now() < deadline {
        if done.load(Ordering::Acquire) {
            break;
        }
        let started = Instant::now();
        if widest {
            client.ping().expect("ping during flood");
        } else {
            client
                .session_resize(&session.id, 100, 30)
                .expect("resize during flood");
        }
        let rtt = started.elapsed();
        assert!(
            rtt <= CONTROL_BOUND,
            "{} took {rtt:?} during flood; bound is {CONTROL_BOUND:?} (control traffic starved)",
            if widest { "ping" } else { "resize" },
        );
        rtts.push((rtt, if widest { "ping" } else { "resize" }));
        widest = !widest;
        std::thread::sleep(Duration::from_millis(25));
    }
    let flood_done = done.load(Ordering::Acquire);
    client.session_close(&session.id).expect("close");
    assert!(flood_done, "flood did not finish within 120 s");
    assert_eq!(
        gaps.load(Ordering::Acquire),
        0,
        "live delivery must not declare gaps"
    );
    assert!(
        rtts.len() >= 40,
        "expected a sustained control stream during the flood, got {} requests",
        rtts.len()
    );
    rtts.sort_by_key(|(rtt, _)| *rtt);
    let p95 = rtts[rtts.len() * 95 / 100].0;
    let p50 = rtts[rtts.len() / 2].0;
    let max = rtts.last().expect("at least one control request").0;
    println!(
        "CONTROL_BOUND requests={} p50_ms={:.1} p95_ms={:.1} max_ms={:.1} bound_ms={}",
        rtts.len(),
        p50.as_secs_f64() * 1_000.0,
        p95.as_secs_f64() * 1_000.0,
        max.as_secs_f64() * 1_000.0,
        CONTROL_BOUND.as_millis(),
    );
}
