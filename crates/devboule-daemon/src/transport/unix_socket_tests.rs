//! Unix transport tests: every bind/connect/identity behavior above, each
//! failing without the code it covers. Unix only; the module they test
//! does not compile on Windows.

use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::{atomic::AtomicBool, Arc};
use std::time::Duration;

use super::unix_socket::{connect, connect_within, peer_identity, peer_is_current, UnixListener};
use super::Listener;
use crate::paths::RuntimePaths;

fn unique_paths() -> RuntimePaths {
    let dir = crate::test_dirs::test_temp_dir("ux");
    RuntimePaths::from_dir(dir)
}

fn stop_flag() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

fn file_mode(path: &Path) -> u32 {
    std::fs::metadata(path)
        .expect("metadata")
        .permissions()
        .mode()
        & 0o777
}

#[test]
fn round_trip_moves_bytes_both_ways() {
    let paths = unique_paths();
    let listener = UnixListener::bind(&paths, stop_flag()).expect("bind");
    let mut listener = UnixListener::bind(&paths, stop_flag()).expect("bind");
    let mut client = connect(&paths).expect("connect");
    let mut server = Listener::accept(&mut listener).expect("accept");

    client.write_all(b"ping").expect("client write");
    let mut buf = [0u8; 4];
    server.read_exact(&mut buf).expect("server read");
    assert_eq!(&buf, b"ping");

    server.write_all(b"pong").expect("server write");
    client.read_exact(&mut buf).expect("client read");
    assert_eq!(&buf, b"pong");
}

#[test]
fn socket_dir_is_private_and_socket_is_owner_only() {
    let paths = unique_paths();
    let _listener = UnixListener::bind(&paths, stop_flag()).expect("bind");
    assert_eq!(file_mode(&paths.dir), 0o700, "runtime dir");
    assert_eq!(file_mode(&paths.socket_path), 0o600, "socket");
}

#[test]
fn too_long_path_is_refused_before_touching_the_fs() {
    let dir = crate::test_dirs::test_temp_dir("ux");
    let mut paths = RuntimePaths::from_dir(&dir);
    paths.socket_path = dir.join("a".repeat(200) + ".sock");
    let error = UnixListener::bind(&paths, stop_flag()).expect_err("too long");
    assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
    assert!(
        error.to_string().contains("104"),
        "names the byte limit: {error}"
    );
    assert!(
        !paths.socket_path.exists(),
        "refused before creating anything"
    );
}

#[test]
fn stale_socket_file_is_unlinked() {
    let paths = unique_paths();
    paths.ensure_dir().expect("runtime dir");
    {
        let lingering =
            std::os::unix::net::UnixListener::bind(&paths.socket_path).expect("stale fixture");
        drop(lingering);
    }
    assert!(paths.socket_path.exists(), "fixture left the file");
    let _listener = UnixListener::bind(&paths, stop_flag()).expect("stale unlinked");
}

#[test]
fn live_socket_refuses_a_second_bind() {
    let paths = unique_paths();
    let _first = UnixListener::bind(&paths, stop_flag()).expect("first bind");
    let error = UnixListener::bind(&paths, stop_flag()).expect_err("second bind");
    assert_eq!(error.kind(), io::ErrorKind::AddrInUse);
    assert!(
        error.to_string().contains("already running"),
        "says so: {error}"
    );
}

#[test]
fn shutdown_removes_the_owned_socket() {
    let paths = unique_paths();
    let mut listener = UnixListener::bind(&paths, stop_flag()).expect("bind");
    assert!(paths.socket_path.exists());
    Listener::shutdown(&mut listener).expect("shutdown");
    assert!(!paths.socket_path.exists(), "owned socket removed");
}

#[test]
fn peer_uid_is_read_and_same_uid_is_accepted() {
    let paths = unique_paths();
    let mut listener = UnixListener::bind(&paths, stop_flag()).expect("bind");
    let _client = connect(&paths).expect("connect");
    let server = Listener::accept(&mut listener).expect("same uid accepted");
    let peer = peer_identity(&server).expect("peer identity");
    // SAFETY: getuid takes no arguments and cannot fail.
    let me = unsafe { libc::getuid() }.to_string();
    assert_eq!(peer.user, me, "kernel uid of the peer");
    assert_eq!(
        peer.pid,
        std::process::id(),
        "the client is this test process"
    );
}

#[test]
fn different_uid_is_refused() {
    let foreign = crate::agent_report::PeerIdentity {
        user: "4294967294".to_string(),
        pid: 1,
    };
    assert!(!peer_is_current(&foreign));
    let me = crate::agent_report::PeerIdentity {
        // SAFETY: getuid takes no arguments and cannot fail.
        user: unsafe { libc::getuid() }.to_string(),
        pid: std::process::id(),
    };
    assert!(peer_is_current(&me));
}

#[test]
fn connect_within_waits_for_a_late_listener() {
    let paths = unique_paths();
    let late = paths.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(150));
        let _listener = UnixListener::bind(&late, stop_flag()).expect("late bind");
        std::thread::sleep(Duration::from_secs(5));
    });
    connect_within(&paths, Duration::from_secs(10)).expect("waited out the late bind");
}

#[test]
fn connect_to_nothing_is_not_found_or_refused() {
    let paths = unique_paths();
    paths.ensure_dir().expect("runtime dir");
    let error = connect(&paths).expect_err("nobody listening");
    assert!(
        matches!(
            error.kind(),
            io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
        ),
        "names the absence: {error}"
    );
}
