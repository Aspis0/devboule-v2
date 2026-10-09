//! Windows Job Object ownership for daemon and PTY process trees.
//!
//! The public API is deliberately tiny. `portable-pty` owns the actual
//! process creation, but its Windows `Child` exposes the native process
//! handle, which is all the Job Object API needs.

#[cfg(all(windows, feature = "server"))]
#[path = "process_tree_attached.rs"]
mod attached;

#[cfg(windows)]
#[path = "process_tree_job_list.rs"]
mod job_list;

#[cfg(windows)]
mod platform {
    use std::io;
    use std::mem;
    use std::os::windows::io::RawHandle;
    use std::ptr;
    use std::thread;
    use std::time::{Duration, Instant};

    #[cfg(feature = "server")]
    use super::attached::{self, CapturedTree};
    #[cfg(feature = "server")]
    use crate::process_index::read_creation_time;

    // `CloseHandle` and `HANDLE` serve `JobObject` in every build. The
    // remaining names are used only by `ProcessHandle`, which is behind
    // `server`, so they carry the same gate rather than sitting here as
    // unused imports in a GUI-only build.
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    #[cfg(feature = "server")]
    use windows_sys::Win32::Foundation::{
        DuplicateHandle, DUPLICATE_SAME_ACCESS, STILL_ACTIVE, WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::JobObjects::JobObjectBasicProcessIdList;
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation,
        JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
        TerminateJobObject, JOBOBJECT_BASIC_ACCOUNTING_INFORMATION,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::ResumeThread;
    #[cfg(feature = "server")]
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetExitCodeProcess, WaitForSingleObject,
    };
    #[cfg(feature = "server")]
    use windows_sys::Win32::System::Threading::{OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION};

    /// Which processes a kill through a job reaches. A whole-job scope owns
    /// everything its provider starts. An attached scope is a terminal's shell:
    /// only the tree still attached to that root is a kill target, because a
    /// process the shell detached is outside the terminal's life.
    #[cfg_attr(not(feature = "server"), allow(dead_code))]
    #[derive(Debug)]
    enum Scope {
        Whole,
        Attached { root: u32, created_at: Option<u64> },
    }

    /// An owned Job Object. Every job is created empty for exactly one child
    /// tree, and the handle is intentionally not duplicated: its owner is the
    /// code that spawned that tree (the live session, the git probe, the npm
    /// install, the version probe, the ACP terminal).
    #[derive(Debug)]
    pub struct JobObject {
        handle: HANDLE,
        scope: Scope,
    }

    // A Job Object handle is a process-wide kernel capability. Assigning
    // processes from multiple session-spawn threads is safe, and the handle
    // remains valid until this owner is dropped.
    unsafe impl Send for JobObject {}
    unsafe impl Sync for JobObject {}

    impl JobObject {
        pub fn new() -> io::Result<Self> {
            Self::create(Scope::Whole)
        }

        /// A job for a terminal's shell. It records membership and kills nothing
        /// on close, so a process the shell detached outlives the terminal.
        pub fn attached(root: u32) -> io::Result<Self> {
            Self::create(Scope::Attached {
                root,
                // Without the server nothing captures, so no clock is recorded.
                #[cfg(feature = "server")]
                created_at: root_creation_time(root),
                #[cfg(not(feature = "server"))]
                created_at: None,
            })
        }

        /// A terminal's job by opener: an agent's terminal owns the whole job so
        /// nothing it started survives the close; a human's only records membership.
        pub fn terminal(root: u32, created_by: Option<&str>) -> io::Result<Self> {
            if created_by.is_some() {
                Self::new()
            } else {
                Self::attached(root)
            }
        }

        fn create(scope: Scope) -> io::Result<Self> {
            let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }

            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { mem::zeroed() };
            if matches!(scope, Scope::Whole) {
                limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            }
            let result = unsafe {
                SetInformationJobObject(
                    handle,
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
                )
            };
            if result == 0 {
                let error = io::Error::last_os_error();
                unsafe { CloseHandle(handle) };
                return Err(error);
            }

            Ok(Self { handle, scope })
        }

        pub fn assign(&self, process: RawHandle) -> io::Result<()> {
            let result = unsafe { AssignProcessToJobObject(self.handle, process as HANDLE) };
            if result == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }

