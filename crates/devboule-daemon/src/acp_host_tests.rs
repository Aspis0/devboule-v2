//! Tests for the ACP host: the session bridge, capability negotiation and teardown.

use super::super::permission_broker::PermissionBroker;
use super::super::{ConnHandle, SessionRuntime};
use super::{slice_lines, AcpHost, BoundedBuffer, MAX_FS_BYTES};
use crate::process_tree::JobObject;
use devboule_protocol::{PermissionOutcome, SessionEvent};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

struct TestDirs {
    host: Arc<AcpHost>,
    cwd: PathBuf,
    runtime: PathBuf,
}

fn unique_dir(label: &str) -> PathBuf {
    crate::test_dirs::test_temp_dir(&format!("devboule-acp-{label}"))
}

fn host() -> TestDirs {
    let cwd = unique_dir("cwd");
    let runtime = unique_dir("runtime");
    let host = AcpHost::new(
        cwd.clone(),
        runtime.clone(),
        Arc::new(JobObject::new().expect("job")),
    );
    host.set_session_id("stub-session".to_string());
    TestDirs { host, cwd, runtime }
}

#[test]
fn bounded_buffer_truncates_from_the_start_on_a_char_boundary() {
    let mut buffer = BoundedBuffer::new(3);
    buffer.push("éé".as_bytes());
    let (text, truncated) = buffer.snapshot();
    assert!(truncated);
    assert_eq!(text, "é");
}

#[test]
fn line_slice_is_one_based_and_keeps_newlines() {
    let contents = "a\nb\nc\n";
    assert_eq!(
        slice_lines(contents, Some(2), Some(1)).expect("slice"),
        "b\n"
    );
    assert_eq!(
        slice_lines(contents, Some(1), None).expect("slice"),
        contents
    );
    assert!(slice_lines(contents, Some(0), None).is_err());
    assert_eq!(slice_lines(contents, Some(9), None).expect("empty"), "");
}

#[test]
fn relative_path_is_rejected() {
    assert!(super::resolve_path(Path::new("relative.txt")).is_err());
    assert_eq!(MAX_FS_BYTES, 8 * 1024 * 1024);
}

#[test]
fn fs_read_and_write_round_trip_absolute_paths() {
    let test = host();
    let path = test.cwd.join("note.txt");
    let write = test
        .host
        .write_text_file(serde_json::json!({
            "sessionId": "stub-session",
            "path": path,
            "content": "one\ntwo\nthree\n"
        }))
        .expect("write");
    assert!(write.is_object());
    let read = test
        .host
        .read_text_file(serde_json::json!({
            "sessionId": "stub-session",
            "path": path,
            "line": 2,
            "limit": 1
        }))
        .expect("read");
    assert_eq!(read["content"], "two\n");
    let relative = test.host.write_text_file(serde_json::json!({
        "sessionId": "stub-session",
        "path": "relative.txt",
        "content": "no"
    }));
    assert!(relative.is_err());
    let _ = std::fs::remove_dir_all(test.cwd);
    let _ = std::fs::remove_dir_all(test.runtime);
}

#[test]
fn write_refuses_the_daemon_journal_even_when_the_path_is_absolute() {
    let test = host();
    let journal = test.runtime.join("journal.db");
    std::fs::write(&journal, b"precious").expect("seed journal");
    let result = test.host.write_text_file(serde_json::json!({
        "sessionId": "stub-session",
        "path": journal,
        "content": "wiped"
    }));
    assert!(result.is_err(), "journal write must be refused: {result:?}");
    assert!(
        test.host
            .read_text_file(serde_json::json!({
                "sessionId": "stub-session",
                "path": journal
            }))
            .is_err(),
        "journal read must be refused"
    );
    assert_eq!(
        std::fs::read(&journal).expect("journal still there"),
        b"precious"
    );
    let _ = std::fs::remove_dir_all(test.cwd);
    let _ = std::fs::remove_dir_all(test.runtime);
}

#[cfg(windows)]
#[test]
fn write_refuses_a_junction_that_escapes_the_workspace() {
    let test = host();
    let outside = unique_dir("outside");
    let link = test.cwd.join("escape");
    let status = std::process::Command::new("cmd")
        .args([
            "/c",
            "mklink",
            "/J",
            &link.to_string_lossy(),
            &outside.to_string_lossy(),
        ])
        .status()
        .expect("mklink");
    assert!(
        status.success(),
        "could not create junction for the escape test"
    );
    let stolen = link.join("stolen.txt");
    let result = test.host.write_text_file(serde_json::json!({
        "sessionId": "stub-session",
        "path": stolen,
        "content": "pwned"
    }));
    assert!(
        result.is_err(),
        "junction escape must be refused: {result:?}"
    );
    assert!(
        !outside.join("stolen.txt").exists(),
        "file was written through the junction"
    );
    let _ = std::fs::remove_dir(link);
    let _ = std::fs::remove_dir_all(outside);
    let _ = std::fs::remove_dir_all(test.cwd);
    let _ = std::fs::remove_dir_all(test.runtime);
}

