//! Slice 1 of the endpoint: the bearer gate, the placeholder, and the record
//! the daemon will read. No engine exists yet, so nothing here touches a
//! workspace root.

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use devboule_daemon::{
    current_user_sid, dacl_is_current_user_only, dacl_sddl_for_path, oracle_app_lock_path,
    OracleAppRecord, OracleAppState, RuntimePaths,
};

use crate::oracle::OracleEndpoint;

const QUERY: &str = "/oracle/v1/query";

fn unique_paths() -> (RuntimePaths, DirGuard) {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let dir = std::env::temp_dir().join(format!(
        "devboule oracle endpoint {}-{}",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let guard = DirGuard(dir.clone());
    (RuntimePaths::from_dir(dir), guard)
}

struct DirGuard(PathBuf);

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The record as a fresh endpoint leaves it: live, bound, carrying the port
/// and token a caller needs.
fn published(paths: &RuntimePaths) -> OracleAppRecord {
    match OracleAppState::read(&oracle_app_lock_path(paths)) {
        OracleAppState::Live(record) => record,
        other => panic!("expected a live record right after start, got {other:?}"),
    }
}

/// Send one request and return (status code, full response). The server
/// closes the connection after answering, which ends the read.
fn send(port: u16, method: &str, path: &str, authorization: &str) -> (u16, String) {
    let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .expect("read timeout");
    let body = r#"{"root":"C:\\x","query":"q","limit":10}"#;
    let request = format!(
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1\r\nAuthorization: {authorization}\r\nContent-Length: {}\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes()).expect("write");
    let mut response = String::new();
    stream.read_to_string(&mut response).expect("read");
    let status = response
        .split_whitespace()
        .nth(1)
        .and_then(|token| token.parse::<u16>().ok())
        .unwrap_or_else(|| panic!("no status line in {response:?}"));
    (status, response)
}

#[test]
fn a_wrong_token_is_refused_with_401_before_anything_else() {
    let (paths, _guard) = unique_paths();
    let endpoint = OracleEndpoint::default();
    endpoint.start_at(&paths).expect("start");
    let record = published(&paths);

    let (status, response) = send(record.port, "POST", QUERY, "Bearer wrong-token");
    assert_eq!(status, 401, "{response}");
    assert!(
        response.contains(r#"{"error":"unauthorized"}"#),
        "{response}"
    );

    endpoint.stop();
}

/// The gate runs before the path: even a path that does not exist gets 401
/// from a wrong bearer, never 404.
#[test]
fn a_wrong_token_on_an_unknown_path_is_still_401() {
    let (paths, _guard) = unique_paths();
    let endpoint = OracleEndpoint::default();
    endpoint.start_at(&paths).expect("start");
    let record = published(&paths);

    let (status, response) = send(record.port, "POST", "/oracle/v1/nope", "Bearer wrong-token");
    assert_eq!(status, 401, "{response}");

    endpoint.stop();
}

#[test]
fn an_unknown_path_with_the_right_token_is_404() {
    let (paths, _guard) = unique_paths();
    let endpoint = OracleEndpoint::default();
    endpoint.start_at(&paths).expect("start");
    let record = published(&paths);

    let (status, response) = send(
        record.port,
        "POST",
        "/oracle/v1/nope",
        &format!("Bearer {}", record.token),
    );
    assert_eq!(status, 404, "{response}");
    assert!(response.contains(r#"{"error":"not found"}"#), "{response}");

    endpoint.stop();
}

#[test]
fn a_get_on_the_query_route_with_the_right_token_is_405() {
    let (paths, _guard) = unique_paths();
    let endpoint = OracleEndpoint::default();
    endpoint.start_at(&paths).expect("start");
    let record = published(&paths);

    let (status, response) = send(
        record.port,
        "GET",
        QUERY,
        &format!("Bearer {}", record.token),
    );
    assert_eq!(status, 405, "{response}");
    assert!(
        response.contains(r#"{"error":"method not allowed"}"#),
        "{response}"
    );

    endpoint.stop();
}

#[test]
fn the_token_read_from_the_record_is_admitted_with_not_implemented() {
    let (paths, _guard) = unique_paths();
    let endpoint = OracleEndpoint::default();
    endpoint.start_at(&paths).expect("start");
    let record = published(&paths);

    let (status, response) = send(
        record.port,
        "POST",
        QUERY,
        &format!("Bearer {}", record.token),
    );
    assert_eq!(status, 200, "{response}");
    assert!(
        response.contains(r#"{"ok":false,"reason":"not_implemented"}"#),
        "{response}"
    );

    endpoint.stop();
}

/// The record carries the bearer: its DACL is what keeps that secret on this
/// user. The temp dir has to start out wider, or the mutation that drops the
/// narrowing would pass for the wrong reason.
#[test]
fn the_record_file_is_readable_by_the_current_user_only() {
    let (paths, _guard) = unique_paths();
    let sid = current_user_sid().expect("sid");
    let inherited = dacl_sddl_for_path(&paths.dir).expect("dir dacl");
    assert!(
        !dacl_is_current_user_only(&inherited, &sid),
        "the temp dir must be wider than the record file for this test to mean anything"
    );

    let endpoint = OracleEndpoint::default();
    endpoint.start_at(&paths).expect("start");

    let path = oracle_app_lock_path(&paths);
    let sddl = dacl_sddl_for_path(&path).expect("record dacl");
    assert!(
        dacl_is_current_user_only(&sddl, &sid),
        "start must narrow the record file down to this user: {sddl}"
    );

    endpoint.stop();
}

#[test]
fn the_record_names_the_port_that_answers_and_is_removed_on_stop() {
    let (paths, _guard) = unique_paths();
    let endpoint = OracleEndpoint::default();
    endpoint.start_at(&paths).expect("start");
    let record = published(&paths);
    assert_ne!(record.port, 0, "the record carries the ephemeral port");

    let (status, response) = send(
        record.port,
        "POST",
        QUERY,
        &format!("Bearer {}", record.token),
    );
    assert_eq!(status, 200, "{response}");

    endpoint.stop();
    assert!(
        !oracle_app_lock_path(&paths).exists(),
        "stop must delete the record it published"
    );
}

/// The app exits through `stop()` on the window's thread: a client that
/// opens a connection, owes the rest of its header and never pays must not
/// be waited for.
#[test]
fn stop_returns_while_a_client_still_owes_the_rest_of_its_header() {
    let (paths, _guard) = unique_paths();
    let endpoint = OracleEndpoint::default();
    endpoint.start_at(&paths).expect("start");
    let record = published(&paths);

    let mut stream = TcpStream::connect(("127.0.0.1", record.port)).expect("connect");
    stream.write_all(b"GET ").expect("one fragment of a header");
    thread::sleep(Duration::from_millis(200));

    let started = Instant::now();
    endpoint.stop();
    let elapsed = started.elapsed();
    assert!(
        elapsed < Duration::from_millis(1500),
        "stop waited {elapsed:?} for a client it must not wait for"
    );
    assert!(
        !oracle_app_lock_path(&paths).exists(),
        "stop must delete the record it published"
    );

    drop(stream);
}

/// One byte every second keeps every single read inside its own 2 s timeout:
/// only the total request deadline can close this connection.
#[test]
fn a_client_that_trickles_bytes_past_the_request_deadline_is_closed() {
    let (paths, _guard) = unique_paths();
    let endpoint = OracleEndpoint::default();
    endpoint.start_at(&paths).expect("start");
    let record = published(&paths);

    let mut stream = TcpStream::connect(("127.0.0.1", record.port)).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_millis(300)))
        .expect("read timeout");
    let started = Instant::now();
    let mut byte = [0u8; 1];
    let mut closed_at = None;
    while started.elapsed() < Duration::from_millis(6800) {
        if stream.write_all(b"X").is_err() {
            closed_at = Some(started.elapsed());
            break;
        }
        match stream.read(&mut byte) {
            Ok(0) => {
                closed_at = Some(started.elapsed());
                break;
            }
            Ok(_) => {}
            Err(error)
                if error.kind() == io::ErrorKind::TimedOut
                    || error.kind() == io::ErrorKind::WouldBlock => {}
            Err(_) => {
                closed_at = Some(started.elapsed());
                break;
            }
        }
        thread::sleep(Duration::from_millis(700));
    }
    let closed =
        closed_at.unwrap_or_else(|| panic!("the connection outlived the request deadline"));
    assert!(
        closed <= Duration::from_millis(6500),
        "closed after {closed:?}, past the deadline"
    );

    endpoint.stop();
}
