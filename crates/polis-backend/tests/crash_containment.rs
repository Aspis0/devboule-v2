//! A plugin crash degrades its surface and nothing else. Kill the backend
//! mid-request; the host session must error; a respawn must work.

#![cfg(windows)]

use std::collections::BTreeMap;
use std::time::Duration;

use devboule_daemon::{connect_pipe, Framed};
use devboule_plugin_rpc::{host_owner, PluginError, PluginSession, SpawnSpec, HOST_PID_ENV};
use devboule_augur::{shipped_rule_matches, FindingId, Ledger};
use oracle_core::OracleDataPaths;
use devboule_protocol::{
    caps, plugin_backend_capabilities, ClientHello, ClientMessage, DaemonMessage, ErrorCode,
};
use std::sync::Arc;

#[test]
fn backend_started_through_cmd_parent_reaches_the_pipe() {
    let pipe_name = devboule_plugin_rpc::unique_pipe_name("cmd-parent");
    let mut child = spawn_backend_through_cmd(&pipe_name);
    let mut last_error = None;
    let file = (0..50)
        .find_map(|_| match connect_pipe(&pipe_name) {
            Ok(file) => Some(file),
            Err(error) => {
                last_error = Some(error);
                if child.try_wait().expect("poll cmd parent").is_some() {
                    let mut stderr = child.stderr.take().expect("cmd stderr");
                    let mut stderr_text = String::new();
                    std::io::Read::read_to_string(&mut stderr, &mut stderr_text)
                        .expect("read cmd stderr");
                    panic!(
                        "cmd parent exited before the backend pipe was ready: {last_error:?}; stderr={stderr_text:?}"
                    );
                }
                std::thread::sleep(Duration::from_millis(50));
                None
            }
        })
        .unwrap_or_else(|| panic!("connect through cmd parent: {last_error:?}"));

    let framed = Framed::new(file);
    framed
        .send(&ClientMessage::Hello(ClientHello::plugin_host(
            host_owner().expect("owner"),
            "devboule-app",
            plugin_backend_capabilities(),
            BTreeMap::new(),
        )))
        .expect("hello");
    assert!(matches!(
        framed
            .recv_timeout::<DaemonMessage>(Duration::from_secs(2))
            .expect("hello reply"),
        DaemonMessage::Hello(_)
    ));
    framed
        .send(&ClientMessage::Shutdown { id: 1 })
        .expect("shutdown");
    assert!(matches!(
        framed
            .recv_timeout::<DaemonMessage>(Duration::from_secs(2))
            .expect("shutdown reply"),
        DaemonMessage::Shutdown { accepted: true, .. }
    ));
    child.wait().expect("cmd parent exit");
}