        /// Kill every process in the job without closing the handle. Used when
        /// OS liveness observes the root is dead but descendants still hold
        /// pipes open (so EOF never arrives).
        pub fn terminate(&self) -> io::Result<()> {
            let result = unsafe { TerminateJobObject(self.handle, 1) };
            if result == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
        }

        /// Terminate the complete tree and wait until the Job Object reports
        /// no active members. `Child::kill` only targets the wrapper process;
        /// this is the bounded cleanup path for helpers such as Git-for-
        /// Windows' `cmd\\git.exe` launcher.
        pub fn terminate_and_wait(&self, timeout: Duration) -> io::Result<()> {
            self.terminate()?;
            let deadline = Instant::now() + timeout;
            loop {
                let mut accounting = JOBOBJECT_BASIC_ACCOUNTING_INFORMATION::default();
                let ok = unsafe {
                    QueryInformationJobObject(
                        self.handle,
                        JobObjectBasicAccountingInformation,
                        (&mut accounting as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                        mem::size_of::<JOBOBJECT_BASIC_ACCOUNTING_INFORMATION>() as u32,
                        ptr::null_mut(),
                    )
                };
                if ok == 0 {
                    return Err(io::Error::last_os_error());
                }
                if accounting.ActiveProcesses == 0 {
                    return Ok(());
                }
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "job object did not become empty before the deadline",
                    ));
                }
                thread::sleep(Duration::from_millis(10));
            }
        }

        /// The tree a stop must end, read while the root still lives. A child that
        /// outlives its parent is detached and drops out of the tree, so a capture
        /// taken after the root exits would miss what was still attached.
        /// A child spawned after the second capture still escapes; closing that
        /// would need the root suspended.
        #[cfg(feature = "server")]
        pub fn capture_tree(&self) -> CapturedTree {
            match self.scope {
                Scope::Whole => CapturedTree::default(),
                Scope::Attached { root, created_at } => match self.pids() {
                    Ok(members) => attached::capture(root, created_at, &members, &|| {
                        self.pids().unwrap_or_default()
                    }),
                    Err(error) => {
                        eprintln!(
                            "could not read the terminal's processes, so only its shell is stopped: {error}"
                        );
                        CapturedTree::default()
                    }
                },
            }
        }

        /// Ends the captured tree without waiting. A whole-job scope ends through
        /// the job, the kill its owner has always made.
        #[cfg(feature = "server")]
        pub fn kill_tree(&self, tree: &CapturedTree) -> io::Result<()> {
            match self.scope {
                Scope::Whole => self.terminate(),
                Scope::Attached { .. } => {
                    tree.terminate();
                    // A child born between the capture and this kill has a parent
                    // in the captured set, so one second pass takes it too.
                    if let Ok(members) = self.pids() {
                        attached::capture_newcomers(tree, &members).terminate();
                    }
                    Ok(())
                }
            }
        }

        /// `kill_tree`, then a bounded wait until the captured processes exit.
        #[cfg(feature = "server")]
        pub fn terminate_tree_and_wait(
            &self,
            tree: &CapturedTree,
            timeout: Duration,
        ) -> io::Result<()> {
            match self.scope {
                Scope::Whole => self.terminate_and_wait(timeout),
                Scope::Attached { .. } => {
                    let deadline = Instant::now() + timeout;
                    tree.terminate();
                    let newcomers = self
                        .pids()
                        .map(|members| attached::capture_newcomers(tree, &members))
                        .unwrap_or_default();
                    newcomers.terminate();
                    tree.wait(deadline)?;
                    newcomers.wait(deadline)
                }
            }
        }

        /// The PIDs currently assigned to this job: the proof that a process
        /// belongs to this tree, read straight from the kernel object rather
        /// than from a machine-wide image name, so a co-tenant process cannot
        /// pass as a member.
        pub fn pids(&self) -> io::Result<Vec<u32>> {
            super::job_list::read_pid_list(|buffer| self.query_pid_list(buffer))
        }

        /// One `JobObjectBasicProcessIdList` answer into the buffer. A buffer
        /// too small for the answer is grown to the size the kernel asked for,
        /// a bounded number of times; the list's own count is judged by the
        /// caller, because a short answer is not an error here.
        fn query_pid_list(&self, buffer: &mut Vec<u8>) -> io::Result<()> {
            // ERROR_INSUFFICIENT_BUFFER and ERROR_MORE_DATA both mean "the
            // size in return_length is what the next read needs".
            const ERROR_INSUFFICIENT_BUFFER: i32 = 122;
            const ERROR_MORE_DATA: i32 = 234;
            for _ in 0..super::job_list::JOB_LIST_ATTEMPTS {
                let mut return_length = 0u32;
                let ok = unsafe {
                    QueryInformationJobObject(
                        self.handle,
                        JobObjectBasicProcessIdList,
                        buffer.as_mut_ptr().cast(),
                        buffer.len() as u32,
                        &mut return_length,
                    )
                };
                if ok != 0 {
                    return Ok(());
                }
                let error = io::Error::last_os_error();
                if !matches!(
                    error.raw_os_error(),
                    Some(ERROR_INSUFFICIENT_BUFFER | ERROR_MORE_DATA)
                ) {
                    return Err(error);
                }
                buffer.resize(return_length as usize, 0);
            }
            Err(io::Error::other(
                "the job list kept needing more room than it was given",
            ))
        }

        /// Assign a process created with CREATE_SUSPENDED, then resume its
        /// initial thread. The child cannot execute user code before it is in
        /// the job, so there is no spawn-then-assign escape window.
        pub fn assign_suspended(
            &self,
            process: RawHandle,
            initial_thread: RawHandle,
        ) -> io::Result<()> {
            self.assign(process)?;
            resume_initial_thread(initial_thread)
        }
    }

    fn resume_initial_thread(initial_thread: RawHandle) -> io::Result<()> {
        let resumed = unsafe { ResumeThread(initial_thread as HANDLE) };
        if resumed == u32::MAX {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    /// The root's creation time at spawn, the identity a later open proves
    /// against. `None` when the kernel would not vouch for it even then.
    #[cfg(feature = "server")]
    fn root_creation_time(pid: u32) -> Option<u64> {
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            return None;
        }
        let created_at = read_creation_time(handle);
        unsafe { CloseHandle(handle) };
        created_at
    }

    impl Drop for JobObject {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.handle) };
        }
    }

    /// Owned duplicate of a process handle. Held so liveness can be queried
    /// without a PID, which the OS may reuse after the original process dies.
    ///
    /// Only the `server` feature constructs one: the PTY session and the agent
    /// clients hold it for liveness queries. A GUI-only build has no session
    /// code, so it does not see the type at all.
    #[cfg(feature = "server")]
    #[derive(Debug)]
    pub struct ProcessHandle {
        handle: HANDLE,
    }

    #[cfg(feature = "server")]
    unsafe impl Send for ProcessHandle {}
    #[cfg(feature = "server")]
    unsafe impl Sync for ProcessHandle {}

    #[cfg(feature = "server")]
    impl ProcessHandle {
        pub fn duplicate(source: RawHandle) -> io::Result<Self> {
            if source.is_null() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "null process handle",
                ));
            }
            let mut dest = ptr::null_mut();
            let ok = unsafe {
                DuplicateHandle(
                    GetCurrentProcess(),
                    source as HANDLE,
                    GetCurrentProcess(),
                    &mut dest,
                    0,
                    0,
                    DUPLICATE_SAME_ACCESS,
                )
            };
            if ok == 0 || dest.is_null() {
                return Err(io::Error::last_os_error());
            }
            Ok(Self { handle: dest })
        }

        /// Non-blocking OS query. `WAIT_TIMEOUT` means the process object is
        /// still unsignaled (alive). `WAIT_OBJECT_0` means it has exited.
        /// A failed wait does not claim death: the wait-thread/EOF path still
        /// reaps the child, and a false Finished is worse than a delayed one.
        pub fn is_alive(&self) -> bool {
            match unsafe { WaitForSingleObject(self.handle, 0) } {
                WAIT_OBJECT_0 => false,
                WAIT_TIMEOUT => true,
                _ => true,
            }
        }

        pub fn exit_code(&self) -> Option<u32> {
            if self.is_alive() {
                return None;
            }
            let mut code = 0u32;
            let ok = unsafe { GetExitCodeProcess(self.handle, &mut code) };
            if ok == 0 || code == STILL_ACTIVE as u32 {
                None
            } else {
                Some(code)
            }
        }
    }

    #[cfg(feature = "server")]
    impl Drop for ProcessHandle {
        fn drop(&mut self) {
            if !self.handle.is_null() {
                unsafe { CloseHandle(self.handle) };
            }
        }
    }
}

