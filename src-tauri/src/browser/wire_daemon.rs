//! A daemon on the daemon's own pipe, for the host loop's tests: the handshake,
//! the frames and the connection lifetime, with nothing stubbed between a
//! caller and the app. The broker that decides which host a call goes to is
//! 4c's lane and is not here; what these drive is the app's side of a request
//! queue.

#![cfg(windows)]

//! The host loop over a real connection: register, answer a command that
//! arrives after an idle, and do it all again on the next connection.
//!
//! The pipe, the framing, the handshake, the frames and the client are the
//! daemon crate's own, so this crosses the seam that broke — the app's side of
//! a request queue — with nothing stubbed in it. The one thing not here is the
//! daemon's broker, which decides which host a call goes to and is 4c's lane.
//!
//! It fails the way the live agent's calls did if the loop ever stops reading:
//! the answer never arrives, and the daemon's own reason for that refusal
//! (`browser_busy`, from a queue with no reader) is what the agent saw.

#![cfg(windows)]

use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use devboule_daemon::{connect, DaemonClient, Framed, RuntimePaths};
use devboule_protocol::{
    caps, BrowserCaller, BrowserExecuteRequest, BrowserOutcome, Capability, ClientHello,
    ClientMessage, DaemonHello, DaemonMessage, OwnerId, PROTOCOL_MIN_VERSION, PROTOCOL_VERSION,
};
use serde_json::{json, Value};
use windows::Win32::Foundation::{ERROR_PIPE_CONNECTED, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::Storage::FileSystem::{FILE_FLAG_OVERLAPPED, PIPE_ACCESS_DUPLEX};
use windows::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_WAIT,
};

use crate::browser::commands::tabs;
use crate::browser::registry::BrowserRegistry;

const HANDSHAKE: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(50);
pub const ANSWER: Duration = Duration::from_secs(5);
/// How long the daemon says nothing before it sends a command. The loop that
/// broke gave up after thirty; anything above a poll interval proves the read
/// is not timed, and the wait is short enough for a test suite.
pub const IDLE: Duration = Duration::from_millis(1_200);

/// The host id the wire daemon hands out, and the one every request carries.
pub const HOST_ID: &str = "wire.1";

/// What the test tells a connection to do. Every write goes through the
/// connection's own thread, so it is the only holder of the pipe: closing a
/// connection is that thread letting go of it, which is what closes it.
enum ToServer {
    // Boxed because a `DaemonMessage` is three hundred bytes and `Close` is
    // none, and a channel moves one of these per test step.
    Push(Box<DaemonMessage>),
    Close,
}

pub struct WireDaemon {
    paths: RuntimePaths,
    connection: Arc<Mutex<Vec<Sender<ToServer>>>>,
    frames: Receiver<ClientMessage>,
}

impl WireDaemon {
    pub fn start(label: &str) -> Self {
        let dir =
            devboule_daemon::test_dirs::test_temp_dir(&format!("devboule-browser-host-{label}"));
        let paths = RuntimePaths::from_dir(dir);
        paths.ensure_dir().expect("runtime dir");
        let connection: Arc<Mutex<Vec<Sender<ToServer>>>> = Arc::new(Mutex::new(Vec::new()));
        let (frames_tx, frames) = channel();
        let accepting = Arc::clone(&connection);
        let name = paths.pipe_name.clone();
        // The first instance is created HERE, before the caller gets a client:
        // a pipe that does not exist yet is a connect that fails with
        // ERROR_FILE_NOT_FOUND, which is not one of the errors it retries.
        let first = bound_pipe(&name).expect("the pipe is bound");
        std::thread::Builder::new()
            .name("wire-accept".into())
            .spawn(move || accept(&name, first, accepting, frames_tx))
            .expect("accept thread");
        Self {
            paths,
            connection,
            frames,
        }
    }

    /// A connection that negotiated `browser.host`, as the app's does.
    pub fn client(&self) -> DaemonClient {
        connect(&self.paths, hello()).expect("connect")
    }

    /// The daemon pushing a command to the host it registered.
    pub fn push(&self, request: BrowserExecuteRequest) {
        self.send(ToServer::Push(Box::new(
            DaemonMessage::BrowserExecuteRequest(request),
        )));
    }