#[test]
fn copied_backend_starts_from_an_empty_directory_without_sidecar_dlls() {
    let source = std::path::Path::new(env!("CARGO_BIN_EXE_polis-backend"));
    let temp = tempfile::tempdir().expect("tempdir");
    let copied = temp.path().join("polis-backend.exe");
    std::fs::copy(source, &copied).expect("copy backend by itself");
    let pipe_name = devboule_plugin_rpc::unique_pipe_name("self-contained");
    let mut child = std::process::Command::new(&copied)
        .current_dir(temp.path())
        .arg("--pipe")
        .arg(&pipe_name)
        .env(HOST_PID_ENV, std::process::id().to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn copied backend");

    let mut last_error = None;
    let file = (0..50)
        .find_map(|_| match connect_pipe(&pipe_name) {
            Ok(file) => Some(file),
            Err(error) => {
                last_error = Some(error);
                if child.try_wait().expect("poll copied backend").is_some() {
                    let mut stderr = child.stderr.take().expect("copied stderr");
                    let mut text = String::new();
                    std::io::Read::read_to_string(&mut stderr, &mut text).expect("read stderr");
                    panic!("copied backend exited before handshake: {last_error:?}; stderr={text:?}");
                }
                std::thread::sleep(Duration::from_millis(50));
                None
            }
        })
        .unwrap_or_else(|| panic!("connect copied backend: {last_error:?}"));
    let framed = Framed::new(file);
    framed
        .send(&ClientMessage::Hello(ClientHello::plugin_host(
            host_owner().expect("owner"),
            "devboule-app",
            plugin_backend_capabilities(),
            BTreeMap::new(),
        )))
        .expect("hello copied backend");
    assert!(matches!(
        framed
            .recv_timeout::<DaemonMessage>(Duration::from_secs(2))
            .expect("copied hello reply"),
        DaemonMessage::Hello(_)
    ));
    framed
        .send(&ClientMessage::Shutdown { id: 2 })
        .expect("shutdown copied backend");
    assert!(matches!(
        framed
            .recv_timeout::<DaemonMessage>(Duration::from_secs(2))
            .expect("copied shutdown reply"),
        DaemonMessage::Shutdown { accepted: true, .. }
    ));
    child.wait().expect("copied backend exit");
}

fn spawn_backend_through_cmd(pipe_name: &str) -> std::process::Child {
    let binary = std::path::Path::new(env!("CARGO_BIN_EXE_polis-backend"));
    let command_line = format!("polis-backend.exe --pipe {pipe_name}");
    std::process::Command::new("cmd.exe")
        .args(["/D", "/S", "/C", &command_line])
        .current_dir(binary.parent().expect("backend directory"))
        .env(HOST_PID_ENV, std::process::id().to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn cmd parent")
}

fn spec(root: &std::path::Path) -> SpawnSpec {
    let mut grants = BTreeMap::new();
    grants.insert(
        caps::WORKSPACE_ROOT.to_string(),
        root.to_string_lossy().into_owned(),
    );
    SpawnSpec {
        binary: std::path::PathBuf::from(env!("CARGO_BIN_EXE_polis-backend")),
        plugin_id: "polis".to_string(),
        capabilities: plugin_backend_capabilities(),
        grants,
        owner: host_owner().expect("owner"),
        hang_ms: None,
    }
}

#[test]
fn inherited_hang_ms_does_not_sleep_when_spec_leaves_it_unset() {
    let previous = std::env::var_os("DEVBOULE_PLUGIN_HANG_MS");
    std::env::set_var("DEVBOULE_PLUGIN_HANG_MS", "8000");
    struct Restore(Option<std::ffi::OsString>);
    impl Drop for Restore {
        fn drop(&mut self) {
            match &self.0 {
                Some(value) => std::env::set_var("DEVBOULE_PLUGIN_HANG_MS", value),
                None => std::env::remove_var("DEVBOULE_PLUGIN_HANG_MS"),
            }
        }
    }
    let _restore = Restore(previous);

    let dir = tempfile::tempdir().expect("tempdir");
    let session = PluginSession::spawn(spec(dir.path())).expect("spawn");
    let started = std::time::Instant::now();
    let value = session
        .invoke(caps::WORKSPACE_ROOT, None)
        .expect("workspace.root");
    let elapsed = started.elapsed();
    assert_eq!(value["status"], "ok");
    assert!(
        elapsed < Duration::from_secs(2),
        "child inherited DEVBOULE_PLUGIN_HANG_MS=8000 and slept: {elapsed:?}"
    );
}

#[test]
fn workspace_root_round_trips_over_the_pipe() {
    let dir = tempfile::tempdir().expect("tempdir");
    let session = PluginSession::spawn(spec(dir.path())).expect("spawn");
    session.ping().expect("ping");
    let value = session
        .invoke(caps::WORKSPACE_ROOT, None)
        .expect("workspace.root");
    assert_eq!(
        value["root"].as_str().expect("root"),
        dir.path().to_string_lossy().as_ref()
    );
    assert_eq!(value["status"], "ok");
}

#[test]
fn city_get_round_trips_over_the_backend_pipe() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(dir.path().join("src")).expect("src directory");
    std::fs::write(dir.path().join("src/main.ts"), "export const main = 1;\n")
        .expect("main source");
    let session = PluginSession::spawn(spec(dir.path())).expect("spawn");
    let city = session
        .invoke(caps::CITY_GET, None)
        .expect("city.get");
    assert_eq!(city["dataSource"], "host");
    assert_eq!(city["files"][0]["id"], "src/main.ts");
    assert_eq!(city["agents"], serde_json::json!([]));
    assert_eq!(city["findings"], serde_json::json!([]));
}

#[test]
fn findings_get_round_trips_over_the_backend_pipe() {
    let prefix = "AKIA";
    let body = "BHCEFGHIJKLMNOPQ";
    let aws = format!("{prefix}{body}");
    assert!(
        shipped_rule_matches("aws-access-token", &aws),
        "assembled fixture no longer matches gitleaks aws-access-token"
    );

    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(dir.path().join("src")).expect("src directory");
    std::fs::write(
        dir.path().join("src/auth.rs"),
        format!("const KEY: &str = \"{aws}\";\n"),
    )
    .expect("auth source");
    let session = PluginSession::spawn(spec(dir.path())).expect("spawn");
    let value = session
        .invoke(caps::FINDINGS_GET, None)
        .expect("findings.get");
    assert_eq!(value["scanned"], true);
    let findings = value["findings"].as_array().expect("findings array");
    let secret = findings
        .iter()
        .find(|finding| finding["rule"] == "aws-access-token")
        .expect("scan should report the assembled AWS token");
    assert_eq!(secret["fileId"], "src/auth.rs");
    assert_eq!(secret["severity"], "inferno");
    assert!(secret["id"].as_str().expect("id").len() == 64);
    assert!(secret.get("evidence").is_none(), "inspector fields must stay off the wire");
    assert!(secret.get("startLine").is_none());
    let completed = value["completed"].as_array().expect("completed");
    assert!(completed.iter().any(|id| id == "secrets"));
    assert!(completed.iter().any(|id| id == "untested"));
    assert!(
        !completed.iter().any(|id| id == "clippy"),
        "clippy ran on the request path: {completed:?}"
    );
}

fn inspect_error(session: &PluginSession, payload: Option<serde_json::Value>) -> PluginError {
    session
        .invoke(caps::FINDING_INSPECT, payload)
        .expect_err("finding.inspect must refuse")
}

fn assert_invalid_request(error: PluginError, message: &str) {
    match error {
        PluginError::Handshake(wire) => {
            assert_eq!(wire.code, ErrorCode::InvalidRequest);
            assert_eq!(wire.message, message);
        }
        other => panic!("expected InvalidRequest handshake, got {other:?}"),
    }
}

#[test]
fn finding_inspect_round_trips_over_the_backend_pipe() {
    let prefix = "AKIA";
    let body = "BHCEFGHIJKLMNOPQ";
    let aws = format!("{prefix}{body}");
    assert!(
        shipped_rule_matches("aws-access-token", &aws),
        "assembled fixture no longer matches gitleaks aws-access-token"
    );

    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(dir.path().join("src")).expect("src directory");
    std::fs::write(
        dir.path().join("src/auth.rs"),
        format!("const KEY: &str = \"{aws}\";\n"),
    )
    .expect("auth source");
    let session = PluginSession::spawn(spec(dir.path())).expect("spawn");
    let list = session
        .invoke(caps::FINDINGS_GET, None)
        .expect("findings.get");
    let secret = list["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .find(|finding| finding["rule"] == "aws-access-token")
        .expect("secret finding");
    let id = secret["id"].as_str().expect("id").to_string();

    let inspected = session
        .invoke(
            caps::FINDING_INSPECT,
            Some(serde_json::json!({ "id": id })),
        )
        .expect("finding.inspect");
    assert_eq!(inspected["id"], id);
    assert_eq!(inspected["rule"], "aws-access-token");
    assert_eq!(inspected["severity"], "inferno");
    assert_eq!(inspected["source"], "secrets");
    assert_eq!(inspected["startLine"], 1);
    assert_eq!(inspected["endLine"], 1);
    assert!(inspected["title"].as_str().expect("title").len() > 0);
    let locations = inspected["locations"].as_array().expect("locations");
    assert!(!locations.is_empty());
    assert_eq!(locations[0]["startLine"], 1);
    assert_eq!(locations[0]["endLine"], 1);
    let keys: Vec<&str> = inspected
        .as_object()
        .expect("object")
        .keys()
        .map(String::as_str)
        .collect();
    for key in &keys {
        assert!(
            matches!(
                *key,
                "id" | "rule" | "severity" | "title" | "source" | "startLine" | "endLine" | "locations"
            ),
            "inspector must not ship extra fields, found {key}"
        );
    }
    assert_eq!(keys.len(), 8, "exactly the eight contract keys: {keys:?}");
    assert!(inspected.get("evidence").is_none(), "inspector fields must stay off the wire");
    assert!(inspected.get("snippet").is_none());
    assert!(inspected.get("file").is_none());
    assert!(inspected.get("fileId").is_none());
}

#[test]
fn finding_inspect_unknown_and_invalid_ids_are_invalid_request() {
    let dir = tempfile::tempdir().expect("tempdir");
    let session = PluginSession::spawn(spec(dir.path())).expect("spawn");
    session.invoke(caps::FINDINGS_GET, None).expect("scan to open the ledger");

    assert_invalid_request(
        inspect_error(&session, Some(serde_json::json!({"id": "a".repeat(64)}))),
        "finding not found",
    );
    assert_invalid_request(
        inspect_error(&session, None),
        "finding.inspect requires a 64-hex id",
    );
    assert_invalid_request(
        inspect_error(&session, Some(serde_json::json!({}))),
        "finding.inspect requires a 64-hex id",
    );
    assert_invalid_request(
        inspect_error(&session, Some(serde_json::json!({"id": ""}))),
        "finding.inspect requires a 64-hex id",
    );
    assert_invalid_request(
        inspect_error(&session, Some(serde_json::json!({"id": "short"}))),
        "finding.inspect requires a 64-hex id",
    );
    assert_invalid_request(
        inspect_error(&session, Some(serde_json::json!({"id": "g".repeat(64)}))),
        "finding.inspect requires a 64-hex id",
    );
    assert_invalid_request(
        inspect_error(&session, Some(serde_json::json!({"id": 1}))),
        "finding.inspect requires a 64-hex id",
    );
}

#[test]
fn finding_inspect_dismissed_id_is_indistinguishable_from_absent() {
    let prefix = "AKIA";
    let body = "BHCEFGHIJKLMNOPQ";
    let aws = format!("{prefix}{body}");
    assert!(
        shipped_rule_matches("aws-access-token", &aws),
        "assembled fixture no longer matches gitleaks aws-access-token"
    );
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(dir.path().join("src")).expect("src directory");
    std::fs::write(
        dir.path().join("src/auth.rs"),
        format!("const KEY: &str = \"{aws}\";\n"),
    )
    .expect("auth source");
    let session = PluginSession::spawn(spec(dir.path())).expect("spawn");
    let list = session
        .invoke(caps::FINDINGS_GET, None)
        .expect("findings.get");
    let id = list["findings"]
        .as_array()
        .expect("findings")
        .iter()
        .find(|finding| finding["rule"] == "aws-access-token")
        .expect("secret")["id"]
        .as_str()
        .expect("id")
        .to_string();

    let canonical = std::fs::canonicalize(dir.path()).expect("canonical");
    let ledger = Ledger::open(&OracleDataPaths::from_root_without_env(&canonical).augur_ledger())
        .expect("ledger");
    let finding_id = FindingId::from_stored(id.clone()).expect("id");
    ledger.dismiss(&finding_id).expect("dismiss");

    assert_invalid_request(
        inspect_error(&session, Some(serde_json::json!({"id": id}))),
        "finding not found",
    );
}

#[test]
fn findings_get_production_invoke_survives_a_dispatch_slower_than_five_seconds() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("src.rs"), "pub fn x() {}\n").expect("source");
    let mut hung = spec(dir.path());
    hung.hang_ms = Some(6_000);
    let session = PluginSession::spawn(hung).expect("spawn");
    let started = std::time::Instant::now();
    let value = session
        .invoke(caps::FINDINGS_GET, None)
        .expect("production invoke() must wait out a findings.get longer than 5s");
    assert!(
        started.elapsed() >= Duration::from_secs(6),
        "dispatch did not actually hang: {:?}",
        started.elapsed()
    );
    assert_eq!(value["scanned"], true);
}

