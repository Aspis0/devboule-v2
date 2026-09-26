//! ACP proof against the local stub agent.
//!
//! This test is intentionally separate from the known-flaky ignored ConPTY
//! suite. It exercises direct stdio, malformed/partial-safe framing, stderr,
//! CREATE_NO_WINDOW, per-agent Job Object containment, and close teardown.
//!
//! The stub is built by the same `cargo test` invocation
//! (`CARGO_BIN_EXE_devboule-acp-stub`), and a stale one ignores the knobs it
//! does not know **in silence**. A `cargo test --lib` selects no bin target, so
//! before a targeted lib run rebuild it with `cargo build -p devboule-daemon
//! --bin devboule-acp-stub --features test-support` (the lib fixture refuses a
//! stale binary by name, `session_resume_fixture.rs`).

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Barrier, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use devboule_daemon::{
    connect, current_user_sid, spawn_daemon, spawn_daemon_with_env, DaemonClient, EventHandler,
    RuntimePaths, SessionStateHandler,
};
use devboule_protocol::{
    AgentTaskState, AttentionReason, ClientHello, Cursor, DaemonMessage, ErrorCode, FinishArtifact,
    NoticeSeverity, OwnerId, PermissionOutcome, PermissionRequestKind, Persistence,
    PersistenceKind, ResumeResult, SessionEvent, SessionKind, SessionStateSnapshot,
    WorkspaceIsolation,
};
use rusqlite::Connection;
use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows_sys::Win32::System::JobObjects::IsProcessInJob;
use windows_sys::Win32::System::Threading::{
    OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
};

static TEST_LOCK: Mutex<()> = Mutex::new(());

fn lock_tests() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

/// Point the daemons these tests spawn at the **file** secret store, rooted in
/// that daemon's own temp runtime dir, so the identity it writes dies with the
/// directory.
///
/// The daemon binary is a production build: with no `DEVBOULE_SECRET_STORE` it
/// selects the OS credential store and writes a `noise-static-<runtime dir
/// hash>` entry that nothing ever deletes (measured: 66 entries from this file
/// in one suite run, and enough of them make `CredWrite` fail with Windows
/// error 8 -- see `reports/remote-agents/keyring-test-leak-fix-report.md`).
/// Every spawn in this file passes through `daemon_bin()`, so the call lives
/// there.
fn file_secret_store() {
    // Set once, before the first spawn: the tests run in parallel threads, so a
    // process-wide write per call would race them.
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| std::env::set_var("DEVBOULE_SECRET_STORE", "file"));
}

fn daemon_bin() -> PathBuf {
    // Cargo names this env var after the bin verbatim (dashes included). A
    // stale-binary fallback would silently run "the past" and report green —
    // on this machine the app holds devboule-daemon.exe open and a test that
    // cannot find the Cargo-provided binary must fail loudly instead.
    file_secret_store();

    if let Ok(path) = std::env::var("CARGO_BIN_EXE_devboule-daemon") {
        return PathBuf::from(path);
    }
    if let Some(path) = option_env!("CARGO_BIN_EXE_devboule-daemon") {
        return PathBuf::from(path);
    }
    panic!(
        "CARGO_BIN_EXE_devboule-daemon was not provided by Cargo; refusing to \
         guess a target directory binary (a stale one would test the past)"
    );
}

fn stub_bin() -> PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_devboule-acp-stub") {
        return PathBuf::from(path);
    }
    if let Some(path) = option_env!("CARGO_BIN_EXE_devboule-acp-stub") {
        return PathBuf::from(path);
    }
    panic!(
        "CARGO_BIN_EXE_devboule-acp-stub was not provided by Cargo; refusing to \
         guess a target directory binary (a stale one would test the past)"
    );
}

/// A directory no other run can hand back.
///
/// `process::id()` plus a per-process counter is not unique across runs:
/// Windows recycles pids, and `create_dir_all` reuses a directory it finds
/// without clearing it, so a recycled pid used to hand this run the previous
/// run's observation files — which `wait_for_observations` reads whole.
/// The nonce makes the name unrepeatable; the removal covers the directory a
/// crashed earlier run could still own.
fn unique_dir() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock past the epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "devboule acp {}-{}-{nonce}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    if dir.exists() {
        let _ = std::fs::remove_dir_all(&dir);
    }
    std::fs::create_dir_all(&dir).expect("runtime directory with spaces");
    dir
}

fn hello(name: &str) -> ClientHello {
    let sid = current_user_sid().expect("current user SID");
    ClientHello::m3a(
        OwnerId::new(sid, format!("acp-{name}-{}", std::process::id())).expect("owner"),
        "devboule-acp-test",
    )
}

struct EnvGuard {
    names: Vec<&'static str>,
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        for name in &self.names {
            std::env::remove_var(name);
        }
    }
}

struct Harness {
    dir: PathBuf,
    paths: RuntimePaths,
    child: Option<Child>,
}

impl Harness {
    fn spawn() -> Self {
        Self::spawn_with_env(&[])
    }

    /// A daemon that also gets `extra_env` (and passes it on to its providers).
    ///
    /// The slice-5 battery's knobs go through here rather than through
    /// `std::env::set_var`: the process environment is shared with every other
    /// test in this binary, and the battery must not depend on who holds the
    /// file's test lock.
    fn spawn_with_env(extra_env: &[(&str, &str)]) -> Self {
        Self::spawn_with_env_and_profiles(extra_env, None)
    }

    /// The same daemon, with a profile document in its runtime directory.
    ///
    /// Written **before** the daemon starts: `agent_profiles.rs` reads the file
    /// once, at startup (`AgentProfilesStore::load`), so a document written after
    /// the spawn would be a document this daemon never saw. The file name is the
    /// store's own (`PROFILES_FILE`, `agent-profiles.json`), spelled here because
    /// the module is private to the crate — and a test that got it wrong would
    /// fail on its own assertions rather than silently.
    fn spawn_with_env_and_profiles(
        extra_env: &[(&str, &str)],
        profiles: Option<&serde_json::Value>,
    ) -> Self {
        let dir = unique_dir();
        let paths = RuntimePaths::from_dir(&dir);
        if let Some(profiles) = profiles {
            std::fs::write(
                dir.join("agent-profiles.json"),
                serde_json::to_vec(profiles).expect("profiles json"),
            )
            .expect("the profile document the daemon reads at startup");
        }
        let child = spawn_daemon_with_env(&daemon_bin(), &paths, extra_env).expect("spawn daemon");
        let harness = Self {
            dir,
            paths,
            child: Some(child),
        };
        harness.wait_until_up();
        harness
    }

    fn wait_until_up(&self) {
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            if connect(&self.paths, hello("wait")).is_ok() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("daemon did not start");
    }

    fn client(&self) -> DaemonClient {
        self.client_named("client")
    }

    fn client_named(&self, name: &str) -> DaemonClient {
        connect(&self.paths, hello(name)).expect("connect")
    }

    fn restart(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        std::thread::sleep(Duration::from_millis(150));
        self.child = Some(spawn_daemon(&daemon_bin(), &self.paths).expect("spawn daemon"));
        self.wait_until_up();
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn wait_for<F>(events: &Mutex<Vec<SessionEvent>>, timeout: Duration, predicate: F)
where
    F: Fn(&[SessionEvent]) -> bool,
{
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate(&events.lock().expect("events lock")) {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!(
        "timed out waiting for ACP event: {:?}",
        events.lock().unwrap()
    );
}

fn collect_state_handler(
    received: Arc<Mutex<Vec<Vec<SessionStateSnapshot>>>>,
) -> SessionStateHandler {
    Arc::new(move |snapshots| {
        received
            .lock()
            .expect("state snapshots lock")
            .push(snapshots);
    })
}

fn wait_for_attention(
    snapshots: &Mutex<Vec<Vec<SessionStateSnapshot>>>,
    session_id: &str,
    reason: AttentionReason,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if snapshots
            .lock()
            .expect("state snapshots lock")
            .iter()
            .any(|snapshot| {
                snapshot.iter().any(|session| {
                    session.id == session_id
                        && session
                            .attention
                            .is_some_and(|attention| attention.reason == reason)
                })
            })
        {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!(
        "timed out waiting for {reason:?} attention: {:?}",
        snapshots.lock().expect("state snapshots lock")
    );
}

fn wait_for_cleared_attention(
    snapshots: &Mutex<Vec<Vec<SessionStateSnapshot>>>,
    session_id: &str,
    after_count: usize,
) {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        let snapshots_guard = snapshots.lock().expect("state snapshots lock");
        if snapshots_guard.len() > after_count
            && snapshots_guard.last().is_some_and(|snapshot| {
                snapshot
                    .iter()
                    .any(|session| session.id == session_id && session.attention.is_none())
            })
        {
            return;
        }
        drop(snapshots_guard);
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!(
        "timed out waiting for cleared attention: {:?}",
        snapshots.lock().expect("state snapshots lock")
    );
}

/// Make the stub's build directory resolvable by the daemon-side provider
/// catalog, which scans PATH. The guard restores the original PATH on drop.
struct PathGuard {
    original: std::ffi::OsString,
}

impl Drop for PathGuard {
    fn drop(&mut self) {
        std::env::set_var("PATH", &self.original);
    }
}

fn prepend_stub_dir_to_path() -> PathGuard {
    let original = std::env::var_os("PATH").unwrap_or_default();
    let mut entries = std::env::split_paths(&original).collect::<Vec<_>>();
    entries.insert(
        0,
        stub_bin().parent().expect("stub build dir").to_path_buf(),
    );
    std::env::set_var(
        "PATH",
        std::env::join_paths(&entries).expect("joinable PATH"),
    );
    PathGuard { original }
}

fn stub_only_path() -> PathGuard {
    let original = std::env::var_os("PATH").unwrap_or_default();
    std::env::set_var("PATH", stub_bin().parent().expect("stub build dir"));
    PathGuard { original }
}

/// Ask until the ACP feature read has answered, so a test can say "the read is
/// over" without sleeping on a guess. `probing` while the worker runs; the
/// answer is the list.
fn wait_for_probing(client: &DaemonClient) -> devboule_protocol::VocabularyFeatures {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let DaemonMessage::ProviderVocabulary { features, .. } = client
            .provider_vocabulary_get("devboule-acp-stub", None, false)
            .expect("vocabulary rpc")
        else {
            panic!("unexpected vocabulary reply");
        };
        let axis = features.expect("a new daemon answers the axis");
        if axis.state == devboule_protocol::VocabularyState::Present {
            return axis;
        }
        assert!(
            axis.probing,
            "a final absent answer means the read failed: {axis:?}"
        );
        assert!(Instant::now() < deadline, "the feature read never answered");
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn wait_for_file(path: &Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Ok(value) = std::fs::read_to_string(path) {
            if !value.is_empty() {
                return value;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("stub did not write {}", path.display());
}

fn wait_for_file_value(path: &Path, expected: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if let Ok(value) = std::fs::read_to_string(path) {
            if value == expected {
                return value;
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    panic!("stub did not write {expected:?} to {}", path.display());
}

#[test]
fn acp_stdin_is_closed_before_wait_and_child_exits() {
    let _test_lock = lock_tests();
    let test = AcpTest::new(&[]);
    let (session, _) = test.attached_session();
    let pid: u32 = wait_for_file(&test.pid_file()).parse().expect("stub pid");

    let started = Instant::now();
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
    assert!(started.elapsed() < Duration::from_secs(3), "close hung");
    wait_until_gone(pid);
}

#[test]
fn acp_create_no_window_is_asserted() {
    let _test_lock = lock_tests();
    let test = AcpTest::new(&[]);
    let session = test.create_session();
    assert_eq!(wait_for_file(&test.console_file()), "no-console");
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn acp_child_is_contained_in_its_session_job() {
    let _test_lock = lock_tests();
    let test = AcpTest::new(&[]);
    let session = test.create_session();
    let pid: u32 = wait_for_file(&test.pid_file()).parse().expect("stub pid");
    assert!(
        process_is_in_job(pid),
        "ACP child was not assigned to a Job Object"
    );
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
    wait_until_gone(pid);
}

#[test]
fn acp_create_after_the_last_close_still_contains_the_child() {
    let _test_lock = lock_tests();
    let test = AcpTest::new(&[]);
    let first = test.create_session();
    let first_pid: u32 = wait_for_file(&test.pid_file()).parse().expect("stub pid");
    test.client
        .session_close(&first.id)
        .expect("close the last agent session");
    wait_until_gone(first_pid);
    // The first stub is verified gone, so the next pid file write belongs
    // to the second spawn.
    std::fs::remove_file(test.pid_file()).expect("remove stale pid file");
    let second = test.create_session();
    let second_pid: u32 = wait_for_file(&test.pid_file()).parse().expect("stub pid");
    assert!(
        process_is_in_job(second_pid),
        "an agent child spawned after the last close must still be contained in a Job Object"
    );
    test.client
        .session_close(&second.id)
        .expect("close the second ACP session");
    wait_until_gone(second_pid);
}

/// The `session/new` frame is protocol state the agent keeps: it must name
/// the workspace in the plain spelling the daemon hands its children, not
/// the stored verbatim spelling.
#[test]
fn acp_session_new_carries_a_plain_cwd() {
    let _test_lock = lock_tests();
    let stdin_file = std::env::temp_dir().join(format!(
        "devboule acp stdin {}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_millis()
    ));
    std::env::set_var("DEVBOULE_ACP_STUB_REQUESTS_FILE", &stdin_file);
    let mut test = AcpTest::new(&[]);
    test._env.names.push("DEVBOULE_ACP_STUB_REQUESTS_FILE");

    let dir = std::env::temp_dir().join(format!(
        "devboule acp cwd {}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_millis()
    ));
    std::fs::create_dir_all(&dir).expect("project dir");
    let project = test
        .client
        .project_add(&dir.to_string_lossy())
        .expect("project add");
    let workspace = test
        .client
        .workspace_create(&project.id, WorkspaceIsolation::Local, None)
        .expect("workspace create");
    let session = test
        .client
        .session_create(Some(workspace.id.clone()), SessionKind::Acp, None)
        .expect("create ACP session in workspace");

    let deadline = Instant::now() + Duration::from_secs(10);
    let new_request = loop {
        let recorded = std::fs::read_to_string(&stdin_file).unwrap_or_default();
        let line = recorded
            .lines()
            .find(|line| line.contains(r#""session/new""#));
        if let Some(line) = line {
            break serde_json::from_str::<serde_json::Value>(line)
                .expect("session/new must be one JSON line");
        }
        assert!(
            Instant::now() < deadline,
            "the stub never recorded a session/new request"
        );
        std::thread::sleep(Duration::from_millis(20));
    };
    let cwd = new_request["params"]["cwd"]
        .as_str()
        .expect("session/new carries a cwd")
        .to_string();
    assert_eq!(
        cwd,
        dir.to_string_lossy(),
        "session/new must carry the plain cwd"
    );
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_file(&stdin_file);
}

/// Closing an agent whose grandchild inherited the stub's stdout — the pipe
/// the daemon's reader blocks on — must end the grandchild.
///
/// This test CANNOT fail on a base without the close-order fix
/// (terminate the job before the bounded joins): on such a base the wait
/// thread's OS-death callback still terminates the job on the usual
/// schedule, and the measured run (`red-first-green-at-base.log`) passes.
/// What it does prove is that the outcome keeps holding — a pipe-holding
/// grandchild is dead after the close, within the five-second bound — and,
/// after the close-order fix, that the kill happens synchronously in
/// teardown instead of through a detached callback that close may outlive.
#[test]
fn acp_close_kills_a_grandchild_that_holds_the_output_pipe() {
    let _test_lock = lock_tests();
    // The grandchild's pid file must be named before the daemon spawns the
    // stub, which inherits this process's environment and passes it on.
    let grandchild_file = std::env::temp_dir().join(format!(
        "devboule acp grandchild {}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_millis()
    ));
    std::env::set_var("DEVBOULE_ACP_STUB_GRANDCHILD_PID_FILE", &grandchild_file);
    let mut test = AcpTest::new(&[]);
    test._env
        .names
        .push("DEVBOULE_ACP_STUB_GRANDCHILD_PID_FILE");

    let session = test.create_session();
    let grandchild_pid: u32 = wait_for_file(&grandchild_file)
        .parse()
        .expect("grandchild pid");
    test.client
        .session_close(&session.id)
        .expect("close the agent session");
    // The grandchild inherits the stub's stdout — the pipe the daemon's
    // reader blocks on — so only the session job ending can close the pipe
    // and let the reader reach EOF. A close whose job handle survives the
    // bounded joins leaves the grandchild running.
    wait_until_gone(grandchild_pid);
    let _ = std::fs::remove_file(&grandchild_file);
}

#[test]
fn acp_direct_argv_supports_paths_with_spaces_without_shell() {
    let _test_lock = lock_tests();
    let test = AcpTest::new(&["--direct-path-with-spaces"]);
    let (session, events) = test.attached_session();
    test.client
        .session_send(&session.id, "hello from a path with spaces")
        .expect("prompt");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::AgentFinished { stop_reason, .. } if stop_reason == "end_turn")
        })
    });
    assert!(events.lock().expect("events lock").iter().any(|event| {
        matches!(event, SessionEvent::AgentMessage { text, .. } if text == "stub reply")
    }));
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn acp_framing_handles_partial_crlf_and_skips_malformed_lines() {
    let _test_lock = lock_tests();
    let test = AcpTest::new(&[]);
    let (session, events) = test.attached_session();
    test.client
        .session_send(&session.id, "exercise framing")
        .expect("prompt");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::AgentFinished { stop_reason, .. } if stop_reason == "end_turn")
        })
    });
    let events_snapshot = events.lock().expect("events lock");
    assert!(events_snapshot.iter().any(|event| {
        matches!(event, SessionEvent::AgentMessage { text, .. } if text == "stub reply")
    }));
    assert!(events_snapshot.iter().any(|event| {
        matches!(event, SessionEvent::AgentThought { text, .. } if text == "thinking")
    }));
    assert!(events_snapshot
        .iter()
        .any(|event| { matches!(event, SessionEvent::AgentUserMessage { .. }) }));
    assert!(events_snapshot.iter().any(|event| {
        matches!(event, SessionEvent::AvailableCommands { commands } if commands.iter().any(|command| command.name == "compact"))
    }));
    assert!(events_snapshot.iter().any(|event| {
        matches!(event, SessionEvent::AgentToolCall { tool_call_id, .. } if tool_call_id == "tool-1")
    }));
    assert!(events_snapshot.iter().any(|event| {
        matches!(event, SessionEvent::AgentError { message } if message.contains("Malformed ACP output"))
    }));
    drop(events_snapshot);
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn acp_permission_request_is_queued_when_detached_and_answered_by_tool_call_id() {
    let _test_lock = lock_tests();
    std::env::set_var("DEVBOULE_ACP_STUB_PERMISSION_DELAY_MS", "200");
    let mut test = AcpTest::new(&[]);
    test._env
        .names
        .push("DEVBOULE_ACP_STUB_PERMISSION_DELAY_MS");
    let (session, _) = test.attached_session();
    test.client
        .session_send(&session.id, "please request permission")
        .expect("prompt");
    test.client
        .session_detach(&session.id)
        .expect("detach before permission request arrives");
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        test.client
            .journal_usage()
            .expect("flush permission request journal row before attach");
        let connection =
            Connection::open(test._harness.paths.journal_file()).expect("open journal");
        let request_rows = connection
            .query_row(
                "SELECT COUNT(*) FROM events
                 WHERE session_id = ?1 AND kind = 'acp_envelope'
                   AND instr(CAST(payload AS TEXT), 'session/request_permission') > 0",
                [&session.id],
                |row| row.get::<_, i64>(0),
            )
            .expect("find permission request journal row");
        if request_rows > 0 {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "permission request was not journaled before reattach"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    let events = Arc::new(Mutex::new(Vec::<SessionEvent>::new()));
    let received = Arc::clone(&events);
    let handler: EventHandler = Arc::new(move |envelope| {
        received.lock().expect("events lock").push(envelope.event);
    });
    test.client
        .session_attach(&session.id, None, handler)
        .expect("attach ACP session");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::PermissionRequest { tool_call_id, .. } if tool_call_id == "tool-perm-1")
        })
    });
    test.client
        .session_permission_respond(&session.id, "tool-perm-1", PermissionOutcome::AllowOnce)
        .expect("allow once");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::AgentFinished { stop_reason, .. } if stop_reason == "end_turn")
        })
    });
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn acp_resolved_permission_is_not_reopened_after_live_reattach() {
    let _test_lock = lock_tests();
    let test = AcpTest::new(&[]);
    let (session, events) = test.attached_session();
    test.client
        .session_send(&session.id, "initial replay")
        .expect("initial prompt");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::AgentFinished { stop_reason, .. } if stop_reason == "end_turn")
        })
    });
    test.client
        .session_send(&session.id, "please request permission")
        .expect("prompt");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::PermissionRequest { tool_call_id, .. } if tool_call_id == "tool-perm-1")
        })
    });
    test.client
        .session_permission_respond(&session.id, "tool-perm-1", PermissionOutcome::AllowOnce)
        .expect("allow once");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::PermissionResolved { tool_call_id, .. } if tool_call_id == "tool-perm-1")
        })
    });
    test.client
        .session_detach(&session.id)
        .expect("detach resolved session");

    let reattached = Arc::new(Mutex::new(Vec::<SessionEvent>::new()));
    let received = Arc::clone(&reattached);
    let handler: EventHandler = Arc::new(move |envelope| {
        received
            .lock()
            .expect("reattached events lock")
            .push(envelope.event);
    });
    test.client
        .session_attach(&session.id, None, handler)
        .expect("reattach resolved session");
    wait_for(&reattached, Duration::from_secs(5), |events| {
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::AgentMessage { text, .. } if text == "stub reply"))
    });
    let reattached = reattached.lock().expect("reattached events lock");
    assert!(reattached.iter().any(|event| {
        matches!(event, SessionEvent::AgentMessage { text, .. } if text == "stub reply")
    }));
    assert_eq!(
        reattached
            .iter()
            .filter(|event| matches!(event, SessionEvent::SessionManifest { .. }))
            .count(),
        1,
        "reattach must deliver the stored/journaled manifest exactly once"
    );
    assert!(!reattached.iter().any(|event| {
        matches!(event, SessionEvent::PermissionRequest { tool_call_id, .. } if tool_call_id == "tool-perm-1")
    }));
    drop(reattached);
    test.client
        .session_close(&session.id)
        .expect("close resolved session");
}

/// A2a's live check, driven through the real client and broker: a prompt
/// naming the stub's `chooser` trigger asks a question-shaped card (three
/// colours plus a refusal), the daemon marks it a chooser on the wire, and
/// the stub says back exactly what the answer carried — the option id the
/// person picked, or the cancellation — before the turn ends the normal way.
#[test]
fn acp_chooser_reports_the_option_the_person_picked() {
    let _test_lock = lock_tests();
    let test = AcpTest::new(&[]);
    let session = test.create_session();
    let events = Arc::new(Mutex::new(Vec::<SessionEvent>::new()));
    let received = Arc::clone(&events);
    let handler: EventHandler = Arc::new(move |envelope| {
        received.lock().expect("events lock").push(envelope.event);
    });
    let subscription = test
        .client
        .session_attach(&session.id, None, handler)
        .expect("attach ACP session");

    test.client
        .session_send(&session.id, "chooser")
        .expect("prompt");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::PermissionRequest {
                    tool_call_id,
                    options,
                    is_chooser: Some(true),
                    ..
                } if tool_call_id == "tool-chooser-1"
                    && options.len() == 4
                    && options.iter().filter(|option| option.kind == "allow_once").count() == 3
            )
        })
    });
    test.client
        .session_permission_respond_with_subscription(
            &session.id,
            subscription,
            "tool-chooser-1",
            PermissionOutcome::AllowOnce,
            Some("blue"),
            None,
        )
        .expect("pick blue");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::AgentMessage { text, .. } if text == "You picked blue")
        })
    });
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::AgentFinished { stop_reason, .. } if stop_reason == "end_turn"
            )
        })
    });

    // The other arm of the same report: the interrupt releases the pending
    // card as cancelled, and the stub says the cancellation by name.
    test.client
        .session_send(&session.id, "chooser")
        .expect("second prompt");
    wait_for(&events, Duration::from_secs(5), |events| {
        events
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    SessionEvent::PermissionRequest { tool_call_id, .. } if tool_call_id.starts_with("tool-chooser")
                )
            })
            .count()
            == 2
    });
    test.client
        .session_interrupt(&session.id)
        .expect("interrupt the second turn");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::AgentMessage { text, .. } if text == "You cancelled")
        })
    });
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

/// The reject option of a chooser, answered BY OPTION ID through the real
/// respond path — the healthy road a fresh id takes: the agent is told
/// `selected` with the option the person picked, the journal row for that
/// card holds the deny, and the resolution the card reads is the reject
/// option — Denied, not the daemon's "did not say" fallback.
#[test]
fn acp_chooser_reject_option_by_id_is_journaled_and_resolves_denied() {
    let _test_lock = lock_tests();
    let test = AcpTest::new(&[]);
    let session = test.create_session();
    let events = Arc::new(Mutex::new(Vec::<SessionEvent>::new()));
    let received = Arc::clone(&events);
    let handler: EventHandler = Arc::new(move |envelope| {
        received.lock().expect("events lock").push(envelope.event);
    });
    let subscription = test
        .client
        .session_attach(&session.id, None, handler)
        .expect("attach ACP session");

    test.client
        .session_send(&session.id, "chooser")
        .expect("prompt");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::PermissionRequest {
                    tool_call_id,
                    options,
                    is_chooser: Some(true),
                    ..
                } if tool_call_id == "tool-chooser-1"
                    && options.len() == 4
                    && options.iter().filter(|option| option.kind == "allow_once").count() == 3
            )
        })
    });
    test.client
        .session_permission_respond_with_subscription(
            &session.id,
            subscription,
            "tool-chooser-1",
            PermissionOutcome::Deny,
            Some("none"),
            None,
        )
        .expect("the reject pick is an answer, not an error");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::AgentMessage { text, .. } if text == "You picked none")
        })
    });
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::PermissionResolved {
                    tool_call_id,
                    selected_option_id: Some(option_id),
                    selected_option_kind: Some(kind),
                    selected_option_name: Some(name),
                    ..
                } if tool_call_id == "tool-chooser-1"
                    && option_id == "none"
                    && kind == "reject_once"
                    && name == "None"
            )
        })
    });
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::AgentFinished { stop_reason, .. } if stop_reason == "end_turn"
            )
        })
    });

    test.client
        .journal_usage()
        .expect("flush the permission row");
    let connection = Connection::open(test._harness.paths.journal_file()).expect("open journal");
    let outcome: String = connection
        .query_row(
            "SELECT outcome FROM permissions WHERE session_id = ?1 AND request_id = ?2",
            [&session.id, "tool-chooser-1"],
            |row| row.get(0),
        )
        .expect("the deny row exists for the fresh id");
    assert_eq!(outcome, "deny");
    drop(connection);
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

