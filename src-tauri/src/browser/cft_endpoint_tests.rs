//! What the endpoint reader promises without a browser: the marker's port,
//! the loopback address policy, and the HTTP body the debugger announces.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use super::*;

#[test]
fn the_port_is_the_first_line_of_the_marker() {
    let dir = tempfile::tempdir().expect("a profile dir");
    std::fs::write(dir.path().join("DevToolsActivePort"), "9222\ntoken\n").expect("the marker");
    let port = read_devtools_port(dir.path(), Instant::now() + Duration::from_secs(5))
        .expect("the port is read");
    assert_eq!(port, 9222);
}

#[test]
fn a_missing_marker_runs_out_its_wait() {
    let dir = tempfile::tempdir().expect("a profile dir");
    let started = Instant::now();
    let refused = read_devtools_port(dir.path(), started + Duration::from_millis(250));
    assert!(refused.is_err());
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "a missing marker must not hang the launch"
    );
}

#[test]
fn a_zero_port_is_never_a_port() {
    let dir = tempfile::tempdir().expect("a profile dir");
    std::fs::write(
        dir.path().join("DevToolsActivePort"),
        "0\n/devtools/browser/x\n",
    )
    .expect("the marker");
    let started = Instant::now();
    let refused = read_devtools_port(dir.path(), started + Duration::from_millis(250));
    assert!(refused.is_err(), "port 0 is not an endpoint: {refused:?}");
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn only_this_port_on_this_machines_literal_loopback_is_used() {
    assert!(owned_address("ws://127.0.0.1:9222/devtools/browser/x", 9222).is_ok());
    for off in [
        "ws://127.0.0.1:9223/devtools/browser/x",
        "ws://localhost:9222/devtools/browser/x",
        "wss://127.0.0.1:9222/devtools/browser/x",
        "http://127.0.0.1:9222/devtools/browser/x",
        "ws://192.0.2.1:9222/devtools/browser/x",
        "ws://127.0.0.1/devtools/browser/x",
    ] {
        assert!(owned_address(off, 9222).is_err(), "{off} must be refused");
    }
}

#[test]
fn the_body_stops_at_the_announced_length() {
    let body = b"{}";
    let mut answer = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n".to_vec();
    answer.extend_from_slice(body);
    answer.extend_from_slice(b"trailing-bytes-that-must-not-be-needed");
    assert_eq!(body_of(&answer), Some(body.as_slice()));
}

#[test]
fn a_short_body_is_not_a_body_yet() {
    let mut answer = b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\n{}".to_vec();
    assert_eq!(body_of(&answer), None);
    answer.extend_from_slice(b"12345678");
    assert!(body_of(&answer).is_some());
}

/// A loopback stand-in for Chrome's `/json/version`, answering once with the
/// body `make` builds from the port it bound. Reads the whole request
/// before answering: a server that responds to a partial read and drops
/// the socket makes Windows reset the connection while the client is still
/// reading (unread bytes in the buffer turn the close into a RST).
fn version_server(make: impl Fn(u16) -> String) -> (u16, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let port = listener.local_addr().expect("the port").port();
    let body = make(port);
    let server = std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            read_http_request(&mut stream);
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(body.as_bytes());
        }
    });
    (port, server)
}

/// Drain one HTTP request: the client always sends complete headers, and
/// only a fully-read request lets the close land as a FIN.
fn read_http_request(stream: &mut std::net::TcpStream) {
    let mut seen = Vec::new();
    let mut chunk = [0u8; 512];
    while !seen.ends_with(b"\r\n\r\n") {
        match stream.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(read) => seen.extend_from_slice(&chunk[..read]),
        }
    }
}

#[test]
fn the_version_answer_must_be_this_ports_loopback() {
    // Looped: this test caught a Windows-only connection reset from a
    // server that answered a partial read and closed early.
    for _ in 0..30 {
        one_version_round();
    }
}

fn one_version_round() {
    let (port, server) = version_server(|port| {
        format!(r#"{{"webSocketDebuggerUrl":"ws://127.0.0.1:{port}/devtools/browser/x"}}"#)
    });
    assert_eq!(
        browser_ws_url(port).expect("our own address is accepted"),
        format!("ws://127.0.0.1:{port}/devtools/browser/x")
    );
    let _ = server.join();

    let (port, server) = version_server(|port| {
        format!(r#"{{"webSocketDebuggerUrl":"ws://localhost:{port}/devtools/browser/x"}}"#)
    });
    assert!(
        browser_ws_url(port).is_err(),
        "a host name is not our endpoint"
    );
    let _ = server.join();

    let (port, server) = version_server(|port| {
        format!(
            r#"{{"webSocketDebuggerUrl":"ws://127.0.0.1:{}/devtools/browser/x"}}"#,
            port + 1
        )
    });
    assert!(
        browser_ws_url(port).is_err(),
        "another port is not our endpoint"
    );
    let _ = server.join();
}