    /// The connection ending, from the daemon's side.
    pub fn close_current(&self) {
        self.send(ToServer::Close);
    }

    fn send(&self, command: ToServer) {
        let connections = self.connection.lock().expect("wire connections");
        let last = connections.last().cloned().expect("a connection is open");
        // The connection thread holds the pipe; if it is gone the connection is
        // already over and there is nothing to push to.
        let _ = last.send(command);
    }

    /// The next frame the client sent, or None if it says nothing in time.
    fn next_frame(&self) -> Option<ClientMessage> {
        self.frames.recv_timeout(ANSWER).ok()
    }
}

fn hello() -> ClientHello {
    let owner = OwnerId::new(
        devboule_daemon::current_user_sid().expect("sid"),
        format!("browser-host-wire-{}", std::process::id()),
    )
    .expect("owner");
    let mut hello = ClientHello::m3a(owner, "devboule-test");
    hello.capabilities.push(Capability::new(caps::BROWSER_HOST));
    hello
}

fn daemon_hello() -> DaemonHello {
    DaemonHello {
        protocol_version: PROTOCOL_VERSION,
        min_protocol_version: PROTOCOL_MIN_VERSION,
        daemon_version: "browser-host-wire".to_owned(),
        instance_id: "wire".to_owned(),
        pid: std::process::id() as u32,
        capabilities: vec![Capability::new(caps::BROWSER_HOST)],
        workspace_host: None,
    }
}

/// One connection at a time, handled to its end: the handshake, then every
/// frame the client sends, answered as it goes.
///
/// The next instance is bound before this one is handled, so a client that
/// reconnects the moment a connection closes finds a pipe waiting rather than a
/// name that is not there yet.
fn accept(
    name: &str,
    mut next: std::fs::File,
    connections: Arc<Mutex<Vec<Sender<ToServer>>>>,
    frames: Sender<ClientMessage>,
) {
    loop {
        let file = wait_for_client(next);
        next = match bound_pipe(name) {
            Some(bound) => bound,
            None => return,
        };
        let Some(file) = file else {
            return;
        };
        let (commands, queued) = channel();
        connections.lock().expect("wire connections").push(commands);
        let to_test = frames.clone();
        let Ok(thread) = std::thread::Builder::new()
            .name("wire-connection".into())
            .spawn(move || connection(Framed::new(file), queued, to_test))
        else {
            return;
        };
        let _ = thread.join();
    }
}