/// A second question under an id whose answer the journal already holds
/// never becomes a card: the person would answer it and the write-once audit
/// row could not take the answer, so the agent would be told `cancelled`
/// after the human spent the effort (the live P1). The repeat is refused
/// before the card exists — the agent's own refusal frame comes back and the
/// transcript gets its one plain notice — and the first answer's row is
/// untouched. The stub asks reused ids only when the test asks for them
/// (`DEVBOULE_ACP_STUB_REUSE_PERMISSION_IDS`), the way it did before every
/// question carried a fresh id.
#[test]
fn acp_reused_answered_question_id_gets_no_card_and_one_plain_notice() {
    let _test_lock = lock_tests();
    std::env::set_var("DEVBOULE_ACP_STUB_REUSE_PERMISSION_IDS", "1");
    let mut test = AcpTest::new(&[]);
    test._env
        .names
        .push("DEVBOULE_ACP_STUB_REUSE_PERMISSION_IDS");
    let session = test.create_session();
    let events = Arc::new(Mutex::new(Vec::<SessionEvent>::new()));
    let received = Arc::clone(&events);
    let handler: EventHandler = Arc::new(move |envelope| {
        received.lock().expect("events lock").push(envelope.event);
    });
    let subscription = test
        .client
        .session_attach(&session.id, None, handler)
        .expect("attach ACP session");

    // The first question under the reused id: an ordinary card, answered by
    // its option — this is the row the audit keeps.
    test.client
        .session_send(&session.id, "chooser")
        .expect("first prompt");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::PermissionRequest { tool_call_id, .. } if tool_call_id == "tool-chooser"
            )
        })
    });
    test.client
        .session_permission_respond_with_subscription(
            &session.id,
            subscription,
            "tool-chooser",
            PermissionOutcome::AllowOnce,
            Some("green"),
            None,
        )
        .expect("pick green");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::AgentMessage { text, .. } if text == "You picked green")
        })
    });

    // The repeat — same id — is refused before any card exists.
    test.client
        .session_send(&session.id, "chooser")
        .expect("second prompt");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::SessionNotice {
                    text,
                    severity: NoticeSeverity::Info,
                } if text
                    == "The agent reused the id of a question this session already closed, so this request was declined."
            )
        })
    });
    let card_count = events
        .lock()
        .expect("events lock")
        .iter()
        .filter(|event| {
            matches!(
                event,
                SessionEvent::PermissionRequest { tool_call_id, .. } if tool_call_id == "tool-chooser"
            )
        })
        .count();
    assert_eq!(card_count, 1, "the repeat never became a card");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::AgentMessage { text, .. } if text == "You cancelled")
        })
    });

    test.client
        .journal_usage()
        .expect("flush the permission row");
    let connection = Connection::open(test._harness.paths.journal_file()).expect("open journal");
    let outcome: String = connection
        .query_row(
            "SELECT outcome FROM permissions WHERE session_id = ?1 AND request_id = ?2",
            [&session.id, "tool-chooser"],
            |row| row.get(0),
        )
        .expect("the first answer's row still exists");
    assert_eq!(
        outcome, "allow_once",
        "the audit row keeps the first answer"
    );
    drop(connection);
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

/// grok's `_x.ai/ask_user_question` becomes a question card with the agent's
/// labels; the person's pick travels back as labels, which the stub reports
/// before the turn ends the normal way.
#[test]
fn acp_grok_question_answers_with_labels() {
    let _test_lock = lock_tests();
    let test = AcpTest::new(&[]);
    let session = test.create_session();
    let events = Arc::new(Mutex::new(Vec::<SessionEvent>::new()));
    let received = Arc::clone(&events);
    let handler: EventHandler = Arc::new(move |envelope| {
        received.lock().expect("events lock").push(envelope.event);
    });
    let subscription = test
        .client
        .session_attach(&session.id, None, handler)
        .expect("attach ACP session");

    test.client
        .session_send(&session.id, "grok-question")
        .expect("prompt");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::PermissionRequest {
                    tool_call_id,
                    options,
                    kind: Some(PermissionRequestKind::Question),
                    ..
                } if tool_call_id == "tool-grok-1"
                    && options.len() == 3
                    && options.iter().all(|option| option.kind == "allow_once")
            )
        })
    });
    test.client
        .session_permission_respond_with_subscription(
            &session.id,
            subscription,
            "tool-grok-1",
            PermissionOutcome::AllowOnce,
            Some("q0o0"),
            None,
        )
        .expect("pick the first label");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::AgentMessage { text, .. } if text == "You picked Forest green (Recommended)")
        })
    });
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::AgentFinished { stop_reason, .. } if stop_reason == "end_turn"
            )
        })
    });
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

/// Dismissing a grok question answers `cancelled`, and the stub says so.
#[test]
fn acp_grok_question_dismiss_is_cancelled() {
    let _test_lock = lock_tests();
    let test = AcpTest::new(&[]);
    let session = test.create_session();
    let events = Arc::new(Mutex::new(Vec::<SessionEvent>::new()));
    let received = Arc::clone(&events);
    let handler: EventHandler = Arc::new(move |envelope| {
        received.lock().expect("events lock").push(envelope.event);
    });
    let subscription = test
        .client
        .session_attach(&session.id, None, handler)
        .expect("attach ACP session");

    test.client
        .session_send(&session.id, "grok-question")
        .expect("prompt");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::PermissionRequest { tool_call_id, .. } if tool_call_id == "tool-grok-1"
            )
        })
    });
    test.client
        .session_permission_respond_with_subscription(
            &session.id,
            subscription,
            "tool-grok-1",
            PermissionOutcome::Deny,
            None,
            None,
        )
        .expect("dismiss");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::AgentMessage { text, .. } if text == "You cancelled")
        })
    });
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

/// A repeated grok tool call id is refused from the journal before any card
/// exists: one plain notice, no second card, and the audit row keeps the
/// first answer.
#[test]
fn acp_grok_reused_tool_call_id_gets_no_card_and_one_plain_notice() {
    let _test_lock = lock_tests();
    std::env::set_var("DEVBOULE_ACP_STUB_REUSE_PERMISSION_IDS", "1");
    let mut test = AcpTest::new(&[]);
    test._env
        .names
        .push("DEVBOULE_ACP_STUB_REUSE_PERMISSION_IDS");
    let session = test.create_session();
    let events = Arc::new(Mutex::new(Vec::<SessionEvent>::new()));
    let received = Arc::clone(&events);
    let handler: EventHandler = Arc::new(move |envelope| {
        received.lock().expect("events lock").push(envelope.event);
    });
    let subscription = test
        .client
        .session_attach(&session.id, None, handler)
        .expect("attach ACP session");

    test.client
        .session_send(&session.id, "grok-question")
        .expect("first prompt");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::PermissionRequest { tool_call_id, .. } if tool_call_id == "tool-grok"
            )
        })
    });
    test.client
        .session_permission_respond_with_subscription(
            &session.id,
            subscription,
            "tool-grok",
            PermissionOutcome::AllowOnce,
            Some("q0o2"),
            None,
        )
        .expect("pick the third label");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::AgentMessage { text, .. } if text == "You picked Weathered grey")
        })
    });

    test.client
        .session_send(&session.id, "grok-question")
        .expect("second prompt");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::SessionNotice {
                    text,
                    severity: NoticeSeverity::Info,
                } if text
                    == "The agent reused the id of a question this session already closed, so this request was declined."
            )
        })
    });
    let card_count = events
        .lock()
        .expect("events lock")
        .iter()
        .filter(|event| {
            matches!(
                event,
                SessionEvent::PermissionRequest { tool_call_id, .. } if tool_call_id == "tool-grok"
            )
        })
        .count();
    assert_eq!(card_count, 1, "the repeat never became a card");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::AgentMessage { text, .. } if text == "You cancelled")
        })
    });

    test.client
        .journal_usage()
        .expect("flush the permission row");
    let connection = Connection::open(test._harness.paths.journal_file()).expect("open journal");
    let outcome: String = connection
        .query_row(
            "SELECT outcome FROM permissions WHERE session_id = ?1 AND request_id = ?2",
            [&session.id, "tool-grok"],
            |row| row.get(0),
        )
        .expect("the first answer's row still exists");
    assert_eq!(
        outcome, "allow_once",
        "the audit row keeps the first answer"
    );
    drop(connection);
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

// The app attaches right after picking a provider, before any prompt. With
// the journal configured (the daemon always configures it), attach delegates
// delivery to the live-agent replay pull; the manifest must still arrive.
#[test]
fn attach_delivers_the_manifest_before_any_prompt() {
    let _test_lock = lock_tests();
    let test = AcpTest::new(&[]);
    let session = test.create_session();
    let events = Arc::new(Mutex::new(Vec::<SessionEvent>::new()));
    let received = Arc::clone(&events);
    let handler: EventHandler = Arc::new(move |envelope| {
        received.lock().expect("events lock").push(envelope.event);
    });
    test.client
        .session_attach(&session.id, None, handler)
        .expect("attach ACP session");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::SessionManifest { models, .. } if !models.is_empty())
        })
    });
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

// Mid-session reattach with an advanced cursor goes through the same
// live-agent replay pull; the stored manifest must still arrive at the seam.
#[test]
fn attach_mid_session_with_advanced_cursor_still_delivers_the_manifest() {
    let _test_lock = lock_tests();
    let test = AcpTest::new(&[]);
    let (session, first) = test.attached_session();
    test.client
        .session_send(&session.id, "start the session")
        .expect("prompt");
    wait_for(&first, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::AgentFinished { stop_reason, .. } if stop_reason == "end_turn")
        })
    });
    test.client.journal_usage().expect("flush journal");
    let connection = Connection::open(test._harness.paths.journal_file()).expect("open journal");
    let (generation, seq) = connection
        .query_row(
            "SELECT MAX(generation), MAX(seq) FROM events WHERE session_id = ?1",
            [&session.id],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
        )
        .expect("read journal watermark");
    test.client
        .session_detach(&session.id)
        .expect("detach first observer");

    let events = Arc::new(Mutex::new(Vec::<SessionEvent>::new()));
    let received = Arc::clone(&events);
    let handler: EventHandler = Arc::new(move |envelope| {
        received.lock().expect("events lock").push(envelope.event);
    });
    test.client
        .session_attach(
            &session.id,
            Some(Cursor {
                generation: generation as u64,
                seq: seq as u64,
            }),
            handler,
        )
        .expect("attach mid-session with advanced cursor");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::SessionManifest { models, .. } if !models.is_empty())
        })
    });
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn acp_stderr_is_surfaced_after_start_and_during_handshake() {
    let _test_lock = lock_tests();
    let test = AcpTest::new(&[]);
    let (session, events) = test.attached_session();
    test.client
        .session_send(&session.id, "exercise stderr")
        .expect("prompt");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::AgentFinished { stop_reason, .. } if stop_reason == "end_turn")
        })
    });
    let events_snapshot = events.lock().expect("events lock");
    assert!(events_snapshot.iter().any(|event| {
        matches!(event, SessionEvent::AgentStderr { data } if data == "stub-agent stderr marker")
    }));
    assert!(events_snapshot.iter().any(|event| {
        matches!(event, SessionEvent::AgentStderr { data } if data == "stub-agent handshake stderr marker")
    }));
    drop(events_snapshot);
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn acp_startup_failure_includes_agent_stderr() {
    let _test_lock = lock_tests();
    let test = AcpTest::new(&["--fail-initialize"]);
    let error = test
        .client
        .session_create(None, SessionKind::Acp, None)
        .expect_err("startup failure");
    assert!(
        error
            .to_string()
            .contains("stub-agent startup failure stderr marker"),
        "startup error did not include stderr: {}",
        error
    );
    // The stub really exited on its own before the teardown: the naming is
    // the truth here, and this is the direction that keeps the handshake
    // arm's exit read honest after it moved pre-kill.
    assert!(
        error.to_string().contains("provider exited during startup"),
        "an agent that died during its startup is named as that: {error}"
    );
}

#[test]
fn acp_handshake_failure_ends_the_journal_row_and_records_provider_failure() {
    let _test_lock = lock_tests();
    let _path = prepend_stub_dir_to_path();
    std::env::set_var("DEVBOULE_STUB_FAIL_SESSION_NEW", "1");
    struct ClearFailEnv;
    impl Drop for ClearFailEnv {
        fn drop(&mut self) {
            std::env::remove_var("DEVBOULE_STUB_FAIL_SESSION_NEW");
        }
    }
    let _clear = ClearFailEnv;
    let test = AcpTest::new(&[]);

    let error = test
        .client
        .session_create(None, SessionKind::Acp, None)
        .expect_err("a session/new handshake error must reject the create");
    assert!(
        error.to_string().contains("stub credentials expired"),
        "create error did not surface the stub error: {error}"
    );
    // The JSON-RPC error's string message must reach the user, not the
    // serialized error object: auth payloads are noise in a chat banner.
    assert!(
        !error.to_string().contains("authMethods"),
        "create error must not carry the raw error object: {error}"
    );
    assert!(
        error.to_string().contains("(-32000)"),
        "create error must carry the numeric JSON-RPC code: {error}"
    );
    // The stub answers the error and KEEPS RUNNING: the daemon killed it. A
    // post-kill exit read would see our own kill and name this an exited
    // provider — the vacuous check the re-audit flagged (P3-1's sibling).
    assert!(
        !error.to_string().contains("provider exited during startup"),
        "a live agent failing a handshake is not an exited provider: {error}"
    );

    // The journal row was upserted before spawn; a failed spawn must end it,
    // or the roster renders a phantom recovered session with zero events.
    // The journal writer is asynchronous and the upsert lands before the
    // end, so a list taken in between shows the row as `Recovered`: poll
    // until it renders as ended, with a deadline. (CI run 34564790508 caught
    // the loop breaking on the first sighting, `Recovered { generation: 1, .. }`.)
    let deadline = Instant::now() + Duration::from_secs(10);
    let state = loop {
        let sessions = test.client.sessions_list().expect("sessions list");
        let state = sessions
            .iter()
            .find(|session| session.kind == SessionKind::Acp)
            .map(|session| session.state.clone());
        if let Some(state @ devboule_protocol::SessionState::Ended { .. }) = state {
            break state;
        }
        assert!(
            Instant::now() < deadline,
            "the failed-handshake session must render as ended within the deadline, last seen {state:?}"
        );
        std::thread::sleep(Duration::from_millis(100));
    };
    assert!(
        matches!(state, devboule_protocol::SessionState::Ended { .. }),
        "the failed-handshake session must render as ended, got {state:?}"
    );

    let (providers, _) = test.client.providers_list().expect("providers list");
    let stub = providers
        .iter()
        .find(|provider| provider.id == "devboule-acp-stub")
        .expect("stub provider present in providers_list");
    assert!(
        stub.authentication.starts_with("failed:")
            && stub.authentication.contains("stub credentials expired"),
        "stub authentication must carry the failed handshake, got {:?}",
        stub.authentication
    );
    assert!(
        !stub.authentication.contains("authMethods"),
        "the health line must not carry the raw error object, got {:?}",
        stub.authentication
    );
}

#[test]
fn acp_successful_handshake_records_provider_ok() {
    let _test_lock = lock_tests();
    let _path = prepend_stub_dir_to_path();
    let test = AcpTest::new(&[]);
    let session = test.create_session();
    let (providers, _) = test.client.providers_list().expect("providers list");
    let stub = providers
        .iter()
        .find(|provider| provider.id == "devboule-acp-stub")
        .expect("stub provider present in providers_list");
    assert_eq!(
        stub.authentication, "ok",
        "a completed ACP handshake must measure the provider as ok"
    );
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn acp_handshake_agent_info_version_is_visible_in_providers_list() {
    let _test_lock = lock_tests();
    let _path = prepend_stub_dir_to_path();
    let test = AcpTest::new(&[]);
    let session = test.create_session();
    let (providers, _) = test.client.providers_list().expect("providers list");
    let stub = providers
        .iter()
        .find(|provider| provider.id == "devboule-acp-stub")
        .expect("stub provider present in providers_list");
    assert_eq!(stub.agent_version.as_deref(), Some("1"));
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn providers_refresh_probes_stub_version_without_blocking_status() {
    let _test_lock = lock_tests();
    let _path = stub_only_path();
    let version_dir = unique_dir();
    let version_started = version_dir.join("version started.txt");
    std::env::set_var("DEVBOULE_ACP_STUB_VERSION_DELAY_MS", "1500");
    std::env::set_var("DEVBOULE_ACP_STUB_VERSION_FILE", &version_started);
    let mut test = AcpTest::new(&[]);
    test._env.names.push("DEVBOULE_ACP_STUB_VERSION_DELAY_MS");
    test._env.names.push("DEVBOULE_ACP_STUB_VERSION_FILE");
    let client = Arc::clone(&test.client);

    let refresh_client = Arc::clone(&client);
    let refresh = std::thread::spawn(move || refresh_client.providers_refresh());
    let _ = wait_for_file(&version_started);
    let status_started = Instant::now();
    client.status().expect("status during provider refresh");
    assert!(
        status_started.elapsed() < Duration::from_millis(500),
        "status was queued behind refresh: {:?}",
        status_started.elapsed()
    );

    let (providers, _) = refresh.join().expect("refresh thread").expect("refresh");
    let stub = providers
        .iter()
        .find(|provider| provider.id == "devboule-acp-stub")
        .expect("stub provider present in providers_refresh");
    assert_eq!(stub.installed_version.as_deref(), Some("9.9.9"));
    let _ = std::fs::remove_dir_all(version_dir);
}

#[test]
fn acp_session_cancel_reports_cancelled_stop_reason() {
    let _test_lock = lock_tests();
    let test = AcpTest::new(&[]);
    let (session, events) = test.attached_session();
    let pid: u32 = wait_for_file(&test.pid_file()).parse().expect("stub pid");
    test.client
        .session_send(&session.id, "block until cancelled")
        .expect("blocked prompt");
    test.client
        .session_stop(&session.id)
        .expect("cancel ACP prompt");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::AgentFinished { stop_reason, .. } if stop_reason == "cancelled")
        })
    });
    test.client
        .session_close(&session.id)
        .expect("close cancelled ACP session");
    wait_until_gone(pid);
}

#[test]
fn acp_session_interrupt_cancels_the_turn_but_keeps_the_session_alive() {
    let _test_lock = lock_tests();
    let test = AcpTest::new(&[]);
    let (session, events) = test.attached_session();
    let pid: u32 = wait_for_file(&test.pid_file()).parse().expect("stub pid");
    test.client
        .session_send(&session.id, "block until cancelled")
        .expect("blocked prompt");
    test.client
        .session_interrupt(&session.id)
        .expect("interrupt the running turn");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::AgentFinished { stop_reason, .. } if stop_reason == "cancelled")
        })
    });
    // The distinction from stop: the session survives the interrupt, so a
    // second prompt must round-trip normally on the same process.
    test.client
        .session_send(&session.id, "after interrupt")
        .expect("prompt after interrupt");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::AgentFinished { stop_reason, .. } if stop_reason == "end_turn")
        })
    });
    let still_live = test
        .client
        .sessions_list()
        .expect("list sessions")
        .iter()
        .any(|listed| {
            listed.id == session.id
                && matches!(listed.state, devboule_protocol::SessionState::Live { .. })
        });
    assert!(
        still_live,
        "the interrupted session must still list as live"
    );
    test.client
        .session_close(&session.id)
        .expect("close interrupted ACP session");
    wait_until_gone(pid);
}

#[test]
fn acp_set_model_success_publishes_manifest() {
    let _test_lock = lock_tests();
    let test = AcpTest::new(&[]);
    let (session, events) = test.attached_session();
    test.client
        .session_set_model(&session.id, Some("stub-model-new"), Some("low"))
        .expect("set model");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::SessionManifest {
                    current_model_id,
                    ..
                } if current_model_id.as_deref() == Some("stub-model-new")
            )
        })
    });
    let manifest_models: Vec<String> = events
        .lock()
        .expect("events lock")
        .iter()
        .filter_map(|event| match event {
            SessionEvent::SessionManifest {
                current_model_id: Some(model_id),
                ..
            } => Some(model_id.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(
        manifest_models.last().map(String::as_str),
        Some("stub-model-new")
    );
    assert!(
        manifest_models
            .iter()
            .skip(1)
            .all(|model_id| model_id == "stub-model-new"),
        "model switch confirmation regressed after the initial manifest: {manifest_models:?}"
    );
    let confirmed_efforts: Vec<String> = events
        .lock()
        .expect("events lock")
        .iter()
        .filter_map(|event| match event {
            SessionEvent::SessionManifest {
                current_model_id: Some(model_id),
                models,
                ..
            } if model_id == "stub-model-new" => models
                .iter()
                .find(|model| model.model_id == "stub-model-new")
                .and_then(|model| model.current_effort.clone()),
            _ => None,
        })
        .collect();
    assert!(
        !confirmed_efforts.is_empty() && confirmed_efforts.iter().all(|effort| effort == "low"),
        "model+effort confirmation regressed: {confirmed_efforts:?}"
    );
    assert!(
        !events
            .lock()
            .expect("events lock")
            .iter()
            .any(|event| matches!(event, SessionEvent::AgentFinished { .. })),
        "set_model response must not close a prompt turn"
    );
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn acp_set_model_without_provider_push_uses_reply_confirmation() {
    let _test_lock = lock_tests();
    let test = AcpTest::new_without_set_model_push();
    let (session, events) = test.attached_session();
    test.client
        .session_set_model(&session.id, Some("stub-model-new"), None)
        .expect("set model");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::SessionManifest {
                    current_model_id,
                    models,
                    ..
                } if current_model_id.as_deref() == Some("stub-model-new")
                    && models.iter().any(|model| {
                        model.model_id == "stub-model-new"
                            && model.current_effort.as_deref() == Some("high")
                    })
            )
        })
    });
    test.client
        .session_set_model(&session.id, Some("stub-model-no-push-again"), None)
        .expect("set model again");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::SessionManifest {
                    current_model_id,
                    ..
                } if current_model_id.as_deref() == Some("stub-model-no-push-again")
            )
        })
    });
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn acp_config_options_agent_switches_model_via_set_config_option() {
    let _test_lock = lock_tests();
    // The agent deliberately reports a DIFFERENT current model than requested:
    // the daemon must display what the agent said ("sonnet"), not what we
    // asked for ("haiku").
    let test = AcpTest::new_config_options(true);
    let (session, events) = test.attached_session();
    test.client
        .session_set_model(&session.id, Some("haiku"), None)
        .expect("set model");
    assert_eq!(wait_for_file(&test.set_config_file()), "model=haiku");
    assert!(
        !test.set_model_file().exists(),
        "configOptions agents must never receive session/set_model"
    );
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::SessionManifest {
                    current_model_id,
                    models,
                    ..
                } if current_model_id.as_deref() == Some("sonnet")
                    && models.len() == 5
            )
        })
    });
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn acp_config_options_agent_effort_only_switch_targets_the_effort_option() {
    let _test_lock = lock_tests();
    let test = AcpTest::new_config_options(false);
    let (session, events) = test.attached_session();
    test.client
        .session_set_model(&session.id, None, Some("low"))
        .expect("set effort");
    assert_eq!(wait_for_file(&test.set_config_file()), "effort=low");
    assert!(
        !test.set_model_file().exists(),
        "configOptions agents must never receive session/set_model"
    );
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::SessionManifest {
                    current_model_id,
                    models,
                    ..
                } if current_model_id.as_deref() == Some("opus[1m]")
                    && models
                        .iter()
                        .any(|model| model.model_id == "opus[1m]"
                            && model.current_effort.as_deref() == Some("low"))
            )
        })
    });
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn acp_config_options_agent_model_switch_with_effort_chains_the_effort_option() {
    let _test_lock = lock_tests();
    let test = AcpTest::new_config_options(false);
    let (session, events) = test.attached_session();
    test.client
        .session_set_model(&session.id, Some("sonnet"), Some("high"))
        .expect("set model with effort");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::SessionManifest {
                    current_model_id,
                    models,
                    ..
                } if current_model_id.as_deref() == Some("sonnet")
                    && models.iter().any(|model| model.model_id == "sonnet"
                        && model.current_effort.as_deref() == Some("high"))
            )
        })
    });
    // Both config options were set (model first, then the chained effort);
    // the recording file holds the final write.
    assert_eq!(wait_for_file(&test.set_config_file()), "effort=high");
    assert!(
        !test.set_model_file().exists(),
        "configOptions agents must never receive session/set_model"
    );
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn acp_hybrid_config_error_falls_back_to_vendor_model_switch() {
    let _test_lock = lock_tests();
    let test = AcpTest::new_hybrid_config(true, false);
    let (session, events) = test.attached_session();
    test.client
        .session_set_model(&session.id, Some("haiku"), None)
        .expect("set model");
    assert_eq!(wait_for_file(&test.set_config_file()), "model=haiku");
    assert_eq!(wait_for_file(&test.set_model_file()), "haiku");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::SessionManifest { current_model_id, .. }
                    if current_model_id.as_deref() == Some("haiku")
            )
        })
    });
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn acp_hybrid_fallback_membership_guard_preserves_original_error() {
    let _test_lock = lock_tests();
    let test = AcpTest::new_hybrid_config(true, true);
    let (session, events) = test.attached_session();
    test.client
        .session_set_model(&session.id, Some("haiku"), None)
        .expect("set model");
    assert_eq!(wait_for_file(&test.set_config_file()), "model=haiku");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::AgentError { message }
                if message.contains("ACP request failed (-32602)")
                    && message.contains("config option rejected by stub")
                    && message.contains("haiku"))
        })
    });
    assert!(
        !test.set_model_file().exists(),
        "the vendor fallback must not send a value it did not declare"
    );
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn acp_vendor_success_without_model_ack_updates_the_manifest() {
    let _test_lock = lock_tests();
    let test = AcpTest::new_vendor_no_meta();
    let (session, events) = test.attached_session();
    test.client
        .session_set_model(&session.id, Some("stub-model-new"), None)
        .expect("set model");
    assert_eq!(wait_for_file(&test.set_model_file()), "stub-model-new");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::SessionManifest {
                    current_model_id, models, ..
                } if current_model_id.as_deref() == Some("stub-model-new")
                    && !models.is_empty()
                    && models.iter().any(|model| model.model_id == "stub-model-new")
            )
        })
    });
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn acp_effort_config_surface_is_used_when_model_is_vendor_only() {
    let _test_lock = lock_tests();
    let test = AcpTest::new_hybrid_effort_only();
    let (session, events) = test.attached_session();
    test.client
        .session_set_model(&session.id, None, Some("low"))
        .expect("set effort");
    assert_eq!(wait_for_file(&test.set_config_file()), "thought-level=low");
    assert!(
        !test.set_model_file().exists(),
        "effort's declared config surface must be primary"
    );
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::SessionManifest { current_model_id, models, .. }
                    if current_model_id.as_deref() == Some("opus[1m]")
                        && models.iter().any(|model| model.model_id == "opus[1m]"
                            && model.current_effort.as_deref() == Some("low"))
            )
        })
    });
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn acp_config_option_categories_are_advisory_and_declared_ids_are_used() {
    let _test_lock = lock_tests();
    let test = AcpTest::new_categoryless_options();
    let (session, events) = test.attached_session();
    test.client
        .session_set_model(&session.id, Some("m2"), Some("low"))
        .expect("set model and effort");
    // This call sets a model AND an effort, so the stub writes the file twice:
    // `engine=m2` lands first. Waiting for "not empty" races that first write
    // and reads the wrong value, so wait for the value this test is about.
    assert_eq!(
        wait_for_file_value(&test.set_config_file(), "reasoner=low"),
        "reasoner=low",
        "the declared effort id must be the one sent"
    );
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::SessionManifest { current_model_id, models, .. }
                    if current_model_id.as_deref() == Some("m2")
                        && models.iter().any(|model| model.current_effort.as_deref() == Some("low"))
            )
        })
    });
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn acp_config_options_switch_fails_loudly_after_a_modes_only_reattach() {
    let _test_lock = lock_tests();
    // Audit §1: after a daemon restart the reattach (session/load) reply
    // carries modes only. No switch shape may be recorded, and a click must
    // fail loudly — never fall through to session/set_model.
    let mut test = AcpTest::new_config_options_with(false, true, false);
    let session = {
        let (session, events) = test.attached_session();
        test.client
            .session_send(&session.id, "before daemon restart")
            .expect("prompt before restart");
        wait_for(&events, Duration::from_secs(5), |events| {
            events.iter().any(|event| {
                matches!(
                    event,
                    SessionEvent::AgentFinished { stop_reason, .. } if stop_reason == "end_turn"
                )
            })
        });
        session
    };
    test.restart();

    let listed = test.client.sessions_list().expect("list sessions");
    let recovered = listed
        .iter()
        .find(|listed| listed.id == session.id)
        .expect("recovered session missing");
    assert!(matches!(
        recovered.state,
        devboule_protocol::SessionState::Recovered { .. }
    ));

    // The real reattach entry: session_resume spawns the agent again with
    // session/load (the persisted peer id), unlike a plain attach which only
    // replays the journal.
    let resumed = test
        .client
        .session_resume(
            Persistence {
                kind: PersistenceKind::Acp {
                    handle: session.id.clone(),
                },
            },
            None,
        )
        .expect("resume recovered ACP session");
    assert!(matches!(resumed, ResumeResult::Resumed { .. }));

    let (recovered_session, events) = test.attached_existing(&session.id);
    // The reattach parsed to a modes-only manifest with NO switch shape.
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::SessionManifest { models, modes, .. }
                    if models.is_empty() && modes.is_some()
            )
        })
    });

    let error = test
        .client
        .session_set_model(&recovered_session.id, Some("haiku"), None)
        .expect_err("switch without a recorded shape must fail loudly");
    let message = error.to_string();
    assert!(
        message.contains("no model catalog") || message.contains("switch verb is unknown"),
        "unexpected switch error: {message}"
    );
    assert!(
        !test.set_model_file().exists() && !test.set_config_file().exists(),
        "a shapeless switch must not reach the agent"
    );
    test.client
        .session_close(&recovered_session.id)
        .expect("close ACP session");
}

