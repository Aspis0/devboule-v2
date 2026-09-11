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
/// How long an `Absent` answer is trusted. Without this a machine that does
/// not run Tailscale pays the whole connection budget on every call, and the
/// peer transport asks once per accepted connection.
const ABSENT_CACHE_TTL: Duration = Duration::from_secs(5);
/// A missing pipe is retried only this long: long enough for a tailscaled that
/// is still creating its pipe, short enough that "no Tailscale" is a quick
/// answer rather than a two-second one.
const ABSENT_GRACE: Duration = Duration::from_millis(200);

/// The last time each endpoint answered `Absent`. Process-global because the
/// clients are transient (one per call site) while the fact is not.
fn absent_cache() -> &'static std::sync::Mutex<std::collections::HashMap<String, Instant>> {
    static CACHE: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, Instant>>,
    > = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}
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

impl Endpoint {
    /// The cache key. Shows the transport kind and the path, never a secret:
    /// the endpoint has no credential in it today, and a future macOS
    /// port+token endpoint must add its own redaction here.
    pub fn key(&self) -> String {
        match self {
            #[cfg(windows)]
            Self::Pipe(name) => format!("pipe:{name}"),
            #[cfg(not(windows))]
            Self::UnixSocket(path) => format!("socket:{}", path.display()),
        }
    }
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

    /// Point the client at a non-default endpoint. The in-process test server
    /// uses this; a non-default pipe can also be selected with
    /// `DEVBOULE_TAILSCALE_ENDPOINT`.
    pub fn with_endpoint(endpoint: Endpoint, timeout: Duration) -> Self {
        Self { endpoint, timeout }
    }

    /// Point the client at an arbitrary pipe. Test-only: production talks to
    /// `LOCALAPI_PIPE` (or `DEVBOULE_TAILSCALE_ENDPOINT`).
    #[cfg(all(windows, test))]
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
        // A recent `Absent` is trusted, so "no Tailscale" is cheap to ask
        // repeatedly. A success or a different error clears it immediately.
        let key = self.endpoint.key();
        if let Some(since) = absent_cache()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(&key)
            .copied()
        {
            if since.elapsed() < ABSENT_CACHE_TTL {
                return Err(LocalApiError::Absent(format!("{key} is not available")));
            }
        }
        match self.get_uncached(path_and_query) {
            Err(LocalApiError::Absent(message)) => {
                absent_cache()
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .insert(key, Instant::now());
                Err(LocalApiError::Absent(message))
            }
            other => {
                absent_cache()
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .remove(&key);
                other
            }
        }
    }

    fn get_uncached(&self, path_and_query: &str) -> Result<Vec<u8>, LocalApiError> {
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
        SECURITY_IMPERSONATION, SECURITY_SQOS_PRESENT,
    };
    use windows_sys::Win32::System::Pipes::WaitNamedPipeW;

    let name = crate::security::wide(pipe_name);
    // A busy pipe is retried inside the caller's budget; a missing pipe only
    // inside the short grace, so "Tailscale is not running" stays a quick
    // answer (and is then cached by the caller).
    let absent_deadline = Instant::now() + ABSENT_GRACE;
    loop {
        let handle = unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                std::ptr::null(),
                OPEN_EXISTING,
                // The impersonation level is requested **explicitly**. Leaving
                // `SECURITY_SQOS_PRESENT` off does not mean "the default the
                // server wants": it means `SecurityAnonymous`, and tailscaled
                // answers 401 "Unable to impersonate using a named pipe until
                // data has been read" (measured on 1.102.2: the same request
                // returns 200 with any level from `Identification` up, and 401
                // with `None`). `SecurityImpersonation` is the level §9 names
                // and the one that works.
                FILE_ATTRIBUTE_NORMAL
                    | FILE_FLAG_OVERLAPPED
                    | SECURITY_SQOS_PRESENT
                    | SECURITY_IMPERSONATION,
                std::ptr::null_mut(),
            )
        };
        if handle != INVALID_HANDLE_VALUE {
            // SAFETY: CreateFileW returned a new owned handle.
            return Ok(unsafe { std::fs::File::from_raw_handle(handle as RawHandle) });
        }
        let error = unsafe { GetLastError() };
        if error == ERROR_PIPE_BUSY && Instant::now() < deadline {
            unsafe {
                WaitNamedPipeW(name.as_ptr(), 100);
            }
            continue;
        }
        if error == ERROR_FILE_NOT_FOUND && Instant::now() < absent_deadline {
            std::thread::sleep(Duration::from_millis(20));
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

/// How the body after the header block is delimited.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BodyFraming {
    /// `Content-Length: n`.
    Length(usize),
    /// `Transfer-Encoding: chunked`.
    Chunked,
}

