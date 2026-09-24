//! Tests for the forward leg, kept out of the production file: the fake app,
//! the record fixtures and the session scaffolding are test-only weight.
//!
//! Every sentence is compared verbatim: the phrases are the contract an agent
//! reads, and a paraphrase is a broken promise.

use super::*;

use crate::oracle_app_record::OracleAppRecord;
use crate::server::ServerState;
use devboule_protocol::WorkspaceIsolation;
use std::fs::OpenOptions;
use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{self, Receiver};
use std::sync::Arc;
use std::time::SystemTime;

pub(super) const SESSION: &str = "oracle-a";

/// The decided sentences, written out. The tests pin the words themselves,
/// never the production constants: a mutation of a constant must still kill
/// the test that promises it.
pub(super) const APP: &str = "The Oracle semantic search lives in the Devboule desktop app: open the app and retry. The project graph tools do not need it.";
const TIMEOUT: &str = "The Devboule desktop app did not answer within 25 seconds: it may still be loading its model. Wait a moment and retry.";
const SKEW: &str = "The Devboule app answering this daemon does not speak this tool yet: update the app and retry.";
const NO_WORKSPACE: &str = "This session has no workspace, so there is no project to search. Start the session in a project folder and retry.";
const CAP: &str = "The Devboule app's answer exceeded the 1 MiB limit and was refused.";

pub(super) fn owner() -> OwnerId {
    OwnerId::new("S-1-5-21-oracle-forward", "claude").expect("owner")
}

fn temp_dir(tag: &str) -> PathBuf {
    crate::test_dirs::test_temp_dir(&format!("devboule-oracle-forward-{tag}"))
}

pub(super) fn add_session(state: &Arc<ServerState>, session: &str, root: &Path) {
    let project = state
        .sessions
        .project_add(root.to_str().expect("project path"))
        .expect("project row");
    let workspace = state
        .sessions
        .workspace_create(&project.id, WorkspaceIsolation::Local, None)
        .expect("workspace row");
    crate::session::insert_test_live_agent_in_workspace(
        &state.sessions,
        session,
        owner(),
        &workspace.id,
    );
}

/// One project folder and one session whose own workspace is that folder:
/// the state every forward test starts from.
pub(super) fn state_with_session(tag: &str) -> (PathBuf, Arc<ServerState>) {
    let root = temp_dir(tag).join("Project");
    std::fs::create_dir_all(&root).expect("project folder");
    let state = ServerState::new(format!("oracle-forward-{tag}"));
    add_session(&state, SESSION, &root);
    (root, state)
}

/// Publish a record pointing at `port`, the way the app writes it. `ready`
/// is the bind: `false` is a body that reached disk before the listener did.
pub(super) fn publish(state: &ServerState, port: u16, ready: bool) -> PathBuf {
    let path = oracle_app_lock_path(&RuntimePaths::from_dir(state.sessions.runtime_dir()));
    let mut record = OracleAppRecord::new(4321, "instance-oracle", port, "bearer-oracle");
    if ready {
        record.listening();
    }
    std::fs::write(&path, record.body()).expect("record");
    path
}

pub(super) fn read_request(stream: &mut TcpStream) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut chunk = [0u8; 4096];
    let header_end = loop {
        let count = stream.read(&mut chunk).expect("request head");
        if count == 0 {
            return bytes;
        }
        bytes.extend_from_slice(&chunk[..count]);
        if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
    };
    let head = String::from_utf8_lossy(&bytes[..header_end]).to_string();
    let length = head
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.trim().eq_ignore_ascii_case("content-length"))
        .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        .unwrap_or(0);
    while bytes.len() < header_end + length {
        let count = stream.read(&mut chunk).expect("request body");
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&chunk[..count]);
    }
    bytes
}

/// A one-shot loopback app: answers `status` + `body`, and hands the
/// forward's own request back so a test can assert what was sent.
fn fake_app(status: &str, body: &str) -> (u16, Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("fake app bind");
    let port = listener.local_addr().expect("fake app addr").port();
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let request = read_request(&mut stream);
        let _ = tx.send(String::from_utf8_lossy(&request).to_string());
        let _ = stream.write_all(response.as_bytes());
    });
    (port, rx)
}

/// An app that stays reachable but never answers: the port accepts the
/// connection into its backlog and no thread ever replies.
fn mute_app() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mute app bind");
    let port = listener.local_addr().expect("mute app addr").port();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(120));
        drop(listener);
    });
    port
}

/// A port nothing listens on: the record is fresh, the connection is refused.
fn dead_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    port
}

pub(super) fn call(state: &ServerState, arguments: Value) -> Result<Value, GraphError> {
    search_within(
        state,
        SESSION,
        &owner(),
        &arguments,
        Duration::from_secs(10),
    )
}

pub(super) fn assert_refused(result: Result<Value, GraphError>, phrase: &str, label: &str) {
    match result {
        Err(GraphError::Refused(message)) => assert_eq!(message, phrase, "{label}"),
        Err(GraphError::Invalid(message)) => panic!("{label}: a parameter refusal: {message}"),
        Ok(document) => panic!("{label}: answered instead of refusing: {document}"),
    }
}