#[test]
fn acp_modes_only_reattach_vendor_push_restores_model_switch_surface() {
    let _test_lock = lock_tests();
    let mut test = AcpTest::new_modes_only_then_vendor_push();
    let session = {
        let (session, events) = test.attached_session();
        test.client
            .session_send(&session.id, "before daemon restart")
            .expect("prompt before restart");
        wait_for(&events, Duration::from_secs(5), |events| {
            events.iter().any(|event| {
                matches!(event, SessionEvent::AgentFinished { stop_reason, .. } if stop_reason == "end_turn")
            })
        });
        session
    };
    test.restart();
    test.client
        .session_resume(
            Persistence {
                kind: PersistenceKind::Acp {
                    handle: session.id.clone(),
                },
            },
            None,
        )
        .expect("resume recovered ACP session");
    let (recovered_session, events) = test.attached_existing(&session.id);
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::SessionManifest { current_model_id, models, .. }
                    if current_model_id.as_deref() == Some("opus[1m]")
                        && models.iter().any(|model| model.model_id == "haiku")
            )
        })
    });
    test.client
        .session_set_model(&recovered_session.id, Some("haiku"), None)
        .expect("vendor switch after models push");
    assert_eq!(wait_for_file(&test.set_model_file()), "haiku");
    assert!(
        !test.set_config_file().exists(),
        "vendor push must restore the vendor surface, not guess configOptions"
    );
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::SessionManifest { current_model_id, .. }
                    if current_model_id.as_deref() == Some("haiku")
            )
        })
    });
    test.client
        .session_close(&recovered_session.id)
        .expect("close recovered ACP session");
}

#[test]
fn acp_config_options_malformed_success_reply_uses_requested_value() {
    let _test_lock = lock_tests();
    // Audit §6: a JSON-RPC success with no parseable catalog still means the
    // requested value was accepted. Use the requested value, as Paseo does,
    // rather than silently leaving the old model selected.
    let test = AcpTest::new_config_options_with(false, false, true);
    let (session, events) = test.attached_session();
    test.client
        .session_set_model(&session.id, Some("haiku"), None)
        .expect("rpc accepted");
    assert_eq!(wait_for_file(&test.set_config_file()), "model=haiku");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::SessionManifest { current_model_id, .. }
                    if current_model_id.as_deref() == Some("haiku")
            )
        })
    });
    assert!(!events
        .lock()
        .expect("events lock")
        .iter()
        .any(|event| matches!(event, SessionEvent::AgentError { .. })));
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn acp_hybrid_vendor_fallback_still_applies_declared_effort() {
    let _test_lock = lock_tests();
    let test = AcpTest::new_hybrid_config(true, false);
    let (session, events) = test.attached_session();
    test.client
        .session_set_model(&session.id, Some("haiku"), Some("low"))
        .expect("set model and effort");
    assert_eq!(wait_for_file(&test.set_model_file()), "haiku");
    assert_eq!(
        wait_for_file(&test.set_model_effort_file()),
        "low",
        "vendor fallback carries the requested effort"
    );
    assert_eq!(
        wait_for_file_value(&test.set_config_file(), "effort=low"),
        "effort=low",
        "the declared config effort follow-up must still be sent"
    );
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::SessionManifest { current_model_id, models, .. }
                    if current_model_id.as_deref() == Some("haiku")
                        && models.iter().any(|model| {
                            model.model_id == "haiku"
                                && model.current_effort.as_deref() == Some("low")
                        })
            )
        })
    });
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn acp_vendor_only_agent_uses_legacy_set_model() {
    let _test_lock = lock_tests();
    // A vendor-only manifest still selects the legacy session/set_model verb;
    // the config-option path must not fire when no config surface was declared.
    let test = AcpTest::new_without_set_model_push();
    let (session, events) = test.attached_session();
    test.client
        .session_set_model(&session.id, Some("stub-model-new"), None)
        .expect("set model");
    assert_eq!(wait_for_file(&test.set_model_file()), "stub-model-new");
    assert!(
        !test.set_config_file().exists(),
        "grok-shaped agents must never receive session/set_config_option"
    );
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::SessionManifest {
                    current_model_id,
                    ..
                } if current_model_id.as_deref() == Some("stub-model-new")
            )
        })
    });
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn acp_effort_only_set_model_uses_the_current_model_id() {
    let _test_lock = lock_tests();
    let test = AcpTest::new_without_set_model_push();
    let (session, events) = test.attached_session();
    test.client
        .session_set_model(&session.id, None, Some("low"))
        .expect("set effort");
    assert_eq!(wait_for_file(&test.set_model_file()), "stub-model");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::SessionManifest {
                    current_model_id,
                    models,
                    ..
                } if current_model_id.as_deref() == Some("stub-model")
                    && models.iter().any(|model| model.current_effort.as_deref() == Some("low"))
            )
        })
    });
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn acp_model_only_set_model_carries_target_default_effort() {
    let _test_lock = lock_tests();
    let test = AcpTest::new_without_set_model_push();
    let (session, _events) = test.attached_session();
    test.client
        .session_set_model(&session.id, Some("stub-model-new"), None)
        .expect("set model");
    assert_eq!(wait_for_file(&test.set_model_effort_file()), "high");
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn acp_model_only_without_target_efforts_omits_effort() {
    let _test_lock = lock_tests();
    let test = AcpTest::new_without_target_efforts();
    let (session, _events) = test.attached_session();
    test.client
        .session_set_model(&session.id, Some("stub-model-new"), None)
        .expect("set model");
    assert_eq!(wait_for_file(&test.set_model_effort_file()), "<none>");
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn acp_explicit_set_model_effort_wins_target_default() {
    let _test_lock = lock_tests();
    let test = AcpTest::new_without_set_model_push();
    let (session, _events) = test.attached_session();
    test.client
        .session_set_model(&session.id, Some("stub-model-new"), Some("low"))
        .expect("set model and effort");
    assert_eq!(wait_for_file(&test.set_model_effort_file()), "low");
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn acp_model_and_effort_reply_confirmation_carries_both_values() {
    let _test_lock = lock_tests();
    let test = AcpTest::new_without_set_model_push();
    let (session, events) = test.attached_session();
    test.client
        .session_set_model(&session.id, Some("stub-model-new"), Some("low"))
        .expect("set model and effort");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::SessionManifest {
                    current_model_id,
                    models,
                    ..
                } if current_model_id.as_deref() == Some("stub-model-new")
                    && models.iter().any(|model| {
                        model.model_id == "stub-model-new"
                            && model.current_effort.as_deref() == Some("low")
                    })
            )
        })
    });
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn acp_catalog_default_push_does_not_clobber_confirmed_effort() {
    let _test_lock = lock_tests();
    let test = AcpTest::new_with_catalog_default_push();
    let (session, events) = test.attached_session();
    test.client
        .session_set_model(&session.id, Some("stub-model-new"), Some("low"))
        .expect("set model and effort");
    wait_for(&events, Duration::from_secs(5), |events| {
        let manifests = events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::SessionManifest {
                    current_model_id: Some(model_id),
                    models,
                    ..
                } if model_id == "stub-model-new" => models
                    .iter()
                    .find(|model| model.model_id == "stub-model-new")
                    .and_then(|model| model.current_effort.as_deref()),
                _ => None,
            })
            .collect::<Vec<_>>();
        manifests.len() >= 2 && manifests.last() == Some(&"low")
    });
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn acp_sessions_changed_updates_confirmed_effort() {
    let _test_lock = lock_tests();
    let test = AcpTest::new_with_sessions_changed();
    let (session, events) = test.attached_session();
    test.client
        .session_set_model(&session.id, Some("stub-model-new"), None)
        .expect("set model");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(
                event,
                SessionEvent::SessionManifest {
                    current_model_id,
                    models,
                    ..
                } if current_model_id.as_deref() == Some("stub-model-new")
                    && models.iter().any(|model| {
                        model.model_id == "stub-model-new"
                            && model.current_effort.as_deref() == Some("medium")
                    })
            )
        })
    });
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn acp_set_model_error_without_alternate_is_an_agent_error() {
    let _test_lock = lock_tests();
    let test = AcpTest::new_rejecting_set_model();
    let (session, events) = test.attached_session();
    test.client
        .session_set_model(&session.id, Some("unknown-model"), None)
        .expect("daemon accepted the set-model request");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::AgentError { message } if message.contains("unknown model"))
        })
    });
    assert!(!events.lock().expect("events lock").iter().any(|event| {
        matches!(
            event,
            SessionEvent::SessionManifest {
                current_model_id,
                ..
            } if current_model_id.as_deref() == Some("unknown-model")
        )
    }));
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn set_model_on_terminal_session_is_rejected() {
    let _test_lock = lock_tests();
    let test = AcpTest::new(&[]);
    let session = test
        .client
        .session_create(None, SessionKind::Terminal, None)
        .expect("create terminal session");
    let error = test
        .client
        .session_set_model(&session.id, Some("stub-model-new"), None)
        .expect_err("terminal sessions must reject model switching");
    match error {
        devboule_daemon::DaemonError::Handshake(wire) => {
            assert_eq!(wire.code, ErrorCode::InvalidRequest);
            assert_eq!(
                wire.message,
                "Only agent sessions support switching the model or effort."
            );
        }
        other => panic!("expected InvalidRequest, got {other:?}"),
    }
    test.client
        .session_close(&session.id)
        .expect("close terminal session");
}

#[test]
fn acp_session_survives_daemon_restart_and_replays_agent_message() {
    const MARKER: &str = "stub reply";

    let _test_lock = lock_tests();
    let mut test = AcpTest::new(&[]);
    let session = test.create_session();
    let live_events = Arc::new(Mutex::new(Vec::<SessionEvent>::new()));
    let live_received = Arc::clone(&live_events);
    let live_handler: EventHandler = Arc::new(move |envelope| {
        live_received
            .lock()
            .expect("live events lock")
            .push(envelope.event);
    });
    test.client
        .session_attach(&session.id, None, live_handler)
        .expect("attach ACP session");
    test.client
        .session_send(&session.id, "before daemon restart")
        .expect("prompt before daemon restart");
    wait_for(&live_events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::AgentFinished { stop_reason, .. } if stop_reason == "end_turn")
        })
    });
    assert!(
        live_events.lock().expect("live events lock").iter().any(
            |event| matches!(event, SessionEvent::AgentMessage { text, .. } if text == MARKER)
        ),
        "stub did not produce the pre-restart marker"
    );

    // journal_usage waits for the asynchronous journal writer, so the daemon
    // restart cannot race the pre-restart ACP events still being persisted.
    assert!(
        test.journal_event_count(&session.id) > 0,
        "pre-restart ACP events were not journaled"
    );
    test.restart();

    let listed = test.client.sessions_list().expect("list sessions");
    let recovered = listed
        .iter()
        .find(|listed| listed.id == session.id)
        .expect("recovered ACP session missing from sessions_list");
    assert!(
        matches!(
            recovered.state,
            devboule_protocol::SessionState::Recovered { .. }
        ),
        "expected recovered ACP session, got {:?}",
        recovered.state
    );

    let replayed_events = Arc::new(Mutex::new(Vec::<SessionEvent>::new()));
    let replayed_received = Arc::clone(&replayed_events);
    let replayed_handler: EventHandler = Arc::new(move |envelope| {
        replayed_received
            .lock()
            .expect("replayed events lock")
            .push(envelope.event);
    });
    test.client
        .session_attach(&session.id, None, replayed_handler)
        .expect("attach recovered ACP session");
    wait_for(&replayed_events, Duration::from_secs(5), |events| {
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::AgentMessage { text, .. } if text == MARKER))
    });
}

#[test]
fn acp_session_resume_loads_without_rejournaling_replay_and_keeps_identity() {
    let _test_lock = lock_tests();
    let test = AcpTest::new(&[]);
    let session = test.create_session();
    let events = Arc::new(Mutex::new(Vec::<SessionEvent>::new()));
    let received = Arc::clone(&events);
    let handler: EventHandler = Arc::new(move |envelope| {
        received.lock().expect("events lock").push(envelope.event);
    });
    test.client
        .session_attach(&session.id, None, handler)
        .expect("attach ACP session");
    test.client
        .session_send(&session.id, "before resume")
        .expect("initial prompt");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::AgentFinished { stop_reason, .. } if stop_reason == "end_turn")
        })
    });
    let pid: u32 = wait_for_file(&test.pid_file()).parse().expect("stub pid");
    test.client
        .session_stop(&session.id)
        .expect("stop ACP session");
    wait_until_gone(pid);
    let before_resume = test.journal_event_count(&session.id);

    let result = test
        .client
        .session_resume(
            Persistence {
                kind: PersistenceKind::Acp {
                    handle: session.id.clone(),
                },
            },
            None,
        )
        .expect("resume ACP session");
    assert!(matches!(
        result,
        ResumeResult::Resumed { session: resumed }
            if resumed.id == session.id
                && matches!(resumed.state, devboule_protocol::SessionState::Live { generation: 2 })
    ));

    let resumed_received = Arc::clone(&events);
    let resumed_handler: EventHandler = Arc::new(move |envelope| {
        resumed_received
            .lock()
            .expect("events lock")
            .push(envelope.event);
    });
    test.client
        .session_attach(&session.id, None, resumed_handler)
        .expect("attach resumed ACP session");
    wait_for(&events, Duration::from_secs(5), |events| {
        events
            .iter()
            .filter(|event| matches!(event, SessionEvent::SessionManifest { .. }))
            .count()
            == 1
    });
    assert_eq!(test.journal_event_count(&session.id), before_resume);
    test.client
        .session_send(&session.id, "after resume")
        .expect("live prompt after resume");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::AgentFinished { stop_reason, .. } if stop_reason == "end_turn")
        })
    });
    // The journal writer is asynchronous: the AgentFinished publish can win
    // the race against the row landing on disk, and did on the 4-vCPU CI
    // runner. Poll with a deadline instead of asserting a snapshot.
    let journal_deadline = std::time::Instant::now() + Duration::from_secs(10);
    while test.journal_event_count(&session.id) <= before_resume {
        assert!(
            std::time::Instant::now() < journal_deadline,
            "the live prompt after resume must reach the journal"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    test.client
        .session_close(&session.id)
        .expect("close resumed ACP session");
}

/// Settles a SETUP fact (a stop's end marker landing) before the test's
/// real assertions. Never used for the retraction itself: that property is
/// measured by the single immediate read the app itself makes.
fn wait_for_resumable(test: &AcpTest, id: &str, wanted: bool, what: &str) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let row = test
            .client
            .sessions_list()
            .expect("list sessions")
            .into_iter()
            .find(|listed| listed.id == id)
            .unwrap_or_else(|| panic!("session {id} missing from sessions_list"));
        if row.resumable == wanted {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "session {id} resumable stuck at {} (wanted {wanted}): {what}",
            row.resumable
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The journal row's `peer_session_id` column, read through a **fresh**
/// connection — the restart path's own way in — after forcing the
/// asynchronous writer to drain.
fn journal_peer_session_id(test: &AcpTest, session_id: &str) -> Option<String> {
    test.client.journal_usage().expect("flush journal");
    let connection = Connection::open(test._harness.paths.journal_file()).expect("open journal");
    connection
        .query_row(
            "SELECT peer_session_id FROM sessions WHERE id = ?1",
            [session_id],
            |row| row.get::<_, Option<String>>(0),
        )
        .expect("session row")
}

/// A stopped zero-turn session the stub still owns a handle for, ready for
/// its resume to fail in the way the test's stub knob decides.
fn stopped_zero_turn_session(test: &AcpTest) -> devboule_protocol::Session {
    let session = test.create_session();
    test.client
        .session_attach(&session.id, None, Arc::new(|_| {}))
        .expect("attach ACP session");
    let pid: u32 = wait_for_file(&test.pid_file()).parse().expect("stub pid");
    test.client
        .session_stop(&session.id)
        .expect("stop ACP session");
    wait_until_gone(pid);
    wait_for_resumable(
        test,
        &session.id,
        true,
        "the defect precondition: the dead row still offers resume",
    );
    session
}

/// The field defect: a session created and never prompted offers Reopen, the
/// far agent answers "I do not have this session", and the offer must then
/// go. The daemon's own sentence comes back unchanged — what changes is that
/// the row stops offering what cannot work.
#[test]
fn a_resume_the_far_agent_disowns_ends_the_offer() {
    let _test_lock = lock_tests();
    let test = AcpTest::new_refusing_load();
    let session = stopped_zero_turn_session(&test);

    let error = test
        .client
        .session_resume(
            Persistence {
                kind: PersistenceKind::Acp {
                    handle: session.id.clone(),
                },
            },
            None,
        )
        .expect_err("the far agent refuses the load");
    match &error {
        devboule_daemon::DaemonError::Handshake(wire) => {
            // The sentence is byte for byte what the field measured (the
            // answer names the session it lacks), and the code is the one the
            // caller has always seen for this answer: the classification is
            // the daemon's internal channel, never a wire statement about a
            // row this daemon still has.
            assert_eq!(wire.code, ErrorCode::Io);
            assert!(
                wire.message
                    .contains("ACP request failed (-32002): Resource not found: stub-session"),
                "the user-visible sentence must be unchanged, got: {}",
                wire.message
            );
        }
        other => panic!("expected the daemon's own ACP sentence, got {other:?}"),
    }
    // What the app does: ONE read, issued the instant the failing resume
    // returns. A poll would prove an eventual retraction the UI can miss —
    // the race is the defect, so the single read is the assertion.
    //
    // Road scope, plainly: this pins the dispatch-thread road — the one
    // every real client rides — on a healthy queue, where the bounded rpc
    // commits before the failing answer leaves. The saturated-queue road
    // (the detached fallback, which marks first, before the end marker)
    // cannot be forced from here without sabotaging the queue itself; its
    // ordering is fixed in the arm's code, and the bounded write's failure
    // is reported, never swallowed.
    let row = test
        .client
        .sessions_list()
        .expect("list sessions")
        .into_iter()
        .find(|listed| listed.id == session.id)
        .expect("the row the daemon still has");
    assert!(
        !row.resumable,
        "one read, immediately after the failing resume, must already see the retraction"
    );
    assert_eq!(
        row.peer_session_id.as_deref(),
        Some("stub-session"),
        "the handle is never destroyed: the refusal is recorded beside it, \
         because evidence that approximates must not trigger the irreversible"
    );
}

/// The classification is not "any failure clears": a load that never got an
/// answer says nothing about the far session, so the handle and the offer
/// stay exactly as they were.
#[test]
fn a_resume_failure_that_says_nothing_about_the_session_keeps_the_offer() {
    let _test_lock = lock_tests();
    let test = AcpTest::new_exiting_on_load();
    let session = stopped_zero_turn_session(&test);

    test.client
        .session_resume(
            Persistence {
                kind: PersistenceKind::Acp {
                    handle: session.id.clone(),
                },
            },
            None,
        )
        .expect_err("the load gets no answer");
    // Nothing was learned about the far session, so nothing was retracted:
    // the same one immediate read the app fires sees the offer still standing.
    let row = test
        .client
        .sessions_list()
        .expect("list sessions")
        .into_iter()
        .find(|listed| listed.id == session.id)
        .expect("the row the daemon still has");
    assert!(
        row.resumable,
        "a transport failure must keep the offer on the next read"
    );
    assert!(
        row.peer_session_id.is_some(),
        "a transport failure must keep the handle on the row"
    );
    // And the journal row itself, through a fresh connection after the write
    // queue drains: the handle is still there.
    assert!(
        journal_peer_session_id(&test, &session.id).is_some(),
        "a transport failure must not clear the handle"
    );
}

/// The mark is a journal fact, not registry state, and it destroys nothing:
/// a fresh read of the row — the restart path's own way in — finds the
/// handle exactly where it was, with the refusal recorded beside it, and a
/// restarted daemon reports the offer gone.
#[test]
fn a_disowned_handle_stays_retracted_across_a_restart() {
    let _test_lock = lock_tests();
    let mut test = AcpTest::new_refusing_load();
    let session = stopped_zero_turn_session(&test);

    assert!(
        test.client
            .session_resume(
                Persistence {
                    kind: PersistenceKind::Acp {
                        handle: session.id.clone(),
                    },
                },
                None,
            )
            .is_err(),
        "the far agent refuses the load"
    );

    // Give the fallback write every chance to have landed, then insist the
    // handle was never destroyed.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if journal_peer_session_id(&test, &session.id).is_some() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the disown destroyed the handle instead of recording the refusal"
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    test.restart();
    let row = test
        .client
        .sessions_list()
        .expect("list sessions")
        .into_iter()
        .find(|listed| listed.id == session.id)
        .expect("session row after restart");
    assert!(
        !row.resumable,
        "the refusal must survive the restart: the offer stays gone"
    );
    assert_eq!(
        row.peer_session_id.as_deref(),
        Some("stub-session"),
        "the handle survives the restart too"
    );
}

/// A refusal is about a handle — and a successful resume of that very
/// handle is the provider taking it back. The success clears the mark, so
/// the offer returns when the session ends again; without the clear, a mark
/// could hide a working session with no road left that could correct it,
/// because only a resume can prove a handle good.
#[test]
fn a_successful_resume_clears_a_mark_for_the_handle_it_honoured() {
    let _test_lock = lock_tests();
    let test = AcpTest::new_refusing_load_once();
    let session = stopped_zero_turn_session(&test);
    let persistence = Persistence {
        kind: PersistenceKind::Acp {
            handle: session.id.clone(),
        },
    };

    // The first ask: refused, marked, and the one immediate read sees the
    // offer gone.
    let error = test
        .client
        .session_resume(persistence.clone(), None)
        .expect_err("the first ask is refused");
    match &error {
        devboule_daemon::DaemonError::Handshake(wire) => assert_eq!(wire.code, ErrorCode::Io),
        other => panic!("expected the daemon's own ACP sentence, got {other:?}"),
    }
    let marked = test
        .client
        .sessions_list()
        .expect("list sessions")
        .into_iter()
        .find(|listed| listed.id == session.id)
        .expect("the row the daemon still has");
    assert!(!marked.resumable, "the refusal hides the offer");

    // The second ask: the provider honours the very handle it refused.
    let resumed = test
        .client
        .session_resume(persistence, None)
        .expect("the provider honours the handle on the second ask");
    assert!(matches!(
        resumed,
        ResumeResult::Resumed { session: ref r }
            if r.id == session.id
                && matches!(r.state, devboule_protocol::SessionState::Live { generation: 3 })
    ));

    // End the session again and ask the roster question the app asks.
    test.client
        .session_attach(&session.id, None, Arc::new(|_| {}))
        .expect("attach the resumed session");
    let pid: u32 = wait_for_file(&test.pid_file()).parse().expect("stub pid");
    test.client
        .session_stop(&session.id)
        .expect("stop the resumed session");
    wait_until_gone(pid);
    // The stop's own bookkeeping (EOF cleanup, the end marker) settles
    // asynchronously, and until it does the row legitimately reads live.
    // The property under test is the verdict of the SETTLED row: the mark
    // was already cleared when the successful resume returned.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let row = test
            .client
            .sessions_list()
            .expect("list sessions")
            .into_iter()
            .find(|listed| listed.id == session.id)
            .expect("the row the daemon still has");
        if matches!(row.state, devboule_protocol::SessionState::Ended { .. }) {
            assert!(
                row.resumable,
                "the honoured handle's refusal was stale: the offer returns"
            );
            assert_eq!(
                row.peer_session_id.as_deref(),
                Some("stub-session"),
                "the handle was never destroyed"
            );
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the resumed session never settled to an ended row"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A second client that attaches during the spawn window hydrates a
/// `Transcript` entry from the row as it stood BEFORE the mark — and the
/// live map wins a roster read, so that entry would serve its stale
/// `resumable` forever. The failing resume must retire that entry, so the
/// next read comes from the journal that holds the mark.
#[test]
fn an_attach_that_raced_the_spawn_window_sees_the_mark() {
    let _test_lock = lock_tests();
    let test = AcpTest::new_slow_refusing_load();
    let session = stopped_zero_turn_session(&test);

    let resumer = test._harness.client_named("resumer");
    let racer = test._harness.client_named("racer");
    let persistence = Persistence {
        kind: PersistenceKind::Acp {
            handle: session.id.clone(),
        },
    };
    let resume_thread = std::thread::spawn(move || resumer.session_resume(persistence, None));
    // Land inside the widened window: the resumed spawn is mid-handshake,
    // the registry holds no entry for the id, so this attach hydrates one
    // from the journal row as it stood before the mark existed.
    std::thread::sleep(Duration::from_millis(700));
    racer
        .session_attach(&session.id, None, Arc::new(|_| {}))
        .expect("the racing attach lands in the spawn window");
    let result = resume_thread.join().expect("resume thread completes");
    assert!(result.is_err(), "the far agent refuses the load");

    // ONE read, the app's own: the window's stale entry must not serve the
    // old verdict.
    let row = test
        .client
        .sessions_list()
        .expect("list sessions")
        .into_iter()
        .find(|listed| listed.id == session.id)
        .expect("the row the daemon still has");
    assert!(
        !row.resumable,
        "the entry that raced the spawn window must not keep the offer standing"
    );
    // Road scope, plainly: on this tree the daemon dispatch serializes the
    // window — the attach issued mid-window is processed after the mark — so
    // this test pins the observable contract (a read after the failing
    // resume is journal-fresh), and cannot by itself distinguish that
    // ordering from the eviction that retires a stale entry. The eviction
    // stays as structural defense for any window a future dispatch change
    // could open.
}

/// The evidence standard: the code alone proves nothing. `-32002` is the ACP
/// schema's word for ANY missed resource — this daemon's own host answers it
/// for terminals and file reads — so an agent whose workspace directory is
/// gone answers it about that directory while the far conversation is
/// perfectly alive. Only an answer that names the session we asked to load
/// may retract the handle.
#[test]
fn a_resource_miss_that_does_not_name_the_session_keeps_the_offer() {
    let _test_lock = lock_tests();
    let test = AcpTest::new_refusing_load_other_resource();
    let session = stopped_zero_turn_session(&test);

    let error = test
        .client
        .session_resume(
            Persistence {
                kind: PersistenceKind::Acp {
                    handle: session.id.clone(),
                },
            },
            None,
        )
        .expect_err("the agent refuses the load naming another resource");
    match &error {
        devboule_daemon::DaemonError::Handshake(wire) => {
            assert_eq!(wire.code, ErrorCode::Io);
            // The fixture echoes the request — session id included — beside
            // the missed file: the false-positive direction the echo could
            // take. The sentence the app reads is still the agent's own.
            assert!(
                wire.message.contains(
                    "ACP request failed (-32002): Resource not found: file:///gone/workspace \
                     (requested sessionId: stub-session)"
                ),
                "the user-visible sentence must be unchanged, got: {}",
                wire.message
            );
        }
        other => panic!("expected the daemon's own ACP sentence, got {other:?}"),
    }
    // One immediate read, the app's own: the offer and the handle stand,
    // because nothing named the session as gone.
    let row = test
        .client
        .sessions_list()
        .expect("list sessions")
        .into_iter()
        .find(|listed| listed.id == session.id)
        .expect("the row the daemon still has");
    assert!(
        row.resumable,
        "a resource miss that is not the session must keep the offer"
    );
    assert!(
        row.peer_session_id.is_some(),
        "a resource miss that is not the session must keep the handle"
    );
    assert!(
        journal_peer_session_id(&test, &session.id).is_some(),
        "the handle stays on the journal row"
    );
}

#[test]
fn acp_session_resume_does_not_leave_other_observer_silent() {
    let _test_lock = lock_tests();
    let test = AcpTest::new(&[]);
    let session = test.create_session();
    test.client
        .session_attach(&session.id, None, Arc::new(|_| {}))
        .expect("resumer observer attaches");
    let other = test._harness.client_named("other");
    let other_events = Arc::new(Mutex::new(Vec::<SessionEvent>::new()));
    let received = Arc::clone(&other_events);
    let armed = Arc::new(AtomicBool::new(false));
    let first_blocked = Arc::new(AtomicBool::new(false));
    let release = Arc::new(Barrier::new(2));
    let (entered_tx, entered_rx) = mpsc::channel();
    let handler_armed = Arc::clone(&armed);
    let handler_release = Arc::clone(&release);
    let handler: EventHandler = Arc::new(move |envelope| {
        if handler_armed.load(Ordering::Acquire) && !first_blocked.swap(true, Ordering::AcqRel) {
            let _ = entered_tx.send(());
            handler_release.wait();
        }
        received
            .lock()
            .expect("other events lock")
            .push(envelope.event);
    });
    other
        .session_attach(&session.id, None, handler)
        .expect("other observer attaches");
    armed.store(true, Ordering::Release);

    let pid: u32 = wait_for_file(&test.pid_file()).parse().expect("stub pid");
    test.client
        .session_stop(&session.id)
        .expect("stop ACP session");
    wait_until_gone(pid);
    entered_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("observer receives the old generation event");

    test.client
        .session_resume(
            Persistence {
                kind: PersistenceKind::Acp {
                    handle: session.id.clone(),
                },
            },
            None,
        )
        .expect("resume ACP session");
    release.wait();

    wait_for(&other_events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::Exit { .. })
                || matches!(
                    event,
                    SessionEvent::AgentError { message }
                        if message
                            == "Session generation was replaced; reattach to continue observing."
                )
        })
    });

    test.client
        .session_attach(&session.id, None, Arc::new(|_| {}))
        .expect("reattach resumed ACP session");
    test.client
        .session_close(&session.id)
        .expect("close resumed ACP session");
}

