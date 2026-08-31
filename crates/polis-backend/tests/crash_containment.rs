//! A plugin crash degrades its surface and nothing else. Kill the backend
//! mid-request; the host session must error; a respawn must work.

#![cfg(windows)]

use std::collections::BTreeMap;
use std::time::Duration;

use devboule_daemon::{connect_pipe, Framed};
use devboule_plugin_rpc::{host_owner, PluginSession, SpawnSpec, HOST_PID_ENV};
use devboule_protocol::{
    caps, plugin_backend_capabilities, ClientHello, ClientMessage, DaemonMessage,
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
