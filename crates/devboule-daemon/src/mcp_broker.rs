//! The daemon-owned MCP channel for live agent sessions.
//!
//! The broker is deliberately small: one loopback HTTP listener, one bearer
//! token per session, and the six broker tools served to every agent family —
//! ACP and Claude natively, pi through its bridge extension (S5), Codex
//! through its `CODEX_HOME` carrier (S6), both verified post-spawn (S7/S8).
//! Stable agent names are not a protocol field yet, so the roster tool returns
//! `name: null` and keeps the existing title as a separate display-only field.
//!
//! Gemini is the only ACP provider in the catalog not measured on this
//! machine because it is not installed. If it does not call `tools/list`, its
//! first prompt fails after the finite readiness timeout with that honest
//! broker message.

use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::journal::AuditRecord;
use devboule_protocol::{
    CreateAgentCard, OwnerId, PeerRole, PermissionOption, PermissionOutcome, SessionEvent,
    SessionKind, SessionOrigin, SessionOriginKind, ToolPolicyEntry, WireError,
};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::provider_catalog::ToolOverlay;
use crate::server::ServerState;

pub(crate) const MCP_SERVER_NAME: &str = "devboule";
/// Whether a session hosts the daemon's MCP tools, for every surface a person reads.
///
/// A tri-state on purpose: "we have not established whether this session has
/// tools" (`Unverified`) is not "it has none" (`Unavailable`). On any surface
/// a person reads, the unknown never renders as the benign state.
///
/// The wire carries plain strings (`as_str` / `tools_state_from_str`) so no
/// protocol change is needed for the type itself; the broker keeps the reason
/// alongside where it needs one, the wire never sees it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ToolsState {
    Hosted,
    Unavailable,
    Unverified,
}

impl ToolsState {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            ToolsState::Hosted => "hosted",
            ToolsState::Unavailable => "unavailable",
            ToolsState::Unverified => "unverified",
        }
    }
}

/// Parse half of the S1 closed-table walk. No non-test consumer yet: S8's roster
/// parse reads it (a stored word back to the variant). Kept beside `as_str` so
/// the two directions cannot drift; the walk test pins both today.
#[allow(dead_code)]
pub(crate) fn tools_state_from_str(value: &str) -> Option<ToolsState> {
    match value {
        "hosted" => Some(ToolsState::Hosted),
        "unavailable" => Some(ToolsState::Unavailable),
        "unverified" => Some(ToolsState::Unverified),
        _ => None,
    }
}

/// The single computation point for [`ToolsState`]: no other file computes the
/// state; surfaces call this function.
///
/// `registered` is whether the broker holds a registration (a minted bearer)
/// for the session; `verified` is whether the carrier has proven itself since
/// (an authenticated `tools/list` for ACP/Claude today, the pi announce plus
/// in-child round-trip and the Codex poll from S8 tomorrow). `kind` is carried
/// for the S9 `hosts_mcp()` wiring and does not change today's answer:
/// registration is the fact, and a session with a bearer but without
/// verification reads `Unverified` for every kind — never `Hosted`, never
/// `Unavailable` — while a session with no registration reads `Unavailable`.
/// The door is origin-based, not kind-based, so kind never enters the match
/// beyond the unused binding the S9 move will read.
///
/// Provider dimension stays open: this matches on no provider name, only on
/// the two booleans every family shares.
pub(crate) fn compute_tools_state(
    _kind: &SessionKind,
    registered: bool,
    verified: bool,
) -> ToolsState {
    if !registered {
        ToolsState::Unavailable
    } else if verified {
        ToolsState::Hosted
    } else {
        ToolsState::Unverified
    }
}

/// The sentence the gate logs when it returns `Ok(None)` (S2, kept through S9).
///
/// One function so the log line is pinned by a test: it names the session id,
/// the kind and provider, and the state `unavailable` with the reason. Only
/// Terminal takes this branch now — every agent kind registers — so the
/// sentence says what is true for it. `register_with_provider` emits exactly
/// this string via `eprintln!`. Unit tests cannot capture `eprintln!` output,
/// so they pin this sentence; the emission itself is verified by reading the
/// daemon log on a Terminal create (stated adaptation, see report).
pub(crate) fn phase0_gate_log(
    session_id: &str,
    kind: &SessionKind,
    provider_id: Option<&str>,
) -> String {
    format!(
        "mcp broker: session {session_id} kind {kind:?} provider {} tools unavailable: this kind hosts no MCP channel",
        provider_id.unwrap_or("<none>"),
    )
}

/// A provider that never emits its MCP-ready signal gets a finite failure,
/// and its first prompt is rejected with that fact. The daemon never waits
/// forever on an undocumented provider event.
pub(crate) const MCP_READY_TIMEOUT: Duration = Duration::from_secs(15);

/// Who gets a broker (S9): the single predicate every gate site calls. One
/// function, one answer — when it flips, registration, roster, send-readiness
/// and startup flip with it, and every future site flips by calling it instead
/// of spelling kinds. The answer lives in the provider impls
/// (`Provider::hosts_mcp`); this shim keeps the signature the gate sites have
/// always read, now answered through the registry. Never provider-name
/// strings — the provider dimension stays open; the catalog owns names.
pub(crate) fn hosts_mcp(kind: &SessionKind) -> bool {
    crate::session::catalog_registry()
        .provider_for_kind(kind)
        .hosts_mcp()
}

/// Whose first prompt may wait on the broker (S9): ACP/Claude only, deliberately
/// narrower than `hosts_mcp`. Carriers are best-effort and slow (Codex measured
/// ~7.4 s against a dead broker); blocking a healthy pi/Codex child's first
/// prompt on them would make an outage of the broker an outage of the child.
/// The wait itself no-ops without `require_mcp` (S8 split); the twin
/// never-block tests pin the rule. NOT a new flag — the S8 default, named.
/// The answer lives in the provider impls (`Provider::mcp_gates_first_prompt`);
/// this shim keeps the signature the gate sites have always read, now answered
/// through the registry.
pub(crate) fn mcp_gates_first_prompt(kind: &SessionKind) -> bool {
    crate::session::catalog_registry()
        .provider_for_kind(kind)
        .mcp_gates_first_prompt()
}

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
///
/// S4 hygiene: the struct stays bearer+url (+ the Claude path it already had).
/// No `pi_*`/`codex_*` named fields ever land here — family knowledge lives in
/// the client modules, which expose `mcp_launch(&McpLaunchConfig, &runtime_dir)
/// -> Result<McpProviderConfig>` (S5/S6; the provider-trait seam verbatim), and
/// the shared spawn path applies the returned additions without naming a family.
pub(crate) struct McpLaunchConfig {
    pub(crate) url: String,
    bearer: String,
    pub(crate) claude_config_path: Option<PathBuf>,
}

/// What one family's `mcp_launch` returns (S4 seam): opaque additions the shared
/// spawn path applies verbatim. No family names here — `owned_paths`/`owned_dirs`
/// are our files/dirs to revoke and sweep, `env_additions` never contain a secret
/// value's name confusion (the names `DEVBOULE_MCP_TOKEN`/`DEVBOULE_MCP_URL` carry
/// no secret bytes themselves), and argv additions never carry the token (S5/S6
/// assert argv token-free).
#[derive(Default)]
pub(crate) struct McpProviderConfig {
    pub(crate) env_additions: Vec<(String, String)>,
    pub(crate) arg_additions: Vec<String>,
    pub(crate) owned_paths: Vec<PathBuf>,
    pub(crate) owned_dirs: Vec<PathBuf>,
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

    /// The bearer for child **env** (`DEVBOULE_MCP_TOKEN` in S5/S6). Never argv
    /// (`ARCHITETTURA.md` §15.5 rule); S5/S6 own the argv token-free assertion
    /// and this step owns the helper it calls.
    pub(crate) fn bearer(&self) -> &str {
        &self.bearer
    }
}

#[derive(Clone)]
struct RegisteredSession {
    session_id: String,
    owner: OwnerId,
    /// Catalog provider this session was created for (`claude`, `grok`, …).
    /// The tool policy is keyed by it; `None` is a caller that had no
    /// provider id, which serves every broker tool (the pre-policy default).
    provider_id: Option<String>,
    /// How far this session is from a human root (`S5` decision 5): 0 for a
    /// session a person started, 1 for its child, 2 for a grandchild. Read from
    /// the registration and never from a request — the depth cap is a fact
    /// about who asked, and a caller field would be a claim.
    depth: u32,
    /// The tool overlay for this session (`S5` §2), applied on top of
    /// the provider's stored policy at `tools/list` and `tools/call`.
    overlay: crate::provider_catalog::ToolOverlay,
    bearer: String,
    claude_config_path: Option<PathBuf>,
    runtime: Option<Weak<crate::session::SessionRuntime>>,
    broker_ready: Arc<AtomicBool>,
}

/// The facts a registration may not read from a request (`S5` §3): how deep
/// this session is and what its birth overlay turns off.
///
/// [`AgentLineage::root`] is the human's: a session someone started at this
/// machine is depth 0 with every tool its provider offers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentLineage {
    pub(crate) depth: u32,
    pub(crate) overlay: crate::provider_catalog::ToolOverlay,
}

impl AgentLineage {
    pub(crate) const fn root() -> Self {
        Self {
            depth: 0,
            overlay: crate::provider_catalog::ToolOverlay::NONE,
        }
    }
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
        self.register_with_provider(session_id, owner, kind, None, AgentLineage::root())
    }

    /// Register `session_id` for `kind`, recording the catalog provider it
    /// belongs to so `tools/list` and `tools/call` can apply that provider's
    /// tool policy (`SessionCreate.provider`).
    ///
    /// The provider id is the sole key of that gate. A `None` id means "no
    /// policy consulted" — the pre-policy default — and is reachable in
    /// production only through the same-user development override
    /// `DEVBOULE_ACP_COMMAND` set without `DEVBOULE_ACP_PROVIDER_ID`: the
    /// command resolver reads the provider id from the environment and
    /// `session.rs` passes what it resolved. No policy row can name such a
    /// session, so the path cannot be widened from the app: `tool_policy.rs`
    /// refuses a `set` for any id the catalog publishes no tools for, and a
    /// lookup with no id yields `None`. A caller with a provider id therefore
    /// cannot lose its policy, and a caller without one cannot be given
    /// another session's.
    pub(crate) fn register_with_provider(
        self: &Arc<Self>,
        session_id: &str,
        owner: &OwnerId,
        kind: &SessionKind,
        provider_id: Option<&str>,
        lineage: AgentLineage,
    ) -> Result<Option<McpSessionGuard>, WireError> {
        if !hosts_mcp(kind) {
            // The honest surface (S2), now behind the single predicate (S9):
            // kinds without a carrier still take `Ok(None)` with the decision
            // named. Today that is only Terminal.
            eprintln!("{}", phase0_gate_log(session_id, kind, provider_id));
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
            session_id: session_id.to_string(),
            owner: owner.clone(),
            provider_id: provider_id.map(str::to_string),
            depth: lineage.depth,
            overlay: lineage.overlay,
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

    /// Whether the broker holds a registration (a minted bearer) for `session_id`.
    /// S8: surfaces report the registration FACT — a Codex/pi child with a minted
    /// carrier reads `Unverified` (establishing), one without reads `Unavailable`.
    pub(crate) fn is_registered(&self, session_id: &str) -> bool {
        self.sessions
            .lock()
            .ok()
            .is_some_and(|sessions| sessions.by_session.contains_key(session_id))
    }

    /// How deep the session behind this id is (`S5` decision 5). An id with no
    /// registration is depth 0: nothing an agent created.
    pub(crate) fn depth_of(&self, session_id: &str) -> u32 {
        self.sessions
            .lock()
            .ok()
            .and_then(|sessions| sessions.by_session.get(session_id).map(|row| row.depth))
            .unwrap_or(0)
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
            // S2 stores the S1 state on the runtime at spawn: registered but
            // not yet verified. The flip to Hosted lands in `mark_broker_ready`
            // below on the first authenticated `tools/list` (the ACP/Claude half
            // of the S8 flip; the pi/Codex half stays S8).
            runtime.set_tools_state(ToolsState::Unverified);
            Some(registration.broker_ready.load(Ordering::Acquire))
        });
        if self.stop.load(Ordering::Acquire) {
            runtime.fail_mcp("The MCP broker stopped before this session connected.");
        } else if broker_ready == Some(true) {
            runtime.mark_mcp_ready();
            runtime.set_tools_state(ToolsState::Hosted);
        }
    }

    fn mark_broker_ready(&self, registration: &RegisteredSession) {
        registration.broker_ready.store(true, Ordering::Release);
        if let Some(runtime) = registration.runtime.as_ref().and_then(Weak::upgrade) {
            runtime.mark_mcp_ready();
            runtime.set_tools_state(ToolsState::Hosted);
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

/// Who is calling through this bearer, resolved once per `tools/call` from the
/// registry row for `registration.session_id` — never from the loopback
/// connection, which is this machine's own by construction and lies about a
/// peer's child by design (see `session.rs::caller_origin`).
///
/// `Local` is the person at this machine's own agent: the door allows without
/// consulting the policy, so local behaviour and sentences are byte-identical.
/// `Peer` carries the device, the role it was paired as, and that device's
/// current capability set (fail-closed: a missing, unreadable or revoked row
/// holds nothing). `Unknown` is a stored origin the daemon cannot establish —
/// an `Unknown` row, or a peer-shaped row without a device or a role — and the
/// door refuses it hard: unlike absence it never resolves. `Absent` is no
/// readable row at all, and the door refuses it with the pre-existing retryable
/// absence sentence: an agent's first call can land before its own commit, and
/// a reaped session's in-flight calls outlive its row, and in both cases the
/// ecosystem already retries exactly that sentence. Absent is still a refusal —
/// on a consent surface the unknown never renders as the benign one — but it
/// is a transient refusal, not a verdict.
#[derive(Debug)]
enum McpCaller {
    Local,
    Peer {
        device_id: String,
        role: PeerRole,
        caps: Vec<String>,
    },
    Unknown,
    Absent,
}

fn resolve_mcp_caller(state: &ServerState, caller_session_id: &str) -> McpCaller {
    let Some(origin) = state.sessions.caller_origin(caller_session_id) else {
        return McpCaller::Absent;
    };
    match origin.kind {
        SessionOriginKind::Local => McpCaller::Local,
        SessionOriginKind::Peer => match (origin.device_id, origin.role) {
            (Some(device_id), Some(role)) => {
                let caps = state.peer_caps(&device_id);
                McpCaller::Peer {
                    device_id,
                    role,
                    caps,
                }
            }
            _ => McpCaller::Unknown,
        },
        SessionOriginKind::Unknown => McpCaller::Unknown,
    }
}

/// The tool door: judge what this call performs with the same `peer_allows`
/// function the wire dispatcher uses, on the wire equivalents the closed table
/// (`peer_policy::mcp_tool_wire`) names. The first `Deny` wins and nothing is
/// touched; the refusal carries the policy's own sentence, rendered as the wire
/// renders it.
///
/// Returns the reply to send when the call is refused before touching anything.
/// `None` means allowed (local callers always; peers whose device holds every
/// capability the tool's equivalents name; the explicitly unjudged list tool;
/// unknown tool names, which fall through to the broker's own `Unknown tool`
/// arm that touches nothing). Every `Some` is a refusal, never the benign
/// reading: the unknown-origin case hard, the absent-row case with the
/// retryable absence sentence.
fn mcp_peer_door(caller: &McpCaller, tool_name: Option<&str>, id: &Value) -> Option<Value> {
    let tool = tool_name?;
    match caller {
        McpCaller::Local => None,
        McpCaller::Peer { role, caps, .. } => {
            crate::peer_policy::mcp_tool_denial(*role, caps, tool).map(|reason| {
                rpc_error(
                    id.clone(),
                    -32601,
                    &crate::peer_policy::capability_refusal_message(reason),
                )
            })
        }
        McpCaller::Unknown => Some(rpc_error(
            id.clone(),
            -32601,
            "the calling session's origin is unknown; the call is refused",
        )),
        // No readable row: refuse, retryably, with the sentence every caller
        // already retries — the arms below used to answer absence themselves
        // (create with "No session with that id.", the roster with whatever it
        // could list), and the stub's retry loop recognises exactly this one.
        McpCaller::Absent => Some(rpc_error(id.clone(), -32601, "No session with that id.")),
    }
}

/// The connection a tool body acts through: the caller's own identity, not
/// this machine's. A door-allowed peer must have the act performed and judged
/// exactly as the same act over the wire would be — the delivery attributes
/// the message to the true origin (S4-05), the steer-refusal branch reads the
/// caller for its interrupt authority (S4-01), and the ownership checks apply
/// the peer's own scope (a `Daemon`-role device reaches only the sessions of
/// its own origin, §8 R2). A `Local` caller keeps the unmarked connection,
/// byte-identical to before. `Unknown`/`Absent` never reach a body — the door
/// refuses them — and keep it too.
///
/// `paired_by_user` and the transport binding come from the peer's row: the
/// ownership check for a `Client`-role device compares against the user that
/// ran the pairing, and the binding is the facts recorded at pairing time.
/// Dispatch reads no field of the binding — the handshake owned it — so a
/// row-sourced copy is the truthful thing to carry, not a fresh measurement.
fn caller_conn(state: &ServerState, caller: &McpCaller) -> Arc<crate::session::ConnHandle> {
    match caller {
        McpCaller::Peer {
            device_id,
            role,
            caps,
        } => {
            let record = state.peer_get(device_id).ok().flatten();
            let binding = crate::peer_policy::TransportBinding {
                kind: record
                    .as_ref()
                    .map(|record| record.binding_kind.clone())
                    .unwrap_or_default(),
                stable_id: record
                    .as_ref()
                    .and_then(|record| record.binding_stable_id.clone())
                    .unwrap_or_default(),
                node_name: record
                    .as_ref()
                    .and_then(|record| record.binding_node_name.clone())
                    .unwrap_or_default(),
                login_name: record
                    .as_ref()
                    .and_then(|record| record.binding_login_name.clone())
                    .unwrap_or_default(),
            };
            crate::session::ConnHandle::with_peer_caps(
                0,
                None,
                Some(crate::peer_policy::ConnPeer::Remote {
                    device_id: device_id.clone(),
                    role: *role,
                    paired_by_user: record.and_then(|record| record.paired_by_user),
                    binding,
                }),
                caps.clone(),
            )
        }
        McpCaller::Local | McpCaller::Unknown | McpCaller::Absent => {
            crate::session::ConnHandle::with_peer(0, None)
        }
    }
}

/// The audit identity for one tool call. A peer-origin caller names its device
/// and role, never `"local"`; an unestablishable origin names `"unknown"`,
/// never the benign one. Local callers keep exactly what they had: this
/// device's id with `"local"`.
fn audit_mcp_tool(
    state: &ServerState,
    caller: &McpCaller,
    action: &str,
    session_id: &str,
    outcome: &str,
) {
    let (device_id, role) = match caller {
        McpCaller::Local => match state.device_identity() {
            Ok(identity) => (identity.device_id.clone(), "local".to_string()),
            Err(_) => return,
        },
        McpCaller::Peer {
            device_id, role, ..
        } => (device_id.clone(), role.as_str().to_string()),
        // Who called cannot be established; the audit says so rather than the
        // benign thing. Both absences share the label: the refusal message the
        // caller saw already distinguishes the transient one.
        McpCaller::Unknown | McpCaller::Absent => match state.device_identity() {
            Ok(identity) => (identity.device_id.clone(), "unknown".to_string()),
            Err(_) => return,
        },
    };
    state.audit(AuditRecord {
        device_id,
        role,
        claimed_origin: None,
        action: action.to_string(),
        session_id: Some(session_id.to_string()),
        outcome: outcome.to_string(),
    });
}

fn handle_rpc(
    state: &Arc<ServerState>,
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
                    registration.overlay.clone(),
                )},
            })))
        }
        "tools/call" => {
            let tool_name = message.pointer("/params/name").and_then(Value::as_str);
            // The origin door runs before every other guard: who is calling is
            // resolved once from the registry row, and a peer is judged with the
            // same `peer_allows` the wire dispatcher uses before anything is
            // touched. Local callers pass through untouched.
            let caller = resolve_mcp_caller(state, &registration.session_id);
            if let Some(refusal) = mcp_peer_door(&caller, tool_name, &id) {
                if let Some(tool) = tool_name {
                    audit_mcp_tool(state, &caller, tool, &registration.session_id, "denied");
                }
                return Ok(Some(refusal));
            }
            // The policy guard runs before the name check, so a disabled tool
            // is refused for the reason that actually applies and an
            // unserved name cannot be probed past the policy.
            let policy = state.tool_policy.get(registration.provider_id.as_deref());
            if let Some(tool_name) = tool_name {
                if let Some(reason) =
                    tool_call_refusal(policy.as_ref(), &registration.overlay, tool_name)
                {
                    return Ok(Some(rpc_error(id, -32601, reason)));
                }
                // The overlay is folded into the same refusal, above: one
                // sentence for both rules (`S5` §2).
            }
            if tool_name == Some(crate::provider_catalog::MCP_SEND_MESSAGE_TOOL) {
                let to_agent = message
                    .pointer("/params/arguments/to_agent")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty());
                let text = message
                    .pointer("/params/arguments/text")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty());
                let (Some(to_agent), Some(text)) = (to_agent, text) else {
                    return Ok(Some(rpc_error(
                        id,
                        -32602,
                        "to_agent and text are required",
                    )));
                };
                let target = state
                    .sessions
                    .live_agent_entries(&registration.owner)
                    .map_err(|error| {
                        json!({"jsonrpc":"2.0", "id": id, "error": {"code": -32603, "message": error.message}})
                    })?
                    .into_iter()
                    .find(|entry| entry.session.id == to_agent || entry.session.title == to_agent);
                let Some(target) = target else {
                    return Ok(Some(rpc_error(id, -32602, "target agent not found")));
                };
                let internal_conn = caller_conn(state, &caller);
                match state.sessions.agent_message_send(
                    &registration.session_id,
                    &target.session.id,
                    text,
                    &registration.owner,
                    &internal_conn,
                ) {
                    Ok(()) => Ok(Some(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [{"type": "text", "text": "accepted"}],
                            "structuredContent": {"state": "accepted"},
                            "isError": false,
                        },
                    }))),
                    Err(error) => Ok(Some(json!({
                        "jsonrpc": "2.0",
                        "id": id,
                        "result": {
                            "content": [{"type": "text", "text": error.message}],
                            "isError": true,
                        },
                    }))),
                }
            } else if tool_name == Some(crate::provider_catalog::MCP_LIST_PROFILES_TOOL) {
                // Deliberately do not read params.arguments, like the roster
                // tool: the list is the human's, and the bearer is the only
                // identity this call needs.
                Ok(Some(list_profiles(&state.agent_profiles, &id)))
            } else if tool_name == Some(crate::provider_catalog::MCP_CREATE_AGENT_TOOL) {
                let arguments = message
                    .pointer("/params/arguments")
                    .cloned()
                    .unwrap_or(Value::Null);
                match AgentCreateRequest::parse(&arguments) {
                    Ok(request) => Ok(Some(create_agent(
                        state,
                        broker,
                        &caller,
                        registration,
                        &id,
                        request,
                    ))),
                    Err(message) => Ok(Some(rpc_error(id, -32602, &message))),
                }
            } else if tool_name == Some(crate::provider_catalog::MCP_ANSWER_PERMISSION_TOOL) {
                // Identity is the bearer, never the arguments: the card this
                // answers must belong to a child of the session that called.
                let card_id = message
                    .pointer("/params/arguments/cardId")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty());
                let outcome_str = message
                    .pointer("/params/arguments/outcome")
                    .and_then(Value::as_str);
                let (Some(card_id), Some(outcome_str)) = (card_id, outcome_str) else {
                    return Ok(Some(rpc_error(
                        id,
                        -32602,
                        "cardId and outcome are required",
                    )));
                };
                // The closed outcome table, enforced again at the door: an
                // agent's allow is one-shot or nothing.
                let outcome = match outcome_str {
                    "allow_once" => PermissionOutcome::AllowOnce,
                    "deny" => PermissionOutcome::Deny,
                    other => {
                        return Ok(Some(rpc_error(
                            id,
                            -32602,
                            &format!("unknown outcome {other:?}; use allow_once or deny"),
                        )));
                    }
                };
                let result = state.sessions.answer_child_permission(
                    &registration.session_id,
                    card_id,
                    outcome,
                    &|device_id| state.peer_caps(device_id),
                );
                // The audit names who called: the caller's device and role for a
                // peer (`resolve_mcp_caller` above), never `"local"` for one.
                let audit = |outcome_label: &str| {
                    audit_mcp_tool(
                        state,
                        &caller,
                        crate::provider_catalog::MCP_ANSWER_PERMISSION_TOOL,
                        &registration.session_id,
                        outcome_label,
                    );
                };
                match result {
                    Ok(()) => {
                        audit("ok");
                        Ok(Some(json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "content": [{"type": "text", "text": "answered"}],
                                "structuredContent": {"state": "answered", "cardId": card_id},
                                "isError": false,
                            },
                        })))
                    }
                    Err(sentence) => {
                        audit("denied");
                        Ok(Some(json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "content": [{"type": "text", "text": sentence}],
                                "isError": true,
                            },
                        })))
                    }
                }
            } else if tool_name == Some(crate::provider_catalog::MCP_SET_AGENT_PROFILE_TOOL) {
                // Identity is the bearer, never the arguments: the child this
                // moves must be a live child of the session that called, and
                // "mine" is something the daemon knows from the registration —
                // a caller id in the arguments would be a claim, not a fact.
                let session_arg = message
                    .pointer("/params/arguments/session")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty());
                let profile_arg = message
                    .pointer("/params/arguments/profile")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty());
                let (Some(session_arg), Some(profile_arg)) = (session_arg, profile_arg) else {
                    return Ok(Some(rpc_error(
                        id,
                        -32602,
                        "session and profile are required",
                    )));
                };
                let result = state.sessions.set_agent_child_profile(
                    &registration.session_id,
                    session_arg,
                    profile_arg,
                    &|requested| resolve_profile_for_move(&state.agent_profiles, requested),
                );
                // The audit names who called: the caller's device and role for a
                // peer (`resolve_mcp_caller` above), never `"local"` for one.
                let audit = |outcome_label: &str| {
                    audit_mcp_tool(
                        state,
                        &caller,
                        crate::provider_catalog::MCP_SET_AGENT_PROFILE_TOOL,
                        &registration.session_id,
                        outcome_label,
                    );
                };
                match result {
                    Ok(()) => {
                        audit("ok");
                        Ok(Some(json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "content": [{"type": "text", "text": "moved"}],
                                "structuredContent": {
                                    "state": "moved",
                                    "sessionId": session_arg,
                                    "profile": profile_arg,
                                },
                                "isError": false,
                            },
                        })))
                    }
                    Err(sentence) => {
                        audit("denied");
                        Ok(Some(json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "content": [{"type": "text", "text": sentence}],
                                "isError": true,
                            },
                        })))
                    }
                }
            } else if tool_name == Some(crate::provider_catalog::MCP_ACTIVITY_TOOL) {
                // Identity is the bearer; the argument names which of the
                // caller's own live agents to read, by id or display name.
                // A read like the roster: no new identity and no text, only
                // timing metadata (idle age, seqs, kind timestamps) the
                // roster does not show.
                let arguments = message
                    .pointer("/params/arguments")
                    .cloned()
                    .unwrap_or(Value::Null);
                let (session_arg, limit) = match parse_activity_arguments(&arguments) {
                    Ok(parsed) => parsed,
                    Err(message) => return Ok(Some(rpc_error(id, -32602, &message))),
                };
                let mut candidates = state
                    .sessions
                    .live_agent_entries(&registration.owner)
                    .map_err(|error| {
                        json!({"jsonrpc":"2.0", "id": id, "error": {"code": -32603, "message": error.message}})
                    })?
                    .into_iter()
                    .filter(|entry| {
                        entry.session.id == session_arg
                            || entry.session.title == session_arg
                            || entry
                                .session
                                .display_name
                                .as_deref()
                                .unwrap_or(&entry.session.title)
                                == session_arg
                    })
                    .map(|entry| entry.session.id)
                    .collect::<Vec<_>>();
                if candidates.len() > 1 {
                    return Ok(Some(rpc_error(
                        id,
                        -32602,
                        &format!(
                            "more than one of your live agents is called '{session_arg}'; use the session id"
                        ),
                    )));
                }
                let Some(target) = candidates.pop() else {
                    return Ok(Some(rpc_error(id, -32602, "target agent not found")));
                };
                match state
                    .sessions
                    .agent_activity(&target, &registration.owner, limit)
                {
                    Ok(document) => {
                        let text = serde_json::to_string(&document).map_err(|error| {
                            json!({"jsonrpc":"2.0", "id": id, "error": {"code": -32603, "message": format!("Could not encode agent activity: {error}")}})
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
                    Err(_) => Ok(Some(rpc_error(id, -32602, "target agent not found"))),
                }
            } else if tool_name == Some(crate::provider_catalog::MCP_STOP_AGENT_TOOL)
                || tool_name == Some(crate::provider_catalog::MCP_CLOSE_AGENT_TOOL)
            {
                // The destructive pair. Identity is the bearer, never an
                // argument: the sessions layer refuses everything that is
                // not the caller's own live child with one sentence that
                // does not say whether the id exists, and refuses the
                // caller itself outright. Both outcomes are audited, like
                // the profile move.
                let stopping = tool_name == Some(crate::provider_catalog::MCP_STOP_AGENT_TOOL);
                let session_arg = message
                    .pointer("/params/arguments/session")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty());
                let Some(session_arg) = session_arg else {
                    return Ok(Some(rpc_error(id, -32602, "session is required")));
                };
                let tool = if stopping {
                    crate::provider_catalog::MCP_STOP_AGENT_TOOL
                } else {
                    crate::provider_catalog::MCP_CLOSE_AGENT_TOOL
                };
                let audit = |outcome_label: &str| {
                    audit_mcp_tool(
                        state,
                        &caller,
                        tool,
                        &registration.session_id,
                        outcome_label,
                    );
                };
                let action = if stopping {
                    state
                        .sessions
                        .stop_agent_child(&registration.session_id, session_arg)
                } else {
                    state
                        .sessions
                        .close_agent_child(&registration.session_id, session_arg)
                        .map(|_| ())
                };
                match action {
                    Ok(()) => {
                        audit("ok");
                        let word = if stopping { "stopped" } else { "closed" };
                        Ok(Some(json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "content": [{"type": "text", "text": word}],
                                "structuredContent": {"state": word, "sessionId": session_arg},
                                "isError": false,
                            },
                        })))
                    }
                    Err(error) => {
                        audit("denied");
                        Ok(Some(json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "result": {
                                "content": [{"type": "text", "text": error.message}],
                                "isError": true,
                            },
                        })))
                    }
                }
            } else if tool_name == Some(crate::provider_catalog::MCP_LIST_DEVICES_TOOL) {
                // Deliberately do not read params.arguments, like the roster
                // tool: the calling session's own user scopes the list, and
                // the answer never leaves this process — no dial, ever.
                let document = match crate::mcp_device_roster::list_devices_document(
                    state,
                    &registration.owner,
                ) {
                    Ok(document) => document,
                    Err(message) => {
                        return Ok(Some(json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": {"code": -32603, "message": message},
                        })))
                    }
                };
                let text = serde_json::to_string(&document).map_err(|error| {
                    json!({"jsonrpc":"2.0", "id": id, "error": {"code": -32603, "message": format!("Could not encode device list: {error}")}})
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
            } else if tool_name == Some(crate::provider_catalog::MCP_LIST_PEER_AGENTS_TOOL) {
                // One dial, one device, named by argument; the calling
                // session's own rows decide which names are dialable.
                let device_id = message
                    .pointer("/params/arguments/deviceId")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty());
                let Some(device_id) = device_id else {
                    return Ok(Some(rpc_error(id, -32602, "deviceId is required")));
                };
                // This is the tool that dials other machines and comes back
                // with their roster, so every outcome is audited with the
                // caller, like the answer and move tools — and the failure
                // carries its cause (`denied`, `unscoped`, `failed`), because
                // a scope refusal is a different fact from a dead dial.
                let audit = |outcome_label: &str| {
                    audit_mcp_tool(
                        state,
                        &caller,
                        crate::provider_catalog::MCP_LIST_PEER_AGENTS_TOOL,
                        &registration.session_id,
                        outcome_label,
                    );
                };
                match crate::mcp_peer_agents::list_peer_agents(
                    state,
                    &registration.owner,
                    device_id,
                ) {
                    Ok(document) => {
                        audit("ok");
                        let text = serde_json::to_string(&document).map_err(|error| {
                            json!({"jsonrpc":"2.0", "id": id, "error": {"code": -32603, "message": format!("Could not encode peer agents: {error}")}})
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
                    Err(error) => {
                        audit(error.outcome);
                        Ok(Some(rpc_error(id, error.code, &error.sentence)))
                    }
                }
            } else if tool_name != Some(crate::provider_catalog::MCP_ROSTER_TOOL) {
                Ok(Some(rpc_error(id, -32601, "Unknown tool")))
            } else {
                // Deliberately do not read params.arguments. The bearer maps to
                // the caller; an agent id supplied by the model is not identity.
                let agents = state
                .sessions
                .live_agent_entries(&registration.owner)
                .map_err(|error| json!({"jsonrpc":"2.0", "id": id, "error": {"code": -32603, "message": error.message}}))?
                .into_iter()
                .map(|entry| {
                    agent_value(
                        &entry.session,
                        &entry.runtime,
                        broker.depth_of(&entry.session.id),
                    )
                })
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
        }
        _ if message.get("id").is_none() => Ok(None),
        _ => Ok(Some(rpc_error(id, -32601, "Method not found"))),
    }
}

/// The result of one creation, whether it just happened or is being re-answered
/// (`S5` §2; `create-from-profile`):
/// `{sessionId, taskId, contextId, displayName, state: "submitted"}`.
///
/// `taskId` is the session id: a Devboule child *is* the task, and a caller that
/// had to keep a map of task to session would be keeping a private copy of a
/// fact the daemon already has. `contextId` is the child's context, which is the
/// creator's context — so a creator and everything it commissions, at any depth,
/// name one family without any bookkeeping of their own. The fallback is the rule
/// `Session::context_id` states (a session with no creator is its own context),
/// applied for a client that reads a frame from a daemon older than this field.
fn created_result(id: &Value, session: &devboule_protocol::Session, registered: bool) -> Value {
    // S2 honesty, S8 fact: the result reports verification, the card promised
    // it. `registered` is the broker row — a fresh registered child is not yet
    // verified (its first proof lands after this answer); an unregistered one
    // has no tools at all. Hosted is renderable via `created_result_for_tools`
    // (pinned by test) and arrives on live paths when verification flips the
    // runtime. Routed through the S1 single computation point.
    let tools = compute_tools_state(&session.kind, registered, false);
    created_result_for_tools(id, session, tools)
}

/// The result body for one explicit tools state (S2 test hook): the `tools`
/// word rides `structuredContent`, and `unavailable`/`unverified` add the one
/// model-readable sentence. `hosted` adds none — the tools themselves are the
/// proof. The forbidden state is a result claiming `hosted` for a session with
/// no bearer; the test pins it by calling this with `Hosted` for a kind that
/// `created_result` would never produce.
fn created_result_for_tools(
    id: &Value,
    session: &devboule_protocol::Session,
    tools: ToolsState,
) -> Value {
    let tools_sentence = match tools {
        ToolsState::Hosted => "",
        ToolsState::Unavailable => {
            " This session starts without Devboule tools: it cannot create, message or list agents."
        }
        ToolsState::Unverified => {
            " This session's tools are unverified: they will be verified at start."
        }
    };
    let display_name = session
        .display_name
        .clone()
        .unwrap_or_else(|| session.title.clone());
    let context_id = session
        .context_id
        .clone()
        .unwrap_or_else(|| session.id.clone());
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{"type": "text", "text": format!("submitted {}{}", session.id, tools_sentence)}],
            "structuredContent": {
                "sessionId": session.id,
                "taskId": session.id,
                "contextId": context_id,
                "displayName": display_name,
                "state": devboule_protocol::AgentTaskState::Submitted.as_str(),
                "tools": tools.as_str(),
            },
            "isError": false,
        },
    })
}

