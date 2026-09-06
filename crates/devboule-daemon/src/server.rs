use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender, TryRecvError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use devboule_protocol::{
    caps, m3a_daemon_capabilities, negotiate, validate_idempotency_key, ClientMessage, DaemonHello,
    DaemonMessage, DaemonStatusBody, ErrorCode, JournalLimits as WireJournalLimits,
    JournalSessionUsage as WireJournalSessionUsage, JournalUsage as WireJournalUsage, OwnerId,
    PersistenceKind, ResumeResult, RetentionPatch, SessionEvent, SessionEventEnvelope, SessionKind,
    Unreclaimable as WireUnreclaimable, WireError, PROTOCOL_MIN_VERSION, PROTOCOL_VERSION,
};

use crate::error::DaemonError;
use crate::framing::Framed;
use crate::idempotency::{IdempotencyOutcome, IdempotencyStore};
use crate::journal::Journal;
use crate::lock::SingleInstanceLock;
use crate::outbound::ConnOut;
use crate::paths::RuntimePaths;
use crate::process_tree::JobObject;
use crate::session::{ConnHandle, PendingEvent, SessionRegistry};
use crate::transport::{self, Listener};
use crate::IDLE_SHUTDOWN_GRACE;

const JOIN_SLICE: Duration = Duration::from_millis(10);
const JOIN_BUDGET: Duration = Duration::from_millis(500);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);

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
}

