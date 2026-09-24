//! Tests for the ACP client: the handshake, prompt delivery and permissions.

use super::super::permission_broker::{
    permission, permission_path, test_broker, PermissionBroker, MAX_ACP_PERMISSION_FIELD_BYTES,
};
use super::{
    acp_request_error_message, complete_lines, is_mcp_status, observe_mcp_status,
    redact_handshake_error, AcpReader, PendingSwitch, MAX_ACP_PERMISSION_LINE_BYTES,
    MAX_HANDSHAKE_ERROR_BYTES,
};
use crate::journal::Journal;
use crate::session::{ConnHandle, ReaderDispatch, SessionKiller, SessionRuntime};
use devboule_protocol::{
    ErrorCode, PermissionOutcome, SessionEvent, SessionKind, SessionModel, WireError,
};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Pass 2e step 2: a user row resolves through the same named road a
/// catalog row rides, to its own argv and env — the row's command is
/// explicit, so it resolves before the PATH/CDN walk, and the command is
/// stamped with the id the create named.
///
/// The rows go in through the real seam (`apply_user_rows`) and the rows
/// lock is held across the resolution so a concurrent production refresh
/// cannot swap them out under the test. While they are live the snapshot
/// answers only one id no other test names; every built-in answer is
/// exactly what it was.
#[test]
fn a_user_row_resolves_to_its_own_command_and_env() {
    let document = br#"{"row-agent": {"extends": "acp",
            "command": ["/usr/local/bin/row-agent", "--serve"],
            "env": {"ROW_KEY": "row-value"}}}"#;
    let rows = crate::user_providers::parse_providers_document(
        document,
        &crate::session::native_family_ids(),
    )
    .expect("a valid row document");

    let gate = crate::user_providers::lock_rows_state();
    crate::session::apply_user_rows(rows);
    let paths = crate::paths::RuntimePaths::from_dir("row-agent-test");
    let command =
        super::resolve_named("row-agent", &paths).expect("the row resolves through the named road");
    assert_eq!(command.program, "/usr/local/bin/row-agent");
    assert_eq!(command.args, vec!["--serve".to_string()]);
    assert_eq!(
        command.env,
        vec![("ROW_KEY".to_string(), "row-value".to_string())]
    );
    assert_eq!(command.provider_id.as_deref(), Some("row-agent"));

    // Back the rows out while still holding the lock, so the live
    // snapshot the rest of the suite sees is the builtins-only one.
    crate::session::apply_user_rows(std::collections::BTreeMap::new());
    drop(gate);
}

#[test]
fn mcp_status_is_parsed_as_a_hint_and_failure_is_reported() {
    let runtime = Arc::new(SessionRuntime::new());
    runtime.require_mcp();
    let ready = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "_x.ai/mcp/server_status",
        "params": {
            "name": "devboule",
            "status": "ready",
            "reason": "initialized"
        }
    });
    assert!(is_mcp_status(&ready));
    observe_mcp_status(&ready, &runtime);
    assert!(runtime
        .wait_for_mcp_ready(Duration::from_millis(1))
        .is_err());

    let failed = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "_x.ai/mcp/server_status",
        "params": {
            "name": "devboule",
            "status": "failed"
        }
    });
    observe_mcp_status(&failed, &runtime);
    let error = runtime
        .wait_for_mcp_ready(Duration::from_secs(1))
        .expect_err("provider failure must wake the gate");
    assert!(error.message.contains("ACP provider reported"));
}

#[test]
fn mcp_bearer_is_redacted_from_stderr_before_delivery() {
    let (broker, _) = test_broker();
    let (runtime, conn) = attached_runtime("stderr-redaction", broker);
    runtime.set_mcp_bearer("opaque-bearer".to_string());
    runtime.set_mcp_url("http://127.0.0.1:4567/mcp".to_string());
    super::publish_stderr_line(
        &runtime,
        "provider echoed Bearer opaque-bearer at http://127.0.0.1:4567/mcp".to_string(),
    );
    let event = conn
        .pull_events()
        .into_iter()
        .find_map(|event| match event.envelope.event {
            SessionEvent::AgentStderr { data } => Some(data),
            _ => None,
        })
        .expect("stderr event");
    assert_eq!(event, "provider echoed Bearer [redacted] at [redacted]");
    let journal_value = runtime.redact_mcp_value(&serde_json::json!({
        "echo": "Bearer opaque-bearer at http://127.0.0.1:4567/mcp"
    }));
    let journal_text = serde_json::to_string(&journal_value).expect("redacted JSON");
    assert!(!journal_text.contains("opaque-bearer"));
    assert!(!journal_text.contains("4567"));
}

#[test]
fn spawn_handshake_errors_redact_broker_details_with_and_without_stderr() {
    let config =
        crate::mcp_broker::McpLaunchConfig::for_test("http://127.0.0.1:4567/mcp", "opaque-bearer");
    let without_stderr = redact_handshake_error(
        WireError::new(
            ErrorCode::Io,
            "ACP request failed: Bearer opaque-bearer at http://127.0.0.1:4567/mcp",
        ),
        &[],
        Some(&config),
    );
    assert!(!without_stderr.message.contains("opaque-bearer"));
    assert!(!without_stderr.message.contains("4567"));

    let with_stderr = redact_handshake_error(
        WireError::new(ErrorCode::Io, "ACP request failed: handshake rejected"),
        &["provider echoed Bearer opaque-bearer at http://127.0.0.1:4567/mcp".to_string()],
        Some(&config),
    );
    assert!(with_stderr.message.contains("Agent stderr"));
    assert!(!with_stderr.message.contains("opaque-bearer"));
    assert!(!with_stderr.message.contains("4567"));
}

#[test]
fn request_error_without_a_message_never_serializes_the_object() {
    let error = serde_json::json!({
        "code": -32000,
        "data": {"token": "sk-LEAK"}
    });
    let text = acp_request_error_message(&error);
    assert!(
        !text.contains("sk-LEAK"),
        "structured error data must not reach a user-facing banner: {text}"
    );
    assert!(text.contains("(-32000)"), "code must stay visible: {text}");
}

