//! Loopback accept loop and per-connection handling for the MCP channel.

use std::io;
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Weak};
use std::thread;
use std::time::Duration;

use serde_json::Value;
use uuid::Uuid;

use crate::server::ServerState;

use super::dispatch::handle_rpc;
use super::http::{read_http_request, send_http, serve_get_with_lifetime};
use super::McpBroker;

pub(super) const MCP_PATH: &str = "/mcp";

const MCP_PREAUTH_TIMEOUT: Duration = Duration::from_secs(2);
const HTTP_READ_TIMEOUT: Duration = Duration::from_secs(5);

pub(super) fn mcp_accept_loop(
    listener: TcpListener,
    broker: Arc<McpBroker>,
    state: Weak<ServerState>,
    stop: Arc<AtomicBool>,
    get_lifetime: Duration,
) {
    let mut clients = Vec::new();
    while !stop.load(Ordering::Acquire) {
        match listener.accept() {
            Ok((stream, _)) => {
                if !broker.try_acquire_connection() {
                    drop(stream);
                    continue;
                }
                let _ = stream.set_nonblocking(false);
                let _ = stream.set_read_timeout(Some(MCP_PREAUTH_TIMEOUT));
                let _ = stream.set_write_timeout(Some(HTTP_READ_TIMEOUT));
                let client_broker = Arc::clone(&broker);
                let client_state = state.clone();
                let permit = ConnectionPermit {
                    broker: Arc::clone(&broker),
                };
                if let Ok(handle) =
                    thread::Builder::new()
                        .name("mcp-client".into())
                        .spawn(move || {
                            let _permit = permit;
                            handle_connection(stream, client_broker, client_state, get_lifetime);
                        })
                {
                    clients.push(handle);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) if is_transient_accept_error(&error) => {
                thread::sleep(Duration::from_millis(50));
            }
            Err(_) => {
                stop.store(true, Ordering::Release);
                broker.fail_all("The MCP broker listener stopped unexpectedly.");
                break;
            }
        }
        clients.retain(|handle| !handle.is_finished());
    }
    for client in clients {
        let _ = client.join();
    }
}

pub(super) fn is_transient_accept_error(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock
            | io::ErrorKind::Interrupted
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::TimedOut
    ) || matches!(
        error.raw_os_error(),
        Some(12 | 23 | 24 | 105 | 10024 | 10055)
    )
}

pub(super) struct ConnectionPermit {
    pub(super) broker: Arc<McpBroker>,
}

impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        self.broker
            .active_connections
            .fetch_sub(1, Ordering::Release);
    }
}

fn handle_connection(
    mut stream: TcpStream,
    broker: Arc<McpBroker>,
    state: Weak<ServerState>,
    get_lifetime: Duration,
) {
    let Ok(Some(request)) = read_http_request(&mut stream) else {
        return;
    };
    let Some(registration) =
        broker.authenticate(request.headers.get("authorization").map(String::as_str))
    else {
        let _ = send_http(
            &mut stream,
            401,
            "application/json",
            br#"{"error":"unauthorized"}"#,
            &[],
        );
        return;
    };
    let _ = stream.set_read_timeout(Some(HTTP_READ_TIMEOUT));
    let path = request.path.split('?').next().unwrap_or_default();
    if path != MCP_PATH && path != "/" {
        let _ = send_http(
            &mut stream,
            404,
            "application/json",
            br#"{"error":"not found"}"#,
            &[],
        );
        return;
    }
    match request.method.as_str() {
        "GET" => {
            serve_get_with_lifetime(&mut stream, &broker, get_lifetime);
        }
        "DELETE" => {
            let _ = send_http(&mut stream, 200, "application/json", b"{}", &[]);
        }
        "POST" => {
            let Ok(message) = serde_json::from_slice::<Value>(&request.body) else {
                let _ = send_http(
                    &mut stream,
                    400,
                    "application/json",
                    br#"{"error":"invalid json"}"#,
                    &[],
                );
                return;
            };
            let Some(state) = state.upgrade() else {
                let _ = send_http(
                    &mut stream,
                    503,
                    "application/json",
                    br#"{"error":"daemon unavailable"}"#,
                    &[],
                );
                return;
            };
            let initialized = message.get("method").and_then(Value::as_str) == Some("initialize");
            let reply = handle_rpc(&state, &broker, &registration, &message);
            match reply {
                Ok(Some(reply)) => {
                    let Ok(body) = serde_json::to_vec(&reply) else {
                        return;
                    };
                    let mut headers = Vec::new();
                    let session_id = initialized.then(|| format!("mcp-{}", Uuid::new_v4()));
                    if let Some(session_id) = &session_id {
                        headers.push(("Mcp-Session-Id", session_id.as_str()));
                    }
                    let wants_sse = request
                        .headers
                        .get("accept")
                        .map(|accept| {
                            accept.contains("text/event-stream")
                                && !accept.contains("application/json")
                        })
                        .unwrap_or(false);
                    if wants_sse {
                        let payload = format!(
                            "event: message\ndata: {}\n\n",
                            String::from_utf8_lossy(&body)
                        );
                        let _ = send_http(
                            &mut stream,
                            200,
                            "text/event-stream",
                            payload.as_bytes(),
                            &headers,
                        );
                    } else {
                        let _ = send_http(&mut stream, 200, "application/json", &body, &headers);
                    }
                }
                Ok(None) => {
                    let _ = send_http(&mut stream, 202, "application/json", b"", &[]);
                }
                Err(error) => {
                    let Ok(body) = serde_json::to_vec(&error) else {
                        return;
                    };
                    let _ = send_http(&mut stream, 200, "application/json", &body, &[]);
                }
            }
        }
        _ => {
            let _ = send_http(
                &mut stream,
                405,
                "application/json",
                br#"{"error":"method not allowed"}"#,
                &[],
            );
        }
    }
}
