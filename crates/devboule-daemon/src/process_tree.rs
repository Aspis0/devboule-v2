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

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
        SetInformationJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
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
    }

    impl Drop for JobObject {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.handle) };
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
}

pub use platform::JobObject;
