use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use devboule_protocol::{ErrorCode, WireError, WorkspaceIsolation};

use crate::git::detect_git_repository;
use crate::journal::{ProjectRecord, WorkspaceRecord};

static ENTITY_COUNTER: AtomicU64 = AtomicU64::new(1);

pub(crate) fn project_record(path: &str) -> Result<ProjectRecord, WireError> {
    let path = canonical_directory(path)?;
    let path_text = path.to_string_lossy().into_owned();
    let now = now_ms();
    Ok(ProjectRecord {
        id: entity_id("p"),
        name: project_name(&path),
        git_state: detect_git_repository(&path).as_str().to_string(),
        path: path_text,
        created_at_ms: now,
        updated_at_ms: now,
    })
}

pub(crate) fn local_workspace_record(project: &ProjectRecord) -> WorkspaceRecord {
    let now = now_ms();
    WorkspaceRecord {
        id: entity_id("w"),
        project_id: project.id.clone(),
        title: project.name.clone(),
        isolation: WorkspaceIsolation::Local,
        path: project.path.clone(),
        branch: None,
        created_at_ms: now,
        updated_at_ms: now,
    }
}

pub(crate) fn worktree_workspace_record(
    project: &ProjectRecord,
    checkout: &Path,
    branch: &str,
) -> WorkspaceRecord {
    let now = now_ms();
    WorkspaceRecord {
        id: entity_id("w"),
        project_id: project.id.clone(),
        title: format!("{} ({branch})", project.name),
        isolation: WorkspaceIsolation::Worktree,
        path: checkout.to_string_lossy().into_owned(),
        branch: Some(branch.to_string()),
        created_at_ms: now,
        updated_at_ms: now,
    }
}

pub(crate) fn canonical_directory(path: &str) -> Result<PathBuf, WireError> {
    if path.trim().is_empty() {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            "Project path is required.",
        ));
    }
    let candidate = Path::new(path);
    if !candidate.is_absolute() {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            "Project path must be absolute.",
        ));
    }
    let canonical = std::fs::canonicalize(candidate).map_err(|_| {
        WireError::new(
            ErrorCode::InvalidRequest,
            "Project path is not an existing folder.",
        )
    })?;
    if !canonical.is_dir() {
        return Err(WireError::new(
            ErrorCode::InvalidRequest,
            "Project path is not a folder.",
        ));
    }
    Ok(canonical)
}

/// The one place a stored path is turned back into its plain spelling: for
/// the wire, for a person, and for every child process cwd. Stored paths
/// are canonical (`\\?\C:\…`), and the prefix is a real feature — it is the
/// only spelling that can address a path longer than MAX_PATH — so it is
/// removed only when the plain form names the same path a plain caller
/// would reach:
/// - a plain form over `MAX_PATH` keeps the prefix (it is what makes the
///   path usable at all);
/// - a component that is a reserved device name, or that ends with a dot
///   or a space, resolves differently without the prefix (Win32 strips
///   trailing dots and spaces and claims reserved names only in the plain
///   namespace), so stripping would change the meaning;
/// - a `\\?\UNC\` path keeps its spelling: its plain form is still UNC, so
///   stripping removes none of the UNC-cwd hazard and only gives up the
///   long-path guarantee.
pub(crate) fn plain_path(path: &str) -> String {
    const MAX_PATH: usize = 260;
    const VERBATIM_UNC_PREFIX: &str = r"\\?\UNC\";
    const VERBATIM_PREFIX: &str = r"\\?\";
    if path
        .get(..VERBATIM_UNC_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(VERBATIM_UNC_PREFIX))
        && path
            .get(VERBATIM_UNC_PREFIX.len()..)
            .is_some_and(is_verbatim_unc_suffix)
    {
        return path.to_string();
    }
    if path
        .get(..VERBATIM_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(VERBATIM_PREFIX))
        && path
            .get(VERBATIM_PREFIX.len()..)
            .is_some_and(is_verbatim_drive_path)
    {
        let plain = &path[VERBATIM_PREFIX.len()..];
        // Windows counts MAX_PATH in UTF-16 code units plus the terminating
        // NUL, not in bytes: non-ASCII characters cost two UTF-8 bytes each,
        // so a plainly usable path can exceed 260 bytes while staying under
        // 260 units — counting bytes would keep `\\?\` on it and hand a
        // verbatim cwd to cmd.exe.
        if plain.encode_utf16().count() < MAX_PATH && !has_verbatim_only_component(plain) {
            return plain.to_string();
        }
    }
    path.to_string()
}

fn is_verbatim_drive_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && bytes[2] == b'\\'
}

fn is_verbatim_unc_suffix(path: &str) -> bool {
    let mut components = path.split('\\');
    components.next().is_some_and(|server| !server.is_empty())
        && components.next().is_some_and(|share| !share.is_empty())
}

