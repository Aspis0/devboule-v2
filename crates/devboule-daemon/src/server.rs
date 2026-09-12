use std::collections::{HashMap, HashSet, VecDeque};
use std::path::Path;
#[cfg(not(test))]
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender, TryRecvError};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use devboule_protocol::{
    caps, m3a_daemon_capabilities, negotiate, validate_idempotency_key, ClientMessage, DaemonHello,
    DaemonMessage, DaemonStatusBody, ErrorCode, JournalLimits as WireJournalLimits,
    JournalSessionUsage as WireJournalSessionUsage, JournalUsage as WireJournalUsage, OwnerId,
    PersistenceKind, PromptAttachment, ResumeResult, RetentionPatch, SessionEvent,
    SessionEventEnvelope, SessionKind, Unreclaimable as WireUnreclaimable, WireError,
    PROTOCOL_MIN_VERSION, PROTOCOL_VERSION,
};

use crate::device_identity::RemoteState;
use crate::diagnostics::{DiagnosticsInput, DiagnosticsReport};
use crate::error::DaemonError;
use crate::framing::Framed;
use crate::idempotency::{IdempotencyOutcome, IdempotencyStore};
use crate::journal::{AuditRecord, Journal, PeerMutation, PeerRecord, JOURNAL_SCHEMA_VERSION};
use crate::lock::SingleInstanceLock;
use crate::login_shell_env::login_shell_capture_outcome;
use crate::outbound::ConnOut;
use crate::paths::RuntimePaths;
use crate::peer_policy::{peer_allows, ConnPeer, PeerDecision, PeerRole};
use crate::peer_transport::{accept_peers, TokenBucket};
use crate::process_tree::JobObject;
use crate::provider_update::{NpmInstallRunner, ProcessNpmInstallRunner};
use crate::secret_store::SecretStore;
use crate::session::{ConnHandle, PendingEvent, SessionRegistry};
use crate::transport::{self, Listener};
use crate::IDLE_SHUTDOWN_GRACE;

const JOIN_SLICE: Duration = Duration::from_millis(10);
const JOIN_BUDGET: Duration = Duration::from_millis(500);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);
/// How long a loaded `peers` table is trusted by the accept path (M1). Every
/// peer mutation also invalidates it, so this is a backstop for a mutation path
/// that does not, not the mechanism that keeps the table current.
const PEER_TABLE_TTL: Duration = Duration::from_secs(10);

/// The accept path's cached `peers` snapshot.
struct PeerTableView {
    loaded: Option<(Instant, Arc<crate::peer_transport::PeerTable>)>,
    ttl: Duration,
}

impl Default for PeerTableView {
    fn default() -> Self {
        Self {
            loaded: None,
            // Not `Duration::ZERO`: a default of zero would silently disable
            // the cache everywhere the view is constructed.
            ttl: PEER_TABLE_TTL,
        }
    }
}

#[derive(Default)]
struct Lifecycle {
    clients: u32,
    sessions: u32,
    shutting_down: bool,
    idle_generation: u64,
}

pub struct ServerState {
    instance_id: String,
    started: Instant,
    stop: Arc<AtomicBool>,
    lifecycle: Mutex<Lifecycle>,
    shutdown_flag: Arc<Mutex<bool>>,
    shutdown_cvar: Arc<Condvar>,
    idempotency: Mutex<IdempotencyStore>,
    pub(crate) process_job: Arc<JobObject>,
    pub(crate) mcp: Arc<crate::mcp_broker::McpBroker>,
    /// Per-provider tool policy, read by the MCP broker on every
    /// `tools/list` and `tools/call` and written by `ToolPolicySet`. One
    /// instance per daemon: the file beside the journal is this daemon's,
    /// and a paired device's toggles are its own.
    pub(crate) tool_policy: Arc<crate::tool_policy::ToolPolicyStore>,
    pub sessions: SessionRegistry,
    conn_ids: AtomicU64,
    journal_error: Mutex<Option<String>>,
    session_watchers: Mutex<HashMap<u64, SessionWatch>>,
    /// Last measured spawn+handshake outcome per provider id. Contract for
    /// ProviderInfo.authentication: "unknown" (never measured since daemon
    /// start), "ok" (most recent spawn+handshake completed), or
    /// "failed: <reason>" (most recent attempt failed; reason is the error
    /// message collapsed to one line, max 200 chars). A measured last-start
    /// observation, never an auth probe.
    ///
    /// Scoping constraint: the map is keyed by provider id only and is
    /// correct while the daemon is single-user (pipe-peer identity). A
    /// multi-user daemon must key it by owner, or the failure reasons leak
    /// across users.
    provider_health: Mutex<HashMap<String, String>>,
    /// Version declared by the provider's most recent successful ACP
    /// initialize handshake, keyed by provider id.
    provider_versions: Mutex<HashMap<String, String>>,
    /// Version obtained by an explicit native `--version` refresh probe.
    provider_cli_versions: Mutex<HashMap<String, (String, CliVersionFingerprint)>>,
    /// Executable paths whose Claude version probe is already running.
    claude_version_probes: Mutex<HashSet<std::path::PathBuf>>,
    /// The only process-launch seam for provider updates. Tests replace this
    /// runner so no npm or network is ever started by the test suite.
    npm_install_runner: Arc<dyn NpmInstallRunner>,
    /// Runtime paths, kept so the secret store can be selected lazily.
    /// Probing the OS credential store here would put a credential read into
    /// every unit test that builds a `ServerState`.
    paths: RuntimePaths,
    /// The same journal handle the session registry writes through, kept for
    /// the `peers` and `audit` tables (schema v8). `None` when the journal
    /// could not be opened.
    journal: Option<Arc<Journal>>,
    secret_store: OnceLock<(Arc<dyn SecretStore>, &'static str)>,
    /// This device's identity and Noise static key, loaded once. The result is
    /// cached including the error: a missing key must stay missing, not be
    /// retried (and certainly not regenerated) on every call.
    device_identity: OnceLock<
        Result<
            Arc<crate::device_identity::DeviceIdentity>,
            crate::device_identity::DeviceIdentityError,
        >,
    >,
    /// The tailnet listener's state, replaced by the peer accept loop once it
    /// starts. Before that (and with no Tailscale) it says so, with a reason,
    /// rather than pretending the daemon is reachable.
    remote: Mutex<RemoteState>,
    /// Live remote connections, so revocation can close them immediately.
    remote_conns: Mutex<HashMap<u64, (String, Arc<AtomicBool>)>>,
    /// The `peers` snapshot the accept path filters on, cached (M1).
    ///
    /// The previous behaviour loaded the whole table from the journal for every
    /// accepted socket, so a burst of connects was a burst of journal RPCs on
    /// the writer thread. The cache is refreshed by every peer mutation and
    /// expires on its own, so it cannot serve a stale answer after a pairing or
    /// a revoke, and cannot serve one forever if a mutation path is ever missed.
    peer_table: Mutex<PeerTableView>,
    /// Serialises the *load* of the peer table, without being held while the
    /// journal is queried.
    ///
    /// Without it, a burst of connections arriving on a cold cache all miss the
    /// same check and each performs its own journal read — measured: four
    /// simultaneous connects produced three loads. `peer_revoke` only ever takes
    /// `peer_table` (through `invalidate_peer_table`), so a load waiting here
    /// cannot delay a revocation.
    peer_table_load: Mutex<()>,
    /// The tailnet listener's own stop flag and thread, so the listener is a
    /// process-lifetime resource the state owns (C5): `run_windows` starts it
    /// through `ensure_remote_listener` and stops it through
    /// `stop_remote_listener`, and `PairingStart` can start it late.
    peer_stop: Arc<AtomicBool>,
    peer_listener: Mutex<Option<JoinHandle<()>>>,
    /// The transport the peer listener uses, and that the initiator side of a
    /// pairing uses for its own `whois` on the responder's address. One
    /// instance for the process: a `Tailnet` on a real daemon is a unit struct,
    /// so this costs nothing and lets a test substitute a stub.
    peer_transport: OnceLock<Arc<dyn crate::peer_transport::PeerTransport>>,
    pairing: Arc<crate::pairing::PairingService>,
    /// Test-only: real `peers` loads, so a test can prove the cache held.
    #[cfg(test)]
    peer_table_loads: AtomicU64,
    /// Test-only: how many times a tailnet listener was actually started, so a
    /// test can prove `ensure_remote_listener` is idempotent rather than merely
    /// returning `true`.
    #[cfg(test)]
    listener_starts: AtomicU64,
    #[cfg(test)]
    provider_update_catalog: Mutex<Option<crate::provider_catalog::ProviderDiscovery>>,
    #[cfg(test)]
    provider_update_npm_command: Mutex<Option<ProviderUpdateNpmCommand>>,
}

#[cfg(test)]
#[derive(Clone)]
enum ProviderUpdateNpmCommand {
    Resolved(std::path::PathBuf, Vec<String>),
    Missing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CliVersionFingerprint {
    modified: SystemTime,
    len: u64,
}

struct SessionWatch {
    owner: OwnerId,
    conn: Arc<ConnHandle>,
    last_snapshot: Option<Vec<devboule_protocol::SessionStateSnapshot>>,
}

fn session_state_event(
    sessions: Vec<devboule_protocol::SessionStateSnapshot>,
) -> SessionEventEnvelope {
    SessionEventEnvelope {
        // Empty session id and generation zero identify the connection-scoped
        // roster event; attachment events always carry both values.
        session_id: String::new(),
        generation: 0,
        event: SessionEvent::SessionsSnapshot { sessions },
    }
}

impl ServerState {
    #[cfg(test)]
    pub fn new(instance_id: String) -> Arc<Self> {
        static TEST_STATE_COUNTER: AtomicU64 = AtomicU64::new(1);
        let counter = TEST_STATE_COUNTER.fetch_add(1, Ordering::Relaxed);
        // Each test state needs its own SQLite path: parallel WAL writers
        // sharing one test database can legitimately hold each other locked.
        Self::with_paths(
            instance_id,
            RuntimePaths::from_dir(
                std::env::temp_dir()
                    .join(format!("devboule-test-{}-{counter}", std::process::id())),
            ),
        )
        .expect("create daemon process job")
    }

    pub fn with_paths(instance_id: String, paths: RuntimePaths) -> Result<Arc<Self>, DaemonError> {
        Self::with_paths_and_npm_install_runner(
            instance_id,
            paths,
            Arc::new(ProcessNpmInstallRunner),
        )
    }

    pub fn with_paths_and_npm_install_runner(
        instance_id: String,
        paths: RuntimePaths,
        npm_install_runner: Arc<dyn NpmInstallRunner>,
    ) -> Result<Arc<Self>, DaemonError> {
        let _ = paths.ensure_dir();
        let process_job = Arc::new(JobObject::new()?);
        let mcp = Arc::new(crate::mcp_broker::McpBroker::new(&paths.dir)?);
        // Read before `paths` moves into the session registry below.
        let tool_policy = Arc::new(crate::tool_policy::ToolPolicyStore::load(&paths.dir));
        let (journal, journal_error) = match Journal::open(&paths.journal_file()) {
            Ok(journal) => (Some(Arc::new(journal)), None),
            Err(error) => (None, Some(error.to_string())),
        };
        let journal_for_peers = journal.clone();
        let paths_for_state = paths.clone();
        // The store cell is built before the state exists: production leaves it
        // empty and `secret_store()` selects lazily, a test build pins it to the
        // file store under this runtime dir. See `initial_secret_store`.
        let secret_store = Self::initial_secret_store(&paths_for_state.dir);
        let state = Arc::new(Self {
            instance_id,
            started: Instant::now(),
            stop: Arc::new(AtomicBool::new(false)),
            lifecycle: Mutex::new(Lifecycle::default()),
            shutdown_flag: Arc::new(Mutex::new(false)),
            shutdown_cvar: Arc::new(Condvar::new()),
            idempotency: Mutex::new(IdempotencyStore::default()),
            process_job,
            mcp,
            tool_policy,
            sessions: SessionRegistry::new(paths, journal),
            conn_ids: AtomicU64::new(1),
            journal_error: Mutex::new(journal_error),
            session_watchers: Mutex::new(HashMap::new()),
            provider_health: Mutex::new(HashMap::new()),
            provider_versions: Mutex::new(HashMap::new()),
            provider_cli_versions: Mutex::new(HashMap::new()),
            claude_version_probes: Mutex::new(HashSet::new()),
            npm_install_runner,
            paths: paths_for_state,
            journal: journal_for_peers,
            secret_store,
            device_identity: OnceLock::new(),
            remote: Mutex::new(RemoteState::Disabled(
                "the remote listener is not running".to_string(),
            )),
            remote_conns: Mutex::new(HashMap::new()),
            peer_table: Mutex::new(PeerTableView::default()),
            peer_table_load: Mutex::new(()),
            peer_stop: Arc::new(AtomicBool::new(false)),
            peer_listener: Mutex::new(None),
            peer_transport: OnceLock::new(),
            pairing: Arc::new(crate::pairing::PairingService::new()),
            #[cfg(test)]
            peer_table_loads: AtomicU64::new(0),
            #[cfg(test)]
            listener_starts: AtomicU64::new(0),
            #[cfg(test)]
            provider_update_catalog: Mutex::new(None),
            #[cfg(test)]
            provider_update_npm_command: Mutex::new(None),
        });
        let state_for_transitions = Arc::downgrade(&state);
        state.sessions.set_transition_sink(Arc::new(move |owner| {
            if let Some(state) = state_for_transitions.upgrade() {
                state.broadcast_session_state(&owner);
            }
        }));
        Ok(state)
    }

    pub fn alloc_conn(&self) -> u64 {
        self.conn_ids.fetch_add(1, Ordering::Relaxed)
    }

    fn watch_sessions(&self, owner: &OwnerId, conn: &Arc<ConnHandle>) {
        let mut watchers = self
            .session_watchers
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        conn.clear_state_events();
        // Registration and the initial snapshot share the watcher lock with
        // transition broadcasts, so the first pushed change cannot overtake
        // the state that established this subscription.
        let snapshots = self.sessions.state_snapshots(owner);
        watchers.insert(
            conn.id,
            SessionWatch {
                owner: owner.clone(),
                conn: Arc::clone(conn),
                last_snapshot: Some(snapshots.clone()),
            },
        );
        conn.queue_state_event(session_state_event(snapshots));
    }

    fn unwatch_sessions(&self, conn_id: u64) {
        self.session_watchers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&conn_id);
    }

    fn broadcast_session_state(&self, owner: &OwnerId) {
        let mut watchers = self
            .session_watchers
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !watchers.values().any(|watch| watch.owner == *owner) {
            return;
        }
        let snapshots = self.sessions.state_snapshots(owner);
        for watch in watchers.values_mut().filter(|watch| watch.owner == *owner) {
            if watch.last_snapshot.as_ref() == Some(&snapshots) {
                continue;
            }
            watch.last_snapshot = Some(snapshots.clone());
            watch
                .conn
                .queue_state_event(session_state_event(snapshots.clone()));
        }
    }

    pub fn request_shutdown(&self) {
        {
            let mut lifecycle = self.lifecycle.lock().unwrap_or_else(|err| err.into_inner());
            lifecycle.shutting_down = true;
            lifecycle.idle_generation = lifecycle.idle_generation.wrapping_add(1);
        }
        self.signal_shutdown();
    }

    fn signal_shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
        let mut flag = self
            .shutdown_flag
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        *flag = true;
        self.shutdown_cvar.notify_all();
    }

    pub fn wait_until_shutdown(&self) {
        let mut flag = self
            .shutdown_flag
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        while !*flag {
            flag = self
                .shutdown_cvar
                .wait(flag)
                .unwrap_or_else(|err| err.into_inner());
        }
    }

    fn is_shutting_down(&self) -> bool {
        self.lifecycle
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .shutting_down
    }

    /// Admit a client unless shutdown has started. A reconnect that wins this
    /// lock invalidates any idle timer armed by the previous connection.
    fn client_connected(&self) -> bool {
        let mut lifecycle = self.lifecycle.lock().unwrap_or_else(|err| err.into_inner());
        if lifecycle.shutting_down {
            return false;
        }
        lifecycle.clients = lifecycle.clients.saturating_add(1);
        lifecycle.idle_generation = lifecycle.idle_generation.wrapping_add(1);
        true
    }

    fn client_disconnected(self: &Arc<Self>) {
        let generation = {
            let mut lifecycle = self.lifecycle.lock().unwrap_or_else(|err| err.into_inner());
            lifecycle.clients = lifecycle.clients.saturating_sub(1);
            if lifecycle.clients == 0 && lifecycle.sessions == 0 && !lifecycle.shutting_down {
                lifecycle.idle_generation = lifecycle.idle_generation.wrapping_add(1);
                Some(lifecycle.idle_generation)
            } else {
                None
            }
        };
        if let Some(generation) = generation {
            arm_idle_shutdown(Arc::clone(self), generation);
        }
    }

    /// Register a live daemon-owned session. Create calls this; close and a
    /// natural process exit call [`Self::session_finished`]. Detach
    /// deliberately does neither: it only removes a view, so a
    /// detached-but-alive session keeps the daemon up.
    pub fn session_started(&self) -> bool {
        let mut lifecycle = self.lifecycle.lock().unwrap_or_else(|err| err.into_inner());
        if lifecycle.shutting_down {
            return false;
        }
        lifecycle.sessions = lifecycle.sessions.saturating_add(1);
        lifecycle.idle_generation = lifecycle.idle_generation.wrapping_add(1);
        true
    }

    /// Mark a daemon-owned session as no longer alive. This may arm the idle
    /// shutdown timer when no client remains attached to the daemon.
    pub fn session_finished(self: &Arc<Self>) {
        let generation = {
            let mut lifecycle = self.lifecycle.lock().unwrap_or_else(|err| err.into_inner());
            lifecycle.sessions = lifecycle.sessions.saturating_sub(1);
            if lifecycle.clients == 0 && lifecycle.sessions == 0 && !lifecycle.shutting_down {
                lifecycle.idle_generation = lifecycle.idle_generation.wrapping_add(1);
                Some(lifecycle.idle_generation)
            } else {
                None
            }
        };
        if let Some(generation) = generation {
            arm_idle_shutdown(Arc::clone(self), generation);
        }
    }

    /// Record the measured outcome of this provider's most recent spawn +
    /// handshake. `Ok(())` measures "ok"; the error measures
    /// "failed: <reason>" with the error message collapsed to one line and
    /// capped at 200 chars. This is a last-start observation, not an auth
    /// probe: it never contacts the provider on its own.
    pub(crate) fn record_provider_health(
        &self,
        provider_id: &str,
        outcome: Result<(), &WireError>,
    ) {
        let value = match outcome {
            Ok(()) => "ok".to_string(),
            Err(error) => {
                // The handshake error embeds the agent stderr as
                // "<error> Agent stderr: <lines>". The full text stays in
                // the RPC error shown in chat; the health string lands in
                // the Settings status line and persists across renders, so
                // it must not carry stderr, which can echo tokens/paths.
                let base = error
                    .message
                    .split(" Agent stderr:")
                    .next()
                    .unwrap_or(&error.message);
                format!("failed: {}", collapse_health_reason(base))
            }
        };
        // This raw String is internal pre-boundary state. Diagnostics wraps
        // it in SafeText; any future consumer must cross that boundary too.
        self.provider_health
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .insert(provider_id.to_string(), value);
    }

    /// The measured authentication value for this provider id, or "unknown"
    /// when nothing was measured since daemon start.
    pub(crate) fn provider_health(&self, provider_id: &str) -> String {
        self.provider_health
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .get(provider_id)
            .cloned()
            .unwrap_or_else(|| "unknown".to_string())
    }

    pub(crate) fn record_provider_version(&self, provider_id: &str, version: &str) {
        let Some(version) = crate::provider_catalog::cap_external_version(version) else {
            return;
        };
        self.provider_versions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(provider_id.to_string(), version);
    }

    pub(crate) fn provider_version(&self, provider_id: &str) -> Option<String> {
        self.provider_versions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(provider_id)
            .cloned()
    }

    fn record_provider_cli_version(
        &self,
        provider_id: &str,
        version: &str,
        fingerprint: CliVersionFingerprint,
    ) {
        let Some(version) = crate::provider_catalog::cap_external_version(version) else {
            return;
        };
        self.provider_cli_versions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(provider_id.to_string(), (version, fingerprint));
    }

    fn provider_cli_version(
        &self,
        provider_id: &str,
        executable: &std::path::Path,
    ) -> Option<String> {
        let current = executable_fingerprint(executable);
        let guard = self
            .provider_cli_versions
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let (version, cached) = guard.get(provider_id)?;
        cli_version_cache_is_current(cached, current.as_ref()).then(|| version.clone())
    }

    pub(crate) fn claude_models(self: &Arc<Self>) -> crate::claude_catalog::ClaudeCatalogSnapshot {
        let Some(agent) = crate::provider_catalog::find_available("claude") else {
            return crate::claude_catalog::ClaudeCatalogSnapshot::provisional(
                crate::claude_catalog::fallback_models(),
            );
        };
        let (catalog_path, version, script) = match agent.install_channel {
            crate::provider_catalog::InstallChannel::Native => {
                let version = self
                    .provider_cli_version("claude", &agent.executable)
                    .or_else(|| {
                        std::env::var_os("DEVBOULE_TEST_NO_NETWORK").map(|_| "test".to_string())
                    });
                let Some(version) = version else {
                    self.start_claude_version_probe(agent);
                    return crate::claude_catalog::ClaudeCatalogSnapshot::provisional(
                        crate::claude_catalog::fallback_models(),
                    );
                };
                (agent.executable, version, false)
            }
            crate::provider_catalog::InstallChannel::Npm => {
                let Some(script) = agent.prefix_args.first().map(std::path::PathBuf::from) else {
                    eprintln!("Claude npm installation has no local script to scrape");
                    return crate::claude_catalog::ClaudeCatalogSnapshot::provisional(
                        crate::claude_catalog::fallback_models(),
                    );
                };
                let Some(version) = agent.installed_version else {
                    eprintln!(
                        "Claude npm installation has no package version; model catalog unavailable"
                    );
                    return crate::claude_catalog::ClaudeCatalogSnapshot::provisional(
                        crate::claude_catalog::fallback_models(),
                    );
                };
                (script, version, true)
            }
            crate::provider_catalog::InstallChannel::NpxRegistry => {
                return crate::claude_catalog::ClaudeCatalogSnapshot::provisional(
                    crate::claude_catalog::fallback_models(),
                );
            }
        };
        if let Some(models) = crate::claude_catalog::cached(self.sessions.runtime_dir(), &version) {
            return crate::claude_catalog::ClaudeCatalogSnapshot::derived(models);
        }
        self.start_claude_derivation(catalog_path, version, script);
        crate::claude_catalog::ClaudeCatalogSnapshot::provisional(
            crate::claude_catalog::fallback_models(),
        )
    }

    fn start_claude_version_probe(
        self: &Arc<Self>,
        agent: crate::provider_catalog::InstalledAgent,
    ) {
        let mut probes = self
            .claude_version_probes
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if !probes.insert(agent.executable.clone()) {
            return;
        }
        drop(probes);

        let state = Arc::clone(self);
        let executable = agent.executable.clone();
        let probe_path = agent.executable.clone();
        let cleanup_path = probe_path.clone();
        let spawn = std::thread::Builder::new()
            .name("claude-version-probe".to_string())
            .spawn(move || {
                if let Some((version, fingerprint)) = probe_native_version(&state, &agent) {
                    state.record_provider_cli_version("claude", &version, fingerprint);
                    state.start_claude_derivation(executable, version, false);
                }
                state
                    .claude_version_probes
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .remove(&cleanup_path);
            });
        if spawn.is_err() {
            self.claude_version_probes
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .remove(&probe_path);
        }
    }

    fn start_claude_derivation(
        self: &Arc<Self>,
        catalog_path: std::path::PathBuf,
        version: String,
        script: bool,
    ) {
        let state = Arc::clone(self);
        let runtime_dir = self.sessions.runtime_dir().to_path_buf();
        let source = if script {
            crate::claude_catalog::source_for_script(&catalog_path)
        } else {
            crate::claude_catalog::source_for(&catalog_path)
        };
        let _ =
            crate::claude_catalog::start_derivation(source, runtime_dir, version, move |models| {
                state.sessions.publish_claude_catalog(models)
            });
    }

    fn invalidate_provider_update_caches(&self, provider_id: &str) {
        self.provider_cli_versions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(provider_id);
        // Only the installed state changed: dropping the executable fingerprint
        // forces the next --version observation to be fresh. npm latest is a
        // registry property, so it remains valid under its six-hour TTL.
    }

    #[cfg(test)]
    fn set_provider_update_catalog(&self, discovery: crate::provider_catalog::ProviderDiscovery) {
        *self
            .provider_update_catalog
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(discovery);
    }

