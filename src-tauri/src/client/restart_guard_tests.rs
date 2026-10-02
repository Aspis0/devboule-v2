//! What the guard counts as running, and what it does with a roster it could
//! not read. The roads after a lost connection are in the probe tests.

use devboule_daemon::DaemonError;
use devboule_protocol::{ErrorCode, SessionKind, SessionState, TranscriptIntegrity, WireError};

use super::fake::{live, session, FakeRestartClient};
use super::*;

#[test]
fn a_live_agent_refuses_the_restart_and_the_daemon_keeps_running() {
    let client = FakeRestartClient::holding(vec![session(
        "s.1",
        SessionKind::Claude,
        SessionState::Live { generation: 1 },
    )]);

    let error = restart(&client).expect_err("a live agent must refuse the restart");

    assert_eq!(
        error.message,
        "1 agent or terminal is running; restarting the daemon would stop it. \
         Stop it first, then restart."
    );
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(client.kill_calls(), 0, "the daemon must not be stopped");
}

#[test]
fn a_silent_session_still_holds_a_process_and_is_counted() {
    let client = FakeRestartClient::holding(vec![
        live("s.1", 1),
        session(
            "s.2",
            SessionKind::Terminal,
            SessionState::Silent { generation: 4 },
        ),
    ]);

    let error = restart(&client).expect_err("two live sessions must refuse the restart");

    assert_eq!(
        error.message,
        "2 agents or terminals are running; restarting the daemon would stop them. \
         Stop them first, then restart."
    );
    assert_eq!(client.kill_calls(), 0, "the daemon must not be stopped");
}

#[test]
fn a_status_that_counts_live_sessions_refuses_an_empty_roster() {
    let client = FakeRestartClient::idle().with_status(Some(1), Some(1));

    let error = restart(&client).expect_err("the daemon's own counts must refuse too");

    assert_eq!(
        error.message,
        "2 agents or terminals are running; restarting the daemon would stop them. \
         Stop them first, then restart."
    );
    assert_eq!(client.kill_calls(), 0, "the daemon must not be stopped");
}

#[test]
fn a_session_still_configuring_refuses_though_nothing_is_live() {
    let client = FakeRestartClient::idle().with_configuring(1);

    let error = restart(&client).expect_err("a spawned child still starting must refuse");

    assert_eq!(
        error.message,
        "1 agent or terminal is running; restarting the daemon would stop it. \
         Stop it first, then restart."
    );
    assert_eq!(error.code, ErrorCode::InvalidRequest);
    assert_eq!(client.kill_calls(), 0, "the daemon must not be stopped");
}

#[test]
fn configuring_sessions_are_counted_with_the_live_ones() {
    let client = FakeRestartClient::holding(vec![live("s.1", 1)]).with_configuring(2);

    let error = restart(&client).expect_err("live and starting sessions both refuse");

    assert_eq!(
        error.message,
        "3 agents or terminals are running; restarting the daemon would stop them. \
         Stop them first, then restart."
    );
    assert_eq!(client.kill_calls(), 0, "the daemon must not be stopped");
}

#[test]
fn a_status_that_names_no_families_is_not_restarted() {
    let client = FakeRestartClient::idle().with_status(None, None);

    let error = restart(&client).expect_err("an unknown count is not a zero");

    assert_eq!(
        error.message,
        "could not check for running agents and terminals (the daemon's status does not report \
         its agent and terminal counts), so the daemon was not restarted."
    );
    assert_eq!(error.code, ErrorCode::Io);
    assert_eq!(client.kill_calls(), 0, "the daemon must not be stopped");
}

#[test]
fn a_status_without_the_starting_count_is_not_restarted() {
    let client = FakeRestartClient::idle().with_no_configuring_count();

    let error =
        restart(&client).expect_err("an older daemon cannot vouch for its starting children");

    assert_eq!(
        error.message,
        "could not check for running agents and terminals (the daemon's status does not report \
         how many sessions are still starting), so the daemon was not restarted."
    );
    assert_eq!(error.code, ErrorCode::Io);
    assert_eq!(client.kill_calls(), 0, "the daemon must not be stopped");
}

#[test]
fn a_status_read_that_failed_is_not_restarted() {
    let client = FakeRestartClient::idle().with_an_unreadable_status();

    let error = restart(&client).expect_err("the roster cannot see a child still starting");

    assert_eq!(
        error.message,
        "could not check for running agents and terminals (timed out: waiting for a daemon \
         reply), so the daemon was not restarted."
    );
    assert_eq!(error.code, ErrorCode::Io);
    assert_eq!(client.kill_calls(), 0, "the daemon must not be stopped");
}

#[test]
fn an_ended_and_a_recovered_transcript_are_not_a_live_session() {
    let client = FakeRestartClient::holding(vec![
        session(
            "s.1",
            SessionKind::Terminal,
            SessionState::Ended {
                generation: 1,
                code: Some(0),
                integrity: TranscriptIntegrity::Complete,
            },
        ),
        session(
            "s.2",
            SessionKind::Claude,
            SessionState::Recovered {
                generation: 9,
                integrity: TranscriptIntegrity::Unverifiable {
                    dropped_frames: 0,
                    dropped_bytes: 0,
                    trimmed_bytes: 0,
                },
            },
        ),
    ]);

    restart(&client).expect("no session holds a process");

    assert_eq!(client.kill_calls(), 1, "the daemon is stopped as before");
}

#[test]
fn an_error_from_a_daemon_that_is_there_is_not_restarted() {
    let client = FakeRestartClient::roster_answering(Err(DaemonError::Handshake(WireError::new(
        ErrorCode::Internal,
        "session state is unavailable",
    ))));

    let error = restart(&client).expect_err("an unreadable roster is not an empty one");

    assert_eq!(
        error.message,
        "could not check for running agents and terminals (session state is unavailable), so the \
         daemon was not restarted."
    );
    assert_eq!(error.code, ErrorCode::Io);
    assert_eq!(client.kill_calls(), 0, "the daemon must not be stopped");
}

#[test]
fn a_roster_read_that_timed_out_is_not_restarted() {
    let client = FakeRestartClient::roster_answering(Err(DaemonError::TimedOut(
        "waiting for a daemon reply".to_string(),
    )));

    let error = restart(&client).expect_err("a silent daemon is not an empty one");

    assert_eq!(
        error.message,
        "could not check for running agents and terminals (timed out: waiting for a daemon reply), \
         so the daemon was not restarted."
    );
    assert_eq!(client.kill_calls(), 0, "the daemon must not be stopped");
}

#[test]
fn a_kill_that_fails_reports_the_daemons_own_answer() {
    let client = FakeRestartClient::idle().with_a_failing_kill();

    let error = restart(&client).expect_err("the kill's own failure is the answer");

    assert_eq!(error.code, ErrorCode::Internal);
    assert_eq!(
        error.message,
        "the connected daemon is no longer that process"
    );
}
