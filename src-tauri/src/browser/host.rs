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

use devboule_daemon::{DaemonClient, DaemonError};
use devboule_protocol::{BrowserError, BrowserErrorCode, BrowserExecuteRequest, BrowserOutcome};

use super::commands::{self, COMMANDS};
use super::deadline::Deadline;
use super::registry::BrowserRegistry;
use crate::client::BridgeInner;

/// How long a shutdown waits for the worker to finish the command it is in.
const STOP_BUDGET: std::time::Duration = std::time::Duration::from_secs(2);

/// What the bridge tells the host when its connection changes.
enum Change {
    Connected(Arc<DaemonClient>),
    Disconnected,
    Stop,
}

/// The connection this host is registered on and the id the daemon gave it.
/// Shared between the worker that serves it and the side that stops it, so
/// whichever gets there first gives it back, and only once.
type Registration = Arc<Mutex<Option<(Arc<DaemonClient>, String)>>>;

/// The browser host: one worker thread, told about every (re)connect.
pub struct BrowserHost {
    changes: Arc<Mutex<Option<Sender<Change>>>>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
    registration: Registration,
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
        let registration = Registration::default();
        let held = Arc::clone(&registration);
        let worker = std::thread::Builder::new()
            .name("browser-host".into())
            .spawn(move || serve(rx, app, registry, held))
            .ok();
        BrowserHost {
            changes,
            worker: Mutex::new(worker),
            registration,
        }
    }

    /// Stop being the host: end the worker, and unregister from here if the
    /// worker is still inside a command and has not done it. The worker cannot
    /// see a stop while it is answering, and a host that stays registered after
    /// the app is going away is a host the daemon keeps routing commands to.
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
        unregister_within(&self.registration, STOP_BUDGET);
    }
}

impl Drop for BrowserHost {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The worker: register on a connection, serve its commands, repeat.
fn serve(
    changes: Receiver<Change>,
    app: tauri::AppHandle,
    registry: Arc<BrowserRegistry>,
    registration: Registration,
) {
    while let Ok(change) = changes.recv() {
        match change {
            Change::Stop => {
                unregister(&registration);
                return;
            }
            Change::Disconnected => unregister(&registration),
            Change::Connected(client) => match register(&client) {
                Ok(host_id) => {
                    eprintln!(
                        "devboule: browser host {host_id} registered for {} commands",
                        COMMANDS.len()
                    );
                    *registration.lock().expect("browser host poisoned") =
                        Some((Arc::clone(&client), host_id.clone()));
                    // One deadline per command, taken when the command starts:
                    // what it may wait for is what is left of its own budget.
                    let answered = |request: &BrowserExecuteRequest| {
                        tauri::async_runtime::block_on(answer(
                            &app,
                            &registry,
                            request,
                            Deadline::from_now(),
                        ))
                    };
                    let stop = serve_connection(&client, &answered);
                    eprintln!(
                        "devboule: browser host {host_id} stopped serving: {}",
                        stop.why()
                    );
                    // Always: a host that is still registered with the daemon
                    // and is not reading its queue answers every command
                    // `browser_busy`, which looks exactly like an app that is
                    // working and is not.
                    unregister(&registration);
                }
                Err(error) => eprintln!("devboule: the browser host did not register: {error}"),
            },
        }
    }
    unregister(&registration);
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
fn unregister(registration: &Registration) {
    let held = registration.lock().expect("browser host poisoned").take();
    let Some((client, host_id)) = held else {
        return;
    };
    match client.browser_host_unregister(&host_id) {
        Ok(()) => {}
        // The connection is gone, so the daemon dropped this host with it:
        // there is nothing left to unregister and nothing to report.
        Err(DaemonError::ConnectionLost) => {}
        Err(error) => eprintln!("devboule: the browser host did not unregister: {error}"),
    }
}

/// [`unregister`] from a thread of its own, waited for no longer than `budget`.
/// The call is a round trip, and a daemon that never answers must not hold up
/// the app's exit.
fn unregister_within(registration: &Registration, budget: std::time::Duration) {
    if registration
        .lock()
        .expect("browser host poisoned")
        .is_none()
    {
        return;
    }
    let held = Arc::clone(registration);
    let (done, finished) = channel();
    let spawned = std::thread::Builder::new()
        .name("browser-host-unregister".into())
        .spawn(move || {
            unregister(&held);
            let _ = done.send(());
        });
    if spawned.is_ok() {
        let _ = finished.recv_timeout(budget);
    }
}

/// Why a serving loop ended. Every one of them ends it for good: the daemon is
/// told, so the next connection starts from a clean host rather than from one
/// that is registered and reads nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    /// The connection ended. The client drops its half of the request queue
    /// when it fails, and that is the only thing a blocking read can return.
    ConnectionEnded,
    /// The daemon would not take this connection's answer, so the connection is
    /// gone and every answer after it would be refused too.
    AnswerRefused,
    /// This connection's queue had already been handed out, so there is nothing
    /// left to read.
    QueueTaken,
}

impl Stop {
    fn why(self) -> &'static str {
        match self {
            Stop::ConnectionEnded => "the connection ended",
            Stop::AnswerRefused => "the daemon would not take an answer",
            Stop::QueueTaken => "this connection's request queue was already taken",
        }
    }
}

/// Answer every command this connection sends, until the connection ends.
///
/// The read is a BLOCKING one and there is no idle window. That is the whole
/// fix for the failure this app shipped: a timed wait ended the loop when the
/// agent had not called for thirty seconds, the request receiver went with it,
/// and every later command was refused at once with `browser_busy` — a host
/// that still looks registered to the daemon and no longer reads anything. The
/// queue is ended by the connection and by nothing else, so this ends with it.
fn serve_connection(
    client: &Arc<DaemonClient>,
    answer: &(dyn Fn(&BrowserExecuteRequest) -> BrowserOutcome + Send + Sync),
) -> Stop {
    // Handed out once per connection: a second call finds nothing, and a
    // request queued in the meantime has nowhere else to wait.
    let Some(requests) = client.take_browser_requests() else {
        return Stop::QueueTaken;
    };
    loop {
        let Ok(request) = requests.recv() else {
            return Stop::ConnectionEnded;
        };
        let outcome = answer(&request);
        // One line per command: the name and how it ended. Never the address,
        // the arguments or the result — the daemon logs the command names too,
        // and neither log is the place for what a page said.
        match &outcome {
            BrowserOutcome::Ok { .. } => {
                eprintln!("devboule: browser {} answered ok", request.command)
            }
            BrowserOutcome::Err(error) => eprintln!(
                "devboule: browser {} answered {}",
                request.command,
                commands::code_name(error.code)
            ),
        }
        if let Err(error) = client.browser_respond(&request.request_id, &request.host_id, outcome) {
            eprintln!("devboule: a browser command could not be answered: {error}");
            return Stop::AnswerRefused;
        }
    }
}

/// One command, and exactly one answer: the result, or the failure with the
/// code the contract names it by.
async fn answer(
    app: &tauri::AppHandle,
    registry: &BrowserRegistry,
    request: &BrowserExecuteRequest,
    deadline: Deadline,
) -> BrowserOutcome {
    let result: Result<Value, BrowserError> =
        commands::dispatch(app, registry, request, deadline).await;
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

#[cfg(test)]
#[path = "host_tests.rs"]
mod tests;

#[cfg(all(test, windows))]
#[path = "host_wire_tests.rs"]
mod wire_tests;
