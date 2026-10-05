//! Tests for the macOS app-variant LocalAPI: discovery fixtures, the
//! basic-auth header, the attempt order and the token's silence in errors.

use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::sync::{Mutex, MutexGuard, PoisonError};

/// `DEVBOULE_TAILSCALE_ENDPOINT` is process-global: every test that sets
/// it holds this lock for the whole set-and-restore.
static ENDPOINT_LOCK: Mutex<()> = Mutex::new(());

fn endpoint_guard() -> MutexGuard<'static, ()> {
    ENDPOINT_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

fn sample_request() -> Vec<u8> {
    b"GET /localapi/v0/status HTTP/1.1\r\nHost: local-tailscaled.sock\r\nConnection: close\r\n\r\n"
        .to_vec()
}

/// One loopback HTTP server that answers `200 {}` and hands its request
/// text back to the test.
fn serve_once() -> (u16, mpsc::Receiver<String>) {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("the loopback binds");
    let port = listener.local_addr().expect("addr").port();
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut stream = match listener.accept() {
            Ok((stream, _)) => stream,
            Err(_) => return,
        };
        let mut captured = Vec::new();
        let mut chunk = [0u8; 1024];
        while !captured.windows(4).any(|window| window == b"\r\n\r\n") {
            match stream.read(&mut chunk) {
                Ok(0) => break,
                Ok(read) => captured.extend_from_slice(&chunk[..read]),
                Err(_) => break,
            }
        }
        let _ = sender.send(String::from_utf8_lossy(&captured).into_owned());
        let _ = stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}");
    });
    (port, receiver)
}

fn expected_header(token: &str) -> String {
    use base64::Engine;
    format!(
        "Authorization: Basic {}\r\n",
        base64::engine::general_purpose::STANDARD.encode(format!(":{token}"))
    )
}

#[test]
fn lsof_output_names_the_port_and_the_token() {
    let found = parse_lsof_output(
        b"p48221\nf5r\nn/Users/mac/Library/Group Containers/group.ts.tailscale.ipn.macos/\
             sameuserproof-61577-2ae2ec9e0aa2005784f1\n",
    );
    assert_eq!(
        found,
        Some((61577, "2ae2ec9e0aa2005784f1".to_string())),
        "the marker's port and token are split on the first dash"
    );
    assert_eq!(
        parse_lsof_output(b"p1\nf5r\n"),
        None,
        "no marker, no endpoint"
    );
    assert_eq!(
        parse_lsof_output(b"n/x/group.ts.tailscale.ipn.macos/sameuserproof-notaport-abc\n"),
        None,
        "a marker with a port that is not a port ends the search"
    );
}

#[test]
fn the_ipnport_link_and_its_token_file_are_read() {
    let dir = crate::test_dirs::test_temp_dir("devboule-app-lapi-files");
    let anchor = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("the stale-check target");
    let port = anchor.local_addr().expect("addr").port();
    std::os::unix::fs::symlink(port.to_string(), dir.join("ipnport")).expect("the link");
    std::fs::write(
        dir.join(format!("sameuserproof-{port}")),
        "0123deadbeef0123\n",
    )
    .expect("the token file");
    assert_eq!(
        read_macsys_files(&dir),
        Some((port, "0123deadbeef0123".to_string()))
    );

    // An empty token file is refused rather than dialed with nothing.
    let empty_dir = crate::test_dirs::test_temp_dir("devboule-app-lapi-empty");
    std::os::unix::fs::symlink(port.to_string(), empty_dir.join("ipnport")).expect("the link");
    std::fs::write(empty_dir.join(format!("sameuserproof-{port}")), "  \n").expect("empty");
    assert_eq!(read_macsys_files(&empty_dir), None);

    // The filename spelling: token inside `sameuserproof-<port>-<token>`.
    let named_dir = crate::test_dirs::test_temp_dir("devboule-app-lapi-named");
    std::fs::write(
        named_dir.join(format!("sameuserproof-{port}-cafebabecafebabe")),
        Vec::<u8>::new(),
    )
    .expect("the named file");
    assert_eq!(
        read_filename_token_file(&named_dir),
        Some((port, "cafebabecafebabe".to_string()))
    );

    drop(anchor);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&empty_dir);
    let _ = std::fs::remove_dir_all(&named_dir);
}