fn rpc_error(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

/// The `tools/list` body for one policy: the catalog minus the tools that
/// policy disables. The catalog is a parameter so the filter can be tested
/// against a tool other than the always-on roster tool.
fn enabled_tool_list(
    catalog: &[(&str, &str)],
    policy: Option<&ToolPolicyEntry>,
    overlay: crate::provider_catalog::ToolOverlay,
) -> Vec<Value> {
    catalog
        .iter()
        .filter(|(name, _)| crate::tool_policy::is_tool_enabled(policy, name))
        // The preset's overlay, on top of the stored policy and never instead
        // of it (`S5` §2): a `design` child sees neither the tool it may not
        // call nor any tool its provider's policy already turned off.
        .filter(|(name, _)| overlay.allows(name))
        .map(|(name, description)| {
            let input_schema = if *name == crate::provider_catalog::MCP_SEND_MESSAGE_TOOL {
                json!({
                    "type": "object",
                    "properties": {
                        "to_agent": {"type": "string"},
                        "text": {"type": "string"},
                    },
                    "required": ["to_agent", "text"],
                    "additionalProperties": false,
                })
            } else if *name == crate::provider_catalog::MCP_CREATE_AGENT_TOOL {
                crate::provider_catalog::agent_create_input_schema()
            } else if *name == crate::provider_catalog::MCP_ANSWER_PERMISSION_TOOL {
                // The outcome enum is closed at the schema too: an agent is
                // offered allow_once or deny, and nothing else. There is no
                // allow_always from an agent, ever — the protocol's
                // PermissionOutcome has no such variant, and this table does
                // not name one.
                json!({
                    "type": "object",
                    "properties": {
                        "cardId": {"type": "string"},
                        "outcome": {"type": "string", "enum": ["allow_once", "deny"]},
                    },
                    "required": ["cardId", "outcome"],
                    "additionalProperties": false,
                })
            } else if *name == crate::provider_catalog::MCP_SET_AGENT_PROFILE_TOOL {
                // Closed, like the create schema it rhymes with: the child is
                // named by id or display name, the profile by its name, and
                // nothing a caller could state as identity is offered at all.
                crate::provider_catalog::agent_set_profile_input_schema()
            } else if *name == crate::provider_catalog::MCP_LIST_PEER_AGENTS_TOOL {
                // Closed like its siblings: the device is named by the id
                // `devboule_list_devices` answered, and there is deliberately
                // no scope argument — whose roster answers is the responder's
                // own pairing-user fact.
                crate::provider_catalog::peer_agents_input_schema()
            } else if *name == crate::provider_catalog::MCP_LIST_DEVICES_TOOL {
                // Spelled in its own arm rather than left to the default arm
                // at the bottom: a parameterless tool's schema is a claim
                // about the tool, and nothing walks this table to keep a
                // silent default true.
                json!({"type": "object", "properties": {}, "additionalProperties": false})
            } else if *name == crate::provider_catalog::MCP_ACTIVITY_TOOL {
                crate::provider_catalog::agent_activity_input_schema()
            } else if *name == crate::provider_catalog::MCP_STOP_AGENT_TOOL
                || *name == crate::provider_catalog::MCP_CLOSE_AGENT_TOOL
            {
                // One closed document for both verbs: they differ in what
                // they do, not in what they accept.
                crate::provider_catalog::agent_end_input_schema()
            } else {
                json!({"type": "object", "properties": {}, "additionalProperties": false})
            };
            json!({
                "name": name,
                "description": description,
                "inputSchema": input_schema,
            })
        })
        .collect()
}

fn agent_value(
    session: &devboule_protocol::Session,
    runtime: &crate::session::SessionRuntime,
    depth: u32,
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
    // `name` is the display name a created agent was given; a session a person
    // started has none, and the row still needs a name a caller can address, so
    // it falls back to the same title the app renders (`S5` §1). `state` is the
    // A2A word, with a pending card outranking "working": a child parked on a
    // human's answer is the one fact a creator most needs to see.
    let name = session
        .display_name
        .clone()
        .unwrap_or_else(|| session.title.clone());
    let state = crate::session::roster_task_state(session, runtime);
    // S2: every roster entry carries the S1 word, read off the runtime the
    // broker stored at spawn and flipped on verification. S9 lists all agent
    // kinds; the card and result carry the promise for children too young to
    // have verified.
    json!({
        "id": session.id,
        "provider": session.provider.clone().or(manifest_provider),
        "model": model,
        "state": state,
        "name": name,
        "title": session.title,
        "createdBy": session.created_by,
        "depth": depth,
        "tools": runtime.tools_state().as_str(),
    })
}

/// One validated `devboule_agent_activity` call: the child to read plus the
/// bounded recent-lines limit. The known-parameter check is read out of the
/// published schema, so the document and the check cannot disagree.
fn parse_activity_arguments(arguments: &Value) -> Result<(String, usize), String> {
    let empty = json!({});
    let arguments = match arguments {
        Value::Null => &empty,
        Value::Object(_) => arguments,
        _ => return Err("arguments must be an object".to_string()),
    };
    let object = arguments
        .as_object()
        .ok_or_else(|| "arguments must be an object".to_string())?;
    let known: Vec<String> = crate::provider_catalog::agent_activity_input_schema()["properties"]
        .as_object()
        .map(|properties| properties.keys().cloned().collect())
        .unwrap_or_default();
    for key in object.keys() {
        if !known.iter().any(|known| known == key) {
            return Err(format!("unknown parameter '{key}'"));
        }
    }
    let session = object
        .get("session")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "session is required".to_string())?;
    let limit = match object.get("limit") {
        None | Some(Value::Null) => crate::agent_activity::clamp_limit(None),
        Some(Value::Number(n)) => {
            let n = n
                .as_u64()
                .ok_or_else(|| "limit must be an integer 0..50".to_string())?;
            crate::agent_activity::clamp_limit(Some(n))
        }
        Some(_) => return Err("limit must be an integer 0..50".to_string()),
    };
    Ok((session.to_string(), limit))
}

/// One validated `devboule_create_agent` call (`S5` §2, `create-from-profile`).
///
/// The parameters arrive as an MCP `arguments` object; the schema the broker
/// publishes is closed, and this is the enforcement half of it. Every sentence
/// this produces is one of §2's, and the check that a parameter is *known* is
/// read out of the published schema rather than repeated here, so the document
/// an agent sees and the check it hits cannot disagree.
///
/// `profile` is a **name**, and it is the only way this request says what to
/// run: the provider, the model, the mode, the thinking option, the features
/// and the tool overlay all come from the stored profile that name resolves to
/// at the moment of the call. There is no field here that could disagree with
/// it.
#[derive(Debug)]
struct AgentCreateRequest {
    profile: String,
    title: String,
    /// The caller's own labels. The daemon's `devboule.` keys are stamped into
    /// the same map at the creation and are refused here.
    labels: std::collections::BTreeMap<String, String>,
    workspace_id: Option<String>,
    cwd: Option<String>,
    initial_prompt: String,
    notify: bool,
}

/// The prefix the daemon reserves for its own label facts.
const RESERVED_LABEL_PREFIX: &str = "devboule.";

/// The most labels one creation may carry.
///
/// Bounded because the map is written into the session row and travels on every
/// roster push: an unbounded map is a payload every attached client pays for on
/// every push, for an annotation nothing decides anything from.
const MAX_AGENT_LABELS: usize = 32;

/// The longest label key and value, in bytes.
const MAX_LABEL_KEY_BYTES: usize = 64;
const MAX_LABEL_VALUE_BYTES: usize = 256;

impl AgentCreateRequest {
    /// The fields one creation's identity is made of (`S5-08`).
    ///
    /// The list lives in a function rather than inline in the handler, so the
    /// test drives this very list instead of a copy of it.
    fn creation_fingerprint_fields<'a>(
        creator_id: &'a str,
        request: &'a Self,
        notify_field: &'a str,
        labels: &'a str,
    ) -> Vec<&'a str> {
        vec![
            creator_id,
            &request.title,
            &request.profile,
            &request.initial_prompt,
            request.workspace_id.as_deref().unwrap_or(""),
            request.cwd.as_deref().unwrap_or(""),
            notify_field,
            labels,
        ]
    }

    /// The labels as one fingerprint field: their JSON encoding, which is
    /// deterministic for a `BTreeMap`, so two different label sets cannot spell
    /// the same string and the same set always spells the same one.
    fn labels_fingerprint(&self) -> String {
        serde_json::to_string(&self.labels).unwrap_or_default()
    }

    fn parse(arguments: &Value) -> Result<Self, String> {
        let empty = json!({});
        let arguments = match arguments {
            Value::Null => &empty,
            Value::Object(_) => arguments,
            _ => return Err("arguments must be an object".to_string()),
        };
        let object = arguments
            .as_object()
            .ok_or_else(|| "arguments must be an object".to_string())?;
        let known: Vec<String> = crate::provider_catalog::agent_create_input_schema()["properties"]
            .as_object()
            .map(|properties| properties.keys().cloned().collect())
            .unwrap_or_default();
        for key in object.keys() {
            if !known.iter().any(|known| known == key) {
                // `provider`, `preset` and `mode` land here on purpose: this
                // tool has none of them, because a profile chooses what to run
                // (`create-from-profile`).
                return Err(format!("unknown parameter '{key}'"));
            }
        }
        let text = |key: &str| object.get(key).and_then(Value::as_str);
        let required = |key: &str| {
            text(key)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .ok_or_else(|| format!("{key} is required"))
        };
        let title = devboule_protocol::validate_display_name(&required("title")?)?;
        // The profile name is compared against the store's own names, which
        // `agent_profiles.rs` trims on the way in, so a caller that padded its
        // name is naming the same profile rather than a profile that cannot
        // exist. It is deliberately **not** bounded here: a name that matches
        // nothing is refused with the store's sentence, and a name that is
        // longer than the store admits can never match one.
        let profile = required("profile")?;
        let initial_prompt = required("initialPrompt")?;
        // A prompt of spaces is a prompt nobody can act on, and the same rule
        // the display name gets: whitespace is not content.
        let initial_prompt = initial_prompt.trim().to_string();
        if initial_prompt.is_empty() {
            return Err("initialPrompt is required".to_string());
        }
        if initial_prompt.len() > MAX_AGENT_PROMPT_BYTES {
            return Err(format!(
                "initialPrompt is {} bytes; the limit is {MAX_AGENT_PROMPT_BYTES}.",
                initial_prompt.len()
            ));
        }
        let notify = match object.get("notifyOnFinish") {
            None => true,
            Some(Value::Bool(value)) => *value,
            Some(_) => return Err("notifyOnFinish must be a boolean".to_string()),
        };
        Ok(Self {
            profile,
            title,
            labels: parse_labels(object)?,
            workspace_id: optional_text("workspaceId", object)?,
            cwd: optional_text("cwd", object)?,
            initial_prompt,
            notify,
        })
    }
}

/// The caller's labels, checked (`create-from-profile`).
///
/// A free map of string to string, with one reserved prefix: the daemon stamps
/// `devboule.created-by`, `devboule.depth`, `devboule.origin` and
/// `devboule.profile` itself, so a caller that sets or overwrites any of them is
/// refused rather than silently overridden. A caller cannot even arrive at one
/// by accident, because a label nothing can be decided from still has to be
/// honest about who wrote it.
fn parse_labels(
    object: &serde_json::Map<String, Value>,
) -> Result<std::collections::BTreeMap<String, String>, String> {
    let Some(value) = object.get("labels") else {
        return Ok(std::collections::BTreeMap::new());
    };
    let Value::Object(entries) = value else {
        return Err("labels must be an object of strings".to_string());
    };
    if entries.len() > MAX_AGENT_LABELS {
        return Err(format!(
            "labels carries {} entries; the limit is {MAX_AGENT_LABELS}.",
            entries.len()
        ));
    }
    let mut labels = std::collections::BTreeMap::new();
    for (key, value) in entries {
        if key.starts_with(RESERVED_LABEL_PREFIX) {
            return Err("reserved label prefix".to_string());
        }
        if key.trim().is_empty() {
            return Err("a label has an empty key".to_string());
        }
        if key.len() > MAX_LABEL_KEY_BYTES {
            return Err(format!(
                "the label key '{key}' is {} bytes; the limit is {MAX_LABEL_KEY_BYTES}.",
                key.len()
            ));
        }
        let Value::String(value) = value else {
            return Err(format!("the label '{key}' must be a string"));
        };
        if value.len() > MAX_LABEL_VALUE_BYTES {
            return Err(format!(
                "the label '{key}' is {} bytes; the limit is {MAX_LABEL_VALUE_BYTES}.",
                value.len()
            ));
        }
        labels.insert(key.clone(), value.clone());
    }
    Ok(labels)
}

/// The name a profile's labels and the creation record are keyed on: the
/// catalog's own spelling of the name, trimmed exactly as the store trims it.
fn profile_name_key(name: &str) -> &str {
    name.trim()
}

/// One optional string parameter, with its type enforced (audit S5-09).
///
/// A `workspaceId` of `null` is absent; a number, a list or an object is an
/// invalid-params error rather than a silently ignored parameter — the caller
/// asked for something and must be told it was not understood, especially when
/// what it asked for was *where* the child would run.
fn optional_text(
    key: &str,
    object: &serde_json::Map<String, Value>,
) -> Result<Option<String>, String> {
    match object.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.clone())),
        Some(_) => Err(format!("{key} must be a string")),
    }
}

/// Everything one creation's answer depends on, in one string (audit S5-08).
///
/// Every field is length-prefixed, so two different payloads cannot spell the
/// same fingerprint by moving a delimiter: today's fields include the requested
/// workspace, the working directory and whether the creator wants to be told,
/// because a retry that changes any of them is not the same creation.
fn creation_fingerprint(fields: &[&str]) -> String {
    let mut fingerprint = String::from("agent-create");
    for field in fields {
        fingerprint.push(':');
        fingerprint.push_str(&field.len().to_string());
        fingerprint.push(':');
        fingerprint.push_str(field);
    }
    fingerprint
}

/// The largest `initialPrompt` one creation may carry (32 KiB).
///
/// The same number as the artifact cap, for the same reason: this text becomes
/// a child's first turn, and a prompt nobody could read is not a prompt.
const MAX_AGENT_PROMPT_BYTES: usize = 32 * 1024;

/// The labels the daemon stamps into every child it creates.
///
/// The four keys are the daemon's own facts about the child, and they are
/// stamped here, where all four are known, rather than by the session path: the
/// creation is what measured them. `devboule.profile` is the profile's **stable
/// id**, like the session's own field, so the label and the row cannot disagree
/// about which profile made this child, and both survive a rename.
fn stamped_labels(
    caller: &std::collections::BTreeMap<String, String>,
    creator_session_id: &str,
    profile: &ResolvedProfile,
    depth: u32,
    origin: &devboule_protocol::SessionOrigin,
) -> std::collections::BTreeMap<String, String> {
    let mut labels = caller.clone();
    labels.insert(
        "devboule.created-by".to_string(),
        creator_session_id.to_string(),
    );
    labels.insert("devboule.depth".to_string(), depth.to_string());
    labels.insert("devboule.origin".to_string(), origin_label(origin));
    labels.insert("devboule.profile".to_string(), profile.id.clone());
    labels
}

/// `devboule.origin`'s value: the same word the wire uses for the kind, plus the
/// device for a peer's child.
///
/// A label is text a human reads, so a peer's device id is spelled into it
/// rather than left to a second lookup the label has no way to make.
fn origin_label(origin: &devboule_protocol::SessionOrigin) -> String {
    use devboule_protocol::SessionOriginKind;
    match origin.kind {
        SessionOriginKind::Peer => match origin.device_id.as_deref() {
            Some(device) => format!("peer:{device}"),
            None => "peer".to_string(),
        },
        SessionOriginKind::Local => "local".to_string(),
        SessionOriginKind::Unknown => "unknown".to_string(),
    }
}

/// One profile, resolved for one creation: what the store said at the moment of
/// the call and nothing that was cached.
#[derive(Debug)]
struct ResolvedProfile {
    /// The profile's identity, which is what the session records.
    id: String,
    /// The name the human ticked, which is what the card and the creator's
    /// transcript show.
    name: String,
    provider: String,
    /// The provider's own model id, exactly as saved. Carried because the card
    /// states what the human is being asked to approve, and because nothing may
    /// substitute it.
    model: String,
    mode: String,
    /// The provider's thinking option, exactly as saved.
    thinking_option_id: Option<String>,
    /// The provider's feature values, exactly as saved.
    features: serde_json::Map<String, Value>,
    overlay: crate::provider_catalog::ToolOverlay,
}

/// Resolve the profile a creation named, out of the profiles the human ticked.
///
/// Every refusal here is one of §2's sentences, and they are in the order that
/// keeps the answers honest:
///
/// 1. **The list is read now.** `AgentProfilesStore::document` is asked on every
///    call and nothing is cached per session, so a profile the human enabled or
///    un-ticked while an agent was reading `devboule_list_profiles` is answered
///    by the list as it stands when the creation is attempted.
/// 2. **No ticked profile at all** is refused before the requested name is even
///    looked at, and the sentence names **no** profile. That is not politeness:
///    a refusal that said "the profile *X* exists but is not enabled" would tell
///    a caller what it is not allowed to see, and the list an agent reads is the
///    enabled set and nothing else.
/// 3. **A name that is unknown or unticked** gets the sentence that sends the
///    caller to the list, which is where the answer is.
/// 4. **A name two ticked profiles share** is refused rather than resolved.
///    `agent_profiles.rs` allows two profiles to share a name (the id is the
///    identity), so "the first one" would be picking a provider the human did
///    not name.
fn resolve_profile(
    store: &crate::agent_profiles::AgentProfilesStore,
    requested: &str,
) -> Result<ResolvedProfile, String> {
    let document = store.document();
    let enabled: Vec<&devboule_protocol::AgentProfile> = document
        .profiles
        .iter()
        .filter(|profile| profile.enabled_for_agents)
        .collect();
    if enabled.is_empty() {
        return Err("no profile is enabled for agents".to_string());
    }
    let wanted = profile_name_key(requested);
    let matching: Vec<&devboule_protocol::AgentProfile> = enabled
        .into_iter()
        .filter(|profile| profile.name == wanted)
        .collect();
    let profile = match matching.as_slice() {
        [] => return Err("unknown profile; call devboule_list_profiles".to_string()),
        [one] => *one,
        many => return Err(format!("more than one profile is called {}", many[0].name)),
    };
    Ok(ResolvedProfile {
        id: profile.id.clone(),
        name: profile.name.clone(),
        provider: profile.provider.clone(),
        model: profile.model.clone(),
        mode: profile.mode_id.clone(),
        thinking_option_id: profile.thinking_option_id.clone(),
        features: profile.features.clone(),
        // The profile's own deny list, applied on top of the provider's stored
        // policy — the same two places a preset's overlay was applied. The store
        // has already refused a name outside the broker's table, so this can
        // only ever remove a tool the broker serves.
        overlay: crate::provider_catalog::ToolOverlay::from_profile_names(&profile.tool_overlay),
    })
}