/// A plain path that would resolve differently from its verbatim spelling:
/// **any** component — not just the last — that Win32 treats as a device, or
/// trims, in the plain namespace but takes literally under `\\?\`: a child
/// stripped of the prefix starts somewhere else when `release.` becomes
/// `release` or `CON` is claimed mid-path.
fn has_verbatim_only_component(path: &str) -> bool {
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let mut seen = false;
    for component in path
        .split(['\\', '/'])
        .filter(|component| !component.is_empty())
    {
        seen = true;
        let name = component
            .split_once('.')
            .map_or(component, |(name, _)| name);
        if RESERVED
            .iter()
            .any(|device| name.eq_ignore_ascii_case(device))
            || component.ends_with('.')
            || component.ends_with(' ')
        {
            return true;
        }
    }
    // No component at all is not a path a plain caller can name.
    !seen
}

fn project_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| "Project".to_string())
}

fn entity_id(prefix: &str) -> String {
    format!(
        "{prefix}.{:x}.{:x}.{:x}",
        now_ms(),
        std::process::id(),
        ENTITY_COUNTER.fetch_add(1, Ordering::Relaxed)
    )
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use devboule_protocol::ErrorCode;

    use super::{canonical_directory, plain_path};

    #[test]
    fn relative_project_path_is_rejected_before_canonicalization() {
        let error = canonical_directory("relative-project").expect_err("relative path");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error.message.contains("absolute"));
    }

    /// The review's path: the last component is clean, but `release.` is
    /// normalized away in the plain namespace, so a child stripped of the
    /// prefix would start in `C:\release\repo` while the workspace is
    /// `C:\release.\repo`. Every component has to be checked, not just the
    /// last non-empty one.
    #[test]
    fn plain_path_keeps_the_prefix_when_a_middle_component_is_verbatim_only() {
        assert_eq!(plain_path(r"\\?\C:\release.\repo"), r"\\?\C:\release.\repo");
        assert_eq!(plain_path(r"\\?\C:\release \repo"), r"\\?\C:\release \repo");
        // A reserved device name mid-path, with and without an extension:
        // Win32 claims the stem before the dot anywhere in the plain
        // namespace.
        assert_eq!(
            plain_path(r"\\?\C:\repo\con.txt\src"),
            r"\\?\C:\repo\con.txt\src"
        );
        assert_eq!(plain_path(r"\\?\C:\repo\lpt3\src"), r"\\?\C:\repo\lpt3\src");
    }

    /// Windows' MAX_PATH counts UTF-16 code units plus the terminating
    /// NUL, not UTF-8 bytes: this path is 263 bytes but only 133 units,
    /// so the plain spelling is usable and the prefix must come off.
    #[test]
    fn plain_path_measures_the_windows_limit_in_utf16_units_and_the_nul() {
        let plain = format!(r"C:\{}", "é".repeat(130));
        assert!(
            plain.len() >= 260,
            "the fixture is over the byte count: {}",
            plain.len()
        );
        assert_eq!(plain_path(&format!(r"\\?\{plain}")), plain);
    }

    #[test]
    fn plain_path_strips_only_a_short_drive_path_with_plain_safe_components() {
        assert_eq!(
            plain_path(r"\\?\C:\Users\alice\Project"),
            r"C:\Users\alice\Project"
        );
        assert_eq!(plain_path(r"\\?\C:\Project"), r"C:\Project");
        // A plain form over MAX_PATH keeps the prefix: it is the only
        // spelling that addresses the path at all.
        let deep = r"\\?\C:\";
        let long_tail = "word\\";
        let long = format!("{deep}{}", long_tail.repeat(60));
        assert!(plain_path(&long).starts_with(r"\\?\"));
        assert_eq!(plain_path(&long), long);
        // A reserved device name, or a trailing dot or space, resolves
        // differently without the prefix.
        assert_eq!(plain_path(r"\\?\C:\proj\CON"), r"\\?\C:\proj\CON");
        assert_eq!(plain_path(r"\\?\C:\proj\CON.txt"), r"\\?\C:\proj\CON.txt");
        assert_eq!(plain_path(r"\\?\C:\proj\lpt9"), r"\\?\C:\proj\lpt9");
        assert_eq!(plain_path(r"\\?\C:\proj\name."), r"\\?\C:\proj\name.");
        assert_eq!(plain_path(r"\\?\C:\proj\name "), r"\\?\C:\proj\name ");
        // A UNC path keeps its spelling: its plain form is still UNC.
        assert_eq!(
            plain_path(r"\\?\UNC\server\share\Project"),
            r"\\?\UNC\server\share\Project"
        );
        assert_eq!(plain_path(r"\\?\UNC\"), r"\\?\UNC\");
        assert_eq!(plain_path(r"\\?\UNC\server"), r"\\?\UNC\server");
        // Shapes the prefix rules do not recognize stay untouched, as do
        // paths that are already plain.
        assert_eq!(plain_path(r"\\?\"), r"\\?\");
        assert_eq!(
            plain_path(r"\\?\Volume{abcd}\folder"),
            r"\\?\Volume{abcd}\folder"
        );
        assert_eq!(
            plain_path(r"C:\Users\alice\Project"),
            r"C:\Users\alice\Project"
        );
        assert_eq!(plain_path(r"relative\path"), r"relative\path");
    }
}