#[test]
fn client_without_attachment_explains_how_to_send() {
    let _test_lock = lock_tests();
    let test = AcpTest::new(&[]);
    let session = test.create_session();
    let other = test._harness.client_named("unattached");

    let error = other
        .session_send(&session.id, "must be rejected locally")
        .expect_err("unattached client must be rejected");
    assert_eq!(
        error.to_string(),
        "Session is not attached; attach before sending session commands."
    );
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn live_acp_session_journals_conversation_before_termination() {
    let _test_lock = lock_tests();
    let test = AcpTest::new(&[]);
    let (session, initial_events) = test.attached_session();
    test.client
        .session_send(&session.id, "measure live journal")
        .expect("prompt");
    wait_for(&initial_events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::AgentFinished { stop_reason, .. } if stop_reason == "end_turn")
        })
    });

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        test.client.journal_usage().expect("flush journal");
        let connection =
            Connection::open(test._harness.paths.journal_file()).expect("open journal");
        let reply_rows = connection
            .query_row(
                "SELECT COUNT(*) FROM events
                 WHERE session_id = ?1 AND kind = 'acp_envelope'
                   AND instr(CAST(payload AS TEXT), 'stub reply') > 0",
                [&session.id],
                |row| row.get::<_, i64>(0),
            )
            .expect("find live ACP journal row");
        let live = test
            .client
            .sessions_list()
            .expect("sessions list")
            .into_iter()
            .find(|row| row.id == session.id)
            .is_some_and(|row| matches!(row.state, devboule_protocol::SessionState::Live { .. }));
        if reply_rows > 0 && live {
            eprintln!(
                "premise measurement: live session {} has {} journaled stub-reply ACP row(s)",
                session.id, reply_rows
            );
            break;
        }
        assert!(
            Instant::now() < deadline,
            "live ACP conversation was not journaled while session remained live"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    test.client
        .session_detach(&session.id)
        .expect("detach before live reattach");
    let reattached_events = Arc::new(Mutex::new(Vec::<SessionEvent>::new()));
    let reattached_received = Arc::clone(&reattached_events);
    let reattached_handler: EventHandler = Arc::new(move |envelope| {
        reattached_received
            .lock()
            .expect("reattached events lock")
            .push(envelope.event);
    });
    test.client
        .session_attach(&session.id, None, reattached_handler)
        .expect("reattach live session");
    wait_for(&reattached_events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::AgentMessage { text, .. } if text == "stub reply")
        })
    });
    let reattached_events = reattached_events.lock().expect("reattached events lock");
    assert!(reattached_events.iter().any(|event| {
        matches!(event, SessionEvent::AgentUserMessage { text, .. } if text == "measure live journal")
    }));
    drop(reattached_events);

    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn acp_non_echo_provider_gets_prompt_recorded_and_replayed_in_order() {
    let _test_lock = lock_tests();
    let test = AcpTest::new(&["--no-user-echo"]);
    let (session, events) = test.attached_session();
    let prompt = "the provider does not echo this";
    test.client
        .session_send(&session.id, prompt)
        .expect("prompt");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::AgentFinished { stop_reason, .. } if stop_reason == "end_turn")
        })
    });
    let live = events.lock().expect("events lock").clone();
    let user_index = live
        .iter()
        .position(
            |event| matches!(event, SessionEvent::AgentUserMessage { text, .. } if text == prompt),
        )
        .expect("daemon must publish the prompt even when the provider does not echo");
    let reply_index = live
        .iter()
        .position(|event| matches!(event, SessionEvent::AgentMessage { text, .. } if text == "stub reply"))
        .expect("stub reply");
    assert!(
        user_index < reply_index,
        "prompt must precede its reply: {live:?}"
    );

    test.client.journal_usage().expect("flush journal");
    test.client
        .session_detach(&session.id)
        .expect("detach before replay");
    let replayed = Arc::new(Mutex::new(Vec::<SessionEvent>::new()));
    let received = Arc::clone(&replayed);
    test.client
        .session_attach(
            &session.id,
            None,
            Arc::new(move |envelope| {
                received
                    .lock()
                    .expect("replayed events lock")
                    .push(envelope.event);
            }),
        )
        .expect("reattach");
    wait_for(&replayed, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::AgentMessage { text, .. } if text == "stub reply")
        })
    });
    let replayed = replayed.lock().expect("replayed events lock").clone();
    let user_index = replayed
        .iter()
        .position(
            |event| matches!(event, SessionEvent::AgentUserMessage { text, .. } if text == prompt),
        )
        .expect("prompt must be journaled for replay");
    let reply_index = replayed
        .iter()
        .position(|event| matches!(event, SessionEvent::AgentMessage { text, .. } if text == "stub reply"))
        .expect("replayed stub reply");
    assert!(
        user_index < reply_index,
        "replayed prompt must precede reply: {replayed:?}"
    );
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn acp_echoing_provider_gets_one_synthesized_user_message_without_echo_row() {
    let _test_lock = lock_tests();
    let test = AcpTest::new(&[]);
    let (session, events) = test.attached_session();
    let prompt = "the provider echoes this";
    test.client
        .session_send(&session.id, prompt)
        .expect("prompt");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::AgentFinished { stop_reason, .. } if stop_reason == "end_turn")
        })
    });
    let users = events
        .lock()
        .expect("events lock")
        .iter()
        .filter_map(|event| match event {
            SessionEvent::AgentUserMessage {
                message_id, text, ..
            } if text == prompt => Some(message_id.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(
        users.len(),
        1,
        "exactly one user bubble must reach the client"
    );
    assert!(
        users[0].is_some(),
        "the daemon-owned bubble needs a stable id"
    );

    test.client.journal_usage().expect("flush journal");
    let connection = Connection::open(test._harness.paths.journal_file()).expect("open journal");
    let echoed_rows = connection
        .query_row(
            "SELECT COUNT(*) FROM events
             WHERE session_id = ?1 AND kind = 'acp_envelope'
               AND instr(CAST(payload AS TEXT), 'user_message_chunk') > 0",
            [&session.id],
            |row| row.get::<_, i64>(0),
        )
        .expect("count echo rows");
    assert_eq!(
        echoed_rows, 0,
        "the redundant provider echo must not be journaled"
    );
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn acp_second_prompt_while_first_streams_precedes_both_replies() {
    let _test_lock = lock_tests();
    let test = AcpTest::new(&["--stream-first"]);
    let (session, events) = test.attached_session();
    test.client
        .session_send(&session.id, "first queued prompt")
        .expect("first prompt");
    wait_for(&events, Duration::from_secs(2), |events| {
        events.iter().any(
            |event| matches!(event, SessionEvent::AgentThought { text, .. } if text == "thinking"),
        )
    });
    test.client
        .session_send(&session.id, "second queued prompt")
        .expect("second prompt while first streams");
    wait_for(&events, Duration::from_secs(5), |events| {
        events
            .iter()
            .filter(|event| matches!(event, SessionEvent::AgentMessage { text, .. } if text == "stub reply"))
            .count()
            >= 2
    });
    let events = events.lock().expect("events lock").clone();
    let first_user = events
        .iter()
        .position(|event| matches!(event, SessionEvent::AgentUserMessage { text, .. } if text == "first queued prompt"))
        .expect("first prompt event");
    let second_user = events
        .iter()
        .position(|event| matches!(event, SessionEvent::AgentUserMessage { text, .. } if text == "second queued prompt"))
        .expect("second prompt event");
    let replies = events
        .iter()
        .enumerate()
        .filter_map(|(index, event)| {
            matches!(event, SessionEvent::AgentMessage { text, .. } if text == "stub reply")
                .then_some(index)
        })
        .collect::<Vec<_>>();
    assert_eq!(
        replies.len(),
        2,
        "expected one reply per prompt: {events:?}"
    );
    assert!(
        first_user < second_user && second_user < replies[0] && replies[0] < replies[1],
        "queued turn order was not prompt1 < prompt2 < reply1 < reply2: {events:?}"
    );
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn acp_attention_raises_for_finish_and_permission_transitions() {
    let _test_lock = lock_tests();
    let test = AcpTest::new(&["--no-malformed"]);
    let (session, events) = test.attached_session();
    let snapshots = Arc::new(Mutex::new(Vec::<Vec<SessionStateSnapshot>>::new()));
    test.client
        .sessions_watch(collect_state_handler(Arc::clone(&snapshots)))
        .expect("watch sessions");

    test.client
        .session_send(&session.id, "normal attention turn")
        .expect("normal prompt");
    wait_for(&events, Duration::from_secs(5), |events| {
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::AgentFinished { .. }))
    });
    wait_for_attention(&snapshots, &session.id, AttentionReason::Finished);

    test.client
        .session_send(&session.id, "permission attention turn")
        .expect("permission prompt");
    wait_for(&events, Duration::from_secs(5), |events| {
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::PermissionRequest { .. }))
    });
    wait_for_attention(&snapshots, &session.id, AttentionReason::Permission);
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn acp_attention_raises_error_for_a_real_agent_error_transition() {
    let _test_lock = lock_tests();
    let test = AcpTest::new(&[]);
    let (session, events) = test.attached_session();
    let snapshots = Arc::new(Mutex::new(Vec::<Vec<SessionStateSnapshot>>::new()));
    test.client
        .sessions_watch(collect_state_handler(Arc::clone(&snapshots)))
        .expect("watch sessions");
    test.client
        .session_send(&session.id, "malformed error attention turn")
        .expect("error prompt");
    wait_for(&events, Duration::from_secs(5), |events| {
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::AgentError { .. }))
    });
    wait_for_attention(&snapshots, &session.id, AttentionReason::Error);
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn acp_attention_is_suppressed_by_visible_focus() {
    let _test_lock = lock_tests();
    let test = AcpTest::new(&[]);
    let (session, events) = test.attached_session();
    let snapshots = Arc::new(Mutex::new(Vec::<Vec<SessionStateSnapshot>>::new()));
    test.client
        .sessions_watch(collect_state_handler(Arc::clone(&snapshots)))
        .expect("watch sessions");
    test.client
        .session_presence(Some(&session.id), true)
        .expect("visible focus");
    test.client
        .session_send(&session.id, "focused attention turn")
        .expect("prompt");
    wait_for(&events, Duration::from_secs(5), |events| {
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::AgentFinished { .. }))
    });
    std::thread::sleep(Duration::from_millis(250));
    assert!(!snapshots
        .lock()
        .expect("state snapshots lock")
        .iter()
        .any(|snapshot| {
            snapshot
                .iter()
                .any(|entry| entry.id == session.id && entry.attention.is_some())
        }));
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn acp_attention_clears_on_focus_and_is_raised_when_app_is_not_visible() {
    let _test_lock = lock_tests();
    let test = AcpTest::new(&["--no-malformed"]);
    let (session, events) = test.attached_session();
    let snapshots = Arc::new(Mutex::new(Vec::<Vec<SessionStateSnapshot>>::new()));
    test.client
        .sessions_watch(collect_state_handler(Arc::clone(&snapshots)))
        .expect("watch sessions");

    // The frontend sends null when the document is not visible, so this is
    // the actual payload used for the minimised/background case.
    test.client
        .session_presence(None, false)
        .expect("background presence");
    test.client
        .session_send(&session.id, "background attention turn")
        .expect("prompt");
    wait_for(&events, Duration::from_secs(5), |events| {
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::AgentFinished { .. }))
    });
    wait_for_attention(&snapshots, &session.id, AttentionReason::Finished);
    let before_clear = snapshots.lock().expect("state snapshots lock").len();
    test.client
        .session_presence(Some(&session.id), true)
        .expect("focus acknowledges attention");
    wait_for_cleared_attention(&snapshots, &session.id, before_clear);
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

#[test]
fn acp_attention_presence_is_per_connection() {
    let _test_lock = lock_tests();
    let test = AcpTest::new(&[]);
    let (session, events) = test.attached_session();
    let second = test._harness.client_named("second");
    let snapshots = Arc::new(Mutex::new(Vec::<Vec<SessionStateSnapshot>>::new()));
    test.client
        .sessions_watch(collect_state_handler(Arc::clone(&snapshots)))
        .expect("watch sessions");
    test.client
        .session_presence(Some(&session.id), true)
        .expect("first connection focus");
    second
        .session_presence(Some("s.other.1"), true)
        .expect("second connection focuses elsewhere");
    test.client
        .session_send(&session.id, "multi connection attention turn")
        .expect("prompt");
    wait_for(&events, Duration::from_secs(5), |events| {
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::AgentFinished { .. }))
    });
    std::thread::sleep(Duration::from_millis(250));
    assert!(!snapshots
        .lock()
        .expect("state snapshots lock")
        .iter()
        .any(|snapshot| {
            snapshot
                .iter()
                .any(|entry| entry.id == session.id && entry.attention.is_some())
        }));
    test.client
        .session_close(&session.id)
        .expect("close ACP session");
}

struct AcpTest {
    observation_dir: PathBuf,
    _harness: Harness,
    client: Arc<DaemonClient>,
    _env: EnvGuard,
}

impl AcpTest {
    fn new(extra_args: &[&str]) -> Self {
        Self::new_with_reject(extra_args, false)
    }

    fn new_rejecting_set_model() -> Self {
        Self::new_with_reject(&[], true)
    }

    /// The stub answers the resume's `session/load` with the ACP
    /// ResourceNotFound code naming the requested session — the field's own
    /// "I do not have this session".
    fn new_refusing_load() -> Self {
        Self::new_with_options(
            &["--refuse-load"],
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
        )
    }

    /// The stub answers `session/load` with ResourceNotFound naming a
    /// DIFFERENT resource (a gone workspace directory): the schema's generic
    /// resource miss, with the far session possibly alive.
    fn new_refusing_load_other_resource() -> Self {
        Self::new_with_options(
            &["--refuse-load-other"],
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
        )
    }

    /// The stub delays its `session/load` answer long enough for a second
    /// client's attach to land inside the spawn window, then refuses with
    /// the field's disown.
    fn new_slow_refusing_load() -> Self {
        Self::new_with_options(
            &["--refuse-load", "--delay-load-2000"],
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
        )
    }

    /// The stub refuses the FIRST `session/load` and honours the second:
    /// the mark a refusal leaves can then meet the resume that proves it
    /// stale.
    fn new_refusing_load_once() -> Self {
        Self::new_with_options(
            &["--refuse-load-once"],
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
        )
    }

    /// The stub leaves without answering `session/load`: a transport failure
    /// that says nothing about the far session.
    fn new_exiting_on_load() -> Self {
        Self::new_with_options(
            &["--exit-on-load"],
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
        )
    }

    fn new_without_set_model_push() -> Self {
        Self::new_with_options(&[], false, true, false, false, false, false, false, false)
    }

    /// ACP v2 `configOptions` peer (claude-agent-acp 0.76 wire shape).
    /// `wrong_config_value` makes the agent report a DIFFERENT current model
    /// than requested, proving the daemon displays the agent's answer.
    fn new_config_options(wrong_config_value: bool) -> Self {
        Self::new_config_options_with(wrong_config_value, false, false)
    }

    /// `load_modes_only`: the reattach reply (session/load) carries modes
    /// only, so the restart records no switch shape. `malformed_config_reply`:
    /// set_config_option answers JSON-RPC success with no parseable catalog.
    /// The config-options stub with a vendor-authored dial beside the two
    /// switches (`--feature-option`), optionally advertising
    /// `sessionCapabilities.close` so a test can tell the two cleanup answers
    /// apart. A real provider process: the only way to ask what the probe
    /// returns and what it does on the way out.
    fn new_feature_probe(advertise_close: bool) -> Self {
        let args: Vec<&str> = if advertise_close {
            vec!["--config-options", "--feature-option", "--advertise-close"]
        } else {
            vec!["--config-options", "--feature-option"]
        };
        Self::new_with_options(&args, false, false, false, false, true, false, false, false)
    }

    fn new_config_options_with(
        wrong_config_value: bool,
        load_modes_only: bool,
        malformed_config_reply: bool,
    ) -> Self {
        Self::new_with_options(
            &[],
            false,
            false,
            false,
            false,
            true,
            wrong_config_value,
            load_modes_only,
            malformed_config_reply,
        )
    }

    fn new_modes_only_then_vendor_push() -> Self {
        Self::new_with_options(
            &["--load-models-push"],
            false,
            false,
            false,
            false,
            true,
            false,
            true,
            false,
        )
    }

    fn new_hybrid_config(reject_config_once: bool, vendor_mismatch: bool) -> Self {
        let mut args = vec!["--hybrid-config-options"];
        if reject_config_once {
            args.push("--reject-config-once");
        }
        if vendor_mismatch {
            args.push("--hybrid-vendor-mismatch");
        }
        Self::new_with_options(
            &args, false, false, false, false, false, false, false, false,
        )
    }

    fn new_hybrid_effort_only() -> Self {
        Self::new_with_options(
            &["--hybrid-effort-only"],
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
        )
    }

    fn new_categoryless_options() -> Self {
        Self::new_with_options(
            &["--categoryless-options"],
            false,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
        )
    }

    fn new_vendor_no_meta() -> Self {
        Self::new_with_options(
            &["--set-model-no-meta"],
            false,
            true,
            false,
            false,
            false,
            false,
            false,
            false,
        )
    }

    fn new_without_target_efforts() -> Self {
        Self::new_with_options(
            &["--no-target-efforts"],
            false,
            true,
            false,
            false,
            false,
            false,
            false,
            false,
        )
    }

    fn new_with_catalog_default_push() -> Self {
        Self::new_with_options(&[], false, false, true, false, false, false, false, false)
    }

    fn new_with_sessions_changed() -> Self {
        Self::new_with_options(&[], false, true, false, true, false, false, false, false)
    }

