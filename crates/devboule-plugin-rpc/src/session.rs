use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

#[cfg(windows)]
use devboule_daemon::connect_pipe;
use devboule_daemon::{Framed, JobObject};
use devboule_protocol::{
    caps, invoke_method_capability, negotiate, Capability, ClientHello, ClientMessage, DaemonHello,
    DaemonMessage, ErrorCode, OwnerId, WorkspaceRootBody,
};
use serde_json::Value;

use crate::error::PluginError;
use crate::pipe::verify_pipe_server_pid;
use crate::spawn::{spawn_backend, unique_pipe_name, SpawnedBackend};

const CONNECT_ATTEMPTS: u32 = 50;
const CONNECT_SLEEP: Duration = Duration::from_millis(100);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(2);
const DEFAULT_RPC_TIMEOUT: Duration = Duration::from_secs(5);
const FINDINGS_GET_RPC_TIMEOUT: Duration = Duration::from_secs(60);
const CITY_GET_RPC_TIMEOUT: Duration = Duration::from_secs(30);
const FINDING_INSPECT_RPC_TIMEOUT: Duration = Duration::from_secs(10);

/// Production `invoke()` budget. Cheap methods keep 5s; the city walk and
/// the cheap Augur scan are host-perceived waits that routinely exceed that.
/// Keep the map here so protocol stays a name set, not a timer set.
pub fn invoke_timeout_for(method: &str) -> Duration {
    match method {
        name if name == caps::FINDINGS_GET => FINDINGS_GET_RPC_TIMEOUT,
        name if name == caps::CITY_GET => CITY_GET_RPC_TIMEOUT,
        name if name == caps::FINDING_INSPECT => FINDING_INSPECT_RPC_TIMEOUT,
        _ => DEFAULT_RPC_TIMEOUT,
    }
}

#[derive(Clone, Debug)]
pub struct SpawnSpec {
    pub binary: PathBuf,
    pub plugin_id: String,
    pub capabilities: Vec<Capability>,
    pub grants: BTreeMap<String, String>,
    pub owner: OwnerId,
    /// Test-only: the backend sleeps this long on each invoke so a test can
    /// kill it mid-request. Production spawn leaves this `None`.
    pub hang_ms: Option<u64>,
}

struct ProcessState {
    child: SpawnedBackend,
    /// Held so `KILL_ON_JOB_CLOSE` fires when the session is dropped.
    #[allow(dead_code)]
    job: JobObject,
}

struct Transport {
    framed: Framed,
    hello: DaemonHello,
    granted: Vec<Capability>,
}

struct Connected {
    process: ProcessState,
    transport: Transport,
}

pub struct PluginSession {
    spec: SpawnSpec,
    process: Mutex<ProcessState>,
    transport: Mutex<Transport>,
    /// One request/response pair at a time per pipe. This is deliberately
    /// separate from the host's session-map lock: stopping a child must be
    /// able to kill it while this lock is held by a slow IPC call.
    rpc_lock: Mutex<()>,
    next_id: AtomicU64,
    /// Count of invoke() calls that have not yet returned. `ensure_session`
    /// must not ping-kill while this is non-zero: the backend is busy, not
    /// dead, and a ping would sit unread on the serialized pipe.
    inflight: AtomicU32,
}

struct InflightGuard<'a> {
    count: &'a AtomicU32,
}

impl Drop for InflightGuard<'_> {
    fn drop(&mut self) {
        self.count.fetch_sub(1, Ordering::SeqCst);
    }
}

impl PluginSession {
    pub fn spawn(spec: SpawnSpec) -> Result<Self, PluginError> {
        let pipe_name = unique_pipe_name(&spec.plugin_id);
        let connected = spawn_connected(&spec, pipe_name)?;
        Ok(Self {
            spec,
            process: Mutex::new(connected.process),
            transport: Mutex::new(connected.transport),
            rpc_lock: Mutex::new(()),
            next_id: AtomicU64::new(1),
            inflight: AtomicU32::new(0),
        })
    }

    pub fn hello(&self) -> DaemonHello {
        self.transport
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .hello
            .clone()
    }

    pub fn pid(&self) -> u32 {
        self.process
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .child
            .id()
    }

    pub fn granted(&self) -> Vec<Capability> {
        self.transport
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .granted
            .clone()
    }

    pub fn ping(&self) -> Result<u64, PluginError> {
        if self.invoke_in_flight() {
            // Honest liveness: the backend is working, not dead. Sending a
            // ping would wait on rpc_lock (or sit unread) and look like death.
            return Ok(crate::unix_millis());
        }
        let id = self.alloc_id();
        match self.roundtrip(ClientMessage::Ping { id }, DEFAULT_RPC_TIMEOUT)? {
            DaemonMessage::Pong { ts_ms, .. } => Ok(ts_ms),
            DaemonMessage::Error(error) => Err(PluginError::Handshake(error)),
            other => Err(PluginError::Protocol(format!(
                "unexpected plugin frame: {other:?}"
            ))),
        }
    }