struct SessionWatch {
    owner: OwnerId,
    conn: Arc<ConnHandle>,
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
        Self::with_paths(
            instance_id,
            RuntimePaths::from_dir(
                std::env::temp_dir().join(format!("devboule-test-{}", std::process::id())),
            ),
        )
        .expect("create daemon process job")
    }

    pub fn with_paths(instance_id: String, paths: RuntimePaths) -> Result<Arc<Self>, DaemonError> {
        let _ = paths.ensure_dir();
        let process_job = Arc::new(JobObject::new()?);
        let (journal, journal_error) = match Journal::open(&paths.journal_file()) {
            Ok(journal) => (Some(Arc::new(journal)), None),
            Err(error) => (None, Some(error.to_string())),
        };
        let state = Arc::new(Self {
            instance_id,
            started: Instant::now(),
            stop: Arc::new(AtomicBool::new(false)),
            lifecycle: Mutex::new(Lifecycle::default()),
            shutdown_flag: Arc::new(Mutex::new(false)),
            shutdown_cvar: Arc::new(Condvar::new()),
            idempotency: Mutex::new(IdempotencyStore::default()),
            process_job,
            sessions: SessionRegistry::new(paths, journal),
            conn_ids: AtomicU64::new(1),
            journal_error: Mutex::new(journal_error),
            session_watchers: Mutex::new(HashMap::new()),
            provider_health: Mutex::new(HashMap::new()),
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
        watchers.insert(
            conn.id,
            SessionWatch {
                owner: owner.clone(),
                conn: Arc::clone(conn),
            },
        );
        // Registration and the initial snapshot share the watcher lock with
        // transition broadcasts, so the first pushed change cannot overtake
        // the state that established this subscription.
        let snapshots = self.sessions.state_snapshots(owner);
        conn.queue_state_event(session_state_event(snapshots));
    }

    fn unwatch_sessions(&self, conn_id: u64) {
        self.session_watchers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(&conn_id);
    }

    fn broadcast_session_state(&self, owner: &OwnerId) {
        let watchers = self
            .session_watchers
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let connections = watchers
            .values()
            .filter(|watch| watch.owner == *owner)
            .map(|watch| Arc::clone(&watch.conn))
            .collect::<Vec<_>>();
        if connections.is_empty() {
            return;
        }
        let event = session_state_event(self.sessions.state_snapshots(owner));
        for conn in connections {
            conn.queue_state_event(event.clone());
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
                journal_stats: self.sessions.journal_stats(),
            },
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
    let (listener, shutdown) = transport::bind(&paths, Arc::clone(&state.stop))?;
    let accept_state = Arc::clone(&state);
    let accept = std::thread::Builder::new()
        .name("daemon-accept".into())
        .spawn(move || accept_loop(listener, accept_state))
        .map_err(DaemonError::from)?;

    state.wait_until_shutdown();
    // Flush the conversation journal before the listener is torn down so a
    // clean shutdown does not drop the last coalesced frames.
    state.sessions.flush_journal();
    shutdown.shutdown();
    let deadline = Instant::now() + JOIN_BUDGET;
    while !accept.is_finished() && Instant::now() < deadline {
        let _ = transport::connect(&paths);
        std::thread::sleep(JOIN_SLICE);
    }
    bounded_join(accept, JOIN_SLICE);
    drop(lock);
    Ok(())
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
                        if let Err(error) = handle_client(Framed::new(stream), conn_state.clone()) {
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

fn handle_client(framed: Framed, state: Arc<ServerState>) -> Result<(), DaemonError> {
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
    #[cfg(windows)]
    let peer = match transport::peer_identity(framed.as_file().as_ref()) {
        Ok(peer) => peer,
        Err(error) => {
            eprintln!("could not derive named-pipe peer identity: {error}");
            let _ = framed.send(&DaemonMessage::Error(WireError::new(
                ErrorCode::Unauthorized,
                "Could not verify the daemon client identity.",
            )));
            return Err(DaemonError::Io(error));
        }
    };
    #[cfg(windows)]
    let true_owner = match OwnerId::new(peer.user.clone(), format!("process-{}", peer.pid)) {
        Ok(owner) => owner,
        Err(message) => {
            let _ = framed.send(&DaemonMessage::Error(WireError::new(
                ErrorCode::Unauthorized,
                "Could not verify the daemon client identity.",
            )));
            return Err(DaemonError::Protocol(message));
        }
    };
    #[cfg(not(windows))]
    let true_owner = client_hello.owner.clone();
    if client_hello.owner != true_owner {
        eprintln!(
            "client hello owner label {:?} did not match pipe peer {:?}",
            client_hello.owner, true_owner
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
    // The hello owner is diagnostic only. All idempotency and session access
    // below use the identity derived from the connected pipe's peer process.
    let owner = true_owner;
    #[cfg(windows)]
    let conn = ConnHandle::with_peer(state.alloc_conn(), Some(peer));
    #[cfg(not(windows))]
    let conn = ConnHandle::with_peer(state.alloc_conn(), None);
    let (request_tx, request_rx) = mpsc::sync_channel(64);
    let reader_wake = Arc::clone(&conn.outbound);
    let reader_framed = framed.clone();
    let reader = std::thread::Builder::new()
        .name("daemon-client-request".into())
        .spawn(move || read_client_requests(reader_framed, request_tx, reader_wake))
        .map_err(DaemonError::from)?;
    let mut pending_events = VecDeque::new();
    let mut pending_state_events = VecDeque::new();
    let loop_result = (|| -> Result<(), DaemonError> {
        loop {
            if state.stop.load(Ordering::SeqCst) {
                break;
            }
            let observed_generation = conn.outbound.wake_generation();
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
                if drains_events_before_dispatch(&request) {
                    // A close must leave the pull state alive for the
                    // post-dispatch pull: teardown_session joins the
                    // coalescer and may publish its final output there.
                    if !close_request {
                        refill_pending_events(&conn, &mut pending_events);
                        refill_pending_state_events(&conn, &mut pending_state_events);
                    }
                    drain_pending_events(&framed, &conn, &mut pending_events)?;
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
                        send_pending_event(&framed, &conn, event)?;
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
                let reply = dispatch(
                    &state,
                    &owner,
                    request,
                    &conn,
                    sessions_ok,
                    journal_ok,
                    typed_permissions_ok,
                );
                if close_request {
                    // SessionClose joins the coalescer and calls finish(),
                    // which can publish the teardown tail after the pre-drain.
                    refill_pending_events(&conn, &mut pending_events);
                    refill_pending_state_events(&conn, &mut pending_state_events);
                    drain_pending_events(&framed, &conn, &mut pending_events)?;
                    drain_pending_state_events(&framed, &mut pending_state_events)?;
                }
                let shutting_down = matches!(reply, DaemonMessage::Shutdown { accepted: true, .. });
                // Control/lifecycle replies retain the flush barrier. It makes
                // the acknowledgement visible before teardown or a shutdown
                // disconnect; the event stream below must never use that
                // barrier per frame.
                framed.send(&reply)?;
                if shutting_down {
                    state.request_shutdown();
                    break;
                }
                continue;
            }

            if pending_events.is_empty() && pending_state_events.is_empty() {
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
                send_pending_event(&framed, &conn, event)?;
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
    if let Err(error) = drain_pending_events(&framed, &conn, &mut pending_events) {
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
    loop_result
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
) -> Result<(), DaemonError> {
    while let Some(event) = pending_events.pop_front() {
        send_pending_event(framed, conn, event)?;
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
) -> Result<(), DaemonError> {
    if !conn.event_is_current(&event.session_id, event.attachment_generation) {
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
            SessionEvent::AgentError { .. } => " agent_error".to_string(),
            SessionEvent::AgentStderr { .. } => " agent_stderr".to_string(),
            SessionEvent::PermissionRequest { .. } => " permission_request".to_string(),
            SessionEvent::PermissionResolved { .. } => " permission_resolved".to_string(),
            SessionEvent::SessionManifest { .. } => " session_manifest".to_string(),
            SessionEvent::AgentReported { .. } => " agent_reported".to_string(),
        };
        eprintln!(
            "discarded stale pending event for session {} generation {}{}",
            event.session_id, event.attachment_generation, sequence
        );
        return Ok(());
    }
    framed.send_unflushed(&DaemonMessage::Event(event.envelope.clone()))?;
    // The cursor is advanced after the complete frame has been written. The
    // clone above is only for the serialized message; the original envelope
    // retains the acknowledgement metadata.
    conn.event_sent(&event);
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

fn dispatch(
    state: &Arc<ServerState>,
    owner: &OwnerId,
    request: ClientMessage,
    conn: &Arc<ConnHandle>,
    sessions_ok: bool,
    journal_ok: bool,
    typed_permissions_ok: bool,
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
        | ClientMessage::SessionDelete { .. } => {
            if !journal_ok {
                return capability_not_supported(request.request_id(), caps::JOURNAL);
            }
            dispatch_journal(state, owner, request)
        }
        ClientMessage::SessionCreate { .. }
        | ClientMessage::SessionAttach { .. }
        | ClientMessage::SessionDetach { .. }
        | ClientMessage::SessionClose { .. }
        | ClientMessage::SessionStop { .. }
        | ClientMessage::SessionSend { .. }
        | ClientMessage::SessionResize { .. }
        | ClientMessage::SessionInterrupt { .. }
        | ClientMessage::SessionSetModel { .. }
        | ClientMessage::SessionPermissionRespond { .. }
        | ClientMessage::SessionsList { .. }
        | ClientMessage::SessionsWatch { .. }
        | ClientMessage::SessionsUnwatch { .. }
        | ClientMessage::SessionResume { .. }
        | ClientMessage::SessionReportAgent { .. } => {
            if !sessions_ok {
                return capability_not_supported(request.request_id(), caps::SESSIONS);
            }
            dispatch_session(state, owner, request, conn, typed_permissions_ok)
        }
        ClientMessage::ProvidersList { id } => {
            let discovery = crate::provider_catalog::discover_catalog(
                &crate::registry::CdnRegistryFetch,
                state.sessions.runtime_dir(),
            );
            // ProviderInfo.authentication carries the measured last-start
            // outcome for this provider: "ok" when the most recent spawn +
            // handshake completed, "failed: <reason>" when it failed, and
            // "unknown" when nothing was measured since daemon start. This
            // is a recorded observation, not an auth probe.
            let providers = discovery
                .agents
                .into_iter()
                .map(|agent| {
                    let authentication = state.provider_health(&agent.id);
                    wire_provider(agent, authentication)
                })
                .collect();
            DaemonMessage::Providers {
                id,
                providers,
                unreadable_dirs: discovery.unreadable_dirs,
            }
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

fn wire_provider(
    agent: crate::provider_catalog::InstalledAgent,
    authentication: String,
) -> devboule_protocol::ProviderInfo {
    devboule_protocol::ProviderInfo {
        id: agent.id.to_string(),
        executable: agent.executable.to_string_lossy().into_owned(),
        acp_available: agent.acp_command.is_some(),
        authentication,
        protocol: crate::provider_catalog::chat_protocol(&agent).map(str::to_string),
        origin: Some(agent.origin.as_wire().to_string()),
        launch_args: agent.launch_args,
        pickable: agent.pickable,
    }
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
            idempotency_key,
        } => session_create(
            state,
            owner,
            id,
            workspace_id,
            kind,
            provider,
            idempotency_key,
        ),
        ClientMessage::SessionAttach {
            id,
            session_id,
            from_cursor,
        } => reply_result(
            id,
            state
                .sessions
                .attach(&session_id, from_cursor, conn, owner, typed_permissions_ok)
                .map(|()| DaemonMessage::Ok { id }),
        ),
        ClientMessage::SessionDetach { id, session_id } => reply_result(
            id,
            state
                .sessions
                .detach(&session_id, conn, owner)
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
            match state.sessions.close(&session_id, owner) {
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
        ClientMessage::SessionStop { id, session_id } => reply_result(
            id,
            state
                .sessions
                .stop(&session_id, owner)
                .map(|()| DaemonMessage::Ok { id }),
        ),
        ClientMessage::SessionSend {
            id,
            session_id,
            text,
            idempotency_key,
        } => session_send(state, owner, id, session_id, text, idempotency_key),
        ClientMessage::SessionResize {
            id,
            session_id,
            cols,
            rows,
        } => reply_result(
            id,
            state
                .sessions
                .resize(&session_id, cols, rows, owner)
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
                        result: ResumeResult::Resumed { session },
                    },
                    Err(error) => DaemonMessage::Error(error.with_id(id)),
                }
            }
        },
        ClientMessage::SessionInterrupt { id, session_id } => reply_result(
            id,
            state
                .sessions
                .interrupt(&session_id, owner)
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
        ClientMessage::SessionPermissionRespond {
            id,
            session_id,
            request_id,
            outcome,
            idempotency_key,
        } => {
            let fingerprint = format!("permission:{session_id}:{request_id}:{outcome:?}");
            if let Some(reply) =
                idempotent_hit(state, owner, id, idempotency_key.as_deref(), &fingerprint)
            {
                return reply;
            }
            match state
                .sessions
                .permission_respond(&session_id, &request_id, outcome, conn, owner)
            {
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

fn session_create(
    state: &Arc<ServerState>,
    owner: &OwnerId,
    id: u64,
    workspace_id: Option<String>,
    kind: SessionKind,
    provider: Option<String>,
    idempotency_key: Option<String>,
) -> DaemonMessage {
    let fingerprint = format!(
        "create:{}:{}:{}",
        match kind {
            SessionKind::Terminal => "terminal",
            SessionKind::Acp => "acp",
            SessionKind::Claude => "claude",
        },
        provider.as_deref().unwrap_or(""),
        workspace_id.as_deref().unwrap_or("")
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
        .create(state, owner, workspace_id, kind, provider, None)
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

fn session_send(
    state: &Arc<ServerState>,
    owner: &OwnerId,
    id: u64,
    session_id: String,
    text: String,
    idempotency_key: Option<String>,
) -> DaemonMessage {
    let fingerprint = format!("send:{session_id}:{text}");
    if let Some(reply) = idempotent_hit(state, owner, id, idempotency_key.as_deref(), &fingerprint)
    {
        return reply;
    }
    match state.sessions.send(&session_id, &text, owner) {
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
        DaemonMessage::JournalRetention { retention, .. } => {
            DaemonMessage::JournalRetention { id, retention }
        }
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

    fn state() -> Arc<ServerState> {
        ServerState::new("test-instance".to_string())
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
        );
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
                request_id: "tool-1".to_string(),
                outcome: PermissionOutcome::AllowOnce,
                idempotency_key: None,
            },
            &conn,
            true,
            true,
            false,
        );
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
        );
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
            assert!(!provider.executable.is_empty());
            assert_eq!(provider.authentication, "unknown");
        }
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
        );
        assert!(matches!(usage, DaemonMessage::JournalUsage { id: 1, .. }));
        let retention = dispatch(
            &state,
            &owner,
            ClientMessage::JournalRetentionGet { id: 2 },
            &conn,
            true,
            true,
            true,
        );
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
            );
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
        );
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
        );
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
        );
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
        );
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
        );
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
        );
        assert!(matches!(replayed_delete, DaemonMessage::Ok { id: 5 }));
        drop(state);
        let _ = std::fs::remove_dir_all(path);
    }
}
