//! The per-platform signals behind the cleanup plan: one identity
//! confirmation per signal, a graceful ask, then a forced one — and the
//! handles that pin a Windows target to the process the check confirmed.
//! A target that cannot be pinned (no rights to open it) is spared, never
//! signalled by its bare pid.

use super::*;

/// The marker that lets the forced phase hit the same process the check
/// confirmed: a handle on Windows, a unit where a signal carries the
/// identity itself.
#[cfg(windows)]
type ForcedHandle = windows_sys::Win32::Foundation::HANDLE;
#[cfg(target_os = "macos")]
type ForcedHandle = ();

/// macOS only ever arms: a signal carries the pid, so the other outcomes are
/// Windows handle results.
#[cfg_attr(target_os = "macos", allow(dead_code))]
enum ArmOutcome {
    Armed(ForcedHandle),
    AlreadyGone,
    Mismatch,
    /// No rights to open the process: nothing could pin it, so nothing is
    /// sent to it.
    NoAccess,
}

/// How the OS answered the graceful ask.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(target_os = "macos", allow(dead_code))]
pub(super) enum GracefulAsk {
    /// The ask was delivered; the process may still take a while to leave.
    Accepted,
    /// The OS said this process cannot be asked to close.
    Refused,
    /// No answer arrived (the helper could not start or ran out of time):
    /// nothing is known, so the grace is waited out like an accepted ask.
    Unanswered,
}

/// The reason a verdict spares its target; `None` when the target may be
/// signalled or is already gone.
fn spare_reason(verdict: &TargetVerdict) -> Option<&'static str> {
    match verdict {
        TargetVerdict::Changed => Some("creation_time_changed"),
        TargetVerdict::Unverified => Some("identity_unverifiable"),
        TargetVerdict::NotOwned => Some("no_longer_in_session"),
        TargetVerdict::Confirmed | TargetVerdict::Gone => None,
    }
}

