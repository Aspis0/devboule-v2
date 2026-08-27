//! Real process + real named-pipe tests. Ignored by default, same pattern as
//! the ConPTY tests in `session_tests.rs`.
//!
//! Run: `cargo test -p devboule-daemon -- --ignored --nocapture`

#![cfg(windows)]

use std::fs::OpenOptions;
use std::path::PathBuf;
use std::process::Child;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use devboule_daemon::{
    connect, connect_or_spawn, current_user_sid, dacl_is_current_user_only, spawn_daemon,
    DaemonClient, RuntimePaths, IDLE_SHUTDOWN_GRACE,
};
use devboule_protocol::{ClientHello, ClientMessage, ErrorCode, OwnerId, PROTOCOL_VERSION};

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
        "devboule m3a {}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("runtime dir with spaces");
    assert!(
        dir.to_string_lossy().contains(' '),
        "test dir must contain a space: {}",
        dir.display()
    );
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

    fn wait_until_exit(&mut self) -> std::process::ExitStatus {
        let deadline = Instant::now() + IDLE_SHUTDOWN_GRACE + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let Some(child) = &mut self.child {
                if let Some(status) = child.try_wait().expect("wait for daemon") {
                    return status;
                }
            } else {
                panic!("daemon child handle was already taken");
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        panic!("daemon did not exit after becoming idle");
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        drop(self.child.take());
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// Own every process this integration test starts. Dropping a `Child` only
/// drops its handle on Windows; it does not stop the process. This guard makes
/// cleanup unconditional, including when an assertion panics.
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

fn connect_when_ready(
    paths: &RuntimePaths,
    hello: ClientHello,
    process: &mut ChildGuard,
) -> DaemonClient {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match connect(paths, hello.clone()) {
            Ok(client) => return client,
            Err(error) => {
                if let Ok(Some(status)) = process.try_wait() {
                    panic!("daemon exited before listen: {status} ({error})");
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("daemon did not accept within 5s at {}", paths.dir.display());
}

fn test_hello(client: &str) -> ClientHello {
    let owner = OwnerId::new(
        current_user_sid().expect("sid"),
        format!("app-{client}-{}", std::process::id()),
    )
    .expect("owner");
    ClientHello::m3a(owner, "devboule-test")
}

#[test]
#[ignore = "spawns a real daemon and named pipe; run locally with --ignored"]
fn daemon_not_running_then_spawn_connects() {
    let (paths, dir) = unique_paths();
    let mut process = ChildGuard::spawn(&paths);
    let hello = test_hello("spawn");
    let client = connect_when_ready(&paths, hello, &mut process);
    client.ping().expect("ping");
    let status = client.status().expect("status");
    assert_eq!(status.protocol_version, PROTOCOL_VERSION);
    assert!(status.pid > 0);
    let _ = client.shutdown();
    drop(client);
    drop(process);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "spawns a real daemon and named pipe; run locally with --ignored"]
fn daemon_already_running_does_not_start_a_second() {
    let harness = Harness::spawn();
    let first_pid = harness.client("a").status().expect("status").pid;
    let mut second = ChildGuard::spawn(&harness.paths);
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut exited = None;
    while Instant::now() < deadline {
        if let Ok(Some(status)) = second.try_wait() {
            exited = Some(status);
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let status = match exited {
        Some(status) => status,
        None => {
            panic!("second daemon must exit, not stay as a rival");
        }
    };
    assert!(
        status.success(),
        "losing daemon must exit 0 (already running), got {status}"
    );
    let client = harness.client("b");
    assert_eq!(client.status().expect("still up").pid, first_pid);
}

#[test]
#[ignore = "spawns a real daemon and named pipe; run locally with --ignored"]
fn two_clients_connect_at_once() {
    let harness = Harness::spawn();
    let a = harness.client("one");
    let b = harness.client("two");
    a.ping().expect("a ping");
    b.ping().expect("b ping");
    let status = a.status().expect("status");
    assert!(status.clients >= 2);
}

#[test]
#[ignore = "spawns a real daemon and named pipe; run locally with --ignored"]
fn version_mismatch_is_a_clear_error_not_a_hang() {
    let harness = Harness::spawn();
    let mut hello = test_hello("mismatch");
    hello.protocol_version = 99;
    hello.min_protocol_version = 99;
    let started = Instant::now();
    let error = match connect(&harness.paths, hello) {
        Err(error) => error,
        Ok(_) => panic!("must fail handshake"),
    };
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "mismatch hung: {:?}",
        started.elapsed()
    );
    let message = error.to_string();
    assert!(
        message.contains("protocol mismatch")
            || message.contains("older")
            || message.contains("newer"),
        "{message}"
    );
    match error {
        devboule_daemon::DaemonError::Handshake(wire) => {
            assert_eq!(wire.code, ErrorCode::ProtocolVersionMismatch);
            assert!(
                wire.message.contains("Update the daemon")
                    || wire.message.contains("Update the app")
            );
        }
        other => panic!("expected Handshake, got {other:?}"),
    }
}

#[test]
#[ignore = "spawns a real daemon and named pipe; run locally with --ignored"]
fn silent_pipe_client_is_closed_after_handshake_deadline() {
    let harness = Harness::spawn();
    let mute = OpenOptions::new()
        .read(true)
        .write(true)
        .open(&harness.paths.pipe_name)
        .expect("open mute pipe client");
    std::thread::sleep(Duration::from_millis(2500));

    let started = Instant::now();
    harness
        .client("after-mute")
        .ping()
        .expect("daemon still serves clients");
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "a silent handshake stalled the daemon: {:?}",
        started.elapsed()
    );
    drop(mute);
}

#[test]
#[ignore = "spawns a real daemon and named pipe; run locally with --ignored"]
fn daemon_killed_while_connected_reports_without_hang() {
    let mut harness = Harness::spawn();
    let client = harness.client("kill");
    client.ping().expect("before kill");
    let mut child = harness.child.take().expect("child");
    child.child.kill().expect("kill");
    child.child.wait().expect("reap");
    let started = Instant::now();
    let result = client.ping();
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "ping hung after daemon kill: {:?}",
        started.elapsed()
    );
    assert!(result.is_err(), "ping must fail after the daemon is killed");
}

#[test]
#[ignore = "spawns a real daemon and named pipe; run locally with --ignored"]
fn stale_lock_file_from_a_crashed_daemon_is_recovered() {
    let (paths, dir) = unique_paths();
    paths.ensure_dir().expect("dir");
    std::fs::write(
        &paths.lock_file,
        "pid=1\ninstance=dead\npipe=\\\\.\\pipe\\stale\n",
    )
    .expect("stale lock");
    let mut process = ChildGuard::spawn(&paths);
    let hello = test_hello("stale");
    let client = connect_when_ready(&paths, hello, &mut process);
    client.ping().expect("ping after stale lock");
    let _ = client.shutdown();
    drop(client);
    drop(process);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
#[ignore = "spawns a real daemon and named pipe; run locally with --ignored"]
fn slow_client_does_not_stall_other_clients() {
    let harness = Harness::spawn();
    let slow = harness.client("slow");
    let fast = harness.client("fast");
    for index in 0..2_000u64 {
        slow.write_frame(&ClientMessage::Ping { id: index })
            .expect("flood");
    }
    let started = Instant::now();
    fast.ping().expect("fast ping while slow is not reading");
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "fast client stalled behind a slow one: {:?}",
        started.elapsed()
    );
}

#[test]
#[ignore = "spawns a real daemon and named pipe; run locally with --ignored"]
fn pipe_dacl_is_current_user_only() {
    let harness = Harness::spawn();
    let client = harness.client("dacl");
    let sid = current_user_sid().expect("sid");
    let sddl = client.pipe_dacl_sddl().expect("dacl");
    assert!(
        dacl_is_current_user_only(&sddl, &sid),
        "pipe DACL is not restricted to the current user.\nSID={sid}\nSDDL={sddl}"
    );
    assert!(
        sddl.contains(&sid),
        "DACL does not name the current user SID.\nSID={sid}\nSDDL={sddl}"
    );
}

#[test]
#[ignore = "spawns a real daemon and named pipe; run locally with --ignored"]
fn connect_or_spawn_reuses_a_live_daemon() {
    let harness = Harness::spawn();
    let first_pid = harness.client("live").status().expect("status").pid;
    let reused =
        connect_or_spawn(&harness.paths, test_hello("reuse"), Some(&daemon_bin())).expect("reuse");
    assert_eq!(reused.status().expect("status").pid, first_pid);
}

#[test]
#[ignore = "spawns a real daemon and named pipe; run locally with --ignored"]
fn idle_daemon_exits_after_last_client_disconnects() {
    let mut harness = Harness::spawn();
    let client = harness.client("idle");
    drop(client);
    assert!(
        harness.wait_until_exit().success(),
        "idle daemon must exit successfully"
    );
}

#[test]
#[ignore = "spawns a real daemon and named pipe; run locally with --ignored"]
fn connected_client_keeps_daemon_alive_past_idle_grace() {
    let harness = Harness::spawn();
    let client = harness.client("connected");
    std::thread::sleep(IDLE_SHUTDOWN_GRACE + Duration::from_millis(200));
    client.ping().expect("connected client keeps daemon alive");
}

#[test]
#[ignore = "spawns a real daemon and named pipe; run locally with --ignored"]
fn reconnect_inside_idle_grace_keeps_daemon_alive() {
    let harness = Harness::spawn();
    let client = harness.client("blink");
    drop(client);
    std::thread::sleep(IDLE_SHUTDOWN_GRACE / 2);
    let reconnected = harness.client("blink-reconnect");
    reconnected
        .ping()
        .expect("reconnect inside grace must succeed");
    std::thread::sleep(IDLE_SHUTDOWN_GRACE + Duration::from_millis(200));
    reconnected
        .ping()
        .expect("reconnected client keeps daemon alive");
}
