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
    eprintln!(
        "daemon binary not found next to {} (set DEVBOULE_DAEMON)",
        exe.display()
    );
    Err(DaemonError::Protocol(format!(
        "Devboule daemon not found. Set DEVBOULE_DAEMON or install {} beside the app.",
        daemon_file_name()
    )))
}

/// Spawn the daemon as a child of this process. No breakaway, no Service, no
/// WMI: the daemon is allowed to die when Windows tears down this job.
pub fn spawn_daemon(binary: &Path, paths: &RuntimePaths) -> Result<Child, DaemonError> {
    spawn_with_env(binary, paths, &[])
}

/// Hand a spawned daemon to a waiter thread and forget it.
///
/// The app never waits on its daemon — the daemon outlives the window — but
/// on Unix a dropped `Child` stays a zombie on the app until the app exits,
/// and a restart spawns a fresh child every time. One small thread per spawn
/// calls `wait` and ends with the child; off Unix the call keeps the old
/// behaviour, where dropping the child needs no reap.
pub fn reap_spawned_daemon(child: Child) {
    #[cfg(unix)]
    {
        let mut child = child;
        let waiter = std::thread::Builder::new()
            .name("daemon-reaper".to_string())
            .spawn(move || {
                let _ = child.wait();
            });
        if let Err(error) = waiter {
            eprintln!("could not start the daemon reaper thread: {error}");
        }
    }
    #[cfg(not(unix))]
    drop(child);
}

/// [`spawn_daemon`] with extra environment for the daemon (and so for every
/// provider it launches).
///
/// The slice-5 integration battery runs in the same test process as the rest of
/// `acp_sessions.rs`, and the environment is process-global: passing those knobs
/// here rather than through `std::env::set_var` keeps the battery out of a race
/// with any test that does not hold the file's test lock.
#[cfg(any(test, feature = "test-support"))]
pub fn spawn_daemon_with_env(
    binary: &Path,
    paths: &RuntimePaths,
    extra_env: &[(&str, &str)],
) -> Result<Child, DaemonError> {
    spawn_with_env(binary, paths, extra_env)
}

fn spawn_with_env(
    binary: &Path,
    paths: &RuntimePaths,
    extra_env: &[(&str, &str)],
) -> Result<Child, DaemonError> {
    paths.ensure_dir()?;
    let mut command = Command::new(binary);
    command
        .env("DEVBOULE_RUNTIME_DIR", &paths.dir)
        .stdin(Stdio::null());
    #[cfg(windows)]
    command.stdout(Stdio::null()).stderr(Stdio::null());
    // The daemon outlives the app's terminal: its own session, and its
    // output to the daemon log instead of an inherited pipe.
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(paths.dir.join("daemon.log"))
            .map_err(DaemonError::from)?;
        let err_log = log.try_clone().map_err(DaemonError::from)?;
        command.stdout(log).stderr(err_log);
        // SAFETY: `pre_exec` runs between fork and exec; `setsid` takes no
        // arguments. A failure (already a session leader) leaves the child
        // in the parent's session rather than failing the spawn.
        unsafe {
            command.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
    }
    for (key, value) in extra_env {
        command.env(key, value);
    }
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

    #[cfg(windows)]
    #[test]
    fn file_name_is_the_windows_exe() {
        assert_eq!(daemon_file_name(), "devboule-daemon.exe");
    }

    #[cfg(unix)]
    #[test]
    fn file_name_has_no_extension_on_unix() {
        assert_eq!(daemon_file_name(), "devboule-daemon");
    }
}
