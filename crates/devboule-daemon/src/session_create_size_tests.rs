//! The create road's initial terminal geometry: a measured grid reaching
//! both the PTY and the emulator at birth, and an unmeasured create opening
//! at the daemon's default.

use std::sync::Arc;

use devboule_protocol::{OwnerId, SessionKind};

use crate::profile_delivery::ProfileDelivery;
use crate::server::ServerState;

use super::session_create_tests::road_state;
use super::SessionCreateMeta;

fn create_terminal(
    state: &Arc<ServerState>,
    owner: &OwnerId,
    command: super::PtyCommand,
    size: Option<(u16, u16)>,
) -> devboule_protocol::Session {
    state
        .sessions
        .create_with_provider_env(
            state,
            owner,
            None,
            SessionKind::Terminal,
            None,
            ProfileDelivery::none(),
            Some(command),
            &None,
            None,
            &SessionCreateMeta::default(),
            size,
        )
        .expect("the spawn succeeds")
}

/// A lingering child: alive for a few seconds after the create answers, so
/// the registry entry (and with it the master and the runtime) is still there
/// when the test reads the geometry back. An echo child would exit-and-reap
/// mid-assertion. The platform's own long-lived program does the same job on
/// both: `ping` on Windows, `sleep` on Unix.
fn lingering_command(label: &str) -> super::PtyCommand {
    #[cfg(windows)]
    let (program, args) = (
        "cmd.exe",
        vec![
            "/c".to_string(),
            "ping".to_string(),
            "-n".to_string(),
            "4".to_string(),
            "127.0.0.1".to_string(),
        ],
    );
    #[cfg(not(windows))]
    let (program, args) = ("/bin/sleep", vec!["30".to_string()]);
    super::PtyCommand::new(
        program,
        args,
        crate::test_dirs::test_temp_dir(label),
        Vec::new(),
    )
}

/// The session's live PTY grid and screen grid, each as `(cols, rows)`.
pub(super) fn opened_geometry(
    registry: &super::SessionRegistry,
    session_id: &str,
) -> ((u16, u16), (u16, u16)) {
    let map = registry.inner.lock().expect("registry");
    let entry = map
        .get(session_id)
        .expect("the live terminal")
        .as_child_process()
        .expect("a child process entry");
    let master = entry.master.clone();
    let runtime = std::sync::Arc::clone(&entry.runtime);
    drop(map);
    let pty_size = master
        .expect("the terminal road owns a PTY")
        .lock()
        .expect("master")
        .get_size()
        .expect("the PTY reports its size");
    let screen = runtime
        .lock_stream()
        .expect("stream")
        .screen
        .as_ref()
        .expect("a terminal screen")
        .dimensions();
    ((pty_size.cols, pty_size.rows), screen)
}

fn release(state: &Arc<ServerState>, owner: &OwnerId, session_id: &str) {
    let _ = state.sessions.close(session_id, owner, &None);
}

/// Mutant: `open_pty_session` dropping the create's size and opening at the
/// constants — the PTY and the screen (stamped from what that road opened)
/// both come back 120×32 and this fails.
#[test]
fn a_measured_create_opens_the_pty_and_the_screen_at_that_grid() {
    let (state, owner) = road_state("create-size-measured", "S-1-5-21-create-size");
    let session = create_terminal(
        &state,
        &owner,
        lingering_command("devboule-create-size"),
        Some((93, 28)),
    );
    let (pty, screen) = opened_geometry(&state.sessions, &session.id);
    assert_eq!(pty, (93, 28), "the PTY at the ask");
    assert_eq!(screen, (93, 28), "the screen born at the same grid");
    release(&state, &owner, &session.id);
}

/// Mutant: the default applied twice (the create size dropped before the
/// spawn) — the measured test above catches that; this one catches the
/// opposite drift, a fallback that invents a grid of its own.
#[test]
fn an_unmeasured_create_opens_at_the_daemon_default() {
    let (state, owner) = road_state("create-size-default", "S-1-5-21-create-size");
    let session = create_terminal(
        &state,
        &owner,
        lingering_command("devboule-create-size"),
        None,
    );
    let (pty, screen) = opened_geometry(&state.sessions, &session.id);
    assert_eq!(pty, (120, 32));
    assert_eq!(screen, (120, 32));
    release(&state, &owner, &session.id);
}
