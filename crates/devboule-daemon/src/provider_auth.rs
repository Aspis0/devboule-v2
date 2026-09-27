//! Explicit, read-only login checks for the provider settings refresh.

use crate::provider_catalog::ProviderOrigin;
use crate::provider_catalog::{InstalledAgent, KNOWN_AGENTS};
use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
#[cfg(not(test))]
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(not(test))]
const CHECK_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_STATUS_OUTPUT: usize = 64 * 1024;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AuthCheck {
    pub status: &'static str,
    pub reason: &'static str,
    pub checked_at: i64,
}

#[derive(Clone, Copy)]
enum ProbeFailure {
    SpawnFailed,
    TimedOut,
}

fn status_args(provider_id: &str) -> Option<Vec<String>> {
    KNOWN_AGENTS
        .iter()
        .find(|agent| agent.id == provider_id)
        .and_then(|agent| agent.auth_check_args)
        .map(|args| args.iter().map(|arg| (*arg).to_string()).collect())
}

fn pi_status_args(
    default_provider: Option<&str>,
    default_model: Option<&str>,
) -> Option<Vec<String>> {
    let provider = default_provider?.trim();
    if provider.is_empty() {
        return None;
    }
    let mut args = vec![
        "auth".to_string(),
        "check".to_string(),
        "--provider".to_string(),
        provider.to_string(),
    ];
    if let Some(model) = default_model
        .map(str::trim)
        .filter(|model| !model.is_empty())
    {
        args.extend(["--model".to_string(), model.to_string()]);
    }
    args.extend(["--no-refresh".to_string(), "--json".to_string()]);
    Some(args)
}

#[cfg(not(test))]
fn pi_default_args() -> Option<Vec<String>> {
    let settings_path = if let Some(agent_dir) = std::env::var_os("PI_CODING_AGENT_DIR") {
        let mut path = std::path::PathBuf::from(agent_dir);
        if path.starts_with("~") {
            path = std::env::var_os("USERPROFILE")
                .or_else(|| std::env::var_os("HOME"))
                .map(std::path::PathBuf::from)?
                .join(path.strip_prefix("~").ok()?);
        }
        path.join("settings.json")
    } else {
        std::env::var_os("USERPROFILE")
            .or_else(|| std::env::var_os("HOME"))
            .map(std::path::PathBuf::from)?
            .join(".pi")
            .join("agent")
            .join("settings.json")
    };
    let settings = std::fs::read(settings_path).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&settings).ok()?;
    let provider = value.get("defaultProvider")?.as_str()?;
    let model = value
        .get("defaultModel")
        .and_then(serde_json::Value::as_str);
    pi_status_args(Some(provider), model)
}

pub(crate) fn check_if_enabled<T>(enabled: bool, probe: impl FnOnce() -> Option<T>) -> Option<T> {
    if enabled {
        probe()
    } else {
        None
    }
}

