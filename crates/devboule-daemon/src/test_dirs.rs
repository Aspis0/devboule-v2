//! The one place this crate's test code asks the OS for its temp dir:
//! `test_temp_dir(prefix)` gives the test a fresh directory under this run's
//! own root, `%TEMP%\devboule-tests\<pid>\`, composed as a pid +
//! nanosecond + counter name and created with `create_dir`, which refuses
//! anything already there. Nothing is removed while the run lives — on the
//! first call in a process the helper sweeps the roots of the runs whose pid
//! is no longer alive, so a run clears the runs before it.
//! `temp_dir_guard_tests.rs` enforces that this is the only such place by
//! scanning every other file for the token — its header lists the
//! spellings it matches and the spellings it does not see.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

/// Every run's directories under one name in the system temp dir, so the
/// sweep can walk a fixed, known directory and nothing else.
fn runs_root() -> PathBuf {
    std::env::temp_dir().join("devboule-tests")
}

/// This run's own root: `<runs root>\<pid>`, created on first use.
fn run_root() -> PathBuf {
    let root = runs_root().join(std::process::id().to_string());
    std::fs::create_dir_all(&root).expect("this run's temp root");
    root
}

/// Best effort: whether `pid` is gone, read conservatively — only a pid this
/// machine can positively read as dead comes back `true`, so an unreadable
/// pid keeps its directory rather than losing a run that may be alive.
#[cfg(windows)]
fn is_dead(pid: u32) -> bool {
    use windows_sys::Win32::Foundation::{CloseHandle, GetLastError, ERROR_INVALID_PARAMETER};
    use windows_sys::Win32::System::Threading::{
        GetExitCodeProcess, OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION,
    };
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if handle.is_null() {
            // Only “no such pid” reads as dead; every other failure is an
            // unreadable process, which may still be running.
            return GetLastError() == ERROR_INVALID_PARAMETER;
        }
        let mut code: u32 = 0;
        let queried = GetExitCodeProcess(handle, &mut code);
        CloseHandle(handle);
        // 259 is STILL_ACTIVE.
        queried != 0 && code != 259
    }
}

#[cfg(unix)]
fn is_dead(pid: u32) -> bool {
    // `kill(pid, 0)` answers for any process: 0 or EPERM means it is there,
    // ESRCH is the only reading that means it is gone.
    let asked = unsafe { libc::kill(pid as i32, 0) };
    asked != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
}

/// One sweep per process: the first test to ask for a directory clears the
/// roots of the runs whose pid is gone.
fn sweep_dead_runs() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| sweep_runs_under(&runs_root(), std::process::id()));
}

/// The sweep itself, as a function of the root it walks and the pid whose
/// root it must keep: it reads only `root`'s own children, skips anything
/// that is not a pid-named directory, and ignores every error — a directory
/// it cannot remove stays for the next run to try.
fn sweep_runs_under(root: &Path, me: u32) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        let name = entry.file_name();
        let Ok(pid) = name.to_string_lossy().parse::<u32>() else {
            continue;
        };
        if pid == me || !is_dead(pid) {
            continue;
        }
        if std::fs::remove_dir_all(&path).is_err() {
            force_remove(&path);
        }
    }
}

/// A folder the attachment store hardened can sit under a DACL that grants
/// nobody (a protected parent hands its children nothing), and no later run
/// can delete it as it stands: re-grant this user level by level, then remove.
/// Every step ignores errors — what still cannot go stays for the next run.
#[cfg(windows)]
fn force_remove(path: &Path) {
    let _ = crate::security::apply_current_user_dacl(path);
    let Ok(entries) = std::fs::read_dir(path) else {
        let _ = std::fs::remove_file(path);
        return;
    };
    for entry in entries.flatten() {
        force_remove(&entry.path());
    }
    let _ = std::fs::remove_dir_all(path);
}

#[cfg(not(windows))]
fn force_remove(_path: &Path) {}

/// The name, composed as a pure function: the same inputs give the same
/// name, so a test can pin the counter's role on one fixed nanosecond
/// reading without racing a clock — the pid is what separates runs. The
/// prefix names a child of the system temp dir, so a separator or `..` in
/// it would step outside — refused here, where it is composed.
fn temp_dir_name(prefix: &str, pid: u32, nanos: u128, counter: u64) -> String {
    assert!(
        !prefix.contains(['/', '\\']) && !prefix.contains(".."),
        "prefix {prefix:?} must not contain a path separator or .."
    );
    format!("{prefix}-{pid}-{nanos}-{counter}")
}

/// Create exactly `name` under `parent`. `create_dir`, never
/// `create_dir_all`: an existing entry — a directory an earlier run left
/// behind, or a file squatting on the name — is an error, never a
/// starting point.
fn try_create_test_dir_in(parent: &Path, name: &str) -> std::io::Result<PathBuf> {
    let path = parent.join(name);
    std::fs::create_dir(&path)?;
    Ok(path)
}

/// The refusal callers panic on, naming the path and both causes Windows
/// maps to one error kind: an inherited directory, or a file on the name.
fn create_test_dir_in(parent: &Path, name: &str) -> PathBuf {
    match try_create_test_dir_in(parent, name) {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => panic!(
            "refusing temp dir {}: it already exists (inherited from an earlier run, or a file holds this name)",
            parent.join(name).display()
        ),
        Err(error) => panic!(
            "creating temp dir {}: {error}",
            parent.join(name).display()
        ),
    }
}

