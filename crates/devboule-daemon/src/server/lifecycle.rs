//! Lifecycle domain — pass-3a split of `server.rs`: daemon startup
//! (`run`), the accept loops, and the idle-shutdown arming.

use super::*;
use crate::daemon_record::{DaemonRecord, ExitReason, Heartbeat};

/// Begin shutdown only if the lifecycle snapshot that armed this timer is
/// still current. The lifecycle mutex makes the final check and the shutdown
/// transition atomic with client reconnects and session transitions.
pub(super) fn arm_idle_shutdown(state: Arc<ServerState>, generation: u64) {
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
                    lifecycle.exit_reason = Some(ExitReason::Idle);
                    lifecycle.idle_generation = lifecycle.idle_generation.wrapping_add(1);
                }
                should_shutdown
            };
            if should_shutdown {
                state.signal_shutdown();
            }
        });
}

/// The shutdown sequence between the quit signal and teardown, in
/// production order: wait bounded for the accepted write jobs — a delete
/// queued behind a slow read or mid-`git worktree remove` lands its row
/// here, while the writer is still alive — then the one terminal journal
/// close. Past the bound, writes still queued are answered rather than run
/// — the close they were meant to precede is next — while writes already
/// running finish. `run` calls this; the shutdown-drain test calls the
/// same function, so the tested order is the shipped order.
pub(super) fn drain_writes_and_close_journal(state: &Arc<ServerState>) -> bool {
    let drained = state.git_jobs.wait_for_write_jobs(GIT_WRITE_DRAIN_BOUND);
    if !drained {
        state.git_jobs.cancel_queued_writes();
        eprintln!(
            "shutdown: git write jobs unfinished after {GIT_WRITE_DRAIN_BOUND:?}; \
             closing the journal anyway"
        );
    }
    // The writer join below blocks this thread without bound: this is the
    // exit path, and an unjoined writer would leave the SQLite connection
    // open on Windows.
    state.sessions.flush_journal();
    drained
}

/// This daemon process's instance id: 128 fresh bits, hex-encoded.
///
/// It is the value every queue snapshot carries as its epoch, so a client can
/// tell two daemons' revisions apart, and the value the single-instance record
/// publishes. A pid and a clock do not prove it: Windows reuses pids, and two
/// starts inside one millisecond happen on a fast restart loop. Entropy does,
/// and this daemon already asks the OS for it in three places.
pub(super) fn instance_id() -> String {
    let mut bytes = [0u8; 16];
    if getrandom::fill(&mut bytes).is_err() {
        // Entropy failure is not fatal anywhere in this daemon — the session
        // nonce degrades the same way — so a daemon that cannot draw is still
        // a daemon, with an epoch that is only best effort.
        eprintln!("daemon could not draw an instance id from the OS; falling back to pid and time");
        return fallback_instance_id();
    }
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        hex.push_str(&format!("{byte:02x}"));
    }
    hex
}

/// The epoch with no entropy to draw on: the pid, the wall clock in nanoseconds
/// and a monotonic reading, mixed into the same 32-hex shape. Best effort — it
/// makes a collision between two starts unlikely, and cannot rule one out.
pub(super) fn fallback_instance_id() -> String {
    use std::hash::{Hash, Hasher};

    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    let monotonic = std::time::Instant::now();
    let mut high = std::collections::hash_map::DefaultHasher::new();
    (pid, nanos, monotonic).hash(&mut high);
    let mut low = std::collections::hash_map::DefaultHasher::new();
    (monotonic, nanos, pid, 1u8).hash(&mut low);
    format!("{:016x}{:016x}", high.finish(), low.finish())
}

pub fn run() -> Result<(), DaemonError> {
    #[cfg(any(windows, unix))]
    {
        run_shared()
    }
    #[cfg(not(any(windows, unix)))]
    {
        return Err(DaemonError::UnsupportedPlatform);
    }
}

/// The daemon startup both platforms run: lock, record, state, broker,
/// listen, serve, shut down. Platform differences are the small `cfg`
/// blocks below (log sink, ConPTY line, endpoint name); order, messages
/// and files are the same everywhere.
#[cfg(any(windows, unix))]
fn run_shared() -> Result<(), DaemonError> {
    run_with_paths(RuntimePaths::from_env()?)
}

