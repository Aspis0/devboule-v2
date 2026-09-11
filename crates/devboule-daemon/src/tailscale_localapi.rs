//! Tailscale LocalAPI client: `whois` and `status`.
//!
//! The daemon is std threads and blocking I/O with no tokio, so this is a
//! hand-written, bounded HTTP/1.1 GET over the platform's local transport
//! (`DESIGN-remote-agents.md` §9). On Windows that transport is the
//! tailscaled named pipe, opened **without** SQOS flags: the default
//! impersonation level (`SecurityImpersonation`) is what the IPN server
//! requires, and an explicit `Identification` makes it answer 401 "Unable to
//! impersonate" (measured, §9).
//!
//! The transport is opened `FILE_FLAG_OVERLAPPED` so the 2 s budget is a
//! wall-clock deadline on every read and write, reusing the same overlapped
//! pattern as [`crate::framing`]. It is not `File::open`, which cannot set
//! `FILE_FLAG_OVERLAPPED`; the impersonation level is the same either way.
//!
//! Nothing here logs the response. `LoginName` and `DisplayName` are PII and
//! any line that needs them goes through [`crate::device_identity::redact`].
//!
//! Wired by S5 (the peer listener's binding check) and S6 (pairing). Until
//! then the module has no non-test caller; remove this allowance in the step
//! that wires it.
#![allow(dead_code)]

#[cfg(not(windows))]
use std::io::{Read, Write};
use std::net::{IpAddr, SocketAddr};
use std::str::FromStr;
use std::time::{Duration, Instant};

use serde::Deserialize;

/// The tailscaled IPN pipe. `ProtectedPrefix\Administrators` is where the
/// LocalAPI actually listens on Windows; a non-elevated Medium-IL process can
/// open it (measured, §9).
pub const LOCALAPI_PIPE: &str =
    "\\\\.\\pipe\\ProtectedPrefix\\Administrators\\Tailscale\\tailscaled";
/// Standalone tailscaled on non-Windows.
#[cfg(not(windows))]
pub const DEFAULT_UNIX_SOCKET: &str = "/var/run/tailscaled.socket";
/// Body cap. A `/status` for a large tailnet is tens of KiB; 64 KiB is far
/// above that and far below anything that could be called unbounded.
pub const BODY_CAP: usize = 64 * 1024;
/// Header cap. Separate from the body cap so a header flood is refused before
/// the body budget is even reached.
pub const HEADER_CAP: usize = 8 * 1024;
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(2);
/// Override the local socket path (non-Windows) or the pipe name (Windows).
pub const ENDPOINT_ENV: &str = "DEVBOULE_TAILSCALE_ENDPOINT";

#[derive(Debug)]
pub enum LocalApiError {
    /// This platform's transport is not built yet (macOS App Store sandboxed
    /// tailscaled needs a loopback port + same-user token: slice 6).
    #[cfg(not(windows))]
    Unsupported(String),
    /// Tailscale is not running (the pipe or socket is absent).
    Absent(String),
    Transport(String),
    Timeout,
    /// A non-200 status line.
    Status(u16),
    /// The response was not the HTTP/1.1 shape this client sends.
    Protocol(String),
    Parse(String),
}

impl std::fmt::Display for LocalApiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            #[cfg(not(windows))]
            Self::Unsupported(message) => {
                write!(formatter, "tailscale localapi unsupported: {message}")
            }
            Self::Absent(message) => write!(formatter, "tailscale is not running: {message}"),
            Self::Transport(message) => {
                write!(formatter, "tailscale localapi transport: {message}")
            }
            Self::Timeout => write!(formatter, "tailscale localapi timed out"),
            Self::Status(code) => write!(formatter, "tailscale localapi answered HTTP {code}"),
            Self::Protocol(message) => write!(formatter, "tailscale localapi protocol: {message}"),
            Self::Parse(message) => write!(formatter, "tailscale localapi json: {message}"),
        }
    }
}

impl std::error::Error for LocalApiError {}