#[test]
fn concurrent_ensure_does_not_kill_a_healthy_busy_backend() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("src.rs"), "pub fn x() {}\n").expect("source");
    let mut hung = spec(dir.path());
    hung.hang_ms = Some(6_000);
    let session = std::sync::Arc::new(PluginSession::spawn(hung).expect("spawn"));
    let pid_before = session.pid();
    let first = session.clone();
    let worker = std::thread::spawn(move || first.invoke(caps::FINDINGS_GET, None));
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        session.invoke_in_flight(),
        "host must see the long dispatch as in-flight"
    );
    assert_eq!(session.pid(), pid_before, "ping-kill raced the in-flight scan");
    session
        .ping()
        .expect("a busy backend is not dead; ping must not fail the session");
    assert_eq!(session.pid(), pid_before, "ping must not replace the process");
    let second = session
        .invoke(caps::FINDINGS_GET, None)
        .expect("the serialized pipe must wait on the 60s budget, not kill");
    assert_eq!(second["scanned"], true);
    let first_result = worker.join().expect("first invoke thread");
    first_result.expect("in-flight findings.get must complete");
    assert_eq!(session.pid(), pid_before, "backend PID must survive the retry");
}

#[test]
fn killing_the_backend_mid_request_errors_then_respawn_works() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut hung = spec(dir.path());
    hung.hang_ms = Some(3_000);
    let session = PluginSession::spawn(hung).expect("spawn");
    let pid_before = session.pid();

    let id = session
        .write_invoke(caps::WORKSPACE_ROOT, None)
        .expect("write");
    session.kill_process().expect("kill");
    let crashed = session.wait_reply(id, Duration::from_secs(2));
    assert!(
        crashed.is_err(),
        "host must see the kill as a failed request, got {crashed:?}"
    );

    session.respawn().expect("respawn");
    assert_ne!(session.pid(), pid_before, "respawn must be a new process");
    let second = session
        .invoke(caps::WORKSPACE_ROOT, None)
        .expect("invoke after respawn");
    assert_eq!(second["status"], "ok");
    assert_eq!(
        second["root"].as_str().expect("root"),
        dir.path().to_string_lossy().as_ref()
    );
}

