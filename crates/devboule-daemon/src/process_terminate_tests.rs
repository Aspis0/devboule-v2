//! The terminate path against processes this test owns: the real platform
//! identity check before every signal, the identity skip, and the two
//! phases' bounds. Nothing here ever signals a pid it did not spawn.

use super::*;
use std::cell::Cell;
use std::process::{Command, Stdio};

/// A plan for a process this test just spawned: the real creation time the
/// platform reports, so the real check and the Windows arm both confirm it.
fn real_plan(pid: u32) -> PlanTarget {
    match crate::process_index::creation_status(pid) {
        CreationStatus::At(started_at_ms) => PlanTarget {
            pid,
            started_at_ms,
            exe: None,
        },
        CreationStatus::Gone => panic!("our own child must still exist"),
        CreationStatus::Unverified => panic!("our own child must have a readable creation time"),
    }
}

#[cfg(not(windows))]
fn spawn_immune_child() -> std::process::Child {
    let mut command = Command::new("/bin/sh");
    command
        .args(["-c", "trap '' TERM; while :; do sleep 1; done"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    crate::process_tree::lead_own_group(&mut command);
    command.spawn().expect("our own child spawns")
}

#[cfg(windows)]
fn spawn_immune_child() -> std::process::Child {
    // A headless process with no console of its own: `taskkill` without
    // `/F` has no window to close and refuses it, so the graceful phase is
    // provably a no-op and the test reaches the forced one.
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    // Absolute paths throughout: a test environment may hand the process a
    // PATH without System32, and a child that dies at once proves nothing
    // about either phase.
    let system_root =
        std::path::PathBuf::from(std::env::var_os("SystemRoot").expect("%SystemRoot% is set"));
    Command::new(system_root.join("System32").join("ping.exe"))
        .args(["-n", "60", "127.0.0.1"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .expect("our own child spawns")
}

/// The wait is real: a child that ignores the graceful signal holds the full
/// grace, then the forced phase takes it and the report says so.
#[cfg(not(windows))]
#[test]
fn cleanup_waits_then_reports_survivors() {
    let mut child = spawn_immune_child();
    let plan = real_plan(child.id());

    let started = Instant::now();
    let termination = terminate_all(&[plan], Duration::from_millis(300), &os_target_check)
        .expect("termination is bounded");
    let elapsed = started.elapsed();

    assert!(
        elapsed >= Duration::from_millis(300),
        "the graceful phase waits its grace before forcing: {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(10),
        "bounded overall: {elapsed:?}"
    );
    assert_eq!(
        termination.terminated,
        vec![child.id()],
        "forced after the grace"
    );
    assert!(
        termination.still_running.is_empty(),
        "the forced phase took it"
    );
    assert!(
        termination.skipped.is_empty(),
        "the real check confirmed everything"
    );
    let _ = child.wait();
}

/// The same call on Windows: one pid, spawned by this test, terminated
/// through the OS's own two-phase path within the bound.
#[cfg(windows)]
#[test]
fn cleanup_terminates_its_own_process_on_windows() {
    let mut child = spawn_immune_child();
    let plan = real_plan(child.id());

    let started = Instant::now();
    let termination = terminate_all(&[plan], Duration::from_millis(300), &os_target_check)
        .expect("termination is bounded");
    let elapsed = started.elapsed();

    assert!(elapsed < Duration::from_secs(10), "bounded: {elapsed:?}");
    assert_eq!(
        termination.terminated,
        vec![child.id()],
        "the child is gone"
    );
    assert!(termination.still_running.is_empty());
    assert!(termination.skipped.is_empty());
    let _ = child.wait();
}

/// A plan whose identity no longer matches is not signalled at the graceful
/// phase at all: the process this test spawned stays alive, and the skip is
/// reported with its reason.
#[cfg(any(windows, target_os = "macos"))]
#[test]
fn a_changed_identity_is_never_signalled_at_the_graceful_phase() {
    let mut child = spawn_immune_child();
    let plan = real_plan(child.id());

    let termination = terminate_all(&[plan], Duration::from_millis(100), &|_pid, _planned| {
        TargetVerdict::Changed
    })
    .expect("termination is bounded");

    assert_eq!(
        termination.terminated,
        Vec::<u32>::new(),
        "nothing was signalled"
    );
    assert!(termination.still_running.is_empty());
    assert_eq!(
        termination.skipped,
        vec![(child.id(), "creation_time_changed")]
    );
    assert!(
        matches!(
            crate::process_index::creation_status(child.id()),
            CreationStatus::At(_)
        ),
        "our own child is untouched"
    );
    let _ = child.kill();
    let _ = child.wait();
}

/// The forced phase re-checks: the first check confirms, the second (right
/// before the forced signal) reports a changed identity, and the survivor is
/// left alone and reported instead of signalled.
#[cfg(any(windows, target_os = "macos"))]
#[test]
fn the_forced_phase_skips_an_identity_that_changed_after_the_grace() {
    let mut child = spawn_immune_child();
    let plan = real_plan(child.id());
    let calls = Cell::new(0u32);
    let check = |_pid: u32, _planned: u64| {
        calls.set(calls.get() + 1);
        if calls.get() == 1 {
            TargetVerdict::Confirmed
        } else {
            TargetVerdict::Changed
        }
    };

    let termination =
        terminate_all(&[plan], Duration::from_millis(100), &check).expect("termination is bounded");

    assert!(
        calls.get() >= 2,
        "the forced pass checked again: calls={}, skipped={:?}",
        calls.get(),
        termination.skipped
    );
    assert_eq!(
        termination.terminated,
        Vec::<u32>::new(),
        "the forced phase did not act"
    );
    assert_eq!(
        termination.skipped,
        vec![(child.id(), "creation_time_changed")],
        "the skip carries its reason"
    );
    assert!(
        matches!(
            crate::process_index::creation_status(child.id()),
            CreationStatus::At(_)
        ),
        "our own child is untouched after the grace"
    );
    let _ = child.kill();
    let _ = child.wait();
}

/// A target the platform reports as already gone is counted as terminated
/// without a single signal.
#[cfg(any(windows, target_os = "macos"))]
#[test]
fn an_already_gone_target_is_reported_terminated_without_a_signal() {
    let mut child = spawn_immune_child();
    let plan = real_plan(child.id());

    let termination = terminate_all(&[plan], Duration::from_millis(100), &|_pid, _planned| {
        TargetVerdict::Gone
    })
    .expect("termination is bounded");

    assert_eq!(termination.terminated, vec![child.id()]);
    assert!(termination.skipped.is_empty());
    assert!(
        matches!(
            crate::process_index::creation_status(child.id()),
            CreationStatus::At(_)
        ),
        "no signal was sent: the pid's holder is untouched"
    );
    let _ = child.kill();
    let _ = child.wait();
}