#[test]
fn the_token_header_reaches_a_loopback_localapi() {
    let (port, received) = serve_once();
    let token = "f00dfaced00dfaced00dface12";
    let request = with_basic_auth(&sample_request(), token).expect("the header fits");

    let body = exchange_tcp(port, &request, Instant::now() + Duration::from_secs(5))
        .expect("the fake LocalAPI answers");
    assert_eq!(body, b"{}");

    let captured = received
        .recv_timeout(Duration::from_secs(5))
        .expect("the server saw the request");
    assert!(
        captured.contains(&expected_header(token)),
        "the token rides as basic auth: {captured}"
    );
    assert!(
        captured.starts_with("GET /localapi/v0/status HTTP/1.1\r\n"),
        "the token stays out of the request line: {captured}"
    );
}

#[test]
fn no_error_line_carries_the_token() {
    let token = "f00dfaced00dfaced00dface12";
    let request = with_basic_auth(&sample_request(), token).expect("the header fits");
    let deadline = Instant::now() + Duration::from_secs(5);

    // Nothing listening: the refusal names the address, never the token.
    let probe = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("a port to free");
    let quiet_port = probe.local_addr().expect("addr").port();
    drop(probe);
    let refused = exchange_tcp(quiet_port, &request, deadline)
        .expect_err("nothing listens on the freed port");
    assert!(!refused.to_string().contains(token), "{refused}");

    // A server that accepts and never answers: the timeout refusal is
    // built from the wait itself, and carries no token either.
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("the silent server binds");
    let silent_port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        if let Ok((_stream, _)) = listener.accept() {
            std::thread::sleep(Duration::from_secs(2));
        }
    });
    let deadline = Instant::now() + Duration::from_millis(300);
    let silent = exchange_tcp(silent_port, &request, deadline)
        .expect_err("the server never answers within the deadline");
    assert!(!silent.to_string().contains(token), "{silent}");
}

#[test]
fn the_attempt_order_is_override_then_default_socket_then_app() {
    let _guard = endpoint_guard();
    let dir = crate::test_dirs::test_temp_dir("devboule-lapi-chain");
    let live_socket = dir.join("live.sock");
    let gone_socket = dir.join("gone.sock");

    // A live override answers, so the chain stops at its first place.
    let listener =
        std::os::unix::net::UnixListener::bind(&live_socket).expect("the fake socket binds");
    let (sender, received) = mpsc::channel();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let _ = sender.send(());
            let mut captured = Vec::new();
            let mut chunk = [0u8; 1024];
            while !captured.windows(4).any(|window| window == b"\r\n\r\n") {
                match stream.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(read) => captured.extend_from_slice(&chunk[..read]),
                    Err(_) => break,
                }
            }
            let _ = stream.write_all(
                b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\nover",
            );
        }
    });
    let previous = std::env::var_os(super::super::ENDPOINT_ENV);
    std::env::set_var(super::super::ENDPOINT_ENV, &live_socket);
    let answer = super::super::LocalApiClient::new().get("/localapi/v0/status", true);
    match previous {
        Some(value) => std::env::set_var(super::super::ENDPOINT_ENV, value),
        None => std::env::remove_var(super::super::ENDPOINT_ENV),
    }
    assert_eq!(answer.expect("the override answers"), b"over");
    received
        .recv_timeout(Duration::from_secs(5))
        .expect("the override socket served the request");

    // With every place dead the refusal names them in order: the
    // override's path, the default socket, then the app-variant files.
    let previous = std::env::var_os(super::super::ENDPOINT_ENV);
    std::env::set_var(super::super::ENDPOINT_ENV, &gone_socket);
    let refusal = super::super::LocalApiClient::new().get("/localapi/v0/status", true);
    match previous {
        Some(value) => std::env::set_var(super::super::ENDPOINT_ENV, value),
        None => std::env::remove_var(super::super::ENDPOINT_ENV),
    }
    let LocalApiError::Absent(reason) = refusal.expect_err("nothing is reachable") else {
        panic!("a chain that found nothing is an Absent refusal");
    };
    let override_at = reason
        .find(&dir.display().to_string().replace('\\', "/"))
        .expect("the override path is named");
    let socket_at = reason
        .find(super::super::DEFAULT_UNIX_SOCKET)
        .expect("the default socket is named");
    let app_at = reason
        .find("/Library/Tailscale")
        .expect("the app-variant place is named");
    assert!(override_at < socket_at && socket_at < app_at, "{reason}");

    let _ = std::fs::remove_dir_all(&dir);
}
