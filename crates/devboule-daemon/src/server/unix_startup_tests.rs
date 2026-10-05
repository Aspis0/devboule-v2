//! Unix startup smoke: the real server sequence on a temp runtime dir —
//! bind, hello, one RPC, a refused second daemon, graceful shutdown, and
//! the artifacts it must leave behind. Unix only; Windows never runs this.

use std::io::Read;
use std::time::{Duration, Instant};

use crate::paths::RuntimePaths;
use crate::DaemonState;

/// Unique temp runtime dir with a SHORT name: the socket path must fit the
/// 104-byte `sun_path` budget under CI's `$TMPDIR`, so no long prefix.
struct TempDir(std::path::PathBuf);

impl TempDir {
    fn fresh() -> Self {
        Self(crate::test_dirs::test_temp_dir("uxs"))
    }

    fn paths(&self) -> RuntimePaths {
        RuntimePaths::from_dir(&self.0)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn mode_of(path: &std::path::Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .ok()
        .map(|metadata| metadata.permissions().mode() & 0o777)
}

fn wait_for_ready(paths: &RuntimePaths) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        if let DaemonState::Live(record) = DaemonState::read(&paths.lock_file) {
            if record.ready {
                return;
            }
        }
        assert!(
            Instant::now() < deadline,
            "the daemon never published a ready record"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn unix_server_starts_serves_and_cleans_up() {
    let temp = TempDir::fresh();
    let paths = temp.paths();

    // Phase 1: the real sequence, in the background.
    let serving = paths.clone();
    let server = std::thread::spawn(move || super::lifecycle::run_with_paths(serving));

    // Phase 2: readiness through the record, never a connection: probing
    // by connecting would re-arm the idle timer under observation.
    wait_for_ready(&paths);

    // Phase 3: hello with the kernel-uid owner, then one ping round-trip.
    let owner = crate::test_owner("smoke").expect("owner");
    let hello = devboule_protocol::ClientHello::m3a(owner, "devboule-smoke");
    let client = crate::connect(&paths, hello).expect("connect");
    assert!(client.ping().expect("ping") > 0);

    // Phase 4: the journal files are owner-only (WAL/SHM when present —
    // a fresh open always writes schema, so the WAL is there).
    let journal = paths.journal_file();
    assert_eq!(mode_of(&journal), Some(0o600), "journal db");
    let wal = journal.with_extension("db-wal");
    assert!(
        mode_of(&wal).is_none() || mode_of(&wal) == Some(0o600),
        "journal WAL"
    );
    let shm = journal.with_extension("db-shm");
    assert!(
        mode_of(&shm).is_none() || mode_of(&shm) == Some(0o600),
        "journal SHM"
    );

    // Phase 5: a second daemon on the same dir is refused, fast and
    // without side effects (the lock fails before any bind).
    let refused = super::lifecycle::run_with_paths(paths.clone());
    assert!(
        matches!(refused, Err(crate::DaemonError::AlreadyRunning)),
        "second daemon refused: {refused:?}"
    );

    // Phase 6: graceful shutdown through RPC, then join.
    assert!(
        matches!(
            client.request_shutdown(),
            Ok(crate::ShutdownAnswer::Accepted)
        ),
        "shutdown accepted"
    );
    let deadline = Instant::now() + Duration::from_secs(60);
    while !server.is_finished() {
        assert!(Instant::now() < deadline, "the server did not stop");
        std::thread::sleep(Duration::from_millis(100));
    }
    server.join().expect("server thread").expect("clean exit");

    // Phase 7: the socket is gone and the lock is free (a fresh acquire
    // succeeds where phase 5 was refused); the record names the exit.
    assert!(
        !paths.socket_path.exists(),
        "owned socket removed on shutdown"
    );
    drop(client);
    let _free = crate::SingleInstanceLock::acquire(&paths).expect("lock released");
    drop(_free);
    assert!(
        matches!(
            DaemonState::read(&paths.lock_file),
            DaemonState::Stopped(_, crate::ExitReason::Requested)
        ),
        "goodbye recorded"
    );

    // Phase 8: a killed daemon's stale socket is cleaned at the next start.
    // No shutdown ran here — the file is just left behind — so the next
    // serve must unlink it by probe and come up anyway.
    {
        let lingering =
            std::os::unix::net::UnixListener::bind(&paths.socket_path).expect("stale fixture");
        drop(lingering);
    }
    assert!(paths.socket_path.exists(), "stale file planted");
    let serving = paths.clone();
    let server = std::thread::spawn(move || super::lifecycle::run_with_paths(serving));
    wait_for_ready(&paths);
    let owner = crate::test_owner("smoke-again").expect("owner");
    let hello = devboule_protocol::ClientHello::m3a(owner, "devboule-smoke");
    let again = crate::connect(&paths, hello).expect("reconnect");
    assert!(again.ping().expect("ping after stale cleanup") > 0);
    assert!(
        matches!(
            again.request_shutdown(),
            Ok(crate::ShutdownAnswer::Accepted)
        ),
        "second shutdown accepted"
    );
    let deadline = Instant::now() + Duration::from_secs(60);
    while !server.is_finished() {
        assert!(Instant::now() < deadline, "the server did not stop");
        std::thread::sleep(Duration::from_millis(100));
    }
    server.join().expect("server thread").expect("clean exit");
    assert!(
        !paths.socket_path.exists(),
        "stale-cleaned socket removed on shutdown"
    );
}

/// The child half of [`unix_sigterm_reaches_the_graceful_shutdown_path`]:
/// the real server, in its own process, until a signal arrives. Ignored in
/// a normal run — the parent test spawns exactly this test as its host —
/// and a manual `--ignored` run without the directory panics instead of
/// serving nothing.
#[test]
#[ignore = "spawned as a child process by the SIGTERM test"]
fn unix_signal_child_host() {
    let directory = std::env::var("DEVBOULE_TEST_SIGNAL_DIR")
        .expect("the SIGTERM test sets DEVBOULE_TEST_SIGNAL_DIR before spawning this host");
    let paths = RuntimePaths::from_dir(std::path::PathBuf::from(directory));
    super::lifecycle::run_with_paths(paths.clone()).expect("SIGTERM ends the run cleanly");
    assert!(
        !paths.socket_path.exists(),
        "the signalled run must remove its socket before exiting"
    );
    assert!(
        matches!(
            DaemonState::read(&paths.lock_file),
            DaemonState::Stopped(_, crate::ExitReason::Requested)
        ),
        "the signalled run must record a requested goodbye"
    );
}

/// SIGTERM reaches the same graceful shutdown the Shutdown RPC sends: the
/// handler is installed by `run_with_paths` itself, so a real signal to a
/// real server process drains the journal, writes the goodbye and unlinks
/// the socket instead of dying on the kernel's default route. The signal
/// always goes to the child spawned here, never to the test runner.
#[test]
fn unix_sigterm_reaches_the_graceful_shutdown_path() {
    let temp = TempDir::fresh();
    let paths = temp.paths();
    let exe = std::env::current_exe().expect("test executable");
    let mut child = std::process::Command::new(exe)
        .args([
            "server::unix_startup_tests::unix_signal_child_host",
            "--exact",
            "--ignored",
        ])
        .env("DEVBOULE_TEST_SIGNAL_DIR", &temp.0)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn the signal child host");

    // Readiness through the record, as above: the record is what a
    // launcher reads, and connecting would only prove the socket.
    wait_for_ready(&paths);

    let pid = child.id() as libc::pid_t;
    assert_eq!(
        unsafe { libc::kill(pid, libc::SIGTERM) },
        0,
        "SIGTERM delivered to the child host"
    );

    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        match child.try_wait().expect("poll the signal child host") {
            Some(status) => break status,
            None => {
                assert!(
                    Instant::now() < deadline,
                    "the child host ignored SIGTERM and kept serving"
                );
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    };
    let mut stderr = String::new();
    let _ = child
        .stderr
        .take()
        .expect("piped stderr")
        .read_to_string(&mut stderr);
    assert!(status.success(), "child host failed: {status:?}\n{stderr}");

    assert!(
        !paths.socket_path.exists(),
        "the signalled run unlinked its socket"
    );
    assert!(
        matches!(
            DaemonState::read(&paths.lock_file),
            DaemonState::Stopped(_, crate::ExitReason::Requested)
        ),
        "the signalled run recorded the goodbye"
    );
}