#[test]
fn stopping_does_not_wait_for_a_slow_invoke() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut slow = spec(dir.path());
    slow.hang_ms = Some(5_000);
    let session = Arc::new(PluginSession::spawn(slow).expect("spawn"));
    let invoke_session = Arc::clone(&session);
    let invoke = std::thread::spawn(move || invoke_session.invoke(caps::WORKSPACE_ROOT, None));

    std::thread::sleep(Duration::from_millis(100));
    let started = std::time::Instant::now();
    session.kill_process().expect("stop");
    assert!(
        started.elapsed() < Duration::from_millis(500),
        "stop waited for the slow IPC roundtrip: {:?}",
        started.elapsed()
    );
    assert!(invoke.join().expect("invoke thread").is_err());
}

#[test]
fn ungranted_method_never_leaves_the_host() {
    let dir = tempfile::tempdir().expect("tempdir");
    let session = PluginSession::spawn(spec(dir.path())).expect("spawn");
    let error = session
        .invoke(caps::ORACLE_SEARCH, None)
        .expect_err("oracle.search is not granted");
    assert_eq!(
        error.code(),
        devboule_protocol::ErrorCode::CapabilityNotSupported
    );
    // The backend is still alive; a granted method still works.
    session
        .invoke(caps::WORKSPACE_ROOT, None)
        .expect("granted method still works");
}

