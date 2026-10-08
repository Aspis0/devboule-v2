//! The stop contract for a job's tree: a whole-job kill takes a child its root
//! detached, while an attached scope takes only what is still attached to its root.

use std::os::windows::io::AsRawHandle;
use std::os::windows::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, TerminateProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
};

use super::JobObject;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const DEADLINE: Duration = Duration::from_secs(10);

/// Ends a test process when the test ends, even on a failed assertion. The
/// handle is opened when the PID is learned, so the kill cannot reach a PID the
/// OS reused after the process exited. Errors are ignored: a panic in `Drop`
/// during an unwind aborts.
struct KillOnDrop(Option<HANDLE>);

impl KillOnDrop {
    fn of(pid: u32) -> Self {
        let handle = unsafe { OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, 0, pid) };
        Self((!handle.is_null()).then_some(handle))
    }
}

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        if let Some(handle) = self.0 {
            unsafe {
                TerminateProcess(handle, 1);
                CloseHandle(handle);
            }
        }
    }
}

fn spawn_cmd(args: &[&str]) -> Child {
    Command::new("cmd.exe")
        .arg("/d")
        .arg("/c")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .expect("spawn cmd.exe")
}

/// The job's members are not only the processes the test starts: cmd also brings
/// its console host, so a count of members does not identify PING.
fn is_ping(pid: u32) -> bool {
    let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if handle.is_null() {
        return false;
    }
    let mut buffer = [0u16; 512];
    let mut size = buffer.len() as u32;
    let ok = unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut size) };
    unsafe { CloseHandle(handle) };
    ok != 0
        && String::from_utf16_lossy(&buffer[..size as usize])
            .to_ascii_lowercase()
            .ends_with("\\ping.exe")
}

/// Polls the job's member list until `ready` holds, failing at the deadline.
fn wait_for_members(job: &JobObject, ready: impl Fn(&[u32]) -> bool) -> Vec<u32> {
    let deadline = Instant::now() + DEADLINE;
    loop {
        let members = job.pids().expect("read job members");
        if ready(&members) {
            return members;
        }
        assert!(
            Instant::now() < deadline,
            "job members never reached the expected state, last seen: {members:?}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_for_ping(job: &JobObject) -> u32 {
    let members = wait_for_members(job, |members| members.iter().any(|&pid| is_ping(pid)));
    *members
        .iter()
        .find(|&&pid| is_ping(pid))
        .expect("a PING member")
}

/// `cmd /c start /b ping` makes PING a child of a cmd that exits at once, so
/// PING is detached: its parent is gone but it is still a member of the job.
#[test]
fn whole_job_kill_takes_a_detached_child() {
    let job = JobObject::new().expect("whole job");
    let mut root = spawn_cmd(&["start", "/b", "ping", "-n", "60", "127.0.0.1"]);
    job.assign(root.as_raw_handle()).expect("assign root");
    root.wait().expect("reap root");
    let ping = wait_for_ping(&job);
    let _ping_guard = KillOnDrop::of(ping);

    let tree = job.capture_tree();
    job.terminate_tree_and_wait(&tree, DEADLINE)
        .expect("whole-job tree ends");
    wait_for_members(&job, |members| !members.contains(&ping));
}

#[test]
fn attached_stop_spares_a_child_its_root_detached() {
    let mut root = spawn_cmd(&["start", "/b", "ping", "-n", "60", "127.0.0.1"]);
    let job = JobObject::attached(root.id()).expect("attached job");
    job.assign(root.as_raw_handle()).expect("assign root");
    root.wait().expect("reap root");
    let ping = wait_for_ping(&job);
    let _ping_guard = KillOnDrop::of(ping);

    let tree = job.capture_tree();
    job.kill_tree(&tree).expect("attached stop");

    assert!(
        job.pids().expect("read job members").contains(&ping),
        "stopping the terminal ended a process its root had already detached"
    );
}

#[test]
fn attached_stop_kills_a_child_still_attached_to_its_root() {
    let mut root = spawn_cmd(&["ping", "-n", "60", "127.0.0.1"]);
    let root_pid = root.id();
    let _root_guard = KillOnDrop::of(root_pid);
    let job = JobObject::attached(root_pid).expect("attached job");
    job.assign(root.as_raw_handle()).expect("assign root");
    let ping = wait_for_ping(&job);
    let _ping_guard = KillOnDrop::of(ping);

    let tree = job.capture_tree();
    let _ = root.kill();
    root.wait().expect("reap root");
    job.kill_tree(&tree).expect("attached stop");

    wait_for_members(&job, |members| !members.contains(&ping));
}
