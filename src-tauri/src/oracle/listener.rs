//! The loopback HTTP endpoint the daemon's broker forwards Oracle queries to.
//!
//! Slice 1 owns the socket, the bearer gate and the record; the query route
//! arrives with slice 2. HTTP is parsed by hand in the broker's shape
//! (`mcp_broker.rs`) — the app gains no server dependency.
//!
//! Timeouts per connection: 2 s on every `read()`, a 5 s deadline for the
//! whole request from the first byte, 5 s to write the response.

use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use devboule_daemon::{
    apply_current_user_dacl, oracle_app_lock_path, Heartbeat, OracleAppRecord, RuntimePaths,
    SingleInstanceLock,
};

const QUERY_PATH: &str = "/oracle/v1/query";
const MAX_HTTP_HEADER_BYTES: usize = 64 * 1024;
const MAX_HTTP_BODY_BYTES: usize = 64 * 1024;
const PREAUTH_TIMEOUT: Duration = Duration::from_secs(2);
const REQUEST_DEADLINE: Duration = Duration::from_secs(5);
const HTTP_WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const ACCEPT_POLL: Duration = Duration::from_millis(10);
const TOKEN_BYTES: usize = 32;
const INSTANCE_BYTES: usize = 8;
const ERR_UNAUTHORIZED: &[u8] = br#"{"error":"unauthorized"}"#;
const ERR_NOT_FOUND: &[u8] = br#"{"error":"not found"}"#;
const ERR_METHOD: &[u8] = br#"{"error":"method not allowed"}"#;
const NOT_IMPLEMENTED: &[u8] = br#"{"ok":false,"reason":"not_implemented"}"#;

/// The endpoint's own lifecycle, managed as Tauri state: `start` from the
/// `setup` closure, `stop` from `RunEvent::Exit`. The accept loop never runs
/// on the window's thread.
#[derive(Default)]
pub(crate) struct OracleEndpoint {
    running: Mutex<Option<Running>>,
}

struct Running {
    stop: Arc<AtomicBool>,
    accept: JoinHandle<()>,
    heartbeat: Heartbeat,
    _lock: SingleInstanceLock,
    record_path: PathBuf,
}

impl OracleEndpoint {
    /// Production entry: the runtime directory the daemon lock lives in.
    pub(crate) fn start(&self) -> io::Result<()> {
        self.start_at(&RuntimePaths::from_env()?)
    }

    /// Lock, bind, publish (`ready=1`), beat, serve — the daemon's order.
    pub(crate) fn start_at(&self, paths: &RuntimePaths) -> io::Result<()> {
        let mut slot = self.slot();
        if slot.is_some() {
            return Ok(());
        }
        paths.ensure_dir()?;
        let record_path = oracle_app_lock_path(paths);
        let mut lock = SingleInstanceLock::acquire_at(&record_path)
            .map_err(|error| io::Error::other(error.to_string()))?;
        if let Err(error) = apply_current_user_dacl(&record_path) {
            let _ = std::fs::remove_file(&record_path);
            return Err(error);
        }
        let mut entropy = [0u8; TOKEN_BYTES + INSTANCE_BYTES];
        getrandom::fill(&mut entropy).map_err(|error| io::Error::other(error.to_string()))?;
        let token = hex(&entropy[..TOKEN_BYTES]);
        let instance = hex(&entropy[TOKEN_BYTES..]);
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        let mut record = OracleAppRecord::new(std::process::id(), &instance, port, &token);
        record.listening();
        if let Err(error) = lock.write_body(&record.body()) {
            let _ = std::fs::remove_file(&record_path);
            return Err(error);
        }
        let heartbeat = match Heartbeat::start(&record_path) {
            Ok(heartbeat) => heartbeat,
            Err(error) => {
                let _ = std::fs::remove_file(&record_path);
                return Err(error);
            }
        };
        let stop = Arc::new(AtomicBool::new(false));
        let spawned = thread::Builder::new()
            .name("oracle-endpoint".into())
            .spawn({
                let stop = Arc::clone(&stop);
                let record_path = record_path.clone();
                move || accept_loop(listener, stop, token, record_path)
            });
        let accept = match spawned {
            Ok(handle) => handle,
            Err(error) => {
                let _ = std::fs::remove_file(&record_path);
                return Err(error);
            }
        };
        *slot = Some(Running {
            stop,
            accept,
            heartbeat,
            // Held, not read: releasing it on drop is the point.
            _lock: lock,
            record_path,
        });
        Ok(())
    }

    /// Stop the beat, drop the record, release the lock: an app that leaves
    /// must not leave a port behind that reads as live.
    pub(crate) fn stop(&self) {
        let taken = self.slot().take();
        let Some(mut running) = taken else {
            return;
        };
        running.stop.store(true, Ordering::Release);
        let _ = running.accept.join();
        running.heartbeat.stop();
        let _ = std::fs::remove_file(&running.record_path);
    }

    fn slot(&self) -> MutexGuard<'_, Option<Running>> {
        self.running
            .lock()
            .unwrap_or_else(|error| error.into_inner())
    }
}

