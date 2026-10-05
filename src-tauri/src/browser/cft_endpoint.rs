//! The debugger endpoint Chrome for Testing publishes on loopback.
//!
//! One phrase: find the browser's port in its profile and accept only the
//! websocket address that is this machine's own literal loopback on that
//! exact port. Where the bytes go and which process answers is the process
//! manager's business; this module only reads what Chrome wrote and refuses
//! anything else.

#![cfg_attr(not(test), allow(dead_code))]

use std::io::{Read, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use url::{Host, Url};

/// The port the browser wrote into its profile. Port 0 is the flag the caller
/// passed, never a port anything listens on, so it is not a port.
pub(super) fn read_devtools_port(profile: &Path, deadline: Instant) -> std::io::Result<u16> {
    let marker = profile.join("DevToolsActivePort");
    while Instant::now() < deadline {
        if let Ok(text) = std::fs::read_to_string(&marker) {
            if let Some(port) = text
                .lines()
                .next()
                .and_then(|line| line.trim().parse::<u16>().ok())
                .filter(|port| *port != 0)
            {
                return Ok(port);
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    Err(std::io::Error::other(
        "no DevToolsActivePort in the profile in time",
    ))
}

/// Whether `address` is this machine's own debugger on the exact port the
/// profile named. A host name (`localhost`) is refused: the port the profile
/// named belongs to a literal address, and only that literal is spoken to.
fn owned_address(address: &str, port: u16) -> Result<(), String> {
    let refused = || format!("{address} is not ws://127.0.0.1:{port}/...");
    let Ok(parsed) = Url::parse(address) else {
        return Err(refused());
    };
    let ours = parsed.scheme() == "ws"
        && matches!(parsed.host(), Some(Host::Ipv4(host)) if host == std::net::Ipv4Addr::LOCALHOST)
        && parsed.port() == Some(port);
    if ours {
        Ok(())
    } else {
        Err(refused())
    }
}

/// The browser-level debugger address, published at `/json/version`.
pub(super) fn browser_ws_url(port: u16) -> std::io::Result<String> {
    let version: Value =
        serde_json::from_slice(&http_get(port, "/json/version")?).map_err(std::io::Error::other)?;
    let address = version["webSocketDebuggerUrl"]
        .as_str()
        .ok_or_else(|| std::io::Error::other("the version list named no debugger address"))?;
    owned_address(address, port).map_err(std::io::Error::other)?;
    Ok(address.to_owned())
}

/// One page target's own debugger address, published at `/json/list`.
pub(super) fn page_ws_url(port: u16, target_id: &str) -> std::io::Result<String> {
    let targets: Value =
        serde_json::from_slice(&http_get(port, "/json/list")?).map_err(std::io::Error::other)?;
    let address = targets
        .as_array()
        .and_then(|targets| {
            targets.iter().find_map(|target| {
                (target["type"] == json!("page") && target["id"] == json!(target_id))
                    .then(|| target["webSocketDebuggerUrl"].as_str())
                    .flatten()
            })
        })
        .ok_or_else(|| std::io::Error::other("the target list named no page address"))?;
    owned_address(address, port).map_err(std::io::Error::other)?;
    Ok(address.to_owned())
}

/// GET `path` from the loopback debugger. The server keeps the connection open
/// past its answer, so the read stops at the announced length rather than
/// sitting out a timeout for an EOF (measured in slice 1).
fn http_get(port: u16, path: &str) -> std::io::Result<Vec<u8>> {
    let mut socket = std::net::TcpStream::connect(("127.0.0.1", port))?;
    socket.set_read_timeout(Some(Duration::from_secs(10)))?;
    write!(
        socket,
        "GET {path} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\n\r\n"
    )?;
    let mut answer = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        let read = socket.read(&mut chunk)?;
        if read == 0 {
            break;
        }
        answer.extend_from_slice(&chunk[..read]);
        if let Some(body) = body_of(&answer) {
            return Ok(body.to_vec());
        }
    }
    Err(std::io::Error::other(
        "the debugger answer ended before its whole body arrived",
    ))
}

fn body_of(answer: &[u8]) -> Option<&[u8]> {
    let text = std::str::from_utf8(answer).ok()?;
    let (headers, body) = text.split_once("\r\n\r\n")?;
    let announced = headers
        .lines()
        .find_map(|line| line.strip_prefix("Content-Length:"))
        .and_then(|length| length.trim().parse::<usize>().ok())?;
    (body.len() >= announced).then(|| &body.as_bytes()[..announced])
}

#[cfg(test)]
#[path = "cft_endpoint_tests.rs"]
mod tests;
