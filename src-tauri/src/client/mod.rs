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
    connect_or_spawn, current_user_sid, daemon_file_name, DaemonClient, DaemonError, RuntimePaths,
    SessionStateHandler,
};
use devboule_protocol::{ClientHello, DaemonStatusBody, ErrorCode};
use serde::Serialize;
use tauri::State;

use crate::backend::error::CommandError;

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
    pub capabilities: Vec<String>,
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
            capabilities: Vec::new(),
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
            capabilities: Vec::new(),
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
            capabilities: Vec::new(),
            message: Some(message.into()),
        }
    }
}

trait SessionWatchClient {
    fn sessions_watch(&self, handler: SessionStateHandler) -> Result<(), DaemonError>;
    fn sessions_unwatch(&self) -> Result<(), DaemonError>;
}

impl SessionWatchClient for DaemonClient {
    fn sessions_watch(&self, handler: SessionStateHandler) -> Result<(), DaemonError> {
        DaemonClient::sessions_watch(self, handler)
    }

    fn sessions_unwatch(&self) -> Result<(), DaemonError> {
        DaemonClient::sessions_unwatch(self)
    }
}

/// Desired roster subscription owned by the bridge rather than one daemon
/// connection. Rebinding it after a client swap keeps the frontend's single
/// watch alive across daemon recovery; there is no journal state to restore.
#[derive(Default)]
struct RosterSubscription {
    handler: Mutex<Option<SessionStateHandler>>,
}

impl RosterSubscription {
    fn watch<C: SessionWatchClient>(
        &self,
        client: Option<&C>,
        handler: SessionStateHandler,
    ) -> Result<(), DaemonError> {
        *self.handler.lock().unwrap_or_else(|err| err.into_inner()) = Some(Arc::clone(&handler));
        if let Some(client) = client {
            client.sessions_watch(handler)?;
        }
        Ok(())
    }

    fn unwatch<C: SessionWatchClient>(&self, client: Option<&C>) -> Result<(), DaemonError> {
        self.handler
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .take();
        if let Some(client) = client {
            client.sessions_unwatch()?;
        }
        Ok(())
    }

    fn rebind<C: SessionWatchClient>(&self, client: &C) -> Result<(), DaemonError> {
        let handler = self
            .handler
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone();
        if let Some(handler) = handler {
            client.sessions_watch(handler)?;
        }
        Ok(())
    }
}

pub(crate) struct BridgeInner {
    status: Mutex<UiDaemonStatus>,
    client: Mutex<Option<Arc<DaemonClient>>>,
    client_lifecycle: Mutex<()>,
    roster_subscription: RosterSubscription,
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
            client_lifecycle: Mutex::new(()),
            roster_subscription: RosterSubscription::default(),
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

    pub fn sessions_watch(&self, handler: SessionStateHandler) -> Result<(), DaemonError> {
        self.inner.sessions_watch(handler)
    }