#[test]
fn no_record_answers_the_app_phrase() {
    let (_root, state) = state_with_session("t1");
    assert_refused(call(&state, json!({"query": "where is main"})), APP, "T1");
}

#[test]
fn a_121_second_old_record_answers_the_app_phrase() {
    let (_root, state) = state_with_session("t2");
    // The app behind the record would answer `ok:true`: if the stale gate
    // ever lets this record through, the call succeeds and the test dies.
    let body = json!({"ok": true, "query": "q", "results": []}).to_string();
    let (port, _rx) = fake_app("200 OK", &body);
    let path = publish(&state, port, true);
    let file = OpenOptions::new().write(true).open(&path).expect("open");
    file.set_modified(SystemTime::now() - Duration::from_secs(121))
        .expect("age");
    assert_refused(call(&state, json!({"query": "q"})), APP, "T2");
}

#[test]
fn a_live_record_that_never_bound_answers_the_app_phrase() {
    let (_root, state) = state_with_session("t3");
    // Same construction as T2: a working app behind an unbound record, so a
    // verdict that trusts `is_live` fetches an answer and dies here.
    let body = json!({"ok": true, "query": "q", "results": []}).to_string();
    let (port, _rx) = fake_app("200 OK", &body);
    publish(&state, port, false);
    assert_refused(call(&state, json!({"query": "q"})), APP, "T3");
}

