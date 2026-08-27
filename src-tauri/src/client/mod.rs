//! Daemon client hosted by the Tauri process. Sessions live in the daemon;
//! this process forwards RPCs and fans `SessionEventEnvelope` frames into
//! Tauri Channels. A failed daemon must not hang a terminal: attached
//! Channels receive `exit` with a null code.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use devboule_daemon::{
    connect_or_spawn, current_user_sid, daemon_file_name, DaemonClient, RuntimePaths,
};
use devboule_protocol::ClientHello;
use serde::Serialize;
use tauri::State;

const PING_PERIOD: Duration = Duration::from_secs(2);
const JOIN_BUDGET: Duration = Duration::from_millis(1500);

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct UiDaemonStatus {
    pub state: String,
    pub pid: Option<u32>,
    pub instance_id: Option<String>,
    pub protocol_version: Option<u32>,
    pub clients: Option<u32>,
    pub message: Option<String>,
}

impl UiDaemonStatus {
    fn connecting() -> Self {
        Self {
            state: "connecting".to_string(),
            pid: None,
            instance_id: None,
            protocol_version: None,
            clients: None,
            message: None,
        }
    }

    fn disconnected(message: impl Into<String>) -> Self {
        Self {
            state: "disconnected".to_string(),
            pid: None,
            instance_id: None,
            protocol_version: None,
            clients: None,
            message: Some(message.into()),
        }
    }

    fn error(message: impl Into<String>) -> Self {
        Self {
            state: "error".to_string(),
            pid: None,
            instance_id: None,
            protocol_version: None,
            clients: None,
            message: Some(message.into()),
        }
    }
}

pub(crate) struct BridgeInner {
    status: Mutex<UiDaemonStatus>,
    client: Mutex<Option<Arc<DaemonClient>>>,
    generations: Mutex<HashMap<String, u64>>,
}

pub struct DaemonBridge {
    inner: Arc<BridgeInner>,
    stop: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl DaemonBridge {
    pub fn start() -> Self {
        let inner = Arc::new(BridgeInner {
            status: Mutex::new(UiDaemonStatus::connecting()),
            client: Mutex::new(None),
            generations: Mutex::new(HashMap::new()),
        });
        let stop = Arc::new(AtomicBool::new(false));
        let thread_inner = Arc::clone(&inner);
        let thread_stop = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("daemon-client".into())
            .spawn(move || supervisor(thread_inner, thread_stop))
            .ok();
        Self {
            inner,
            stop,
            thread: Mutex::new(thread),
        }
    }

    pub fn snapshot(&self) -> UiDaemonStatus {
        self.inner
            .status
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
    }

    pub fn client(&self) -> Result<Arc<DaemonClient>, String> {
        self.inner.client()
    }

    pub fn generation_tracker(&self) -> Arc<BridgeInner> {
        Arc::clone(&self.inner)
    }

    pub fn generation_for(&self, session_id: &str) -> u64 {
        self.inner.generation_for(session_id)
    }

    pub fn forget_generation(&self, session_id: &str) {
        self.inner.forget_generation(session_id);
    }

    /// Deliberate shutdown so M3c can flush the journal on the daemon side.
    pub fn shutdown(&self) {
        self.stop.store(true, Ordering::SeqCst);
        {
            let client = self
                .inner
                .client
                .lock()
                .unwrap_or_else(|err| err.into_inner())
                .take();
            if let Some(client) = client {
                let _ = client.shutdown();
            }
        }
        let handle = self
            .thread
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .take();
        if let Some(handle) = handle {
            let deadline = Instant::now() + JOIN_BUDGET;
            while !handle.is_finished() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            if handle.is_finished() {
                let _ = handle.join();
            }
        }
    }
}

impl BridgeInner {
    fn client(&self) -> Result<Arc<DaemonClient>, String> {
        self.client
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone()
            .ok_or_else(|| "The daemon connection was lost.".to_string())
    }

    pub fn note_generation(&self, session_id: &str, generation: u64) {
        self.generations
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .insert(session_id.to_string(), generation);
    }

    pub fn forget_generation(&self, session_id: &str) {
        self.generations
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .remove(session_id);
    }

