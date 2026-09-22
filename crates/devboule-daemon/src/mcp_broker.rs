//! The daemon-owned MCP channel for live agent sessions.
//!
//! The broker is deliberately small: one loopback HTTP listener, one bearer
//! token per session, and the six broker tools served to every agent family —
//! ACP and Claude natively, pi through its bridge extension (S5), Codex
//! through `-c` overrides on its launch line (S6), both verified post-spawn
//! (S7/S8).
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

    /// The loopback URL this broker answers on, for the tool test modules that
    /// drive the wire from outside this module.
    #[cfg(test)]
    pub(crate) fn url(&self) -> &str {
        &self.url
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
/// caller for its interrupt authority (S4-01), and the ordinary registry
/// ownership checks apply the peer's own scope. The MCP send body explicitly
/// supplies a local source namespace, so its owner-scoped target lookup does
/// not reach the wire-only daemon-peer allowance; that allowance is reached
/// only by an inbound remote frame. A `Local` caller keeps the unmarked
/// connection, byte-identical to before. `Unknown`/`Absent` never reach a
/// body — the door refuses them — and keep it too.
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
                        .close_agent_child(state, &registration.session_id, session_arg)
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
            } else if tool_name == Some(crate::provider_catalog::MCP_NEIGHBORHOOD_TOOL) {
                // The caller's own workspace decides the graph; the bearer is
                // the identity, and no argument names a path.
                project_graph_reply(
                    &id,
                    crate::mcp_project_graph::neighborhood(
                        state,
                        &registration.session_id,
                        &registration.owner,
                        &project_graph_arguments(message),
                    ),
                )
            } else if tool_name == Some(crate::provider_catalog::MCP_IMPORTS_TOOL) {
                project_graph_reply(
                    &id,
                    crate::mcp_project_graph::imports(
                        state,
                        &registration.session_id,
                        &registration.owner,
                        &project_graph_arguments(message),
                    ),
                )
            } else if tool_name == Some(crate::provider_catalog::MCP_IMPORTERS_TOOL) {
                project_graph_reply(
                    &id,
                    crate::mcp_project_graph::importers(
                        state,
                        &registration.session_id,
                        &registration.owner,
                        &project_graph_arguments(message),
                    ),
                )
            } else if tool_name == Some(crate::provider_catalog::MCP_ORACLE_SEARCH_TOOL) {
                // The caller's own workspace decides the search; the bearer
                // is the identity, and no argument names a path.
                project_graph_reply(
                    &id,
                    crate::oracle_forward::search(
                        state,
                        &registration.session_id,
                        &registration.owner,
                        &project_graph_arguments(message),
                    ),
                )
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
            } else if *name == crate::provider_catalog::MCP_NEIGHBORHOOD_TOOL {
                // Closed like its siblings, and bounded: `depth` is capped at
                // the walk the engine is willing to do (the tools' own parser
                // refuses anything wider), and `kind` is the graph's whole edge
                // vocabulary.
                json!({
                    "type": "object",
                    "properties": {
                        "node": {"type": "string"},
                        "depth": {"type": "integer", "minimum": 1, "maximum": 4},
                        "kind": {"type": "string", "enum": ["IMPORT", "CONTAIN"]},
                    },
                    "required": ["node"],
                    "additionalProperties": false,
                })
            } else if *name == crate::provider_catalog::MCP_IMPORTS_TOOL
                || *name == crate::provider_catalog::MCP_IMPORTERS_TOOL
            {
                // One closed document for both directions: they differ in
                // which way they read the edge, not in what they accept.
                json!({
                    "type": "object",
                    "properties": {"file": {"type": "string"}},
                    "required": ["file"],
                    "additionalProperties": false,
                })
            } else if *name == crate::provider_catalog::MCP_ORACLE_SEARCH_TOOL {
                // Closed and bounded like its siblings: `limit` is clamped to
                // the range the app's own route clamps to, and `root` is
                // deliberately absent — the session's row names the folder.
                json!({
                    "type": "object",
                    "properties": {
                        "query": {"type": "string"},
                        "limit": {"type": "integer", "minimum": 1, "maximum": 10},
                    },
                    "required": ["query"],
                    "additionalProperties": false,
                })
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

/// The arguments of a project-graph call, absent when the request carries
/// none: the tools' own parser is the one place that decides what their closed
/// argument set is, exactly as the create tool's parser does.
fn project_graph_arguments(message: &Value) -> Value {
    message
        .pointer("/params/arguments")
        .cloned()
        .unwrap_or(Value::Null)
}

/// One reply shape for the three project-graph tools and the Oracle search
/// (`GraphError` is the shared refusal type, kept as named): their own document
/// on success, `-32602` for a malformed request, and a tool error
/// (`isError: true`) carrying the sentence that names the missing fact when the
/// answer cannot be produced. A refusal is deliberately not an empty document:
/// `[]` would read as "this node has no neighbours".
fn project_graph_reply(
    id: &Value,
    result: Result<Value, crate::mcp_project_graph::GraphError>,
) -> Result<Option<Value>, Value> {
    match result {
        Ok(document) => {
            let text = serde_json::to_string(&document).map_err(|error| {
                json!({"jsonrpc":"2.0", "id": id, "error": {"code": -32603, "message": format!("Could not encode project graph: {error}")}})
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
        Err(crate::mcp_project_graph::GraphError::Invalid(message)) => {
            Ok(Some(rpc_error(id.clone(), -32602, &message)))
        }
        Err(crate::mcp_project_graph::GraphError::Refused(message)) => {
            Ok(Some(tool_error(id, &message)))
        }
    }
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
        // Legacy Codex homes (S6, retired): the `-c` carrier writes no home
        // any more, so whole trees by our name alone — never by content,
        // never outside our names — are leftovers from older builds, and
        // sweeping them keeps a crashed spawn's goals, sqlite and
        // `installation_id` from accumulating. At daemon start no live
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
#[path = "mcp_broker_tests.rs"]
mod tests;
