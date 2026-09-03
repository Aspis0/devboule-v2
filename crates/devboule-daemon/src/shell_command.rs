#[cfg(windows)]
use std::path::Path;
use std::path::PathBuf;

use devboule_protocol::{ErrorCode, WireError};

use crate::paths::RuntimePaths;

use super::PtyCommand;

const SHELL_OVERRIDE_ENV: &str = "DEVBOULE_SHELL";
const TEST_PTY_COMMAND_FILE: &str = ".test-pty-command";

pub(crate) fn resolve_pty_command(paths: &RuntimePaths) -> Result<PtyCommand, WireError> {
    #[cfg(debug_assertions)]
    {
        if let Some(command) = load_test_pty_command(paths) {
            return Ok(command);
        }
    }
    #[cfg(not(debug_assertions))]
    {
        let _ = paths;
    }
    shell_command()
}

#[cfg(debug_assertions)]
fn load_test_pty_command(paths: &RuntimePaths) -> Option<PtyCommand> {
    let path = paths.dir.join(TEST_PTY_COMMAND_FILE);
    let bytes = std::fs::read(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let program = value.get("program")?.as_str()?.to_string();
    let args = value
        .get("args")
        .and_then(|args| args.as_array())
        .map(|args| {
            args.iter()
                .filter_map(|value| value.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let cwd = value
        .get("cwd")
        .and_then(|cwd| cwd.as_str())
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok())?;
    Some(PtyCommand::new(program, args, cwd, Vec::new()))
}

fn shell_command() -> Result<PtyCommand, WireError> {
    let cwd = std::env::current_dir().map_err(|error| {
        WireError::new(
            ErrorCode::Io,
            format!("Could not determine terminal directory: {error}"),
        )
    })?;
    let (program, args) = configured_shell();
    Ok(PtyCommand::new(program, args, cwd, Vec::new()))
}

fn configured_shell() -> (String, Vec<String>) {
    if let Ok(override_shell) = std::env::var(SHELL_OVERRIDE_ENV) {
        if !override_shell.trim().is_empty() {
            return (override_shell, shell_args());
        }
    }
    #[cfg(windows)]
    {
        let program = if executable_on_path("pwsh.exe") {
            "pwsh.exe"
        } else {
            "powershell.exe"
        };
        (
            program.to_string(),
            vec!["-NoLogo".to_string(), "-NoProfile".to_string()],
        )
    }
    #[cfg(not(windows))]
    {
        let program = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string());
        (program, shell_args())
    }
}

fn shell_args() -> Vec<String> {
    #[cfg(windows)]
    {
        vec!["-NoLogo".to_string(), "-NoProfile".to_string()]
    }
    #[cfg(not(windows))]
    {
        Vec::new()
    }
}

#[cfg(windows)]
fn executable_on_path(program: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|directory| {
        let candidate = Path::new(&directory).join(program);
        candidate.is_file()
    })
}

/// Write a spawn override the next `session_create` will consume. Honored
/// only in debug builds of the daemon (see [`load_test_pty_command`]).
pub fn write_test_pty_command(paths: &RuntimePaths, command: &PtyCommand) -> std::io::Result<()> {
    paths.ensure_dir()?;
    let body = serde_json::json!({
        "program": command.program,
        "args": command.args,
        "cwd": command.cwd,
    });
    std::fs::write(paths.dir.join(TEST_PTY_COMMAND_FILE), body.to_string())
}
