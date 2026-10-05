//! What the download client promises without the network: an origin policy
//! and a redirect policy, both proved over loopback.

use std::io::Write;

use super::*;

/// A loopback HTTP server with two redirect routes, serving exactly
/// `requests` connections: `/go-good` points at this same loopback server,
/// `/go-bad` at an unreachable host.
fn route_server(requests: usize) -> (String, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let port = listener.local_addr().expect("the port").port();
    let server = std::thread::spawn(move || {
        for _ in 0..requests {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let mut request = [0u8; 4096];
            let read = std::io::Read::read(&mut stream, &mut request).unwrap_or(0);
            let line = String::from_utf8_lossy(&request[..read]);
            let path = line.split_whitespace().nth(1).unwrap_or("/").to_owned();
            let location = if path == "/go-good" {
                format!("http://127.0.0.1:{port}/real.zip")
            } else {
                "http://example.invalid:9/x".to_owned()
            };
            let header = format!(
                "HTTP/1.1 302 Found\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            );
            let _ = stream.write_all(header.as_bytes());
        }
    });
    (format!("http://127.0.0.1:{port}"), server)
}

#[test]
fn only_https_or_loopback_http_is_an_origin() {
    assert!(origin_allowed("https://storage.googleapis.com/chrome-mac-arm64.zip").is_ok());
    assert!(origin_allowed("http://127.0.0.1:8080/chrome-win64.zip").is_ok());
    for off in [
        "http://example.invalid/chrome-win64.zip",
        "http://localhost:8080/chrome-win64.zip",
        "ftp://127.0.0.1/chrome-win64.zip",
    ] {
        assert!(origin_allowed(off).is_err(), "{off} must be refused");
    }
}

#[test]
fn a_plain_http_origin_is_refused_without_a_request() {
    let dir = tempfile::tempdir().expect("a scratch dir");
    let refused = Https.fetch("http://example.invalid/x.zip", &dir.path().join("out.zip"));
    let Err(text) = refused else {
        panic!("plain http must be refused before sending");
    };
    assert!(
        text.contains("refusing non-https"),
        "the refusal must name the policy: {text}"
    );
}

#[test]
fn a_redirect_is_followed_only_on_https() {
    assert!(redirect_is_https(
        "https://storage.googleapis.com/chrome-mac-arm64.zip"
    ));
    for off in [
        "http://127.0.0.1:8080/x.zip",
        "http://example.invalid/x.zip",
        "ftp://example.invalid/x.zip",
    ] {
        assert!(!redirect_is_https(off), "{off} must not be followed");
    }
}

#[test]
fn a_redirect_to_plain_http_is_stopped_with_a_reason() {
    for route in ["/go-bad", "/go-good"] {
        let (base, server) = route_server(1);
        let dir = tempfile::tempdir().expect("a scratch dir");
        let refused = Https.fetch(&format!("{base}{route}"), &dir.path().join("out.zip"));
        let Err(text) = refused else {
            panic!("{route} must stop the download");
        };
        assert!(text.contains("a redirect left https"), "{route}: {text}");
        let _ = server.join();
    }
}
