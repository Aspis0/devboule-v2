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

/// The mode block this stub declares when `DEVBOULE_STUB_MODES` is set
/// (`S5` block 2): the standard ACP shape `acp_view::has_standard_modes` reads.
///
/// The value is a comma-separated list of modes, the **first** being the one
/// the session is already in. `ask,default` is what the slice-5 tests use: the
/// preset cells name `default`, the session starts in `ask`, so the daemon has
/// a real switch to send and the test can see the cell's mode arrive. A block
/// that already said `default` would prove nothing — the daemon sends
/// `session/set_mode` only when the current mode is not the requested one.
fn modes_block(spec: &str) -> Value {
    let ids: Vec<&str> = spec
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .collect();
    let current = ids.first().copied().unwrap_or("default");
    let available: Vec<Value> = ids
        .iter()
        .map(|id| {
            json!({
                "id": id,
                "name": id,
                "description": "A mode the stub declares"
            })
        })
        .collect();
    json!({
        "currentModeId": current,
        "availableModes": available
    })
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
    // Slice-5 scenario: the agent declares the modes `DEVBOULE_STUB_MODES`
    // lists (`ask,default` for the tests) and implements `session/set_mode`
    // for them. A daemon only ever *sends* a mode switch to an agent that
    // declared modes at `session/new` (`acp_view::has_standard_modes` gates
    // `AcpSwitcher::set_mode`) *and* whose current mode differs from the one it
    // wants, so both halves matter: without the first the daemon switches the
    // child locally and no test could prove the preset cell's mode reached the
    // provider, and without the second there would be nothing to send.
    let stub_modes: Option<String> = std::env::var("DEVBOULE_STUB_MODES").ok();
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
    // A real agent says what it did with the permission it was granted; the
    // stub's parked run needs that message on the child's transcript, because
    // the finish report deposits the child's last message and there would
    // otherwise be nothing to deposit. Off by default: the tests that only care
    // about the card keep the transcript they had.
    let message_after_permission =
        std::env::var_os("DEVBOULE_STUB_MESSAGE_AFTER_PERMISSION").is_some();
    let mut line = String::new();
    loop {
        line.clear();
        if stdin.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let Ok(request) = serde_json::from_str::<Value>(&line) else {
            // A line that is not a request: the stub ignores it (that is what a
            // real agent does with chatter), but slice-5's tests need to *see*
            // what arrived when something downstream reports malformed output,
            // so the raw line is kept when a file is named.
            if let Ok(file) = std::env::var("DEVBOULE_ACP_STUB_STDIN_FILE") {
                let _ = std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(&file)
                    .and_then(|mut handle| {
                        use std::io::Write;
                        writeln!(handle, "NOT-JSON: {}", line.trim_end())
                    });
            }
            continue;
        };
        if let Ok(file) = std::env::var("DEVBOULE_ACP_STUB_STDIN_FILE") {
            // Every request, so a test can tell "the daemon never sent it" from
            // "the stub never answered it".
            let method = request
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or("<none>");
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&file)
                .and_then(|mut handle| {
                    use std::io::Write;
                    writeln!(handle, "{}: {}", std::process::id(), method)
                });
        }
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
            if message_after_permission && !cancelled {
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
            }
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
                // A provider that is still starting: the request is read and
                // nothing is answered. The daemon's bound is then the only
                // thing that can end the wait, which is what this knob exists
                // to exercise.
                if std::env::var_os("DEVBOULE_STUB_IGNORE_INITIALIZE").is_some() {
                    continue;
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
                let mut new_session_result = if config_mode {
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
                };
                // The R2a delivery scenario: an agent that declares no model
                // catalog at all. The daemon's switch shape is then None, and
                // a profile naming a model must be refused with the absence
                // sentence instead of being sent and hoped for.
                if std::env::var_os("DEVBOULE_STUB_NO_MODELS").is_some() {
                    if let Some(object) = new_session_result.as_object_mut() {
                        object.remove("models");
                    }
                }
                if std::env::var_os("DEVBOULE_STUB_OMIT_MODES").is_some() {
                    // The re-audit's P3-5: the daemon's tick guard has a
                    // sentence for a handshake that declared no modes the
                    // daemon can judge, and reaching it needs an agent that
                    // says nothing about modes at all.
                } else if let Some(modes) = stub_modes.as_deref() {
                    // The stub declares the modes the test asked for (`S5`
                    // block 2's worker cell for this provider), so the daemon's
                    // `has_standard_modes` is true and the child's creation
                    // really sends `session/set_mode` instead of switching
                    // locally. Without the knob the stub keeps the shape every
                    // other stub test measured: no `modes`, no remote switch.
                    let block = modes_block(modes);
                    if let Ok(file) = std::env::var("DEVBOULE_ACP_STUB_MODES_FILE") {
                        // What this process told the daemon, kept so a test can
                        // see the setup its assertion depends on rather than
                        // inferring it from an empty switch file.
                        let _ = std::fs::write(file, block.to_string());
                    }
                    new_session_result["modes"] = block;
                }
                respond(&mut stdout, request.get("id").cloned(), new_session_result)?;
                if stub_is_the_one_to_exit_now() {
                    // The child is gone the instant it exists: the daemon sees
                    // EOF on a session whose creation has just been committed.
                    return Ok(());
                }
                probe_mcp_tools_on_own_thread(&request);
                emit_mcp_ready_if_configured(&mut stdout, &request)?;
            }
            "session/load" => {
                // A slow load widens the spawn window deterministically: a
                // second client's attach can land inside it.
                if let Ok(delay) = std::env::var("DEVBOULE_STUB_DELAY_LOAD_MS") {
                    if let Ok(delay) = delay.parse::<u64>() {
                        std::thread::sleep(std::time::Duration::from_millis(delay));
                    }
                }
                // The once-road: refuse the FIRST load and honour the
                // second. The marker file is the memory, because every
                // resume spawns a fresh stub process.
                if let Ok(marker) = std::env::var("DEVBOULE_STUB_REFUSE_LOAD_ONCE") {
                    if !std::path::Path::new(&marker).exists() {
                        let _ = std::fs::write(&marker, "refused");
                        let asked = request
                            .pointer("/params/sessionId")
                            .and_then(Value::as_str)
                            .unwrap_or("stub-session")
                            .to_string();
                        respond_error(
                            &mut stdout,
                            request.get("id").cloned(),
                            json!({
                                "code": -32002,
                                "message": format!("Resource not found: {asked}")
                            }),
                        )?;
                        continue;
                    }
                }
                // The field's own answer: the ResourceNotFound code with the
                // message naming the session that was asked for. The name is
                // the evidence a conforming agent gives that the missing
                // resource is the session itself.
                if std::env::var_os("DEVBOULE_STUB_REFUSE_LOAD").is_some() {
                    let asked = request
                        .pointer("/params/sessionId")
                        .and_then(Value::as_str)
                        .unwrap_or("stub-session")
                        .to_string();
                    respond_error(
                        &mut stdout,
                        request.get("id").cloned(),
                        json!({"code": -32002, "message": format!("Resource not found: {asked}")}),
                    )?;
                    continue;
                }
                // The same code naming a DIFFERENT resource (a workspace
                // directory that is gone) while echoing the request back —
                // session id included, as agents that log their input do.
                // The schema's generic resource miss names a file, not the
                // session: the far session may be perfectly alive.
                if std::env::var_os("DEVBOULE_STUB_REFUSE_LOAD_OTHER").is_some() {
                    let asked = request
                        .pointer("/params/sessionId")
                        .and_then(Value::as_str)
                        .unwrap_or("stub-session")
                        .to_string();
                    respond_error(
                        &mut stdout,
                        request.get("id").cloned(),
                        json!({
                            "code": -32002,
                            "message": format!(
                                "Resource not found: file:///gone/workspace (requested sessionId: {asked})"
                            ),
                            "data": {
                                "uri": "file:///gone/workspace",
                                "request": {"sessionId": asked}
                            }
                        }),
                    )?;
                    continue;
                }
                // Leave without answering, and with a code: the daemon's load
                // read hits EOF on a child that is already gone, which is the
                // one case the exit code has to explain.
                if std::env::var_os("DEVBOULE_STUB_DIE_ON_LOAD").is_some() {
                    eprintln!("stub-agent died on session/load: stderr marker");
                    std::process::exit(1);
                }
                // Any error the test needs, verbatim. Real peers refuse a
                // session whose working directory is gone with InvalidParams
                // and the path in the message, and the daemon must repeat that
                // message instead of inventing a deadline for it.
                if let Ok(message) = std::env::var("DEVBOULE_STUB_ERROR_LOAD_MESSAGE") {
                    let code = std::env::var("DEVBOULE_STUB_ERROR_LOAD_CODE")
                        .ok()
                        .and_then(|value| value.parse::<i64>().ok())
                        .unwrap_or(-32602);
                    respond_error(
                        &mut stdout,
                        request.get("id").cloned(),
                        json!({"code": code, "message": message}),
                    )?;
                    continue;
                }
                // Leave without answering: the daemon's load read hits EOF, a
                // transport failure that says nothing about the far session.
                if std::env::var_os("DEVBOULE_STUB_EXIT_ON_LOAD").is_some() {
                    return Ok(());
                }
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
                probe_mcp_tools_on_own_thread(&request);
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
            "session/set_mode" => {
                // The mode switch a slice-5 child's preset cell needs. The
                // daemon only sends this at all when the provider declared
                // modes at `session/new` (`has_standard_modes` => remote modes),
                // which the `DEVBOULE_STUB_MODES_DEFAULT` knob below makes the
                // stub do. `default` is accepted; anything else is refused the
                // way a real agent refuses a mode it does not have, so a test
                // can prove the *cell's* mode id is the one that arrived.
                let mode_id = request
                    .get("params")
                    .and_then(|params| params.get("modeId"))
                    .and_then(Value::as_str)
                    .unwrap_or("<missing>");
                if let Ok(path) = std::env::var("DEVBOULE_ACP_STUB_SET_MODE_FILE") {
                    std::fs::write(path, mode_id).ok();
                }
                let declared = stub_modes
                    .as_deref()
                    .map(|modes| modes.split(',').map(str::trim).any(|mode| mode == mode_id))
                    .unwrap_or(false);
                if declared {
                    respond(&mut stdout, request.get("id").cloned(), json!({}))?;
                } else {
                    respond_error(
                        &mut stdout,
                        request.get("id").cloned(),
                        json!({
                            "code": -32602,
                            "message": "Mode not available: session/set_mode",
                            "data": {"modeId": mode_id}
                        }),
                    )?;
                }
            }
            "session/set_model" => {
                if std::env::var_os("DEVBOULE_STUB_DRIBBLE_SET_MODEL").is_some() {
                    // The re-audit's P2-3 agent: bytes keep arriving forever
                    // and none of them is a newline, so the pipe is never
                    // quiet. A read bound consulted only when the pipe is
                    // empty never fires; one consulted on every iteration
                    // does. Runs until the refusal teardown kills the child.
                    loop {
                        let _ = stdout.write_all(b" ");
                        let _ = stdout.flush();
                        std::thread::sleep(Duration::from_millis(5));
                    }
                }
                if std::env::var_os("DEVBOULE_STUB_IGNORE_SET_MODEL").is_some() {
                    // The mute agent the re-audit's P2-2 convicts with: the
                    // switch is taken off the wire and nothing ever comes
                    // back. Only a deadline on the daemon's read turns this
                    // into a refusal instead of an eternal wait.
                    continue;
                }
                if std::env::var_os("DEVBOULE_STUB_DIE_BEFORE_SET_MODEL_REPLY").is_some() {
                    // Dies with the switch on the wire and no answer written:
                    // the creation-time confirm reads an EOF, and the
                    // teardown names what died and why from the stderr tail.
                    eprintln!("stub: dying before the set_model reply, as asked");
                    return Ok(());
                }
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
                if std::env::var_os("DEVBOULE_ACP_STUB_EXIT_ON_PROMPT").is_some() {
                    // The child dies the instant its first prompt arrives
                    // (audit-2 §1, case b): it lived long enough for the
                    // creation to answer Ok, so this exit is a *child end*.
                    return Ok(());
                }
                last_prompt_id = request.get("id").and_then(Value::as_u64);
                let prompt_text = request
                    .get("params")
                    .and_then(|params| params.get("prompt"))
                    .and_then(Value::as_array)
                    .and_then(|prompt| prompt.first())
                    .and_then(|prompt| prompt.get("text"))
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                // The verbatim prompt, when a test asks for it. The stdin file
                // records method names only, which cannot tell *what* the
                // daemon wrote; a test that asserts on the prompt's text needs
                // the agent's own copy of it, not the daemon's transcript.
                if let Ok(file) = std::env::var("DEVBOULE_ACP_STUB_PROMPT_FILE") {
                    let _ = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&file)
                        .and_then(|mut handle| {
                            use std::io::Write;
                            writeln!(handle, "{prompt_text}")
                        });
                }
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
    // Serialized to a string first so the same bytes can be kept for a test
    // (`S5` e2e): when the daemon reports malformed output or a missing answer,
    // the question is always *what did the provider actually write*, and this
    // is the only place that knows.
    let text = serde_json::to_string(&value).map_err(io::Error::other)?;
    if let Ok(file) = std::env::var("DEVBOULE_ACP_STUB_STDOUT_FILE") {
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&file)
            .and_then(|mut handle| {
                use std::io::Write;
                writeln!(handle, "{}: {}", std::process::id(), text)
            });
    }
    stdout.write_all(text.as_bytes())?;
    stdout.flush()?;
    std::thread::sleep(Duration::from_millis(1));
    stdout.write_all(b"\r\n")?;
    stdout.flush()
}