#[test]
fn request_error_data_message_carries_the_provider_diagnosis() {
    let error = serde_json::json!({
        "code": -32603,
        "message": "Internal error",
        "data": {
            "message": "API error (status 402 Payment Required): Grok Build usage balance exhausted"
        }
    });
    let text = acp_request_error_message(&error);
    assert_eq!(
            text,
            "ACP request failed (-32603): API error (status 402 Payment Required): Grok Build usage balance exhausted"
        );
}

#[test]
fn request_error_data_message_surfaces_without_the_payload() {
    let error = serde_json::json!({
        "code": -32603,
        "message": "Internal error",
        "data": {
            "message": "API error (status 402 Payment Required): Grok Build usage balance exhausted",
            "token": "sk-LEAK"
        }
    });
    let text = acp_request_error_message(&error);
    assert!(
        text.contains("402 Payment Required"),
        "the diagnosis in data.message must reach the banner: {text}"
    );
    assert!(
        !text.contains("sk-LEAK"),
        "the rest of the payload must not reach the banner: {text}"
    );
    assert!(text.contains("(-32603)"), "code must stay visible: {text}");
}

#[test]
fn request_error_data_message_must_be_a_string_to_surface() {
    let error = serde_json::json!({
        "code": -32603,
        "data": {"message": {"token": "sk-LEAK"}}
    });
    let text = acp_request_error_message(&error);
    assert!(
        !text.contains("sk-LEAK"),
        "a non-string data.message is payload, not diagnosis: {text}"
    );
    assert!(text.contains("(-32603)"), "code must stay visible: {text}");
}

#[test]
fn request_error_empty_data_message_falls_back_to_the_error_message() {
    let error = serde_json::json!({
        "code": -32000,
        "message": "Authentication required: run the provider login",
        "data": {"message": ""}
    });
    let text = acp_request_error_message(&error);
    assert_eq!(
        text, "ACP request failed (-32000): Authentication required: run the provider login",
        "an empty data.message must not silence the envelope's own sentence"
    );
}

