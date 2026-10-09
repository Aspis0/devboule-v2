//! Helpers shared by the module test blocks.
//!
//! A test that needs an external program — the `node` scripts the Pi and Codex
//! tests use as fake providers — must *skip* when that program is not on PATH
//! instead of failing (A2-13): a machine without `node` is not a broken build,
//! and a suite that reports red for it teaches readers to ignore red.
//!
//! Predicate home: a predicate only one family's suites read stays in that
//! family's own test support; the shared ones live here.

/// A fixture project root that is absolute on the platform under test: the
/// transcript views relativize only absolute paths, and a drive-letter path
/// is relative on Unix.
#[cfg(windows)]
pub(crate) const FIXTURE_ROOT: &str = r"C:\work";
#[cfg(not(windows))]
pub(crate) const FIXTURE_ROOT: &str = "/work";

/// A path under [`FIXTURE_ROOT`] spelled with the platform's own separators:
/// a relativized remainder keeps the separators of the path it was cut from,
/// so both sides of a comparison must be built the same way.
pub(crate) fn fixture_path(relative: &str) -> std::path::PathBuf {
    relative
        .split('/')
        .fold(std::path::PathBuf::from(FIXTURE_ROOT), |path, part| {
            path.join(part)
        })
}

#[cfg(windows)]
use windows_sys::Win32::Foundation::{
    CloseHandle, GetLastError, ERROR_INVALID_PARAMETER, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT,
};
#[cfg(windows)]
use windows_sys::Win32::System::Threading::{
    OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
};

/// Why a test that needs `program` must skip here, or `None` when it can run.
///
/// One implementation for every module's test block: the reason it returns is
/// printed by the skipping test, and it names the program and the OS error so a
/// reader of the log knows what was missing.
pub(crate) fn external_program_skip_reason(program: &str) -> Option<String> {
    match std::process::Command::new(program)
        .arg("--version")
        .output()
    {
        Ok(_) => None,
        Err(error) => Some(format!(
            "skipping: `{program}` is not runnable here ({error}; kind={:?})",
            error.kind()
        )),
    }
}

/// Wait until the process `pid` has exited, or fail at the bound.
///
/// A job can report empty before its members' process objects signal, so an
/// immediate check flakes on a kill that worked. Scoped to PIDs this
/// test started: those it may open, so an open failure short of `ERROR_INVALID_PARAMETER` —
/// `ERROR_ACCESS_DENIED` would be somebody else's process — panics instead of
/// reading as gone. Opening the PID after the death can catch a reused PID of
/// an unrelated process: that can invent a failure, never hide a live
/// original. Twin of `wait_until_gone` in `tests/acp_sessions.rs`, which
/// cannot import lib `cfg(test)` items; the copies diverge on purpose — 5 s
/// and any-open-failure-reads-as-gone there, a 2 s bound and a panic on every
/// other open failure here.
#[cfg(windows)]
pub(crate) fn wait_until_pid_gone(pid: u32, role: &str) {
    const BOUND_MS: u32 = 2_000;
    let handle = match open_process_to_wait_for(pid) {
        Ok(handle) => handle,
        Err(ERROR_INVALID_PARAMETER) => return,
        Err(error) => panic!(
            "OpenProcess({pid}) failed with GetLastError={error}; only a nonexistent PID may read as gone"
        ),
    };
    let waited = unsafe { WaitForSingleObject(handle, BOUND_MS) };
    // Read before CloseHandle: Win32 does not promise to keep the last-error
    // slot across a successful call, so a later read may report another code.
    let wait_error = unsafe { GetLastError() };
    unsafe { CloseHandle(handle) };
    match waited {
        WAIT_OBJECT_0 => {}
        WAIT_TIMEOUT => panic!("{role} {pid} is still alive after {BOUND_MS} ms"),
        code => {
            panic!(
                "{role} {pid}: WaitForSingleObject failed with {code:#x} (GetLastError={wait_error})"
            )
        }
    }
}