/// What `whois` proves about a connection.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WhoIs {
    pub stable_id: String,
    pub node_name: String,
    pub login_name: String,
    pub user_id: String,
    pub addresses: Vec<IpAddr>,
    /// The response carried a non-null `CapMap` (grants configured). `false`
    /// on this machine: `CapMap` is null (§9).
    pub cap_map_present: bool,
}

/// This node's own tailnet identity. Never used to bind a *peer*: it
/// describes the local device only (`DESIGN-remote-agents.md` §8b, muse M2).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelfNode {
    pub stable_id: String,
    pub node_name: String,
    pub addresses: Vec<IpAddr>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Endpoint {
    #[cfg(windows)]
    Pipe(String),
    #[cfg(not(windows))]
    UnixSocket(std::path::PathBuf),
}

pub struct LocalApiClient {
    endpoint: Endpoint,
    timeout: Duration,
}

impl Default for LocalApiClient {
    fn default() -> Self {
        Self::new()
    }
}

impl LocalApiClient {
    pub fn new() -> Self {
        Self::with_endpoint(default_endpoint(), DEFAULT_TIMEOUT)
    }

    pub fn with_timeout(timeout: Duration) -> Self {
        Self::with_endpoint(default_endpoint(), timeout)
    }

    /// Point the client at a non-default endpoint. The in-process test server
    /// uses this; a non-default pipe can also be selected with
    /// `DEVBOULE_TAILSCALE_ENDPOINT`.
    pub fn with_endpoint(endpoint: Endpoint, timeout: Duration) -> Self {
        Self { endpoint, timeout }
    }

    #[cfg(windows)]
    pub fn with_pipe_name(pipe_name: impl Into<String>, timeout: Duration) -> Self {
        Self {
            endpoint: Endpoint::Pipe(pipe_name.into()),
            timeout,
        }
    }

    /// `whois(addr)`: which tailnet node and login own this address.
    pub fn whois(&self, addr: SocketAddr) -> Result<WhoIs, LocalApiError> {
        let body = self.get(&format!("/localapi/v0/whois?addr={addr}"))?;
        parse_whois(&body)
    }

    /// This node's own identity and tailnet addresses.
    pub fn self_node(&self) -> Result<SelfNode, LocalApiError> {
        let body = self.get("/localapi/v0/status")?;
        parse_self_node(&body)
    }

    fn get(&self, path_and_query: &str) -> Result<Vec<u8>, LocalApiError> {
        let request = format!(
            "GET {path_and_query} HTTP/1.1\r\nHost: local-tailscaled.sock\r\nConnection: close\r\n\r\n"
        );
        let deadline = Instant::now() + self.timeout;
        let raw = match &self.endpoint {
            #[cfg(windows)]
            Endpoint::Pipe(pipe_name) => {
                let file = open_pipe(pipe_name, deadline)?;
                crate::framing::write_all_overlapped(&file, request.as_bytes(), Some(deadline))
                    .map_err(|error| map_io(error, "writing the request"))?;
                read_overlapped(&file, deadline)?
            }
            #[cfg(not(windows))]
            Endpoint::UnixSocket(path) => read_unix_socket(path, request.as_bytes(), deadline)?,
        };
        let (status, body) = parse_http_response(&raw)?;
        if status != 200 {
            return Err(LocalApiError::Status(status));
        }
        Ok(body)
    }
}

// ---------------------------------------------------------------------------
// Windows transport
// ---------------------------------------------------------------------------

#[cfg(windows)]
fn default_endpoint() -> Endpoint {
    match std::env::var(ENDPOINT_ENV) {
        Ok(value) if !value.is_empty() => Endpoint::Pipe(value),
        _ => Endpoint::Pipe(LOCALAPI_PIPE.to_string()),
    }
}

#[cfg(not(windows))]
fn default_endpoint() -> Endpoint {
    match std::env::var(ENDPOINT_ENV) {
        Ok(value) if !value.is_empty() => Endpoint::UnixSocket(value.into()),
        _ => Endpoint::UnixSocket(DEFAULT_UNIX_SOCKET.into()),
    }
}

