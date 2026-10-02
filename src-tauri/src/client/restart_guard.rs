//! Refuse a daemon restart that would kill a live session.
//!
//! Every provider child and terminal sits in the daemon's kill-on-close Job
//! Object (`crates/devboule-daemon/src/process_tree.rs`), so stopping the
//! daemon stops those processes too. The app asks for confirmation; this is
//! the daemon's own answer to "is anything running", read immediately before
//! the kill, so the refusal does not depend on the dialog.

use std::io::ErrorKind;
use std::time::Duration;

use devboule_daemon::{DaemonClient, DaemonError};
use devboule_protocol::{DaemonStatusBody, ErrorCode, Session};

use super::{connect_live_within, daemon_declared_exit};
use crate::backend::error::CommandError;

/// How long the second connection waits on a busy pipe: the restart button
/// answers in seconds instead of holding a worker for the pipe's full wait.
const PROBE_BUDGET: Duration = Duration::from_secs(3);

/// The daemon calls a restart makes: the roster read, the status read, the
/// kill, and one more connection when the first one drops. Split out so the
/// order the guard runs them in is testable without a live daemon.
pub(super) trait RestartClient {
    fn sessions_list(&self) -> Result<Vec<Session>, DaemonError>;
    fn status(&self) -> Result<DaemonStatusBody, DaemonError>;
    fn restart_daemon(&self) -> Result<(), DaemonError>;
    /// A connection to whatever daemon is listening now, without starting
    /// one, giving up on a busy pipe after about `budget`. That may not be the
    /// daemon this client reached, so the caller reads and stops through the
    /// new connection alone.
    fn fresh_daemon(&self, budget: Duration) -> Result<Box<dyn RestartClient>, DaemonError>;
    /// Whether the daemon was asked to stop, which the supervisor honours by
    /// not starting a new one.
    fn declared_exit(&self) -> bool;
}

impl RestartClient for DaemonClient {
    fn sessions_list(&self) -> Result<Vec<Session>, DaemonError> {
        DaemonClient::sessions_list(self)
    }

    fn status(&self) -> Result<DaemonStatusBody, DaemonError> {
        DaemonClient::status(self)
    }

    fn restart_daemon(&self) -> Result<(), DaemonError> {
        DaemonClient::restart_daemon(self)
    }

    fn fresh_daemon(&self, budget: Duration) -> Result<Box<dyn RestartClient>, DaemonError> {
        Ok(Box::new(connect_live_within(budget)?))
    }

    fn declared_exit(&self) -> bool {
        daemon_declared_exit()
    }
}

/// Restart the daemon, unless a session still holds a process.
///
/// Two reads answer that: the larger of the roster's live rows and the status
/// body's live counts, plus the children still starting, which only the status
/// body counts. A session one read misses and the other sees still blocks, and
/// a status read that fails or leaves a count out refuses.
///
/// A dropped connection is not an answer: the daemon's process and its
/// children can be alive with this client gone. So the guard asks a second
/// connection instead and never kills through the first again. A daemon that
/// is not there at all has nothing to protect and nothing to stop.
pub(super) fn restart(client: &dyn RestartClient) -> Result<(), CommandError> {
    match running_count(client) {
        Ok(running) => stop_unless_running(client, running),
        Err(error) if is_dropped(&error) => ask_a_fresh_daemon(client),
        Err(error) => Err(unreadable(&error)),
    }
}

/// The same question over a connection this client did not have, asked once:
/// a second drop on that connection refuses rather than probing again. The
/// roster and the kill go through that one connection, so they concern the
/// same daemon. With no pipe instance nothing is stopped and the supervisor's
/// reconnect starts the new daemon, unless the daemon was asked to stop.
fn ask_a_fresh_daemon(client: &dyn RestartClient) -> Result<(), CommandError> {
    let fresh = match client.fresh_daemon(PROBE_BUDGET) {
        Ok(fresh) => fresh,
        Err(error) if is_absent(&error) => return start_is_left_to_the_supervisor(client),
        Err(error) => return Err(unreadable(&error)),
    };
    let running = running_count(&*fresh).map_err(|error| unreadable(&error))?;
    stop_unless_running(&*fresh, running)
}

