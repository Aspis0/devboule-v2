//! Tests for the Codex client: the app-server handshake, turns and model catalog.

use std::path::Path;

use super::super::codex_input_requests::{permission_decision, permission_decision_frame};
use super::super::event_pull::ConnHandle;
use super::super::permission_broker::PermissionBroker;
use super::super::session_runtime::SessionRuntime;
use super::{
    carried_image_paths, codex_delivery, codex_local_image_entry, command_prompt_input,
    empty_commands, initialize_params, interrupt_params, mcp_launch, mode_values,
    notification_frame, plan_codex_prompt, request_frame, send_interrupt_request,
    steer_params_if_current, thread_resume_params, thread_start_params, turn_id_from_response,
    turn_start_params, turn_start_params_for_prompt, turn_start_params_with_images,
    turn_steer_params, validate_mode, CodexCommands, CodexReader, CodexRequests, CodexSteerer,
    ThreadRoad,
};
use crate::attachment_store::AttachmentStore;
use crate::codex_view::{
    catalog_from_response, fixture_frames, CodexState, CodexStdout, CodexView,
};
use crate::raster_metadata::{clean_png, png_with_text_chunk, vector_input, vector_output};
use crate::session::ReaderDispatch;
use devboule_protocol::PromptAttachment;
use devboule_protocol::SessionEvent;
use devboule_protocol::WireError;
use std::collections::HashMap;
use std::sync::atomic::AtomicU64;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

fn method_frame(source: &str, method: &str) -> serde_json::Value {
    fixture_frames(source)
        .into_iter()
        .find(|frame| frame.get("method").and_then(serde_json::Value::as_str) == Some(method))
        .expect("measured request")
}
#[test]
fn codex_modes_use_the_measured_policy_shapes() {
    assert_eq!(
        mode_values("auto"),
        serde_json::json!({
            "approvalPolicy": "on-request",
            "sandboxPolicy": {
                "type": "workspaceWrite",
                "networkAccess": false,
                "writableRoots": []
            }
        })
        .as_object()
        .expect("object")
        .clone()
    );
    assert_eq!(
        mode_values("auto-review").get("approvalsReviewer"),
        Some(&serde_json::json!("auto_review"))
    );
    assert_eq!(
        mode_values("full-access"),
        serde_json::json!({
            "approvalPolicy": "never",
            "sandboxPolicy": { "type": "dangerFullAccess" }
        })
        .as_object()
        .expect("object")
        .clone()
    );
    assert!(validate_mode("bypass").is_err());
}

#[test]
fn measured_control_frames_and_turn_modes_keep_the_wire_shapes() {
    let thread = method_frame(
        include_str!("../fixtures/wire/codex/E1-step3-thread.jsonl"),
        "thread/start",
    );
    let cwd = thread["params"]["cwd"].as_str().expect("cwd");
    assert_eq!(
        thread["params"],
        thread_start_params(Path::new(cwd), "auto")
    );

    let initialized = method_frame(
        include_str!("../fixtures/wire/codex/E1-step1-handshake.jsonl"),
        "initialized",
    );
    assert_eq!(
        initialized,
        notification_frame("initialized", serde_json::json!({}))
    );

    let changed = method_frame(
        include_str!("../fixtures/wire/codex/E1-step6-modechange.jsonl"),
        "turn/start",
    );
    let params = &changed["params"];
    assert_eq!(
        *params,
        turn_start_params(
            params["threadId"].as_str().expect("thread id"),
            params["input"][0]["text"].as_str().expect("prompt"),
            Some("full-access"),
            None,
            None,
            None,
        )
    );
    let interrupt = method_frame(
        include_str!("../fixtures/wire/codex/E1-step6b-interrupt.jsonl"),
        "turn/interrupt",
    );
    assert_eq!(
        interrupt["params"],
        interrupt_params(
            interrupt["params"]["threadId"].as_str().expect("thread id"),
            interrupt["params"]["turnId"].as_str().expect("turn id"),
        )
    );

    for (outcome, decision) in [
        (
            serde_json::json!({"outcome":{"outcome":"selected","optionId":"allow"}}),
            "accept",
        ),
        (
            serde_json::json!({"outcome":{"outcome":"selected","optionId":"deny"}}),
            "decline",
        ),
        (
            serde_json::json!({"outcome":{"outcome":"cancelled"}}),
            "cancel",
        ),
    ] {
        assert_eq!(permission_decision(&outcome), decision);
        let mut bytes =
            serde_json::to_vec(&permission_decision_frame(&serde_json::json!(5), decision))
                .expect("response json");
        bytes.push(b'\n');
        let response: serde_json::Value = serde_json::from_slice(&bytes).expect("response");
        assert_eq!(response["result"]["decision"], decision);
    }
}

#[test]
fn turn_start_carries_the_policy_only_after_a_mode_change() {
    let catalog = catalog_from_response(&serde_json::json!({
        "data": [{ "id": "model", "isDefault": true }]
    }))
    .expect("catalog");
    let state = CodexState::new("thread".to_string(), catalog, "auto");
    assert_eq!(state.mode_override(), None);

    let first = turn_start_params(
        &state.thread_id(),
        "first",
        state.mode_override().as_deref(),
        None,
        None,
        state.service_tier().as_deref(),
    );
    assert!(first.get("approvalPolicy").is_none());
    assert!(first.get("sandboxPolicy").is_none());

    state.set_mode("read-only").expect("set mode");
    let changed = turn_start_params(
        &state.thread_id(),
        "changed",
        state.mode_override().as_deref(),
        None,
        None,
        state.service_tier().as_deref(),
    );
    assert_eq!(changed["approvalPolicy"], "on-request");
    assert_eq!(changed["sandboxPolicy"]["type"], "readOnly");

    // Paseo keeps `hasWorkflowModeOverride` set, so every later turn
    // re-sends the policy too.
    let later = turn_start_params(
        &state.thread_id(),
        "later",
        state.mode_override().as_deref(),
        None,
        None,
        state.service_tier().as_deref(),
    );
    assert_eq!(later["sandboxPolicy"]["type"], "readOnly");
}

#[test]
fn read_only_thread_start_sends_the_read_only_sandbox() {
    let params = thread_start_params(Path::new("C:\\work"), "read-only");
    assert_eq!(params["approvalPolicy"], "on-request");
    assert_eq!(params["sandbox"], "read-only");
}