    /// True while a production invoke has not yet returned. Used by the host
    /// to skip ping-kill of a healthy busy backend.
    pub fn invoke_in_flight(&self) -> bool {
        self.inflight.load(Ordering::SeqCst) > 0
    }

    pub fn invoke(&self, method: &str, payload: Option<Value>) -> Result<Value, PluginError> {
        self.invoke_timeout(method, payload, invoke_timeout_for(method))
    }

    pub fn invoke_timeout(
        &self,
        method: &str,
        payload: Option<Value>,
        timeout: Duration,
    ) -> Result<Value, PluginError> {
        let granted = self.granted();
        if !method_is_granted(method, &granted) {
            return Err(PluginError::CapabilityNotSupported(method.to_string()));
        }
        self.inflight.fetch_add(1, Ordering::SeqCst);
        let _inflight = InflightGuard {
            count: &self.inflight,
        };
        let id = self.alloc_id();
        let result = self.roundtrip(
            ClientMessage::Invoke {
                id,
                method: method.to_string(),
                payload,
            },
            timeout,
        );
        match result? {
            DaemonMessage::InvokeResult { value, .. } => Ok(value),
            DaemonMessage::Error(error) => Err(PluginError::Handshake(error)),
            other => Err(PluginError::Protocol(format!(
                "unexpected plugin frame: {other:?}"
            ))),
        }
    }

    /// Send a request and do not wait. Used by the crash test to kill the
    /// process while a frame is in flight.
    pub fn write_invoke(&self, method: &str, payload: Option<Value>) -> Result<u64, PluginError> {
        let granted = self.granted();
        if !method_is_granted(method, &granted) {
            return Err(PluginError::CapabilityNotSupported(method.to_string()));
        }
        let id = self.alloc_id();
        self.framed().send(&ClientMessage::Invoke {
            id,
            method: method.to_string(),
            payload,
        })?;
        Ok(id)
    }

    pub fn wait_reply(&self, id: u64, timeout: Duration) -> Result<DaemonMessage, PluginError> {
        self.wait_reply_on(&self.framed(), id, timeout)
    }

    fn wait_reply_on(
        &self,
        framed: &Framed,
        id: u64,
        timeout: Duration,
    ) -> Result<DaemonMessage, PluginError> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(PluginError::timed_out("waiting for a plugin reply"));
            }
            match framed.recv_timeout::<DaemonMessage>(remaining) {
                Ok(message) => {
                    let message_id = match &message {
                        DaemonMessage::Error(error) => error.id,
                        DaemonMessage::Pong { id, .. }
                        | DaemonMessage::Status { id, .. }
                        | DaemonMessage::Shutdown { id, .. }
                        | DaemonMessage::Session { id, .. }
                        | DaemonMessage::Sessions { id, .. }
                        | DaemonMessage::Ok { id }
                        | DaemonMessage::Resume { id, .. }
                        | DaemonMessage::InvokeResult { id, .. } => Some(*id),
                        DaemonMessage::Hello(_) | DaemonMessage::Event(_) => None,
                    };
                    if message_id == Some(id) {
                        return Ok(message);
                    }
                }
                Err(error) => return Err(classify_io(error.into())),
            }
        }
    }

    pub fn kill_process(&self) -> Result<(), PluginError> {
        let mut process = self
            .process
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        kill_child(&mut process.child)
    }

    pub fn respawn(&self) -> Result<(), PluginError> {
        let _rpc_guard = self
            .rpc_lock
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        {
            let mut process = self
                .process
                .lock()
                .unwrap_or_else(|poison| poison.into_inner());
            kill_child(&mut process.child)?;
        }
        let mut spec = self.spec.clone();
        spec.hang_ms = None;
        let pipe_name = unique_pipe_name(&spec.plugin_id);
        let next = spawn_connected(&spec, pipe_name)?;
        *self
            .process
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = next.process;
        *self
            .transport
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = next.transport;
        Ok(())
    }

    fn framed(&self) -> Framed {
        self.transport
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .framed
            .clone()
    }

    fn alloc_id(&self) -> u64 {
        self.next_id.fetch_add(1, Ordering::Relaxed)
    }

    fn roundtrip(
        &self,
        message: ClientMessage,
        timeout: Duration,
    ) -> Result<DaemonMessage, PluginError> {
        let Some(id) = message.request_id() else {
            return Err(PluginError::Protocol(
                "roundtrip requires a request id".to_string(),
            ));
        };
        let _rpc_guard = self
            .rpc_lock
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let framed = self.framed();
        framed.send(&message)?;
        match self.wait_reply_on(&framed, id, timeout) {
            Ok(DaemonMessage::Error(error)) if error.code == ErrorCode::Io => {
                Err(PluginError::ProcessExited)
            }
            other => other,
        }
    }
}

