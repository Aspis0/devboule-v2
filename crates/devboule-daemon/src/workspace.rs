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

/// Numbered so two local rows on one folder never read alike.
pub(crate) fn default_local_title(project: &ProjectRecord, siblings: &[WorkspaceRecord]) -> String {
    let folder_already_has_a_local_row = siblings.iter().any(|sibling| {
        sibling.isolation == WorkspaceIsolation::Local && sibling.path == project.path
    });
    if !folder_already_has_a_local_row {
        return project.name.clone();
    }
    let mut number = 2;
    loop {
        let candidate = format!("{} {number}", project.name);
        if !siblings.iter().any(|sibling| sibling.title == candidate) {
            return candidate;
        }
        number += 1;
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

    use super::canonical_directory;

    #[test]
    fn relative_project_path_is_rejected_before_canonicalization() {
        let error = canonical_directory("relative-project").expect_err("relative path");
        assert_eq!(error.code, ErrorCode::InvalidRequest);
        assert!(error.message.contains("absolute"));
    }
}