pub(crate) fn terminate_all(
    targets: &[PlanTarget],
    grace: Duration,
    check: &dyn Fn(u32, u64) -> TargetVerdict,
) -> io::Result<Termination> {
    let mut attempted: Vec<(PlanTarget, ForcedHandle, GracefulAsk)> = Vec::new();
    let mut terminated: Vec<u32> = Vec::new();
    let mut skipped: Vec<(u32, &'static str)> = Vec::new();
    for target in targets {
        let verdict = check(target.pid, target.started_at_ms);
        if let Some(reason) = spare_reason(&verdict) {
            skipped.push((target.pid, reason));
            continue;
        }
        if matches!(verdict, TargetVerdict::Gone) {
            terminated.push(target.pid);
            continue;
        }
        match arm(target.pid, target.started_at_ms) {
            ArmOutcome::Armed(handle) => {
                let ask = graceful(target.pid);
                attempted.push((target.clone(), handle, ask));
            }
            ArmOutcome::AlreadyGone => terminated.push(target.pid),
            ArmOutcome::Mismatch => skipped.push((target.pid, "creation_time_changed")),
            ArmOutcome::NoAccess => skipped.push((target.pid, "access_denied")),
        }
    }

    // Only an ask the OS refused outright (Windows: taskkill's "can only be
    // terminated forcefully" on a headless process) has nothing to wait out.
    // An ask that was accepted or never got an answer gets its full grace:
    // the process may still be closing.
    let (refused, waiting): (Vec<_>, Vec<_>) = attempted
        .iter()
        .partition(|(_, _, ask)| *ask == GracefulAsk::Refused);
    let landed_pids: Vec<u32> = waiting.iter().map(|(target, _, _)| target.pid).collect();
    let refused_pids: Vec<u32> = refused.iter().map(|(target, _, _)| target.pid).collect();
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
        let verdict = check(target.pid, target.started_at_ms);
        if let Some(reason) = spare_reason(&verdict) {
            skipped.push((target.pid, reason));
        } else if matches!(verdict, TargetVerdict::Confirmed) {
            forced(target.pid, handle);
            forced_pids.push(target.pid);
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
    let forced: Vec<u32> = forced_pids
        .into_iter()
        .filter(|pid| terminated.contains(pid))
        .collect();
    Ok(Termination {
        terminated,
        forced,
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
fn close_handle(handle: &ForcedHandle) {
    unsafe { windows_sys::Win32::Foundation::CloseHandle(*handle) };
}

#[cfg(target_os = "macos")]
fn close_handle(_handle: &ForcedHandle) {}

/// The graceful signal: the OS's own soft ask, before anything is forced.
/// The armed handle is still open here, so the pid cannot name another
/// process while the ask is in flight.
#[cfg(windows)]
fn graceful(pid: u32) -> GracefulAsk {
    ask_outcome(taskkill_command(pid), TASKKILL_LIMIT)
}

#[cfg(target_os = "macos")]
fn graceful(pid: u32) -> GracefulAsk {
    signal(pid, libc::SIGTERM);
    GracefulAsk::Accepted
}

/// The forced signal, through the handle the check confirmed: it kills the
/// original process whatever the pid means now.
#[cfg(windows)]
fn forced(_pid: u32, handle: &ForcedHandle) {
    // SAFETY: the handle was opened with PROCESS_TERMINATE for the
    // process whose creation time the check just confirmed.
    unsafe { windows_sys::Win32::System::Threading::TerminateProcess(*handle, 1) };
}

#[cfg(target_os = "macos")]
fn forced(pid: u32, _handle: &ForcedHandle) {
    signal(pid, libc::SIGKILL);
}

/// How long `taskkill` gets to answer before the ask counts as unanswered.
#[cfg(windows)]
const TASKKILL_LIMIT: Duration = Duration::from_secs(3);

/// `taskkill` without `/F` is the OS's graceful attempt for a foreign
/// process — there is no documented native equivalent for one this daemon
/// did not create a console group for. The executable comes from
/// `%SystemRoot%`, never from `PATH`; without it nothing can be asked.
#[cfg(windows)]
fn taskkill_command(pid: u32) -> Option<std::process::Command> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let system_root = std::env::var_os("SystemRoot")?;
    let mut command = Command::new(
        std::path::PathBuf::from(system_root)
            .join("System32")
            .join("taskkill.exe"),
    );
    command
        .args(["/PID", &pid.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW);
    Some(command)
}

/// Run the ask and classify what came back: an exit status is the OS's
/// verdict (success accepts, a failure status refuses — "can only be
/// terminated forcefully"), while a command that cannot start or does not
/// finish in `limit` is no verdict at all.
#[cfg(windows)]
pub(super) fn ask_outcome(command: Option<std::process::Command>, limit: Duration) -> GracefulAsk {
    let Some(mut command) = command else {
        return GracefulAsk::Unanswered;
    };
    let Ok(mut child) = command.spawn() else {
        return GracefulAsk::Unanswered;
    };
    let deadline = Instant::now() + limit;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return GracefulAsk::Accepted,
            Ok(Some(_)) => return GracefulAsk::Refused,
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return GracefulAsk::Unanswered;
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

/// Whether a pid still runs. An open we cannot interpret keeps the pid in the
/// survivor set — a false `still_running` is safer than a missed kill.
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
    // A pid that does not fit `pid_t` names nothing: wrapped, it would be a
    // negative pid, which `kill` reads as a process group.
    let Ok(signalled) = i32::try_from(pid) else {
        return false;
    };
    // SAFETY: signal 0 only asks whether the pid exists.
    if unsafe { libc::kill(signalled, 0) } != 0 {
        return false;
    }
    // A killed child its parent has not reaped yet still answers signal 0; it
    // is gone, not a survivor.
    !crate::process_index::has_exited(pid)
}