#[test]
fn dropping_the_session_reaps_the_backend() {
    let dir = tempfile::tempdir().expect("tempdir");
    let session = PluginSession::spawn(spec(dir.path())).expect("spawn");
    session.ping().expect("ping");
    session.kill_process().expect("kill");
    // A reply after the process is gone must not hang the host.
    let error = session
        .invoke(caps::WORKSPACE_ROOT, None)
        .expect_err("dead");
    assert!(
        matches!(
            error,
            devboule_plugin_rpc::PluginError::ProcessExited
                | devboule_plugin_rpc::PluginError::TimedOut(_)
                | devboule_plugin_rpc::PluginError::Io(_)
                | devboule_plugin_rpc::PluginError::Protocol(_)
        ),
        "dead backend must surface an error, got {error}"
    );
}

#[test]
fn handshake_hello_is_the_plugin_tenant() {
    let dir = tempfile::tempdir().expect("tempdir");
    let session = PluginSession::spawn(spec(dir.path())).expect("spawn");
    let hello = session.hello();
    assert_eq!(hello.protocol_version, 1);
    assert!(hello
        .capabilities
        .iter()
        .any(|capability| capability.as_str() == caps::WORKSPACE_ROOT));
    assert!(!hello
        .capabilities
        .iter()
        .any(|capability| capability.as_str() == caps::SESSIONS));
    let _ = DaemonMessage::Hello(hello.clone());
}
