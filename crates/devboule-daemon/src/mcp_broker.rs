//! The daemon-owned MCP channel for live agent sessions.
//!
//! The broker is deliberately small: one loopback HTTP listener, one bearer
//! token per session, and one read-only tool. Pi is not listed here because
//! its RPC wire has no MCP concept. Stable agent names are not a protocol
//! field yet, so the tool returns `name: null` and keeps the existing title as
//! a separate display-only field.
//!
//! Gemini is the only ACP provider in the catalog not measured on this
//! machine because it is not installed. If it does not call `tools/list`, its
//! first prompt fails after the finite readiness timeout with that honest
//! broker message.

use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use devboule_protocol::{
    OwnerId, SessionEvent, SessionKind, SessionState, ToolPolicyEntry, WireError,
};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::server::ServerState;

pub(crate) const MCP_SERVER_NAME: &str = "devboule";
/// A provider that never emits its MCP-ready signal gets a finite failure,
/// and its first prompt is rejected with that fact. The daemon never waits
/// forever on an undocumented provider event.
pub(crate) const MCP_READY_TIMEOUT: Duration = Duration::from_secs(15);

const MCP_PATH: &str = "/mcp";
const MAX_HTTP_HEADER_BYTES: usize = 64 * 1024;
const MAX_HTTP_BODY_BYTES: usize = 64 * 1024;
const MCP_PREAUTH_TIMEOUT: Duration = Duration::from_secs(2);
const HTTP_READ_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_MCP_CONNECTIONS: usize = 64;
const MCP_GET_LIFETIME: Duration = Duration::from_secs(30);
const CONFIG_PREFIX: &str = "devboule-mcp-";

/// Launch data is kept out of `PtyCommand` so provider command tests cannot
/// accidentally serialize a bearer into a debug value or a normal log.
pub(crate) struct McpLaunchConfig {
    pub(crate) url: String,
    bearer: String,
    pub(crate) claude_config_path: Option<PathBuf>,
}

impl McpLaunchConfig {
    #[cfg(test)]
    pub(crate) fn for_test(url: &str, bearer: &str) -> Self {
        Self {
            url: url.to_string(),
            bearer: bearer.to_string(),
            claude_config_path: None,
        }
    }

    pub(crate) fn acp_server_value(&self) -> Value {
        json!({
            "type": "http",
            "name": MCP_SERVER_NAME,
            "url": self.url,
            "headers": [{
                "name": "Authorization",
                "value": format!("Bearer {}", self.bearer),
            }],
        })
    }

    pub(crate) fn redact_text(&self, text: &str) -> String {
        redact_broker_text(text, Some(&self.url), Some(&self.bearer))
    }
}

#[derive(Clone)]
struct RegisteredSession {
    owner: OwnerId,
    /// Catalog provider this session was created for (`claude`, `grok`, …).
    /// The tool policy is keyed by it; `None` is a caller that had no
    /// provider id, which serves every broker tool (the pre-policy default).
    provider_id: Option<String>,
    bearer: String,
    claude_config_path: Option<PathBuf>,
    runtime: Option<Weak<crate::session::SessionRuntime>>,
    broker_ready: Arc<AtomicBool>,
}

#[derive(Default)]
struct SessionIndex {
    by_session: HashMap<String, RegisteredSession>,
    by_bearer: HashMap<String, String>,
}

/// Held by the live registry entry. Dropping it revokes the bearer and
/// removes the Claude configuration file, including on provider exit.
pub(crate) struct McpSessionGuard {
    broker: Arc<McpBroker>,
    session_id: String,
    bearer: String,
    claude_config_path: Option<PathBuf>,
}

impl Drop for McpSessionGuard {
    fn drop(&mut self) {
        self.broker.remove_if_current(
            &self.session_id,
            &self.bearer,
            self.claude_config_path.as_deref(),
        );
    }
}

pub(crate) struct McpBroker {
    runtime_dir: PathBuf,
    url: String,
    listener: Mutex<Option<TcpListener>>,
    sessions: Mutex<SessionIndex>,
    stop: Arc<AtomicBool>,
    active_connections: AtomicUsize,
}

impl McpBroker {
    pub(crate) fn new(runtime_dir: &Path) -> io::Result<Self> {
        let _ = cleanup_stale_configs(runtime_dir);
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let port = listener.local_addr()?.port();
        Ok(Self {
            runtime_dir: runtime_dir.to_path_buf(),
            url: format!("http://127.0.0.1:{port}{MCP_PATH}"),
            listener: Mutex::new(Some(listener)),
            sessions: Mutex::new(SessionIndex {
                by_session: HashMap::new(),
                by_bearer: HashMap::new(),
            }),
            stop: Arc::new(AtomicBool::new(false)),
            active_connections: AtomicUsize::new(0),
        })
    }

    /// Register a session whose catalog provider the caller did not supply.
    /// Equivalent to [`Self::register_with_provider`] with no provider id.
    /// Test-only seam: the ungated registration every session used before
    /// `session.rs` passed its provider id. It exists so a unit test can prove
    /// the policy is consulted only through `register_with_provider`.
    #[cfg(test)]
    pub(crate) fn register(
        self: &Arc<Self>,
        session_id: &str,
        owner: &OwnerId,
        kind: &SessionKind,
    ) -> Result<Option<McpSessionGuard>, WireError> {
        self.register_with_provider(session_id, owner, kind, None)
    }

