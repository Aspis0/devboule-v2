//! Terminating a cleanup plan's proven members: identity re-checked
//! immediately before each signal, graceful first, forced after the grace,
//! and what was skipped reported rather than assumed.
//!
//! The plan carries (pid, creation time); nothing is signalled when the
//! platform no longer vouches that the pid still names that process and that
//! the session's job or group still holds it. On Windows both phases run
//! while the handle opened at check time is held, which pins the original
//! process even if the pid is reused meanwhile.

use std::io;
use std::time::{Duration, Instant};

use crate::process_index::{CreationStatus, Membership};
use crate::process_plan::PlanTarget;
use crate::process_tree::JobObject;

/// What the identity check says right before a signal.
pub(crate) enum TargetVerdict {
    /// The pid still names the planned process.
    Confirmed,
    /// The pid names a process created at some other time.
    Changed,
    /// No process answers for the pid: it is already gone.
    Gone,
    /// The creation time could not be read: never signal blind.
    Unverified,
    /// The process is the planned one but the session's job or group no
    /// longer holds it: it left the proof the plan was drawn from.
    NotOwned,
}

/// The real check: the platform's creation-time read a moment before the
/// signal, compared against the time the plan recorded.
pub(crate) fn os_target_check(pid: u32, planned: u64) -> TargetVerdict {
    match crate::process_index::creation_status(pid) {
        CreationStatus::Gone => TargetVerdict::Gone,
        CreationStatus::At(actual) if actual == planned => TargetVerdict::Confirmed,
        CreationStatus::At(_) => TargetVerdict::Changed,
        CreationStatus::Unverified => TargetVerdict::Unverified,
    }
}

/// The check a cleanup runs before every signal: the identity check, then
/// whether the session's job or group still holds the pid right now. Without
/// a live job nothing is the session's to stop.
pub(crate) fn owned_target_check(job: Option<&JobObject>, pid: u32, planned: u64) -> TargetVerdict {
    let verdict = os_target_check(pid, planned);
    if !matches!(verdict, TargetVerdict::Confirmed) {
        return verdict;
    }
    let Some(job) = job else {
        return TargetVerdict::NotOwned;
    };
    match crate::process_index::membership(job, pid) {
        Membership::Member => TargetVerdict::Confirmed,
        Membership::Outside => TargetVerdict::NotOwned,
        Membership::Unreadable => TargetVerdict::Unverified,
    }
}

/// What the two phases left: everything that is gone (and which of those
/// needed the forced signal), whatever still answers after it, and every
/// target no signal was sent to — with the reason it was spared.
pub(crate) struct Termination {
    pub(crate) terminated: Vec<u32>,
    pub(crate) forced: Vec<u32>,
    pub(crate) still_running: Vec<u32>,
    pub(crate) skipped: Vec<(u32, &'static str)>,
}

/// How long the forced phase gets to take effect.
const FORCED_WAIT: Duration = Duration::from_millis(500);
const ALIVE_POLL: Duration = Duration::from_millis(10);

#[cfg(any(windows, target_os = "macos"))]
#[path = "process_terminate_platform.rs"]
mod platform;

#[cfg(not(any(windows, target_os = "macos")))]
mod platform {
    use super::*;

    pub(crate) fn terminate_all(
        _targets: &[PlanTarget],
        _grace: Duration,
        _check: &dyn Fn(u32, u64) -> TargetVerdict,
    ) -> io::Result<Termination> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "process cleanup is only implemented on Windows and macOS",
        ))
    }
}

pub(crate) fn terminate_all(
    targets: &[PlanTarget],
    grace: Duration,
    check: &dyn Fn(u32, u64) -> TargetVerdict,
) -> io::Result<Termination> {
    platform::terminate_all(targets, grace, check)
}

#[cfg(test)]
#[path = "process_terminate_tests.rs"]
mod tests;
