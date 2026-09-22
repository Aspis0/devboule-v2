//! Tests for the phase after the head: the app has already answered (status
//! and headers arrived) and the body then goes wrong — cut off mid-answer, or
//! never sent at all. Kept out of both sibling files to respect the size rule.

use super::tests::{
    assert_refused, call, owner, publish, read_request, state_with_session, SESSION,
};
use super::*;

use std::io::Write;
use std::net::TcpListener;

/// The decided sentences, written out: the tests pin the words themselves,
/// never the production constants (a constant mutation must still kill the
/// test that promises it).
const TIMEOUT: &str = "The Devboule desktop app did not answer within 25 seconds: it may still be loading its model. Wait a moment and retry.";
const CUT: &str = "The Devboule desktop app answered this search but the connection was cut before the answer could be read. Check that the app is still running and retry.";

/// An app that answers the head (`Content-Length: 4096`), sends a few body
/// bytes and closes the socket: the connection is cut mid-answer.
fn app_cut_mid_body() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("cut app bind");
    let port = listener.local_addr().expect("cut app addr").port();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let _ = read_request(&mut stream);
        let head = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 4096\r\nConnection: close\r\n\r\n";
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.write_all(br#"{"ok":tr"#);
        // the stream drops here: 8 of the promised 4096 bytes, then close.
    });
    port
}

/// An app that answers the head and then holds the socket open in silence:
/// only the client's own deadline can end this body read.
fn app_head_then_silence() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("silent app bind");
    let port = listener.local_addr().expect("silent app addr").port();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let _ = read_request(&mut stream);
        let head = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 4096\r\nConnection: close\r\n\r\n";
        let _ = stream.write_all(head.as_bytes());
        std::thread::sleep(Duration::from_secs(120));
        drop(stream);
    });
    port
}

#[test]
fn a_body_cut_mid_answer_gets_the_cut_phrase() {
    let (_root, state) = state_with_session("t17");
    publish(&state, app_cut_mid_body(), true);
    // If a cut ever reads as a short body instead, `answer` parses the
    // truncated document and this test dies on the skew phrase.
    assert_refused(call(&state, json!({"query": "q"})), CUT, "T17");
}

#[test]
fn an_app_that_answers_the_head_then_stays_silent_gets_the_timeout_phrase() {
    let (_root, state) = state_with_session("t18");
    publish(&state, app_head_then_silence(), true);
    let result = search_within(
        &state,
        SESSION,
        &owner(),
        &json!({"query": "q"}),
        Duration::from_millis(300),
    );
    // The head is in, so the deadline hits the body read and the downcast in
    // `body_timed_out` is what must recognise it — not the `send` arm.
    assert_refused(result, TIMEOUT, "T18");
}
