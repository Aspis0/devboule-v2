//! Tests for the Claude client: stream ingestion, task envelopes and resume.

use super::*;
use crate::claude_catalog::ClaudeCatalogSnapshot;
use crate::raster_metadata::{clean_png, png_with_text_chunk, vector_input, vector_output};
use crate::session::{ConnHandle, PendingEvent, StaticImageSink};
use devboule_protocol::PermissionOutcome;
use devboule_protocol::PromptAttachment;
use devboule_protocol::SessionModelEffort;
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout};

const CLAUDE_MODE_CAPTURE: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/fixtures/wire/claude-set-mode.jsonl"
));

fn drain(conn: &ConnHandle) -> Vec<SessionEvent> {
    let mut events = Vec::new();
    loop {
        let batch = conn.pull_events();
        if batch.is_empty() {
            return events;
        }
        for event in &batch {
            conn.event_sent(event);
        }
        events.extend(
            batch
                .into_iter()
                .map(|pending: PendingEvent| pending.envelope.event),
        );
    }
}

fn attached(broker: &Arc<PermissionBroker>) -> (Arc<SessionRuntime>, Arc<ConnHandle>) {
    let runtime = SessionRuntime::for_acp("s.claude.test".to_string(), None, Arc::clone(broker));
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach");
    conn.track_with_agent_replay(
        "s.claude.test",
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    (runtime, conn)
}

#[test]
fn resume_slug_matches_the_directories_the_cli_lays_out() {
    // Measured against `%USERPROFILE%\.claude\projects` on this machine:
    // the two killed sessions tonight sat under exactly these slugs.
    assert_eq!(
        claude_projects_slug(Path::new(
            r"C:\Users\gualt\Desktop\New devboule\devboule-v2"
        )),
        "C--Users-gualt-Desktop-New-devboule-devboule-v2"
    );
    assert_eq!(
        claude_projects_slug(Path::new(r"C:\Users\gualt\Desktop")),
        "C--Users-gualt-Desktop"
    );
}

#[test]
fn resume_argv_adds_the_flag_and_keeps_the_launch() {
    let base = vec!["-p".to_string(), "--verbose".to_string()];
    assert_eq!(
        push_resume_flag(base, "peer-1"),
        vec!["-p", "--verbose", "--resume", "peer-1"]
    );
}

#[test]
fn history_lookup_finds_the_conversation_and_refuses_traversal() {
    let home = crate::test_dirs::test_temp_dir("devboule-claude-hist");
    let cwd = Path::new(r"C:\work\shop");
    let dir = home
        .join(".claude")
        .join("projects")
        .join(claude_projects_slug(cwd));
    std::fs::create_dir_all(&dir).expect("history dir");
    std::fs::write(dir.join("peer-1.jsonl"), "{}\n").expect("history file");
    assert!(matches!(
        find_claude_history(&home, cwd, "peer-1"),
        ClaudeHistoryLookup::Found
    ));
    assert!(matches!(
        find_claude_history(&home, cwd, "missing"),
        ClaudeHistoryLookup::Absent
    ));
    assert!(matches!(
        find_claude_history(&home, cwd, "../evil"),
        ClaudeHistoryLookup::Absent
    ));
    assert!(matches!(
        find_claude_history(&home, cwd, "a/b"),
        ClaudeHistoryLookup::Absent
    ));
    // A Windows prefix discards the whole base under `Path::join`, with
    // none of the tokens above present — so it refuses at find level too.
    assert!(matches!(
        find_claude_history(&home, cwd, "C:evil"),
        ClaudeHistoryLookup::Absent
    ));
    // A conversation filed under another slug still proves the id exists:
    // the CLI resolves `--resume` by id, and our slug rule for an exotic
    // cwd may be the thing that is wrong.
    let elsewhere = home.join(".claude").join("projects").join("C--elsewhere");
    std::fs::create_dir_all(&elsewhere).expect("sibling dir");
    std::fs::write(elsewhere.join("peer-9.jsonl"), "{}\n").expect("sibling file");
    assert!(matches!(
        find_claude_history(&home, cwd, "peer-9"),
        ClaudeHistoryLookup::Found
    ));
    // A directory the lookup cannot read is NOT absence — that is the
    // whole point of the third state: "could not look" concludes nothing.
    // Destructive, so it runs last.
    std::fs::remove_dir_all(home.join(".claude").join("projects")).expect("drop projects");
    std::fs::write(home.join(".claude").join("projects"), "not a directory")
        .expect("block the lookup");
    assert!(matches!(
        find_claude_history(&home, cwd, "peer-1"),
        ClaudeHistoryLookup::Unreadable(_)
    ));
    let _ = std::fs::remove_dir_all(&home);
}

#[test]
fn missing_history_refusal_names_the_peer_and_the_expected_path() {
    let error = missing_history_error("peer-9", Path::new(r"C:\work"), Path::new(r"C:\Users\me"));
    // The internal disown sentinel: the resume arm rewrites it to the
    // wire's `InvalidRequest`; the sentence is what the app has always
    // read.
    assert_eq!(error.code, ErrorCode::SessionNotFound);
    assert!(
        error.message.contains("peer-9"),
        "the refusal names the conversation: {}",
        error.message
    );
    assert!(
        error.message.contains("deleted") || error.message.contains("rotated"),
        "the refusal says what happened to the file: {}",
        error.message
    );
}

#[test]
fn peer_session_id_is_a_closed_alphabet() {
    // Provider UUIDs and the stub's ids pass; anything that could steer a
    // `Path::join` — separators, dot-dot, Windows prefixes — refuses.
    for ok in [
        "550e8400-e29b-41d4-a716-446655440000",
        "stub-peer-1",
        "peer-1",
    ] {
        assert!(valid_peer_session_id(ok), "{ok} must pass");
    }
    for evil in [
        "",
        "C:evil",
        r"C:\evil",
        r"\\server\share\x",
        "../evil",
        "a/b",
        r"a\b",
        "..",
        "peer 1",
        "peer.jsonl",
    ] {
        assert!(!valid_peer_session_id(evil), "{evil:?} must refuse");
    }
}

#[test]
fn mcp_status_is_parsed_as_a_hint_and_failure_is_reported() {
    let runtime = Arc::new(SessionRuntime::new());
    runtime.require_mcp();
    let ready = serde_json::json!({
        "type": "system",
        "subtype": "init",
        "mcp_servers": [{"name": "devboule", "status": "connected"}]
    });
    observe_mcp_status(&ready, &runtime);
    assert!(runtime
        .wait_for_mcp_ready(std::time::Duration::from_millis(1))
        .is_err());

    let failed = serde_json::json!({
        "type": "system",
        "subtype": "init",
        "mcp_servers": [{"name": "devboule", "status": "failed"}]
    });
    observe_mcp_status(&failed, &runtime);
    let error = runtime
        .wait_for_mcp_ready(std::time::Duration::from_secs(1))
        .expect_err("provider failure must wake the gate");
    assert!(error.message.contains("Claude reported"));
}

#[test]
fn interrupt_frame_matches_the_measured_control_request_wire() {
    let bytes = interrupt_frame_bytes("interrupt-7").expect("frame");
    let line = std::str::from_utf8(&bytes).expect("utf8");
    assert!(line.ends_with('\n'));
    let value: Value = serde_json::from_str(line.trim_end()).expect("json");
    assert_eq!(value["type"], "control_request");
    assert_eq!(value["request_id"], "interrupt-7");
    assert_eq!(value["request"]["subtype"], "interrupt");
}

fn claude_model(model_id: &str) -> devboule_protocol::SessionModel {
    devboule_protocol::SessionModel {
        model_id: model_id.to_string(),
        name: model_id.to_string(),
        description: None,
        context_tokens: None,
        current_effort: None,
        efforts: None,
    }
}

fn pinned(argv: Vec<String>, model_id: Option<&str>) -> Vec<String> {
    launch_with_model(launch_in_bypass_mode(argv), model_id)
}

#[test]
fn claude_launch_always_uses_bypass_permission_mode() {
    let args = launch_in_bypass_mode(vec![
        "-p".to_string(),
        "--permission-mode".to_string(),
        "plan".to_string(),
    ]);
    assert_eq!(
        args.windows(2)
            .find(|pair| pair[0] == "--permission-mode")
            .map(|pair| pair[1].as_str()),
        Some("bypassPermissions")
    );
    assert_eq!(
        args.iter()
            .filter(|arg| arg.as_str() == "--permission-mode")
            .count(),
        1
    );
}

#[test]
fn claude_launch_pins_the_catalog_model() {
    let derived = vec![
        claude_model("claude-sonnet-5"),
        claude_model("claude-opus-5"),
    ];
    let args = pinned(
        vec!["-p".to_string()],
        crate::claude_catalog::default_model_id(&derived).as_deref(),
    );
    assert_eq!(
        args.windows(2)
            .find(|pair| pair[0] == "--model")
            .map(|pair| pair[1].as_str()),
        Some("claude-opus-5")
    );

    let no_opus = vec![
        claude_model("claude-sonnet-5"),
        claude_model("claude-haiku-5"),
    ];
    let args = pinned(
        vec!["-p".to_string()],
        crate::claude_catalog::default_model_id(&no_opus).as_deref(),
    );
    assert_eq!(
        args.windows(2)
            .find(|pair| pair[0] == "--model")
            .map(|pair| pair[1].as_str()),
        Some("claude-sonnet-5")
    );
    assert_eq!(args.iter().filter(|arg| *arg == "--model").count(), 1);

    // An explicit launch model is replaced, never duplicated.
    let replaced = launch_with_model(
        vec![
            "-p".to_string(),
            "--model".to_string(),
            "sonnet".to_string(),
            "--model=haiku".to_string(),
        ],
        Some("claude-opus-5"),
    );
    assert_eq!(replaced, ["-p", "--model", "claude-opus-5"]);
}

#[test]
fn set_mode_frame_matches_the_measured_control_request_wire() {
    let bytes = control_request_frame_bytes(
        "measure-acceptEdits",
        serde_json::json!({
            "subtype": "set_permission_mode",
            "mode": "acceptEdits",
        }),
    )
    .expect("frame");
    let value: Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(
        value,
        serde_json::json!({
            "type": "control_request",
            "request_id": "measure-acceptEdits",
            "request": {
                "subtype": "set_permission_mode",
                "mode": "acceptEdits",
            }
        })
    );
}

#[test]
fn measured_set_mode_control_responses_resolve_success_and_error() {
    let mode_responses = Arc::new(Mutex::new(HashMap::new()));
    let (success_tx, success_rx) = mpsc::channel();
    let (error_tx, error_rx) = mpsc::channel();
    mode_responses.lock().expect("mode responses").extend([
        ("measure-acceptEdits".to_string(), success_tx),
        ("measure-bypassPermissions".to_string(), error_tx),
    ]);
    let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let mut reader = ClaudeReader::new(
        ClaudeView::new(Some(PathBuf::from(r"C:\work"))),
        Arc::clone(&broker),
        Arc::new(Mutex::new(HashMap::new())),
        Arc::clone(&mode_responses),
        Arc::new(AtomicU64::new(1)),
    );
    let runtime = Arc::new(SessionRuntime::new());
    let mut lines = CLAUDE_MODE_CAPTURE.lines();
    let success = lines.next().expect("measured success response");
    let error = lines.next().expect("measured error response");
    assert!(lines.next().is_none());
    reader
        .feed(format!("{success}\n{error}\n").as_bytes(), &runtime)
        .expect("feed");
    assert_eq!(success_rx.recv().expect("success response"), Ok(()));
    assert_eq!(
            error_rx.recv().expect("error response"),
            Err("Cannot set permission mode to bypassPermissions because the session was not launched with --dangerously-skip-permissions".to_string())
        );
}

#[test]
fn writer_frames_buffered_text_as_a_user_message() {
    let bytes = frame_user_message("Reply with exactly one word: PONG", None, None).expect("frame");
    let line = std::str::from_utf8(&bytes).expect("utf8");
    assert!(line.ends_with('\n'));
    let value: Value = serde_json::from_str(line.trim_end()).expect("json");
    assert_eq!(value["type"], "user");
    assert_eq!(value["message"]["role"], "user");
    assert_eq!(value["message"]["content"][0]["type"], "text");
    assert_eq!(
        value["message"]["content"][0]["text"],
        "Reply with exactly one word: PONG"
    );
}

#[test]
fn claude_steer_frame_is_byte_exact_and_carries_priority() {
    assert_eq!(
            frame_user_message("hello", Some("uuid-1"), Some("next")).expect("frame"),
            br#"{"type":"user","message":{"role":"user","content":[{"type":"text","text":"hello"}]},"uuid":"uuid-1","priority":"next"}
"#
        );
}

struct InitialModeHarness {
    child: Child,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    stdout: BufReader<ChildStdout>,
    gate: ClaudeModeGateRef,
    next_id: Arc<AtomicU64>,
}

fn initial_mode_test_setup() -> InitialModeHarness {
    let mut child = std::process::Command::new("node")
        .args([
            "-e",
            "process.stdin.on('data', data => process.stdout.write(data))",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("node is required for the Claude mode gate test");
    let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
    let stdout = child.stdout.take().expect("stdout");
    let next_id = Arc::new(AtomicU64::new(1));
    let gate = start_initial_mode(&stdin, &next_id, "default".to_string()).expect("mode request");
    InitialModeHarness {
        child,
        stdin,
        stdout: BufReader::new(stdout),
        gate,
        next_id,
    }
}

fn initial_mode_test_reader(
    broker: &Arc<PermissionBroker>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    gate: ClaudeModeGateRef,
    next_id: Arc<AtomicU64>,
    mode_responses: ClaudeModeResponses,
) -> ClaudeReader {
    initial_mode_test_reader_with_timeout(
        broker,
        stdin,
        gate,
        next_id,
        mode_responses,
        CONTROL_RESPONSE_TIMEOUT,
    )
}

fn initial_mode_test_reader_with_timeout(
    broker: &Arc<PermissionBroker>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    gate: ClaudeModeGateRef,
    next_id: Arc<AtomicU64>,
    mode_responses: ClaudeModeResponses,
    timeout: Duration,
) -> ClaudeReader {
    ClaudeReader::with_mode_gate(
        ClaudeView::new(Some(PathBuf::from(r"C:\work"))),
        Arc::clone(broker),
        Arc::new(Mutex::new(HashMap::new())),
        mode_responses,
        next_id,
        ClaudeModeGateWiring {
            stdin,
            gate,
            timeout,
            delivery_settings: Arc::new(Mutex::new(HashMap::new())),
        },
    )
}

fn read_json_line(stdout: &mut BufReader<ChildStdout>) -> Value {
    let mut line = String::new();
    stdout.read_line(&mut line).expect("child output");
    serde_json::from_str(&line).expect("child output json")
}

fn initial_mode_response(request: &Value) -> Value {
    initial_mode_response_with_mode(request, "default")
}

fn initial_mode_response_with_mode(request: &Value, mode: &str) -> Value {
    serde_json::json!({
        "type": "control_response",
        "response": {
            "subtype": "success",
            "request_id": request["request_id"],
            "response": {"mode": mode}
        }
    })
}

fn initial_mode_error(request: &Value, message: &str) -> Value {
    serde_json::json!({
        "type": "control_response",
        "response": {
            "subtype": "error",
            "request_id": request["request_id"],
            "error": message
        }
    })
}

#[test]
fn initial_claude_mode_response_flushes_prompt_without_init() {
    let mut harness = initial_mode_test_setup();
    let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let mut reader = initial_mode_test_reader(
        &broker,
        Arc::clone(&harness.stdin),
        Arc::clone(&harness.gate),
        Arc::clone(&harness.next_id),
        Arc::new(Mutex::new(HashMap::new())),
    );
    let mut writer = ClaudeWriter {
        stdin: Arc::clone(&harness.stdin),
        pending: Vec::new(),
        mode_gate: reader.mode_gate.clone(),
    };
    writer.write_all(b"Reply DONE").expect("buffer prompt");
    writer.flush().expect("queue prompt");

    let request = read_json_line(&mut harness.stdout);
    assert_eq!(request["request"]["subtype"], "set_permission_mode");
    assert_eq!(request["request"]["mode"], "default");
    let runtime = Arc::new(SessionRuntime::new());
    let response = initial_mode_response(&request);
    reader
        .feed(format!("{response}\n").as_bytes(), &runtime)
        .expect("mode response");
    let prompt = read_json_line(&mut harness.stdout);
    assert_eq!(prompt["type"], "user");
    assert_eq!(prompt["message"]["content"][0]["text"], "Reply DONE");
    drop(writer);
    let _ = harness.child.kill();
    let _ = harness.child.wait();
}

#[test]
fn claude_prompts_waiting_for_initial_mode_response_flush_in_order() {
    let mut harness = initial_mode_test_setup();
    let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let mut reader = initial_mode_test_reader(
        &broker,
        Arc::clone(&harness.stdin),
        Arc::clone(&harness.gate),
        Arc::clone(&harness.next_id),
        Arc::new(Mutex::new(HashMap::new())),
    );
    let mut writer = ClaudeWriter {
        stdin: Arc::clone(&harness.stdin),
        pending: Vec::new(),
        mode_gate: Some(Arc::clone(&harness.gate)),
    };
    writer.write_all(b"FIRST").expect("buffer first prompt");
    writer.flush().expect("queue first prompt");
    writer.write_all(b"SECOND").expect("buffer second prompt");
    writer.flush().expect("queue second prompt");

    let request = read_json_line(&mut harness.stdout);
    let runtime = Arc::new(SessionRuntime::new());
    let response = initial_mode_response(&request);
    reader
        .feed(format!("{response}\n").as_bytes(), &runtime)
        .expect("mode response");
    let first = read_json_line(&mut harness.stdout);
    let second = read_json_line(&mut harness.stdout);
    assert_eq!(first["message"]["content"][0]["text"], "FIRST");
    assert_eq!(second["message"]["content"][0]["text"], "SECOND");
    drop(writer);
    let _ = harness.child.kill();
    let _ = harness.child.wait();
}

/// A2-01: a steer that arrives while the mode gate is still
/// `AwaitingResponse` is *refused*, not queued.
///
/// The queue is for the frames that start a session: the mode request and
/// the prompt behind it are one ordered batch, and a gate that fails drops
/// it rather than delivering it late. A steer in that queue is a different
/// thing entirely — the caller is answered `Ok(true)`, which says its bytes
/// reached the provider inside the running turn, while in fact the provider
/// has not been sent a prompt at all. The honest answer for that window is
/// the refusal every unavailable steer gets, with its pre-existing fallback.
#[test]
fn a_steer_while_the_mode_gate_is_awaiting_is_refused_not_queued() {
    let mut harness = initial_mode_test_setup();
    let runtime = Arc::new(SessionRuntime::new());
    let mut writer = ClaudeWriter {
        stdin: Arc::clone(&harness.stdin),
        pending: Vec::new(),
        mode_gate: Some(Arc::clone(&harness.gate)),
    };
    // The running turn's own prompt: queued, not written, because the gate
    // has not released yet.
    writer.write_all(b"Reply DONE").expect("buffer prompt");
    writer.flush().expect("queue prompt");
    // The daemon counts that prompt as a running turn, which is what admits
    // a steer for it.
    runtime.begin_turn();
    let mut steerer = ClaudeSteerer {
        stdin: Arc::clone(&harness.stdin),
        mode_gate: Some(Arc::clone(&harness.gate)),
    };
    let steered = runtime.with_active_turn(runtime.turn_counter(), |turn| {
        steerer.steer_active_turn("Turn left instead", turn)
    });
    assert!(
        matches!(steered, Some(Ok(false))),
        "the gate is still awaiting the mode response: the steerer refuses"
    );

    // The queue holds the prompt and nothing else, so the batch that will be
    // written is exactly the batch that was queued before the steer arrived.
    let queued = harness.gate.lock().expect("gate").pending_frames.clone();
    assert_eq!(
        queued.len(),
        1,
        "the refused steer added nothing to the queue"
    );
    let prompt: Value = serde_json::from_slice(&queued[0]).expect("prompt json");
    assert_eq!(prompt["message"]["content"][0]["text"], "Reply DONE");

    // The release writes that one frame, and only that one.
    let request = read_json_line(&mut harness.stdout);
    assert_eq!(request["request"]["subtype"], "set_permission_mode");
    let mut view = ClaudeView::new(None);
    {
        let mut gate = harness.gate.lock().expect("gate");
        assert!(
            flush_gate_frames(&mut gate, &harness.stdin, &mut view, "default").is_none(),
            "the batch writes cleanly"
        );
    }
    let first = read_json_line(&mut harness.stdout);
    assert_eq!(first["message"]["content"][0]["text"], "Reply DONE");
    assert!(
        harness.gate.lock().expect("gate").pending_frames.is_empty(),
        "the flush wrote everything that was queued"
    );
    drop(writer);
    let _ = harness.child.kill();
    let _ = harness.child.wait();
}

#[test]
fn a_refused_steer_leaves_the_gate_s_failing_batch_untouched() {
    // The same refusal on a gate that never releases: the steer is never in
    // the batch, so what the failure drops is exactly the prompt that was
    // queued for it — the steer contributed nothing to lose.
    let mut harness = initial_mode_test_setup();
    let runtime = Arc::new(SessionRuntime::new());
    let mut writer = ClaudeWriter {
        stdin: Arc::clone(&harness.stdin),
        pending: Vec::new(),
        mode_gate: Some(Arc::clone(&harness.gate)),
    };
    writer.write_all(b"Reply DONE").expect("buffer prompt");
    writer.flush().expect("queue prompt");
    runtime.begin_turn();
    let mut steerer = ClaudeSteerer {
        stdin: Arc::clone(&harness.stdin),
        mode_gate: Some(Arc::clone(&harness.gate)),
    };
    let steered = runtime.with_active_turn(runtime.turn_counter(), |turn| {
        steerer.steer_active_turn("Turn left instead", turn)
    });
    assert!(matches!(steered, Some(Ok(false))));
    assert_eq!(
        harness.gate.lock().expect("gate").pending_frames.len(),
        1,
        "the gate holds the prompt and nothing the steer added"
    );

    let request = read_json_line(&mut harness.stdout);
    let request_id = request["request_id"]
        .as_str()
        .expect("the mode request names itself")
        .to_string();
    assert!(
            fail_initial_mode_parts(
                &harness.gate,
                Some(&harness.stdin),
                &runtime,
                Some(&request_id),
                true,
                "Claude mode response timed out; queued prompt(s) were not delivered because Claude never confirmed the permission mode.",
            ),
            "the gate fails on the request it is awaiting"
        );
    assert!(
        harness.gate.lock().expect("gate").pending_frames.is_empty(),
        "the queued steer is dropped with the batch"
    );
    assert!(
        harness.stdin.lock().expect("stdin").is_none(),
        "the transport a late delivery would use is closed"
    );
    let _ = harness.child.kill();
    let _ = harness.child.wait();
}

/// S4-09: the gate guard is held across the write, so the decision and the
/// write really are one critical section — which is what the function claims.
///
/// The frame is bigger than any pipe buffer and the fake child never reads it,
/// so the write is *inside* the gate while this test looks. A guard released
/// before the write leaves the gate free for the whole window.
#[test]
fn the_steer_write_holds_the_gate_while_it_writes() {
    use std::process::Stdio;

    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    // A fake Claude that never reads its stdin, so a large write blocks in the
    // pipe and stays there.
    let mut child = std::process::Command::new("node")
        .args(["-e", "setTimeout(() => {}, 60000)"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("node is required for the Claude gate-hold test");
    let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
    let gate: super::ClaudeModeGateRef = Arc::new(Mutex::new(super::ClaudeModeGate {
        state: super::ClaudeModeGateState::Ready,
        pending_frames: Vec::new(),
    }));
    let mut steerer = super::ClaudeSteerer {
        stdin: Arc::clone(&stdin),
        mode_gate: Some(Arc::clone(&gate)),
    };
    let text = "x".repeat(1024 * 1024);
    let steer = std::thread::spawn(move || {
        crate::test_support::steer_through_the_turn(&mut steerer, &text)
    });
    // Let the write reach the pipe, then look at the gate for a bounded
    // window: it must never be free while the bytes are going out.
    std::thread::sleep(Duration::from_millis(150));
    let mut available = 0;
    for _ in 0..200 {
        if gate.try_lock().is_ok() {
            available += 1;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert_eq!(
        available, 0,
        "the gate was free during the write: the guard is not held across it (S4-09)"
    );
    let _ = child.kill();
    let _ = child.wait();
    let _ = steer.join();
}

#[test]
fn initial_claude_mode_request_is_written_before_the_first_prompt() {
    let mut harness = initial_mode_test_setup();
    let mut writer = ClaudeWriter {
        stdin: Arc::clone(&harness.stdin),
        pending: Vec::new(),
        mode_gate: Some(Arc::clone(&harness.gate)),
    };
    writer.write_all(b"FIRST").expect("buffer prompt");
    writer.flush().expect("queue prompt");

    let request = read_json_line(&mut harness.stdout);
    assert_eq!(request["type"], "control_request");
    assert_eq!(request["request"]["subtype"], "set_permission_mode");
    drop(writer);
    let _ = harness.child.kill();
    let _ = harness.child.wait();
}

#[test]
fn initial_claude_mode_error_publishes_once_and_closes_stdin() {
    let mut harness = initial_mode_test_setup();
    let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let mut reader = initial_mode_test_reader(
        &broker,
        Arc::clone(&harness.stdin),
        Arc::clone(&harness.gate),
        Arc::clone(&harness.next_id),
        Arc::new(Mutex::new(HashMap::new())),
    );
    let mut writer = ClaudeWriter {
        stdin: Arc::clone(&harness.stdin),
        pending: Vec::new(),
        mode_gate: Some(Arc::clone(&harness.gate)),
    };
    writer.write_all(b"DROP ME").expect("buffer prompt");
    writer.flush().expect("queue prompt");
    let request = read_json_line(&mut harness.stdout);
    let (runtime, conn) = attached(&broker);
    reader
        .feed(
            format!("{}\n", initial_mode_error(&request, "rejected")).as_bytes(),
            &runtime,
        )
        .expect("mode error");

    let events = drain(&conn);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, SessionEvent::AgentError { .. }))
            .count(),
        1
    );
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::AgentError { message } if message == "rejected"
    )));
    let gate = harness.gate.lock().expect("gate");
    assert!(matches!(&gate.state, ClaudeModeGateState::Failed(message) if message == "rejected"));
    assert!(gate.pending_frames.is_empty());
    drop(gate);
    assert!(harness.stdin.lock().expect("stdin").is_none());
    drop(writer);
    let _ = harness.child.kill();
    let _ = harness.child.wait();
}

#[test]
fn initial_claude_mode_timeout_publishes_once_and_drops_frames() {
    let mut harness = initial_mode_test_setup();
    let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let mut reader = initial_mode_test_reader_with_timeout(
        &broker,
        Arc::clone(&harness.stdin),
        Arc::clone(&harness.gate),
        Arc::clone(&harness.next_id),
        Arc::new(Mutex::new(HashMap::new())),
        Duration::ZERO,
    );
    let mut writer = ClaudeWriter {
        stdin: Arc::clone(&harness.stdin),
        pending: Vec::new(),
        mode_gate: Some(Arc::clone(&harness.gate)),
    };
    writer.write_all(b"DROP ME").expect("buffer prompt");
    writer.flush().expect("queue prompt");
    let (runtime, conn) = attached(&broker);
    let _request = read_json_line(&mut harness.stdout);
    reader.feed(b"", &runtime).expect("start timeout");
    reader
        .initial_mode_timer_thread
        .take()
        .expect("timeout thread")
        .join()
        .expect("timeout thread join");
    let events = drain(&conn);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, SessionEvent::AgentError { .. }))
            .count(),
        1
    );
    let gate = harness.gate.lock().expect("gate");
    assert!(
        matches!(&gate.state, ClaudeModeGateState::Failed(message) if message == "Claude mode response timed out; queued prompt(s) were not delivered because Claude never confirmed the permission mode.")
    );
    assert!(gate.pending_frames.is_empty());
    drop(gate);
    assert!(harness.stdin.lock().expect("stdin").is_none());
    drop(writer);
    let _ = harness.child.kill();
    let _ = harness.child.wait();
}

