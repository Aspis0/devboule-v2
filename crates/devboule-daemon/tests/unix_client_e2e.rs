//! End-to-end Unix client: `connect_or_spawn` starts the real daemon binary
//! on a temp runtime dir, hello, ping, `restart_daemon` hands a new instance
//! over, shutdown. The secret store is the file one, rooted in the daemon's
//! own temp dir, so no OS keychain is touched. Unix only: the Windows
//! announcement battery covers the pipe side.

#![cfg(unix)]

use std::path::PathBuf;
use std::time::{Duration, Instant};

use devboule_daemon::{
    connect_or_spawn, test_owner, ClientHello, DaemonState, ExitReason, RuntimePaths,
    ShutdownAnswer,
};

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
    let dir = std::env::temp_dir().join(format!(
        "devboule-uxc-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir(&dir).expect("temp runtime dir");
    (RuntimePaths::from_dir(&dir), TempGuard(dir))
}

struct TempGuard(PathBuf);

impl Drop for TempGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn hello(client: &str) -> ClientHello {
    let owner = test_owner(client).expect("owner");
    ClientHello::m3a(owner, "devboule-uxc-test")
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

#[test]
fn connect_spawn_restart_and_shutdown_end_to_end() {
    let binary = daemon_bin();
    test_env(&binary);
    let (paths, _guard) = temp_paths();

    let client = connect_or_spawn(&paths, hello("uxc-first"), Some(&binary)).expect("spawn");
    wait_for_ready(&paths);
    assert!(client.ping().expect("ping") > 0);
    let first = client.hello().instance_id.clone();

    client.restart_daemon().expect("restart");

    let again = connect_or_spawn(&paths, hello("uxc-second"), Some(&binary)).expect("reconnect");
    assert!(again.ping().expect("ping the new instance") > 0);
    let second = again.hello().instance_id.clone();
    assert_ne!(first, second, "restart handed over a new daemon instance");
    assert!(
        matches!(again.request_shutdown(), Ok(ShutdownAnswer::Accepted)),
        "shutdown accepted"
    );
    drop(again);
    drop(client);

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
}