/// No daemon is listening, so this command starts none: a lost connection sends
/// the supervisor to `connect_or_spawn` — unless the daemon declared its exit,
/// when `run_supervisor_loop` returns `Stopped` and nothing comes back.
fn start_is_left_to_the_supervisor(client: &dyn RestartClient) -> Result<(), CommandError> {
    if client.declared_exit() {
        return Err(CommandError::new(
            ErrorCode::Io,
            "The daemon is shutting down, so it was not restarted.",
        ));
    }
    Ok(())
}

/// The kill, or the refusal that stands in its place.
fn stop_unless_running(client: &dyn RestartClient, running: usize) -> Result<(), CommandError> {
    if running > 0 {
        return Err(CommandError::new(
            ErrorCode::InvalidRequest,
            refusal(running),
        ));
    }
    Ok(client.restart_daemon()?)
}

/// How many sessions own a process: live rows plus children still starting.
/// The roster never lists a child still starting, so the status body is the
/// only witness for it: a status that fails, or does not give a count, is an
/// unreadable answer and never a zero.
fn running_count(client: &dyn RestartClient) -> Result<usize, DaemonError> {
    let listed = client
        .sessions_list()?
        .iter()
        .filter(|session| session.state.is_live())
        .count();
    let status = client.status()?;
    let (Some(agents), Some(terminals)) = (status.agents, status.terminals) else {
        return Err(missing_count("its agent and terminal counts"));
    };
    let Some(configuring) = status.configuring_sessions else {
        return Err(missing_count("how many sessions are still starting"));
    };
    let live = agents.saturating_add(terminals) as usize;
    Ok(listed.max(live).saturating_add(configuring as usize))
}

fn missing_count(what: &str) -> DaemonError {
    DaemonError::Protocol(format!("the daemon's status does not report {what}"))
}

/// Whether the read failed because *this* connection is gone, which says
/// nothing about the daemon: the client's read loop reports every non-timeout
/// framing failure that way (`client_read_loop` in
/// `crates/devboule-daemon/src/client.rs`), so a live daemon can drop one
/// connection with its children untouched.
fn is_dropped(error: &DaemonError) -> bool {
    match error {
        DaemonError::ConnectionLost => true,
        DaemonError::Io(error) => matches!(
            error.kind(),
            ErrorKind::BrokenPipe | ErrorKind::NotConnected
        ),
        _ => false,
    }
}

/// Whether the fresh connection found no daemon at all: `CreateFileW` on the
/// pipe name answered `ERROR_FILE_NOT_FOUND`, the kind `io` reports as
/// `NotFound` (`connect_pipe_retrying` in
/// `crates/devboule-daemon/src/transport/windows_pipe.rs`). A busy pipe, a
/// denied one and a handshake failure are the opposite answer — something is
/// there, or something we cannot read past — and `connect_without_spawning`
/// reports its own faults as `Protocol` so only the pipe's own refusal lands
/// here.
fn is_absent(error: &DaemonError) -> bool {
    matches!(error, DaemonError::Io(error) if error.kind() == ErrorKind::NotFound)
}

/// The refusal for a roster this client could not read, carrying the daemon's
/// own reason.
fn unreadable(error: &DaemonError) -> CommandError {
    CommandError::new(
        ErrorCode::Io,
        format!(
            "could not check for running agents and terminals ({error}), so the daemon was not \
             restarted."
        ),
    )
}

/// The refusal, in one sentence, agreeing with the count that produced it: the
/// app renders this string as it is.
fn refusal(live: usize) -> String {
    let (noun, verb, pronoun) = match live {
        1 => ("agent or terminal", "is", "it"),
        _ => ("agents or terminals", "are", "them"),
    };
    format!(
        "{live} {noun} {verb} running; restarting the daemon would stop {pronoun}. \
         Stop {pronoun} first, then restart."
    )
}

#[cfg(test)]
#[path = "restart_guard_fake.rs"]
mod fake;
#[cfg(test)]
#[path = "restart_guard_probe_tests.rs"]
mod probe_tests;
#[cfg(test)]
#[path = "restart_guard_tests.rs"]
mod tests;
