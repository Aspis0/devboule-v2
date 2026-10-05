//! Unix spawn-level coverage for the plugin channel: handshake over the
//! inherited socketpair, a child that dies before it, and the process group
//! a dropped session must leave empty.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use devboule_plugin_rpc::{host_owner, PluginSession, SpawnSpec};
use devboule_protocol::{caps, Capability, DEFAULT_PLUGIN_PAYLOAD_BYTES};

const TEST_CHILD: &str = env!("CARGO_BIN_EXE_devboule-plugin-test-child");

fn spec(plugin_id: &str) -> SpawnSpec {
    SpawnSpec {
        binary: PathBuf::from(TEST_CHILD),
        plugin_id: plugin_id.to_string(),
        capabilities: vec![Capability::new(caps::PING)],
        grants: BTreeMap::new(),
        owner: host_owner().expect("host owner"),
        max_payload_bytes: DEFAULT_PLUGIN_PAYLOAD_BYTES,
        hang_ms: None,
    }
}

fn process_gone(pid: u32) -> bool {
    let result = unsafe { libc::kill(pid as libc::pid_t, 0) };
    result != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

fn wait_until_gone(pid: u32, budget: Duration) -> bool {
    let deadline = Instant::now() + budget;
    loop {
        if process_gone(pid) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn wait_for_pid_file(path: &Path) -> Option<u32> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(text) = std::fs::read_to_string(path) {
            if let Ok(pid) = text.trim().parse::<u32>() {
                return Some(pid);
            }
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn grandchild_pid_file() -> PathBuf {
    std::env::temp_dir().join(format!("devboule-plugin-orphan-{}", std::process::id()))
}

#[test]
fn a_spawned_child_handshakes_round_trips_and_dies_on_kill() {
    let session = PluginSession::spawn(spec("roundtrip")).expect("spawn and handshake");
    let value = session
        .invoke(caps::PING, Some(serde_json::json!({ "probe": 7 })))
        .expect("round trip");
    assert_eq!(value, serde_json::json!({ "probe": 7 }));

    let pid = session.pid();
    session.kill_process().expect("kill");
    assert!(
        wait_until_gone(pid, Duration::from_secs(5)),
        "child {pid} must die with its process group"
    );
}

#[test]
fn a_child_that_exits_before_the_handshake_is_reported() {
    let error = PluginSession::spawn(spec("quitter"))
        .err()
        .expect("spawn must fail");
    let text = error.to_string();
    assert!(text.contains("exited"), "unexpected failure: {text}");
}

#[test]
fn dropping_a_session_leaves_no_grandchild_behind() {
    let pid_file = grandchild_pid_file();
    let _ = std::fs::remove_file(&pid_file);

    let session = PluginSession::spawn(spec("forker")).expect("spawn and handshake");
    let grandchild = wait_for_pid_file(&pid_file).expect("grandchild pid file");
    assert!(
        !process_gone(grandchild),
        "grandchild {grandchild} must be alive before the drop"
    );

    let child = session.pid();
    drop(session);
    assert!(
        wait_until_gone(child, Duration::from_secs(5)),
        "child {child} must be reaped"
    );
    assert!(
        wait_until_gone(grandchild, Duration::from_secs(10)),
        "grandchild {grandchild} must not outlive its group"
    );
    let _ = std::fs::remove_file(&pid_file);
}