#[test]
fn write_creates_parent_directories_inside_the_workspace() {
    let test = host();
    let path = test.cwd.join("src").join("new").join("file.txt");
    test.host
        .write_text_file(serde_json::json!({
            "sessionId": "stub-session",
            "path": path,
            "content": "hello\n"
        }))
        .expect("write into new nested directory");
    assert_eq!(
        std::fs::read_to_string(&path).expect("read back"),
        "hello\n"
    );
    let outside = test.cwd.join("..").join("escaped").join("nope.txt");
    let escaped = test.host.write_text_file(serde_json::json!({
        "sessionId": "stub-session",
        "path": outside,
        "content": "no"
    }));
    assert!(
        escaped.is_err(),
        "must not create parents outside the workspace: {escaped:?}"
    );
    let _ = std::fs::remove_dir_all(test.cwd);
    let _ = std::fs::remove_dir_all(test.runtime);
}

#[test]
fn write_refuses_a_hard_link_to_the_journal() {
    let test = host();
    let journal = test.runtime.join("journal.db");
    std::fs::write(&journal, b"precious").expect("seed journal");
    let alias = test.cwd.join("innocent.txt");
    std::fs::hard_link(&journal, &alias).expect("hard link");
    let result = test.host.write_text_file(serde_json::json!({
        "sessionId": "stub-session",
        "path": alias,
        "content": "wiped"
    }));
    assert!(
        result.is_err(),
        "hard-link journal write must be refused: {result:?}"
    );
    assert_eq!(
        std::fs::read(&journal).expect("journal still there"),
        b"precious"
    );
    let _ = std::fs::remove_dir_all(test.cwd);
    let _ = std::fs::remove_dir_all(test.runtime);
}

#[cfg(windows)]
#[test]
fn kill_terminates_a_process_that_would_not_exit_alone() {
    let test = host();
    let (broker, _runtime) = bind_gate(&test.host);
    let _allow = AutoAllow::start(Arc::clone(&broker));
    let host = test.host;
    let created = host
        .create_terminal(serde_json::json!({
            "sessionId": "stub-session",
            "command": "ping.exe",
            "args": ["-t", "127.0.0.1"]
        }))
        .expect("create");
    let terminal_id = created["terminalId"].clone();
    std::thread::sleep(std::time::Duration::from_millis(200));
    let before = host
        .terminal_output(serde_json::json!({
            "sessionId": "stub-session",
            "terminalId": terminal_id
        }))
        .expect("output before kill");
    assert!(
        before.get("exitStatus").is_none() || before["exitStatus"].is_null(),
        "ping -t exited before kill: {before}"
    );
    host.kill_terminal(serde_json::json!({
        "sessionId": "stub-session",
        "terminalId": terminal_id
    }))
    .expect("kill");
    let (tx, rx) = std::sync::mpsc::channel();
    let respond: super::RpcRespond = Arc::new(move |_, result| {
        let _ = tx.send(result);
    });
    host.wait_for_exit(
        serde_json::json!(1),
        serde_json::json!({
            "sessionId": "stub-session",
            "terminalId": terminal_id
        }),
        respond,
    );
    let exit = rx
        .recv_timeout(std::time::Duration::from_secs(5))
        .expect("wait_for_exit after kill")
        .expect("kill must make the process exit");
    assert!(
        exit.get("exitCode").is_some() || exit.get("signal").is_some(),
        "kill left no exit status: {exit}"
    );
    host.release_terminal(serde_json::json!({
        "sessionId": "stub-session",
        "terminalId": terminal_id
    }))
    .expect("release");
    let _ = std::fs::remove_dir_all(test.cwd);
    let _ = std::fs::remove_dir_all(test.runtime);
}

