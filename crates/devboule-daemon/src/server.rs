use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use devboule_protocol::{
    caps, m3a_daemon_capabilities, negotiate, ClientMessage, DaemonHello, DaemonMessage,
    DaemonStatusBody, ErrorCode, OwnerId, WireError, PROTOCOL_MIN_VERSION, PROTOCOL_VERSION,
};

use crate::error::DaemonError;
use crate::framing::Framed;
use crate::idempotency::IdempotencyStore;
use crate::lock::SingleInstanceLock;
use crate::paths::RuntimePaths;
use crate::transport::{self, Listener};

const LAST_CLIENT_GRACE: Duration = Duration::from_secs(1);
const JOIN_SLICE: Duration = Duration::from_millis(10);
const JOIN_BUDGET: Duration = Duration::from_millis(500);

pub struct ServerState {
    instance_id: String,
    started: Instant,
    stop: Arc<AtomicBool>,
    clients: Arc<AtomicU32>,
    shutdown_flag: Arc<Mutex<bool>>,
    shutdown_cvar: Arc<Condvar>,
    /// Remembered create/send/permission-response keys. M3a does not serve
    /// those RPCs yet; the store is here so M3b does not have to change the
    /// daemon's memory shape.
    #[allow(dead_code)]
    idempotency: Mutex<IdempotencyStore>,
}

impl ServerState {
    pub fn new(instance_id: String) -> Arc<Self> {
        Arc::new(Self {
            instance_id,
            started: Instant::now(),
            stop: Arc::new(AtomicBool::new(false)),
            clients: Arc::new(AtomicU32::new(0)),
            shutdown_flag: Arc::new(Mutex::new(false)),
            shutdown_cvar: Arc::new(Condvar::new()),
            idempotency: Mutex::new(IdempotencyStore::default()),
        })
    }

    pub fn request_shutdown(&self) {
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

    fn client_connected(&self) {
        self.clients.fetch_add(1, Ordering::SeqCst);
    }

    fn client_disconnected(&self) {
        let previous = self.clients.fetch_sub(1, Ordering::SeqCst);
        if previous == 1 && !self.stop.load(Ordering::SeqCst) {
            arm_idle_shutdown(self);
        }
    }

    fn status_body(&self, request_id: u64) -> DaemonMessage {
        DaemonMessage::Status {
            id: request_id,
            body: DaemonStatusBody {
                instance_id: self.instance_id.clone(),
                protocol_version: PROTOCOL_VERSION,
                daemon_version: env!("CARGO_PKG_VERSION").to_string(),
                pid: std::process::id(),
                uptime_ms: u64::try_from(self.started.elapsed().as_millis()).unwrap_or(u64::MAX),
                clients: self.clients.load(Ordering::SeqCst),
                sessions: 0,
                capabilities: m3a_daemon_capabilities(),
            },
        }
    }
}

/// Last-client grace. M3c's journal flush hooks `request_shutdown`, which is
/// the only deliberate process-exit path besides a fatal error.
fn arm_idle_shutdown(state: &ServerState) {
    let stop = Arc::clone(&state.stop);
    let clients = Arc::clone(&state.clients);
    let flag = Arc::clone(&state.shutdown_flag);
    let cvar = Arc::clone(&state.shutdown_cvar);
    let _ = std::thread::Builder::new()
        .name("daemon-idle".into())
        .spawn(move || {
            std::thread::sleep(LAST_CLIENT_GRACE);
            if clients.load(Ordering::SeqCst) == 0 && !stop.load(Ordering::SeqCst) {
                stop.store(true, Ordering::SeqCst);
                let mut locked = flag.lock().unwrap_or_else(|err| err.into_inner());
                *locked = true;
                cvar.notify_all();
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

    let state = ServerState::new(instance_id);
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
                if state.stop.load(Ordering::SeqCst) {
                    drop(stream);
                    break;
                }
                state.client_connected();
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

fn handle_client(mut framed: Framed, state: Arc<ServerState>) -> Result<(), DaemonError> {
    let hello: ClientMessage = framed.recv()?;
    let ClientMessage::Hello(client_hello) = hello else {
        framed.send(&DaemonMessage::Error(WireError::new(
            ErrorCode::InvalidRequest,
            "first frame must be hello",
        )))?;
        return Ok(());
    };
    let daemon_hello = daemon_hello(&state);
    match negotiate(&client_hello, &daemon_hello) {
        Ok(_agreed) => {
            framed.send(&DaemonMessage::Hello(daemon_hello))?;
        }
        Err(error) => {
            framed.send(&DaemonMessage::Error(error))?;
            return Ok(());
        }
    }
    let owner = client_hello.owner;
    loop {
        if state.stop.load(Ordering::SeqCst) {
            return Ok(());
        }
        let request: ClientMessage = match framed.recv() {
            Ok(request) => request,
            Err(DaemonError::Io(error))
                if error.kind() == std::io::ErrorKind::UnexpectedEof
                    || error.kind() == std::io::ErrorKind::BrokenPipe
                    || error.kind() == std::io::ErrorKind::ConnectionReset =>
            {
                return Ok(());
            }
            Err(error) => return Err(error),
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
        let reply = dispatch(&state, &owner, request);
        let shutting_down = matches!(reply, DaemonMessage::Shutdown { accepted: true, .. });
        framed.send(&reply)?;
        if shutting_down {
            state.request_shutdown();
            return Ok(());
        }
    }
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

fn dispatch(state: &ServerState, _owner: &OwnerId, request: ClientMessage) -> DaemonMessage {
    if state.stop.load(Ordering::SeqCst) && !matches!(request, ClientMessage::Shutdown { .. }) {
        let mut error = WireError::new(ErrorCode::ShuttingDown, "daemon is shutting down");
        if let Some(id) = request.request_id() {
            error = error.with_id(id);
        }
        return DaemonMessage::Error(error);
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
        other => session_not_in_m3a(other),
    }
}

fn session_not_in_m3a(request: ClientMessage) -> DaemonMessage {
    let id = request.request_id();
    let mut error = WireError::new(
        ErrorCode::CapabilityNotSupported,
        format!(
            "capability '{}' is not offered in M3a; sessions still run in-process in the app",
            caps::SESSIONS
        ),
    );
    if let Some(id) = id {
        error = error.with_id(id);
    }
    DaemonMessage::Error(error)
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
