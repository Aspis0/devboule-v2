//! Tests for the macOS app-variant LocalAPI: discovery fixtures, the
//! basic-auth header, the deadline bounds, the attempt order and the
//! token's silence in errors.

use super::super::{LocalApiClient, ENDPOINT_ENV};
use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::os::unix::fs::PermissionsExt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// The environment this file swaps — `DEVBOULE_TAILSCALE_ENDPOINT` and
/// `PATH` — is process-global: every test that touches either holds this
/// lock for the whole set-and-restore.
static ENV_LOCK: Mutex<()> = Mutex::new(());

fn env_guard() -> MutexGuard<'static, ()> {
    ENV_LOCK.lock().unwrap_or_else(PoisonError::into_inner)
}

fn sample_request() -> Vec<u8> {
    b"GET /localapi/v0/status HTTP/1.1\r\nHost: local-tailscaled.sock\r\nConnection: close\r\n\r\n"
        .to_vec()
}

fn expected_header(token: &str) -> String {
    use base64::Engine;
    format!(
        "Authorization: Basic {}\r\n",
        base64::engine::general_purpose::STANDARD.encode(format!(":{token}"))
    )
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

/// A fake `lsof` on `PATH`: `script` is executed instead of the real one.
/// Returns the directory to prepend and the value to restore.
fn install_fake_lsof(dir: &Path, script_body: &str) {
    let script = dir.join("lsof");
    std::fs::write(&script, script_body).expect("the fake lsof is written");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
        .expect("the fake lsof is executable");
    let previous = std::env::var_os("PATH").unwrap_or_default();
    std::env::set_var(
        "PATH",
        format!("{}:{}", dir.display(), previous.to_string_lossy()),
    );
}

fn restore_env(endpoint: Option<std::ffi::OsString>, path: Option<std::ffi::OsString>) {
    match endpoint {
        Some(value) => std::env::set_var(ENDPOINT_ENV, value),
        None => std::env::remove_var(ENDPOINT_ENV),
    }
    match path {
        Some(value) => std::env::set_var("PATH", value),
        None => std::env::remove_var("PATH"),
    }
}

#[test]
fn lsof_output_records_carry_the_port_and_the_token() {
    let found = parse_lsof_output(
        b"p48221\ncIPNExtension\nf5r\nn/Users/mac/Library/Group Containers/group.ts.tailscale.ipn.macos/sameuserproof-61577-2ae2ec9e0aa2005784f1\n",
    );
    assert_eq!(
        found,
        Some((61577, "2ae2ec9e0aa2005784f1".to_string())),
        "an n field inside an IPNExtension file record carries the port and token"
    );
    assert_eq!(
        parse_lsof_output(b"p1\ncOther\ncmd\nf5\nn/x/sameuserproof-61577-tok\n"),
        None,
        "a record that is not IPNExtension is not our endpoint"
    );
    assert_eq!(
        parse_lsof_output(b"p1\nf5r\n"),
        None,
        "no filename field, no endpoint"
    );
    assert_eq!(
        parse_lsof_output(
            b"p48221\ncIPNExtension\nf5r\nn/tmp/evil\nsameuserproof-9999-deadbeefdeadbeef\n"
        ),
        None,
        "a continuation line from a crafted filename carries no field prefix and is ignored"
    );
    assert_eq!(
        parse_lsof_output(
            b"p48221\ncIPNExtension\nf5r\nn/tmp/real-path\nn/spoof/group.ts.tailscale.ipn.macos/sameuserproof-9999-deadbeef\n"
        ),
        None,
        "a newline-spliced second n line is past its file record and is ignored"
    );
    assert_eq!(
        parse_lsof_output(
            b"p48221\ncIPNExtension\nf5r\nn/x/group.ts.tailscale.ipn.macos/sameuserproof-notaport-abc\n"
        ),
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
    let deadline = Instant::now() + Duration::from_secs(5);
    assert_eq!(
        read_macsys_files(&dir, deadline),
        Some((port, "0123deadbeef0123".to_string()))
    );

    // An empty token file is refused rather than dialed with nothing.
    let empty_dir = crate::test_dirs::test_temp_dir("devboule-app-lapi-empty");
    std::os::unix::fs::symlink(port.to_string(), empty_dir.join("ipnport")).expect("the link");
    std::fs::write(empty_dir.join(format!("sameuserproof-{port}")), "  \n").expect("empty");
    assert_eq!(read_macsys_files(&empty_dir, deadline), None);

    drop(anchor);
    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&empty_dir);
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
    // The assertion text never carries the header itself: a failing CI log
    // must not print even a synthetic credential.
    assert!(
        captured.contains(&expected_header(token)),
        "the fake LocalAPI must receive the basic-auth header"
    );
    let request_line = captured.lines().next().unwrap_or_default();
    assert!(
        request_line.starts_with("GET /localapi/v0/status HTTP/1.1"),
        "the token stays out of the request line: {request_line}"
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

    // A server that accepts and never answers: the timeout refusal is built
    // from the wait itself, and carries no token either.
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

/// One absolute deadline spans discovery too: a wedged `lsof` is killed with
/// the caller's budget instead of holding the daemon's start or a pairing
/// request hostage.
#[test]
fn a_wedged_lsof_fails_within_the_deadline() {
    let _guard = env_guard();
    let dir = crate::test_dirs::test_temp_dir("devboule-fake-lsof");
    let previous_path = std::env::var_os("PATH");
    install_fake_lsof(&dir, "#!/bin/sh\nsleep 30\n");

    let started = Instant::now();
    let deadline = started + Duration::from_millis(500);
    let found = discover(Path::new("/nonexistent-tailscale-dir"), deadline);
    let elapsed = started.elapsed();
    restore_env(None, previous_path);

    assert!(found.is_err(), "a wedged lsof yields no endpoint");
    assert!(
        elapsed < Duration::from_secs(3),
        "the child dies with the deadline, not after its sleep: {elapsed:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// The end-to-end deadline cuts a peer that trickles bytes: every read is
/// re-armed from the one absolute deadline, so a stream that never completes
/// a response cannot stretch the request.
#[test]
fn a_trickling_server_is_cut_at_the_deadline() {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).expect("the trickler binds");
    let port = listener.local_addr().expect("addr").port();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            for _ in 0..100 {
                if stream.write_all(b"x").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    });

    let started = Instant::now();
    let deadline = started + Duration::from_millis(400);
    let result = exchange_tcp(port, &sample_request(), deadline);
    let elapsed = started.elapsed();

    assert!(
        matches!(result, Err(LocalApiError::Timeout)),
        "the trickler is refused at the deadline: {result:?}"
    );
    assert!(
        elapsed < Duration::from_secs(3),
        "bounded by the deadline, not by the trickle: {elapsed:?}"
    );
}

/// The chain contacts the candidates in order: the override first (it
/// answers badly, so the chain moves on), then the app-variant discovery —
/// proven by the fake `lsof`, which can only run after the override was
/// already contacted.
#[test]
fn the_chain_contacts_the_override_before_the_app_discovery() {
    let _guard = env_guard();
    let dir = crate::test_dirs::test_temp_dir("devboule-lapi-order");
    let override_mark = dir.join("override.mark");
    let order_file = dir.join("lsof-order");
    install_fake_lsof(
        &dir,
        &format!(
            "#!/bin/sh\nif [ -f '{}' ]; then echo override-first > '{}'; else echo lsof-first > '{}'; fi\n",
            override_mark.display(),
            order_file.display(),
            order_file.display()
        ),
    );

    let contacts = Arc::new(AtomicUsize::new(0));
    let socket_path = dir.join("override.sock");
    let listener =
        std::os::unix::net::UnixListener::bind(&socket_path).expect("the fake socket binds");
    let serve_contacts = Arc::clone(&contacts);
    let serve_mark = override_mark.clone();
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            serve_contacts.fetch_add(1, Ordering::SeqCst);
            let _ = std::fs::write(&serve_mark, b"1");
            let mut captured = Vec::new();
            let mut chunk = [0u8; 1024];
            while !captured.windows(4).any(|window| window == b"\r\n\r\n") {
                match stream.read(&mut chunk) {
                    Ok(0) => break,
                    Ok(read) => captured.extend_from_slice(&chunk[..read]),
                    Err(_) => break,
                }
            }
            // Not a LocalAPI response, so the chain moves to its next place.
            let _ = stream.write_all(b"HTTP/x\r\n\r\n");
        }
    });

    let previous_endpoint = std::env::var_os(ENDPOINT_ENV);
    let previous_path = std::env::var_os("PATH");
    std::env::set_var(ENDPOINT_ENV, &socket_path);
    let refusal = LocalApiClient::new().get("/localapi/v0/status", true);
    restore_env(previous_endpoint, previous_path);

    assert!(
        refusal.is_err(),
        "a chain whose every place answers badly ends in a refusal"
    );
    assert_eq!(
        contacts.load(Ordering::SeqCst),
        1,
        "the override is contacted exactly once"
    );
    let order = std::fs::read_to_string(&order_file).expect("the fake lsof ran");
    assert_eq!(
        order.trim(),
        "override-first",
        "the app discovery runs only after the override has been contacted"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

/// The refusal after a dead chain names each place that was looked at, in
/// the order they were looked at.
#[test]
fn a_dead_chain_names_every_place_in_order() {
    let _guard = env_guard();
    let dir = crate::test_dirs::test_temp_dir("devboule-lapi-chain");
    let gone_socket = dir.join("gone.sock");
    // A fake `lsof` that finds nothing keeps the outcome the same on a
    // developer Mac where Tailscale really runs.
    install_fake_lsof(&dir, "#!/bin/sh\nexit 1\n");

    let previous_endpoint = std::env::var_os(ENDPOINT_ENV);
    let previous_path = std::env::var_os("PATH");
    std::env::set_var(ENDPOINT_ENV, &gone_socket);
    let refusal = LocalApiClient::new().get("/localapi/v0/status", true);
    restore_env(previous_endpoint, previous_path);

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