    pub fn sessions_unwatch(&self) -> Result<(), DaemonError> {
        self.inner.sessions_unwatch()
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
        if let Some(client) = self.inner.take_client() {
            let _ = client.shutdown();
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

    fn sessions_watch(&self, handler: SessionStateHandler) -> Result<(), DaemonError> {
        // Serialize desired-subscription changes with client replacement. The
        // lifecycle guard is always acquired before the client mutex, and no
        // path acquires them in the reverse order.
        let _lifecycle = self
            .client_lifecycle
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let client = self
            .client
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone();
        self.roster_subscription.watch(client.as_deref(), handler)
    }

    fn sessions_unwatch(&self) -> Result<(), DaemonError> {
        let _lifecycle = self
            .client_lifecycle
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let client = self
            .client
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .clone();
        self.roster_subscription.unwatch(client.as_deref())
    }

    fn replace_client(&self, client: Arc<DaemonClient>) -> Result<(), DaemonError> {
        let _lifecycle = self
            .client_lifecycle
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        *self.client.lock().unwrap_or_else(|err| err.into_inner()) = Some(Arc::clone(&client));
        if let Err(error) = self.roster_subscription.rebind(client.as_ref()) {
            let mut current = self.client.lock().unwrap_or_else(|err| err.into_inner());
            if current
                .as_ref()
                .is_some_and(|active| Arc::ptr_eq(active, &client))
            {
                *current = None;
            }
            return Err(error);
        }
        Ok(())
    }

    fn clear_client(&self, expected: &Arc<DaemonClient>) {
        let _lifecycle = self
            .client_lifecycle
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        let mut current = self.client.lock().unwrap_or_else(|err| err.into_inner());
        if current
            .as_ref()
            .is_some_and(|active| Arc::ptr_eq(active, expected))
        {
            *current = None;
        }
    }

    fn take_client(&self) -> Option<Arc<DaemonClient>> {
        let _lifecycle = self
            .client_lifecycle
            .lock()
            .unwrap_or_else(|err| err.into_inner());
        self.client
            .lock()
            .unwrap_or_else(|err| err.into_inner())
            .take()
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

#[tauri::command]
pub fn daemon_restart(bridge: State<'_, DaemonBridge>) -> Result<(), CommandError> {
    let client = bridge
        .client()
        .map_err(|message| CommandError::new(ErrorCode::Io, message))?;
    Ok(client.restart_daemon()?)
}

const STATUS_FAILURE_THRESHOLD: u32 = 3;

#[derive(Clone, Debug, PartialEq, Eq)]
struct StatusSignal {
    state: &'static str,
    message: Option<String>,
}

#[derive(Default)]
struct StatusFailureTracker {
    consecutive_failures: u32,
    silent_since: Option<Instant>,
}

impl StatusFailureTracker {
    fn record_failure(
        &mut self,
        attempt_started: Instant,
        observed_at: Instant,
        error: &str,
    ) -> StatusSignal {
        let silent_since = *self.silent_since.get_or_insert(attempt_started);
        self.consecutive_failures = self.consecutive_failures.saturating_add(1);
        if self.consecutive_failures >= STATUS_FAILURE_THRESHOLD {
            return self.unresponsive_status(observed_at, silent_since);
        }
        StatusSignal {
            state: "error",
            message: Some(error.to_string()),
        }
    }

    fn record_success(&mut self) {
        self.consecutive_failures = 0;
        self.silent_since = None;
    }

    fn connection_status(&self, now: Instant) -> Option<StatusSignal> {
        (self.consecutive_failures >= STATUS_FAILURE_THRESHOLD)
            .then(|| self.unresponsive_status(now, self.silent_since.unwrap_or(now)))
    }

    fn unresponsive_status(&self, now: Instant, silent_since: Instant) -> StatusSignal {
        let silent_seconds = now.saturating_duration_since(silent_since).as_secs();
        StatusSignal {
            state: "unresponsive",
            message: Some(format!(
                "The daemon has not answered status checks for at least {silent_seconds} seconds ({count} consecutive failures).",
                count = self.consecutive_failures,
            )),
        }
    }
}

trait StatusSource {
    fn status(&self) -> Result<DaemonStatusBody, DaemonError>;
}

impl StatusSource for DaemonClient {
    fn status(&self) -> Result<DaemonStatusBody, DaemonError> {
        DaemonClient::status(self)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StatusLoopExit {
    ConnectionLost,
    Stopped,
}

enum StatusUpdate {
    Connected(DaemonStatusBody),
    Failure(StatusSignal),
}

fn status_error_is_connection_lost(error: &DaemonError) -> bool {
    match error {
        DaemonError::TimedOut(_) => false,
        DaemonError::Io(error) => error.kind() != std::io::ErrorKind::TimedOut,
        DaemonError::ConnectionLost => true,
        DaemonError::Protocol(_)
        | DaemonError::AlreadyRunning
        | DaemonError::UnsupportedPlatform
        | DaemonError::Handshake(_) => false,
    }
}

fn run_status_loop<S, Sleep, Publish>(
    source: &S,
    status_tracker: &mut StatusFailureTracker,
    stop: &AtomicBool,
    mut sleep: Sleep,
    mut publish: Publish,
) -> StatusLoopExit
where
    S: StatusSource,
    Sleep: FnMut() -> bool,
    Publish: FnMut(StatusUpdate),
{
    loop {
        if stop.load(Ordering::SeqCst) {
            return StatusLoopExit::Stopped;
        }
        let status_attempt_started = Instant::now();
        match source.status() {
            Ok(body) => {
                status_tracker.record_success();
                publish(StatusUpdate::Connected(body));
            }
            Err(error) => {
                let signal = status_tracker.record_failure(
                    status_attempt_started,
                    Instant::now(),
                    &error.to_string(),
                );
                publish(StatusUpdate::Failure(signal));
                if status_error_is_connection_lost(&error) {
                    return StatusLoopExit::ConnectionLost;
                }
            }
        }
        if !sleep() {
            return StatusLoopExit::Stopped;
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SupervisorLoopExit {
    Stopped,
}

fn run_supervisor_loop<C, Connect, Connected, Sleep>(
    stop: &AtomicBool,
    mut connect: Connect,
    mut connected: Connected,
    mut sleep: Sleep,
) -> SupervisorLoopExit
where
    Connect: FnMut() -> Result<C, String>,
    Connected: FnMut(C) -> StatusLoopExit,
    Sleep: FnMut() -> bool,
{
    loop {
        if stop.load(Ordering::SeqCst) {
            return SupervisorLoopExit::Stopped;
        }
        match connect() {
            Ok(connection) => match connected(connection) {
                // A lost connection is the normal handoff back to the
                // reconnect path. Only a deliberate stop terminates the
                // supervisor itself.
                StatusLoopExit::ConnectionLost => continue,
                StatusLoopExit::Stopped => return SupervisorLoopExit::Stopped,
            },
            Err(_) => {
                if !sleep() {
                    return SupervisorLoopExit::Stopped;
                }
            }
        }
    }
}

fn supervisor(inner: Arc<BridgeInner>, stop: Arc<AtomicBool>) {
    // This tracker deliberately lives outside the connection loop. A
    // successful handshake is not a successful status check, so reconnecting
    // to the same hung daemon must not erase the evidence of silence.
    let status_tracker = std::cell::RefCell::new(StatusFailureTracker::default());
    let _ = run_supervisor_loop(
        &stop,
        || {
            set_status(&inner.status, UiDaemonStatus::connecting());
            match connect_once() {
                Ok(client) => {
                    let client = Arc::new(client);
                    let hello = client.hello().clone();
                    inner
                        .replace_client(Arc::clone(&client))
                        .map_err(|error| error.to_string())?;
                    let connection_signal =
                        status_tracker.borrow().connection_status(Instant::now());
                    set_status(
                        &inner.status,
                        UiDaemonStatus {
                            state: connection_signal.as_ref().map_or_else(
                                || "connected".to_string(),
                                |signal| signal.state.into(),
                            ),
                            pid: Some(hello.pid),
                            instance_id: Some(hello.instance_id.clone()),
                            protocol_version: Some(hello.protocol_version),
                            clients: None,
                            capabilities: hello
                                .capabilities
                                .iter()
                                .map(|capability| capability.as_str().to_string())
                                .collect(),
                            message: connection_signal.and_then(|signal| signal.message),
                        },
                    );
                    Ok((client, hello))
                }
                Err(error) => {
                    if let Some(signal) = status_tracker.borrow().connection_status(Instant::now())
                    {
                        set_status(
                            &inner.status,
                            UiDaemonStatus {
                                state: signal.state.to_string(),
                                pid: None,
                                instance_id: None,
                                protocol_version: None,
                                clients: None,
                                capabilities: Vec::new(),
                                message: signal.message,
                            },
                        );
                    } else {
                        set_status(&inner.status, UiDaemonStatus::error(error.clone()));
                    }
                    Err(error)
                }
            }
        },
        |(client, hello)| match run_status_loop(
            client.as_ref(),
            &mut status_tracker.borrow_mut(),
            &stop,
            || sleep_interruptible(&stop, PING_PERIOD),
            |update| match update {
                StatusUpdate::Connected(body) => set_status(
                    &inner.status,
                    UiDaemonStatus {
                        state: "connected".to_string(),
                        pid: Some(body.pid),
                        instance_id: Some(body.instance_id),
                        protocol_version: Some(body.protocol_version),
                        clients: Some(body.clients),
                        capabilities: hello
                            .capabilities
                            .iter()
                            .map(|capability| capability.as_str().to_string())
                            .collect(),
                        message: body.journal_error,
                    },
                ),
                StatusUpdate::Failure(signal) => set_status(
                    &inner.status,
                    UiDaemonStatus {
                        state: signal.state.to_string(),
                        pid: None,
                        instance_id: None,
                        protocol_version: None,
                        clients: None,
                        capabilities: Vec::new(),
                        message: signal.message,
                    },
                ),
            },
        ) {
            StatusLoopExit::ConnectionLost => {
                inner.clear_client(&client);
                StatusLoopExit::ConnectionLost
            }
            StatusLoopExit::Stopped => {
                let _ = client.shutdown();
                inner.clear_client(&client);
                set_status(
                    &inner.status,
                    UiDaemonStatus::disconnected("daemon stopped"),
                );
                StatusLoopExit::Stopped
            }
        },
        || sleep_interruptible(&stop, PING_PERIOD),
    );
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

#[cfg(test)]
mod tests {
    use super::*;
    use devboule_daemon::SessionStateHandler;
    use devboule_protocol::DaemonStatusBody;
    use devboule_protocol::{SessionState, SessionStateSnapshot};
    use std::sync::atomic::AtomicUsize;

    #[derive(Default)]
    struct FakeRosterClient {
        handler: Mutex<Option<SessionStateHandler>>,
    }

    impl FakeRosterClient {
        fn emit(&self, snapshot: SessionStateSnapshot) {
            if let Some(handler) = self
                .handler
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .as_ref()
            {
                handler(vec![snapshot]);
            }
        }
    }

    impl SessionWatchClient for FakeRosterClient {
        fn sessions_watch(&self, handler: SessionStateHandler) -> Result<(), DaemonError> {
            *self
                .handler
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = Some(handler);
            Ok(())
        }

        fn sessions_unwatch(&self) -> Result<(), DaemonError> {
            self.handler
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .take();
            Ok(())
        }
    }

    fn roster_snapshot(id: &str) -> SessionStateSnapshot {
        SessionStateSnapshot {
            id: id.to_string(),
            title: "new daemon".to_string(),
            state: SessionState::Live { generation: 1 },
            elapsed_ms: Some(1),
            attention: None,
        }
    }

    #[test]
    fn roster_update_reaches_a_subscription_after_client_replacement() {
        let subscription = RosterSubscription::default();
        let received = Arc::new(Mutex::new(Vec::new()));
        let received_by_handler = Arc::clone(&received);
        let handler: SessionStateHandler = Arc::new(move |snapshots| {
            received_by_handler
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .extend(snapshots);
        });
        let old_client = FakeRosterClient::default();
        let new_client = FakeRosterClient::default();

        subscription
            .watch(Some(&old_client), handler)
            .expect("watch old client");
        subscription
            .rebind(&new_client)
            .expect("rebind replacement client");
        new_client.emit(roster_snapshot("new-daemon-session"));

        assert_eq!(
            received
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .iter()
                .map(|snapshot| snapshot.id.as_str())
                .collect::<Vec<_>>(),
            vec!["new-daemon-session"]
        );
    }

    #[test]
    fn roster_subscription_registered_while_disconnected_binds_later() {
        let subscription = RosterSubscription::default();
        let received = Arc::new(Mutex::new(Vec::new()));
        let received_by_handler = Arc::clone(&received);
        let handler: SessionStateHandler = Arc::new(move |snapshots| {
            received_by_handler
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .extend(snapshots);
        });
        let new_client = FakeRosterClient::default();

        subscription
            .watch::<FakeRosterClient>(None, handler)
            .expect("save watch");
        subscription.rebind(&new_client).expect("bind new client");
        new_client.emit(roster_snapshot("connected-later"));

        assert_eq!(
            received
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .iter()
                .map(|snapshot| snapshot.id.as_str())
                .collect::<Vec<_>>(),
            vec!["connected-later"]
        );
    }

    #[test]
    fn roster_unwatch_stays_unsubscribed_across_client_replacement() {
        let subscription = RosterSubscription::default();
        let received = Arc::new(Mutex::new(Vec::new()));
        let received_by_handler = Arc::clone(&received);
        let handler: SessionStateHandler = Arc::new(move |snapshots| {
            received_by_handler
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .extend(snapshots);
        });
        let old_client = FakeRosterClient::default();
        let new_client = FakeRosterClient::default();

        subscription
            .watch(Some(&old_client), handler)
            .expect("watch old client");
        subscription
            .unwatch(Some(&old_client))
            .expect("unwatch old client");
        subscription
            .rebind(&new_client)
            .expect("rebind replacement client");
        new_client.emit(roster_snapshot("must-not-arrive"));

        assert!(received
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_empty());
    }

    struct TimeoutStatusSource {
        calls: AtomicUsize,
    }

    impl StatusSource for TimeoutStatusSource {
        fn status(&self) -> Result<DaemonStatusBody, devboule_daemon::DaemonError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(devboule_daemon::DaemonError::timed_out("fake status"))
        }
    }

    struct LostStatusSource {
        calls: AtomicUsize,
    }

    impl StatusSource for LostStatusSource {
        fn status(&self) -> Result<DaemonStatusBody, devboule_daemon::DaemonError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            Err(devboule_daemon::DaemonError::ConnectionLost)
        }
    }

    #[test]
    fn supervisor_reconnects_after_a_connected_loop_reports_connection_loss() {
        let stop = AtomicBool::new(false);
        let mut connect_attempts = 0;
        let mut connected_runs = 0;
        let outcome = run_supervisor_loop(
            &stop,
            || {
                connect_attempts += 1;
                Ok(connect_attempts)
            },
            |_| {
                connected_runs += 1;
                if connected_runs == 1 {
                    StatusLoopExit::ConnectionLost
                } else {
                    StatusLoopExit::Stopped
                }
            },
            || true,
        );

        assert_eq!(outcome, SupervisorLoopExit::Stopped);
        assert_eq!(connect_attempts, 2);
        assert_eq!(connected_runs, 2);
    }

    #[test]
    fn connected_timeout_source_reaches_unresponsive_without_reconnect() {
        let source = TimeoutStatusSource {
            calls: AtomicUsize::new(0),
        };
        let stop = AtomicBool::new(false);
        let mut tracker = StatusFailureTracker::default();
        let mut updates = Vec::new();
        let mut sleeps = 0;
        let outcome = run_status_loop(
            &source,
            &mut tracker,
            &stop,
            || {
                sleeps += 1;
                sleeps < 3
            },
            |update| updates.push(update),
        );

        assert_eq!(outcome, StatusLoopExit::Stopped);
        assert_eq!(source.calls.load(Ordering::SeqCst), 3);
        assert!(updates.iter().any(|update| matches!(
            update,
            StatusUpdate::Failure(signal) if signal.state == "unresponsive"
        )));
    }

    #[test]
    fn typed_connection_loss_exits_the_status_loop_for_reconnect() {
        let source = LostStatusSource {
            calls: AtomicUsize::new(0),
        };
        let stop = AtomicBool::new(false);
        let mut tracker = StatusFailureTracker::default();
        let mut updates = Vec::new();
        let outcome = run_status_loop(
            &source,
            &mut tracker,
            &stop,
            || true,
            |update| updates.push(update),
        );

        assert_eq!(outcome, StatusLoopExit::ConnectionLost);
        assert_eq!(source.calls.load(Ordering::SeqCst), 1);
        assert!(matches!(updates.as_slice(), [StatusUpdate::Failure(_)]));
    }

    #[test]
    fn two_status_failures_do_not_raise_unresponsive_but_three_do() {
        let first_attempt = Instant::now();
        let mut tracker = StatusFailureTracker::default();
        let first = tracker.record_failure(
            first_attempt,
            first_attempt + Duration::from_secs(30),
            "timed out",
        );
        assert_eq!(first.state, "error");
        let second = tracker.record_failure(
            first_attempt + Duration::from_secs(30),
            first_attempt + Duration::from_secs(60),
            "timed out",
        );
        assert_eq!(second.state, "error");
        let third = tracker.record_failure(
            first_attempt + Duration::from_secs(60),
            first_attempt + Duration::from_secs(90),
            "timed out",
        );
        assert_eq!(third.state, "unresponsive");
        assert!(third
            .message
            .as_deref()
            .is_some_and(|message| message.contains("90 seconds")));
    }

    #[test]
    fn a_success_resets_the_failure_count() {
        let first_attempt = Instant::now();
        let mut tracker = StatusFailureTracker::default();
        tracker.record_failure(
            first_attempt,
            first_attempt + Duration::from_secs(30),
            "timed out",
        );
        tracker.record_success();
        let first_after_success = tracker.record_failure(
            first_attempt + Duration::from_secs(60),
            first_attempt + Duration::from_secs(90),
            "timed out",
        );
        let second_after_success = tracker.record_failure(
            first_attempt + Duration::from_secs(90),
            first_attempt + Duration::from_secs(120),
            "timed out",
        );
        assert_eq!(first_after_success.state, "error");
        assert_eq!(second_after_success.state, "error");
    }

    #[test]
    fn reconnect_does_not_reset_failures_when_status_keeps_failing() {
        let first_attempt = Instant::now();
        let mut tracker = StatusFailureTracker::default();
        tracker.record_failure(
            first_attempt,
            first_attempt + Duration::from_secs(30),
            "timed out",
        );
        assert!(tracker
            .connection_status(first_attempt + Duration::from_secs(30))
            .is_none());
        tracker.record_failure(
            first_attempt + Duration::from_secs(30),
            first_attempt + Duration::from_secs(60),
            "timed out",
        );
        let status = tracker.record_failure(
            first_attempt + Duration::from_secs(60),
            first_attempt + Duration::from_secs(90),
            "timed out",
        );
        assert_eq!(status.state, "unresponsive");
    }

    #[test]
    fn daemon_restart_has_the_frozen_tauri_signature() {
        let _: fn(State<'_, DaemonBridge>) -> Result<(), crate::backend::error::CommandError> =
            daemon_restart;
    }
}
