//! State domain — pass-3a split of `server.rs`: `ServerState` and its
//! implementation, the state-adjacent types, and the test seams that live
//! with them (the eleven scattered `#[cfg(test)]` items move along; the two
//! inline test-only branches stay inside their methods).

use super::*;

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
pub(super) struct Lifecycle {
    pub(super) clients: u32,
    pub(super) sessions: u32,
    pub(super) shutting_down: bool,
    pub(super) idle_generation: u64,
}

pub struct ServerState {
    pub(super) instance_id: String,
    pub(super) started: Instant,
    pub(super) stop: Arc<AtomicBool>,
    pub(super) lifecycle: Mutex<Lifecycle>,
    shutdown_flag: Arc<Mutex<bool>>,
    shutdown_cvar: Arc<Condvar>,
    pub(super) idempotency: Mutex<IdempotencyStore>,
    pub(crate) process_job: Arc<JobObject>,
    pub(crate) mcp: Arc<crate::mcp_broker::McpBroker>,
    /// Per-provider tool policy, read by the MCP broker on every
    /// `tools/list` and `tools/call` and written by `ToolPolicySet`. One
    /// instance per daemon: the file beside the journal is this daemon's,
    /// and a paired device's toggles are its own.
    pub(crate) tool_policy: Arc<crate::tool_policy::ToolPolicyStore>,
    /// The agent-profile document: the ordered list a creation resolves and the
    /// standing instructions that travel with it. Written by
    /// `AgentProfilesSet`, read by the creation path at the moment it resolves a
    /// profile — never cached per session, so an edit takes effect on the next
    /// creation.
    pub(crate) agent_profiles: Arc<crate::agent_profiles::AgentProfilesStore>,
    /// The permission-delegation switch. Written by `DelegationSet`; the only
    /// readers are the `DelegationGet`/`DelegationSet` dispatch arms — the
    /// read-cadence rule lives at the store, and the nothing-reads-it test in
    /// `delegation_store.rs` holds this field to it.
    pub(crate) delegation: Arc<crate::delegation_store::DelegationStore>,
    pub sessions: SessionRegistry,
    conn_ids: AtomicU64,
    pub(super) journal_error: Mutex<Option<String>>,
    pub(super) session_watchers: Mutex<HashMap<u64, SessionWatch>>,
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
    pub(super) claude_version_probes: Mutex<HashSet<std::path::PathBuf>>,
    /// Test-only: how many times `probe_native_version` was entered. That
    /// function is the spawn seam itself — entering it is what becomes a
    /// `claude --version` process in a release build — so the test that pins
    /// a Claude read's process cost counts entries here instead of watching
    /// `provider_health`, which no real probe writes.
    #[cfg(test)]
    pub(super) version_probe_entries: AtomicU64,
    /// The provider-vocabulary cache: what each installed provider's models
    /// and modes look like, keyed by canonical provider id and invalidated by
    /// `refresh: true`, by changed discovery facts, by a 30-minute TTL, and
    /// by the catalog derivation completing (`provider_vocabulary.rs`). It
    /// feeds the profile form only — the live `SessionManifest` never reads
    /// it, and per-session chips keep coming from the manifest as before.
    pub(crate) provider_vocabulary: crate::provider_vocabulary::VocabularyCache,
    /// The only process-launch seam for provider updates. Tests replace this
    /// runner so no npm or network is ever started by the test suite.
    pub(super) npm_install_runner: Arc<dyn NpmInstallRunner>,
    /// Runtime paths, kept so the secret store can be selected lazily.
    /// Probing the OS credential store here would put a credential read into
    /// every unit test that builds a `ServerState`.
    pub(super) paths: RuntimePaths,
    /// The same journal handle the session registry writes through, kept for
    /// the `peers` and `audit` tables (schema v8). `None` when the journal
    /// could not be opened.
    pub(super) journal: Option<Arc<Journal>>,
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
    pub(super) peer_stop: Arc<AtomicBool>,
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
    pub(super) provider_update_catalog: Mutex<Option<crate::provider_catalog::ProviderDiscovery>>,
    #[cfg(test)]
    pub(super) provider_update_npm_command: Mutex<Option<ProviderUpdateNpmCommand>>,
}