/// Whether `pid` names a live process right now: the census a test takes
/// before waiting its tracked PIDs out, so the log records how many were
/// still alive when the probe timed out, success or failure; the wait's
/// panic is what names the PID that never exits.
///
/// A census reports; it does not fail the test: an open it could not make is
/// not an observation of death, so only `ERROR_INVALID_PARAMETER` — no such
/// process — counts as dead, and any other open failure counts as alive.
#[cfg(windows)]
pub(crate) fn pid_is_alive(pid: u32) -> bool {
    match open_process_to_wait_for(pid) {
        Err(error) => error != ERROR_INVALID_PARAMETER,
        Ok(handle) => {
            // A failed wait must not claim death.
            let alive = unsafe { WaitForSingleObject(handle, 0) } != WAIT_OBJECT_0;
            unsafe { CloseHandle(handle) };
            alive
        }
    }
}

/// The handle to wait on for `pid`, or the `GetLastError` of the failed open.
///
/// The raw failure, not a verdict: which failures may read as "gone" belongs
/// to the caller, whose own PIDs are the ones being opened.
#[cfg(windows)]
fn open_process_to_wait_for(pid: u32) -> Result<HANDLE, u32> {
    let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
    if handle.is_null() {
        Err(unsafe { GetLastError() })
    } else {
        Ok(handle)
    }
}

/// Run one steerer through a real turn admission, the way the daemon admits a
/// steer: a turn is begun, and the steerer is handed the token that
/// `with_active_turn` gives out. `None` means the turn was already over at
/// admission, which is a different answer from any the steerer can give.
#[cfg(test)]
pub(crate) fn steer_through_the_turn(
    steerer: &mut dyn crate::session::SessionSteerer,
    text: &str,
) -> Option<Result<bool, devboule_protocol::WireError>> {
    let runtime = std::sync::Arc::new(crate::session::SessionRuntime::new());
    runtime.begin_turn();
    runtime.with_active_turn(runtime.turn_counter(), |turn| {
        steerer.steer_active_turn(text, turn, crate::session::SteerOrigin::Person)
    })
}

/// A canned Noise responder for loopback peer dials: completes the responder
/// handshake, answers the hello advertising exactly `capabilities`, and sends
/// `reply` as the answer to whatever request arrives. Loopback is a test-only
/// dial target (`is_tailnet_or_test_loopback`). One helper for every test
/// that dials, so the handshake choreography is written once.
#[cfg(test)]
pub(crate) fn spawn_canned_noise_responder(
    static_private: [u8; 32],
    capabilities: Vec<devboule_protocol::Capability>,
    reply: devboule_protocol::DaemonMessage,
) -> std::net::SocketAddr {
    spawn_capturing_noise_responder(static_private, capabilities, reply, 1).0
}

/// The same responder, reporting each of `dials` requests; `None` is a
/// handshake with no request, which is how a capability refusal looks from the far side.
#[cfg(test)]
pub(crate) fn spawn_capturing_noise_responder(
    static_private: [u8; 32],
    capabilities: Vec<devboule_protocol::Capability>,
    reply: devboule_protocol::DaemonMessage,
    dials: usize,
) -> (
    std::net::SocketAddr,
    std::sync::mpsc::Receiver<Option<devboule_protocol::ClientMessage>>,
) {
    use crate::framing::Framed;
    use crate::peer_transport::{
        responder_handshake, split_session, HANDSHAKE_DEADLINE, PEER_NOISE_PATTERN, PEER_PROLOGUE,
    };
    use devboule_protocol::{
        ClientMessage, DaemonHello, DaemonMessage, PROTOCOL_MIN_VERSION, PROTOCOL_VERSION,
    };
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the fake responder");
    let address = listener.local_addr().expect("fake responder address");
    let (requests_tx, requests_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for _ in 0..dials {
            let (stream, _) = listener.accept().expect("accept one dial");
            let session = responder_handshake(
                &stream,
                Instant::now() + HANDSHAKE_DEADLINE,
                &static_private,
                PEER_PROLOGUE,
                None,
                PEER_NOISE_PATTERN,
            )
            .expect("fake responder handshake");
            let (reader, writer, closer) = split_session(&stream, session).expect("split");
            let framed = Framed::from_stream(reader, writer, closer);
            let hello: ClientMessage = framed
                .recv_timeout(Duration::from_secs(10))
                .expect("the dial's hello");
            assert!(matches!(hello, ClientMessage::Hello(_)));
            framed
                .send(&DaemonMessage::Hello(DaemonHello {
                    protocol_version: PROTOCOL_VERSION,
                    min_protocol_version: PROTOCOL_MIN_VERSION,
                    daemon_version: "test".to_string(),
                    instance_id: "fake-responder".to_string(),
                    pid: std::process::id(),
                    capabilities: capabilities.clone(),
                    workspace_host: None,
                }))
                .expect("hello reply");
            let request = match framed.recv_timeout::<ClientMessage>(Duration::from_secs(10)) {
                Ok(request) => request,
                Err(_) => {
                    let _ = requests_tx.send(None);
                    continue;
                }
            };
            // The reply must go out even when the test dropped the report channel.
            let reported = requests_tx.send(Some(request));
            framed.send(&reply).expect("send the canned reply");
            if reported.is_err() {
                return;
            }
        }
    });
    (address, requests_rx)
}

