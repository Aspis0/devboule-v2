//! Shared tool-path relativization for the transcript views.
//!
//! Moved out of `claude_view` so the codex view reuses the same rules
//! instead of copying them: a path equal to the cwd falls back to the full
//! path (never `""`), and on Windows a drive-letter case mismatch still
//! relativizes.

use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;

/// Relativize `path` against `cwd`. Paths that are not under cwd are
/// returned unchanged (absolute, as received).
pub(crate) fn relativize_tool_path(path: &str, cwd: Option<&Path>) -> String {
    let Some(cwd) = cwd else {
        return path.to_string();
    };
    let given = Path::new(path);
    if !given.is_absolute() {
        return path.to_string();
    }
    if let Ok(stripped) = given.strip_prefix(cwd) {
        if stripped.as_os_str().is_empty() {
            return path.to_string();
        }
        return stripped.to_string_lossy().into_owned();
    }
    #[cfg(windows)]
    {
        if let Some(relative) = relativize_windows_case_insensitive(given, cwd) {
            return relative;
        }
    }
    path.to_string()
}

#[cfg(windows)]
fn relativize_windows_case_insensitive(path: &Path, cwd: &Path) -> Option<String> {
    use std::path::Component;
    let path_components: Vec<Component<'_>> = path.components().collect();
    let cwd_components: Vec<Component<'_>> = cwd.components().collect();
    if path_components.len() <= cwd_components.len() {
        return None;
    }
    for (left, right) in path_components.iter().zip(cwd_components.iter()) {
        if !windows_components_eq_ignore_case(left, right) {
            return None;
        }
    }
    let remainder: PathBuf = path_components[cwd_components.len()..].iter().collect();
    if remainder.as_os_str().is_empty() {
        return None;
    }
    Some(remainder.to_string_lossy().into_owned())
}

#[cfg(windows)]
fn windows_components_eq_ignore_case(
    left: &std::path::Component<'_>,
    right: &std::path::Component<'_>,
) -> bool {
    use std::path::Component;
    match (*left, *right) {
        (Component::Prefix(a), Component::Prefix(b)) => {
            a.as_os_str().eq_ignore_ascii_case(b.as_os_str())
        }
        (Component::RootDir, Component::RootDir)
        | (Component::CurDir, Component::CurDir)
        | (Component::ParentDir, Component::ParentDir) => true,
        (Component::Normal(a), Component::Normal(b)) => a.eq_ignore_ascii_case(b),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::relativize_tool_path;

    #[test]
    fn path_outside_cwd_is_forwarded_absolute() {
        let root = crate::test_support::FIXTURE_ROOT;
        let outside = format!("{root}-elsewhere/secret.txt");
        let relativized = relativize_tool_path(&outside, Some(Path::new(root)));
        assert_eq!(relativized, outside);
    }

    #[test]
    fn path_under_cwd_is_relativized() {
        let root = Path::new(crate::test_support::FIXTURE_ROOT);
        let under = crate::test_support::fixture_path("src/lib.rs");
        let relativized = relativize_tool_path(&under.to_string_lossy(), Some(root));
        assert_eq!(
            relativized,
            PathBuf::from("src").join("lib.rs").to_string_lossy()
        );
    }

    #[cfg(windows)]
    #[test]
    fn windows_path_differing_only_by_case_is_relativized() {
        let relativized = relativize_tool_path(r"c:\work\src\Lib.rs", Some(Path::new(r"C:\Work")));
        assert_eq!(
            relativized,
            PathBuf::from("src").join("Lib.rs").to_string_lossy(),
            "Windows prefix match is case-insensitive; remainder keeps original casing"
        );
    }
}
