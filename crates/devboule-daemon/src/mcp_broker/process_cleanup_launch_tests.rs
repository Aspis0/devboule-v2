//! The cleanup on the direct launch shape, with real processes: the provider is
//! the daemon's own child with no shim above it, and the helper it starts is a
//! descendant inside the session's job. A cleanup must leave the helper alone.

use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use super::process_cleanup_mode_tests::{
    audit_outcome, join_session, live_session, run_cleanup, terminated_pids,
};
use crate::server::ServerState;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn system_root() -> PathBuf {
    PathBuf::from(std::env::var_os("SystemRoot").expect("%SystemRoot% is set"))
}

fn system_tool(name: &str) -> PathBuf {
    system_root().join("System32").join(name)
}

/// Whether the pid names a live process, read from the OS task list now.
fn is_running(pid: u32) -> bool {
    let output = Command::new(system_tool("tasklist.exe"))
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .expect("tasklist runs");
    String::from_utf8_lossy(&output.stdout).contains(&pid.to_string())
}

/// The helper's pid, once the provider has written it to the file.
fn wait_for_pid(file: &std::path::Path) -> u32 {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let pid = std::fs::read_to_string(file)
            .ok()
            .and_then(|text| text.trim().parse::<u32>().ok());
        if let Some(pid) = pid {
            return pid;
        }
        assert!(
            Instant::now() < deadline,
            "the provider writes the helper pid within twenty seconds"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A provider started the way the daemon starts one: the PowerShell process is
/// the direct child, and it waits a moment before starting a helper, so the
/// helper is born after the provider has joined the job.
fn direct_provider(pid_file: &std::path::Path) -> std::process::Child {
    let script = format!(
        "Start-Sleep -Seconds 2; $p = Start-Process -FilePath '{}' -ArgumentList '-n 60 127.0.0.1' -WindowStyle Hidden -PassThru; Set-Content -Path '{}' -Value $p.Id; Start-Sleep -Seconds 60",
        system_tool("ping.exe").display(),
        pid_file.display()
    );
    let shell = system_root()
        .join("System32")
        .join("WindowsPowerShell")
        .join("v1.0")
        .join("powershell.exe");
    Command::new(shell)
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .expect("PowerShell starts")
}

/// The helper of a direct provider is a descendant, so it is protected: no
/// target, no signal, and the row names it as a spared descendant.
#[test]
fn a_helper_under_a_direct_provider_is_spared_and_keeps_running() {
    let state = ServerState::new("pm-cleanup-launch".to_string());
    live_session(&state, "bypassPermissions");
    let pid_file = crate::test_dirs::test_temp_dir("pm-cleanup-launch").join("helper.pid");
    let _ = std::fs::remove_file(&pid_file);
    let mut provider = direct_provider(&pid_file);
    join_session(&state, provider.id());
    let helper = wait_for_pid(&pid_file);

    let reply = run_cleanup(&state);

    assert!(
        terminated_pids(&reply).is_empty(),
        "a helper under the provider is never a target"
    );
    assert!(is_running(helper), "the helper keeps running");
    let outcome = audit_outcome(&state);
    assert!(
        outcome.contains(&format!("{helper}:agent_descendant")),
        "{outcome}"
    );

    let _ = provider.kill();
    let _ = provider.wait();
    let _ = Command::new(system_tool("taskkill.exe"))
        .args(["/PID", &helper.to_string(), "/F"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status();
}