fn kill_child(child: &mut SpawnedBackend) -> Result<(), PluginError> {
    child.kill().map_err(PluginError::from)
}

impl Drop for PluginSession {
    fn drop(&mut self) {
        if let Ok(mut process) = self.process.lock() {
            let _ = kill_child(&mut process.child);
        }
    }
}

pub fn method_is_granted(method: &str, granted: &[Capability]) -> bool {
    let capability = invoke_method_capability(method);
    granted.iter().any(|item| item.as_str() == capability)
}

pub fn workspace_root_from_value(value: &Value) -> Result<WorkspaceRootBody, PluginError> {
    serde_json::from_value(value.clone()).map_err(PluginError::from)
}

fn spawn_connected(spec: &SpawnSpec, pipe_name: String) -> Result<Connected, PluginError> {
    let job = JobObject::new()?;
    let mut child = spawn_backend(&spec.binary, &spec.plugin_id, &pipe_name, spec.hang_ms)?;
    #[cfg(windows)]
    {
        if let Err(error) =
            job.assign_suspended(child.process_handle(), child.primary_thread_handle())
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(PluginError::from(error));
        }
    }

    let file = match connect_with_retry(&pipe_name, &mut child) {
        Ok(file) => file,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(error);
        }
    };
    #[cfg(windows)]
    if let Err(error) = verify_pipe_server_pid(&file, child.id()) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(PluginError::from(error));
    }
    let framed = Framed::new(file);
    let hello = ClientHello::plugin_host(
        spec.owner.clone(),
        "devboule-app",
        spec.capabilities.clone(),
        spec.grants.clone(),
    );
    if let Err(error) = framed.send(&ClientMessage::Hello(hello.clone())) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(error.into());
    }
    let reply: DaemonMessage = match framed.recv_timeout(HANDSHAKE_TIMEOUT) {
        Ok(reply) => reply,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(classify_io(error.into()));
        }
    };
    let daemon_hello = match reply {
        DaemonMessage::Hello(daemon_hello) => daemon_hello,
        DaemonMessage::Error(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(PluginError::Handshake(error));
        }
        other => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(PluginError::Protocol(format!(
                "unexpected plugin hello: {other:?}"
            )));
        }
    };
    let negotiation = match negotiate(&hello, &daemon_hello) {
        Ok(negotiation) => negotiation,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(PluginError::Handshake(error));
        }
    };
    Ok(Connected {
        process: ProcessState { child, job },
        transport: Transport {
            framed,
            hello: daemon_hello,
            granted: negotiation.capabilities,
        },
    })
}

fn connect_with_retry(
    pipe_name: &str,
    child: &mut SpawnedBackend,
) -> Result<std::fs::File, PluginError> {
    #[cfg(windows)]
    {
        let mut last = None;
        for attempt in 0..CONNECT_ATTEMPTS {
            if let Some(status) = child.try_wait().map_err(PluginError::from)? {
                return Err(PluginError::Protocol(format!(
                    "plugin backend exited before the pipe was up (status {status})"
                )));
            }
            match connect_pipe(pipe_name) {
                Ok(file) => return Ok(file),
                Err(error) => {
                    last = Some(error);
                    if attempt + 1 == CONNECT_ATTEMPTS {
                        break;
                    }
                    std::thread::sleep(CONNECT_SLEEP);
                }
            }
        }
        Err(PluginError::from(last.unwrap_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "connecting to the plugin backend",
            )
        })))
    }
    #[cfg(not(windows))]
    {
        let _ = (pipe_name, child);
        Err(PluginError::Protocol(
            "plugin named pipes are Windows-only".to_string(),
        ))
    }
}

fn classify_io(error: PluginError) -> PluginError {
    match error {
        PluginError::Io(inner)
            if inner.kind() == std::io::ErrorKind::UnexpectedEof
                || inner.kind() == std::io::ErrorKind::BrokenPipe
                || inner.kind() == std::io::ErrorKind::ConnectionReset =>
        {
            PluginError::ProcessExited
        }
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn method_is_granted_matches_capability_name() {
        let granted = vec![Capability::new(devboule_protocol::caps::WORKSPACE_ROOT)];
        assert!(method_is_granted("workspace.root", &granted));
        assert!(!method_is_granted("oracle.search", &granted));
    }

    #[test]
    fn production_invoke_budgets_are_per_method() {
        assert_eq!(invoke_timeout_for(caps::FINDINGS_GET), Duration::from_secs(60));
        assert_eq!(invoke_timeout_for(caps::CITY_GET), Duration::from_secs(30));
        assert_eq!(invoke_timeout_for(caps::FINDING_INSPECT), Duration::from_secs(10));
        assert_eq!(invoke_timeout_for(caps::WORKSPACE_ROOT), Duration::from_secs(5));
        assert_eq!(invoke_timeout_for(caps::PING), Duration::from_secs(5));
    }
}
