//! The processes a terminal's shell still holds, captured while the shell lives.
//! A kill through these handles cannot be redirected by a PID the OS reuses
//! later, and a process the shell detached is never in the set.

use std::collections::{HashMap, HashSet};
use std::io;
use std::time::Instant;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows_sys::Win32::System::Threading::{
    OpenProcess, TerminateProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
};

use crate::process_index::{process_parents, read_creation_time};

/// Handles on the captured processes, in capture order: the root first, then
/// each descendant after its parent.
#[derive(Debug, Default)]
pub struct CapturedTree {
    members: Vec<Held>,
}

#[derive(Debug)]
struct Held {
    pid: u32,
    handle: HANDLE,
    created_at: u64,
}

impl Held {
    /// `None` when the process is gone or its handle cannot be opened.
    /// An exited process stays openable while handles to it are outstanding,
    /// so only a still-live process opens: a dead root is gone even when the
    /// daemon's own handles keep its object around, and everything under it
    /// is detached by definition.
    fn open(pid: u32) -> Option<Self> {
        let handle = unsafe {
            OpenProcess(
                PROCESS_TERMINATE | PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                0,
                pid,
            )
        };
        if handle.is_null() {
            return None;
        }
        if unsafe { WaitForSingleObject(handle, 0) } != WAIT_TIMEOUT {
            unsafe { CloseHandle(handle) };
            return None;
        }
        match read_creation_time(handle) {
            Some(created_at) => Some(Self {
                pid,
                handle,
                created_at,
            }),
            None => {
                unsafe { CloseHandle(handle) };
                None
            }
        }
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.handle) };
    }
}

/// The root and its live descendants that are members of the job. A child must
/// have been created no earlier than its parent, so a PID the OS reused for an
/// unrelated process is not followed through a stale parent link.
pub(super) fn capture(
    root: u32,
    expected_root_created_at: Option<u64>,
    job_members: &[u32],
    fresh_members: &dyn Fn() -> Vec<u32>,
) -> CapturedTree {
    let mut tree = CapturedTree::default();
    let Some(parents) = process_parents() else {
        eprintln!("the process snapshot is unavailable, so only the terminal's shell is stopped");
        return tree;
    };
    let members: HashSet<u32> = job_members.iter().copied().collect();
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (&pid, &ppid) in &parents {
        if members.contains(&pid) {
            children.entry(ppid).or_default().push(pid);
        }
    }
    let Some(root_held) = Held::open(root) else {
        eprintln!("could not open the terminal's shell, so only its shell is stopped");
        return tree;
    };
    // The spawn-time identity: a reused PID opens as its successor, so a root
    // whose clock disagrees with the spawn's is not the shell.
    if expected_root_created_at != Some(root_held.created_at) {
        eprintln!("the terminal's shell is not the spawned process, so only its shell is stopped");
        return tree;
    }
    tree.members.push(root_held);
    let mut next = 0;
    while next < tree.members.len() {
        let (parent, parent_created_at) = (tree.members[next].pid, tree.members[next].created_at);
        next += 1;
        for &child in children.get(&parent).into_iter().flatten() {
            if tree.members.iter().any(|held| held.pid == child) {
                continue;
            }
            let Some(held) = Held::open(child) else {
                continue;
            };
            // The snapshot predates the open: a PID reused in between opens as
            // its successor, so membership and the clock are re-proved here.
            if !fresh_members().contains(&held.pid) {
                continue;
            }
            if held.created_at < parent_created_at {
                continue;
            }
            tree.members.push(held);
        }
    }
    tree
}

/// Members that appeared after a capture and whose parent chain leads into
/// it: the second pass after the first kill, so a child born in the window
/// does not escape with its parent dead.
pub(super) fn capture_newcomers(
    captured: &CapturedTree,
    job_members: &[u32],
) -> CapturedTree {
    let mut newcomers = CapturedTree::default();
    let Some(parents) = process_parents() else {
        return newcomers;
    };
    let members: HashSet<u32> = job_members.iter().copied().collect();
    let mut times = captured.member_times();
    let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
    for (&pid, &ppid) in &parents {
        if members.contains(&pid) {
            children.entry(ppid).or_default().push(pid);
        }
    }
    let mut queue: Vec<u32> = times.keys().copied().collect();
    let mut next = 0;
    while next < queue.len() {
        let parent = queue[next];
        next += 1;
        let Some(&parent_created_at) = times.get(&parent) else {
            continue;
        };
        for &child in children.get(&parent).into_iter().flatten() {
            if times.contains_key(&child) {
                continue;
            }
            let Some(held) = Held::open(child) else {
                continue;
            };
            if held.created_at < parent_created_at {
                continue;
            }
            times.insert(held.pid, held.created_at);
            queue.push(held.pid);
            newcomers.members.push(held);
        }
    }
    newcomers
}

impl CapturedTree {
    pub(super) fn terminate(&self) {
        for held in &self.members {
            unsafe { TerminateProcess(held.handle, 1) };
        }
    }

    /// One capture's clock table, for the second pass to prove newcomers against.
    fn member_times(&self) -> HashMap<u32, u64> {
        self.members
            .iter()
            .map(|held| (held.pid, held.created_at))
            .collect()
    }

    /// Waits until every captured process has exited, or the deadline passes.
    pub(super) fn wait(&self, deadline: Instant) -> io::Result<()> {
        for held in &self.members {
            let remaining = deadline
                .saturating_duration_since(Instant::now())
                .as_millis()
                .min(u32::MAX as u128) as u32;
            match unsafe { WaitForSingleObject(held.handle, remaining) } {
                WAIT_OBJECT_0 => {}
                WAIT_TIMEOUT => {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!("process {} did not exit before the deadline", held.pid),
                    ))
                }
                _ => return Err(io::Error::last_os_error()),
            }
        }
        Ok(())
    }
}