/// Split and validate one bounded HTTP/1.1 response. Fail closed on a body
/// that is not framed exactly once, in a form this client decodes.
fn parse_http_response(raw: &[u8]) -> Result<(u16, Vec<u8>), LocalApiError> {
    let header_end = find_header_end(raw).ok_or_else(|| {
        LocalApiError::Protocol("response headers did not end within 8 KiB".to_string())
    })?;
    let (status, framing) = parse_headers(&raw[..header_end])?;
    let body_start = header_end + 4;
    let declared = &raw[body_start..];
    let body = match framing {
        BodyFraming::Length(length) => {
            let body = declared.get(..length).ok_or_else(|| {
                LocalApiError::Protocol("response body is shorter than its length".to_string())
            })?;
            // Anything after the declared body is a second, undeclared
            // response. Refused rather than ignored: a reader that stops at
            // Content-Length drops it silently, one that keeps reading parses
            // it, and the two disagreeing is the smuggling shape.
            if declared.len() > length {
                return Err(LocalApiError::Protocol(format!(
                    "response carries {} bytes after its declared body",
                    declared.len() - length
                )));
            }
            body.to_vec()
        }
        BodyFraming::Chunked => {
            let (body, consumed) = decode_chunked(declared)?
                .ok_or_else(|| LocalApiError::Protocol("chunked body is incomplete".to_string()))?;
            if declared.len() > consumed {
                return Err(LocalApiError::Protocol(format!(
                    "response carries {} bytes after its final chunk",
                    declared.len() - consumed
                )));
            }
            body
        }
    };
    Ok((status, body))
}

/// Returns the status code and how the body is delimited.
///
/// Exactly one framing is required. `Transfer-Encoding: chunked` together with
/// `Content-Length` is the request-smuggling shape and is refused rather than
/// resolved in either direction (RFC 7230 §3.3.3); so is a repeated
/// `Content-Length`.
///
/// Note for review: §9 of the design recorded this API as answering with
/// `Content-Length` and no chunking. On Tailscale 1.102.2 `/localapi/v0/status`
/// answers `Transfer-Encoding: chunked`, so chunked is decoded rather than
/// refused. Refusing it would make `self_node()` impossible to satisfy on the
/// machine the design was measured on.
fn parse_headers(header_bytes: &[u8]) -> Result<(u16, BodyFraming), LocalApiError> {
    let headers = std::str::from_utf8(header_bytes)
        .map_err(|_| LocalApiError::Protocol("response headers are not UTF-8".to_string()))?;
    let mut lines = headers.split("\r\n");
    let status_line = lines
        .next()
        .ok_or_else(|| LocalApiError::Protocol("empty response".to_string()))?;
    let status = parse_status_line(status_line)?;
    let mut content_length = None;
    let mut chunked = false;
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
            // Only the one encoding this client decodes is accepted.
            let mut codings = value.split(',').map(str::trim);
            let last = codings.next_back().unwrap_or_default();
            if !last.eq_ignore_ascii_case("chunked") || codings.any(|coding| !coding.is_empty()) {
                return Err(LocalApiError::Protocol(format!(
                    "unsupported Transfer-Encoding {value:?}"
                )));
            }
            chunked = true;
        }
        if name == "content-length" {
            if content_length.is_some() {
                return Err(LocalApiError::Protocol(
                    "response repeats Content-Length".to_string(),
                ));
            }
            let parsed = value.parse::<usize>().map_err(|_| {
                LocalApiError::Protocol(format!("invalid Content-Length {value:?}"))
            })?;
            if parsed > BODY_CAP {
                return Err(LocalApiError::Protocol(format!(
                    "Content-Length {parsed} exceeds the {BODY_CAP} byte body cap"
                )));
            }
            content_length = Some(parsed);
        }
    }
    match (chunked, content_length) {
        (true, Some(_)) => Err(LocalApiError::Protocol(
            "response carries both Transfer-Encoding and Content-Length".to_string(),
        )),
        (true, None) => Ok((status, BodyFraming::Chunked)),
        (false, Some(length)) => Ok((status, BodyFraming::Length(length))),
        (false, None) => Err(LocalApiError::Protocol(
            "response has no Content-Length and no chunked framing".to_string(),
        )),
    }
}