impl Drop for OracleEndpoint {
    fn drop(&mut self) {
        self.stop();
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The port this record names cannot accept anymore: removing the record is
/// the only honest answer — a heartbeat cannot refresh what is not there.
fn accept_loop(listener: TcpListener, stop: Arc<AtomicBool>, token: String, record_path: PathBuf) {
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                let token = token.clone();
                let _ = thread::Builder::new()
                    .name("oracle-endpoint-client".into())
                    .spawn(move || handle_client(stream, &token));
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => thread::sleep(ACCEPT_POLL),
            Err(error) if is_transient_accept_error(&error) => {
                thread::sleep(Duration::from_millis(50))
            }
            Err(_) => {
                let _ = std::fs::remove_file(&record_path);
                break;
            }
        }
    }
}

fn is_transient_accept_error(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::Interrupted
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::TimedOut
    ) || matches!(
        error.raw_os_error(),
        Some(12 | 23 | 24 | 105 | 10024 | 10055)
    )
}

fn handle_client(mut stream: TcpStream, token: &str) {
    let _ = stream.set_nonblocking(false);
    let _ = stream.set_read_timeout(Some(PREAUTH_TIMEOUT));
    let _ = stream.set_write_timeout(Some(HTTP_WRITE_TIMEOUT));
    let Ok(Some(request)) = read_http_request(&mut stream) else {
        return;
    };
    // The gate runs before path, method or body: same order as the broker.
    let authorization = request.headers.get("authorization").map(String::as_str);
    if !authorized(authorization, token) {
        let _ = send_http(&mut stream, 401, "application/json", ERR_UNAUTHORIZED, &[]);
        return;
    }
    let path = request.path.split('?').next().unwrap_or_default();
    if path != QUERY_PATH {
        let _ = send_http(&mut stream, 404, "application/json", ERR_NOT_FOUND, &[]);
        return;
    }
    if request.method != "POST" {
        let _ = send_http(&mut stream, 405, "application/json", ERR_METHOD, &[]);
        return;
    }
    // Slice 1 placeholder: the engine arrives with slice 2. The bearer has
    // already been proved against the one in the record.
    let _ = send_http(&mut stream, 200, "application/json", NOT_IMPLEMENTED, &[]);
}

fn authorized(authorization: Option<&str>, token: &str) -> bool {
    authorization
        .and_then(|header| header.strip_prefix("Bearer "))
        .is_some_and(|bearer| bearer == token)
}

struct HttpRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
}

/// Header cap, chunked refusal, body cap: the broker's parser, copied.
fn read_http_request(stream: &mut TcpStream) -> io::Result<Option<HttpRequest>> {
    let opened = Instant::now();
    let mut bytes = Vec::new();
    let header_end = loop {
        if opened.elapsed() > REQUEST_DEADLINE {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "request deadline"));
        }
        let mut chunk = [0u8; 4096];
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            return Ok(None);
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.len() > MAX_HTTP_HEADER_BYTES {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "HTTP headers too large",
            ));
        }
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let header_text = std::str::from_utf8(&bytes[..header_end - 4])
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "HTTP headers are not UTF-8"))?;
    let mut lines = header_text.split("\r\n");
    let request_line = lines
        .next()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing HTTP request line"))?;
    let mut request_parts = request_line.split_whitespace();
    let method = request_parts.next().unwrap_or_default().to_string();
    let path = request_parts.next().unwrap_or_default().to_string();
    if request_parts.next().is_none() || method.is_empty() || path.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid HTTP request line",
        ));
    }
    let mut headers = HashMap::new();
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid HTTP header",
            ));
        };
        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
    }
    if headers
        .get("transfer-encoding")
        .is_some_and(|value| !value.eq_ignore_ascii_case("identity"))
    {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "chunked HTTP is unsupported",
        ));
    }
    let content_length = headers
        .get("content-length")
        .map(|value| {
            value
                .parse::<usize>()
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "invalid content length"))
        })
        .transpose()?
        .unwrap_or(0);
    if content_length > MAX_HTTP_BODY_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "HTTP body too large",
        ));
    }
    while bytes.len() < header_end + content_length {
        if opened.elapsed() > REQUEST_DEADLINE {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "request deadline"));
        }
        let mut chunk = [0u8; 4096];
        let count = stream.read(&mut chunk)?;
        if count == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "truncated HTTP body",
            ));
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    Ok(Some(HttpRequest {
        method,
        path,
        headers,
    }))
}

fn send_http(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
    extra_headers: &[(&str, &str)],
) -> io::Result<()> {
    let reason = match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        _ => "Error",
    };
    let mut response = format!(
        "HTTP/1.1 {status} {reason}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    for (name, value) in extra_headers {
        response.push_str(name);
        response.push_str(": ");
        response.push_str(value);
        response.push_str("\r\n");
    }
    response.push_str("\r\n");
    stream.write_all(response.as_bytes())?;
    stream.write_all(body)
}