/// The move tool's profile resolution (slice 5b §2 check 3, Pass A): the one
/// resolver, [`resolve_profile`], plus the third refusal §1.2 demands.
///
/// `resolve_profile` is the create surface's resolver and deliberately
/// conflates unticked with unknown — the list an agent reads is the ticked set
/// and nothing else, so the sentence that sends the caller back to the list is
/// the honest one there. The move surface is bound to the three-state
/// discipline instead: a name the store holds but the human has not ticked is
/// its own refusal, distinct from a name nobody ever wrote. Only the sentence
/// is refined: the matching, the ambiguity refusal and the read-now rule all
/// stay `resolve_profile`'s, so the two surfaces cannot drift into a second
/// resolver with a second ambiguity answer.
fn resolve_profile_for_move(
    store: &crate::agent_profiles::AgentProfilesStore,
    requested: &str,
) -> Result<crate::session::ChildProfileFacts, String> {
    match resolve_profile(store, requested) {
        Ok(profile) => Ok(crate::session::ChildProfileFacts {
            profile_id: profile.id,
            mode_id: profile.mode,
            model: profile.model,
            thinking_option_id: profile.thinking_option_id,
        }),
        Err(message) => {
            let wanted = profile_name_key(requested);
            let unticked = store
                .document()
                .profiles
                .iter()
                .any(|profile| profile.name == wanted && !profile.enabled_for_agents);
            if unticked {
                Err(format!(
                    "the profile '{wanted}' exists but the human has not enabled it for agents; only a ticked profile can be moved onto"
                ))
            } else {
                Err(message)
            }
        }
    }
}

/// The per-profile **prediction** the list and the card serve (F6): the same
/// tri-state the child's birth will derive, judged before the child exists
/// from the profile's mode and the family that mode would be delivered in.
/// No session exists yet, so nothing here is observed — it is the delivery's
/// own dictionary answering for the mode the profile names.
fn predicted_unattended(provider: &str, mode_id: &str) -> devboule_protocol::UnattendedState {
    crate::peer_policy::unattended_mode(
        crate::provider_catalog::session_kind_for(provider),
        Some(mode_id),
    )
}

/// One `devboule_list_profiles` call (`create-from-profile`).
///
/// The ticked profiles, in the human's stored order and never sorted, as
/// `{name, note, provider, model, mode, unattended}`, the last a tri-state
/// **prediction** (`"yes" | "no" | "unknown"`) whose meaning the tool's
/// description spells out for the caller. Nothing else is served:
/// not the id (a caller names a profile by its name, and the id is the daemon's
/// key for the session it records), not a profile the human did not tick, and
/// not the standing instructions — those are not a profile's business to read.
///
/// `note` is verbatim and never truncated. It is the only thing a model has to
/// route work with, so a truncated note is a different instruction, not a
/// shorter display of the same one.
fn list_profiles(store: &crate::agent_profiles::AgentProfilesStore, id: &Value) -> Value {
    let document = store.document();
    let profiles: Vec<Value> = document
        .profiles
        .iter()
        .filter(|profile| profile.enabled_for_agents)
        .map(|profile| {
            json!({
                "name": profile.name,
                "note": profile.note,
                "provider": profile.provider,
                "model": profile.model,
                "mode": profile.mode_id,
                // The prediction, not a promise: the same tri-state the
                // child's birth will derive, judged from the mode and the
                // family that mode would be delivered in. `unknown` is a
                // value here — an agent picking a profile *because it will
                // not ask* must get `unknown`, never a false `no`.
                "unattended": predicted_unattended(&profile.provider, &profile.mode_id),
            })
        })
        .collect();
    let document = json!({ "profiles": profiles });
    let text = serde_json::to_string(&document)
        .unwrap_or_else(|error| format!("Could not encode the profile list: {error}"));
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{"type": "text", "text": text}],
            "structuredContent": document,
            "isError": false,
        },
    })
}

/// The `devboule_create_agent` tool (`S5` §2 and §3; `create-from-profile`).
///
/// The caller is the session whose Bearer authenticated the connection: the
/// `registration` is the only identity this function uses, and there is no
/// `from_session` parameter to lie about.
///
/// The order is the checklist's: resolve the profile the caller named from the
/// profiles the human **ticked, read now** (the provider, the model, the mode,
/// the features and the tool overlay come from there and never from the
/// caller), reserve the budget, raise the creation card once per creator
/// session, create through the `SessionCreate` path with the creator's own
/// origin and owner, and answer
/// `{sessionId, taskId, contextId, displayName, state: "submitted"}`.
fn create_agent(
    state: &Arc<ServerState>,
    _broker: &McpBroker,
    caller: &McpCaller,
    registration: &RegisteredSession,
    id: &Value,
    request: AgentCreateRequest,
) -> Value {
    let creator_id = registration.session_id.clone();
    // The retry identity, and the payload it must match (`S5` block 7, audit
    // S5-03 and S5-08).
    //
    // An MCP `tools/call` has no idempotency parameter — §2's schema is closed
    // and defines none — so the only identity a *retry* has is the frame's own
    // id, which a client reuses when it re-sends a request whose answer it lost.
    // The fingerprint is everything the answer depends on, so a key reused with
    // a different payload is a conflict, not a retry.
    //
    // The key is held *before* the store is read: a second call that arrives
    // while this one is still raising a card is in flight, not a retry, and is
    // refused without spending a slot. Every refusal below releases it through
    // the hold's own scope.
    //
    // The store is consulted **before the profile is resolved**: a retry
    // arrives after the first attempt's answer was lost, and in that window the
    // human may have renamed or un-ticked the profile the first attempt ran
    // under — the child it created is alive either way. Refusing the retry at
    // the profile check would tell the creator its creation failed, and a
    // second call would spend a second slot on a child it already has. A
    // *new* call — a different frame id — has no remembered answer and still
    // meets the profile check below, with the store exactly as it stands now.
    let retry_key = crate::server::creation_retry_key(&creator_id, id);
    let mut hold = match retry_key.as_deref() {
        Some(key) => match state.sessions.hold_creation_key(key) {
            Ok(hold) => Some(hold),
            Err(error) => return tool_error(id, &error.message),
        },
        None => None,
    };
    let notify_field = if request.notify { "notify" } else { "quiet" };
    let labels_field = request.labels_fingerprint();
    let fingerprint = creation_fingerprint(&AgentCreateRequest::creation_fingerprint_fields(
        &creator_id,
        &request,
        notify_field,
        &labels_field,
    ));
    if let Some(key) = retry_key.as_deref() {
        if let Some(existing) = crate::server::idempotent_creation_session(
            state,
            &registration.owner,
            key,
            &fingerprint,
        ) {
            // A retry answers the first call's session and creates nothing:
            // no second card, no second slot, no second child.
            if let Some(hold) = hold.as_mut() {
                hold.commit();
            }
            return created_result(id, &existing, state.mcp.is_registered(&existing.id));
        }
    }
    let profile = match resolve_profile(&state.agent_profiles, &request.profile) {
        Ok(profile) => profile,
        Err(message) => return tool_error(id, &message),
    };
    let creator = match state
        .sessions
        .agent_creator(&creator_id, &registration.owner)
    {
        Ok(creator) => creator,
        Err(error) => return tool_error(id, &error.message),
    };
    // Where the child runs, before anything is spent on it (audit S5-05): a
    // workspace is either the caller's own or the call is refused, and the
    // working directory must stay inside it. The card then states the directory
    // the child will really get.
    let workspace_id = match request.workspace_id.as_deref() {
        Some(requested) if Some(requested) != creator.workspace_id.as_deref() => {
            return tool_error(id, "workspace must be the caller's");
        }
        _ => creator.workspace_id.clone(),
    };
    let cwd = match state
        .sessions
        .resolve_child_cwd(workspace_id.as_deref(), request.cwd.as_deref())
    {
        Ok(cwd) => cwd,
        Err(error) => return tool_error(id, &error.message),
    };
    // The depth comes from the registration, never from the request
    // (`S5` checklist): a session at depth 2 may not create, whatever it says.
    let depth = registration.depth.saturating_add(1);
    if depth > crate::session::MAX_AGENT_DEPTH {
        return tool_error(id, "depth limit; do not retry");
    }
    // The device that owns the creator must still be allowed to create
    // sessions: a child of a peer's session is a session on that peer's device,
    // so the gate the peer already passed for its own `SessionCreate` is the
    // gate its child passes here (`S5` §3, "closed set"). A revoked or
    // capability-stripped device fails closed.
    if !creator.may_create_sessions(state) {
        return tool_error(id, "not allowed for this peer");
    }
    // A provider this daemon cannot launch is refused before a session id, a
    // card or a slot is spent on it (`S5` §2). The provider is the profile's:
    // a creation cannot name one, so this is the only provider that can be
    // missing, and the sentence says what it is about.
    if !provider_is_launchable(&profile.provider) {
        return tool_error(id, "provider not installed");
    }
    // The contradiction the profile alone decides is decided **here**, before
    // the reservation and before the card: a tick over a mode that asks the
    // human is refused without spending the human's consent on a creation
    // the daemon had already decided to refuse, and without the card reading
    // "auto accept: Yes (mode ask)" for exactly that configuration (the R2a
    // audit's F7).
    //
    // The refusal is bounded by **authorship** (the re-audit's P1): this gate
    // concludes only where the daemon owns the rule. Claude and Pi's tick
    // rule is the daemon's own — start in a mode the broker answers — and
    // the profile's mode is the delivered mode for both. Codex's knob
    // (`full-access`) and every ACP agent's modes are the family's own
    // vocabulary, so the daemon refuses nothing there: the client re-judges
    // at spawn time, where the delivered mode is the fact. The old shape
    // judged every provider from the shared table alone and refused a Codex
    // `full-access` profile its own client accepts.
    if matches!(
        crate::provider_catalog::judge_auto_accept_tick(
            &profile.provider,
            &profile.mode,
            &profile.features
        ),
        crate::provider_catalog::AutoAcceptTick::Contradicts
    ) {
        return tool_error(
            id,
            &format!(
                "the profile asks the agent to approve its own permission prompts and also to start in mode '{}', which asks the human; the two contradict, so the creation is refused",
                profile.mode
            ),
        );
    }
    // The child's labels, stamped here where all four facts are known. Stamped
    // into the same map the caller wrote, so a human reads one list; refused if
    // the caller tried to write one of them (`parse_labels`), so the daemon's
    // facts are the daemon's.
    let labels = stamped_labels(
        &request.labels,
        &creator_id,
        &profile,
        depth,
        &creator.origin,
    );
    // Read before `creator` moves into the creation below.
    let context_id = creator.context_id.clone();
    let ticket = match state.sessions.reserve_agent_creation(&creator_id, depth) {
        Ok(ticket) => ticket,
        Err(error) => return tool_error(id, &error.message),
    };
    if ticket.card_owed() {
        // The card is raised on the creator's own session, through the same
        // broker entry every other card uses: the same decision frame answers
        // it, the same per-device budget bounds a peer's, and a refusal leaves
        // the gate shut (`S5` decision 4).
        if state
            .sessions
            .live_runtime(&creator_id, &registration.owner)
            .and_then(|runtime| runtime.permission_broker())
            .is_none()
        {
            return tool_error(id, "permission refused");
        }
        let self_answer_note = self_answer_note(state, caller);
        let card = creation_card(
            &creator_id,
            creator.name(),
            &request,
            &profile,
            &labels,
            &ticket,
            self_answer_note.as_deref(),
        );
        let authorized = state
            .sessions
            .ask_creation_card(&creator_id, &registration.owner, card);
        if !authorized {
            return tool_error(id, "permission refused");
        }
        state.sessions.accept_agent_creation(&creator_id);
    }
    let creator_runtime = state
        .sessions
        .live_runtime(&creator_id, &registration.owner);
    let creation = crate::session::AgentCreation {
        creator_session_id: creator_id.clone(),
        creator_runtime,
        display_name: request.title.clone(),
        creator,
        provider: profile.provider.clone(),
        // The session records the profile's **id** and its **name** is what the
        // creator's transcript shows: a rename later changes nothing about a
        // child that is already running (`Session.profile_id`), while the
        // sentence a human reads names the profile the way they ticked it.
        profile_id: profile.id.clone(),
        profile_name: profile.name.clone(),
        // What the card named is what the child gets: the profile's own mode,
        // model, thinking option and auto-accept tick, as the one typed
        // delivery the spawn path applies. A value that cannot be delivered
        // refuses the creation; nothing here is substituted.
        delivery: crate::profile_delivery::ProfileDelivery::for_child(
            &profile.mode,
            &profile.model,
            profile.thinking_option_id.as_deref(),
            &profile.features,
        ),
        overlay: profile.overlay.clone(),
        labels,
        context_id: Some(context_id),
        depth,
        cwd,
        initial_prompt: request.initial_prompt,
        notify: request.notify,
        workspace_id,
    };
    match state
        .sessions
        .create_session_for_agent(state, creation, ticket)
    {
        Ok(session) => {
            if let Some(key) = retry_key.as_deref() {
                crate::server::remember_creation_session(
                    state,
                    &registration.owner,
                    key,
                    &fingerprint,
                    &session,
                );
            }
            // The result is remembered, so the key stops being in flight: a
            // client that re-sends now reads the answer above instead of being
            // told a creation is in progress (`S5-03`).
            if let Some(hold) = hold.as_mut() {
                hold.commit();
            }
            created_result(id, &session, state.mcp.is_registered(&session.id))
        }
        // Every refusal above and this failure release the reservation
        // through the ticket's own `Drop` (audit S5B-02): one release path,
        // taken exactly once, whatever happened.
        Err(error) => tool_error(id, &error.message),
    }
}

/// A refusal an agent reads: the sentence, and never a session id.
/// Can this daemon launch `provider`? Two sources, because there are two ways
/// a provider can exist: a catalogue row must be found on PATH, while a
/// user-declared row carries its own argv and is launchable without being on
/// PATH at all (`acp_client::resolve_named` reads the live registry before the
/// PATH/CDN walk). Asking PATH alone refused every user provider on this road
/// while the wire create road spawned it — one provider, two answers,
/// depending on which door the caller came through.
fn provider_is_launchable(provider: &str) -> bool {
    crate::session::catalog_registry()
        .user_row_for(provider)
        .is_some()
        || crate::provider_catalog::find_available(provider).is_some()
}

fn tool_error(id: &Value, message: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{"type": "text", "text": message}],
            "isError": true,
        },
    })
}

/// The card's auto-accept line, worded per the tri-state's own rule — never
/// assert the negative, and never assert the affirmative of the unknown: a
/// mode whose vocabulary is the agent's own is "cannot establish", not "No",
/// because the child may yet run without asking. One function so the wording
/// and its test cannot disagree.
fn auto_accept_line(answer: devboule_protocol::UnattendedState, mode_id: &str) -> String {
    match answer {
        devboule_protocol::UnattendedState::Yes => format!("Yes (mode {mode_id})"),
        devboule_protocol::UnattendedState::No => format!("No — mode {mode_id} asks the human"),
        devboule_protocol::UnattendedState::Unknown => format!(
            "Cannot establish — mode {mode_id} belongs to the agent's own vocabulary, so whether it asks is not something Devboule can check"
        ),
    }
}

/// The one consent-surface fact the composition adds (F1, decided): a peer
/// holding `answer_permissions` may answer its own creation card, and a human
/// reading the card must see that the asking device is also a potential
/// answerer. Paseo's model is the reference — its `create_agent_request`
/// needs the capability pair and shows no card at all, the grant IS the
/// consent — so this is not a gate to add but a fact to state on the card we
/// keep as the human's courtesy surface.
fn self_answer_note(state: &ServerState, caller: &McpCaller) -> Option<String> {
    let McpCaller::Peer {
        device_id, caps, ..
    } = caller
    else {
        return None;
    };
    if !caps
        .iter()
        .any(|cap| cap == crate::peer_policy::CAP_ANSWER_PERMISSIONS)
    {
        return None;
    }
    let name = state
        .peer_get(device_id.as_str())
        .ok()
        .flatten()
        .map(|record| record.display_name)
        .unwrap_or_else(|| device_id.clone());
    Some(format!(
        "The asking device '{name}' holds answer_permissions and may answer this card itself."
    ))
}