#[cfg(windows)]
fn open_pipe(pipe_name: &str, deadline: Instant) -> Result<std::fs::File, LocalApiError> {
    use std::os::windows::io::{FromRawHandle, RawHandle};
    use windows_sys::Win32::Foundation::{
        GetLastError, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, GENERIC_READ, GENERIC_WRITE,
        INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_OVERLAPPED, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Pipes::WaitNamedPipeW;

    let name = crate::security::wide(pipe_name);
    // A missing pipe means Tailscale is not running, and a busy pipe means its
    // instance count is exhausted for this instant. Both are retried inside
    // the caller's budget: a tailscaled still creating its pipe would
    // otherwise look absent for one scheduling quantum.
    loop {
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL | FILE_FLAG_OVERLAPPED,
                std::ptr::null_mut(),
            )
        };
        if handle != INVALID_HANDLE_VALUE {
            // SAFETY: CreateFileW returned a new owned handle.
            return Ok(unsafe { std::fs::File::from_raw_handle(handle as RawHandle) });
        }
        let error = unsafe { GetLastError() };
        let retryable = error == ERROR_PIPE_BUSY || error == ERROR_FILE_NOT_FOUND;
        if retryable && Instant::now() < deadline {
            if error == ERROR_PIPE_BUSY {
                unsafe {
                    WaitNamedPipeW(name.as_ptr(), 100);
                }
            } else {
                std::thread::sleep(Duration::from_millis(20));
            }
            continue;
        }
        if error == ERROR_FILE_NOT_FOUND {
            return Err(LocalApiError::Absent(format!("{pipe_name} does not exist")));
        }
        return Err(LocalApiError::Transport(format!(
            "CreateFileW({pipe_name}) failed with os error {error}"
        )));
    }
}

#[cfg(windows)]
fn read_overlapped(file: &std::fs::File, deadline: Instant) -> Result<Vec<u8>, LocalApiError> {
    let mut raw = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match crate::framing::read_chunk(file, &mut chunk, Some(deadline)) {
            Ok(Some(0)) => break,
            Ok(Some(read)) => raw.extend_from_slice(&chunk[..read]),
            // `None` is read_chunk's deadline signal.
            Ok(None) => return Err(LocalApiError::Timeout),
            Err(error) => return Err(map_io(error, "reading the response")),
        }
        if raw.len() > HEADER_CAP + BODY_CAP {
            return Err(LocalApiError::Protocol(
                "response exceeds the 64 KiB body cap".to_string(),
            ));
        }
        if response_is_complete(&raw)? {
            return Ok(raw);
        }
    }
    Ok(raw)
}

// ---------------------------------------------------------------------------
// Non-Windows transport
// ---------------------------------------------------------------------------

#[cfg(not(windows))]
fn read_unix_socket(
    path: &std::path::Path,
    request: &[u8],
    deadline: Instant,
) -> Result<Vec<u8>, LocalApiError> {
    use std::os::unix::net::UnixStream;

    let stream = UnixStream::connect(path).map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            LocalApiError::Absent(format!("{} does not exist", path.display()))
        } else {
            LocalApiError::Transport(error.to_string())
        }
    })?;
    let remaining = deadline.saturating_duration_since(Instant::now());
    if remaining.is_zero() {
        return Err(LocalApiError::Timeout);
    }
    let _ = stream.set_read_timeout(Some(remaining));
    let _ = stream.set_write_timeout(Some(remaining));
    let mut stream = stream;
    stream
        .write_all(request)
        .map_err(|error| map_io(error, "writing the request"))?;
    let mut raw = Vec::new();
    let mut chunk = [0u8; 8192];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => raw.extend_from_slice(&chunk[..read]),
            Err(error) => return Err(map_io(error, "reading the response")),
        }
        if raw.len() > HEADER_CAP + BODY_CAP {
            return Err(LocalApiError::Protocol(
                "response exceeds the 64 KiB body cap".to_string(),
            ));
        }
        if response_is_complete(&raw)? {
            return Ok(raw);
        }
    }
    Ok(raw)
}

