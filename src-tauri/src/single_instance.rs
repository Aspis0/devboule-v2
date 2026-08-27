//! Single app-instance behavior for the GUI.
//!
//! The daemon guarantees that two processes never own the same sessions and
//! journal (`daemon.lock`, an OS byte-range lock). This module adds the
//! desktop-app behavior on top: one Devboule window per user session.
//!
//! The GUI holds `app.lock` (next to the daemon's `daemon.lock`) for its
//! lifetime, with the same lock primitive the daemon uses. A second app
//! instance fails to take it, brings the running window to the front, and
//! exits with code 0. Stale files are never fatal: the OS releases the
//! byte-range lock when the owning process dies, crashed or not.
//!
//! If the lock file itself cannot be touched (for example access denied),
//! instance enforcement degrades: the app logs the reason and continues
//! rather than refusing to start. The single-daemon guarantee lives in the
//! daemon and does not depend on this module.

use std::path::PathBuf;

use devboule_daemon::{DaemonError, RuntimePaths, SingleInstanceLock};

/// What `acquire` decided for this process.
pub enum StartupInstance {
    /// This process owns the single app slot. Hold the guard for the
    /// lifetime of the app; the OS releases the lock on exit.
    Acquired(AppInstance),
    /// Another Devboule window is already running.
    AlreadyRunning,
}

/// Held for the lifetime of the app. `lock` is `None` when enforcement
/// degraded (the lock file could not be created or locked for a reason other
/// than a live second instance) — the app still runs, just unguarded.
pub struct AppInstance {
    _lock: Option<SingleInstanceLock>,
}

/// Take the single app slot, or report that another instance owns it.
pub fn acquire() -> StartupInstance {
    let Some(lock_file) = app_lock_path() else {
        eprintln!(
            "Devboule could not locate its runtime folder, so it cannot \
             enforce single-instance behavior for this session."
        );
        return StartupInstance::Acquired(AppInstance { _lock: None });
    };
    if let Some(parent) = lock_file.parent() {
        if let Err(error) = std::fs::create_dir_all(parent) {
            eprintln!(
                "Devboule could not create its runtime folder at {} ({error}), \
                 so it cannot enforce single-instance behavior for this session.",
                parent.display()
            );
            return StartupInstance::Acquired(AppInstance { _lock: None });
        }
    }
    match SingleInstanceLock::acquire_at(&lock_file) {
        Ok(lock) => StartupInstance::Acquired(AppInstance { _lock: Some(lock) }),
        Err(DaemonError::AlreadyRunning) => StartupInstance::AlreadyRunning,
        Err(error) => {
            eprintln!(
                "Devboule could not lock {} ({error}), so it cannot enforce \
                 single-instance behavior for this session.",
                lock_file.display()
            );
            StartupInstance::Acquired(AppInstance { _lock: None })
        }
    }
}

