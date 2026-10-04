//! The host's own rules, checked without a connection: what a stop says, and
//! the one line of its source that this slice exists to protect.

use super::*;

/// Why a loop stopped, in the words the log line carries. The daemon's own
/// refusal for a host that is registered and reads nothing is `browser_busy`,
/// which reads like an app that is working; these three say which of the three
/// endings it was.
#[test]
fn every_stop_says_why_it_stopped() {
    let endings = [Stop::ConnectionEnded, Stop::AnswerRefused, Stop::QueueTaken];
    let said: Vec<&str> = endings.iter().map(|stop| stop.why()).collect();
    assert_eq!(said.len(), 3);
    assert!(said[0].contains("connection"), "{}", said[0]);
    assert!(said[1].contains("answer"), "{}", said[1]);
    assert!(said[2].contains("queue"), "{}", said[2]);
}

/// The serving loop reads with no clock on it.
///
/// This is the whole fix, pinned in the source because nothing else here can
/// see it: a timed read that gives up on an idle ends the loop, the request
/// receiver goes with it, and the app keeps answering commands for minutes
/// with a queue nothing reads — which the daemon refuses at once as
/// `browser_busy`. A wire test cannot wait out the window that broke it, so
/// this reads the call instead. `host_wire_tests.rs` is what proves the loop
/// still answers.
#[test]
fn the_serving_loop_reads_until_the_connection_ends() {
    let host = include_str!("host.rs");
    let loop_body = host
        .split_once("fn serve_connection")
        .expect("the serving loop is in host.rs")
        .1
        .split_once("/// One command")
        .expect("the loop ends before the answer helper")
        .0;

    assert!(
        loop_body.contains("requests.recv()"),
        "the loop waits on the connection's own end:\n{loop_body}"
    );
    assert!(
        !loop_body.contains("recv_timeout"),
        "a timed read ends the loop on an idle, and the request queue with it:\n{loop_body}"
    );
}