#[test]
fn poisoned_pending_response_tracking_publishes_an_agent_error() {
    let pending = Arc::new(Mutex::new(HashSet::from([7_u64])));
    let poisoned = Arc::clone(&pending);
    let panic = thread::spawn(move || {
        let _guard = poisoned.lock().expect("pending lock");
        panic!("poison pending lock");
    })
    .join();
    assert!(panic.is_err());

    let (broker, _) = test_broker();
    let (runtime, conn) = attached_runtime("stub-session", Arc::clone(&broker));
    let reader = AcpReader::for_test(pending, "stub-session".to_string(), broker);
    reader.dispatch_line(r#"{"jsonrpc":"2.0","id":7,"result":{}}"#, &runtime);

    let events = conn.pull_events();
    assert!(events.iter().any(|event| {
        matches!(
            &event.envelope.event,
            SessionEvent::AgentError { message }
                if message.contains("response tracking lock was poisoned")
                    && message.contains("switch outcome is unknown")
        )
    }));
}

#[cfg(windows)]
#[test]
fn vendor_switch_without_a_manifest_still_publishes_the_new_model() {
    use super::{AcpHost, AcpTransport};
    use std::process::{Command, Stdio};

    let mut child = Command::new("cmd.exe")
        .args(["/c", "exit"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("cmd");
    let stdin = child.stdin.take().expect("stdin");
    let cwd = crate::test_dirs::test_temp_dir("devboule-acp-host");
    let host = AcpHost::new(cwd.clone(), cwd);
    let transport = Arc::new(AcpTransport::new(stdin, Arc::clone(&host)));
    transport.set_session_id("stub-session".to_string());
    let (broker, _) = test_broker();
    let (runtime, conn) = attached_runtime("stub-session", Arc::clone(&broker));
    let reader = AcpReader::for_test_with_transport(
        Arc::new(Mutex::new(HashSet::new())),
        "stub-session".to_string(),
        broker,
        host,
        transport,
    );

    reader.complete_vendor_switch(&runtime, "fallback-model".to_string(), None, None, None);

    let events = conn.pull_events();
    assert!(events.iter().any(|event| {
        matches!(
            &event.envelope.event,
            SessionEvent::SessionManifest {
                current_model_id: Some(model_id),
                ..
            } if model_id == "fallback-model"
        )
    }));
    let _ = child.wait();
}

#[cfg(windows)]
#[test]
fn negotiated_prompt_capabilities_are_kept_on_the_session_transport() {
    use super::{AcpHost, AcpTransport};
    use crate::acp_view::{PromptCapabilities, PromptCapabilityState};
    use std::process::{Command, Stdio};

    let mut child = Command::new("cmd.exe")
        .args(["/c", "exit"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("cmd");
    let stdin = child.stdin.take().expect("stdin");
    let cwd = crate::test_dirs::test_temp_dir("devboule-acp-host");
    let host = AcpHost::new(cwd.clone(), cwd);
    let transport = AcpTransport::new(stdin, host);

    // A session that never declared anything stays absent, not `false`.
    assert_eq!(
        transport.prompt_capabilities(),
        PromptCapabilities::default()
    );

    transport.set_prompt_capabilities(PromptCapabilities {
        image: PromptCapabilityState::Supported,
        audio: PromptCapabilityState::Unsupported,
        embedded_context: PromptCapabilityState::Absent,
    });
    let stored = transport.prompt_capabilities();
    assert_eq!(stored.image, PromptCapabilityState::Supported);
    assert_eq!(stored.audio, PromptCapabilityState::Unsupported);
    assert_eq!(stored.embedded_context, PromptCapabilityState::Absent);
    let _ = child.wait();
}

#[cfg(windows)]
#[test]
fn poisoned_manifest_lock_preserves_the_prior_model_catalog() {
    use super::{AcpHost, AcpTransport};
    use std::process::{Command, Stdio};

    let mut child = Command::new("cmd.exe")
        .args(["/c", "exit"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("cmd");
    let stdin = child.stdin.take().expect("stdin");
    let cwd = crate::test_dirs::test_temp_dir("devboule-acp-host");
    let host = AcpHost::new(cwd.clone(), cwd);
    let transport = Arc::new(AcpTransport::new(stdin, Arc::clone(&host)));
    transport.remember_manifest(&SessionEvent::SessionManifest {
        provider_id: Some("stub".to_string()),
        current_model_id: Some("old-model".to_string()),
        models: vec![
            SessionModel {
                model_id: "old-model".to_string(),
                name: "Old model".to_string(),
                description: None,
                context_tokens: None,
                current_effort: None,
                efforts: None,
            },
            SessionModel {
                model_id: "new-model".to_string(),
                name: "New model".to_string(),
                description: None,
                context_tokens: None,
                current_effort: None,
                efforts: None,
            },
        ],
        modes: None,
    });
    let poisoned = Arc::clone(&transport);
    let panic = thread::spawn(move || {
        let _guard = poisoned.last_manifest.lock().expect("manifest lock");
        panic!("poison manifest lock");
    })
    .join();
    assert!(panic.is_err());

    let (broker, _) = test_broker();
    let (runtime, conn) = attached_runtime("stub-session", Arc::clone(&broker));
    let reader = AcpReader::for_test_with_transport(
        Arc::new(Mutex::new(HashSet::new())),
        "stub-session".to_string(),
        broker,
        host,
        transport,
    );
    reader.complete_vendor_switch(&runtime, "new-model".to_string(), None, None, None);

    let events = conn.pull_events();
    assert!(events.iter().any(|event| {
        matches!(
            &event.envelope.event,
            SessionEvent::SessionManifest {
                current_model_id: Some(model_id),
                models,
                ..
            } if model_id == "new-model"
                && models.len() == 2
                && models.iter().any(|model| model.model_id == "new-model")
        )
    }));
    let _ = child.wait();
}

#[test]
fn turn_error_does_not_publish_structured_error_data() {
    let (broker, _) = test_broker();
    let (runtime, conn) = attached_runtime("stub-session", Arc::clone(&broker));
    let mut reader = AcpReader::for_test(
        Arc::new(Mutex::new(HashSet::from([9u64]))),
        "stub-session".to_string(),
        broker,
    );
    let turn_id = runtime.turn_counter();
    runtime.begin_turn();
    reader.turn.start_prompt(9);
    reader
            .feed(
                br#"{"jsonrpc":"2.0","id":9,"error":{"code":-32602,"message":"unknown model","data":{"secret":"do-not-publish"}}}
"#,
                &runtime,
            )
            .expect("feed");
    let events = conn.pull_events();
    assert_eq!(events.len(), 2, "error response must close the turn");
    assert!(matches!(
        &events[0].envelope.event,
        SessionEvent::AgentError { message }
            if message == "ACP request 9 failed: ACP request failed (-32602): unknown model"
    ));
    assert!(matches!(
        &events[1].envelope.event,
        SessionEvent::AgentFinished { stop_reason, .. } if stop_reason == "error"
    ));
    assert!(!runtime.is_turn_active(turn_id));
    let message = match &events[0].envelope.event {
        SessionEvent::AgentError { message } => message,
        _ => unreachable!("first event was not the ACP error"),
    };
    assert_eq!(
        message,
        "ACP request 9 failed: ACP request failed (-32602): unknown model"
    );
    assert!(!message.contains("do-not-publish"));
}

#[test]
fn silent_prompt_abandonment_publishes_error_and_finishes_the_turn() {
    let (broker, _) = test_broker();
    let (runtime, conn) = attached_runtime("stub-session", Arc::clone(&broker));
    let reader = AcpReader::for_test(
        Arc::new(Mutex::new(HashSet::from([88u64]))),
        "stub-session".to_string(),
        broker,
    );
    let turn_id = runtime.turn_counter();
    runtime.begin_turn();
    reader.turn.bind_runtime(&runtime);
    reader.turn.start_prompt(88);
    *reader.turn.last_activity.lock().expect("activity lock") =
        Instant::now() - reader.turn.silence - Duration::from_secs(1);

    reader.turn.tick();

    let events = conn.pull_events();
    assert_eq!(events.len(), 2, "watchdog abandonment must close the turn");
    assert!(matches!(
        &events[0].envelope.event,
        SessionEvent::AgentError { message } if message.contains("stayed silent")
    ));
    assert!(matches!(
        &events[1].envelope.event,
        SessionEvent::AgentFinished { stop_reason, .. } if stop_reason == "cancelled"
    ));
    assert!(!runtime.is_turn_active(turn_id));
}

#[test]
fn skipped_user_echo_does_not_burn_a_stream_sequence() {
    let (broker, _) = test_broker();
    let (runtime, conn) = attached_runtime("stub-session", Arc::clone(&broker));
    let reader = AcpReader::for_test(
        Arc::new(Mutex::new(HashSet::new())),
        "stub-session".to_string(),
        broker,
    );
    reader.dispatch_line(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"stub-session","update":{"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"echo"}}}}
"#,
            &runtime,
        );
    assert_eq!(
        runtime.current_agent_seq(),
        0,
        "skipped echo consumed a seq"
    );
    reader.dispatch_line(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"stub-session","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"reply"}}}}
"#,
            &runtime,
        );
    let _event = conn
        .pull_events()
        .into_iter()
        .find(|event| matches!(event.envelope.event, SessionEvent::AgentMessage { .. }))
        .expect("reply event");
    assert_eq!(runtime.current_agent_seq(), 1);
}

#[test]
fn foreign_user_echo_is_not_silently_dropped() {
    let (broker, _) = test_broker();
    let (runtime, conn) = attached_runtime("stub-session", Arc::clone(&broker));
    let reader = AcpReader::for_test(
        Arc::new(Mutex::new(HashSet::new())),
        "stub-session".to_string(),
        broker,
    );
    reader.dispatch_line(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"other-session","update":{"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"foreign echo"}}}}
"#,
            &runtime,
        );
    assert_eq!(runtime.current_agent_seq(), 1);
    assert!(conn.pull_events().is_empty());
}

#[test]
fn non_text_content_chunk_is_counted_not_silently_dropped() {
    let (broker, _) = test_broker();
    let (runtime, conn) = attached_runtime("stub-session", Arc::clone(&broker));
    let reader = AcpReader::for_test(
        Arc::new(Mutex::new(HashSet::new())),
        "stub-session".to_string(),
        broker,
    );
    assert_eq!(reader.unmodeled_content_count.load(Ordering::Relaxed), 0);
    reader.dispatch_line(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"stub-session","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"image","mimeType":"image/png","data":"AAAA"}}}}
"#,
            &runtime,
        );
    assert_eq!(
        reader.unmodeled_content_count.load(Ordering::Relaxed),
        1,
        "an image block must be counted, not discarded without trace"
    );
    assert!(
        !conn
            .pull_events()
            .iter()
            .any(|event| matches!(event.envelope.event, SessionEvent::AgentMessage { .. })),
        "an image block must not be rendered as an empty message"
    );

    // A text chunk is modeled, so it must not move the counter.
    reader.dispatch_line(
            r#"{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"stub-session","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"hello"}}}}
"#,
            &runtime,
        );
    assert_eq!(reader.unmodeled_content_count.load(Ordering::Relaxed), 1);
    assert!(conn.pull_events().iter().any(|event| matches!(
        event.envelope.event,
        SessionEvent::AgentMessage { ref text, .. } if text == "hello"
    )));
}

#[test]
fn model_switch_response_does_not_finish_a_live_prompt() {
    let (broker, _) = test_broker();
    let (runtime, _conn) = attached_runtime("stub-session", Arc::clone(&broker));
    let pending = Arc::new(Mutex::new(HashSet::from([42u64])));
    let reader = AcpReader::for_test(Arc::clone(&pending), "stub-session".to_string(), broker);
    reader
        .model_switches
        .lock()
        .expect("model-switch lock")
        .insert(
            42,
            PendingSwitch::SetModel {
                model_id: "new-model".to_string(),
                effort: None,
                alternate: None,
                followup: None,
            },
        );
    reader.turn.start_prompt(42);
    let mut reader = reader;
    reader
        .feed(
            br#"{"jsonrpc":"2.0","id":42,"result":{"_meta":{"model":{"Ok":"new-model"}}}}
"#,
            &runtime,
        )
        .expect("feed");
    assert!(
        reader.turn.prompt_is_live(),
        "a model-switch response must not finish a live prompt"
    );
}

#[test]
fn journal_keeps_raw_envelope_and_replay_derives_the_view() {
    let path = permission_path("envelope");
    let journal = Journal::open(&path).expect("journal");
    journal
        .upsert_blocking(crate::journal::new_session_record(
            "s.envelope",
            "owner",
            None,
            devboule_protocol::SessionKind::Acp,
            "Agent",
        ))
        .expect("upsert");
    let envelope = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": "01a06c70-ea2b-7882-ad27-aae8188fc243",
            "update": {
                "sessionUpdate": "agent_thought_chunk",
                "content": {"type": "text", "text": "The"}
            }
        }
    });
    journal
        .append_blocking(
            crate::journal::acp_envelope_record("s.envelope", 1, 1, &envelope).expect("record"),
        )
        .expect("append");
    let replay = journal.replay("s.envelope").expect("replay");
    assert!(
        replay.events.iter().any(|event| matches!(
            event,
            SessionEvent::AgentThought { text, .. } if text == "The"
        )),
        "replay lost the derived thought: {:?}",
        replay.events
    );
    journal.shutdown();
    let stored: serde_json::Value = {
        let conn = rusqlite::Connection::open(&path).expect("inspect");
        let payload: Vec<u8> = conn
            .query_row(
                "SELECT payload FROM events WHERE session_id = ?1 AND kind = 'acp_envelope'",
                ["s.envelope"],
                |row| row.get(0),
            )
            .expect("payload");
        serde_json::from_slice(&payload).expect("json")
    };
    assert_eq!(stored["method"], "session/update");
    assert_eq!(
        stored["params"]["update"]["sessionUpdate"],
        "agent_thought_chunk"
    );
    let _ = std::fs::remove_file(path);
}

#[test]
fn old_journaled_user_echo_still_replays_as_a_user_message() {
    let path = permission_path("old-user-echo-replay");
    let journal = Journal::open(&path).expect("journal");
    journal
        .upsert_blocking(crate::journal::new_session_record(
            "s.old-user-echo",
            "owner",
            None,
            SessionKind::Acp,
            "Agent",
        ))
        .expect("upsert");
    let envelope = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": "stub-session",
            "update": {
                "sessionUpdate": "user_message_chunk",
                "content": {"type": "text", "text": "old prompt"}
            }
        }
    });
    journal
        .append_blocking(
            crate::journal::acp_envelope_record("s.old-user-echo", 1, 1, &envelope)
                .expect("record"),
        )
        .expect("append");
    let replay = journal.replay("s.old-user-echo").expect("replay");
    assert!(replay.events.iter().any(|event| matches!(
        event,
        SessionEvent::AgentUserMessage { text, .. } if text == "old prompt"
    )));
    journal.shutdown();
    let _ = std::fs::remove_file(path);
}

#[test]
fn initialize_declares_only_implemented_fs_and_terminal() {
    let params = super::advertised_initialize_params().expect("initialize params");
    assert_eq!(params["clientCapabilities"]["fs"]["readTextFile"], true);
    assert_eq!(params["clientCapabilities"]["fs"]["writeTextFile"], true);
    assert_eq!(params["clientCapabilities"]["terminal"], true);
    assert!(
        params["clientCapabilities"].get("elicitation").is_none()
            || params["clientCapabilities"]["elicitation"].is_null()
    );
    assert_eq!(params["clientInfo"]["name"], "devboule");
}

#[test]
fn ndjson_buffers_partial_lines_and_strips_crlf_at_dispatch_boundary() {
    let mut buffer = b"{\"id\":1}\r".to_vec();
    assert!(complete_lines(&mut buffer).is_empty());
    buffer.extend_from_slice(b"\n{\"id\":2");
    let lines = complete_lines(&mut buffer);
    assert_eq!(lines, vec![b"{\"id\":1}\r\n".to_vec()]);
    assert_eq!(buffer, b"{\"id\":2");
}

#[test]
fn oversized_permission_field_is_cancelled_before_storage() {
    let (broker, sent) = test_broker();
    let reader = AcpReader::for_test(
        Arc::new(Mutex::new(HashSet::new())),
        "stub-session".to_string(),
        Arc::clone(&broker),
    );
    let runtime = Arc::new(SessionRuntime::new());
    reader.dispatch_permission(
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 62,
            "method": "session/request_permission",
            "params": {
                "sessionId": "stub-session",
                "title": "x".repeat(MAX_ACP_PERMISSION_FIELD_BYTES + 1),
                "toolCall": {"toolCallId": "oversized"},
                "options": [{"optionId": "allow", "name": "Allow once", "kind": "allow_once"}]
            }
        }),
        &runtime,
        None,
    );
    let sent = sent.lock().expect("sent lock");
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0].0, 62);
    assert_eq!(sent[0].1["outcome"]["outcome"], "cancelled");
    assert_eq!(broker.pending_len(), 0);
}

