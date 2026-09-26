//! Helpers shared by the module test blocks.
//!
//! A test that needs an external program — the `node` scripts the Pi and Codex
//! tests use as fake providers — must *skip* when that program is not on PATH
//! instead of failing (A2-13): a machine without `node` is not a broken build,
//! and a suite that reports red for it teaches readers to ignore red.

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
        steerer.steer_active_turn(text, turn)
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
                capabilities,
            }))
            .expect("hello reply");
        let _request: ClientMessage = framed
            .recv_timeout(Duration::from_secs(10))
            .expect("the dial's request");
        framed.send(&reply).expect("send the canned reply");
    });
    address
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