#[test]
fn initial_claude_mode_timeout_after_success_is_a_noop() {
    let mut harness = initial_mode_test_setup();
    let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let mut reader = initial_mode_test_reader_with_timeout(
        &broker,
        Arc::clone(&harness.stdin),
        Arc::clone(&harness.gate),
        Arc::clone(&harness.next_id),
        Arc::new(Mutex::new(HashMap::new())),
        Duration::from_secs(1),
    );
    let mut writer = ClaudeWriter {
        stdin: Arc::clone(&harness.stdin),
        pending: Vec::new(),
        mode_gate: Some(Arc::clone(&harness.gate)),
    };
    writer.write_all(b"KEEP ME").expect("buffer prompt");
    writer.flush().expect("queue prompt");
    let request = read_json_line(&mut harness.stdout);
    let (runtime, conn) = attached(&broker);
    reader
        .feed(
            format!("{}\n", initial_mode_response(&request)).as_bytes(),
            &runtime,
        )
        .expect("mode response");
    let prompt = read_json_line(&mut harness.stdout);
    assert_eq!(prompt["message"]["content"][0]["text"], "KEEP ME");
    assert!(drain(&conn)
        .iter()
        .all(|event| !matches!(event, SessionEvent::AgentError { .. })));
    assert!(reader.initial_mode_timer_thread.is_none());
    let gate = harness.gate.lock().expect("gate");
    assert!(matches!(&gate.state, ClaudeModeGateState::Ready));
    assert!(gate.pending_frames.is_empty());
    drop(gate);
    assert!(harness.stdin.lock().expect("stdin").is_some());
    drop(writer);
    let _ = harness.child.kill();
    let _ = harness.child.wait();
}

