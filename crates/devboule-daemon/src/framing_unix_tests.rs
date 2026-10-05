//! The Unix socket carries a duplex conversation: a reader parked waiting for
//! the peer must not stop the same connection from sending, a read deadline
//! must be honoured, and a parked reader must be wakeable. Each wait here is
//! bounded, so a regression fails with a message instead of hanging.

use std::fs::File;
use std::os::unix::net::UnixStream;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::Framed;
use crate::error::DaemonError;

const BOUND: Duration = Duration::from_secs(5);

fn connected_pair() -> (Framed, Framed) {
    let (left, right) = UnixStream::pair().expect("a socket pair");
    (
        Framed::new(File::from(std::os::fd::OwnedFd::from(left))),
        Framed::new(File::from(std::os::fd::OwnedFd::from(right))),
    )
}

/// Run `work` on its own thread and hand back its result, or `None` when it
/// did not finish within [`BOUND`]. A thread that missed the bound stays
/// parked; the test fails and the process ends it.
fn finishes<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> Option<T> {
    let (done, answer) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = done.send(work());
    });
    answer.recv_timeout(BOUND).ok()
}

#[test]
fn a_parked_read_does_not_block_a_send_on_the_same_connection() {
    let (client, server) = connected_pair();
    let reader = client.clone();
    let parked = std::thread::spawn(move || reader.recv::<Value>());
    // Long enough for the reader thread to be inside its blocking read.
    std::thread::sleep(Duration::from_millis(200));

    let sender = client.clone();
    let sent = finishes(move || sender.send(&json!({"type": "ping"})));
    assert!(
        matches!(sent, Some(Ok(()))),
        "a send behind a parked read never completed: {sent:?}"
    );
    let request: Value = server
        .recv_timeout(BOUND)
        .expect("the peer got the request");
    assert_eq!(request["type"], "ping");

    server
        .send(&json!({"type": "pong"}))
        .expect("the peer answers");
    let reply = finishes(move || parked.join().expect("the reader thread"));
    assert!(
        matches!(reply, Some(Ok(ref value)) if value["type"] == "pong"),
        "the parked reader never saw the answer: {reply:?}"
    );
}

#[test]
fn a_read_deadline_expires_on_a_silent_peer() {
    let (client, _silent) = connected_pair();
    let started = Instant::now();
    let outcome = client.recv_timeout::<Value>(Duration::from_millis(150));
    assert!(
        matches!(outcome, Err(DaemonError::TimedOut(_))),
        "a silent peer must time the read out: {outcome:?}"
    );
    assert!(
        started.elapsed() < BOUND,
        "the deadline was not honoured: {:?}",
        started.elapsed()
    );
}

#[test]
fn cancel_read_wakes_a_parked_reader() {
    let (server, _peer) = connected_pair();
    let reader = server.clone();
    let parked = std::thread::spawn(move || reader.recv::<Value>());
    std::thread::sleep(Duration::from_millis(200));

    server.cancel_read();
    let woken = finishes(move || parked.join().expect("the reader thread"));
    assert!(
        matches!(woken, Some(Err(_))),
        "the parked reader was not woken: {woken:?}"
    );
}
