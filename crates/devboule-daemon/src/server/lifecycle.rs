//! Lifecycle domain — pass-3a split of `server.rs`: daemon startup
//! (`run`), the accept loops, and the idle-shutdown arming.

use super::*;

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

fn accept_loop(mut listener: transport::BoundListener, state: Arc<ServerState>) {
    let mut threads: Vec<JoinHandle<()>> = Vec::new();
    loop {
        if state.stop.load(Ordering::SeqCst) {
            break;
        }
        match listener.accept() {
            Ok(stream) => {
                let Some(slot) = state.admit_client() else {
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
                        if let Err(error) =
                            handle_client(Framed::new(stream), conn_state.clone(), None)
                        {
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