#[test]
fn claude_mode_timeout_on_a_ready_gate_is_a_noop() {
    let mut harness = initial_mode_test_setup();
    let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let mut reader = initial_mode_test_reader(
        &broker,
        Arc::clone(&harness.stdin),
        Arc::clone(&harness.gate),
        Arc::clone(&harness.next_id),
        Arc::new(Mutex::new(HashMap::new())),
    );
    let mut writer = ClaudeWriter {
        stdin: Arc::clone(&harness.stdin),
        pending: Vec::new(),
        mode_gate: Some(Arc::clone(&harness.gate)),
    };
    writer.write_all(b"KEEP ME").expect("buffer prompt");
    writer.flush().expect("queue prompt");
    let _request = read_json_line(&mut harness.stdout);
    let (runtime, conn) = attached(&broker);
    reader.feed(b"", &runtime).expect("start timeout");

    // Hold the gate so the fired deadline cannot inspect it before the
    // Ready transition lands; dropping the timer sender is the deadline
    // firing, without a sleep.
    let mut gate = harness.gate.lock().expect("gate");
    drop(reader.initial_mode_timer_cancel.take());
    assert!(flush_gate_frames(&mut gate, &harness.stdin, &mut reader.view, "default",).is_none());
    drop(gate);

    reader
        .initial_mode_timer_thread
        .take()
        .expect("timeout thread")
        .join()
        .expect("timeout thread join");
    let prompt = read_json_line(&mut harness.stdout);
    assert_eq!(prompt["message"]["content"][0]["text"], "KEEP ME");
    assert!(drain(&conn)
        .iter()
        .all(|event| !matches!(event, SessionEvent::AgentError { .. })));
    let gate = harness.gate.lock().expect("gate");
    assert!(matches!(&gate.state, ClaudeModeGateState::Ready));
    assert!(gate.pending_frames.is_empty());
    drop(gate);
    assert!(harness.stdin.lock().expect("stdin").is_some());
    drop(writer);
    let _ = harness.child.kill();
    let _ = harness.child.wait();
}

#[test]
fn claude_finish_while_initial_mode_is_pending_publishes_once_without_flushing() {
    let mut harness = initial_mode_test_setup();
    let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let mut reader = initial_mode_test_reader(
        &broker,
        Arc::clone(&harness.stdin),
        Arc::clone(&harness.gate),
        Arc::clone(&harness.next_id),
        Arc::new(Mutex::new(HashMap::new())),
    );
    let mut writer = ClaudeWriter {
        stdin: Arc::clone(&harness.stdin),
        pending: Vec::new(),
        mode_gate: Some(Arc::clone(&harness.gate)),
    };
    writer.write_all(b"DROP ON EXIT").expect("buffer prompt");
    writer.flush().expect("queue prompt");
    let _request = read_json_line(&mut harness.stdout);
    let (runtime, conn) = attached(&broker);
    reader.finish(&runtime);
    reader.finish(&runtime);
    let events = drain(&conn);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, SessionEvent::AgentError { .. }))
            .count(),
        1
    );
    assert!(events.iter().any(|event| matches!(
            event,
            SessionEvent::AgentError { message }
                if message == "Claude exited before confirming the permission mode; queued prompt(s) were not delivered because Claude never confirmed the permission mode."
        )));
    let gate = harness.gate.lock().expect("gate");
    assert!(
        matches!(&gate.state, ClaudeModeGateState::Failed(message) if message == "Claude exited before confirming the permission mode; queued prompt(s) were not delivered because Claude never confirmed the permission mode.")
    );
    assert!(gate.pending_frames.is_empty());
    drop(gate);
    assert!(harness.stdin.lock().expect("stdin").is_none());
    drop(writer);
    let _ = harness.child.kill();
    let _ = harness.child.wait();
}

