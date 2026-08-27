use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use crate::error::DaemonError;
use crate::paths::RuntimePaths;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub fn daemon_file_name() -> &'static str {
    #[cfg(windows)]
    {
        "devboule-daemon.exe"
    }
    #[cfg(not(windows))]
    {
        "devboule-daemon"
    }
}

pub fn resolve_daemon_binary() -> Result<PathBuf, DaemonError> {
    if let Some(path) = std::env::var_os("DEVBOULE_DAEMON") {
        return Ok(PathBuf::from(path));
    }
    let exe = std::env::current_exe()?;
    let sibling = exe.with_file_name(daemon_file_name());
    if sibling.is_file() {
        return Ok(sibling);
    }
    Err(DaemonError::Protocol(format!(
        "daemon binary not found next to {} (set DEVBOULE_DAEMON)",
        exe.display()
    )))
}

/// Spawn the daemon as a child of this process. No breakaway, no Service, no
/// WMI: the daemon is allowed to die when Windows tears down this job.
pub fn spawn_daemon(binary: &Path, paths: &RuntimePaths) -> Result<Child, DaemonError> {
    paths.ensure_dir()?;
    let mut command = Command::new(binary);
    command
        .env("DEVBOULE_RUNTIME_DIR", &paths.dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    Ok(command.spawn()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_name_is_the_windows_exe() {
        assert_eq!(daemon_file_name(), "devboule-daemon.exe");
    }
}
