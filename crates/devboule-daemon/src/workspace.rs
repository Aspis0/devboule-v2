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

/// Keep the canonical verbatim path for filesystem work, but remove only its
/// Windows display prefix at the wire boundary. The prefix is what preserves
/// long-path cwd support; exposing it to a person would make the UI show an
/// implementation detail instead of the selected folder.
pub(crate) fn display_path(path: &str) -> String {
    const VERBATIM_UNC_PREFIX: &str = r"\\?\UNC\";
    const VERBATIM_PREFIX: &str = r"\\?\";
    if path
        .get(..VERBATIM_UNC_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(VERBATIM_UNC_PREFIX))
        && path
            .get(VERBATIM_UNC_PREFIX.len()..)
            .is_some_and(is_verbatim_unc_suffix)
    {
        return format!(r"\\{}", &path[VERBATIM_UNC_PREFIX.len()..]);
    }
    if path
        .get(..VERBATIM_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(VERBATIM_PREFIX))
        && path
            .get(VERBATIM_PREFIX.len()..)
            .is_some_and(is_verbatim_drive_path)
    {
        return path[VERBATIM_PREFIX.len()..].to_string();
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

    use super::{canonical_directory, display_path};

    #[test]
    fn relative_project_path_is_rejected_before_canonicalization() {
        let error = canonical_directory("relative-project").expect_err("relative path");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error.message.contains("absolute"));
    }

    #[test]
    fn display_path_only_strips_recognized_verbatim_drive_and_unc_paths() {
        assert_eq!(
            display_path(r"\\?\C:\Users\alice\Project"),
            r"C:\Users\alice\Project"
        );
        assert_eq!(
            display_path(r"\\?\UNC\server\share\Project"),
            r"\\server\share\Project"
        );
        assert_eq!(display_path(r"\\?\"), r"\\?\");
        assert_eq!(display_path(r"\\?\UNC\"), r"\\?\UNC\");
        assert_eq!(display_path(r"\\?\UNC\server"), r"\\?\UNC\server");
        assert_eq!(
            display_path(r"\\?\Volume{abcd}\folder"),
            r"\\?\Volume{abcd}\folder"
        );
    }
}