    fn new_with_reject(extra_args: &[&str], reject_set_model: bool) -> Self {
        Self::new_with_options(
            extra_args,
            reject_set_model,
            false,
            false,
            false,
            false,
            false,
            false,
            false,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn new_with_options(
        extra_args: &[&str],
        reject_set_model: bool,
        no_set_model_push: bool,
        catalog_default_push: bool,
        sessions_changed: bool,
        config_options: bool,
        wrong_config_value: bool,
        load_modes_only: bool,
        malformed_config_reply: bool,
    ) -> Self {
        let observation_dir = unique_dir();
        let pid_file = observation_dir.join("stub pid.txt");
        let console_file = observation_dir.join("stub console.txt");
        let set_model_file = observation_dir.join("stub set model.txt");
        let set_model_effort_file = observation_dir.join("stub set model effort.txt");
        let set_config_file = observation_dir.join("stub set config.txt");
        let mut argv = vec![stub_bin().to_string_lossy().into_owned()];
        if config_options {
            argv.push("--config-options".to_string());
        }
        argv.extend(extra_args.iter().map(|arg| (*arg).to_string()));
        let command = serde_json::to_string(&argv).expect("ACP argv");
        std::env::set_var("DEVBOULE_ACP_COMMAND", command);
        std::env::set_var("DEVBOULE_ACP_PROVIDER_ID", "devboule-acp-stub");
        std::env::set_var("DEVBOULE_TEST_NO_NETWORK", "1");
        std::env::set_var("DEVBOULE_ACP_STUB_PID_FILE", &pid_file);
        std::env::set_var("DEVBOULE_ACP_STUB_CONSOLE_FILE", &console_file);
        std::env::set_var("DEVBOULE_ACP_STUB_SET_MODEL_FILE", &set_model_file);
        std::env::set_var(
            "DEVBOULE_ACP_STUB_SET_MODEL_EFFORT_FILE",
            &set_model_effort_file,
        );
        let session_close_file = observation_dir.join("stub session close.txt");
        std::env::set_var("DEVBOULE_ACP_STUB_CLOSE_FILE", &session_close_file);
        std::env::set_var("DEVBOULE_ACP_STUB_SET_CONFIG_FILE", &set_config_file);
        let mut env_names = vec![
            "DEVBOULE_ACP_COMMAND",
            "DEVBOULE_ACP_PROVIDER_ID",
            "DEVBOULE_TEST_NO_NETWORK",
            "DEVBOULE_ACP_STUB_PID_FILE",
            "DEVBOULE_ACP_STUB_CONSOLE_FILE",
            "DEVBOULE_ACP_STUB_SET_MODEL_FILE",
            "DEVBOULE_ACP_STUB_SET_MODEL_EFFORT_FILE",
            "DEVBOULE_ACP_STUB_CLOSE_FILE",
            "DEVBOULE_ACP_STUB_SET_CONFIG_LOG_FILE",
            "DEVBOULE_ACP_STUB_SET_CONFIG_FILE",
        ];
        if reject_set_model {
            std::env::set_var("DEVBOULE_STUB_REJECT_SET_MODEL", "1");
            env_names.push("DEVBOULE_STUB_REJECT_SET_MODEL");
        }
        if no_set_model_push {
            std::env::set_var("DEVBOULE_STUB_SET_MODEL_NO_PUSH", "1");
            env_names.push("DEVBOULE_STUB_SET_MODEL_NO_PUSH");
        }
        if catalog_default_push {
            std::env::set_var("DEVBOULE_STUB_SET_MODEL_CATALOG_DEFAULT_PUSH", "1");
            env_names.push("DEVBOULE_STUB_SET_MODEL_CATALOG_DEFAULT_PUSH");
        }
        if sessions_changed {
            std::env::set_var("DEVBOULE_STUB_SET_MODEL_SESSIONS_CHANGED", "1");
            env_names.push("DEVBOULE_STUB_SET_MODEL_SESSIONS_CHANGED");
        }
        if wrong_config_value {
            std::env::set_var("DEVBOULE_STUB_CONFIG_WRONG_VALUE", "1");
            env_names.push("DEVBOULE_STUB_CONFIG_WRONG_VALUE");
        }
        if load_modes_only {
            std::env::set_var("DEVBOULE_STUB_LOAD_MODES_ONLY", "1");
            env_names.push("DEVBOULE_STUB_LOAD_MODES_ONLY");
        }
        if malformed_config_reply {
            std::env::set_var("DEVBOULE_STUB_CONFIG_MALFORMED_REPLY", "1");
            env_names.push("DEVBOULE_STUB_CONFIG_MALFORMED_REPLY");
        }
        if extra_args.contains(&"--reject-config-once") {
            std::env::set_var("DEVBOULE_STUB_REJECT_CONFIG_ONCE", "1");
            env_names.push("DEVBOULE_STUB_REJECT_CONFIG_ONCE");
        }
        if extra_args.contains(&"--refuse-load") {
            std::env::set_var("DEVBOULE_STUB_REFUSE_LOAD", "1");
            env_names.push("DEVBOULE_STUB_REFUSE_LOAD");
        }
        if extra_args.contains(&"--delay-load-2000") {
            std::env::set_var("DEVBOULE_STUB_DELAY_LOAD_MS", "2000");
            env_names.push("DEVBOULE_STUB_DELAY_LOAD_MS");
        }
        if extra_args.contains(&"--refuse-load-once") {
            std::env::set_var(
                "DEVBOULE_STUB_REFUSE_LOAD_ONCE",
                observation_dir.join("refused once"),
            );
            env_names.push("DEVBOULE_STUB_REFUSE_LOAD_ONCE");
        }
        if extra_args.contains(&"--refuse-load-other") {
            std::env::set_var("DEVBOULE_STUB_REFUSE_LOAD_OTHER", "1");
            env_names.push("DEVBOULE_STUB_REFUSE_LOAD_OTHER");
        }
        if extra_args.contains(&"--exit-on-load") {
            std::env::set_var("DEVBOULE_STUB_EXIT_ON_LOAD", "1");
            env_names.push("DEVBOULE_STUB_EXIT_ON_LOAD");
        }
        if extra_args.contains(&"--hybrid-vendor-mismatch") {
            std::env::set_var("DEVBOULE_STUB_HYBRID_VENDOR_MISMATCH", "1");
            env_names.push("DEVBOULE_STUB_HYBRID_VENDOR_MISMATCH");
        }
        if extra_args.contains(&"--set-model-no-meta") {
            std::env::set_var("DEVBOULE_STUB_SET_MODEL_NO_META", "1");
            env_names.push("DEVBOULE_STUB_SET_MODEL_NO_META");
        }
        let env = EnvGuard { names: env_names };
        let harness = Harness::spawn();
        let client = Arc::new(harness.client());
        Self {
            observation_dir,
            _harness: harness,
            client,
            _env: env,
        }
    }

    fn create_session(&self) -> devboule_protocol::Session {
        let session = self
            .client
            .session_create(None, SessionKind::Acp, None)
            .expect("create ACP session");
        assert_eq!(session.kind, SessionKind::Acp);
        assert_eq!(session.title, "Agent");
        session
    }

    fn attached_session(&self) -> (devboule_protocol::Session, Arc<Mutex<Vec<SessionEvent>>>) {
        let session = self.create_session();
        let events = Arc::new(Mutex::new(Vec::<SessionEvent>::new()));
        let received = Arc::clone(&events);
        let handler: EventHandler = Arc::new(move |envelope| {
            received.lock().expect("events lock").push(envelope.event);
        });
        self.client
            .session_attach(&session.id, None, handler)
            .expect("attach ACP session");
        (session, events)
    }

    fn attached_existing(
        &self,
        session_id: &str,
    ) -> (devboule_protocol::Session, Arc<Mutex<Vec<SessionEvent>>>) {
        let session = self
            .client
            .sessions_list()
            .expect("list sessions")
            .into_iter()
            .find(|listed| listed.id == session_id)
            .unwrap_or_else(|| panic!("session {session_id} missing after restart"));
        let events = Arc::new(Mutex::new(Vec::<SessionEvent>::new()));
        let received = Arc::clone(&events);
        let handler: EventHandler = Arc::new(move |envelope| {
            received.lock().expect("events lock").push(envelope.event);
        });
        self.client
            .session_attach(&session.id, None, handler)
            .expect("attach recovered ACP session");
        (session, events)
    }

    fn restart(&mut self) {
        self._harness.restart();
        self.client = Arc::new(self._harness.client_named("restarted"));
    }

    fn pid_file(&self) -> PathBuf {
        self.observation_dir.join("stub pid.txt")
    }

    fn console_file(&self) -> PathBuf {
        self.observation_dir.join("stub console.txt")
    }

    fn set_model_file(&self) -> PathBuf {
        self.observation_dir.join("stub set model.txt")
    }

    fn set_model_effort_file(&self) -> PathBuf {
        self.observation_dir.join("stub set model effort.txt")
    }

    /// Where the stub records the **session** it was asked to close.
    fn session_close_file(&self) -> PathBuf {
        self.observation_dir.join("stub session close.txt")
    }

    fn set_config_file(&self) -> PathBuf {
        self.observation_dir.join("stub set config.txt")
    }

    fn journal_event_count(&self, session_id: &str) -> i64 {
        self.client.journal_usage().expect("flush journal");
        let connection =
            Connection::open(self._harness.paths.journal_file()).expect("open journal");
        connection
            .query_row(
                "SELECT COUNT(*) FROM events WHERE session_id = ?1",
                [session_id],
                |row| row.get(0),
            )
            .expect("count journal events")
    }
}

impl Drop for AcpTest {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.observation_dir);
    }
}

fn process_is_in_job(pid: u32) -> bool {
    let handle = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
            0,
            pid,
        )
    };
    assert!(!handle.is_null(), "could not open ACP child {pid}");
    let mut in_job = 0;
    let result = unsafe { IsProcessInJob(handle, std::ptr::null_mut(), &mut in_job) } != 0;
    unsafe { CloseHandle(handle) };
    result
}

#[test]
fn acp_mute_prompt_times_out_instead_of_waiting_forever() {
    let _test_lock = lock_tests();
    std::env::set_var("DEVBOULE_ACP_TURN_TIMEOUT_MS", "800");
    struct ClearTimeout;
    impl Drop for ClearTimeout {
        fn drop(&mut self) {
            std::env::remove_var("DEVBOULE_ACP_TURN_TIMEOUT_MS");
        }
    }
    let _clear = ClearTimeout;
    let test = AcpTest::new(&[]);
    let (session, events) = test.attached_session();
    test.client
        .session_send(&session.id, "block until cancelled")
        .expect("blocked prompt");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::AgentError { message } if message.contains("stayed silent"))
                || matches!(event, SessionEvent::AgentFinished { stop_reason, .. } if stop_reason == "cancelled")
        })
    });
    std::env::remove_var("DEVBOULE_ACP_TURN_TIMEOUT_MS");
    test.client
        .session_close(&session.id)
        .expect("close timed-out ACP session");
}

#[test]
#[ignore = "talks to a live grok agent; costs real tokens"]
fn grok_prompt_completes_with_fragments_and_end_turn() {
    let _test_lock = lock_tests();
    let argv = grok_acp_argv().expect("grok.exe on PATH");
    std::env::set_var(
        "DEVBOULE_ACP_COMMAND",
        serde_json::to_string(&argv).expect("argv"),
    );
    let harness = Harness::spawn();
    let client = harness.client();
    let session = client
        .session_create(None, SessionKind::Acp, None)
        .expect("create grok ACP session");
    let events = Arc::new(Mutex::new(Vec::<SessionEvent>::new()));
    let received = Arc::clone(&events);
    let handler: EventHandler = Arc::new(move |envelope| {
        received.lock().expect("events lock").push(envelope.event);
    });
    client
        .session_attach(&session.id, None, handler)
        .expect("attach grok");
    client
        .session_send(&session.id, "Reply with exactly one word: PONG")
        .expect("prompt grok");
    wait_for(&events, Duration::from_secs(60), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::AgentFinished { stop_reason, .. } if stop_reason == "end_turn")
        })
    });
    let snapshot = events.lock().expect("events lock");
    let message: String = snapshot
        .iter()
        .filter_map(|event| match event {
            SessionEvent::AgentMessage { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    assert!(
        message.contains("PONG"),
        "grok message fragments were {message:?}; events={snapshot:?}"
    );
    assert!(
        snapshot
            .iter()
            .any(|event| matches!(event, SessionEvent::AgentThought { .. })),
        "grok did not stream thought fragments: {snapshot:?}"
    );
    let thoughts = snapshot
        .iter()
        .filter(|event| matches!(event, SessionEvent::AgentThought { .. }))
        .count();
    let finished = snapshot.iter().find_map(|event| match event {
        SessionEvent::AgentFinished {
            stop_reason,
            model_id,
            usage,
        } => Some((stop_reason.as_str(), model_id.clone(), usage.clone())),
        _ => None,
    });
    let view_bytes: usize = snapshot
        .iter()
        .map(|event| {
            serde_json::to_vec(event)
                .map(|bytes| bytes.len())
                .unwrap_or(0)
        })
        .sum();
    eprintln!(
        "grok turn: thought_chunks={thoughts} message={message:?} finished={finished:?} view_bytes={view_bytes} events={}",
        snapshot.len()
    );
    drop(snapshot);
    client
        .session_close(&session.id)
        .expect("close grok session");
    let journal_path = harness.paths.journal_file();
    for suffix in ["", "-wal", "-shm"] {
        let path = PathBuf::from(format!("{}{suffix}", journal_path.display()));
        if let Ok(meta) = std::fs::metadata(&path) {
            eprintln!("grok journal {} bytes at {}", meta.len(), path.display());
        }
    }
    std::env::remove_var("DEVBOULE_ACP_COMMAND");
}

fn grok_acp_argv() -> Option<Vec<String>> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join("grok.exe");
        if candidate.is_file() {
            return Some(vec![
                candidate.to_string_lossy().into_owned(),
                "agent".to_string(),
                "stdio".to_string(),
            ]);
        }
    }
    None
}

fn wait_until_gone(pid: u32) {
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        return;
    }
    let result = unsafe { WaitForSingleObject(handle, 5_000) };
    unsafe { CloseHandle(handle) };
    assert_eq!(result, WAIT_OBJECT_0, "ACP child remained after close");
}

// ---------------------------------------------------------------------------
// Slice 5 end to end: an agent creates an agent, through the daemon's own MCP
// broker, with the stub as the provider on both sides.
//
// Two stub capabilities make this possible, both behind knobs that no other
// test sets: it declares one ACP mode and implements `session/set_mode`
// (`DEVBOULE_STUB_MODES_DEFAULT`), and it calls the MCP endpoint with the
// Bearer it was handed (`DEVBOULE_ACP_STUB_MCP_CALL`). The second is what turns
// a *session* into the caller of `devboule_create_agent` — the caller identity
// under test is the Bearer's session, not a client connection.
//
// Each test's daemon gets its own environment through `Harness::spawn_with_env`:
// the knobs below are the daemon's own (and its providers'), never this
// process's, so the battery cannot race a test that does not hold the lock.
// ---------------------------------------------------------------------------

struct Slice5Test {
    dir: PathBuf,
    harness: Harness,
    client: DaemonClient,
}

impl Drop for Slice5Test {
    fn drop(&mut self) {
        // The observation files live here and `Harness` removes only its own
        // runtime directory: this is the one fixture that used to leak a
        // directory per construction (measured: 28 per suite run).
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// One profile, as the Settings form saves it: the stub provider, the model
/// the stub publishes **but does not start on** (`stub-model-new`; the stub's
/// own default is `stub-model`), the mode the preset cells used to name
/// (`default` — the stub declares `ask,default`), the overlay the caller
/// passes, and ticked for agents.
///
/// The saved model differing from the provider's default is load-bearing: an
/// assertion on the child is what makes the delivery real, and a fixture that
/// saved the default would pass even if the daemon delivered nothing. `id` is
/// spelled rather than minted (`profile-<name>`), so a test can assert the id
/// the session recorded and rename the profile while keeping it.
fn stub_profile(name: &str, overlay: &[&str]) -> serde_json::Value {
    stub_profile_with(
        name,
        &format!("profile-{name}"),
        "default",
        "stub-model-new",
        serde_json::json!({}),
        overlay,
        true,
    )
}

/// The same profile with every field a test needs to choose: the mode (a
/// `bypass`-family mode is what makes a child unattended), the model, the
/// features, the overlay and whether it is ticked for agents.
fn stub_profile_with(
    name: &str,
    id: &str,
    mode: &str,
    model: &str,
    features: serde_json::Value,
    overlay: &[&str],
    enabled: bool,
) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "name": name,
        "note": "the profile the slice-5 battery creates from",
        "provider": "devboule-acp-stub",
        "model": model,
        "modeId": mode,
        "features": features,
        "toolOverlay": overlay,
        "enabledForAgents": enabled,
    })
}

/// A document literal as the wire type the client sends.
fn profile_document(document: &serde_json::Value) -> devboule_protocol::AgentProfilesDocument {
    serde_json::from_value(document.clone()).expect("the document the settings save")
}

/// The same document with standing instructions in it.
fn with_standing_instructions(
    mut document: serde_json::Value,
    instructions: &str,
) -> serde_json::Value {
    document["standingInstructions"] = serde_json::json!(instructions);
    document
}

/// The document the battery's daemon reads at startup: one ticked `worker`
/// profile, which is what every creation in this file names unless a test says
/// otherwise. An empty `standingInstructions`, so the first prompt a creation
/// sends is the preamble and the caller's text, exactly as before this slice.
fn worker_profile_document() -> serde_json::Value {
    serde_json::json!({
        "profiles": [stub_profile("worker", &[])],
        "standingInstructions": "",
    })
}

/// The same, with the `design` profile: the stub's design cell, whose overlay is
/// what keeps a child from creating or messaging anyone.
fn design_profile_document() -> serde_json::Value {
    serde_json::json!({
        "profiles": [stub_profile(
            "design",
            &["devboule_send_message", "devboule_create_agent"],
        )],
        "standingInstructions": "",
    })
}

/// The same, with a profile naming the provider whose binary cannot exist:
/// `provider not installed` is reachable only through a profile now.
fn absent_provider_profile_document() -> serde_json::Value {
    serde_json::json!({
        "profiles": [{
            "id": "profile-nowhere",
            "name": "nowhere",
            "note": "names a provider that cannot be installed",
            "provider": "devboule-absent-probe",
            "model": "stub-default",
            "modeId": "default",
            "enabledForAgents": true,
        }],
        "standingInstructions": "",
    })
}

/// One observation file's path, as the string the daemon's environment carries.
fn file_name(dir: &Path, name: &str) -> String {
    dir.join(name).to_string_lossy().into_owned()
}

impl Slice5Test {
    /// A daemon whose provider is the stub, with `creation` as the arguments of
    /// the `devboule_create_agent` call every stub process makes as soon as its
    /// `session/new` handshake has been answered.
    fn new(creation: &serde_json::Value) -> Self {
        Self::with_profiles(creation, &worker_profile_document(), &[])
    }

    /// The same daemon, with the `design` profile instead: the one the human
    /// ticked for a child that must not create or message anyone.
    fn new_with_design_profile(creation: &serde_json::Value) -> Self {
        Self::with_profiles(creation, &design_profile_document(), &[])
    }

    /// The same daemon, with `extra` added to its environment. The extra values
    /// outlive the call: the harness copies them into the daemon's own
    /// environment, which is where its stub processes read them from.
    fn new_with_env(creation: &serde_json::Value, extra: &[(&str, &str)]) -> Self {
        Self::with_profiles(creation, &worker_profile_document(), extra)
    }

    /// The same daemon with stub **argv** flags: the shape of the agent a test
    /// needs to describe (`--config-options` for a v1 selector surface,
    /// `--feature-option` for a vendor-authored dial beside the two switches).
    /// Flags, not environment, because an agent's surface is a property of the
    /// build and this stub's knobs are argv.
    fn with_profiles_and_args(
        creation: &serde_json::Value,
        profiles: &serde_json::Value,
        extra: &[(&str, &str)],
        args: &[&str],
    ) -> Self {
        Self::with_profiles_impl(creation, profiles, extra, args)
    }

    fn with_profiles(
        creation: &serde_json::Value,
        profiles: &serde_json::Value,
        extra: &[(&str, &str)],
    ) -> Self {
        Self::with_profiles_impl(creation, profiles, extra, &[])
    }

    fn with_profiles_impl(
        creation: &serde_json::Value,
        profiles: &serde_json::Value,
        extra: &[(&str, &str)],
        args: &[&str],
    ) -> Self {
        let dir = unique_dir();
        let mut stub_argv = vec![stub_bin().to_string_lossy().into_owned()];
        stub_argv.extend(args.iter().map(|arg| (*arg).to_string()));
        let argv = serde_json::to_string(&stub_argv).expect("stub argv");
        // Owned strings first: the daemon gets these as its own environment,
        // which is where its providers read them from.
        let values = [
            ("DEVBOULE_ACP_COMMAND", argv),
            ("DEVBOULE_ACP_PROVIDER_ID", "devboule-acp-stub".to_string()),
            ("DEVBOULE_TEST_NO_NETWORK", "1".to_string()),
            ("DEVBOULE_STUB_MODES", "ask,default".to_string()),
            ("DEVBOULE_STUB_MESSAGE_AFTER_PERMISSION", "1".to_string()),
            ("DEVBOULE_ACP_STUB_MCP_DELAY_MS", "700".to_string()),
            (
                "DEVBOULE_ACP_STUB_SET_MODE_FILE",
                file_name(&dir, "set mode.txt"),
            ),
            (
                "DEVBOULE_ACP_STUB_MODES_FILE",
                file_name(&dir, "stub modes.txt"),
            ),
            (
                "DEVBOULE_ACP_STUB_SET_CONFIG_FILE",
                file_name(&dir, "set config.txt"),
            ),
            (
                "DEVBOULE_ACP_STUB_SET_CONFIG_LOG_FILE",
                file_name(&dir, "set config log.txt"),
            ),
            (
                "DEVBOULE_ACP_STUB_CLOSE_FILE",
                file_name(&dir, "session close.txt"),
            ),
            (
                "DEVBOULE_ACP_STUB_SET_MODEL_FILE",
                file_name(&dir, "set model.txt"),
            ),
            (
                "DEVBOULE_ACP_STUB_SET_MODEL_EFFORT_FILE",
                file_name(&dir, "set model effort.txt"),
            ),
            (
                "DEVBOULE_ACP_STUB_STDIN_FILE",
                file_name(&dir, "stub stdin.txt"),
            ),
            (
                "DEVBOULE_ACP_STUB_STDOUT_FILE",
                file_name(&dir, "stub stdout.txt"),
            ),
            (
                "DEVBOULE_ACP_STUB_MCP_TOOLS_FILE",
                file_name(&dir, "mcp tools.txt"),
            ),
            (
                "DEVBOULE_ACP_STUB_MCP_CALL",
                "devboule_create_agent".to_string(),
            ),
            ("DEVBOULE_ACP_STUB_MCP_CALL_ARGUMENTS", creation.to_string()),
            (
                "DEVBOULE_ACP_STUB_MCP_CALL_FILE",
                file_name(&dir, "mcp calls.txt"),
            ),
            (
                "DEVBOULE_ACP_STUB_PIDS_FILE",
                file_name(&dir, "stub pids.txt"),
            ),
            (
                "DEVBOULE_ACP_STUB_ARGV_FILE",
                file_name(&dir, "stub argv.txt"),
            ),
        ];
        let env: Vec<(&str, &str)> = values
            .iter()
            .map(|(key, value)| (*key, value.as_str()))
            .chain(extra.iter().copied())
            .collect();
        let harness = Harness::spawn_with_env_and_profiles(&env, Some(profiles));
        let client = harness.client();
        Self {
            dir,
            harness,
            client,
        }
    }