    /// Register `session_id` for `kind`, recording the catalog provider it
    /// belongs to so `tools/list` and `tools/call` can apply that provider's
    /// tool policy (`SessionCreate.provider`).
    pub(crate) fn register_with_provider(
        self: &Arc<Self>,
        session_id: &str,
        owner: &OwnerId,
        kind: &SessionKind,
        provider_id: Option<&str>,
    ) -> Result<Option<McpSessionGuard>, WireError> {
        if !matches!(kind, SessionKind::Acp | SessionKind::Claude) {
            return Ok(None);
        }
        if self.stop.load(Ordering::Acquire) {
            return Err(WireError::new(
                devboule_protocol::ErrorCode::Io,
                "The MCP broker is stopped.",
            ));
        }

        self.cleanup_session(session_id);
        let bearer = Uuid::new_v4().to_string();
        let claude_config_path = if *kind == SessionKind::Claude {
            let path = self
                .runtime_dir
                .join(format!("{CONFIG_PREFIX}{}.json", Uuid::new_v4()));
            let config = json!({
                "mcpServers": {
                    MCP_SERVER_NAME: {
                        "type": "http",
                        "url": self.url,
                        "headers": {
                            "Authorization": format!("Bearer {bearer}"),
                        },
                    },
                },
            });
            if let Err(error) = write_protected_json(&path, &config) {
                return Err(WireError::new(
                    devboule_protocol::ErrorCode::Io,
                    format!("Could not prepare the MCP configuration: {error}"),
                ));
            }
            Some(path)
        } else {
            None
        };

        let registration = RegisteredSession {
            owner: owner.clone(),
            provider_id: provider_id.map(str::to_string),
            bearer: bearer.clone(),
            claude_config_path: claude_config_path.clone(),
            runtime: None,
            broker_ready: Arc::new(AtomicBool::new(false)),
        };
        let result = self
            .sessions
            .lock()
            .map_err(|_| {
                WireError::new(
                    devboule_protocol::ErrorCode::Internal,
                    "MCP state is unavailable.",
                )
            })
            .map(|mut sessions| {
                if let Some(previous) = sessions
                    .by_session
                    .insert(session_id.to_string(), registration.clone())
                {
                    sessions.by_bearer.remove(&previous.bearer);
                }
                sessions
                    .by_bearer
                    .insert(bearer.clone(), session_id.to_string());
            });
        if result.is_err() {
            remove_file(claude_config_path.as_deref());
        }
        // Return the revocation guard with the registration so callers never
        // hold an unguarded token between these two operations.
        result.map(|()| {
            Some(McpSessionGuard {
                broker: Arc::clone(self),
                session_id: session_id.to_string(),
                bearer,
                claude_config_path,
            })
        })
    }

    pub(crate) fn launch_config(&self, session_id: &str) -> Option<McpLaunchConfig> {
        let sessions = self.sessions.lock().ok()?;
        let registration = sessions.by_session.get(session_id)?;
        Some(McpLaunchConfig {
            url: self.url.clone(),
            bearer: registration.bearer.clone(),
            claude_config_path: registration.claude_config_path.clone(),
        })
    }

    pub(crate) fn bind_runtime(
        &self,
        session_id: &str,
        runtime: &Arc<crate::session::SessionRuntime>,
    ) {
        let broker_ready = self.sessions.lock().ok().and_then(|mut sessions| {
            let registration = sessions.by_session.get_mut(session_id)?;
            registration.runtime = Some(Arc::downgrade(runtime));
            runtime.set_mcp_bearer(registration.bearer.clone());
            runtime.set_mcp_url(self.url.clone());
            Some(registration.broker_ready.load(Ordering::Acquire))
        });
        if self.stop.load(Ordering::Acquire) {
            runtime.fail_mcp("The MCP broker stopped before this session connected.");
        } else if broker_ready == Some(true) {
            runtime.mark_mcp_ready();
        }
    }

    fn mark_broker_ready(&self, registration: &RegisteredSession) {
        registration.broker_ready.store(true, Ordering::Release);
        if let Some(runtime) = registration.runtime.as_ref().and_then(Weak::upgrade) {
            runtime.mark_mcp_ready();
        }
    }

