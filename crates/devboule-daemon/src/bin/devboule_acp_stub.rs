//! Small local ACP peer used by the Windows integration test.
//!
//! It intentionally emits one malformed line, CRLF framing, stderr, a text
//! update, a tool update, and a correlated prompt response. It also exits on
//! stdin EOF so the test covers the daemon's shutdown ownership.

use std::io::{self, BufRead, Write};
use std::time::Duration;

use serde_json::{json, Value};

fn main() -> io::Result<()> {
    write_observation_files();
    eprintln!("stub-agent handshake stderr marker");
    let fail_initialize = std::env::args().any(|arg| arg == "--fail-initialize");
    let stdin = io::stdin();
    let mut stdin = stdin.lock();
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    let mut last_prompt_id = None;
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
            "session/new" => respond(
                &mut stdout,
                request.get("id").cloned(),
                json!({"sessionId": "stub-session"}),
            )?,
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
                eprintln!("stub-agent stderr marker");
                stdout.write_all(b"not-json\r\n")?;
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
