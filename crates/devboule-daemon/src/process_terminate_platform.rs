//! The per-platform signals behind the cleanup plan: one identity
//! confirmation per signal, a graceful ask, then a forced one — and the
//! handles that pin a Windows target to the process the check confirmed.

use super::*;

/// The marker that lets the forced phase hit the same process the check
/// confirmed: a handle on Windows, a unit where a signal carries the
/// identity itself.
#[cfg(windows)]
type ForcedHandle = windows_sys::Win32::Foundation::HANDLE;
#[cfg(target_os = "macos")]
type ForcedHandle = ();

enum ArmOutcome {
    Armed(ForcedHandle),
    AlreadyGone,
    Mismatch,
    /// Opened without the rights to force through a handle (Windows
    /// only): the target still gets its phases, forced by pid.
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    NoAccess,
}

pub(crate) fn terminate_all(
    targets: &[PlanTarget],
    grace: Duration,
    check: &dyn Fn(u32, u64) -> TargetVerdict,
) -> io::Result<Termination> {
    let mut attempted: Vec<(PlanTarget, ForcedHandle, bool)> = Vec::new();
    let mut terminated: Vec<u32> = Vec::new();
    let mut skipped: Vec<(u32, &'static str)> = Vec::new();
    for target in targets {
        match check(target.pid, target.started_at_ms) {
            TargetVerdict::Gone => terminated.push(target.pid),
            TargetVerdict::Changed => skipped.push((target.pid, "creation_time_changed")),
            TargetVerdict::Unverified => skipped.push((target.pid, "identity_unverifiable")),
            TargetVerdict::Confirmed => match arm(target.pid, target.started_at_ms) {
                ArmOutcome::Armed(handle) => {
                    let landed = graceful(target.pid);
                    attempted.push((target.clone(), handle, landed));
                }
                ArmOutcome::AlreadyGone => {
                    terminated.push(target.pid);
                }
                ArmOutcome::Mismatch => skipped.push((target.pid, "creation_time_changed")),
                ArmOutcome::NoAccess => {
                    let landed = graceful(target.pid);
                    attempted.push((target.clone(), no_handle(), landed));
                }
            },
        }
    }

    // A graceful ask the OS refused (Windows: taskkill's "can only be
    // terminated forcefully" on a headless process) has nothing to wait out —
    // waiting the grace would only burn it, so those targets are checked
    // once and handed to the forced pass straight away.
    let landed_pids: Vec<u32> = attempted
        .iter()
        .filter(|(_, _, landed)| *landed)
        .map(|(target, _, _)| target.pid)
        .collect();
    let refused_pids: Vec<u32> = attempted
        .iter()
        .filter(|(_, _, landed)| !*landed)
        .map(|(target, _, _)| target.pid)
        .collect();
    let mut survivors = wait_for_gone(&landed_pids, grace);
    survivors.extend(refused_pids.into_iter().filter(|pid| is_alive(*pid)));
    survivors.sort_unstable();
    survivors.dedup();

    let mut forced_pids: Vec<u32> = Vec::new();
    for (target, handle, _) in &attempted {
        if !survivors.contains(&target.pid) {
            close_handle(handle);
            continue;
        }
        match check(target.pid, target.started_at_ms) {
            TargetVerdict::Confirmed => {
                forced(target.pid, *handle);
                forced_pids.push(target.pid);
            }
            TargetVerdict::Gone => {}
            TargetVerdict::Changed => skipped.push((target.pid, "creation_time_changed")),
            TargetVerdict::Unverified => skipped.push((target.pid, "identity_unverifiable")),
        }
        close_handle(handle);
    }
    let still_running = wait_for_gone(&forced_pids, FORCED_WAIT);
    for (target, _, _) in &attempted {
        if !still_running.contains(&target.pid)
            && !skipped.iter().any(|(pid, _)| *pid == target.pid)
        {
            terminated.push(target.pid);
        }
    }
    terminated.sort_unstable();
    terminated.dedup();
    skipped.sort_by_key(|(pid, _)| *pid);
    Ok(Termination {
        terminated,
        still_running,
        skipped,
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
    alive.sort_unstable();
    alive
}

/// Open and confirm the target now: on Windows the handle both proves
/// the creation time and pins the process the forced phase will use; on
/// macOS a signal carries the pid, so confirming was the check's job.
#[cfg(windows)]
fn arm(pid: u32, planned: u64) -> ArmOutcome {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_INVALID_PARAMETER};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
    };

    let handle = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE,
            0,
            pid,
        )
    };
    if handle.is_null() {
        return if unsafe { GetLastError() } == ERROR_INVALID_PARAMETER {
            ArmOutcome::AlreadyGone
        } else {
            ArmOutcome::NoAccess
        };
    }
    let confirmed = creation_time_of(handle).is_some_and(|actual| actual == planned);
    if !confirmed {
        unsafe { CloseHandle(handle) };
        // Whether the pid died or changed, the plan's process is not the
        // one this handle would kill — the same verdict either way.
        return ArmOutcome::Mismatch;
    }
    ArmOutcome::Armed(handle)
}