#[test]
fn user_mode_switch_during_initial_mode_is_fifo_and_wins_in_the_view() {
    let mut harness = initial_mode_test_setup();
    let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let mode_responses = Arc::new(Mutex::new(HashMap::new()));
    let mut reader = initial_mode_test_reader(
        &broker,
        Arc::clone(&harness.stdin),
        Arc::clone(&harness.gate),
        Arc::clone(&harness.next_id),
        Arc::clone(&mode_responses),
    );
    let switcher = ClaudeSwitcher {
        stdin: Arc::clone(&harness.stdin),
        next_id: Arc::clone(&harness.next_id),
        mode_responses,
        mode_gate: Some(Arc::clone(&harness.gate)),
    };
    let user_mode = std::thread::spawn(move || switcher.set_mode("acceptEdits"));
    let initial_request = read_json_line(&mut harness.stdout);
    let user_request = read_json_line(&mut harness.stdout);
    assert_eq!(initial_request["request"]["mode"], "default");
    assert_eq!(user_request["request"]["mode"], "acceptEdits");
    assert_ne!(initial_request["request_id"], user_request["request_id"]);

    let (runtime, conn) = attached(&broker);
    let init = serde_json::json!({
        "type": "system",
        "subtype": "init",
        "session_id": "s1",
        "model": "model-a",
        "permissionMode": "default"
    });
    reader
        .feed(format!("{init}\n").as_bytes(), &runtime)
        .expect("init");
    reader
        .feed(
            format!("{}\n", initial_mode_response(&initial_request)).as_bytes(),
            &runtime,
        )
        .expect("initial mode response");
    reader
        .feed(
            format!(
                "{}\n",
                initial_mode_response_with_mode(&user_request, "acceptEdits")
            )
            .as_bytes(),
            &runtime,
        )
        .expect("user mode response");
    assert!(user_mode.join().expect("mode thread").is_ok());

    let assistant = serde_json::json!({
        "type": "assistant",
        "message": {
            "model": "model-b",
            "id": "message-b",
            "role": "assistant",
            "content": []
        }
    });
    reader
        .feed(format!("{assistant}\n").as_bytes(), &runtime)
        .expect("assistant model change");
    let events = drain(&conn);
    let current_mode = events.iter().rev().find_map(|event| match event {
        SessionEvent::SessionManifest {
            modes: Some(modes), ..
        } => Some(modes.current_mode_id.clone()),
        _ => None,
    });
    assert_eq!(current_mode.as_deref(), Some("acceptEdits"));
    let _ = harness.child.kill();
    let _ = harness.child.wait();
}

#[test]
fn control_response_allow_and_deny_match_the_measured_wire() {
    let input = serde_json::json!({
        "command": r"cmd /c del /q C:\Windows\Temp\devboule-nonexistent.txt",
        "description": "Delete a nonexistent temp file"
    });
    let allow = control_response_frame(
        "e73c118e-6742-481e-b60a-e8486a9bde4e",
        &input,
        &serde_json::json!({"outcome": {"outcome": "selected", "optionId": "allow"}}),
    );
    assert_eq!(allow["type"], "control_response");
    assert_eq!(allow["response"]["subtype"], "success");
    assert_eq!(
        allow["response"]["request_id"],
        "e73c118e-6742-481e-b60a-e8486a9bde4e"
    );
    assert_eq!(allow["response"]["response"]["behavior"], "allow");
    assert_eq!(allow["response"]["response"]["updatedInput"], input);

    let deny = control_response_frame(
        "620d31b5-1123-4170-b3d6-7465dc7ceced",
        &input,
        &serde_json::json!({"outcome": {"outcome": "selected", "optionId": "deny"}}),
    );
    assert_eq!(deny["response"]["response"]["behavior"], "deny");
    assert_eq!(
        deny["response"]["response"]["message"],
        "The user declined this command."
    );
}

fn test_reader(
    broker: Arc<PermissionBroker>,
    controls: Arc<Mutex<HashMap<u64, ClaudePendingControl>>>,
) -> ClaudeReader {
    ClaudeReader::new(
        ClaudeView::new(Some(PathBuf::from(r"C:\work"))),
        broker,
        controls,
        Arc::new(Mutex::new(HashMap::new())),
        Arc::new(AtomicU64::new(1)),
    )
}

#[test]
fn reader_assembles_a_line_split_across_feed_chunks() {
    let (broker, _) = {
        let sent = Arc::new(Mutex::new(Vec::<Value>::new()));
        let sender: Arc<PermissionSender> = Arc::new(move |_, _| Ok(()));
        let _ = sent;
        (PermissionBroker::for_test(sender), ())
    };
    let mut reader = test_reader(broker.clone(), Arc::new(Mutex::new(HashMap::new())));
    let (runtime, conn) = attached(&broker);
    reader
        .feed(br#"{"type":"system","subtype":"ini"#, &runtime)
        .expect("partial");
    assert!(drain(&conn)
        .iter()
        .all(|event| !matches!(event, SessionEvent::SessionManifest { .. })));
    reader
        .feed(
            b"t\",\"session_id\":\"abc\",\"model\":\"claude-opus-5\"}\n",
            &runtime,
        )
        .expect("rest");
    let events = drain(&conn);
    assert!(
        events.iter().any(|event| matches!(
            event,
            SessionEvent::SessionManifest {
                provider_id,
                current_model_id,
                ..
            } if provider_id.as_deref() == Some("claude")
                && current_model_id.as_deref() == Some("claude-opus-5")
        )),
        "split init line must become a manifest: {events:?}"
    );
    assert_eq!(runtime.peer_session_id().as_deref(), Some("abc"));
}

#[test]
fn reader_parses_crlf_delimited_init_frames() {
    let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let mut reader = test_reader(Arc::clone(&broker), Arc::new(Mutex::new(HashMap::new())));
    let (runtime, conn) = attached(&broker);
    // recon/probes/claude-perm-probe2-allow-host.txt system/init shape.
    let frames = concat!(
        r#"{"type":"system","subtype":"init","cwd":"C:\\tmp","session_id":"cbe439d8-8e95-42c3-b6c7-40c7e5d3b3cd","tools":["Bash","Read"],"model":"claude-opus-5[1m]","permissionMode":"default","claude_code_version":"2.1.260"}"#,
        "\r\n",
        r#"{"type":"system","subtype":"init","cwd":"C:\\tmp","session_id":"eb3f000a-87c3-4278-affb-cf183769f7e2","tools":["Bash","Read"],"model":"claude-opus-5","permissionMode":"default","claude_code_version":"2.1.260"}"#,
        "\r\n",
    );
    reader.feed(frames.as_bytes(), &runtime).expect("crlf feed");
    let events = drain(&conn);
    let manifests: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            SessionEvent::SessionManifest {
                current_model_id, ..
            } => current_model_id.as_deref(),
            _ => None,
        })
        .collect();
    assert_eq!(
        manifests,
        ["claude-opus-5[1m]", "claude-opus-5"],
        "both CRLF frames must parse: {events:?}"
    );
    assert_eq!(
        runtime.peer_session_id().as_deref(),
        Some("eb3f000a-87c3-4278-affb-cf183769f7e2")
    );
}

#[test]
fn reader_discards_a_huge_unterminated_line_without_killing_the_session() {
    let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let mut reader = test_reader(Arc::clone(&broker), Arc::new(Mutex::new(HashMap::new())));
    let (runtime, conn) = attached(&broker);
    reader
        .feed(&vec![b'x'; MAX_LINE_BYTES + 1], &runtime)
        .expect("oversized input is reported, not fatal");
    assert!(reader.buffer.is_empty());
    let events = drain(&conn);
    assert!(events.iter().any(|event| matches!(
        event,
        SessionEvent::AgentError { message } if message.contains("exceeded")
    )));
}

#[test]
fn permission_allow_and_deny_write_control_response_frames() {
    let captured = Arc::new(Mutex::new(Vec::new()));
    let controls = Arc::new(Mutex::new(HashMap::new()));
    let captured_for_sender = Arc::clone(&captured);
    let controls_for_sender = Arc::clone(&controls);
    let sender: Arc<PermissionSender> = Arc::new(move |id, result| {
        let pending: ClaudePendingControl = controls_for_sender
            .lock()
            .expect("controls")
            .remove(&id)
            .expect("pending control");
        let frame = control_response_frame(&pending.request_id, &pending.input, &result);
        captured_for_sender.lock().expect("captured").push(frame);
        Ok(())
    });
    let broker = PermissionBroker::for_test(sender);
    let mut reader = test_reader(Arc::clone(&broker), Arc::clone(&controls));
    let (runtime, conn) = attached(&broker);
    let line = serde_json::json!({
        "type": "control_request",
        "request_id": "e73c118e-6742-481e-b60a-e8486a9bde4e",
        "request": {
            "subtype": "can_use_tool",
            "tool_name": "Bash",
            "display_name": "Bash",
            "input": {
                "command": r"cmd /c del /q C:\Windows\Temp\devboule-nonexistent.txt",
                "description": "Delete a nonexistent temp file"
            },
            "tool_use_id": "toolu_01FgWLJkmeyU9wAGYkx3YFXu",
            "decision_reason": "This command requires approval"
        }
    });
    reader
        .feed(format!("{line}\n").as_bytes(), &runtime)
        .expect("feed");
    let events = drain(&conn);
    let permission = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::PermissionRequest {
                tool_call_id,
                title,
                description,
                ..
            } if tool_call_id == "toolu_01FgWLJkmeyU9wAGYkx3YFXu" => {
                Some((title.clone(), description.clone()))
            }
            _ => None,
        })
        .expect("permission request");
    // `decision_reason` is the permission engine's internal vocabulary:
    // the card shows the input's own description, never the reason.
    assert_eq!(permission.0, "Bash");
    assert_eq!(
        permission.1.as_deref(),
        Some("Delete a nonexistent temp file")
    );
    let encoded = serde_json::to_value(&events).expect("events json");
    assert!(
        !encoded.to_string().contains("requires approval"),
        "decision_reason must not reach the event: {encoded}"
    );
    broker
        .respond(
            "toolu_01FgWLJkmeyU9wAGYkx3YFXu",
            PermissionOutcome::AllowOnce,
        )
        .expect("allow");
    let frames = captured.lock().expect("captured");
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0]["response"]["response"]["behavior"], "allow");
    assert_eq!(
        frames[0]["response"]["request_id"],
        "e73c118e-6742-481e-b60a-e8486a9bde4e"
    );
    drop(frames);

    let captured = Arc::new(Mutex::new(Vec::new()));
    let controls = Arc::new(Mutex::new(HashMap::new()));
    let captured_for_sender = Arc::clone(&captured);
    let controls_for_sender = Arc::clone(&controls);
    let sender: Arc<PermissionSender> = Arc::new(move |id, result| {
        let pending: ClaudePendingControl = controls_for_sender
            .lock()
            .expect("controls")
            .remove(&id)
            .expect("pending control");
        let frame = control_response_frame(&pending.request_id, &pending.input, &result);
        captured_for_sender.lock().expect("captured").push(frame);
        Ok(())
    });
    let broker = PermissionBroker::for_test(sender);
    let mut reader = test_reader(Arc::clone(&broker), Arc::clone(&controls));
    let (runtime, _conn) = attached(&broker);
    reader
        .feed(format!("{line}\n").as_bytes(), &runtime)
        .expect("feed deny");
    broker
        .respond("toolu_01FgWLJkmeyU9wAGYkx3YFXu", PermissionOutcome::Deny)
        .expect("deny");
    let frames = captured.lock().expect("captured");
    assert_eq!(frames[0]["response"]["response"]["behavior"], "deny");
}