#[test]
fn a_wrong_token_answers_the_app_phrase() {
    let (_root, state) = state_with_session("t4");
    let (port, _rx) = fake_app("401 Unauthorized", r#"{"error":"unauthorized"}"#);
    publish(&state, port, true);
    assert_refused(call(&state, json!({"query": "q"})), APP, "T4");
}

#[test]
fn a_fresh_record_pointing_at_a_dead_port_answers_the_app_phrase() {
    let (_root, state) = state_with_session("t5");
    publish(&state, dead_port(), true);
    assert_refused(call(&state, json!({"query": "q"})), APP, "T5");
}

#[test]
fn an_app_that_never_answers_falls_into_the_timeout_phrase() {
    let (_root, state) = state_with_session("t6");
    publish(&state, mute_app(), true);
    let result = search_within(
        &state,
        SESSION,
        &owner(),
        &json!({"query": "q"}),
        Duration::from_millis(300),
    );
    assert_refused(result, TIMEOUT, "T6");
    // Source pins: production stays at 25 s — the sentence promises it, and
    // pi's own bridge timeout (30 s) stays the tightest client in the room.
    let source = include_str!("oracle_forward.rs");
    assert!(
        source.contains("const FORWARD_TIMEOUT: Duration = Duration::from_secs(25);"),
        "the production timeout is 25 s: {source}"
    );
    assert!(TIMEOUT.contains("within 25 seconds"));
    let bridge = include_str!("pi_client.rs");
    assert!(
        bridge.contains("MCP_TIMEOUT_MS = 30000"),
        "pi stays at 30 s"
    );
    assert!(FORWARD_TIMEOUT < Duration::from_millis(30_000));
}

#[test]
fn the_index_refusal_reaches_the_agent_verbatim() {
    let (_root, state) = state_with_session("t7");
    let message = "The folder P has no Oracle index. Index this folder before searching it; Oracle will not answer from another folder's index.";
    let body = json!({"ok": false, "reason": "no_index", "message": message}).to_string();
    let (port, _rx) = fake_app("200 OK", &body);
    publish(&state, port, true);
    assert_refused(call(&state, json!({"query": "q"})), message, "T7");
}

#[test]
fn the_warming_refusal_is_forwarded_and_is_never_the_app_phrase() {
    let (_root, state) = state_with_session("t8");
    let warming = "Oracle's embedding model is still loading in the background. Retry this query in a few seconds.";
    let body = json!({"ok": false, "reason": "warming", "message": warming}).to_string();
    let (port, _rx) = fake_app("200 OK", &body);
    publish(&state, port, true);
    match call(&state, json!({"query": "q"})) {
        Err(GraphError::Refused(message)) => {
            assert_eq!(message, warming, "the app's sentence verbatim");
            assert_ne!(message, APP, "an open app never reads as closed");
        }
        Err(GraphError::Invalid(message)) => panic!("a parameter refusal: {message}"),
        Ok(document) => panic!("answered instead of refusing: {document}"),
    }
}

#[test]
fn the_success_body_reaches_the_reply_unchanged() {
    let (_root, state) = state_with_session("t9");
    let body = json!({
        "ok": true,
        "query": "where is main",
        "results": [{"path": "src/main.rs", "line_start": 1, "line_end": 40,
                     "snippet": "fn main()", "score": 0.015625, "match_type": "dense+reranked"}],
    })
    .to_string();
    let (port, _rx) = fake_app("200 OK", &body);
    publish(&state, port, true);
    let answer = call(&state, json!({"query": "where is main", "limit": 3})).expect("answer");
    let expected: Value = serde_json::from_str(&body).expect("body");
    assert_eq!(answer, expected, "the document is the app's body, whole");
}

#[test]
fn the_call_carries_the_sessions_own_root_and_no_argument_can_name_one() {
    let (_root_a, state) = state_with_session("t10a");
    let root_b = temp_dir("t10b").join("ProjectB");
    std::fs::create_dir_all(&root_b).expect("project folder b");
    add_session(&state, "oracle-b", &root_b);

    let body = json!({"ok": true, "query": "q", "results": []}).to_string();
    let (port, rx) = fake_app("200 OK", &body);
    publish(&state, port, true);
    let answer = call(&state, json!({"query": "q"})).expect("answer");
    assert_eq!(answer["ok"], true);

    let request = rx.recv_timeout(Duration::from_secs(5)).expect("request");
    let sent: Value = serde_json::from_str(request.split_once("\r\n\r\n").expect("HTTP body").1)
        .expect("forward body");
    // The root in the spelling the daemon hands its children (plain — the
    // stored journal form stays verbatim); the app builds its paths from it
    // with `from_root_without_env`, like the graph tools open their store.
    let expected_root = state
        .sessions
        .session_workspace_root(SESSION, &owner())
        .expect("root lookup")
        .expect("A has a workspace");
    let other_root = state
        .sessions
        .session_workspace_root("oracle-b", &owner())
        .expect("root lookup b")
        .expect("B has a workspace");
    assert_eq!(
        sent["root"],
        json!(expected_root.to_string_lossy()),
        "A's own root"
    );
    assert_ne!(
        sent["root"],
        json!(other_root.to_string_lossy()),
        "never B's"
    );
    assert_eq!(sent["query"], "q");
    assert_eq!(sent["limit"], 10, "the default the schema promises");

    // And no argument can name a root: the key is unknown, so it is refused
    // before any record is read, let alone any folder searched.
    match call(
        &state,
        json!({"query": "q", "root": root_b.to_string_lossy()}),
    ) {
        Err(GraphError::Invalid(message)) => assert!(
            message.contains("unknown parameter 'root'"),
            "root is not an argument: {message}"
        ),
        other => panic!("a root argument is a parameter refusal, got {other:?}"),
    }
}

#[test]
fn a_session_without_a_workspace_is_refused_with_the_workspace_phrase() {
    let state = ServerState::new("oracle-forward-t11".to_string());
    crate::session::insert_test_live_agent(&state.sessions, SESSION, owner());
    // An app is published and would answer — the refusal happens before the
    // forward leg runs, so that answer is never fetched.
    let body = json!({"ok": true, "query": "q", "results": []}).to_string();
    let (port, _rx) = fake_app("200 OK", &body);
    publish(&state, port, true);
    assert_refused(call(&state, json!({"query": "q"})), NO_WORKSPACE, "T11");
}

#[test]
fn bad_arguments_are_invalid_not_forwarded() {
    let (_root, state) = state_with_session("t12");
    // No record is published: a parameter refusal must happen first.
    for (arguments, sentence) in [
        (json!({}), "query is required"),
        (
            json!({"query": "q", "depth": 2}),
            "unknown parameter 'depth'",
        ),
        (
            json!({"query": "a".repeat(MAX_QUERY_CHARS + 1)}),
            "query is longer than 4096 characters",
        ),
        (json!([]), "arguments must be an object"),
    ] {
        match call(&state, arguments.clone()) {
            Err(GraphError::Invalid(message)) => assert!(
                message.contains(sentence),
                "{arguments}: {message} does not say {sentence}"
            ),
            other => panic!("{arguments}: -32602 expected, got {other:?}"),
        }
    }
}

#[test]
fn an_oversized_answer_is_refused_with_its_own_sentence() {
    let (_root, state) = state_with_session("t15");
    let oversized = format!(
        r#"{{"ok":true,"query":"q","pad":"{}"}}"#,
        "a".repeat(MAX_FORWARD_BODY_BYTES)
    );
    let (port, _rx) = fake_app("200 OK", &oversized);
    publish(&state, port, true);
    assert_refused(call(&state, json!({"query": "q"})), CAP, "T15");
}

#[test]
fn a_mixed_version_answer_gets_the_skew_phrase_and_never_the_app_phrase() {
    let (_root, state) = state_with_session("t16");
    for (index, (status, body)) in [
        ("404 Not Found", r#"{"error":"not found"}"#),
        ("200 OK", r#"{"ok":false,"reason":"not_implemented"}"#),
        ("200 OK", "this is not json"),
    ]
    .iter()
    .enumerate()
    {
        let (port, _rx) = fake_app(status, body);
        publish(&state, port, true);
        assert_refused(
            call(&state, json!({"query": "q"})),
            SKEW,
            &format!("T16-{index}"),
        );
    }
    // A `400` is the app answering: its own sentence travels, not the skew
    // one and never the app phrase.
    let body = json!({"error": "bad request", "message": "the app's own sentence"}).to_string();
    let (port, _rx) = fake_app("400 Bad Request", &body);
    publish(&state, port, true);
    assert_refused(
        call(&state, json!({"query": "q"})),
        "the app's own sentence",
        "T16-400",
    );
}