    fn fail_all(&self, message: &str) {
        let runtimes = self
            .sessions
            .lock()
            .map(|sessions| {
                sessions
                    .by_session
                    .values()
                    .filter_map(|registration| {
                        registration.runtime.as_ref().and_then(Weak::upgrade)
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        for runtime in runtimes {
            runtime.fail_mcp_broker(message.to_string());
        }
    }

    /// Start the already-bound listener. Binding happens in `new` so session
    /// creation tests can build launch config without spawning a thread, while
    /// the production daemon still owns exactly one listener.
    pub(crate) fn start(self: &Arc<Self>, state: &Arc<ServerState>) -> io::Result<McpServerHandle> {
        self.start_with_get_lifetime(state, MCP_GET_LIFETIME)
    }

    fn start_with_get_lifetime(
        self: &Arc<Self>,
        state: &Arc<ServerState>,
        get_lifetime: Duration,
    ) -> io::Result<McpServerHandle> {
        let listener = self
            .listener
            .lock()
            .map_err(|_| io::Error::other("MCP listener state is unavailable"))?
            .take()
            .ok_or_else(|| io::Error::other("MCP listener was already started"))?;
        let stop = Arc::clone(&self.stop);
        let thread_stop = Arc::clone(&stop);
        let broker = Arc::clone(self);
        let state = Arc::downgrade(state);
        let join = thread::Builder::new()
            .name("mcp-accept".into())
            .spawn(move || mcp_accept_loop(listener, broker, state, thread_stop, get_lifetime))?;
        Ok(McpServerHandle {
            broker: Arc::clone(self),
            stop,
            join: Some(join),
        })
    }

    fn try_acquire_connection(&self) -> bool {
        self.active_connections
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |active| {
                (active < MAX_MCP_CONNECTIONS).then_some(active + 1)
            })
            .is_ok()
    }

    fn authenticate(&self, authorization: Option<&str>) -> Option<RegisteredSession> {
        let bearer = authorization?.strip_prefix("Bearer ")?;
        if bearer.is_empty() {
            return None;
        }
        let sessions = self.sessions.lock().ok()?;
        let session_id = sessions.by_bearer.get(bearer)?;
        sessions.by_session.get(session_id).cloned()
    }

    fn cleanup_session(&self, session_id: &str) {
        let registration = self.sessions.lock().ok().and_then(|mut sessions| {
            let registration = sessions.by_session.remove(session_id)?;
            sessions.by_bearer.remove(&registration.bearer);
            Some(registration)
        });
        if let Some(registration) = registration {
            remove_file(registration.claude_config_path.as_deref());
        }
    }

    fn remove_if_current(&self, session_id: &str, bearer: &str, claude_config_path: Option<&Path>) {
        let removed = self.sessions.lock().ok().and_then(|mut sessions| {
            if sessions.by_bearer.get(bearer).map(String::as_str) != Some(session_id) {
                return None;
            }
            let removed = sessions.by_session.remove(session_id)?;
            sessions.by_bearer.remove(&removed.bearer);
            Some(removed)
        });
        remove_file(
            removed
                .as_ref()
                .and_then(|registration| registration.claude_config_path.as_deref())
                .or(claude_config_path),
        );
    }

    #[cfg(test)]
    pub(crate) fn test_token(&self, session_id: &str) -> Option<String> {
        self.sessions.lock().ok().and_then(|sessions| {
            sessions
                .by_session
                .get(session_id)
                .map(|entry| entry.bearer.clone())
        })
    }
}

impl Drop for McpBroker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.fail_all("The MCP broker stopped before the next prompt.");
        let registrations = self
            .sessions
            .get_mut()
            .ok()
            .map(std::mem::take)
            .unwrap_or_default();
        for registration in registrations.by_session.values() {
            remove_file(registration.claude_config_path.as_deref());
        }
    }
}

pub(crate) struct McpServerHandle {
    broker: Arc<McpBroker>,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl Drop for McpServerHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.broker
            .fail_all("The MCP broker stopped before the next prompt.");
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn mcp_accept_loop(
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

fn is_transient_accept_error(error: &io::Error) -> bool {
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

struct ConnectionPermit {
    broker: Arc<McpBroker>,
}

impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        self.broker
            .active_connections
            .fetch_sub(1, Ordering::Release);
    }
}

struct HttpRequest {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
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

fn handle_rpc(
    state: &ServerState,
    broker: &McpBroker,
    registration: &RegisteredSession,
    message: &Value,
) -> Result<Option<Value>, Value> {
    let id = message.get("id").cloned().unwrap_or(Value::Null);
    let method = message.get("method").and_then(Value::as_str);
    let Some(method) = method else {
        return Ok(Some(rpc_error(id, -32600, "Invalid Request")));
    };
    match method {
        "initialize" => Ok(Some(json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "protocolVersion": message
                    .pointer("/params/protocolVersion")
                    .cloned()
                    .unwrap_or_else(|| json!("2025-03-26")),
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": MCP_SERVER_NAME, "version": env!("CARGO_PKG_VERSION")},
            },
        }))),
        "notifications/initialized" | "notifications/cancelled" => Ok(None),
        "ping" => Ok(Some(json!({"jsonrpc": "2.0", "id": id, "result": {}}))),
        "server/discover" => Ok(Some(rpc_error(
            id,
            -32601,
            "Method not found: server/discover",
        ))),
        "tools/list" => {
            // An authenticated tools/list is the broker's proof that this
            // provider has connected with this session's Bearer.
            broker.mark_broker_ready(registration);
            let policy = state.tool_policy.get(registration.provider_id.as_deref());
            Ok(Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {"tools": enabled_tool_list(
                    crate::provider_catalog::MCP_BROKER_TOOLS,
                    policy.as_ref(),
                )},
            })))
        }
        "tools/call" => {
            let tool_name = message.pointer("/params/name").and_then(Value::as_str);
            // The policy guard runs before the name check, so a disabled tool
            // is refused for the reason that actually applies and an
            // unserved name cannot be probed past the policy.
            let policy = state.tool_policy.get(registration.provider_id.as_deref());
            if let Some(tool_name) = tool_name {
                if !crate::tool_policy::is_tool_enabled(policy.as_ref(), tool_name) {
                    return Ok(Some(rpc_error(id, -32601, "Tool disabled by policy")));
                }
            }
            if tool_name != Some(crate::provider_catalog::MCP_ROSTER_TOOL) {
                return Ok(Some(rpc_error(id, -32601, "Unknown tool")));
            }
            // Deliberately do not read params.arguments. The bearer maps to
            // the caller; an agent id supplied by the model is not identity.
            let agents = state
                .sessions
                .live_agent_entries(&registration.owner)
                .map_err(|error| json!({"jsonrpc":"2.0", "id": id, "error": {"code": -32603, "message": error.message}}))?
                .into_iter()
                .map(|entry| agent_value(&entry.session, &entry.runtime))
                .collect::<Vec<_>>();
            let document = json!({"agents": agents});
            let text = serde_json::to_string(&document).map_err(|error| {
                json!({"jsonrpc":"2.0", "id": id, "error": {"code": -32603, "message": format!("Could not encode agent roster: {error}")}})
            })?;
            Ok(Some(json!({
                "jsonrpc": "2.0",
                "id": id,
                "result": {
                    "content": [{"type": "text", "text": text}],
                    "structuredContent": document,
                    "isError": false,
                },
            })))
        }
        _ if message.get("id").is_none() => Ok(None),
        _ => Ok(Some(rpc_error(id, -32601, "Method not found"))),
    }
}

