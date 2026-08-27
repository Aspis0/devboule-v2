use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender, TryRecvError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use devboule_protocol::{
    caps, m3a_daemon_capabilities, negotiate, validate_idempotency_key, ClientMessage, DaemonHello,
    DaemonMessage, DaemonStatusBody, ErrorCode, OwnerId, SessionKind, WireError,
    PROTOCOL_MIN_VERSION, PROTOCOL_VERSION,
};

use crate::error::DaemonError;
use crate::framing::Framed;
use crate::idempotency::{IdempotencyOutcome, IdempotencyStore};
use crate::lock::SingleInstanceLock;
use crate::outbound::ConnOut;
use crate::paths::RuntimePaths;
use crate::session::{ConnHandle, SessionRegistry};
use crate::transport::{self, Listener};
use crate::IDLE_SHUTDOWN_GRACE;

const JOIN_SLICE: Duration = Duration::from_millis(10);
const JOIN_BUDGET: Duration = Duration::from_millis(500);

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
    pub sessions: SessionRegistry,
    conn_ids: AtomicU64,
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
    }

    pub fn with_paths(instance_id: String, paths: RuntimePaths) -> Arc<Self> {
        Arc::new(Self {
            instance_id,
            started: Instant::now(),
            stop: Arc::new(AtomicBool::new(false)),
            lifecycle: Mutex::new(Lifecycle::default()),
            shutdown_flag: Arc::new(Mutex::new(false)),
            shutdown_cvar: Arc::new(Condvar::new()),
            idempotency: Mutex::new(IdempotencyStore::default()),
            sessions: SessionRegistry::new(paths),
            conn_ids: AtomicU64::new(1),
        })
    }

    pub fn alloc_conn(&self) -> u64 {
        self.conn_ids.fetch_add(1, Ordering::Relaxed)
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

    fn status_body(&self, request_id: u64) -> DaemonMessage {
        let lifecycle = self.lifecycle.lock().unwrap_or_else(|err| err.into_inner());
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

    let state = ServerState::with_paths(instance_id, paths.clone());
    let (listener, shutdown) = transport::bind(&paths, Arc::clone(&state.stop))?;
    let accept_state = Arc::clone(&state);
    let accept = std::thread::Builder::new()
        .name("daemon-accept".into())
        .spawn(move || accept_loop(listener, accept_state))
        .map_err(DaemonError::from)?;

    state.wait_until_shutdown();
    // M3c journal flush goes here, before the listener is torn down.
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
                        let _ = handle_client(Framed::new(stream), conn_state.clone());
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
    let hello: ClientMessage = framed.recv()?;
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
    let daemon_hello = daemon_hello(&state);
    let agreed = match negotiate(&client_hello, &daemon_hello) {
        Ok(agreed) => {
            framed.send(&DaemonMessage::Hello(daemon_hello))?;
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
    let owner = client_hello.owner;
    let conn = ConnHandle::new(state.alloc_conn());
    let (request_tx, request_rx) = mpsc::sync_channel(64);
    let reader_wake = Arc::clone(&conn.outbound);
    let reader_framed = framed.clone();
    let reader = std::thread::Builder::new()
        .name("daemon-client-request".into())
        .spawn(move || read_client_requests(reader_framed, request_tx, reader_wake))
        .map_err(DaemonError::from)?;
    loop {
        if state.stop.load(Ordering::SeqCst) {
            break;
        }
        let observed_generation = conn.outbound.wake_generation();
        for event in conn.pull_events() {
            framed.send(&DaemonMessage::Event(event))?;
        }
        let request = match request_rx.try_recv() {
            Ok(Ok(request)) => request,
            Ok(Err(error)) if connection_closed(&error) || state.stop.load(Ordering::SeqCst) => {
                break;
            }
            Ok(Err(error)) => {
                conn.detach_all(&state.sessions);
                conn.outbound.close();
                framed.cancel_read();
                bounded_join(reader, JOIN_BUDGET);
                return Err(error);
            }
            Err(TryRecvError::Empty) => {
                if !conn
                    .outbound
                    .wait_for_notify_since(observed_generation, conn.next_exit_wake())
                {
                    break;
                }
                continue;
            }
            Err(TryRecvError::Disconnected) => break,
        };
        if let ClientMessage::Hello(_) = request {
            let id = request.request_id();
            let mut error = WireError::new(ErrorCode::InvalidRequest, "hello already completed");
            if let Some(id) = id {
                error = error.with_id(id);
            }
            framed.send(&DaemonMessage::Error(error))?;
            continue;
        }
        let reply = dispatch(&state, &owner, request, &conn, sessions_ok);
        let shutting_down = matches!(reply, DaemonMessage::Shutdown { accepted: true, .. });
        framed.send(&reply)?;
        for event in conn.pull_events() {
            framed.send(&DaemonMessage::Event(event))?;
        }
        if shutting_down {
            state.request_shutdown();
            break;
        }
    }
    framed.cancel_read();
    conn.detach_all(&state.sessions);
    conn.outbound.close();
    bounded_join(reader, JOIN_BUDGET);
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
    conn: &ConnHandle,
    sessions_ok: bool,
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
        ClientMessage::Ping { id } => DaemonMessage::Pong {
            id,
            ts_ms: unix_millis(),
        },
        ClientMessage::Status { id } => state.status_body(id),
        ClientMessage::Shutdown { id } => DaemonMessage::Shutdown { id, accepted: true },
        ClientMessage::SessionCreate { .. }
        | ClientMessage::SessionAttach { .. }
        | ClientMessage::SessionDetach { .. }
        | ClientMessage::SessionClose { .. }
        | ClientMessage::SessionStop { .. }
        | ClientMessage::SessionSend { .. }
        | ClientMessage::SessionResize { .. }
        | ClientMessage::SessionInterrupt { .. }
        | ClientMessage::SessionPermissionRespond { .. }
        | ClientMessage::SessionsList { .. }
        | ClientMessage::SessionResume { .. } => {
            if !sessions_ok {
                return capability_not_supported(request.request_id());
            }
            dispatch_session(state, owner, request, conn)
        }
    }
}

fn capability_not_supported(id: Option<u64>) -> DaemonMessage {
    let mut error = WireError::new(
        ErrorCode::CapabilityNotSupported,
        format!("capability '{}' was not negotiated", caps::SESSIONS),
    );
    if let Some(id) = id {
        error = error.with_id(id);
    }
    DaemonMessage::Error(error)
}

fn dispatch_session(
    state: &Arc<ServerState>,
    owner: &OwnerId,
    request: ClientMessage,
    conn: &ConnHandle,
) -> DaemonMessage {
    match request {
        ClientMessage::SessionCreate {
            id,
            workspace_id,
            kind,
            idempotency_key,
        } => session_create(state, owner, id, workspace_id, kind, idempotency_key),
        ClientMessage::SessionAttach {
            id,
            session_id,
            from_cursor,
        } => reply_result(
            id,
            state
                .sessions
                .attach(&session_id, from_cursor, conn)
                .map(|()| DaemonMessage::Ok { id }),
        ),
        ClientMessage::SessionDetach { id, session_id } => reply_result(
            id,
            state
                .sessions
                .detach(&session_id, conn)
                .map(|()| DaemonMessage::Ok { id }),
        ),
        ClientMessage::SessionClose { id, session_id } => match state.sessions.close(&session_id) {
            Ok(removed) => {
                if removed {
                    state.session_finished();
                }
                DaemonMessage::Ok { id }
            }
            Err(error) => DaemonMessage::Error(error.with_id(id)),
        },
        ClientMessage::SessionStop { id, session_id } => reply_result(
            id,
            state
                .sessions
                .stop(&session_id)
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
                .resize(&session_id, cols, rows)
                .map(|()| DaemonMessage::Ok { id }),
        ),
        ClientMessage::SessionsList { id } => match state.sessions.list() {
            Ok(sessions) => DaemonMessage::Sessions { id, sessions },
            Err(error) => DaemonMessage::Error(error.with_id(id)),
        },
        ClientMessage::SessionInterrupt { id, .. }
        | ClientMessage::SessionPermissionRespond { id, .. }
        | ClientMessage::SessionResume { id, .. } => DaemonMessage::Error(
            WireError::new(ErrorCode::Unimplemented, "not implemented in M3b").with_id(id),
        ),
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
    idempotency_key: Option<String>,
) -> DaemonMessage {
    let fingerprint = format!(
        "create:{}:{}",
        match kind {
            SessionKind::Terminal => "terminal",
            SessionKind::Acp => "acp",
        },
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
        .create(state, owner, workspace_id, kind, None)
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
    match state.sessions.send(&session_id, &text) {
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
    use devboule_protocol::{ClientMessage, OwnerId};

    fn state() -> Arc<ServerState> {
        ServerState::new("test-instance".to_string())
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
}
