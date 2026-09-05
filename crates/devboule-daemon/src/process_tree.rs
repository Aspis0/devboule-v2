//! Windows Job Object ownership for daemon and PTY process trees.
//!
//! The public API is deliberately tiny. `portable-pty` owns the actual
//! process creation, but its Windows `Child` exposes the native process
//! handle, which is all the Job Object API needs.

#[cfg(windows)]
mod platform {
    use std::io;
    use std::mem;
    use std::os::windows::io::RawHandle;
    use std::ptr;

    use windows_sys::Win32::Foundation::{
        CloseHandle, DuplicateHandle, DUPLICATE_SAME_ACCESS, HANDLE, STILL_ACTIVE, WAIT_OBJECT_0,
        WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
    };
    use windows_sys::Win32::System::Threading::{
        GetCurrentProcess, GetExitCodeProcess, ResumeThread, WaitForSingleObject,
    };

    /// An owned Job Object configured to kill its members when this handle is
    /// closed. The handle is intentionally not duplicated: the owner is the
    /// daemon state for the daemon job and the live session for a session job.
    #[derive(Debug)]
    pub struct JobObject {
        handle: HANDLE,
    }

    // A Job Object handle is a process-wide kernel capability. Assigning
    // processes from multiple session-spawn threads is safe, and the handle
    // remains valid until this owner is dropped.
    unsafe impl Send for JobObject {}
    unsafe impl Sync for JobObject {}

    impl JobObject {
        pub fn new() -> io::Result<Self> {
            let handle = unsafe { CreateJobObjectW(ptr::null(), ptr::null()) };
            if handle.is_null() {
                return Err(io::Error::last_os_error());
            }

            let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { mem::zeroed() };
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
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

            Ok(Self { handle })
        }

        pub fn assign(&self, process: RawHandle) -> io::Result<()> {
            let result = unsafe { AssignProcessToJobObject(self.handle, process as HANDLE) };
            if result == 0 {
                return Err(io::Error::last_os_error());
            }
            Ok(())
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

    impl Drop for JobObject {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.handle) };
        }
    }

    /// Owned duplicate of a process handle. Held so liveness can be queried
    /// without a PID, which the OS may reuse after the original process dies.
    #[derive(Debug)]
    pub struct ProcessHandle {
        handle: HANDLE,
    }

    unsafe impl Send for ProcessHandle {}
    unsafe impl Sync for ProcessHandle {}

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

    impl Drop for ProcessHandle {
        fn drop(&mut self) {
            if !self.handle.is_null() {
                unsafe { CloseHandle(self.handle) };
            }
        }
    }
}

#[cfg(not(windows))]
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
    }

    /// Unix keeps the type so session code can store `Option<ProcessHandle>`
    /// without cfg on the struct. This milestone observes liveness on Windows.
    #[derive(Debug)]
    pub struct ProcessHandle;

    impl ProcessHandle {
        pub fn is_alive(&self) -> bool {
            true
        }

        pub fn exit_code(&self) -> Option<u32> {
            None
        }
    }
}

pub use platform::{JobObject, ProcessHandle};

#[cfg(all(test, windows))]
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
