//! A short-lived Codex app-server that answers the pre-agent model catalog:
//! the `model/list` pages and `config/read`. No session starts in it.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use super::codex_client::initialize_params;

/// The whole probe answers within this, or it answers nothing.
const PROBE_BUDGET: Duration = Duration::from_secs(15);
const MODEL_PAGE_LIMIT: u64 = 100;
const MAX_MODEL_PAGES: usize = 20;

/// The listing rows and the config a probe read.
#[derive(Clone)]
pub(crate) struct CodexCatalogRead {
    pub(crate) rows: Vec<Value>,
    pub(crate) config: Value,
}

/// Starts the installed Codex with its app-server argv and the same PATH a
/// session gets, reads the catalog, and stops it. `None` when Codex is not
/// installed or does not answer within the budget.
pub(crate) fn read_catalog() -> Option<CodexCatalogRead> {
    let installed = crate::provider_catalog::find_available("codex")?;
    let mut argv = installed.app_server_command?;
    if argv.is_empty() {
        return None;
    }
    let program = argv.remove(0);
    let mut command = Command::new(program);
    command
        .args(argv)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some((key, value)) = installed.spawn_path_env {
        command.env(key, value);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }
    let mut child = command.spawn().ok()?;
    let answer = converse(&mut child);
    let _ = child.kill();
    let _ = child.wait();
    answer
}

fn converse(child: &mut Child) -> Option<CodexCatalogRead> {
    let stdin = child.stdin.take()?;
    let stdout = child.stdout.take()?;
    let mut conversation = Conversation {
        stdin,
        frames: frames_of(stdout),
        deadline: Instant::now() + PROBE_BUDGET,
        next_id: 0,
    };
    conversation.request("initialize", initialize_params())?;
    conversation.notify("initialized");
    let mut rows = Vec::new();
    let mut cursor = Value::Null;
    for _ in 0..MAX_MODEL_PAGES {
        let page = conversation.request(
            "model/list",
            json!({ "limit": MODEL_PAGE_LIMIT, "includeHidden": false, "cursor": cursor }),
        )?;
        if let Some(data) = page.get("data").and_then(Value::as_array) {
            rows.extend(data.iter().cloned());
        }
        match page.get("nextCursor").and_then(Value::as_str) {
            Some(next) if !next.is_empty() => cursor = Value::String(next.to_string()),
            _ => break,
        }
    }
    // A Codex without `config/read` still lists its models; the draft then has no configured default.
    let config = conversation
        .request("config/read", json!({}))
        .unwrap_or(Value::Null);
    Some(CodexCatalogRead { rows, config })
}

/// The JSON frames a child writes to its stdout, one per line, read off a
/// thread so the probe's own deadline can stop waiting on them.
fn frames_of(stdout: std::process::ChildStdout) -> Receiver<Value> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Ok(frame) = serde_json::from_str::<Value>(&line) {
                if sender.send(frame).is_err() {
                    break;
                }
            }
        }
    });
    receiver
}

struct Conversation {
    stdin: ChildStdin,
    frames: Receiver<Value>,
    deadline: Instant,
    next_id: u64,
}

impl Conversation {
    /// The `result` of the answer to `method`, or `None` on an error answer,
    /// a silent child, or the budget running out.
    fn request(&mut self, method: &str, params: Value) -> Option<Value> {
        self.next_id += 1;
        let id = self.next_id;
        self.write(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }))?;
        loop {
            let remaining = self.deadline.checked_duration_since(Instant::now())?;
            let frame = self.frames.recv_timeout(remaining).ok()?;
            if frame.get("id") == Some(&json!(id)) && frame.get("method").is_none() {
                return frame.get("result").cloned();
            }
        }
    }

    fn notify(&mut self, method: &str) {
        let _ = self.write(&json!({ "jsonrpc": "2.0", "method": method, "params": {} }));
    }

    fn write(&mut self, frame: &Value) -> Option<()> {
        writeln!(self.stdin, "{frame}").ok()?;
        self.stdin.flush().ok()
    }
}

/// The installed Codex's own version, read from `--version` once per daemon.
pub(crate) fn cli_version() -> Option<[u64; 3]> {
    static VERSION: OnceLock<Option<[u64; 3]>> = OnceLock::new();
    *VERSION.get_or_init(|| {
        let installed = crate::provider_catalog::find_available("codex")?;
        // The app-server flag is the last element of the argv; `--version` takes its place.
        let mut argv = installed.app_server_command?;
        argv.pop()?;
        argv.push("--version".to_string());
        let program = argv.remove(0);
        let mut command = Command::new(program);
        command
            .args(argv)
            .stdin(Stdio::null())
            .stderr(Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let output = command.output().ok()?;
        parse_version(&String::from_utf8_lossy(&output.stdout))
    })
}

/// The first `major.minor.patch` in a `--version` line, such as `codex-cli 0.162.0`.
pub(crate) fn parse_version(text: &str) -> Option<[u64; 3]> {
    text.split_whitespace().find_map(|token| {
        let mut parts = token.split('.').map(|part| part.parse::<u64>().ok());
        Some([parts.next()??, parts.next()??, parts.next()??])
    })
}

#[cfg(test)]
mod tests {
    use super::parse_version;

    #[test]
    fn the_version_is_read_from_the_version_line() {
        assert_eq!(parse_version("codex-cli 0.162.0\n"), Some([0, 162, 0]));
        assert_eq!(parse_version("0.115.0"), Some([0, 115, 0]));
        assert_eq!(parse_version("codex-cli unknown"), None);
        assert_eq!(parse_version(""), None);
    }
}