#[cfg(unix)]
#[path = "process_tree_unix.rs"]
mod platform;

#[cfg(all(not(windows), not(unix)))]
mod platform {
    use std::io;

    /// Unix keeps the same ownership shape so the daemon remains buildable
    /// for client/library tests. Unix PTY process-group containment is not
    /// part of this Windows-only daemon milestone.
    #[derive(Debug)]
    pub struct JobObject;

    impl JobObject {
        pub fn new() -> io::Result<Self> {
            Ok(Self)
        }

        pub fn terminate(&self) -> io::Result<()> {
            Ok(())
        }

        pub fn terminate_and_wait(&self, _timeout: std::time::Duration) -> io::Result<()> {
            Ok(())
        }

        #[cfg(feature = "server")]
        pub fn capture_tree(&self) -> CapturedTree {
            CapturedTree
        }

        #[cfg(feature = "server")]
        pub fn kill_tree(&self, _tree: &CapturedTree) -> io::Result<()> {
            self.terminate()
        }

        #[cfg(feature = "server")]
        pub fn terminate_tree_and_wait(
            &self,
            _tree: &CapturedTree,
            timeout: std::time::Duration,
        ) -> io::Result<()> {
            self.terminate_and_wait(timeout)
        }
    }

    #[cfg(feature = "server")]
    #[derive(Debug, Default)]
    pub struct CapturedTree;

