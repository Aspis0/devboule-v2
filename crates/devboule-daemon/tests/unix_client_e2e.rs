//! End-to-end Unix client: `connect_or_spawn` starts the real daemon binary
//! on a temp runtime dir, hello, ping, the guarded kill the restart falls back
//! to, `restart_daemon` hands a new instance over, shutdown. The secret store
//! is the file one, rooted in the daemon's own temp dir, so no OS keychain is
//! touched. Unix only: the Windows announcement battery covers the pipe side.

#![cfg(unix)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use devboule_daemon::{
    connect_or_spawn, connect_within, kill_verified_daemon, test_owner, DaemonState, ExitReason,
    RuntimePaths, ShutdownAnswer,
};
use devboule_protocol::ClientHello;

/// This test binary's only test shares the process with nothing else, so
/// process-global selections cannot race a neighbor: the file secret store
/// (no OS keychain is touched) and the daemon binary the restart spawns.
fn test_env(binary: &PathBuf) {
    std::env::set_var("DEVBOULE_SECRET_STORE", "file");
    std::env::set_var("DEVBOULE_DAEMON", binary);
}

fn daemon_bin() -> PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_devboule-daemon") {
        return PathBuf::from(path);
    }
    if let Some(path) = option_env!("CARGO_BIN_EXE_devboule-daemon") {
        return PathBuf::from(path);
    }
    panic!(
        "CARGO_BIN_EXE_devboule-daemon was not provided by Cargo; refusing to guess a target directory binary"
    );
}

fn temp_paths() -> (RuntimePaths, TempGuard) {
    let dir = devboule_daemon::test_dirs::test_temp_dir("devboule-uxc");
    let paths = RuntimePaths::from_dir(&dir);
    (paths.clone(), TempGuard { paths })
}

const GUARD_CONNECT: Duration = Duration::from_millis(500);
const GUARD_WAIT: Duration = Duration::from_secs(15);
const GUARD_POLL: Duration = Duration::from_millis(100);
/// Above the sum of the restart's own bounded phases — the shutdown RPC, the
/// graceful stop, the kill and the replacement, four waits of 30 s each.
const RESTART_PHASE: Duration = Duration::from_secs(150);

/// The runtime dir and, on drop — including on a failing assertion's unwind —
/// the daemon that may still be running on it: ask it to stop, kill it through
/// the same verified path the restart uses if it will not, and remove the
/// directory last. Without this a panicking run leaves a detached daemon and
/// its socket behind on the runner.
struct TempGuard {
    paths: RuntimePaths,
}

impl Drop for TempGuard {
    fn drop(&mut self) {
        // Nothing here may panic: this runs while a failed assertion unwinds.
        if let Ok(owner) = test_owner("uxc-guard") {
            let hello = ClientHello::m3a(owner, "devboule-uxc-test");
            if let Ok(client) = connect_within(&self.paths, hello, GUARD_CONNECT) {
                let _ = client.request_shutdown();
            }
        }
        let deadline = Instant::now() + GUARD_WAIT;
        loop {
            let pid = match DaemonState::read(&self.paths.lock_file) {
                DaemonState::Live(record) => record.pid,
                // Absent, stale or stopped all mean nothing is asked to die.
                _ => break,
            };
            if !process_alive(pid) {
                break;
            }
            if Instant::now() >= deadline {
                let _ = kill_verified_daemon(&self.paths, pid);
                break;
            }
            std::thread::sleep(GUARD_POLL);
        }
        let _ = std::fs::remove_dir_all(&self.paths.dir);
    }
}

fn hello(client: &str) -> ClientHello {
    let owner = test_owner(client).expect("owner");
    ClientHello::m3a(owner, "devboule-uxc-test")
}

/// Whether the pid still exists in any state, a zombie included: signal 0
/// asks the kernel nothing else.
fn process_alive(pid: u32) -> bool {
    // SAFETY: no signal is delivered; the call only asks the kernel.
    unsafe { libc::kill(pid as libc::pid_t, 0) == 0 }
}

/// The pid of the daemon the record currently names.
fn live_pid(paths: &RuntimePaths) -> u32 {
    match DaemonState::read(&paths.lock_file) {
        DaemonState::Live(record) => record.pid,
        other => panic!("the daemon must be live, record read as {other:?}"),
    }
}