#[test]
fn permission_description_uses_wire_description_never_decision_reason() {
    // Measured wire (journal, live session): `decision_reason` carries the
    // permission engine's internal vocabulary and must not reach the card.
    let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let mut reader = test_reader(Arc::clone(&broker), Arc::new(Mutex::new(HashMap::new())));
    let (runtime, conn) = attached(&broker);
    let line = serde_json::json!({
        "type": "control_request",
        "request_id": "live-probe-request",
        "request": {
            "subtype": "can_use_tool",
            "tool_name": "Bash",
            "display_name": "Bash",
            "input": {
                "command": r#"printf devboule-live-probe > "$TEMP/devboule-live-probe.txt""#,
                "description": "Write probe string to a temp file"
            },
            "description": "Write probe string to a temp file",
            "permission_suggestions": [],
            "decision_reason": "Contains simple_expansion",
            "decision_reason_type": "other",
            "tool_use_id": "toolu_live_probe"
        }
    });
    reader
        .feed(format!("{line}\n").as_bytes(), &runtime)
        .expect("feed");
    let events = drain(&conn);
    let permission = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::PermissionRequest {
                tool_call_id,
                title,
                description,
                command,
                ..
            } => Some((
                tool_call_id.clone(),
                title.clone(),
                description.clone(),
                command.clone(),
            )),
            _ => None,
        })
        .expect("permission request");
    assert_eq!(permission.0, "toolu_live_probe");
    assert_eq!(permission.1, "Bash");
    assert_eq!(
        permission.3.as_deref(),
        Some(r#"printf devboule-live-probe > "$TEMP/devboule-live-probe.txt""#)
    );
    assert_eq!(
        permission.2.as_deref(),
        Some("Write probe string to a temp file")
    );
    let encoded = serde_json::to_value(&events).expect("events json");
    assert!(
        !encoded.to_string().contains("simple_expansion"),
        "decision_reason must not reach the event: {encoded}"
    );
    broker
        .respond("toolu_live_probe", PermissionOutcome::Deny)
        .expect("deny");
}

#[test]
fn permission_request_level_description_wins_over_input_description() {
    // Gap audit: the measured frame uses the identical string in both
    // places, so precedence needs its own frame.
    let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let mut reader = test_reader(Arc::clone(&broker), Arc::new(Mutex::new(HashMap::new())));
    let (runtime, conn) = attached(&broker);
    let line = serde_json::json!({
        "type": "control_request",
        "request_id": "precedence-request",
        "request": {
            "subtype": "can_use_tool",
            "tool_name": "Bash",
            "display_name": "Bash",
            "input": {
                "command": "echo precedence",
                "description": "input-level description"
            },
            "description": "request-level description",
            "permission_suggestions": [],
            "tool_use_id": "toolu_precedence"
        }
    });
    reader
        .feed(format!("{line}\n").as_bytes(), &runtime)
        .expect("feed");
    let events = drain(&conn);
    let description = events
        .iter()
        .find_map(|event| match event {
            SessionEvent::PermissionRequest {
                tool_call_id,
                description,
                ..
            } if tool_call_id == "toolu_precedence" => Some(description.clone()),
            _ => None,
        })
        .expect("permission request");
    assert_eq!(description.as_deref(), Some("request-level description"));
    broker
        .respond("toolu_precedence", PermissionOutcome::Deny)
        .expect("deny");
}

#[test]
fn claude_bypass_auto_answers_can_use_tool_without_client_prompt() {
    let sent = Arc::new(Mutex::new(Vec::new()));
    let sent_for_sender = Arc::clone(&sent);
    let sender: Arc<PermissionSender> = Arc::new(move |_, result| {
        sent_for_sender.lock().expect("sent").push(result);
        Ok(())
    });
    let broker = PermissionBroker::for_test(sender);
    let controls = Arc::new(Mutex::new(HashMap::new()));
    let mut reader = test_reader(Arc::clone(&broker), controls);
    let (runtime, conn) = attached(&broker);
    runtime.store_session_manifest(SessionEvent::SessionManifest {
        provider_id: Some("claude".to_string()),
        current_model_id: None,
        models: Vec::new(),
        modes: Some(devboule_protocol::SessionModeStateView {
            current_mode_id: "bypassPermissions".to_string(),
            available_modes: Vec::new(),
        }),
    });
    let line = serde_json::json!({
        "type": "control_request",
        "request_id": "claude-bypass-request",
        "request": {
            "subtype": "can_use_tool",
            "tool_name": "Bash",
            "display_name": "Bash",
            "input": {"command": "echo devboule-probe"},
            "tool_use_id": "claude-bypass-tool",
        }
    });
    reader
        .feed(format!("{line}\n").as_bytes(), &runtime)
        .expect("feed bypass request");
    assert!(!drain(&conn)
        .iter()
        .any(|event| matches!(event, SessionEvent::PermissionRequest { .. })));
    assert_eq!(broker.pending_len(), 0);
    assert_eq!(
        sent.lock().expect("sent")[0]["outcome"]["optionId"],
        "allow"
    );

    let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let mut reader = test_reader(Arc::clone(&broker), Arc::new(Mutex::new(HashMap::new())));
    let (runtime, conn) = attached(&broker);
    runtime.store_session_manifest(SessionEvent::SessionManifest {
        provider_id: Some("claude".to_string()),
        current_model_id: None,
        models: Vec::new(),
        modes: Some(devboule_protocol::SessionModeStateView {
            current_mode_id: "default".to_string(),
            available_modes: Vec::new(),
        }),
    });
    reader
        .feed(format!("{line}\n").as_bytes(), &runtime)
        .expect("feed default request");
    assert!(drain(&conn).iter().any(|event| matches!(
        event,
        SessionEvent::PermissionRequest { tool_call_id, .. }
            if tool_call_id == "claude-bypass-tool"
    )));
    broker
        .respond("claude-bypass-tool", PermissionOutcome::Deny)
        .expect("deny default request");
}

