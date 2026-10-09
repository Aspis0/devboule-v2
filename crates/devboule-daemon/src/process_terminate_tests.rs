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
        CreationStatus::At(started_at_ticks) => PlanTarget {
            pid,
            started_at_ticks,
            exe: None,
        },
        CreationStatus::Gone => panic!("our own child must still exist"),
        CreationStatus::Unverified => panic!("our own child must have a readable creation time"),
    }
}

#[cfg(target_os = "macos")]
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

/// macOS reads creation times in whole seconds, so an equal time cannot tell
/// this process from a pid reused within the same second. The identity check
/// therefore spares it before any signal: no grace is spent, nothing is sent,
/// and the child keeps running.
#[cfg(target_os = "macos")]
#[test]
fn a_coarse_identity_is_spared_without_a_signal() {
    let mut child = spawn_immune_child();
    let plan = real_plan(child.id());

    let started = Instant::now();
    let termination = terminate_all(&[plan], Duration::from_millis(300), &os_target_check)
        .expect("termination is bounded");
    let elapsed = started.elapsed();

    assert_eq!(
        termination.skipped,
        vec![(child.id(), "identity_unverifiable")],
        "an equal coarse time proves nothing"
    );
    assert!(termination.terminated.is_empty(), "nothing was stopped");
    assert!(
        child.try_wait().expect("the child's status").is_none(),
        "no signal reached the child"
    );
    assert!(
        elapsed < Duration::from_millis(300),
        "a spared target spends no grace: {elapsed:?}"
    );
    let _ = child.kill();
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

/// A graceful ask the OS refuses has nothing to wait for: with a grace far
/// longer than the bound below, the forced phase must start at once.
#[cfg(windows)]
#[test]
fn a_refused_graceful_ask_does_not_burn_the_grace() {
    let mut child = spawn_immune_child();
    let plan = real_plan(child.id());

    let started = Instant::now();
    let termination = terminate_all(&[plan], Duration::from_secs(20), &os_target_check)
        .expect("termination is bounded");
    let elapsed = started.elapsed();

    assert!(
        elapsed < Duration::from_secs(10),
        "forced without waiting out the grace: {elapsed:?}"
    );
    assert_eq!(termination.terminated, vec![child.id()]);
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

/// A target the session's job no longer holds is spared and reported, and
/// nothing is sent to it.
#[cfg(any(windows, target_os = "macos"))]
#[test]
fn a_target_that_left_the_session_is_spared_and_reported() {
    let mut child = spawn_immune_child();
    let plan = real_plan(child.id());

    let termination = terminate_all(&[plan], Duration::from_millis(100), &|_pid, _planned| {
        TargetVerdict::NotOwned
    })
    .expect("termination is bounded");

    assert!(termination.terminated.is_empty(), "nothing was signalled");
    assert_eq!(
        termination.skipped,
        vec![(child.id(), "no_longer_in_session")]
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

/// The ownership half of the check, against a real job: a process outside it
/// is not owned, once assigned it is, and a session with no live job owns
/// nothing.
#[cfg(windows)]
#[test]
fn ownership_is_read_from_the_job_at_the_moment_of_the_check() {
    use std::os::windows::io::AsRawHandle;

    let mut child = spawn_immune_child();
    let started_at_ticks = real_plan(child.id()).started_at_ticks;
    let job = crate::process_tree::JobObject::new().expect("job");

    assert!(matches!(
        owned_target_check(Some(&job), child.id(), started_at_ticks),
        TargetVerdict::NotOwned
    ));
    job.assign(child.as_raw_handle()).expect("child joins");
    assert!(matches!(
        owned_target_check(Some(&job), child.id(), started_at_ticks),
        TargetVerdict::Confirmed
    ));
    assert!(matches!(
        owned_target_check(None, child.id(), started_at_ticks),
        TargetVerdict::NotOwned
    ));
    assert!(
        matches!(
            owned_target_check(Some(&job), child.id(), started_at_ticks + 1),
            TargetVerdict::Changed
        ),
        "a different creation time is the identity verdict, not an ownership one"
    );
    let _ = child.kill();
    let _ = child.wait();
}

/// A graceful ask that comes back with a failure status is a refusal; one
/// that never comes back, or never started, is unanswered — and only the
/// refusal skips the grace.
#[cfg(windows)]
#[test]
fn the_graceful_ask_tells_a_refusal_from_a_timeout() {
    use super::platform::{ask_outcome, GracefulAsk};

    let system_root =
        std::path::PathBuf::from(std::env::var_os("SystemRoot").expect("%SystemRoot% is set"));
    let cmd = |code: &str| {
        let mut command = Command::new(system_root.join("System32").join("cmd.exe"));
        command.args(["/C", "exit", code]);
        Some(command)
    };
    let limit = Duration::from_secs(5);

    assert_eq!(ask_outcome(cmd("0"), limit), GracefulAsk::Accepted);
    assert_eq!(ask_outcome(cmd("1"), limit), GracefulAsk::Refused);

    let mut slow = Command::new(system_root.join("System32").join("ping.exe"));
    slow.args(["-n", "60", "127.0.0.1"]);
    let started = Instant::now();
    assert_eq!(
        ask_outcome(Some(slow), Duration::from_millis(200)),
        GracefulAsk::Unanswered,
        "a helper that outlives its limit gave no verdict"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "and was cut off"
    );
    assert_eq!(ask_outcome(None, limit), GracefulAsk::Unanswered);
}

/// Two creation times one 100 ns tick apart name two processes: a pid reused a
/// tick later is not the planned process, so it is never confirmed.
#[test]
fn a_creation_time_one_tick_apart_is_a_different_process() {
    assert!(matches!(
        creation_verdict(1_001, 1_000, true),
        TargetVerdict::Changed
    ));
}

/// An equal time confirms the process only where the platform's times are at
/// full resolution. A coarse time cannot tell a reused pid apart, so it stays
/// unverified and the process is spared.
#[test]
fn an_equal_coarse_time_is_unverified_and_an_equal_full_time_confirms() {
    assert!(matches!(
        creation_verdict(1_000, 1_000, true),
        TargetVerdict::Confirmed
    ));
    assert!(matches!(
        creation_verdict(1_000, 1_000, false),
        TargetVerdict::Unverified
    ));
}