#[test]
fn initialize_request_includes_paseo_capabilities_on_the_wire() {
    let frame = super::request_frame("d-1", "initialize", initialize_params());
    let mut bytes = serde_json::to_vec(&frame).expect("initialize request");
    bytes.push(b'\n');
    let wire = std::str::from_utf8(&bytes).expect("initialize bytes");
    assert!(wire.contains(r#""experimentalApi":true"#));
    assert!(wire.contains(r#""mcpServerOpenaiFormElicitation":true"#));
    assert_eq!(
        frame["params"]["clientInfo"],
        serde_json::json!({
            "name": "codex_app_server_daemon",
            "title": "Codex App Server Daemon",
            "version": "0.0.0"
        })
    );
}

#[test]
fn unknown_server_request_gets_a_method_not_supported_error() {
    use std::io::{BufRead, BufReader};

    // S4-06: the fake app-server is a `node` script, so this skips where
    // there is no node rather than failing there, like every other
    // node-backed test in this file.
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let mut child = std::process::Command::new("node")
        .args([
            "-e",
            "process.stdin.on('data', data => process.stdout.write(data))",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("node is required for the Codex request test");
    let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
    let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let catalog = catalog_from_response(&serde_json::json!({
        "data": [{ "id": "model", "isDefault": true }]
    }))
    .expect("catalog");
    let mut reader = CodexReader {
        commands: empty_commands(),
        available_commands: None,
        buffer: Vec::new(),
        discarding_oversized_line: false,
        deferred: Vec::new(),
        manifest: None,
        state: Arc::new(CodexState::new("thread".to_string(), catalog, "auto")),
        view: CodexView::new(None),
        permission_broker: Arc::clone(&broker),
        response_ids: Arc::new(Mutex::new(HashMap::new())),
        stdin,
        next_id: Arc::new(AtomicU64::new(1)),
        spawn_nonce: "test-spawn".to_string(),
        requests: Arc::new(CodexRequests::new()),
        compactions: crate::codex_compaction::CodexCompactions::default(),
    };
    let runtime = Arc::new(SessionRuntime::new());
    let request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "server-17",
        "method": "codex/futureRequest",
        "params": {}
    });
    reader.dispatch_value(request, &runtime);
    let mut line = String::new();
    stdout.read_line(&mut line).expect("response");
    let response: serde_json::Value = serde_json::from_str(&line).expect("response json");
    assert_eq!(
        response,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": "server-17",
            "error": { "code": -32601, "message": "method not supported" }
        })
    );
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn interrupt_without_a_current_turn_is_an_ok_noop() {
    let catalog = catalog_from_response(&serde_json::json!({
        "data": [{ "id": "model", "isDefault": true }]
    }))
    .expect("catalog");
    let state = CodexState::new("thread".to_string(), catalog, "auto");
    let stdin = Mutex::new(None);
    let next_id = AtomicU64::new(1);
    assert!(matches!(
        send_interrupt_request(&stdin, &next_id, &state),
        Ok(false)
    ));
    assert_eq!(next_id.load(std::sync::atomic::Ordering::Relaxed), 1);
}

#[test]
fn turn_start_response_records_the_turn_before_started_notification() {
    let catalog = catalog_from_response(&serde_json::json!({
        "data": [{ "id": "model", "isDefault": true }]
    }))
    .expect("catalog");
    let state = CodexState::new("thread".to_string(), catalog, "auto");
    let response = serde_json::json!({
        "jsonrpc": "2.0",
        "id": "d-7",
        "result": { "turn": { "id": "turn-7" } }
    });
    state.set_turn(turn_id_from_response(&response));
    assert_eq!(state.current_turn().as_deref(), Some("turn-7"));
}

// --- image delivery (the static route) --------------------------------
//
// The routing decision lives in `plan_codex_prompt`, tested here
// against the attachment store directly, without spawning a child — the
// same arrangement the ACP sibling seam's tests use. The wire shape of
// one entry is pinned against the live-verified `localImage` input:
// `{"type":"localImage","path":...}` with no `detail`.

struct PlanTempDir(std::path::PathBuf);

impl PlanTempDir {
    fn new(tag: &str) -> Self {
        let dir = crate::test_dirs::test_temp_dir(&format!("devboule-codex-plan-{tag}"));
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
fn codex_delivery_is_the_static_variant() {
    // No handshake to negotiate with and deliberately no probe: an
    // unknown method on this surface answers `-32600`, not `-32601`, so
    // a method-not-found fallback would never fire. The format accepts
    // images, so the delivery is the static one the route reads.
    assert_eq!(
        codex_delivery(),
        super::super::ImageDelivery::StaticImageBlock
    );
}

#[test]
fn a_capable_codex_prompt_plans_local_image_paths_and_no_path_line() {
    // Each raster becomes one `localImage` path; the text is the bare
    // user text, with no path line.
    let temp = PlanTempDir::new("capable");
    let store = AttachmentStore::new(&temp.0);
    let session_id = "codex-plan-capable";
    // A container the walk accepts but changes: what sits at the planned
    // path must be the stripped bytes, never the wire bytes.
    let sent = png_with_text_chunk();
    let kept = clean_png(0x01);
    assert_ne!(
        sent, kept,
        "the fixture must actually carry something that leaves"
    );
    let plan = plan_codex_prompt(
        &store,
        session_id,
        "describe this",
        &[plan_attachment("photo.png", "image/png", &sent)],
    )
    .expect("materialized")
    .expect("a raster plans paths");
    assert_eq!(plan.fallback_text, "describe this", "no path line");
    assert_eq!(carried_image_paths(Some(&plan)).len(), 1);
    let planned = carried_image_paths(Some(&plan))[0];
    assert_eq!(
        std::fs::read(planned).expect("read"),
        kept,
        "the path names the stripped file"
    );
    let params = turn_start_params_with_images(
        "thread-1",
        &plan.fallback_text,
        &plan.image_paths,
        None,
        None,
        None,
        None,
    );
    assert_eq!(params["threadId"], "thread-1");
    let input = params["input"].as_array().expect("input array");
    assert_eq!(input.len(), 2);
    assert_eq!(
        input[0],
        serde_json::json!({"type": "text", "text": "describe this"})
    );
    // The exact entry shape, pinned literally: `type` plus `path`, no
    // `detail`, and `localImage` — not the unmeasured `image` the schema
    // also lists.
    assert_eq!(input[1]["type"], "localImage");
    assert_eq!(
        input[1]["path"].as_str().expect("path"),
        planned.to_string_lossy(),
        "the entry names the materialized file"
    );
    assert!(input[1].get("detail").is_none(), "no unmeasured detail");
    let entry = codex_local_image_entry(planned);
    assert_eq!(entry["type"], "localImage");
    assert!(entry.get("detail").is_none());
}

#[test]
fn an_svg_only_codex_prompt_plans_no_paths_and_still_builds_the_legacy_text() {
    // SVG takes no inline shape on this surface. The plan still answers
    // with the text, and that text is exactly what the legacy write would
    // have produced, which is why a prompt with no carried path can take
    // the route without moving a byte on the wire. (This test used to
    // assert `plan.is_none()`: the route answers with the text now, so
    // that the send path never walks the attachments twice.)
    let temp = PlanTempDir::new("svg-only");
    let store = AttachmentStore::new(&temp.0);
    let session_id = "codex-plan-svg-only";
    let source = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>";
    let attachment = plan_attachment("drawing.svg", "image/svg+xml", source);
    let plan = plan_codex_prompt(
        &store,
        session_id,
        "logo",
        std::slice::from_ref(&attachment),
    )
    .expect("materialized")
    .expect("an SVG plans no path, but the plan still carries the text");
    assert!(plan.image_paths.is_empty(), "an SVG plans no path");
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
fn the_static_route_builds_the_turn_the_plan_decided() {
    // The route's frame is the measured `turn/start`: one text entry, then
    // one `localImage` per carried path, and nothing else moved — the
    // thread id and the model still come from the live state.
    let catalog = catalog_from_response(&serde_json::json!({
        "data": [{ "id": "model", "isDefault": true }]
    }))
    .expect("catalog");
    let state = CodexState::new("thread".to_string(), catalog, "auto");
    let temp = PlanTempDir::new("route");
    let store = AttachmentStore::new(&temp.0);
    let plan = plan_codex_prompt(
        &store,
        "codex-route",
        "describe this",
        &[plan_attachment("photo.png", "image/png", &clean_png(0x51))],
    )
    .expect("materialized")
    .expect("a raster plans a path");
    assert_eq!(plan.image_paths.len(), 1);
    let carried = turn_start_params_for_prompt(&state, &plan.fallback_text, &plan.image_paths);
    assert_eq!(carried["threadId"], "thread");
    let input = carried["input"].as_array().expect("input array");
    assert_eq!(input.len(), 2, "the text entry, then the path");
    assert_eq!(
        input[0],
        serde_json::json!({ "type": "text", "text": "describe this" })
    );
    assert_eq!(input[1]["type"], "localImage");
    // No carried path: the text-only turn, one entry.
    let bare = turn_start_params_for_prompt(&state, "describe this", &[]);
    assert_eq!(bare["input"].as_array().expect("input array").len(), 1);
}

#[test]
fn a_params_builder_without_images_is_the_text_only_one() {
    // The static route sends every planned prompt through the images
    // builder, including one whose paths are all path lines. With no
    // carried path it has to be the text-only `turn/start` this surface
    // has always sent: same keys, same values, same order.
    // The service tier is in the tuple because it is a key the two builders
    // must also agree on: a fast-mode turn written by one and not the other
    // would mean the static image route silently stopped running fast.
    for (policy_mode, model, effort, tier) in [
        (None, None, None, None),
        (
            Some("workspace-write"),
            Some("gpt-5-codex"),
            Some("high"),
            None,
        ),
        (None, Some("gpt-5.6"), None, Some("fast")),
    ] {
        assert_eq!(
            turn_start_params_with_images(
                "thread-1",
                "describe this",
                &[],
                policy_mode,
                model,
                effort,
                tier,
            ),
            turn_start_params(
                "thread-1",
                "describe this",
                policy_mode,
                model,
                effort,
                tier,
            ),
            "no carried path must not move a key"
        );
    }
}

#[test]
fn codex_turn_steer_frame_is_byte_exact() {
    let frame = request_frame(
        "d-7",
        "turn/steer",
        turn_steer_params("thread-1", "turn-2", "hello"),
    );
    assert_eq!(
            serde_json::to_vec(&frame).expect("frame"),
            br#"{"jsonrpc":"2.0","id":"d-7","method":"turn/steer","params":{"threadId":"thread-1","expectedTurnId":"turn-2","input":[{"type":"text","text":"hello"}]}}"#
        );
}

/// A `CodexState` on `thread-1` whose running turn is `turn_id`.
fn state_on_turn(turn_id: &str) -> Arc<CodexState> {
    let catalog = catalog_from_response(&serde_json::json!({
        "data": [{ "id": "model", "isDefault": true }]
    }))
    .expect("catalog");
    let state = CodexState::new("thread-1".to_string(), catalog, "auto");
    state.set_turn(Some(turn_id.to_string()));
    Arc::new(state)
}

#[test]
fn a_codex_steer_carries_the_turn_it_was_checked_for() {
    // The frame names the turn the caller captured as its precondition, so
    // Codex itself refuses a steer aimed at a turn that is over.
    let state = state_on_turn("turn-3");
    let params = steer_params_if_current(&state, "turn-3", "turn left")
        .expect("the captured turn is the current one");
    assert_eq!(params["threadId"], "thread-1");
    assert_eq!(params["expectedTurnId"], "turn-3");
    assert_eq!(params["input"][0]["text"], "turn left");
}

#[test]
fn a_codex_steer_for_a_turn_that_has_moved_on_is_never_written() {
    // The write-time check, not the caller's earlier one: the state says
    // Codex is on `turn-4` while the steer was admitted for `turn-3`, and
    // nothing is written at all. The stdin here is absent, so a write
    // attempt would answer `Err` instead of `Ok(false)` — what this pins is
    // that the frame is never built, and no answer is ever waited for.
    let state = state_on_turn("turn-4");
    assert!(steer_params_if_current(&state, "turn-3", "turn left").is_none());
    let stdin: Arc<Mutex<Option<std::process::ChildStdin>>> = Arc::new(Mutex::new(None));
    let steerer = CodexSteerer {
        stdin,
        next_id: Arc::new(AtomicU64::new(1)),
        state,
        requests: Arc::new(CodexRequests::new()),
        commands: empty_commands(),
    };
    assert!(matches!(
        steerer.begin_steer("turn-3", "turn left"),
        Ok(None)
    ));
}

/// S4-04: the app-server's output ending fails every waiter still registered,
/// so a steer answers `Err` — the fate is unknown — instead of waiting out
/// the whole fifteen-second timeout for an answer that cannot come.
#[test]
fn a_codex_steer_is_not_left_waiting_when_the_app_server_ends() {
    use std::io::BufReader;
    use std::process::Stdio;

    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    // A fake Codex that reads one request and exits without answering it.
    let mut child = std::process::Command::new("node")
        .args(["-e", "process.stdin.once('data', () => process.exit(0))"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("node is required for the Codex end-of-transport test");
    let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
    let stdout = child.stdout.take().expect("stdout");
    let requests = Arc::new(CodexRequests::new());
    let mut steerer = CodexSteerer {
        stdin,
        next_id: Arc::new(AtomicU64::new(7)),
        state: state_on_turn("turn-3"),
        requests: Arc::clone(&requests),
        commands: empty_commands(),
    };
    let mut reader = CodexReader {
        commands: empty_commands(),
        available_commands: None,
        buffer: Vec::new(),
        discarding_oversized_line: false,
        deferred: Vec::new(),
        manifest: None,
        state: state_on_turn("turn-3"),
        view: CodexView::new(None),
        permission_broker: PermissionBroker::for_test(Arc::new(|_, _| Ok(()))),
        response_ids: Arc::new(Mutex::new(HashMap::new())),
        stdin: Arc::new(Mutex::new(None)),
        next_id: Arc::new(AtomicU64::new(1)),
        spawn_nonce: "test-spawn".to_string(),
        requests: Arc::clone(&requests),
        compactions: crate::codex_compaction::CodexCompactions::default(),
    };
    // The reader runs the real end-of-transport path: read to EOF, then
    // `finish`, which is where the waiters are failed.
    let runtime = Arc::new(SessionRuntime::new());
    let reader_runtime = Arc::clone(&runtime);
    let reader_thread = std::thread::spawn(move || {
        let mut stdout = BufReader::new(stdout);
        let mut bytes = Vec::new();
        let _ = std::io::Read::read_to_end(&mut stdout, &mut bytes);
        reader.finish(&reader_runtime);
    });

    let started = Instant::now();
    let answer = crate::test_support::steer_through_the_turn(&mut steerer, "turn left");
    let error = match answer {
        Some(Err(error)) => error,
        other => panic!("the transport is over: expected an error, got {other:?}"),
    };
    assert!(
        error.message.contains("control channel closed"),
        "the failure is the transport ending, not a timeout: {}",
        error.message
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "the waiter was failed by the EOF, not by its own fifteen-second timeout"
    );
    let _ = child.wait();
    reader_thread.join().expect("the reader thread");
}

/// Spawn a fake Codex that echoes each line it reads back, so a test can
/// read the frame the client wrote. The caller must have checked `node`
/// first (`external_program_skip_reason`).
fn spawn_codex_echoing() -> std::process::Child {
    std::process::Command::new("node")
        .args([
            "-e",
            "process.stdin.on('data', data => process.stdout.write(data))",
        ])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("node is required for the Codex steer tests")
}

/// Spawn a fake Codex that answers each `turn/steer` with `body`, whose `id`
/// it fills with the request's own — the app-server's shape, so the test
/// exercises the real id-correlated delivery. The caller must have checked
/// `node` first (`external_program_skip_reason`).
fn spawn_codex_answering(body: &str) -> std::process::Child {
    let script = r#"
let buf = '';
process.stdin.on('data', data => {
  buf += data;
  let i;
  while ((i = buf.indexOf('\n')) >= 0) {
    const line = buf.slice(0, i);
    buf = buf.slice(i + 1);
    let request;
    try { request = JSON.parse(line); } catch (error) { continue; }
    if (request.method === 'turn/steer') {
      const answer = __ANSWER__;
      answer.id = request.id;
      process.stdout.write(JSON.stringify(answer) + '\n');
    }
  }
});
"#
    .replace("__ANSWER__", body);
    std::process::Command::new("node")
        .args(["-e", &script])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .spawn()
        .expect("node is required for the Codex steer tests")
}

/// One steer against a fake Codex that answers `body`: the child, the real
/// delivery (`CodexRequests::deliver`, which is what the reader calls) and
/// the steer all run, so the answer the steerer returns is the one the
/// response produced.
fn codex_steer_against(body: &str) -> Result<bool, WireError> {
    let requests = Arc::new(CodexRequests::new());
    let mut child = spawn_codex_answering(body);
    let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
    let stdout = child.stdout.take().expect("stdout");
    let delivered = Arc::clone(&requests);
    let reader = std::thread::spawn(move || {
        use std::io::BufRead;
        let mut stdout = std::io::BufReader::new(stdout);
        let mut line = String::new();
        loop {
            line.clear();
            match stdout.read_line(&mut line) {
                Ok(0) | Err(_) => break,
                Ok(_) => {}
            }
            let value: serde_json::Value =
                serde_json::from_str(line.trim()).expect("the answer the fake Codex wrote");
            assert!(
                delivered.deliver(&value),
                "the answer named an id no waiter registered"
            );
        }
    });
    let mut steerer = CodexSteerer {
        stdin,
        next_id: Arc::new(AtomicU64::new(7)),
        state: state_on_turn("turn-3"),
        requests,
        commands: empty_commands(),
    };
    let answer = crate::test_support::steer_through_the_turn(&mut steerer, "turn left");
    let _ = child.kill();
    let _ = child.wait();
    reader.join().expect("the fake Codex reader");
    answer.expect("the turn was running at admission")
}

#[test]
fn a_codex_steer_is_accepted_only_by_a_response_for_the_steered_turn() {
    // A2-03: the write is a request, not a decision. The acceptance is the
    // app-server's own response, and only one that names the turn the steer
    // was written into counts as one.
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    assert!(
        matches!(
            codex_steer_against(r#"{"jsonrpc":"2.0","result":{"turn":{"id":"turn-3"}}}"#),
            Ok(true)
        ),
        "the response naming the steered turn is the acceptance"
    );
}

#[test]
fn a_codex_steer_response_for_another_turn_is_a_refusal_not_an_acceptance() {
    // The response says the app-server took a steer — for a *different*
    // turn. Answering `Ok(true)` here would tell the caller its text landed
    // in the turn it was admitted for when the provider said otherwise.
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    assert!(matches!(
        codex_steer_against(r#"{"jsonrpc":"2.0","result":{"turn":{"id":"turn-9"}}}"#),
        Ok(false)
    ));
}

#[test]
fn a_codex_steer_answered_by_an_error_or_no_turn_is_a_refusal() {
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    assert!(
        matches!(
            codex_steer_against(
                r#"{"jsonrpc":"2.0","error":{"code":-32600,"message":"steer refused"}}"#
            ),
            Ok(false)
        ),
        "an error result is not a steer"
    );
    assert!(
        matches!(
            codex_steer_against(r#"{"jsonrpc":"2.0","result":{}}"#),
            Ok(false)
        ),
        "a response that names no turn is not a steer for this one"
    );
}

#[test]
fn a_codex_steer_that_is_still_current_is_written_with_its_precondition() {
    // The frame itself, read back from the child: the request names the turn
    // the caller captured as its precondition, so Codex refuses a steer
    // aimed at a turn that is over. The answer is not read here — this pins
    // the bytes, and `CodexRequests` is what turns them into a decision.
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    use std::io::{BufRead, BufReader};

    let mut child = spawn_codex_echoing();
    let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
    let mut stdout = BufReader::new(child.stdout.take().expect("stdout"));
    let steerer = CodexSteerer {
        stdin,
        next_id: Arc::new(AtomicU64::new(7)),
        state: state_on_turn("turn-3"),
        requests: Arc::new(CodexRequests::new()),
        commands: empty_commands(),
    };
    let request = steerer
        .begin_steer("turn-3", "turn left")
        .expect("the frame was written");
    assert!(
        request.is_some(),
        "the current turn is the one the steer is written for"
    );
    let mut line = String::new();
    stdout
        .read_line(&mut line)
        .expect("the frame the child read");
    let frame: serde_json::Value = serde_json::from_str(&line).expect("frame json");
    assert_eq!(frame["jsonrpc"], "2.0");
    assert_eq!(frame["method"], "turn/steer");
    assert_eq!(frame["params"]["expectedTurnId"], "turn-3");
    assert_eq!(frame["params"]["input"][0]["text"], "turn left");
    assert_eq!(
        frame["id"], "d-7",
        "the request id is the one its answer will name"
    );
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn an_svg_keeps_its_path_line_beside_codex_image_paths() {
    // A mixed prompt carries both: the raster as a `localImage` path,
    // the SVG as a path line in the text.
    let temp = PlanTempDir::new("mixed");
    let store = AttachmentStore::new(&temp.0);
    let source = b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>";
    let plan = plan_codex_prompt(
        &store,
        "codex-plan-mixed",
        "logo and photo",
        &[
            plan_attachment("photo.png", "image/png", &clean_png(0x13)),
            plan_attachment("drawing.svg", "image/svg+xml", source),
        ],
    )
    .expect("materialized")
    .expect("the raster plans a path");
    assert_eq!(carried_image_paths(Some(&plan)).len(), 1);
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
    let params = turn_start_params_with_images(
        "thread-1",
        &plan.fallback_text,
        &plan.image_paths,
        None,
        None,
        None,
        None,
    );
    let input = params["input"].as_array().expect("array");
    assert_eq!(input.len(), 2);
    assert!(input[0]["text"].as_str().expect("text").ends_with(".svg]"));
    assert_eq!(input[1]["type"], "localImage");
}

#[test]
fn a_first_command_with_an_attachment_keeps_its_prefix_and_image_block() {
    let temp = PlanTempDir::new("first-command-image");
    let home = temp.0.join("home");
    let cwd = temp.0.join("workspace");
    std::fs::create_dir_all(home.join("prompts")).unwrap();
    std::fs::create_dir_all(&cwd).unwrap();
    std::fs::write(
        home.join("prompts/commit.md"),
        "---\ndescription: Draft\n---\nReview $1\n",
    )
    .unwrap();
    let commands = CodexCommands::new(&home, Some(&cwd), false);
    let raw = "/prompts:commit release";
    let text = "standing\n\nspawn\n\nrecovered\n\n/prompts:commit release";
    let store = AttachmentStore::new(&temp.0.join("store"));
    let mut plan = plan_codex_prompt(
        &store,
        "codex-first-command-image",
        text,
        &[plan_attachment(
            "picture.png",
            "image/png",
            &clean_png(0x31),
        )],
    )
    .expect("plan the attachment")
    .expect("the raster plans a path");
    plan.command_input = command_prompt_input(&commands, raw, "standing\n\nspawn\n\nrecovered")
        .expect("expand command");

    let input = plan.input_blocks();
    assert_eq!(input.len(), 2);
    assert_eq!(
        input[0]["text"],
        "standing\n\nspawn\n\nrecovered\n\nReview release\n"
    );
    assert_eq!(input[1]["type"], "localImage");
}

#[test]
fn a_jpeg_plans_its_stripped_path_on_codex() {
    const EXIF_JPEG_VECTOR: &str =
        "a jpeg whose APP1 holds EXIF, between a kept JFIF APP0 and a kept ICC APP2";
    let temp = PlanTempDir::new("jpeg");
    let store = AttachmentStore::new(&temp.0);
    let sent = vector_input(EXIF_JPEG_VECTOR);
    let kept = vector_output(EXIF_JPEG_VECTOR);
    assert_ne!(sent, kept, "the vector must actually strip something");
    let plan = plan_codex_prompt(
        &store,
        "codex-plan-jpeg",
        "describe this",
        &[plan_attachment("photo.jpg", "image/jpeg", &sent)],
    )
    .expect("materialized")
    .expect("a JPEG plans a path");
    assert_eq!(plan.fallback_text, "describe this");
    let planned = carried_image_paths(Some(&plan))[0];
    assert_eq!(
        std::fs::read(planned).expect("read"),
        kept,
        "stripped JPEG bytes at the planned path"
    );
}
// ---- S6: the Codex carrier rides `-c` overrides ----

#[test]
fn codex_mcp_launch_rides_config_overrides_and_keeps_the_token_in_env() {
    // S4 seam body for Codex: env carries the token value; argv carries the
    // `-c` overrides naming the broker URL and the token env var. No owned
    // files or dirs — the child keeps the human's real home, where the
    // credentials and the rollout this family resumes live.
    let config = crate::mcp_broker::McpLaunchConfig::for_test(
        "http://127.0.0.1:4321/mcp",
        "secret-bearer-launch",
    );
    let carrier = mcp_launch(&config, Path::new("unused")).expect("carrier");
    let env: std::collections::HashMap<_, _> = carrier.env_additions.iter().cloned().collect();
    assert_eq!(
        env.get(crate::mcp_broker::MCP_TOKEN_ENV)
            .map(String::as_str),
        Some("secret-bearer-launch")
    );
    assert!(carrier.owned_paths.is_empty(), "nothing on disk to revoke");
    assert!(carrier.owned_dirs.is_empty(), "no per-session home");
    assert_eq!(
        carrier.arg_additions,
        vec![
            "-c".to_string(),
            format!(
                "mcp_servers.{}.url=\"http://127.0.0.1:4321/mcp\"",
                crate::mcp_broker::MCP_SERVER_NAME
            ),
            "-c".to_string(),
            format!(
                "mcp_servers.{}.bearer_token_env_var=\"{}\"",
                crate::mcp_broker::MCP_SERVER_NAME,
                crate::mcp_broker::MCP_TOKEN_ENV
            ),
        ]
    );
    assert!(
        !carrier
            .arg_additions
            .iter()
            .any(|arg| arg.contains("secret-bearer-launch")),
        "argv stays token-free"
    );
}

/// A fake `codex app-server` (node): answers initialize/model-list and both
/// thread roads. stderr is nulled: the tests read the protocol, never the
/// log. `FAKE_CODEX_METHODS` names a file that receives every request method
/// the child sees, in arrival order; `FAKE_CODEX_RESUME_HANDLES` names one
/// that receives the `threadId` of every `thread/resume` frame — so a test
/// can pin the value the road sent, not only its method name.
const FAKE_CODEX_HANDSHAKE: &str = r#"
const methods = process.env.FAKE_CODEX_METHODS || "";
const handles = process.env.FAKE_CODEX_RESUME_HANDLES || "";
let buf = "";
process.stdin.on("data", (chunk) => {
  buf += chunk.toString();
  let nl;
  while ((nl = buf.indexOf("\n")) >= 0) {
    const line = buf.slice(0, nl);
    buf = buf.slice(nl + 1);
    if (!line.trim()) continue;
    let msg;
    try { msg = JSON.parse(line); } catch { continue; }
    if (msg.id === undefined || msg.id === null) continue;
    if (methods) require("fs").appendFileSync(methods, msg.method + "\n");
    if (handles && msg.method === "thread/resume") {
      const handle = msg.params && msg.params.threadId;
      require("fs").appendFileSync(handles, (handle === undefined ? "missing" : handle) + "\n");
    }
    let result = {};
    if (msg.method === "initialize") result = { userAgent: "fake-codex" };
    else if (msg.method === "model/list") result = { data: [{ id: "fake-model", isDefault: true }] };
    else if (msg.method === "thread/start") result = { thread: { id: "thread-fake" } };
    else if (msg.method === "thread/resume") result = { thread: { id: "thread-resumed" } };
    process.stdout.write(JSON.stringify({ id: msg.id, result }) + "\n");
  }
});
"#;

/// The handle a dead generation persisted: distinctive enough that no
/// hard-coded substitute can stand in for it on the wire.
const PERSISTED_THREAD: &str = "01a0c179-a889-7432-8c89-2ca801ccf9fa";

fn fake_codex_child() -> std::process::Child {
    fake_codex_child_recording(&[])
}

fn fake_codex_child_recording(records: &[(&str, std::path::PathBuf)]) -> std::process::Child {
    let mut command = std::process::Command::new("node");
    command
        .args(["-e", FAKE_CODEX_HANDSHAKE])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null());
    for (name, path) in records {
        command.env(name, path);
    }
    command
        .spawn()
        .expect("node is required for the fake Codex handshake")
}

#[test]
fn codex_handshake_starts_a_thread_on_the_fresh_road() {
    // Full `perform_handshake` through a fake child: the fresh road sends
    // `thread/start` and reads the response's thread id.
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let dir = crate::test_dirs::test_temp_dir("devboule-codex-handshake");
    let mut child = fake_codex_child();
    let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
    let mut stdout = CodexStdout::spawn(child.stdout.take().expect("stdout")).expect("reader");
    let next_id = AtomicU64::new(1);
    let handshake = super::perform_handshake(
        &mut stdout,
        &stdin,
        &next_id,
        &dir,
        "auto",
        ThreadRoad::Fresh,
    )
    .expect("the fresh handshake answers");
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(handshake.thread_id, "thread-fake");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn codex_resume_handshake_loads_the_thread_and_never_starts_one() {
    // The resume road must send `thread/resume {threadId}` after initialize —
    // not `thread/start`, and not a placeholder handle. The child records
    // both the method it sees and the `threadId` on the frame, so a mutation
    // of either fails on the record, not on a comment.
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let dir = crate::test_dirs::test_temp_dir("devboule-codex-resume");
    let methods_file = dir.join("methods.txt");
    let handles_file = dir.join("handles.txt");
    let mut child = fake_codex_child_recording(&[
        ("FAKE_CODEX_METHODS", methods_file.clone()),
        ("FAKE_CODEX_RESUME_HANDLES", handles_file.clone()),
    ]);
    let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
    let mut stdout = CodexStdout::spawn(child.stdout.take().expect("stdout")).expect("reader");
    let next_id = AtomicU64::new(1);
    let handshake = super::perform_handshake(
        &mut stdout,
        &stdin,
        &next_id,
        &dir,
        "auto",
        ThreadRoad::Resuming(PERSISTED_THREAD),
    )
    .expect("the resume handshake answers");
    let _ = child.kill();
    let _ = child.wait();
    assert_eq!(
        handshake.thread_id, "thread-resumed",
        "the handle is the response's thread id"
    );
    let seen = std::fs::read_to_string(&methods_file).expect("the child recorded its methods");
    let methods: Vec<&str> = seen.lines().collect();
    assert!(
        methods.contains(&"thread/resume"),
        "the resume road is the one on the wire: {methods:?}"
    );
    assert!(
        !methods.contains(&"thread/start"),
        "a resume never starts a new thread: {methods:?}"
    );
    let sent = std::fs::read_to_string(&handles_file).expect("the child recorded its handles");
    assert_eq!(
        sent,
        format!("{PERSISTED_THREAD}\n"),
        "the frame carries the persisted handle, exactly once"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn codex_resume_spawn_sends_only_the_handle_and_returns_the_thread() {
    // The production resume entry point: `spawn_process_resuming` drives the
    // fake child through `thread/resume` and nothing that could re-send our
    // journal — no `turn/start`, no `thread/start` — and the provider's
    // answer becomes the session's peer id.
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let state = crate::server::ServerState::new("codex-resume-road".to_string());
    let dir = crate::test_dirs::test_temp_dir("devboule-codex-resume-road");
    let methods_file = dir.join("methods.txt");
    let handles_file = dir.join("handles.txt");
    let command = crate::session::PtyCommand::new(
        "node",
        vec![
            "-e".to_string(),
            FAKE_CODEX_HANDSHAKE.to_string(),
            "--".to_string(),
        ],
        crate::test_dirs::test_temp_dir("devboule-codex-cwd"),
        vec![
            (
                "FAKE_CODEX_METHODS".to_string(),
                methods_file.to_string_lossy().into_owned(),
            ),
            (
                "FAKE_CODEX_RESUME_HANDLES".to_string(),
                handles_file.to_string_lossy().into_owned(),
            ),
        ],
    );
    let mut spawned =
        super::spawn_process_resuming(&state, command, PERSISTED_THREAD.to_string(), None)
            .expect("the resume road spawns");
    assert_eq!(spawned.peer_session_id.as_deref(), Some("thread-resumed"));
    let seen = std::fs::read_to_string(&methods_file).expect("the child recorded its methods");
    let methods: Vec<&str> = seen.lines().collect();
    assert_eq!(
        methods,
        ["initialize", "model/list", "thread/resume"],
        "a resume sends the handle and nothing else"
    );
    let sent = std::fs::read_to_string(&handles_file).expect("the child recorded its handles");
    assert_eq!(
        sent,
        format!("{PERSISTED_THREAD}\n"),
        "the production road sends the row's own handle, not a placeholder"
    );
    spawned.killer.kill();
    let _ = std::fs::remove_dir_all(&dir);
}

/// A fake child that records its own argv to `FAKE_ARGV_FILE` and the child
/// values of `FAKE_ENV_KEYS` (a comma-separated list) to `FAKE_ENV_FILE`,
/// then answers the fresh handshake so `spawn_process` returns.
const ARGV_FAKE: &str = r#"
const fs = require("fs");
const out = process.env.FAKE_ARGV_FILE || "";
if (out) fs.writeFileSync(out, process.argv.slice(1).join("\n"));
const envOut = process.env.FAKE_ENV_FILE || "";
if (envOut) {
  const report = {};
  for (const key of (process.env.FAKE_ENV_KEYS || "").split(",")) {
    if (key) report[key] = process.env[key] === undefined ? null : process.env[key];
  }
  fs.writeFileSync(envOut, JSON.stringify(report));
}
let buf = "";
process.stdin.on("data", (chunk) => {
  buf += chunk.toString();
  let nl;
  while ((nl = buf.indexOf("\n")) >= 0) {
    const line = buf.slice(0, nl);
    buf = buf.slice(nl + 1);
    if (!line.trim()) continue;
    let msg;
    try { msg = JSON.parse(line); } catch { continue; }
    if (msg.id === undefined || msg.id === null) continue;
    let result = {};
    if (msg.method === "initialize") result = { userAgent: "fake-argv" };
    else if (msg.method === "model/list") result = { data: [{ id: "fake-model", isDefault: true }] };
    else if (msg.method === "thread/start") result = { thread: { id: "thread-argv" } };
    process.stdout.write(JSON.stringify({ id: msg.id, result }) + "\n");
  }
});
"#;

/// Drives production's `spawn_process` on the carrier road and answers what
/// the child was started with: its argv, and the value it read for each name
/// in `env_keys` (`null` for a name the child does not have).
fn spawn_carrier_road(
    state: &Arc<crate::server::ServerState>,
    config: crate::mcp_broker::McpLaunchConfig,
    dir: &Path,
    env_keys: &[&str],
) -> (super::SpawnedSession, String, serde_json::Value) {
    let argv_file = dir.join("argv.txt");
    let env_file = dir.join("env.json");
    let command = crate::session::PtyCommand::new(
        "node",
        vec!["-e".to_string(), ARGV_FAKE.to_string(), "--".to_string()],
        crate::test_dirs::test_temp_dir("devboule-codex-cwd"),
        vec![
            (
                "FAKE_ARGV_FILE".to_string(),
                argv_file.to_string_lossy().into_owned(),
            ),
            (
                "FAKE_ENV_FILE".to_string(),
                env_file.to_string_lossy().into_owned(),
            ),
            ("FAKE_ENV_KEYS".to_string(), env_keys.join(",")),
        ],
    );
    let spawned = super::spawn_process(
        state,
        command,
        Some(config),
        crate::profile_delivery::ProfileDelivery::none(),
    )
    .expect("the carrier road spawns");
    let argv = std::fs::read_to_string(&argv_file).expect("the child recorded its argv");
    let env = serde_json::from_str(&std::fs::read_to_string(&env_file).expect("env report"))
        .expect("the env report parses");
    (spawned, argv, env)
}

#[test]
fn codex_carrier_road_puts_the_overrides_on_argv_and_the_token_in_env() {
    // The launch line the child actually reads: the `-c` overrides ride argv
    // (URL and env-var pointer, no secret), and the bearer the broker minted
    // rides the child env — applied by production's own loop, not by this
    // test. The fake child records argv and its environment, so a mutation
    // back to a config-file carrier, a token spliced onto argv, or an env
    // loop that stopped running all fail here.
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let state = crate::server::ServerState::new("codex-argv-road".to_string());
    let owner = devboule_protocol::OwnerId::new("local", "argv").expect("owner");
    let id = "s.argv.1";
    let guard = state
        .mcp
        .register_with_provider(
            id,
            &owner,
            &devboule_protocol::SessionKind::Codex,
            Some("codex"),
            crate::mcp_broker::AgentLineage::root(),
        )
        .expect("register")
        .expect("bearer");
    let config = state.mcp.launch_config(id).expect("launch config");
    let bearer = config.bearer().to_string();
    let dir = crate::test_dirs::test_temp_dir("devboule-codex-argv");
    let (mut spawned, argv, env) =
        spawn_carrier_road(&state, config, &dir, &[crate::mcp_broker::MCP_TOKEN_ENV]);
    assert!(
        argv.contains(&format!(
            "mcp_servers.{}.url=",
            crate::mcp_broker::MCP_SERVER_NAME
        )),
        "the URL rides a `-c` override: {argv}"
    );
    assert!(
        argv.contains(&format!(
            "mcp_servers.{}.bearer_token_env_var=\"{}\"",
            crate::mcp_broker::MCP_SERVER_NAME,
            crate::mcp_broker::MCP_TOKEN_ENV
        )),
        "the env-var pointer rides a `-c` override: {argv}"
    );
    assert!(
        argv.lines().filter(|line| *line == "-c").count() == 2,
        "exactly the two overrides: {argv}"
    );
    assert!(
        !argv.contains(&bearer),
        "the token never rides argv: {argv}"
    );
    assert_eq!(
        env.get(crate::mcp_broker::MCP_TOKEN_ENV)
            .and_then(serde_json::Value::as_str),
        Some(bearer.as_str()),
        "the bearer the broker minted is the value the child reads: {env}"
    );
    spawned.killer.kill();
    drop(guard);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn thread_resume_params_carry_the_handle_and_nothing_else() {
    // Measured against the installed schema: `ThreadResumeParams` requires
    // exactly `threadId`; the thread on disk carries its cwd, model and policy.
    assert_eq!(
        thread_resume_params(PERSISTED_THREAD),
        serde_json::json!({ "threadId": PERSISTED_THREAD })
    );
}

/// The broker's own entry in a `mcpServerStatus/list` answer. The human's real
/// home may configure other servers, so the tests find ours by name.
fn devboule_entry(status: &serde_json::Value) -> &serde_json::Value {
    status["data"]
        .as_array()
        .and_then(|entries| {
            entries
                .iter()
                .find(|entry| entry["name"] == crate::mcp_broker::MCP_SERVER_NAME)
        })
        .unwrap_or_else(|| panic!("the devboule entry is listed: {status}"))
}

#[test]
fn live_codex_reports_a_configured_but_dead_broker_honestly() {
    // S6 live (real codex-cli, unreachable broker): the `-c` overrides
    // configure the entry, and `mcpServerStatus/list` shows it with a
    // `toolsError` — the probe's failure shape, never `connected`. Skips
    // where Codex is not runnable (gate PATH caveat, stated in report).
    let Some((program, args)) = live_codex_command() else {
        eprintln!("skipping: live codex is not runnable here");
        return;
    };
    let carrier = super::mcp_launch(
        &crate::mcp_broker::McpLaunchConfig::for_test("http://127.0.0.1:9/mcp", "live-canary"),
        Path::new("unused"),
    )
    .expect("carrier");
    let mut args = args;
    args.extend(carrier.arg_additions.iter().cloned());
    let status = live_codex_result(
        &program,
        &args,
        "live-canary-token",
        "mcpServerStatus/list",
        serde_json::json!({}),
    );
    let entry = devboule_entry(&status);
    assert!(
        entry["toolsError"]
            .as_str()
            .is_some_and(|message| !message.is_empty()),
        "configured-and-failed reads from toolsError, never runtimeStatus: {status}"
    );
    assert!(
        entry["runtimeStatus"].is_null(),
        "the probe's null-status trap holds live: {status}"
    );
}

#[test]
fn live_codex_sends_the_bearer_and_serves_a_catalog_without_tools_error() {
    // S6/S7 live (real codex-cli + local stub broker): the token the daemon
    // put in the child env flies on the stub's wire, argv stays clean, and
    // `mcpServerStatus/list` reports the entry with a tools catalog and a null
    // `toolsError` — the success shape the probe left unmeasured (Q2, closed
    // by the +12 s two-poll scratch: `runtimeStatus` stays null even fully
    // working, so the criterion keys on catalog + null error, never status).
    // Skips where Codex is not runnable.
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicBool, Ordering};
    let Some((program, args)) = live_codex_command() else {
        eprintln!("skipping: live codex is not runnable here");
        return;
    };
    let seen: Arc<Mutex<Vec<(String, String)>>> = Arc::new(Mutex::new(Vec::new()));
    let stop = Arc::new(AtomicBool::new(false));
    let listener = TcpListener::bind(("127.0.0.1", 0)).expect("stub binds");
    listener.set_nonblocking(true).expect("stub nonblocking");
    let port = listener.local_addr().expect("stub port").port();
    let stub_url = format!("http://127.0.0.1:{port}/mcp");
    let stub_seen = Arc::clone(&seen);
    let stub_stop = Arc::clone(&stop);
    let stub = std::thread::spawn(move || {
        let mut served = 0;
        while !stub_stop.load(Ordering::Acquire) && served < 24 {
            let Ok((mut stream, _)) = listener.accept() else {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
            let mut head = Vec::new();
            let mut byte = [0u8; 1];
            let end = loop {
                match stream.read(&mut byte) {
                    Ok(0) => break None,
                    Ok(_) => {
                        head.extend_from_slice(&byte);
                        if head.len() > 65536 {
                            break None;
                        }
                        if head.windows(4).any(|w| w == b"\r\n\r\n") {
                            break Some(head.len());
                        }
                    }
                    Err(_) => break None,
                }
            };
            let Some(end) = end else { continue };
            let head_text = String::from_utf8_lossy(&head[..end]).into_owned();
            let length = head_text
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.trim()
                        .eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse::<usize>().unwrap_or(0))
                })
                .unwrap_or(0);
            let mut body = vec![0u8; length.min(65536)];
            let mut read = 0;
            while read < body.len() {
                match stream.read(&mut body[read..]) {
                    Ok(0) => break,
                    Ok(n) => read += n,
                    Err(_) => break,
                }
            }
            let auth = head_text
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.trim()
                        .eq_ignore_ascii_case("authorization")
                        .then(|| value.trim().to_string())
                })
                .unwrap_or_default();
            let message: serde_json::Value =
                serde_json::from_slice(&body[..read]).unwrap_or(serde_json::Value::Null);
            let method = message
                .get("method")
                .and_then(|value| value.as_str())
                .unwrap_or("")
                .to_string();
            stub_seen
                .lock()
                .expect("stub log")
                .push((auth, method.clone()));
            let id = message
                .get("id")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let (status, body) = if method == "notifications/initialized" {
                ("202 Accepted", String::new())
            } else if method == "initialize" {
                (
                    "200 OK",
                    serde_json::json!({
                        "jsonrpc": "2.0", "id": id,
                        "result": {
                            "protocolVersion": "2025-06-18", "capabilities": {},
                            "serverInfo": { "name": "stub", "version": "1" },
                        },
                    })
                    .to_string(),
                )
            } else if method == "tools/list" {
                (
                    "200 OK",
                    serde_json::json!({
                        "jsonrpc": "2.0", "id": id,
                        "result": { "tools": [
                            { "name": "stub_tool", "description": "A stub tool.",
                              "inputSchema": { "type": "object", "properties": {} } },
                        ] },
                    })
                    .to_string(),
                )
            } else {
                (
                    "200 OK",
                    serde_json::json!({
                        "jsonrpc": "2.0", "id": id,
                        "result": {
                            "content": [{ "type": "text", "text": "stub-result" }],
                            "isError": false,
                        },
                    })
                    .to_string(),
                )
            };
            let _ = write!(
                    stream,
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            served += 1;
        }
    });
    let carrier = super::mcp_launch(
        &crate::mcp_broker::McpLaunchConfig::for_test(&stub_url, "live-bearer-xyz"),
        Path::new("unused"),
    )
    .expect("carrier");
    let mut args = args;
    args.extend(carrier.arg_additions.iter().cloned());
    let started = Instant::now();
    let status = live_codex_result(
        &program,
        &args,
        "live-bearer-xyz",
        "mcpServerStatus/list",
        serde_json::json!({}),
    );
    let elapsed = started.elapsed();
    stop.store(true, Ordering::Release);
    let _ = stub.join();
    // The golden success shape (Q2, measured live +12 s apart with identical
    // results): present, catalog served, no toolsError. `runtimeStatus` is
    // null even fully working — keyed on nothing, documented here.
    let entry = devboule_entry(&status);
    assert!(
        entry["tools"].get("stub_tool").is_some(),
        "the stub catalog arrives: {status}"
    );
    assert!(
        entry.get("toolsError").is_none_or(|value| value.is_null()),
        "no toolsError on success: {status}"
    );
    // The token the daemon put in the child env flies on the stub's wire.
    let seen = seen.lock().expect("stub log");
    assert!(!seen.is_empty(), "the child dialed the stub");
    assert!(
        seen.iter()
            .any(|(auth, _)| auth == "Bearer live-bearer-xyz"),
        "Bearer equals the env token: {seen:?}"
    );
    // And it never rode argv: the launch line names the env var, not the
    // token, and the carrier owns no file on disk.
    assert!(
        !args.iter().any(|arg| arg.contains("live-bearer-xyz")),
        "argv is token-free"
    );
    assert!(
        args.iter()
            .any(|arg| arg.contains(crate::mcp_broker::MCP_TOKEN_ENV)),
        "the bearer rides the env-var pointer: {args:?}"
    );
    assert!(carrier.owned_paths.is_empty() && carrier.owned_dirs.is_empty());
    eprintln!("live stub handshake+status took {elapsed:?}");
}

// ---- S7: post-spawn verification ----

fn golden_status() -> serde_json::Value {
    // The Q2 golden, byte-shaped like the live stub answered (status null
    // even working — keyed on nothing).
    serde_json::json!({
        "data": [{
            "name": "devboule",
            "runtimeStatus": null,
            "tools": { "stub_tool": { "name": "stub_tool" } },
            "toolsError": null,
        }],
    })
}

#[test]
fn codex_status_mapping_keys_on_presence_catalog_and_error() {
    use crate::mcp_broker::ToolsState;
    // Golden: present + catalog + null error → the only Hosted.
    assert_eq!(
        super::map_codex_status(&golden_status()),
        ToolsState::Hosted
    );
    // Configured and failed → established without working tools.
    let mut failed = golden_status();
    failed["data"][0]["toolsError"] = serde_json::json!("MCP startup failed: refused");
    assert_eq!(super::map_codex_status(&failed), ToolsState::Unverified);
    // Absent from data (never configured, or vanished): not proof of absence.
    assert_eq!(
        super::map_codex_status(&serde_json::json!({ "data": [] })),
        ToolsState::Unverified
    );
    assert_eq!(
        super::map_codex_status(&serde_json::json!({})),
        ToolsState::Unverified
    );
    // No error but no catalog either: no evidence either way.
    let mut empty = golden_status();
    empty["data"][0]["tools"] = serde_json::json!({});
    assert_eq!(super::map_codex_status(&empty), ToolsState::Unverified);
    // A different server's health says nothing about ours.
    assert_eq!(
        super::map_codex_status(
            &serde_json::json!({ "data": [{ "name": "other", "runtimeStatus": "connected",
                    "tools": { "t": {} }, "toolsError": null }] })
        ),
        ToolsState::Unverified
    );
    // Empty-string error reads as null (no error), not as failure.
    let mut blank = golden_status();
    blank["data"][0]["toolsError"] = serde_json::json!("");
    assert_eq!(super::map_codex_status(&blank), ToolsState::Hosted);
}

/// A fake child that answers one `mcpServerStatus/list` with `answer`, then
/// goes quiet: the verification waiter gets exactly one chance.
fn verify_harness(
    answer: Option<serde_json::Value>,
) -> (
    super::CodexVerifyBundle,
    std::thread::JoinHandle<()>,
    std::process::Child,
    std::sync::Arc<super::CodexRequests>,
) {
    let script = if let Some(answer) = answer {
        "let b='';process.stdin.on('data',c=>{b+=c;let n;while((n=b.indexOf('\\n'))>=0){const l=b.slice(0,n);b=b.slice(n+1);if(!l.trim())continue;let m;try{m=JSON.parse(l)}catch{continue}if(m.id===undefined||m.id===null)continue;process.stdout.write(JSON.stringify({id:m.id,result:ANSWER})+'\\n');}});"
                .replace("ANSWER", &serde_json::to_string(&answer).expect("answer"))
    } else {
        "process.stdin.on('data',()=>{});setTimeout(()=>{},30000); void 0;".to_string()
    };
    let mut child = std::process::Command::new("node")
        .args(["-e", &script])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("node is required for the Codex verify harness");
    let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
    let stdout = child.stdout.take().expect("stdout");
    let requests = Arc::new(super::CodexRequests::new());
    let bundle = super::CodexVerifyBundle {
        stdin: Arc::clone(&stdin),
        next_id: Arc::new(AtomicU64::new(1)),
        requests: Arc::clone(&requests),
    };
    // The reader's role: lines off stdout into id-matched delivery.
    let pump_requests = Arc::clone(&requests);
    let pump = std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            pump_requests.deliver(&value);
        }
    });
    (bundle, pump, child, requests)
}

#[test]
fn codex_verify_runs_off_thread_and_maps_the_golden_to_hosted() {
    // S7 threading (Q6): the poll runs on a worker while delivery is pumped
    // elsewhere — the reentrancy rule as executable proof. The signature
    // (bundle, never `&mut CodexReader`) is the compile-level half.
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let (bundle, pump, mut child, _) = verify_harness(Some(golden_status()));
    let worker = std::thread::spawn(move || {
        super::verify_codex_mcp_with_timeout(&bundle, Duration::from_secs(10))
    });
    let state = worker.join().expect("verify thread joins");
    assert_eq!(
        state,
        crate::mcp_broker::ToolsState::Hosted,
        "the golden poll establishes Hosted"
    );
    let _ = child.kill();
    let _ = child.wait();
    pump.join().expect("pump drains");
}

#[test]
fn codex_verify_degrades_on_timeout_transport_end_and_dead_stdin() {
    use crate::mcp_broker::ToolsState;
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    // A black hole: bounded by the timeout, never a hang, and the child
    // survives the observation (verification is not a fate).
    let (bundle, pump, mut child, _) = verify_harness(None);
    let started = Instant::now();
    assert_eq!(
        super::verify_codex_mcp_with_timeout(&bundle, Duration::from_secs(2)),
        ToolsState::Unverified
    );
    assert!(
        started.elapsed() < Duration::from_secs(15),
        "the wait is bounded, never a hang"
    );
    assert!(
        child.try_wait().expect("poll child").is_none(),
        "a timed-out verification leaves the child alive"
    );
    let _ = child.kill();
    let _ = child.wait();
    pump.join().expect("pump drains");
    // A transport end wakes the waiter fast with Unverified, never a hang:
    // parked first (polled for determinism), then failed.
    let (bundle, pump, mut child, requests) = verify_harness(None);
    let worker = std::thread::spawn(move || {
        super::verify_codex_mcp_with_timeout(&bundle, Duration::from_secs(30))
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    while requests.pending_count() == 0 && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        requests.pending_count(),
        1,
        "the poll parked exactly one waiter"
    );
    requests.fail_pending("Codex control channel closed before the response arrived.");
    let started = Instant::now();
    assert_eq!(
        worker.join().expect("woken waiter joins"),
        ToolsState::Unverified
    );
    assert!(
        started.elapsed() < Duration::from_secs(25),
        "the transport end wakes the waiter; it never sits out the timeout"
    );
    let _ = child.kill();
    let _ = child.wait();
    pump.join().expect("pump drains");
    // A dead stdin refuses at the write: Unverified, immediately.
    let dead = super::CodexVerifyBundle {
        stdin: Arc::new(Mutex::new(None)),
        next_id: Arc::new(AtomicU64::new(1)),
        requests: Arc::new(super::CodexRequests::new()),
    };
    assert_eq!(
        super::verify_codex_mcp_with_timeout(&dead, Duration::from_secs(5)),
        ToolsState::Unverified
    );
}

#[test]
fn codex_verify_bundle_comes_with_the_carrier_only() {
    // Which spawns verify is a one-line rule, pinned without spawning.
    let stdin = Arc::new(Mutex::new(None));
    let next_id = Arc::new(AtomicU64::new(1));
    let requests = Arc::new(super::CodexRequests::new());
    assert!(
        super::codex_verify_bundle_for(&None, &stdin, &next_id, &requests).is_none(),
        "today's only road verifies nothing"
    );
    assert!(
        super::codex_verify_bundle_for(
            &Some(crate::mcp_broker::McpProviderConfig::default()),
            &stdin,
            &next_id,
            &requests
        )
        .is_some(),
        "a minted carrier is verified"
    );
}

#[test]
fn codex_stderr_belt_marks_unverified_on_invalid_configuration() {
    // S7 belt (never authoritative): the exact stderr line the probe measured
    // flips the runtime Unverified; anything else leaves it alone. A real
    // child writes it, so this runs the drain, not the predicate.
    use crate::session::StderrSource;
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let run = |line: &str| {
        let script = format!(
            "console.error({});",
            serde_json::to_string(line).expect("quote")
        );
        let mut child = std::process::Command::new("node")
            .args(["-e", &script])
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("node writes stderr");
        let stderr = child.stderr.take().expect("stderr");
        let runtime = Arc::new(super::super::session_runtime::SessionRuntime::new());
        let handle = Box::new(super::CodexStderr::start(stderr)).spawn(Arc::clone(&runtime));
        let _ = child.wait();
        handle.expect("drain joins").join().expect("drain returns");
        runtime.tools_state()
    };
    assert_eq!(
        run("ERROR codex_app_server: Invalid configuration; using defaults."),
        crate::mcp_broker::ToolsState::Unverified,
        "the probe's line trips the belt"
    );
    assert_eq!(
        run("some unrelated warning"),
        crate::mcp_broker::ToolsState::Unavailable,
        "anything else leaves the state alone"
    );
}

#[test]
fn codex_verify_trigger_flips_the_runtime_detached() {
    // S8 trigger, driving the real `spawn_codex_verify_thread`: `None` is a
    // no-op; a carrier bundle flips the runtime when the poll lands, on a
    // thread that is not this one (the create path never waits for it).
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let runtime = Arc::new(SessionRuntime::new());
    super::super::spawn_codex_verify_thread(None, &runtime, "s.verify.none");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        runtime.tools_state(),
        crate::mcp_broker::ToolsState::Unavailable,
        "no bundle changes nothing"
    );
    // A fake child answering the golden list, pumped like the reader would.
    let mut child = std::process::Command::new("node")
            .args(["-e",
                "let b='';process.stdin.on('data',c=>{b+=c;let n;while((n=b.indexOf('\\n'))>=0){const l=b.slice(0,n);b=b.slice(n+1);if(!l.trim())continue;let m;try{m=JSON.parse(l)}catch{continue}if(m.id===undefined||m.id===null)continue;process.stdout.write(JSON.stringify({id:m.id,result:{data:[{name:'devboule',runtimeStatus:null,tools:{t:{}},toolsError:null}]}})+'\\n');}});"])
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("node is required for the Codex trigger test");
    let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
    let stdout = child.stdout.take().expect("stdout");
    let requests = Arc::new(super::CodexRequests::new());
    let pump_requests = Arc::clone(&requests);
    let pump = std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                continue;
            };
            pump_requests.deliver(&value);
        }
    });
    let bundle = super::codex_verify_bundle_for(
        &Some(crate::mcp_broker::McpProviderConfig::default()),
        &stdin,
        &Arc::new(AtomicU64::new(1)),
        &requests,
    )
    .expect("a carrier verifies");
    super::super::spawn_codex_verify_thread(Some(bundle), &runtime, "s.verify.some");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if runtime.tools_state() == crate::mcp_broker::ToolsState::Hosted {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "the detached flip lands: {:?}",
            runtime.tools_state()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    let _ = child.kill();
    let _ = child.wait();
    pump.join().expect("pump drains");
}

#[test]
fn codex_none_road_installs_no_carrier_and_verifies_nothing() {
    // End-of-pass OFF property, executable: production's `None` road spawns
    // with no extra env and no verification bundle — a Codex session obtains
    // no bearer, no launch-line override and no tool. (The gate itself is
    // pinned unit-level by `registration_is_a_fact`; this pins the spawn road
    // that S9 will light.)
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let state = crate::server::ServerState::new("codex-none-road".to_string());
    let runtime_dir = state.sessions.runtime_dir().to_path_buf();
    // The `--` is not decoration: the launcher appends its own flags after the
    // app-server args (the goals gate, `codex_goals::Goals::launch_args`), and
    // without the separator node would try to parse them as its own options.
    let command = crate::session::PtyCommand::new(
        "node",
        vec![
            "-e".to_string(),
            FAKE_CODEX_HANDSHAKE.to_string(),
            "--".to_string(),
        ],
        crate::test_dirs::test_temp_dir("devboule-codex-cwd"),
        Vec::new(),
    );
    let mut spawned = super::spawn_process(
        &state,
        command,
        None,
        crate::profile_delivery::ProfileDelivery::none(),
    )
    .expect("the None road spawns exactly as before");
    assert!(
        spawned.pending_codex_verify.is_none(),
        "no carrier means no verification bundle"
    );
    spawned.killer.kill();
    let orphans: Vec<_> = std::fs::read_dir(&runtime_dir)
        .expect("runtime dir")
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("devboule-codex-home-"))
        })
        .collect();
    assert!(orphans.is_empty(), "no home prepared: {orphans:?}");
}

/// A fake child for the live carrier road: handshake answers plus a golden
/// `mcpServerStatus/list` after `LIST_DELAY_MS`.
const ROAD_FAKE: &str = r#"
const listDelay = parseInt(process.env.LIST_DELAY_MS || "0", 10);
let buf = "";
process.stdin.on("data", (chunk) => {
  buf += chunk.toString();
  let nl;
  while ((nl = buf.indexOf("\n")) >= 0) {
    const line = buf.slice(0, nl);
    buf = buf.slice(nl + 1);
    if (!line.trim()) continue;
    let msg;
    try { msg = JSON.parse(line); } catch { continue; }
    if (msg.id === undefined || msg.id === null) continue;
    const reply = (result) => process.stdout.write(JSON.stringify({ id: msg.id, result }) + "\n");
    if (msg.method === "initialize") reply({ userAgent: "fake-road" });
    else if (msg.method === "model/list") reply({ data: [{ id: "fake-model", isDefault: true }] });
    else if (msg.method === "thread/start") reply({ thread: { id: "thread-road" } });
    else if (msg.method === "mcpServerStatus/list") {
      const golden = { data: [{ name: "devboule", runtimeStatus: null, tools: { t: {} }, toolsError: null }] };
      if (listDelay > 0) setTimeout(() => reply(golden), listDelay);
      else reply(golden);
    }
  }
});
"#;

#[test]
fn codex_live_carrier_road_registers_verifies_and_lists() {
    // S9 wiring, end to end through production code: broker register (the
    // flipped gate admits Codex) → carrier → spawn → bind (Unverified
    // installed) → detached verify → roster lists the child as Hosted. The
    // part-2 report's two uncovered lines — the trigger CALL and the
    // else-bind LINE — are both load-bearing here: without the bind the word
    // never leaves Unavailable, without the trigger it never leaves
    // Unverified.
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    use crate::mcp_broker::ToolsState;
    let state = crate::server::ServerState::new("codex-road".to_string());
    let owner = devboule_protocol::OwnerId::new("local", "road").expect("owner");
    let id = "s.road.1";
    let guard = state
        .mcp
        .register_with_provider(
            id,
            &owner,
            &devboule_protocol::SessionKind::Codex,
            Some("codex"),
            crate::mcp_broker::AgentLineage::root(),
        )
        .expect("S9 registers Codex")
        .expect("a bearer is minted");
    assert!(state.mcp.is_registered(id));
    let config = state.mcp.launch_config(id).expect("launch config");
    let command = crate::session::PtyCommand::new(
        "node",
        // `--` so the carrier's `-c` additions the spawn appends are read as
        // script arguments by the fake child, not as node options.
        vec!["-e".to_string(), ROAD_FAKE.to_string(), "--".to_string()],
        crate::test_dirs::test_temp_dir("devboule-codex-cwd"),
        vec![("LIST_DELAY_MS".to_string(), "1500".to_string())],
    );
    let spawned = super::spawn_process(
        &state,
        command,
        Some(config),
        crate::profile_delivery::ProfileDelivery::none(),
    )
    .expect("the carrier road spawns");
    assert!(
        spawned.pending_codex_verify.is_some(),
        "a minted carrier verifies"
    );
    let metadata = devboule_protocol::Session {
        id: id.to_string(),
        workspace_id: None,
        cwd: None,
        kind: devboule_protocol::SessionKind::Codex,
        title: "Road".to_string(),
        state: devboule_protocol::SessionState::Live { generation: 1 },
        elapsed_ms: Some(0),
        provider: Some("codex".to_string()),
        peer_session_id: None,
        created_at_ms: 1,
        origin: devboule_protocol::SessionOrigin::local(),
        display_name: Some("road".to_string()),
        created_by: None,
        profile_id: None,
        context_id: None,
        unattended: devboule_protocol::UnattendedState::No,
        labels: Default::default(),
        resumable: false,
    };
    crate::session::start_spawned_session(
        &state,
        &state.sessions,
        metadata,
        owner.clone(),
        None,
        None,
        spawned,
        Some(guard),
    )
    .expect("the road starts");
    // Unverified first (the else-bind ran), Hosted after the delayed poll
    // (the trigger ran): order matters, and the delay makes it deterministic.
    let deadline = Instant::now() + Duration::from_secs(25);
    let mut saw_unverified = false;
    let hosted = loop {
        let entries = state.sessions.live_agent_entries(&owner).expect("roster");
        if let Some(entry) = entries.iter().find(|entry| entry.session.id == id) {
            let word = entry.runtime.tools_state();
            if word == ToolsState::Unverified {
                saw_unverified = true;
            }
            if word == ToolsState::Hosted {
                break true;
            }
        }
        if Instant::now() >= deadline {
            break false;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(
        saw_unverified,
        "bind installed Unverified before verify landed"
    );
    assert!(hosted, "the detached poll flipped the roster to Hosted");
    let _ = state.sessions.close(id, &owner, &None);
}

/// Attached-runtime helper mirroring ACP's: a runtime with a broker plus a
/// subscription whose published events the test can pull back out.
fn attached_runtime(
    session_id: &str,
    broker: Arc<PermissionBroker>,
) -> (Arc<SessionRuntime>, Arc<ConnHandle>) {
    let runtime = SessionRuntime::for_acp(session_id.to_string(), None, Arc::clone(&broker));
    let conn = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("attach");
    conn.track_with_agent_replay(
        session_id,
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    (runtime, conn)
}

#[test]
fn codex_bearer_is_redacted_from_stderr_before_delivery() {
    // Broker-4: a bearer-shaped secret planted in a Codex stderr line must
    // not reach any observer's transcript. The belt still reads the raw
    // marker line in the same call.
    let broker = PermissionBroker::for_test(Arc::new(|_, _| Ok(())));
    let (runtime, conn) = attached_runtime("stderr-redaction-codex", broker);
    runtime.set_mcp_bearer("opaque-bearer-codex".to_string());
    runtime.set_mcp_url("http://127.0.0.1:4567/mcp".to_string());
    super::publish_stderr_line(
        &runtime,
        "codex echoed Bearer opaque-bearer-codex at http://127.0.0.1:4567/mcp".to_string(),
    );
    let event = conn
        .pull_events()
        .into_iter()
        .find_map(|event| match event.envelope.event {
            SessionEvent::AgentStderr { data } => Some(data),
            _ => None,
        })
        .expect("stderr event");
    assert_eq!(event, "codex echoed Bearer [redacted] at [redacted]");
    super::publish_stderr_line(
        &runtime,
        "ERROR codex_app_server: Invalid configuration; using defaults.".to_string(),
    );
    assert_eq!(
        runtime.tools_state(),
        crate::mcp_broker::ToolsState::Unverified,
        "the belt still reads the raw marker"
    );
}

#[test]
fn codex_spawn_failure_names_the_program_that_would_not_start() {
    // The 9th early-error path: an unstartable program refuses with the
    // program named, and no child exists to kill.
    let state = crate::server::ServerState::new("codex-early-error".to_string());
    let command = crate::session::PtyCommand::new(
        "devboule-no-such-program-9f1a",
        Vec::new(),
        crate::test_dirs::test_temp_dir("devboule-codex-cwd"),
        Vec::new(),
    );
    let config =
        crate::mcp_broker::McpLaunchConfig::for_test("http://127.0.0.1:9/mcp", "early-error-token");
    let error = match super::spawn_process(
        &state,
        command,
        Some(config),
        crate::profile_delivery::ProfileDelivery::none(),
    ) {
        Err(error) => error,
        Ok(_) => panic!("an unstartable program fails the spawn"),
    };
    assert!(
        error.message.contains("Could not start Codex")
            && error.message.contains("devboule-no-such-program-9f1a"),
        "the refusal names the program: {}",
        error.message
    );
}

#[test]
fn codex_carrier_road_adds_no_home_redirect_to_the_child_env() {
    // The contract the removed per-session carrier left behind: the child
    // keeps the human's Codex home, where the credentials and the rollouts
    // live — a daemon-chosen `CODEX_HOME` carries no `auth.json`, and every
    // turn against it answers 401. The fake child records the env it was
    // actually started with, so re-adding the redirect fails here, not in a
    // comment. (That this road owns no file is asserted by
    // `codex_mcp_launch_rides_config_overrides_and_keeps_the_token_in_env`.)
    if let Some(reason) = crate::test_support::external_program_skip_reason("node") {
        eprintln!("{reason}");
        return;
    }
    let state = crate::server::ServerState::new("codex-home-contract".to_string());
    let dir = crate::test_dirs::test_temp_dir("devboule-codex-home");
    let config =
        crate::mcp_broker::McpLaunchConfig::for_test("http://127.0.0.1:9/mcp", "home-contract");
    let (mut spawned, argv, env) = spawn_carrier_road(&state, config, &dir, &["CODEX_HOME"]);
    assert!(
        argv.contains(&format!(
            "mcp_servers.{}.url=",
            crate::mcp_broker::MCP_SERVER_NAME
        )),
        "the carrier still rode argv: {argv}"
    );
    assert_eq!(
        env.get("CODEX_HOME").and_then(serde_json::Value::as_str),
        std::env::var("CODEX_HOME").ok().as_deref(),
        "the child inherits the human's home, never one the daemon chose: {env}"
    );
    spawned.killer.kill();
    let _ = std::fs::remove_dir_all(&dir);
}

/// The real launch line, resolved exactly like production
/// (`find_available("codex")` → shim-unwrapped `app_server_command`). `None`
/// when Codex is not runnable here: the live tests skip, like the node tests.
///
/// Deliberately NOT gated on spawning bare `codex`: on Windows CreateProcess
/// cannot run npm's extensionless shim (probe-measured PATH trap), while the
/// catalog resolves the shim to `node <script>` — so the catalog IS the gate.
fn live_codex_command() -> Option<(std::path::PathBuf, Vec<String>)> {
    let agent = crate::provider_catalog::find_available("codex")?;
    let mut argv = agent.app_server_command?;
    if argv.is_empty() {
        return None;
    }
    let program = std::path::PathBuf::from(argv.remove(0));
    Some((program, argv))
}

/// Drive one app-server request against a live child and read the `result`.
/// No thread needed: `initialize` + `mcpServerStatus/list` are pre-thread.
fn live_codex_result(
    program: &std::path::Path,
    args: &[String],
    token: &str,
    method: &str,
    params: serde_json::Value,
) -> serde_json::Value {
    let mut child = std::process::Command::new(program)
        .args(args)
        .env(crate::mcp_broker::MCP_TOKEN_ENV, token)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .current_dir(crate::test_dirs::test_temp_dir("devboule-codex-cwd"))
        .spawn()
        .expect("live codex spawns");
    let stdin = Arc::new(Mutex::new(Some(child.stdin.take().expect("stdin"))));
    let mut stdout = CodexStdout::spawn(child.stdout.take().expect("stdout")).expect("reader");
    let next_id = AtomicU64::new(1);
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut deferred = Vec::new();
    // The id counter is internal (`d-N`); no caller id needed.
    let mut send = |method: &str, params: serde_json::Value| {
        super::request_response(
            &mut stdout,
            &stdin,
            &next_id,
            method,
            params,
            deadline,
            &mut deferred,
        )
        .unwrap_or_else(|_| panic!("live {method} answers"))
    };
    let _initialize = send("initialize", super::initialize_params());
    // Notifications get no response, so they ride `send_frame`, never
    // `request_response` (which would park until the deadline). Mirrors
    // `perform_handshake`'s own `initialized` send.
    super::send_frame(
        &stdin,
        &super::notification_frame("initialized", serde_json::json!({})),
        "Codex",
    )
    .expect("live initialized notifies");
    let result = send(method, params);
    let _ = child.kill();
    let _ = child.wait();
    result
}