fn fresh_name(prefix: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    temp_dir_name(
        prefix,
        std::process::id(),
        nanos,
        COUNTER.fetch_add(1, Ordering::Relaxed),
    )
}

/// A fresh temp dir no other run can inherit: `<run root>\prefix-pid-nanos-counter`.
pub fn test_temp_dir(prefix: &str) -> PathBuf {
    sweep_dead_runs();
    create_test_dir_in(&run_root(), &fresh_name(prefix))
}

/// The same, directly under `/tmp`, for a unix-domain socket: `sun_path` holds
/// 104 bytes on macOS, and the system temp dir there is most of that already.
#[cfg(target_os = "macos")]
pub fn short_test_dir(prefix: &str) -> PathBuf {
    create_test_dir_in(Path::new("/tmp"), &fresh_name(prefix))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The refusal itself, on its `Result`: no panic, no process-global
    /// hook — an `AlreadyExists` is the inheritance signal and must stay
    /// an ordinary error the test reads.
    #[test]
    fn the_inner_create_refuses_a_dir_that_already_exists() {
        let parent = test_temp_dir("devboule-refusal");
        try_create_test_dir_in(&parent, "child").expect("the first create wins");
        let error = try_create_test_dir_in(&parent, "child")
            .expect_err("a dir that is already there must be refused, not adopted");
        assert_eq!(
            error.kind(),
            std::io::ErrorKind::AlreadyExists,
            "the refusal is the inheritance signal"
        );
    }

    /// The panicking wrapper names the path and says both causes, because
    /// Windows returns `AlreadyExists` for a file on the name too — a
    /// message claiming only "inherited" would be false.
    #[test]
    fn the_panicking_wrapper_names_the_path_and_both_possible_causes() {
        let parent = test_temp_dir("devboule-refusal-message");
        try_create_test_dir_in(&parent, "taken").expect("seed the taken name");
        let caught = std::panic::catch_unwind(|| create_test_dir_in(&parent, "taken"));
        let payload = caught.expect_err("an existing name must panic");
        let message = payload
            .downcast_ref::<String>()
            .map(String::as_str)
            .or_else(|| payload.downcast_ref::<&str>().copied())
            .unwrap_or_default();
        assert!(
            message.contains(&parent.join("taken").display().to_string()),
            "{message}"
        );
        assert!(message.contains("already exists"), "{message}");
        assert!(message.contains("a file holds this name"), "{message}");
    }

    /// Within one run, two names that share a nanosecond reading differ
    /// only by the counter — the pid is what separates runs — and the
    /// helper threads the counter through to a live path.
    #[test]
    fn two_names_with_the_same_nanos_differ_by_counter() {
        let first = temp_dir_name("devboule-unique-prefix", 1234, 555_555, 1);
        let second = temp_dir_name("devboule-unique-prefix", 1234, 555_555, 2);
        assert_ne!(
            first, second,
            "same prefix, pid and nanos: only the counter can keep the names apart"
        );
        assert_eq!(first, "devboule-unique-prefix-1234-555555-1");

        let live_first = test_temp_dir("devboule-unique-prefix");
        let live_second = test_temp_dir("devboule-unique-prefix");
        assert_ne!(live_first, live_second);
    }

    /// The prefix names a child of the system temp dir: a path separator
    /// or `..` would make the helper write somewhere else entirely.
    #[test]
    fn a_prefix_that_could_escape_the_temp_dir_is_refused() {
        for prefix in ["devboule/child", "devboule\\child", "devboule..escape"] {
            let caught = std::panic::catch_unwind(|| temp_dir_name(prefix, 1, 2, 3));
            let payload = caught.expect_err("a separator or .. in the prefix must stop the helper");
            let message = payload
                .downcast_ref::<String>()
                .map(String::as_str)
                .or_else(|| payload.downcast_ref::<&str>().copied())
                .unwrap_or_default();
            assert!(message.contains("path separator"), "{message}");
        }
        assert_eq!(temp_dir_name("devboule-ok", 7, 9, 3), "devboule-ok-7-9-3");
    }

    /// The whole scheme in one assertion: a directory the helper hands out
    /// lives under this run's own root, never loose in the system temp dir,
    /// and this process reads as alive so its root is never a candidate.
    #[test]
    fn a_directory_the_helper_hands_out_lives_under_this_runs_own_root() {
        let dir = test_temp_dir("devboule-run-root");
        let mine = runs_root().join(std::process::id().to_string());
        assert!(dir.starts_with(&mine), "{}", dir.display());
        assert!(!is_dead(std::process::id()), "this process is alive");
    }

    /// The sweep's two refusals: it never touches this run's own root, and
    /// only a pid-named directory is ever a candidate.
    #[test]
    fn the_sweep_keeps_its_own_root_and_anything_that_is_not_a_pid() {
        let root = runs_root();
        let mine = root.join(std::process::id().to_string());
        let not_a_pid = root.join("not-a-pid");
        std::fs::create_dir_all(&mine).expect("my own root");
        std::fs::create_dir_all(&not_a_pid).expect("a foreign entry");
        sweep_runs_under(&root, std::process::id());
        assert!(mine.is_dir(), "the sweep never removes this run's own root");
        assert!(not_a_pid.is_dir(), "only pid-named roots are candidates");
        let _ = std::fs::remove_dir(not_a_pid);
    }
}