/// `ps` answers `Z…` while nobody has reaped the child and nothing once its
/// parent has; the app's reaper answers on its own thread, so the check is
/// bounded rather than instant.
fn assert_no_zombie(pid: u32, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let output = std::process::Command::new("/bin/ps")
            .args(["-o", "stat=", "-p", &pid.to_string()])
            .output()
            .expect("ps runs");
        let stat = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !stat.starts_with('Z') {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{what} (pid {pid}) was left a zombie: ps stat {stat:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn wait_for_ready(paths: &RuntimePaths) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let DaemonState::Live(record) = DaemonState::read(&paths.lock_file) {
            if record.ready {
                return;
            }
        }
        assert!(
            Instant::now() < deadline,
            "the spawned daemon never published a ready record"
        );
        std::thread::sleep(Duration::from_millis(200));
    }
}

fn wait_gone(pid: u32, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while process_alive(pid) {
        assert!(
            Instant::now() < deadline,
            "{what} (pid {pid}) is still alive"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Run one phase under its own ceiling: the phase's own waits are bounded,
/// and this names a wedged call instead of leaving the failure to the step's
/// timeout. The thread is detached on purpose — joining it would wait for the
/// very call the ceiling rejects.
fn within<T: Send + 'static>(
    phase: &str,
    budget: Duration,
    run: impl FnOnce() -> T + Send + 'static,
) -> T {
    let (sender, receiver) = std::sync::mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name(phase.to_string())
        .spawn(move || {
            let _ = sender.send(run());
        })
        .expect("the phase thread starts");
    receiver
        .recv_timeout(budget)
        .unwrap_or_else(|_| panic!("{phase} did not finish within {budget:?}"))
}

#[test]
fn connect_spawn_restart_and_shutdown_end_to_end() {
    let binary = daemon_bin();
    test_env(&binary);
    let (paths, _guard) = temp_paths();

    let client = connect_or_spawn(&paths, hello("uxc-first"), Some(&binary)).expect("spawn");
    wait_for_ready(&paths);
    assert!(client.ping().expect("ping") > 0);
    let first = client.hello().instance_id.clone();
    let first_pid = live_pid(&paths);

    // The kill path signals only through the held connection: a pid that
    // connection does not report is refused, and the daemon answers after.
    assert!(
        kill_verified_daemon(&paths, first_pid + 1).is_err(),
        "a pid the held connection does not confirm must not be signalled"
    );
    assert!(
        client.ping().expect("ping after the refused kill") > 0,
        "the refused kill left the daemon running"
    );

    within("restart", RESTART_PHASE, move || client.restart_daemon()).expect("restart");
    assert_no_zombie(first_pid, "the daemon that exited for the restart");

    let again = connect_or_spawn(&paths, hello("uxc-second"), Some(&binary)).expect("reconnect");
    assert!(again.ping().expect("ping the new instance") > 0);
    let second = again.hello().instance_id.clone();
    assert_ne!(first, second, "restart handed over a new daemon instance");
    let second_pid = live_pid(&paths);

    // The confirmed pid is the one the held connection reports: the signal
    // lands, the daemon goes, and its child is reaped rather than left a
    // zombie. The stale socket and record are what the next start cleans.
    kill_verified_daemon(&paths, second_pid).expect("guarded kill");
    wait_gone(second_pid, "the guarded kill");
    assert_no_zombie(second_pid, "the daemon killed through the guard");

    let third = connect_or_spawn(&paths, hello("uxc-third"), Some(&binary)).expect("respawn");
    assert!(third.ping().expect("ping the respawned instance") > 0);
    let third_pid = live_pid(&paths);
    assert!(matches!(
        third.request_shutdown(),
        Ok(ShutdownAnswer::Accepted)
    ));
    drop(third);
    drop(again);

    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if !paths.socket_path.exists() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the socket was not removed on shutdown"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    // The socket goes before the goodbye is written; the goodbye lands
    // before the process exits, so the exit is what makes it readable.
    wait_gone(third_pid, "the daemon that shut down");
    assert!(
        matches!(
            DaemonState::read(&paths.lock_file),
            DaemonState::Stopped(_, ExitReason::Requested)
        ),
        "goodbye recorded"
    );
    // A shut-down daemon stays down: the socket is gone, so nothing answers.
    assert!(
        devboule_daemon::connect(&paths, hello("uxc-gone")).is_err(),
        "nothing listening after shutdown"
    );
    assert_no_zombie(third_pid, "the daemon that shut down");
}