/// The shared startup with an explicit runtime dir: production passes the
/// environment's, the smoke test passes a temp dir. Split out so tests
/// drive the real sequence without touching process-global env.
#[cfg(any(windows, unix))]
pub(crate) fn run_with_paths(paths: RuntimePaths) -> Result<(), DaemonError> {
    let mut lock = SingleInstanceLock::acquire(&paths)?;
    // Only now — the single-instance lock is ours — may the log rotate: a
    // losing second daemon must never move the running daemon's log aside.
    #[cfg(windows)]
    crate::daemon_log::rotate_after_lock(&paths.dir);
    // The vendored loader has no logger, so this line reports which ConPTY
    // implementation the process pinned; it sits below `rotate_after_lock`
    // so it lands in the fresh log instead of joining the over-cap one the
    // rotation moves aside whole. The stderr takeover is itself pre-lock —
    // a losing daemon still writes into the running daemon's log; the lock
    // gates the rotation, not the writes — and on a kernel without ConPTY
    // the forced source panics before anything is served or the record
    // written, but after the reopen request whose notice lands in the
    // fresh log — unless the writer's queue stayed full past
    // `ROTATE_RETRY` and the rotation was skipped and recorded in
    // `Status.logError` instead.
    #[cfg(windows)]
    eprintln!("ConPTY: using {}", portable_pty::conpty_source());
    let pid = std::process::id();
    let instance_id = instance_id();
    // The record is written before anything can be served and re-read by
    // nobody here: it exists for the processes that will read it while this
    // one holds the lock.
    #[cfg(windows)]
    let endpoint = paths.pipe_name.clone();
    #[cfg(unix)]
    let endpoint = paths.socket_path.to_string_lossy().into_owned();
    let mut record = DaemonRecord::starting(pid, &instance_id, &endpoint);
    lock.write_body(&record.body())?;

    let state = ServerState::with_paths(instance_id, paths.clone())?;
    // SIGTERM/SIGINT must reach the same shutdown the Shutdown RPC sends:
    // this guard lives for the run and, on drop, unregisters the handlers
    // and joins the reader while the state is still alive.
    #[cfg(unix)]
    let _signal_shutdown = SignalShutdown::install(Arc::clone(&state));
    // Attachments survive a session close that never ran, because the daemon was
    // killed first. Sweep the ones past the retention window on every start:
    // this is the fallback existence's only reason to be here.
    let swept = state.sessions.sweep_attachments(SystemTime::now());
    if !swept.is_empty() {
        // The bytes are summed over the folders that could be read; one that
        // could not is counted as a folder and not as zero bytes, so the line
        // never claims to have reclaimed less than it did.
        let reclaimed: u64 = swept.iter().filter_map(|(_, bytes)| *bytes).sum();
        eprintln!(
            "daemon removed {} attachment folder(s) left by sessions that never closed, \
             reclaiming at least {reclaimed} byte(s)",
            swept.len()
        );
    }
    let mcp_server = state.mcp.start(&state).map_err(DaemonError::from)?;
    let (listener, shutdown) = transport::bind(&paths, Arc::clone(&state.stop))?;
    // Only now is a connect admitted, so only now is the record allowed to say
    // so: a readiness probe reads this and connects on the strength of it.
    record.listening();
    lock.write_body(&record.body())?;
    let heartbeat = match Heartbeat::start(&paths.lock_file) {
        Ok(heartbeat) => Some(heartbeat),
        Err(error) => {
            eprintln!("daemon heartbeat did not start: {error}");
            None
        }
    };
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
    // The one terminal journal close lives in the sequence the shutdown
    // test also runs: drain the accepted write jobs bounded, then close.
    let _ = drain_writes_and_close_journal(&state);
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
    // The beat stops before the goodbye: a beat landing after it would date
    // the record to a moment the daemon was already gone.
    drop(heartbeat);
    record.stopped(state.exit_reason().unwrap_or(ExitReason::Unknown));
    if let Err(error) = lock.write_body(&record.body()) {
        // The record is how the next process tells a deliberate exit from a
        // crash. Failing to write it is worth a line, not a failed exit: the
        // daemon did what it was asked.
        eprintln!("daemon could not record why it stopped: {error}");
    }
    // Flush the log pipeline so the goodbye lines land, then hand stderr
    // back to the launcher's sink.
    #[cfg(windows)]
    crate::log_pipeline::shutdown_log();
    drop(lock);
    Ok(())
}

/// Bind and serve the tailnet listener, or record why it is not up.
///
/// The body of [`ServerState::ensure_remote_listener`], split out so the
/// idempotence and the join handle live with the state that owns them.
#[cfg(windows)]
pub(super) fn try_start_remote_listener(state: &Arc<ServerState>) -> Option<JoinHandle<()>> {
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
pub(super) fn try_start_remote_listener(state: &Arc<ServerState>) -> Option<JoinHandle<()>> {
    state.set_remote_state(RemoteState::Disabled(
        "the daemon does not run on this platform yet".to_string(),
    ));
    None
}

pub(super) fn accept_loop(mut listener: transport::BoundListener, state: Arc<ServerState>) {
    let mut threads: Vec<JoinHandle<()>> = Vec::new();
    loop {
        if state.stop.load(Ordering::SeqCst) {
            break;
        }
        match listener.accept() {
            Ok(stream) => {
                let Some((slot, quit_intent)) = state.admit_client(ClientKind::LocalApp) else {
                    reject_shutting_down(stream);
                    break;
                };
                let conn_state = Arc::clone(&state);
                // A spawn that never started drops the closure with the slot
                // inside it, so a failure releases through the guard as well.
                if let Ok(handle) = std::thread::Builder::new()
                    .name("daemon-client".into())
                    .spawn(move || {
                        // Held for the whole connection, and released by `Drop`
                        // even if `handle_client` unwinds.
                        let _slot = slot;
                        if let Err(error) = handle_client(
                            Framed::new(stream),
                            conn_state.clone(),
                            None,
                            quit_intent,
                        ) {
                            eprintln!("daemon client connection failed: {error}");
                        }
                    })
                {
                    threads.push(handle);
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
