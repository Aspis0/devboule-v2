//! Small local ACP peer used by the Windows integration test.
//!
//! It intentionally emits one malformed line, CRLF framing, stderr, a text
//! update, a tool update, and a correlated prompt response. It also exits on
//! stdin EOF so the test covers the daemon's shutdown ownership.

use std::io::{self, BufRead, Write};
use std::time::Duration;

use serde_json::{json, Value};

fn main() -> io::Result<()> {
    if std::env::args().any(|arg| arg == "--version") {
        if let Ok(path) = std::env::var("DEVBOULE_ACP_STUB_VERSION_FILE") {
            let _ = std::fs::write(path, "started");
        }
        if let Some(delay_ms) = std::env::var("DEVBOULE_ACP_STUB_VERSION_DELAY_MS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
        {
            std::thread::sleep(Duration::from_millis(delay_ms));
        }
        println!("9.9.9");
        return Ok(());
    }
    write_observation_files();
    eprintln!("stub-agent handshake stderr marker");
    let fail_initialize = std::env::args().any(|arg| arg == "--fail-initialize");
    let echo_user = !std::env::args().any(|arg| arg == "--no-user-echo");
    let stream_first = std::env::args().any(|arg| arg == "--stream-first");
    // Emulate an expired-credentials peer: session/new answers with a
    // JSON-RPC error and the process keeps reading instead of exiting, so
    // the daemon observes a handshake failure against a live process.
    let fail_session_new = std::env::var_os("DEVBOULE_STUB_FAIL_SESSION_NEW").is_some();
    let stdin = io::stdin();
    let mut stdin = stdin.lock();
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    let mut last_prompt_id = None;
    let mut permission_request_id = None;
    let mut line = String::new();
    loop {
        line.clear();
        if stdin.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let Ok(request) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let method = request
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if method.is_empty() && permission_request_id == request.get("id").and_then(Value::as_u64) {
            permission_request_id = None;
            let cancelled = request
                .get("result")
                .and_then(|result| result.get("outcome"))
                .and_then(|outcome| outcome.get("outcome"))
                .and_then(Value::as_str)
                != Some("selected");
            respond(
                &mut stdout,
                last_prompt_id.map(Value::from),
                json!({"stopReason": if cancelled { "cancelled" } else { "end_turn" }}),
            )?;
            continue;
        }
        match method {
            "initialize" => {
                if fail_initialize {
                    eprintln!("stub-agent startup failure stderr marker");
                    return Ok(());
                }
                respond(
                    &mut stdout,
                    request.get("id").cloned(),
                    json!({
                        "protocolVersion": 1,
                        "agentCapabilities": {},
                        "agentInfo": {"name": "devboule-acp-stub", "version": "1"}
                    }),
                )?;
            }
            "session/new" => {
                if fail_session_new {
                    respond_error(
                        &mut stdout,
                        request.get("id").cloned(),
                        json!({
                            "code": -32000,
                            "message": "Authentication required: stub credentials expired",
                            // Mirror real peers (qwen): error objects carry
                            // structured auth payloads the user never needs
                            // in an error banner.
                            "data": {"authMethods": [{"id": "oauth"}]}
                        }),
                    )?;
                    continue;
                }
                emit(
                    &mut stdout,
                    json!({
                        "jsonrpc": "2.0",
                        "method": "session/update",
                        "params": {
                            "sessionId": "stub-session",
                            "update": {
                                "sessionUpdate": "available_commands_update",
                                "availableCommands": [{
                                    "name": "compact",
                                    "description": "Compress conversation history",
                                    "input": {"hint": "optional context"}
                                }]
                            }
                        }
                    }),
                )?;
                respond(
                    &mut stdout,
                    request.get("id").cloned(),
                    json!({
                        "sessionId": "stub-session",
                        "models": {
                            "currentModelId": "stub-model",
                            "availableModels": [{
                                "modelId": "stub-model",
                                "name": "Stub Model",
                                "_meta": {
                                    "supportsReasoningEffort": true,
                                    "reasoningEffort": "high",
                                    "reasoningEfforts": [
                                        {"id": "high", "label": "High"},
                                        {"id": "low", "label": "Low"}
                                    ]
                                }
                            }, {
                                "modelId": "stub-model-new",
                                "name": "stub-model-new",
                                "_meta": if std::env::args().any(|arg| arg == "--no-target-efforts") {
                                    json!({"supportsReasoningEffort": false})
                                } else {
                                    json!({
                                        "supportsReasoningEffort": true,
                                        "reasoningEfforts": [
                                            {"id": "high", "label": "High", "default": true},
                                            {"id": "low", "label": "Low"}
                                        ]
                                    })
                                }
                            }]
                        }
                    }),
                )?;
            }
            "session/load" => {
                for update in [
                    json!({
                        "sessionUpdate": "user_message_chunk",
                        "content": {"type": "text", "text": "replayed user"}
                    }),
                    json!({
                        "sessionUpdate": "agent_thought_chunk",
                        "content": {"type": "text", "text": "replayed thought"}
                    }),
                    json!({
                        "sessionUpdate": "agent_message_chunk",
                        "messageId": "replayed-message",
                        "content": {"type": "text", "text": "replayed answer"}
                    }),
                ] {
                    emit(
                        &mut stdout,
                        json!({
                            "jsonrpc": "2.0",
                            "method": "session/update",
                            "_meta": {"isReplay": true},
                            "params": {
                                "sessionId": "stub-session",
                                "update": update
                            }
                        }),
                    )?;
                }
                respond(
                    &mut stdout,
                    request.get("id").cloned(),
                    json!({
                        "models": {
                            "currentModelId": "stub-model",
                            "availableModels": [{
                                "modelId": "stub-model",
                                "name": "Stub Model",
                                "_meta": {
                                    "supportsReasoningEffort": true,
                                    "reasoningEfforts": [{"id": "high", "label": "High"}]
                                }
                            }]
                        }
                    }),
                )?;
            }
            "session/set_model" => {
                let model_id = request
                    .get("params")
                    .and_then(|params| params.get("modelId"))
                    .and_then(Value::as_str)
                    .unwrap_or("stub-model");
                if let Ok(path) = std::env::var("DEVBOULE_ACP_STUB_SET_MODEL_EFFORT_FILE") {
                    let effort = request
                        .get("params")
                        .and_then(|params| params.get("_meta"))
                        .and_then(|meta| meta.get("reasoningEffort"))
                        .and_then(Value::as_str)
                        .unwrap_or("<none>");
                    std::fs::write(path, effort).ok();
                }
                if let Ok(path) = std::env::var("DEVBOULE_ACP_STUB_SET_MODEL_FILE") {
                    std::fs::write(path, model_id).ok();
                }
                if std::env::var_os("DEVBOULE_STUB_REJECT_SET_MODEL").is_some() {
                    respond_error(
                        &mut stdout,
                        request.get("id").cloned(),
                        json!({"code": -32602, "message": "unknown model"}),
                    )?;
                    continue;
                }
                respond(
                    &mut stdout,
                    request.get("id").cloned(),
                    json!({"_meta": {"model": {"Ok": model_id}}}),
                )?;
                if std::env::var_os("DEVBOULE_STUB_SET_MODEL_SESSIONS_CHANGED").is_some() {
                    emit(
                        &mut stdout,
                        json!({
                            "jsonrpc": "2.0",
                            "method": "_x.ai/sessions/changed",
                            "params": {
                                "upserted": [{
                                    "sessionId": "stub-session",
                                    "modelId": model_id,
                                    "reasoningEffort": "medium"
                                }]
                            }
                        }),
                    )?;
                }
                if std::env::var_os("DEVBOULE_STUB_SET_MODEL_NO_PUSH").is_some() {
                    continue;
                }
                let catalog_effort =
                    if std::env::var_os("DEVBOULE_STUB_SET_MODEL_CATALOG_DEFAULT_PUSH").is_some() {
                        "xhigh"
                    } else {
                        request
                            .get("params")
                            .and_then(|params| params.get("_meta"))
                            .and_then(|meta| meta.get("reasoningEffort"))
                            .and_then(Value::as_str)
                            .unwrap_or("high")
                    };
                emit(
                    &mut stdout,
                    json!({
                        "jsonrpc": "2.0",
                        "method": "_x.ai/models/update",
                        "params": {
                            "currentModelId": model_id,
                            "availableModels": [{
                                "modelId": model_id,
                                "name": model_id,
                                "_meta": {
                                    "supportsReasoningEffort": true,
                                    "reasoningEffort": catalog_effort,
                                    "reasoningEfforts": [
                                        {"id": "high", "label": "High"},
                                        {"id": "low", "label": "Low"}
                                    ]
                                }
                            }]
                        }
                    }),
                )?;
            }
            "session/prompt" => {
                last_prompt_id = request.get("id").and_then(Value::as_u64);
                let prompt_text = request
                    .get("params")
                    .and_then(|params| params.get("prompt"))
                    .and_then(Value::as_array)
                    .and_then(|prompt| prompt.first())
                    .and_then(|prompt| prompt.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                if prompt_text.contains("block") {
                    continue;
                }
                if prompt_text.contains("permission") {
                    permission_request_id = Some(99);
                    emit(
                        &mut stdout,
                        json!({
                            "jsonrpc": "2.0",
                            "id": 99,
                            "method": "session/request_permission",
                            "params": {
                                "sessionId": "stub-session",
                                "title": "Run command",
                                "description": "The stub wants to run a command.",
                                "toolCall": {
                                    "toolCallId": "tool-perm",
                                    "title": "Run command",
                                    "status": "in_progress"
                                },
                                "options": [
                                    {"optionId": "allow", "name": "Allow once", "kind": "allow_once"},
                                    {"optionId": "deny", "name": "Deny", "kind": "reject_once"}
                                ]
                            }
                        }),
                    )?;
                    continue;
                }
                eprintln!("stub-agent stderr marker");
                stdout.write_all(b"not-json\r\n")?;
                if echo_user {
                    emit(
                        &mut stdout,
                        json!({
                            "jsonrpc": "2.0",
                            "method": "session/update",
                            "params": {
                                "sessionId": "stub-session",
                                "update": {
                                    "sessionUpdate": "user_message_chunk",
                                    "content": {"type": "text", "text": prompt_text}
                                }
                            }
                        }),
                    )?;
                }
                emit(
                    &mut stdout,
                    json!({
                        "jsonrpc": "2.0",
                        "method": "session/update",
                        "params": {
                            "sessionId": "stub-session",
                            "update": {
                                "sessionUpdate": "agent_thought_chunk",
                                "content": {"type": "text", "text": "thinking"}
                            }
                        }
                    }),
                )?;
                if stream_first {
                    std::thread::sleep(Duration::from_millis(200));
                }
                emit(
                    &mut stdout,
                    json!({
                        "jsonrpc": "2.0",
                        "method": "session/update",
                        "params": {
                            "sessionId": "stub-session",
                            "update": {
                                "sessionUpdate": "agent_message_chunk",
                                "messageId": "m1",
                                "content": {"type": "text", "text": "stub reply"}
                            }
                        }
                    }),
                )?;
                emit(
                    &mut stdout,
                    json!({
                        "jsonrpc": "2.0",
                        "method": "session/update",
                        "params": {
                            "sessionId": "stub-session",
                            "update": {
                                "sessionUpdate": "tool_call",
                                "toolCallId": "tool-1",
                                "title": "stub tool",
                                "status": "completed"
                            }
                        }
                    }),
                )?;
                respond(
                    &mut stdout,
                    last_prompt_id.map(Value::from),
                    json!({"stopReason": "end_turn"}),
                )?;
            }
            "session/cancel" => {
                if let Some(id) = last_prompt_id {
                    respond(
                        &mut stdout,
                        Some(Value::from(id)),
                        json!({"stopReason": "cancelled"}),
                    )?;
                }
            }
            _ => {}
        }
    }
}

fn respond_error(stdout: &mut impl Write, id: Option<Value>, error: Value) -> io::Result<()> {
    emit(stdout, json!({"jsonrpc": "2.0", "id": id, "error": error}))
}

fn respond(stdout: &mut impl Write, id: Option<Value>, result: Value) -> io::Result<()> {
    emit(
        stdout,
        json!({"jsonrpc": "2.0", "id": id, "result": result}),
    )
}

fn emit(stdout: &mut impl Write, value: Value) -> io::Result<()> {
    serde_json::to_writer(&mut *stdout, &value).map_err(io::Error::other)?;
    stdout.flush()?;
    std::thread::sleep(Duration::from_millis(1));
    stdout.write_all(b"\r\n")?;
    stdout.flush()
}

fn write_observation_files() {
    if let Ok(path) = std::env::var("DEVBOULE_ACP_STUB_PID_FILE") {
        let _ = std::fs::write(path, std::process::id().to_string());
    }
    if let Ok(path) = std::env::var("DEVBOULE_ACP_STUB_CONSOLE_FILE") {
        #[cfg(windows)]
        let no_console =
            unsafe { windows_sys::Win32::System::Console::GetConsoleWindow().is_null() };
        #[cfg(not(windows))]
        let no_console = true;
        let _ = std::fs::write(path, if no_console { "no-console" } else { "console" });
    }
}