#[cfg(test)]
#[derive(Clone)]
pub(super) enum ProviderUpdateNpmCommand {
    Resolved(std::path::PathBuf, Vec<String>),
    Missing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CliVersionFingerprint {
    pub(super) modified: SystemTime,
    pub(super) len: u64,
}

pub(super) struct SessionWatch {
    pub(super) owner: OwnerId,
    conn: Arc<ConnHandle>,
    last_snapshot: Option<Vec<devboule_protocol::SessionStateSnapshot>>,
}

pub(super) fn session_state_event(
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
        // The user's provider rows must be live BEFORE the profile store
        // validates, because a profile may name one. Without this the load
        // asks a catalogue that has no user rows yet, the profile naming one
        // is refused, and `load` quarantines the WHOLE document: the user
        // loses every profile and their standing instructions on a restart,
        // for a configuration that is legitimate.
        crate::user_providers::refresh_user_rows(&paths.dir);
        // Read at startup like the tool policy, and read again at every
        // creation: the store holds the document, the creation path asks it for
        // one, and nothing in a session keeps a copy.
        let agent_profiles = Arc::new(crate::agent_profiles::AgentProfilesStore::load(&paths.dir));
        // The delegation switch loads the same way: the store holds the
        // boolean, every consumer asks it at the moment it decides (the
        // read-cadence rule is stated at the store), and a corrupt file
        // quarantines into read-off with one log line.
        let delegation = Arc::new(crate::delegation_store::DelegationStore::load(&paths.dir));
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
        let sessions = SessionRegistry::new(paths, journal);
        // The registry reads the profile store at one moment — a session's first
        // prompt — so it holds the handle, not a copy of the document
        // (`create-from-profile`). It is built here rather than inline in the
        // struct literal below because the store has to be attached to it.
        sessions.attach_agent_profiles(Arc::clone(&agent_profiles));
        // The switch rides along the same way: the registry holds the store,
        // and asks it at the moment of each decision.
        sessions.attach_delegation(Arc::clone(&delegation));
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
            agent_profiles,
            delegation,
            sessions,
            conn_ids: AtomicU64::new(1),
            journal_error: Mutex::new(journal_error),
            session_watchers: Mutex::new(HashMap::new()),
            provider_health: Mutex::new(HashMap::new()),
            provider_versions: Mutex::new(HashMap::new()),
            provider_cli_versions: Mutex::new(HashMap::new()),
            claude_version_probes: Mutex::new(HashSet::new()),
            #[cfg(test)]
            version_probe_entries: AtomicU64::new(0),
            provider_vocabulary: crate::provider_vocabulary::VocabularyCache::default(),
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

    pub(super) fn watch_sessions(&self, owner: &OwnerId, conn: &Arc<ConnHandle>) {
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

    pub(super) fn unwatch_sessions(&self, conn_id: u64) {
        self.session_watchers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&conn_id);
    }

    pub(super) fn broadcast_session_state(&self, owner: &OwnerId) {
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

    /// Push the delegation switch to every session-watching connection.
    ///
    /// The setting is global to this daemon and its `DelegationGet` is read
    /// once at the app's mount, so a write that reached only the writer would
    /// leave every other client — and the writer's other surfaces — holding a
    /// stale value, and a stale OFF hides the very control that stops
    /// delegation. The value pushed is the pair the store returned from
    /// `set`, so every client converges on what the daemon holds rather than
    /// on what any request said.
    ///
    /// Session watchers are the audience because they are every local app
    /// connection that shows live state: `sessions.watch` is refused to peers,
    /// and peers are refused the switch (`peer_allows`) and never hold it. A
    /// local connection that never watches has no switch surface to go stale;
    /// it answers `DelegationGet` fresh on its next ask. There is no
    /// last-snapshot suppression here the way the roster broadcast has: the
    /// pair is two small values, and a repeat of the same value is what lets
    /// a client that missed an earlier push converge.
    pub(super) fn broadcast_delegation(
        &self,
        enabled: bool,
        source: devboule_protocol::DelegationSource,
    ) {
        let watchers = self
            .session_watchers
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        for watch in watchers.values() {
            watch
                .conn
                .outbound
                .enqueue_reply(DaemonMessage::DelegationChanged { enabled, source });
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

    pub(super) fn signal_shutdown(&self) {
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

    pub(super) fn is_shutting_down(&self) -> bool {
        self.lifecycle
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .shutting_down
    }

    /// Admit a client unless shutdown has started. A reconnect that wins this
    /// lock invalidates any idle timer armed by the previous connection.
    pub(super) fn client_connected(&self) -> bool {
        let mut lifecycle = self.lifecycle.lock().unwrap_or_else(|err| err.into_inner());
        if lifecycle.shutting_down {
            return false;
        }
        lifecycle.clients = lifecycle.clients.saturating_add(1);
        lifecycle.idle_generation = lifecycle.idle_generation.wrapping_add(1);
        true
    }

    pub(super) fn client_disconnected(self: &Arc<Self>) {
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

    pub(super) fn record_provider_cli_version(
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

    /// How many times the native version probe was entered: the process
    /// count of whatever path is under test, observed where a release build
    /// actually spawns.
    #[cfg(test)]
    pub(super) fn version_probe_entry_count(&self) -> u64 {
        self.version_probe_entries.load(Ordering::SeqCst)
    }

    pub(crate) fn provider_cli_version(
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
        self.claude_models_in_paths(&crate::provider_catalog::path_directories_for_available())
    }

    /// `claude_models` with the PATH scan injected: the same body over
    /// explicit search directories, so a test can install a native Claude
    /// that exists nowhere else on the machine. Production callers go
    /// through [`ServerState::claude_models`].
    pub(crate) fn claude_models_in_paths(
        self: &Arc<Self>,
        directories: &[std::path::PathBuf],
    ) -> crate::claude_catalog::ClaudeCatalogSnapshot {
        let Some(agent) = crate::provider_catalog::find_available_in_paths("claude", directories)
        else {
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
                state.claude_catalog_derived(models)
            });
    }

    /// What happens when the catalog derivation delivers: the manifest
    /// publication the live sessions read, and the eviction of Claude's
    /// vocabulary-cache entry. The eviction is the fourth invalidation: a
    /// completed derivation changes the vocabulary answer without changing
    /// any discovery fact — the executable and the version are byte-identical
    /// before and after — so an entry filled from the provisional fallback
    /// while the derivation ran cannot be caught by the facts and must be
    /// dropped here, or the next read serves it, labelled `cache`, until the
    /// TTL expires.
    pub(crate) fn claude_catalog_derived(
        self: &Arc<Self>,
        models: Vec<devboule_protocol::SessionModel>,
    ) {
        self.provider_vocabulary.invalidate("claude");
        self.sessions.publish_claude_catalog(models);
    }

    pub(super) fn invalidate_provider_update_caches(&self, provider_id: &str) {
        self.provider_cli_versions
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(provider_id);
        // Only the installed state changed: dropping the executable fingerprint
        // forces the next --version observation to be fresh. npm latest is a
        // registry property, so it remains valid under its six-hour TTL.
    }

    #[cfg(test)]
    pub(super) fn set_provider_update_catalog(
        &self,
        discovery: crate::provider_catalog::ProviderDiscovery,
    ) {
        *self
            .provider_update_catalog
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(discovery);
    }

    #[cfg(test)]
    pub(super) fn set_provider_update_npm_command(
        &self,
        program: std::path::PathBuf,
        prefix: Vec<String>,
    ) {
        *self
            .provider_update_npm_command
            .lock()
            .unwrap_or_else(|error| error.into_inner()) =
            Some(ProviderUpdateNpmCommand::Resolved(program, prefix));
    }

    #[cfg(test)]
    pub(super) fn set_provider_update_npm_missing(&self) {
        *self
            .provider_update_npm_command
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(ProviderUpdateNpmCommand::Missing);
    }

    pub(super) fn status_body(&self, request_id: u64) -> DaemonMessage {
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
    pub(super) fn secret_store(&self) -> &(Arc<dyn SecretStore>, &'static str) {
        self.secret_store.get_or_init(|| {
            let (store, kind) = crate::secret_store::select_secret_store(&self.paths);
            (store, kind.as_str())
        })
    }

    pub(super) fn remote_state(&self) -> devboule_protocol::RemoteState {
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
