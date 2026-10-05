//! Unix containment for a child: the process group the child itself leads.
//!
//! The Windows twin is a kernel Job Object, assigned the handle of a
//! created-suspended process. Unix has no object a running process can be
//! adopted into, so the group is chosen before the child runs — `setpgid` on
//! a piped command, portable-pty's own `setsid` on the PTY road — and this
//! owner remembers the group id. Every signal goes to `-group`: a leader's
//! pid alone is never signalled, so a pid the OS has since reused cannot take
//! a signal meant for the tree.

use std::io;
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

/// How long a group gets between checks while a bounded wait watches it
/// empty.
const POLL: Duration = Duration::from_millis(10);

/// One child tree's process group. The group id is the child's pid because
/// both spawn roads make the child the group leader.
#[derive(Debug)]
pub struct JobObject {
    group: Mutex<Option<i32>>,
}

impl JobObject {
    pub fn new() -> io::Result<Self> {
        Ok(Self {
            group: Mutex::new(None),
        })
    }

    /// The group this owner still holds — the root of every membership
    /// question the process index asks. `None` once ownership was cleared by
    /// a confirmed-empty sweep, which honestly means there is nothing left
    /// to prove membership of.
    pub fn group_id(&self) -> Option<u32> {
        let group = *self.group.lock().unwrap_or_else(PoisonError::into_inner);
        group.and_then(|group| u32::try_from(group).ok())
    }

    /// Own the group `pid` leads. The parent also calls `setpgid`, the other
    /// half of the standard two-sided protocol: whichever side wins, the
    /// group exists with the child as its leader by the time this returns.
    /// The `getpgid` check is the authority — a child in another group (the
    /// daemon's own, say) is refused, never signalled.
    pub fn assign_group(&self, pid: u32) -> io::Result<()> {
        let child = i32::try_from(pid).map_err(|_| invalid_pid(pid))?;
        if child <= 0 {
            return Err(invalid_pid(pid));
        }
        // SAFETY: setpgid names a pid this process spawned. Either it creates
        // the child's group or it loses the race to the child's own
        // setpgid/setsid, which is the same state; EACCES (the child already
        // exec'd) and EPERM (it is a session leader) are both "too late to
        // help", so the check below decides.
        unsafe { libc::setpgid(child, child) };
        // SAFETY: getpgid reads the group of a live pid.
        let group = unsafe { libc::getpgid(child) };
        if group < 0 {
            return Err(io::Error::last_os_error());
        }
        if group != child {
            return Err(io::Error::other(format!(
                "child {child} runs in process group {group}, not one of its own"
            )));
        }
        *self.group() = Some(child);
        Ok(())
    }

    /// Ask the whole group to stop. Nothing here waits: the bounded,
    /// forceful sweep is [`Self::terminate_and_wait`].
    pub fn terminate(&self) -> io::Result<()> {
        let mut guard = self.group();
        let Some(group) = *guard else {
            return Ok(());
        };
        if !leader_reserves(group) {
            // The leader is gone and reaped: the number is no longer ours to
            // signal, and a reused group id must not take this SIGTERM.
            *guard = None;
            return Ok(());
        }
        signal_group(group, libc::SIGTERM)
    }

    /// SIGTERM the group, give it `grace` to leave, then SIGKILL what is
    /// left and wait `grace` more for the kernel to clear it. A group that
    /// never empties is a timeout, exactly as the Windows job's bounded wait
    /// is; the child's own waiter thread is what reaps the leader, so no
    /// zombie is left to the caller. The whole sequence runs under the
    /// group lock, so a concurrent terminate on the same owner cannot signal
    /// a stale number between the check and the kill, and an owner whose
    /// leader has already been reaped signals nothing at all.
    pub fn terminate_and_wait(&self, grace: Duration) -> io::Result<()> {
        let mut guard = self.group();
        let Some(group) = *guard else {
            return Ok(());
        };
        if !leader_reserves(group) {
            *guard = None;
            return Ok(());
        }
        signal_group(group, libc::SIGTERM)?;
        if wait_for_empty_group(group, Instant::now() + grace) {
            *guard = None;
            return Ok(());
        }
        if !leader_reserves(group) {
            // Reaped during the grace: the id is no longer ours to signal,
            // so the forceful phase is refused rather than aimed blind.
            *guard = None;
            return Ok(());
        }
        signal_group(group, libc::SIGKILL)?;
        if wait_for_empty_group(group, Instant::now() + grace) {
            *guard = None;
            return Ok(());
        }
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "process group did not become empty before the deadline",
        ))
    }

    fn group(&self) -> MutexGuard<'_, Option<i32>> {
        self.group.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// The leader still reserves the group id: `kill(pid, 0)` succeeds only
/// while the pid is allocated — the child alive or a zombie — and the group
/// it leads is the one stored here. A leader that has been reaped leaves the
/// number free for the kernel to hand out again, and this owner must not
/// signal whatever now wears it.
fn leader_reserves(leader: i32) -> bool {
    // SAFETY: signal 0 only asks the kernel whether the pid exists.
    if unsafe { libc::kill(leader, 0) } != 0 {
        return false;
    }
    // SAFETY: getpgid reads a live or zombie pid's group.
    let group = unsafe { libc::getpgid(leader) };
    group == leader
}

fn invalid_pid(pid: u32) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        format!("a child pid is needed to own a process group, got {pid}"),
    )
}