#[test]
fn oversized_unterminated_line_is_dropped_and_reported() {
    let (broker, _) = test_broker();
    let mut reader = AcpReader::for_test(
        Arc::new(Mutex::new(HashSet::new())),
        "stub-session".to_string(),
        broker,
    );
    let runtime = Arc::new(SessionRuntime::new());
    reader
        .feed(&vec![b'x'; MAX_ACP_PERMISSION_LINE_BYTES + 1], &runtime)
        .expect("oversized input is reported, not fatal to the reader");
    assert!(reader.buffer.is_empty());
}

#[test]
fn detached_permission_is_queued_for_capable_reattach_and_removed_after_expiry() {
    let (broker, _) = test_broker();
    let runtime = Arc::new(SessionRuntime::for_acp(
        "s.permission.queue".to_string(),
        None,
        Arc::clone(&broker),
    ));
    let first = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &first, true)
        .expect("first attach");
    first.track_with_agent_replay(
        "s.permission.queue",
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    runtime.detach_if_conn(first.id);

    let request = permission("queued");
    let pending = broker
        .register(7, request.clone(), &runtime)
        .expect("register");
    runtime.publish_agent_event(request, None);

    let second = ConnHandle::new(2);
    let outcome = runtime
        .try_attach_with_replay(None, &second, true)
        .expect("reattach");
    second.track_with_agent_replay(
        "s.permission.queue",
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    assert!(second.pull_events().iter().any(|event| matches!(
        event.envelope.event,
        SessionEvent::PermissionRequest { ref tool_call_id, .. } if tool_call_id == "queued"
    )));

    assert!(broker.expire("queued", &pending));
    let after_expiry = second.pull_events();
    assert!(
        after_expiry.iter().any(|event| matches!(
            event.envelope.event,
            SessionEvent::PermissionResolved { ref tool_call_id, .. } if tool_call_id == "queued"
        )),
        "expiry must tell the attached client the card is gone: {after_expiry:?}"
    );
    assert!(
        !after_expiry
            .iter()
            .any(|event| matches!(event.envelope.event, SessionEvent::PermissionRequest { .. })),
        "expiry must not re-deliver the request: {after_expiry:?}"
    );
}

#[test]
fn detached_permission_expiry_is_not_replayed_on_later_reattach() {
    let (broker, _) = test_broker();
    let runtime = Arc::new(SessionRuntime::for_acp(
        "s.permission.expired".to_string(),
        None,
        Arc::clone(&broker),
    ));
    let request = permission("expired-detached");
    let pending = broker
        .register(8, request.clone(), &runtime)
        .expect("register");
    runtime.publish_agent_event(request, None);
    assert!(broker.expire("expired-detached", &pending));

    let conn = ConnHandle::new(3);
    let outcome = runtime
        .try_attach_with_replay(None, &conn, true)
        .expect("reattach");
    conn.track_with_agent_replay(
        "s.permission.expired",
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    let events = conn.pull_events();
    assert!(
        !events
            .iter()
            .any(|event| matches!(event.envelope.event, SessionEvent::PermissionRequest { .. })),
        "expired permission must not replay as a request: {events:?}"
    );
}

#[test]
fn reattach_reemits_the_stored_session_manifest() {
    let (broker, _) = test_broker();
    let runtime =
        SessionRuntime::for_acp("s.manifest.reattach".to_string(), None, Arc::clone(&broker));
    runtime.store_session_manifest(SessionEvent::SessionManifest {
        provider_id: Some("grok".to_string()),
        current_model_id: Some("grok-4.6".to_string()),
        models: Vec::new(),
        modes: None,
    });

    let first = ConnHandle::new(1);
    let outcome = runtime
        .try_attach_with_replay(None, &first, true)
        .expect("attach");
    first.track_with_agent_replay(
        "s.manifest.reattach",
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    let first_events = first.pull_events();
    assert!(
        first_events.iter().any(|event| matches!(
            event.envelope.event,
            SessionEvent::SessionManifest {
                ref current_model_id,
                ..
            } if current_model_id.as_deref() == Some("grok-4.6")
        )),
        "first attach must deliver the stored manifest: {first_events:?}"
    );

    runtime.detach_if_conn(first.id);
    let second = ConnHandle::new(2);
    let outcome = runtime
        .try_attach_with_replay(None, &second, true)
        .expect("reattach");
    second.track_with_agent_replay(
        "s.manifest.reattach",
        Arc::clone(&runtime),
        false,
        None,
        outcome.generation,
        outcome.live_agent_replay,
    );
    let second_events = second.pull_events();
    assert!(
        second_events.iter().any(|event| matches!(
            event.envelope.event,
            SessionEvent::SessionManifest {
                ref current_model_id,
                ..
            } if current_model_id.as_deref() == Some("grok-4.6")
        )),
        "reattach must re-emit the stored manifest: {second_events:?}"
    );
}

#[cfg(windows)]
#[test]
fn reader_finish_releases_terminals_left_by_a_dead_agent() {
    use super::AcpHost;
    let cwd = crate::test_dirs::test_temp_dir("devboule-acp-finish-cwd");
    let runtime = crate::test_dirs::test_temp_dir("devboule-acp-finish-rt");
    let host = AcpHost::new(cwd.clone(), runtime.clone());
    host.set_session_id("stub-session".to_string());
    let (broker, _) = test_broker();
    let session_runtime = Arc::new(SessionRuntime::for_acp(
        "stub-session".to_string(),
        None,
        Arc::clone(&broker),
    ));
    host.bind_permission_gate(&broker, &session_runtime);
    let allow_broker = Arc::clone(&broker);
    let allow = thread::spawn(move || {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(id) = allow_broker.pending_ids().into_iter().next() {
                let _ = allow_broker.respond(&id, PermissionOutcome::AllowOnce);
                return;
            }
            if std::time::Instant::now() >= deadline {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
    });
    host.test_create_terminal(serde_json::json!({
        "sessionId": "stub-session",
        "command": "ping.exe",
        "args": ["-t", "127.0.0.1"]
    }))
    .expect("create lingering terminal");
    let _ = allow.join();
    assert_eq!(host.live_terminal_count(), 1);
    let (broker, _) = test_broker();
    let mut reader = AcpReader::for_test_on_host(
        Arc::new(Mutex::new(HashSet::new())),
        "stub-session".to_string(),
        broker,
        Arc::clone(&host),
    );
    let session_runtime = Arc::new(SessionRuntime::new());
    reader.finish(&session_runtime);
    assert_eq!(
        host.live_terminal_count(),
        0,
        "EOF must shut down ACP terminals the dead agent left behind"
    );
    let _ = std::fs::remove_dir_all(cwd);
    let _ = std::fs::remove_dir_all(runtime);
}

#[cfg(windows)]
#[test]
fn killer_does_not_block_on_a_full_agent_stdin() {
    use super::{AcpHost, AcpKiller, AcpTransport};
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let mut child = Command::new("ping.exe")
        .args(["-n", "99999", "127.0.0.1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000)
        .spawn()
        .expect("ping");
    let stdin = child.stdin.take().expect("stdin");
    let cwd = crate::test_dirs::test_temp_dir("devboule-acp-host");
    let host = AcpHost::new(cwd.clone(), cwd);
    let transport = Arc::new(AcpTransport::new(stdin, host));
    transport.set_session_id("stub-session".to_string());
    let runtime = Arc::new(SessionRuntime::new());
    transport
        .permission_broker
        .register(1, permission("stuck-kill"), &runtime)
        .expect("pending permission so close must write stdin");
    let filler = {
        let transport = Arc::clone(&transport);
        thread::spawn(move || {
            let blob = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "session/prompt",
                "params": { "pad": "x".repeat(4096) }
            });
            for _ in 0..64 {
                if transport.send_line(&blob).is_err() {
                    break;
                }
            }
        })
    };
    thread::sleep(Duration::from_millis(200));
    let mut killer = AcpKiller {
        process: Arc::new(Mutex::new(child)),
        permission_broker: Arc::clone(&transport.permission_broker),
        transport: Arc::clone(&transport),
        cancelled: Arc::new(AtomicBool::new(false)),
    };
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    let started = Instant::now();
    thread::spawn(move || {
        killer.kill();
        let _ = done_tx.send(started.elapsed());
    });
    let elapsed = done_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("kill blocked waiting on agent stdin");
    let _ = filler.join();
    assert!(
        elapsed < Duration::from_secs(2),
        "kill blocked for {elapsed:?} waiting on agent stdin"
    );
}

#[cfg(windows)]
#[test]
fn kill_unblocks_a_pending_terminal_create_gate() {
    use super::{AcpHost, AcpKiller, AcpTransport};
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::Instant;
    let mut child = Command::new("ping.exe")
        .args(["-n", "30", "127.0.0.1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000)
        .spawn()
        .expect("ping");
    let stdin = child.stdin.take().expect("stdin");
    let cwd = crate::test_dirs::test_temp_dir("devboule-acp-kill-gate");
    let host = AcpHost::new(cwd.clone(), cwd.clone());
    host.set_session_id("stub-session".to_string());
    let transport = Arc::new(AcpTransport::new(stdin, Arc::clone(&host)));
    let runtime = SessionRuntime::for_acp(
        "stub-session".to_string(),
        None,
        Arc::clone(&transport.permission_broker),
    );
    host.bind_permission_gate(&transport.permission_broker, &runtime);
    let create_host = Arc::clone(&host);
    let create = thread::spawn(move || {
        create_host.test_create_terminal(serde_json::json!({
            "sessionId": "stub-session",
            "command": "cmd.exe",
            "args": ["/c", "exit"]
        }))
    });
    let deadline = Instant::now() + Duration::from_secs(2);
    while transport.permission_broker.pending_len() == 0 {
        if create.is_finished() {
            panic!(
                "create finished without a pending gate: {:?}",
                create.join()
            );
        }
        if Instant::now() >= deadline {
            panic!("create never reached the terminal permission gate");
        }
        thread::sleep(Duration::from_millis(5));
    }
    let mut killer = AcpKiller {
        process: Arc::new(Mutex::new(child)),
        permission_broker: Arc::clone(&transport.permission_broker),
        transport: Arc::clone(&transport),
        cancelled: Arc::new(AtomicBool::new(false)),
    };
    let started = Instant::now();
    killer.kill();
    let result = create.join().expect("create thread");
    let elapsed = started.elapsed();
    let _ = std::fs::remove_dir_all(&cwd);
    let error = result.expect_err("kill must deny the pending terminal create");
    assert!(
        elapsed < Duration::from_secs(2),
        "kill left the terminal gate blocked for {elapsed:?}"
    );
    assert_eq!(error.code, -32001);
    assert_eq!(error.message, "the user denied this command");
    assert_eq!(host.spawned_count(), 0);
}

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

fn event_kinds(conn: &ConnHandle) -> Vec<&'static str> {
    conn.pull_events()
        .into_iter()
        .map(|event| match event.envelope.event {
            SessionEvent::AgentFinished { .. } => "finished",
            SessionEvent::AgentError { .. } => "error",
            SessionEvent::PermissionRequest { .. } => "permission",
            SessionEvent::PermissionResolved { .. } => "permission_resolved",
            SessionEvent::AgentThought { .. } => "thought",
            SessionEvent::Snapshot { .. } => "snapshot",
            _ => "other",
        })
        .collect()
}

#[test]
fn successful_prompt_publishes_one_finish_and_closes_the_turn() {
    let (broker, _) = test_broker();
    let (runtime, conn) = attached_runtime("stub-session", Arc::clone(&broker));
    let mut reader = AcpReader::for_test(
        Arc::new(Mutex::new(HashSet::from([7u64]))),
        "stub-session".to_string(),
        broker,
    );
    let turn_id = runtime.turn_counter();
    runtime.begin_turn();
    reader.turn.start_prompt(7);
    reader
        .feed(
            br#"{"jsonrpc":"2.0","id":7,"result":{"stopReason":"end_turn"}}"#
                .as_ref()
                .iter()
                .copied()
                .chain(std::iter::once(b'\n'))
                .collect::<Vec<_>>()
                .as_slice(),
            &runtime,
        )
        .expect("feed");

    let events = conn.pull_events();
    assert_eq!(
        events.len(),
        1,
        "successful prompt must finish exactly once"
    );
    assert!(matches!(
        &events[0].envelope.event,
        SessionEvent::AgentFinished { stop_reason, .. } if stop_reason == "end_turn"
    ));
    assert!(!runtime.is_turn_active(turn_id));
}

#[test]
fn late_prompt_result_after_cancel_is_not_a_second_outcome() {
    let (broker, _) = test_broker();
    let (runtime, conn) = attached_runtime("stub-session", Arc::clone(&broker));
    let mut reader = AcpReader::for_test(
        Arc::new(Mutex::new(HashSet::from([7u64]))),
        "stub-session".to_string(),
        broker,
    );
    reader.turn.start_prompt(7);
    reader.turn.abandon_live_prompt();
    let _ = event_kinds(&conn);
    reader
        .feed(
            br#"{"jsonrpc":"2.0","id":7,"result":{"stopReason":"end_turn"}}"#
                .as_ref()
                .iter()
                .copied()
                .chain(std::iter::once(b'\n'))
                .collect::<Vec<_>>()
                .as_slice(),
            &runtime,
        )
        .expect("feed");
    let kinds = event_kinds(&conn);
    assert!(
        !kinds.contains(&"finished"),
        "timed-out turn published AgentFinished: {kinds:?}"
    );
}

#[test]
fn permission_after_turn_cancel_is_not_shown_to_the_user() {
    let (broker, sent) = test_broker();
    let (runtime, conn) = attached_runtime("stub-session", Arc::clone(&broker));
    let reader = AcpReader::for_test(
        Arc::new(Mutex::new(HashSet::new())),
        "stub-session".to_string(),
        Arc::clone(&broker),
    );
    reader.turn.start_prompt(1);
    reader.turn.abandon_live_prompt();
    let _ = event_kinds(&conn);
    reader.dispatch_permission(
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 88,
            "method": "session/request_permission",
            "params": {
                "sessionId": "stub-session",
                "title": "Run command",
                "toolCall": {"toolCallId": "late-perm"},
                "options": [{"optionId": "allow", "name": "Allow once", "kind": "allow_once"}]
            }
        }),
        &runtime,
        None,
    );
    let kinds = event_kinds(&conn);
    assert_eq!(broker.pending_len(), 0, "late permission stayed pending");
    assert!(
        !kinds.contains(&"permission"),
        "cancelled turn published a permission prompt: {kinds:?}"
    );
    let sent = sent.lock().expect("sent lock");
    assert!(
        sent.iter()
            .any(|(id, result)| *id == 88 && result["outcome"]["outcome"] == "cancelled"),
        "agent was not told the late permission was cancelled: {sent:?}"
    );
}

#[cfg(windows)]
#[test]
fn cancel_closure_does_not_keep_transport_alive() {
    use super::{AcpHost, AcpTransport};
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    let mut child = Command::new("ping.exe")
        .args(["-n", "2", "127.0.0.1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000)
        .spawn()
        .expect("ping");
    let stdin = child.stdin.take().expect("stdin");
    let cwd = crate::test_dirs::test_temp_dir("devboule-acp-host");
    let host = AcpHost::new(cwd.clone(), cwd);
    let transport = Arc::new(AcpTransport::new(stdin, host));
    transport.bind_turn();
    let weak = Arc::downgrade(&transport);
    drop(transport);
    let leaked = weak.upgrade();
    let _ = child.kill();
    let _ = child.wait();
    assert!(
        leaked.is_none(),
        "TurnWatch cancel closure kept AcpTransport alive after the session dropped it"
    );
}

#[cfg(windows)]
#[test]
fn reader_keeps_dispatching_while_a_host_call_is_blocked() {
    use super::{AcpHost, AcpTransport};
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    let cwd = crate::test_dirs::test_temp_dir("devboule-acp-e-cwd");
    let runtime_dir = crate::test_dirs::test_temp_dir("devboule-acp-e-rt");
    let host = AcpHost::new(cwd.clone(), runtime_dir.clone());
    host.set_session_id("stub-session".to_string());
    let gap = Arc::new(Barrier::new(2));
    host.set_create_gap(Arc::clone(&gap));
    let mut child = Command::new("ping.exe")
        .args(["-n", "30", "127.0.0.1"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(0x0800_0000)
        .spawn()
        .expect("ping");
    let stdin = child.stdin.take().expect("stdin");
    let transport = Arc::new(AcpTransport::new(stdin, Arc::clone(&host)));
    transport.set_session_id("stub-session".to_string());
    let (broker, _) = test_broker();
    let (session_runtime, conn) = attached_runtime("stub-session", Arc::clone(&broker));
    let mut reader = AcpReader::for_test_with_transport(
        Arc::new(Mutex::new(HashSet::new())),
        "stub-session".to_string(),
        Arc::clone(&broker),
        Arc::clone(&host),
        transport,
    );
    let create = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "terminal/create",
        "params": {
            "sessionId": "stub-session",
            "command": "cmd.exe",
            "args": ["/c", "exit"]
        }
    });
    let thought = serde_json::json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": "stub-session",
            "update": {
                "sessionUpdate": "agent_thought_chunk",
                "content": {"type": "text", "text": "still-alive"}
            }
        }
    });
    let mut bytes = serde_json::to_vec(&create).expect("create line");
    bytes.push(b'\n');
    bytes.extend(serde_json::to_vec(&thought).expect("thought line"));
    bytes.push(b'\n');
    let feed_runtime = Arc::clone(&session_runtime);
    let feed_thread = thread::spawn(move || reader.feed(&bytes, &feed_runtime));
    let allow_deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if let Some(id) = broker.pending_ids().into_iter().next() {
            broker
                .respond(&id, PermissionOutcome::AllowOnce)
                .expect("allow blocked terminal/create so it can hit the create gap");
            break;
        }
        if Instant::now() >= allow_deadline {
            panic!("terminal/create never registered a host permission");
        }
        thread::sleep(Duration::from_millis(5));
    }
    let deadline = Instant::now() + Duration::from_millis(500);
    let mut saw_thought = false;
    while Instant::now() < deadline {
        if conn.pull_events().iter().any(|event| {
            matches!(
                &event.envelope.event,
                SessionEvent::AgentThought { text, .. } if text == "still-alive"
            )
        }) {
            saw_thought = true;
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    gap.wait();
    feed_thread.join().expect("feed thread").expect("feed");
    host.shutdown();
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(cwd);
    let _ = std::fs::remove_dir_all(runtime_dir);
    assert!(
        saw_thought,
        "reader stayed blocked on terminal/create and never dispatched the next update"
    );
}

/// Audit S5C-04 (the test the last leg owed) and audit-3 S5D-03: a provider
/// that dies during startup after writing a hundred kilobytes to stderr
/// cannot push that through the tool caller. The banner fits the bound it
/// declares — the `…` that marks the cut is inside it, which the multibyte
/// halves are here to hold it to — and the boundary is still the first thing
/// a human reads.
#[test]
fn a_startup_death_banner_is_bounded() {
    let huge_stderr = "é".repeat(50 * 1024);
    let error = redact_handshake_error(
        WireError::new(
            ErrorCode::Io,
            format!("provider exited during startup: {}", "é".repeat(50 * 1024)),
        ),
        &[huge_stderr],
        None,
    );
    assert!(
        error.message.starts_with("provider exited during startup:"),
        "the boundary is named first: {}",
        error.message.chars().take(80).collect::<String>()
    );
    assert!(
        error.message.ends_with('…'),
        "the cut is marked, so the excerpt really is truncated: {} bytes",
        error.message.len()
    );
    assert!(
        error.message.len() <= MAX_HANDSHAKE_ERROR_BYTES,
        "the banner fits the bound it declares, got {} bytes",
        error.message.len()
    );
}

/// The default ACP launch route (no `DEVBOULE_ACP_COMMAND`, no named id)
/// launches the agent the picker chose with the same spawn PATH the named
/// route applies: a provider found through a registry folder must not launch
/// with an environment that predates its folder (review #2).
#[cfg(windows)]
#[test]
fn the_default_acp_route_carries_the_spawn_path_of_the_picked_agent() {
    use crate::provider_catalog::discover_with_path_source;
    use crate::windows_path_env::WindowsPathSource;
    use crate::windows_registry_path::{RegistryPathError, RegistryPathRead};
    use std::ffi::OsString;
    use std::fs;
    use std::path::PathBuf;
    use std::sync::Mutex;

    struct StalePathSource {
        process: OsString,
        user: Mutex<Option<String>>,
    }

    impl WindowsPathSource for StalePathSource {
        fn process_path(&self) -> Option<OsString> {
            Some(self.process.clone())
        }

        fn machine_path(&self) -> RegistryPathRead {
            RegistryPathRead::Failed(RegistryPathError::Win32(2))
        }

        fn user_path(&self) -> RegistryPathRead {
            match &*self.user.lock().expect("user path lock") {
                Some(value) => RegistryPathRead::Read(value.clone()),
                None => RegistryPathRead::Failed(RegistryPathError::Win32(2)),
            }
        }
    }

    fn grok_install_directory() -> PathBuf {
        let dir = crate::test_dirs::test_temp_dir("acp-default-route-grok");
        fs::create_dir_all(&dir).expect("grok install directory");
        fs::write(dir.join("grok.exe"), b"stub").expect("grok stub executable");
        dir
    }

    let installed = grok_install_directory();
    let source = StalePathSource {
        process: OsString::from("devboule-no-such-inherited-path"),
        user: Mutex::new(Some(installed.to_string_lossy().into_owned())),
    };

    let agent = discover_with_path_source(&source)
        .agents
        .into_iter()
        .find(|agent| agent.id == "grok")
        .expect("grok discovered through the registry PATH");

    let command = super::catalog_acp_command(agent, PathBuf::from(r"C:\workdir"));

    assert_eq!(command.provider_id.as_deref(), Some("grok"));
    assert_eq!(
        command.env,
        vec![(
            "PATH".to_string(),
            format!(
                "devboule-no-such-inherited-path;{}",
                installed.to_string_lossy()
            )
        )],
        "the default ACP launch carries the registry folders on the child PATH"
    );

    fs::remove_dir_all(installed).expect("temporary directory cleanup");
}