#[cfg(not(windows))]
#[allow(dead_code)]
fn macos_sandbox_is_a_later_slice() -> LocalApiError {
    LocalApiError::Unsupported(
        "the macOS App Store tailscaled transport (loopback port plus a same-user token) \
         is slice 6"
            .to_string(),
    )
}

fn map_io(error: std::io::Error, step: &str) -> LocalApiError {
    match error.kind() {
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock => LocalApiError::Timeout,
        _ => LocalApiError::Transport(format!("{step}: {error}")),
    }
}

// ---------------------------------------------------------------------------
// HTTP/1.1 response parsing
// ---------------------------------------------------------------------------

/// Split and validate one bounded HTTP/1.1 response. Fail closed: a missing
/// `Content-Length`, a `Transfer-Encoding`, or an oversized body is an error,
/// never a guess.
fn parse_http_response(raw: &[u8]) -> Result<(u16, Vec<u8>), LocalApiError> {
    let (status, body) = split_response(raw)?;
    Ok((status, body.to_vec()))
}

fn split_response(raw: &[u8]) -> Result<(u16, &[u8]), LocalApiError> {
    let header_end = find_header_end(raw).ok_or_else(|| {
        LocalApiError::Protocol("response headers did not end within 8 KiB".to_string())
    })?;
    let (status, length) = parse_headers(&raw[..header_end])?;
    let body_start = header_end + 4;
    let body = raw.get(body_start..body_start + length).ok_or_else(|| {
        LocalApiError::Protocol("response body is shorter than its length".to_string())
    })?;
    Ok((status, body))
}

/// Returns the status code and the declared body length. Fails closed on a
/// `Transfer-Encoding`, a missing or oversized `Content-Length`.
fn parse_headers(header_bytes: &[u8]) -> Result<(u16, usize), LocalApiError> {
    let headers = std::str::from_utf8(header_bytes)
        .map_err(|_| LocalApiError::Protocol("response headers are not UTF-8".to_string()))?;
    let mut lines = headers.split("\r\n");
    let status_line = lines
        .next()
        .ok_or_else(|| LocalApiError::Protocol("empty response".to_string()))?;
    let status = parse_status_line(status_line)?;
    let mut content_length = None;
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let (name, value) = line
            .split_once(':')
            .ok_or_else(|| LocalApiError::Protocol(format!("malformed header {line:?}")))?;
        let name = name.trim().to_ascii_lowercase();
        let value = value.trim();
        if name == "transfer-encoding" {
            return Err(LocalApiError::Protocol(
                "chunked responses are refused".to_string(),
            ));
        }
        if name == "content-length" {
            let parsed = value.parse::<usize>().map_err(|_| {
                LocalApiError::Protocol(format!("invalid Content-Length {value:?}"))
            })?;
            if parsed > BODY_CAP {
                return Err(LocalApiError::Protocol(format!(
                    "Content-Length {parsed} exceeds the 64 KiB body cap"
                )));
            }
            content_length = Some(parsed);
        }
    }
    let length = content_length
        .ok_or_else(|| LocalApiError::Protocol("response has no Content-Length".to_string()))?;
    Ok((status, length))
}

/// Whether `raw` already holds the whole response, so the read loop can stop
/// without waiting for EOF.
fn response_is_complete(raw: &[u8]) -> Result<bool, LocalApiError> {
    let Some(header_end) = find_header_end(raw) else {
        if raw.len() > HEADER_CAP {
            return Err(LocalApiError::Protocol(
                "response headers exceed 8 KiB".to_string(),
            ));
        }
        return Ok(false);
    };
    let (_, length) = parse_headers(&raw[..header_end])?;
    Ok(raw.len() >= header_end + 4 + length)
}