/// A negative pid names the group. ESRCH means the group is already gone,
/// which is the goal, not a failure.
fn signal_group(group: i32, signal: i32) -> io::Result<()> {
    // SAFETY: kill takes a pid and a signal; `-group` with a positive group
    // id is the documented way to signal a process group.
    if unsafe { libc::kill(-group, signal) } == 0 {
        return Ok(());
    }
    let error = io::Error::last_os_error();
    if error.raw_os_error() == Some(libc::ESRCH) {
        return Ok(());
    }
    Err(error)
}

fn wait_for_empty_group(group: i32, deadline: Instant) -> bool {
    loop {
        // SAFETY: signal 0 only asks the kernel whether the group exists.
        if unsafe { libc::kill(-group, 0) } != 0 {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(POLL);
    }
}

/// The Windows twin is a duplicated kernel handle, so a liveness query cannot
/// be confused by a reused pid. Unix keeps the type — session code stores
/// `Option<ProcessHandle>` without a cfg at each use site — and never
/// constructs one: `os_handle` stays `None`, and the child's own EOF/`wait` is
/// the death signal.
#[cfg(feature = "server")]
#[derive(Debug)]
pub struct ProcessHandle;

#[cfg(feature = "server")]
impl ProcessHandle {
    pub fn is_alive(&self) -> bool {
        true
    }

    pub fn exit_code(&self) -> Option<u32> {
        None
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;
    use crate::process_tree::{contain_spawned, lead_own_group};
    use std::io::BufRead;
    use std::process::{Command, Stdio};
    use std::time::Duration;

    fn alive(pid: i32) -> bool {
        // SAFETY: signal 0 only asks the kernel whether the pid exists.
        unsafe { libc::kill(pid, 0) == 0 }
    }

    /// A tree, not a process: the shell spawns a grandchild that stays in the
    /// shell's group (a non-interactive shell has no job control to move it
    /// out), and the group kill has to take both and leave neither behind.
    #[test]
    fn a_killed_group_takes_the_child_and_its_grandchild() {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "sleep 300 & echo $!; wait"])
            .stdout(Stdio::piped());
        lead_own_group(&mut command);
        let mut child = command.spawn().expect("the shell spawns");
        let job = contain_spawned(child.id()).expect("the shell leads a group of its own");
        let stdout = child.stdout.take().expect("the pid line is piped");
        let mut line = String::new();
        std::io::BufReader::new(stdout)
            .read_line(&mut line)
            .expect("the grandchild pid");
        let grandchild: i32 = line.trim().parse().expect("a pid");
        assert!(alive(grandchild), "the grandchild is running");

        job.terminate_and_wait(Duration::from_secs(5))
            .expect("the group empties");

        // The killed grandchild is reparented and stays a zombie, which
        // signal 0 still sees, until init reaps it.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while alive(grandchild) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !alive(grandchild),
            "the grandchild went with the group it stayed in"
        );
        let _ = child.wait();
    }

    /// The safety half: a child that runs in the daemon's own group is
    /// refused rather than owned, so no kill can ever name the daemon's
    /// group. The child has exec'd (its line came back) before the assign.
    #[test]
    fn a_child_that_leads_no_group_of_its_own_is_refused() {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "echo ready; sleep 300"])
            .stdout(Stdio::piped())
            .spawn()
            .expect("the shell spawns");
        let stdout = child.stdout.take().expect("the ready line is piped");
        let mut line = String::new();
        std::io::BufReader::new(stdout)
            .read_line(&mut line)
            .expect("the ready line");
        assert_eq!(line.trim(), "ready");

        let job = JobObject::new().expect("an owner");
        let error = job
            .assign_group(child.id())
            .expect_err("a child in the daemon's group is not owned");
        assert!(error.to_string().contains("not one of its own"), "{error}");
        assert!(job.terminate().is_ok(), "an unowned job signals nothing");

        let _ = child.kill();
        let _ = child.wait();
    }

    /// The order the reuse guard exists for: while the leader is alive (or a
    /// zombie) the group id is reserved and the signal lands; once the leader
    /// is reaped the number is free again, and an owner that still holds it
    /// must refuse to signal — otherwise a reused id would take a SIGTERM
    /// meant for this tree.
    #[test]
    fn a_reaped_leader_leaves_the_owner_nothing_to_signal() {
        let mut command = Command::new("/bin/sh");
        command
            .args(["-c", "sleep 300 & echo $!; exit 0"])
            .stdout(Stdio::piped());
        lead_own_group(&mut command);
        let mut child = command.spawn().expect("the shell spawns");
        let job = contain_spawned(child.id()).expect("the shell leads a group of its own");
        let stdout = child.stdout.take().expect("the pid line is piped");
        let mut line = String::new();
        std::io::BufReader::new(stdout)
            .read_line(&mut line)
            .expect("the grandchild pid");
        let grandchild: i32 = line.trim().parse().expect("a pid");
        assert!(alive(grandchild), "the grandchild is running");

        // The leader exits on its own; the reap frees its pid.
        let _ = child.wait().expect("the leader is reaped");

        // The group still holds the grandchild, but the leader that reserved
        // the id is gone — this call must refuse rather than signal.
        job.terminate_and_wait(Duration::from_secs(2))
            .expect("a reaped leader refuses without error");
        assert!(
            alive(grandchild),
            "no signal went to the freed group id: the grandchild still runs"
        );
        // Ownership was cleared with the refusal: a later call stays quiet.
        job.terminate().expect("the cleared owner signals nothing");
        assert!(alive(grandchild), "the cleared owner never signals");

        // SAFETY: the test's own cleanup of the process it spawned.
        assert_eq!(unsafe { libc::kill(grandchild, libc::SIGKILL) }, 0);
    }
}
