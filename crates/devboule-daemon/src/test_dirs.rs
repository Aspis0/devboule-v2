//! The one place this crate's test code asks the OS for its temp dir:
//! `test_temp_dir(prefix)` composes a pid + nanosecond + counter name and
//! creates it with `create_dir`, which refuses anything already there.
//! `temp_dir_guard_tests.rs` enforces that this is the only such place by
//! scanning every other file for the token — its header lists the
//! spellings it matches and the spellings it does not see.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

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

/// A fresh temp dir no other run can inherit: `prefix-pid-nanos-counter`.
pub(crate) fn test_temp_dir(prefix: &str) -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let name = temp_dir_name(
        prefix,
        std::process::id(),
        nanos,
        COUNTER.fetch_add(1, Ordering::Relaxed),
    );
    create_test_dir_in(&std::env::temp_dir(), &name)
}

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
}