fn rpc_error(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// The `tools/list` body for one policy: the catalog minus the tools that
/// policy disables. The catalog is a parameter so the filter can be tested
/// against a tool other than the always-on roster tool.
fn enabled_tool_list(catalog: &[(&str, &str)], policy: Option<&ToolPolicyEntry>) -> Vec<Value> {
    catalog
        .iter()
        .filter(|(name, _)| crate::tool_policy::is_tool_enabled(policy, name))
        .map(|(name, description)| {
            json!({
                "name": name,
                "description": description,
                "inputSchema": {"type": "object", "properties": {}, "additionalProperties": false},
            })
        })
        .collect()
}

fn agent_value(
    session: &devboule_protocol::Session,
    runtime: &crate::session::SessionRuntime,
) -> Value {
    let manifest = runtime.session_manifest();
    let manifest_provider = manifest.as_ref().and_then(|event| match event {
        SessionEvent::SessionManifest { provider_id, .. } => provider_id.clone(),
        _ => None,
    });
    let model = manifest.as_ref().and_then(|event| match event {
        SessionEvent::SessionManifest {
            current_model_id, ..
        } => current_model_id.clone(),
        _ => None,
    });
    let state = match session.state {
        SessionState::Live { .. } => "live",
        SessionState::Silent { .. } => "silent",
        SessionState::Ended { .. } => "ended",
        SessionState::Recovered { .. } => "recovered",
    };
    json!({
        "id": session.id,
        "provider": session.provider.clone().or(manifest_provider),
        "model": model,
        "state": state,
        "name": Value::Null,
        "title": session.title,
    })
}

fn read_http_request(stream: &mut TcpStream) -> io::Result<Option<HttpRequest>> {
    let mut bytes = Vec::new();
    let header_end = loop {
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
        body: bytes[header_end..header_end + content_length].to_vec(),
    }))
}

fn send_sse_open(stream: &mut TcpStream) -> io::Result<()> {
    stream.write_all(
        b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nConnection: keep-alive\r\n\r\n:\n\n",
    )
}

fn serve_get_with_lifetime(stream: &mut TcpStream, broker: &McpBroker, lifetime: Duration) {
    if send_sse_open(stream).is_err() {
        return;
    }
    let deadline = Instant::now() + lifetime;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(100)));
    let mut byte = [0u8; 1];
    while !broker.stop.load(Ordering::Acquire) {
        if Instant::now() >= deadline {
            break;
        }
        match stream.read(&mut byte) {
            Ok(0) => break,
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {}
            Err(_) => break,
        }
    }
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
        202 => "Accepted",
        400 => "Bad Request",
        401 => "Unauthorized",
        404 => "Not Found",
        405 => "Method Not Allowed",
        413 => "Payload Too Large",
        503 => "Service Unavailable",
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

fn write_protected_json(path: &Path, value: &Value) -> io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            "MCP config has no parent directory",
        )
    })?;
    fs::create_dir_all(parent)?;
    let temp = path.with_extension("tmp");
    let bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&temp)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        protect_file(&temp)?;
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

fn cleanup_stale_configs(runtime_dir: &Path) -> io::Result<()> {
    let Ok(entries) = fs::read_dir(runtime_dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let stale = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| {
                name.starts_with(CONFIG_PREFIX)
                    && (name.ends_with(".json") || name.ends_with(".tmp"))
            });
        if stale {
            let _ = fs::remove_file(path);
        }
    }
    Ok(())
}

#[cfg(windows)]
fn protect_file(path: &Path) -> io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;
    use windows_sys::Win32::Foundation::{GetLastError, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
    };
    use windows_sys::Win32::Security::{
        SetFileSecurityW, DACL_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR,
    };

    let sid = crate::security::current_user_sid()?;
    let sddl = crate::security::user_only_sddl(&sid);
    let wide_sddl: Vec<u16> = sddl.encode_utf16().chain(std::iter::once(0)).collect();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    let converted = unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            wide_sddl.as_ptr(),
            SDDL_REVISION_1,
            &mut descriptor,
            ptr::null_mut(),
        )
    };
    if converted == 0 || descriptor.is_null() {
        return Err(io::Error::from_raw_os_error(
            unsafe { GetLastError() } as i32
        ));
    }
    let wide_path: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let result =
        unsafe { SetFileSecurityW(wide_path.as_ptr(), DACL_SECURITY_INFORMATION, descriptor) };
    let error = (result == 0).then(|| unsafe { GetLastError() });
    unsafe {
        LocalFree(descriptor as _);
    }
    error.map_or(Ok(()), |error| {
        Err(io::Error::from_raw_os_error(error as i32))
    })
}

#[cfg(not(windows))]
fn protect_file(_path: &Path) -> io::Result<()> {
    Ok(())
}

fn remove_file(path: Option<&Path>) {
    if let Some(path) = path {
        let _ = fs::remove_file(path);
    }
}

pub(crate) fn redact_secret(text: &str, secret: &str) -> String {
    if secret.is_empty() {
        text.to_string()
    } else {
        text.replace(secret, "[redacted]")
    }
}

pub(crate) fn redact_broker_text(text: &str, url: Option<&str>, bearer: Option<&str>) -> String {
    let mut redacted = bearer
        .map(|bearer| redact_secret(text, bearer))
        .unwrap_or_else(|| text.to_string());
    let Some(url) = url else {
        return redacted;
    };
    redacted = redact_secret(&redacted, url);
    let Some(endpoint) = url
        .strip_prefix("http://")
        .and_then(|url| url.split('/').next())
    else {
        return redacted;
    };
    redacted = redact_secret(&redacted, endpoint);
    if let Some(port) = endpoint
        .rsplit_once(':')
        .map(|(_, port)| port)
        .filter(|port| !port.is_empty())
    {
        redacted = redact_secret(&redacted, port);
    }
    redacted
}