#[test]
fn soft_interrupt_cancels_the_pending_permission_and_keeps_the_broker_open() {
    let mut command = Command::new("ping");
    command
        .args(["-t", "127.0.0.1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let child = command.spawn().expect("ping");
    let pid = child.id();
    let sent = Arc::new(Mutex::new(Vec::new()));
    let sent_for_sender = Arc::clone(&sent);
    let sender: Arc<PermissionSender> = Arc::new(move |_, result| {
        sent_for_sender.lock().expect("sent").push(result);
        Ok(())
    });
    let broker = PermissionBroker::for_test(sender);
    let mut reader = test_reader(Arc::clone(&broker), Arc::new(Mutex::new(HashMap::new())));
    let (runtime, conn) = attached(&broker);
    let first = serde_json::json!({
        "type": "control_request",
        "request_id": "req-before-stop",
        "request": {
            "subtype": "can_use_tool",
            "tool_name": "Bash",
            "display_name": "Bash",
            "input": {"command": "echo one"},
            "tool_use_id": "tool-before-stop"
        }
    });
    reader
        .feed(format!("{first}\n").as_bytes(), &runtime)
        .expect("feed pending request");
    assert!(drain(&conn).iter().any(|event| matches!(
        event,
        SessionEvent::PermissionRequest { tool_call_id, .. }
            if tool_call_id == "tool-before-stop"
    )));

    let mut killer = ClaudeKiller {
        process: Arc::new(Mutex::new(child)),
        stdin: Arc::new(Mutex::new(None)),
        next_id: Arc::new(AtomicU64::new(1)),
        permission_broker: Arc::clone(&broker),
        cancelled: Arc::new(AtomicBool::new(false)),
    };
    killer.interrupt();
    let stopped = drain(&conn);
    assert!(stopped.iter().any(|event| matches!(
        event,
        SessionEvent::PermissionResolved {
            tool_call_id,
            selected_option_id: None,
            ..
        } if tool_call_id == "tool-before-stop"
    )));
    assert_eq!(broker.pending_len(), 0);

    let second = serde_json::json!({
        "type": "control_request",
        "request_id": "req-after-stop",
        "request": {
            "subtype": "can_use_tool",
            "tool_name": "Bash",
            "display_name": "Bash",
            "input": {"command": "echo two"},
            "tool_use_id": "tool-after-stop"
        }
    });
    reader
        .feed(format!("{second}\n").as_bytes(), &runtime)
        .expect("feed request after the soft stop");
    let after = drain(&conn);
    assert!(after.iter().any(|event| matches!(
        event,
        SessionEvent::PermissionRequest { tool_call_id, .. }
            if tool_call_id == "tool-after-stop"
    )));
    assert!(!after.iter().any(|event| matches!(
        event,
        SessionEvent::AgentError { message } if message.contains("closed")
    )));
    assert_eq!(broker.pending_len(), 1);
    // The fake agent is still alive here by design: `interrupt` is soft
    // and must not kill it (the live second permission above proves it).
    // Reap it explicitly and prove the OS process is gone. Dropping
    // `Child` does not terminate, and an infinite `ping -t` orphaned
    // past the test binary keeps the shared test log open on Windows.
    killer.kill();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        match killer
            .process
            .lock()
            .expect("fake agent")
            .try_wait()
            .expect("poll fake agent")
        {
            Some(_) => break,
            None => {
                assert!(
                    std::time::Instant::now() < deadline,
                    "fake agent {pid} still alive 5s after kill"
                );
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }
}

#[test]
fn kill_does_not_wait_for_the_child_to_cooperate() {
    let mut command = Command::new("ping");
    command
        .args(["-t", "127.0.0.1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn().expect("ping");
    let stdin = child.stdin.take();
    let process = Arc::new(Mutex::new(child));
    let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let mut killer = ClaudeKiller {
        process: Arc::clone(&process),
        stdin: Arc::new(Mutex::new(stdin)),
        next_id: Arc::new(AtomicU64::new(1)),
        permission_broker: broker,
        cancelled: Arc::new(AtomicBool::new(false)),
    };
    killer.kill();
    let started = std::time::Instant::now();
    loop {
        let done = process
            .lock()
            .expect("process")
            .try_wait()
            .expect("wait")
            .is_some();
        if done {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "kill must not wait for the child to read stdin"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

// --- image delivery (the static route) --------------------------------
//
// The routing decision lives in `plan_claude_prompt`, tested here
// against the attachment store directly, without spawning a child — the
// same arrangement the ACP sibling seam's tests use. The wire shape of
// one block is pinned against the exact JSON Paseo's `toSdkUserMessage`
// emits (`{"type":"image","source":{"type":"base64","media_type":...}}`).

struct PlanTempDir(PathBuf);

impl PlanTempDir {
    fn new(tag: &str) -> Self {
        let dir = crate::test_dirs::test_temp_dir(&format!("devboule-claude-plan-{tag}"));
        Self(dir)
    }
}

impl Drop for PlanTempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn plan_attachment(name: &str, mime_type: &str, bytes: &[u8]) -> PromptAttachment {
    use base64::Engine;
    PromptAttachment {
        name: name.to_string(),
        mime_type: mime_type.to_string(),
        data: base64::engine::general_purpose::STANDARD.encode(bytes),
    }
}

#[test]
fn claude_delivery_is_the_static_variant() {
    // No handshake to negotiate with: the format accepts images, so the
    // delivery is the static one the route reads.
    assert_eq!(
        claude_delivery(),
        super::super::ImageDelivery::StaticImageBlock
    );
}

#[test]
fn a_capable_claude_prompt_builds_the_nested_source_block_and_no_path_line() {
    // The raster becomes one image block; the text is the bare user text,
    // with no path line.
    let temp = PlanTempDir::new("capable");
    let store = AttachmentStore::new(&temp.0);
    let session_id = "claude-plan-capable";
    // A container the walk accepts but changes: what the block carries
    // must be the stripped bytes, never the wire bytes.
    let sent = png_with_text_chunk();
    let kept = clean_png(0x01);
    assert_ne!(
        sent, kept,
        "the fixture must actually carry something that leaves"
    );
    let plan = plan_claude_prompt(
        &store,
        session_id,
        "describe this",
        &[plan_attachment("photo.png", "image/png", &sent)],
    )
    .expect("materialized")
    .expect("a raster plans a block");
    assert_eq!(plan.fallback_text, "describe this", "no path line");
    assert_eq!(carried_mime_types(Some(&plan)), vec!["image/png"]);
    assert_eq!(plan.images.len(), 1);
    {
        use base64::Engine;
        assert_eq!(
            plan.images[0].data_base64,
            base64::engine::general_purpose::STANDARD.encode(&kept),
            "the block carries the stripped bytes"
        );
    }
    let bytes = frame_user_message_with_images(&plan.fallback_text, &plan.images).expect("frame");
    let line = std::str::from_utf8(&bytes).expect("utf8");
    assert!(line.ends_with('\n'));
    let value: Value = serde_json::from_str(line.trim_end()).expect("json");
    assert_eq!(value["type"], "user");
    assert_eq!(value["message"]["role"], "user");
    let content = value["message"]["content"]
        .as_array()
        .expect("content array");
    assert_eq!(content.len(), 2);
    assert_eq!(
        content[0],
        serde_json::json!({"type": "text", "text": "describe this"})
    );
    // The exact nested shape, pinned literally: `source` with
    // `media_type`, not the flat ACP `{type, mimeType, data}` no other
    // provider uses.
    assert_eq!(content[1]["type"], "image");
    assert_eq!(content[1]["source"]["type"], "base64");
    assert_eq!(content[1]["source"]["media_type"], "image/png");
    assert!(
        content[1]["source"]["data"].as_str().is_some(),
        "the Claude image block carries the stripped base64 under source"
    );
    assert!(content[1].get("mimeType").is_none(), "no flat mimeType");
    assert!(content[1].get("data").is_none(), "no flat data");
}

#[test]
fn an_svg_only_claude_prompt_plans_no_block_and_still_builds_the_legacy_text() {
    // SVG never becomes a block — no provider accepts it inline. The plan
    // still answers with the text, and that text is exactly what the
    // legacy write would have produced, which is why a block-less prompt
    // can take the route without moving a byte on the wire. (This test
    // used to assert `plan.is_none()`: the route answers with the text
    // now, so that the send path never walks the attachments twice.)
    let temp = PlanTempDir::new("svg-only");
    let store = AttachmentStore::new(&temp.0);
    let session_id = "claude-plan-svg-only";
    let source = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>";
    let attachment = plan_attachment("drawing.svg", "image/svg+xml", source);
    let plan = plan_claude_prompt(
        &store,
        session_id,
        "logo",
        std::slice::from_ref(&attachment),
    )
    .expect("materialized")
    .expect("an SVG plans no block, but the plan still carries the text");
    assert!(plan.images.is_empty(), "an SVG plans no block");
    let stored = store
        .session(session_id)
        .expect("session")
        .materialize(&attachment)
        .expect("stored");
    assert_eq!(
        plan.fallback_text,
        format!("logo\n\n[Image available at: {}]", stored.to_string_lossy()),
        "the plan's text is the legacy path line, byte for byte"
    );
}

#[test]
fn an_svg_keeps_its_path_line_beside_claude_image_blocks() {
    // A mixed prompt carries both: the raster as a block, the SVG as a
    // path line in the text.
    let temp = PlanTempDir::new("mixed");
    let store = AttachmentStore::new(&temp.0);
    let session_id = "claude-plan-mixed";
    let source = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>";
    let plan = plan_claude_prompt(
        &store,
        session_id,
        "logo and photo",
        &[
            plan_attachment("photo.png", "image/png", &clean_png(0x13)),
            plan_attachment("drawing.svg", "image/svg+xml", source),
        ],
    )
    .expect("materialized")
    .expect("the raster plans a block");
    assert_eq!(carried_mime_types(Some(&plan)), vec!["image/png"]);
    assert!(
        plan.fallback_text
            .starts_with("logo and photo\n\n[Image available at: "),
        "{}",
        plan.fallback_text
    );
    assert!(
        plan.fallback_text.ends_with(".svg]"),
        "{}",
        plan.fallback_text
    );
    assert!(
        !plan.fallback_text.contains(".png]"),
        "the raster left no path line: {}",
        plan.fallback_text
    );
    let bytes = frame_user_message_with_images(&plan.fallback_text, &plan.images).expect("frame");
    let value: Value =
        serde_json::from_str(std::str::from_utf8(&bytes).expect("utf8").trim_end()).expect("json");
    let content = value["message"]["content"].as_array().expect("array");
    assert_eq!(content.len(), 2);
    assert!(content[0]["text"]
        .as_str()
        .expect("text")
        .ends_with(".svg]"));
    assert_eq!(content[1]["type"], "image");
}

#[test]
fn a_jpeg_stays_a_jpeg_in_the_claude_block() {
    // The label `materialize` checked against the sniffed container is
    // the label the block carries.
    const EXIF_JPEG_VECTOR: &str =
        "a jpeg whose APP1 holds EXIF, between a kept JFIF APP0 and a kept ICC APP2";
    let temp = PlanTempDir::new("jpeg");
    let store = AttachmentStore::new(&temp.0);
    let sent = vector_input(EXIF_JPEG_VECTOR);
    let kept = vector_output(EXIF_JPEG_VECTOR);
    assert_ne!(sent, kept, "the vector must actually strip something");
    let plan = plan_claude_prompt(
        &store,
        "claude-plan-jpeg",
        "describe this",
        &[plan_attachment("photo.jpg", "image/jpeg", &sent)],
    )
    .expect("materialized")
    .expect("a JPEG plans a block");
    assert_eq!(plan.fallback_text, "describe this");
    assert_eq!(carried_mime_types(Some(&plan)), vec!["image/jpeg"]);
    {
        use base64::Engine;
        assert_eq!(
            plan.images[0].data_base64,
            base64::engine::general_purpose::STANDARD.encode(&kept),
            "stripped JPEG bytes, JPEG label"
        );
    }
}

#[test]
fn a_frame_without_blocks_is_the_text_only_frame() {
    // The static route frames every prompt it plans through the images
    // builder, including one whose blocks are all path lines (an SVG).
    // That has to be the frame the writer has always sent, or a
    // block-less prompt would move bytes the moment the route took it.
    assert_eq!(
        frame_user_message_with_images("logo", &[]).expect("frame"),
        frame_user_message("logo", None, None).expect("frame"),
    );
}

#[test]
fn the_static_route_frames_the_blocks_it_planned_through_the_mode_gate() {
    // The route owns the frame, and the gate still orders it: with the
    // initial mode response outstanding the frame is queued rather than
    // written, so a test can read back exactly what would have reached
    // the child. No child and no stdin are involved.
    let temp = PlanTempDir::new("route");
    let store = AttachmentStore::new(&temp.0);
    let session_id = "claude-route";
    let sent = png_with_text_chunk();
    // Cutting the removed `tEXt` chunk out of `sent` is exactly this
    // container (`raster_metadata::png_with_text_chunk` says so), so the
    // frame below must carry these bytes and not the ones that arrived.
    let kept = clean_png(0x01);
    assert_ne!(
        sent, kept,
        "the fixture must actually carry something that leaves"
    );
    let gate: ClaudeModeGateRef = Arc::new(Mutex::new(ClaudeModeGate {
        state: ClaudeModeGateState::AwaitingResponse {
            request_id: "initial-permission-mode-0".to_string(),
            requested_mode: "default".to_string(),
        },
        pending_frames: Vec::new(),
    }));
    let route = ClaudeStaticPrompt::new(Arc::new(Mutex::new(None)), Some(Arc::clone(&gate)));
    let plan = route
        .plan_prompt(
            &store,
            session_id,
            "describe this",
            &[plan_attachment("photo.png", "image/png", &sent)],
        )
        .expect("planned")
        .expect("a raster plans a frame");
    assert_eq!(plan.text(), "describe this", "no path line");
    plan.send().expect("send");
    let frames = gate.lock().expect("gate").pending_frames.clone();
    assert_eq!(frames.len(), 1, "the frame queued behind the mode response");
    let value: Value = serde_json::from_slice(&frames[0]).expect("json");
    assert_eq!(value["type"], "user");
    let content = value["message"]["content"].as_array().expect("array");
    assert_eq!(content.len(), 2);
    assert_eq!(
        content[0],
        serde_json::json!({"type": "text", "text": "describe this"})
    );
    assert_eq!(content[1]["source"]["type"], "base64");
    assert_eq!(content[1]["source"]["media_type"], "image/png");
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(&kept);
    assert_eq!(
        content[1]["source"]["data"].as_str(),
        Some(encoded.as_str()),
        "the frame carries the stripped bytes"
    );
}

#[test]
fn the_static_route_declines_a_prompt_with_no_attachments() {
    // A plain text prompt has nothing to plan: the route answers `None`
    // and the send path writes it through the writer, exactly as before.
    let temp = PlanTempDir::new("route-none");
    let store = AttachmentStore::new(&temp.0);
    let route = ClaudeStaticPrompt::new(Arc::new(Mutex::new(None)), None);
    assert!(route
        .plan_prompt(&store, "claude-route-none", "describe this", &[])
        .expect("planned")
        .is_none());
}

fn model(id: &str, effort_ids: Option<Vec<&str>>) -> SessionModel {
    SessionModel {
        model_id: id.to_string(),
        name: id.to_string(),
        description: None,
        context_tokens: None,
        current_effort: None,
        efforts: effort_ids.map(|ids| {
            ids.into_iter()
                .map(|effort| SessionModelEffort {
                    id: effort.to_string(),
                    label: effort.to_string(),
                    description: None,
                    default: None,
                })
                .collect()
        }),
    }
}

/// Model — refuse, never substitute: the mismatch sentence names the
/// published list the id is missing from.
#[test]
fn a_claude_model_outside_the_vocabulary_is_refused_with_the_mismatch_sentence() {
    let models = vec![model("claude-opus-5", None), model("claude-sonnet-5", None)];
    let error = validate_model_choice(&models, Some("claude-bogus-9"))
        .expect_err("an unknown model id must be refused");
    assert!(
        error.message.contains("claude-bogus-9"),
        "{}",
        error.message
    );
    assert!(
        error.message.contains("is not among the models"),
        "the mismatch sentence, not the absence one: {}",
        error.message
    );
}

/// An empty published list is a different refusal from an unknown id:
/// there is no list the name could have been a typo from.
///
/// The state this test constructs is one production does not build: the
/// R2a audit's F10 found no production path to an empty vocabulary (the
/// fallback lists three aliases, a derivation never caches empty, and
/// since the F4 fix a provisional catalog is not judged at all). The
/// arm's remaining reachable input is a hand-edited cache file carrying
/// `models: []`; the sentence is pinned for that state, and for its own
/// integrity as the absence half of the two-sentence split.
#[test]
fn a_claude_model_against_an_empty_vocabulary_is_refused_with_the_absence_sentence() {
    let error = validate_model_choice(&[], Some("claude-bogus-9"))
        .expect_err("an empty vocabulary must be refused");
    assert!(
        error.message.contains("publishes no models"),
        "the absence sentence, not the mismatch one: {}",
        error.message
    );
    assert!(
        !error.message.contains("is not among"),
        "the two sentences must stay distinct: {}",
        error.message
    );
}

/// A model the catalog does publish is delivered.
#[test]
fn a_claude_model_inside_the_vocabulary_is_delivered() {
    let models = vec![model("claude-opus-5", None)];
    validate_model_choice(&models, Some("claude-opus-5")).expect("delivered");
}

/// Thinking — the absence sentence ("this model has no thinking options")
/// is not the mismatch sentence ("that option is not among the model's").
/// The ninth catch: collapsing them sends a human hunting a typo when the
/// provider simply has no dial.
#[test]
fn claude_thinking_absence_and_mismatch_are_two_distinct_refusals() {
    let models = vec![
        model("claude-no-efforts", None),
        model("claude-opus-5", Some(vec!["low", "high"])),
    ];
    let error = validate_thinking_choice(&models, "claude-no-efforts", "high")
        .expect_err("a model without thinking options must be refused");
    assert!(
        error.message.contains("has no thinking options"),
        "the absence sentence: {}",
        error.message
    );

    let error = validate_thinking_choice(&models, "claude-opus-5", "bogus")
        .expect_err("an unknown thinking option must be refused");
    assert!(
        error.message.contains("is not among"),
        "the mismatch sentence: {}",
        error.message
    );
    assert!(
        !error.message.contains("has no thinking options"),
        "the two sentences must stay distinct: {}",
        error.message
    );

    validate_thinking_choice(&models, "claude-opus-5", "high").expect("delivered");
}

/// `autoAccept` is a constraint on which mode is delivered: the only
/// The tick contradiction, walked over the **whole** closed mode
/// vocabulary `claude_view::mode_state` declares — not two hand-picked
/// ids (the R2a audit's F9): for every mode, a tick is admitted exactly
/// when the daemon's own broker answers that mode, and refused with the
/// contradiction sentence otherwise. A provisional catalog stands in for
/// the model axis, which this test does not exercise: the mode and tick
/// are judged before the catalog is ever consulted.
#[test]
fn a_claude_auto_accept_tick_over_an_asking_mode_is_refused() {
    let catalog = ClaudeCatalogSnapshot::provisional(crate::claude_catalog::fallback_models());
    let vocabulary = crate::claude_view::mode_state("default")
        .available_modes
        .iter()
        .map(|mode| mode.id.clone())
        .collect::<Vec<_>>();
    assert!(
        vocabulary.len() >= 5,
        "the vocabulary this test walks is the one claude_view declares: {vocabulary:?}"
    );
    for mode_id in &vocabulary {
        let mut delivery = ProfileDelivery::for_request(Some(mode_id.clone()));
        delivery.auto_accept = true;
        if crate::provider_catalog::mode_is_auto_answered(mode_id) {
            validate_delivery(&catalog, &delivery).unwrap_or_else(|error| {
                panic!("{mode_id} answers its own prompts: {}", error.message)
            });
        } else {
            let error = validate_delivery(&catalog, &delivery)
                .expect_err("a tick over an asking mode is the contradiction");
            assert!(
                error.message.contains("contradict") && error.message.contains(mode_id),
                "the refusal names both halves for {mode_id}: {}",
                error.message
            );
        }

        // The same mode without the tick is fine everywhere.
        let mut delivery = ProfileDelivery::for_request(Some(mode_id.clone()));
        delivery.auto_accept = false;
        validate_delivery(&catalog, &delivery)
            .unwrap_or_else(|error| panic!("{mode_id} without the tick: {}", error.message));
    }
}

/// The launch argv is where the model is delivered to a Claude child: the
/// delivered model wins, and only a create that resolved no profile falls
/// back to the catalog's preference. The `--model` flag is the delivery —
/// dropping the delivered value here would start a child on a model the
/// card did not name, at a price the human did not approve.
#[test]
fn the_delivered_model_is_what_the_claude_argv_pins() {
    let models = vec![model("claude-opus-5", None), model("claude-sonnet-5", None)];
    let mut delivery = ProfileDelivery::none();
    delivery.model_id = Some("claude-sonnet-5".to_string());
    let args = launch_with_model(
        launch_in_bypass_mode(vec!["-p".to_string()]),
        launch_model_id(&delivery, &models).as_deref(),
    );
    assert_eq!(
        args.windows(2)
            .find(|pair| pair[0] == "--model")
            .map(|pair| pair[1].as_str()),
        Some("claude-sonnet-5"),
        "the delivered model is the launch flag, never the catalog default"
    );

    // No profile named a model: the catalog's preference, as before.
    let args = launch_with_model(
        launch_in_bypass_mode(vec!["-p".to_string()]),
        launch_model_id(&ProfileDelivery::none(), &models).as_deref(),
    );
    assert_eq!(
        args.windows(2)
            .find(|pair| pair[0] == "--model")
            .map(|pair| pair[1].as_str()),
        Some("claude-opus-5")
    );
}
/// The frame's shape, checked where it is built: a stored `true` names the
/// setting the CLI reads, and a refusal of it fails the session rather than
/// leaving a child running unflagged behind a card that said it was fast.
#[test]
fn the_fast_mode_frame_names_the_flag_and_its_refusal_names_it_back() {
    let mut harness = initial_mode_test_setup();
    let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let delivery_settings: ClaudeDeliverySettings = Arc::new(Mutex::new(HashMap::new()));
    let fast_request_id =
        send_initial_fast_mode(&harness.stdin, &harness.next_id, &delivery_settings, true)
            .expect("the fast-mode frame is written synchronously");
    let mut reader = ClaudeReader::with_mode_gate(
        ClaudeView::new(Some(PathBuf::from(r"C:\work"))),
        Arc::clone(&broker),
        Arc::new(Mutex::new(HashMap::new())),
        Arc::new(Mutex::new(HashMap::new())),
        Arc::clone(&harness.next_id),
        ClaudeModeGateWiring {
            stdin: Arc::clone(&harness.stdin),
            gate: Arc::clone(&harness.gate),
            timeout: CONTROL_RESPONSE_TIMEOUT,
            delivery_settings,
        },
    );
    let (runtime, conn) = attached(&broker);

    let mode_request = read_json_line(&mut harness.stdout);
    assert_eq!(mode_request["request"]["subtype"], "set_permission_mode");
    let fast_echo = read_json_line(&mut harness.stdout);
    assert_eq!(
        fast_echo["request"]["subtype"], "apply_flag_settings",
        "the flag rides the settings frame, not a new verb: {fast_echo}"
    );
    assert_eq!(fast_echo["request_id"], fast_request_id);
    assert_eq!(
        fast_echo["request"]["settings"]["fastMode"],
        serde_json::json!(true),
        "the setting is the one Paseo's SDK writes: {fast_echo}"
    );

    let refusal = serde_json::json!({
        "type": "control_response",
        "response": {
            "subtype": "error",
            "request_id": fast_request_id,
            "error": "unknown setting fastMode",
        }
    });
    reader
        .feed(
            format!(
                "{refusal}
"
            )
            .as_bytes(),
            &runtime,
        )
        .expect("fast-mode response");
    let events = drain(&conn);
    assert!(
        events.iter().any(|event| matches!(
            event,
            SessionEvent::AgentError { message }
                if message.contains("refused the delivered fast mode")
                    && message.contains("unknown setting fastMode")
        )),
        "a CLI that will not take the flag fails the session, naming it: {:?}",
        slice_of_kinds(&events)
    );
}

/// R2a F2, the write: the delivery's effort frame is written
/// **synchronously**, so a stdin that cannot take it refuses the
/// delivery here — at the spawn, with the child still young — instead of
/// failing silently on a thread nobody joins.
#[test]
fn an_effort_frame_that_cannot_be_written_refuses_the_delivery() {
    let stdin: Arc<Mutex<Option<ChildStdin>>> = Arc::new(Mutex::new(None));
    let efforts: ClaudeDeliverySettings = Arc::new(Mutex::new(HashMap::new()));
    let error = send_initial_effort(&stdin, &AtomicU64::new(1), &efforts, "low")
        .expect_err("a closed stdin refuses the effort delivery");
    assert!(
        format!("{error}").contains("stdin is closed"),
        "the write failure is the delivery's refusal: {error}"
    );
    assert!(
        efforts.lock().expect("efforts").is_empty(),
        "a request that never reached the pipe is not left pending"
    );
}

/// R2a F2, the answer: the delivery's effort request is **tracked**. A
/// CLI that refuses `apply_flag_settings` fails the session the way a
/// refused initial mode does — an AgentError naming the refused effort,
/// stdin closed so the child cannot go on answering at its own level —
/// instead of the response disappearing while the card's promise quietly
/// does not hold.
#[test]
fn a_refused_delivery_effort_fails_the_session_instead_of_passing_silently() {
    let mut harness = initial_mode_test_setup();
    let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let delivery_settings: ClaudeDeliverySettings = Arc::new(Mutex::new(HashMap::new()));
    // The spawn's delivery, in pipe order right behind the mode frame.
    let effort_request_id =
        send_initial_effort(&harness.stdin, &harness.next_id, &delivery_settings, "low")
            .expect("the effort frame is written synchronously");
    let mut reader = ClaudeReader::with_mode_gate(
        ClaudeView::new(Some(PathBuf::from(r"C:\work"))),
        Arc::clone(&broker),
        Arc::new(Mutex::new(HashMap::new())),
        Arc::new(Mutex::new(HashMap::new())),
        Arc::clone(&harness.next_id),
        ClaudeModeGateWiring {
            stdin: Arc::clone(&harness.stdin),
            gate: Arc::clone(&harness.gate),
            timeout: CONTROL_RESPONSE_TIMEOUT,
            delivery_settings,
        },
    );
    let (runtime, conn) = attached(&broker);
    let mut writer = ClaudeWriter {
        stdin: Arc::clone(&harness.stdin),
        pending: Vec::new(),
        mode_gate: Some(Arc::clone(&harness.gate)),
    };
    writer.write_all(b"Reply DONE").expect("buffer prompt");
    writer.flush().expect("queue prompt");

    // The pipe already holds mode, then effort: the effort frame's echo
    // is readable before any response was fed, which is the ordering the
    // synchronous write buys.
    let mode_request = read_json_line(&mut harness.stdout);
    assert_eq!(mode_request["request"]["subtype"], "set_permission_mode");
    let effort_echo = read_json_line(&mut harness.stdout);
    assert_eq!(
        effort_echo["request"]["subtype"], "apply_flag_settings",
        "the effort frame is second on the wire: {effort_echo}"
    );
    assert_eq!(effort_echo["request_id"], effort_request_id);

    // The CLI confirms the mode; the gate opens and the prompt flows.
    let response = initial_mode_response(&mode_request);
    reader
        .feed(format!("{response}\n").as_bytes(), &runtime)
        .expect("mode response");
    let prompt = read_json_line(&mut harness.stdout);
    assert_eq!(prompt["message"]["content"][0]["text"], "Reply DONE");

    // Then the CLI refuses the delivered effort. The response must not
    // be swallowed.
    let refusal = serde_json::json!({
        "type": "control_response",
        "response": {
            "subtype": "error",
            "request_id": effort_request_id,
            "error": "effort level not supported",
        }
    });
    reader
        .feed(format!("{refusal}\n").as_bytes(), &runtime)
        .expect("effort response");

    let events = drain(&conn);
    assert!(
        events.iter().any(|event| matches!(
            event,
            SessionEvent::AgentError { message }
                if message.contains("refused the delivered thinking option 'low'")
                    && message.contains("effort level not supported")
        )),
        "the refusal is published, naming the effort and the CLI's words: {:?}",
        slice_of_kinds(&events)
    );
    assert!(
        matches!(
            harness.gate.lock().expect("gate").state,
            ClaudeModeGateState::Failed(_)
        ),
        "the gate is failed: nothing further flows"
    );
    assert!(
        harness.stdin.lock().expect("stdin").is_none(),
        "the transport is closed so the child cannot answer anything"
    );
    drop(writer);
    let _ = harness.child.kill();
    let _ = harness.child.wait();
}

fn slice_of_kinds(events: &[SessionEvent]) -> Vec<String> {
    events
        .iter()
        .map(|event| match event {
            SessionEvent::AgentError { message } => {
                format!("error({})", message.chars().take(120).collect::<String>())
            }
            other => format!("{other:?}"),
        })
        .collect()
}

/// R2a F4: a **provisional** catalog is no vocabulary, and the creation
/// path now refuses to judge on it, exactly as the runtime path
/// (`validate_claude_effort`) already does. A CLI upgrade empties the
/// version-keyed cache and the fallback's three aliases are a
/// placeholder, not a list to judge a saved id against; judging it there
/// manufactured intermittent refusals of legitimate profiles. The
/// derived catalog judges, with the suffix-tolerant comparison the
/// runtime paths use.
#[test]
fn a_model_is_not_judged_against_a_provisional_catalog_and_the_derived_one_matches_suffixes() {
    fn delivery(model: &str, thinking: Option<&str>) -> ProfileDelivery {
        ProfileDelivery::for_child("default", model, thinking, &serde_json::Map::new())
    }

    fn derived_with(model_id: &str, efforts: &[&str]) -> ClaudeCatalogSnapshot {
        ClaudeCatalogSnapshot::derived(vec![model(model_id, Some(efforts.to_vec()))])
    }

    // Provisional: the fallback aliases are not a vocabulary to judge
    // with, so a profile naming a real derived id is admitted, not
    // refused with a sentence blaming a typo that is not there.
    let provisional = ClaudeCatalogSnapshot::provisional(crate::claude_catalog::fallback_models());
    validate_delivery(
        &provisional,
        &delivery("claude-opus-5-20260101", Some("high")),
    )
    .expect("a provisional catalog judges nothing");

    // Derived: the real list judges, through the suffix-tolerant match
    // the live switch uses — the CLI's own `[1m]` spelling is a real,
    // displayed model id.
    let derived = derived_with("claude-opus-5-20260101[1m]", &["low", "high"]);
    validate_delivery(&derived, &delivery("claude-opus-5-20260101", Some("high")))
        .expect("the `[1m]` spelling matches without its suffix");

    // An id the derived list genuinely does not carry is still refused —
    // the mismatch sentence, not the absence one.
    let error = validate_delivery(&derived, &delivery("claude-bogus", None))
        .expect_err("an unknown id in a derived catalog is refused");
    assert!(
        error.message.contains("is not among the models"),
        "the mismatch sentence: {}",
        error.message
    );

    // The thinking axis still judges against the delivered model's own
    // options once the catalog is derived.
    let error = validate_delivery(&derived, &delivery("claude-opus-5-20260101", Some("bogus")))
        .expect_err("an unknown thinking option is refused");
    assert!(
        error.message.contains("is not among model"),
        "the thinking mismatch sentence: {}",
        error.message
    );
}

/// P1 of the review: a delivered fast-mode flag is **confirmed before the child
/// is accepted**, and each of the three answers is its own outcome. The base
/// write-and-forget shape left a create path that returned a running session
/// while the CLI had either refused the setting or never answered it — a card
/// whose promise nobody had checked, which is the exact rule the delivery
/// carries ("a child that exists was delivered everything its card printed").
///
/// The `node` echo child stands in for the CLI: the frames this client writes
/// come back on its stdout, and the test writes the control responses the real
/// CLI would send.
#[test]
fn a_delivered_fast_mode_is_confirmed_on_the_create_path_in_all_three_answers() {
    // Success: accepted, and the lines that were not the answer are handed to
    // the session reader rather than dropped.
    let mut harness = initial_mode_test_setup();
    let settings: ClaudeDeliverySettings = Arc::new(Mutex::new(HashMap::new()));
    let fast_id =
        send_initial_fast_mode(&harness.stdin, &harness.next_id, &settings, true).expect("written");
    let mode_request = read_json_line_bounded(&mut harness.stdout);
    assert_eq!(mode_request["request"]["subtype"], "set_permission_mode");
    let fast_request = read_json_line_bounded(&mut harness.stdout);
    assert_eq!(fast_request["request"]["subtype"], "apply_flag_settings");
    assert_eq!(
        fast_request["request"]["settings"]["fastMode"],
        serde_json::json!(true),
        "the setting Paseo's SDK writes, in the frame this family already uses: {fast_request}"
    );
    // A line that is not the answer — the init event the real CLI sends first.
    let init = serde_json::json!({"type": "system", "subtype": "init"});
    write_line_to_child(&harness.stdin, &init);
    let mut prelude = Vec::new();
    let success = serde_json::json!({
        "type": "control_response",
        "response": {"subtype": "success", "request_id": fast_id}
    });
    write_line_to_child(&harness.stdin, &success);
    confirm_delivery_settings(
        &mut harness.stdout,
        &mut prelude,
        &fast_id,
        &settings,
        CONTROL_RESPONSE_TIMEOUT,
    )
    .expect("the CLI took the flag");
    // Everything the wait read is handed on, in order — the init line it did not
    // need and the answer it did. The session reader parses both through the one
    // parser that understands them; the answer is then inert, because its
    // request id is no longer registered.
    let replayed = String::from_utf8_lossy(&prelude).to_string();
    assert!(
        replayed.contains("\"subtype\":\"init\""),
        "the init line the wait read past is replayed, not dropped: {replayed}"
    );
    assert!(
        replayed.find("\"subtype\":\"init\"") < replayed.find(&fast_id),
        "the replay keeps the order the pipe delivered: {replayed}"
    );
    assert!(
        settings.lock().expect("settings").is_empty(),
        "the answered request is retired, so the reader ignores it: {settings:?}"
    );

    let _ = harness.child.kill();
    let _ = harness.child.wait();

    // Error: refused, in a sentence naming the setting and the CLI's reason.
    let mut harness = initial_mode_test_setup();
    let settings: ClaudeDeliverySettings = Arc::new(Mutex::new(HashMap::new()));
    let fast_id =
        send_initial_fast_mode(&harness.stdin, &harness.next_id, &settings, true).expect("written");
    let _ = read_json_line_bounded(&mut harness.stdout);
    let _ = read_json_line_bounded(&mut harness.stdout);
    let refusal = serde_json::json!({
        "type": "control_response",
        "response": {
            "subtype": "error", "request_id": fast_id,
            "error": "fast mode needs a newer Claude CLI"
        }
    });
    write_line_to_child(&harness.stdin, &refusal);
    let mut prelude = Vec::new();
    let error = confirm_delivery_settings(
        &mut harness.stdout,
        &mut prelude,
        &fast_id,
        &settings,
        CONTROL_RESPONSE_TIMEOUT,
    )
    .expect_err("a refused flag refuses the creation");
    assert!(
        error.message.contains("refused the delivered fast mode")
            && error.message.contains("fast mode needs a newer Claude CLI"),
        "the refusal names the setting and the CLI's words: {}",
        error.message
    );

    let _ = harness.child.kill();
    let _ = harness.child.wait();

    // Silence: also refused. This is the case the async reader could never
    // answer - no line ever arrives, so nothing was ever published, and the
    // child ran on its own default behind a card that said otherwise.
    let mut harness = initial_mode_test_setup();
    let settings: ClaudeDeliverySettings = Arc::new(Mutex::new(HashMap::new()));
    let fast_id =
        send_initial_fast_mode(&harness.stdin, &harness.next_id, &settings, true).expect("written");

    let _ = read_json_line_bounded(&mut harness.stdout);

    let _ = read_json_line_bounded(&mut harness.stdout);

    let mut prelude = Vec::new();
    let error = confirm_delivery_settings(
        &mut harness.stdout,
        &mut prelude,
        &fast_id,
        &settings,
        Duration::from_millis(200),
    )
    .expect_err("a CLI that never answers must not be accepted");
    assert!(
        error.message.contains("did not answer") && error.message.contains("creation is refused"),
        "the timeout is a refusal, not a silence: {error:?}"
    );
    assert!(
        settings.lock().expect("settings").is_empty(),
        "an unanswered request is not left registered for the reader to answer later"
    );
    let _ = harness.child.kill();
    let _ = harness.child.wait();
}

/// `read_json_line` with a real bound, through the same non-blocking reader the
/// delivery confirmation uses. `BufRead::read_line` blocks until a line arrives,
/// so a deadline checked after it is unreachable — and a test that waits on a
/// line that never comes shows up as a hung suite, not a failed assertion.
fn read_json_line_bounded(stdout: &mut BufReader<ChildStdout>) -> Value {
    let text = crate::session::acp_client::read_line_bounded(
        stdout,
        std::time::Instant::now() + std::time::Duration::from_secs(10),
        std::time::Duration::from_secs(10),
    )
    .expect("a line from the echo child within 10s");
    serde_json::from_str(text.trim_end()).expect("child output json")
}

/// One JSON line into the echo child's stdin, which the child writes straight
/// back to the pipe this client reads. Written through the same `Arc` the client
/// holds, because the harness moved the child's own handle into it — the line is
/// a byte string, not a value the client parses.
fn write_line_to_child(stdin: &Arc<Mutex<Option<ChildStdin>>>, value: &Value) {
    use std::io::Write;
    let mut bytes = serde_json::to_vec(value).expect("json");
    // One LF, as a number: a char literal for it kept being rewritten into a
    // real newline by the scripted edits this file went through.
    bytes.push(10);
    let mut guard = stdin.lock().expect("stdin");
    let child_stdin = guard.as_mut().expect("child stdin");
    child_stdin.write_all(&bytes).expect("write");
    child_stdin.flush().expect("flush");
}