fn find_header_end(raw: &[u8]) -> Option<usize> {
    raw.windows(4).position(|window| window == b"\r\n\r\n")
}

fn parse_status_line(line: &str) -> Result<u16, LocalApiError> {
    let mut parts = line.split(' ');
    let version = parts.next().unwrap_or_default();
    if !version.starts_with("HTTP/1.") {
        return Err(LocalApiError::Protocol(format!(
            "unexpected status line {line:?}"
        )));
    }
    let code = parts
        .next()
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| LocalApiError::Protocol(format!("unexpected status line {line:?}")))?;
    Ok(code)
}

// ---------------------------------------------------------------------------
// JSON parsing
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct WhoIsNode {
    #[serde(rename = "StableID", default)]
    stable_id: Option<String>,
    #[serde(rename = "Name", default)]
    name: Option<String>,
    #[serde(rename = "DNSName", default)]
    dns_name: Option<String>,
    #[serde(rename = "Addresses", default)]
    addresses: Vec<String>,
    #[serde(rename = "TailscaleIPs", default)]
    tailscale_ips: Vec<String>,
}

#[derive(Deserialize)]
struct WhoIsUser {
    #[serde(rename = "ID", default)]
    id: Option<serde_json::Value>,
    #[serde(rename = "LoginName", default)]
    login_name: Option<String>,
}

#[derive(Deserialize)]
struct WhoIsResponse {
    #[serde(rename = "Node", default)]
    node: Option<WhoIsNode>,
    #[serde(rename = "UserProfile", default)]
    user_profile: Option<WhoIsUser>,
    #[serde(rename = "CapMap", default)]
    cap_map: serde_json::Value,
}

#[derive(Deserialize)]
struct StatusNode {
    #[serde(rename = "StableID", default)]
    stable_id: Option<String>,
    #[serde(rename = "Name", default)]
    name: Option<String>,
    #[serde(rename = "DNSName", default)]
    dns_name: Option<String>,
    #[serde(rename = "Addresses", default)]
    addresses: Vec<String>,
    #[serde(rename = "TailscaleIPs", default)]
    tailscale_ips: Vec<String>,
}

#[derive(Deserialize)]
struct StatusResponse {
    #[serde(rename = "Self", default)]
    self_node: Option<StatusNode>,
}

pub fn parse_whois(body: &[u8]) -> Result<WhoIs, LocalApiError> {
    let response: WhoIsResponse =
        serde_json::from_slice(body).map_err(|error| LocalApiError::Parse(error.to_string()))?;
    let node = response
        .node
        .ok_or_else(|| LocalApiError::Parse("whois response has no Node".to_string()))?;
    let stable_id = node
        .stable_id
        .filter(|value| !value.is_empty())
        .ok_or_else(|| LocalApiError::Parse("whois Node has no StableID".to_string()))?;
    let user = response.user_profile.unwrap_or(WhoIsUser {
        id: None,
        login_name: None,
    });
    Ok(WhoIs {
        stable_id,
        node_name: string_or_empty(node.name.clone().or(node.dns_name)),
        login_name: string_or_empty(user.login_name),
        user_id: json_id(user.id),
        addresses: parse_addresses(&node.addresses, &node.tailscale_ips),
        cap_map_present: !response.cap_map.is_null(),
    })
}

pub fn parse_self_node(body: &[u8]) -> Result<SelfNode, LocalApiError> {
    let response: StatusResponse =
        serde_json::from_slice(body).map_err(|error| LocalApiError::Parse(error.to_string()))?;
    let node = response
        .self_node
        .ok_or_else(|| LocalApiError::Parse("status response has no Self".to_string()))?;
    Ok(SelfNode {
        stable_id: string_or_empty(node.stable_id),
        node_name: string_or_empty(node.name.clone().or(node.dns_name)),
        addresses: parse_addresses(&node.addresses, &node.tailscale_ips),
    })
}

