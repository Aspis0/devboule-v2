//! Explicit, read-only login checks for the provider settings refresh.

use crate::provider_catalog::ProviderOrigin;
use crate::provider_catalog::{InstalledAgent, KNOWN_AGENTS};
use std::io::{self, Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
#[cfg(not(test))]
use std::time::{SystemTime, UNIX_EPOCH};

#[cfg(not(test))]
const CHECK_TIMEOUT: Duration = Duration::from_secs(5);
const MAX_STATUS_OUTPUT: usize = 64 * 1024;

/// A child that exited has closed its end of the pipe; the reader only has
/// to drain what is left — a tighter bound can time out a check that exited.
const DRAIN_GRACE: Duration = Duration::from_secs(1);

/// The check's tree-reap bound: a successful wait means the job reported no active members;
/// a failed wait leaves cleanup best effort, a kill-on-close drop only requests termination.
#[cfg(windows)]
const REAP_TIMEOUT: Duration = Duration::from_millis(500);

/// Test-only: the buffer the reader thread captured on the last
/// `run_check_with_timeout` call, so a test can assert what the real reader
/// saw — the capture bound included — instead of calling the pure helper.
#[cfg(test)]
static LAST_CAPTURED_OUTPUT: std::sync::Mutex<Option<Vec<u8>>> = std::sync::Mutex::new(None);

/// Test-only: the PIDs the job held just before the last terminate, so a
/// test can check the whole tree against the processes it actually spawned.
/// `None` when the runner cleared the seam at entry and no terminate
/// recorded anything — the tree assertions must fail, not read the
/// previous run's list. Windows-only like the tree kill it observes.
#[cfg(all(test, windows))]
static LAST_JOB_PIDS: std::sync::Mutex<Option<Vec<u32>>> = std::sync::Mutex::new(None);

/// Whether this provider's classification reads the captured stdout. The
/// drain wait and the capture seam both branch on this one predicate, so a
/// new stdout-parsing provider cannot silently get an empty buffer.
fn reads_stdout(provider_id: &str) -> bool {
    provider_id == "pi"
}

/// Test-only: serialises the runner tests, which share the two seams
/// above; without it, parallel tests overwrite each other's observations.
#[cfg(test)]
pub(crate) fn runner_test_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

#[cfg(test)]
pub(crate) fn last_captured_output() -> Option<Vec<u8>> {
    LAST_CAPTURED_OUTPUT
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
}

#[cfg(all(test, windows))]
pub(crate) fn last_job_pids() -> Vec<u32> {
    LAST_JOB_PIDS
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
        .unwrap_or_default()
}

/// Test-only: records the job's member PIDs before a terminate, so the
/// caller can assert the kill against exactly those processes. A failed
/// record stores `None`, never the previous run's PIDs.
#[cfg(all(test, windows))]
fn record_job_pids(job: &crate::process_tree::JobObject) {
    *LAST_JOB_PIDS
        .lock()
        .unwrap_or_else(|error| error.into_inner()) = job.pids().ok();
}

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
    #[cfg(test)]
    {
        *LAST_CAPTURED_OUTPUT
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = None;
        // Cleared at entry too: a failed record_job_pids must leave the
        // tree assertions with nothing to read, never the previous run's
        // PIDs.
        #[cfg(windows)]
        {
            *LAST_JOB_PIDS
                .lock()
                .unwrap_or_else(|error| error.into_inner()) = None;
        }
    }
    let exit_status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => {
                #[cfg(all(test, windows))]
                record_job_pids(&_job);
                #[cfg(windows)]
                reap_check_job(&agent.id, "timeout", &_job);
                let _ = child.kill();
                let _ = child.wait();
                return classify_failure(ProbeFailure::TimedOut);
            }
        }
    };
    #[cfg(all(test, windows))]
    record_job_pids(&_job);
    #[cfg(windows)]
    reap_check_job(&agent.id, "success", &_job);
    // claude and codex classify by exit code alone; waiting for the pipe on
    // their path would let a descendant that outlives the direct child turn
    // a successful exit into a false timeout. pi parses stdout, so it drains
    // with the grace extended by whatever budget is left.
    let stdout = if reads_stdout(&agent.id) {
        match output_rx
            .recv_timeout(DRAIN_GRACE.max(deadline.saturating_duration_since(Instant::now())))
        {
            Ok(output) => output,
            Err(_) => return classify_failure(ProbeFailure::TimedOut),
        }
    } else {
        Vec::new()
    };
    #[cfg(test)]
    if reads_stdout(&agent.id) {
        *LAST_CAPTURED_OUTPUT
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(stdout.clone());
    }

    classify_result(
        &agent.id,
        exit_status.and_then(|status| status.code()),
        &stdout,
    )
}

/// A failed wait is printed, never returned: cleanup trouble is not evidence
/// about credentials, so it must not change the classification.
#[cfg(windows)]
fn reap_check_job(provider_id: &str, path: &str, job: &crate::process_tree::JobObject) {
    report_failed_reap(
        provider_id,
        path,
        job.terminate_and_wait(REAP_TIMEOUT),
        &mut io::stderr(),
    );
}

/// Writes the failed-wait line naming the provider and the path — never the
/// check's arguments — and ignores the sink's own error: reporting must not
/// become a failure of the auth check.
#[cfg_attr(not(any(windows, test)), allow(dead_code))]
fn report_failed_reap(provider_id: &str, path: &str, wait: io::Result<()>, sink: &mut impl Write) {
    if let Err(error) = wait {
        let _ = writeln!(
            sink,
            "provider auth cleanup for {provider_id} failed on the {path} path: {error}"
        );
    }
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
            // The reader keeps the first 64 KiB; a pi build that logs more
            // than that before its status field is truncated by the cap, and
            // blaming the CLI for an unrecognised result would be false.
            _ if output.len() >= MAX_STATUS_OUTPUT => (
                "unknown",
                "The provider status output was too large to read.",
            ),
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