/// A far end that takes the request and goes quiet; it holds the connection
/// past the dialer's deadline so the failure is the timeout, not a racing close.
#[cfg(test)]
pub(crate) fn spawn_silent_noise_responder(
    static_private: [u8; 32],
    capabilities: Vec<devboule_protocol::Capability>,
) -> (
    std::net::SocketAddr,
    std::sync::mpsc::Receiver<Option<devboule_protocol::ClientMessage>>,
) {
    use crate::framing::Framed;
    use crate::peer_transport::{
        responder_handshake, split_session, HANDSHAKE_DEADLINE, PEER_NOISE_PATTERN, PEER_PROLOGUE,
    };
    use devboule_protocol::{
        ClientMessage, DaemonHello, DaemonMessage, PROTOCOL_MIN_VERSION, PROTOCOL_VERSION,
    };
    use std::net::TcpListener;
    use std::time::{Duration, Instant};

    let listener = TcpListener::bind("127.0.0.1:0").expect("bind the fake responder");
    let address = listener.local_addr().expect("fake responder address");
    let (requests_tx, requests_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let (stream, _) = listener.accept().expect("accept one dial");
        let session = responder_handshake(
            &stream,
            Instant::now() + HANDSHAKE_DEADLINE,
            &static_private,
            PEER_PROLOGUE,
            None,
            PEER_NOISE_PATTERN,
        )
        .expect("fake responder handshake");
        let (reader, writer, closer) = split_session(&stream, session).expect("split");
        let framed = Framed::from_stream(reader, writer, closer);
        let hello: ClientMessage = framed
            .recv_timeout(Duration::from_secs(10))
            .expect("the dial's hello");
        assert!(matches!(hello, ClientMessage::Hello(_)));
        framed
            .send(&DaemonMessage::Hello(DaemonHello {
                protocol_version: PROTOCOL_VERSION,
                min_protocol_version: PROTOCOL_MIN_VERSION,
                daemon_version: "test".to_string(),
                instance_id: "fake-responder".to_string(),
                pid: std::process::id(),
                capabilities: capabilities.clone(),
                workspace_host: None,
            }))
            .expect("hello reply");
        match framed.recv_timeout::<ClientMessage>(Duration::from_secs(10)) {
            Ok(request) => {
                let _ = requests_tx.send(Some(request));
            }
            Err(_) => {
                let _ = requests_tx.send(None);
                return;
            }
        }
        std::thread::sleep(Duration::from_secs(15));
    });
    (address, requests_rx)
}

/// Every `.rs` file under `src/mcp_broker/`, read from disk at test time.
///
/// The broker's guard tests scan the module's own sources for needles that
/// must appear exactly zero or one times. They read the tree instead of
/// naming files with `include_str!` so a file the module gains later — the
/// next tool group, a rename — is covered the moment it exists instead of
/// never: a hand-written path list is a guard that quietly stops guarding.
pub(crate) fn mcp_broker_sources() -> Vec<String> {
    fn collect(dir: &std::path::Path, files: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("the mcp_broker module directory exists") {
            let path = entry.expect("one readable directory entry").path();
            if path.is_dir() {
                collect(&path, files);
            } else if path
                .extension()
                .is_some_and(|extension| extension.to_str() == Some("rs"))
            {
                files.push(path);
            }
        }
    }
    let mut files = Vec::new();
    collect(
        &std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("mcp_broker"),
        &mut files,
    );
    files.sort();
    files
        .iter()
        .map(|path| {
            std::fs::read_to_string(path)
                .unwrap_or_else(|error| panic!("read the broker source {path:?}: {error}"))
        })
        .collect()
}

