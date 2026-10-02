//! The guard's roads after its first read was lost: one more connection is
//! asked, and only the daemon that connection reached may be stopped.

use std::io;
use std::sync::atomic::Ordering;
use std::time::Duration;

use devboule_daemon::DaemonError;
use devboule_protocol::ErrorCode;

use super::fake::{live, no_daemon_at_all, pipe_is_busy, FakeRestartClient};
use super::*;

#[test]
fn a_dropped_connection_refuses_what_the_second_one_finds() {
    let second = FakeRestartClient::holding(vec![live("s.1", 1)]);
    let second_kills = second.kill_counter();
    let client =
        FakeRestartClient::roster_answering(Err(DaemonError::ConnectionLost)).answered_by(second);

    let error = restart(&client).expect_err("a live session behind a dropped pipe still refuses");

    assert_eq!(
        error.message,
        "1 agent or terminal is running; restarting the daemon would stop it. \
         Stop it first, then restart."
    );
    assert_eq!(
        client.kill_calls(),
        0,
        "the first connection is not stopped"
    );
    assert_eq!(second_kills.load(Ordering::SeqCst), 0, "nor is the second");
}

#[test]
fn a_dropped_connection_that_finds_no_daemon_kills_nothing() {
    let client = FakeRestartClient::roster_answering(Err(DaemonError::ConnectionLost))
        .with_the_second_connection_failing(no_daemon_at_all());

    restart(&client).expect("no daemon is there to protect, and none to stop");

    assert_eq!(
        client.kill_calls(),
        0,
        "a roster nobody read must not authorise a kill"
    );
}

#[test]
fn a_daemon_that_was_asked_to_stop_is_not_reported_as_restarted() {
    let client = FakeRestartClient::roster_answering(Err(DaemonError::ConnectionLost))
        .with_the_second_connection_failing(no_daemon_at_all())
        .with_a_declared_exit();

    let error = restart(&client)
        .expect_err("the supervisor will not start a daemon that was asked to stop");

    assert_eq!(
        error.message,
        "The daemon is shutting down, so it was not restarted."
    );
    assert_eq!(error.code, ErrorCode::Io);
    assert_eq!(client.kill_calls(), 0, "nothing is stopped");
}

#[test]
fn the_second_connection_waits_only_a_short_while_on_a_busy_pipe() {
    let client = FakeRestartClient::roster_answering(Err(DaemonError::ConnectionLost))
        .with_the_second_connection_failing(no_daemon_at_all());

    restart(&client).expect("no daemon is there");

    let budgets = client.probe_budgets();
    assert_eq!(budgets.len(), 1, "one probe, asked once");
    assert!(
        budgets[0] <= Duration::from_secs(3),
        "the probe must not hold a worker for the pipe's full wait: {:?}",
        budgets[0]
    );
}

#[test]
fn a_second_connection_that_drops_too_is_not_a_reason_to_kill() {
    let second = FakeRestartClient::roster_answering(Err(DaemonError::ConnectionLost));
    let second_kills = second.kill_counter();
    let client =
        FakeRestartClient::roster_answering(Err(DaemonError::ConnectionLost)).answered_by(second);

    let error = restart(&client).expect_err("two dropped reads are no roster");

    assert_eq!(error.code, ErrorCode::Io);
    assert_eq!(
        client.kill_calls(),
        0,
        "the first connection is not stopped"
    );
    assert_eq!(second_kills.load(Ordering::SeqCst), 0, "nor is the second");
}

#[test]
fn a_second_connection_that_failed_another_way_is_not_restarted() {
    let client = FakeRestartClient::roster_answering(Err(DaemonError::ConnectionLost))
        .with_the_second_connection_failing(pipe_is_busy());

    let error = restart(&client).expect_err("a busy pipe is not an absent daemon");

    assert_eq!(
        error.message,
        "could not check for running agents and terminals (named pipe is busy), so the daemon was \
         not restarted."
    );
    assert_eq!(error.code, ErrorCode::Io);
    assert_eq!(client.kill_calls(), 0, "the daemon must not be stopped");
}

#[test]
fn a_second_connection_that_finds_nothing_live_is_the_one_that_stops_its_daemon() {
    let second = FakeRestartClient::idle();
    let second_kills = second.kill_counter();
    let client = FakeRestartClient::roster_answering(Err(DaemonError::Io(io::Error::from(
        io::ErrorKind::BrokenPipe,
    ))))
    .answered_by(second);

    restart(&client).expect("nothing running on the daemon that is there");

    assert_eq!(
        second_kills.load(Ordering::SeqCst),
        1,
        "the daemon whose roster was read is the daemon stopped"
    );
    assert_eq!(
        client.kill_calls(),
        0,
        "the first connection read nothing and stops nothing"
    );
}