pub(crate) fn ready_timeout() -> Duration {
    std::env::var("DEVBOULE_MCP_READY_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .filter(|value| !value.is_zero())
        .unwrap_or(MCP_READY_TIMEOUT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Shutdown;
    use std::sync::mpsc;

    fn owner(user: &str, client: &str) -> OwnerId {
        OwnerId::new(user, client).expect("owner")
    }

    fn endpoint(url: &str) -> String {
        url.strip_prefix("http://")
            .expect("loopback URL")
            .split('/')
            .next()
            .expect("loopback endpoint")
            .to_string()
    }

    fn http_request(url: &str, authorization: Option<&str>, body: &str) -> String {
        let mut stream = TcpStream::connect(endpoint(url)).expect("MCP listener");
        let authorization = authorization
            .map(|value| format!("Authorization: {value}\r\n"))
            .unwrap_or_default();
        let request = format!(
            "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{authorization}\r\n{body}",
            body.len()
        );
        stream.write_all(request.as_bytes()).expect("MCP request");
        stream.shutdown(Shutdown::Write).expect("request shutdown");
        let mut response = Vec::new();
        stream.read_to_end(&mut response).expect("MCP response");
        String::from_utf8(response).expect("HTTP response")
    }

    fn response_json(response: &str) -> Value {
        serde_json::from_str(
            response
                .split_once("\r\n\r\n")
                .expect("HTTP response body")
                .1,
        )
        .expect("JSON response")
    }

    #[test]
    fn loopback_requests_require_the_session_bearer() {
        let state = ServerState::new("mcp-auth".to_string());
        let first_owner = owner("mcp-user-first", "mcp-client-first");
        let second_owner = owner("mcp-user-second", "mcp-client-second");
        let first_guard = state
            .mcp
            .register("first", &first_owner, &SessionKind::Acp)
            .expect("first registration")
            .expect("first MCP guard");
        let second_guard = state
            .mcp
            .register("second", &second_owner, &SessionKind::Acp)
            .expect("second registration")
            .expect("second MCP guard");
        let first_token = state.mcp.test_token("first").expect("first token");
        let second_token = state.mcp.test_token("second").expect("second token");
        assert_eq!(
            state
                .mcp
                .authenticate(Some(&format!("Bearer {first_token}")))
                .expect("first auth")
                .owner,
            first_owner
        );
        assert_eq!(
            state
                .mcp
                .authenticate(Some(&format!("Bearer {second_token}")))
                .expect("second auth")
                .owner,
            second_owner
        );
        assert!(state.mcp.authenticate(None).is_none());
        assert!(state
            .mcp
            .authenticate(Some("Bearer token-from-another-session"))
            .is_none());

        let server = state.mcp.start(&state).expect("MCP server");
        let url = state.mcp.url.clone();
        let no_bearer = http_request(
            &url,
            None,
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        );
        assert!(no_bearer.starts_with("HTTP/1.1 401"));
        let wrong_bearer = http_request(
            &url,
            Some("Bearer token-from-another-session"),
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        );
        assert!(wrong_bearer.starts_with("HTTP/1.1 401"));
        drop(first_guard);
        let revoked = http_request(
            &url,
            Some(&format!("Bearer {first_token}")),
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#,
        );
        assert!(revoked.starts_with("HTTP/1.1 401"));
        drop(second_guard);
        drop(server);
    }

    #[test]
    fn connection_cap_is_enforced_before_client_spawn() {
        let state = ServerState::new("mcp-connection-cap".to_string());
        let permits = (0..MAX_MCP_CONNECTIONS)
            .map(|_| {
                assert!(state.mcp.try_acquire_connection());
                ConnectionPermit {
                    broker: Arc::clone(&state.mcp),
                }
            })
            .collect::<Vec<_>>();
        assert_eq!(permits.len(), MAX_MCP_CONNECTIONS);
        assert!(!state.mcp.try_acquire_connection());
        drop(permits);
        assert!(state.mcp.try_acquire_connection());
        let permit = ConnectionPermit {
            broker: Arc::clone(&state.mcp),
        };
        drop(permit);
    }

    #[test]
    fn listener_rejects_overflow_and_admits_a_client_after_preauth_expiry() {
        let state = ServerState::new("mcp-listener-cap".to_string());
        let owner = owner("mcp-listener-user", "mcp-listener-client");
        let guard = state
            .mcp
            .register("listener-session", &owner, &SessionKind::Acp)
            .expect("registration")
            .expect("MCP guard");
        let token = state.mcp.test_token("listener-session").expect("token");
        let server = state.mcp.start(&state).expect("MCP server");
        let url = state.mcp.url.clone();
        let mut flood = Vec::with_capacity(MAX_MCP_CONNECTIONS);
        for _ in 0..MAX_MCP_CONNECTIONS {
            let mut stream = TcpStream::connect(endpoint(&url)).expect("flood connection");
            stream.write_all(b"GET ").expect("flood request prefix");
            flood.push(stream);
        }
        let deadline = Instant::now() + Duration::from_secs(1);
        while state.mcp.active_connections.load(Ordering::Acquire) < MAX_MCP_CONNECTIONS
            && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(
            state.mcp.active_connections.load(Ordering::Acquire),
            MAX_MCP_CONNECTIONS
        );

        let mut overflow = TcpStream::connect(endpoint(&url)).expect("overflow connection");
        overflow
            .set_read_timeout(Some(Duration::from_secs(1)))
            .expect("overflow read timeout");
        overflow
            .write_all(
                format!(
                    "POST /mcp HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\nContent-Length: 0\r\n\r\n"
                )
                .as_bytes(),
            )
            .expect("overflow request");
        overflow
            .shutdown(Shutdown::Write)
            .expect("overflow shutdown");
        thread::sleep(Duration::from_millis(100));
        let mut overflow_response = Vec::new();
        let overflow_result = overflow.read_to_end(&mut overflow_response);
        let overflow_closed = match &overflow_result {
            Ok(_) => true,
            Err(error) => matches!(
                error.kind(),
                io::ErrorKind::ConnectionAborted
                    | io::ErrorKind::ConnectionReset
                    | io::ErrorKind::TimedOut
            ),
        };
        assert!(
            overflow_response.is_empty() && overflow_closed,
            "overflow connection must be dropped before dispatch: {overflow_result:?} {overflow_response:?}"
        );

        let deadline = Instant::now() + Duration::from_secs(5);
        while state.mcp.active_connections.load(Ordering::Acquire) != 0 && Instant::now() < deadline
        {
            thread::sleep(Duration::from_millis(25));
        }
        assert_eq!(state.mcp.active_connections.load(Ordering::Acquire), 0);
        let response = http_request(
            &url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        );
        assert!(response.starts_with("HTTP/1.1 200"));
        drop(flood);
        drop(server);
        drop(guard);
    }

    #[test]
    fn accept_resource_errors_are_retried_but_permanent_errors_stop() {
        let transient_kinds = [
            io::ErrorKind::WouldBlock,
            io::ErrorKind::Interrupted,
            io::ErrorKind::ConnectionAborted,
            io::ErrorKind::ConnectionReset,
            io::ErrorKind::TimedOut,
        ];
        for kind in transient_kinds {
            assert!(is_transient_accept_error(&io::Error::from(kind)));
        }
        for code in [12, 23, 24, 105, 10024, 10055] {
            assert!(is_transient_accept_error(&io::Error::from_raw_os_error(
                code
            )));
        }
        assert!(!is_transient_accept_error(&io::Error::from(
            io::ErrorKind::BrokenPipe,
        )));
        assert!(!is_transient_accept_error(&io::Error::from_raw_os_error(
            12345
        )));
    }

    #[test]
    fn get_stream_has_a_bounded_lifetime() {
        let state = ServerState::new("mcp-get-lifetime".to_string());
        let owner = owner("mcp-get-user", "mcp-get-client");
        let guard = state
            .mcp
            .register("get-session", &owner, &SessionKind::Acp)
            .expect("registration")
            .expect("MCP guard");
        let token = state.mcp.test_token("get-session").expect("token");
        let server = state
            .mcp
            .start_with_get_lifetime(&state, Duration::from_millis(10))
            .expect("MCP server");
        let mut client = TcpStream::connect(endpoint(&state.mcp.url)).expect("test client");
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .expect("client read timeout");
        client
            .write_all(
                format!(
                    "GET /mcp HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer {token}\r\n\r\n"
                )
                .as_bytes(),
            )
            .expect("GET request");
        let mut response = Vec::new();
        client.read_to_end(&mut response).expect("GET response");
        let response = String::from_utf8(response).expect("GET response text");
        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(response.contains("Content-Type: text/event-stream"));
        drop(server);
        drop(guard);
    }

    #[test]
    fn http_bearer_owns_the_filtered_roster_not_tool_arguments() {
        let state = ServerState::new("mcp-arguments".to_string());
        let caller = owner("mcp-argument-user-a", "mcp-argument-client-a");
        let other = owner("mcp-argument-user-b", "mcp-argument-client-b");
        crate::session::insert_test_live_agent(&state.sessions, "agent-a", caller.clone());
        crate::session::insert_test_live_agent(&state.sessions, "agent-b", other.clone());
        let caller_guard = state
            .mcp
            .register("agent-a", &caller, &SessionKind::Acp)
            .expect("registration")
            .expect("caller MCP guard");
        let other_guard = state
            .mcp
            .register("agent-b", &other, &SessionKind::Acp)
            .expect("registration")
            .expect("other MCP guard");
        let caller_token = state.mcp.test_token("agent-a").expect("caller token");
        let server = state.mcp.start(&state).expect("MCP server");
        let response = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {caller_token}")),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_list_agents","arguments":{"agent_id":"agent-b"}}}"#,
        );
        assert!(response.starts_with("HTTP/1.1 200"));
        let body = response_json(&response);
        let agents = body
            .pointer("/result/structuredContent/agents")
            .and_then(Value::as_array)
            .expect("agent roster");
        assert!(agents.iter().any(|agent| agent["id"] == "agent-a"));
        assert!(!agents.iter().any(|agent| agent["id"] == "agent-b"));
        let other_token = state.mcp.test_token("agent-b").expect("other token");
        let other_response = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {other_token}")),
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"devboule_list_agents"}}"#,
        );
        let other_body = response_json(&other_response);
        let other_agents = other_body
            .pointer("/result/structuredContent/agents")
            .and_then(Value::as_array)
            .expect("other agent roster");
        assert!(!other_agents.iter().any(|agent| agent["id"] == "agent-a"));
        assert!(other_agents.iter().any(|agent| agent["id"] == "agent-b"));
        drop(caller_guard);
        drop(other_guard);
        drop(server);
    }

    #[test]
    fn broker_tools_list_is_the_readiness_authority() {
        let state = ServerState::new("mcp-readiness".to_string());
        let owner = owner("mcp-readiness-user", "mcp-readiness-client");
        let guard = state
            .mcp
            .register("session", &owner, &SessionKind::Acp)
            .expect("registration")
            .expect("MCP guard");
        let runtime = Arc::new(crate::session::SessionRuntime::new());
        runtime.require_mcp();
        state.mcp.bind_runtime("session", &runtime);
        let token = state.mcp.test_token("session").expect("token");
        let server = state.mcp.start(&state).expect("MCP server");
        let waiting = Arc::clone(&runtime);
        let result = std::thread::spawn(move || waiting.wait_for_mcp_ready(Duration::from_secs(1)));
        let response = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        );
        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(result.join().expect("readiness waiter").is_ok());
        drop(server);
        let error = runtime
            .wait_for_mcp_ready(Duration::from_millis(1))
            .expect_err("a stopped broker must revoke readiness");
        assert!(error.message.contains("MCP broker stopped"));
        drop(guard);
    }

    #[test]
    fn the_tool_list_filter_drops_a_named_disabled_tool() {
        let catalog: &[(&str, &str)] = &[
            (crate::provider_catalog::MCP_ROSTER_TOOL, "the roster"),
            ("some_future_tool", "a tool a policy can turn off"),
        ];
        assert_eq!(enabled_tool_list(catalog, None).len(), 2);

        let selective = ToolPolicyEntry {
            provider_id: "claude".to_string(),
            enabled: Some(true),
            disabled_tools: vec!["some_future_tool".to_string()],
        };
        let filtered = enabled_tool_list(catalog, Some(&selective));
        assert_eq!(filtered.len(), 1);
        assert_eq!(
            filtered[0]["name"],
            crate::provider_catalog::MCP_ROSTER_TOOL
        );

        let globally_off = ToolPolicyEntry {
            provider_id: "claude".to_string(),
            enabled: Some(false),
            disabled_tools: Vec::new(),
        };
        let always_on = enabled_tool_list(catalog, Some(&globally_off));
        assert_eq!(
            always_on.len(),
            1,
            "a disabled policy still lists the always-on roster tool"
        );
    }

    #[test]
    fn a_disabled_tool_is_refused_at_call_time_and_the_roster_still_answers() {
        let state = ServerState::new("mcp-tool-policy-call".to_string());
        let owner = owner("mcp-policy-user", "mcp-policy-client");
        let guard = state
            .mcp
            .register_with_provider("policy-session", &owner, &SessionKind::Acp, Some("claude"))
            .expect("registration")
            .expect("MCP guard");
        state
            .tool_policy
            .set("claude", Some(true), vec!["some_future_tool".to_string()])
            .expect("policy");
        let token = state.mcp.test_token("policy-session").expect("token");
        let server = state.mcp.start(&state).expect("MCP server");

        let refused = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"some_future_tool"}}"#,
        );
        let refused_body = response_json(&refused);
        assert_eq!(refused_body.pointer("/error/code"), Some(&json!(-32601)));
        assert_eq!(
            refused_body.pointer("/error/message"),
            Some(&json!("Tool disabled by policy"))
        );

        // The same policy leaves the always-on roster tool working: a toggle
        // that locked the agent out of its own roster would be a footgun.
        let allowed = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"devboule_list_agents"}}"#,
        );
        assert!(allowed.starts_with("HTTP/1.1 200"));
        assert_eq!(response_json(&allowed)["result"]["isError"], false);

        // A name no policy mentions is still `Unknown tool`: the two refusals
        // mean different things to the agent.
        let unknown = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"no_such_tool"}}"#,
        );
        assert_eq!(
            response_json(&unknown).pointer("/error/message"),
            Some(&json!("Unknown tool"))
        );

        // And `tools/list` for the same session still reports the roster tool.
        let listed = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/list"}"#,
        );
        assert_eq!(
            response_json(&listed)
                .pointer("/result/tools")
                .and_then(Value::as_array)
                .map(|tools| tools.len()),
            Some(1)
        );
        drop(server);
        drop(guard);
    }

    #[test]
    fn a_globally_disabled_policy_still_lists_the_always_on_tool() {
        let state = ServerState::new("mcp-tool-policy-list".to_string());
        let owner = owner("mcp-policy-list-user", "mcp-policy-list-client");
        let guard = state
            .mcp
            .register_with_provider("silent-session", &owner, &SessionKind::Acp, Some("grok"))
            .expect("registration")
            .expect("MCP guard");
        state
            .tool_policy
            .set(
                "grok",
                Some(false),
                vec![crate::provider_catalog::MCP_ROSTER_TOOL.to_string()],
            )
            .expect("policy");
        let token = state.mcp.test_token("silent-session").expect("token");
        let server = state.mcp.start(&state).expect("MCP server");
        let response = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        );
        assert!(response.starts_with("HTTP/1.1 200"));
        let body = response_json(&response);
        let tools = body
            .pointer("/result/tools")
            .and_then(Value::as_array)
            .expect("tool list");
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0]["name"], crate::provider_catalog::MCP_ROSTER_TOOL);
        drop(server);
        drop(guard);
    }

    /// The inertness proof the contract asks for: without the peer's
    /// `register_with_provider` switch this gate cannot fire.
    ///
    /// [`McpBroker::register`] carries no provider id, so a session
    /// registered through it has `provider_id: None`, `is_tool_enabled(None,
    /// _)` is true for every tool, and the whole catalog is served whatever
    /// the store holds — no name is ever refused as `Tool disabled by
    /// policy`. The same request against the same stored policy is refused in
    /// `the_provider_registered_path_applies_that_policy`; the two tests
    /// differ only in the register call the session came through, so together
    /// they pin the gate to the provider id taken at registration rather than
    /// to the policy file alone.
    ///
    /// This is not a gap pinned for the record: `session.rs` still registers
    /// through the three-argument call, so this is the live production path
    /// today and the test runs. When the peer switches both call sites to
    /// `register_with_provider`, `register` loses its last caller and this
    /// test goes with it — until then it is the only thing standing between
    /// "the gate is inert" and an assertion nobody executes.
    #[test]
    fn the_unpatched_register_path_does_not_consult_a_stored_policy() {
        let state = ServerState::new("mcp-tool-policy-gap".to_string());
        let owner = owner("mcp-policy-gap-user", "mcp-policy-gap-client");
        let guard = state
            .mcp
            .register("unnamed-session", &owner, &SessionKind::Acp)
            .expect("registration")
            .expect("MCP guard");
        state
            .tool_policy
            .set("claude", Some(true), vec!["some_future_tool".to_string()])
            .expect("policy");
        let token = state.mcp.test_token("unnamed-session").expect("token");
        let server = state.mcp.start(&state).expect("MCP server");

        // A policy that disables `some_future_tool` for claude is on disk and
        // this session has no provider id, so the lookup yields nothing and
        // the name falls through to the unknown-tool answer. "Tool disabled
        // by policy" here would mean the gate had been reached.
        let response = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"some_future_tool"}}"#,
        );
        assert_eq!(
            response_json(&response).pointer("/error/message"),
            Some(&json!("Unknown tool")),
            "the unpatched path must not reach the policy"
        );

        // And the broker still lists its whole catalog — measured against
        // the catalog rather than against a literal, so the claim stays
        // "every tool" as the catalog grows.
        let listed = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        );
        let listed_body = response_json(&listed);
        assert_eq!(
            listed_body
                .pointer("/result/tools")
                .and_then(Value::as_array)
                .map(|tools| tools.len()),
            Some(crate::provider_catalog::MCP_BROKER_TOOLS.len())
        );

        // The one tool the catalog does serve still answers, so the session
        // is fully served: nothing on this path consults a policy.
        let roster = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"devboule_list_agents"}}"#,
        );
        assert!(roster.starts_with("HTTP/1.1 200"));
        assert_eq!(response_json(&roster)["result"]["isError"], false);
        drop(server);
        drop(guard);
    }

    /// The inverse expectation of the ignored test above: the same request
    /// and the same stored policy, refused by the policy because this session
    /// did name its provider. This is the behaviour the peer's `session.rs`
    /// patch switches the production path onto.
    #[test]
    fn the_provider_registered_path_applies_that_policy() {
        let state = ServerState::new("mcp-tool-policy-gated".to_string());
        let owner = owner("mcp-policy-gated-user", "mcp-policy-gated-client");
        let guard = state
            .mcp
            .register_with_provider("gated-session", &owner, &SessionKind::Acp, Some("claude"))
            .expect("registration")
            .expect("MCP guard");
        state
            .tool_policy
            .set("claude", Some(true), vec!["some_future_tool".to_string()])
            .expect("policy");
        let token = state.mcp.test_token("gated-session").expect("token");
        let server = state.mcp.start(&state).expect("MCP server");

        let response = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"some_future_tool"}}"#,
        );
        assert_eq!(
            response_json(&response).pointer("/error/message"),
            Some(&json!("Tool disabled by policy"))
        );
        drop(server);
        drop(guard);
    }

    #[test]
    fn broker_readiness_is_isolated_by_the_authenticated_session() {
        let state = ServerState::new("mcp-readiness-isolation".to_string());
        let first_owner = owner("mcp-readiness-first", "mcp-readiness-first-client");
        let second_owner = owner("mcp-readiness-second", "mcp-readiness-second-client");
        let first_guard = state
            .mcp
            .register("first", &first_owner, &SessionKind::Acp)
            .expect("first registration")
            .expect("first MCP guard");
        let second_guard = state
            .mcp
            .register("second", &second_owner, &SessionKind::Acp)
            .expect("second registration")
            .expect("second MCP guard");
        let first_runtime = Arc::new(crate::session::SessionRuntime::new());
        first_runtime.require_mcp();
        state.mcp.bind_runtime("first", &first_runtime);
        let second_runtime = Arc::new(crate::session::SessionRuntime::new());
        second_runtime.require_mcp();
        state.mcp.bind_runtime("second", &second_runtime);
        let first_token = state.mcp.test_token("first").expect("first token");
        let server = state.mcp.start(&state).expect("MCP server");
        let response = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {first_token}")),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        );
        assert!(response.starts_with("HTTP/1.1 200"));
        assert!(first_runtime
            .wait_for_mcp_ready(Duration::from_millis(1))
            .is_ok());
        let second_error = second_runtime
            .wait_for_mcp_ready(Duration::from_millis(1))
            .expect_err("the other session must remain unready");
        assert!(second_error.message.contains("first prompt was not sent"));
        drop(server);
        drop(first_guard);
        drop(second_guard);
    }

    #[test]
    fn provider_failure_after_broker_proof_does_not_revoke_readiness() {
        let runtime = crate::session::SessionRuntime::new();
        runtime.require_mcp();
        runtime.mark_mcp_ready();
        runtime.fail_mcp("provider reported a transient MCP failure");
        assert!(runtime.wait_for_mcp_ready(Duration::from_millis(1)).is_ok());
    }

    #[test]
    fn claude_bearer_file_is_removed_with_the_session_guard() {
        let state = ServerState::new("mcp-config".to_string());
        let session_id = "claude-session";
        let agent = owner("mcp-config-user", "mcp-config-client");
        let guard = state
            .mcp
            .register(session_id, &agent, &SessionKind::Claude)
            .expect("registration")
            .expect("MCP guard");
        let config = state.mcp.launch_config(session_id).expect("launch config");
        let path = config.claude_config_path.clone().expect("Claude path");
        assert!(path.exists());
        let contents = fs::read_to_string(&path).expect("config contents");
        assert!(contents.contains("Authorization"));
        drop(guard);
        assert!(!path.exists());
        assert!(state.mcp.test_token(session_id).is_none());
    }

    #[test]
    fn first_prompt_waits_for_ready_and_provider_silence_times_out() {
        let runtime = Arc::new(crate::session::SessionRuntime::new());
        runtime.require_mcp();
        let waiting = Arc::clone(&runtime);
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            tx.send(waiting.wait_for_mcp_ready(Duration::from_secs(1)))
                .expect("wait result");
        });
        assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
        runtime.mark_mcp_ready();
        assert!(rx
            .recv_timeout(Duration::from_secs(1))
            .expect("ready result")
            .is_ok());

        let silent = crate::session::SessionRuntime::new();
        silent.require_mcp();
        let error = silent
            .wait_for_mcp_ready(Duration::from_millis(1))
            .expect_err("silent provider must not wait forever");
        assert!(error.message.contains("first prompt was not sent"));
    }

    #[test]
    fn session_exit_wakes_an_mcp_readiness_waiter() {
        let runtime = Arc::new(crate::session::SessionRuntime::new());
        runtime.require_mcp();
        let waiting = Arc::clone(&runtime);
        let (tx, rx) = mpsc::channel();
        thread::spawn(move || {
            tx.send(waiting.wait_for_mcp_ready(Duration::from_secs(1)))
                .expect("wait result");
        });
        assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
        runtime.mark_exited(None);
        let error = rx
            .recv_timeout(Duration::from_secs(1))
            .expect("exit should wake the waiter")
            .expect_err("an exited session cannot become MCP ready");
        assert!(error.message.contains("process exited"));
    }
}