    /// Unix keeps the type so session code can store `Option<ProcessHandle>`
    /// without a cfg at each use site. It exists under `server`, the only
    /// build that has session code; liveness is observed on Windows only.
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
}

/// Make a piped child the leader of its own process group. Windows contains a
/// child by assigning it to a job after spawn; a Unix process cannot be moved
/// into another group once it has exec'd, so the group is chosen before it
/// runs. A platform with neither has nothing to do here.
#[cfg(not(windows))]
pub fn lead_own_group(command: &mut std::process::Command) {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(not(unix))]
    let _ = command;
}

/// Own a just-spawned child's own process group: on Unix the child leads it
/// ([`lead_own_group`], or portable-pty's own `setsid`), and elsewhere there
/// is nothing to own.
#[cfg(not(windows))]
pub fn contain_spawned(pid: u32) -> std::io::Result<JobObject> {
    let job = JobObject::new()?;
    #[cfg(unix)]
    job.assign_group(pid)?;
    #[cfg(not(unix))]
    let _ = pid;
    Ok(job)
}

// `JobObject` stays unconditional: lib.rs re-exports it as public API, so a
// GUI-only build can still name the type and nothing here is dead. The only
// reachable paths to `ProcessHandle` run through the `server` session modules,
// so both this re-export and the definition above carry the same gate; without
// it the re-export is the dead import clippy reports and the struct is
// unreachable. The guard test needs `server` for the same reason.
pub use platform::JobObject;
#[cfg(feature = "server")]
pub use platform::ProcessHandle;

#[cfg(all(test, windows, feature = "server"))]
#[path = "process_tree_tree_tests.rs"]
mod tree_tests;

#[cfg(all(test, windows, feature = "server"))]
mod tests {
    use super::ProcessHandle;
    use std::os::windows::io::AsRawHandle;
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    fn spawn_innocuous() -> std::process::Child {
        Command::new("cmd.exe")
            .args(["/d", "/c", "ping", "-n", "30", "127.0.0.1"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .expect("spawn innocuous ping")
    }

    // Deliberately NOT ignored, unlike the two Git probe tests parked in the
    // same pass. Those were measured red on the GitHub runner; this one has
    // been running and passing there since before the workspace slice existed.
    // Parking a test because it belongs to the same category as two that
    // failed trades away working coverage for a symmetry nobody asked for —
    // and this file is shared with PTY session lifetime.
    #[test]
    fn os_query_reports_alive_then_exited_after_kill() {
        let mut child = spawn_innocuous();
        let handle = ProcessHandle::duplicate(child.as_raw_handle()).expect("duplicate handle");
        assert!(
            handle.is_alive(),
            "a just-spawned process must be observed alive"
        );
        child.kill().expect("kill ping");
        let _ = child.wait();
        assert!(
            !handle.is_alive(),
            "WaitForSingleObject on the duplicated handle must observe the kill; a PID is not enough"
        );
    }
}
