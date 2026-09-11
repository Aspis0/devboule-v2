//! ACP proof against the local stub agent.
//!
//! This test is intentionally separate from the known-flaky ignored ConPTY
//! suite. It exercises direct stdio, malformed/partial-safe framing, stderr,
//! CREATE_NO_WINDOW, two-level Job Object assignment, and close teardown.

#![cfg(windows)]

use std::path::{Path, PathBuf};
use std::process::Child;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Barrier, Mutex};
use std::time::{Duration, Instant};

use devboule_daemon::{
    connect, current_user_sid, spawn_daemon, DaemonClient, EventHandler, RuntimePaths,
    SessionStateHandler,
};
use devboule_protocol::{
    AttentionReason, ClientHello, Cursor, ErrorCode, OwnerId, PermissionOutcome, Persistence,
    PersistenceKind, ResumeResult, SessionEvent, SessionKind, SessionStateSnapshot,
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
    // Cargo names this env var after the bin verbatim (dashes included). A
    // stale-binary fallback would silently run "the past" and report green —
    // on this machine the app holds devboule-daemon.exe open and a test that
    // cannot find the Cargo-provided binary must fail loudly instead.
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
            matches!(event, SessionEvent::PermissionRequest { tool_call_id, .. } if tool_call_id == "tool-perm")
        })
    });
    test.client
        .session_permission_respond(&session.id, "tool-perm", PermissionOutcome::AllowOnce)
        .expect("allow once");
    wait_for(&events, Duration::from_secs(5), |events| {
        events.iter().any(|event| {
            matches!(event, SessionEvent::PermissionResolved { tool_call_id } if tool_call_id == "tool-perm")
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
        matches!(event, SessionEvent::PermissionRequest { tool_call_id, .. } if tool_call_id == "tool-perm")
    }));
    drop(reattached);
    test.client
        .session_close(&session.id)
        .expect("close resolved session");
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
            SessionEvent::AgentUserMessage { message_id, text } if text == prompt => {
                Some(message_id.clone())
            }
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
        std::env::set_var("DEVBOULE_ACP_STUB_SET_CONFIG_FILE", &set_config_file);
        let mut env_names = vec![
            "DEVBOULE_ACP_COMMAND",
            "DEVBOULE_ACP_PROVIDER_ID",
            "DEVBOULE_TEST_NO_NETWORK",
            "DEVBOULE_ACP_STUB_PID_FILE",
            "DEVBOULE_ACP_STUB_CONSOLE_FILE",
            "DEVBOULE_ACP_STUB_SET_MODEL_FILE",
            "DEVBOULE_ACP_STUB_SET_MODEL_EFFORT_FILE",
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