#[cfg(windows)]
#[test]
fn concurrent_creates_never_exceed_the_live_terminal_limit() {
    let cwd = unique_dir("term-cwd");
    let runtime = unique_dir("term-runtime");
    let host = AcpHost::with_terminal_limit(
        cwd.clone(),
        runtime.clone(),
        Arc::new(JobObject::new().expect("job")),
        2,
    );
    host.set_session_id("stub-session".to_string());
    let (broker, _runtime) = bind_gate(&host);
    let _allow = AutoAllow::start(Arc::clone(&broker));
    host.create_terminal(serde_json::json!({
        "sessionId": "stub-session",
        "command": "cmd.exe",
        "args": ["/c", "exit"]
    }))
    .expect("fill to one live terminal");
    let start = Arc::new(std::sync::Barrier::new(3));
    let successes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut threads = Vec::new();
    for _ in 0..2 {
        let host = Arc::clone(&host);
        let start = Arc::clone(&start);
        let successes = Arc::clone(&successes);
        threads.push(std::thread::spawn(move || {
            start.wait();
            if host
                .create_terminal(serde_json::json!({
                    "sessionId": "stub-session",
                    "command": "cmd.exe",
                    "args": ["/c", "exit"]
                }))
                .is_ok()
            {
                successes.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            }
        }));
    }
    start.wait();
    for thread in threads {
        thread.join().expect("create thread");
    }
    let live = host.live_terminal_count();
    let spawned = host.spawned_count();
    host.shutdown();
    let _ = std::fs::remove_dir_all(cwd);
    let _ = std::fs::remove_dir_all(runtime);
    assert!(
        live <= 2,
        "live terminals={live} successes={}",
        successes.load(std::sync::atomic::Ordering::SeqCst)
    );
    assert!(
        spawned <= 2,
        "started {spawned} processes under a limit of 2 (map live={live})"
    );
}

