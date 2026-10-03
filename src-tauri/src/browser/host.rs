//! This app as the daemon's browser host: register the commands it runs, take
//! the requests the daemon pushes, answer each one exactly once.
//!
//! The work happens on this module's own thread, never on the connection's
//! reader. The reader is the only thing that delivers the daemon's replies,
//! and an answer is a reply: a host that ran a command there and then waited
//! for its own answer to come back would be waiting on itself.
//!
//! One registration per connection. The daemon keys a host by its registration
//! id and strands the old host's tabs as gone when the same connection
//! registers again, so this re-registers when the connection changes and never
//! twice for the same one.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

use serde_json::Value;

use devboule_daemon::DaemonClient;
use devboule_protocol::{BrowserError, BrowserErrorCode, BrowserExecuteRequest, BrowserOutcome};

use super::commands::{self, COMMANDS};
use super::registry::BrowserRegistry;
use crate::client::BridgeInner;

/// How long the worker waits for one command before looking at its connection
/// again. A command that takes longer than this is not interrupted; the check
/// is only for a host that has to notice a dead connection.
const IDLE: std::time::Duration = std::time::Duration::from_secs(30);

/// How long a shutdown waits for the worker to finish the command it is in.
const STOP_BUDGET: std::time::Duration = std::time::Duration::from_secs(2);

/// What the bridge tells the host when its connection changes.
enum Change {
    Connected(Arc<DaemonClient>),
    Disconnected,
    Stop,
}

/// The browser host: one worker thread, told about every (re)connect.
pub struct BrowserHost {
    changes: Arc<Mutex<Option<Sender<Change>>>>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl BrowserHost {
    /// Start the host and tell the bridge to report its connections to it.
    ///
    /// `install` is what the bridge calls on every connect and disconnect; it
    /// is handed in rather than looked up so this module owns the ordering and
    /// the bridge owns only the event.
    pub fn start(
        bridge: Arc<BridgeInner>,
        app: tauri::AppHandle,
        registry: Arc<BrowserRegistry>,
    ) -> Self {
        let (tx, rx) = channel();
        let changes: Arc<Mutex<Option<Sender<Change>>>> = Arc::new(Mutex::new(Some(tx)));
        let for_hook = Arc::clone(&changes);
        bridge.on_client_change(Arc::new(move |client| {
            let change = match client {
                Some(client) => Change::Connected(Arc::clone(client)),
                None => Change::Disconnected,
            };
            if let Some(changes) = for_hook.lock().expect("browser host poisoned").as_ref() {
                let _ = changes.send(change);
            }
        }));
        let worker = std::thread::Builder::new()
            .name("browser-host".into())
            .spawn(move || serve(rx, app, registry))
            .ok();
        BrowserHost {
            changes,
            worker: Mutex::new(worker),
        }
    }

    /// Stop being the host: unregister, then end the worker.
    pub fn stop(&self) {
        if let Some(changes) = self.changes.lock().expect("browser host poisoned").as_ref() {
            let _ = changes.send(Change::Stop);
        }
        let worker = self.worker.lock().expect("browser host poisoned").take();
        if let Some(worker) = worker {
            let deadline = std::time::Instant::now() + STOP_BUDGET;
            while !worker.is_finished() && std::time::Instant::now() < deadline {
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            if worker.is_finished() {
                let _ = worker.join();
            }
        }
    }
}

impl Drop for BrowserHost {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The worker: register on a connection, serve its commands, repeat.
fn serve(changes: Receiver<Change>, app: tauri::AppHandle, registry: Arc<BrowserRegistry>) {
    let mut registration: Option<(Arc<DaemonClient>, String)> = None;
    while let Ok(change) = changes.recv() {
        match change {
            Change::Stop => {
                unregister(&mut registration);
                return;
            }
            Change::Disconnected => unregister(&mut registration),
            Change::Connected(client) => match register(&client) {
                Ok(host_id) => {
                    registration = Some((Arc::clone(&client), host_id));
                    serve_connection(&client, &app, &registry);
                }
                Err(error) => eprintln!("devboule: the browser host did not register: {error}"),
            },
        }
    }
    unregister(&mut registration);
}

fn register(client: &Arc<DaemonClient>) -> Result<String, String> {
    let commands: Vec<String> = COMMANDS.iter().map(|name| (*name).to_owned()).collect();
    client
        .browser_host_register(&commands)
        .map_err(|error| error.to_string())
}

/// Give the host id back before its connection goes: a call still waiting on
/// this host then fails as `browser_no_host` rather than waiting out its
/// deadline.
fn unregister(registration: &mut Option<(Arc<DaemonClient>, String)>) {
    if let Some((client, host_id)) = registration.take() {
        if let Err(error) = client.browser_host_unregister(&host_id) {
            eprintln!("devboule: the browser host did not unregister: {error}");
        }
    }
}

/// Answer every command this connection sends, until the connection ends.
fn serve_connection(
    client: &Arc<DaemonClient>,
    app: &tauri::AppHandle,
    registry: &BrowserRegistry,
) {
    // Handed out once per connection: a second call would find nothing, and a
    // request queued in the meantime has nowhere else to wait.
    let Some(requests) = client.take_browser_requests() else {
        return;
    };
    while let Ok(request) = requests.recv_timeout(IDLE) {
        let outcome = tauri::async_runtime::block_on(answer(app, registry, &request));
        if let Err(error) = client.browser_respond(&request.request_id, &request.host_id, outcome) {
            // The connection is gone: its reader ended the queue, so every
            // answer after this one would be refused too.
            eprintln!("devboule: a browser command could not be answered: {error}");
            return;
        }
    }
}

/// One command, and exactly one answer: the result, or the failure with the
/// code the contract names it by.
pub async fn answer(
    app: &tauri::AppHandle,
    registry: &BrowserRegistry,
    request: &BrowserExecuteRequest,
) -> BrowserOutcome {
    let result: Result<Value, BrowserError> = commands::dispatch(app, registry, request).await;
    match result {
        Ok(result) => {
            // The host checks its own answer: a result over the frame budget
            // costs this connection, not the caller's step.
            if result.to_string().len() > devboule_protocol::MAX_BROWSER_PAYLOAD_BYTES {
                return BrowserOutcome::Err(BrowserError::daemon(
                    BrowserErrorCode::ResultTooLarge,
                    "That answer is larger than one browser frame can carry.",
                ));
            }
            BrowserOutcome::Ok { result }
        }
        Err(error) => BrowserOutcome::Err(error),
    }
}