/// The creation card (`S5` decisions 4 and 5; `create-from-profile`).
///
/// An ordinary [`SessionEvent::PermissionRequest`] with the `create_agent`
/// payload filled in: the same pending entry, the same allow/deny decision
/// frame, the same origin stamp and per-device budget as any other card. The
/// caps are in the text *and* in the payload — the text is what a person reads,
/// the payload is what a surface renders, and both come from one reservation.
///
/// The text states what the human is being asked to **approve**, which is the
/// profile and what it resolves to: the provider, the model, the mode, the
/// thinking option, whether the child will approve prompts in their place (and
/// which mode does the answering), the feature keys the daemon does not
/// interpret — named as uninterpreted, never silently dropped — and the
/// caller's labels. A card that named only the profile would ask for a
/// decision against a word, and the word is the one thing the human cannot
/// check without opening Settings.
///
/// With the creation refusing every value the clients cannot deliver, this
/// text is honest by construction rather than by wording: a card a human can
/// approve into an existing child prints only what the child was delivered.
fn creation_card(
    creator_session_id: &str,
    creator_name: &str,
    request: &AgentCreateRequest,
    profile: &ResolvedProfile,
    labels: &std::collections::BTreeMap<String, String>,
    ticket: &crate::session::AgentCreationTicket<'_>,
    self_answer_note: Option<&str>,
) -> SessionEvent {
    let caps = ticket.caps().clone();
    // `Auto accept: Yes` is the one phrase that has to be readable at a glance:
    // it is the difference between a child that will ask this human and one that
    // will not.
    //
    // `autoAccept` is the feature the daemon interprets, so it is rendered by
    // the auto-accept line — naming the **mode** that does the answering,
    // because consent to a mechanism is not consent to a word — and every
    // other key is named as what it is: stored, delivered never, promised
    // never. Absent is a third state here, never a silence and never a claim.
    let uninterpreted = profile
        .features
        .iter()
        .filter(|(key, _)| key.as_str() != crate::provider_catalog::AUTO_ACCEPT_FEATURE)
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>();
    let features = if uninterpreted.is_empty() {
        "none".to_string()
    } else {
        format!(
            "{} (not interpreted by this daemon; carried but never delivered)",
            uninterpreted.join(", ")
        )
    };
    // The card's auto-accept line reads the mode, and only the mode, through
    // the same prediction the birth will apply: the tick over an asking mode
    // is refused before this card is raised (the R2a audit's F7), so on every
    // card carrying a tick, "Yes" names the mode that does the answering.
    let auto = auto_accept_line(
        predicted_unattended(&profile.provider, &profile.mode),
        &profile.mode,
    );
    let thinking = profile.thinking_option_id.as_deref().unwrap_or("none");
    // The caller's own labels, and only those: the daemon's four `devboule.`
    // keys are stamped at the creation and would tell the human nothing they are
    // not already reading on this card.
    let labels = if labels.is_empty() {
        "none".to_string()
    } else {
        labels
            .iter()
            .map(|(key, value)| format!("{key}={value}"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    // S2 honesty: the card promises verification (precedence rule). The tools
    // state comes from the one function above; the sentence is glued to the
    // description and the word rides the payload.
    let card_tools = card_tools_for_provider(&profile.provider);
    let tools_sentence = card_tools_sentence(card_tools);
    // The consent surface names its own composition (F1, decided): a paired
    // device holding `answer_permissions` may answer this card itself, and a
    // human reading it must be able to see that the asking device is also a
    // potential answerer. Paseo's model is the reference: its
    // `create_agent_request` needs the capability pair and shows no card at
    // all - the grant IS the consent. Ours keeps the card as the human's
    // courtesy surface and states the fact on it.
    let self_answer = match self_answer_note {
        Some(note) => format!(" {note}"),
        None => String::new(),
    };
    SessionEvent::PermissionRequest {
        tool_call_id: creation_permission_id(),
        title: format!("Create an agent: {} ({})", request.title, profile.name),
        description: Some(format!(
            "Asked for by '{creator_name}'. Profile '{name}' ({id}): provider {provider}, model {model}, mode {mode}, thinking {thinking}, features {features}, auto accept: {auto}. Labels: {labels}. Caps: live children {} of {}, creations this hour {} of {}, depth {} of {}, live agent sessions {} of {}.{tools_sentence}{self_answer}",
            caps.live_children,
            caps.max_live_children,
            caps.creations_this_hour,
            caps.max_creations_per_hour,
            caps.depth,
            caps.max_depth,
            caps.live_agent_sessions,
            caps.max_live_agent_sessions,
            name = profile.name,
            id = profile.id,
            provider = profile.provider,
            model = profile.model,
            mode = profile.mode,
            auto = auto,
            tools_sentence = tools_sentence,
        )),
        command: None,
        args: None,
        cwd: None,
        env: None,
        options: vec![
            PermissionOption {
                option_id: "allow".to_string(),
                name: "Create once".to_string(),
                kind: "allow_once".to_string(),
            },
            PermissionOption {
                option_id: "deny".to_string(),
                name: "Deny".to_string(),
                kind: "reject_once".to_string(),
            },
        ],
        // A placeholder: the permission broker stamps the creator's own origin
        // on the way in, exactly as it does for a provider's own card.
        origin: SessionOrigin::unknown(),
        create_agent: Some(CreateAgentCard {
            creator_session_id: creator_session_id.to_string(),
            provider: profile.provider.clone(),
            profile: profile.name.clone(),
            title: request.title.clone(),
            tools: card_tools.as_str().to_string(),
            caps,
        }),
    }
}

/// The tools state a creation card promises, from the profile's provider (S2).
///
/// One function so the card cannot drift from the catalog: the provider name
/// is resolved through `provider_catalog::session_kind_for` — the one place a
/// provider name is consulted — and only the resulting `SessionKind` is
/// matched (never a provider string here, per the open-provider rule). S9: all
/// agent families host carriers, so every family promises `Hosted`, with the
/// description carrying "will be verified at start" per the precedence rule
/// (the card promises verification, the result/roster report it).
pub(crate) fn card_tools_for_provider(provider: &str) -> ToolsState {
    let _kind = crate::provider_catalog::session_kind_for(provider);
    ToolsState::Hosted
}

/// The card's tools sentence for one promised state (S2). The unavailable
/// sentence is the plan's words; the hosted sentence keeps the precedence
/// rule's required phrase.
pub(crate) fn card_tools_sentence(state: ToolsState) -> &'static str {
    match state {
        ToolsState::Unavailable => {
            " The child will start without Devboule tools: it cannot create, message or list agents."
        }
        ToolsState::Hosted => " The child will host Devboule tools and will be verified at start.",
        ToolsState::Unverified => " The child's tools are unverified and will be verified at start.",
    }
}

/// The correlation id of one creation card. Distinct per call, like the
/// terminal gate's, so two creations from one session cannot collide in the
/// pending table.
fn creation_permission_id() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    format!(
        "create:{:x}-{:x}-{}",
        std::process::id(),
        nanos,
        COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

/// Why a `tools/call` name is refused, if it is (`S5` §2).
///
/// The stored policy first, then the preset's overlay — and one sentence for
/// both, so a `design` child that calls a name it was never offered is refused
/// exactly like one a policy disabled, and cannot probe past the list.
fn tool_call_refusal(
    policy: Option<&ToolPolicyEntry>,
    overlay: &ToolOverlay,
    name: &str,
) -> Option<&'static str> {
    if !crate::tool_policy::is_tool_enabled(policy, name) || !overlay.allows(name) {
        return Some("Tool disabled by policy");
    }
    None
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

/// The one protected-bytes primitive every carrier writer uses (S4) now lives
/// in `crate::atomic` (P2: one writer, all callers). Both format wrappers below
/// go through it.
fn write_protected_json(path: &Path, value: &Value) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(value).map_err(io::Error::other)?;
    crate::atomic::write_protected_bytes(path, &bytes)
}

/// The second wrapper (S4): text carriers (the pi bridge in S5, the Codex TOML
/// in S6) go through the same primitive. The TOML parse-back before rename
/// that S6 needs is S6's hook on top of this; this step owns the helper it calls.
pub(crate) fn write_protected_str(path: &Path, text: &str) -> io::Result<()> {
    crate::atomic::write_protected_bytes(path, text.as_bytes())
}

/// Child-env names for the broker carrier (S5/S6). The names carry no secret
/// bytes themselves (pinned by the S4 redaction test); the token travels only
/// as the env *value*, never argv (`ARCHITETTURA.md` §15.5 rule).
pub(crate) const MCP_TOKEN_ENV: &str = "DEVBOULE_MCP_TOKEN";
pub(crate) const MCP_URL_ENV: &str = "DEVBOULE_MCP_URL";

fn cleanup_stale_configs(runtime_dir: &Path) -> io::Result<()> {
    let Ok(entries) = fs::read_dir(runtime_dir) else {
        return Ok(());
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        // Owned Codex homes (S6): whole trees by our name alone — never by
        // content, never outside our names — so a crashed spawn's goals, sqlite
        // and `installation_id` cannot accumulate. At daemon start no live
        // session exists, so every match is an orphan by construction.
        if name.starts_with("devboule-codex-home-") {
            let _ = fs::remove_dir_all(&path);
            continue;
        }
        let stale = {
            // Our carriers, by our names: the Claude config plus the pi
            // permission/bridge files S5 writes beside it (and their temps).
            // These are the daemon's own file names, not provider dispatch —
            // no behaviour branches on them — so listing them here keeps the
            // provider dimension open while orphans from a dead daemon (or a
            // crashed spawn) cannot accumulate. At daemon start no live session
            // exists, so every match is an orphan by construction. Never match
            // by content, and never sweep a tree we do not own (the Codex home
            // dir in S6 gets its own owned-dir removal for the same reason).
            (name.starts_with(CONFIG_PREFIX) && (name.ends_with(".json") || name.ends_with(".tmp")))
                || (name.starts_with("devboule-pi-permissions-")
                    && (name.ends_with(".ts") || name.ends_with(".tmp")))
                || (name.starts_with("devboule-pi-bridge-")
                    && (name.ends_with(".ts") || name.ends_with(".tmp")))
        };
        if stale {
            let _ = fs::remove_file(path);
        }
    }
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
    use crate::provider_catalog::{MCP_CREATE_AGENT_TOOL, MCP_ROSTER_TOOL, MCP_SEND_MESSAGE_TOOL};
    use std::net::Shutdown;
    use std::sync::mpsc;

    /// A user-declared row carries its own argv, so it is launchable without
    /// being on PATH — `resolve_named` reads the live registry before the
    /// PATH/CDN walk. Asking PATH alone refused every user provider on the
    /// MCP create-from-profile road while the wire create road spawned it:
    /// one provider, two answers, depending on which door the caller came in.
    #[test]
    fn a_user_row_is_launchable_even_though_it_is_not_on_path() {
        let rows = crate::user_providers::parse_providers_document(
            br#"{"launchable-agent": {"extends": "acp", "command": ["/bin/launchable"]}}"#,
            &crate::session::native_family_ids(),
        )
        .expect("a valid row");
        let gate = crate::user_providers::lock_rows_state();
        crate::session::apply_user_rows(rows);

        assert!(
            crate::provider_catalog::find_available("launchable-agent").is_none(),
            "it is not on PATH: that is what makes this test mean something"
        );
        assert!(
            provider_is_launchable("launchable-agent"),
            "a live user row is launchable through its own argv"
        );

        crate::session::apply_user_rows(std::collections::BTreeMap::new());
        drop(gate);
        assert!(
            !provider_is_launchable("launchable-agent"),
            "and once the row is retired it is not launchable again"
        );
    }

    #[test]
    fn tools_state_tri_state_and_single_computation_point() {
        // Closed-table walk: three variants, three distinct wire strings,
        // each string parsing back to exactly one variant.
        let states = [
            ToolsState::Hosted,
            ToolsState::Unavailable,
            ToolsState::Unverified,
        ];
        let words: Vec<&str> = states.iter().map(|state| state.as_str()).collect();
        assert_eq!(words.len(), 3);
        assert!(words.iter().all(|word| !word.is_empty()));
        let mut sorted = words.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 3, "each ToolsState must serialise distinctly");
        for state in states {
            assert_eq!(tools_state_from_str(state.as_str()), Some(state));
        }
        assert_eq!(tools_state_from_str(""), None);
        assert_eq!(tools_state_from_str("HOSTED"), None);
        // The forbidden combination the type exists to name: a pi/Codex
        // session WITH a bearer but WITHOUT verification reads Unverified —
        // never Hosted, never Unavailable. Built through the single
        // computation point directly (registration state as booleans).
        for kind in [
            SessionKind::Acp,
            SessionKind::Claude,
            SessionKind::Pi,
            SessionKind::Codex,
        ] {
            assert_eq!(
                compute_tools_state(&kind, true, false),
                ToolsState::Unverified,
                "registered-but-unverified must be Unverified for {kind:?}"
            );
            assert_eq!(
                compute_tools_state(&kind, true, true),
                ToolsState::Hosted,
                "registered-and-verified must be Hosted for {kind:?}"
            );
            // The inverse forbidden state: no registration and no bearer
            // reads Unavailable, never Unverified.
            assert_eq!(
                compute_tools_state(&kind, false, false),
                ToolsState::Unavailable,
                "unregistered must be Unavailable for {kind:?}"
            );
            assert_eq!(
                compute_tools_state(&kind, false, true),
                ToolsState::Unavailable,
                "verification without registration is still Unavailable for {kind:?}"
            );
        }
        // Storage starts unknown-as-absent: a fresh runtime reads
        // Unavailable until registration flips it (S8).
        let runtime = crate::session::SessionRuntime::new();
        assert_eq!(runtime.tools_state(), ToolsState::Unavailable);
        runtime.set_tools_state(ToolsState::Unverified);
        assert_eq!(runtime.tools_state(), ToolsState::Unverified);
    }

    fn s2_session(kind: devboule_protocol::SessionKind) -> devboule_protocol::Session {
        devboule_protocol::Session {
            id: "s.s2.1".to_string(),
            workspace_id: None,
            cwd: None,
            kind,
            title: "Agent".to_string(),
            provider: None,
            peer_session_id: None,
            state: devboule_protocol::SessionState::Live { generation: 1 },
            elapsed_ms: Some(0),
            created_at_ms: 1,
            origin: devboule_protocol::SessionOrigin::local(),
            display_name: Some("builder".to_string()),
            created_by: Some("s.s2.0".to_string()),
            profile_id: None,
            context_id: Some("s.s2.0".to_string()),
            unattended: devboule_protocol::UnattendedState::No,
            labels: Default::default(),
            resumable: false,
        }
    }

    #[test]
    fn bind_without_registration_is_a_noop() {
        // S8 bind-split safety: `bind_runtime` without a row touches nothing —
        // no bearer, no URL, no state flip. This is what makes the else-branch
        // bind production-identical without a row. (The registered half is
        // wired by the S9 road test, which drives a minted carrier live.)
        let state = ServerState::new("mcp-bind-noop".to_string());
        let runtime = Arc::new(crate::session::SessionRuntime::new());
        state.mcp.bind_runtime("s.nobody.9", &runtime);
        assert_eq!(runtime.tools_state(), ToolsState::Unavailable);
    }

    #[test]
    fn registration_is_a_fact_the_surfaces_read() {
        // S9: the flipped gate admits every agent kind — Acp AND Codex rows
        // exist; unknown ids do not. (In parts 1–2 this same test pinned the
        // closed gate with Codex → None; the flip is the pass.)
        let state = ServerState::new("mcp-registered-fact".to_string());
        let owner = owner("mcp-user-reg", "mcp-client-reg");
        assert!(!state.mcp.is_registered("s.nobody.1"));
        let _guard = state
            .mcp
            .register("s.reg.1", &owner, &SessionKind::Acp)
            .expect("registration")
            .expect("MCP guard");
        assert!(state.mcp.is_registered("s.reg.1"));
        let codex = state
            .mcp
            .register_with_provider(
                "s.codex.1",
                &owner,
                &SessionKind::Codex,
                Some("codex"),
                AgentLineage::root(),
            )
            .expect("the gate answers")
            .expect("S9 mints for Codex");
        assert!(state.mcp.is_registered("s.codex.1"));
        drop(codex);
        assert!(
            !state.mcp.is_registered("s.codex.1"),
            "dropping the guard revokes the bearer"
        );
    }

    /// Pass 2c: nobody spells the MCP question as a list of kinds any more,
    /// anywhere. The answers live in the provider impls (`Provider::hosts_mcp`,
    /// `Provider::mcp_gates_first_prompt`) and the broker's two predicates are
    /// shims answering through the registry; the zero counts below pin that no
    /// file reintroduces an inline kind gate, and the walk pins the exact
    /// answers through the shim (shim -> registry -> impl), including the
    /// deliberate narrowness: pi/Codex host but never block a first prompt.
    /// (Needles are concatenated so this very test does not match itself.)
    #[test]
    fn mcp_predicates_are_provider_facts_not_kind_lists() {
        let two = ["SessionKind::Acp ", "| SessionKind::Claude"].concat();
        let four_tail = ["| SessionKind::Pi ", "| SessionKind::Codex"].concat();
        let sources = [
            include_str!("mcp_broker.rs"),
            include_str!("session.rs"),
            include_str!("provider.rs"),
        ];
        let mut narrow = 0;
        let mut wide = 0;
        for source in sources {
            for line in source.lines() {
                if line.contains(two.as_str()) {
                    if line.contains(four_tail.as_str()) {
                        wide += 1;
                    } else {
                        narrow += 1;
                    }
                }
            }
        }
        assert_eq!(wide, 0, "no file spells the four-kind MCP gate any more");
        assert_eq!(
            narrow, 0,
            "no file spells the two-kind wait rule any more; gate sites call, never spell"
        );
        for (kind, hosts, gates) in [
            (SessionKind::Acp, true, true),
            (SessionKind::Claude, true, true),
            (SessionKind::Pi, true, false),
            (SessionKind::Codex, true, false),
            (SessionKind::Terminal, false, false),
        ] {
            assert_eq!(hosts_mcp(&kind), hosts, "hosts_mcp for {kind:?}");
            assert_eq!(
                mcp_gates_first_prompt(&kind),
                gates,
                "mcp_gates_first_prompt for {kind:?}"
            );
        }
    }

    #[test]
    fn phase0_gate_names_the_decision_it_makes() {
        // The guard is still the branch that decides. Post-S9 only Terminal
        // takes `Ok(None)`; every agent kind registers. The sentence it logs
        // names the session id, the kind and provider, and the state
        // `unavailable` with the reason. `eprintln!` output cannot be captured
        // in a unit test, so the test pins the sentenced string the guard
        // emits (stated adaptation); the emission itself is verified by reading
        // the daemon log on a Terminal create.
        let line = phase0_gate_log("s.term.1", &SessionKind::Terminal, None);
        assert!(line.contains("s.term.1"), "names the session: {line}");
        assert!(line.contains("Terminal"), "names the kind: {line}");
        assert!(
            line.contains("unavailable"),
            "never renders the unknown as the benign state: {line}"
        );
        let state = ServerState::new("mcp-phase0".to_string());
        let owner = owner("mcp-user-phase0", "mcp-client-phase0");
        let terminal = state
            .mcp
            .register_with_provider(
                "s.phase0.1",
                &owner,
                &SessionKind::Terminal,
                None,
                AgentLineage::root(),
            )
            .expect("the gate answers");
        assert!(terminal.is_none(), "Terminal hosts no broker, still");
        for kind in [
            SessionKind::Acp,
            SessionKind::Claude,
            SessionKind::Pi,
            SessionKind::Codex,
        ] {
            assert!(hosts_mcp(&kind), "S9: every agent kind hosts: {kind:?}");
        }
        assert!(!hosts_mcp(&SessionKind::Terminal));
    }

    #[test]
    fn creation_card_promises_tools_honestly_per_provider() {
        // S9: every agent family hosts a carrier, so every card promises
        // Hosted with the precedence rule's phrase. (In parts 1–2 pi/codex
        // promised Unavailable with the no-tools sentence; the flip retires
        // that branch — the sentence helpers below stay for result/roster.)
        // Forbidden state: a card without the verification promise.
        for provider in ["pi", "codex", "claude", "gemini", "grok", "qwen"] {
            assert_eq!(
                card_tools_for_provider(provider),
                ToolsState::Hosted,
                "{provider} children host tools"
            );
        }
        let unavailable = card_tools_sentence(ToolsState::Unavailable);
        assert!(
            unavailable.contains("without Devboule tools"),
            "pi/Codex card carries the no-tools sentence"
        );
        assert!(
            unavailable.contains("cannot create, message or list agents"),
            "the sentence states the consequence: {unavailable}"
        );
        let hosted = card_tools_sentence(ToolsState::Hosted);
        assert!(
            hosted.contains("will be verified at start"),
            "card promises verification, never bare has-tools: {hosted}"
        );
        assert!(
            !hosted.contains("without Devboule tools"),
            "hosted card never carries the no-tools sentence: {hosted}"
        );
    }

    #[test]
    fn creation_result_reports_verification_per_state() {
        // Result-shape test across the three states: the `tools` word rides
        // every result, and unavailable/unverified add the model-readable
        // sentence while hosted adds none. Forbidden state: a result claiming
        // hosted for a session with no bearer (call `_for_tools` with Hosted
        // for a Pi-kind session — the wrapper below would never produce it).
        let id = json!(7);
        for (state, word, sentence) in [
            (ToolsState::Hosted, "hosted", None),
            (ToolsState::Unverified, "unverified", Some("unverified")),
            (
                ToolsState::Unavailable,
                "unavailable",
                Some("without Devboule tools"),
            ),
        ] {
            let session = s2_session(SessionKind::Acp);
            let result = created_result_for_tools(&id, &session, state);
            let structured = &result["result"]["structuredContent"];
            assert_eq!(
                structured["tools"], word,
                "every result carries the tools word"
            );
            let text = result["result"]["content"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .to_string();
            match sentence {
                Some(needle) => assert!(
                    text.contains(needle),
                    "{word} result carries its sentence: {text}"
                ),
                None => assert!(
                    !text.contains("without Devboule tools") && !text.contains("unverified"),
                    "hosted result adds no tools sentence: {text}"
                ),
            }
        }
        // S8: the wrapper reads the registration FACT, not the kind — a Codex
        // child with a minted carrier reads unverified (establishing), one
        // without reads unavailable. Production-identical while the gate holds.
        let unregistered = created_result(&id, &s2_session(SessionKind::Codex), false);
        assert_eq!(
            unregistered["result"]["structuredContent"]["tools"],
            "unavailable"
        );
        let registered = created_result(&id, &s2_session(SessionKind::Codex), true);
        assert_eq!(
            registered["result"]["structuredContent"]["tools"],
            "unverified"
        );
        let acp = created_result(&id, &s2_session(SessionKind::Acp), true);
        assert_eq!(acp["result"]["structuredContent"]["tools"], "unverified");
    }

    #[test]
    fn protected_bytes_write_is_mode_narrow_and_atomic() {
        // S4 DACL-order test: the primitive creates narrow and stays narrow.
        // On unix the assertion is the 0o600 mode bit; on Windows the DACL call
        // runs before the first byte (code inspection) and the test pins content
        // + cleanup. Mutation: drop the 0o600 mode (unix) → this test red.
        let dir = std::env::temp_dir().join(format!("devboule-s4-write-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("carrier.json");
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("tmp"));
        crate::atomic::write_protected_bytes(&path, b"{\"a\":1}").expect("protected write");
        assert_eq!(
            std::fs::read(&path).expect("read back").as_slice(),
            b"{\"a\":1}"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path)
                .expect("metadata")
                .permissions()
                .mode();
            assert_eq!(
                mode & 0o777,
                0o600,
                "carrier files are owner-only (actual {mode:o})"
            );
        }
        // The temp name is the writer's own: a second write replaces atomically
        // via rename (create_new guards the temp, not the target).
        crate::atomic::write_protected_bytes(&path, b"{}").expect("atomic replace");
        assert_eq!(std::fs::read(&path).expect("read back").as_slice(), b"{}");
        // Missing parent is created, missing grandparent chain included.
        let nested = dir.join("sub").join("deep.txt");
        write_protected_str(&nested, "hello").expect("nested write");
        assert_eq!(std::fs::read_to_string(&nested).expect("read"), "hello");
        // JSON wrapper round-trips through the same primitive.
        let json_path = dir.join("roundtrip.json");
        write_protected_json(&json_path, &json!({"x": [1, 2]})).expect("json write");
        let back: Value =
            serde_json::from_slice(&std::fs::read(&json_path).expect("read")).expect("parse");
        assert_eq!(back, json!({"x": [1, 2]}));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn sweep_removes_owned_carriers_and_keeps_strangers() {
        // S4 sweep test, S6-extended: stale Claude configs, pi permission/bridge
        // files and their temps go, plus whole owned Codex home trees (S6); a
        // non-matching file — and a non-matching dir — stay. Forbidden states:
        // an orphan bridge file surviving teardown, an orphan home tree
        // surviving it (leave either → red).
        let dir = std::env::temp_dir().join(format!("devboule-s4-sweep-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        for name in [
            "devboule-mcp-abc.json",
            "devboule-mcp-abc.tmp",
            "devboule-pi-permissions-7.ts",
            "devboule-pi-permissions-7.tmp",
            "devboule-pi-bridge-9.ts",
            "devboule-pi-bridge-9.tmp",
        ] {
            std::fs::write(dir.join(name), b"orphan").expect("plant orphan");
        }
        let home = dir.join("devboule-codex-home-9");
        std::fs::create_dir_all(home.join("state")).expect("plant orphan home tree");
        std::fs::write(home.join("config.toml"), b"orphan").expect("plant orphan config");
        std::fs::write(home.join("state").join("sqlite"), b"orphan").expect("plant orphan state");
        std::fs::write(dir.join("notes.txt"), b"mine").expect("plant stranger");
        std::fs::create_dir_all(dir.join("someone-elses-dir")).expect("plant stranger dir");
        std::fs::write(dir.join("devboule-mcp-abc.json.bak"), b"bak").expect("plant bak");
        cleanup_stale_configs(&dir).expect("sweep");
        for name in [
            "devboule-mcp-abc.json",
            "devboule-mcp-abc.tmp",
            "devboule-pi-permissions-7.ts",
            "devboule-pi-permissions-7.tmp",
            "devboule-pi-bridge-9.ts",
            "devboule-pi-bridge-9.tmp",
        ] {
            assert!(!dir.join(name).exists(), "orphan {name} is swept");
        }
        assert!(dir.join("notes.txt").exists(), "strangers are kept");
        assert!(
            dir.join("someone-elses-dir").is_dir(),
            "stranger dirs are kept"
        );
        assert!(
            !dir.join("devboule-codex-home-9").exists(),
            "orphan Codex home trees are swept"
        );
        assert!(
            dir.join("devboule-mcp-abc.json.bak").exists(),
            ".bak is not our temp suffix and is kept"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn redaction_covers_bearer_and_url_but_not_env_names() {
        // S4 redaction test: bearer+url vanish from errors/logs for the new
        // carriers too; env names (`DEVBOULE_MCP_TOKEN`, `DEVBOULE_MCP_URL`)
        // carry no secret bytes themselves and pass through.
        let config = McpLaunchConfig {
            url: "http://127.0.0.1:4567/mcp".to_string(),
            bearer: "secret-bearer-xyz".to_string(),
            claude_config_path: None,
        };
        let error = format!(
            "bridge dial {} with Bearer {} failed",
            config.url,
            config.bearer()
        );
        let redacted = config.redact_text(&error);
        assert!(
            !redacted.contains("secret-bearer-xyz"),
            "bearer redacted: {redacted}"
        );
        assert!(!redacted.contains("4567"), "endpoint redacted: {redacted}");
        let argv = "pi --mode rpc -e bridge.ts with DEVBOULE_MCP_TOKEN and DEVBOULE_MCP_URL";
        assert_eq!(config.redact_text(argv), argv, "env names are not secrets");
        // S8: the same cover for Codex carrier errors (home path + names pass,
        // secrets do not).
        let codex_error = format!(
            "Could not prepare the Codex home: dial {} with Bearer {} (CODEX_HOME set)",
            config.url,
            config.bearer()
        );
        let redacted = config.redact_text(&codex_error);
        assert!(
            !redacted.contains("secret-bearer-xyz"),
            "bearer redacted: {redacted}"
        );
        assert!(!redacted.contains("4567"), "endpoint redacted: {redacted}");
        assert!(redacted.contains("CODEX_HOME"), "names pass through");
    }

    #[test]
    fn roster_entries_carry_the_tools_word() {
        // Roster test with mixed states (and, since S8, mixed kinds — the word
        // is kind-blind by construction): every entry carries `tools`.
        // Forbidden state: one entry with the field dropped (remove the field
        // in the fixture → red).
        for kind in [SessionKind::Acp, SessionKind::Codex] {
            for state in [
                ToolsState::Hosted,
                ToolsState::Unavailable,
                ToolsState::Unverified,
            ] {
                let runtime = crate::session::SessionRuntime::new();
                runtime.set_tools_state(state);
                let value = agent_value(&s2_session(kind.clone()), &runtime, 1);
                assert_eq!(
                    value["tools"],
                    state.as_str(),
                    "every roster entry carries its tools word"
                );
            }
        }
    }

    #[test]
    fn agent_activity_tool_serves_one_agents_metadata() {
        let state = ServerState::new("mcp-activity".to_string());
        let stranger_owner = owner("mcp-stranger-user", "mcp-stranger-client");
        let owner = owner("mcp-activity-user", "mcp-activity-client");
        crate::session::insert_test_live_agent(&state.sessions, "activity-caller", owner.clone());
        let child = crate::session::insert_test_live_agent(
            &state.sessions,
            "activity-child",
            owner.clone(),
        );
        child.publish_agent_event(
            SessionEvent::AgentThought {
                message_id: None,
                text: "thinking".to_string(),
                parent_tool_use_id: None,
                spawn_depth: None,
            },
            None,
        );
        let guard = state
            .mcp
            .register("activity-caller", &owner, &SessionKind::Acp)
            .expect("registration")
            .expect("caller MCP guard");
        let token = state.mcp.test_token("activity-caller").expect("token");
        let server = state.mcp.start(&state).expect("MCP server");
        let listed = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        );
        let listed_body = response_json(&listed);
        let tools = listed_body["result"]["tools"].as_array().expect("tools");
        let activity = tools
            .iter()
            .find(|tool| tool["name"] == crate::provider_catalog::MCP_ACTIVITY_TOOL)
            .expect("activity is served");
        assert_eq!(activity["inputSchema"]["required"], json!(["session"]));
        let response = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"devboule_agent_activity","arguments":{"session":"activity-child"}}}"#,
        );
        let body = response_json(&response);
        assert_eq!(body["result"]["isError"], false);
        let doc = &body["result"]["structuredContent"];
        assert_eq!(doc["sessionId"], "activity-child");
        assert_eq!(doc["activity"], "idle");
        let recent = doc["recent"].as_array().expect("recent");
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0]["kind"], "agent_thought");
        assert!(
            recent[0].get("text").is_none(),
            "kinds only, never transcript text"
        );
        assert!(doc.get("summary").is_none());
        let missing = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"devboule_agent_activity","arguments":{"session":"activity-missing"}}}"#,
        );
        assert_eq!(response_json(&missing)["error"]["code"], -32602);
        let bogus = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"devboule_agent_activity","arguments":{"session":"activity-child","bogus":1}}}"#,
        );
        assert_eq!(response_json(&bogus)["error"]["code"], -32602);
        // A stranger's session is the same refusal as a missing one: the
        // daemon cannot and must not say which.
        crate::session::insert_test_live_agent(
            &state.sessions,
            "activity-stranger",
            stranger_owner,
        );
        let stranger = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"devboule_agent_activity","arguments":{"session":"activity-stranger"}}}"#,
        );
        assert_eq!(response_json(&stranger)["error"]["code"], -32602);
        // Every test session is titled "Agent": naming the title refuses
        // with the remedy instead of silently reading the lowest id.
        let vague = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"devboule_agent_activity","arguments":{"session":"Agent"}}}"#,
        );
        let vague_body = response_json(&vague);
        assert_eq!(vague_body["error"]["code"], -32602);
        assert!(
            vague_body["error"]["message"]
                .as_str()
                .is_some_and(|message| message.contains("use the session id")),
            "ambiguity names the remedy: {}",
            vague_body["error"]["message"]
        );
        // The limit is honored and capped: 0 reads state only, a huge
        // number stops at the cap.
        for _ in 0..55 {
            child.publish_agent_event(
                SessionEvent::AgentThought {
                    message_id: None,
                    text: "thinking".to_string(),
                    parent_tool_use_id: None,
                    spawn_depth: None,
                },
                None,
            );
        }
        let capped = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"devboule_agent_activity","arguments":{"session":"activity-child","limit":5000}}}"#,
        );
        assert_eq!(
            response_json(&capped)["result"]["structuredContent"]["recent"]
                .as_array()
                .expect("recent")
                .len(),
            crate::agent_activity::ACTIVITY_MAX_LIMIT
        );
        let state_only = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"devboule_agent_activity","arguments":{"session":"activity-child","limit":0}}}"#,
        );
        assert!(
            response_json(&state_only)["result"]["structuredContent"]["recent"]
                .as_array()
                .expect("recent")
                .is_empty()
        );
        drop(guard);
        drop(server);
    }

    #[test]
    fn a_stored_policy_can_take_the_activity_tool_away() {
        // The catalog promises it: supervision is disableable, unlike the
        // roster and the profile list. A disabled tool is refused before
        // anything is touched, and vanishes from tools/list.
        let state = ServerState::new("mcp-activity-policy".to_string());
        let owner = owner("mcp-activity-policy-user", "mcp-activity-policy-client");
        crate::session::insert_test_live_agent(&state.sessions, "policy-caller", owner.clone());
        let guard = state
            .mcp
            .register_with_provider(
                "policy-caller",
                &owner,
                &SessionKind::Acp,
                Some("claude"),
                AgentLineage::root(),
            )
            .expect("registration")
            .expect("MCP guard");
        state
            .tool_policy
            .set(
                "claude",
                Some(true),
                vec![crate::provider_catalog::MCP_ACTIVITY_TOOL.to_string()],
            )
            .expect("policy");
        let token = state.mcp.test_token("policy-caller").expect("token");
        let server = state.mcp.start(&state).expect("MCP server");
        let listed = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        );
        let listed_body = response_json(&listed);
        let names: Vec<&str> = listed_body["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        assert!(!names.contains(&crate::provider_catalog::MCP_ACTIVITY_TOOL));
        let refused = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"devboule_agent_activity","arguments":{"session":"policy-caller"}}}"#,
        );
        let refused_body = response_json(&refused);
        assert_eq!(refused_body.pointer("/error/code"), Some(&json!(-32601)));
        assert_eq!(
            refused_body.pointer("/error/message"),
            Some(&json!("Tool disabled by policy"))
        );
        drop(guard);
        drop(server);
    }

    #[test]
    fn the_end_tools_stop_and_close_a_callers_own_children_only() {
        // The destructive pair end to end: served, scoped to the caller's
        // own children, and refused for the caller's parent, itself, and an
        // invented id — with the sentences the sessions layer owns.
        let state = ServerState::new("mcp-end".to_string());
        let owner = owner("mcp-end-user", "mcp-end-client");
        crate::session::insert_test_live_agent(&state.sessions, "end-parent", owner.clone());
        crate::session::insert_test_child_agent(
            &state.sessions,
            "end-caller",
            owner.clone(),
            "end-parent",
        );
        crate::session::insert_test_child_agent(
            &state.sessions,
            "end-child",
            owner.clone(),
            "end-caller",
        );
        let guard = state
            .mcp
            .register("end-caller", &owner, &SessionKind::Acp)
            .expect("registration")
            .expect("caller MCP guard");
        let token = state.mcp.test_token("end-caller").expect("token");
        let server = state.mcp.start(&state).expect("MCP server");
        let call = |name: &str, arguments: &str| {
            http_request(
                &state.mcp.url,
                Some(&format!("Bearer {token}")),
                &format!(
                    r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"{name}","arguments":{arguments}}}}}"#
                ),
            )
        };
        let sentence = |reply: &str| {
            response_json(reply)["result"]["content"][0]["text"]
                .as_str()
                .expect("the refusal sentence")
                .to_string()
        };
        let listed = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        );
        let tools = response_json(&listed)["result"]["tools"]
            .as_array()
            .expect("tools")
            .to_vec();
        for name in [
            crate::provider_catalog::MCP_STOP_AGENT_TOOL,
            crate::provider_catalog::MCP_CLOSE_AGENT_TOOL,
        ] {
            let tool = tools
                .iter()
                .find(|tool| tool["name"] == json!(name))
                .unwrap_or_else(|| panic!("{name} is served"));
            assert_eq!(tool["inputSchema"]["required"], json!(["session"]));
        }
        // The green stop: the child stops, the row stays.
        let stopped = response_json(&call(
            crate::provider_catalog::MCP_STOP_AGENT_TOOL,
            r#"{"session":"end-child"}"#,
        ));
        assert_eq!(stopped["result"]["isError"], json!(false));
        assert_eq!(stopped["result"]["content"][0]["text"], json!("stopped"));
        let live_ids = || {
            state
                .sessions
                .live_agent_entries(&owner)
                .expect("entries")
                .into_iter()
                .map(|entry| entry.session.id)
                .collect::<Vec<_>>()
        };
        assert!(live_ids().iter().any(|id| id == "end-child"));
        // The parent, itself, and an invented id: refused either way, and
        // the invented one is indistinguishable from the parent.
        assert!(sentence(&call(
            crate::provider_catalog::MCP_STOP_AGENT_TOOL,
            r#"{"session":"end-parent"}"#,
        ))
        .contains("none of your live children"));
        assert!(sentence(&call(
            crate::provider_catalog::MCP_STOP_AGENT_TOOL,
            r#"{"session":"end-caller"}"#,
        ))
        .contains("not its own child"));
        assert!(sentence(&call(
            crate::provider_catalog::MCP_STOP_AGENT_TOOL,
            r#"{"session":"end-invented"}"#,
        ))
        .contains("none of your live children"));
        // A missing argument is a protocol error before any scope runs.
        let bogus = response_json(&call(
            crate::provider_catalog::MCP_STOP_AGENT_TOOL,
            r#"{"bogus":1}"#,
        ));
        assert_eq!(bogus["error"]["code"], json!(-32602));
        // The green close: the row goes, everything else stays.
        let closed = response_json(&call(
            crate::provider_catalog::MCP_CLOSE_AGENT_TOOL,
            r#"{"session":"end-child"}"#,
        ));
        assert_eq!(closed["result"]["isError"], json!(false));
        assert_eq!(closed["result"]["content"][0]["text"], json!("closed"));
        let remaining = live_ids();
        assert!(!remaining.iter().any(|id| id == "end-child"));
        assert!(remaining.iter().any(|id| id == "end-caller"));
        assert!(remaining.iter().any(|id| id == "end-parent"));
        drop(guard);
        drop(server);
    }

    #[test]
    fn a_stored_policy_can_take_the_end_tools_away() {
        // The catalog promises it: the destructive pair is supervision,
        // disableable like the send tool, unlike the roster and the profile
        // list. A disabled tool is refused before anything is touched.
        let state = ServerState::new("mcp-end-policy".to_string());
        let owner = owner("mcp-end-policy-user", "mcp-end-policy-client");
        crate::session::insert_test_live_agent(&state.sessions, "end-policy-caller", owner.clone());
        let guard = state
            .mcp
            .register_with_provider(
                "end-policy-caller",
                &owner,
                &SessionKind::Acp,
                Some("claude"),
                AgentLineage::root(),
            )
            .expect("registration")
            .expect("MCP guard");
        state
            .tool_policy
            .set(
                "claude",
                Some(true),
                vec![
                    crate::provider_catalog::MCP_STOP_AGENT_TOOL.to_string(),
                    crate::provider_catalog::MCP_CLOSE_AGENT_TOOL.to_string(),
                ],
            )
            .expect("policy");
        let token = state.mcp.test_token("end-policy-caller").expect("token");
        let server = state.mcp.start(&state).expect("MCP server");
        let listed = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        );
        let listed_body = response_json(&listed);
        let names: Vec<&str> = listed_body["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        assert!(!names.contains(&crate::provider_catalog::MCP_STOP_AGENT_TOOL));
        assert!(!names.contains(&crate::provider_catalog::MCP_CLOSE_AGENT_TOOL));
        let refused = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"devboule_stop_agent","arguments":{"session":"end-policy-caller"}}}"#,
        );
        assert_eq!(
            response_json(&refused).pointer("/error/code"),
            Some(&json!(-32601))
        );
        drop(guard);
        drop(server);
    }

    #[test]
    fn a_restored_overlay_hides_and_refuses_both_denied_tools() {
        // The two gates a resumed lineage feeds, on the exact functions the
        // broker calls: `enabled_tool_list` for tools/list,
        // `tool_call_refusal` for tools/call. One tool proving one gate
        // does not prove the restriction.
        use crate::provider_catalog::{
            MCP_ACTIVITY_TOOL, MCP_CREATE_AGENT_TOOL, MCP_ROSTER_TOOL, MCP_SEND_MESSAGE_TOOL,
        };
        let overlay = crate::provider_catalog::ToolOverlay::from_profile_names(&[
            MCP_SEND_MESSAGE_TOOL.to_string(),
            MCP_CREATE_AGENT_TOOL.to_string(),
        ]);
        let names: Vec<String> = enabled_tool_list(
            crate::provider_catalog::MCP_BROKER_TOOLS,
            None,
            overlay.clone(),
        )
        .into_iter()
        .filter_map(|tool| tool.get("name").and_then(Value::as_str).map(str::to_string))
        .collect();
        assert!(
            !names.iter().any(|name| name == MCP_SEND_MESSAGE_TOOL),
            "send is hidden from the list: {names:?}"
        );
        assert!(
            !names.iter().any(|name| name == MCP_CREATE_AGENT_TOOL),
            "create is hidden from the list: {names:?}"
        );
        assert!(
            names.iter().any(|name| name == MCP_ROSTER_TOOL)
                && names.iter().any(|name| name == MCP_ACTIVITY_TOOL),
            "the rest is still served: {names:?}"
        );
        assert_eq!(
            tool_call_refusal(None, &overlay, MCP_SEND_MESSAGE_TOOL),
            Some("Tool disabled by policy")
        );
        assert_eq!(
            tool_call_refusal(None, &overlay, MCP_CREATE_AGENT_TOOL),
            Some("Tool disabled by policy")
        );
        assert_eq!(tool_call_refusal(None, &overlay, MCP_ROSTER_TOOL), None);
    }

    #[test]
    fn a_journal_row_restriction_reaches_the_broker_registration() {
        // From journal bytes to broker gates through every production
        // function on the wiring path: row → resumed_lineage →
        // register_with_provider → HTTP tools/list + tools/call. It does not
        // execute resume()'s call site (which needs a live provider for the
        // respawn) nor the respawn itself: reverting that one line escapes
        // this test, and only the live e2e battery covers it.
        use crate::provider_catalog::{MCP_CREATE_AGENT_TOOL, MCP_SEND_MESSAGE_TOOL};
        let dir =
            std::env::temp_dir().join(format!("devboule-overlay-wire-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let journal = crate::journal::Journal::open(&dir.join("journal.db")).expect("journal");
        let mut record = crate::journal::new_session_record(
            "wire-child",
            "wire-user",
            None,
            SessionKind::Acp,
            "Agent",
        );
        record.created_by = Some("wire-creator".to_string());
        record.overlay = Some(crate::provider_catalog::ToolOverlay::from_profile_names(&[
            MCP_SEND_MESSAGE_TOOL.to_string(),
            MCP_CREATE_AGENT_TOOL.to_string(),
        ]));
        record.depth = Some(1);
        journal.create_session(record).expect("birth row");
        journal.shutdown();
        // The restart: a new journal on the same file.
        let journal = crate::journal::Journal::open(&dir.join("journal.db")).expect("reopen");
        let row = journal
            .list()
            .expect("list")
            .into_iter()
            .find(|row| row.id == "wire-child")
            .expect("the birth row survived");
        let lineage = crate::session::SessionRegistry::resumed_lineage(Some(&row))
            .expect("readable row restores");
        assert_eq!(lineage.depth, 1);
        // Register the way resume() does, then ask over HTTP like a child
        // would: both denied tools stay hidden and refused.
        let state = ServerState::new("mcp-overlay-wire".to_string());
        let owner = owner("wire-user", "wire-client");
        crate::session::insert_test_live_agent(&state.sessions, "wire-child", owner.clone());
        let guard = state
            .mcp
            .register_with_provider("wire-child", &owner, &SessionKind::Acp, None, lineage)
            .expect("registration")
            .expect("MCP guard");
        let token = state.mcp.test_token("wire-child").expect("token");
        let server = state.mcp.start(&state).expect("MCP server");
        let listed = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        );
        let listed_body = response_json(&listed);
        let names: Vec<&str> = listed_body["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        assert!(!names.contains(&MCP_SEND_MESSAGE_TOOL));
        assert!(!names.contains(&MCP_CREATE_AGENT_TOOL));
        for (id, tool) in [(2, MCP_SEND_MESSAGE_TOOL), (3, MCP_CREATE_AGENT_TOOL)] {
            let call = http_request(
                &state.mcp.url,
                Some(&format!("Bearer {token}")),
                &format!(
                    r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"{tool}","arguments":{{}}}}}}"#
                ),
            );
            let body = response_json(&call);
            assert_eq!(
                body.pointer("/error/code"),
                Some(&json!(-32601)),
                "{tool} refused"
            );
            assert_eq!(
                body.pointer("/error/message"),
                Some(&json!("Tool disabled by policy"))
            );
        }
        let runtime_dir = state.sessions.runtime_dir().to_path_buf();
        drop(server);
        drop(guard);
        drop(state);
        journal.shutdown();
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(runtime_dir);
    }

    #[test]
    fn pi_bridge_fetch_hygiene_against_the_real_broker() {
        // S5/Q1 measurement: the bridge's exact header set against the REAL broker,
        // raw bytes. Dual Accept takes the JSON branch (not SSE framing); the
        // `notifications/initialized` second call is 202-empty (success without a
        // result — never parsed, never failed); RPC errors ride HTTP 200 (a `res.ok`
        // branch would read refusals as success); chunked is refused; no bearer is
        // 401. The Node-`fetch`-sends-`Content-Length` half is spike-measured +
        // template-pinned (string bodies); this pins the broker half it speaks to.
        use std::net::Shutdown;
        fn raw_post(url: &str, headers: &[(&str, &str)], body: &str) -> String {
            let endpoint = url
                .strip_prefix("http://")
                .expect("loopback URL")
                .split('/')
                .next()
                .expect("loopback endpoint")
                .to_string();
            let mut stream = TcpStream::connect(endpoint).expect("MCP listener");
            let mut request = format!(
                "POST /mcp HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n",
                body.len()
            );
            for (name, value) in headers {
                request.push_str(&format!("{name}: {value}\r\n"));
            }
            request.push_str(&format!("\r\n{body}"));
            stream.write_all(request.as_bytes()).expect("MCP request");
            stream.shutdown(Shutdown::Write).expect("request shutdown");
            let mut response = Vec::new();
            stream.read_to_end(&mut response).expect("MCP response");
            String::from_utf8(response).expect("HTTP response")
        }
        fn split_response(response: &str) -> (&str, &str) {
            response.split_once("\r\n\r\n").expect("HTTP response body")
        }
        let state = ServerState::new("mcp-bridge-hygiene".to_string());
        let owner = owner("mcp-user-hygiene", "mcp-client-hygiene");
        let _guard = state
            .mcp
            .register("hygiene", &owner, &SessionKind::Acp)
            .expect("registration")
            .expect("MCP guard");
        let token = state.mcp.test_token("hygiene").expect("token");
        // The door resolves callers from the registry row: give the session one
        // (a local row, like a person-started session), or every `tools/call`
        // below is refused as `Absent` before anything is touched.
        crate::session::insert_test_live_agent(&state.sessions, "hygiene", owner.clone());
        let server = state.mcp.start(&state).expect("MCP server");
        let url = state.mcp.url.clone();
        let bearer = format!("Bearer {token}");
        // The bridge's exact header set: JSON body, dual Accept, Bearer.
        let headers_ref: Vec<(&str, &str)> = vec![
            ("Content-Type", "application/json"),
            ("Accept", "application/json, text/event-stream"),
            ("Authorization", bearer.as_str()),
        ];
        // initialize → 200 JSON (raw body opens with `{`: the JSON branch, not
        // SSE framing — the Q1 dual-Accept verdict).
        let init = raw_post(
            &url,
            &headers_ref,
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"devboule-pi-bridge","version":"1"}}}"#,
        );
        let (head, body) = split_response(&init);
        assert!(
            head.starts_with("HTTP/1.1 200"),
            "initialize status: {head}"
        );
        assert!(head.contains("application/json"), "JSON branch: {head}");
        assert!(body.starts_with('{'), "raw JSON body, no event framing");
        let reply: Value = serde_json::from_str(body).expect("initialize reply");
        assert_eq!(reply["result"]["serverInfo"]["name"], "devboule");
        // notifications/initialized → 202 with an empty body: success with no
        // result. The bridge's `mcpNotify` never parses it.
        let notified = raw_post(
            &url,
            &headers_ref,
            r#"{"jsonrpc":"2.0","id":2,"method":"notifications/initialized"}"#,
        );
        let (head, body) = split_response(&notified);
        assert!(
            head.starts_with("HTTP/1.1 202"),
            "notification status: {head}"
        );
        assert!(body.is_empty(), "202 carries no body");
        // tools/list over the same headers → the seven tools as JSON.
        let listed = raw_post(
            &url,
            &headers_ref,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/list"}"#,
        );
        let (head, body) = split_response(&listed);
        assert!(head.starts_with("HTTP/1.1 200"), "list status: {head}");
        let reply: Value = serde_json::from_str(body).expect("list reply");
        let names: Vec<&str> = reply["result"]["tools"]
            .as_array()
            .expect("tools array")
            .iter()
            .filter_map(|tool| tool["name"].as_str())
            .collect();
        assert_eq!(names.len(), crate::provider_catalog::MCP_BROKER_TOOLS.len());
        for (name, _) in crate::provider_catalog::MCP_BROKER_TOOLS {
            assert!(names.contains(name), "broker serves {name}");
        }
        // The trap the bridge avoids: SSE-only Accept gets one-event framing.
        let sse_headers: Vec<(&str, &str)> = vec![
            ("Content-Type", "application/json"),
            ("Accept", "text/event-stream"),
            ("Authorization", &bearer),
        ];
        let sse = raw_post(
            &url,
            &sse_headers,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/list"}"#,
        );
        let (head, body) = split_response(&sse);
        assert!(head.starts_with("HTTP/1.1 200"), "sse status: {head}");
        assert!(
            body.starts_with("event: message\ndata: "),
            "SSE-only gets event framing the bridge avoids by sending dual Accept"
        );
        // RPC errors ride HTTP 200: an unknown tool is a 200 with an error
        // payload, never an HTTP error — a `res.ok` branch reads it as success.
        let unknown = raw_post(
            &url,
            &headers_ref,
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"no_such_tool","arguments":{}}}"#,
        );
        let (head, body) = split_response(&unknown);
        assert!(head.starts_with("HTTP/1.1 200"), "rpc error status: {head}");
        let reply: Value = serde_json::from_str(body).expect("error reply");
        assert_eq!(
            reply["error"]["code"],
            serde_json::json!(-32601),
            "rpc error code"
        );
        // A served call answers a result on the same 200.
        let roster = raw_post(
            &url,
            &headers_ref,
            r#"{"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"devboule_list_agents","arguments":{}}}"#,
        );
        let (head, body) = split_response(&roster);
        assert!(head.starts_with("HTTP/1.1 200"), "call status: {head}");
        let reply: Value = serde_json::from_str(body).expect("call reply");
        assert!(reply["result"]["structuredContent"]["agents"].is_array());
        // No bearer is 401 before anything is touched.
        let bare: Vec<(&str, &str)> = vec![
            ("Content-Type", "application/json"),
            ("Accept", "application/json, text/event-stream"),
        ];
        let denied = raw_post(
            &url,
            &bare,
            r#"{"jsonrpc":"2.0","id":7,"method":"tools/list"}"#,
        );
        assert!(denied.starts_with("HTTP/1.1 401"), "bearer required");
        drop(server);
    }

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
        let send_response = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {caller_token}")),
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"devboule_send_message","arguments":{"to_agent":"agent-missing","text":"hello"}}}"#,
        );
        let send_body = response_json(&send_response);
        assert_eq!(send_body["error"]["code"], -32602);
        drop(caller_guard);
        drop(other_guard);
        drop(server);
    }

    /// The brief's second refusal, measured rather than assumed.
    ///
    /// A connection owned by a session's Bearer is an MCP connection, and its
    /// only channel into the daemon is a tool *name*: the broker runs in this
    /// process and never carries a pipe frame, so there is no
    /// `AgentProfilesGet`/`AgentProfilesSet` a bearer could send. What a bearer
    /// can try is a tool named after the store, and this is what it gets.
    #[test]
    fn a_bearers_tool_name_cannot_reach_the_agent_profile_store() {
        let state = ServerState::new("mcp-agent-profiles".to_string());
        let owner = owner("mcp-profile-user", "mcp-profile-client");
        // The caller is a live local session: the tool door resolves every
        // bearer to its registry row, and a rowless registration is refused.
        crate::session::insert_test_live_agent(&state.sessions, "session", owner.clone());
        let guard = state
            .mcp
            .register("session", &owner, &SessionKind::Acp)
            .expect("registration")
            .expect("MCP guard");
        let token = state.mcp.test_token("session").expect("token");
        let server = state.mcp.start(&state).expect("MCP server");

        // What is forbidden, exactly: any name in the bearer's closed table
        // that carries the profile-store vocabulary, because a tool name is a
        // bearer's only channel into the daemon — the store's own RPCs
        // (`AgentProfilesGet`/`AgentProfilesSet`) travel a different surface
        // and cannot be reached from here. Two deliberate exceptions, each
        // with its own authority, and nothing else: `devboule_list_profiles`,
        // the read-only ticked list the design serves; and
        // `devboule_set_agent_profile`, which reads the store through the same
        // resolver the create tool uses and writes only a child's own row —
        // its authority is the `created_by` link, never the name. Every other
        // `profile` spelling must fail this assertion, including one-letter
        // neighbours of the allowed names such as `devboule_agent_profile_get`,
        // which a substring deny on `agent_profiles`/`set_profile` used to
        // wave through.
        for (name, _) in crate::provider_catalog::MCP_BROKER_TOOLS {
            assert!(
                name == &crate::provider_catalog::MCP_LIST_PROFILES_TOOL
                    || name == &crate::provider_catalog::MCP_SET_AGENT_PROFILE_TOOL
                    || !name.contains("profile"),
                "the broker's closed table must not reach the profile store: \
                 {name} carries the profile vocabulary and is neither of the two \
                 allowed tools (the read-only list, the created_by-gated move)"
            );
        }

        for (index, name) in [
            "devboule_agent_profiles",
            "devboule_set_profile",
            "agent_profiles_set",
            "devboule_agent_profile_get",
        ]
        .iter()
        .enumerate()
        {
            let response = http_request(
                &state.mcp.url,
                Some(&format!("Bearer {token}")),
                &format!(
                    r#"{{"jsonrpc":"2.0","id":{},"method":"tools/call","params":{{"name":"{name}"}}}}"#,
                    index + 1
                ),
            );
            let body = response_json(&response);
            assert_eq!(body["error"]["code"], -32601, "{name}: {body}");
            assert_eq!(body["error"]["message"], "Unknown tool", "{name}: {body}");
        }

        // The delegation switch is refused the same way, on the same grounds:
        // its RPCs (`DelegationGet`/`DelegationSet`) travel the app's wire and
        // cannot be reached from a bearer, so the only attack is a tool named
        // after it. `delegat` catches every delegation spelling; `grant`
        // catches the vocabulary a per-session or per-creator allow would
        // reach for, and this slice deliberately has no tool by that name —
        // the only thing that answers a card is `devboule_answer_permission`,
        // which matches neither word because it reads the pending table, not
        // the switch.
        for (name, _) in crate::provider_catalog::MCP_BROKER_TOOLS {
            assert!(
                !name.contains("delegat") && !name.contains("grant"),
                "the broker's closed table must not reach the delegation switch: \
                 {name} carries the switch's vocabulary"
            );
        }

        drop(guard);
        drop(server);
    }

    /// C8, at the two doors an agent's answer has: the schema the model
    /// reads offers `allow_once` and `deny` and nothing else, and the arm
    /// itself refuses any other outcome string before it looks at a card.
    #[test]
    fn the_answer_tool_offers_allow_once_or_deny_and_nothing_else() {
        let state = ServerState::new("mcp-answer-c8".to_string());
        let owner = owner("mcp-answer-user", "mcp-answer-client");
        // The caller is a live local session: the tool door resolves every
        // bearer to its registry row, and a rowless registration is refused.
        crate::session::insert_test_live_agent(&state.sessions, "session", owner.clone());
        let guard = state
            .mcp
            .register("session", &owner, &SessionKind::Acp)
            .expect("registration")
            .expect("MCP guard");
        let token = state.mcp.test_token("session").expect("token");
        let server = state.mcp.start(&state).expect("MCP server");

        let response = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        );
        let body = response_json(&response);
        let schema = body["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .find(|tool| tool["name"] == crate::provider_catalog::MCP_ANSWER_PERMISSION_TOOL)
            .expect("the answer tool is listed")["inputSchema"]
            .clone();
        assert_eq!(
            schema["properties"]["outcome"]["enum"],
            serde_json::json!(["allow_once", "deny"]),
            "the closed outcome table, at the schema: {schema}"
        );

        // The arm refuses a durable allow before any card is consulted:
        // allow_always is not representable from an agent, ever.
        for (id, arguments, why) in [
            (
                2,
                r#"{"cardId":"card-1","outcome":"allow_always"}"#,
                "allow_always must be refused at the door",
            ),
            (
                3,
                r#"{"cardId":"card-1"}"#,
                "a missing outcome is the caller's mistake",
            ),
            (
                4,
                r#"{"outcome":"deny"}"#,
                "a missing cardId is the caller's mistake",
            ),
        ] {
            let message = format!(
                r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"devboule_answer_permission","arguments":{arguments}}}}}"#
            );
            let response = http_request(&state.mcp.url, Some(&format!("Bearer {token}")), &message);
            let body = response_json(&response);
            assert_eq!(body["error"]["code"], -32602, "{why}: {body}");
            assert!(
                !serde_json::to_string(&body).unwrap().contains("pending"),
                "{why}: the refusal says nothing about any card: {body}"
            );
        }

        drop(guard);
        drop(server);
    }

    /// C12, on the answer side: a delegated answer audited with its actor
    /// session, and a refused answer audited as denied. The rows name the
    /// actor, never the card's contents.
    #[test]
    fn an_answer_through_the_tool_is_audited_with_its_actor() {
        let state = ServerState::new("mcp-answer-audit".to_string());
        let owner = owner("mcp-answer-audit-user", "mcp-answer-client");
        // The state's own store: the registry already holds it, attached at
        // construction, and the OnceLock keeps the first.
        let store = Arc::clone(&state.delegation);
        store.set(true).expect("set on");

        let creator = "s.creator.1".to_string();
        let child = "s.creator.1.child".to_string();
        crate::session::insert_test_live_agent(&state.sessions, &creator, owner.clone());
        state
            .sessions
            .insert_test_child(&child, owner.clone(), &creator);

        let guard = state
            .mcp
            .register(&creator, &owner, &SessionKind::Acp)
            .expect("registration")
            .expect("MCP guard");
        let token = state.mcp.test_token(&creator).expect("token");
        let server = state.mcp.start(&state).expect("MCP server");
        let runtime = Arc::new(crate::session::SessionRuntime::new());
        runtime.require_mcp();
        state.mcp.bind_runtime(&creator, &runtime);
        state.sessions.test_park_card(&child, "card-audit");

        let message = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_answer_permission","arguments":{"cardId":"card-audit","outcome":"deny"}}}"#;
        let response = http_request(&state.mcp.url, Some(&format!("Bearer {token}")), message);
        let body = response_json(&response);
        assert_eq!(body["result"]["isError"], false, "{body}");

        // The refusal side: with the switch off, the same answer is audited
        // as denied — and the card stays pending for the human.
        store.set(false).expect("set off");
        state.sessions.test_park_card(&child, "card-off");
        let message = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"devboule_answer_permission","arguments":{"cardId":"card-off","outcome":"deny"}}}"#;
        let response = http_request(&state.mcp.url, Some(&format!("Bearer {token}")), message);
        let body = response_json(&response);
        assert_eq!(body["result"]["isError"], true, "{body}");

        let runtime_dir = state.sessions.runtime_dir().to_path_buf();
        let connection =
            rusqlite::Connection::open(runtime_dir.join("journal.db")).expect("journal db");
        let mut statement = connection
            .prepare("SELECT action, session_id, outcome FROM audit ORDER BY id")
            .expect("prepare");
        let rows: Vec<(String, Option<String>, String)> = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .expect("query")
            .map(Result::unwrap)
            .collect();
        assert!(
            rows.contains(&(
                "devboule_answer_permission".to_string(),
                Some(creator.clone()),
                "ok".to_string()
            )),
            "the accepted answer names its actor session: {rows:?}"
        );
        assert!(
            rows.contains(&(
                "devboule_answer_permission".to_string(),
                Some(creator.clone()),
                "denied".to_string()
            )),
            "the refused answer is audited as denied: {rows:?}"
        );

        drop(guard);
        drop(server);
    }

    // -----------------------------------------------------------------------
    // Pass A: `devboule_set_agent_profile` — a creator moves its own live
    // child onto a ticked profile (slice 5b §2).
    // -----------------------------------------------------------------------

    /// The move tool is listed with the closed schema it documents, and the
    /// arm demands both arguments at the door.
    #[test]
    fn the_move_tool_is_listed_with_a_closed_schema_and_demands_both_arguments() {
        let state = ServerState::new("mcp-move-schema".to_string());
        let owner = owner("mcp-move-user", "mcp-move-client");
        // The caller is a live local session: the tool door resolves every
        // bearer to its registry row, and a rowless registration is refused.
        crate::session::insert_test_live_agent(&state.sessions, "session", owner.clone());
        let guard = state
            .mcp
            .register("session", &owner, &SessionKind::Acp)
            .expect("registration")
            .expect("MCP guard");
        let token = state.mcp.test_token("session").expect("token");
        let server = state.mcp.start(&state).expect("MCP server");

        let response = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        );
        let body = response_json(&response);
        let schema = body["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .find(|tool| tool["name"] == crate::provider_catalog::MCP_SET_AGENT_PROFILE_TOOL)
            .expect("the move tool is listed")["inputSchema"]
            .clone();
        assert_eq!(
            schema["required"],
            serde_json::json!(["session", "profile"]),
            "both arguments, required: {schema}"
        );
        assert_eq!(schema["additionalProperties"], false, "{schema}");
        assert_eq!(schema["properties"]["session"]["type"], "string");
        assert_eq!(schema["properties"]["profile"]["type"], "string");

        // Missing arguments are the caller's mistake, refused at the door.
        for (id, arguments, why) in [
            (2, r#"{}"#, "neither argument arrived"),
            (
                3,
                r#"{"session":"child-1"}"#,
                "a missing profile is the caller's mistake",
            ),
            (
                4,
                r#"{"profile":"Solo"}"#,
                "a missing session is the caller's mistake",
            ),
        ] {
            let message = format!(
                r#"{{"jsonrpc":"2.0","id":{id},"method":"tools/call","params":{{"name":"devboule_set_agent_profile","arguments":{arguments}}}}}"#
            );
            let response = http_request(&state.mcp.url, Some(&format!("Bearer {token}")), &message);
            let body = response_json(&response);
            assert_eq!(body["error"]["code"], -32602, "{why}: {body}");
            assert_eq!(
                body["error"]["message"], "session and profile are required",
                "{why}: {body}"
            );
        }

        drop(guard);
        drop(server);
    }

    /// §1.2, at the move surface: unknown, ambiguous and unticked are three
    /// refusals with three sentences — and the tick is read at the moment of
    /// the call, never from a list cached earlier.
    #[test]
    fn the_move_resolver_distinguishes_unknown_ambiguous_and_unticked_and_reads_now() {
        let store = profile_store(document(
            vec![
                profile(
                    "Solo",
                    "p-1",
                    "claude",
                    "default",
                    serde_json::json!({}),
                    &[],
                    true,
                ),
                profile(
                    "Ticked off",
                    "p-2",
                    "claude",
                    "default",
                    serde_json::json!({}),
                    &[],
                    false,
                ),
                profile(
                    "Dup",
                    "p-3",
                    "claude",
                    "default",
                    serde_json::json!({}),
                    &[],
                    true,
                ),
                profile(
                    "Dup",
                    "p-4",
                    "claude",
                    "default",
                    serde_json::json!({}),
                    &[],
                    true,
                ),
            ],
            "",
        ));
        let facts = resolve_profile_for_move(&store, "Solo").expect("ticked");
        assert_eq!(facts.profile_id, "p-1");
        assert_eq!(facts.mode_id, "default");
        assert_eq!(facts.thinking_option_id.as_deref(), Some("high"));

        let error = resolve_profile_for_move(&store, "Ghost").expect_err("unknown");
        assert!(error.contains("unknown profile"), "{error}");

        let error = resolve_profile_for_move(&store, "Ticked off").expect_err("unticked");
        assert!(
            error.contains("has not enabled it for agents"),
            "unticked is its own sentence, not unknown's: {error}"
        );

        let error = resolve_profile_for_move(&store, "Dup").expect_err("ambiguous");
        assert!(error.contains("more than one profile is called"), "{error}");

        // The read-now rule: un-tick Solo and the next ask is refused unticked.
        // A resolver that cached the ticked list would still answer Ok here.
        store
            .set(
                serde_json::from_value(document(
                    vec![profile(
                        "Solo",
                        "p-1",
                        "claude",
                        "default",
                        serde_json::json!({}),
                        &[],
                        false,
                    )],
                    "",
                ))
                .expect("the document"),
            )
            .expect("the un-ticked document is admitted");
        let error = resolve_profile_for_move(&store, "Solo").expect_err("read at the call");
        assert!(error.contains("has not enabled it for agents"), "{error}");
    }

    /// Pass A's audit: a move through the tool names its actor session, and a
    /// refused move is audited as denied — the answer arm's shape, on the move
    /// surface.
    #[test]
    fn a_move_through_the_tool_is_audited_with_its_actor() {
        let state = ServerState::new("mcp-move-audit".to_string());
        let owner = owner("mcp-move-audit-user", "mcp-move-client");
        state
            .agent_profiles
            .set(
                serde_json::from_value(document(
                    vec![profile(
                        "Solo",
                        "profile-solo",
                        "claude",
                        "bypassPermissions",
                        serde_json::json!({}),
                        &[],
                        true,
                    )],
                    "",
                ))
                .expect("the document"),
            )
            .expect("the store admits this document");
        let creator = "s.mover.1".to_string();
        let child = "s.mover.1.child".to_string();
        crate::session::insert_test_live_agent(&state.sessions, &creator, owner.clone());
        state.sessions.insert_test_move_child(
            &child,
            owner.clone(),
            &creator,
            "Worker",
            &["bypassPermissions"],
            Some("model-a"),
            false,
        );

        let guard = state
            .mcp
            .register(&creator, &owner, &SessionKind::Acp)
            .expect("registration")
            .expect("MCP guard");
        let token = state.mcp.test_token(&creator).expect("token");
        let server = state.mcp.start(&state).expect("MCP server");

        let message = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_set_agent_profile","arguments":{"session":"Worker","profile":"Solo"}}}"#;
        let response = http_request(&state.mcp.url, Some(&format!("Bearer {token}")), message);
        let body = response_json(&response);
        assert_eq!(body["result"]["isError"], false, "{body}");
        assert_eq!(
            body["result"]["structuredContent"]["state"], "moved",
            "{body}"
        );

        // The refusal side: an unknown profile is refused, the child untouched,
        // and the refusal is audited as denied.
        let message = r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"devboule_set_agent_profile","arguments":{"session":"Worker","profile":"Ghost"}}}"#;
        let response = http_request(&state.mcp.url, Some(&format!("Bearer {token}")), message);
        let body = response_json(&response);
        assert_eq!(body["result"]["isError"], true, "{body}");

        let runtime_dir = state.sessions.runtime_dir().to_path_buf();
        let connection =
            rusqlite::Connection::open(runtime_dir.join("journal.db")).expect("journal db");
        let mut statement = connection
            .prepare("SELECT action, session_id, outcome FROM audit ORDER BY id")
            .expect("prepare");
        let rows: Vec<(String, Option<String>, String)> = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .expect("query")
            .map(Result::unwrap)
            .collect();
        assert!(
            rows.contains(&(
                crate::provider_catalog::MCP_SET_AGENT_PROFILE_TOOL.to_string(),
                Some(creator.clone()),
                "ok".to_string()
            )),
            "the move names its actor session: {rows:?}"
        );
        assert!(
            rows.contains(&(
                crate::provider_catalog::MCP_SET_AGENT_PROFILE_TOOL.to_string(),
                Some(creator.clone()),
                "denied".to_string()
            )),
            "the refused move is audited as denied: {rows:?}"
        );

        drop(guard);
        drop(server);
    }

    // ------------------------------------------------------------------
    // P0 — the broker asks where its caller came from, once, at the door.
    // ------------------------------------------------------------------

    /// A `peers` row the door's capability reads can see.
    fn peer_row(device_id: &str, caps: &[&str]) -> crate::journal::PeerRecord {
        crate::journal::PeerRecord {
            device_id: device_id.to_string(),
            display_name: "Peer".to_string(),
            role: "client".to_string(),
            public_key: vec![7u8; 32],
            paired_by_user: None,
            binding_kind: "tailnet".to_string(),
            binding_stable_id: Some("npeer".to_string()),
            binding_node_name: None,
            binding_login_name: None,
            address: "100.64.0.2:47831".to_string(),
            paired_at: 1,
            revoked_at: None,
            caps: caps.iter().map(|cap| cap.to_string()).collect(),
        }
    }

    /// The door is a no-op for the person at this machine: every served tool
    /// passes, and unknown names fall through to the broker's own arm. This is
    /// the local case that must not regress — same behaviour, same sentences.
    #[test]
    fn local_callers_pass_the_door_for_every_tool() {
        let caller = McpCaller::Local;
        for (name, _) in crate::provider_catalog::MCP_BROKER_TOOLS {
            assert!(
                mcp_peer_door(&caller, Some(name), &json!(1)).is_none(),
                "{name}: a local caller is never judged"
            );
        }
        assert!(mcp_peer_door(&caller, Some("devboule_no_such_tool"), &json!(1)).is_none());
        assert!(mcp_peer_door(&caller, None, &json!(1)).is_none());
    }

    /// The discovery tool answers from this daemon's own rows, scoped to the
    /// calling session's own user. Its schema lives in its own
    /// `enabled_tool_list` arm, and the assertions below pin the shape that
    /// arm claims, so a default-arm change cannot silently reshape it.
    #[test]
    fn the_devices_tool_answers_scoped_from_this_daemons_rows() {
        let state = ServerState::new("mcp-devices-tool".to_string());
        let owner = owner("S-1-5-21-devtool", "mcp-devices-client");
        crate::session::insert_test_live_agent(&state.sessions, "session", owner.clone());
        state
            .peer_upsert(crate::journal::PeerRecord {
                device_id: "dev-mine".to_string(),
                display_name: "Work laptop".to_string(),
                role: "daemon".to_string(),
                public_key: vec![7u8; 32],
                paired_by_user: Some(owner.user.clone()),
                binding_kind: "tailnet".to_string(),
                binding_stable_id: Some("nstable".to_string()),
                binding_node_name: None,
                binding_login_name: None,
                address: "100.64.0.2:47831".to_string(),
                paired_at: 1,
                revoked_at: None,
                caps: vec![crate::peer_policy::CAP_VIEW.to_string()],
            })
            .expect("peer row");
        let guard = state
            .mcp
            .register("session", &owner, &SessionKind::Acp)
            .expect("registration")
            .expect("MCP guard");
        let token = state.mcp.test_token("session").expect("token");
        let server = state.mcp.start(&state).expect("MCP server");

        let listed = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        );
        let listed_body = response_json(&listed);
        let tool = listed_body["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .find(|tool| tool["name"] == crate::provider_catalog::MCP_LIST_DEVICES_TOOL)
            .expect("the devices tool is listed")
            .clone();
        assert_eq!(tool["inputSchema"]["type"], "object");
        assert_eq!(
            tool["inputSchema"]["properties"],
            serde_json::json!({}),
            "the tool takes no arguments, and the schema says so"
        );

        let call = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"devboule_list_devices","arguments":{}}}"#,
        );
        let body = response_json(&call);
        assert_eq!(body["result"]["isError"], false, "{body}");
        let devices = body["result"]["structuredContent"]["devices"]
            .as_array()
            .expect("devices array");
        assert_eq!(devices.len(), 1, "{body}");
        assert_eq!(devices[0]["deviceId"], "dev-mine");
        assert_eq!(devices[0]["displayName"], "Work laptop");
        assert_eq!(devices[0]["role"], "daemon");
        assert_eq!(devices[0]["online"], false);

        drop(guard);
        drop(server);
    }

    /// The one-dial roster tool, over the real broker: its schema demands
    /// `deviceId`, a device outside the calling session's own rows refuses
    /// by name — absent is never an empty roster — and the refusal carries a
    /// sentence, not a debug string.
    #[test]
    fn the_peer_agents_tool_refuses_an_unknown_device_by_name() {
        let state = ServerState::new("mcp-peer-agents".to_string());
        let owner = owner("S-1-5-21-peeragents", "mcp-peer-agents-client");
        crate::session::insert_test_live_agent(&state.sessions, "session", owner.clone());
        let guard = state
            .mcp
            .register("session", &owner, &SessionKind::Acp)
            .expect("registration")
            .expect("MCP guard");
        let token = state.mcp.test_token("session").expect("token");
        let server = state.mcp.start(&state).expect("MCP server");

        let listed = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#,
        );
        let listed_body = response_json(&listed);
        let tool = listed_body["result"]["tools"]
            .as_array()
            .expect("tools")
            .iter()
            .find(|tool| tool["name"] == crate::provider_catalog::MCP_LIST_PEER_AGENTS_TOOL)
            .expect("the peer agents tool is listed")
            .clone();
        assert_eq!(
            tool["inputSchema"]["required"],
            serde_json::json!(["deviceId"])
        );

        let missing = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"devboule_list_peer_agents","arguments":{}}}"#,
        );
        assert_eq!(
            response_json(&missing).pointer("/error/message"),
            Some(&json!("deviceId is required"))
        );

        let unknown = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"devboule_list_peer_agents","arguments":{"deviceId":"dev-nowhere"}}}"#,
        );
        let body = response_json(&unknown);
        assert_eq!(body.pointer("/error/code"), Some(&json!(-32602)), "{body}");
        let sentence = body
            .pointer("/error/message")
            .and_then(Value::as_str)
            .expect("a sentence");
        assert!(
            sentence.contains("No paired device named 'dev-nowhere'"),
            "{sentence}"
        );
        assert!(
            sentence.contains("devboule_list_devices"),
            "the sentence names the discovery tool: {sentence}"
        );

        // The refused call names its actor in the audit table: this is the
        // tool that dials other machines, so even its refusals are facts.
        let runtime_dir = state.sessions.runtime_dir().to_path_buf();
        let connection =
            rusqlite::Connection::open(runtime_dir.join("journal.db")).expect("journal db");
        let mut statement = connection
            .prepare("SELECT action, session_id, outcome FROM audit ORDER BY id")
            .expect("prepare");
        let rows: Vec<(String, Option<String>, String)> = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .expect("query")
            .map(Result::unwrap)
            .collect();
        assert!(
            rows.contains(&(
                crate::provider_catalog::MCP_LIST_PEER_AGENTS_TOOL.to_string(),
                Some("session".to_string()),
                "denied".to_string()
            )),
            "the refused roster call is audited as denied: {rows:?}"
        );

        drop(guard);
        drop(server);
        let _ = std::fs::remove_dir_all(runtime_dir);
    }

    /// A roster call that goes out is audited with its actor: the tool opens
    /// an outbound connection to another machine and comes back with that
    /// machine's roster, which is exactly what the audit table exists to
    /// remember.
    #[test]
    fn a_roster_call_that_dials_is_audited_with_its_actor() {
        let keypair = snow::Builder::new(
            crate::peer_transport::PEER_NOISE_PATTERN
                .parse()
                .expect("pattern"),
        )
        .generate_keypair()
        .expect("keypair");
        let canned = devboule_protocol::DaemonMessage::PeerAgents {
            id: 0,
            scope: devboule_protocol::PeerRosterScope::PairingUser,
            agents: vec![devboule_protocol::PeerAgent {
                session_id: "s.far.1".to_string(),
                name: "Builder".to_string(),
                provider: Some("claude".to_string()),
                model: None,
                state: devboule_protocol::AgentTaskState::Working,
                depth: 1,
            }],
        };
        let private: [u8; 32] = keypair.private.clone().try_into().expect("32 bytes");
        let address = crate::test_support::spawn_canned_noise_responder(
            private,
            vec![devboule_protocol::Capability::new(
                devboule_protocol::caps::PEER_AGENTS,
            )],
            canned,
        );

        let state = ServerState::new("mcp-peer-agents-audit".to_string());
        let owner = owner("S-1-5-21-peeragents-audit", "mcp-peer-agents-client");
        crate::session::insert_test_live_agent(&state.sessions, "session", owner.clone());
        state
            .peer_upsert(crate::journal::PeerRecord {
                device_id: "dev-audit".to_string(),
                display_name: "Far daemon".to_string(),
                role: "daemon".to_string(),
                public_key: keypair.public.clone(),
                paired_by_user: Some(owner.user.clone()),
                binding_kind: "tailnet".to_string(),
                binding_stable_id: None,
                binding_node_name: None,
                binding_login_name: None,
                address: address.to_string(),
                paired_at: 1,
                revoked_at: None,
                caps: vec![crate::peer_policy::CAP_ROSTER.to_string()],
            })
            .expect("peer row");
        let guard = state
            .mcp
            .register("session", &owner, &SessionKind::Acp)
            .expect("registration")
            .expect("MCP guard");
        let token = state.mcp.test_token("session").expect("token");
        let server = state.mcp.start(&state).expect("MCP server");

        let call = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_list_peer_agents","arguments":{"deviceId":"dev-audit"}}}"#,
        );
        let body = response_json(&call);
        assert_eq!(body["result"]["isError"], false, "{body}");

        let runtime_dir = state.sessions.runtime_dir().to_path_buf();
        let connection =
            rusqlite::Connection::open(runtime_dir.join("journal.db")).expect("journal db");
        let mut statement = connection
            .prepare("SELECT action, session_id, outcome FROM audit ORDER BY id")
            .expect("prepare");
        let rows: Vec<(String, Option<String>, String)> = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .expect("query")
            .map(Result::unwrap)
            .collect();
        assert!(
            rows.contains(&(
                crate::provider_catalog::MCP_LIST_PEER_AGENTS_TOOL.to_string(),
                Some("session".to_string()),
                "ok".to_string()
            )),
            "the dialled roster call is audited as ok with its actor: {rows:?}"
        );

        drop(guard);
        drop(server);
        let _ = std::fs::remove_dir_all(runtime_dir);
    }

    /// The far side's scope refusal keeps its word in this machine's trail
    /// too: a canned `unscoped` answer is audited `unscoped`, never
    /// `denied` and never `failed` — "the device declined to scope its
    /// roster to me" reads differently from a dead dial.
    #[test]
    fn a_far_side_scope_refusal_is_audited_as_unscoped() {
        let keypair = snow::Builder::new(
            crate::peer_transport::PEER_NOISE_PATTERN
                .parse()
                .expect("pattern"),
        )
        .generate_keypair()
        .expect("keypair");
        let canned = devboule_protocol::DaemonMessage::PeerAgents {
            id: 0,
            scope: devboule_protocol::PeerRosterScope::Unscoped,
            agents: Vec::new(),
        };
        let private: [u8; 32] = keypair.private.clone().try_into().expect("32 bytes");
        let address = crate::test_support::spawn_canned_noise_responder(
            private,
            vec![devboule_protocol::Capability::new(
                devboule_protocol::caps::PEER_AGENTS,
            )],
            canned,
        );

        let state = ServerState::new("mcp-peer-agents-far-unscoped".to_string());
        let owner = owner("S-1-5-21-peeragents-far-unscoped", "mcp-peer-agents-client");
        crate::session::insert_test_live_agent(&state.sessions, "session", owner.clone());
        state
            .peer_upsert(crate::journal::PeerRecord {
                device_id: "dev-far".to_string(),
                display_name: "Far daemon".to_string(),
                role: "daemon".to_string(),
                public_key: keypair.public.clone(),
                paired_by_user: Some(owner.user.clone()),
                binding_kind: "tailnet".to_string(),
                binding_stable_id: None,
                binding_node_name: None,
                binding_login_name: None,
                address: address.to_string(),
                paired_at: 1,
                revoked_at: None,
                caps: vec![crate::peer_policy::CAP_ROSTER.to_string()],
            })
            .expect("peer row");
        let guard = state
            .mcp
            .register("session", &owner, &SessionKind::Acp)
            .expect("registration")
            .expect("MCP guard");
        let token = state.mcp.test_token("session").expect("token");
        let server = state.mcp.start(&state).expect("MCP server");

        let call = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_list_peer_agents","arguments":{"deviceId":"dev-far"}}}"#,
        );
        let body = response_json(&call);
        assert_eq!(body.pointer("/error/code"), Some(&json!(-32602)), "{body}");
        assert!(
            body.pointer("/error/message")
                .and_then(Value::as_str)
                .is_some_and(|message| message.contains("cannot scope its roster")),
            "{body}"
        );

        let runtime_dir = state.sessions.runtime_dir().to_path_buf();
        let connection =
            rusqlite::Connection::open(runtime_dir.join("journal.db")).expect("journal db");
        let mut statement = connection
            .prepare("SELECT action, session_id, outcome FROM audit ORDER BY id")
            .expect("prepare");
        let rows: Vec<(String, Option<String>, String)> = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .expect("query")
            .map(Result::unwrap)
            .collect();
        assert!(
            rows.contains(&(
                crate::provider_catalog::MCP_LIST_PEER_AGENTS_TOOL.to_string(),
                Some("session".to_string()),
                "unscoped".to_string()
            )),
            "the scope refusal is audited as unscoped: {rows:?}"
        );

        drop(guard);
        drop(server);
        let _ = std::fs::remove_dir_all(runtime_dir);
    }

    /// A dial that goes nowhere is a failure, not a refusal: the closed
    /// loopback port refuses immediately, so the test pays no connect
    /// timeout, and the trail says `failed`.
    #[test]
    fn a_dial_that_goes_nowhere_is_audited_as_failed() {
        let state = ServerState::new("mcp-peer-agents-dial-failed".to_string());
        let owner = owner("S-1-5-21-peeragents-dial-failed", "mcp-peer-agents-client");
        crate::session::insert_test_live_agent(&state.sessions, "session", owner.clone());
        state
            .peer_upsert(crate::journal::PeerRecord {
                device_id: "dev-asleep".to_string(),
                display_name: "Sleeping daemon".to_string(),
                role: "daemon".to_string(),
                public_key: vec![7u8; 32],
                paired_by_user: Some(owner.user.clone()),
                binding_kind: "tailnet".to_string(),
                binding_stable_id: None,
                binding_node_name: None,
                binding_login_name: None,
                address: "127.0.0.1:1".to_string(),
                paired_at: 1,
                revoked_at: None,
                caps: vec![crate::peer_policy::CAP_ROSTER.to_string()],
            })
            .expect("peer row");
        let guard = state
            .mcp
            .register("session", &owner, &SessionKind::Acp)
            .expect("registration")
            .expect("MCP guard");
        let token = state.mcp.test_token("session").expect("token");
        let server = state.mcp.start(&state).expect("MCP server");

        let call = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_list_peer_agents","arguments":{"deviceId":"dev-asleep"}}}"#,
        );
        let body = response_json(&call);
        assert_eq!(body.pointer("/error/code"), Some(&json!(-32602)), "{body}");

        let runtime_dir = state.sessions.runtime_dir().to_path_buf();
        let connection =
            rusqlite::Connection::open(runtime_dir.join("journal.db")).expect("journal db");
        let mut statement = connection
            .prepare("SELECT action, session_id, outcome FROM audit ORDER BY id")
            .expect("prepare");
        let rows: Vec<(String, Option<String>, String)> = statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .expect("query")
            .map(Result::unwrap)
            .collect();
        assert!(
            rows.contains(&(
                crate::provider_catalog::MCP_LIST_PEER_AGENTS_TOOL.to_string(),
                Some("session".to_string()),
                "failed".to_string()
            )),
            "the dead dial is audited as failed, never denied: {rows:?}"
        );

        drop(guard);
        drop(server);
        let _ = std::fs::remove_dir_all(runtime_dir);
    }

    /// A bearer with no readable row is refused, but retryably: an agent's first
    /// call can land before its own commit, and a reaped session's in-flight
    /// calls outlive its row, and the ecosystem retries exactly this sentence.
    /// Still a refusal — never the local person's answer. (This is why the older
    /// tests above register their callers as live local sessions first.)
    #[test]
    fn a_caller_without_a_row_is_refused_as_absent() {
        let state = ServerState::new("mcp-p0-norow".to_string());
        let owner = owner("mcp-p0-norow-user", "mcp-p0-norow-client");
        let guard = state
            .mcp
            .register("ghost", &owner, &SessionKind::Acp)
            .expect("registration")
            .expect("MCP guard");
        let token = state.mcp.test_token("ghost").expect("token");
        let server = state.mcp.start(&state).expect("MCP server");
        let response = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_list_agents"}}"#,
        );
        let body = response_json(&response);
        assert_eq!(body.pointer("/error/code"), Some(&json!(-32601)));
        assert_eq!(
            body.pointer("/error/message"),
            Some(&json!("No session with that id.")),
            "absent is a refusal, and a retryable one: {body}"
        );
        drop(guard);
        drop(server);
    }

    /// A stored `Unknown` origin is not an absence: it never resolves, so the
    /// refusal is hard rather than retryable.
    #[test]
    fn a_caller_with_an_unknown_origin_is_refused_hard() {
        let state = ServerState::new("mcp-p0-unknown".to_string());
        let owner = owner("mcp-p0-unknown-user", "mcp-p0-unknown-client");
        let creator = "s.unknown.1".to_string();
        crate::session::insert_test_live_agent(&state.sessions, &creator, owner.clone());
        state
            .sessions
            .set_test_origin(&creator, SessionOrigin::unknown());
        let guard = state
            .mcp
            .register(&creator, &owner, &SessionKind::Acp)
            .expect("registration")
            .expect("MCP guard");
        let token = state.mcp.test_token(&creator).expect("token");
        let server = state.mcp.start(&state).expect("MCP server");
        let response = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_list_agents"}}"#,
        );
        let body = response_json(&response);
        assert_eq!(body.pointer("/error/code"), Some(&json!(-32601)));
        assert!(
            body.pointer("/error/message")
                .and_then(Value::as_str)
                .is_some_and(|message| message.contains("origin is unknown")),
            "a stored unknown never renders as the benign one: {body}"
        );
        drop(guard);
        drop(server);
    }

    /// A peer-origin caller is refused the profile move with the policy's own
    /// model sentence — even holding every capability — and the child is
    /// untouched: the door returns before the move's checks run, so no mode
    /// ask lands, no model ask lands, and no profile change is recorded.
    #[test]
    fn a_peer_caller_is_refused_the_model_half_with_the_policy_sentence() {
        let state = ServerState::new("mcp-p0-peer-model".to_string());
        let owner = owner("mcp-p0-model-user", "mcp-p0-model-client");
        state
            .agent_profiles
            .set(
                serde_json::from_value(document(
                    vec![profile(
                        "Solo",
                        "profile-solo",
                        "claude",
                        "bypassPermissions",
                        serde_json::json!({}),
                        &[],
                        true,
                    )],
                    "",
                ))
                .expect("the document"),
            )
            .expect("the store admits this document");
        let creator = "s.peer.1".to_string();
        let child = "s.peer.1.child".to_string();
        crate::session::insert_test_live_agent(&state.sessions, &creator, owner.clone());
        state.sessions.insert_test_move_child(
            &child,
            owner.clone(),
            &creator,
            "Worker",
            &["bypassPermissions"],
            Some("model-a"),
            false,
        );
        state.sessions.set_test_origin(
            &creator,
            SessionOrigin::peer("device-p0", crate::peer_policy::PeerRole::Client),
        );
        state
            .peer_upsert(peer_row(
                "device-p0",
                &["view", "send", "answer_permissions", "create_sessions"],
            ))
            .expect("store a peer");
        let guard = state
            .mcp
            .register(&creator, &owner, &SessionKind::Acp)
            .expect("registration")
            .expect("MCP guard");
        let token = state.mcp.test_token(&creator).expect("token");
        let server = state.mcp.start(&state).expect("MCP server");
        let response = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_set_agent_profile","arguments":{"session":"Worker","profile":"Solo"}}}"#,
        );
        let body = response_json(&response);
        assert_eq!(body.pointer("/error/code"), Some(&json!(-32601)));
        assert_eq!(
            body.pointer("/error/message"),
            Some(&json!("capability 'session.set_model' was not negotiated")),
            "the policy's own sentence: {body}"
        );
        drop(guard);
        drop(server);
    }

    /// Without `send` the same call is refused one half earlier, with that
    /// half's own sentence: the two refusals must not read the same.
    #[test]
    fn a_peer_without_send_is_refused_the_mode_half_first() {
        let state = ServerState::new("mcp-p0-peer-mode".to_string());
        let owner = owner("mcp-p0-mode-user", "mcp-p0-mode-client");
        let creator = "s.peer.2".to_string();
        crate::session::insert_test_live_agent(&state.sessions, &creator, owner.clone());
        state.sessions.set_test_origin(
            &creator,
            SessionOrigin::peer("device-p0-mode", crate::peer_policy::PeerRole::Client),
        );
        state
            .peer_upsert(peer_row("device-p0-mode", &["view"]))
            .expect("store a peer");
        let guard = state
            .mcp
            .register(&creator, &owner, &SessionKind::Acp)
            .expect("registration")
            .expect("MCP guard");
        let token = state.mcp.test_token(&creator).expect("token");
        let server = state.mcp.start(&state).expect("MCP server");
        let response = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_set_agent_profile","arguments":{"session":"Worker","profile":"Solo"}}}"#,
        );
        let body = response_json(&response);
        assert_eq!(
            body.pointer("/error/message"),
            Some(&json!("capability 'send' was not negotiated")),
            "the mode half fires first: {body}"
        );
        drop(guard);
        drop(server);
    }

    /// The audit row for a peer-origin caller names the device, not `"local"`.
    #[test]
    fn a_peer_denial_is_audited_under_the_device_not_local() {
        let state = ServerState::new("mcp-p0-audit".to_string());
        let owner = owner("mcp-p0-audit-user", "mcp-p0-audit-client");
        let store = Arc::clone(&state.delegation);
        store.set(true).expect("set on");
        let creator = "s.peer.3".to_string();
        let child = "s.peer.3.child".to_string();
        crate::session::insert_test_live_agent(&state.sessions, &creator, owner.clone());
        state
            .sessions
            .insert_test_child(&child, owner.clone(), &creator);
        state.sessions.set_test_origin(
            &creator,
            SessionOrigin::peer("device-p0-audit", crate::peer_policy::PeerRole::Client),
        );
        // No `answer_permissions` on the row: the door refuses.
        state
            .peer_upsert(peer_row("device-p0-audit", &["view"]))
            .expect("store a peer");
        let guard = state
            .mcp
            .register(&creator, &owner, &SessionKind::Acp)
            .expect("registration")
            .expect("MCP guard");
        let token = state.mcp.test_token(&creator).expect("token");
        let server = state.mcp.start(&state).expect("MCP server");
        let runtime = Arc::new(crate::session::SessionRuntime::new());
        runtime.require_mcp();
        state.mcp.bind_runtime(&creator, &runtime);
        state.sessions.test_park_card(&child, "card-p0");
        let response = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_answer_permission","arguments":{"cardId":"card-p0","outcome":"deny"}}}"#,
        );
        let body = response_json(&response);
        assert_eq!(body.pointer("/error/code"), Some(&json!(-32601)));
        assert!(
            body.pointer("/error/message")
                .and_then(Value::as_str)
                .is_some_and(|message| message.contains("answer_permissions")),
            "{body}"
        );
        let runtime_dir = state.sessions.runtime_dir().to_path_buf();
        let connection =
            rusqlite::Connection::open(runtime_dir.join("journal.db")).expect("journal db");
        let mut statement = connection
            .prepare("SELECT device_id, role, action, session_id, outcome FROM audit ORDER BY id")
            .expect("prepare");
        let rows: Vec<(String, String, String, Option<String>, String)> = statement
            .query_map([], |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                ))
            })
            .expect("query")
            .map(Result::unwrap)
            .collect();
        assert!(
            rows.contains(&(
                "device-p0-audit".to_string(),
                "client".to_string(),
                crate::provider_catalog::MCP_ANSWER_PERMISSION_TOOL.to_string(),
                Some(creator.clone()),
                "denied".to_string()
            )),
            "the log names the device, never local: {rows:?}"
        );
        assert!(
            !rows.iter().any(|(_, role, action, _, _)| role == "local"
                && action == crate::provider_catalog::MCP_ANSWER_PERMISSION_TOOL),
            "no peer denial is ever recorded as local: {rows:?}"
        );
        drop(guard);
        drop(server);
    }

    /// The roster is the `view` act: a peer without it is refused, a peer with
    /// it is answered — and past the door the behaviour is the local behaviour.
    #[test]
    fn a_peer_roster_is_the_view_act() {
        let state = ServerState::new("mcp-p0-roster".to_string());
        let owner = owner("mcp-p0-roster-user", "mcp-p0-roster-client");
        let creator = "s.peer.4".to_string();
        crate::session::insert_test_live_agent(&state.sessions, &creator, owner.clone());
        state.sessions.set_test_origin(
            &creator,
            SessionOrigin::peer("device-p0-roster", crate::peer_policy::PeerRole::Client),
        );
        state
            .peer_upsert(peer_row("device-p0-roster", &[]))
            .expect("store a peer");
        let guard = state
            .mcp
            .register(&creator, &owner, &SessionKind::Acp)
            .expect("registration")
            .expect("MCP guard");
        let token = state.mcp.test_token(&creator).expect("token");
        let server = state.mcp.start(&state).expect("MCP server");
        let refused = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_list_agents"}}"#,
        );
        assert_eq!(
            response_json(&refused).pointer("/error/message"),
            Some(&json!("capability 'view' was not negotiated"))
        );
        state
            .peer_upsert(peer_row("device-p0-roster", &["view"]))
            .expect("grant view");
        let allowed = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"devboule_list_agents"}}"#,
        );
        assert_eq!(response_json(&allowed)["result"]["isError"], false);
        drop(guard);
        drop(server);
    }

    /// The sender is the `send` act: past a refused door the next check reads
    /// exactly what a local caller reads.
    #[test]
    fn a_peer_send_is_the_send_act() {
        let state = ServerState::new("mcp-p0-send".to_string());
        let owner = owner("mcp-p0-send-user", "mcp-p0-send-client");
        let creator = "s.peer.5".to_string();
        crate::session::insert_test_live_agent(&state.sessions, &creator, owner.clone());
        state.sessions.set_test_origin(
            &creator,
            SessionOrigin::peer("device-p0-send", crate::peer_policy::PeerRole::Client),
        );
        state
            .peer_upsert(peer_row("device-p0-send", &["view"]))
            .expect("store a peer");
        let guard = state
            .mcp
            .register(&creator, &owner, &SessionKind::Acp)
            .expect("registration")
            .expect("MCP guard");
        let token = state.mcp.test_token(&creator).expect("token");
        let server = state.mcp.start(&state).expect("MCP server");
        let refused = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_send_message","arguments":{"to_agent":"nobody","text":"hi"}}}"#,
        );
        assert_eq!(
            response_json(&refused).pointer("/error/message"),
            Some(&json!("capability 'send' was not negotiated"))
        );
        state
            .peer_upsert(peer_row("device-p0-send", &["view", "send"]))
            .expect("grant send");
        // Past the door, the missing target reads as it does for a local caller.
        let missing = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"devboule_send_message","arguments":{"to_agent":"nobody","text":"hi"}}}"#,
        );
        assert_eq!(
            response_json(&missing).pointer("/error/message"),
            Some(&json!("target agent not found"))
        );
        drop(guard);
        drop(server);
    }

    /// F4 (MAX RECALL, authority): the tool's act is performed AS the caller.
    /// The send used to act through an unmarked connection, so a peer's
    /// delivery read `local` to the receiving agent (S4-05) and carried the
    /// steer-refusal interrupt authority the wire denies every peer (S4-01).
    /// The connection now carries the caller's resolved identity: the envelope
    /// arrives naming the device, exactly as the wire's own S4-05 test pins.
    #[test]
    fn a_peer_tool_send_is_attributed_to_the_peer() {
        let state = ServerState::new("mcp-f4-send".to_string());
        let owner = owner("S-1-5-21-f4-user", "mcp-f4-client");
        let creator = "s.peer.7".to_string();
        crate::session::insert_test_live_agent(&state.sessions, &creator, owner.clone());
        state.sessions.set_test_origin(
            &creator,
            SessionOrigin::peer("device-f4-send", crate::peer_policy::PeerRole::Client),
        );
        let received = crate::session::insert_test_live_agent_with_recording_writer(
            &state.sessions,
            "s.f4.target",
            owner.clone(),
            SessionKind::Pi,
        );
        let mut row = peer_row("device-f4-send", &["view", "send"]);
        row.paired_by_user = Some("S-1-5-21-f4-user".to_string());
        state.peer_upsert(row).expect("store a peer");
        let guard = state
            .mcp
            .register(&creator, &owner, &SessionKind::Acp)
            .expect("registration")
            .expect("MCP guard");
        let token = state.mcp.test_token(&creator).expect("token");
        let server = state.mcp.start(&state).expect("MCP server");
        let reply = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_send_message","arguments":{"to_agent":"s.f4.target","text":"hello from the peer"}}}"#,
        );
        let body = response_json(&reply);
        assert_eq!(
            body.pointer("/result/isError"),
            Some(&json!(false)),
            "a paired client-role device may message its own user's agents: {body}"
        );
        let envelope = String::from_utf8(received.lock().expect("received").clone())
            .expect("the envelope is utf8");
        assert!(
            envelope.contains("origin: peer:device-f4-send"),
            "the delivery must name the device, not this machine: {envelope}"
        );
        drop(guard);
        drop(server);
    }

    /// The same identity, judged with the wire's own scope (§8 R2): a
    /// `Daemon`-role device's reach is the sessions of its own origin, so its
    /// agent cannot put a message in front of a session it did not create —
    /// including this machine's own. The old unmarked connection passed this
    /// call and misattributed it; now the scope check sees the device.
    #[test]
    fn a_daemon_role_tool_send_beyond_its_origin_is_refused() {
        let state = ServerState::new("mcp-f4-scope".to_string());
        let owner = owner("S-1-5-21-f4d-user", "mcp-f4d-client");
        let creator = "s.peer.8".to_string();
        crate::session::insert_test_live_agent(&state.sessions, &creator, owner.clone());
        state.sessions.set_test_origin(
            &creator,
            SessionOrigin::peer("device-f4-scope", crate::peer_policy::PeerRole::Daemon),
        );
        let received = crate::session::insert_test_live_agent_with_recording_writer(
            &state.sessions,
            "s.f4d.local",
            owner.clone(),
            SessionKind::Pi,
        );
        let mut row = peer_row("device-f4-scope", &["view", "send"]);
        row.paired_by_user = Some("S-1-5-21-f4d-user".to_string());
        state.peer_upsert(row).expect("store a peer");
        let guard = state
            .mcp
            .register(&creator, &owner, &SessionKind::Acp)
            .expect("registration")
            .expect("MCP guard");
        let token = state.mcp.test_token(&creator).expect("token");
        let server = state.mcp.start(&state).expect("MCP server");
        let reply = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_send_message","arguments":{"to_agent":"s.f4d.local","text":"hello from outside my scope"}}}"#,
        );
        let body = response_json(&reply);
        assert_eq!(
            body.pointer("/result/isError"),
            Some(&json!(true)),
            "a daemon-role device cannot reach a session outside its origin: {body}"
        );
        assert!(
            body.pointer("/result/content/0/text")
                .and_then(Value::as_str)
                .is_some_and(|text| text.contains("not authorized")),
            "the refusal is the ownership sentence: {body}"
        );
        assert!(
            received.lock().expect("received").is_empty(),
            "the refused delivery must deliver nothing"
        );
        drop(guard);
        drop(server);
    }

    /// Creation is the `create_sessions` act **plus** the `send` the mandatory
    /// initial prompt performs: past the door the profile check reads exactly
    /// what a local caller reads, and a device that may create but may not talk
    /// is refused with the policy's own sentence before anything is spawned —
    /// the live row count proves no child was created-then-refused.
    #[test]
    fn a_peer_create_is_the_create_act() {
        let state = ServerState::new("mcp-p0-create".to_string());
        let owner = owner("mcp-p0-create-user", "mcp-p0-create-client");
        let creator = "s.peer.6".to_string();
        crate::session::insert_test_live_agent(&state.sessions, &creator, owner.clone());
        state.sessions.set_test_origin(
            &creator,
            SessionOrigin::peer("device-p0-create", crate::peer_policy::PeerRole::Client),
        );
        state
            .peer_upsert(peer_row("device-p0-create", &["view"]))
            .expect("store a peer");
        let guard = state
            .mcp
            .register(&creator, &owner, &SessionKind::Acp)
            .expect("registration")
            .expect("MCP guard");
        let token = state.mcp.test_token(&creator).expect("token");
        let server = state.mcp.start(&state).expect("MCP server");
        let refused = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_create_agent","arguments":{"profile":"Solo","title":"Kid","initialPrompt":"hi"}}}"#,
        );
        assert_eq!(
            response_json(&refused).pointer("/error/message"),
            Some(&json!("capability 'create_sessions' was not negotiated"))
        );
        state
            .peer_upsert(peer_row("device-p0-create", &["view", "create_sessions"]))
            .expect("grant create");
        // The re-audit's row: `create_sessions` without `send` is refused the
        // whole tool — the initial prompt always sends — with the policy's own
        // sentence, and the refusal lands at the door, before anything is
        // spawned: the roster still holds only the creator.
        let sendless = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"devboule_create_agent","arguments":{"profile":"Solo","title":"Kid","initialPrompt":"hi"}}}"#,
        );
        assert_eq!(
            response_json(&sendless).pointer("/error/message"),
            Some(&json!("capability 'send' was not negotiated"))
        );
        assert_eq!(
            state
                .sessions
                .live_agent_entries(&owner)
                .expect("roster")
                .len(),
            1,
            "the refused create spawned nothing"
        );
        state
            .peer_upsert(peer_row(
                "device-p0-create",
                &["view", "create_sessions", "send"],
            ))
            .expect("grant send");
        // Past the door, the profile check reads as it does for a local caller.
        let missing = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"devboule_create_agent","arguments":{"profile":"Solo","title":"Kid","initialPrompt":"hi"}}}"#,
        );
        let body = response_json(&missing);
        assert_eq!(body["result"]["isError"], true);
        assert!(
            body["result"]["content"][0]["text"]
                .as_str()
                .is_some_and(|text| text.contains("profile")),
            "past the door, the empty store reads as it does for a local caller: {body}"
        );
        drop(guard);
        drop(server);
    }

    /// F1 (MAX RECALL, authority), the decided composition stated on the
    /// consent surface: a peer holding `answer_permissions` may answer its own
    /// creation card, so the card names that device. Without the cap — and for
    /// every local caller — the card says nothing about answering.
    #[test]
    fn a_creation_card_names_a_device_that_may_answer_it() {
        let state = ServerState::new("mcp-f1-note".to_string());
        let mut row = peer_row("device-f1-note", &["view", "answer_permissions"]);
        row.display_name = "Phone".to_string();
        row.paired_by_user = Some("S-1-5-21-f1".to_string());
        state.peer_upsert(row).expect("store a peer");
        let holder = McpCaller::Peer {
            device_id: "device-f1-note".to_string(),
            role: crate::peer_policy::PeerRole::Client,
            caps: vec!["view".to_string(), "answer_permissions".to_string()],
        };
        let note = self_answer_note(&state, &holder)
            .expect("a device holding answer_permissions gets the note");
        assert!(note.contains("Phone"), "the note names the device: {note}");
        assert!(
            note.contains("answer_permissions"),
            "the note names the grant: {note}"
        );
        let plain = McpCaller::Peer {
            device_id: "device-f1-note".to_string(),
            role: crate::peer_policy::PeerRole::Client,
            caps: vec!["view".to_string()],
        };
        assert!(
            self_answer_note(&state, &plain).is_none(),
            "no note without the cap"
        );
        assert!(self_answer_note(&state, &McpCaller::Local).is_none());
    }

    /// The door reads origin, never kind (S9): a pi-kind caller meets exactly the
    /// judgment an ACP-kind caller meets. A peer's pi session without `send` is
    /// refused the create tool with the policy's sentence and spawns nothing;
    /// with `send` it passes the door to the same profile check. A local pi
    /// session passes untouched.
    #[test]
    fn a_peer_pi_caller_meets_the_same_door_as_acp() {
        let state = ServerState::new("mcp-door-pi".to_string());
        let owner = owner("mcp-door-pi-user", "mcp-door-pi-client");
        let creator = "s.peer.8".to_string();
        crate::session::insert_test_live_agent_with_kind(
            &state.sessions,
            &creator,
            owner.clone(),
            SessionKind::Pi,
        );
        state.sessions.set_test_origin(
            &creator,
            SessionOrigin::peer("device-pi-door", crate::peer_policy::PeerRole::Client),
        );
        state
            .peer_upsert(peer_row("device-pi-door", &["view", "create_sessions"]))
            .expect("store a peer");
        let guard = state
            .mcp
            .register_with_provider(
                &creator,
                &owner,
                &SessionKind::Pi,
                Some("pi"),
                AgentLineage::root(),
            )
            .expect("S9 registers pi")
            .expect("a bearer is minted");
        let token = state.mcp.test_token(&creator).expect("token");
        let server = state.mcp.start(&state).expect("MCP server");
        let refused = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_create_agent","arguments":{"profile":"Solo","title":"Kid","initialPrompt":"hi"}}}"#,
        );
        assert_eq!(
            response_json(&refused).pointer("/error/message"),
            Some(&json!("capability 'send' was not negotiated")),
            "a peer's pi session without send is refused like any other kind"
        );
        assert_eq!(
            state
                .sessions
                .live_agent_entries(&owner)
                .expect("roster")
                .len(),
            1,
            "the refused create spawned nothing"
        );
        state
            .peer_upsert(peer_row(
                "device-pi-door",
                &["view", "create_sessions", "send"],
            ))
            .expect("grant send");
        let missing = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"devboule_create_agent","arguments":{"profile":"Solo","title":"Kid","initialPrompt":"hi"}}}"#,
        );
        let body = response_json(&missing);
        assert_eq!(body["result"]["isError"], true);
        assert!(
            body["result"]["content"][0]["text"]
                .as_str()
                .is_some_and(|text| text.contains("profile")),
            "past the door, the profile check reads as for a local caller: {body}"
        );
        drop(guard);
        drop(server);
    }

    /// The ticked list performs nothing judged: a peer holding nothing reads it.
    #[test]
    fn a_peer_reads_the_ticked_list_holding_nothing() {
        let state = ServerState::new("mcp-p0-list".to_string());
        let owner = owner("mcp-p0-list-user", "mcp-p0-list-client");
        let creator = "s.peer.7".to_string();
        crate::session::insert_test_live_agent(&state.sessions, &creator, owner.clone());
        state.sessions.set_test_origin(
            &creator,
            SessionOrigin::peer("device-p0-list", crate::peer_policy::PeerRole::Client),
        );
        state
            .peer_upsert(peer_row("device-p0-list", &[]))
            .expect("store a peer");
        let guard = state
            .mcp
            .register(&creator, &owner, &SessionKind::Acp)
            .expect("registration")
            .expect("MCP guard");
        let token = state.mcp.test_token(&creator).expect("token");
        let server = state.mcp.start(&state).expect("MCP server");
        let response = http_request(
            &state.mcp.url,
            Some(&format!("Bearer {token}")),
            r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"devboule_list_profiles"}}"#,
        );
        let body = response_json(&response);
        assert_eq!(body["result"]["isError"], false, "{body}");
        drop(guard);
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
        assert_eq!(enabled_tool_list(catalog, None, ToolOverlay::NONE).len(), 2);

        let selective = ToolPolicyEntry {
            provider_id: "claude".to_string(),
            enabled: Some(true),
            disabled_tools: vec!["some_future_tool".to_string()],
        };
        let filtered = enabled_tool_list(catalog, Some(&selective), ToolOverlay::NONE);
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
        let always_on = enabled_tool_list(catalog, Some(&globally_off), ToolOverlay::NONE);
        assert_eq!(
            always_on.len(),
            1,
            "a disabled policy still lists the always-on roster tool"
        );

        let broker_tools = enabled_tool_list(
            crate::provider_catalog::MCP_BROKER_TOOLS,
            None,
            ToolOverlay::NONE,
        );
        let send_tool = broker_tools
            .iter()
            .find(|tool| tool["name"] == crate::provider_catalog::MCP_SEND_MESSAGE_TOOL)
            .expect("message tool");
        assert_eq!(
            send_tool["inputSchema"]["required"],
            serde_json::json!(["to_agent", "text"])
        );
    }

    #[test]
    fn a_disabled_tool_is_refused_at_call_time_and_the_roster_still_answers() {
        let state = ServerState::new("mcp-tool-policy-call".to_string());
        let owner = owner("mcp-policy-user", "mcp-policy-client");
        // The caller is a live local session: the tool door resolves every
        // bearer to its registry row, and a rowless registration is refused.
        crate::session::insert_test_live_agent(&state.sessions, "policy-session", owner.clone());
        let guard = state
            .mcp
            .register_with_provider(
                "policy-session",
                &owner,
                &SessionKind::Acp,
                Some("claude"),
                AgentLineage::root(),
            )
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

        // And `tools/list` for the same session still reports every tool the
        // session is served: the roster, the device list, the peer-agents
        // read, the profile list, the sender, the creation tool, the
        // delegated permission answer, the profile move, the activity read,
        // and the stop/close pair. Disabling one does not shrink the other
        // rows, which is the point of this test.
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
            Some(11)
        );
        let runtime_dir = state.sessions.runtime_dir().to_path_buf();
        drop(server);
        drop(guard);
        drop(state);
        let _ = std::fs::remove_dir_all(runtime_dir);
    }

    #[test]
    fn a_globally_disabled_policy_still_lists_the_always_on_tool() {
        let state = ServerState::new("mcp-tool-policy-list".to_string());
        let owner = owner("mcp-policy-list-user", "mcp-policy-list-client");
        let guard = state
            .mcp
            .register_with_provider(
                "silent-session",
                &owner,
                &SessionKind::Acp,
                Some("grok"),
                AgentLineage::root(),
            )
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
        assert_eq!(
            tools.len(),
            2,
            "the always-on pair: the roster and the profile list"
        );
        assert_eq!(tools[0]["name"], crate::provider_catalog::MCP_ROSTER_TOOL);
        assert_eq!(
            tools[1]["name"],
            crate::provider_catalog::MCP_LIST_PROFILES_TOOL
        );
        let runtime_dir = state.sessions.runtime_dir().to_path_buf();
        drop(server);
        drop(guard);
        drop(state);
        let _ = std::fs::remove_dir_all(runtime_dir);
    }

    /// What a `None` provider id means at HEAD, and nothing more.
    ///
    /// [`McpBroker::register`] is a `#[cfg(test)]` seam: both production call
    /// sites in `session.rs` — spawn and ACP resume — register through
    /// [`McpBroker::register_with_provider`], so this test does not exercise a
    /// live production path. It pins one behaviour: a session registered
    /// without a provider id has `provider_id: None`, `is_tool_enabled(None,
    /// _)` is therefore true for every tool, and the whole catalog is served
    /// whatever the store holds — no name is ever refused as `Tool disabled by
    /// policy`. The same request against the same stored policy is refused in
    /// `the_provider_registered_path_applies_that_policy`; the two differ only
    /// in the provider id the session registered with, so together they pin
    /// the gate to that id rather than to the policy file alone.
    ///
    /// The `None` id is reachable in production only through the same-user
    /// `DEVBOULE_ACP_COMMAND` development override, described on
    /// [`McpBroker::register_with_provider`].
    #[test]
    fn a_registration_without_a_provider_id_consults_no_policy() {
        let state = ServerState::new("mcp-tool-policy-gap".to_string());
        let owner = owner("mcp-policy-gap-user", "mcp-policy-gap-client");
        let guard = state
            .mcp
            .register("unnamed-session", &owner, &SessionKind::Acp)
            .expect("registration")
            .expect("MCP guard");
        // The caller is a live local session: the tool door resolves every
        // bearer to its registry row, and a rowless registration is refused.
        crate::session::insert_test_live_agent(&state.sessions, "unnamed-session", owner.clone());
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
            "a registration without a provider id consults no policy"
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
        let runtime_dir = state.sessions.runtime_dir().to_path_buf();
        drop(server);
        drop(guard);
        drop(state);
        let _ = std::fs::remove_dir_all(runtime_dir);
    }

    /// The inverse of `a_registration_without_a_provider_id_consults_no_policy`:
    /// the same request and the same stored policy, refused by the policy
    /// because this session did name its provider. Both production call sites
    /// in `session.rs` register through [`McpBroker::register_with_provider`],
    /// so this is the production path.
    #[test]
    fn the_provider_registered_path_applies_that_policy() {
        let state = ServerState::new("mcp-tool-policy-gated".to_string());
        let owner = owner("mcp-policy-gated-user", "mcp-policy-gated-client");
        // The caller is a live local session: the tool door resolves every
        // bearer to its registry row, and a rowless registration is refused.
        crate::session::insert_test_live_agent(&state.sessions, "gated-session", owner.clone());
        let guard = state
            .mcp
            .register_with_provider(
                "gated-session",
                &owner,
                &SessionKind::Acp,
                Some("claude"),
                AgentLineage::root(),
            )
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
        let runtime_dir = state.sessions.runtime_dir().to_path_buf();
        drop(server);
        drop(guard);
        drop(state);
        let _ = std::fs::remove_dir_all(runtime_dir);
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

    /// The closed schema (`S5` §2) is the first bound: nothing beyond the seven
    /// parameters the tool publishes, no way to name a mode, and no way to name a
    /// provider or a preset (`create-from-profile`).
    #[test]
    fn the_creation_schema_refuses_unknown_parameters_and_has_no_mode() {
        let full = json!({
            "title": "  builder  ",
            "profile": "worker",
            "labels": {"ticket": "S5-42"},
            "workspaceId": "workspace-1",
            "cwd": "crates",
            "initialPrompt": "count the tests",
            "notifyOnFinish": false,
        });
        let request = AgentCreateRequest::parse(&full).expect("the schema's own parameters");
        assert_eq!(request.title, "builder", "the name is trimmed");
        assert!(!request.notify);
        assert_eq!(request.cwd.as_deref(), Some("crates"));
        assert_eq!(
            request.labels.get("ticket").map(String::as_str),
            Some("S5-42"),
            "the caller's own labels come through as written"
        );
        let bare = json!({
            "title": "builder",
            "profile": "worker",
            "initialPrompt": "count the tests",
        });
        assert!(
            AgentCreateRequest::parse(&bare)
                .expect("notify defaults")
                .notify
        );
        for (arguments, sentence) in [
            (
                json!({"title": "b", "profile": "worker", "initialPrompt": "x", "mode": "bypass"}),
                "unknown parameter 'mode'",
            ),
            (
                json!({"title": "b", "profile": "worker", "initialPrompt": "x", "depth": 1}),
                "unknown parameter 'depth'",
            ),
            (
                json!({"title": "b", "profile": "worker", "initialPrompt": "x", "bypassMode": true}),
                "unknown parameter 'bypassMode'",
            ),
            (
                json!({"title": "b", "profile": "worker"}),
                "initialPrompt is required",
            ),
            (
                json!({"title": "b", "profile": "worker", "initialPrompt": "   "}),
                "initialPrompt is required",
            ),
            (
                json!({"title": "b".repeat(61).as_str(), "profile": "worker", "initialPrompt": "x"}),
                "display name is 61 characters",
            ),
            (
                json!({"title": "b", "profile": "worker", "initialPrompt": "x", "notifyOnFinish": "yes"}),
                "notifyOnFinish must be a boolean",
            ),
            // Audit S5-09: a wrong type is an invalid-params error, never a
            // silently ignored parameter. A caller that asked for a workspace
            // and got its word ignored would create a child somewhere it did
            // not ask for.
            (
                json!({"title": "b", "profile": "worker", "initialPrompt": "x", "workspaceId": 5}),
                "workspaceId must be a string",
            ),
            (
                json!({"title": "b", "profile": "worker", "initialPrompt": "x", "workspaceId": ["w"]}),
                "workspaceId must be a string",
            ),
            (
                json!({"title": "b", "profile": "worker", "initialPrompt": "x", "cwd": {"path": "crates"}}),
                "cwd must be a string",
            ),
            (
                json!({"title": "b", "profile": "worker", "initialPrompt": "x", "cwd": true}),
                "cwd must be a string",
            ),
            // The two parameters this slice removed are refused like any other
            // name the schema does not publish (`S5` §2, rev 9).
            (
                json!({"title": "b", "profile": "worker", "initialPrompt": "x", "provider": "claude"}),
                "unknown parameter 'provider'",
            ),
            (
                json!({"title": "b", "profile": "worker", "initialPrompt": "x", "preset": "worker"}),
                "unknown parameter 'preset'",
            ),
            (
                json!(["not", "an", "object"]),
                "arguments must be an object",
            ),
        ] {
            let refused = AgentCreateRequest::parse(&arguments).expect_err("refused");
            assert!(
                refused.contains(sentence),
                "{refused:?} should contain {sentence:?}"
            );
        }
        // `null` is absent, not an error: a client that serializes an optional
        // field as null asked for nothing, and gets the creator's own values.
        let explicit_null = json!({
            "title": "b",
            "profile": "worker",
            "initialPrompt": "x",
            "workspaceId": null,
            "cwd": null,
        });
        let parsed = AgentCreateRequest::parse(&explicit_null).expect("null is absent");
        assert!(parsed.workspace_id.is_none());
        assert!(parsed.cwd.is_none());
    }

    /// Audit S5-08: the fingerprint is what a retry must match, so it covers
    /// every field the answer depends on — including the ones that say *where*
    /// the child runs and whether the creator wants to hear about it.
    ///
    /// Two payloads that differ in any of them are different creations, and a
    /// fingerprint that ignored them would answer the second call with the
    /// first child: a session in somebody else's workspace, or a quiet child
    /// for a creator that asked to be told.
    #[test]
    fn a_creation_fingerprint_covers_workspace_cwd_and_notify() {
        let base = [
            "session-alex",
            "builder",
            "builder",
            "count the tests",
            "workspace-1",
            "crates",
            "notify",
            "{}",
        ];
        let request = AgentCreateRequest {
            profile: "builder".to_string(),
            title: "builder".to_string(),
            labels: std::collections::BTreeMap::new(),
            workspace_id: Some("workspace-1".to_string()),
            cwd: Some("crates".to_string()),
            initial_prompt: "count the tests".to_string(),
            notify: true,
        };
        let labels = request.labels_fingerprint();
        let fingerprint = creation_fingerprint(&AgentCreateRequest::creation_fingerprint_fields(
            "session-alex",
            &request,
            "notify",
            &labels,
        ));
        let labels = request.labels_fingerprint();
        let elsewhere = creation_fingerprint(&AgentCreateRequest::creation_fingerprint_fields(
            "session-alex",
            &AgentCreateRequest {
                cwd: Some("elsewhere".to_string()),
                ..request
            },
            "notify",
            &labels,
        ));
        assert_ne!(
            fingerprint, elsewhere,
            "a retry that changed cwd is not the same creation"
        );
        for (fields, what) in [
            (
                [
                    "session-alex",
                    "builder",
                    "builder",
                    "count the tests",
                    "workspace-1",
                    "crates",
                    "quiet",
                    "{}",
                ],
                "notifyOnFinish",
            ),
            (
                [
                    "session-alex",
                    "builder",
                    "builder",
                    "count the tests",
                    "workspace-2",
                    "crates",
                    "notify",
                    "{}",
                ],
                "workspaceId",
            ),
            (
                [
                    "session-alex",
                    "builder",
                    "builder",
                    "count the tests",
                    "workspace-1",
                    "src",
                    "notify",
                    "{}",
                ],
                "cwd",
            ),
        ] {
            let other = creation_fingerprint(&fields);
            assert_ne!(
                fingerprint, other,
                "a retry that changed {what} is not the same creation"
            );
        }
        // The length prefix is what keeps two different payloads from spelling
        // the same string by moving a delimiter.
        assert_ne!(
            creation_fingerprint(&["ab", "c"]),
            creation_fingerprint(&["a", "bc"]),
            "fields must not run into each other"
        );
        assert_eq!(
            creation_fingerprint(&base),
            creation_fingerprint(&base),
            "the same payload always spells the same fingerprint"
        );
    }

    /// The overlay is the same rule in both places (`S5` §2 and its checklist):
    /// hidden at `tools/list`, refused at `tools/call` — and the roster survives
    /// both, because naming a session is how an agent reports to a human.
    #[test]
    fn the_design_overlay_hides_both_tools_from_list_and_call() {
        let listed = enabled_tool_list(
            crate::provider_catalog::MCP_BROKER_TOOLS,
            None,
            ToolOverlay::DESIGN,
        )
        .into_iter()
        .filter_map(|tool| tool["name"].as_str().map(str::to_string))
        .collect::<Vec<_>>();
        assert!(!listed.iter().any(|name| name == MCP_CREATE_AGENT_TOOL));
        assert!(!listed.iter().any(|name| name == MCP_SEND_MESSAGE_TOOL));
        assert!(listed.iter().any(|name| name == MCP_ROSTER_TOOL));
        assert_eq!(
            tool_call_refusal(None, &ToolOverlay::DESIGN, MCP_CREATE_AGENT_TOOL),
            Some("Tool disabled by policy")
        );
        assert_eq!(
            tool_call_refusal(None, &ToolOverlay::DESIGN, MCP_SEND_MESSAGE_TOOL),
            Some("Tool disabled by policy")
        );
        assert_eq!(
            tool_call_refusal(None, &ToolOverlay::DESIGN, MCP_ROSTER_TOOL),
            None
        );
        assert_eq!(
            tool_call_refusal(None, &ToolOverlay::NONE, MCP_CREATE_AGENT_TOOL),
            None
        );
        // A worker has all three: the overlay is what removes them, nothing else.
        let listed = enabled_tool_list(
            crate::provider_catalog::MCP_BROKER_TOOLS,
            None,
            ToolOverlay::NONE,
        );
        assert_eq!(
            listed.len(),
            crate::provider_catalog::MCP_BROKER_TOOLS.len()
        );
    }
    /// Both enforcement points consult a profile-sourced overlay: the two
    /// peer tools hidden at `tools/list` and refused at `tools/call`, the
    /// roster kept — while a profile without the tick serves everything. The
    /// store is real, but this stops at the resolved overlay: it does not
    /// walk the creation road that stamps it onto the child
    /// (`overlay: profile.overlay.clone()` at the spawn assembly).
    #[test]
    fn a_profile_overlay_hides_peer_tools_from_the_child_it_creates() {
        let store = profile_store(document(
            vec![
                profile(
                    "hermit",
                    "profile-hermit",
                    "claude",
                    "default",
                    serde_json::json!({}),
                    &[MCP_SEND_MESSAGE_TOOL, MCP_CREATE_AGENT_TOOL],
                    true,
                ),
                profile(
                    "social",
                    "profile-social",
                    "claude",
                    "default",
                    serde_json::json!({}),
                    &[],
                    true,
                ),
            ],
            "",
        ));
        let names = |overlay: &ToolOverlay| {
            enabled_tool_list(
                crate::provider_catalog::MCP_BROKER_TOOLS,
                None,
                overlay.clone(),
            )
            .into_iter()
            .filter_map(|tool| tool["name"].as_str().map(str::to_string))
            .collect::<Vec<_>>()
        };
        let hermit = resolve_profile(&store, "hermit").expect("ticked");
        let listed = names(&hermit.overlay);
        assert!(!listed.iter().any(|name| name == MCP_CREATE_AGENT_TOOL));
        assert!(!listed.iter().any(|name| name == MCP_SEND_MESSAGE_TOOL));
        assert!(listed.iter().any(|name| name == MCP_ROSTER_TOOL));
        assert_eq!(
            tool_call_refusal(None, &hermit.overlay, MCP_CREATE_AGENT_TOOL),
            Some("Tool disabled by policy")
        );
        assert_eq!(
            tool_call_refusal(None, &hermit.overlay, MCP_SEND_MESSAGE_TOOL),
            Some("Tool disabled by policy")
        );
        assert_eq!(
            tool_call_refusal(None, &hermit.overlay, MCP_ROSTER_TOOL),
            None
        );
        let social = resolve_profile(&store, "social").expect("ticked");
        let listed = names(&social.overlay);
        assert!(listed.iter().any(|name| name == MCP_CREATE_AGENT_TOOL));
        assert!(listed.iter().any(|name| name == MCP_SEND_MESSAGE_TOOL));
        assert_eq!(
            tool_call_refusal(None, &social.overlay, MCP_CREATE_AGENT_TOOL),
            None
        );
    }
    // -----------------------------------------------------------------------
    // `create-from-profile`: resolving a profile, and the sentences a refusal
    // uses (`BRIEF-slice-5.md` §2, rev 9).
    // -----------------------------------------------------------------------

    /// A profile store holding `document`, in a directory of its own.
    ///
    /// The real store, not a fake: the resolution rules are about what the human
    /// has saved *at the moment of the call*, and a fake that answered from a map
    /// would be a second implementation of the thing under test.
    fn profile_store(document: serde_json::Value) -> crate::agent_profiles::AgentProfilesStore {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let dir = std::env::temp_dir().join(format!(
            "devboule broker profiles {}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let store = crate::agent_profiles::AgentProfilesStore::load(&dir);
        store
            .set(serde_json::from_value(document).expect("a profile document"))
            .expect("the store admits this document");
        store
    }

    /// One profile, with every field a test wants to choose.
    fn profile(
        name: &str,
        id: &str,
        provider: &str,
        mode: &str,
        features: serde_json::Value,
        overlay: &[&str],
        enabled: bool,
    ) -> serde_json::Value {
        serde_json::json!({
            "id": id,
            "name": name,
            "note": "when to use this one",
            "provider": provider,
            "model": "the model the human saved",
            "modeId": mode,
            "thinkingOptionId": "high",
            "features": features,
            "toolOverlay": overlay,
            "enabledForAgents": enabled,
        })
    }

    fn document(profiles: Vec<serde_json::Value>, standing: &str) -> serde_json::Value {
        serde_json::json!({ "profiles": profiles, "standingInstructions": standing })
    }

    /// The card's three auto-accept wordings, one per arm of the tri-state:
    /// the affirmative names the mode that answers, the negative names the
    /// mode that asks, and the unknown asserts neither direction.
    #[test]
    fn the_card_auto_accept_line_speaks_all_three_answers() {
        use devboule_protocol::UnattendedState;
        assert_eq!(
            auto_accept_line(UnattendedState::Yes, "auto_accept"),
            "Yes (mode auto_accept)"
        );
        assert_eq!(
            auto_accept_line(UnattendedState::No, "default"),
            "No — mode default asks the human"
        );
        assert_eq!(
            auto_accept_line(UnattendedState::Unknown, "default"),
            "Cannot establish — mode default belongs to the agent's own vocabulary, so whether it asks is not something Devboule can check",
        );
    }

    /// No profile enabled at all: every creation is refused, and the refusal
    /// names **no** profile — an agent must not learn what exists but is
    /// forbidden.
    #[test]
    fn no_enabled_profile_refuses_and_names_none() {
        let store = profile_store(document(
            vec![profile(
                "design",
                "profile-design",
                "claude",
                "default",
                serde_json::json!({}),
                &[],
                false,
            )],
            "",
        ));
        let refusal = resolve_profile(&store, "design").expect_err("nothing is enabled");
        assert_eq!(refusal, "no profile is enabled for agents");
        assert!(
            !refusal.contains("design") && !refusal.contains("profile-design"),
            "the refusal names no profile: {refusal}"
        );
    }

    /// An unknown name and an un-ticked one are refused the same way, and neither
    /// refusal tells the caller what it is not allowed to name.
    #[test]
    fn an_unknown_or_unticked_profile_is_refused_with_the_list_sentence() {
        let store = profile_store(document(
            vec![
                profile(
                    "worker",
                    "profile-worker",
                    "claude",
                    "default",
                    serde_json::json!({}),
                    &[],
                    true,
                ),
                profile(
                    "design",
                    "profile-design",
                    "claude",
                    "default",
                    serde_json::json!({}),
                    &[],
                    false,
                ),
            ],
            "",
        ));
        assert!(resolve_profile(&store, "worker").is_ok());
        for name in ["nobody", "design"] {
            let refusal = resolve_profile(&store, name).expect_err("refused");
            assert_eq!(refusal, "unknown profile; call devboule_list_profiles");
            assert!(
                !refusal.contains(name),
                "the sentence does not echo what was asked for: {refusal}"
            );
        }
    }

    /// Two ticked profiles may share a name (the id is the identity), so a name
    /// that resolves to both is refused rather than answered with the first: the
    /// first would be a provider the human did not name.
    #[test]
    fn one_name_on_two_enabled_profiles_is_refused() {
        let twin = |id: &str, provider: &str, enabled: bool| {
            profile(
                "worker",
                id,
                provider,
                "default",
                serde_json::json!({}),
                &[],
                enabled,
            )
        };
        let two = profile_store(document(
            vec![
                twin("profile-a", "claude", true),
                twin("profile-b", "grok", true),
            ],
            "",
        ));
        assert_eq!(
            resolve_profile(&two, "worker").expect_err("two of them"),
            "more than one profile is called worker"
        );

        // One ticked and one not is one profile: the un-ticked twin is not a
        // candidate at all, so the name resolves to the ticked one.
        let one = profile_store(document(
            vec![
                twin("profile-a", "claude", true),
                twin("profile-b", "grok", false),
            ],
            "",
        ));
        let resolved = resolve_profile(&one, "worker").expect("one ticked twin");
        assert_eq!(resolved.id, "profile-a");
    }

    /// The ticked list is read **at the moment of the call**, never cached: a
    /// profile un-ticked between a `devboule_list_profiles` and the creation is
    /// not enabled when the creation happens, and one ticked in between is.
    #[test]
    fn the_ticked_list_is_read_at_the_moment_of_the_call() {
        // Two profiles, so that un-ticking the one a creation names is answered
        // by the *name* rule and not by the empty-list rule below.
        let naming = |worker: bool, design: bool| {
            document(
                vec![
                    profile(
                        "worker",
                        "profile-worker",
                        "claude",
                        "default",
                        serde_json::json!({}),
                        &[],
                        worker,
                    ),
                    profile(
                        "design",
                        "profile-design",
                        "claude",
                        "default",
                        serde_json::json!({}),
                        &[],
                        design,
                    ),
                ],
                "",
            )
        };
        let store = profile_store(naming(true, true));
        // What an agent would have read a moment ago.
        let listed = list_profiles(&store, &json!(1));
        let listed = listed["result"]["structuredContent"]["profiles"].clone();
        assert_eq!(listed[0]["name"], "worker");

        // The human un-ticks the one this creation names.
        store
            .set(serde_json::from_value(naming(false, true)).expect("document"))
            .expect("store");
        assert_eq!(
            resolve_profile(&store, "worker").expect_err("un-ticked"),
            "unknown profile; call devboule_list_profiles"
        );
        assert!(
            resolve_profile(&store, "design").is_ok(),
            "the profile it did not name is still creatable"
        );

        // Un-ticking everything instead refuses every creation, and the refusal
        // names no profile at all.
        store
            .set(serde_json::from_value(naming(false, false)).expect("document"))
            .expect("store");
        assert_eq!(
            resolve_profile(&store, "worker").expect_err("nothing ticked"),
            "no profile is enabled for agents"
        );

        // And ticking it again is enough for the next call: nothing was cached
        // from the list above, in either direction.
        store
            .set(serde_json::from_value(naming(true, true)).expect("document"))
            .expect("store");
        assert!(resolve_profile(&store, "worker").is_ok());
    }

    /// What the creation runs is exactly what the human saved, with no
    /// substitution in either direction — including a mode no preset table would
    /// ever have named.
    #[test]
    fn a_profile_resolves_to_exactly_what_was_saved() {
        let store = profile_store(document(
            vec![profile(
                "runner",
                "profile-runner",
                "grok",
                "bypass",
                serde_json::json!({"autoAccept": true, "sandbox": "none"}),
                &["devboule_send_message"],
                true,
            )],
            "",
        ));
        let resolved = resolve_profile(&store, "runner").expect("ticked");
        assert_eq!(resolved.id, "profile-runner");
        assert_eq!(resolved.name, "runner");
        assert_eq!(resolved.provider, "grok");
        assert_eq!(resolved.model, "the model the human saved");
        assert_eq!(resolved.mode, "bypass");
        assert_eq!(resolved.thinking_option_id.as_deref(), Some("high"));
        assert_eq!(
            resolved.features.get("sandbox"),
            Some(&serde_json::json!("none"))
        );
        assert!(!resolved.overlay.allows("devboule_send_message"));
        assert!(resolved.overlay.allows("devboule_list_agents"));
        // The marker is no longer a field on the resolution: the birth derives
        // it from the delivery. The same prediction the list serves says what
        // this profile's mode would earn.
        assert_eq!(
            predicted_unattended(&resolved.provider, &resolved.mode),
            devboule_protocol::UnattendedState::Yes,
            "the mode auto-answers permission prompts"
        );
    }

    /// A retry of a creation that **committed** is answered by the idempotency
    /// store even when the profile it named is gone: the human renamed or
    /// un-ticked it inside the retry window, and the child from the first
    /// attempt is already alive. Refusing the retry would tell the creator its
    /// creation failed, and the re-issue it would then spend a second live slot
    /// on a child it already has — which is why the store is consulted before
    /// the profile is resolved.
    ///
    /// The other half, unchanged by that ordering: a **new** call — a different
    /// frame id, no remembered answer — naming the same stale name is still
    /// refused, with the same sentence the store's state earns.
    #[test]
    fn a_retry_is_answered_even_when_its_profile_is_gone() {
        let state = ServerState::new("mcp-retry-stale-profile".to_string());
        let owner = owner("mcp-retry-user", "mcp-retry-client");
        // The profile was ticked when the first attempt ran; it has since been
        // un-ticked. (A rename reads the same way here: the name no longer
        // resolves, and the refusal sentence is resolve_profile's to choose.)
        state
            .agent_profiles
            .set(
                serde_json::from_value(document(
                    vec![profile(
                        "worker",
                        "profile-worker",
                        "claude",
                        "default",
                        serde_json::json!({}),
                        &[],
                        false,
                    )],
                    "",
                ))
                .expect("the document"),
            )
            .expect("the store admits this document");

        let registration = RegisteredSession {
            session_id: "s.creator".to_string(),
            owner: owner.clone(),
            provider_id: None,
            depth: 0,
            overlay: crate::provider_catalog::ToolOverlay::NONE,
            bearer: "the bearer".to_string(),
            claude_config_path: None,
            runtime: None,
            broker_ready: Arc::new(AtomicBool::new(false)),
        };
        let arguments = json!({
            "profile": "worker",
            "title": "child",
            "initialPrompt": "report your result",
        });
        let id = serde_json::json!(7);

        // The first attempt's answer, remembered under this frame's key while
        // the profile was still ticked: the same key and the same fingerprint
        // the handler itself computes for the re-sent frame.
        let creator_id = registration.session_id.clone();
        let request = AgentCreateRequest::parse(&arguments).expect("the request parses");
        let retry_key = crate::server::creation_retry_key(&creator_id, &id).expect("retry key");
        let notify_field = if request.notify { "notify" } else { "quiet" };
        let labels_field = request.labels_fingerprint();
        let fingerprint = creation_fingerprint(&AgentCreateRequest::creation_fingerprint_fields(
            &creator_id,
            &request,
            notify_field,
            &labels_field,
        ));
        let first_child = crate::journal::new_session_record(
            "s.child.1",
            "mcp-retry-user",
            None,
            SessionKind::Acp,
            "child",
        )
        .to_session();
        crate::server::remember_creation_session(
            &state,
            &owner,
            &retry_key,
            &fingerprint,
            &first_child,
        );

        // The re-sent frame: same id, same payload, stale profile. It is
        // answered with the first child, and creates nothing.
        let answer = create_agent(
            &state,
            &state.mcp,
            &McpCaller::Local,
            &registration,
            &id,
            request,
        );
        assert_eq!(
            answer["result"]["structuredContent"]["sessionId"], "s.child.1",
            "the retry answers the first call's session: {answer}"
        );

        // A genuinely new call naming the stale profile is still refused, with
        // the sentence the empty ticked list earns.
        let new_id = serde_json::json!(8);
        let new_request = AgentCreateRequest::parse(&arguments).expect("the request parses");
        let refusal = create_agent(
            &state,
            &state.mcp,
            &McpCaller::Local,
            &registration,
            &new_id,
            new_request,
        );
        assert_eq!(
            refusal["result"]["content"][0]["text"], "no profile is enabled for agents",
            "a new call still meets the profile check: {refusal}"
        );
        assert_eq!(
            refusal["result"]["isError"], true,
            "the refusal is an error result, not a session: {refusal}"
        );
    }

    /// The labels a caller may write, and the ones it may not.
    #[test]
    fn a_caller_cannot_write_a_reserved_label_and_may_write_its_own() {
        let parsed = parse_labels(
            &json!({"labels": {"ticket": "S5", "note": "a sentence"}})
                .as_object()
                .expect("object")
                .clone(),
        )
        .expect("free labels");
        assert_eq!(parsed.get("ticket").map(String::as_str), Some("S5"));
        assert_eq!(parsed.get("note").map(String::as_str), Some("a sentence"));

        for key in [
            "devboule.created-by",
            "devboule.depth",
            "devboule.origin",
            "devboule.profile",
            "devboule.",
        ] {
            let refusal = parse_labels(
                &json!({"labels": {key: "mine"}})
                    .as_object()
                    .expect("object")
                    .clone(),
            )
            .expect_err("reserved");
            assert_eq!(refusal, "reserved label prefix", "{key}");
        }

        // Absent is empty, and a value that is not a string is refused by name.
        assert!(
            parse_labels(&json!({}).as_object().expect("object").clone())
                .expect("no labels")
                .is_empty()
        );
        assert_eq!(
            parse_labels(
                &json!({"labels": {"ticket": 5}})
                    .as_object()
                    .expect("object")
                    .clone()
            )
            .expect_err("not a string"),
            "the label 'ticket' must be a string"
        );
    }

    /// The four facts the daemon stamps into every child, from its own bookkeeping
    /// and never from the request.
    #[test]
    fn the_daemon_stamps_its_four_labels_into_the_callers_map() {
        let store = profile_store(document(
            vec![profile(
                "runner",
                "profile-runner",
                "grok",
                "default",
                serde_json::json!({}),
                &[],
                true,
            )],
            "",
        ));
        let profile = resolve_profile(&store, "runner").expect("ticked");
        let mut caller = std::collections::BTreeMap::new();
        caller.insert("ticket".to_string(), "S5".to_string());
        let labels = stamped_labels(
            &caller,
            "s.parent.1",
            &profile,
            2,
            &devboule_protocol::SessionOrigin::peer(
                "device-phone",
                devboule_protocol::PeerRole::Client,
            ),
        );
        assert_eq!(labels.get("ticket").map(String::as_str), Some("S5"));
        assert_eq!(
            labels.get("devboule.created-by").map(String::as_str),
            Some("s.parent.1")
        );
        assert_eq!(labels.get("devboule.depth").map(String::as_str), Some("2"));
        assert_eq!(
            labels.get("devboule.origin").map(String::as_str),
            Some("peer:device-phone")
        );
        assert_eq!(
            labels.get("devboule.profile").map(String::as_str),
            Some("profile-runner"),
            "the stamp is the profile's stable id, like the session's own field"
        );
    }

    /// The A2A result names the task and the context (`S5` §2, decision 8b), with
    /// no bookkeeping of the caller's own.
    #[test]
    fn a_creation_result_carries_the_task_id_and_the_context() {
        let session = devboule_protocol::Session {
            id: "s.parent.2".to_string(),
            workspace_id: None,
            cwd: None,
            kind: devboule_protocol::SessionKind::Acp,
            title: "Agent".to_string(),
            provider: Some("grok".to_string()),
            peer_session_id: None,
            state: devboule_protocol::SessionState::Live { generation: 1 },
            elapsed_ms: Some(0),
            created_at_ms: 1,
            origin: devboule_protocol::SessionOrigin::local(),
            display_name: Some("builder".to_string()),
            created_by: Some("s.parent.1".to_string()),
            profile_id: Some("profile-worker".to_string()),
            context_id: Some("s.parent.1".to_string()),
            unattended: devboule_protocol::UnattendedState::No,
            labels: Default::default(),
            resumable: false,
        };
        let result = created_result(&json!(7), &session, true);
        let content = &result["result"]["structuredContent"];
        assert_eq!(content["sessionId"], "s.parent.2");
        assert_eq!(
            content["taskId"], content["sessionId"],
            "a child is the task; there is no second id to keep"
        );
        assert_eq!(
            content["contextId"], "s.parent.1",
            "the context is the creator's, not a fresh one"
        );
        assert_eq!(content["displayName"], "builder");
        assert_eq!(content["state"], "submitted");
    }

    /// `devboule_list_profiles` serves the ticked profiles, in the human's order,
    /// with the note verbatim — and nothing else.
    #[test]
    fn the_profile_list_is_the_humans_order_with_verbatim_notes() {
        let long_note = "a".repeat(2000);
        let store = profile_store(document(
            vec![
                profile(
                    "second",
                    "profile-2",
                    "claude",
                    "default",
                    serde_json::json!({}),
                    &[],
                    true,
                ),
                profile(
                    "first",
                    "profile-1",
                    "grok",
                    "bypass",
                    serde_json::json!({"autoAccept": true}),
                    &[],
                    true,
                ),
                profile(
                    "hidden",
                    "profile-3",
                    "codex",
                    "default",
                    serde_json::json!({}),
                    &[],
                    false,
                ),
            ],
            "the standing instructions are not a profile's business to read",
        ));
        let mut listed = list_profiles(&store, &json!(1));
        let profiles = listed["result"]["structuredContent"]["profiles"]
            .as_array_mut()
            .expect("an array of profiles")
            .clone();
        assert_eq!(profiles.len(), 2, "only the ticked ones");
        assert_eq!(profiles[0]["name"], "second");
        assert_eq!(
            profiles[1]["name"], "first",
            "the human's order, never sorted"
        );
        assert_eq!(profiles[0]["note"], "when to use this one");
        assert_eq!(profiles[0]["provider"], "claude");
        assert_eq!(profiles[0]["model"], "the model the human saved");
        assert_eq!(profiles[0]["mode"], "default");
        assert_eq!(profiles[0]["unattended"], "no");
        assert_eq!(
            profiles[1]["unattended"], "yes",
            "the mode auto-answers, so creating from it yields a session that will not ask"
        );
        assert_eq!(
            profiles[0]["provider"], "claude",
            "fixture sanity: the first profile is the daemon-authored asking mode"
        );
        assert!(
            !listed["result"]["structuredContent"]
                .to_string()
                .contains("standing"),
            "the standing instructions are not served to a caller"
        );
        assert!(
            !listed["result"]["structuredContent"]
                .to_string()
                .contains("profile-1"),
            "the id is the daemon's key for the session it records, not the caller's to name"
        );

        // A note is never truncated: the only thing a model routes work with.
        let store = profile_store(document(
            vec![profile(
                "long",
                "profile-long",
                "claude",
                "default",
                serde_json::json!({}),
                &[],
                true,
            )],
            "",
        ));
        let mut document = store.document();
        document.profiles[0].note = long_note.clone();
        store.set(document).expect("store");
        let listed = list_profiles(&store, &json!(1));
        assert_eq!(
            listed["result"]["structuredContent"]["profiles"][0]["note"],
            serde_json::json!(long_note),
            "the note arrives as the human wrote it, whole"
        );
    }
}