struct AutoAllow {
    stop: Arc<std::sync::atomic::AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl AutoAllow {
    fn start(broker: Arc<PermissionBroker>) -> Self {
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let handle = std::thread::spawn(move || {
            while !stop_thread.load(std::sync::atomic::Ordering::SeqCst) {
                for id in broker.pending_ids() {
                    let _ = broker.respond(&id, PermissionOutcome::AllowOnce);
                }
                std::thread::sleep(Duration::from_millis(5));
            }
        });
        Self {
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for AutoAllow {
    fn drop(&mut self) {
        self.stop.store(true, std::sync::atomic::Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn discard_broker() -> Arc<PermissionBroker> {
    PermissionBroker::for_test(Arc::new(|_, _| Ok(())))
}

fn bind_gate(host: &AcpHost) -> (Arc<PermissionBroker>, Arc<SessionRuntime>) {
    let broker = discard_broker();
    let runtime = SessionRuntime::for_acp("stub-session".to_string(), None, Arc::clone(&broker));
    host.bind_permission_gate(&broker, &runtime);
    (broker, runtime)
}

fn wait_for_pending(broker: &PermissionBroker, timeout: Duration) -> String {
    wait_for_pending_or_progress(broker, None, None, timeout).unwrap_or_else(|| {
        panic!(
            "timed out waiting for a pending terminal permission (pending={})",
            broker.pending_len()
        )
    })
}

fn wait_for_pending_or_progress(
    broker: &PermissionBroker,
    host: Option<&AcpHost>,
    thread: Option<&std::thread::JoinHandle<Result<serde_json::Value, super::RpcError>>>,
    timeout: Duration,
) -> Option<String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(id) = broker.pending_ids().into_iter().next() {
            return Some(id);
        }
        if thread.is_some_and(std::thread::JoinHandle::is_finished)
            || host.is_some_and(|host| host.spawned_count() > 0)
        {
            return None;
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn create_params(test: &TestDirs) -> serde_json::Value {
    serde_json::json!({
        "sessionId": "stub-session",
        "command": "cmd.exe",
        "args": ["/c", "echo", "gated"],
        "cwd": test.cwd,
    })
}

fn spawn_create(
    host: Arc<AcpHost>,
    params: serde_json::Value,
) -> std::thread::JoinHandle<Result<serde_json::Value, super::RpcError>> {
    std::thread::spawn(move || host.create_terminal(params))
}

#[test]
fn terminal_create_does_not_spawn_before_a_permission_decision() {
    let test = host();
    let (broker, _runtime) = bind_gate(&test.host);
    let thread = spawn_create(Arc::clone(&test.host), create_params(&test));
    let pending = wait_for_pending_or_progress(
        &broker,
        Some(&test.host),
        Some(&thread),
        Duration::from_secs(2),
    );
    let spawned = test.host.spawned_count();
    if let Some(ref id) = pending {
        let _ = broker.respond(id, PermissionOutcome::Deny);
    }
    let _ = thread.join();
    test.host.shutdown();
    let _ = std::fs::remove_dir_all(&test.cwd);
    let _ = std::fs::remove_dir_all(&test.runtime);
    assert_eq!(
        spawned, 0,
        "terminal/create spawned before a permission decision (spawned={spawned})"
    );
    assert!(
        pending.is_some(),
        "terminal/create never registered a host permission request"
    );
}

#[cfg(windows)]
#[test]
fn terminal_create_allow_spawns_the_command() {
    let test = host();
    let (broker, _runtime) = bind_gate(&test.host);
    let thread = spawn_create(Arc::clone(&test.host), create_params(&test));
    let id = wait_for_pending(&broker, Duration::from_secs(2));
    assert_eq!(test.host.spawned_count(), 0);
    broker
        .respond(&id, PermissionOutcome::AllowOnce)
        .expect("allow");
    let created = thread
        .join()
        .expect("create thread")
        .expect("allowed create");
    assert!(
        created.get("terminalId").is_some(),
        "missing terminalId: {created}"
    );
    assert_eq!(test.host.spawned_count(), 1);
    test.host.shutdown();
    let _ = std::fs::remove_dir_all(&test.cwd);
    let _ = std::fs::remove_dir_all(&test.runtime);
}

#[test]
fn terminal_create_allow_after_exit_does_not_spawn() {
    let test = host();
    let (broker, runtime) = bind_gate(&test.host);
    let thread = spawn_create(Arc::clone(&test.host), create_params(&test));
    let id = wait_for_pending(&broker, Duration::from_secs(2));
    runtime.mark_exited(Some(1));
    broker
        .respond(&id, PermissionOutcome::AllowOnce)
        .expect("allow after the agent is already dead");
    let error = thread
        .join()
        .expect("create thread")
        .expect_err("allow after OS death must not spawn");
    let spawned = test.host.spawned_count();
    let live = test.host.live_terminal_count();
    test.host.shutdown();
    let _ = std::fs::remove_dir_all(&test.cwd);
    let _ = std::fs::remove_dir_all(&test.runtime);
    assert_eq!(error.code, -32001);
    assert_eq!(error.message, "the agent is gone");
    assert_eq!(spawned, 0, "dead agent must not spawn (spawned={spawned})");
    assert_eq!(live, 0, "reserved slot must be released (live={live})");
}

#[cfg(windows)]
fn spawn_innocuous() -> std::process::Child {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    Command::new("cmd.exe")
        .args(["/d", "/c", "ping", "-n", "30", "127.0.0.1"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .expect("spawn ping")
}

#[cfg(windows)]
#[test]
fn os_death_cancels_pending_terminal_create_without_eof() {
    use crate::process_tree::ProcessHandle;
    use std::os::windows::io::AsRawHandle;
    let test = host();
    let (broker, runtime) = bind_gate(&test.host);
    let wake = Arc::clone(&broker);
    runtime.set_on_os_death(Arc::new(move || wake.close()));
    let mut child = spawn_innocuous();
    let handle = ProcessHandle::duplicate(AsRawHandle::as_raw_handle(&child)).expect("duplicate");
    runtime.install_os_handle(handle);
    let thread = spawn_create(Arc::clone(&test.host), create_params(&test));
    let _id = wait_for_pending(&broker, Duration::from_secs(2));
    child.kill().expect("kill ping");
    let _ = child.wait();
    let started = Instant::now();
    assert!(
        runtime.observe_os_liveness(),
        "OS observation must mark Exited without waiting on ACP stdout EOF"
    );
    let deadline = Instant::now() + Duration::from_millis(800);
    while !thread.is_finished() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(5));
    }
    let error = thread
        .join()
        .expect("create thread")
        .expect_err("OS death must deny the pending gate");
    let elapsed = started.elapsed();
    let spawned = test.host.spawned_count();
    let live = test.host.live_terminal_count();
    test.host.shutdown();
    let _ = std::fs::remove_dir_all(&test.cwd);
    let _ = std::fs::remove_dir_all(&test.runtime);
    assert!(
        elapsed < Duration::from_millis(800),
        "pending terminal/create stayed blocked for {elapsed:?} after OS death"
    );
    assert_eq!(error.code, -32001);
    assert_eq!(spawned, 0);
    assert_eq!(live, 0);
}

#[test]
fn terminal_create_deny_returns_server_error_and_releases_the_slot() {
    let test = host();
    let (broker, _runtime) = bind_gate(&test.host);
    let thread = spawn_create(Arc::clone(&test.host), create_params(&test));
    let id = wait_for_pending(&broker, Duration::from_secs(2));
    broker.respond(&id, PermissionOutcome::Deny).expect("deny");
    let error = thread
        .join()
        .expect("create thread")
        .expect_err("deny must not create a terminal");
    test.host.shutdown();
    let spawned = test.host.spawned_count();
    let live = test.host.live_terminal_count();
    let _ = std::fs::remove_dir_all(&test.cwd);
    let _ = std::fs::remove_dir_all(&test.runtime);
    assert_eq!(
        error.code, -32001,
        "deny must be JSON-RPC -32001: {error:?}"
    );
    assert_eq!(error.message, "the user denied this command");
    assert_eq!(spawned, 0, "deny must not spawn (spawned={spawned})");
    assert_eq!(live, 0, "deny must release the reserved slot (live={live})");
}

#[test]
fn terminal_create_waits_for_user_decision() {
    let test = host();
    let (broker, _runtime) = bind_gate(&test.host);
    let thread = spawn_create(Arc::clone(&test.host), create_params(&test));
    let _id = wait_for_pending(&broker, Duration::from_secs(2));
    std::thread::sleep(Duration::from_millis(50));
    assert!(
        !thread.is_finished(),
        "pending create must wait for the user"
    );
    broker.cancel_pending();
    let error = thread
        .join()
        .expect("create thread")
        .expect_err("cancel must deny");
    test.host.shutdown();
    let spawned = test.host.spawned_count();
    let live = test.host.live_terminal_count();
    let _ = std::fs::remove_dir_all(&test.cwd);
    let _ = std::fs::remove_dir_all(&test.runtime);
    assert_eq!(error.code, -32001);
    assert_eq!(error.message, "the user denied this command");
    assert_eq!(spawned, 0);
    assert_eq!(live, 0);
}

#[test]
fn cancel_pending_unblocks_a_pending_terminal_create_with_deny() {
    let test = host();
    let (broker, _runtime) = bind_gate(&test.host);
    let thread = spawn_create(Arc::clone(&test.host), create_params(&test));
    let _id = wait_for_pending(&broker, Duration::from_secs(2));
    let started = Instant::now();
    broker.cancel_pending();
    let error = thread
        .join()
        .expect("create thread")
        .expect_err("cancel_pending must deny the gate");
    let elapsed = started.elapsed();
    test.host.shutdown();
    let spawned = test.host.spawned_count();
    let live = test.host.live_terminal_count();
    let _ = std::fs::remove_dir_all(&test.cwd);
    let _ = std::fs::remove_dir_all(&test.runtime);
    assert!(
        elapsed < Duration::from_secs(2),
        "cancel_pending left the create thread blocked for {elapsed:?}"
    );
    assert_eq!(error.code, -32001);
    assert_eq!(error.message, "the user denied this command");
    assert_eq!(spawned, 0);
    assert_eq!(live, 0);
}

fn permission_rows(path: &Path) -> Vec<(String, String, serde_json::Value)> {
    let conn = rusqlite::Connection::open(path).expect("inspect journal");
    let mut stmt = conn
        .prepare("SELECT request_id, outcome, payload FROM permissions ORDER BY ts_ms, request_id")
        .expect("prepare");
    stmt.query_map([], |row| {
        let payload: Vec<u8> = row.get(2)?;
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, String>(1)?,
            serde_json::from_slice(&payload).expect("payload json"),
        ))
    })
    .expect("query")
    .map(|row| row.expect("row"))
    .collect()
}

#[test]
fn terminal_create_decisions_are_journaled_with_spawn_payload() {
    let test = host();
    let path = test.runtime.join("journal.db");
    let journal = Arc::new(crate::journal::Journal::open(&path).expect("journal"));
    journal
        .upsert_blocking(crate::journal::new_session_record(
            "stub-session",
            "owner",
            None,
            devboule_protocol::SessionKind::Acp,
            "Agent",
        ))
        .expect("upsert");
    let broker = discard_broker();
    let runtime = SessionRuntime::for_acp(
        "stub-session".to_string(),
        Some(Arc::clone(&journal)),
        Arc::clone(&broker),
    );
    test.host.bind_permission_gate(&broker, &runtime);

    let deny_thread = spawn_create(Arc::clone(&test.host), create_params(&test));
    let deny_id = wait_for_pending(&broker, Duration::from_secs(2));
    broker
        .respond(&deny_id, PermissionOutcome::Deny)
        .expect("deny");
    let _ = deny_thread.join();

    let cancel_thread = spawn_create(Arc::clone(&test.host), create_params(&test));
    let _ = wait_for_pending(&broker, Duration::from_secs(2));
    broker.cancel_pending();
    let _ = cancel_thread.join();

    journal.flush().expect("flush");
    let rows = permission_rows(&path);
    journal.shutdown();
    test.host.shutdown();
    let _ = std::fs::remove_dir_all(&test.cwd);
    let _ = std::fs::remove_dir_all(&test.runtime);

    let outcomes: Vec<&str> = rows
        .iter()
        .map(|(_, outcome, _)| outcome.as_str())
        .collect();
    assert!(outcomes.contains(&"deny"), "missing deny row: {outcomes:?}");
    assert!(
        outcomes.contains(&"cancelled"),
        "missing cancelled row: {outcomes:?}"
    );
    for (request_id, _, payload) in &rows {
        assert!(
            request_id.starts_with("terminal:"),
            "host permission id should be synthetic: {request_id}"
        );
        assert_eq!(payload["command"], "cmd.exe");
        assert_eq!(payload["args"][0], "/c");
        assert_eq!(payload["args"][1], "echo");
        assert_eq!(payload["args"][2], "gated");
        let cwd = payload["cwd"].as_str().expect("cwd");
        assert!(
            cwd.contains("devboule-acp-cwd") || Path::new(cwd) == test.cwd.as_path(),
            "journaled cwd {cwd} was not the spawn cwd"
        );
    }
}

fn wait_for_output_containing(
    host: &AcpHost,
    terminal_id: &serde_json::Value,
    needle: &str,
    timeout: Duration,
) -> String {
    let deadline = Instant::now() + timeout;
    let mut last = String::new();
    loop {
        if let Ok(output) = host.terminal_output(serde_json::json!({
            "sessionId": "stub-session",
            "terminalId": terminal_id
        })) {
            last = output["output"].as_str().unwrap_or("").to_string();
            if last.contains(needle) {
                return last;
            }
        }
        if Instant::now() >= deadline {
            panic!("terminal output never contained {needle:?}; last={last:?}");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_exit_code(host: &AcpHost, terminal_id: serde_json::Value) -> u32 {
    let (tx, rx) = std::sync::mpsc::channel();
    let respond: super::RpcRespond = Arc::new(move |_, result| {
        let _ = tx.send(result);
    });
    host.wait_for_exit(
        serde_json::json!(1),
        serde_json::json!({
            "sessionId": "stub-session",
            "terminalId": terminal_id
        }),
        respond,
    );
    let exit = rx
        .recv_timeout(Duration::from_secs(5))
        .expect("wait_for_exit")
        .expect("exit status");
    exit["exitCode"]
        .as_u64()
        .expect("exitCode")
        .try_into()
        .expect("exit code fits u32")
}

#[cfg(windows)]
#[test]
fn terminal_create_shell_line_without_args_runs_through_cmd() {
    let test = host();
    let (broker, _runtime) = bind_gate(&test.host);
    let _allow = AutoAllow::start(Arc::clone(&broker));
    let created = test
        .host
        .create_terminal(serde_json::json!({
            "sessionId": "stub-session",
            "command": "cmd /c echo devboule-gate-marker"
        }))
        .expect("create shell line");
    let terminal_id = created["terminalId"].clone();
    let output = wait_for_output_containing(
        &test.host,
        &terminal_id,
        "devboule-gate-marker",
        Duration::from_secs(5),
    );
    let code = wait_for_exit_code(&test.host, terminal_id.clone());
    test.host
        .release_terminal(serde_json::json!({
            "sessionId": "stub-session",
            "terminalId": terminal_id
        }))
        .expect("release");
    test.host.shutdown();
    let _ = std::fs::remove_dir_all(&test.cwd);
    let _ = std::fs::remove_dir_all(&test.runtime);
    assert!(
        output.contains("devboule-gate-marker"),
        "shell-line output missed the marker: {output:?}"
    );
    assert_eq!(code, 0, "shell-line command must exit 0");
}

#[cfg(windows)]
#[test]
fn terminal_create_with_args_keeps_argv_semantics() {
    let test = host();
    let (broker, _runtime) = bind_gate(&test.host);
    let _allow = AutoAllow::start(Arc::clone(&broker));
    let created = test
        .host
        .create_terminal(serde_json::json!({
            "sessionId": "stub-session",
            "command": "cmd.exe",
            "args": ["/c", "echo", "devboule-argv-marker"]
        }))
        .expect("create argv");
    let terminal_id = created["terminalId"].clone();
    let output = wait_for_output_containing(
        &test.host,
        &terminal_id,
        "devboule-argv-marker",
        Duration::from_secs(5),
    );
    let code = wait_for_exit_code(&test.host, terminal_id.clone());
    test.host
        .release_terminal(serde_json::json!({
            "sessionId": "stub-session",
            "terminalId": terminal_id
        }))
        .expect("release");
    test.host.shutdown();
    let _ = std::fs::remove_dir_all(&test.cwd);
    let _ = std::fs::remove_dir_all(&test.runtime);
    assert!(
        output.contains("devboule-argv-marker"),
        "argv output missed the marker: {output:?}"
    );
    assert_eq!(code, 0);
}

#[cfg(windows)]
#[test]
fn terminal_create_shell_line_permission_shows_the_real_spawn_argv() {
    let test = host();
    let (broker, runtime) = bind_gate(&test.host);
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach");
    conn.track_with_agent_replay(
        "stub-session",
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    let line = "cmd /c echo devboule-gate-marker";
    let thread = spawn_create(
        Arc::clone(&test.host),
        serde_json::json!({
            "sessionId": "stub-session",
            "command": line
        }),
    );
    let id = wait_for_pending(&broker, Duration::from_secs(2));
    let events = conn.pull_events();
    broker
        .respond(&id, PermissionOutcome::Deny)
        .expect("deny after inspecting the prompt");
    let _ = thread.join();
    test.host.shutdown();
    let _ = std::fs::remove_dir_all(&test.cwd);
    let _ = std::fs::remove_dir_all(&test.runtime);
    let request = events.iter().find_map(|event| match &event.envelope.event {
        SessionEvent::PermissionRequest { command, args, .. } => {
            Some((command.clone(), args.clone()))
        }
        _ => None,
    });
    let (command, args) = request.expect("shell-line create must publish a PermissionRequest");
    assert_eq!(command.as_deref(), Some(line));
    assert_eq!(
        args, None,
        "shell-line prompt must show the original line, not the tempfile argv"
    );
}

#[cfg(windows)]
#[test]
fn terminal_create_shell_line_preserves_quoted_echo_text() {
    let test = host();
    let (broker, _runtime) = bind_gate(&test.host);
    let _allow = AutoAllow::start(Arc::clone(&broker));
    let created = test
        .host
        .create_terminal(serde_json::json!({
            "sessionId": "stub-session",
            "command": "echo \"hello world\""
        }))
        .expect("create quoted echo");
    let terminal_id = created["terminalId"].clone();
    let output = wait_for_output_containing(
        &test.host,
        &terminal_id,
        "hello world",
        Duration::from_secs(5),
    );
    let code = wait_for_exit_code(&test.host, terminal_id.clone());
    test.host
        .release_terminal(serde_json::json!({
            "sessionId": "stub-session",
            "terminalId": terminal_id
        }))
        .expect("release");
    test.host.shutdown();
    let _ = std::fs::remove_dir_all(&test.cwd);
    let _ = std::fs::remove_dir_all(&test.runtime);
    assert!(
        !output.contains(r#"\""#),
        "cmd.exe saw Win32-escaped quotes: {output:?}"
    );
    assert_eq!(code, 0);
}

#[test]
fn close_rejects_later_terminal_create_without_registering() {
    let test = host();
    let (broker, runtime) = bind_gate(&test.host);
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach");
    conn.track_with_agent_replay(
        "stub-session",
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    broker.close();
    let started = Instant::now();
    let error = test
        .host
        .create_terminal(create_params(&test))
        .expect_err("closed broker must deny create");
    let elapsed = started.elapsed();
    let events = conn.pull_events();
    test.host.shutdown();
    let _ = std::fs::remove_dir_all(&test.cwd);
    let _ = std::fs::remove_dir_all(&test.runtime);
    assert!(
        elapsed < Duration::from_secs(1),
        "closed create blocked {elapsed:?}"
    );
    assert_eq!(error.code, -32001);
    assert_eq!(test.host.spawned_count(), 0);
    assert_eq!(broker.pending_len(), 0);
    assert!(
        !events.iter().any(|event| {
            matches!(event.envelope.event, SessionEvent::PermissionRequest { .. })
        }),
        "closed broker published a permission request: {events:?}"
    );
}

#[test]
fn terminal_create_denies_when_typed_permissions_are_absent() {
    let test = host();
    let (_broker, runtime) = bind_gate(&test.host);
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, false)
        .expect("attach without typed_permissions");
    conn.track_with_agent_replay(
        "stub-session",
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    let started = Instant::now();
    let error = test
        .host
        .create_terminal(create_params(&test))
        .expect_err("missing typed_permissions must deny");
    let elapsed = started.elapsed();
    let events = conn.pull_events();
    test.host.shutdown();
    let _ = std::fs::remove_dir_all(&test.cwd);
    let _ = std::fs::remove_dir_all(&test.runtime);
    assert!(
        elapsed < Duration::from_secs(1),
        "capability deny blocked {elapsed:?}"
    );
    assert_eq!(error.code, -32001);
    assert_eq!(test.host.spawned_count(), 0);
    assert!(
        !events.iter().any(|event| {
            matches!(event.envelope.event, SessionEvent::PermissionRequest { .. })
        }),
        "incapable client was shown a permission prompt: {events:?}"
    );
}

#[test]
fn terminal_create_permission_includes_env() {
    let test = host();
    let (broker, runtime) = bind_gate(&test.host);
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach");
    conn.track_with_agent_replay(
        "stub-session",
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    let thread = spawn_create(
        Arc::clone(&test.host),
        serde_json::json!({
            "sessionId": "stub-session",
            "command": "cmd.exe",
            "args": ["/c", "exit"],
            "env": [{ "name": "DB_GATE", "value": "SAFE" }]
        }),
    );
    let id = wait_for_pending(&broker, Duration::from_secs(2));
    let events = conn.pull_events();
    broker.respond(&id, PermissionOutcome::Deny).expect("deny");
    let _ = thread.join();
    test.host.shutdown();
    let _ = std::fs::remove_dir_all(&test.cwd);
    let _ = std::fs::remove_dir_all(&test.runtime);
    let env = events.iter().find_map(|event| match &event.envelope.event {
        SessionEvent::PermissionRequest { env, .. } => env.clone(),
        _ => None,
    });
    let env = env.expect("permission must include env");
    assert_eq!(env.len(), 1);
    assert_eq!(env[0].name, "DB_GATE");
    assert_eq!(env[0].value, "SAFE");
}

#[test]
fn dsr_counter_counts_every_query_in_a_chunk() {
    assert_eq!(super::count_dsr_queries(b"\x1b[6n\x1b[6n"), 2);
    assert_eq!(super::count_dsr_queries(b"abc"), 0);
    assert_eq!(super::count_dsr_queries(b"\x1b[6nX\x1b[6n"), 2);
}

#[cfg(windows)]
#[test]
fn shell_batch_paths_do_not_collide_across_hosts() {
    let dir = unique_dir("batch");
    let plan = super::spawn_plan("echo collide", &[]);
    let first = super::prepare_spawn(&plan, &dir, "t-1").expect("first batch");
    let second = super::prepare_spawn(&plan, &dir, "t-1").expect("second batch");
    let left = first.batch_file.expect("first path");
    let right = second.batch_file.expect("second path");
    let same = left == right;
    let _ = std::fs::remove_file(&left);
    let _ = std::fs::remove_file(&right);
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        !same,
        "two hosts sharing a runtime_dir and terminal_id must not share a .cmd path: {left:?}"
    );
}

#[cfg(windows)]
#[test]
fn terminal_kill_deletes_the_shell_batch_file() {
    let test = host();
    let (broker, _runtime) = bind_gate(&test.host);
    let _allow = AutoAllow::start(Arc::clone(&broker));
    let created = test
        .host
        .create_terminal(serde_json::json!({
            "sessionId": "stub-session",
            "command": "echo hello"
        }))
        .expect("create");
    let terminal_id = created["terminalId"].as_str().expect("id").to_string();
    let suffix = format!("-{terminal_id}.cmd");
    let batch = std::fs::read_dir(&test.runtime)
        .expect("runtime dir")
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .find(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("acp-") && name.ends_with(&suffix))
        })
        .expect("spawn must write a unique acp-*.cmd");
    assert!(batch.is_file(), "spawn must write {batch:?}");
    test.host
        .kill_terminal(serde_json::json!({
            "sessionId": "stub-session",
            "terminalId": terminal_id
        }))
        .expect("kill");
    let exists_after_kill = batch.is_file();
    test.host.shutdown();
    let _ = std::fs::remove_dir_all(&test.cwd);
    let _ = std::fs::remove_dir_all(&test.runtime);
    assert!(
        !exists_after_kill,
        "kill must delete the shell batch file before release"
    );
}

#[cfg(windows)]
#[test]
fn terminal_create_shell_line_runs_when_runtime_dir_has_a_space() {
    let cwd = unique_dir("cwd");
    let runtime = crate::test_dirs::test_temp_dir("acp gate space");
    assert!(
        runtime.to_string_lossy().contains(' '),
        "fixture runtime dir must contain a space: {runtime:?}"
    );
    let host = AcpHost::new(
        cwd.clone(),
        runtime.clone(),
        Arc::new(JobObject::new().expect("job")),
    );
    host.set_session_id("stub-session".to_string());
    let (broker, _session) = bind_gate(&host);
    let _allow = AutoAllow::start(Arc::clone(&broker));
    let created = host
        .create_terminal(serde_json::json!({
            "sessionId": "stub-session",
            "command": "echo SPACE_OK"
        }))
        .expect("create");
    let terminal_id = created["terminalId"].clone();
    let output =
        wait_for_output_containing(&host, &terminal_id, "SPACE_OK", Duration::from_secs(5));
    let code = wait_for_exit_code(&host, terminal_id.clone());
    host.release_terminal(serde_json::json!({
        "sessionId": "stub-session",
        "terminalId": terminal_id
    }))
    .expect("release");
    host.shutdown();
    let _ = std::fs::remove_dir_all(&cwd);
    let _ = std::fs::remove_dir_all(&runtime);
    assert!(
        output.contains("SPACE_OK"),
        "spaced runtime dir missed the marker: {output:?}"
    );
    assert_eq!(code, 0, "spaced runtime dir command must exit 0");
}