    /// One observation file's lines, empty when the file is not there yet.
    fn observations(&self, name: &str) -> Vec<String> {
        std::fs::read_to_string(self.dir.join(name))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    fn wait_for_observations(&self, name: &str, count: usize) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            let lines = self.observations(name);
            if lines.len() >= count {
                return lines;
            }
            assert!(
                Instant::now() < deadline,
                "{name} held {} lines, wanted {count}: {lines:?}",
                lines.len()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// The observation lines one stub process wrote, by the pid every line
    /// starts with.
    ///
    /// Order is not evidence here: the two stubs write to the same file and
    /// which of them gets there first depends on how fast the daemon answers a
    /// card, so every assertion names the process it is talking about.
    fn lines_of(&self, name: &str, pid: &str) -> Vec<String> {
        self.observations(name)
            .into_iter()
            .filter(|line| line.starts_with(&format!("{pid} ")))
            .collect()
    }

    /// The same, waiting for at least `count` such lines.
    fn wait_for_lines_of(&self, name: &str, pid: &str, count: usize) -> Vec<String> {
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            let lines = self.lines_of(name, pid);
            if lines.len() >= count {
                return lines;
            }
            assert!(
                Instant::now() < deadline,
                "{name} has {} lines for pid {pid}, wanted {count}",
                lines.len()
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// The files the daemon's attachment store holds for one session.
    fn stored_attachments(&self, session_id: &str) -> Vec<PathBuf> {
        let mut files: Vec<PathBuf> =
            std::fs::read_dir(self.harness.dir.join("attachments").join(session_id))
                .map(|entries| {
                    entries
                        .filter_map(Result::ok)
                        .map(|entry| entry.path())
                        .collect()
                })
                .unwrap_or_default();
        files.sort();
        files
    }

    fn creator_session(&self) -> devboule_protocol::Session {
        let session = self
            .client
            .session_create(None, SessionKind::Acp, None)
            .expect("the creator's own session");
        assert_eq!(session.kind, SessionKind::Acp);
        assert_eq!(session.created_by, None, "a human's session has no parent");
        session
    }

    fn attach(&self, session: &devboule_protocol::Session) -> Arc<Mutex<Vec<SessionEvent>>> {
        let events = Arc::new(Mutex::new(Vec::<SessionEvent>::new()));
        let received = Arc::clone(&events);
        let handler: EventHandler = Arc::new(move |envelope| {
            // Poison-tolerant on purpose: a panic in another thread that held
            // this lock must not silently stop the subscription, or a test
            // would read "no event arrived" from a handler that died.
            received
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(envelope.event);
        });
        self.client
            .session_attach(&session.id, None, handler)
            .expect("attach the creator");
        events
    }

    /// Attach to a created child once its own provider has reached the broker.
    ///
    /// The child's journal row lists it before the spawn has finished, so an
    /// attach that lands in that window reads the journal once — a recovered
    /// transcript — and is never told about the live session's events. The
    /// child's own `tools/list` observation is the first fact its process
    /// produces after the handshake completed and the session is live, so it is
    /// what this waits for; from there the attach replays everything journaled
    /// and receives the rest live.
    fn attach_child(&self, child: &devboule_protocol::Session) -> Arc<Mutex<Vec<SessionEvent>>> {
        let pids = self.wait_for_observations("stub pids.txt", 2);
        let child_pid = pids[1].trim().to_string();
        self.wait_for_lines_of("mcp tools.txt", &child_pid, 1);
        self.attach(child)
    }

    /// The child the creator created, as the daemon's own list reports it.
    fn child_of(&self, creator: &str) -> devboule_protocol::Session {
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            let child = self
                .client
                .sessions_list()
                .expect("session list")
                .into_iter()
                .find(|session| session.created_by.as_deref() == Some(creator));
            if let Some(child) = child {
                return child;
            }
            assert!(
                Instant::now() < deadline,
                "the creator has no child in the daemon's list"
            );
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Answer the creation card that arrived on `session_id` with an allow, and
    /// answer it once: a second card would mean the gate was not atomic.
    fn allow_creation_card(
        &self,
        session_id: &str,
        events: &Mutex<Vec<SessionEvent>>,
    ) -> CreateAgentCardFacts {
        let facts = wait_for_creation_card(events, Duration::from_secs(45));
        self.client
            .session_permission_respond(
                session_id,
                &facts.tool_call_id,
                PermissionOutcome::AllowOnce,
            )
            .expect("allow the creation");
        facts
    }
}

/// What the creation card said, and the id a decision has to carry.
struct CreateAgentCardFacts {
    tool_call_id: String,
    title: String,
    provider: String,
    /// The profile the card names: the human's word for what they are approving.
    profile: String,
    caps: devboule_protocol::CreateAgentCaps,
}

fn wait_for_creation_card(
    events: &Mutex<Vec<SessionEvent>>,
    timeout: Duration,
) -> CreateAgentCardFacts {
    let deadline = Instant::now() + timeout;
    loop {
        {
            let events = events.lock().expect("events lock");
            if let Some(facts) = events.iter().find_map(|event| match event {
                SessionEvent::PermissionRequest {
                    tool_call_id,
                    create_agent: Some(card),
                    ..
                } => Some(CreateAgentCardFacts {
                    tool_call_id: tool_call_id.clone(),
                    title: card.title.clone(),
                    provider: card.provider.clone(),
                    profile: card.profile.clone(),
                    caps: card.caps.clone(),
                }),
                _ => None,
            }) {
                return facts;
            }
        }
        assert!(
            Instant::now() < deadline,
            "no creation card arrived on the creator"
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The creation card's own text, off the creator's transcript: what the human
/// read when the decision was made. The R2a assertions hold the card against
/// the child's wire — a card a human could approve into an existing child
/// printed only what the child was delivered.
fn creation_card_description(events: &Mutex<Vec<SessionEvent>>) -> String {
    events
        .lock()
        .expect("events lock")
        .iter()
        .find_map(|event| match event {
            SessionEvent::PermissionRequest {
                description: Some(description),
                create_agent: Some(_),
                ..
            } => Some(description.clone()),
            _ => None,
        })
        .expect("the creation card's text")
}

fn slice5_events(events: &Mutex<Vec<SessionEvent>>) -> Vec<SessionEvent> {
    events.lock().expect("events lock").clone()
}

/// The first index at which `predicate` holds, for order assertions that are
/// about *when* two records were published rather than that both exist.
fn slice5_index_of<F>(events: &[SessionEvent], predicate: F) -> Option<usize>
where
    F: Fn(&SessionEvent) -> bool,
{
    events.iter().position(predicate)
}

/// The `<devboule-system>` text message whose body contains `needle`, with the
/// id the delivery gave it.
fn slice5_system_message(events: &[SessionEvent], needle: &str) -> (String, Option<String>) {
    events
        .iter()
        .rev()
        .find_map(|event| match event {
            SessionEvent::AgentUserMessage {
                message_id, text, ..
            } if text.contains("<devboule-system>") && text.contains(needle) => {
                Some((text.clone(), message_id.clone()))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("no <devboule-system> message mentioning {needle}"))
}

/// The event kinds in the order they were published, for a failure message that
/// says *when* something arrived rather than only that it did.
fn slice5_kinds(events: &[SessionEvent]) -> String {
    events
        .iter()
        .map(|event| match event {
            SessionEvent::AgentError { message } => {
                format!("error({})", message.chars().take(160).collect::<String>())
            }
            other => slice5_kind(other).to_string(),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// One event's name in that list.
fn slice5_kind(event: &SessionEvent) -> &'static str {
    match event {
        SessionEvent::AgentCreated { .. } => "created",
        SessionEvent::ChildFinished { .. } => "finished",
        SessionEvent::AgentFinished { .. } => "turn_end",
        SessionEvent::AgentError { .. } => "error",
        SessionEvent::PermissionRequest { .. } => "card",
        SessionEvent::PermissionResolved { .. } => "card_answered",
        SessionEvent::AgentUserMessage { text, .. } if text.contains("agent_finished") => {
            "text:finished"
        }
        SessionEvent::AgentUserMessage { text, .. } if text.contains("agent_input_required") => {
            "text:notice"
        }
        SessionEvent::AgentUserMessage { .. } => "text",
        _ => "other",
    }
}

/// The child's end, as the structured record states it.
fn slice5_finished(
    events: &[SessionEvent],
) -> (Option<String>, AgentTaskState, Vec<FinishArtifact>) {
    events
        .iter()
        .find_map(|event| match event {
            SessionEvent::ChildFinished {
                message_id,
                state,
                artifacts,
                ..
            } => Some((message_id.clone(), *state, artifacts.clone())),
            _ => None,
        })
        .expect("no ChildFinished on the creator")
}

/// `S5` block 6, the whole creation: a session that is a *provider's* session
/// asks the daemon for a child, the human allows it once, the child runs, and
/// both finish records come back to the creator.
#[test]
fn an_agent_creates_an_agent_and_the_finish_carries_both_records() {
    let _lock = lock_tests();
    let test = Slice5Test::new(&serde_json::json!({
        "title": "builder",
        "profile": "worker",
        "initialPrompt": "report your result",
    }));
    let creator = test.creator_session();
    let events = test.attach(&creator);
    let card = test.allow_creation_card(&creator.id, &events);
    assert_eq!(card.title, "builder", "the card names the child");
    assert_eq!(card.provider, "devboule-acp-stub");
    assert_eq!(card.profile, "worker");
    assert_eq!(card.caps.depth, 1, "a human's child is depth 1");
    assert_eq!(
        card.caps.live_children, 1,
        "the slot is counted in the card"
    );

    // The child is a session of the daemon's own list, named and parented as
    // the creation said (S5-12 read back through the wire, not the journal).
    let child = test.child_of(&creator.id);
    assert_eq!(child.display_name.as_deref(), Some("builder"));
    assert_eq!(child.created_by.as_deref(), Some(creator.id.as_str()));
    assert_eq!(child.kind, SessionKind::Acp);
    assert_eq!(child.provider.as_deref(), Some("devboule-acp-stub"));
    // `create-from-profile`: the session records the profile's **stable id** and
    // not its name, belongs to the creator's context, carries the honest marker
    // — `unknown`, because the stub's `default` mode is a vocabulary the
    // provider authored and the daemon was never told whether it asks — and
    // carries the four labels the daemon stamped.
    assert_eq!(child.profile_id.as_deref(), Some("profile-worker"));
    assert_eq!(child.context_id.as_deref(), Some(creator.id.as_str()));
    assert_eq!(
        child.unattended,
        devboule_protocol::UnattendedState::Unknown
    );
    assert_eq!(
        child.labels.get("devboule.created-by").map(String::as_str),
        Some(creator.id.as_str())
    );
    assert_eq!(
        child.labels.get("devboule.depth").map(String::as_str),
        Some("1")
    );
    assert_eq!(
        child.labels.get("devboule.origin").map(String::as_str),
        Some("local")
    );
    assert_eq!(
        child.labels.get("devboule.profile").map(String::as_str),
        Some("profile-worker")
    );
    // The preset cell's mode reached the provider: the stub writes down every
    // `session/set_mode` it is sent. The child's *row* exists before its
    // provider is even spawned (the journal row is the durable boundary), so
    // this waits for the switch rather than assuming it happened with the row.
    let announced = test.wait_for_observations("stub modes.txt", 1);
    assert!(
        announced[0].contains("\"currentModeId\":\"ask\"") && announced[0].contains("default"),
        "the stub declared a mode it was not already in, so a switch is owed: {}",
        announced[0]
    );
    let switched = test.wait_for_observations("set mode.txt", 1);
    assert_eq!(
        switched[0].trim(),
        "default",
        "the worker cell's mode is what the child was switched to"
    );

    // R2a — what the card printed, the child runs. The fixture deliberately
    // saves a model the stub does not start on (`stub-model-new`; the stub's
    // own default is `stub-model`), so an assertion on the child is what
    // makes the delivery real: a daemon that delivered nothing would be
    // caught here, because the stub writes down every `session/set_model`
    // it is sent.
    let set_model = test.wait_for_observations("set model.txt", 1);
    assert_eq!(
        set_model[0].trim(),
        "stub-model-new",
        "the child was started on the profile's model, not the provider's default: {set_model:?}"
    );
    // And the card named that model and that mode — the human approved what
    // the child was delivered, by construction.
    let description = creation_card_description(&events);
    assert!(
        description.contains("model stub-model-new, mode default"),
        "the card names the delivered model and mode: {description}"
    );
    // The stub's `default` mode is a vocabulary the provider authored, so the
    // card asserts neither direction (R2b): it says cannot-establish, which
    // is honest, and never the plain "No" the old binary card printed.
    assert!(
        description.contains("auto accept: Cannot establish"),
        "a provider-authored mode is not asserted either way: {description}"
    );

    wait_for(&events, Duration::from_secs(60), |events| {
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::ChildFinished { .. }))
    });
    let transcript = slice5_events(&events);
    let created_at = slice5_index_of(&transcript, |event| {
        matches!(event, SessionEvent::AgentCreated { .. })
    })
    .expect("AgentCreated on the creator");
    let finished_at = slice5_index_of(&transcript, |event| {
        matches!(event, SessionEvent::ChildFinished { .. })
    })
    .expect("ChildFinished on the creator");
    let (envelope, envelope_id) = slice5_system_message(&transcript, "kind: agent_finished");
    assert!(
        created_at < finished_at,
        "the creation is recorded before the finish"
    );
    assert!(
        envelope.contains("state: completed"),
        "the stub stopped with end_turn, which is completed: {envelope}\n[{}]",
        slice5_kinds(&transcript)
    );
    assert!(
        envelope.contains(&format!("childSessionId: {}", child.id)),
        "the envelope names the child: {envelope}"
    );

    // S5-04: the two records name one delivery, and that delivery is the
    // `<devboule-system>` text the creator's transcript actually holds.
    let (message_id, state, artifacts) = slice5_finished(&transcript);
    assert_eq!(
        message_id, envelope_id,
        "ChildFinished.message_id is the id of the text message beside it"
    );
    assert_eq!(state, AgentTaskState::Completed);

    // S5 decision 10: one artifact, deposited in the *creator's* folder, and it
    // is the child's own message.
    assert_eq!(artifacts.len(), 1, "one artifact: {artifacts:?}");
    let artifact = &artifacts[0];
    assert!(!artifact.artifact_id.is_empty());
    assert_eq!(artifact.parts.len(), 1);
    let part = &artifact.parts[0];
    assert!(
        part.url
            .starts_with(&format!("devboule-attachment:{}/", creator.id)),
        "the artifact is the creator's: {}",
        part.url
    );
    assert_eq!(part.mime_type, "text/markdown");
    let stored = test.stored_attachments(&creator.id);
    assert_eq!(
        stored.len(),
        1,
        "the deposit really exists in the creator's folder: {stored:?}"
    );
    let deposited = std::fs::read_to_string(&stored[0]).expect("the deposited artifact");
    assert!(
        deposited.contains("stub reply"),
        "the artifact is the child's own message: {deposited}"
    );
    assert_eq!(
        part.metadata.as_ref().map(|metadata| metadata.stored_bytes),
        Some(deposited.len() as u64),
        "the part's size is the store's size"
    );
}

/// R2a: the thinking option the card named is delivered on the same wire —
/// the effort the child's `session/set_model` request carried is what the
/// stub writes down, and the card's thinking line names the value the human
/// approved.
#[test]
fn the_child_is_delivered_the_thinking_option_the_card_named() {
    let _lock = lock_tests();
    let mut profiles = worker_profile_document();
    profiles["profiles"][0]["thinkingOptionId"] = serde_json::json!("low");
    let test = Slice5Test::with_profiles(
        &serde_json::json!({
            "title": "thinker",
            "profile": "worker",
            "initialPrompt": "report your result",
        }),
        &profiles,
        &[],
    );
    let creator = test.creator_session();
    let events = test.attach(&creator);
    test.allow_creation_card(&creator.id, &events);
    let child = test.child_of(&creator.id);
    assert_eq!(child.profile_id.as_deref(), Some("profile-worker"));

    let description = creation_card_description(&events);
    assert!(
        description.contains("thinking low"),
        "the card names the delivered thinking option: {description}"
    );
    let effort = test.wait_for_observations("set model effort.txt", 1);
    assert_eq!(
        effort[0].trim(),
        "low",
        "the child starts on the thinking option the card named: {effort:?}"
    );
}

/// R2a, model axis: a profile naming a model the agent does not publish is
/// **refused** — the mismatch sentence, not substituted with the agent's
/// default. The child never exists, and nothing was sent on the wire.
#[test]
fn a_profile_naming_a_model_the_agent_does_not_publish_is_refused() {
    let _lock = lock_tests();
    let mut profiles = worker_profile_document();
    profiles["profiles"][0]["model"] = serde_json::json!("stub-nope");
    let test = Slice5Test::with_profiles(
        &serde_json::json!({
            "title": "wrong model",
            "profile": "worker",
            "initialPrompt": "report your result",
        }),
        &profiles,
        &[],
    );
    let creator = test.creator_session();
    let events = test.attach(&creator);
    test.allow_creation_card(&creator.id, &events);

    let calls = test.wait_for_observations("mcp calls.txt", 1);
    assert!(
        calls[0].contains("stub-nope") && calls[0].contains("is not among the model values"),
        "the refusal names the id and the agent's declared values: {calls:?}"
    );
    assert!(
        test.observations("set model.txt").is_empty(),
        "nothing was sent on the wire for a refused delivery"
    );
}

/// R2a, absence versus mismatch: an agent that declares **no** model surface
/// cannot deliver any model at all, so a profile naming one is refused with
/// the absence sentence — a different answer from an unknown id in a
/// published list, because there is no list the name could have been a typo
/// from.
#[test]
fn an_agent_that_declares_no_model_surface_refuses_a_profile_with_a_model() {
    let _lock = lock_tests();
    let test = Slice5Test::with_profiles(
        &serde_json::json!({
            "title": "no models",
            "profile": "worker",
            "initialPrompt": "report your result",
        }),
        &worker_profile_document(),
        &[("DEVBOULE_STUB_NO_MODELS", "1")],
    );
    let creator = test.creator_session();
    let events = test.attach(&creator);
    test.allow_creation_card(&creator.id, &events);

    let calls = test.wait_for_observations("mcp calls.txt", 1);
    assert!(
        calls[0].contains("declares no model or effort switch surface"),
        "the absence sentence: {calls:?}"
    );
    assert!(
        !calls[0].contains("is not among the model values"),
        "absence and mismatch are two different refusals: {calls:?}"
    );
}

/// R2a, thinking axis: a thinking option outside what the agent declares for
/// the delivered model is refused with the mismatch sentence.
#[test]
fn a_thinking_option_the_agent_does_not_declare_is_refused() {
    let _lock = lock_tests();
    let mut profiles = worker_profile_document();
    profiles["profiles"][0]["thinkingOptionId"] = serde_json::json!("bogus");
    let test = Slice5Test::with_profiles(
        &serde_json::json!({
            "title": "wrong effort",
            "profile": "worker",
            "initialPrompt": "report your result",
        }),
        &profiles,
        &[],
    );
    let creator = test.creator_session();
    let events = test.attach(&creator);
    test.allow_creation_card(&creator.id, &events);

    let calls = test.wait_for_observations("mcp calls.txt", 1);
    assert!(
        calls[0].contains("bogus") && calls[0].contains("is not among the thinking options"),
        "the mismatch sentence for the thinking axis: {calls:?}"
    );
    assert!(
        test.observations("set model.txt").is_empty(),
        "nothing was delivered for a refused thinking option"
    );
}

/// R2a F3 — the switch is **confirmed**, not just sent: an agent that
/// answers the delivered `session/set_model` with an error refuses the
/// creation. Before the confirm existed, this shape ended with a live child
/// on the agent's own model, an `AgentError` on a transcript nobody reads at
/// creation time, and a tool result that had already said the child was
/// delivered the card's model.
#[test]
fn an_agent_that_refuses_the_delivered_model_refuses_the_creation() {
    let _lock = lock_tests();
    let test = Slice5Test::with_profiles(
        &serde_json::json!({
            "title": "refused model",
            "profile": "worker",
            "initialPrompt": "report your result",
        }),
        &worker_profile_document(),
        // The stub takes the switch off the wire (it writes `set model.txt`)
        // and then answers it the way a plan-limited agent does: an error.
        &[("DEVBOULE_STUB_REJECT_SET_MODEL", "1")],
    );
    let creator = test.creator_session();
    let events = test.attach(&creator);
    test.allow_creation_card(&creator.id, &events);

    // The switch went out, and the agent's own refusal came back as the
    // creation's answer — not as a session id plus a transcript error.
    let received = test.wait_for_observations("set model.txt", 1);
    assert_eq!(
        received[0].trim(),
        "stub-model-new",
        "the switch was on the wire: {received:?}"
    );
    let calls = test.wait_for_observations("mcp calls.txt", 1);
    assert!(
        calls[0].contains("the agent refused the delivered model"),
        "the creation carries the refusal: {calls:?}"
    );
    assert!(
        calls[0].contains("unknown model"),
        "the agent's own words travel with it: {calls:?}"
    );
    assert!(
        calls[0].contains("stub-model-new"),
        "the refusal names the model the card promised: {calls:?}"
    );
    // The agent was ALIVE when the daemon tore it down: naming it an exited
    // provider would be the lie the pre-kill exit read exists to prevent
    // (the re-audit's P3-1, the direction the old mutation table missed).
    assert!(
        !calls[0].contains("provider exited during startup"),
        "a live agent that refused is not an exited provider: {calls:?}"
    );

    // Nothing survives to answer a prompt the card did not describe.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let live_child = test
            .client
            .sessions_list()
            .expect("session list")
            .into_iter()
            .any(|session| {
                session.created_by.as_deref() == Some(creator.id.as_str())
                    && matches!(session.state, devboule_protocol::SessionState::Live { .. })
            });
        assert!(
            !live_child,
            "no live child was left behind by the refused creation"
        );
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// R2a F5 — the delivery refusal's teardown is the handshake's, including
/// the three behaviours the old copy dropped: an agent that **dies** with
/// the switch unanswered is named as a provider that exited during startup,
/// its last stderr line travels with the message, and the whole banner goes
/// through the same redaction the handshake failure uses.
#[test]
fn an_agent_that_dies_mid_delivery_is_named_as_an_exited_provider() {
    let _lock = lock_tests();
    let test = Slice5Test::with_profiles(
        &serde_json::json!({
            "title": "dies mid delivery",
            "profile": "worker",
            "initialPrompt": "report your result",
        }),
        &worker_profile_document(),
        &[("DEVBOULE_STUB_DIE_BEFORE_SET_MODEL_REPLY", "1")],
    );
    let creator = test.creator_session();
    let events = test.attach(&creator);
    test.allow_creation_card(&creator.id, &events);

    let calls = test.wait_for_observations("mcp calls.txt", 1);
    assert!(
        calls[0].contains("provider exited during startup"),
        "the death is named as a startup exit, not a profile refusal: {calls:?}"
    );
    assert!(
        calls[0].contains("Agent stderr"),
        "the stderr tail travels with the message: {calls:?}"
    );
    assert!(
        calls[0].contains("dying before the set_model reply"),
        "the provider's own last line is in the tail: {calls:?}"
    );

    // And nothing survives: the child is gone, and the tool call carries the
    // refusal instead of a session id.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let live_child = test
            .client
            .sessions_list()
            .expect("session list")
            .into_iter()
            .any(|session| {
                session.created_by.as_deref() == Some(creator.id.as_str())
                    && matches!(session.state, devboule_protocol::SessionState::Live { .. })
            });
        assert!(
            !live_child,
            "no live child was left behind by the refused creation"
        );
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// R2a, the contradiction — bounded by **authority** (the re-audit's P1):
/// the pre-card gate refuses only where the daemon owns the tick rule, and
/// an ACP agent's modes are the agent's own vocabulary, so a ticked profile
/// naming a non-daemon mode is **not** refused before the card. The client
/// judges at spawn time, where the delivered mode is the fact: the stub
/// declares `ask,default` and starts in `ask`, and the daemon's
/// post-handshake guard refuses the tick over that delivered mode. The
/// handshake delivers the profile's mode first, so the fact the guard
/// judges is the mode the child would actually start in.
///
/// What the previous shape of this test caught that this one cannot, and
/// what this one catches back: the old test pinned the pre-card refusal of
/// an ACP-family creation over a non-daemon mode id — a pin this must not
/// keep, because that same refusal convicted Codex `full-access` profiles
/// the Codex client itself accepts. If the too-wide gate returns, this test
/// goes red from the other direction: the card is never raised (the
/// `allow_creation_card` below times out) and no delivery ever runs. The
/// ACP guard's own sentence — unasserted since the old contradiction test
/// replaced it (the re-audit's P3-5) — is asserted here again, at its real
/// site.
#[test]
fn an_auto_accept_tick_over_the_delivered_asking_mode_is_refused_by_the_delivery() {
    let _lock = lock_tests();
    let mut profiles = worker_profile_document();
    profiles["profiles"][0]["features"] = serde_json::json!({"autoAccept": true});
    let test = Slice5Test::with_profiles(
        &serde_json::json!({
            "title": "contradiction",
            "profile": "worker",
            "initialPrompt": "report your result",
        }),
        &profiles,
        &[],
    );
    let creator = test.creator_session();
    let events = test.attach(&creator);

    // The card IS raised: the daemon does not claim to know what the
    // profile's mode means for an agent whose vocabulary it does not own,
    // so consent is spent on the only judgement that can decide — the
    // agent's own handshake.
    test.allow_creation_card(&creator.id, &events);

    // The refusal carries the delivery guard's sentence, naming the mode the
    // handshake actually DELIVERED: the handshake switches the session to the
    // profile's mode first, and the guard judges that fact — here the
    // delivered mode and the profile's spelling coincide, because the switch
    // succeeded. The stub's starting mode is never named: what is judged is
    // what the child would start in.
    let calls = test.wait_for_observations("mcp calls.txt", 1);
    assert!(
        calls[0].contains("which asks the human"),
        "the refusal names the contradiction: {calls:?}"
    );
    assert!(
        calls[0].contains("mode 'default'"),
        "the refusal names the delivered mode: {calls:?}"
    );

    // Nothing survives to answer a prompt the card did not describe.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let live_child = test
            .client
            .sessions_list()
            .expect("session list")
            .into_iter()
            .any(|session| {
                session.created_by.as_deref() == Some(creator.id.as_str())
                    && matches!(session.state, devboule_protocol::SessionState::Live { .. })
            });
        assert!(
            !live_child,
            "no live child was left behind by the refused creation"
        );
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The P3-5 half a black-box test can pin: a ticked profile whose mode the
/// agent has no vocabulary for is refused at the handshake, before any
/// switch is promised — the stub here declares no modes at all
/// (`DEVBOULE_STUB_OMIT_MODES`), so there is nothing the daemon could ask
/// the agent about and nothing its guard could judge. The guard's absence
/// sentence itself ("declared no modes the daemon can judge") sits behind a
/// path profile creations cannot reach: a profile always names a mode, and a
/// mode absent from the agent's declarations is refused by the handshake
/// first — exactly the refusal asserted here. The absence arm stays as the
/// defensive half of the match for a mode-less delivery no creator can
/// express today.
#[test]
fn a_ticked_profile_over_an_agent_with_no_modes_is_refused_at_the_handshake() {
    let _lock = lock_tests();
    let mut profiles = worker_profile_document();
    profiles["profiles"][0]["features"] = serde_json::json!({"autoAccept": true});
    let test = Slice5Test::with_profiles(
        &serde_json::json!({
            "title": "no modes declared",
            "profile": "worker",
            "initialPrompt": "report your result",
        }),
        &profiles,
        &[("DEVBOULE_STUB_OMIT_MODES", "1")],
    );
    let creator = test.creator_session();
    let events = test.attach(&creator);
    test.allow_creation_card(&creator.id, &events);

    let calls = test.wait_for_observations("mcp calls.txt", 1);
    assert!(
        calls[0].contains("is not available"),
        "the refusal names the mode the agent has no vocabulary for: {calls:?}"
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let live_child = test
            .client
            .sessions_list()
            .expect("session list")
            .into_iter()
            .any(|session| {
                session.created_by.as_deref() == Some(creator.id.as_str())
                    && matches!(session.state, devboule_protocol::SessionState::Live { .. })
            });
        assert!(
            !live_child,
            "no live child was left behind by the refused creation"
        );
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The re-audit's P2-2: the creation-time confirm carries a deadline. An
/// agent that takes the switch off the wire and never answers cannot hold
/// the creation — the child, the reservation and the caller's tool call —
/// forever: the tool call comes back inside the bound with an `Io` refusal,
/// and nothing survives.
#[test]
fn an_agent_that_ignores_the_model_switch_cannot_hang_the_creation() {
    let _lock = lock_tests();
    let test = Slice5Test::with_profiles(
        &serde_json::json!({
            "title": "mute on the switch",
            "profile": "worker",
            "initialPrompt": "report your result",
        }),
        &worker_profile_document(),
        &[
            ("DEVBOULE_STUB_IGNORE_SET_MODEL", "1"),
            ("DEVBOULE_ACP_RESPONSE_TIMEOUT_MS", "1000"),
        ],
    );
    let creator = test.creator_session();
    let events = test.attach(&creator);
    test.allow_creation_card(&creator.id, &events);

    let calls = test.wait_for_observations("mcp calls.txt", 1);
    assert!(
        calls[0].contains("did not answer within"),
        "the mute agent is named as an unanswered wait, not a hang: {calls:?}"
    );

    // Nothing survives to answer a prompt the card did not describe.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let live_child = test
            .client
            .sessions_list()
            .expect("session list")
            .into_iter()
            .any(|session| {
                session.created_by.as_deref() == Some(creator.id.as_str())
                    && matches!(session.state, devboule_protocol::SessionState::Live { .. })
            });
        assert!(
            !live_child,
            "no live child was left behind by the refused creation"
        );
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The re-audit's P2-3, as a test: an agent that keeps the pipe non-empty
/// without ever writing a newline. A read bound consulted only when the
/// pipe is quiet never fires against this agent; the bound must hold while
/// bytes keep coming, and the refusal must name the wait the same way the
/// mute agent's does.
#[test]
fn an_agent_that_dribbles_without_a_newline_cannot_hang_the_creation() {
    let _lock = lock_tests();
    let test = Slice5Test::with_profiles(
        &serde_json::json!({
            "title": "dribble on the switch",
            "profile": "worker",
            "initialPrompt": "report your result",
        }),
        &worker_profile_document(),
        &[
            ("DEVBOULE_STUB_DRIBBLE_SET_MODEL", "1"),
            ("DEVBOULE_ACP_RESPONSE_TIMEOUT_MS", "1000"),
        ],
    );
    let creator = test.creator_session();
    let events = test.attach(&creator);
    test.allow_creation_card(&creator.id, &events);

    let calls = test.wait_for_observations("mcp calls.txt", 1);
    assert!(
        calls[0].contains("did not answer within"),
        "the dribbler is named as an unanswered wait, not a hang: {calls:?}"
    );

    // Nothing survives to answer a prompt the card did not describe.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let live_child = test
            .client
            .sessions_list()
            .expect("session list")
            .into_iter()
            .any(|session| {
                session.created_by.as_deref() == Some(creator.id.as_str())
                    && matches!(session.state, devboule_protocol::SessionState::Live { .. })
            });
        assert!(
            !live_child,
            "no live child was left behind by the refused creation"
        );
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// R2a, the identity the refusal buys: a tick over a mode the daemon's own
/// broker answers admits the creation, the child starts in that mode, and the
/// marker on the child is the fact the human ticked for — the delivered mode
/// and the marker are the same fact.
#[test]
fn an_auto_accept_child_starts_in_the_mode_the_tick_demands() {
    let _lock = lock_tests();
    let mut profiles = worker_profile_document();
    profiles["profiles"][0]["modeId"] = serde_json::json!("auto_accept");
    profiles["profiles"][0]["features"] = serde_json::json!({"autoAccept": true});
    let test = Slice5Test::with_profiles(
        &serde_json::json!({
            "title": "unattended",
            "profile": "worker",
            "initialPrompt": "report your result",
        }),
        &profiles,
        // The stub must declare the mode so the daemon has a real switch to
        // send; `auto_accept` is one of the ids the daemon's own broker
        // answers.
        &[("DEVBOULE_STUB_MODES", "ask,auto_accept")],
    );
    let creator = test.creator_session();
    let events = test.attach(&creator);
    test.allow_creation_card(&creator.id, &events);
    let description = creation_card_description(&events);
    assert!(
        description.contains("auto accept: Yes (mode auto_accept)"),
        "the card names the answering mode: {description}"
    );
    let child = test.child_of(&creator.id);
    assert_eq!(
        child.unattended,
        devboule_protocol::UnattendedState::Yes,
        "the marker and the delivered mode are one fact"
    );
    let switched = test.wait_for_observations("set mode.txt", 1);
    assert_eq!(
        switched[0].trim(),
        "auto_accept",
        "the child was delivered the mode the tick demands: {switched:?}"
    );
}

/// A feature key the agent never declared is named on the card **as what it
/// is**: a value the daemon will set if the agent declares it, and a creation
/// the daemon refuses rather than starts without it. This is the card's third
/// rule, rewritten by the feature surface — the old sentence ("stored, delivered
/// never, promised never") described a daemon that delivered no feature at all,
/// and that daemon is gone. The wording is part of the guarantee now, so the
/// test pins both halves: what the human is told, and that the creation still
/// succeeds for a key the agent does not need to declare.
#[test]
fn a_feature_key_the_agent_never_declared_is_named_as_a_condition_on_the_card() {
    let _lock = lock_tests();
    let mut profiles = worker_profile_document();
    profiles["profiles"][0]["features"] = serde_json::json!({"sandbox": "gVisor"});
    let test = Slice5Test::with_profiles(
        &serde_json::json!({
            "title": "feature keeper",
            "profile": "worker",
            "initialPrompt": "report your result",
        }),
        &profiles,
        &[],
    );
    let creator = test.creator_session();
    let events = test.attach(&creator);
    test.allow_creation_card(&creator.id, &events);
    let description = creation_card_description(&events);
    // Feature values render as JSON literals, exactly as the store holds them.
    assert!(
        description.contains("sandbox=gVisor"),
        "the card names the stored feature with its value: {description}"
    );
    // And it says what will happen to it, because no read has answered this
    // provider: the child's own handshake is the authority, and a refusal is
    // the answer rather than a silently missing value.
    assert!(
        description.contains("refuses the creation rather than starting without it"),
        "the card states the condition: {description}"
    );
    assert!(
        !description.contains("not interpreted") && !description.contains("never delivered"),
        "the old promise-nothing sentence is gone: {description}"
    );
    // An uninterpreted key is not a tick — and the provider-authored mode is
    // not asserted either way (R2b): the line says cannot-establish.
    assert!(
        description.contains("auto accept: Cannot establish"),
        "a stored value is not a tick, and the mode is not judged by name: {description}"
    );
}

// Retain the main-branch test name across the behavior change while exercising
// the current card contract in the shared test body above.
#[test]
fn an_unknown_feature_key_is_named_as_uninterpreted_on_the_card() {
    a_feature_key_the_agent_never_declared_is_named_as_a_condition_on_the_card();
}

/// `S5` block 2's overlay, measured on the child's *own* broker connection:
/// the design preset hides both tools from `tools/list` and refuses them at
/// `tools/call`, while the creator keeps all three.
///
/// One daemon, two stub processes, two MCP connections: the first line of each
/// observation file is the creator's, the second is the child's.
#[test]
fn the_childs_own_connection_sees_the_design_overlay() {
    let _lock = lock_tests();
    let test = Slice5Test::new_with_design_profile(&serde_json::json!({
        "title": "designer",
        "profile": "design",
        "initialPrompt": "report your result",
    }));
    let creator = test.creator_session();
    let events = test.attach(&creator);
    let card = test.allow_creation_card(&creator.id, &events);
    assert_eq!(card.profile, "design");

    let child = test.child_of(&creator.id);
    let pids = test.wait_for_observations("stub pids.txt", 2);
    let creator_pid = pids[0].trim().to_string();
    let child_pid = pids[1].trim().to_string();
    // Both processes asked for their own tool list, and the child's own call to
    // a hidden tool was refused: the overlay is per connection, not per daemon.
    let creator_tools = test.wait_for_lines_of("mcp tools.txt", &creator_pid, 1);
    let child_tools = test.wait_for_lines_of("mcp tools.txt", &child_pid, 1);
    let child_calls = test.wait_for_lines_of("mcp calls.txt", &child_pid, 1);
    // The names are read out of the parsed `tools/list` body rather than
    // substring-matched against the raw line: the profile list's description
    // points its reader at `devboule_create_agent` by name, so a description
    // mentioning a hidden tool is not the tool being served.
    let creator_names = tool_names(&creator_tools[0]);
    let child_names = tool_names(&child_tools[0]);
    assert!(
        creator_names.contains(&"devboule_create_agent".to_string())
            && creator_names.contains(&"devboule_send_message".to_string()),
        "the creator keeps both tools: {creator_names:?}"
    );
    assert!(
        child_names.contains(&"devboule_list_agents".to_string()),
        "a design child keeps the roster: {child_names:?}"
    );
    assert!(
        !child_names.contains(&"devboule_create_agent".to_string()),
        "the design overlay hides create_agent from the child's list: {child_names:?}"
    );
    assert!(
        !child_names.contains(&"devboule_send_message".to_string()),
        "and send_message: {child_names:?}"
    );
    assert!(
        child_calls[0].contains("Tool disabled by policy"),
        "a hidden tool is refused at call time too: {}",
        child_calls[0]
    );
    // The refusal is the whole effect: the child created nothing.
    assert_eq!(
        test.client
            .sessions_list()
            .expect("session list")
            .into_iter()
            .filter(|session| session.created_by.as_deref() == Some(child.id.as_str()))
            .count(),
        0,
        "a refused call creates no grandchild"
    );
}

/// The child id the creator's creation record names, waiting for it.
///
/// One client request is made here, before the creation is in flight: a
/// `sessions_list` issued *while* the creation runs competes with the events on
/// the same connection and the transcript stops arriving.
fn wait_for_agent_created(
    events: &Mutex<Vec<SessionEvent>>,
    what: &str,
    timeout: Duration,
) -> String {
    let deadline = Instant::now() + timeout;
    loop {
        let created = slice5_events(events).iter().find_map(|event| match event {
            SessionEvent::AgentCreated {
                child_session_id, ..
            } => Some(child_session_id.clone()),
            _ => None,
        });
        if let Some(child_session_id) = created {
            return child_session_id;
        }
        assert!(
            Instant::now() < deadline,
            "no creation record for {what}: [{}]",
            slice5_kinds(&slice5_events(events))
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The creator's whole transcript, waiting for the structured finish record.
fn wait_for_finish(
    events: &Mutex<Vec<SessionEvent>>,
    what: &str,
    timeout: Duration,
) -> Vec<SessionEvent> {
    let deadline = Instant::now() + timeout;
    loop {
        let transcript = slice5_events(events);
        if transcript
            .iter()
            .any(|event| matches!(event, SessionEvent::ChildFinished { .. }))
        {
            return transcript;
        }
        assert!(
            Instant::now() < deadline,
            "no finish report for {what}: [{}]",
            slice5_kinds(&transcript)
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Wait until the daemon's list reports `session_id` ended, and say whether its
/// row is still there at all (a stop keeps the transcript, an exit need not).
fn wait_for_session_ended(test: &Slice5Test, session_id: &str, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        let row = test
            .client
            .sessions_list()
            .expect("session list")
            .into_iter()
            .find(|session| session.id == session_id);
        match row {
            Some(row) => {
                if matches!(row.state, devboule_protocol::SessionState::Ended { .. }) {
                    return true;
                }
            }
            None => return false,
        }
        assert!(
            Instant::now() < deadline,
            "session {session_id} never reached an ended state"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Attach a session the test issues commands against but reads no events from:
/// the stop command rides the session's control subscription.
fn attach_control(test: &Slice5Test, session_id: &str) {
    let handler: EventHandler = Arc::new(|_| {});
    test.client
        .session_attach(session_id, None, handler)
        .expect("attach the session for control");
}

/// Wait until the daemon's list reports `session_id` live.
fn wait_for_session_live(test: &Slice5Test, session_id: &str, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    loop {
        let live = test
            .client
            .sessions_list()
            .expect("session list")
            .into_iter()
            .find(|session| session.id == session_id)
            .is_some_and(|session| {
                matches!(session.state, devboule_protocol::SessionState::Live { .. })
            });
        if live {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "session {session_id} never became live"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The daemon-wide held-creation count, read where the daemon states it: a
/// second creator's card carries it, and answering that card with an allow
/// clears the gate so the extra session leaves nothing behind.
fn daemon_wide_held_creations(test: &Slice5Test, why: &str) -> u32 {
    let second = test.creator_session();
    let second_events = test.attach(&second);
    let card = wait_for_creation_card(&second_events, Duration::from_secs(45));
    test.client
        .session_permission_respond(&second.id, &card.tool_call_id, PermissionOutcome::AllowOnce)
        .expect("allow the second creation");
    let held = card.caps.live_agent_sessions;
    println!("daemon-wide held creations {why}: {held}");
    held
}

/// A creator whose provider is the stub, with one creation in flight: the
/// returned test is attached and the card is answered, so the caller only has
/// to read the records.
fn slice5_creator_with_a_creation(
    creation: &serde_json::Value,
    profiles: &serde_json::Value,
    extra: &[(&str, &str)],
) -> (
    Slice5Test,
    devboule_protocol::Session,
    Arc<Mutex<Vec<SessionEvent>>>,
) {
    let test = Slice5Test::with_profiles(creation, profiles, extra);
    let creator = test.creator_session();
    let events = test.attach(&creator);
    test.allow_creation_card(&creator.id, &events);
    (test, creator, events)
}

/// Audit S5-01 end to end: a child whose process exits on its own — the reader
/// reaching EOF — gives its slot back.
///
/// The slot is measured where the daemon states it: a second creator's card
/// carries the daemon-wide count, which is one (its own reservation) when the
/// first child was released and two when it was not.
#[test]
fn a_child_that_exits_by_eof_gives_its_slot_back() {
    let _lock = lock_tests();
    let test = Slice5Test::new_with_design_profile(&serde_json::json!({
        "title": "builder",
        "profile": "design",
        "initialPrompt": "report your result",
    }));
    let first = test.creator_session();
    let first_events = test.attach(&first);
    test.allow_creation_card(&first.id, &first_events);
    // The creation record arrives on the creator's subscription *before* the
    // child produces anything (audit S5-10), and it carries the child's id: the
    // whole test can run without another client request, so the subscription is
    // never competing with one for the connection.
    let child_id = {
        let deadline = Instant::now() + Duration::from_secs(45);
        loop {
            let created = slice5_events(&first_events)
                .iter()
                .find_map(|event| match event {
                    SessionEvent::AgentCreated {
                        child_session_id, ..
                    } => Some(child_session_id.clone()),
                    _ => None,
                });
            if let Some(child_session_id) = created {
                break child_session_id;
            }
            assert!(
                Instant::now() < deadline,
                "no AgentCreated before the child's output: [{}]",
                slice5_kinds(&slice5_events(&first_events))
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    let pids = test.wait_for_observations("stub pids.txt", 2);
    assert_eq!(
        pids.len(),
        2,
        "the creator and its child are the only stubs so far: {pids:?}"
    );

    // The child's provider exits on its own. Nothing closes the session: the
    // daemon sees EOF, and that path has to account for the child like any
    // other end.
    let child_pid: u32 = pids[1].trim().parse().expect("the child's pid");
    unsafe {
        let handle = OpenProcess(PROCESS_TERMINATE, 0, child_pid);
        assert!(!handle.is_null(), "the child's process was found");
        let _ = TerminateProcess(handle, 0);
        CloseHandle(handle);
    }
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        // The row stays (a transcript of an ended session is still a row); what
        // the end has to change is its state, and — with it — the slot it held.
        let ended = test
            .client
            .sessions_list()
            .expect("session list")
            .into_iter()
            .find(|session| session.id == child_id)
            .is_none_or(|session| {
                matches!(session.state, devboule_protocol::SessionState::Ended { .. })
            });
        if ended {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the child is still live after its process exited"
        );
        std::thread::sleep(Duration::from_millis(50));
    }

    // Audit S5B-11: an end by EOF owes the creator the *report*, not only the
    // slot. Both records, and the structured one carries the id the text
    // message beside it really has.
    let report_deadline = Instant::now() + Duration::from_secs(45);
    loop {
        if slice5_events(&first_events)
            .iter()
            .any(|event| matches!(event, SessionEvent::ChildFinished { .. }))
        {
            break;
        }
        assert!(
            Instant::now() < report_deadline,
            "no finish report after the EOF: [{}]",
            slice5_kinds(&slice5_events(&first_events))
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    let transcript = slice5_events(&first_events);
    let (envelope, envelope_id) = slice5_system_message(&transcript, "kind: agent_finished");
    assert!(
        envelope.contains(&format!("childSessionId: {child_id}")),
        "the report after an EOF names the child: {envelope}"
    );
    let (message_id, _, _) = slice5_finished(&transcript);
    assert_eq!(
        message_id, envelope_id,
        "the structured record carries the id of the text record beside it"
    );

    // A second creator's card states the daemon-wide count as the daemon sees
    // it *after* that end: one held creation, this one.
    let second = test.creator_session();
    let second_events = test.attach(&second);
    let card = wait_for_creation_card(&second_events, Duration::from_secs(45));
    assert_eq!(
        card.caps.live_agent_sessions, 1,
        "the exited child's slot was given back, not leaked"
    );
    test.client
        .session_permission_respond(&second.id, &card.tool_call_id, PermissionOutcome::AllowOnce)
        .expect("allow the second creation");
}

/// `S5` §3 end to end: a child that parks on a permission card tells its creator
/// once, and the finish report follows the human's answer.
#[test]
fn a_child_parked_on_a_card_tells_its_creator_once_and_finishes_after_the_answer() {
    let _lock = lock_tests();
    let test = Slice5Test::new(&serde_json::json!({
        "title": "parker",
        "profile": "worker",
        // The stub asks for permission when its prompt mentions permission.
        "initialPrompt": "please request permission",
    }));
    let creator = test.creator_session();
    let events = test.attach(&creator);
    test.allow_creation_card(&creator.id, &events);
    let child = test.child_of(&creator.id);

    // The notice: one, and one only, however many cards the child raises.
    wait_for(&events, Duration::from_secs(60), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::AgentUserMessage { text, .. }
                if text.contains("kind: agent_input_required"))
        })
    });
    let (notice, _) = slice5_system_message(&slice5_events(&events), "kind: agent_input_required");
    assert!(
        notice.contains(&format!("childSessionId: {}", child.id)),
        "the notice names the parked child: {notice}"
    );

    // The human answers the *child's* card; the child then finishes and the
    // report arrives with the artifact the same way it does without a park.
    let _child_events = test.attach(&child);
    test.client
        .session_permission_respond(&child.id, "tool-perm-1", PermissionOutcome::AllowOnce)
        .expect("answer the child's card");
    wait_for(&events, Duration::from_secs(60), |events| {
        events
            .iter()
            .any(|event| matches!(event, SessionEvent::ChildFinished { .. }))
    });
    let transcript = slice5_events(&events);
    let notices = transcript
        .iter()
        .filter(|event| {
            matches!(event, SessionEvent::AgentUserMessage { text, .. }
                if text.contains("kind: agent_input_required"))
        })
        .count();
    assert_eq!(notices, 1, "one notice per child, not one per card");
    let (envelope, envelope_id) = slice5_system_message(&transcript, "kind: agent_finished");
    assert!(
        envelope.contains("state: completed"),
        "the answer un-parked the child, which then finished: {envelope}"
    );
    let (message_id, _, artifacts) = slice5_finished(&transcript);
    assert_eq!(message_id, envelope_id);
    assert_eq!(
        artifacts.len(),
        1,
        "the artifact followed the answer: {envelope}"
    );
    assert!(test.stored_attachments(&creator.id).len() == 1);
}

/// `S5` §2 end to end: a creation naming a provider this daemon cannot launch
/// is refused before a card, a slot or a session.
#[test]
fn a_creation_naming_an_uninstalled_provider_is_refused() {
    let _lock = lock_tests();
    let test = Slice5Test::with_profiles(
        &serde_json::json!({
            "title": "nowhere",
            "profile": "nowhere",
            "initialPrompt": "report your result",
        }),
        // The profile is the only thing that can name a provider now: this one
        // names the id whose binary cannot exist anywhere.
        &absent_provider_profile_document(),
        &[],
    );
    let creator = test.creator_session();
    let events = test.attach(&creator);
    let calls = test.wait_for_observations("mcp calls.txt", 1);
    assert!(
        calls[0].contains("provider not installed"),
        "the refusal is the sentence §2 names: {}",
        calls[0]
    );
    assert!(
        test.client
            .sessions_list()
            .expect("session list")
            .iter()
            .all(|session| session.created_by.is_none()),
        "nothing was created"
    );
    assert_eq!(
        test.observations("stub pids.txt").len(),
        1,
        "and no provider was spawned for it"
    );
    assert!(
        slice5_events(&events).iter().all(|event| !matches!(
            event,
            SessionEvent::PermissionRequest {
                create_agent: Some(_),
                ..
            }
        )),
        "a refused provider raises no card"
    );
}

/// Audit S5B-03: a child stopped *with its transcript kept* reaches EOF, and
/// that end owes the creator the report and the slot while the row stays.
#[test]
fn a_child_stopped_with_its_transcript_kept_is_reported_and_gives_its_slot_back() {
    let _lock = lock_tests();
    let (test, _creator, events) = slice5_creator_with_a_creation(
        &serde_json::json!({
            "title": "stopper",
            "profile": "design",
            "initialPrompt": "report your result",
        }),
        // The design profile: its overlay hides `create_agent` from the child,
        // so the child's own handshake adds no grandchild to the count below.
        &design_profile_document(),
        &[],
    );
    let child_id = wait_for_agent_created(&events, "the child", Duration::from_secs(45));
    wait_for_session_live(&test, &child_id, Duration::from_secs(45));

    // The stop path: the daemon kills the provider and keeps the transcript.
    // The stop command rides the session's control subscription: attach first.
    // The child must also be past its own handshake — its provider's probe is
    // the readiness fact — so this exercises the stop of a live child rather
    // than the never-started path a too-early kill would take.
    let child_pids = test.wait_for_observations("stub pids.txt", 2);
    test.wait_for_lines_of("mcp tools.txt", child_pids[1].trim(), 1);
    attach_control(&test, &child_id);
    test.client.session_stop(&child_id).expect("stop the child");
    let transcript = wait_for_finish(&events, "the stopped child", Duration::from_secs(45));
    let (envelope, envelope_id) = slice5_system_message(&transcript, "kind: agent_finished");
    assert!(
        envelope.contains(&format!("childSessionId: {child_id}")),
        "the report names the stopped child: {envelope}"
    );
    let (message_id, _, _) = slice5_finished(&transcript);
    assert_eq!(
        message_id, envelope_id,
        "the stopped child's record carries the id of the text report"
    );

    // S5B-03: the transcript stays, so the row stays — and it is ended.
    assert!(
        wait_for_session_ended(&test, &child_id, Duration::from_secs(45)),
        "a stopped child keeps its row"
    );
    assert_eq!(
        daemon_wide_held_creations(&test, "after a child was stopped"),
        1,
        "the stopped child gave its slot back"
    );
}

/// Audit-2 §1, case (a): the boundary is the return of the creation call. A
/// provider that dies during its own startup — here it answers `session/new`
/// and then goes — never became a session: the tool call answers an error, the
/// reservation rolls back (slot, in-flight, gate), and the creator hears
/// nothing about a child it never had.
#[test]
fn a_provider_that_dies_during_its_own_startup_is_a_failed_creation() {
    let _lock = lock_tests();
    let test = Slice5Test::new_with_env(
        &serde_json::json!({
            "title": "stillborn",
            "profile": "worker",
            "initialPrompt": "report your result",
        }),
        &[("DEVBOULE_ACP_STUB_EXIT_AFTER_SESSION_NEW", "2")],
    );
    let creator = test.creator_session();
    let events = test.attach(&creator);
    test.allow_creation_card(&creator.id, &events);

    // The tool call answers the boundary's error, and the stub recorded what
    // the daemon sent back to the agent that called it.
    let calls = test.wait_for_observations("mcp calls.txt", 1);
    assert!(
        calls
            .iter()
            .any(|line| line.contains("provider exited during startup")),
        "the creation call must answer the startup error, got: {calls:?}"
    );

    // Nothing is published on the creator: no creation record, no report.
    let transcript = slice5_events(&events);
    assert!(
        slice5_index_of(&transcript, |event| matches!(
            event,
            SessionEvent::AgentCreated { .. }
        ))
        .is_none(),
        "a failed creation publishes no AgentCreated: [{}]",
        slice5_kinds(&transcript)
    );
    assert!(
        slice5_index_of(&transcript, |event| matches!(
            event,
            SessionEvent::ChildFinished { .. }
        ))
        .is_none(),
        "a failed creation owes no report: [{}]",
        slice5_kinds(&transcript)
    );

    // The reservation rolled back: the next creation is admitted, and the
    // daemon-wide count that card states is its own reservation alone.
    let second = test.creator_session();
    let second_events = test.attach(&second);
    let second_card = wait_for_creation_card(&second_events, Duration::from_secs(45));
    assert_eq!(
        second_card.caps.live_agent_sessions, 1,
        "the failed creation's slot was given back, not leaked"
    );
    test.client
        .session_permission_respond(
            &second.id,
            &second_card.tool_call_id,
            PermissionOutcome::AllowOnce,
        )
        .expect("allow the creation after the failed one");
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        if slice5_index_of(&slice5_events(&second_events), |event| {
            matches!(event, SessionEvent::AgentCreated { .. })
        })
        .is_some()
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the creation after the failed one never happened: [{}]",
            slice5_kinds(&slice5_events(&second_events))
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Audit-2 §1, case (b): a child that dies *after* its creation answered Ok is
/// a child end, not a failed creation. This one goes the moment its first
/// prompt arrives, so its creation record is already on the creator's
/// transcript and the end still has to reach it — both records, one slot back.
#[test]
fn a_child_that_dies_on_its_first_prompt_is_reported_and_gives_its_slot_back() {
    let _lock = lock_tests();
    let test = Slice5Test::new_with_env(
        &serde_json::json!({
            "title": "prompt-fatal",
            "profile": "worker",
            "initialPrompt": "report your result",
        }),
        &[("DEVBOULE_ACP_STUB_EXIT_ON_PROMPT", "1")],
    );
    let creator = test.creator_session();
    let events = test.attach(&creator);
    test.allow_creation_card(&creator.id, &events);

    let child_id = wait_for_agent_created(
        &events,
        "the child that dies on its prompt",
        Duration::from_secs(45),
    );
    let transcript = wait_for_finish(
        &events,
        "the child that died on its first prompt",
        Duration::from_secs(45),
    );
    let (envelope, envelope_id) = slice5_system_message(&transcript, "kind: agent_finished");
    assert!(
        envelope.contains(&format!("childSessionId: {child_id}")),
        "the report names the child: {envelope}"
    );
    let (message_id, _, _) = slice5_finished(&transcript);
    assert_eq!(
        message_id, envelope_id,
        "ChildFinished carries the id of the text report beside it"
    );

    // No dead link, and the slot came back.
    wait_for_session_ended(&test, &child_id, Duration::from_secs(45));
    assert_eq!(
        daemon_wide_held_creations(&test, "the dead child's slot was given back"),
        1,
        "the dead child's slot was given back"
    );
}

// ---------------------------------------------------------------------------
// `create-from-profile`: standing instructions, the profile a session records,
// the unattended marker and the context a child inherits.
// ---------------------------------------------------------------------------

/// The one prompt text a session's transcript shows as the user's own first
/// message, which is what the daemon wrote to the provider.
fn first_user_message(events: &Mutex<Vec<SessionEvent>>) -> Option<String> {
    let events = events.lock().expect("events lock");
    events.iter().find_map(|event| match event {
        SessionEvent::AgentUserMessage { text, .. } => Some(text.clone()),
        _ => None,
    })
}

/// Wait for the transcript's first user message, the way every other assertion
/// in this battery waits: the daemon writes the prompt after the provider
/// handshake, so a reader that looks the moment the session row exists can be
/// early, and early is not wrong.
fn wait_for_user_message(events: &Mutex<Vec<SessionEvent>>, timeout: Duration) -> String {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(prompt) = first_user_message(events) {
            return prompt;
        }
        assert!(Instant::now() < deadline, "the prompt never arrived");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The preamble a created child's first prompt carries, spelled here because
/// `AGENT_PREAMBLE` is private to the daemon crate: this is the wire text, and a
/// test that read the constant would not notice it changing.
const PREAMBLE: &str =
    "You were created by another agent; report your result in your final message.";

/// `S5` §5c rev 10, the created-child route: the human's standing instructions
/// are prefixed to the **first prompt** of a child an agent creates, in front of
/// the preset preamble and of the caller's own text, in that order.
#[test]
fn the_standing_instructions_reach_a_child_an_agent_creates() {
    let _lock = lock_tests();
    let profiles = with_standing_instructions(
        worker_profile_document(),
        "Always answer in English and keep the diff small.",
    );
    let test = Slice5Test::with_profiles(
        &serde_json::json!({
            "title": "builder",
            "profile": "worker",
            "initialPrompt": "report your result",
        }),
        &profiles,
        &[],
    );
    let creator = test.creator_session();
    let events = test.attach(&creator);
    test.allow_creation_card(&creator.id, &events);
    let child = test.child_of(&creator.id);

    // The child's transcript carries the prompt the daemon wrote to its provider
    // (one value, two destinations: the writer and the journal).
    let child_events = test.attach_child(&child);
    let prompt = wait_for_user_message(&child_events, Duration::from_secs(45));
    assert_eq!(
        prompt,
        format!(
            "Always answer in English and keep the diff small.\n\n{PREAMBLE}\n\nreport your result"
        ),
        "standing instructions, then the preamble, then the caller's prompt"
    );
}

/// The same rule on the route a human's own session takes: the daemon composes
/// the standing instructions in front of the first message a client sends,
/// because a session a person opened has no daemon-composed prompt of its own.
#[test]
fn the_standing_instructions_reach_a_session_a_human_opens() {
    let _lock = lock_tests();
    let profiles = with_standing_instructions(
        worker_profile_document(),
        "Always answer in English and keep the diff small.",
    );
    let test = Slice5Test::with_profiles(
        &serde_json::json!({
            "title": "unused",
            "profile": "worker",
            "initialPrompt": "Not used: this test's session is a human's own.",
        }),
        &profiles,
        &[],
    );
    let session = test.creator_session();
    let events = test.attach(&session);
    test.client
        .session_send(&session.id, "what is the state of the repo?")
        .expect("send the human's first message");
    assert_eq!(
        wait_for_user_message(&events, Duration::from_secs(45)),
        "Always answer in English and keep the diff small.\n\nwhat is the state of the repo?",
        "the standing instructions and the human's message, in that order"
    );
}

/// The resume road (`session_resume` → `spawn_resumed_session` →
/// `start_spawned_session`): the session it builds is **mid-conversation**, so
/// the standing instructions must not be prefixed onto the next prompt the
/// human sends. A fresh runtime starts owing a first prompt; a resumed session
/// is not a fresh session — its first prompt happened in the generation being
/// resumed, and the human's rules arriving a second time, as the user's own
/// words, is the defect this pins.
#[test]
fn a_resumed_session_does_not_re_inject_the_standing_instructions() {
    let _lock = lock_tests();
    let profiles = with_standing_instructions(
        worker_profile_document(),
        "Always answer in English and keep the diff small.",
    );
    let test = Slice5Test::with_profiles(
        &serde_json::json!({
            "title": "unused",
            "profile": "worker",
            "initialPrompt": "Not used: this test's session is a human's own.",
        }),
        &profiles,
        &[],
    );
    let session = test.creator_session();
    let events = test.attach(&session);
    test.client
        .session_send(&session.id, "first prompt")
        .expect("send the first prompt");
    assert_eq!(
        wait_for_user_message(&events, Duration::from_secs(45)),
        "Always answer in English and keep the diff small.\n\nfirst prompt",
        "control: the session's first prompt carries the standing instructions"
    );

    let pids = test.wait_for_observations("stub pids.txt", 1);
    let pid: u32 = pids[0].trim().parse().expect("stub pid");
    test.client
        .session_stop(&session.id)
        .expect("stop the session");
    wait_until_gone(pid);

    let resumed = test
        .client
        .session_resume(
            Persistence {
                kind: PersistenceKind::Acp {
                    handle: session.id.clone(),
                },
            },
            None,
        )
        .expect("resume the session");
    assert!(matches!(
        resumed,
        ResumeResult::Resumed { session: ref r } if r.id == session.id
    ));
    let after = test.attach(&session);
    test.client
        .session_send(&session.id, "second prompt")
        .expect("send the prompt after the resume");

    // Since `905d70b` a fresh attach replays the whole transcript across
    // generations, pre-resume history included: this stream carries
    // generation 1's "Always answer …\n\nfirst prompt" and generation 1's
    // own `end_turn`, so neither a finished event nor a `last()` read can
    // tell the live turn from the replay — which is exactly how this test
    // used to pass and fail on the weather. The one event only the
    // post-resume turn can supply is its own user message: no earlier
    // generation ever contained "second prompt". Its arrival is the
    // delivery signal.
    wait_for(&after, Duration::from_secs(45), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::AgentUserMessage { text, .. } if text == "second prompt")
        })
    });
    let prompts: Vec<String> = {
        let events = after.lock().expect("events lock");
        events
            .iter()
            .filter_map(|event| match event {
                SessionEvent::AgentUserMessage { text, .. } => Some(text.clone()),
                _ => None,
            })
            .collect()
    };
    // The property, checked against EVERY user message the stream holds,
    // replayed or live, not against whichever landed last: the post-resume
    // prompt is the caller's own text, exactly once, with no standing
    // instructions and no separator re-attached.
    assert_eq!(
        prompts
            .iter()
            .filter(|prompt| prompt.as_str() == "second prompt")
            .count(),
        1,
        "the post-resume prompt reaches the transcript exactly once, as the caller sent it"
    );
    let re_composed = "Always answer in English and keep the diff small.\n\nsecond prompt";
    assert!(
        !prompts.iter().any(|prompt| prompt.as_str() == re_composed),
        "the daemon must not prefix standing instructions onto a resumed session's prompt: {:?}",
        prompts
    );
    test.client
        .session_close(&session.id)
        .expect("close the resumed session");
}

/// The tool names one `tools/list` observation carried. The line is
/// `<pid> <bearer fingerprint> <json body>`, so the names come out of the
/// parsed body: a description that mentions another tool's name (the profile
/// list points its reader at the creation tool) must not read as that tool
/// being served.
fn tool_names(line: &str) -> Vec<String> {
    let body = line.splitn(3, ' ').nth(2).unwrap_or(line);
    let parsed: serde_json::Value = serde_json::from_str(body).expect("the tools/list body");
    parsed["result"]["tools"]
        .as_array()
        .expect("the tools array")
        .iter()
        .map(|tool| tool["name"].as_str().expect("a tool name").to_string())
        .collect()
}

/// The Design host's route: the app composes one long grounded prompt and sends
/// it as the session's first message, which is the same daemon entry point as
/// any chat message — so the standing instructions land in front of the Design
/// instructions, and no Design session can be the exception to the rule.
#[test]
fn the_standing_instructions_reach_the_design_host() {
    let _lock = lock_tests();
    let profiles = with_standing_instructions(
        worker_profile_document(),
        "Always answer in English and keep the diff small.",
    );
    let test = Slice5Test::with_profiles(
        &serde_json::json!({
            "title": "unused",
            "profile": "worker",
            "initialPrompt": "Not used: this test's session is the Design host.",
        }),
        &profiles,
        &[],
    );
    let host = test.creator_session();
    let events = test.attach(&host);
    // The shape the app's `groundedPrompt` builds: the work instruction, then
    // the request, then the doctrine.
    let grounded = "Work on the requested design change in the active Devboule workspace.\n\
                    User request: make the header sticky.\n\
                    Doctrine: change tokens, not components.";
    test.client
        .session_send(&host.id, grounded)
        .expect("send the Design host's first prompt");
    assert_eq!(
        wait_for_user_message(&events, Duration::from_secs(45)),
        format!("Always answer in English and keep the diff small.\n\n{grounded}"),
        "the Design prompt is the prompt; the instructions go in front of it"
    );
}

/// The other half of the same rule: with no standing instructions the first
/// prompt is exactly what it was before this slice — no separator, no blank
/// line, nothing.
#[test]
fn empty_standing_instructions_leave_the_first_prompt_alone() {
    let _lock = lock_tests();
    let test = Slice5Test::new(&serde_json::json!({
        "title": "builder",
        "profile": "worker",
        "initialPrompt": "report your result",
    }));
    let creator = test.creator_session();
    let events = test.attach(&creator);
    test.allow_creation_card(&creator.id, &events);
    let child = test.child_of(&creator.id);
    let child_events = test.attach_child(&child);
    assert_eq!(
        wait_for_user_message(&child_events, Duration::from_secs(45)),
        format!("{PREAMBLE}\n\nreport your result"),
        "an empty standing-instructions text adds nothing to the child's prompt"
    );

    // And on a session a human opened, the first prompt is the message itself.
    // The creator of this test has a child, and the child's finish report is
    // itself an agent user message on the creator's transcript, so the message
    // just sent is not the transcript's first — wait for its text.
    test.client
        .session_send(&creator.id, "what is the state of the repo?")
        .expect("send the human's first message");
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        let arrived = events.lock().expect("events lock").iter().any(|event| {
            matches!(
                event,
                SessionEvent::AgentUserMessage { text, .. }
                    if text == "what is the state of the repo?"
            )
        });
        if arrived {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the human's message never arrived on the creator's transcript"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// The unattended marker is a fact of the child's **birth**: it is decided by
/// the profile's own mode at the creation, and un-ticking that profile
/// afterwards leaves the running child as it was — while every new creation is
/// refused, and the refusal names no profile at all.
///
/// Both surfaces the fact lives on are read: the live registry view
/// (`sessions_list`) *and* the journal row, which is what the name claims —
/// `sessions_list` alone serves a live child from its in-memory birth
/// metadata and would pass against the pre-tri-state journal, so the durable
/// half is asserted against the stored row (audit R2b-1 §8.2).
#[test]
fn a_child_born_unattended_stays_unattended_after_its_profile_is_un_ticked() {
    let _lock = lock_tests();
    let ticked = serde_json::json!({
        "profiles": [stub_profile_with(
            "runner",
            "profile-runner",
            "bypass",
            "stub-model-new",
            serde_json::json!({}),
            &[],
            true,
        )],
        "standingInstructions": "",
    });
    let test = Slice5Test::with_profiles(
        &serde_json::json!({
            "title": "runner-child",
            "profile": "runner",
            "initialPrompt": "report your result",
        }),
        &ticked,
        &[],
    );
    let creator = test.creator_session();
    let events = test.attach(&creator);
    test.allow_creation_card(&creator.id, &events);
    let child = test.child_of(&creator.id);
    assert_eq!(child.profile_id.as_deref(), Some("profile-runner"));
    assert_eq!(
        child.unattended,
        devboule_protocol::UnattendedState::Yes,
        "a profile whose mode auto-answers permission prompts makes an unattended child"
    );

    // The human un-ticks it. The child is untouched: it did run unattended.
    let unticked = serde_json::json!({
        "profiles": [stub_profile_with(
            "runner",
            "profile-runner",
            "bypass",
            "stub-model-new",
            serde_json::json!({}),
            &[],
            false,
        )],
        "standingInstructions": "",
    });
    test.client
        .agent_profiles_set(profile_document(&unticked))
        .expect("un-tick the profile");
    let again = test
        .client
        .sessions_list()
        .expect("session list")
        .into_iter()
        .find(|session| session.id == child.id)
        .expect("the child is still listed");
    assert_eq!(
        again.unattended,
        devboule_protocol::UnattendedState::Yes,
        "the marker is a fact of the birth, not a view of the current settings"
    );

    // The durable half, the one the name claims: the journal row keeps the
    // birth marker after the un-tick. `journal_usage` waits for the
    // asynchronous journal writer, so the read cannot race the write.
    test.client.journal_usage().expect("flush journal");
    let stored: i64 = Connection::open(test.harness.paths.journal_file())
        .expect("open journal")
        .query_row(
            "SELECT unattended_state FROM sessions WHERE id = ?1",
            [child.id.as_str()],
            |row| row.get(0),
        )
        .expect("the child's journal row");
    assert_eq!(
        stored, 2,
        "the stored rank is `yes`: the un-tick rewrote neither the live child \
         nor the row it was born with"
    );

    // A new creation is refused, and the sentence names no profile: an agent must
    // not learn which profiles exist but are forbidden. The refusal arrives on
    // its own clock — the first creation's own reply may land first or second,
    // and the two sagas share no happens-before — so this waits for the sentence
    // itself rather than a position (the file's own convention: order is not
    // evidence; every assertion names the process it is talking about).
    let second = test.creator_session();
    let _ = test.attach(&second);
    let deadline = Instant::now() + Duration::from_secs(45);
    let line = loop {
        let lines = test.observations("mcp calls.txt");
        if let Some(line) = lines
            .iter()
            .find(|line| line.contains("no profile is enabled for agents"))
        {
            break line.clone();
        }
        assert!(
            Instant::now() < deadline,
            "the unticked refusal never arrived: {lines:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(
        !line.contains("runner"),
        "the refusal must not leak what exists but is forbidden: {line}"
    );
}

/// The restart is the one path a durable format exists for (audit R2b-1
/// §8.3): after the daemon process is replaced, the child's marker is read
/// back from the journal on **both** post-restart surfaces — the recovered
/// roster row, and the resumed session whose metadata comes from
/// `session_metadata_for_resume` copying `record.unattended_state`. A resume
/// is not a creation, so nothing on the path re-derives: a child born `yes`
/// comes back `yes` from the row alone.
#[test]
fn the_unattended_marker_survives_a_daemon_restart_and_a_resume() {
    let _test_lock = lock_tests();
    // The delivery is judged against the modes the agent publishes at the
    // handshake, so the stub must declare the route-A id the create names.
    // Process-global like AcpTest's own knobs, and safe under the test lock;
    // the guard keeps a failure from leaking it into the next test.
    struct ClearModes;
    impl Drop for ClearModes {
        fn drop(&mut self) {
            std::env::remove_var("DEVBOULE_STUB_MODES");
        }
    }
    std::env::set_var("DEVBOULE_STUB_MODES", "ask,bypass");
    let _clear = ClearModes;
    let mut test = AcpTest::new(&[]);
    // Route A: the daemon's own broker answers this delivered id whatever
    // the agent's own vocabulary says — the same fact the profile battery's
    // bypass children carry, reached without a profile.
    let session = test
        .client
        .session_create_with(
            None,
            SessionKind::Acp,
            None,
            Some("bypass".to_string()),
            None,
        )
        .expect("create an unattended ACP session");
    assert_eq!(
        session.unattended,
        devboule_protocol::UnattendedState::Yes,
        "the birth marker: the broker answers this id"
    );
    // journal_usage waits for the asynchronous journal writer, so the
    // restart cannot race the birth row still being persisted.
    test.client.journal_usage().expect("flush journal");

    test.restart();

    let recovered = test
        .client
        .sessions_list()
        .expect("list sessions")
        .into_iter()
        .find(|listed| listed.id == session.id)
        .expect("recovered session missing after restart");
    assert_eq!(
        recovered.unattended,
        devboule_protocol::UnattendedState::Yes,
        "the recovered roster row reads what the birth wrote"
    );

    // The real resume entry: `session_resume` reads the journal record and
    // hands it to `session_metadata_for_resume`, whose `unattended` field is
    // the row's, never a re-derivation.
    let resumed = test
        .client
        .session_resume(
            Persistence {
                kind: PersistenceKind::Acp {
                    handle: session.id.clone(),
                },
            },
            None,
        )
        .expect("resume recovered ACP session");
    assert!(
        matches!(&resumed, ResumeResult::Resumed { session }
            if session.unattended == devboule_protocol::UnattendedState::Yes),
        "the resumed session keeps the birth marker: {resumed:?}"
    );
    test.client
        .session_close(&session.id)
        .expect("close the resumed session");
}

/// Renaming the profile a child was started from changes nothing about the child:
/// the session records the profile's stable **id**, and a creation that names the
/// old name is unknown.
#[test]
fn renaming_a_profile_does_not_change_what_a_running_child_was_started_from() {
    let _lock = lock_tests();
    let test = Slice5Test::new(&serde_json::json!({
        "title": "builder",
        "profile": "worker",
        "initialPrompt": "report your result",
    }));
    let creator = test.creator_session();
    let events = test.attach(&creator);
    test.allow_creation_card(&creator.id, &events);
    let child = test.child_of(&creator.id);
    assert_eq!(child.profile_id.as_deref(), Some("profile-worker"));

    // The human renames the profile they ticked: same id, new name.
    let mut renamed = worker_profile_document();
    renamed["profiles"][0]["name"] = serde_json::json!("foreman");
    test.client
        .agent_profiles_set(profile_document(&renamed))
        .expect("rename the profile");
    let after = test
        .client
        .sessions_list()
        .expect("session list")
        .into_iter()
        .find(|session| session.id == child.id)
        .expect("the child is still listed");
    assert_eq!(
        after.profile_id.as_deref(),
        Some("profile-worker"),
        "the running child still says which profile made it, not what it is called now"
    );

    // The name is not a fact about the child either: a creation that names the
    // name it used to have is refused as unknown.
    let second = test.creator_session();
    let _ = test.attach(&second);
    // Every stub process in this test appends to the one observation file, in
    // the order its HTTP answer lands — not in the order the test creates
    // sessions — so the file's line positions say nothing. The sentence is the
    // fact under test: some creation named the old name after the rename, and
    // the daemon refused it without naming what still exists.
    let deadline = Instant::now() + Duration::from_secs(45);
    loop {
        let refused = test
            .observations("mcp calls.txt")
            .iter()
            .any(|line| line.contains("unknown profile; call devboule_list_profiles"));
        if refused {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "no creation naming the old name was refused as unknown"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A creator and everything it commissions share one context, at any depth: the
/// child's child reports the human's session id.
#[test]
fn a_grandchild_shares_the_context_of_the_human_session_it_came_from() {
    let _lock = lock_tests();
    let test = Slice5Test::new(&serde_json::json!({
        "title": "builder",
        "profile": "worker",
        "initialPrompt": "report your result",
    }));
    let creator = test.creator_session();
    let creator_events = test.attach(&creator);
    test.allow_creation_card(&creator.id, &creator_events);
    let child = test.child_of(&creator.id);
    assert_eq!(child.context_id.as_deref(), Some(creator.id.as_str()));

    // The child's own provider asks for a child of its own (every stub process
    // makes the call `DEVBOULE_ACP_STUB_MCP_CALL` names), so the card that
    // arrives on the *child's* session is the grandchild's.
    let child_events = test.attach_child(&child);
    let grandchild_card = wait_for_creation_card(&child_events, Duration::from_secs(45));
    test.client
        .session_permission_respond(
            &child.id,
            &grandchild_card.tool_call_id,
            PermissionOutcome::AllowOnce,
        )
        .expect("allow the grandchild");
    let grandchild = test.child_of(&child.id);
    assert_eq!(
        grandchild.context_id.as_deref(),
        Some(creator.id.as_str()),
        "a grandchild shares the context of the session the family came from"
    );
    assert_eq!(grandchild.created_by.as_deref(), Some(child.id.as_str()));
}

/// Extra stub knobs for one test, cleared when it ends.
///
/// `AcpTest::new_with_options` takes flags and clears its own variables; the
/// leaves below need knobs that carry a *value* (a message, a code), so they
/// are set here — before the daemon is spawned, because the daemon and the
/// provider it launches inherit this process's environment — and removed on
/// drop, in the same way the harness's own guard removes its own.
struct StubKnobs(Vec<&'static str>);

impl StubKnobs {
    fn set(extra: &[(&'static str, String)]) -> Self {
        for (name, value) in extra {
            std::env::set_var(name, value);
        }
        Self(extra.iter().map(|(name, _)| *name).collect())
    }
}

impl Drop for StubKnobs {
    fn drop(&mut self) {
        for name in self.0.drain(..) {
            std::env::remove_var(name);
        }
    }
}

fn resume_error(test: &AcpTest, session_id: &str) -> devboule_protocol::WireError {
    let error = test
        .client
        .session_resume(
            Persistence {
                kind: PersistenceKind::Acp {
                    handle: session_id.to_string(),
                },
            },
            None,
        )
        .expect_err("the resume is refused");
    match error {
        devboule_daemon::DaemonError::Handshake(wire) => wire,
        other => panic!("expected the daemon's own ACP sentence, got {other:?}"),
    }
}

/// The truth the app was missing (measured 2026-09-21): the agent answers
/// `session/load` in 42 ms with a JSON-RPC error naming the folder that is
/// gone, and the human must read **that** — code, message, path — not a
/// deadline the agent never missed.
///
/// Mutants: the error object dropped for the generic transport sentence (the
/// message would be the EOF or the timeout text instead of the provider's);
/// `acp_request_error_message` serialising the whole object (the code and the
/// diagnosis would arrive as JSON).
#[test]
fn a_provider_refusal_on_load_reaches_the_human_in_the_providers_own_words() {
    let _test_lock = lock_tests();
    let message = concat!(
        "Invalid params: `cwd` does not exist on the machine running the agent: ",
        "C:",
        r"\gone-worktree\src-tauri"
    );
    let _knobs = StubKnobs::set(&[
        ("DEVBOULE_STUB_ERROR_LOAD_MESSAGE", message.to_string()),
        ("DEVBOULE_STUB_ERROR_LOAD_CODE", "-32602".to_string()),
    ]);
    let test = AcpTest::new(&[]);
    let session = stopped_zero_turn_session(&test);

    let wire = resume_error(&test, &session.id);
    assert_eq!(
        wire.code,
        ErrorCode::Io,
        "the provider answered: this is not a missing session: {wire:?}"
    );
    assert!(
        wire.message
            .starts_with(&format!("ACP request failed (-32602): {message}")),
        "the agent's own words, with its code, and the provider's stderr after them: {}",
        wire.message
    );
    assert!(
        !wire.message.contains("did not answer within"),
        "an agent that answered in 42 ms is not a silent one: {}",
        wire.message
    );
}

/// The second branch of the same precedence: the child is **gone**, so the
/// sentence names the code it left with, and its last words travel with it.
/// "The agent is gone" and "the agent is gone with 1" are different facts, and
/// only the second one can be acted on.
///
/// Mutants: the exit code dropped (`provider_exit_before_teardown` back to a
/// bool — the message says the provider exited and nothing about why); the
/// stderr excerpt dropped (`redact_handshake_error` not called — the marker the
/// child wrote is what names the failure, and the test dies on it).
#[test]
fn a_provider_that_dies_on_load_is_named_with_its_exit_code_and_its_last_words() {
    let _test_lock = lock_tests();
    let _knobs = StubKnobs::set(&[("DEVBOULE_STUB_DIE_ON_LOAD", "1".to_string())]);
    let test = AcpTest::new(&[]);
    let session = stopped_zero_turn_session(&test);

    let wire = resume_error(&test, &session.id);
    assert!(
        wire.message.contains("provider exited during startup: "),
        "the death is named as a death, not as a protocol fault: {}",
        wire.message
    );
    assert!(
        wire.message.contains("(the child's exit code was 1)"),
        "the code the child left with: {}",
        wire.message
    );
    assert!(
        wire.message.contains("stub-agent died on session/load"),
        "the child's own last line is what names the failure: {}",
        wire.message
    );
}

/// The third branch, and only the third: no answer, no exit — the provider is
/// alive and mute. Then the deadline sentence is the truth, it names the budget
/// that actually fired, and that budget is the **first answer's** (the ordinary
/// one is set four times shorter here, so a message saying "within 0s" is the
/// wrong budget wearing the right sentence).
///
/// Mutants: the first-answer budget dropped from the `initialize` read (the
/// ordinary budget fires and the sentence names it); the silence arm turned
/// into the JSON-RPC error arm (a mute provider would be reported as refusing).
#[test]
fn a_mute_provider_is_refused_by_the_first_answer_budget_and_says_which_one_fired() {
    let _test_lock = lock_tests();
    let _knobs = StubKnobs::set(&[
        ("DEVBOULE_STUB_IGNORE_INITIALIZE", "1".to_string()),
        ("DEVBOULE_ACP_RESPONSE_TIMEOUT_MS", "250".to_string()),
        ("DEVBOULE_ACP_FIRST_RESPONSE_TIMEOUT_MS", "1500".to_string()),
    ]);
    let test = AcpTest::new(&[]);

    let error = test
        .client
        .session_create(None, SessionKind::Acp, None)
        .expect_err("a provider that never answers never becomes a session");
    let wire = match error {
        devboule_daemon::DaemonError::Handshake(wire) => wire,
        other => panic!("expected the daemon's own ACP sentence, got {other:?}"),
    };
    assert!(
        wire.message.contains("did not answer within 1s"),
        "the first answer's own budget, in whole seconds: {}",
        wire.message
    );
    assert!(
        !wire.message.contains("did not answer within 0s"),
        "the ordinary budget's 250 ms is not what bounded the first answer: {}",
        wire.message
    );
}

/// A stored ACP select reaches the **child** through the create route. The
/// profile stores `fast=on`, the agent declares that option, and the proof is
/// the stub's own record of a `session/set_config_option` naming both — not a
/// call into a pure helper.
///
/// The first draft of this slice pinned `declared_feature_frames` alone, so a
/// route that stopped calling the delivery left those green while every child
/// started on the provider's default. Only a wire assertion can see that, and it
/// earned its keep: the arms of `apply_profile_delivery` that skip a model switch
/// also skipped the feature delivery until this test refused to pass.
#[test]
fn a_created_child_receives_its_profiles_declared_feature() {
    let _lock = lock_tests();
    let mut profiles = worker_profile_document();
    // `sonnet` is a model the config-options stub really declares; the profile's
    // own `stub-model-new` is not in that frame, and the pre-existing model rule
    // refuses the creation long before any feature is considered.
    profiles["profiles"][0]["model"] = serde_json::json!("sonnet");
    profiles["profiles"][0]["features"] = serde_json::json!({ "fast": "on" });
    let test = Slice5Test::with_profiles_and_args(
        &serde_json::json!({
            "title": "feature child",
            "profile": "worker",
            "initialPrompt": "report your result",
        }),
        &profiles,
        &[],
        &["--config-options", "--feature-option"],
    );
    let creator = test.creator_session();
    let events = test.attach(&creator);
    test.allow_creation_card(&creator.id, &events);
    // The frame arrived, naming the agent's own option id and the stored value.
    // Read from the append log, not the last-value file: the model switch and the
    // feature switch have no promised order, and a one-line file the next frame
    // overwrites cannot answer "did *my* frame arrive".
    let frames = test.wait_for_observations("set config log.txt", 2);
    assert!(
        frames.iter().any(|line| line.trim() == "fast=on"),
        "no session/set_config_option named fast=on; saw {frames:?}"
    );
    let child = test.child_of(&creator.id);
    assert_eq!(child.profile_id.as_deref(), Some("profile-worker"));
}

/// The probe closes the session it opened when the agent advertises
/// `sessionCapabilities.close` — Paseo's `closeProbe` gate
/// (`acp-agent.ts:1441`) — and never sends it when the agent does not. Killing
/// the process is not the equivalent: an agent that persists sessions past
/// process exit keeps an orphan per settings-panel open, in the user's history
/// or spending a session quota. An agent that did not offer the verb answers
/// `session/close` with a method-not-found error, so sending one would be a log
/// line and no cleanup.
#[test]
fn the_feature_probe_closes_its_session_only_when_the_agent_advertises_close() {
    let _lock = lock_tests();
    let advertising = AcpTest::new_feature_probe(true);
    wait_for_probing(&advertising.client);
    let closed = wait_for_file(&advertising.session_close_file());
    assert!(
        !closed.trim().is_empty(),
        "the probe sent session/close for the session it opened: {closed}"
    );

    let silent = AcpTest::new_feature_probe(false);
    wait_for_probing(&silent.client);
    // `wait_for_probing` returns only once the read has answered, and the close
    // is sent before the process is torn down — so a wrong request would already
    // have been written.
    assert!(
        !silent.session_close_file().exists(),
        "an agent without the capability is never sent session/close"
    );
}

/// The ACP feature read, end to end. The first ask cannot answer with a list it
/// has not read, so it starts the provider and says `probing`; a later ask
/// answers the list the agent declared — the `fast` dial beside the daemon's own
/// tick — with the model, effort and mode selectors left out, because a profile
/// already stores those as its own fields.
#[test]
fn acp_feature_read_answers_the_declared_dial_after_starting_the_provider() {
    let _lock = lock_tests();
    let test = AcpTest::new_feature_probe(false);
    let mut saw_probing = false;
    let first = test
        .client
        .provider_vocabulary_get("devboule-acp-stub", None, false)
        .expect("vocabulary rpc");
    if let DaemonMessage::ProviderVocabulary { features, .. } = &first {
        let axis = features.clone().expect("the axis is answered");
        saw_probing = axis.probing;
    }
    let axis = wait_for_probing(&test.client);
    assert!(
        saw_probing,
        "the first ask answers `probing` rather than a list it has not read"
    );
    assert_eq!(
        axis.items
            .iter()
            .map(|feature| feature.id.as_str())
            .collect::<Vec<_>>(),
        ["autoAccept", "fast", "agent"],
        "the daemon's tick beside the agent's own dials, and no switch or mode: {axis:?}"
    );
    assert_eq!(
        axis.items
            .iter()
            .map(|feature| feature.author)
            .collect::<Vec<_>>(),
        [
            devboule_protocol::VocabularyOrigin::Daemon,
            devboule_protocol::VocabularyOrigin::Provider,
            devboule_protocol::VocabularyOrigin::Provider
        ],
        "authorship is per row, because the list is mixed: {axis:?}"
    );
    // The answer is cached: the same question is answered without a new read.
    let again = test
        .client
        .provider_vocabulary_get("devboule-acp-stub", None, false)
        .expect("second vocabulary rpc");
    let DaemonMessage::ProviderVocabulary { features, .. } = again else {
        panic!("unexpected reply");
    };
    let cached = features.expect("the axis is answered");
    assert!(!cached.probing, "a cached answer is final: {cached:?}");
}

/// The cache key carries the model, so a read made against one model is never
/// the answer for another — the half that protects a stored value, since the
/// store's prune asks for the profile's own pair.
#[test]
fn acp_feature_read_answers_per_model_and_not_across_models() {
    let _lock = lock_tests();
    let test = AcpTest::new_feature_probe(false);
    let axis_a = test
        .client
        .provider_vocabulary_get("devboule-acp-stub", Some("stub-model-new"), false)
        .expect("vocabulary rpc");
    let DaemonMessage::ProviderVocabulary { features, .. } = axis_a else {
        panic!("unexpected reply");
    };
    let first = features.expect("the axis is answered");
    assert!(
        first.probing || first.state == devboule_protocol::VocabularyState::Present,
        "model A's question starts its own read: {first:?}"
    );
    // Model B was never read. It must not be answered with A's list: the honest
    // reply is a second read, not a stale one.
    let axis_b = test
        .client
        .provider_vocabulary_get("devboule-acp-stub", Some("stub-model"), false)
        .expect("vocabulary rpc");
    let DaemonMessage::ProviderVocabulary { features, .. } = axis_b else {
        panic!("unexpected reply");
    };
    let second = features.expect("the axis is answered");
    assert!(
        second.probing,
        "a second model gets its own read, not the first's answer: {second:?}"
    );
}