fn app_lock_path() -> Option<PathBuf> {
    let paths = RuntimePaths::from_env().ok()?;
    Some(paths.dir.join("app.lock"))
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TempDir(PathBuf);
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn unique_dir() -> TempDir {
        let dir = std::env::temp_dir().join(format!(
            "devboule app-instance {}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        TempDir(dir)
    }

    #[test]
    fn app_lock_is_stale_safe() {
        // A crashed previous instance leaves a lock file behind. The OS has
        // already released its byte-range lock, so the next instance must
        // acquire it without spawning any process.
        let dir = unique_dir();
        let lock_file = dir.0.join("app.lock");
        std::fs::write(&lock_file, "pid=999999\ninstance=dead\n").expect("stale file");
        SingleInstanceLock::acquire_at(&lock_file).expect("stale lock must be lockable");
    }

    #[test]
    fn second_app_instance_is_detected_without_spawning_processes() {
        let dir = unique_dir();
        let lock_file = dir.0.join("app.lock");
        let _first = SingleInstanceLock::acquire_at(&lock_file).expect("first");
        match SingleInstanceLock::acquire_at(&lock_file) {
            Err(DaemonError::AlreadyRunning) => {}
            Ok(_) => panic!("second app lock succeeded"),
            Err(error) => panic!("expected AlreadyRunning, got {error}"),
        }
    }

    // `focus_existing_window` and `notify_already_running` need a real
    // window and a real second process; there is no honest headless test
    // for them, so there is none here.
}

/// Bring the running Devboule window to the front. Returns false when no
/// such window could be found (the caller then explains the situation).
#[cfg(windows)]
pub fn focus_existing_window() -> bool {
    focus_windows::focus_existing_window()
}

#[cfg(not(windows))]
pub fn focus_existing_window() -> bool {
    eprintln!("Devboule is already running; switch to its window.");
    false
}

/// Explain, in place of the missing window, that Devboule already runs.
#[cfg(windows)]
pub fn notify_already_running() {
    focus_windows::notify_already_running();
}

#[cfg(not(windows))]
pub fn notify_already_running() {
    eprintln!("Devboule is already running; switch to its window.");
}

#[cfg(windows)]
mod focus_windows {
    use std::path::Path;

    use windows_sys::Win32::Foundation::{CloseHandle, BOOL, HWND, LPARAM};
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
        MessageBoxW, SetForegroundWindow, ShowWindow, MB_ICONINFORMATION, MB_OK, MB_SETFOREGROUND,
        SW_RESTORE,
    };

    const WINDOW_TITLE: &str = "Devboule";
    const PROCESS_IMAGE: &str = "devboule.exe";

    struct WindowMatch {
        hwnd: HWND,
    }

    /// Find the first visible top-level window titled like Devboule that
    /// belongs to another running devboule.exe process. Title alone is not
    /// enough: any Explorer window over a folder named Devboule would match.
    pub fn focus_existing_window() -> bool {
        let mut found: Option<WindowMatch> = None;
        let found_ptr = &mut found as *mut Option<WindowMatch> as LPARAM;
        unsafe {
            EnumWindows(Some(enum_callback), found_ptr);
        }
        let Some(WindowMatch { hwnd }) = found else {
            return false;
        };
        unsafe {
            if IsIconic(hwnd) != 0 {
                ShowWindow(hwnd, SW_RESTORE);
            }
            SetForegroundWindow(hwnd);
        }
        true
    }

    pub fn notify_already_running() {
        let text: Vec<u16> = "Devboule is already running.\0".encode_utf16().collect();
        let caption: Vec<u16> = "Devboule\0".encode_utf16().collect();
        unsafe {
            MessageBoxW(
                std::ptr::null_mut(),
                text.as_ptr(),
                caption.as_ptr(),
                MB_OK | MB_ICONINFORMATION | MB_SETFOREGROUND,
            );
        }
    }

    unsafe extern "system" fn enum_callback(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let slot = unsafe { &mut *(lparam as *mut Option<WindowMatch>) };
        if slot.is_some() {
            return 1;
        }
        if unsafe { IsWindowVisible(hwnd) } == 0 {
            return 1;
        }
        let mut title = [0u16; 64];
        let len = unsafe { GetWindowTextW(hwnd, title.as_mut_ptr(), title.len() as i32) };
        let title = String::from_utf16_lossy(&title[..len.max(0) as usize]);
        if title != WINDOW_TITLE && !title.starts_with(WINDOW_TITLE) {
            return 1;
        }
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, &mut pid) };
        if pid == 0 || pid == std::process::id() {
            return 1;
        }
        if !process_image_is_devboule(pid) {
            return 1;
        }
        *slot = Some(WindowMatch { hwnd });
        1
    }

    fn process_image_is_devboule(pid: u32) -> bool {
        unsafe {
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
            if process.is_null() {
                return false;
            }
            let mut buffer = [0u16; 1024];
            let mut len = buffer.len() as u32;
            let ok = QueryFullProcessImageNameW(process, 0, buffer.as_mut_ptr(), &mut len);
            CloseHandle(process);
            if ok == 0 {
                return false;
            }
            Path::new(&String::from_utf16_lossy(&buffer[..len as usize]))
                .file_name()
                .is_some_and(|name| name.eq_ignore_ascii_case(PROCESS_IMAGE))
        }
    }
}