/// De-chunk `body`, strictly. `Ok(None)` means the terminator has not arrived
/// yet and the caller should keep reading; otherwise the decoded body and the
/// number of input bytes consumed.
///
/// Strictness is the point: only hex sizes, an optional `;ext`, CRLF in the
/// right places, and a decoded body bounded by `BODY_CAP`. A malformed chunk is
/// an error rather than a best-effort parse, which is what keeps this from
/// re-introducing the ambiguity that refusing chunked was protecting against.
fn decode_chunked(body: &[u8]) -> Result<Option<(Vec<u8>, usize)>, LocalApiError> {
    let mut position = 0usize;
    let mut decoded = Vec::new();
    loop {
        let Some(line_end) = find_crlf(&body[position..]) else {
            return Ok(None);
        };
        let header = &body[position..position + line_end];
        let size_text = header
            .split(|byte| *byte == b';')
            .next()
            .unwrap_or_default();
        let size_text = std::str::from_utf8(size_text)
            .map_err(|_| LocalApiError::Protocol("chunk size is not ASCII".to_string()))?
            .trim();
        if size_text.is_empty() || size_text.len() > 8 {
            return Err(LocalApiError::Protocol(format!(
                "invalid chunk size {size_text:?}"
            )));
        }
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| LocalApiError::Protocol(format!("invalid chunk size {size_text:?}")))?;
        position += line_end + 2;
        if size == 0 {
            // Trailer section: headers until a blank line. None are expected
            // from this API; they are skipped, not interpreted.
            loop {
                let Some(line_end) = find_crlf(&body[position..]) else {
                    return Ok(None);
                };
                if line_end == 0 {
                    position += 2;
                    return Ok(Some((decoded, position)));
                }
                position += line_end + 2;
            }
        }
        if decoded.len() + size > BODY_CAP {
            return Err(LocalApiError::Protocol(format!(
                "chunked body exceeds the {BODY_CAP} byte body cap"
            )));
        }
        let end = position + size;
        if body.len() < end + 2 {
            return Ok(None);
        }
        decoded.extend_from_slice(&body[position..end]);
        if &body[end..end + 2] != b"\r\n" {
            return Err(LocalApiError::Protocol(
                "chunk is not terminated by CRLF".to_string(),
            ));
        }
        position = end + 2;
    }
}