    #[cfg(test)]
    fn set_provider_update_npm_command(&self, program: std::path::PathBuf, prefix: Vec<String>) {
        *self
            .provider_update_npm_command
            .lock()
            .unwrap_or_else(|error| error.into_inner()) =
            Some(ProviderUpdateNpmCommand::Resolved(program, prefix));
    }

    #[cfg(test)]
    fn set_provider_update_npm_missing(&self) {
        *self
            .provider_update_npm_command
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(ProviderUpdateNpmCommand::Missing);
    }

    fn status_body(&self, request_id: u64) -> DaemonMessage {
        self.sessions.refresh_journal_degradation();
        let output_metrics = self.sessions.output_metrics();
        let lifecycle = self.lifecycle.lock().unwrap_or_else(|err| err.into_inner());
        let journal_error = self
            .journal_error
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
            .or_else(|| {
                self.sessions.has_live_journal_degradation().then(|| {
                    "Journal output is degraded; some terminal output may not be saved.".to_string()
                })
            });
        DaemonMessage::Status {
            id: request_id,
            body: DaemonStatusBody {
                instance_id: self.instance_id.clone(),
                protocol_version: PROTOCOL_VERSION,
                daemon_version: env!("CARGO_PKG_VERSION").to_string(),
                pid: std::process::id(),
                uptime_ms: u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
                clients: lifecycle.clients,
                sessions: lifecycle.sessions,
                capabilities: m3a_daemon_capabilities(),
                // Wire names predate M3.5 (they described a 256 KiB byte
                // ring). The ring is gone: these now report the bounded
                // per-attachment output queue and how often a slow viewer's
                // unsent suffix was coalesced into a fresh snapshot.
                peak_ring_bytes: output_metrics.peak_pending_bytes,
                ring_evicted_bytes: output_metrics.coalesced_bytes,
                ring_dropped_frames: output_metrics.coalesced_frames,
                journal_error,
                journal_stats: self.sessions.journal_stats().map(Box::new),
                // The selector, never the key. A peer is denied `Status`
                // precisely because this body is local-only (muse M7).
                secret_store: Some(self.secret_store().1.to_string()),
                remote: Some(Box::new(self.remote_state())),
            },
        }
    }

    /// The secret-store cell a fresh `ServerState` starts with.
    ///
    /// Production leaves the cell empty: [`ServerState::secret_store`] selects
    /// and caches the store on first use — the OS credential store when it
    /// initialises, the private file store when it does not.
    #[cfg(not(test))]
    fn initial_secret_store(_dir: &Path) -> OnceLock<(Arc<dyn SecretStore>, &'static str)> {
        OnceLock::new()
    }

    /// A test build pins the cell to the private file store under this state's
    /// runtime directory, which every test creates as its own temp dir: a
    /// test-built identity is written to `<runtime dir>/secrets/noise-static.bin`
    /// and dies with that directory.
    ///
    /// Measured, before this: a test that reached `device_identity()` got the
    /// OS credential store instead, keyed by the hash of that same temp dir, and
    /// nothing ever removed the entry. One `cargo test -p devboule-daemon` run
    /// left 146 `noise-static-<16 hex>` entries in the real Windows Credential
    /// Manager; after ~6 runs (869 entries) `CredWrite` started failing with
    /// `Windows error code 8`, which turned six pairing/transport/server tests
    /// red on `credential store failure`. The credential store is not a
    /// behaviour under test here: this key is per-test scratch, and the file
    /// store is the one the integration tests and a headless daemon already use
    /// (`DEVBOULE_SECRET_STORE=file`).
    #[cfg(test)]
    fn initial_secret_store(dir: &Path) -> OnceLock<(Arc<dyn SecretStore>, &'static str)> {
        let store: Arc<dyn SecretStore> = Arc::new(crate::secret_store::FileStore::new(dir));
        let cell = OnceLock::new();
        let _ = cell.set((store, crate::secret_store::SecretStoreKind::File.as_str()));
        cell
    }

    /// The selected secret store: the OS credential store when it
    /// initialises, the private file store otherwise. Selected once, lazily,
    /// so a unit test that builds a `ServerState` does not read the OS
    /// credential store unless it asks about the remote listener — and in a
    /// test build the cell is already pinned by
    /// [`ServerState::initial_secret_store`], so the selector below is never
    /// reached from a test at all.
    fn secret_store(&self) -> &(Arc<dyn SecretStore>, &'static str) {
        self.secret_store.get_or_init(|| {
            let (store, kind) = crate::secret_store::select_secret_store(&self.paths);
            (store, kind.as_str())
        })
    }

    fn remote_state(&self) -> devboule_protocol::RemoteState {
        self.remote
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .to_wire()
    }

    /// This device's live peers, revoked rows included. The transport filters
    /// to the non-revoked ones; the Devices panel shows both.
    pub(crate) fn peers(&self) -> Result<Vec<crate::journal::PeerRecord>, String> {
        let Some(journal) = &self.journal else {
            return Err("the journal is unavailable".to_string());
        };
        journal.peers_list().map_err(|error| error.to_string())
    }

    /// The cached `peers` table the accept path filters on (M1).
    ///
    /// Loaded at most once per [`PEER_TABLE_TTL`], and dropped by every peer
    /// mutation, so the cost of accepting a connection is a mutex read rather
    /// than a journal round trip.
    pub(crate) fn peer_table(&self) -> Result<Arc<crate::peer_transport::PeerTable>, String> {
        let now = Instant::now();
        {
            let cache = self
                .peer_table
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some((loaded_at, table)) = cache.loaded.as_ref() {
                if now.saturating_duration_since(*loaded_at) < cache.ttl {
                    return Ok(Arc::clone(table));
                }
            }
        }
        // One loader at a time, and the cache is re-checked after acquiring it:
        // a burst of connections on a cold cache must produce one journal read,
        // not one each. The cache mutex itself is not held across the journal
        // call, so a revoke is never blocked behind it.
        let _loading = self
            .peer_table_load
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let now = Instant::now();
        {
            let cache = self
                .peer_table
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some((loaded_at, table)) = cache.loaded.as_ref() {
                if now.saturating_duration_since(*loaded_at) < cache.ttl {
                    return Ok(Arc::clone(table));
                }
            }
        }
        let table = Arc::new(crate::peer_transport::PeerTable::load(self)?);
        #[cfg(test)]
        self.peer_table_loads.fetch_add(1, Ordering::Relaxed);
        let mut cache = self
            .peer_table
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        cache.loaded = Some((now, Arc::clone(&table)));
        Ok(table)
    }

    /// Forget the cached peer table. Called by every path that changes the
    /// `peers` table, so a pairing or a revoke is visible to the next accepted
    /// connection rather than up to a TTL later.
    fn invalidate_peer_table(&self) {
        self.peer_table
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .loaded = None;
    }

    /// Test-only: how many times the table was really loaded, which is what
    /// proves the cache is doing something.
    #[cfg(test)]
    pub(crate) fn peer_table_loads(&self) -> u64 {
        self.peer_table_loads.load(Ordering::Relaxed)
    }

    /// Test-only: drive expiry without waiting for [`PEER_TABLE_TTL`].
    #[cfg(test)]
    pub(crate) fn set_peer_table_ttl(&self, ttl: Duration) {
        self.peer_table
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .ttl = ttl;
    }

    /// This device's identity and Noise static key, loaded at most once.
    ///
    /// Borrowed rather than cloned: the private key is inside, and the value is
    /// the same for the lifetime of the daemon.
    pub(crate) fn device_identity(
        &self,
    ) -> &Result<
        Arc<crate::device_identity::DeviceIdentity>,
        crate::device_identity::DeviceIdentityError,
    > {
        self.device_identity.get_or_init(|| {
            let (store, _) = self.secret_store();
            crate::device_identity::load_or_create(&self.paths, store.as_ref()).map(Arc::new)
        })
    }

    /// The shutdown flag the peer accept loop polls.
    pub(crate) fn stop_flag(&self) -> &Arc<AtomicBool> {
        &self.stop
    }

    /// This device's own user SID, or `None` on a platform without one. This
    /// is what `paired_by_user` records: the person here who ran the pairing,
    /// never a value from the wire.
    #[cfg(windows)]
    pub(crate) fn local_user_sid(&self) -> Option<String> {
        crate::security::current_user_sid().ok()
    }

    #[cfg(not(windows))]
    pub(crate) fn local_user_sid(&self) -> Option<String> {
        None
    }

    pub(crate) fn peer_upsert(&self, record: PeerRecord) -> Result<PeerRecord, String> {
        let journal = self
            .journal
            .as_ref()
            .ok_or_else(|| "the journal is unavailable".to_string())?;
        let stored = journal
            .peer_upsert(record)
            .map_err(|error| error.to_string())?;
        self.invalidate_peer_table();
        Ok(stored)
    }

    pub(crate) fn peer_get(&self, device_id: &str) -> Result<Option<PeerRecord>, String> {
        let journal = self
            .journal
            .as_ref()
            .ok_or_else(|| "the journal is unavailable".to_string())?;
        journal
            .peer_get(device_id)
            .map_err(|error| error.to_string())
    }

    pub(crate) fn peer_revoke(&self, device_id: &str, at: i64) -> Result<PeerMutation, String> {
        let journal = self
            .journal
            .as_ref()
            .ok_or_else(|| "the journal is unavailable".to_string())?;
        let outcome = journal
            .peer_revoke(device_id, at)
            .map_err(|error| error.to_string())?;
        // The next accepted connection must see the revocation, not a cached
        // row that still owns the address.
        self.invalidate_peer_table();
        Ok(outcome)
    }

    pub(crate) fn peer_set_caps(
        &self,
        device_id: &str,
        caps: Vec<String>,
    ) -> Result<PeerMutation, String> {
        let journal = self
            .journal
            .as_ref()
            .ok_or_else(|| "the journal is unavailable".to_string())?;
        let outcome = journal
            .peer_set_caps(device_id, caps)
            .map_err(|error| error.to_string())?;
        self.invalidate_peer_table();
        Ok(outcome)
    }

    /// The capability set of one paired device, read from its `peers` row.
    ///
    /// Resolved once per connection, not once per request: the journal is the
    /// slow path this design keeps out of dispatch (muse M1), and a
    /// `PeerSetCaps` closes that device's live connections, so a running
    /// connection can never hold a capability the row no longer grants.
    ///
    /// A missing row, an unreadable journal, a revoked device and an unknown
    /// device all yield an empty set, which `peer_allows` reads as "holds
    /// nothing": the fail-closed direction.
    pub(crate) fn peer_caps(&self, device_id: &str) -> Vec<String> {
        let Ok(peers) = self.peers() else {
            return Vec::new();
        };
        peers
            .into_iter()
            .find(|record| record.device_id == device_id && !record.is_revoked())
            .map(|record| record.caps)
            .unwrap_or_default()
    }

    /// Record a live remote connection so a revoke can close it.
    pub(crate) fn register_remote_conn(&self, conn_id: u64, device_id: &str) -> Arc<AtomicBool> {
        let close = Arc::new(AtomicBool::new(false));
        self.remote_conns
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(conn_id, (device_id.to_string(), Arc::clone(&close)));
        close
    }

    pub(crate) fn unregister_remote_conn(&self, conn_id: u64) {
        self.remote_conns
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&conn_id);
    }

    /// Close every live connection of `device_id`. Returns how many were
    /// asked to close; the flag is what actually drops them, on the next turn
    /// of their own loop.
    pub(crate) fn revoke_peer_connections(&self, device_id: &str) -> usize {
        // The close flags are collected under the lock and raised after it is
        // released. Iterating the map while setting an atomic is already
        // O(connections) with a cap of 32, but this keeps the accept loop's
        // `register_remote_conn` from ever queueing behind a revoke on a busy
        // connection set.
        let closes: Vec<Arc<AtomicBool>> = self
            .remote_conns
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .values()
            .filter(|(owner, _)| owner == device_id)
            .map(|(_, close)| Arc::clone(close))
            .collect();
        for close in &closes {
            close.store(true, Ordering::SeqCst);
        }
        closes.len()
    }

    pub(crate) fn is_peer_online(&self, device_id: &str) -> bool {
        self.remote_conns
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .values()
            .any(|(owner, _)| owner == device_id)
    }

    /// The addresses `Status.remote` and `SelfInfo` advertise.
    pub(crate) fn remote_addresses(&self) -> Vec<String> {
        self.remote
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .addresses()
    }

    pub(crate) fn remote_port(&self) -> Option<u16> {
        self.remote
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .port()
    }

    /// The pairing service: the displayed code, its lockouts, and the parked
    /// confirmations.
    pub(crate) fn pairing(&self) -> &Arc<crate::pairing::PairingService> {
        &self.pairing
    }

    /// The peer transport, defaulting to the tailnet. `set_peer_transport` is
    /// how a test substitutes a stub that answers `pre_noise_filter` and
    /// `binding` without Tailscale.
    pub(crate) fn peer_transport(&self) -> Arc<dyn crate::peer_transport::PeerTransport> {
        Arc::clone(self.peer_transport.get_or_init(|| {
            Arc::new(crate::peer_transport::Tailnet)
                as Arc<dyn crate::peer_transport::PeerTransport>
        }))
    }

    /// Install the transport before the peer listener starts. `Ok(())` means it
    /// was not already chosen.
    #[cfg(test)]
    pub(crate) fn set_peer_transport(
        &self,
        transport: Arc<dyn crate::peer_transport::PeerTransport>,
    ) -> Result<(), Arc<dyn crate::peer_transport::PeerTransport>> {
        self.peer_transport.set(transport)
    }

    /// Ensure the tailnet listener is running, starting it if it is not.
    ///
    /// Idempotent, and safe to call from more than one place: the mutex means
    /// two concurrent callers cannot both bind (the second would lose the port
    /// race and overwrite `Enabled` with `Disabled`), and the slot check inside
    /// means the second finds the first's listener instead of starting another.
    ///
    /// Returns whether a listener is up after the call. A failure leaves
    /// `Status.remote` saying why, which is what the panel renders.
    pub(crate) fn ensure_remote_listener(self: &Arc<Self>) -> bool {
        let mut slot = self
            .peer_listener
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if slot.is_some() {
            return true;
        }
        // Once the listener has been stopped the daemon is on its way down, and
        // the stop flag would make a fresh loop exit immediately. Starting one
        // would leave a `listening` state with nothing behind it.
        if self.peer_stop.load(Ordering::SeqCst) {
            return false;
        }
        match try_start_remote_listener(self) {
            Some(handle) => {
                #[cfg(test)]
                self.listener_starts.fetch_add(1, Ordering::Relaxed);
                *slot = Some(handle);
                true
            }
            None => false,
        }
    }

    /// Test-only: how many listeners were really started.
    #[cfg(test)]
    pub(crate) fn listener_starts(&self) -> u64 {
        self.listener_starts.load(Ordering::Relaxed)
    }

    /// Stop the tailnet listener if one is running and join it.
    ///
    /// Bounded, like every other join in this file: the accept loop polls its
    /// stop flag, so this is one `HOUSEKEEPING_TICK` at worst.
    pub(crate) fn stop_remote_listener(&self) {
        self.peer_stop.store(true, Ordering::SeqCst);
        let handle = self
            .peer_listener
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        if let Some(handle) = handle {
            bounded_join(handle, JOIN_BUDGET);
        }
    }

    /// Whether a tailnet listener is currently held. Test-only.
    #[cfg(test)]
    pub(crate) fn has_remote_listener(&self) -> bool {
        self.peer_listener
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_some()
    }

    /// Replace the remote-listener state. Called by the peer accept loop when
    /// it binds (and by the start-up path when it cannot).
    pub(crate) fn set_remote_state(&self, state: RemoteState) {
        *self
            .remote
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = state;
    }

    /// Append one audit row.
    ///
    /// The request it describes proceeds either way: losing the trail is bad,
    /// refusing a legitimate request because the trail could not be written is
    /// worse. The line names the action and the error only — never a peer
    /// identity, a key or a code.
    pub(crate) fn audit(&self, record: AuditRecord) {
        let action = record.action.clone();
        let Some(journal) = &self.journal else {
            eprintln!("daemon could not audit {action}: the journal is unavailable");
            return;
        };
        if let Err(error) = journal.audit_append(record) {
            eprintln!("daemon could not audit {action}: {error}");
        }
    }
}

/// Begin shutdown only if the lifecycle snapshot that armed this timer is
/// still current. The lifecycle mutex makes the final check and the shutdown
/// transition atomic with client reconnects and session transitions.
fn arm_idle_shutdown(state: Arc<ServerState>, generation: u64) {
    let _ = std::thread::Builder::new()
        .name("daemon-idle".into())
        .spawn(move || {
            std::thread::sleep(IDLE_SHUTDOWN_GRACE);
            let should_shutdown = {
                let mut lifecycle = state
                    .lifecycle
                    .lock()
                    .unwrap_or_else(|err| err.into_inner());
                let should_shutdown = lifecycle.idle_generation == generation
                    && lifecycle.clients == 0
                    && lifecycle.sessions == 0
                    && !lifecycle.shutting_down;
                if should_shutdown {
                    lifecycle.shutting_down = true;
                    lifecycle.idle_generation = lifecycle.idle_generation.wrapping_add(1);
                }
                should_shutdown
            };
            if should_shutdown {
                state.signal_shutdown();
            }
        });
}

fn diagnostics_reply(state: &Arc<ServerState>, request_id: u64, owner: &OwnerId) -> DaemonMessage {
    match diagnostics_report(state, owner) {
        Ok(report) => match serde_json::to_value(report) {
            Ok(report) => DaemonMessage::Diagnostics {
                id: request_id,
                report,
            },
            Err(error) => DaemonMessage::Error(
                WireError::new(
                    ErrorCode::Internal,
                    format!("could not encode diagnostics: {error}"),
                )
                .with_id(request_id),
            ),
        },
        Err(error) => DaemonMessage::Error(error.with_id(request_id)),
    }
}

#[cfg(windows)]
fn host_os_version() -> String {
    use std::mem::MaybeUninit;
    use windows_sys::Wdk::System::SystemServices::RtlGetVersion;
    use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOW;

    let mut info = MaybeUninit::<OSVERSIONINFOW>::zeroed();
    let size = std::mem::size_of::<OSVERSIONINFOW>() as u32;
    // RtlGetVersion reports the kernel version without the GetVersionEx
    // compatibility shim, which otherwise makes an unmanifested process look
    // like Windows 8. This is a bounded, local API; no shell command or
    // user-provided executable path is involved.
    unsafe {
        (*info.as_mut_ptr()).dwOSVersionInfoSize = size;
        if RtlGetVersion(info.as_mut_ptr()) == 0 {
            let info = info.assume_init();
            return format!(
                "Windows {}.{}.{} ({})",
                info.dwMajorVersion,
                info.dwMinorVersion,
                info.dwBuildNumber,
                std::env::consts::ARCH
            );
        }
    }
    format!("Windows ({})", std::env::consts::ARCH)
}

#[cfg(not(windows))]
fn host_os_version() -> String {
    format!("{} ({})", std::env::consts::OS, std::env::consts::ARCH)
}

fn diagnostics_report(
    state: &Arc<ServerState>,
    owner: &OwnerId,
) -> Result<DiagnosticsReport, WireError> {
    state.sessions.refresh_journal_degradation();
    let sessions = state.sessions.list(owner)?;
    let output_metrics = state.sessions.output_metrics();
    let lifecycle = state
        .lifecycle
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let clients = lifecycle.clients;
    let daemon_sessions = lifecycle.sessions;
    drop(lifecycle);

    // These are the same bounded provider discovery and journal queries used
    // by existing RPCs: registry fetches and journal worker calls have finite
    // deadlines. Diagnostics never waits on a child process or an unbounded
    // database operation.
    let providers = match providers_reply(state, 0, false) {
        DaemonMessage::Providers { providers, .. } => providers,
        _ => Vec::new(),
    };
    let (journal_file_bytes, journal_file_error) = match state.sessions.journal_file_bytes() {
        Some(Ok(bytes)) => (Some(bytes), None),
        Some(Err(error)) => (None, Some(error)),
        None => (None, None),
    };
    let mut journal_error = state
        .journal_error
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
        .or_else(|| {
            state
                .sessions
                .has_live_journal_degradation()
                .then(|| "Journal output is degraded; some output may not be saved.".to_string())
        });
    if let Some(error) = journal_file_error {
        journal_error = Some(match journal_error {
            Some(previous) => format!("{previous}; journal file size: {error}"),
            None => format!("journal file size: {error}"),
        });
    }

    Ok(DiagnosticsReport::new(DiagnosticsInput {
        instance_id: state.instance_id.clone(),
        daemon_version: env!("CARGO_PKG_VERSION").to_string(),
        protocol_version: PROTOCOL_VERSION,
        pid: std::process::id(),
        uptime_ms: u64::try_from(state.started.elapsed().as_millis()).unwrap_or(u64::MAX),
        clients,
        daemon_sessions,
        capabilities: m3a_daemon_capabilities()
            .into_iter()
            .map(|capability| capability.as_str().to_string())
            .collect(),
        peak_ring_bytes: output_metrics.peak_pending_bytes,
        ring_evicted_bytes: output_metrics.coalesced_bytes,
        ring_dropped_frames: output_metrics.coalesced_frames,
        journal_stats: state.sessions.journal_stats(),
        journal_error,
        journal_schema_version: JOURNAL_SCHEMA_VERSION,
        journal_file_bytes,
        sessions,
        providers,
        os_version: host_os_version(),
        app_version: env!("CARGO_PKG_VERSION").to_string(),
        runtime_dir: state.sessions.runtime_dir().to_string_lossy().into_owned(),
        pipe_name: state.sessions.pipe_name().to_string(),
        login_shell_capture: login_shell_capture_outcome(),
    }))
}

pub fn run() -> Result<(), DaemonError> {
    #[cfg(not(windows))]
    {
        return Err(DaemonError::UnsupportedPlatform);
    }
    #[cfg(windows)]
    {
        run_windows()
    }
}

#[cfg(windows)]
fn run_windows() -> Result<(), DaemonError> {
    let paths = RuntimePaths::from_env()?;
    let mut lock = SingleInstanceLock::acquire(&paths)?;
    let pid = std::process::id();
    let instance_id = format!(
        "{pid}-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_millis())
            .unwrap_or(0)
    );
    lock.write_identity(pid, &instance_id, &paths.pipe_name)?;

    let state = ServerState::with_paths(instance_id, paths.clone())?;
    // Attachments survive a session close that never ran, because the daemon was
    // killed first. Sweep the ones past the retention window on every start:
    // this is the fallback existence's only reason to be here.
    let swept = state.sessions.sweep_attachments(SystemTime::now());
    if swept > 0 {
        eprintln!("daemon removed {swept} attachment folder(s) left by sessions that never closed");
    }
    let mcp_server = state.mcp.start(&state).map_err(DaemonError::from)?;
    let (listener, shutdown) = transport::bind(&paths, Arc::clone(&state.stop))?;
    let accept_state = Arc::clone(&state);
    let accept = std::thread::Builder::new()
        .name("daemon-accept".into())
        .spawn(move || accept_loop(listener, accept_state))
        .map_err(DaemonError::from)?;

    // The peer listener is best-effort and runs beside the pipe: no Tailscale,
    // no tailnet address, or a missing key leaves the daemon local-only and
    // says why in `Status.remote` rather than failing to start. It is no longer
    // a one-shot attempt (C5): `pairing_address` retries through the same
    // function, so a user who starts Tailscale and shows a code again gets a
    // listener rather than the same refusal until the daemon restarts.
    let _ = state.ensure_remote_listener();

    state.wait_until_shutdown();
    // Flush the conversation journal before the listener is torn down so a
    // clean shutdown does not drop the last coalesced frames.
    state.sessions.flush_journal();
    shutdown.shutdown();
    // The peer loop polls, so its stop is a flag rather than a wake-up connect.
    state.stop_remote_listener();
    let deadline = Instant::now() + JOIN_BUDGET;
    while !accept.is_finished() && Instant::now() < deadline {
        let _ = transport::connect(&paths);
        std::thread::sleep(JOIN_SLICE);
    }
    bounded_join(accept, JOIN_SLICE);
    drop(mcp_server);
    drop(lock);
    Ok(())
}