/// Whether an event is the turn watchdog's expiry notice: the shared
/// constructor's sentence, for either family, matched by fragment so a
/// reword fails the suites' assertions instead of greening them.
pub(crate) fn is_watchdog_error(event: &devboule_protocol::SessionEvent) -> bool {
    matches!(
        event,
        devboule_protocol::SessionEvent::AgentError { message }
            if message.contains("produced no output") && message.contains("the run was ended")
    )
}

/// Whether a pull carries the watchdog's error finish.
pub(crate) fn is_error_finish(event: &devboule_protocol::SessionEvent) -> bool {
    matches!(
        event,
        devboule_protocol::SessionEvent::AgentFinished { stop_reason, .. }
            if stop_reason == "error"
    )
}

/// Whether a pull carries any run finish.
pub(crate) fn is_any_finish(event: &devboule_protocol::SessionEvent) -> bool {
    matches!(event, devboule_protocol::SessionEvent::AgentFinished { .. })
}

/// The node echo child the Claude delivered-prompt and delivered-steer
/// tests read back through: every byte written comes back, so a released
/// gate delivers and the test reads what the daemon wrote.
pub(crate) fn spawn_node_echo() -> std::process::Child {
    std::process::Command::new("node")
        .args([
            "-e",
            "process.stdin.on('data', data => process.stdout.write(data))",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("node is required for the echo-child tests")
}

/// One JSON line off an echo child. Both Claude harnesses read exactly one
/// line per gate release; a missing or malformed line fails the test.
pub(crate) fn read_echoed_json(reader: &mut impl std::io::BufRead) -> serde_json::Value {
    let mut buf = Vec::new();
    reader.read_until(b'\n', &mut buf).expect("echoed line");
    serde_json::from_str::<serde_json::Value>(String::from_utf8_lossy(&buf).trim_end())
        .expect("echoed json")
}

/// The CLI's answer to a mode request: success for the echoed request,
/// releasing the gate with the default mode — the one shape both
/// harnesses use.
pub(crate) fn mode_control_response(request: &serde_json::Value) -> serde_json::Value {
    serde_json::json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request["request_id"],
            "response": {"mode": "default"},
        },
    })
}

/// The stdin line-framer every node fake shares: chunks accumulate, and
/// each `\n`-terminated line (newline kept) goes to the fake's own
/// `onFramedLine`. Fakes differ only there: the echo and stats fakes
/// handle the line whole, the local-command fake strips the newline first.
pub(crate) const NODE_FRAMED_STDIN: &str = r#"
let buffered = "";
process.stdin.on("data", (chunk) => {
  buffered += chunk;
  let index;
  while ((index = buffered.indexOf("\n")) >= 0) {
    const line = buffered.slice(0, index + 1);
    buffered = buffered.slice(index + 1);
    onFramedLine(line);
  }
});
"#;

/// Attach a fresh connection to a test runtime: no cursor, typed
/// permissions, tracked with the runtime's own generation. The runtime's
/// construction (live or journaled, its session id, its agent kind) stays
/// at the call site — only this tail is shared.
pub(crate) fn attach_and_track(
    runtime: &std::sync::Arc<crate::session::SessionRuntime>,
    session_id: &str,
) -> std::sync::Arc<crate::session::ConnHandle> {
    let conn = crate::session::ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach");
    conn.track_with_agent_replay(
        session_id,
        std::sync::Arc::clone(runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    conn
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The policy the node-backed tests take, pinned with a program that is
    /// certainly not on PATH: an unrunnable program is a *reason to skip*, never
    /// a panic and never `None`. A refactor that turns the failure into an
    /// `expect`, or that reports `None` for a program it could not spawn, fails
    /// this before it can turn a missing `node` into a red suite.
    #[test]
    fn a_program_that_cannot_be_spawned_is_a_skip_not_a_failure() {
        let reason = external_program_skip_reason("devboule-no-such-program-3f1a")
            .expect("an unrunnable program is a skip");
        assert!(reason.contains("skipping"), "{reason}");
        assert!(reason.contains("devboule-no-such-program-3f1a"), "{reason}");
    }
}