/// Bind one instance of the daemon's pipe. It exists from here on, whether or
/// not a client has arrived.
fn bound_pipe(name: &str) -> Option<std::fs::File> {
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let handle = unsafe {
        CreateNamedPipeW(
            windows::core::PCWSTR(wide.as_ptr()),
            // Overlapped, because the daemon's framing reads this pipe that
            // way: a synchronous handle and an overlapped read do not mix.
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
            // The daemon's own pipe takes sixteen, and this needs more than
            // one for the same reason it does: the next instance is bound
            // while this one is handled, so a client that reconnects finds a
            // pipe waiting.
            16,
            64 * 1024,
            64 * 1024,
            0,
            None,
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return None;
    }
    // SAFETY: CreateNamedPipeW returned a new owned handle, and this file is
    // the only owner of it. Waiting for a client happens on the handle inside.
    Some(unsafe { std::fs::File::from_raw_handle(handle.0) })
}

/// Wait for a client on a pipe that is already bound.
fn wait_for_client(pipe: std::fs::File) -> Option<std::fs::File> {
    match unsafe { ConnectNamedPipe(HANDLE(pipe.as_raw_handle()), None) } {
        Ok(()) => {}
        // A client that connected between the bind and the wait is already
        // connected, which is the same thing as being waited for.
        Err(error) if error.code() == ERROR_PIPE_CONNECTED.into() => {}
        Err(_) => return None,
    }
    Some(pipe)
}

/// Write one frame, and say whether the connection is still there. A frame
/// that does not go out is the only thing that ends this thread's turn.
fn sent(framed: &Framed, reply: &DaemonMessage) -> bool {
    match framed.send(reply) {
        Ok(()) => true,
        Err(error) => {
            eprintln!("wire: a reply did not go out: {error}");
            false
        }
    }
}

fn connection(framed: Framed, commands: Receiver<ToServer>, frames: Sender<ClientMessage>) {
    let Ok(ClientMessage::Hello(_)) = framed.recv_timeout(HANDSHAKE) else {
        return;
    };
    if framed.send(&DaemonMessage::Hello(daemon_hello())).is_err() {
        return;
    }
    loop {
        // Whatever the test asked for first: a command to push, or the end.
        match commands.try_recv() {
            Ok(ToServer::Close) => return,
            Ok(ToServer::Push(message)) => {
                if !sent(&framed, &message) {
                    return;
                }
                continue;
            }
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
            Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
        }
        let Ok(frame) = framed.recv_timeout(POLL) else {
            continue;
        };
        match frame {
            ClientMessage::BrowserHostRegister { id, .. } => {
                let reply = DaemonMessage::BrowserHostRegistered {
                    id,
                    host_id: HOST_ID.to_owned(),
                };
                if !sent(&framed, &reply) {
                    return;
                }
            }
            // Every answer the host sends is a round trip of its own, so it has
            // to be acknowledged or the host waits out its deadline.
            message @ (ClientMessage::BrowserExecuteResponse { .. }
            | ClientMessage::BrowserHostUnregister { .. }) => {
                let id = match &message {
                    ClientMessage::BrowserExecuteResponse { id, .. } => *id,
                    ClientMessage::BrowserHostUnregister { id, .. } => *id,
                    _ => 0,
                };
                if !sent(&framed, &DaemonMessage::Ok { id }) || frames.send(message).is_err() {
                    return;
                }
            }
            _ => {}
        }
    }
}

pub fn caller() -> BrowserCaller {
    BrowserCaller {
        caller_session_id: "s.agent.1".to_owned(),
        workspace_id: Some("ws-1".to_owned()),
    }
}

pub fn request(id: &str) -> BrowserExecuteRequest {
    command(id, caller())
}

/// The same command from a caller with no workspace: the one answer the real
/// `list_tabs` refuses, through the real refusal.
pub fn request_without_a_workspace(id: &str) -> BrowserExecuteRequest {
    command(
        id,
        BrowserCaller {
            caller_session_id: "s.agent.1".to_owned(),
            workspace_id: None,
        },
    )
}

fn command(id: &str, caller: BrowserCaller) -> BrowserExecuteRequest {
    BrowserExecuteRequest {
        request_id: id.to_owned(),
        host_id: HOST_ID.to_owned(),
        command: "list_tabs".to_owned(),
        args: json!({}),
        caller,
    }
}

/// The production answer for the one command this loop runs: the real
/// `list_tabs`, on the real registry. A tab command never touches a page, so
/// nothing in it needs the window a `WebviewPage` would.
pub fn list_tabs(registry: &BrowserRegistry, request: &BrowserExecuteRequest) -> BrowserOutcome {
    match tabs::list_tabs(registry, &request.caller) {
        Ok(result) => BrowserOutcome::Ok { result },
        Err(error) => BrowserOutcome::Err(error),
    }
}

/// The first answer to a command, read off the wire: the request it belongs
/// to, and the outcome itself.
pub fn next_answer(daemon: &WireDaemon) -> (String, Value) {
    for _ in 0..10 {
        match daemon.next_frame() {
            Some(ClientMessage::BrowserExecuteResponse {
                request_id,
                outcome,
                ..
            }) => {
                return (
                    request_id,
                    serde_json::to_value(&outcome).expect("the outcome is JSON"),
                )
            }
            Some(_) => continue,
            None => break,
        }
    }
    panic!("no browser answer arrived within {ANSWER:?}");
}

/// The host id the next unregistration on the wire names.
pub fn next_unregister(daemon: &WireDaemon) -> String {
    for _ in 0..10 {
        match daemon.next_frame() {
            Some(ClientMessage::BrowserHostUnregister { host_id, .. }) => return host_id,
            Some(_) => continue,
            None => break,
        }
    }
    panic!("no browser host unregistered within {ANSWER:?}");
}