fn find_crlf(raw: &[u8]) -> Option<usize> {
    raw.windows(2).position(|window| window == b"\r\n")
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
    match parse_headers(&raw[..header_end])?.1 {
        BodyFraming::Length(length) => Ok(raw.len() >= header_end + 4 + length),
        // Chunked completeness is only knowable by scanning for the
        // terminator, which is exactly what `decode_chunked` returning `Some`
        // reports.
        BodyFraming::Chunked => Ok(decode_chunked(&raw[header_end + 4..])?.is_some()),
    }
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
    /// The stable node id. Tailscale's `/status` names this `ID` on
    /// 1.102.2 (measured): the `StableID` spelling is what `/whois` uses and
    /// what older builds emitted, and both are accepted because the value is
    /// the same one. This is the identity the peers table pins, so a build
    /// that stopped serving either spelling would be a startup failure, not a
    /// silent nil.
    #[serde(rename = "StableID", default)]
    stable_id: Option<String>,
    #[serde(rename = "ID", default)]
    id: Option<String>,
    #[serde(rename = "Name", default)]
    name: Option<String>,
    #[serde(rename = "HostName", default)]
    host_name: Option<String>,
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
    // `ID` and `StableID` carry the same value here (measured against
    // `whois` of this node's own address); a build that starts emitting a
    // fresh `ID` series would be caught by `whois_live`'s equality assertion.
    let stable_id = node.stable_id.clone().or(node.id.clone());
    Ok(SelfNode {
        stable_id: string_or_empty(stable_id),
        node_name: string_or_empty(
            node.name
                .clone()
                .or(node.dns_name.clone())
                .or(node.host_name),
        ),
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
    fn self_node_parses_the_id_spelling_that_1_102_2_actually_sends() {
        let node = parse_self_node(STATUS_FIXTURE.as_bytes()).expect("status parses");
        // The recorded fixture uses `ID`, which is what Tailscale 1.102.2's
        // `/status` `Self` object carries (the design's §9 recorded
        // `StableID`, which is the `/whois` spelling).
        assert_eq!(node.stable_id, "nbeQUiqfU811CNTRL");
        assert_eq!(node.node_name, "marcolenovo.tail80a42d.ts.net.");
        // `Self.TailscaleIPs` is what the node record carries, and on this
        // machine it is the IPv4 address alone. The top-level `TailscaleIPs`
        // array also holds the IPv6 address, but it is derived from the same
        // node record, so `Self` is the one read here.
        assert_eq!(
            node.addresses,
            vec!["100.102.128.70".parse::<IpAddr>().unwrap()]
        );
    }

    /// Both spellings are accepted, and `StableID` wins when a build sends
    /// both. A missing id is an empty string rather than a panic: the caller
    /// decides what to do about a node that cannot name itself.
    #[test]
    fn the_stable_id_is_read_from_either_spelling() {
        let both = br#"{ "Self": { "ID": "nID000", "StableID": "nStable000" } }"#;
        assert_eq!(parse_self_node(both).expect("both").stable_id, "nStable000");

        let id_only = br#"{ "Self": { "ID": "nID000" } }"#;
        assert_eq!(
            parse_self_node(id_only).expect("id only").stable_id,
            "nID000"
        );

        let stable_only = br#"{ "Self": { "StableID": "nStable000" } }"#;
        assert_eq!(
            parse_self_node(stable_only).expect("stable only").stable_id,
            "nStable000"
        );

        // `HostName` is the third spelling of the node name.
        let host_name = br#"{ "Self": { "ID": "nID000", "HostName": "Marcolenovo" } }"#;
        assert_eq!(
            parse_self_node(host_name).expect("host name").node_name,
            "Marcolenovo"
        );

        let nameless = br#"{ "Self": {} }"#;
        assert_eq!(parse_self_node(nameless).expect("nameless").stable_id, "");
    }

    #[test]
    fn http_response_requires_exactly_one_body_framing() {
        let ok = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}";
        let (status, body) = parse_http_response(ok).expect("well formed");
        assert_eq!(status, 200);
        assert_eq!(body, b"{}");

        let missing = b"HTTP/1.1 200 OK\r\n\r\nhello";
        assert!(matches!(
            parse_http_response(missing),
            Err(LocalApiError::Protocol(_))
        ));

        // Both framings at once is the smuggling shape: refused, not resolved.
        let both =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nContent-Length: 2\r\n\r\n0\r\n\r\n";
        match parse_http_response(both) {
            Err(LocalApiError::Protocol(message)) => {
                assert!(message.contains("both"), "{message}")
            }
            other => panic!("both framings must be refused, got {other:?}"),
        }

        // An encoding this client does not decode is refused.
        let gzip = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: gzip\r\n\r\nx";
        assert!(matches!(
            parse_http_response(gzip),
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

    /// F8: a repeated `Content-Length` (smuggling) and bytes after the declared
    /// body (a second, undeclared response) are both refused rather than
    /// silently believed.
    #[test]
    fn a_repeated_content_length_or_trailing_bytes_is_refused() {
        let repeated = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nContent-Length: 3\r\n\r\n{}";
        match parse_http_response(repeated) {
            Err(LocalApiError::Protocol(message)) => {
                assert!(message.contains("Content-Length"), "{message}")
            }
            other => panic!("a repeated Content-Length must be refused, got {other:?}"),
        }

        let trailing = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}HTTP/1.1 500";
        match parse_http_response(trailing) {
            Err(LocalApiError::Protocol(message)) => {
                assert!(message.contains("after its declared body"), "{message}")
            }
            other => panic!("trailing bytes must be refused, got {other:?}"),
        }

        // The exact body is still accepted, so the check is not "anything
        // extra at all".
        let exact = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}";
        assert!(parse_http_response(exact).is_ok());
    }

    /// Tailscale 1.102.2 answers `/localapi/v0/status` with
    /// `Transfer-Encoding: chunked`, so it is decoded rather than refused (the
    /// design's §9 recorded `Content-Length`; that measurement is stale). The
    /// decoded bytes are the body, and the parser rejects every malformed
    /// shape rather than guessing where the body ends.
    #[test]
    fn a_chunked_body_is_decoded_strictly() {
        let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\n\r\n";
        let (status, body) = parse_http_response(chunked).expect("chunked decodes");
        assert_eq!(status, 200);
        assert_eq!(body, b"hello world");

        // A chunk extension is ignored, not interpreted.
        let extended =
            b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2;ext=1\r\nok\r\n0\r\n\r\n";
        assert_eq!(parse_http_response(extended).expect("ext").1, b"ok");

        // Trailer headers are skipped.
        let trailers = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nok\r\n0\r\nX-Trace: 1\r\n\r\n";
        assert_eq!(parse_http_response(trailers).expect("trailers").1, b"ok");

        // Incomplete input is not an error yet: the caller keeps reading.
        let partial = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhel";
        assert!(
            !response_is_complete(partial).expect("partial is not a failure"),
            "a half-arrived chunk must not read as complete"
        );
        assert!(matches!(
            parse_http_response(partial),
            Err(LocalApiError::Protocol(_))
        ));
        assert!(response_is_complete(chunked).expect("complete"));

        // Malformed shapes are errors, never a best-effort parse.
        for malformed in [
            // Non-hex size.
            &b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nzz\r\nok\r\n0\r\n\r\n"[..],
            // Missing CRLF after the data.
            &b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nokXX0\r\n\r\n"[..],
            // Chunk shorter than declared.
            &b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n9\r\nok\r\n0\r\n\r\n"[..],
            // No terminating zero chunk.
            &b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nok\r\n"[..],
        ] {
            assert!(
                matches!(
                    parse_http_response(malformed),
                    Err(LocalApiError::Protocol(_))
                ),
                "malformed chunked body must be refused: {:?}",
                String::from_utf8_lossy(malformed)
            );
        }

        // Bytes after the final chunk are a second, undeclared response.
        let trailing = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nok\r\n0\r\n\r\nHTTP/1.1 500";
        match parse_http_response(trailing) {
            Err(LocalApiError::Protocol(message)) => {
                assert!(message.contains("after its final chunk"), "{message}")
            }
            other => panic!("trailing bytes must be refused, got {other:?}"),
        }

        // A chunked body that would blow the cap is refused while decoding,
        // before the whole thing is buffered.
        let huge = format!(
            "HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n",
            BODY_CAP + 1
        );
        match parse_http_response(huge.as_bytes()) {
            Err(LocalApiError::Protocol(message)) => {
                assert!(message.contains("body cap"), "{message}")
            }
            other => panic!("an oversized chunk must be refused, got {other:?}"),
        }
    }

    /// F6: a machine without Tailscale must not pay the whole connection
    /// budget on every call.
    #[cfg(windows)]
    #[test]
    fn an_absent_endpoint_is_remembered_for_a_while() {
        let name = format!(
            "\\\\.\\pipe\\devboule-localapi-absent-cache-{}",
            std::process::id()
        );
        let key = format!("pipe:{name}");
        absent_cache()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&key);
        let client = LocalApiClient::with_pipe_name(name, DEFAULT_TIMEOUT);

        let started = Instant::now();
        match client.self_node() {
            Err(LocalApiError::Absent(_)) => {}
            other => panic!("expected Absent, got {other:?}"),
        }
        let first = started.elapsed();
        assert!(
            first >= ABSENT_GRACE,
            "the first probe must actually look, took {first:?}"
        );

        let started = Instant::now();
        match client.self_node() {
            Err(LocalApiError::Absent(_)) => {}
            other => panic!("expected a cached Absent, got {other:?}"),
        }
        let second = started.elapsed();
        assert!(
            second < ABSENT_GRACE,
            "the cached answer must be immediate, took {second:?}"
        );
        absent_cache()
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&key);
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
        // Assertions only: this test's output is captured by S7's stderr gate,
        // and an identity or an address in a log line is the thing that gate
        // exists to catch.
        assert_eq!(
            whois.stable_id, node.stable_id,
            "whois of this node's own address must resolve to this node"
        );
        assert!(!whois.stable_id.is_empty());
        assert!(!node.addresses.is_empty());
    }
}
