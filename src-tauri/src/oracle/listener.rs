//! The loopback HTTP endpoint the daemon's broker forwards Oracle queries to.
//!
//! The listener owns the socket, the bearer gate and the record; the codec is
//! [`super::endpoint_http`] and the query route is [`super::endpoint_query`].
//! HTTP is parsed by hand in the broker's shape (`mcp_broker/http.rs`) — the app
//! gains no server dependency.
//!
//! Timeouts per connection: 2 s on every `read()`, a 5 s deadline for the
//! whole request from the first byte, 5 s to write the response.

use std::io;
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use devboule_daemon::{
    apply_current_user_dacl, oracle_app_lock_path, Heartbeat, OracleAppRecord, RuntimePaths,
    SingleInstanceLock,
};

use super::endpoint_http::{read_http_request, send_http};
use super::endpoint_query::{respond, AppHost, QueryHost};

const QUERY_PATH: &str = "/oracle/v1/query";
const PREAUTH_TIMEOUT: Duration = Duration::from_secs(2);
const HTTP_WRITE_TIMEOUT: Duration = Duration::from_secs(5);
const ACCEPT_POLL: Duration = Duration::from_millis(10);
const TOKEN_BYTES: usize = 32;
const INSTANCE_BYTES: usize = 8;
const ERR_UNAUTHORIZED: &[u8] = br#"{"error":"unauthorized"}"#;
const ERR_NOT_FOUND: &[u8] = br#"{"error":"not found"}"#;
const ERR_METHOD: &[u8] = br#"{"error":"method not allowed"}"#;

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
    pub(crate) fn start(&self, app: tauri::AppHandle) -> io::Result<()> {
        let host = Arc::new(AppHost::new(app));
        self.start_at(&RuntimePaths::from_env()?, host.clone())?;
        host.warm();
        Ok(())
    }

    /// Lock, bind, publish (`ready=1`), beat, serve — the daemon's order.
    pub(super) fn start_at(
        &self,
        paths: &RuntimePaths,
        host: Arc<dyn QueryHost>,
    ) -> io::Result<()> {
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
        if let Err(error) = getrandom::fill(&mut entropy) {
            let _ = std::fs::remove_file(&record_path);
            return Err(io::Error::other(error.to_string()));
        }
        let token = hex(&entropy[..TOKEN_BYTES]);
        let instance = hex(&entropy[TOKEN_BYTES..]);
        let listener = match TcpListener::bind(("127.0.0.1", 0)) {
            Ok(listener) => listener,
            Err(error) => {
                let _ = std::fs::remove_file(&record_path);
                return Err(error);
            }
        };
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
                let host = Arc::clone(&host);
                move || accept_loop(listener, stop, token, record_path, host)
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
fn accept_loop(
    listener: TcpListener,
    stop: Arc<AtomicBool>,
    token: String,
    record_path: PathBuf,
    host: Arc<dyn QueryHost>,
) {
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                let token = token.clone();
                let host = Arc::clone(&host);
                let _ = thread::Builder::new()
                    .name("oracle-endpoint-client".into())
                    .spawn(move || handle_client(stream, &token, host.as_ref()));
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

fn handle_client(mut stream: TcpStream, token: &str, host: &dyn QueryHost) {
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
    // The bearer has been proved against the one in the record; from here the
    // route owns the answer, refusal envelopes included.
    let (status, body) = respond(host, &request.body);
    let _ = send_http(&mut stream, status, "application/json", &body, &[]);
}

fn authorized(authorization: Option<&str>, token: &str) -> bool {
    authorization
        .and_then(|header| header.strip_prefix("Bearer "))
        .is_some_and(|bearer| bearer == token)
}