/// Whether *this* stub is the one the exit knob names.
///
/// The knob's value is the **1-based line of the pids file** whose stub exits
/// the moment its handshake is answered (`...=2` is the first child a test's
/// creator spawns). Naming the line rather than "every stub" is what keeps the
/// creator — and the second creator a test needs to read the daemon-wide caps —
/// alive while the child under measurement disappears at once.
fn stub_is_the_one_to_exit_now() -> bool {
    let Some(wanted) = std::env::var("DEVBOULE_ACP_STUB_EXIT_AFTER_SESSION_NEW").ok() else {
        return false;
    };
    let Some(file) = std::env::var("DEVBOULE_ACP_STUB_PIDS_FILE").ok() else {
        return false;
    };
    let Ok(contents) = std::fs::read_to_string(&file) else {
        return false;
    };
    let mine = std::process::id().to_string();
    let position = contents
        .lines()
        .position(|line| line.trim() == mine)
        .map(|index| index + 1);
    matches!((wanted.parse::<usize>(), position), (Ok(wanted), Some(mine)) if wanted == mine)
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

/// Run the MCP probe off the stub's message loop.
///
/// A real agent probes from its own connection work, not from inside the
/// handshake it is still finishing; an inline probe would hold this loop while
/// the daemon waits for answers to the handshake's later steps.
fn probe_mcp_tools_on_own_thread(request: &Value) {
    let request = request.clone();
    let _ = std::thread::Builder::new()
        .name("mcp-probe".into())
        .spawn(move || {
            if let Err(error) = call_mcp_tools_list_if_configured(&request) {
                eprintln!("mcp probe failed: {error}");
            }
        });
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
    // A real agent connects to the broker *after* its handshake completes; the
    // stub used to call in the middle of `session/new`, which is before the
    // daemon has the session in its registry, so the call was refused and the
    // session's readiness was never proved. The delay (0 by default, so every
    // other stub test keeps its timing) lets a test reproduce the real order.
    let mcp_delay_ms = std::env::var("DEVBOULE_ACP_STUB_MCP_DELAY_MS")
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(0);
    if mcp_delay_ms > 0 {
        std::thread::sleep(Duration::from_millis(mcp_delay_ms));
    }
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
    let response = mcp_post(endpoint, &path, authorization, body)?;
    if !response.starts_with(b"HTTP/1.1 200") {
        return Err(io::Error::other("MCP tools/list was rejected"));
    }
    // What the daemon answered this session, appended for a test to read (`S5`
    // block 6: the overlay is proved at the *child's own* `tools/list`).
    //
    // Appended, not overwritten: one test drives both a creator and the child
    // it creates, and this file is how the two answers are told apart — the
    // creator's entry first, the child's after it. Each line names the process
    // and a fingerprint of the Bearer that asked, so two lines that differ are
    // two connections and the token itself is never written anywhere.
    if let Ok(file) = std::env::var("DEVBOULE_ACP_STUB_MCP_TOOLS_FILE") {
        append_observation(&file, authorization, &mcp_body(&response));
    }
    // And one `tools/call`, when a test names a tool: the other half of the
    // same rule, where a hidden tool is refused rather than omitted.
    if let Ok(tool) = std::env::var("DEVBOULE_ACP_STUB_MCP_CALL") {
        let arguments = std::env::var("DEVBOULE_ACP_STUB_MCP_CALL_ARGUMENTS")
            .unwrap_or_else(|_| "{}".to_string());
        let call = json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": tool, "arguments": serde_json::from_str::<Value>(&arguments)
                .unwrap_or_else(|_| json!({}))}
        })
        .to_string();
        // The probe can land while this session's own startup is still being
        // committed, and the answer is then "No session with that id." for a
        // session that plainly exists a moment later. Retry briefly on that
        // one sentence, so the probe measures the broker rather than the spawn
        // race, and stop on any other answer.
        let mut response = mcp_post(endpoint, &path, authorization, &call)?;
        let mut retries = 0u32;
        while mcp_body(&response).contains("No session with that id.") && retries < 40 {
            retries += 1;
            std::thread::sleep(Duration::from_millis(250));
            response = mcp_post(endpoint, &path, authorization, &call)?;
        }
        if let Ok(file) = std::env::var("DEVBOULE_ACP_STUB_MCP_CALL_FILE") {
            append_observation(&file, authorization, &mcp_body(&response));
        }
    }
    Ok(())
}