/// Bind and serve the tailnet listener, or record why it is not up.
///
/// The body of [`ServerState::ensure_remote_listener`], split out so the
/// idempotence and the join handle live with the state that owns them.
#[cfg(windows)]
fn try_start_remote_listener(state: &Arc<ServerState>) -> Option<JoinHandle<()>> {
    // A missing key is a refusal, not an environment fact: creating a new one
    // would silently orphan every pairing this device has.
    if let Err(error) = state.device_identity() {
        state.set_remote_state(match error {
            crate::device_identity::DeviceIdentityError::KeyMissing => RemoteState::KeyMissing,
            other => RemoteState::Disabled(other.to_string()),
        });
        return None;
    }
    let transport = state.peer_transport();
    // `_fresh` inside `Tailnet::listen` bypasses the LocalAPI `Absent` cache, so
    // a retry after the user starts Tailscale really probes instead of reading
    // a cached "not running" for up to the cache TTL.
    let listener = match transport.listen(&state.paths, Arc::clone(&state.peer_stop)) {
        Ok(listener) => listener,
        Err(error) => {
            state.set_remote_state(RemoteState::Disabled(error.to_string()));
            return None;
        }
    };
    // The pairing service is the same object the RPCs use, so a code shown in
    // the panel is the code this listener accepts, and a parked confirmation
    // is visible to `DevicesList`. Coerced to the trait object here rather than
    // stored as one: the state's field is the concrete type the RPCs call.
    let pairing: Arc<dyn crate::peer_transport::PairingHook> = state.pairing().clone();
    // Read the bound addresses **before** the listener moves into the accept
    // thread; `listener.addrs()` is the only thing that knows the real port.
    let addresses: Vec<std::net::IpAddr> = listener.addrs().iter().map(|addr| addr.ip()).collect();
    let port = listener
        .addrs()
        .first()
        .map(|addr| addr.port())
        .unwrap_or_else(crate::peer_transport::peer_port);
    let accept_state = Arc::clone(state);
    let handle = std::thread::Builder::new()
        .name("daemon-peer-accept".into())
        .spawn(move || accept_peers(listener, transport, accept_state, pairing))
        .ok()?;
    // Published only once the thread is actually running, so `Status.remote` can
    // never say `listening` with nothing behind it.
    state.set_remote_state(RemoteState::Enabled { addresses, port });
    Some(handle)
}

#[cfg(not(windows))]
fn try_start_remote_listener(state: &Arc<ServerState>) -> Option<JoinHandle<()>> {
    state.set_remote_state(RemoteState::Disabled(
        "the daemon does not run on this platform yet".to_string(),
    ));
    None
}

