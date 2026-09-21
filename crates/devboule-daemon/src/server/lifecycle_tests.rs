//! Tests for the daemon's own lifecycle: what a probe may touch, and what a
//! daemon must record on its way out. The probe test asserts the daemon's
//! shutdown flag rather than its client count: a count read here would also
//! pass with the increment in the wrong branch, which is exactly the bug
//! (`server/state.rs`, `client_connected`).

use super::*;
use crate::daemon_record::{DaemonRecord, DaemonState, Heartbeat};
use std::path::PathBuf;
use std::sync::atomic::AtomicU64;

fn unique_paths() -> (RuntimePaths, DirGuard) {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let dir = std::env::temp_dir().join(format!(
        "devboule lifecycle {}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let paths = RuntimePaths::from_dir(dir.clone());
    (paths, DirGuard(dir))
}

struct DirGuard(PathBuf);

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A daemon bound to a private pipe in a private runtime folder, with a record
/// on disk that says it is listening. The record plus the heartbeat are what a
/// probe reads; nothing here is a connection.
struct BoundDaemon {
    paths: RuntimePaths,
    state: Arc<ServerState>,
    record: PathBuf,
    shutdown: transport::ListenerShutdown,
    accept: Option<JoinHandle<()>>,
}

impl BoundDaemon {
    fn start(paths: RuntimePaths) -> Self {
        let mut record =
            DaemonRecord::starting(std::process::id(), "lifecycle-instance", &paths.pipe_name);
        record.listening();
        std::fs::write(&paths.lock_file, record.body()).expect("record");

        let state = ServerState::with_paths("lifecycle-instance".to_string(), paths.clone())
            .expect("state");
        let (listener, shutdown) =
            transport::bind(&paths, Arc::clone(&state.stop)).expect("bind listener");
        let accept_state = Arc::clone(&state);
        let accept = std::thread::Builder::new()
            .name("lifecycle-test-accept".into())
            .spawn(move || accept_loop(listener, accept_state))
            .ok();
        Self {
            paths: paths.clone(),
            state,
            record: paths.lock_file.clone(),
            shutdown,
            accept,
        }
    }

    fn client(&self) -> crate::client::DaemonClient {
        crate::client::connect(&self.paths, test_hello()).expect("connect")
    }
}

impl Drop for BoundDaemon {
    fn drop(&mut self) {
        self.shutdown.shutdown();
        self.state.stop.store(true, Ordering::SeqCst);
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
    }
}

fn test_hello() -> devboule_protocol::ClientHello {
    let owner = devboule_protocol::OwnerId::new(
        crate::security::current_user_sid().expect("sid"),
        format!("app-lifecycle-test-{}", std::process::id()),
    )
    .expect("owner");
    devboule_protocol::ClientHello::m3a(owner, "devboule-test")
}

/// The probe that asks whether a daemon is alive must not be a connection.
///
/// Connecting is the only way to ask today, and the accept path answers it by
/// taking a client slot and bumping the idle generation — so a readiness probe
/// re-arms the shutdown it is about to observe. This asserts the consequence:
/// with the probe running for longer than the grace, the daemon still begins
/// shutting down. A probe that connected would keep re-arming the timer, the
/// flag would still be false at the deadline, and this goes red.
#[cfg(windows)]
#[test]
fn a_liveness_probe_that_reads_the_record_does_not_postpone_the_idle_exit() {
    let (paths, _guard) = unique_paths();
    paths.ensure_dir().expect("runtime dir");
    let daemon = BoundDaemon::start(paths);
    let _heartbeat =
        Heartbeat::with_interval(&daemon.record, Duration::from_millis(20)).expect("heartbeat");

    // Arm the idle timer the way a disconnect does: one client, then gone.
    let client = daemon.client();
    drop(client);

    let probe_deadline = Instant::now() + IDLE_SHUTDOWN_GRACE * 4;
    while !daemon.state.stop.load(Ordering::SeqCst) && Instant::now() < probe_deadline {
        let _ = DaemonState::read(&daemon.record);
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        daemon.state.stop.load(Ordering::SeqCst),
        "the probe postponed the idle exit it was measuring"
    );
}

/// The heartbeat is a writer in the daemon's own process, so it must not count
/// as a client or as a session. If it did, the daemon would hold itself alive
/// and the idle exit would never fire again.
#[cfg(windows)]
#[test]
fn the_heartbeat_is_not_a_client_and_does_not_hold_the_daemon_up() {
    let (paths, _guard) = unique_paths();
    paths.ensure_dir().expect("runtime dir");
    let daemon = BoundDaemon::start(paths);
    let heartbeat =
        Heartbeat::with_interval(&daemon.record, Duration::from_millis(5)).expect("heartbeat");

    std::thread::sleep(Duration::from_millis(80));

    assert_eq!(
        daemon.state.live_client_count(),
        0,
        "the heartbeat took a client slot"
    );
    assert_eq!(daemon.state.live_session_count(), 0);
    assert!(
        !daemon.state.stop.load(Ordering::SeqCst),
        "the heartbeat is not a client and must not decide anything"
    );
    assert!(
        DaemonState::read(&daemon.record).is_live(),
        "the heartbeat still has to do its own job"
    );
    drop(heartbeat);
}
