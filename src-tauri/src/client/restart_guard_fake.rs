//! The fake the restart guard's tests share: a client that holds its answers
//! until a call takes them, so a second call cannot silently reuse one.

use std::io;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use devboule_daemon::DaemonError;
use devboule_protocol::{
    DaemonStatusBody, Session, SessionKind, SessionOrigin, SessionState, UnattendedState,
};

use super::RestartClient;

pub(super) struct FakeRestartClient {
    roster: Mutex<Option<Result<Vec<Session>, DaemonError>>>,
    status: Mutex<Option<Result<DaemonStatusBody, DaemonError>>>,
    fresh: Mutex<Option<Result<Box<dyn RestartClient>, DaemonError>>>,
    kill: Mutex<Option<Result<(), DaemonError>>>,
    /// Shared so a test can keep counting the kills of a fake the guard owns
    /// once it is boxed as the second connection.
    kills: Arc<AtomicUsize>,
    probe_budgets: Mutex<Vec<Duration>>,
    declared_exit: bool,
}

impl FakeRestartClient {
    pub(super) fn roster_answering(roster: Result<Vec<Session>, DaemonError>) -> Self {
        Self {
            roster: Mutex::new(Some(roster)),
            status: Mutex::new(Some(Ok(status_body(Some(0), Some(0))))),
            fresh: Mutex::new(Some(Err(no_daemon_at_all()))),
            kill: Mutex::new(Some(Ok(()))),
            kills: Arc::new(AtomicUsize::new(0)),
            probe_budgets: Mutex::new(Vec::new()),
            declared_exit: false,
        }
    }

    /// Both reads answered, nothing running.
    pub(super) fn idle() -> Self {
        Self::roster_answering(Ok(Vec::new()))
    }

    pub(super) fn holding(sessions: Vec<Session>) -> Self {
        Self::roster_answering(Ok(sessions))
    }

    pub(super) fn with_status(self, agents: Option<u32>, terminals: Option<u32>) -> Self {
        *self.status.lock().unwrap() = Some(Ok(status_body(agents, terminals)));
        self
    }

    /// The status body's count of sessions still starting.
    pub(super) fn with_configuring(self, count: u32) -> Self {
        let mut status = self.status.lock().unwrap();
        let Some(Ok(body)) = status.as_mut() else {
            panic!("the status answer is a body");
        };
        body.configuring_sessions = Some(count);
        drop(status);
        self
    }

    /// A status body from a daemon that predates the count of starting sessions.
    pub(super) fn with_no_configuring_count(self) -> Self {
        let mut status = self.status.lock().unwrap();
        let Some(Ok(body)) = status.as_mut() else {
            panic!("the status answer is a body");
        };
        body.configuring_sessions = None;
        drop(status);
        self
    }

    pub(super) fn with_a_declared_exit(mut self) -> Self {
        self.declared_exit = true;
        self
    }

    pub(super) fn with_an_unreadable_status(self) -> Self {
        *self.status.lock().unwrap() = Some(Err(DaemonError::TimedOut(
            "waiting for a daemon reply".to_string(),
        )));
        self
    }

    /// The second connection the guard buys after a drop.
    pub(super) fn answered_by(self, fresh: FakeRestartClient) -> Self {
        *self.fresh.lock().unwrap() = Some(Ok(Box::new(fresh)));
        self
    }

    pub(super) fn with_the_second_connection_failing(self, error: DaemonError) -> Self {
        *self.fresh.lock().unwrap() = Some(Err(error));
        self
    }

    pub(super) fn with_a_failing_kill(self) -> Self {
        *self.kill.lock().unwrap() = Some(Err(DaemonError::Protocol(
            "the connected daemon is no longer that process".to_string(),
        )));
        self
    }

    pub(super) fn kill_calls(&self) -> usize {
        self.kills.load(Ordering::SeqCst)
    }

    /// A handle on this fake's kill count that outlives boxing it.
    pub(super) fn kill_counter(&self) -> Arc<AtomicUsize> {
        Arc::clone(&self.kills)
    }

    pub(super) fn probe_budgets(&self) -> Vec<Duration> {
        self.probe_budgets.lock().unwrap().clone()
    }

    fn take<T>(answer: &Mutex<Option<T>>) -> T {
        answer
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take()
            .expect("the guard reads each answer once")
    }
}

impl RestartClient for FakeRestartClient {
    fn sessions_list(&self) -> Result<Vec<Session>, DaemonError> {
        Self::take(&self.roster)
    }

    fn status(&self) -> Result<DaemonStatusBody, DaemonError> {
        Self::take(&self.status)
    }

    fn fresh_daemon(&self, budget: Duration) -> Result<Box<dyn RestartClient>, DaemonError> {
        self.probe_budgets.lock().unwrap().push(budget);
        Self::take(&self.fresh)
    }

    fn restart_daemon(&self) -> Result<(), DaemonError> {
        self.kills.fetch_add(1, Ordering::SeqCst);
        Self::take(&self.kill)
    }

    fn declared_exit(&self) -> bool {
        self.declared_exit
    }
}

/// `CreateFileW` on the pipe name answered `ERROR_FILE_NOT_FOUND`.
pub(super) fn no_daemon_at_all() -> DaemonError {
    DaemonError::Io(io::Error::from(io::ErrorKind::NotFound))
}

/// A pipe that exists and cannot be reached: `connect_pipe` gives up on a busy
/// one with exactly this
/// (`crates/devboule-daemon/src/transport/windows_pipe.rs`, `connect_pipe_retrying`).
pub(super) fn pipe_is_busy() -> DaemonError {
    DaemonError::Io(io::Error::new(
        io::ErrorKind::TimedOut,
        "named pipe is busy",
    ))
}

fn status_body(agents: Option<u32>, terminals: Option<u32>) -> DaemonStatusBody {
    DaemonStatusBody {
        instance_id: "i".to_string(),
        protocol_version: 21,
        daemon_version: "0.1.0".to_string(),
        pid: 1,
        uptime_ms: 0,
        clients: 1,
        local_clients: 1,
        sessions: 0,
        agents,
        terminals,
        configuring_sessions: Some(0),
        capabilities: Vec::new(),
        peak_ring_bytes: 0,
        ring_evicted_bytes: 0,
        ring_dropped_frames: 0,
        journal_error: None,
        tool_policy_error: None,
        log_error: None,
        journal_stats: None,
        secret_store: None,
        remote: None,
    }
}

pub(super) fn session(id: &str, kind: SessionKind, state: SessionState) -> Session {
    Session {
        id: id.to_string(),
        workspace_id: None,
        cwd: None,
        kind,
        title: id.to_string(),
        provider: None,
        peer_session_id: None,
        state,
        elapsed_ms: Some(1),
        created_at_ms: 1,
        origin: SessionOrigin::local(),
        display_name: None,
        created_by: None,
        profile_id: None,
        context_id: None,
        unattended: UnattendedState::Unknown,
        labels: Default::default(),
        resumable: false,
    }
}

pub(super) fn live(id: &str, generation: u64) -> Session {
    session(id, SessionKind::Terminal, SessionState::Live { generation })
}