fn string_or_empty(value: Option<String>) -> String {
    value.unwrap_or_default()
}

/// `UserProfile.ID` is a number in the LocalAPI JSON and a string in some
/// versions; both become a string here without inventing a value for absent.
fn json_id(value: Option<serde_json::Value>) -> String {
    match value {
        Some(serde_json::Value::String(text)) => text,
        Some(serde_json::Value::Number(number)) => number.to_string(),
        Some(serde_json::Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    }
}

/// `Addresses` are CIDR ("100.64.0.2/32"); `TailscaleIPs` are bare. Both are
/// accepted, duplicates removed, in first-seen order.
fn parse_addresses(addresses: &[String], tailscale_ips: &[String]) -> Vec<IpAddr> {
    let mut parsed = Vec::new();
    for entry in addresses.iter().chain(tailscale_ips.iter()) {
        let bare = entry.split('/').next().unwrap_or(entry);
        if let Ok(address) = IpAddr::from_str(bare) {
            if !parsed.contains(&address) {
                parsed.push(address);
            }
        }
    }
    parsed
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("../fixtures/wire/tailscale-whois.json");
    const STATUS_FIXTURE: &str = include_str!("../fixtures/wire/tailscale-status.json");

    #[test]
    fn recorded_whois_response_parses() {
        let whois = parse_whois(FIXTURE.as_bytes()).expect("fixture parses");
        assert_eq!(whois.stable_id, "nxd5gUfvzj11CNTRL");
        assert_eq!(whois.node_name, "marcos-macbook-pro.tail80a42d.ts.net.");
        assert_eq!(whois.login_name, "user@example.com");
        assert!(!whois.user_id.is_empty());
        assert!(!whois.cap_map_present, "CapMap is null on this machine");
        assert!(whois.addresses.contains(&"100.74.116.126".parse().unwrap()));
    }

    #[test]
    fn whois_without_a_node_is_a_parse_error() {
        assert!(matches!(
            parse_whois(br#"{"UserProfile":{"LoginName":"a@b"}}"#),
            Err(LocalApiError::Parse(_))
        ));
        assert!(matches!(
            parse_whois(br#"{"Node":{"Name":"host"}}"#),
            Err(LocalApiError::Parse(_))
        ));
        assert!(matches!(
            parse_whois(b"not json"),
            Err(LocalApiError::Parse(_))
        ));
    }

    #[test]
    fn self_node_parses_stable_id_and_addresses() {
        let node = parse_self_node(STATUS_FIXTURE.as_bytes()).expect("status parses");
        assert_eq!(node.stable_id, "nJZ7SELFNODE0001");
        assert_eq!(node.node_name, "marcolenovo.tail80a42d.ts.net.");
        assert_eq!(
            node.addresses,
            vec!["100.102.128.70".parse::<IpAddr>().unwrap()]
        );
    }

    #[test]
    fn http_response_requires_a_content_length_and_refuses_chunking() {
        let ok = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}";
        let (status, body) = parse_http_response(ok).expect("well formed");
        assert_eq!(status, 200);
        assert_eq!(body, b"{}");

        let missing = b"HTTP/1.1 200 OK\r\n\r\nhello";
        assert!(matches!(
            parse_http_response(missing),
            Err(LocalApiError::Protocol(_))
        ));

        let chunked =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Length: 2\r\n\r\n{}";
        assert!(matches!(
            parse_http_response(chunked),
            Err(LocalApiError::Protocol(_))
        ));

        let oversized = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
            BODY_CAP + 1
        );
        assert!(matches!(
            parse_http_response(oversized.as_bytes()),
            Err(LocalApiError::Protocol(_))
        ));

        let not_http = b"GARBAGE\r\nContent-Length: 0\r\n\r\n";
        assert!(matches!(
            parse_http_response(not_http),
            Err(LocalApiError::Protocol(_))
        ));
    }

    #[test]
    fn response_is_complete_only_with_the_whole_body() {
        assert!(!response_is_complete(b"HTTP/1.1 200 OK\r\nContent-Len").expect("partial headers"));
        assert!(
            !response_is_complete(b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nabc")
                .expect("short body")
        );
        assert!(
            response_is_complete(b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabc")
                .expect("complete")
        );
    }

    #[test]
    fn non_200_is_a_status_error() {
        let raw = b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\n\r\n";
        let (status, _) = parse_http_response(raw).expect("parses");
        assert_eq!(status, 401);
    }

    #[test]
    fn cidr_and_bare_addresses_are_both_accepted_without_duplicates() {
        let addresses = vec!["100.64.0.2/32".to_string(), "100.64.0.2/32".to_string()];
        let bare = vec!["100.64.0.2".to_string(), "fd7a::1".to_string()];
        let parsed = parse_addresses(&addresses, &bare);
        assert_eq!(
            parsed,
            vec![
                "100.64.0.2".parse::<IpAddr>().unwrap(),
                "fd7a::1".parse::<IpAddr>().unwrap()
            ]
        );
    }

    #[cfg(windows)]
    #[test]
    fn whois_is_served_from_an_in_process_named_pipe() {
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
        use std::sync::Arc;

        use crate::paths::RuntimePaths;
        use crate::transport::{Listener, NamedPipeListener};

        static COUNTER: AtomicU64 = AtomicU64::new(1);
        let dir = std::env::temp_dir().join(format!(
            "devboule localapi {}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        let mut paths = RuntimePaths::from_dir(&dir);
        paths.pipe_name = format!(
            "\\\\.\\pipe\\devboule-localapi-test-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        );

        let mut listener =
            NamedPipeListener::bind(&paths, Arc::new(AtomicBool::new(false))).expect("bind");
        let pipe_name = paths.pipe_name.clone();
        let server = std::thread::spawn(move || {
            let file = listener.accept().expect("accept");
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut request = [0u8; 1024];
            let read = crate::framing::read_chunk(&file, &mut request, Some(deadline))
                .expect("read request")
                .expect("request arrives");
            let request = String::from_utf8_lossy(&request[..read]);
            assert!(
                request.starts_with("GET /localapi/v0/whois?addr=100.74.116.126:443 HTTP/1.1"),
                "unexpected request line: {request}"
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                FIXTURE.len(),
                FIXTURE
            );
            crate::framing::write_all_overlapped(&file, response.as_bytes(), Some(deadline))
                .expect("write response");
        });

        let client = LocalApiClient::with_pipe_name(pipe_name, Duration::from_secs(5));
        let whois = client
            .whois("100.74.116.126:443".parse().expect("addr"))
            .expect("whois");
        assert_eq!(whois.stable_id, "nxd5gUfvzj11CNTRL");
        server.join().expect("server thread");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn a_missing_pipe_is_reported_as_absent() {
        let client = LocalApiClient::with_pipe_name(
            "\\\\.\\pipe\\devboule-localapi-does-not-exist",
            Duration::from_millis(200),
        );
        match client.self_node() {
            Err(LocalApiError::Absent(_)) => {}
            other => panic!("expected Absent, got {other:?}"),
        }
    }

    /// Run once on a machine with Tailscale (the design's §9 measurement):
    /// `cargo test -p devboule-daemon -- --ignored whois_live`.
    #[cfg(windows)]
    #[test]
    #[ignore = "requires a running tailscaled"]
    fn whois_live() {
        let client = LocalApiClient::new();
        let node = client.self_node().expect("live status");
        let address = node
            .addresses
            .first()
            .copied()
            .expect("this node has a tailnet address");
        let whois = client
            .whois(SocketAddr::new(address, 47831))
            .expect("live whois");
        assert_eq!(
            whois.stable_id, node.stable_id,
            "whois of this node's own address must resolve to this node"
        );
        // The raw response is never printed; the stable id is not PII.
        eprintln!(
            "whois_live ok: stable_id={} addresses={:?}",
            whois.stable_id, node.addresses
        );
    }
}