/// One HTTP/1.1 POST to the daemon's MCP endpoint, answering the whole
/// response as bytes (headers included).
fn mcp_post(endpoint: &str, path: &str, authorization: &str, body: &str) -> io::Result<Vec<u8>> {
    let mut stream = TcpStream::connect(endpoint)?;
    let request = format!(
        "POST {path} HTTP/1.1\r\nHost: localhost\r\nContent-Type: application/json\r\nAuthorization: {authorization}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream.write_all(request.as_bytes())?;
    stream.shutdown(Shutdown::Write)?;
    let mut response = Vec::new();
    stream.read_to_end(&mut response)?;
    Ok(response)
}

/// Append one MCP observation as `<pid> <bearer fingerprint> <body>`.
///
/// Appended rather than rewritten, and the Bearer is fingerprinted rather than
/// stored: a test needs to tell two connections apart, not to read a credential
/// it is not entitled to. Newlines inside the body become spaces so one
/// observation is one line.
fn append_observation(file: &str, authorization: &str, body: &str) {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    authorization.hash(&mut hasher);
    let line = format!(
        "{} {:016x} {}\n",
        std::process::id(),
        hasher.finish(),
        body.replace('\n', " ")
    );
    let _ = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(file)
        .and_then(|mut handle| std::io::Write::write_all(&mut handle, line.as_bytes()));
}

/// The body of an HTTP response: everything after the blank line that ends the
/// headers, as text.
fn mcp_body(response: &[u8]) -> String {
    let text = String::from_utf8_lossy(response).to_string();
    match text.split_once("\r\n\r\n") {
        Some((_, body)) => body.to_string(),
        None => text,
    }
}

fn write_observation_files() {
    // What this process was launched with, for a test that has to tell one
    // launch from another (the args are the provider's, never a secret).
    if let Ok(path) = std::env::var("DEVBOULE_ACP_STUB_ARGV_FILE") {
        let args = std::env::args().collect::<Vec<_>>().join(" ");
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .and_then(|mut handle| {
                use std::io::Write;
                writeln!(handle, "{} {}", std::process::id(), args)
            });
    }
    if let Ok(path) = std::env::var("DEVBOULE_ACP_STUB_PID_FILE") {
        let _ = std::fs::write(path, std::process::id().to_string());
    }
    // Every stub process that starts, in order (`S5` e2e): one test spawns a
    // creator *and* the child it creates, and killing the child is how the
    // reader's EOF path is reached on purpose. The single-pid file above cannot
    // say which of the two is which; this one can, because it keeps both.
    if let Ok(path) = std::env::var("DEVBOULE_ACP_STUB_PIDS_FILE") {
        let _ = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .and_then(|mut handle| {
                std::io::Write::write_all(
                    &mut handle,
                    format!("{}\n", std::process::id()).as_bytes(),
                )
            });
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
