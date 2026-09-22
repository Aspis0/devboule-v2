//! Tests for the daemon-side request trace on a live connection: the arrival
//! line, its place ahead of the queue (and therefore of any dispatch), and
//! its shape on disk.
//!
//! The fake connection is a real named pipe: the reader path under test is
//! the production `framed.recv`, and a closing client gives it a real EOF.

use super::*;
use crate::rpc_trace::tests::{read_daemon_log, scratch, trace_on};
use std::thread;

const SENTINEL: &str = "TRACE-SENTINEL-9c21 the prompt body must not be logged";

/// The arrival line exists by the moment the request reaches the queue —
/// which is the only place it can stand *before* a dispatch — it carries the
/// command's name, and it carries no payload.
///
/// Mutant: the `arrival` record in `read_client_requests` removed — there is
/// no daemon log at all, and this test dies reading it.
#[cfg(windows)]
#[test]
fn arrival_is_on_disk_before_the_request_reaches_the_queue_and_names_nothing_else() {
    let dir = scratch("arrival");
    let _env = trace_on(&dir);
    let paths = crate::paths::RuntimePaths::from_dir(&dir);
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let mut listener =
        crate::transport::NamedPipeListener::bind(&paths, std::sync::Arc::clone(&stop))
            .expect("bind");

    let message = ClientMessage::AgentMessageSend {
        id: 7,
        from_session: "session-a".to_string(),
        to_session: "session-b".to_string(),
        text: SENTINEL.to_string(),
        idempotency_key: None,
    };
    let wire = serde_json::to_string(&message).expect("serialize") + "\n";

    let (accepted_tx, accepted_rx) = mpsc::channel();
    let acceptor = thread::spawn(move || {
        let file = listener.accept().expect("accept");
        accepted_tx.send(file).expect("hand the connection over");
    });
    let client = thread::spawn(move || {
        let mut file = crate::transport::connect_pipe(&paths.pipe_name).expect("connect");
        std::io::Write::write_all(&mut file, wire.as_bytes()).expect("write request");
        // The drop closes the pipe: the reader sees EOF and stops.
    });

    let file = accepted_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("accepted connection");
    acceptor.join().expect("acceptor thread");
    let framed = Framed::new(file);
    let (inbox_tx, inbox_rx) = mpsc::sync_channel(64);
    let reader = thread::spawn(move || {
        read_client_requests(framed, inbox_tx, ConnOut::new(), 123456789);
    });

    // At the moment the serving loop would take the request, the arrival
    // line is already on disk — and no dispatch marker can predate it,
    // because this harness never dispatches.
    let queued = inbox_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("a queued request");
    let queued = queued.expect("the frame parses");
    assert!(matches!(queued, ClientMessage::AgentMessageSend { .. }));

    let log = read_daemon_log(&dir);
    // The sink is process-wide: a daemon spawned by another test inherits
    // the variable and would append here too. Only this conn id is ours —
    // and it has exactly one line: the arrival. If a dispatch marker for
    // this conn existed, the count would not be 1.
    let mine: Vec<&str> = log
        .lines()
        .filter(|line| line.contains(" conn=123456789"))
        .collect();
    assert_eq!(mine.len(), 1, "one arrival for this conn: {log}");
    let line = mine[0];
    let tokens: Vec<&str> = line.split_whitespace().collect();
    assert_eq!(tokens.first().copied(), Some("rpc"), "{line}");
    let parsed: Vec<(&str, &str)> = tokens[1..]
        .iter()
        .map(|token| token.split_once('=').expect("key=value token"))
        .collect();
    let keys: Vec<&str> = parsed.iter().map(|(key, _)| *key).collect();
    assert_eq!(keys, ["side", "event", "t", "name", "id", "conn"], "{line}");
    assert_eq!(parsed[0].1, "daemon");
    assert_eq!(parsed[1].1, "arrival");
    parsed[2].1.parse::<u64>().expect("t is epoch ms");
    assert_eq!(parsed[3].1, "AgentMessageSend");
    assert_eq!(parsed[4].1, "7");
    assert_eq!(parsed[5].1, "123456789");

    // Neither the prompt's text nor its sessions reach the line.
    assert!(!log.contains("TRACE-SENTINEL"), "{log}");
    assert!(!log.contains("session-a"), "{log}");

    reader.join().expect("reader thread");
    client.join().expect("client thread");
    drop(stop);
    let _ = std::fs::remove_dir_all(&dir);
}
