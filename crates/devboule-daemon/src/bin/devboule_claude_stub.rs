//! Fake `claude` CLI speaking just enough stream-json for resume tests.
//!
//! It records its argv (proving the daemon passed `--resume <peer>`),
//! answers the initial permission-mode control request the daemon writes
//! first, and echoes every user prompt back as an assistant text plus a
//! result envelope. A `--resume` argv reuses that peer id in its init
//! envelope, the way the real CLI continues its own conversation. It exits
//! on stdin EOF.

use std::io::{self, BufRead, Read, Write};
use std::net::TcpStream;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

fn flag_value(argv: &[String], flag: &str) -> Option<String> {
    argv.windows(2).find_map(|pair| {
        if pair[0] == flag {
            Some(pair[1].clone())
        } else {
            None
        }
    })
}

fn emit(value: &Value) {
    let mut stdout = io::stdout().lock();
    let _ = writeln!(
        stdout,
        "{}",
        serde_json::to_string(value).unwrap_or_default()
    );
    let _ = stdout.flush();
}

/// The slug rule the daemon's pre-resume check implements
/// (`claude_client::claude_projects_slug`): every byte outside
/// `[A-Za-z0-9-]` becomes `-`. The stub writes the file so the daemon's
/// exact-path lookup hits the way a real CLI's conversation file does.
fn projects_slug(cwd: &std::path::Path) -> String {
    cwd.to_string_lossy()
        .chars()
        .map(|cell| {
            if cell.is_ascii_alphanumeric() || cell == '-' {
                cell
            } else {
                '-'
            }
        })
        .collect()
}

/// Mimic the real CLI's on-disk conversation: without it the daemon's
/// resume pre-check would refuse, correctly, that there is nothing to take
/// back.
fn persist_history(peer: &str) {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
    let cwd = std::env::current_dir().unwrap_or_default();
    if let Some(home) = home {
        let dir = std::path::Path::new(&home)
            .join(".claude")
            .join("projects")
            .join(projects_slug(&cwd));
        if std::fs::create_dir_all(&dir).is_ok() {
            let _ = std::fs::write(
                dir.join(format!("{peer}.jsonl")),
                format!(r#"{{"type":"stub-conversation","sessionId":"{peer}"}}"#,),
            );
        }
    }
}

/// One raw HTTP/1.1 POST over loopback, without a client dependency: the
/// broker serves plain HTTP (`127.0.0.1`, path `/mcp`), bearer-authenticated.
/// Returns the response headers plus body on any well-formed reply.
fn http_post(
    url: &str,
    authorization: &str,
    session: Option<&str>,
    body: &str,
) -> Option<(String, String)> {
    let rest = url.strip_prefix("http://")?;
    let (authority, path) = match rest.find('/') {
        Some(at) => (&rest[..at], &rest[at..]),
        None => (rest, "/"),
    };
    let mut stream = TcpStream::connect(authority).ok()?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .ok()?;
    let mut request = format!(
        "POST {path} HTTP/1.1\r\nHost: {authority}\r\nContent-Type: application/json\r\nAccept: application/json\r\nAuthorization: {authorization}\r\nContent-Length: {}\r\nConnection: close\r\n",
        body.len()
    );
    if let Some(session) = session {
        request.push_str(&format!("Mcp-Session-Id: {session}\r\n"));
    }
    request.push_str(&format!("\r\n{body}"));
    stream.write_all(request.as_bytes()).ok()?;
    let mut raw = Vec::new();
    stream.read_to_end(&mut raw).ok()?;
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n")?;
    Some((head.to_string(), body.to_string()))
}

fn header_value(head: &str, name: &str) -> Option<String> {
    head.lines().skip(1).find_map(|line| {
        let (key, value) = line.split_once(':')?;
        key.trim()
            .eq_ignore_ascii_case(name)
            .then(|| value.trim().to_string())
    })
}

/// The handshake the real CLI performs with `--mcp-config`: initialize,
/// initialized, then the `tools/list` that marks the broker ready and opens
/// the daemon's first-prompt gate. Best-effort inside a bounded budget; a
/// failure surfaces later as the gate's own timeout, never as stub output.
fn shake_broker_hand(config_path: &str) {
    let Ok(config) = std::fs::read_to_string(config_path) else {
        return;
    };
    let Ok(config) = serde_json::from_str::<Value>(&config) else {
        return;
    };
    let servers = config.pointer("/mcpServers").and_then(Value::as_object);
    let Some((_, server)) = servers.and_then(|servers| servers.iter().next()) else {
        return;
    };
    let (Some(url), Some(authorization)) = (
        server.get("url").and_then(Value::as_str),
        server
            .pointer("/headers/Authorization")
            .and_then(Value::as_str),
    ) else {
        return;
    };
    let deadline = Instant::now() + Duration::from_secs(25);
    let mut session: Option<String> = None;
    let mut listed = false;
    while Instant::now() < deadline {
        if session.is_none() {
            let body = json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": {"name": "devboule-claude-stub", "version": "1"},
                },
            })
            .to_string();
            if let Some((head, _)) = http_post(url, authorization, None, &body) {
                session = header_value(&head, "mcp-session-id");
                let _ = http_post(
                    url,
                    authorization,
                    session.as_deref(),
                    r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                );
            }
        } else {
            let body = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}).to_string();
            if http_post(url, authorization, session.as_deref(), &body).is_some() {
                listed = true;
                break;
            }
            session = None;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    let _ = listed;
}

fn main() -> io::Result<()> {
    let argv: Vec<String> = std::env::args().collect();
    let argv_file = flag_value(&argv, "--argv-file").unwrap_or_default();
    let console_file = flag_value(&argv, "--console-file").unwrap_or_default();
    let fresh_peer = flag_value(&argv, "--peer-id").unwrap_or_else(|| "stub-peer-1".to_string());
    if !argv_file.is_empty() {
        let _ = std::fs::write(&argv_file, argv.join("\n"));
    }
    // The daemon's resume flag, when present, is the conversation we are:
    // the init envelope below then names the same id the journal row holds.
    let peer = flag_value(&argv, "--resume").unwrap_or(fresh_peer);
    persist_history(&peer);
    if let Some(config) = flag_value(&argv, "--mcp-config") {
        shake_broker_hand(&config);
    }
    emit(&json!({
        "type": "system",
        "subtype": "init",
        "session_id": peer,
        "model": "stub-model",
        "permissionMode": "default",
    }));
    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(frame) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        if frame.get("type").and_then(Value::as_str) == Some("control_request") {
            let request_id = frame
                .get("request_id")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            emit(&json!({
                "type": "control_response",
                "response": {
                    "subtype": "success",
                    "request_id": request_id,
                    "response": {"mode": "bypassPermissions"},
                },
            }));
            continue;
        }
        if frame.get("type").and_then(Value::as_str) != Some("user") {
            continue;
        }
        let text = frame
            .pointer("/message/content/0/text")
            .and_then(Value::as_str)
            .unwrap_or("");
        if !console_file.is_empty() {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&console_file)?;
            writeln!(file, "{text}")?;
        }
        emit(&json!({
            "type": "assistant",
            "message": {
                "id": "stub-message-1",
                "model": "stub-model",
                "role": "assistant",
                "content": [{"type": "text", "text": format!("STUB-ECHO:{text}")}],
            },
        }));
        emit(&json!({
            "type": "result",
            "subtype": "success",
            "stop_reason": "end_turn",
            "model": "stub-model",
        }));
    }
    Ok(())
}