fn accept_loop(mut listener: transport::BoundListener, state: Arc<ServerState>) {
    let mut threads: Vec<JoinHandle<()>> = Vec::new();
    loop {
        if state.stop.load(Ordering::SeqCst) {
            break;
        }
        match listener.accept() {
            Ok(stream) => {
                if !state.client_connected() {
                    reject_shutting_down(stream);
                    break;
                }
                let conn_state = Arc::clone(&state);
                match std::thread::Builder::new()
                    .name("daemon-client".into())
                    .spawn(move || {
                        if let Err(error) =
                            handle_client(Framed::new(stream), conn_state.clone(), None)
                        {
                            eprintln!("daemon client connection failed: {error}");
                        }
                        conn_state.client_disconnected();
                    }) {
                    Ok(handle) => threads.push(handle),
                    Err(_) => {
                        state.client_disconnected();
                    }
                }
            }
            Err(_) if state.stop.load(Ordering::SeqCst) => break,
            Err(_) => {
                if state.stop.load(Ordering::SeqCst) {
                    break;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        threads.retain(|handle| !handle.is_finished());
    }
    for handle in threads {
        bounded_join(handle, JOIN_BUDGET);
    }
}

/// Serve one connection until it closes.
///
/// `conn_peer` is `Some(ConnPeer::Remote {..})` when this connection arrived
/// over Noise: its identity was authenticated before this call. A pipe
/// connection passes `None` and is identified by the kernel through the pipe
/// handle. The two are never mixed: a remote peer has no pipe handle at all.
pub(crate) fn handle_client(
    framed: Framed,
    state: Arc<ServerState>,
    conn_peer: Option<ConnPeer>,
) -> Result<(), DaemonError> {
    if state.is_shutting_down() {
        send_shutting_down(&framed, None)?;
        return Ok(());
    }
    let hello: ClientMessage = framed.recv_timeout(HANDSHAKE_TIMEOUT)?;
    let ClientMessage::Hello(client_hello) = hello else {
        framed.send(&DaemonMessage::Error(WireError::new(
            ErrorCode::InvalidRequest,
            "first frame must be hello",
        )))?;
        return Ok(());
    };
    if state.is_shutting_down() {
        send_shutting_down(&framed, None)?;
        return Ok(());
    }
    // `as_file()` is `Option` because a stream connection has no pipe handle;
    // that case is routine, so it is a branch and never an unwrap.
    #[cfg(windows)]
    let peer: Option<crate::agent_report::PeerIdentity> = match framed.as_file() {
        Some(file) => match transport::peer_identity(&file) {
            Ok(peer) => Some(peer),
            Err(error) => {
                eprintln!("could not derive named-pipe peer identity: {error}");
                let _ = framed.send(&DaemonMessage::Error(WireError::new(
                    ErrorCode::Unauthorized,
                    "Could not verify the daemon client identity.",
                )));
                return Err(DaemonError::Io(error));
            }
        },
        None => None,
    };
    #[cfg(not(windows))]
    let peer: Option<crate::agent_report::PeerIdentity> = None;

    // Authority is `OwnerId.user`: a kernel SID (starts with `S-` on Windows)
    // for a local client, `peer_<device_id>` for a remote one.
    let true_owner = match &conn_peer {
        Some(ConnPeer::Remote {
            device_id, role, ..
        }) => OwnerId::new(format!("peer_{device_id}"), role.as_str())
            .map_err(DaemonError::Protocol)?,
        _ => match &peer {
            Some(peer) => match OwnerId::new(peer.user.clone(), format!("process-{}", peer.pid)) {
                Ok(owner) => owner,
                Err(message) => {
                    let _ = framed.send(&DaemonMessage::Error(WireError::new(
                        ErrorCode::Unauthorized,
                        "Could not verify the daemon client identity.",
                    )));
                    return Err(DaemonError::Protocol(message));
                }
            },
            // No pipe identity on this platform: today's behaviour, the hello
            // owner label. The peer case is handled above.
            None => client_hello.owner.clone(),
        },
    };
    if client_hello.owner != true_owner {
        // Redacted, not printed: this line used to carry the user SID and, on
        // a peer connection, `peer_<device_id>` — both of which §8 R7 keeps out
        // of logs. The mismatch is still diagnosable; the identities are not in
        // the file.
        eprintln!(
            "client hello owner label {} did not match the connection peer {}",
            crate::device_identity::redact(&client_hello.owner.user),
            crate::device_identity::redact(&true_owner.user)
        );
    }
    let daemon_hello = daemon_hello(&state);
    let agreed = match negotiate(&client_hello, &daemon_hello) {
        Ok(agreed) => {
            // The client learns the usable capability set from this hello;
            // do not expose daemon-only capabilities as if they were agreed.
            let mut agreed_hello = daemon_hello.clone();
            agreed_hello.capabilities = agreed.capabilities.clone();
            framed.send(&DaemonMessage::Hello(agreed_hello))?;
            agreed
        }
        Err(error) => {
            framed.send(&DaemonMessage::Error(error))?;
            return Ok(());
        }
    };
    let sessions_ok = agreed
        .capabilities
        .iter()
        .any(|capability| capability.as_str() == caps::SESSIONS);
    let journal_ok = agreed
        .capabilities
        .iter()
        .any(|capability| capability.as_str() == caps::JOURNAL);
    let typed_permissions_ok = agreed
        .capabilities
        .iter()
        .any(|capability| capability.as_str() == caps::TYPED_PERMISSIONS);
    let devices_ok = agreed
        .capabilities
        .iter()
        .any(|capability| capability.as_str() == caps::DEVICES);
    // The hello owner is diagnostic only. All idempotency and session access
    // below use the identity decided above.
    //
    // A paired `Client` speaks for the person who paired it, so every session
    // request it makes is that user's request: the registry's owner-user filter
    // is then the whole scope (§8b A3), and it is also what makes a
    // peer-created session appear in the desktop's list. A `Daemon` peer keeps
    // the `peer_<device_id>` identity slice 1 gave it, so the same filter
    // scopes it to the sessions it created (R2).
    let owner = match &conn_peer {
        Some(ConnPeer::Remote {
            role: PeerRole::Client,
            paired_by_user: Some(paired),
            ..
        }) => OwnerId::new(paired.clone(), PeerRole::Client.as_str())
            .unwrap_or_else(|_| true_owner.clone()),
        _ => true_owner.clone(),
    };
    // The peer's capability set, resolved once from its `peers` row: the gate
    // reads it on every request, and a `PeerSetCaps` closes this connection, so
    // a running connection can never hold a capability the row dropped.
    let peer_caps = match &conn_peer {
        Some(ConnPeer::Remote { device_id, .. }) => state.peer_caps(device_id),
        _ => Vec::new(),
    };
    let conn = ConnHandle::with_peer_caps(state.alloc_conn(), peer, conn_peer.clone(), peer_caps);
    let (request_tx, request_rx) = mpsc::sync_channel(64);
    let reader_wake = Arc::clone(&conn.outbound);
    let reader_framed = framed.clone();
    let reader = std::thread::Builder::new()
        .name("daemon-client-request".into())
        .spawn(move || read_client_requests(reader_framed, request_tx, reader_wake))
        .map_err(DaemonError::from)?;
    let mut pending_events = VecDeque::new();
    let mut pending_state_events = VecDeque::new();
    let mut pending_replies = VecDeque::new();
    // Remote connections are rate limited; a local pipe is not (muse M1).
    let is_remote = matches!(conn.conn_peer, Some(ConnPeer::Remote { .. }));
    let mut bucket = TokenBucket::new(Instant::now());
    // A revocation must drop a live connection, so the connection registers
    // itself and polls the flag its own revoke sets.
    let close_requested = match &conn.conn_peer {
        Some(ConnPeer::Remote { device_id, .. }) => {
            Some(state.register_remote_conn(conn.id, device_id))
        }
        _ => None,
    };
    let loop_result = (|| -> Result<(), DaemonError> {
        loop {
            if state.stop.load(Ordering::SeqCst) {
                break;
            }
            if close_requested
                .as_ref()
                .is_some_and(|close| close.load(Ordering::SeqCst))
            {
                break;
            }
            let observed_generation = conn.outbound.wake_generation();
            if pending_replies.is_empty() {
                pending_replies.extend(conn.outbound.pull_replies());
            }
            if let Some(reply) = pending_replies.pop_front() {
                framed.send(&redact_for_conn(&conn, reply))?;
                continue;
            }
            let (request, request_channel_closed) = match request_rx.try_recv() {
                Ok(request) => (Some(request), false),
                Err(TryRecvError::Empty) => (None, false),
                Err(TryRecvError::Disconnected) => (None, true),
            };
            if request_channel_closed {
                break;
            }
            if let Some(request) = request {
                let request = match request {
                    Ok(request) => request,
                    Err(error) => {
                        if connection_closed(&error) || state.stop.load(Ordering::SeqCst) {
                            break;
                        }
                        return Err(error);
                    }
                };
                let close_request = matches!(&request, ClientMessage::SessionClose { .. });
                if is_remote && !bucket.take(Instant::now()) {
                    // Exactly one audit row, then close: the audit table must
                    // not amplify a flood.
                    if let Some(ConnPeer::Remote {
                        device_id, role, ..
                    }) = &conn.conn_peer
                    {
                        state.audit(AuditRecord {
                            device_id: device_id.clone(),
                            role: role.as_str().to_string(),
                            claimed_origin: None,
                            action: "rate_limited".to_string(),
                            session_id: None,
                            outcome: "denied".to_string(),
                        });
                    }
                    break;
                }
                if drains_events_before_dispatch(&request) {
                    // A close must leave the pull state alive for the
                    // post-dispatch pull: teardown_session joins the
                    // coalescer and may publish its final output there.
                    if !close_request {
                        refill_pending_events(&conn, &mut pending_events);
                        refill_pending_state_events(&conn, &mut pending_state_events);
                    }
                    drain_pending_events(&framed, &conn, &mut pending_events, &state.sessions)?;
                    drain_pending_state_events(&framed, &mut pending_state_events)?;
                } else {
                    // Give the event stream one turn before every ordinary
                    // request. This is deliberately one frame, not a bulk
                    // drain: a continuously replenished request stream cannot
                    // starve output, while a DSR/control request waits behind
                    // at most the single event write already in progress.
                    refill_pending_events(&conn, &mut pending_events);
                    refill_pending_state_events(&conn, &mut pending_state_events);
                    if let Some(event) = pending_events.pop_front() {
                        send_pending_event(&framed, &conn, event, &state.sessions)?;
                    } else if let Some(event) = pending_state_events.pop_front() {
                        send_state_event(&framed, event)?;
                    }
                }
                if let ClientMessage::Hello(_) = request {
                    let id = request.request_id();
                    let mut error =
                        WireError::new(ErrorCode::InvalidRequest, "hello already completed");
                    if let Some(id) = id {
                        error = error.with_id(id);
                    }
                    framed.send(&DaemonMessage::Error(error))?;
                    continue;
                }
                let Some(reply) = dispatch(
                    &state,
                    &owner,
                    request,
                    &conn,
                    sessions_ok,
                    journal_ok,
                    typed_permissions_ok,
                    devices_ok,
                ) else {
                    continue;
                };
                if close_request {
                    // SessionClose joins the coalescer and calls finish(),
                    // which can publish the teardown tail after the pre-drain.
                    refill_pending_events(&conn, &mut pending_events);
                    refill_pending_state_events(&conn, &mut pending_state_events);
                    drain_pending_events(&framed, &conn, &mut pending_events, &state.sessions)?;
                    drain_pending_state_events(&framed, &mut pending_state_events)?;
                }
                let shutting_down = matches!(reply, DaemonMessage::Shutdown { accepted: true, .. });
                // Control/lifecycle replies retain the flush barrier. It makes
                // the acknowledgement visible before teardown or a shutdown
                // disconnect; the event stream below must never use that
                // barrier per frame.
                framed.send(&redact_for_conn(&conn, reply))?;
                if shutting_down {
                    state.request_shutdown();
                    break;
                }
                continue;
            }

            if pending_events.is_empty() && pending_state_events.is_empty() {
                if pending_replies.is_empty() {
                    pending_replies.extend(conn.outbound.pull_replies());
                }
                if let Some(reply) = pending_replies.pop_front() {
                    framed.send(&redact_for_conn(&conn, reply))?;
                    continue;
                }
                refill_pending_events(&conn, &mut pending_events);
                refill_pending_state_events(&conn, &mut pending_state_events);
                if pending_events.is_empty() && pending_state_events.is_empty() {
                    if !conn
                        .outbound
                        .wait_for_notify_since(observed_generation, conn.next_exit_wake())
                    {
                        break;
                    }
                    continue;
                }
            }

            // Send at most one event before looking for control traffic again.
            // In particular, no bulk output batch can hold a DSR, resize, or
            // kill request behind a sequence of flushes.
            if let Some(event) = pending_events.pop_front() {
                send_pending_event(&framed, &conn, event, &state.sessions)?;
            } else {
                let event = pending_state_events
                    .pop_front()
                    .expect("state event queue was checked above");
                send_state_event(&framed, event)?;
            }
        }
        Ok(())
    })();

    // Stop the request reader before the final pull so no new request/error
    // can race connection cleanup. This path is shared by normal disconnects,
    // write/read errors, daemon shutdown, and idle exit.
    framed.cancel_read();
    conn.outbound.close();
    bounded_join(reader, JOIN_BUDGET);
    refill_pending_events(&conn, &mut pending_events);
    refill_pending_state_events(&conn, &mut pending_state_events);
    if let Err(error) = drain_pending_events(&framed, &conn, &mut pending_events, &state.sessions) {
        eprintln!("daemon connection final event drain failed: {error}");
    }
    if let Err(error) = drain_pending_state_events(&framed, &mut pending_state_events) {
        eprintln!("daemon connection final state event drain failed: {error}");
    }
    // This is the deliberate teardown-only pipe barrier: it makes every frame
    // accepted above client-readable before the server drops this connection.
    // FlushFileBuffers stays out of the per-frame event path because it waits
    // for the client to consume the pipe.
    let _ = framed.flush_pipe();
    state.sessions.detach_conn(&conn);
    state.unwatch_sessions(conn.id);
    state.sessions.clear_presence(conn.id);
    state.unregister_remote_conn(conn.id);
    loop_result
}

/// The owner whose sessions a connection may read.
///
/// Derived from the connection's peer identity rather than from whatever owner
/// the caller passed, so a caller cannot widen the projection: a `Client` peer
/// reads the paired user's sessions, a `Daemon` peer reads what its own device
/// created, and the local pipe reads its own. `handle_client` builds the same
/// owner for a `Client` peer on every other request, so this restates the rule
/// where the reply is built instead of trusting the argument
/// (`DESIGN-remote-agents.md` §8b A3, §8 R2).
fn session_list_owner(conn_peer: &Option<ConnPeer>, caller: &OwnerId) -> OwnerId {
    let projected = match conn_peer {
        Some(ConnPeer::Remote {
            role: PeerRole::Client,
            paired_by_user: Some(paired),
            ..
        }) => OwnerId::new(paired.clone(), PeerRole::Client.as_str()),
        Some(ConnPeer::Remote {
            role: PeerRole::Daemon,
            device_id,
            ..
        }) => OwnerId::new(format!("peer_{device_id}"), PeerRole::Daemon.as_str()),
        _ => return caller.clone(),
    };
    projected.unwrap_or_else(|_| caller.clone())
}

/// The one gate every reply passes on its way out to a peer connection.
///
/// A `DaemonMessage::Error` carries text written for the person at this
/// machine: absolute paths, this device's own id, key fingerprints. A remote
/// reader gets the same error with those facts replaced (`DESIGN-remote-agents.md`
/// §8 R7); a local connection gets it untouched, because
/// `WireError::redacted_for(None)` is the identity. It is deliberately not
/// applied to the event stream: a permission card's text is the owner's own
/// screen, shown to whoever is driving the session (§8b A14).
fn redact_for_conn(conn: &ConnHandle, reply: DaemonMessage) -> DaemonMessage {
    let role = conn.conn_peer.as_ref().and_then(|peer| peer.role());
    match reply {
        DaemonMessage::Error(error) => DaemonMessage::Error(error.redacted_for(role.as_ref())),
        other => other,
    }
}

fn refill_pending_events(conn: &ConnHandle, pending_events: &mut VecDeque<PendingEvent>) {
    // pull_events() starts at the last successfully written sequence. A
    // non-empty queue already owns every event after that cursor, so pulling
    // again would append the same envelopes and duplicate them on the wire.
    if pending_events.is_empty() {
        pending_events.extend(conn.pull_events());
    }
}

fn drain_pending_events(
    framed: &Framed,
    conn: &ConnHandle,
    pending_events: &mut VecDeque<PendingEvent>,
    sessions: &SessionRegistry,
) -> Result<(), DaemonError> {
    while let Some(event) = pending_events.pop_front() {
        send_pending_event(framed, conn, event, sessions)?;
    }
    Ok(())
}

fn refill_pending_state_events(
    conn: &ConnHandle,
    pending_events: &mut VecDeque<SessionEventEnvelope>,
) {
    if pending_events.is_empty() {
        pending_events.extend(conn.pull_state_events());
    }
}

fn drain_pending_state_events(
    framed: &Framed,
    pending_events: &mut VecDeque<SessionEventEnvelope>,
) -> Result<(), DaemonError> {
    while let Some(event) = pending_events.pop_front() {
        send_state_event(framed, event)?;
    }
    Ok(())
}

fn send_state_event(framed: &Framed, event: SessionEventEnvelope) -> Result<(), DaemonError> {
    framed.send_unflushed(&DaemonMessage::Event(event))
}

fn send_pending_event(
    framed: &Framed,
    conn: &ConnHandle,
    event: PendingEvent,
    sessions: &SessionRegistry,
) -> Result<(), DaemonError> {
    if !conn.event_is_current(event.subscription_id, event.attachment_generation) {
        let sequence = match &event.envelope.event {
            SessionEvent::Output { seq, .. } => format!(" seq={seq}"),
            SessionEvent::Exit { .. } => " exit".to_string(),
            SessionEvent::Recovered { .. } => " recovered".to_string(),
            SessionEvent::Silent { .. } => " silent".to_string(),
            SessionEvent::JournalDegraded { .. } => " journal_degraded".to_string(),
            SessionEvent::SessionsSnapshot { .. } => " sessions_snapshot".to_string(),
            // A snapshot is screen state and has no replay sequence.
            SessionEvent::Snapshot { .. } => " snapshot".to_string(),
            SessionEvent::AgentMessage { .. } => " agent_message".to_string(),
            SessionEvent::AgentUserMessage { .. } => " agent_user_message".to_string(),
            SessionEvent::AgentThought { .. } => " agent_thought".to_string(),
            SessionEvent::AvailableCommands { .. } => " available_commands".to_string(),
            SessionEvent::AgentToolCall { .. } => " agent_tool_call".to_string(),
            SessionEvent::AgentToolUpdate { .. } => " agent_tool_update".to_string(),
            SessionEvent::AgentFinished { .. } => " agent_finished".to_string(),
            SessionEvent::AgentTaskStarted { .. } => " agent_task_started".to_string(),
            SessionEvent::AgentTaskNotification { .. } => " agent_task_notification".to_string(),
            SessionEvent::AgentBackgroundTasksChanged { .. } => {
                " agent_background_tasks_changed".to_string()
            }
            SessionEvent::AgentError { .. } => " agent_error".to_string(),
            SessionEvent::AgentStderr { .. } => " agent_stderr".to_string(),
            SessionEvent::PermissionRequest { .. } => " permission_request".to_string(),
            SessionEvent::PermissionResolved { .. } => " permission_resolved".to_string(),
            SessionEvent::SessionManifest { .. } => " session_manifest".to_string(),
            SessionEvent::SessionNotice { .. } => " session_notice".to_string(),
            SessionEvent::AgentReported { .. } => " agent_reported".to_string(),
        };
        eprintln!(
            "discarded stale pending event for session {} generation {}{}",
            event.session_id, event.attachment_generation, sequence
        );
        return Ok(());
    }
    framed.send_unflushed(&DaemonMessage::SubscriptionEvent {
        subscription_id: event.subscription_id,
        envelope: event.envelope.clone(),
    })?;
    // The cursor is advanced after the complete frame has been written. The
    // clone above is only for the serialized message; the original envelope
    // retains the acknowledgement metadata.
    if let Some(session_id) = conn.event_sent(&event) {
        sessions.subscription_event_sent(&session_id);
    }
    Ok(())
}

fn read_client_requests(
    framed: Framed,
    inbox: SyncSender<Result<ClientMessage, DaemonError>>,
    wake: Arc<ConnOut>,
) {
    loop {
        let request = framed.recv::<ClientMessage>();
        let finished = request.is_err();
        if inbox.send(request).is_err() {
            break;
        }
        wake.notify();
        if finished {
            break;
        }
    }
}

fn connection_closed(error: &DaemonError) -> bool {
    matches!(
        error,
        DaemonError::Io(error)
            if error.kind() == std::io::ErrorKind::UnexpectedEof
                || error.kind() == std::io::ErrorKind::BrokenPipe
                || error.kind() == std::io::ErrorKind::ConnectionReset
                || error.raw_os_error() == Some(995)
    )
}

fn drains_events_before_dispatch(request: &ClientMessage) -> bool {
    matches!(
        request,
        ClientMessage::Shutdown { .. }
            | ClientMessage::SessionClose { .. }
            | ClientMessage::SessionsUnwatch { .. }
    )
}

fn reject_shutting_down(stream: std::fs::File) {
    let framed = Framed::new(stream);
    let _ = send_shutting_down(&framed, None);
}

fn send_shutting_down(framed: &Framed, id: Option<u64>) -> Result<(), DaemonError> {
    let mut error = WireError::new(ErrorCode::ShuttingDown, "daemon is shutting down");
    if let Some(id) = id {
        error = error.with_id(id);
    }
    framed.send(&DaemonMessage::Error(error))
}

fn daemon_hello(state: &ServerState) -> DaemonHello {
    DaemonHello {
        protocol_version: PROTOCOL_VERSION,
        min_protocol_version: PROTOCOL_MIN_VERSION,
        daemon_version: env!("CARGO_PKG_VERSION").to_string(),
        instance_id: state.instance_id.clone(),
        pid: std::process::id(),
        capabilities: m3a_daemon_capabilities(),
    }
}

/// The eighth argument is whether the `devices` capability was negotiated.
/// `session_send` in this file already declines a parameter object for the
/// same reason: one call shape, one place to read.
#[allow(clippy::too_many_arguments)]
fn dispatch(
    state: &Arc<ServerState>,
    owner: &OwnerId,
    request: ClientMessage,
    conn: &Arc<ConnHandle>,
    sessions_ok: bool,
    journal_ok: bool,
    typed_permissions_ok: bool,
    devices_ok: bool,
) -> Option<DaemonMessage> {
    // The peer gate is the first statement: nothing below (not the provider
    // spawns, not the readiness check) runs for a remote connection before
    // its request has a decision (`DESIGN-remote-agents.md` §8b A1). The
    // capability set is consulted first of all: it is the whole permission
    // model for a paired device (A9/A11), and a request it does not open is
    // refused before anything looks at what the request would do.
    if let Some(ConnPeer::Remote { role, .. }) = &conn.conn_peer {
        match peer_allows(*role, &conn.peer_caps, &request) {
            PeerDecision::Deny(reason) => {
                audit_peer_request(state, &conn.conn_peer, &request, "denied");
                return Some(capability_not_supported(request.request_id(), reason));
            }
            PeerDecision::Allow => {
                // §8b A4/A5: an allowed request that would run a session
                // without asking this machine's user is refused here, and
                // recorded as such — a paired device asking for unattended
                // execution is a different event in the trail from a device
                // asking for something it may not have.
                if peer_mode_refusal(state, &request) {
                    audit_peer_request(
                        state,
                        &conn.conn_peer,
                        &request,
                        crate::peer_policy::PROMPT_SKIPPING_REFUSED,
                    );
                    return Some(prompt_skipping_refused(request.request_id()));
                }
                // Only state-changing requests audit on success. An allowed
                // read must never write a row: a `Ping` loop would fill the
                // disk (muse M1).
                if request.is_state_changing() {
                    audit_peer_request(state, &conn.conn_peer, &request, "ok");
                }
            }
        }
    }
    // A remote peer's session list is a projection, not the local list, and it
    // is derived from the *connection* rather than from the `owner` this call
    // was handed: a caller that passes something else cannot widen the
    // projection. A `Client` sees the sessions of the user it was paired by
    // (what `handle_client` computes for every other request too, §8b A3); a
    // `Daemon` sees the sessions its own device created (§8 R2); the local pipe
    // sees its own list. In all three cases the registry's single owner-user
    // filter is the whole rule.
    if let Some(ConnPeer::Remote { .. }) = &conn.conn_peer {
        if let ClientMessage::SessionsList { id } = &request {
            let projected = session_list_owner(&conn.conn_peer, owner);
            return Some(match state.sessions.list(&projected) {
                Ok(sessions) => DaemonMessage::Sessions { id: *id, sessions },
                Err(error) => DaemonMessage::Error(error.with_id(*id)),
            });
        }
    }
    if state.is_shutting_down() && !matches!(request, ClientMessage::Shutdown { .. }) {
        let mut error = WireError::new(ErrorCode::ShuttingDown, "daemon is shutting down");
        if let Some(id) = request.request_id() {
            error = error.with_id(id);
        }
        return Some(DaemonMessage::Error(error));
    }
    if let ClientMessage::ProvidersRefresh { id } = request {
        let worker_state = Arc::clone(state);
        let outbound = Arc::clone(&conn.outbound);
        let failure_outbound = Arc::clone(&outbound);
        let spawn = std::thread::Builder::new()
            .name("daemon-providers-refresh".to_string())
            .spawn(move || {
                let reply = providers_reply(&worker_state, id, true);
                outbound.enqueue_reply(reply);
            });
        if spawn.is_err() {
            failure_outbound.enqueue_reply(DaemonMessage::Error(
                WireError::new(ErrorCode::Io, "could not start provider refresh").with_id(id),
            ));
        }
        return None;
    }
    // Deliberately do not serialize concurrent updates: this pipe is single-user,
    // the frontend runs one npm update at a time, and npm's global lockfile
    // serializes racers. Revisit if the daemon becomes multi-client.
    if let ClientMessage::ProviderUpdate { id, provider_id } = request {
        let worker_state = Arc::clone(state);
        let outbound = Arc::clone(&conn.outbound);
        let failure_outbound = Arc::clone(&outbound);
        let spawn = std::thread::Builder::new()
            .name("daemon-provider-update".to_string())
            .spawn(move || {
                let reply = provider_update_reply(&worker_state, id, &provider_id);
                outbound.enqueue_reply(reply);
            });
        if spawn.is_err() {
            failure_outbound.enqueue_reply(DaemonMessage::Error(
                WireError::new(ErrorCode::Io, "could not start provider update").with_id(id),
            ));
        }
        return None;
    }
    Some(dispatch_immediate(
        state,
        owner,
        request,
        conn,
        sessions_ok,
        journal_ok,
        typed_permissions_ok,
        devices_ok,
    ))
}

#[allow(clippy::too_many_arguments)]
fn dispatch_immediate(
    state: &Arc<ServerState>,
    owner: &OwnerId,
    request: ClientMessage,
    conn: &Arc<ConnHandle>,
    sessions_ok: bool,
    journal_ok: bool,
    typed_permissions_ok: bool,
    devices_ok: bool,
) -> DaemonMessage {
    if state.is_shutting_down() && !matches!(request, ClientMessage::Shutdown { .. }) {
        return DaemonMessage::Error({
            let mut error = WireError::new(ErrorCode::ShuttingDown, "daemon is shutting down");
            if let Some(id) = request.request_id() {
                error = error.with_id(id);
            }
            error
        });
    }
    match request {
        ClientMessage::Hello(_) => DaemonMessage::Error(WireError::new(
            ErrorCode::InvalidRequest,
            "hello already completed",
        )),
        ClientMessage::SessionPermissionRespond { .. } if !typed_permissions_ok => {
            capability_not_supported(request.request_id(), caps::TYPED_PERMISSIONS)
        }
        ClientMessage::Ping { id } => DaemonMessage::Pong {
            id,
            ts_ms: unix_millis(),
        },
        ClientMessage::Status { id } => state.status_body(id),
        ClientMessage::DaemonDiagnostics { id } => diagnostics_reply(state, id, owner),
        ClientMessage::Shutdown { id } => {
            // The reply is the app's last chance to know the journal is on
            // disk. Flush before accepting so a follow-up kill/restart cannot
            // race the shutdown path.
            state.sessions.flush_journal();
            DaemonMessage::Shutdown { id, accepted: true }
        }
        ClientMessage::JournalUsage { .. }
        | ClientMessage::JournalRetentionGet { .. }
        | ClientMessage::JournalRetentionSet { .. }
        | ClientMessage::SessionDelete { .. }
        | ClientMessage::ProjectsList { .. }
        | ClientMessage::ProjectAdd { .. }
        | ClientMessage::WorkspacesList { .. }
        | ClientMessage::WorkspaceCreate { .. }
        | ClientMessage::WorkspaceDelete { .. } => {
            if !journal_ok {
                return capability_not_supported(request.request_id(), caps::JOURNAL);
            }
            dispatch_journal(state, owner, request)
        }
        ClientMessage::SessionCreate { .. }
        | ClientMessage::SessionAttach { .. }
        | ClientMessage::SessionDetach { .. }
        | ClientMessage::SessionClaim { .. }
        | ClientMessage::SessionClose { .. }
        | ClientMessage::SessionStop { .. }
        | ClientMessage::SessionSend { .. }
        | ClientMessage::SessionResize { .. }
        | ClientMessage::SessionInterrupt { .. }
        | ClientMessage::SessionSetModel { .. }
        | ClientMessage::SessionSetMode { .. }
        | ClientMessage::SessionPermissionRespond { .. }
        | ClientMessage::SessionsList { .. }
        | ClientMessage::SessionsWatch { .. }
        | ClientMessage::SessionsUnwatch { .. }
        | ClientMessage::SessionsPresence { .. }
        | ClientMessage::SessionResume { .. }
        | ClientMessage::SessionReportAgent { .. } => {
            if !sessions_ok {
                return capability_not_supported(request.request_id(), caps::SESSIONS);
            }
            dispatch_session(state, owner, request, conn, typed_permissions_ok)
        }
        ClientMessage::ProvidersList { id } => providers_reply(state, id, false),
        ClientMessage::ToolPolicyGet { id } => DaemonMessage::ToolPolicy {
            id,
            policies: state.tool_policy.entries(),
        },
        ClientMessage::ToolPolicySet {
            id,
            provider_id,
            enabled,
            disabled_tools,
        } => match state.tool_policy.set(&provider_id, enabled, disabled_tools) {
            Ok(()) => DaemonMessage::ToolPolicySetOk { id },
            // A request over a cap, or one naming a provider the daemon
            // publishes no tools for, is the caller's mistake and is reported
            // as one: retrying it would fail the same way. A write failure is
            // the daemon's, and the store kept the policy it already had.
            Err(error) => {
                let code = match error {
                    crate::tool_policy::PolicyError::InvalidRequest(_) => ErrorCode::InvalidRequest,
                    crate::tool_policy::PolicyError::Io(_) => ErrorCode::Io,
                };
                DaemonMessage::Error(
                    WireError::new(
                        code,
                        format!("Could not save the tool policy for '{provider_id}': {error}"),
                    )
                    .with_id(id),
                )
            }
        },
        ClientMessage::DevicesList { .. }
        | ClientMessage::PairingStart { .. }
        | ClientMessage::PairingComplete { .. }
        | ClientMessage::PairingConfirm { .. }
        | ClientMessage::PeerRevoke { .. }
        | ClientMessage::PeerSetCaps { .. } => {
            if !devices_ok {
                return capability_not_supported(request.request_id(), caps::DEVICES);
            }
            dispatch_devices(state, conn, request)
        }
        ClientMessage::ProvidersRefresh { .. } => {
            unreachable!("ProvidersRefresh is dispatched by the async wrapper")
        }
        ClientMessage::ProviderUpdate { .. } => {
            unreachable!("ProviderUpdate is dispatched by the async wrapper")
        }
        ClientMessage::Invoke { id, method, .. } => DaemonMessage::Error(
            WireError::new(
                ErrorCode::Unimplemented,
                format!("this daemon is not a plugin backend; invoke '{method}' is refused"),
            )
            .with_id(id),
        ),
    }
}

fn providers_reply(state: &Arc<ServerState>, id: u64, force: bool) -> DaemonMessage {
    // The settings list is also the normal pre-session discovery path. Make
    // sure a Claude session can start with a non-empty model manifest even if
    // the user has not opened the settings panel's Refresh button.
    if !force {
        let _ = state.claude_models();
    }
    let discovery = if force {
        refresh_provider_catalog(state)
    } else {
        crate::provider_catalog::discover_catalog(
            &crate::registry::CdnRegistryFetch,
            state.sessions.runtime_dir(),
        )
    };
    // ProviderInfo.authentication carries the measured last-start outcome for
    // this provider. It is a recorded observation, not an auth probe.
    let providers = discovery
        .agents
        .into_iter()
        .map(|agent| {
            let authentication = state.provider_health(&agent.id);
            wire_provider(state, agent, authentication)
        })
        .collect();
    DaemonMessage::Providers {
        id,
        providers,
        unreadable_dirs: discovery.unreadable_dirs,
    }
}

fn refresh_provider_catalog(
    state: &Arc<ServerState>,
) -> crate::provider_catalog::ProviderDiscovery {
    let directories = crate::provider_catalog::path_directories();
    let local = crate::provider_catalog::discover_in_paths(&directories);
    let cache_dir = state.sessions.runtime_dir().to_path_buf();

    let registry_cache_dir = cache_dir.clone();
    let registry_refresh = std::thread::spawn(move || {
        crate::registry::refresh_npx_entries(
            &crate::registry::CdnRegistryFetch,
            &registry_cache_dir,
        );
    });

    let mut npm_packages = HashSet::new();
    let mut latest_fetches = Vec::new();
    for package in crate::provider_catalog::KNOWN_AGENTS
        .iter()
        .filter_map(|agent| agent.npm_package)
    {
        if !npm_packages.insert(package) {
            continue;
        }
        latest_fetches.push(std::thread::spawn(move || {
            let _ = crate::registry::load_latest_npm_version(
                &crate::registry::CdnNpmVersionFetch,
                package,
                true,
            );
        }));
    }

    let mut version_probes = Vec::new();
    // This fan-out is structurally bounded: native probes are at most one per
    // fixed KNOWN_AGENTS row (plus fixed debug test rows), npm fetches are at
    // most one per distinct const package name, plus the single registry
    // refresh. Do not make this registry-driven without adding an explicit
    // concurrency bound.
    for agent in local
        .agents
        .iter()
        .filter(|agent| agent.install_channel == crate::provider_catalog::InstallChannel::Native)
    {
        let state = Arc::clone(state);
        let agent = agent.clone();
        version_probes.push(std::thread::spawn(move || {
            probe_native_version(&state, &agent)
                .map(|(version, fingerprint)| (agent.id, version, fingerprint))
        }));
    }

    let _ = registry_refresh.join();
    for fetch in latest_fetches {
        let _ = fetch.join();
    }
    for probe in version_probes {
        if let Ok(Some((provider_id, version, fingerprint))) = probe.join() {
            state.record_provider_cli_version(&provider_id, &version, fingerprint);
        }
    }

    let _ = state.claude_models();

    crate::provider_catalog::discover_catalog_in_paths(
        &crate::registry::CdnRegistryFetch,
        state.sessions.runtime_dir(),
        &directories,
    )
}

fn wire_provider(
    state: &ServerState,
    agent: crate::provider_catalog::InstalledAgent,
    authentication: String,
) -> devboule_protocol::ProviderInfo {
    let installed_version = match agent.install_channel {
        crate::provider_catalog::InstallChannel::Native => agent
            .installed_version
            .clone()
            .or_else(|| state.provider_cli_version(&agent.id, &agent.executable)),
        crate::provider_catalog::InstallChannel::Npm => agent.installed_version.clone(),
        crate::provider_catalog::InstallChannel::NpxRegistry => None,
    };
    let latest_version = agent.latest_version.clone().or_else(|| {
        agent
            .npm_package
            .and_then(crate::registry::cached_latest_npm_version)
    });
    devboule_protocol::ProviderInfo {
        id: agent.id.to_string(),
        executable: agent.executable.to_string_lossy().into_owned(),
        acp_available: agent.acp_command.is_some(),
        authentication,
        protocol: crate::provider_catalog::chat_protocol(&agent).map(str::to_string),
        origin: agent.installed.then(|| agent.origin.as_wire().to_string()),
        launch_args: agent.launch_args,
        pickable: agent.pickable,
        installed_version,
        latest_version,
        agent_version: state.provider_version(&agent.id),
        install_channel: Some(agent.install_channel.as_wire().to_string()),
        installed: agent.installed,
        npm_package: agent.npm_package.map(str::to_string),
        tools: agent.tools,
    }
}

fn provider_update_reply(state: &Arc<ServerState>, id: u64, provider_id: &str) -> DaemonMessage {
    #[cfg(test)]
    let discovery_override = state
        .provider_update_catalog
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone();
    let agents: Vec<crate::provider_catalog::InstalledAgent> = if let Some(discovery) = {
        #[cfg(test)]
        {
            discovery_override
        }
        #[cfg(not(test))]
        {
            None::<crate::provider_catalog::ProviderDiscovery>
        }
    } {
        discovery.agents
    } else {
        crate::provider_catalog::discover_catalog(
            &crate::registry::CdnRegistryFetch,
            state.sessions.runtime_dir(),
        )
        .agents
    };
    let agent = agents.into_iter().find(|agent| agent.id == provider_id);
    let Some(agent) = agent else {
        return DaemonMessage::Error(
            WireError::new(
                ErrorCode::InvalidRequest,
                format!("Unknown provider '{provider_id}'."),
            )
            .with_id(id),
        );
    };

    match agent.install_channel {
        crate::provider_catalog::InstallChannel::Native => {
            return DaemonMessage::Error(
                WireError::new(
                    ErrorCode::InvalidRequest,
                    format!(
                        "Provider '{provider_id}' uses a native installation; npm updates are unavailable for native providers."
                    ),
                )
                .with_id(id),
            );
        }
        crate::provider_catalog::InstallChannel::NpxRegistry => {
            return DaemonMessage::Error(
                WireError::new(
                    ErrorCode::InvalidRequest,
                    format!(
                        "Provider '{provider_id}' is an npx-registry wrapper; update its registry entry instead of installing it globally."
                    ),
                )
                .with_id(id),
            );
        }
        crate::provider_catalog::InstallChannel::Npm => {}
    }
    let Some(package) = agent.npm_package else {
        return DaemonMessage::Error(
            WireError::new(
                ErrorCode::InvalidRequest,
                format!(
                    "Provider '{provider_id}' has no known npm package and cannot be updated with npm."
                ),
            )
            .with_id(id),
        );
    };
    if crate::provider_catalog::known_npm_package(provider_id) != Some(package) {
        return DaemonMessage::Error(
            WireError::new(
                ErrorCode::InvalidRequest,
                format!("Provider '{provider_id}' is not a known npm provider row."),
            )
            .with_id(id),
        );
    }

    let npm_command = {
        #[cfg(test)]
        {
            match state
                .provider_update_npm_command
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .clone()
            {
                Some(ProviderUpdateNpmCommand::Resolved(program, prefix_args)) => {
                    Some((program, prefix_args))
                }
                Some(ProviderUpdateNpmCommand::Missing) => None,
                None => crate::provider_catalog::resolve_npm_command(
                    &crate::provider_catalog::path_directories(),
                ),
            }
        }
        #[cfg(not(test))]
        {
            crate::provider_catalog::resolve_npm_command(
                &crate::provider_catalog::path_directories(),
            )
        }
    };
    let Some((program, prefix_args)) = npm_command else {
        return DaemonMessage::ProviderUpdated {
            id,
            ok: false,
            exit_code: None,
            log: "npm was not found on PATH; install Node.js/npm and try again.".to_string(),
        };
    };
    let args = vec![
        "install".to_string(),
        "-g".to_string(),
        format!("{package}@latest"),
    ];
    let result = state
        .npm_install_runner
        .run(&program, &prefix_args, &args, &state.process_job);
    let ok = result.exit_code == Some(0);
    if ok {
        state.invalidate_provider_update_caches(provider_id);
    }
    DaemonMessage::ProviderUpdated {
        id,
        ok,
        exit_code: result.exit_code,
        log: crate::provider_update::bounded_log(result.log.as_bytes()),
    }
}

fn probe_native_version(
    state: &ServerState,
    agent: &crate::provider_catalog::InstalledAgent,
) -> Option<(String, CliVersionFingerprint)> {
    #[cfg(test)]
    {
        let _ = state;
        let _ = agent;
        None
    }
    #[cfg(not(test))]
    {
        if agent.id == "claude" && std::env::var_os("DEVBOULE_TEST_NO_NETWORK").is_some() {
            return None;
        }
        #[cfg(not(windows))]
        let _ = state;
        let fingerprint = executable_fingerprint(&agent.executable)?;
        let mut command = Command::new(&agent.executable);
        command
            .args(&agent.prefix_args)
            .arg("--version")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn().ok()?;
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            if state.process_job.assign(child.as_raw_handle()).is_err() {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
            }
        }
        let output = child.wait_with_output().ok()?;
        let version = parse_version_token(&output.stdout)?;
        Some((version, fingerprint))
    }
}

fn executable_fingerprint(path: &std::path::Path) -> Option<CliVersionFingerprint> {
    let metadata = std::fs::metadata(path).ok()?;
    Some(CliVersionFingerprint {
        modified: metadata.modified().ok()?,
        len: metadata.len(),
    })
}

fn cli_version_cache_is_current(
    cached: &CliVersionFingerprint,
    current: Option<&CliVersionFingerprint>,
) -> bool {
    current == Some(cached)
}

fn parse_version_token(output: &[u8]) -> Option<String> {
    let mut run = String::new();
    let inspect = |run: &mut String| {
        let candidate = std::mem::take(run);
        let components: Vec<&str> = candidate.split('.').collect();
        (components.len() >= 3
            && components.iter().all(|component| {
                !component.is_empty() && component.bytes().all(|b| b.is_ascii_digit())
            }))
        .then_some(candidate)
    };
    for byte in output {
        if byte.is_ascii_digit() || *byte == b'.' {
            run.push(*byte as char);
        } else if let Some(version) = inspect(&mut run) {
            return crate::provider_catalog::cap_external_version(&version);
        }
    }
    inspect(&mut run).and_then(|version| crate::provider_catalog::cap_external_version(&version))
}

/// Collapse an error message to a single line for the provider-health
/// string: newlines, tabs and repeated spaces become single spaces, then
/// the result is truncated to 200 chars.
fn collapse_health_reason(message: &str) -> String {
    let mut reason = String::with_capacity(message.len());
    let mut pending_space = false;
    for ch in message.chars() {
        if ch.is_whitespace() {
            pending_space = !reason.is_empty();
        } else {
            if pending_space {
                reason.push(' ');
                pending_space = false;
            }
            reason.push(ch);
        }
    }
    if reason.chars().count() > 200 {
        reason = reason.chars().take(200).collect();
    }
    reason
}

/// The device commands. Everything here is a local act except `DevicesList`,
/// which is projected for whoever asks: a local client sees the full rows, a
/// remote peer sees a subset (design §8b A13, muse M4).
fn dispatch_devices(
    state: &Arc<ServerState>,
    conn: &Arc<ConnHandle>,
    request: ClientMessage,
) -> DaemonMessage {
    match request {
        ClientMessage::DevicesList { id } => match devices_reply(state, &conn.conn_peer) {
            Ok(reply) => DaemonMessage::Devices {
                id,
                self_info: reply.self_info,
                peers: reply.peers,
                pending: reply.pending,
            },
            Err(error) => DaemonMessage::Error(error.with_id(id)),
        },
        ClientMessage::PairingStart { id, role } => {
            // The address shown is where *this* device can be reached.
            match pairing_address(state) {
                Err(error) => DaemonMessage::Error(error.with_id(id)),
                Ok(address) => match state.pairing().start(role) {
                    Ok((code, expires_at)) => DaemonMessage::PairingCode {
                        id,
                        code,
                        expires_at,
                        address,
                    },
                    // Only the OS entropy source can fail here, and refusing
                    // is the only safe answer.
                    Err(error) => DaemonMessage::Error(
                        WireError::new(ErrorCode::Internal, error.to_string()).with_id(id),
                    ),
                },
            }
        }
        ClientMessage::PairingComplete {
            id,
            address,
            code,
            role,
        } => {
            // The pairing service takes this device's transport from its own
            // state, so the binding check on both the immediate path and the
            // deferred answer thread see the same one.
            match state.pairing().complete(state, &address, &code, role) {
                Ok(crate::pairing::PairingOutcome::Pending(peer)) => {
                    DaemonMessage::PairingPending { id, peer }
                }
                Ok(crate::pairing::PairingOutcome::Done(peer)) => {
                    DaemonMessage::PairingDone { id, peer }
                }
                // The message never contains the code: `PairingError` renders
                // only reasons, and `PairingSecret`'s `Debug` is redacted.
                Err(error) => DaemonMessage::Error(
                    WireError::new(ErrorCode::InvalidRequest, error.to_string()).with_id(id),
                ),
            }
        }
        ClientMessage::PairingConfirm {
            id,
            device_id,
            accept,
        } => match state.pairing().confirm(state, &device_id, accept) {
            Ok(crate::pairing::ConfirmOutcome::Accepted(peer)) => {
                DaemonMessage::PeerUpdated { id, peer: *peer }
            }
            // A decline is a completed act, not an error: the panel must not
            // render it as a failure.
            Ok(crate::pairing::ConfirmOutcome::Declined) => {
                DaemonMessage::PairingDeclined { id, device_id }
            }
            Err(error) => DaemonMessage::Error(
                WireError::new(ErrorCode::InvalidRequest, error.to_string()).with_id(id),
            ),
        },
        ClientMessage::PeerRevoke { id, device_id } => {
            match state.peer_revoke(&device_id, unix_millis() as i64) {
                Ok(PeerMutation::Updated) => {
                    // Revocation closes live connections under the same lock
                    // that recorded it, so at most one already-decoded frame
                    // is processed afterwards (design §8 R8).
                    let closed = state.revoke_peer_connections(&device_id);
                    let _ = closed;
                    match state.peer_get(&device_id) {
                        Ok(Some(record)) => DaemonMessage::PeerUpdated {
                            id,
                            peer: crate::pairing::peer_row(state, &record),
                        },
                        _ => DaemonMessage::Ok { id },
                    }
                }
                // A row that is already revoked is a different fact from a row
                // that does not exist, and the panel shows this sentence
                // verbatim (C9: a double click, or a second device's panel,
                // used to be told the peer did not exist).
                Ok(PeerMutation::Revoked) => DaemonMessage::Error(
                    WireError::new(
                        ErrorCode::InvalidRequest,
                        "That device is already revoked. Pair it again to use it.",
                    )
                    .with_id(id),
                ),
                Ok(PeerMutation::NotFound) => DaemonMessage::Error(
                    WireError::new(ErrorCode::InvalidRequest, "No such peer to revoke.")
                        .with_id(id),
                ),
                Err(error) => {
                    DaemonMessage::Error(WireError::new(ErrorCode::Internal, error).with_id(id))
                }
            }
        }
        ClientMessage::PeerSetCaps {
            id,
            device_id,
            caps,
        } => match state.peer_get(&device_id) {
            Err(error) => {
                DaemonMessage::Error(WireError::new(ErrorCode::Internal, error).with_id(id))
            }
            Ok(None) => DaemonMessage::Error(
                WireError::new(ErrorCode::InvalidRequest, "No such peer.").with_id(id),
            ),
            Ok(Some(record)) => {
                let role = PeerRole::parse(&record.role).unwrap_or(PeerRole::Daemon);
                match crate::pairing::validate_caps(role, &caps) {
                    Err(message) => DaemonMessage::Error(
                        WireError::new(ErrorCode::InvalidRequest, message).with_id(id),
                    ),
                    Ok(caps) => match state.peer_set_caps(&device_id, caps) {
                        Ok(PeerMutation::Updated) => {
                            // A capability change takes effect on the next
                            // connection: the live one holds the set it read at
                            // connect, and a device must not keep a capability
                            // this row no longer grants. The flag is what drops
                            // it, on that connection's own next turn.
                            state.revoke_peer_connections(&device_id);
                            match state.peer_get(&device_id) {
                                Ok(Some(refreshed)) => DaemonMessage::PeerUpdated {
                                    id,
                                    peer: crate::pairing::peer_row(state, &refreshed),
                                },
                                _ => DaemonMessage::Ok { id },
                            }
                        }
                        // A revoked device's capabilities cannot be rewritten
                        // (C8): the stored state would disagree with the
                        // panel's "Revoked" section, and the old array would
                        // silently revive if the row is re-paired.
                        Ok(PeerMutation::Revoked) => DaemonMessage::Error(
                            WireError::new(
                                ErrorCode::InvalidRequest,
                                "That device is revoked; its capabilities cannot be changed. \
                                 Pair it again to use it.",
                            )
                            .with_id(id),
                        ),
                        Ok(PeerMutation::NotFound) => DaemonMessage::Error(
                            WireError::new(ErrorCode::InvalidRequest, "No such peer.").with_id(id),
                        ),
                        Err(error) => DaemonMessage::Error(
                            WireError::new(ErrorCode::Internal, error).with_id(id),
                        ),
                    },
                }
            }
        },
        other => DaemonMessage::Error(WireError::new(
            ErrorCode::InvalidRequest,
            format!("unexpected device frame {other:?}"),
        )),
    }
}

struct DevicesReply {
    self_info: devboule_protocol::SelfInfo,
    peers: Vec<devboule_protocol::PeerRow>,
    pending: Vec<devboule_protocol::PendingPairing>,
}

/// Build the reply for the requesting role.
///
/// A `Daemon` peer sees `{device_id, display_name, role, online}` per peer and
/// `{device_id, display_name, daemon_version, protocol_version}` for this
/// device: never an address, a binding, a public key, or the SID this device
/// paired it from. Its own Noise key is what it needs, and it already has it.
fn devices_reply(
    state: &Arc<ServerState>,
    conn_peer: &Option<ConnPeer>,
) -> Result<DevicesReply, WireError> {
    let records = state
        .peers()
        .map_err(|error| WireError::new(ErrorCode::Internal, error))?;
    let identity = state
        .device_identity()
        .as_ref()
        .map_err(|error| WireError::new(ErrorCode::Internal, error.to_string()))?;
    // Computed only for the local projection, below (C16): taking the pairing
    // mutex and building `PendingPairing` vecs on every remote poll was work the
    // remote projections then discarded.
    match conn_peer {
        Some(ConnPeer::Remote {
            role: PeerRole::Daemon,
            ..
        }) => {
            let peers = records
                .iter()
                .map(|record| devboule_protocol::PeerRow {
                    device_id: record.device_id.clone(),
                    display_name: record.display_name.clone(),
                    role: PeerRole::parse(&record.role).unwrap_or(PeerRole::Daemon),
                    public_key: String::new(),
                    key_fingerprint: String::new(),
                    binding_kind: String::new(),
                    binding_node_name: None,
                    binding_login_name: None,
                    address: String::new(),
                    paired_at: record.paired_at,
                    revoked_at: record.revoked_at,
                    caps: Vec::new(),
                    paired_by_user: None,
                    online: state.is_peer_online(&record.device_id),
                })
                .collect();
            Ok(DevicesReply {
                self_info: devboule_protocol::SelfInfo {
                    device_id: identity.device_id.clone(),
                    display_name: identity.display_name.clone(),
                    public_key: String::new(),
                    key_fingerprint: String::new(),
                    addresses: Vec::new(),
                    port: 0,
                    daemon_version: env!("CARGO_PKG_VERSION").to_string(),
                    protocol_version: PROTOCOL_VERSION,
                    // Withheld from a machine peer by **value**: the empty
                    // string, the empty array and `0` in the four fields above,
                    // and `remote` by omission (the one field `RemoteState`
                    // cannot express as "unknown"). The keys stay present
                    // because the 1b contract types them as required and the
                    // panel reads them unconditionally (C4).
                    remote: None,
                },
                peers,
                // A peer has no business deciding this device's pairings.
                pending: Vec::new(),
            })
        }
        Some(ConnPeer::Remote {
            role: PeerRole::Client,
            ..
        }) => Ok(DevicesReply {
            self_info: devboule_protocol::SelfInfo {
                device_id: identity.device_id.clone(),
                display_name: identity.display_name.clone(),
                public_key: identity.public_key_b64(),
                key_fingerprint: identity.key_fingerprint.clone(),
                // Empty rather than absent, for the same reason as the Daemon
                // arm: where this device sits on the tailnet is not a client's
                // business, and the panel must still be able to read the field.
                addresses: Vec::new(),
                port: 0,
                daemon_version: env!("CARGO_PKG_VERSION").to_string(),
                protocol_version: PROTOCOL_VERSION,
                remote: None,
            },
            peers: records
                .iter()
                .map(|record| {
                    let mut row = crate::pairing::peer_row(state, record);
                    // The SID this device paired the peer from is local-only.
                    row.paired_by_user = None;
                    row
                })
                .collect(),
            pending: Vec::new(),
        }),
        _ => Ok(DevicesReply {
            self_info: devboule_protocol::SelfInfo {
                device_id: identity.device_id.clone(),
                display_name: identity.display_name.clone(),
                public_key: identity.public_key_b64(),
                key_fingerprint: identity.key_fingerprint.clone(),
                addresses: state.remote_addresses(),
                port: state.remote_port().unwrap_or(0),
                daemon_version: env!("CARGO_PKG_VERSION").to_string(),
                protocol_version: PROTOCOL_VERSION,
                remote: Some(state.remote_state()),
            },
            peers: records
                .iter()
                .map(|record| crate::pairing::peer_row(state, record))
                .collect(),
            pending: state.pairing().pending_snapshot(),
        }),
    }
}

/// Where this device can be reached, for the code it displays.
fn pairing_address(state: &Arc<ServerState>) -> Result<String, WireError> {
    // No address yet? The listener is started at daemon start-up, but Tailscale
    // may have come up since (or been started precisely because the panel said
    // to). Try once more before refusing (C5), so the instruction this error
    // carries — start Tailscale and show a code again — is one the daemon
    // actually honours.
    if let Some(address) = remote_address(state) {
        return Ok(address);
    }
    if !state.ensure_remote_listener() {
        return Err(no_tailnet_address());
    }
    match remote_address(state) {
        Some(address) => Ok(address),
        None => Err(no_tailnet_address()),
    }
}

/// The `ip:port` this device advertises for pairing, when it has one.
fn remote_address(state: &Arc<ServerState>) -> Option<String> {
    let addresses = state.remote_addresses();
    let port = state.remote_port();
    match (addresses.first(), port) {
        (Some(address), Some(port)) => Some(format!("{address}:{port}")),
        _ => None,
    }
}

fn no_tailnet_address() -> WireError {
    WireError::new(
        ErrorCode::InvalidRequest,
        // The remedy is real: showing a code again goes through
        // `pairing_address`, which retries the listener. Nothing here tells the
        // user to restart the daemon, which used to be the only thing that
        // worked.
        "This device has no tailnet address to pair over. Start Tailscale, then show a code again.",
    )
}

fn capability_not_supported(id: Option<u64>, capability: &str) -> DaemonMessage {
    let mut error = WireError::new(
        ErrorCode::CapabilityNotSupported,
        format!("capability '{capability}' was not negotiated"),
    );
    if let Some(id) = id {
        error = error.with_id(id);
    }
    DaemonMessage::Error(error)
}

/// §8b A4/A5: would this request run a session without asking this machine's
/// user?
///
/// Two shapes reach here. `SessionCreate` names its own kind and mode in the
/// frame, so no lookup is needed. `SessionSetMode`, `SessionSend` and
/// `SessionAttach` name a session, and the registry answers with that
/// session's kind and the mode it is in now (`SessionRegistry::session_mode_guard`):
/// a local session sitting in a prompt-skipping mode refuses a remote send, and
/// a peer-origin session refuses a switch into one.
///
/// A session this daemon does not know is not a refusal here: the request still
/// has to pass the ownership check, and the answer for an unknown id is
/// `SessionNotFound` rather than a policy verdict.
fn peer_mode_refusal(state: &ServerState, request: &ClientMessage) -> bool {
    match request {
        ClientMessage::SessionCreate {
            kind,
            mode: Some(mode),
            ..
        } => crate::peer_policy::prompt_skipping_mode(kind.clone(), mode),
        ClientMessage::SessionSetMode {
            session_id,
            mode_id,
            ..
        } => state
            .sessions
            .session_mode_guard(session_id)
            .is_some_and(|(kind, _)| crate::peer_policy::prompt_skipping_mode(kind, mode_id)),
        ClientMessage::SessionSend { session_id, .. }
        | ClientMessage::SessionAttach { session_id, .. } => state
            .sessions
            .session_mode_guard(session_id)
            .and_then(|(kind, mode)| {
                mode.map(|mode| crate::peer_policy::prompt_skipping_mode(kind, &mode))
            })
            .unwrap_or(false),
        _ => false,
    }
}

/// The frame a peer gets when §8b A4/A5 refuses it.
fn prompt_skipping_refused(id: Option<u64>) -> DaemonMessage {
    let mut error = WireError::new(
        ErrorCode::CapabilityNotSupported,
        "Modes that skip the permission prompt are not available to a paired device.",
    );
    if let Some(id) = id {
        error = error.with_id(id);
    }
    DaemonMessage::Error(error)
}

/// Audit one request that came from a remote peer.
///
/// `device_id` and `role` come from the Noise-authenticated `ConnPeer`, never
/// from the frame, and the action is the variant name. The outcome is refined
/// by [`peer_outcome`].
fn audit_peer_request(
    state: &Arc<ServerState>,
    conn_peer: &Option<ConnPeer>,
    request: &ClientMessage,
    outcome: &str,
) {
    let Some(ConnPeer::Remote {
        device_id, role, ..
    }) = conn_peer
    else {
        return;
    };
    state.audit(AuditRecord {
        device_id: device_id.clone(),
        role: role.as_str().to_string(),
        claimed_origin: None,
        action: request.name().to_string(),
        session_id: request_session_id(request),
        outcome: peer_outcome(request, outcome).to_string(),
    });
}

/// The audit outcome for a peer request: the decision, refined by whether the
/// request named a prompt-skipping mode.
///
/// `ClientMessage::SessionCreate` is the only request in the protocol carrying
/// both a session kind and a mode, so it is the only one this can classify
/// without a session lookup — and this gate runs before any session is
/// touched. A `SessionSetMode` or `SessionSend` refusal is classified by the
/// decision that made it (`peer_mode_refusal`), which has the registry in hand,
/// and reaches the trail through this function unchanged. The request is
/// refused either way; the two outcomes are worth distinguishing because "a
/// paired machine asked for unattended execution" is a different event in the
/// trail from "a paired machine asked for something it may not have" (design
/// §8b A5).
fn peer_outcome(request: &ClientMessage, outcome: &str) -> &'static str {
    const REFUSED: &str = crate::peer_policy::PROMPT_SKIPPING_REFUSED;
    match outcome {
        "ok" => "ok",
        // The refusal already names itself: one label, one vocabulary.
        REFUSED => REFUSED,
        "denied" => match request {
            ClientMessage::SessionCreate {
                kind,
                mode: Some(mode),
                ..
            } if crate::peer_policy::prompt_skipping_mode(kind.clone(), mode) => REFUSED,
            _ => "denied",
        },
        // Any other word is not a decision this gate produces; the trail says
        // `denied` rather than echoing it.
        _ => "denied",
    }
}

/// The session id a request names, when it names one. Deliberately not a
/// closed match: this is audit context, and the exhaustiveness that matters
/// lives in `peer_allows` (`peer_policy.rs`).
fn request_session_id(request: &ClientMessage) -> Option<String> {
    match request {
        ClientMessage::SessionAttach { session_id, .. }
        | ClientMessage::SessionDetach { session_id, .. }
        | ClientMessage::SessionClaim { session_id, .. }
        | ClientMessage::SessionClose { session_id, .. }
        | ClientMessage::SessionStop { session_id, .. }
        | ClientMessage::SessionSend { session_id, .. }
        | ClientMessage::SessionResize { session_id, .. }
        | ClientMessage::SessionInterrupt { session_id, .. }
        | ClientMessage::SessionSetModel { session_id, .. }
        | ClientMessage::SessionSetMode { session_id, .. }
        | ClientMessage::SessionPermissionRespond { session_id, .. }
        | ClientMessage::SessionReportAgent { session_id, .. }
        | ClientMessage::SessionDelete { session_id, .. } => Some(session_id.clone()),
        _ => None,
    }
}

fn dispatch_journal(
    state: &Arc<ServerState>,
    owner: &OwnerId,
    request: ClientMessage,
) -> DaemonMessage {
    match request {
        ClientMessage::JournalUsage { id } => match state.sessions.journal_usage() {
            Ok(usage) => DaemonMessage::JournalUsage {
                id,
                usage: wire_journal_usage(usage),
            },
            Err(error) => DaemonMessage::Error(error.with_id(id)),
        },
        ClientMessage::JournalRetentionGet { id } => match state.sessions.journal_retention_get() {
            Ok(retention) => DaemonMessage::JournalRetention { id, retention },
            Err(error) => DaemonMessage::Error(error.with_id(id)),
        },
        ClientMessage::JournalRetentionSet {
            id,
            max_age_ms,
            max_bytes,
            max_sessions,
            session_max_bytes,
            idempotency_key,
        } => {
            let fingerprint = format!(
                "retention:{max_age_ms:?}:{max_bytes:?}:{max_sessions:?}:{session_max_bytes:?}"
            );
            if let Some(reply) =
                idempotent_hit(state, owner, id, idempotency_key.as_deref(), &fingerprint)
            {
                return reply;
            }
            match state.sessions.journal_retention_set(RetentionPatch {
                max_age_ms,
                max_bytes,
                max_sessions,
                session_max_bytes,
            }) {
                Ok(retention) => {
                    let reply = DaemonMessage::JournalRetention { id, retention };
                    remember(
                        state,
                        owner,
                        idempotency_key.as_deref(),
                        &fingerprint,
                        &reply,
                    );
                    reply
                }
                Err(error) => DaemonMessage::Error(error.with_id(id)),
            }
        }
        ClientMessage::SessionDelete {
            id,
            session_id,
            idempotency_key,
        } => {
            let fingerprint = format!("delete:{session_id}");
            if let Some(reply) =
                idempotent_hit(state, owner, id, idempotency_key.as_deref(), &fingerprint)
            {
                return reply;
            }
            match state.sessions.delete_session(&session_id, owner) {
                Ok(()) => {
                    let reply = DaemonMessage::Ok { id };
                    remember(
                        state,
                        owner,
                        idempotency_key.as_deref(),
                        &fingerprint,
                        &reply,
                    );
                    reply
                }
                Err(error) => DaemonMessage::Error(error.with_id(id)),
            }
        }
        ClientMessage::ProjectsList { id } => match state.sessions.projects_list() {
            Ok(projects) => DaemonMessage::Projects { id, projects },
            Err(error) => DaemonMessage::Error(error.with_id(id)),
        },
        ClientMessage::ProjectAdd { id, path } => match state.sessions.project_add(&path) {
            Ok(project) => DaemonMessage::Project { id, project },
            Err(error) => DaemonMessage::Error(error.with_id(id)),
        },
        ClientMessage::WorkspacesList { id, project_id } => {
            match state.sessions.workspaces_list(&project_id) {
                Ok(workspaces) => DaemonMessage::Workspaces { id, workspaces },
                Err(error) => DaemonMessage::Error(error.with_id(id)),
            }
        }
        ClientMessage::WorkspaceCreate {
            id,
            project_id,
            isolation,
            branch,
        } => match state
            .sessions
            .workspace_create(&project_id, isolation, branch)
        {
            Ok(workspace) => DaemonMessage::Workspace { id, workspace },
            Err(error) => DaemonMessage::Error(error.with_id(id)),
        },
        ClientMessage::WorkspaceDelete {
            id,
            workspace_id,
            force,
        } => match state.sessions.workspace_delete(&workspace_id, force) {
            Ok(()) => DaemonMessage::Ok { id },
            Err(error) => DaemonMessage::Error(error.with_id(id)),
        },
        other => DaemonMessage::Error(WireError::new(
            ErrorCode::InvalidRequest,
            format!("unexpected journal frame {other:?}"),
        )),
    }
}

fn wire_journal_usage(usage: crate::journal::JournalUsage) -> WireJournalUsage {
    WireJournalUsage {
        total_bytes: usage.total_bytes,
        session_count: usage.session_count,
        deleted_by_user: usage.deleted_by_user,
        deleted_by_retention: usage.deleted_by_retention,
        unreclaimable: WireUnreclaimable {
            bytes_over: usage.unreclaimable.bytes_over,
            sessions_over: usage.unreclaimable.sessions_over,
            aged_out: usage.unreclaimable.aged_out,
        },
        limits: WireJournalLimits {
            snapshot_every_bytes: usage.limits.snapshot_every_bytes,
            session_max_bytes: usage.limits.session_max_bytes,
            max_bytes: usage.limits.max_bytes,
            max_sessions: usage.limits.max_sessions,
            max_age_ms: usage.limits.max_age_ms,
        },
        per_session: usage
            .per_session
            .into_iter()
            .map(|session| WireJournalSessionUsage {
                id: session.id,
                title: session.title,
                kind: session.kind,
                bytes: session.bytes,
                updated_at_ms: session.updated_at_ms,
            })
            .collect(),
    }
}

fn dispatch_session(
    state: &Arc<ServerState>,
    owner: &OwnerId,
    request: ClientMessage,
    conn: &Arc<ConnHandle>,
    typed_permissions_ok: bool,
) -> DaemonMessage {
    match request {
        ClientMessage::SessionCreate {
            id,
            workspace_id,
            kind,
            provider,
            mode,
            idempotency_key,
        } => session_create(
            state,
            owner,
            &conn.conn_peer,
            id,
            workspace_id,
            kind,
            provider,
            mode,
            idempotency_key,
        ),
        ClientMessage::SessionAttach {
            id,
            session_id,
            subscription_id,
            from_cursor,
        } => reply_result(
            id,
            state
                .sessions
                .attach_with_subscription(
                    &session_id,
                    subscription_id,
                    from_cursor,
                    conn,
                    owner,
                    typed_permissions_ok,
                )
                .map(|()| DaemonMessage::SessionAttached {
                    id,
                    subscription_id,
                }),
        ),
        ClientMessage::SessionDetach {
            id,
            session_id,
            subscription_id,
        } => reply_result(
            id,
            state
                .sessions
                .detach_with_subscription(&session_id, subscription_id, conn, owner)
                .map(|()| DaemonMessage::Ok { id }),
        ),
        ClientMessage::SessionClaim {
            id,
            session_id,
            subscription_id,
        } => reply_result(
            id,
            state
                .sessions
                .claim_resize_with_subscription(&session_id, subscription_id, owner, conn)
                .map(|()| DaemonMessage::Ok { id }),
        ),
        ClientMessage::SessionClose {
            id,
            session_id,
            idempotency_key,
        } => {
            let fingerprint = format!("close:{session_id}");
            if let Some(reply) =
                idempotent_hit(state, owner, id, idempotency_key.as_deref(), &fingerprint)
            {
                return reply;
            }
            match state.sessions.close(&session_id, owner, &conn.conn_peer) {
                Ok(removed) => {
                    if removed {
                        state.session_finished();
                    }
                    let reply = DaemonMessage::Ok { id };
                    remember(
                        state,
                        owner,
                        idempotency_key.as_deref(),
                        &fingerprint,
                        &reply,
                    );
                    reply
                }
                Err(error) => DaemonMessage::Error(error.with_id(id)),
            }
        }
        ClientMessage::SessionsWatch { id } => {
            state.watch_sessions(owner, conn);
            DaemonMessage::Ok { id }
        }
        ClientMessage::SessionsUnwatch { id } => {
            state.unwatch_sessions(conn.id);
            conn.clear_state_events();
            DaemonMessage::Ok { id }
        }
        ClientMessage::SessionsPresence {
            id,
            focused_session_id,
            app_visible,
        } => reply_result(
            id,
            state
                .sessions
                .set_presence(conn.id, owner, focused_session_id, app_visible)
                .map(|()| DaemonMessage::Ok { id }),
        ),
        ClientMessage::SessionStop {
            id,
            session_id,
            subscription_id,
        } => reply_result(
            id,
            state
                .sessions
                .stop_with_subscription(&session_id, subscription_id, owner, conn)
                .map(|()| DaemonMessage::Ok { id }),
        ),
        ClientMessage::SessionSend {
            id,
            session_id,
            subscription_id,
            text,
            attachments,
            idempotency_key,
        } => session_send(
            state,
            owner,
            conn,
            id,
            session_id,
            subscription_id,
            text,
            attachments,
            idempotency_key,
        ),
        ClientMessage::SessionResize {
            id,
            session_id,
            subscription_id,
            cols,
            rows,
        } => reply_result(
            id,
            state
                .sessions
                .resize_with_subscription(&session_id, subscription_id, cols, rows, owner, conn)
                .map(|()| DaemonMessage::Ok { id }),
        ),
        ClientMessage::SessionsList { id } => match state.sessions.list(owner) {
            Ok(sessions) => DaemonMessage::Sessions { id, sessions },
            Err(error) => DaemonMessage::Error(error.with_id(id)),
        },
        ClientMessage::SessionReportAgent {
            id,
            session_id,
            source,
            agent,
            state: agent_state,
            message,
            seq,
            agent_session_id,
            agent_session_path,
            session_start_source,
        } => {
            let report = crate::agent_report::AgentReport {
                source,
                agent,
                state: agent_state,
                message,
                seq,
                agent_session_id,
                agent_session_path,
                session_start_source: crate::agent_report::normalize_session_start_source(
                    session_start_source,
                ),
            };
            reply_result(
                id,
                state
                    .sessions
                    .report_agent(&session_id, report, conn.peer.as_ref())
                    .map(|_| DaemonMessage::Ok { id }),
            )
        }
        ClientMessage::SessionResume {
            id, persistence, ..
        } => match persistence.kind {
            PersistenceKind::None => DaemonMessage::Resume {
                id,
                result: ResumeResult::NotSupported,
            },
            PersistenceKind::Acp { handle } => {
                match state.sessions.resume(state, &handle, owner, conn) {
                    Ok(session) => DaemonMessage::Resume {
                        id,
                        result: ResumeResult::Resumed {
                            session: Box::new(session),
                        },
                    },
                    Err(error) => DaemonMessage::Error(error.with_id(id)),
                }
            }
        },
        ClientMessage::SessionInterrupt {
            id,
            session_id,
            subscription_id,
        } => reply_result(
            id,
            state
                .sessions
                .interrupt_with_subscription(&session_id, subscription_id, owner, conn)
                .map(|()| DaemonMessage::Ok { id }),
        ),
        ClientMessage::SessionSetModel {
            id,
            session_id,
            model_id,
            effort,
        } => reply_result(
            id,
            state
                .sessions
                .set_model(&session_id, owner, model_id.as_deref(), effort.as_deref())
                .map(|()| DaemonMessage::Ok { id }),
        ),
        ClientMessage::SessionSetMode {
            id,
            session_id,
            mode_id,
        } => reply_result(
            id,
            state
                .sessions
                .set_mode(&session_id, owner, &mode_id)
                .map(|()| DaemonMessage::Ok { id }),
        ),
        ClientMessage::SessionPermissionRespond {
            id,
            session_id,
            subscription_id,
            request_id,
            outcome,
            option_id,
            idempotency_key,
        } => {
            let fingerprint = format!(
                "permission:{session_id}:{request_id}:{outcome:?}:{}",
                option_id.as_deref().unwrap_or("")
            );
            if let Some(reply) =
                idempotent_hit(state, owner, id, idempotency_key.as_deref(), &fingerprint)
            {
                return reply;
            }
            match state.sessions.permission_respond_with_subscription(
                crate::session::PermissionResponse {
                    session_id: &session_id,
                    request_id: &request_id,
                    outcome,
                    option_id: option_id.as_deref(),
                },
                subscription_id,
                conn,
                owner,
            ) {
                Ok(()) => {
                    let reply = DaemonMessage::Ok { id };
                    remember(
                        state,
                        owner,
                        idempotency_key.as_deref(),
                        &fingerprint,
                        &reply,
                    );
                    reply
                }
                Err(error) => DaemonMessage::Error(error.with_id(id)),
            }
        }
        other => DaemonMessage::Error(WireError::new(
            ErrorCode::InvalidRequest,
            format!("unexpected session frame {other:?}"),
        )),
    }
}

#[allow(clippy::too_many_arguments)]
fn session_create(
    state: &Arc<ServerState>,
    owner: &OwnerId,
    conn_peer: &Option<ConnPeer>,
    id: u64,
    workspace_id: Option<String>,
    kind: SessionKind,
    provider: Option<String>,
    mode: Option<String>,
    idempotency_key: Option<String>,
) -> DaemonMessage {
    let fingerprint = format!(
        "create:{}:{}:{}:{}",
        match kind {
            SessionKind::Terminal => "terminal",
            SessionKind::Acp => "acp",
            SessionKind::Claude => "claude",
            SessionKind::Pi => "pi",
            SessionKind::Codex => "codex",
        },
        provider.as_deref().unwrap_or(""),
        workspace_id.as_deref().unwrap_or(""),
        mode.as_deref().unwrap_or("")
    );
    if let Some(reply) = idempotent_hit(state, owner, id, idempotency_key.as_deref(), &fingerprint)
    {
        return reply;
    }
    if !state.session_started() {
        return DaemonMessage::Error(
            WireError::new(ErrorCode::ShuttingDown, "daemon is shutting down").with_id(id),
        );
    }
    match state
        .sessions
        .create(state, owner, workspace_id, kind, provider, mode, conn_peer)
    {
        Ok(session) => {
            let reply = DaemonMessage::Session { id, session };
            remember(
                state,
                owner,
                idempotency_key.as_deref(),
                &fingerprint,
                &reply,
            );
            reply
        }
        Err(error) => {
            state.session_finished();
            DaemonMessage::Error(error.with_id(id))
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn session_send(
    state: &Arc<ServerState>,
    owner: &OwnerId,
    conn: &ConnHandle,
    id: u64,
    session_id: String,
    subscription_id: u64,
    text: String,
    attachments: Vec<PromptAttachment>,
    idempotency_key: Option<String>,
) -> DaemonMessage {
    let fingerprint = send_fingerprint(&session_id, &text, &attachments);
    if let Some(reply) = idempotent_hit(state, owner, id, idempotency_key.as_deref(), &fingerprint)
    {
        return reply;
    }
    // The attachment counter is the peer's deposit branch (S7's scope
    // correction): `budget_for` in `peer_policy.rs` is where it will be read.
    // Until it lands, a send that arrived over a peer connection may not carry
    // attachments at all — refused here, at the point where the origin is
    // known, before anything decodes the base64 or touches the store. A local
    // send is unchanged.
    if !attachments.is_empty() {
        let origin = crate::session::session_origin_for(&conn.conn_peer);
        if !origin.is_local() {
            return DaemonMessage::Error(
                WireError::new(
                    ErrorCode::InvalidRequest,
                    crate::peer_policy::PEER_ATTACHMENTS_UNSUPPORTED,
                )
                .with_id(id),
            );
        }
    }
    // The public six-argument entry point is the one that builds the
    // `SendRequest`; it is called from here rather than the private
    // one-argument form, which would leave this wrapper dead in a non-test
    // build.
    match state.sessions.send_with_subscription(
        &session_id,
        subscription_id,
        &text,
        &attachments,
        owner,
        conn,
    ) {
        Ok(()) => {
            let reply = DaemonMessage::Ok { id };
            remember(
                state,
                owner,
                idempotency_key.as_deref(),
                &fingerprint,
                &reply,
            );
            reply
        }
        Err(error) => DaemonMessage::Error(error.with_id(id)),
    }
}

/// The idempotency fingerprint of one send.
///
/// The attachment digests belong in it because the text is not the whole
/// payload. Without them, two sends with the same text and different images
/// share a fingerprint, and the second comes back as an idempotent replay of
/// the first: the user swaps the picture, presses Generate, and gets the
/// previous answer. The count fixes how many digests follow so the text cannot
/// be mistaken for one of them.
///
/// The digests are sha256 hex, not the encoded bytes. This string is stored
/// beside every key the daemon has seen and must not weigh as much as the
/// images it identifies.
fn send_fingerprint(session_id: &str, text: &str, attachments: &[PromptAttachment]) -> String {
    let mut fingerprint = format!("send:{session_id}:{}", attachments.len());
    for attachment in attachments {
        fingerprint.push(':');
        fingerprint.push_str(&crate::attachment_store::attachment_digest(attachment));
    }
    fingerprint.push(':');
    fingerprint.push_str(text);
    fingerprint
}

fn idempotent_hit(
    state: &ServerState,
    owner: &OwnerId,
    request_id: u64,
    key: Option<&str>,
    fingerprint: &str,
) -> Option<DaemonMessage> {
    let key = key?;
    if let Err(message) = validate_idempotency_key(key) {
        return Some(DaemonMessage::Error(
            WireError::new(ErrorCode::InvalidRequest, message).with_id(request_id),
        ));
    }
    let owner_key = format!("{}.{}", owner.user, owner.client);
    let mut store = state
        .idempotency
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    match store.check(&owner_key, key, fingerprint, Instant::now()) {
        IdempotencyOutcome::Hit(message) => Some(rewrite_id(message, request_id)),
        IdempotencyOutcome::Conflict => Some(DaemonMessage::Error(
            WireError::new(
                ErrorCode::IdempotencyConflict,
                "idempotency key reused with a different payload",
            )
            .with_id(request_id),
        )),
        IdempotencyOutcome::Miss => None,
    }
}

fn remember(
    state: &ServerState,
    owner: &OwnerId,
    key: Option<&str>,
    fingerprint: &str,
    reply: &DaemonMessage,
) {
    let Some(key) = key else {
        return;
    };
    if validate_idempotency_key(key).is_err() {
        return;
    }
    let owner_key = format!("{}.{}", owner.user, owner.client);
    let mut store = state
        .idempotency
        .lock()
        .unwrap_or_else(|err| err.into_inner());
    store.remember(
        owner_key,
        key.to_string(),
        fingerprint.to_string(),
        reply.clone(),
        Instant::now(),
    );
}

fn rewrite_id(message: DaemonMessage, id: u64) -> DaemonMessage {
    match message {
        DaemonMessage::Session { session, .. } => DaemonMessage::Session { id, session },
        DaemonMessage::Ok { .. } => DaemonMessage::Ok { id },
        DaemonMessage::Sessions { sessions, .. } => DaemonMessage::Sessions { id, sessions },
        DaemonMessage::Projects { projects, .. } => DaemonMessage::Projects { id, projects },
        DaemonMessage::Project { project, .. } => DaemonMessage::Project { id, project },
        DaemonMessage::Workspaces { workspaces, .. } => {
            DaemonMessage::Workspaces { id, workspaces }
        }
        DaemonMessage::Workspace { workspace, .. } => DaemonMessage::Workspace { id, workspace },
        DaemonMessage::SessionAttached {
            subscription_id, ..
        } => DaemonMessage::SessionAttached {
            id,
            subscription_id,
        },
        DaemonMessage::JournalRetention { retention, .. } => {
            DaemonMessage::JournalRetention { id, retention }
        }
        DaemonMessage::ProviderUpdated {
            ok, exit_code, log, ..
        } => DaemonMessage::ProviderUpdated {
            id,
            ok,
            exit_code,
            log,
        },
        DaemonMessage::Error(error) => DaemonMessage::Error(error.with_id(id)),
        other => other,
    }
}

fn reply_result(id: u64, result: Result<DaemonMessage, WireError>) -> DaemonMessage {
    match result {
        Ok(message) => message,
        Err(error) => DaemonMessage::Error(error.with_id(id)),
    }
}

fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(0))
        .unwrap_or(0)
}

fn bounded_join(handle: JoinHandle<()>, budget: Duration) {
    let deadline = Instant::now() + budget;
    while !handle.is_finished() && Instant::now() < deadline {
        std::thread::sleep(JOIN_SLICE);
    }
    if handle.is_finished() {
        let _ = handle.join();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use devboule_protocol::{ClientMessage, OwnerId, PermissionOutcome, RetentionPatch};

    use crate::journal::new_session_record;
    use crate::peer_policy::TransportBinding;

    fn state() -> Arc<ServerState> {
        ServerState::new("test-instance".to_string())
    }

    /// A state with a runtime dir whose `journal.db` the test can also open
    /// directly, for asserting what the daemon wrote to `audit`/`peers`.
    fn temp_state(tag: &str) -> (std::path::PathBuf, Arc<ServerState>) {
        let path = std::env::temp_dir().join(format!(
            "devboule-{tag}-{}-{}",
            std::process::id(),
            unix_millis()
        ));
        let state = ServerState::with_paths(
            "test-instance".to_string(),
            RuntimePaths::from_dir(path.clone()),
        )
        .expect("state");
        (path, state)
    }

    fn remote_conn(role: PeerRole, paired_by_user: Option<&str>) -> Arc<ConnHandle> {
        remote_conn_with_caps(role, paired_by_user, &[])
    }

    /// The same connection, holding the capability set `caps` names: what the
    /// gate actually reads (`DESIGN-remote-agents.md` §8b A9/A11).
    fn remote_conn_with_caps(
        role: PeerRole,
        paired_by_user: Option<&str>,
        caps: &[&str],
    ) -> Arc<ConnHandle> {
        ConnHandle::with_peer_caps(
            7,
            None,
            Some(ConnPeer::Remote {
                device_id: "dev-peer-1".to_string(),
                role,
                paired_by_user: paired_by_user.map(str::to_string),
                binding: TransportBinding::tailnet(
                    "nstable",
                    "host.tailnet.ts.net.",
                    "user@example.com",
                ),
            }),
            caps.iter().map(|cap| cap.to_string()).collect(),
        )
    }

    /// Every file under `dir`, recursively. A missing `dir` is zero files.
    fn files_under(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut files = Vec::new();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return files;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                files.extend(files_under(&path));
            } else {
                files.push(path);
            }
        }
        files
    }

    /// Audit rows as `action:outcome`, in insertion order.
    fn audit_rows(path: &std::path::Path) -> Vec<String> {
        let connection = rusqlite::Connection::open(path.join("journal.db")).expect("journal");
        let mut statement = connection
            .prepare("SELECT action, outcome FROM audit ORDER BY id")
            .expect("prepare");
        statement
            .query_map([], |row| {
                Ok(format!(
                    "{}:{}",
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?
                ))
            })
            .expect("query")
            .collect::<Result<Vec<_>, _>>()
            .expect("rows")
    }

    fn wire_attachment(name: &str, bytes: &[u8]) -> PromptAttachment {
        use base64::Engine;
        PromptAttachment {
            name: name.to_string(),
            mime_type: "image/png".to_string(),
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        }
    }

    #[test]
    fn one_text_with_two_images_is_two_fingerprints() {
        // The defect this closes: with the text alone in the fingerprint, the
        // second send comes back as an idempotent replay of the first, so the
        // user swaps the picture, presses Generate, and gets the old answer.
        let first = send_fingerprint("s.a.1", "draw this", &[wire_attachment("a.png", b"one")]);
        let second = send_fingerprint("s.a.1", "draw this", &[wire_attachment("a.png", b"two")]);
        assert_ne!(first, second);
    }

    #[test]
    fn the_same_text_and_images_keep_one_fingerprint() {
        let attachments = [
            wire_attachment("a.png", b"one"),
            wire_attachment("b.png", b"two"),
        ];
        assert_eq!(
            send_fingerprint("s.a.1", "draw this", &attachments),
            send_fingerprint("s.a.1", "draw this", &attachments),
        );
    }

    #[test]
    fn attachment_order_is_part_of_the_fingerprint() {
        let first = wire_attachment("a.png", b"one");
        let second = wire_attachment("b.png", b"two");
        assert_ne!(
            send_fingerprint("s.a.1", "draw this", &[first.clone(), second.clone()]),
            send_fingerprint("s.a.1", "draw this", &[second, first]),
        );
    }

    #[test]
    fn a_text_that_looks_like_a_digest_does_not_collide_with_one() {
        // The count field is what makes the digest region unambiguous, so a text
        // ending in hex cannot be read as an attachment's digest.
        let attachment = wire_attachment("a.png", b"one");
        let digest = crate::attachment_store::attachment_digest(&attachment);
        assert_ne!(
            send_fingerprint("s.a.1", &format!(":{digest}"), &[]),
            send_fingerprint("s.a.1", "", &[attachment]),
        );
    }

    #[test]
    fn the_fingerprint_does_not_carry_the_encoded_bytes() {
        let attachment = wire_attachment("a.png", b"the png bytes");
        let fingerprint = send_fingerprint("s.a.1", "draw this", std::slice::from_ref(&attachment));
        assert!(!fingerprint.contains(&attachment.data));
        assert!(fingerprint.contains(&crate::attachment_store::attachment_digest(&attachment)));
    }

    #[test]
    fn record_provider_health_strips_agent_stderr_from_the_reason() {
        let state = state();
        let error = WireError::new(
            ErrorCode::Io,
            "ACP request failed: {\"code\":-32000} Agent stderr: SECRET-TOKEN leaked | C:\\Users",
        );
        state.record_provider_health("stub", Err(&error));
        let value = state.provider_health("stub");
        assert!(
            value.starts_with("failed: ") && value.contains("ACP request failed"),
            "the pre-stderr part of the message must survive: {value:?}"
        );
        assert!(
            !value.contains("SECRET-TOKEN"),
            "health must not carry agent stderr: {value:?}"
        );
    }

    #[test]
    fn collapse_health_reason_cases() {
        assert_eq!(collapse_health_reason(""), "");
        assert_eq!(collapse_health_reason("  \n\t "), "");
        let exactly_200 = "x".repeat(200);
        assert_eq!(collapse_health_reason(&exactly_200), exactly_200);
        assert_eq!(
            collapse_health_reason(&"y".repeat(201)).chars().count(),
            200
        );
        // Char-boundary-safe truncation: 300 two-byte characters must yield
        // exactly 200 valid characters, not a byte slice mid-character.
        let collapsed = collapse_health_reason(&"\u{e8}".repeat(300));
        assert_eq!(collapsed, "\u{e8}".repeat(200));
        assert_eq!(collapse_health_reason("a\n\tb   c"), "a b c");
    }

    #[test]
    fn unchanged_roster_transition_is_not_resent() {
        let state = state();
        let owner = OwnerId::new("roster-user", "roster-client").expect("owner");
        let conn = ConnHandle::new(1);

        state.watch_sessions(&owner, &conn);
        assert_eq!(conn.pull_state_events().len(), 1, "initial snapshot");

        state.broadcast_session_state(&owner);

        assert!(
            conn.pull_state_events().is_empty(),
            "an unchanged full roster must not be resent"
        );
    }

    fn wait_for_shutdown(state: &ServerState) {
        let deadline = Instant::now() + IDLE_SHUTDOWN_GRACE + Duration::from_millis(500);
        while !state.is_shutting_down() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(state.is_shutting_down(), "idle daemon did not shut down");
    }

    #[test]
    fn idle_daemon_exits_after_grace_period() {
        let state = state();
        assert!(state.client_connected());
        state.client_disconnected();
        wait_for_shutdown(&state);
    }

    #[test]
    fn connected_client_prevents_idle_shutdown() {
        let state = state();
        assert!(state.client_connected());
        std::thread::sleep(IDLE_SHUTDOWN_GRACE + Duration::from_millis(100));
        assert!(!state.is_shutting_down());
        state.client_disconnected();
        wait_for_shutdown(&state);
    }

    #[test]
    fn live_session_prevents_shutdown_even_without_a_client() {
        let state = state();
        assert!(state.session_started());
        std::thread::sleep(IDLE_SHUTDOWN_GRACE + Duration::from_millis(100));
        assert!(!state.is_shutting_down());
        state.session_finished();
        wait_for_shutdown(&state);
    }

    #[test]
    fn reconnect_inside_grace_invalidates_idle_shutdown() {
        let state = state();
        assert!(state.client_connected());
        state.client_disconnected();
        std::thread::sleep(IDLE_SHUTDOWN_GRACE / 2);
        assert!(state.client_connected());
        std::thread::sleep(IDLE_SHUTDOWN_GRACE + Duration::from_millis(100));
        assert!(!state.is_shutting_down());
        state.client_disconnected();
        wait_for_shutdown(&state);
    }

    #[test]
    fn shutting_down_rejects_new_client_with_stable_error() {
        let state = state();
        state.request_shutdown();
        assert!(!state.client_connected());
        let conn = ConnHandle::new(1);
        let reply = dispatch(
            &state,
            &OwnerId::new("test-user", "test-client").expect("owner"),
            ClientMessage::Ping { id: 7 },
            &conn,
            true,
            true,
            true,
            true,
        )
        .expect("immediate dispatch reply");
        assert!(matches!(
            reply,
            DaemonMessage::Error(WireError {
                code: ErrorCode::ShuttingDown,
                id: Some(7),
                ..
            })
        ));
    }

    #[test]
    fn permission_response_requires_the_negotiated_typed_capability() {
        let state = state();
        let owner = OwnerId::new("test-user", "test-client").expect("owner");
        let conn = ConnHandle::new(8);
        let reply = dispatch(
            &state,
            &owner,
            ClientMessage::SessionPermissionRespond {
                id: 9,
                session_id: "s.test-client.missing".to_string(),
                subscription_id: 1,
                request_id: "tool-1".to_string(),
                outcome: PermissionOutcome::AllowOnce,
                option_id: None,
                idempotency_key: None,
            },
            &conn,
            false,
            true,
            true,
            false,
        )
        .expect("immediate dispatch reply");
        assert!(matches!(
            reply,
            DaemonMessage::Error(WireError {
                code: ErrorCode::CapabilityNotSupported,
                id: Some(9),
                ..
            })
        ));
    }

    #[test]
    fn providers_list_returns_catalog_entries_with_unknown_authentication() {
        let path = std::env::temp_dir().join(format!(
            "devboule-providers-test-{}-{}",
            std::process::id(),
            unix_millis()
        ));
        let state = ServerState::with_paths(
            "test-instance".to_string(),
            RuntimePaths::from_dir(path.clone()),
        )
        .expect("state");
        let owner = OwnerId::new("test-user", "test-client").expect("owner");
        let conn = ConnHandle::new(2);
        let reply = dispatch(
            &state,
            &owner,
            ClientMessage::ProvidersList { id: 11 },
            &conn,
            false,
            false,
            false,
            false,
        )
        .expect("immediate dispatch reply");
        let _ = std::fs::remove_dir_all(&path);
        let DaemonMessage::Providers {
            id,
            providers,
            unreadable_dirs: _,
        } = reply
        else {
            panic!("providers_list must reply with Providers, got {reply:?}");
        };
        assert_eq!(id, 11);
        for provider in &providers {
            assert!(!provider.id.is_empty());
            assert_eq!(provider.authentication, "unknown");
            if provider.installed {
                assert!(!provider.executable.is_empty());
            } else {
                assert!(provider.executable.is_empty());
                assert!(!provider.acp_available);
                assert_eq!(provider.protocol, None);
                assert_eq!(
                    provider.pickable,
                    Some(false),
                    "synthetic not-installed providers have no launchable chat protocol"
                );
                assert!(provider.npm_package.is_some());
            }
        }
    }

    #[test]
    fn tool_policy_set_then_get_round_trips_through_dispatch() {
        let path = std::env::temp_dir().join(format!(
            "devboule-tool-policy-dispatch-{}-{}",
            std::process::id(),
            unix_millis()
        ));
        let state = ServerState::with_paths(
            "test-instance".to_string(),
            RuntimePaths::from_dir(path.clone()),
        )
        .expect("state");
        let owner = OwnerId::new("test-user", "test-client").expect("owner");
        let conn = ConnHandle::new(4);

        let reply = dispatch(
            &state,
            &owner,
            ClientMessage::ToolPolicySet {
                id: 21,
                provider_id: "claude".to_string(),
                enabled: Some(false),
                disabled_tools: vec!["devboule_list_agents".to_string()],
            },
            &conn,
            false,
            false,
            false,
            false,
        )
        .expect("set reply");
        assert!(
            matches!(reply, DaemonMessage::ToolPolicySetOk { id: 21 }),
            "got {reply:?}"
        );

        let reply = dispatch(
            &state,
            &owner,
            ClientMessage::ToolPolicyGet { id: 22 },
            &conn,
            false,
            false,
            false,
            false,
        )
        .expect("get reply");
        let DaemonMessage::ToolPolicy { id, policies } = reply else {
            panic!("tool_policy_get must reply with ToolPolicy, got {reply:?}");
        };
        assert_eq!(id, 22);
        assert_eq!(policies.len(), 1);
        assert_eq!(policies[0].provider_id, "claude");
        assert_eq!(policies[0].enabled, Some(false));
        assert_eq!(policies[0].disabled_tools, ["devboule_list_agents"]);

        // The file lives beside the journal, and a store loaded fresh from
        // the same directory sees the write — which is what a restart does.
        assert!(path.join("tool-policies.json").is_file());
        let reopened = crate::tool_policy::ToolPolicyStore::load(&path);
        assert_eq!(
            reopened.get(Some("claude")).map(|policy| policy.enabled),
            Some(Some(false))
        );
        let _ = std::fs::remove_dir_all(&path);
    }

    #[test]
    fn a_refused_tool_policy_set_is_an_invalid_request_not_an_io_failure() {
        let state = ServerState::new("tool-policy-refused".to_string());
        let owner = OwnerId::new("test-user", "test-client").expect("owner");
        let conn = ConnHandle::new(5);

        // A provider id the catalog publishes no tools for: the gate is keyed
        // by that id, so the daemon refuses the row instead of storing one it
        // could never consult. The code is what tells the app to fix the
        // request rather than to retry a write that failed.
        let reply = dispatch(
            &state,
            &owner,
            ClientMessage::ToolPolicySet {
                id: 31,
                provider_id: "does-not-exist".to_string(),
                enabled: Some(false),
                disabled_tools: Vec::new(),
            },
            &conn,
            false,
            false,
            false,
            false,
        )
        .expect("dispatch reply");
        let DaemonMessage::Error(error) = reply else {
            panic!("a refused policy must be an error, got {reply:?}");
        };
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert_eq!(error.id, Some(31));
        assert!(
            error.message.contains("does-not-exist"),
            "the sentence names what was refused: {}",
            error.message
        );

        let runtime_dir = state.sessions.runtime_dir().to_path_buf();
        drop(state);
        let _ = std::fs::remove_dir_all(runtime_dir);
    }

    #[test]
    fn diagnostics_rpc_reports_the_open_journal_without_user_content() {
        let state = state();
        let owner = OwnerId::new("test-user", "test-client").expect("owner");
        let conn = ConnHandle::new(3);
        let reply = dispatch(
            &state,
            &owner,
            ClientMessage::DaemonDiagnostics { id: 12 },
            &conn,
            true,
            true,
            true,
            true,
        )
        .expect("immediate diagnostics reply");
        let DaemonMessage::Diagnostics { id, report } = reply else {
            panic!("diagnostics must reply with Diagnostics");
        };
        assert_eq!(id, 12);
        assert_eq!(report["daemon"]["instanceId"], "test-instance");
        assert_eq!(
            report["health"]["journalSchemaVersion"],
            JOURNAL_SCHEMA_VERSION
        );
        assert!(
            report["health"]["journalFileBytes"]
                .as_u64()
                .is_some_and(|bytes| bytes > 0),
            "journal file bytes were not positive: report={report}, journal_error={:?}",
            state
                .journal_error
                .lock()
                .ok()
                .and_then(|error| error.clone())
        );
        let encoded = report.to_string();
        assert!(!encoded.contains("title"));
        assert!(!encoded.contains("transcript"));
        assert!(!encoded.contains("AgentStderr"));
        assert!(!encoded.contains("permission env"));
    }

    #[cfg(windows)]
    #[test]
    fn host_os_version_matches_an_independent_windows_version_report() {
        // Resolve PowerShell by absolute path. A child spawned from a POSIX shell can
        // inherit a PATH without System32, and `program not found` would then read exactly
        // like a version mismatch — the environment failing, disguised as the assertion failing.
        let system_root = std::env::var("SystemRoot").expect("SystemRoot must be set on Windows");
        let powershell = std::path::Path::new(&system_root)
            .join("System32")
            .join("WindowsPowerShell")
            .join("v1.0")
            .join("powershell.exe");
        assert!(
            powershell.is_file(),
            "PowerShell must exist at {}",
            powershell.display()
        );
        let output = std::process::Command::new(&powershell)
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                "(Get-CimInstance Win32_OperatingSystem).Version",
            ])
            .output()
            .expect("PowerShell must report the Windows version");
        assert!(
            output.status.success(),
            "PowerShell version query failed: {:?}",
            output.status
        );
        let independent = String::from_utf8(output.stdout)
            .expect("PowerShell version must be UTF-8")
            .trim()
            .to_string();
        let reported = host_os_version();
        let reported_version = reported
            .strip_prefix("Windows ")
            .and_then(|value| value.split_whitespace().next())
            .expect("host OS report must contain a Windows version");
        assert_eq!(reported_version, independent);
    }

    struct RecordingNpmRunner {
        calls: Arc<Mutex<Vec<Vec<String>>>>,
        result: crate::provider_update::NpmInstallResult,
    }

    impl NpmInstallRunner for RecordingNpmRunner {
        fn run(
            &self,
            _program: &std::path::Path,
            _prefix_args: &[String],
            args: &[String],
            _job: &JobObject,
        ) -> crate::provider_update::NpmInstallResult {
            self.calls
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .push(args.to_vec());
            self.result.clone()
        }
    }

    fn update_test_agent(
        id: &str,
        package: Option<&'static str>,
        installed: bool,
        install_channel: crate::provider_catalog::InstallChannel,
        executable: std::path::PathBuf,
    ) -> crate::provider_catalog::InstalledAgent {
        crate::provider_catalog::InstalledAgent {
            id: id.to_string(),
            aliases: &[],
            installed,
            executable,
            prefix_args: Vec::new(),
            acp_command: None,
            stream_json_command: None,
            rpc_command: None,
            app_server_command: None,
            authentication: crate::provider_catalog::AuthenticationStatus::Unknown,
            origin: crate::provider_catalog::ProviderOrigin::UserBinary,
            launch_args: None,
            pickable: None,
            installed_version: None,
            latest_version: None,
            install_channel,
            npm_package: package,
            tools: crate::provider_catalog::mcp_tools_for(id),
        }
    }

    fn wait_for_update_reply(conn: &ConnHandle) -> DaemonMessage {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(reply) = conn.outbound.pull_replies().pop_front() {
                return reply;
            }
            assert!(
                Instant::now() < deadline,
                "provider update worker did not reply"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn provider_update_drops_fingerprint_but_preserves_latest_version_cache() {
        let path = std::env::temp_dir().join(format!(
            "devboule-provider-update-test-{}-{}",
            std::process::id(),
            unix_millis()
        ));
        std::fs::create_dir_all(&path).expect("runtime directory");
        let executable = path.join("codex.cmd");
        std::fs::write(&executable, b"shim").expect("fake executable");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let runner = Arc::new(RecordingNpmRunner {
            calls: Arc::clone(&calls),
            result: crate::provider_update::NpmInstallResult {
                exit_code: Some(0),
                log: "npm stdout\nnpm stderr".to_string(),
            },
        });
        let state = ServerState::with_paths_and_npm_install_runner(
            "test-instance".to_string(),
            RuntimePaths::from_dir(path.clone()),
            runner,
        )
        .expect("state");
        state.set_provider_update_catalog(crate::provider_catalog::ProviderDiscovery {
            agents: vec![update_test_agent(
                "codex",
                Some("@openai/codex"),
                true,
                crate::provider_catalog::InstallChannel::Npm,
                executable.clone(),
            )],
            unreadable_dirs: 0,
        });
        state.set_provider_update_npm_command(std::path::PathBuf::from(r"C:\fake\npm.cmd"), vec![]);
        let fingerprint = executable_fingerprint(&executable).expect("fingerprint");
        state.record_provider_cli_version("codex", "1.0.0", fingerprint);
        crate::registry::reset_npm_version_cache("@openai/codex");
        struct FakeNpmVersion;
        impl crate::registry::NpmVersionFetch for FakeNpmVersion {
            fn latest(&self, _package: &str) -> Result<String, String> {
                Ok("9.9.9".to_string())
            }
        }
        assert_eq!(
            crate::registry::load_latest_npm_version(&FakeNpmVersion, "@openai/codex", true),
            Some("9.9.9".to_string())
        );
        assert_eq!(
            state.provider_cli_version("codex", &executable),
            Some("1.0.0".to_string())
        );

        let owner = OwnerId::new("test-user", "test-client").expect("owner");
        let conn = ConnHandle::new(42);
        assert!(dispatch(
            &state,
            &owner,
            ClientMessage::ProviderUpdate {
                id: 43,
                provider_id: "codex".to_string(),
            },
            &conn,
            false,
            false,
            false,
            false,
        )
        .is_none());
        assert_eq!(
            wait_for_update_reply(&conn),
            DaemonMessage::ProviderUpdated {
                id: 43,
                ok: true,
                exit_code: Some(0),
                log: "npm stdout\nnpm stderr".to_string(),
            }
        );
        assert_eq!(
            calls
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_slice(),
            &[vec![
                "install".to_string(),
                "-g".to_string(),
                "@openai/codex@latest".to_string()
            ]]
        );
        assert_eq!(
            state.provider_cli_version("codex", &executable),
            None,
            "successful update must drop the native --version cache entry"
        );
        assert_eq!(
            crate::registry::cached_latest_npm_version("@openai/codex"),
            Some("9.9.9".to_string()),
            "successful update must preserve the npm latest cache entry"
        );
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn provider_update_failure_preserves_both_version_caches() {
        let path = std::env::temp_dir().join(format!(
            "devboule-provider-update-failure-test-{}-{}",
            std::process::id(),
            unix_millis()
        ));
        std::fs::create_dir_all(&path).expect("runtime directory");
        let package = "@qwen-code/qwen-code";
        crate::registry::reset_npm_version_cache(package);
        let executable = path.join("qwen.cmd");
        std::fs::write(&executable, b"shim").expect("fake executable");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let runner = Arc::new(RecordingNpmRunner {
            calls: Arc::clone(&calls),
            result: crate::provider_update::NpmInstallResult {
                exit_code: Some(1),
                log: "npm failed".to_string(),
            },
        });
        let state = ServerState::with_paths_and_npm_install_runner(
            "test-instance".to_string(),
            RuntimePaths::from_dir(path.clone()),
            runner,
        )
        .expect("state");
        state.set_provider_update_catalog(crate::provider_catalog::ProviderDiscovery {
            agents: vec![update_test_agent(
                "qwen",
                Some(package),
                true,
                crate::provider_catalog::InstallChannel::Npm,
                executable.clone(),
            )],
            unreadable_dirs: 0,
        });
        state.set_provider_update_npm_command(std::path::PathBuf::from(r"C:\fake\npm.cmd"), vec![]);
        let fingerprint = executable_fingerprint(&executable).expect("fingerprint");
        state.record_provider_cli_version("qwen", "2.0.0", fingerprint);
        struct FakeNpmVersion;
        impl crate::registry::NpmVersionFetch for FakeNpmVersion {
            fn latest(&self, _package: &str) -> Result<String, String> {
                Ok("8.8.8".to_string())
            }
        }
        assert_eq!(
            crate::registry::load_latest_npm_version(&FakeNpmVersion, package, true),
            Some("8.8.8".to_string())
        );

        let owner = OwnerId::new("test-user", "test-client").expect("owner");
        let conn = ConnHandle::new(50);
        assert!(dispatch(
            &state,
            &owner,
            ClientMessage::ProviderUpdate {
                id: 51,
                provider_id: "qwen".to_string(),
            },
            &conn,
            false,
            false,
            false,
            false,
        )
        .is_none());
        assert_eq!(
            wait_for_update_reply(&conn),
            DaemonMessage::ProviderUpdated {
                id: 51,
                ok: false,
                exit_code: Some(1),
                log: "npm failed".to_string(),
            }
        );
        assert_eq!(
            state.provider_cli_version("qwen", &executable),
            Some("2.0.0".to_string()),
            "failed update must preserve the --version cache entry"
        );
        assert_eq!(
            crate::registry::cached_latest_npm_version(package),
            Some("8.8.8".to_string()),
            "failed update must preserve the npm latest cache entry"
        );
        crate::registry::reset_npm_version_cache(package);
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn provider_update_refuses_native_even_when_a_package_is_known() {
        let state = state();
        state.set_provider_update_catalog(crate::provider_catalog::ProviderDiscovery {
            agents: vec![update_test_agent(
                "claude",
                Some("@anthropic-ai/claude-code"),
                true,
                crate::provider_catalog::InstallChannel::Native,
                std::path::PathBuf::from(r"C:\Program Files\claude.exe"),
            )],
            unreadable_dirs: 0,
        });
        let owner = OwnerId::new("test-user", "test-client").expect("owner");
        let conn = ConnHandle::new(44);
        assert!(dispatch(
            &state,
            &owner,
            ClientMessage::ProviderUpdate {
                id: 45,
                provider_id: "claude".to_string(),
            },
            &conn,
            false,
            false,
            false,
            false,
        )
        .is_none());
        let DaemonMessage::Error(error) = wait_for_update_reply(&conn) else {
            panic!("native provider update must return an InvalidRequest");
        };
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error.message.contains("native installation"));
    }

    #[test]
    fn provider_update_refuses_the_native_debug_stub_before_package_lookup() {
        let state = state();
        state.set_provider_update_catalog(crate::provider_catalog::ProviderDiscovery {
            agents: vec![update_test_agent(
                "devboule-acp-stub",
                None,
                true,
                crate::provider_catalog::InstallChannel::Native,
                std::path::PathBuf::from(r"C:\devboule-acp-stub.exe"),
            )],
            unreadable_dirs: 0,
        });
        let owner = OwnerId::new("test-user", "test-client").expect("owner");
        let conn = ConnHandle::new(46);
        assert!(dispatch(
            &state,
            &owner,
            ClientMessage::ProviderUpdate {
                id: 47,
                provider_id: "devboule-acp-stub".to_string(),
            },
            &conn,
            false,
            false,
            false,
            false,
        )
        .is_none());
        let DaemonMessage::Error(error) = wait_for_update_reply(&conn) else {
            panic!("native debug stub update must return an InvalidRequest");
        };
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error.message.contains("native installation"));
    }

    #[test]
    fn provider_update_reports_missing_npm_without_invoking_the_runner() {
        let path = std::env::temp_dir().join(format!(
            "devboule-provider-update-missing-npm-{}-{}",
            std::process::id(),
            unix_millis()
        ));
        let calls = Arc::new(Mutex::new(Vec::new()));
        let runner = Arc::new(RecordingNpmRunner {
            calls: Arc::clone(&calls),
            result: crate::provider_update::NpmInstallResult {
                exit_code: Some(0),
                log: "must not run".to_string(),
            },
        });
        let state = ServerState::with_paths_and_npm_install_runner(
            "test-instance".to_string(),
            RuntimePaths::from_dir(path.clone()),
            runner,
        )
        .expect("state");
        state.set_provider_update_catalog(crate::provider_catalog::ProviderDiscovery {
            agents: vec![update_test_agent(
                "codex",
                Some("@openai/codex"),
                false,
                crate::provider_catalog::InstallChannel::Npm,
                std::path::PathBuf::new(),
            )],
            unreadable_dirs: 0,
        });
        state.set_provider_update_npm_missing();
        let owner = OwnerId::new("test-user", "test-client").expect("owner");
        let conn = ConnHandle::new(48);
        assert!(dispatch(
            &state,
            &owner,
            ClientMessage::ProviderUpdate {
                id: 49,
                provider_id: "codex".to_string(),
            },
            &conn,
            false,
            false,
            false,
            false,
        )
        .is_none());
        assert_eq!(
            wait_for_update_reply(&conn),
            DaemonMessage::ProviderUpdated {
                id: 49,
                ok: false,
                exit_code: None,
                log: "npm was not found on PATH; install Node.js/npm and try again.".to_string(),
            }
        );
        assert!(calls
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_empty());
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn version_token_parser_finds_first_semver_like_token() {
        assert_eq!(
            parse_version_token(b"grok version 1.2.3\n"),
            Some("1.2.3".to_string())
        );
        assert_eq!(
            parse_version_token(b"v2.10.0-beta.1"),
            Some("2.10.0".to_string())
        );
        assert_eq!(parse_version_token(b"build 2026-09-06"), None);
        assert_eq!(parse_version_token(b"no version here"), None);
    }

    #[test]
    fn cli_version_cache_is_invalidated_when_executable_metadata_changes() {
        let cached = CliVersionFingerprint {
            modified: UNIX_EPOCH + Duration::from_secs(10),
            len: 100,
        };
        assert!(cli_version_cache_is_current(&cached, Some(&cached)));
        assert!(!cli_version_cache_is_current(
            &cached,
            Some(&CliVersionFingerprint {
                modified: UNIX_EPOCH + Duration::from_secs(11),
                len: 100,
            })
        ));
        assert!(!cli_version_cache_is_current(
            &cached,
            Some(&CliVersionFingerprint {
                modified: cached.modified,
                len: 101,
            })
        ));
        assert!(!cli_version_cache_is_current(&cached, None));
    }

    #[test]
    fn journal_commands_dispatch_and_reject_invalid_retention_patches() {
        let path = std::env::temp_dir().join(format!(
            "devboule-command-test-{}-{}",
            std::process::id(),
            unix_millis()
        ));
        let state = ServerState::with_paths(
            "test-instance".to_string(),
            RuntimePaths::from_dir(path.clone()),
        )
        .expect("state");
        let owner = OwnerId::new("test-user", "test-client").expect("owner");
        let conn = ConnHandle::new(2);

        let usage = dispatch(
            &state,
            &owner,
            ClientMessage::JournalUsage { id: 1 },
            &conn,
            true,
            true,
            true,
            true,
        )
        .expect("immediate dispatch reply");
        assert!(matches!(usage, DaemonMessage::JournalUsage { id: 1, .. }));
        let retention = dispatch(
            &state,
            &owner,
            ClientMessage::JournalRetentionGet { id: 2 },
            &conn,
            true,
            true,
            true,
            true,
        )
        .expect("immediate dispatch reply");
        assert!(matches!(
            retention,
            DaemonMessage::JournalRetention { id: 2, .. }
        ));

        for patch in [
            RetentionPatch {
                max_age_ms: Some(-1),
                max_bytes: None,
                max_sessions: None,
                session_max_bytes: None,
            },
            RetentionPatch {
                max_age_ms: None,
                max_bytes: Some(-1),
                max_sessions: None,
                session_max_bytes: None,
            },
            RetentionPatch {
                max_age_ms: None,
                max_bytes: None,
                max_sessions: Some(-1),
                session_max_bytes: None,
            },
            RetentionPatch {
                max_age_ms: None,
                max_bytes: None,
                max_sessions: None,
                session_max_bytes: Some(-1),
            },
            RetentionPatch {
                max_age_ms: None,
                max_bytes: Some(10),
                max_sessions: None,
                session_max_bytes: Some(11),
            },
        ] {
            let RetentionPatch {
                max_age_ms,
                max_bytes,
                max_sessions,
                session_max_bytes,
            } = patch;
            let reply = dispatch(
                &state,
                &owner,
                ClientMessage::JournalRetentionSet {
                    id: 3,
                    max_age_ms,
                    max_bytes,
                    max_sessions,
                    session_max_bytes,
                    idempotency_key: None,
                },
                &conn,
                true,
                true,
                true,
                true,
            )
            .expect("immediate dispatch reply");
            assert!(matches!(
                reply,
                DaemonMessage::Error(WireError {
                    code: ErrorCode::InvalidRequest,
                    id: Some(3),
                    ..
                })
            ));
        }
        let db = rusqlite::Connection::open(RuntimePaths::from_dir(path.clone()).journal_file())
            .expect("open journal for live row");
        db.execute(
            "INSERT INTO sessions (
                id, owner, kind, title, created_at_ms, updated_at_ms, generation,
                status, closed, last_seq, degraded, payload_bytes, unsnapshotted_bytes, reaped
             ) VALUES (?1, ?2, 'terminal', 'Live', 1, 1, 1, 'live', 0, 0, 0, 0, 0, 0)",
            ["s.test-client.live", "test-user"],
        )
        .expect("insert live row");
        drop(db);
        let delete = dispatch(
            &state,
            &owner,
            ClientMessage::SessionDelete {
                id: 4,
                session_id: "s.test-client.live".to_string(),
                idempotency_key: None,
            },
            &conn,
            true,
            true,
            true,
            true,
        )
        .expect("immediate dispatch reply");
        assert!(matches!(
            delete,
            DaemonMessage::Error(WireError {
                code: ErrorCode::InvalidRequest,
                id: Some(4),
                message,
                ..
            }) if message == "Close the session before deleting it."
        ));
        assert!(state
            .sessions
            .list(&owner)
            .expect("list after refused delete")
            .iter()
            .any(|session| session.id == "s.test-client.live"));
        state.sessions.flush_journal();
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn journal_mutations_replay_idempotently() {
        let path = std::env::temp_dir().join(format!(
            "devboule-idempotency-test-{}-{}",
            std::process::id(),
            unix_millis()
        ));
        let state = ServerState::with_paths(
            "test-instance".to_string(),
            RuntimePaths::from_dir(path.clone()),
        )
        .expect("state");
        let owner = OwnerId::new("test-user", "test-client").expect("owner");
        let conn = ConnHandle::new(3);

        let first_retention = dispatch(
            &state,
            &owner,
            ClientMessage::JournalRetentionSet {
                id: 1,
                max_age_ms: None,
                max_bytes: Some(20_000),
                max_sessions: None,
                session_max_bytes: Some(10_000),
                idempotency_key: Some("retention-once".to_string()),
            },
            &conn,
            true,
            true,
            true,
            true,
        )
        .expect("immediate dispatch reply");
        assert!(matches!(
            first_retention,
            DaemonMessage::JournalRetention { id: 1, .. }
        ));
        let replayed_retention = dispatch(
            &state,
            &owner,
            ClientMessage::JournalRetentionSet {
                id: 2,
                max_age_ms: None,
                max_bytes: Some(20_000),
                max_sessions: None,
                session_max_bytes: Some(10_000),
                idempotency_key: Some("retention-once".to_string()),
            },
            &conn,
            true,
            true,
            true,
            true,
        )
        .expect("immediate dispatch reply");
        assert!(matches!(
            replayed_retention,
            DaemonMessage::JournalRetention { id: 2, .. }
        ));
        let conflict = dispatch(
            &state,
            &owner,
            ClientMessage::JournalRetentionSet {
                id: 3,
                max_age_ms: None,
                max_bytes: Some(20_001),
                max_sessions: None,
                session_max_bytes: Some(10_000),
                idempotency_key: Some("retention-once".to_string()),
            },
            &conn,
            true,
            true,
            true,
            true,
        )
        .expect("immediate dispatch reply");
        assert!(matches!(
            conflict,
            DaemonMessage::Error(WireError {
                code: ErrorCode::IdempotencyConflict,
                id: Some(3),
                ..
            })
        ));

        let db = rusqlite::Connection::open(RuntimePaths::from_dir(path.clone()).journal_file())
            .expect("open journal for deleted row");
        db.execute(
            "INSERT INTO sessions (
                id, owner, kind, title, created_at_ms, updated_at_ms, generation,
                status, closed, last_seq, degraded, payload_bytes, unsnapshotted_bytes, reaped
             ) VALUES (?1, ?2, 'terminal', 'Deleted', 1, 1, 1, 'ended', 0, 0, 0, 0, 0, 0)",
            ["s.test-client.idempotent-delete", "test-user"],
        )
        .expect("insert deleted row");
        drop(db);
        let first_delete = dispatch(
            &state,
            &owner,
            ClientMessage::SessionDelete {
                id: 4,
                session_id: "s.test-client.idempotent-delete".to_string(),
                idempotency_key: Some("delete-once".to_string()),
            },
            &conn,
            true,
            true,
            true,
            true,
        )
        .expect("immediate dispatch reply");
        assert!(matches!(first_delete, DaemonMessage::Ok { id: 4 }));
        let replayed_delete = dispatch(
            &state,
            &owner,
            ClientMessage::SessionDelete {
                id: 5,
                session_id: "s.test-client.idempotent-delete".to_string(),
                idempotency_key: Some("delete-once".to_string()),
            },
            &conn,
            true,
            true,
            true,
            true,
        )
        .expect("immediate dispatch reply");
        assert!(matches!(replayed_delete, DaemonMessage::Ok { id: 5 }));
        drop(state);
        let _ = std::fs::remove_dir_all(path);
    }

    /// The peer gate must return **before** the `ProvidersRefresh` and
    /// `ProviderUpdate` spawns, so a remote peer can never trigger an npm
    /// install or a catalog refresh on this device (design §8b A1).
    ///
    /// Two assertions make this real rather than a claim about a reply's shape:
    ///
    /// 1. the same `dispatch` call from a **local** connection returns `None`
    ///    for these variants, which is the async wrapper's own signature for
    ///    "I spawned a worker" — so `Some(Error)` from a peer is the gate
    ///    returning early, not the variant being synchronous by nature; and
    /// 2. the update path's process-launch seam is a recording runner, and a
    ///    remote `ProviderUpdate` leaves it with no new invocation while a
    ///    local one adds one. That is the side effect the gate exists to
    ///    prevent, observed directly.
    #[test]
    fn the_peer_gate_denies_the_destructive_set_before_any_spawn() {
        let path = std::env::temp_dir().join(format!(
            "devboule-peer-gate-{}-{}",
            std::process::id(),
            unix_millis()
        ));
        std::fs::create_dir_all(&path).expect("runtime directory");
        let executable = path.join("codex.cmd");
        std::fs::write(&executable, b"shim").expect("fake executable");
        let calls = Arc::new(Mutex::new(Vec::new()));
        let runner = Arc::new(RecordingNpmRunner {
            calls: Arc::clone(&calls),
            result: crate::provider_update::NpmInstallResult {
                exit_code: Some(0),
                log: String::new(),
            },
        });
        let state = ServerState::with_paths_and_npm_install_runner(
            "test-instance".to_string(),
            RuntimePaths::from_dir(path.clone()),
            runner,
        )
        .expect("state");
        state.set_provider_update_catalog(crate::provider_catalog::ProviderDiscovery {
            agents: vec![update_test_agent(
                "codex",
                Some("@openai/codex"),
                true,
                crate::provider_catalog::InstallChannel::Npm,
                executable.clone(),
            )],
            unreadable_dirs: 0,
        });
        state.set_provider_update_npm_command(std::path::PathBuf::from(r"C:\fake\npm.cmd"), vec![]);

        let owner = OwnerId::new("test-user", "test-client").expect("owner");
        let local = ConnHandle::new(1);
        let remote = remote_conn(PeerRole::Daemon, None);

        // 1. The local path really does spawn, for both async variants. This is
        //    the control that makes the peer assertion below meaningful.
        for request in [
            ClientMessage::ProvidersRefresh { id: 2 },
            ClientMessage::ProviderUpdate {
                id: 3,
                provider_id: "codex".to_string(),
            },
        ] {
            let name = request.name();
            assert!(
                dispatch(&state, &owner, request, &local, true, true, true, true).is_none(),
                "the local path spawns a worker for {name}: `None` is how the async wrapper says so"
            );
        }
        // The local update reaches the runner, which proves the seam is armed.
        let deadline = Instant::now() + Duration::from_secs(5);
        while calls.lock().unwrap_or_else(|e| e.into_inner()).is_empty()
            && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        let local_calls = calls.lock().unwrap_or_else(|e| e.into_inner()).len();
        assert_eq!(
            local_calls, 1,
            "a local ProviderUpdate must reach the npm runner"
        );

        // 2. Every one of these is refused to a peer, synchronously.
        let requests = [
            ClientMessage::Shutdown { id: 1 },
            ClientMessage::ProvidersRefresh { id: 2 },
            ClientMessage::ProviderUpdate {
                id: 3,
                provider_id: "codex".to_string(),
            },
            ClientMessage::Status { id: 4 },
        ];
        for request in requests {
            let name = request.name();
            let reply = dispatch(&state, &owner, request, &remote, true, true, true, true)
                .unwrap_or_else(|| {
                    panic!("the gate must answer {name} instead of spawning a worker")
                });
            match reply {
                DaemonMessage::Error(error) => assert_eq!(
                    error.code,
                    ErrorCode::CapabilityNotSupported,
                    "unexpected refusal for {name}: {error:?}"
                ),
                other => panic!("expected CapabilityNotSupported for {name}, got {other:?}"),
            }
        }

        // Give a thread that should not exist a moment to prove otherwise: if
        // the gate had let the update through, this is where its npm call would
        // land, and the count would move past the local one.
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(
            calls.lock().unwrap_or_else(|e| e.into_inner()).len(),
            local_calls,
            "a remote ProviderUpdate must not start an npm install"
        );

        drop(state);
        assert_eq!(
            audit_rows(&path),
            vec![
                "Shutdown:denied",
                "ProvidersRefresh:denied",
                "ProviderUpdate:denied",
                "Status:denied",
            ]
        );
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn an_allowed_read_writes_no_audit_row() {
        let (path, state) = temp_state("peer-ping");
        let owner = OwnerId::new("test-user", "test-client").expect("owner");
        let conn = remote_conn(PeerRole::Client, Some("S-1-5-21-1"));
        for id in 0..20 {
            let reply = dispatch(
                &state,
                &owner,
                ClientMessage::Ping { id },
                &conn,
                true,
                true,
                true,
                true,
            )
            .expect("ping replies");
            assert!(matches!(reply, DaemonMessage::Pong { .. }));
        }
        drop(state);
        assert!(
            audit_rows(&path).is_empty(),
            "20 allowed pings must not write an audit row"
        );
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn a_remote_sessions_list_is_projected_by_role_and_paired_user() {
        let (path, state) = temp_state("peer-sessions");
        let owner = OwnerId::new("test-user", "test-client").expect("owner");
        {
            let journal = state.journal.as_ref().expect("journal");
            journal
                .upsert_blocking(new_session_record(
                    "s.user-a.1",
                    "S-user-a",
                    None,
                    SessionKind::Terminal,
                    "A",
                ))
                .expect("session a");
            journal
                .upsert_blocking(new_session_record(
                    "s.user-b.1",
                    "S-user-b",
                    None,
                    SessionKind::Terminal,
                    "B",
                ))
                .expect("session b");
        }

        let ids = |conn: &Arc<ConnHandle>| match dispatch(
            &state,
            &owner,
            ClientMessage::SessionsList { id: 1 },
            conn,
            true,
            true,
            true,
            true,
        )
        .expect("sessions reply")
        {
            DaemonMessage::Sessions { sessions, .. } => sessions
                .into_iter()
                .map(|session| session.id)
                .collect::<Vec<_>>(),
            other => panic!("expected Sessions, got {other:?}"),
        };

        assert_eq!(
            ids(&remote_conn(PeerRole::Client, Some("S-user-a"))),
            vec!["s.user-a.1".to_string()]
        );
        assert_eq!(
            ids(&remote_conn(PeerRole::Client, Some("S-user-b"))),
            vec!["s.user-b.1".to_string()]
        );
        assert!(ids(&remote_conn(PeerRole::Client, None)).is_empty());
        assert!(ids(&remote_conn(PeerRole::Daemon, Some("S-user-a"))).is_empty());

        // A read is a read: none of these wrote an audit row.
        drop(state);
        assert!(audit_rows(&path).is_empty());
        let _ = std::fs::remove_dir_all(path);
    }

    /// The initiator path must be reachable from the dispatch gate, not just
    /// compiled. The address is a closed port, so the pairing fails at connect:
    /// what this asserts is that the request reaches `PairingService::complete`
    /// (an error, not a `CapabilityNotSupported` refusal and not a panic) and
    /// that the failure text never echoes the code the caller typed.
    #[test]
    fn pairing_complete_reaches_the_initiator_and_never_echoes_the_code() {
        use std::net::TcpListener;

        let (path, state) = temp_state("pairing-complete");
        let owner = OwnerId::new("test-user", "test-client").expect("owner");
        let conn = ConnHandle::new(3);
        assert!(
            state
                .set_peer_transport(Arc::new(crate::peer_transport::TestTransport::default()))
                .is_ok(),
            "the stub transport is installed before anything can choose the real one"
        );

        // A port that was bound and then released: connecting is refused rather
        // than left hanging.
        let port = {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
            listener.local_addr().expect("addr").port()
        };
        let address = format!("127.0.0.1:{port}");

        let reply = dispatch(
            &state,
            &owner,
            ClientMessage::PairingComplete {
                id: 5,
                address,
                code: devboule_protocol::PairingSecret::new("ABCD2345"),
                role: PeerRole::Client,
            },
            &conn,
            true,
            true,
            true,
            true,
        )
        .expect("immediate dispatch reply");
        match reply {
            DaemonMessage::Error(error) => {
                assert_eq!(error.id, Some(5));
                assert_ne!(
                    error.code,
                    ErrorCode::CapabilityNotSupported,
                    "the devices capability is negotiated here, so this must not be a policy refusal"
                );
                assert!(
                    !error.message.contains("ABCD2345"),
                    "a pairing failure must never carry the code: {}",
                    error.message
                );
            }
            other => panic!("expected a pairing failure, got {other:?}"),
        }

        // A code the alphabet cannot express is refused before any socket work.
        let reply = dispatch(
            &state,
            &owner,
            ClientMessage::PairingComplete {
                id: 6,
                address: "127.0.0.1:1".to_string(),
                code: devboule_protocol::PairingSecret::new("aaaa0000"),
                role: PeerRole::Client,
            },
            &conn,
            true,
            true,
            true,
            true,
        )
        .expect("immediate dispatch reply");
        match reply {
            DaemonMessage::Error(error) => {
                assert!(!error.message.contains("aaaa0000"), "{}", error.message)
            }
            other => panic!("expected a malformed-code refusal, got {other:?}"),
        }

        drop(state);
        let _ = std::fs::remove_dir_all(path);
    }

    #[test]
    fn status_carries_the_secret_store_selector_and_a_remote_state() {
        let (path, state) = temp_state("status");
        let _ = state.secret_store();

        let remote_of = |state: &Arc<ServerState>| match state.status_body(9) {
            DaemonMessage::Status { body, .. } => body.remote.expect("remote is always reported"),
            other => panic!("expected Status, got {other:?}"),
        };

        // The selector is one of exactly two values, and it is reported.
        match state.status_body(9) {
            DaemonMessage::Status { body, .. } => assert!(
                matches!(body.secret_store.as_deref(), Some("keyring" | "file")),
                "unexpected selector: {:?}",
                body.secret_store
            ),
            other => panic!("expected Status, got {other:?}"),
        }

        // `disabled` is the start-up state, with a reason.
        let disabled = remote_of(&state);
        assert_eq!(disabled.state, devboule_protocol::RemoteStateKind::Disabled);
        assert!(
            disabled.reason.is_some(),
            "a disabled state explains itself"
        );

        // `enabled`: the listener is up. Status carries the state and reason
        // only — the addresses and port live in `SelfInfo`.
        state.set_remote_state(RemoteState::Enabled {
            addresses: vec!["100.102.128.70".parse().expect("ip")],
            port: 47831,
        });
        let enabled = remote_of(&state);
        assert_eq!(enabled.state, devboule_protocol::RemoteStateKind::Enabled);
        assert!(
            enabled.reason.is_none(),
            "an enabled listener has nothing to explain: {enabled:?}"
        );
        let json = serde_json::to_value(&enabled).expect("json");
        assert_eq!(json["state"], "enabled");
        assert!(json["reason"].is_null(), "the key is present as null");

        // `key_missing`: device.json exists but the private key does not. This
        // is a refusal to guess, and it must be distinguishable from
        // `disabled` because the remedy differs (re-pair vs start Tailscale).
        state.set_remote_state(RemoteState::KeyMissing);
        let missing = remote_of(&state);
        assert_eq!(
            missing.state,
            devboule_protocol::RemoteStateKind::KeyMissing
        );
        assert!(missing.reason.is_some());
        assert_ne!(
            serde_json::to_value(&missing).expect("json")["state"],
            "disabled",
            "a missing key is not the same state as a disabled listener"
        );

        drop(state);
        let _ = std::fs::remove_dir_all(path);
    }

    /// The leak that this guards: a state built by a test must not reach the OS
    /// credential store.
    ///
    /// `ServerState::initial_secret_store` pins a test build to the file store,
    /// so the identity lands in `<runtime dir>/secrets/noise-static.bin` and
    /// dies with the temp dir. The credential store is deliberately not read
    /// here — reading it is still touching it, and keeping this suite out of it
    /// is the point; `cmdkey /list | Select-String noise-static-` from outside
    /// the process is the gate that observes that, and the store selector plus
    /// the file path below are what is observable from in here.
    ///
    /// See `reports/remote-agents/keyring-test-leak-fix-report.md`.
    #[test]
    fn a_test_built_state_uses_the_file_store_and_never_the_credential_store() {
        let (path, state) = temp_state("secret-store-pin");
        assert_eq!(
            state.secret_store().1,
            "file",
            "a test build must select the file store, never the credential store"
        );

        let identity = state.device_identity().as_ref().expect("identity");
        let stored = path.join("secrets").join("noise-static.bin");
        let bytes = std::fs::read(&stored).expect("the static key under the runtime dir");
        // The bytes under the temp dir are this identity's own envelope: the
        // store is rooted in this state's runtime directory, not somewhere else.
        let expected = crate::device_identity::encode_envelope(identity.private_key());
        assert_eq!(bytes.as_slice(), &expected[..]);
        // ...and the path is the one the file store derives for that name, so a
        // later test cannot satisfy this through some other mechanism.
        assert_eq!(
            crate::secret_store::FileStore::new(&path)
                .path_for(crate::device_identity::NOISE_STATIC_SECRET_NAME)
                .expect("plain secret name"),
            stored
        );

        drop(state);
        let _ = std::fs::remove_dir_all(path);
    }
    /// M1: the accept path must not hit the journal per accepted socket. The
    /// table is loaded once and reused, and every peer mutation drops it so the
    /// next connection sees the change.
    #[test]
    fn the_peer_table_is_loaded_once_and_refreshed_on_change() {
        let (path, state) = temp_state("peer-table-cache");

        // First read loads; the next N reads within the TTL do not.
        let first = state.peer_table().expect("first load");
        assert!(first.rows().is_empty(), "no peers yet");
        let loads_after_first = state.peer_table_loads();
        for _ in 0..16 {
            state.peer_table().expect("cached read");
        }
        assert_eq!(
            state.peer_table_loads(),
            loads_after_first,
            "16 reads within the TTL must not reload the table"
        );

        // A pairing invalidates it: the next read sees the new row.
        state
            .peer_upsert(PeerRecord {
                device_id: "6f1e5b7a-0000-4000-8000-00000000c0db".to_string(),
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
                caps: vec!["view".to_string()],
            })
            .expect("store a peer");
        let after_pairing = state.peer_table().expect("reload after pairing");
        assert_eq!(after_pairing.rows().len(), 1, "the new peer is visible");
        assert_eq!(
            state.peer_table_loads(),
            loads_after_first + 1,
            "a pairing loads exactly once more"
        );

        // A revoke invalidates it too: the row stops owning its address.
        state
            .peer_revoke("6f1e5b7a-0000-4000-8000-00000000c0db", 2)
            .expect("revoke");
        let after_revoke = state.peer_table().expect("reload after revoke");
        assert_eq!(
            after_revoke.rows().len(),
            1,
            "the revoked row is still listed"
        );
        assert!(
            after_revoke
                .by_address(&"100.64.0.2".parse().expect("ip"))
                .is_none(),
            "a revoked peer's address must no longer pass the filter"
        );
        assert_eq!(state.peer_table_loads(), loads_after_first + 2);

        // And the TTL is a backstop for a mutation path that forgets to
        // invalidate: a zero TTL forces the next read to reload.
        state.set_peer_table_ttl(Duration::ZERO);
        state.peer_table().expect("reload after ttl");
        assert_eq!(state.peer_table_loads(), loads_after_first + 3);

        drop(state);
        let _ = std::fs::remove_dir_all(path);
    }

    /// C5: the listener is a resource the state owns, started idempotently and
    /// stoppable. This is the machinery `PairingStart` relies on when it retries
    /// after the user starts Tailscale, exercised here with the stub transport
    /// (which binds loopback) so it needs no Tailscale.
    #[test]
    fn the_remote_listener_starts_once_and_stops() {
        let (path, state) = temp_state("listener-lifecycle");
        // `is_ok()`, not `expect`: the error is an `Arc<dyn PeerTransport>`,
        // which is not `Debug` and so cannot be printed by `expect`.
        assert!(
            state
                .set_peer_transport(Arc::new(crate::peer_transport::TestTransport::default()))
                .is_ok(),
            "the stub transport is installed before anything picks the real one"
        );

        // Starts, and reports `Enabled` with the bound port.
        assert!(state.ensure_remote_listener(), "the first call starts it");
        assert_eq!(state.listener_starts(), 1);
        assert!(state.has_remote_listener());
        let enabled = state.remote_state();
        assert_eq!(
            enabled.state,
            devboule_protocol::RemoteStateKind::Enabled,
            "a started listener is reported as enabled"
        );
        assert!(enabled.reason.is_none(), "nothing to explain when it is up");

        // Idempotent: further calls do not start a second listener.
        for _ in 0..3 {
            assert!(state.ensure_remote_listener());
        }
        assert_eq!(
            state.listener_starts(),
            1,
            "ensure_remote_listener must not start a second listener"
        );

        // Stopping takes it down, and a start afterwards is refused because the
        // daemon is shutting down (the stop flag would make a new loop exit at
        // once, so `listening` would be a lie).
        state.stop_remote_listener();
        assert!(!state.has_remote_listener());
        assert!(
            !state.ensure_remote_listener(),
            "a listener must not be started after it has been stopped"
        );

        drop(state);
        let _ = std::fs::remove_dir_all(path);
    }

    /// A capability a peer does not hold is the whole answer: the request is
    /// refused before anything looks at what it would do (§8b A9/A11). The
    /// same request with the capability is answered by the session layer —
    /// which is what proves the gate, and not the missing session, stopped it.
    #[test]
    fn a_peer_request_the_caps_do_not_open_never_reaches_the_session_layer() {
        let (path, state) = temp_state("peer-caps-first");
        let owner = OwnerId::new("test-user", "test-client").expect("owner");
        let send = || ClientMessage::SessionSend {
            id: 1,
            session_id: "s.none.1".to_string(),
            subscription_id: 1,
            text: "hello".to_string(),
            attachments: Vec::new(),
            idempotency_key: None,
        };

        let viewer = remote_conn_with_caps(
            PeerRole::Client,
            Some("S-user-a"),
            &[crate::peer_policy::CAP_VIEW],
        );
        match dispatch(&state, &owner, send(), &viewer, true, true, true, true)
            .expect("the gate answers")
        {
            DaemonMessage::Error(error) => {
                assert_eq!(error.code, ErrorCode::CapabilityNotSupported, "{error:?}")
            }
            other => panic!("a view-only peer must not reach a session: {other:?}"),
        }

        let sender = remote_conn_with_caps(
            PeerRole::Client,
            Some("S-user-a"),
            &[crate::peer_policy::CAP_VIEW, crate::peer_policy::CAP_SEND],
        );
        match dispatch(&state, &owner, send(), &sender, true, true, true, true)
            .expect("the gate answers")
        {
            DaemonMessage::Error(error) => assert_ne!(
                error.code,
                ErrorCode::CapabilityNotSupported,
                "with the capability the answer comes from the session layer: {error:?}"
            ),
            other => panic!("expected a session-layer error, got {other:?}"),
        }

        drop(state);
        // The refusal is recorded, and the allowed send that followed is
        // recorded as the decision it was: the trail distinguishes the two.
        let rows = audit_rows(&path);
        assert_eq!(
            rows.first().map(String::as_str),
            Some("SessionSend:denied"),
            "the capability refusal comes first: {rows:?}"
        );
        assert_eq!(
            rows.len(),
            2,
            "the refusal, then the allowed send: {rows:?}"
        );
        let _ = std::fs::remove_dir_all(path);
    }

    /// §8b A5 in the trail: a paired device asking for a mode that runs without
    /// the prompt is refused, and the row says *that*, not a plain denial.
    #[test]
    fn a_peer_create_in_a_prompt_skipping_mode_is_refused_and_labelled() {
        let (path, state) = temp_state("peer-prompt-skipping");
        let owner = OwnerId::new("test-user", "test-client").expect("owner");
        let creator = remote_conn_with_caps(
            PeerRole::Client,
            Some("S-user-a"),
            &[
                crate::peer_policy::CAP_VIEW,
                crate::peer_policy::CAP_CREATE_SESSIONS,
            ],
        );

        let reply = dispatch(
            &state,
            &owner,
            ClientMessage::SessionCreate {
                id: 1,
                workspace_id: None,
                kind: SessionKind::Claude,
                provider: Some("claude".to_string()),
                mode: Some("bypassPermissions".to_string()),
                idempotency_key: None,
            },
            &creator,
            true,
            true,
            true,
            true,
        )
        .expect("the gate answers");
        match reply {
            DaemonMessage::Error(error) => {
                assert_eq!(error.code, ErrorCode::CapabilityNotSupported, "{error:?}");
                assert!(
                    error.message.contains("permission prompt"),
                    "the refusal must say why: {}",
                    error.message
                );
            }
            other => panic!("a prompt-skipping create must be refused: {other:?}"),
        }

        drop(state);
        assert_eq!(
            audit_rows(&path),
            vec!["SessionCreate:prompt_skipping_refused"]
        );
        let _ = std::fs::remove_dir_all(path);
    }

    /// The A4/A5 decision itself, keyed on the frame's own facts. The
    /// session-backed half (send, set-mode) is the registry guard — tested in
    /// `session.rs` — composed with this same list.
    #[test]
    fn the_prompt_skipping_decision_reads_the_frame_and_refuses_unknown_sessions() {
        let (path, state) = temp_state("peer-prompt-skipping-table");
        let create = |kind: SessionKind, mode: &str| ClientMessage::SessionCreate {
            id: 1,
            workspace_id: None,
            kind,
            provider: None,
            mode: Some(mode.to_string()),
            idempotency_key: None,
        };

        assert!(peer_mode_refusal(
            &state,
            &create(SessionKind::Claude, "bypassPermissions")
        ));
        assert!(peer_mode_refusal(
            &state,
            &create(SessionKind::Claude, "auto")
        ));
        assert!(peer_mode_refusal(
            &state,
            &create(SessionKind::Codex, "full-access")
        ));
        // Codex `auto-review` and Claude's `acceptEdits` are refusals too, and
        // the ones that only prompt are not.
        assert!(!peer_mode_refusal(
            &state,
            &create(SessionKind::Codex, "auto")
        ));
        assert!(!peer_mode_refusal(
            &state,
            &create(SessionKind::Claude, "default")
        ));
        assert!(!peer_mode_refusal(
            &state,
            &create(SessionKind::Terminal, "bypassPermissions")
        ));

        // A session this daemon does not know is not a policy verdict: the
        // answer for it is `SessionNotFound`, from the ownership path.
        assert!(!peer_mode_refusal(
            &state,
            &ClientMessage::SessionSend {
                id: 2,
                session_id: "s.nobody.1".to_string(),
                subscription_id: 1,
                text: "hello".to_string(),
                attachments: Vec::new(),
                idempotency_key: None,
            }
        ));
        assert!(!peer_mode_refusal(
            &state,
            &ClientMessage::SessionSetMode {
                id: 3,
                session_id: "s.nobody.1".to_string(),
                mode_id: "bypassPermissions".to_string(),
            }
        ));

        drop(state);
        let _ = std::fs::remove_dir_all(path);
    }

    /// §8 R7 at the boundary: one gate, and every local fact in an error is
    /// replaced for a peer while the person's own pipe sees it unchanged.
    #[test]
    fn a_remote_reply_is_redacted_at_the_boundary_and_a_local_one_is_not() {
        let path = "C:\\Users\\me\\AppData\\Local\\devboule\\journal.db";
        let digest = "a".repeat(64);
        // Quoted, because a path run deliberately swallows an unquoted tail
        // (`path_run_len` stops at punctuation, not at a colon or a space): the
        // digest has to be its own token for this test to be about the digest.
        let message =
            format!("could not write \"{path}\"; key {digest} was rejected by device dev-phone");
        let error = || WireError::new(ErrorCode::Io, message.clone());

        let local = redact_for_conn(&ConnHandle::new(1), DaemonMessage::Error(error()));
        match local {
            DaemonMessage::Error(error) => {
                assert_eq!(error.message, message, "the pipe is unchanged")
            }
            other => panic!("expected an error, got {other:?}"),
        }

        let remote = redact_for_conn(
            &remote_conn_with_caps(PeerRole::Client, Some("S-user-a"), &[]),
            DaemonMessage::Error(error()),
        );
        match remote {
            DaemonMessage::Error(error) => {
                assert!(
                    !error.message.contains("C:\\Users"),
                    "a peer must not learn a path: {}",
                    error.message
                );
                assert!(error.message.contains("<path>"), "{}", error.message);
                assert!(
                    !error.message.contains(&digest),
                    "a peer must not learn a digest: {}",
                    error.message
                );
                assert!(error.message.contains("<digest>"), "{}", error.message);
                assert_eq!(error.code, ErrorCode::Io, "the code stays: what failed");
            }
            other => panic!("expected an error, got {other:?}"),
        }

        // Everything that is not an error is not a place to rewrite: the event
        // stream carries the owner's own screen (§8b A14).
        let event = DaemonMessage::Ok { id: 4 };
        assert!(matches!(
            redact_for_conn(&remote_conn_with_caps(PeerRole::Daemon, None, &[]), event),
            DaemonMessage::Ok { id: 4 }
        ));
    }

    /// The capability set is read from the device's own row, and every failure
    /// — unknown device, revoked row — yields the empty set, which the gate
    /// reads as "no capability".
    #[test]
    fn the_capability_set_of_a_device_comes_from_its_row_and_fails_closed() {
        let (path, state) = temp_state("peer-caps-lookup");
        assert!(state.peer_caps("dev-unknown").is_empty());

        let mut record = PeerRecord {
            device_id: "dev-phone".to_string(),
            display_name: "Phone".to_string(),
            role: "client".to_string(),
            // The store refuses a peer key that is not a 32-byte X25519 public key,
            // and that refusal is the point: a fixture cannot skip the shape.
            public_key: vec![7u8; 32],
            paired_by_user: Some("S-user-a".to_string()),
            binding_kind: "tailnet".to_string(),
            binding_stable_id: Some("nstable".to_string()),
            binding_node_name: Some("node".to_string()),
            binding_login_name: Some("user@example.com".to_string()),
            address: "100.64.0.2:47831".to_string(),
            paired_at: 1,
            revoked_at: None,
            caps: vec![
                crate::peer_policy::CAP_VIEW.to_string(),
                crate::peer_policy::CAP_SEND.to_string(),
            ],
        };
        state.peer_upsert(record.clone()).expect("store");

        let mut caps = state.peer_caps("dev-phone");
        caps.sort();
        assert_eq!(
            caps,
            vec![
                crate::peer_policy::CAP_SEND.to_string(),
                crate::peer_policy::CAP_VIEW.to_string()
            ]
        );

        // A revoked row grants nothing, whatever it still carries.
        record.revoked_at = Some(2);
        state.peer_upsert(record).expect("re-store");
        assert!(state.peer_caps("dev-phone").is_empty());

        drop(state);
        let _ = std::fs::remove_dir_all(path);
    }

    /// S7 after the scope correction: the attachment counter is the peer's
    /// deposit branch, so until it lands a send from a paired device that
    /// carries an attachment is refused — before any decode, and the store
    /// stays empty. A peer's text-only send and a local send are unchanged.
    #[test]
    fn a_peer_send_with_attachments_is_refused_until_the_deposit_counter_lands() {
        let (path, state) = temp_state("peer-attachments");
        let owner = OwnerId::new("test-user", "test-client").expect("owner");
        let sender = remote_conn_with_caps(
            PeerRole::Client,
            Some("S-user-a"),
            &[crate::peer_policy::CAP_VIEW, crate::peer_policy::CAP_SEND],
        );
        let send = |attachments: Vec<PromptAttachment>| ClientMessage::SessionSend {
            id: 1,
            session_id: "s.none.1".to_string(),
            subscription_id: 1,
            text: "hello".to_string(),
            attachments,
            idempotency_key: None,
        };
        let dispatch_send = |attachments: Vec<PromptAttachment>, conn: &Arc<ConnHandle>| {
            dispatch(
                &state,
                &owner,
                send(attachments),
                conn,
                true,
                true,
                true,
                true,
            )
            .expect("the gate answers")
        };

        match dispatch_send(vec![wire_attachment("a.png", b"one")], &sender) {
            DaemonMessage::Error(error) => {
                assert_eq!(error.code, ErrorCode::InvalidRequest, "{error:?}");
                assert_eq!(
                    error.message,
                    crate::peer_policy::PEER_ATTACHMENTS_UNSUPPORTED
                );
            }
            other => panic!("a peer's attachment send must be refused: {other:?}"),
        }
        assert_eq!(
            files_under(&path.join("attachments")).len(),
            0,
            "a refused send must not write an attachment file"
        );

        match dispatch_send(Vec::new(), &sender) {
            DaemonMessage::Error(error) => assert_ne!(
                error.message,
                crate::peer_policy::PEER_ATTACHMENTS_UNSUPPORTED,
                "the refusal is about the attachments, not the device"
            ),
            other => panic!("a peer's text-only send reaches the session layer: {other:?}"),
        }

        match dispatch_send(vec![wire_attachment("a.png", b"one")], &ConnHandle::new(3)) {
            DaemonMessage::Error(error) => assert_ne!(
                error.message,
                crate::peer_policy::PEER_ATTACHMENTS_UNSUPPORTED,
                "the local pipe keeps its attachments"
            ),
            other => panic!("a local send reaches the session layer: {other:?}"),
        }

        drop(state);
        let _ = std::fs::remove_dir_all(path);
    }
}
