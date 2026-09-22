//! The endpoint's transport properties: the bearer gate before everything
//! else, the record the daemon reads, and a token that reaches the query
//! route itself. The route's answers live in [`super::endpoint_query`];
//! the wire helpers live in [`super::host`].

use std::io::{self, Read, Write};
use std::net::TcpStream;
use std::thread;
use std::time::{Duration, Instant};

use devboule_daemon::{
    current_user_sid, dacl_is_current_user_only, dacl_sddl_for_path, oracle_app_lock_path,
};

use super::host::{published, send, unique_paths, TestHost, QUERY_PATH as QUERY};
use super::support::TestEnvironment;
use crate::oracle::OracleEndpoint;

const BODY: &str = r#"{"root":"C:\\x","query":"q","limit":10}"#;

#[test]
fn a_wrong_token_is_refused_with_401_before_anything_else() {
    let (paths, _guard) = unique_paths();
    let endpoint = OracleEndpoint::default();
    endpoint.start_at(&paths, TestHost::bare()).expect("start");
    let record = published(&paths);

    let (status, response) = send(record.port, "POST", QUERY, "Bearer wrong-token", BODY);
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
    endpoint.start_at(&paths, TestHost::bare()).expect("start");
    let record = published(&paths);

    let (status, response) = send(
        record.port,
        "POST",
        "/oracle/v1/nope",
        "Bearer wrong-token",
        BODY,
    );
    assert_eq!(status, 401, "{response}");

    endpoint.stop();
}

#[test]
fn an_unknown_path_with_the_right_token_is_404() {
    let (paths, _guard) = unique_paths();
    let endpoint = OracleEndpoint::default();
    endpoint.start_at(&paths, TestHost::bare()).expect("start");
    let record = published(&paths);

    let (status, response) = send(
        record.port,
        "POST",
        "/oracle/v1/nope",
        &format!("Bearer {}", record.token),
        BODY,
    );
    assert_eq!(status, 404, "{response}");
    assert!(response.contains(r#"{"error":"not found"}"#), "{response}");

    endpoint.stop();
}

#[test]
fn a_get_on_the_query_route_with_the_right_token_is_405() {
    let (paths, _guard) = unique_paths();
    let endpoint = OracleEndpoint::default();
    endpoint.start_at(&paths, TestHost::bare()).expect("start");
    let record = published(&paths);

    let (status, response) = send(
        record.port,
        "GET",
        QUERY,
        &format!("Bearer {}", record.token),
        BODY,
    );
    assert_eq!(status, 405, "{response}");
    assert!(
        response.contains(r#"{"error":"method not allowed"}"#),
        "{response}"
    );

    endpoint.stop();
}

/// The token read from the record reaches the query route itself. The test
/// runtime has no configured workspace, so the route answers its honest
/// `no_app_workspace` envelope — never the gate's 401/404/405, never a
/// placeholder.
#[test]
fn the_token_read_from_the_record_reaches_the_query_route() {
    let _env = TestEnvironment::new("candle");
    let (paths, _guard) = unique_paths();
    let endpoint = OracleEndpoint::default();
    endpoint.start_at(&paths, TestHost::bare()).expect("start");
    let record = published(&paths);

    let (status, response) = send(
        record.port,
        "POST",
        QUERY,
        &format!("Bearer {}", record.token),
        BODY,
    );
    assert_eq!(status, 200, "{response}");
    assert!(
        response.contains(r#""reason":"no_app_workspace""#),
        "{response}"
    );
    assert!(
        response.contains("Oracle embedding is unavailable until you choose an existing workspace"),
        "{response}"
    );
    assert!(!response.contains(r#""reason":"no_model""#), "{response}");
    assert!(!response.contains("not_implemented"), "{response}");

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
    endpoint.start_at(&paths, TestHost::bare()).expect("start");

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
    endpoint.start_at(&paths, TestHost::bare()).expect("start");
    let record = published(&paths);
    assert_ne!(record.port, 0, "the record carries the ephemeral port");

    let (status, response) = send(
        record.port,
        "POST",
        QUERY,
        &format!("Bearer {}", record.token),
        BODY,
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
    endpoint.start_at(&paths, TestHost::bare()).expect("start");
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
    endpoint.start_at(&paths, TestHost::bare()).expect("start");
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
