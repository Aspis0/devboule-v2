//! Small local ACP peer used by the Windows integration test.
//!
//! It intentionally emits one malformed line, CRLF framing, stderr, a text
//! update, a tool update, and a correlated prompt response. It also exits on
//! stdin EOF so the test covers the daemon's shutdown ownership.

use std::io::{self, BufRead, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::time::Duration;

use serde_json::{json, Value};

/// Verbatim `session/new` response captured from
/// `@agentclientprotocol/claude-agent-acp@0.76.0` over raw stdio
/// (2026-09-09; no prompt sent). Used verbatim by the `--config-options`
/// mode, with only the sessionId overridden.
const CLAUDE_076_SESSION_NEW: &str = include_str!("../../fixtures/acp-claude-076-session-new.json");

fn claude_076_result(session_id: &str) -> Value {
    let mut value: Value =
        serde_json::from_str(CLAUDE_076_SESSION_NEW).expect("claude 0.76 fixture");
    value["result"]["sessionId"] = json!(session_id);
    value["result"].take()
}

fn vendor_models_result() -> Value {
    json!({
        "models": {
            "currentModelId": "opus[1m]",
            "availableModels": [
                {
                    "modelId": "opus[1m]",
                    "name": "Opus 5",
                    "_meta": {
                        "supportsReasoningEffort": true,
                        "reasoningEffort": "xhigh",
                        "reasoningEfforts": [
                            {"id": "low", "label": "Low"},
                            {"id": "high", "label": "High"},
                            {"id": "xhigh", "label": "Extra high", "default": true}
                        ]
                    }
                },
                {
                    "modelId": "haiku",
                    "name": "Haiku",
                    "_meta": {
                        "supportsReasoningEffort": true,
                        "reasoningEfforts": [
                            {"id": "low", "label": "Low"},
                            {"id": "high", "label": "High"}
                        ]
                    }
                }
            ]
        }
    })
}

fn vendor_effort_only_result() -> Value {
    json!({
        "sessionId": "stub-session",
        "models": {
            "currentModelId": "opus[1m]",
            "availableModels": [{
                "modelId": "opus[1m]",
                "name": "Opus 5",
                "_meta": {
                    "supportsReasoningEffort": true,
                    "reasoningEffort": "high",
                    "reasoningEfforts": [
                        {"id": "low", "label": "Low"},
                        {"id": "high", "label": "High"}
                    ]
                }
            }]
        },
        "configOptions": [{
            "id": "thought-level",
            "type": "select",
            "category": "future_thought_selector",
            "currentValue": "high",
            "options": [
                {"value": "low", "name": "Low"},
                {"value": "high", "name": "High"}
            ]
        }]
    })
}

fn categoryless_result() -> Value {
    json!({
        "sessionId": "stub-session",
        "configOptions": [
            {
                "id": "engine",
                "type": "select",
                "currentValue": "m1",
                "options": [
                    {"value": "m1", "name": "Model one"},
                    {"value": "m2", "name": "Model two"}
                ]
            },
            {
                "id": "reasoner",
                "type": "select",
                "category": "future_reasoning_selector",
                "currentValue": "high",
                "options": [
                    {"value": "low", "name": "Low"},
                    {"value": "high", "name": "High"}
                ]
            }
        ]
    })
}

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
    let emit_malformed = !std::env::args().any(|arg| arg == "--no-malformed");
    // Emulate an ACP v1 `configOptions` peer (claude-agent-acp 0.76 shape):
    // session/new answers with the verbatim captured frame; model/effort
    // switches go through session/set_config_option with a plain-string
    // value; session/set_model does not exist (-32601, measured).
    let config_options = std::env::args().any(|arg| arg == "--config-options");
    let hybrid_config_options = std::env::args().any(|arg| arg == "--hybrid-config-options");
    let hybrid_effort_only = std::env::args().any(|arg| arg == "--hybrid-effort-only");
    let categoryless_options = std::env::args().any(|arg| arg == "--categoryless-options");
    let config_mode =
        config_options || hybrid_config_options || hybrid_effort_only || categoryless_options;
    // Audit §1 scenario: after a daemon restart the reattach (session/load)
    // reply carries modes only — no configOptions, no models. The switch
    // shape must be None and a click must fail loudly.
    let load_modes_only = std::env::var_os("DEVBOULE_STUB_LOAD_MODES_ONLY").is_some();
    let load_models_push = std::env::args().any(|arg| arg == "--load-models-push");
    // Audit §6 scenario: a JSON-RPC success with no parseable catalog.
    let malformed_config_reply = std::env::var_os("DEVBOULE_STUB_CONFIG_MALFORMED_REPLY").is_some();
    let hybrid_vendor_mismatch = std::env::var_os("DEVBOULE_STUB_HYBRID_VENDOR_MISMATCH").is_some();
    let mut reject_config_once = std::env::var_os("DEVBOULE_STUB_REJECT_CONFIG_ONCE").is_some();
    // A real agent keeps its config-option state across a session: model
    // switches must not reset the effort option and vice versa. Start from
    // the verbatim captured session/new result and mutate it in place.
    let mut config_state = if config_mode {
        Some(if hybrid_effort_only {
            vendor_effort_only_result()
        } else if categoryless_options {
            categoryless_result()
        } else {
            claude_076_result("stub-session")
        })
    } else {
        None
    };
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
                    if config_mode {
                        let mut result = config_state
                            .clone()
                            .unwrap_or_else(|| claude_076_result("stub-session"));
                        if hybrid_config_options {
                            let vendor = vendor_models_result();
                            if hybrid_vendor_mismatch {
                                result["models"]["availableModels"] =
                                    json!([vendor["models"]["availableModels"][0].clone()]);
                            } else {
                                result["models"] = vendor["models"].clone();
                            }
                        }
                        result
                    } else {
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
                        })
                    },
                )?;
                call_mcp_tools_list_if_configured(&request)?;
                emit_mcp_ready_if_configured(&mut stdout, &request)?;
            }
            "session/load" => {
                if config_mode {
                    let result = config_state
                        .clone()
                        .unwrap_or_else(|| claude_076_result("stub-session"));
                    let payload = if load_modes_only {
                        // The reattach reply carries modes only; the config
                        // surface is absent.
                        json!({
                            "sessionId": "stub-session",
                            "modes": result.get("modes").cloned().unwrap_or(json!({}))
                        })
                    } else {
                        result
                    };
                    respond(&mut stdout, request.get("id").cloned(), payload)?;
                    if load_models_push {
                        let vendor = vendor_models_result();
                        emit(
                            &mut stdout,
                            json!({
                                "jsonrpc": "2.0",
                                "method": "_x.ai/models/update",
                                "params": vendor["models"].clone()
                            }),
                        )?;
                    }
                    continue;
                }
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
                call_mcp_tools_list_if_configured(&request)?;
                emit_mcp_ready_if_configured(&mut stdout, &request)?;
            }
            "session/set_config_option" => {
                let params = request.get("params").cloned().unwrap_or(json!({}));
                let config_id = params
                    .get("configId")
                    .and_then(Value::as_str)
                    .unwrap_or("<none>")
                    .to_string();
                let value = params
                    .get("value")
                    .and_then(Value::as_str)
                    .unwrap_or("<none>")
                    .to_string();
                if let Ok(path) = std::env::var("DEVBOULE_ACP_STUB_SET_CONFIG_FILE") {
                    std::fs::write(path, format!("{config_id}={value}")).ok();
                }
                if !config_mode {
                    respond_error(
                        &mut stdout,
                        request.get("id").cloned(),
                        json!({"code": -32601, "message": "stub does not speak set_config_option"}),
                    )?;
                    continue;
                }
                if malformed_config_reply {
                    // JSON-RPC success with no parseable catalog: the daemon
                    // must use the requested value, never go silent.
                    respond(&mut stdout, request.get("id").cloned(), json!({}))?;
                    continue;
                }
                if reject_config_once {
                    reject_config_once = false;
                    respond_error(
                        &mut stdout,
                        request.get("id").cloned(),
                        json!({"code": -32602, "message": "config option rejected by stub"}),
                    )?;
                    continue;
                }
                // The agent is authoritative: unless the test opts in, the
                // reply echoes the requested value; with
                // DEVBOULE_STUB_CONFIG_WRONG_VALUE the reply reports a
                // DIFFERENT model, and the daemon must display what the
                // agent said, not what we asked for.
                let reported = if config_id == "model"
                    && std::env::var_os("DEVBOULE_STUB_CONFIG_WRONG_VALUE").is_some()
                {
                    "sonnet"
                } else {
                    value.as_str()
                };
                let state = config_state
                    .as_mut()
                    .expect("config_state present in --config-options mode");
                if let Some(options) = state.get_mut("configOptions").and_then(Value::as_array_mut)
                {
                    for option in options.iter_mut() {
                        if option.get("id").and_then(Value::as_str) == Some(config_id.as_str()) {
                            option["currentValue"] = json!(reported);
                        }
                    }
                }
                respond(&mut stdout, request.get("id").cloned(), state.clone())?;
            }
            "session/set_model" => {
                if config_mode && !hybrid_config_options && !hybrid_effort_only && !load_models_push
                {
                    // Measured on claude-agent-acp 0.76.0: this verb does not
                    // exist on configOptions-shaped agents.
                    respond_error(
                        &mut stdout,
                        request.get("id").cloned(),
                        json!({
                            "code": -32601,
                            "message": "Method not found: session/set_model",
                            "data": {"method": "session/set_model"}
                        }),
                    )?;
                    continue;
                }
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
                let reply = if std::env::var_os("DEVBOULE_STUB_SET_MODEL_NO_META").is_some() {
                    json!({})
                } else {
                    json!({"_meta": {"model": {"Ok": model_id}}})
                };
                respond(&mut stdout, request.get("id").cloned(), reply)?;
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
                    if let Some(delay_ms) = std::env::var("DEVBOULE_ACP_STUB_PERMISSION_DELAY_MS")
                        .ok()
                        .and_then(|value| value.parse::<u64>().ok())
                    {
                        std::thread::sleep(Duration::from_millis(delay_ms));
                    }
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
                if emit_malformed {
                    eprintln!("stub-agent stderr marker");
                    stdout.write_all(b"not-json\r\n")?;
                }
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

fn emit_mcp_ready_if_configured(stdout: &mut impl Write, request: &Value) -> io::Result<()> {
    let configured = request
        .pointer("/params/mcpServers")
        .and_then(Value::as_array)
        .is_some_and(|servers| {
            servers
                .iter()
                .any(|server| server.get("name").and_then(Value::as_str) == Some("devboule"))
        });
    if configured {
        emit(
            stdout,
            json!({
                "jsonrpc": "2.0",
                "method": "_x.ai/mcp/server_status",
                "params": {
                    "sessionId": "stub-session",
                    "name": "devboule",
                    "source": "local",
                    "status": "ready",
                    "reason": "initialized",
                    "tools": null
                }
            }),
        )?;
    }
    Ok(())
}

fn call_mcp_tools_list_if_configured(request: &Value) -> io::Result<()> {
    let Some(server) = request
        .pointer("/params/mcpServers")
        .and_then(Value::as_array)
        .and_then(|servers| {
            servers
                .iter()
                .find(|server| server.get("name").and_then(Value::as_str) == Some("devboule"))
        })
    else {
        return Ok(());
    };
    let url = server
        .get("url")
        .and_then(Value::as_str)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "MCP URL is missing"))?;
    let endpoint = url
        .strip_prefix("http://")
        .and_then(|url| url.split('/').next())
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "MCP URL is invalid"))?;
    let path = url
        .strip_prefix("http://")
        .and_then(|url| url.split_once('/').map(|(_, path)| format!("/{path}")))
        .unwrap_or_else(|| "/".to_string());
    let authorization = server
        .pointer("/headers")
        .and_then(Value::as_array)
        .and_then(|headers| {
            headers.iter().find_map(|header| {
                (header.get("name").and_then(Value::as_str) == Some("Authorization"))
                    .then(|| header.get("value").and_then(Value::as_str))
                    .flatten()
            })
        })
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "MCP Bearer is missing"))?;
    let body = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list"}"#;
    let mut stream = TcpStream::connect(endpoint)?;
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nAuthorization: {authorization}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes())?;
    stream.shutdown(Shutdown::Write)?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    if !response.starts_with(b"HTTP/1.1 200") {
        return Err(io::Error::other("MCP tools/list was rejected"));
    }
    Ok(())
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
