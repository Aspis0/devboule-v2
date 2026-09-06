//! ACP proof against the local stub agent.
//!
//! This test is intentionally separate from the known-flaky ignored ConPTY
//! suite. It exercises direct stdio, malformed/partial-safe framing, stderr,
//! CREATE_NO_WINDOW, two-level Job Object assignment, and close teardown.

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use devboule_daemon::{
    connect, current_user_sid, spawn_daemon, DaemonClient, EventHandler, RuntimePaths,
};
use devboule_protocol::{
    ClientHello, OwnerId, PermissionOutcome, Persistence, PersistenceKind, ResumeResult,
    SessionEvent, SessionKind,
};
use rusqlite::Connection;
use windows_sys::Win32::Foundation::{CloseHandle, WAIT_OBJECT_0};
use windows_sys::Win32::System::JobObjects::IsProcessInJob;
use windows_sys::Win32::System::Threading::{
    OpenProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE,
};

static TEST_LOCK: Mutex<()> = Mutex::new(());

fn lock_tests() -> std::sync::MutexGuard<'static, ()> {
    TEST_LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

fn daemon_bin() -> PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_devboule_daemon") {
        return PathBuf::from(path);
    }
    if let Some(path) = option_env!("CARGO_BIN_EXE_devboule_daemon") {
        return PathBuf::from(path);
    }
    target_bin("devboule-daemon.exe")
}

fn stub_bin() -> PathBuf {
    if let Ok(path) = std::env::var("CARGO_BIN_EXE_devboule_acp_stub") {
        return PathBuf::from(path);
    }
    if let Some(path) = option_env!("CARGO_BIN_EXE_devboule_acp_stub") {
        return PathBuf::from(path);
    }
    target_bin("devboule-acp-stub.exe")
}

fn target_bin(name: &str) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.pop();
    path.pop();
    path.push("target");
    path.push(if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    });
    path.push(name);
    path
}

fn unique_dir() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let dir = std::env::temp_dir().join(format!(
        "devboule acp {}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
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
        let dir = unique_dir();
        let paths = RuntimePaths::from_dir(&dir);
        let child = spawn_daemon(&daemon_bin(), &paths).expect("spawn daemon");
        let harness = Self {
            dir,
            paths,
            child: Some(child),
        };
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            if connect(&harness.paths, hello("wait")).is_ok() {
                return harness;
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
fn acp_child_is_contained_in_the_two_level_job() {
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
    let test = AcpTest::new(&[]);
    let session = test.create_session();
    test.client
        .session_send(&session.id, "please request permission")
        .expect("prompt");

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
            matches!(event, SessionEvent::PermissionRequest { tool_call_id, .. } if tool_call_id == "tool-perm")
        })
    });
    test.client
        .session_permission_respond(&session.id, "tool-perm", PermissionOutcome::AllowOnce)
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

    // The journal row was upserted before spawn; a failed spawn must end it,
    // or the roster renders a phantom recovered session with zero events.
    // The journal writer is asynchronous, so poll with a deadline.
    let deadline = Instant::now() + Duration::from_secs(10);
    let state = loop {
        let sessions = test.client.sessions_list().expect("sessions list");
        if let Some(session) = sessions
            .iter()
            .find(|session| session.kind == SessionKind::Acp)
        {
            break session.state.clone();
        }
        assert!(
            Instant::now() < deadline,
            "the failed session never appeared in sessions_list"
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

struct AcpTest {
    observation_dir: PathBuf,
    _harness: Harness,
    client: DaemonClient,
    _env: EnvGuard,
}

impl AcpTest {
    fn new(extra_args: &[&str]) -> Self {
        let observation_dir = unique_dir();
        let pid_file = observation_dir.join("stub pid.txt");
        let console_file = observation_dir.join("stub console.txt");
        let mut argv = vec![stub_bin().to_string_lossy().into_owned()];
        argv.extend(extra_args.iter().map(|arg| (*arg).to_string()));
        let command = serde_json::to_string(&argv).expect("ACP argv");
        std::env::set_var("DEVBOULE_ACP_COMMAND", command);
        std::env::set_var("DEVBOULE_ACP_PROVIDER_ID", "devboule-acp-stub");
        std::env::set_var("DEVBOULE_ACP_STUB_PID_FILE", &pid_file);
        std::env::set_var("DEVBOULE_ACP_STUB_CONSOLE_FILE", &console_file);
        let env = EnvGuard {
            names: vec![
                "DEVBOULE_ACP_COMMAND",
                "DEVBOULE_ACP_PROVIDER_ID",
                "DEVBOULE_ACP_STUB_PID_FILE",
                "DEVBOULE_ACP_STUB_CONSOLE_FILE",
            ],
        };
        let harness = Harness::spawn();
        let client = harness.client();
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

    fn pid_file(&self) -> PathBuf {
        self.observation_dir.join("stub pid.txt")
    }

    fn console_file(&self) -> PathBuf {
        self.observation_dir.join("stub console.txt")
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
