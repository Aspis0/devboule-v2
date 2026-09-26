//! The daemon-owned MCP channel for live agent sessions.
//!
//! The broker is deliberately small: one loopback HTTP listener, one bearer
//! token per session, and the broker tools served to every agent family —
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
use std::io;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use devboule_protocol::{OwnerId, SessionKind, WireError};
use serde_json::{json, Value};
use uuid::Uuid;

use crate::server::ServerState;

mod caller;
mod config_files;
mod dispatch;
mod http;
mod redact;
mod tools;
mod transport;

use self::config_files::{cleanup_stale_configs, remove_file, write_protected_json, CONFIG_PREFIX};
use self::transport::{mcp_accept_loop, MCP_PATH};

pub(crate) use self::config_files::write_protected_str;
pub(crate) use self::redact::redact_broker_text;

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

const MAX_MCP_CONNECTIONS: usize = 64;
const MCP_GET_LIFETIME: Duration = Duration::from_secs(30);

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

/// Child-env names for the broker carrier (S5/S6). The names carry no secret
/// bytes themselves (pinned by the S4 redaction test); the token travels only
/// as the env *value*, never argv (`ARCHITETTURA.md` §15.5 rule).
pub(crate) const MCP_TOKEN_ENV: &str = "DEVBOULE_MCP_TOKEN";
pub(crate) const MCP_URL_ENV: &str = "DEVBOULE_MCP_URL";

pub(crate) fn ready_timeout() -> Duration {
    std::env::var("DEVBOULE_MCP_READY_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .map(Duration::from_millis)
        .filter(|value| !value.is_zero())
        .unwrap_or(MCP_READY_TIMEOUT)
}

#[cfg(test)]
#[path = "../mcp_broker_tests.rs"]
mod tests;