    fn generation_for(&self, session_id: &str) -> u64 {
        self.generations
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .get(session_id)
            .copied()
            .unwrap_or(1)
    }
}

#[tauri::command]
pub fn daemon_status(bridge: State<'_, DaemonBridge>) -> UiDaemonStatus {
    bridge.snapshot()
}

fn supervisor(inner: Arc<BridgeInner>, stop: Arc<AtomicBool>) {
    loop {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        set_status(&inner.status, UiDaemonStatus::connecting());
        match connect_once() {
            Ok(client) => {
                let client = Arc::new(client);
                let hello = client.hello().clone();
                {
                    *inner.client.lock().unwrap_or_else(|err| err.into_inner()) =
                        Some(Arc::clone(&client));
                }
                set_status(
                    &inner.status,
                    UiDaemonStatus {
                        state: "connected".to_string(),
                        pid: Some(hello.pid),
                        instance_id: Some(hello.instance_id.clone()),
                        protocol_version: Some(hello.protocol_version),
                        clients: None,
                        message: None,
                    },
                );
                loop {
                    if stop.load(Ordering::SeqCst) {
                        let _ = client.shutdown();
                        *inner.client.lock().unwrap_or_else(|err| err.into_inner()) = None;
                        set_status(
                            &inner.status,
                            UiDaemonStatus::disconnected("daemon stopped"),
                        );
                        return;
                    }
                    match client.status() {
                        Ok(body) => {
                            set_status(
                                &inner.status,
                                UiDaemonStatus {
                                    state: "connected".to_string(),
                                    pid: Some(body.pid),
                                    instance_id: Some(body.instance_id),
                                    protocol_version: Some(body.protocol_version),
                                    clients: Some(body.clients),
                                    message: body.journal_error,
                                },
                            );
                        }
                        Err(error) => {
                            *inner.client.lock().unwrap_or_else(|err| err.into_inner()) = None;
                            set_status(&inner.status, UiDaemonStatus::error(error.to_string()));
                            break;
                        }
                    }
                    if !sleep_interruptible(&stop, PING_PERIOD) {
                        let _ = client.shutdown();
                        *inner.client.lock().unwrap_or_else(|err| err.into_inner()) = None;
                        set_status(
                            &inner.status,
                            UiDaemonStatus::disconnected("daemon stopped"),
                        );
                        return;
                    }
                }
            }
            Err(error) => {
                set_status(&inner.status, UiDaemonStatus::error(error.to_string()));
                if !sleep_interruptible(&stop, PING_PERIOD) {
                    return;
                }
            }
        }
    }
}

fn connect_once() -> Result<DaemonClient, String> {
    let paths = RuntimePaths::from_env().map_err(|error| error.to_string())?;
    let owner = {
        let user = current_user_sid().map_err(|error| error.to_string())?;
        let client = format!("app-{}", std::process::id());
        devboule_protocol::OwnerId::new(user, client)?
    };
    let hello = ClientHello::m3a(owner, "devboule-app");
    let binary = locate_daemon_binary()?;
    connect_or_spawn(&paths, hello, Some(&binary)).map_err(|error| error.to_string())
}

fn locate_daemon_binary() -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os("DEVBOULE_DAEMON") {
        return Ok(PathBuf::from(path));
    }
    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
    let sibling = exe.with_file_name(daemon_file_name());
    if sibling.is_file() {
        return Ok(sibling);
    }
    let fallback = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("target")
        .join(if cfg!(debug_assertions) {
            "debug"
        } else {
            "release"
        })
        .join(daemon_file_name());
    if fallback.is_file() {
        return Ok(fallback);
    }
    eprintln!(
        "daemon binary not found next to {} or at {}",
        exe.display(),
        fallback.display()
    );
    Err(
        "Devboule daemon not found. Set DEVBOULE_DAEMON or install devboule-daemon.exe beside the app."
            .to_string(),
    )
}

fn set_status(status: &Mutex<UiDaemonStatus>, next: UiDaemonStatus) {
    *status.lock().unwrap_or_else(|err| err.into_inner()) = next;
}

fn sleep_interruptible(stop: &AtomicBool, total: Duration) -> bool {
    let deadline = Instant::now() + total;
    while Instant::now() < deadline {
        if stop.load(Ordering::SeqCst) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    !stop.load(Ordering::SeqCst)
}
