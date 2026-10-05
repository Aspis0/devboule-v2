//! Terminating a cleanup plan's proven members: a graceful signal first, a
//! forced one after the grace, and survivors reported rather than assumed.
//!
//! Only the pids the plan named are ever signalled — the plan came from the
//! index's proof — and the session's own root is not in the plan at all.

use std::io;
use std::time::{Duration, Instant};

/// What the two phases left: everything that is gone, and whatever still
/// answers after the forced signal.
pub(crate) struct Termination {
    pub(crate) terminated: Vec<u32>,
    pub(crate) still_running: Vec<u32>,
}

/// How long the forced phase gets to take effect.
const FORCED_WAIT: Duration = Duration::from_millis(500);
const ALIVE_POLL: Duration = Duration::from_millis(10);

#[cfg(any(windows, target_os = "macos"))]
mod platform {
    use super::*;

    pub(crate) fn terminate_all(targets: &[u32], grace: Duration) -> io::Result<Termination> {
        for pid in targets {
            graceful(*pid);
        }
        let survivors = wait_for_gone(targets, grace);
        if survivors.is_empty() {
            return Ok(Termination {
                terminated: targets.to_vec(),
                still_running: Vec::new(),
            });
        }
        for pid in &survivors {
            forced(*pid);
        }
        let still_running = wait_for_gone(&survivors, FORCED_WAIT);
        let mut terminated: Vec<u32> = targets.to_vec();
        terminated.retain(|pid| !still_running.contains(pid));
        Ok(Termination {
            terminated,
            still_running,
        })
    }

    /// Poll until none of the pids answers, bounded by `budget` — a survivor
    /// after the bound is reported, never assumed dead.
    fn wait_for_gone(pids: &[u32], budget: Duration) -> Vec<u32> {
        let deadline = Instant::now() + budget;
        let mut alive: Vec<u32> = pids.iter().copied().filter(|pid| is_alive(*pid)).collect();
        while !alive.is_empty() && Instant::now() < deadline {
            std::thread::sleep(ALIVE_POLL);
            alive.retain(|pid| is_alive(*pid));
        }
        alive
    }

    /// The graceful signal: the OS's own soft ask, before anything is forced.
    #[cfg(windows)]
    fn graceful(pid: u32) {
        taskkill(pid, false);
    }

    #[cfg(target_os = "macos")]
    fn graceful(pid: u32) {
        signal(pid, libc::SIGTERM);
    }

    #[cfg(windows)]
    fn forced(pid: u32) {
        taskkill(pid, true);
    }

    #[cfg(target_os = "macos")]
    fn forced(pid: u32) {
        signal(pid, libc::SIGKILL);
    }

    /// `taskkill` without `/F` is the OS's graceful attempt for a foreign
    /// process; there is no documented native equivalent for a process this
    /// daemon did not create a console group for. The executable comes from
    /// `%SystemRoot%`, never from `PATH`, and a taskkill that cannot be
    /// spawned leaves the pid to the forced phase (and possibly to
    /// `still_running`), which is the honest answer rather than a claim.
    #[cfg(windows)]
    fn taskkill(pid: u32, force: bool) {
        use std::os::windows::process::CommandExt;
        use std::path::PathBuf;
        use std::process::{Command, Stdio};

        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let Some(system_root) = std::env::var_os("SystemRoot") else {
            return;
        };
        let program = PathBuf::from(system_root)
            .join("System32")
            .join("taskkill.exe");
        let mut arguments = vec!["/PID".to_string(), pid.to_string()];
        if force {
            arguments.insert(0, "/F".to_string());
        }
        let mut child = match Command::new(program)
            .args(&arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
        {
            Ok(child) => child,
            Err(_) => return,
        };
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Ok(None) | Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return;
                }
            }
        }
    }

    #[cfg(target_os = "macos")]
    fn signal(pid: u32, signal: i32) {
        let Ok(pid) = i32::try_from(pid) else {
            return;
        };
        // SAFETY: a signal to our plan's own pid; a failure (already gone)
        // reads as nothing to signal.
        unsafe {
            libc::kill(pid, signal);
        }
    }

    /// Whether a pid still answers. An open we cannot interpret keeps the
    /// pid in the survivor set — a false `still_running` is safer than a
    /// missed kill.
    #[cfg(windows)]
    fn is_alive(pid: u32) -> bool {
        use windows_sys::Win32::Foundation::{
            CloseHandle, GetLastError, ERROR_INVALID_PARAMETER, WAIT_OBJECT_0,
        };
        use windows_sys::Win32::System::Threading::{
            OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
        };

        let handle = unsafe { OpenProcess(PROCESS_SYNCHRONIZE, 0, pid) };
        if handle.is_null() {
            return unsafe { GetLastError() } != ERROR_INVALID_PARAMETER;
        }
        let alive = unsafe { WaitForSingleObject(handle, 0) } != WAIT_OBJECT_0;
        unsafe { CloseHandle(handle) };
        alive
    }

    #[cfg(target_os = "macos")]
    fn is_alive(pid: u32) -> bool {
        // SAFETY: signal 0 only asks whether the pid exists. A zombie still
        // answers, so a reaped orphan may be reported for a short while —
        // reaping happens outside our hands.
        unsafe { libc::kill(pid, 0) == 0 }
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
mod platform {
    use super::*;

    pub(crate) fn terminate_all(_targets: &[u32], _grace: Duration) -> io::Result<Termination> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "process cleanup is only implemented on Windows and macOS",
        ))
    }
}

pub(crate) fn terminate_all(targets: &[u32], grace: Duration) -> io::Result<Termination> {
    platform::terminate_all(targets, grace)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::{Command, Stdio};

    /// The wait is real: a child that ignores the graceful signal holds the
    /// full grace, then the forced phase takes it and the report says so.
    #[cfg(not(windows))]
    #[test]
    fn cleanup_waits_then_reports_survivors() {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "trap '' TERM; while :; do sleep 1; done"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        crate::process_tree::lead_own_group(&mut command);
        let mut child = command.spawn().expect("our own child spawns");
        let pid = child.id();

        let started = Instant::now();
        let termination =
            terminate_all(&[pid], Duration::from_millis(300)).expect("termination is bounded");
        let elapsed = started.elapsed();

        assert!(
            elapsed >= Duration::from_millis(300),
            "the graceful phase waits its grace before forcing: {elapsed:?}"
        );
        assert!(
            elapsed < Duration::from_secs(10),
            "bounded overall: {elapsed:?}"
        );
        assert_eq!(termination.terminated, vec![pid], "forced after the grace");
        assert!(
            termination.still_running.is_empty(),
            "the forced phase took it"
        );
        let _ = child.wait();
    }

    /// The same call on Windows: one pid, spawned by this test, terminated
    /// through the OS's own two-phase path within the bound.
    #[cfg(windows)]
    #[test]
    fn cleanup_terminates_its_own_process_on_windows() {
        let system_root =
            std::path::PathBuf::from(std::env::var_os("SystemRoot").expect("%SystemRoot% is set"));
        let mut child = Command::new(system_root.join("System32").join("cmd.exe"))
            .args(["/C", "ping", "-n", "60", "127.0.0.1"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("our own child spawns");
        let pid = child.id();

        let started = Instant::now();
        let termination =
            terminate_all(&[pid], Duration::from_millis(300)).expect("termination is bounded");
        let elapsed = started.elapsed();

        assert!(elapsed < Duration::from_secs(10), "bounded: {elapsed:?}");
        assert_eq!(termination.terminated, vec![pid], "the child is gone");
        assert!(termination.still_running.is_empty());
        let _ = child.wait();
    }
}