#[cfg(test)]
pub(crate) fn check(agent: &InstalledAgent) -> Option<AuthCheck> {
    // Never launch host provider CLIs or inspect its credential store in tests.
    if agent.id == "claude" {
        TEST_CLAUDE_CHECK_CALLS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    None
}

#[cfg(test)]
static TEST_CLAUDE_CHECK_CALLS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
pub(crate) fn test_check_call_count() -> usize {
    TEST_CLAUDE_CHECK_CALLS.load(std::sync::atomic::Ordering::SeqCst)
}

#[cfg(not(test))]
pub(crate) fn check(agent: &InstalledAgent) -> Option<AuthCheck> {
    if !agent.installed || agent.origin == ProviderOrigin::NpxWrapper {
        return None;
    }
    let result = match status_args(&agent.id) {
        Some(_) if agent.id == "pi" => match pi_default_args() {
            Some(args) => run_check(agent, &args),
            None => (
                "unknown",
                "Pi's configured default provider could not be determined.",
            ),
        },
        Some(args) => run_check(agent, &args),
        None if agent.id == "grok" => grok_credentials(),
        None => return None,
    };
    Some(AuthCheck {
        status: result.0,
        reason: result.1,
        checked_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis()
            .min(i64::MAX as u128) as i64,
    })
}

#[cfg(not(test))]
fn run_check(agent: &InstalledAgent, args: &[String]) -> (&'static str, &'static str) {
    run_check_with_timeout(agent, args, CHECK_TIMEOUT)
}

fn run_check_with_timeout(
    agent: &InstalledAgent,
    args: &[String],
    timeout: Duration,
) -> (&'static str, &'static str) {
    let mut command = Command::new(&agent.executable);
    command
        .args(&agent.prefix_args)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    if let Some((key, value)) = &agent.spawn_path_env {
        command.env(key, value);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000);
    }

    let _job = match crate::process_tree::JobObject::new() {
        Ok(job) => job,
        Err(_) => return classify_failure(ProbeFailure::SpawnFailed),
    };
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => return classify_failure(ProbeFailure::SpawnFailed),
    };
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        if _job.assign(child.as_raw_handle()).is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return classify_failure(ProbeFailure::SpawnFailed);
        }
    }
    let Some(mut stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return (
            "unknown",
            "The provider status check could not read its result.",
        );
    };
    let (output_tx, output_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut output = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            match stdout.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => append_bounded_output(&mut output, &chunk[..read]),
            }
        }
        let _ = output_tx.send(output);
    });
    let deadline = Instant::now() + timeout;
    let exit_status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                #[cfg(windows)]
                let _ = _job.terminate_and_wait(Duration::from_millis(250));
                let _ = child.kill();
                let _ = child.wait();
                return classify_failure(ProbeFailure::TimedOut);
            }
        }
    };
    #[cfg(windows)]
    let _ = _job.terminate();
    let stdout = match output_rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        Ok(output) => output,
        Err(_) => return classify_failure(ProbeFailure::TimedOut),
    };

    classify_result(
        &agent.id,
        exit_status.and_then(|status| status.code()),
        &stdout,
    )
}

fn classify_failure(failure: ProbeFailure) -> (&'static str, &'static str) {
    match failure {
        ProbeFailure::SpawnFailed => ("unknown", "The provider status check could not start."),
        ProbeFailure::TimedOut => ("unknown", "The provider status check timed out."),
    }
}

fn append_bounded_output(output: &mut Vec<u8>, chunk: &[u8]) {
    let keep = chunk
        .len()
        .min(MAX_STATUS_OUTPUT.saturating_sub(output.len()));
    output.extend_from_slice(&chunk[..keep]);
}

fn classify_result(
    provider_id: &str,
    exit_code: Option<i32>,
    output: &[u8],
) -> (&'static str, &'static str) {
    match provider_id {
        "claude" => match exit_code {
            Some(0) => ("logged_in", "CLI confirmed an active login."),
            Some(1) => ("logged_out", "CLI reported no active login."),
            _ => ("unknown", "CLI returned an unrecognized status result."),
        },
        "codex" => match exit_code {
            Some(0) => ("logged_in", "CLI confirmed an active login."),
            _ => ("unknown", "CLI returned an unrecognized status result."),
        },
        "pi" => match serde_json::from_slice::<serde_json::Value>(output)
            .ok()
            .and_then(|value| value.get("status")?.as_str().map(str::to_owned))
            .as_deref()
        {
            Some("ready") => ("logged_in", "CLI confirmed an active login."),
            Some("not_ready" | "invalid") => ("logged_out", "CLI reported no active login."),
            _ => ("unknown", "CLI returned an unrecognized status result."),
        },
        _ => (
            "unknown",
            "No status check is configured for this provider.",
        ),
    }
}

#[cfg(not(test))]
fn grok_credentials() -> (&'static str, &'static str) {
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"));
    let Some(home) = home else {
        return ("unknown", "The credential file location is unavailable.");
    };
    match std::path::Path::new(&home)
        .join(".grok")
        .join("auth.json")
        .try_exists()
    {
        Ok(true) => (
            "credentials_found",
            "Credential file found; this does not verify the login.",
        ),
        Ok(false) => ("logged_out", "No credential file was found."),
        Err(_) => ("unknown", "The credential file could not be checked."),
    }
}

#[cfg(test)]
#[path = "provider_auth_tests.rs"]
mod tests;