#[cfg(windows)]
fn creation_time_of(handle: windows_sys::Win32::Foundation::HANDLE) -> Option<u64> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::GetProcessTimes;

    let mut creation = unsafe { std::mem::zeroed::<FILETIME>() };
    let mut exit = unsafe { std::mem::zeroed::<FILETIME>() };
    let mut kernel = unsafe { std::mem::zeroed::<FILETIME>() };
    let mut user = unsafe { std::mem::zeroed::<FILETIME>() };
    let ok = unsafe { GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user) };
    if ok == 0 {
        return None;
    }
    let ticks = ((creation.dwHighDateTime as u64) << 32) | creation.dwLowDateTime as u64;
    Some((ticks / 10_000).saturating_sub(crate::process_index::FILETIME_EPOCH_MS))
}

#[cfg(target_os = "macos")]
fn arm(_pid: u32, _planned: u64) -> ArmOutcome {
    ArmOutcome::Armed(())
}

#[cfg(windows)]
fn no_handle() -> ForcedHandle {
    std::ptr::null_mut()
}

#[cfg(target_os = "macos")]
fn no_handle() -> ForcedHandle {}

#[cfg(windows)]
fn close_handle(handle: &ForcedHandle) {
    if !handle.is_null() {
        unsafe { windows_sys::Win32::Foundation::CloseHandle(*handle) };
    }
}

#[cfg(target_os = "macos")]
fn close_handle(_handle: &ForcedHandle) {}

/// The graceful signal: the OS's own soft ask, before anything is forced.
#[cfg(windows)]
fn graceful(pid: u32) -> bool {
    taskkill(pid, false)
}

#[cfg(target_os = "macos")]
fn graceful(pid: u32) -> bool {
    signal(pid, libc::SIGTERM);
    true
}

/// The forced signal. A handle takes the identity the check confirmed —
/// it kills the original process whatever the pid means now; a null
/// handle (no rights) falls back to the pid, which is why the check ran
/// a moment before.
#[cfg(windows)]
fn forced(pid: u32, handle: ForcedHandle) {
    if handle.is_null() {
        taskkill(pid, true);
        return;
    }
    // SAFETY: the handle was opened with PROCESS_TERMINATE for the
    // process whose creation time the check just confirmed.
    unsafe { windows_sys::Win32::System::Threading::TerminateProcess(handle, 1) };
}

#[cfg(target_os = "macos")]
fn forced(pid: u32, _handle: ForcedHandle) {
    signal(pid, libc::SIGKILL);
}

/// `taskkill` without `/F` is the OS's graceful attempt for a foreign
/// process — there is no documented native equivalent for one this daemon
/// did not create a console group for — and its exit status is the OS's
/// verdict on that ask: success means something may still land within the
/// grace, a refusal ("can only be terminated forcefully" on a headless
/// process) means the forced phase should not wait for it. The executable
/// comes from `%SystemRoot%`, never from `PATH`.
#[cfg(windows)]
fn taskkill(pid: u32, force: bool) -> bool {
    use std::os::windows::process::CommandExt;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let Some(system_root) = std::env::var_os("SystemRoot") else {
        return false;
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
        Err(_) => return false,
    };
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
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
