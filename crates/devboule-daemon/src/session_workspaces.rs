//! Workspace and project lifecycle: the journal reads behind `projects_list`
//! and `workspaces_list`, the local-versus-worktree split of `workspace_create`,
//! and the working directory a session command is started in.
//!
//! Split out of `session.rs` without a rewrite: every line below this header is
//! byte-identical to its `4c3acca` text, apart from five `pub(super)` markers the
//! parent module needs to keep calling in.

use super::*;

/// The stored path in the spelling a child process receives: see
/// `plain_path` for what stays verbatim.
pub(super) fn plain_cwd(path: &Path) -> PathBuf {
    crate::workspace::plain_path(&path.to_string_lossy()).into()
}

impl super::SessionRegistry {
    pub fn projects_list(&self) -> Result<Vec<Project>, WireError> {
        self.journal
            .as_ref()
            .ok_or_else(journal_unavailable)?
            .projects_list()
            .map(|projects| {
                projects
                    .into_iter()
                    .map(|project| project.to_project())
                    .collect()
            })
            .map_err(WireError::from)
    }

    pub fn project_add(&self, path: &str) -> Result<Project, WireError> {
        let record = crate::workspace::project_record(path)?;
        self.journal
            .as_ref()
            .ok_or_else(journal_unavailable)?
            .project_add(record)
            .map(|project| project.to_project())
            .map_err(WireError::from)
    }

    pub fn workspaces_list(&self, project_id: &str) -> Result<Vec<Workspace>, WireError> {
        self.journal
            .as_ref()
            .ok_or_else(journal_unavailable)?
            .workspaces_list(project_id)
            .map(|workspaces| {
                workspaces
                    .into_iter()
                    .map(|workspace| workspace.to_workspace())
                    .collect()
            })
            .map_err(WireError::from)
    }

    pub fn workspace_create(
        &self,
        project_id: &str,
        isolation: WorkspaceIsolation,
        branch: Option<String>,
    ) -> Result<Workspace, WireError> {
        self.workspace_create_titled(project_id, isolation, branch, None)
    }

    pub(crate) fn workspace_create_titled(
        &self,
        project_id: &str,
        isolation: WorkspaceIsolation,
        branch: Option<String>,
        title: Option<&str>,
    ) -> Result<Workspace, WireError> {
        match isolation {
            WorkspaceIsolation::Local => {
                if branch.is_some() {
                    return Err(WireError::new(
                        ErrorCode::InvalidRequest,
                        "Local workspaces do not take a branch.",
                    ));
                }
                self.create_local_workspace(project_id, title)
            }
            WorkspaceIsolation::Worktree => {
                self.create_worktree_workspace(project_id, branch, title)
            }
        }
    }

    /// The caller's workspace and its project, from the session row and never
    /// from a request field. A session with no workspace names no project.
    pub(crate) fn caller_workspace_scope(
        &self,
        session_id: &str,
        owner: &OwnerId,
    ) -> Result<(String, String), WireError> {
        let creator = self.agent_creator(session_id, owner)?;
        let workspace_id = creator.workspace_id.ok_or_else(|| {
            WireError::new(
                ErrorCode::InvalidRequest,
                "This session has no workspace, so there is no project to scope this call to.",
            )
        })?;
        let journal = self.journal.as_ref().ok_or_else(journal_unavailable)?;
        let project_id = journal
            .workspace_get(&workspace_id)
            .map_err(WireError::from)?
            .map(|record| record.project_id)
            .ok_or_else(|| {
                WireError::new(
                    ErrorCode::InvalidRequest,
                    "This session's workspace is gone from the journal.",
                )
            })?;
        Ok((workspace_id, project_id))
    }

    /// The project's name and folder, for surfaces that name where a
    /// create will land before the create runs.
    pub(crate) fn project_name_and_path(
        &self,
        project_id: &str,
    ) -> Result<(String, PathBuf), WireError> {
        let journal = self.journal.as_ref().ok_or_else(journal_unavailable)?;
        let project = self.require_project(journal, project_id)?;
        Ok((project.name, PathBuf::from(project.path)))
    }

    /// The project's workspace rows with their branches, which the wire
    /// `Workspace` drops.
    pub(crate) fn workspace_records(
        &self,
        project_id: &str,
    ) -> Result<Vec<crate::journal::WorkspaceRecord>, WireError> {
        self.journal
            .as_ref()
            .ok_or_else(journal_unavailable)?
            .workspaces_list(project_id)
            .map_err(WireError::from)
    }

    fn create_local_workspace(
        &self,
        project_id: &str,
        title: Option<&str>,
    ) -> Result<Workspace, WireError> {
        let journal = self.journal.as_ref().ok_or_else(journal_unavailable)?;
        let project = self.require_project(journal, project_id)?;
        let mut record = crate::workspace::local_workspace_record(&project);
        if let Some(title) = title.map(str::trim).filter(|title| !title.is_empty()) {
            record.title = title.to_string();
        }
        let workspace = journal.workspace_create(record).map_err(WireError::from)?;
        self.remember_workspace_path(&workspace.id, PathBuf::from(&workspace.path));
        Ok(workspace.to_workspace())
    }

    fn create_worktree_workspace(
        &self,
        project_id: &str,
        branch: Option<String>,
        title: Option<&str>,
    ) -> Result<Workspace, WireError> {
        let _serial = self
            .worktree_creation
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        #[cfg(test)]
        let _probe = self.worktree_probe.enter();
        let journal = self.journal.as_ref().ok_or_else(journal_unavailable)?;
        let project = self.require_project(journal, project_id)?;
        let project_path = PathBuf::from(&project.path);
        let live = crate::git::detect_git_repository(&project_path);
        refuse_worktree_unless_live_git_allows(&project.git_state, live.as_str(), project_id)?;
        let branch = match branch.filter(|value| !value.trim().is_empty()) {
            Some(branch) => branch,
            None => crate::worktree::generated_branch_slug(worktree_branch_seed()),
        };
        let Some(checkout) = crate::worktree::checkout_path_for_branch(&project_path, &branch)
        else {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                format!("Project '{project_id}' has no parent directory for a sibling worktree."),
            ));
        };
        let Some(root) = checkout.parent() else {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                format!("Project '{project_id}' has no parent directory for a sibling worktree."),
            ));
        };
        std::fs::create_dir_all(root).map_err(|error| {
            WireError::new(
                ErrorCode::Io,
                format!(
                    "Worktree directory '{}' is not writable: {error}",
                    crate::workspace::plain_path(&root.to_string_lossy())
                ),
            )
        })?;
        self.note_worktree_add_start(&checkout);
        if let Err(error) =
            crate::worktree::run_worktree_add_command(&project_path, &checkout, &branch, "HEAD")
        {
            let checkout_path = crate::workspace::plain_path(&checkout.to_string_lossy());
            // A killed add is repaired, not judged: git may have registered
            // the path before dying, which no listing can tell from a
            // winner — but the recorded path is always this call's own, so
            // it is removed and pruned exactly, and nothing else is touched.
            if error == crate::worktree::WorktreeAddError::TimedOut
                && self.take_worktree_add_start(&checkout)
            {
                return Err(WireError::new(
                    ErrorCode::WorkspaceUnavailable,
                    repair_killed_worktree_add(project_id, &project_path, &checkout),
                ));
            }
            self.clear_worktree_add_start();
            return Err(WireError::new(
                ErrorCode::WorkspaceUnavailable,
                match loser_checkout_cleanup(&project_path, &checkout, &branch) {
                    LoserCleanup::Removed => {
                        format!("Could not add git worktree for '{project_id}': {error}")
                    }
                    LoserCleanup::KeptLive => format!(
                        "Could not add git worktree for '{project_id}': {error}; the existing checkout at '{checkout_path}' was left alone",
                    ),
                    LoserCleanup::RemoveFailed(cleanup_error) => format!(
                        "Could not add git worktree for '{project_id}': {error}; leftover checkout at '{checkout_path}' ({cleanup_error})",
                    ),
                },
            ));
        }
        self.clear_worktree_add_start();
        let checkout = std::fs::canonicalize(&checkout).unwrap_or(checkout);
        let mut record = crate::workspace::worktree_workspace_record(&project, &checkout, &branch);
        if let Some(title) = title.map(str::trim).filter(|title| !title.is_empty()) {
            record.title = title.to_string();
        }
        let workspace = match journal.workspace_create(record) {
            Ok(workspace) => workspace,
            Err(error) => {
                if let Err(cleanup_error) = cleanup_failed_worktree_add(&project_path, &checkout) {
                    return Err(WireError::new(
                        ErrorCode::Journal,
                        format!(
                            "{error}; leftover checkout at '{}' ({cleanup_error})",
                            crate::workspace::plain_path(&checkout.to_string_lossy())
                        ),
                    ));
                }
                return Err(WireError::from(error));
            }
        };
        self.remember_workspace_path(&workspace.id, PathBuf::from(&workspace.path));
        Ok(workspace.to_workspace())
    }

    fn require_project(
        &self,
        journal: &Journal,
        project_id: &str,
    ) -> Result<crate::journal::ProjectRecord, WireError> {
        let project = journal
            .project_get(project_id)
            .map_err(WireError::from)?
            .ok_or_else(|| {
                WireError::new(
                    ErrorCode::WorkspaceUnavailable,
                    format!("Project '{project_id}' does not exist."),
                )
            })?;
        if !std::path::Path::new(&project.path).is_dir() {
            return Err(WireError::new(
                ErrorCode::WorkspaceUnavailable,
                format!("Project '{project_id}' is no longer an existing folder."),
            ));
        }
        Ok(project)
    }

    pub fn workspace_delete(&self, workspace_id: &str, force: bool) -> Result<(), WireError> {
        let journal = self.journal.as_ref().ok_or_else(journal_unavailable)?;
        let workspace = journal
            .workspace_get(workspace_id)
            .map_err(WireError::from)?
            .ok_or_else(|| workspace_unavailable(workspace_id, "it does not exist"))?;
        match workspace.isolation {
            WorkspaceIsolation::Local => {
                return Err(WireError::new(
                    ErrorCode::InvalidRequest,
                    "The local workspace is the project folder and is not removed as a worktree.",
                ));
            }
            WorkspaceIsolation::Worktree => {}
        }
        let checkout = PathBuf::from(&workspace.path);
        let project = journal
            .project_get(&workspace.project_id)
            .map_err(WireError::from)?;
        let Some(project) = project else {
            return self.detach_worktree_row(
                journal,
                workspace_id,
                &checkout,
                "its project row is gone",
            );
        };
        let repo = PathBuf::from(&project.path);
        if !repo.is_dir() {
            return self.detach_worktree_row(
                journal,
                workspace_id,
                &checkout,
                "its project folder is gone",
            );
        }
        let Some(root) = crate::worktree::worktree_root_beside_project(&repo) else {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "Project has no parent directory for a sibling worktree.",
            ));
        };
        if !crate::worktree::path_is_within(&checkout, &root) {
            let path = crate::workspace::plain_path(&checkout.to_string_lossy());
            let root = crate::workspace::plain_path(&root.to_string_lossy());
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                format!("Checkout '{path}' is not inside worktree root '{root}'."),
            )
            .with_details(ErrorDetails::WorktreeNotConfined { path, root }));
        }
        let expected_branch = workspace.branch.as_deref().unwrap_or("");
        match crate::worktree::list_existing_worktrees(&repo) {
            Ok(entries) => {
                match crate::worktree::identify_worktree_at_path(
                    &entries,
                    &checkout,
                    expected_branch,
                ) {
                    crate::worktree::WorktreeIdentity::Locked => {
                        let path = crate::workspace::plain_path(&checkout.to_string_lossy());
                        return Err(WireError::new(
                            ErrorCode::InvalidRequest,
                            format!(
                                "Worktree '{path}' is locked. Unlock it before removing; --force does not override a lock."
                            ),
                        )
                        .with_details(ErrorDetails::WorktreeLocked { path }));
                    }
                    crate::worktree::WorktreeIdentity::BranchMismatch { observed } => {
                        let path = crate::workspace::plain_path(&checkout.to_string_lossy());
                        return Err(WireError::new(
                            ErrorCode::InvalidRequest,
                            format!(
                                "Worktree at '{path}' is branch '{}', not '{}'. Refusing to remove another workspace's checkout.",
                                observed.as_deref().unwrap_or("(detached)"),
                                expected_branch
                            ),
                        )
                        .with_details(ErrorDetails::WorktreeMismatch {
                            path,
                            expected_branch: expected_branch.to_string(),
                            observed_branch: observed,
                        }));
                    }
                    crate::worktree::WorktreeIdentity::Match
                    | crate::worktree::WorktreeIdentity::Missing => {}
                }
            }
            Err(error) => {
                return Err(WireError::new(
                    ErrorCode::WorkspaceUnavailable,
                    format!("Could not list worktrees for '{workspace_id}': {error}"),
                ));
            }
        }
        if !force {
            if let Ok(true) = crate::worktree::checkout_has_dirty_files(&checkout) {
                return Err(WireError::new(
                    ErrorCode::InvalidRequest,
                    crate::worktree::worktree_dirty_remove_message(&checkout),
                )
                .with_details(ErrorDetails::WorktreeDirty {
                    path: crate::workspace::plain_path(&checkout.to_string_lossy()),
                    force_required: true,
                }));
            }
        }
        let command = crate::worktree::build_worktree_remove_command(&repo, &checkout, force);
        if let Err(error) = crate::worktree::run_worktree_remove_command_with_recovery(
            &command, &repo, &checkout, force,
        ) {
            if crate::worktree::is_dirty_worktree_remove_error(&error) {
                return Err(WireError::new(
                    ErrorCode::InvalidRequest,
                    crate::worktree::worktree_dirty_remove_message(&checkout),
                )
                .with_details(ErrorDetails::WorktreeDirty {
                    path: crate::workspace::plain_path(&checkout.to_string_lossy()),
                    force_required: true,
                }));
            }
            return Err(WireError::new(
                ErrorCode::WorkspaceUnavailable,
                format!("Could not remove worktree '{workspace_id}': {error}"),
            ));
        }
        journal
            .workspace_delete(workspace_id)
            .map_err(WireError::from)?;
        self.invalidate_workspace_path(workspace_id);
        Ok(())
    }

    fn detach_worktree_row(
        &self,
        journal: &Journal,
        workspace_id: &str,
        checkout: &Path,
        reason: &str,
    ) -> Result<(), WireError> {
        let leftover = checkout
            .exists()
            .then(|| crate::workspace::plain_path(&checkout.to_string_lossy()));
        journal
            .workspace_delete(workspace_id)
            .map_err(WireError::from)?;
        self.invalidate_workspace_path(workspace_id);
        eprintln!(
            "workspace '{workspace_id}' detached because {reason}; checkout left at {leftover:?}"
        );
        Ok(())
    }

    /// The folder a workspace's row names, in the **stored** spelling
    /// (canonical, `\\?\…`): what a birth row records, and what
    /// [`Self::workspace_cwd`] converts on its way to a child.
    pub(super) fn workspace_stored_path(&self, workspace_id: &str) -> Result<PathBuf, WireError> {
        if let Some(path) = self.cached_workspace_path(workspace_id) {
            if path.is_dir() {
                return Ok(path);
            }
            // The path can disappear after it was cached. Drop it before a
            // bounded journal refresh so a later mutation can repair it.
            self.invalidate_workspace_path(workspace_id);
        }
        let journal = self.journal.as_ref().ok_or_else(|| {
            workspace_journal_error(
                workspace_id,
                crate::journal::JournalError::Unavailable("journal is not open".to_string()),
            )
        })?;
        let workspace = journal
            .workspace_get_for_session(workspace_id)
            .map_err(|error| workspace_journal_error(workspace_id, error))?
            .ok_or_else(|| workspace_unavailable(workspace_id, "it does not exist"))?;
        let path = PathBuf::from(workspace.path);
        if !path.is_dir() {
            return Err(workspace_unavailable(
                workspace_id,
                "its folder is no longer available",
            ));
        }
        self.remember_workspace_path(workspace_id, path.clone());
        Ok(path)
    }

    /// `pub(crate)` because the workspace git-status read resolves its root
    /// from an id the same way a session does, and never from a request field.
    ///
    /// The answer is the plain spelling (`plain_path`), because this is the
    /// value every child process receives as its cwd and every agent reads
    /// as its workspace: a verbatim cwd sends cmd-side commands to
    /// `C:\Windows` and prints an alien PowerShell prompt. The cache and the
    /// journal keep the stored verbatim form.
    pub(crate) fn workspace_cwd(&self, workspace_id: &str) -> Result<PathBuf, WireError> {
        self.workspace_stored_path(workspace_id)
            .map(|path| plain_cwd(&path))
    }

    pub(super) fn apply_workspace_cwd(
        &self,
        workspace_id: Option<&str>,
        command: &mut PtyCommand,
    ) -> Result<(), WireError> {
        if let Some(workspace_id) = workspace_id {
            command.cwd = self.workspace_cwd(workspace_id)?;
        }
        Ok(())
    }
}

fn git_state_allows_worktree(state: &str) -> bool {
    matches!(state, "repository" | "inside_repository")
}

pub(super) fn refuse_worktree_unless_live_git_allows(
    recorded: &str,
    observed: &str,
    project_id: &str,
) -> Result<(), WireError> {
    if git_state_allows_worktree(observed) {
        return Ok(());
    }
    Err(
        WireError::new(
            ErrorCode::WorkspaceUnavailable,
            format!(
                "Project '{project_id}' cannot host a worktree (git state is '{observed}'; recorded '{recorded}')."
            ),
        )
        .with_details(ErrorDetails::WorktreeGitState {
            recorded: recorded.to_string(),
            observed: observed.to_string(),
        }),
    )
}

impl super::SessionRegistry {
    fn note_worktree_add_start(&self, checkout: &Path) {
        if let Ok(mut in_flight) = self.worktree_add_in_flight.lock() {
            *in_flight = Some(checkout.to_path_buf());
        }
    }

    /// Take the recorded path when it is this call's checkout. Under the
    /// creation serial a recorded path is always the recorder's own; a
    /// mismatch means a bug elsewhere, and the caller falls back to the
    /// listing guard instead of touching anything.
    fn take_worktree_add_start(&self, checkout: &Path) -> bool {
        let Ok(mut in_flight) = self.worktree_add_in_flight.lock() else {
            return false;
        };
        match in_flight.take() {
            Some(recorded) => recorded.as_path() == checkout,
            None => false,
        }
    }

    fn clear_worktree_add_start(&self) {
        if let Ok(mut in_flight) = self.worktree_add_in_flight.lock() {
            *in_flight = None;
        }
    }
}

/// Repair a `git worktree add` killed mid-registration: remove exactly the
/// recorded path and prune the worktree metadata git may have written for
/// it. The path is this call's own (taken from the in-flight record), so
/// no winner's checkout is reachable here.
pub(crate) fn repair_killed_worktree_add(project_id: &str, repo: &Path, checkout: &Path) -> String {
    let checkout_path = crate::workspace::plain_path(&checkout.to_string_lossy());
    let base = format!("Could not add git worktree for '{project_id}': git timed out");
    match cleanup_failed_worktree_add(repo, checkout) {
        Err(error) => format!("{base}; leftover checkout at '{checkout_path}' ({error})"),
        Ok(()) => {
            match crate::worktree::run_worktree_command(
                &crate::worktree::build_worktree_prune_command(repo),
            ) {
                Ok(()) => {
                    format!("{base}; removed the partial checkout at '{checkout_path}' and pruned")
                }
                Err(error) => format!(
                    "{base}; removed the partial checkout at '{checkout_path}' but pruning failed ({error})"
                ),
            }
        }
    }
}

fn cleanup_failed_worktree_add(repo: &Path, checkout: &Path) -> Result<(), String> {
    let remove = crate::worktree::build_worktree_remove_command(repo, checkout, true);
    crate::worktree::run_worktree_remove_command_with_recovery(&remove, repo, checkout, true)
}

/// What the loser's cleanup decided after a failed `git worktree add`.
/// `KeptLive` is the fail-closed answer: on any doubt the path is left
/// alone and the caller says so.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum LoserCleanup {
    Removed,
    KeptLive,
    RemoveFailed(String),
}

/// The loser's cleanup after a failed `git worktree add`: remove the
/// leftover only when nothing live stands there. Under the creation serial
/// above, a same-branch winner is fully finished by the time the loser
/// asks, so a path git still lists is somebody's — never the loser's to
/// remove, on whatever branch. A git listing that fails at all is doubt,
/// not absence, so the path is kept and the caller reports it.
pub(crate) fn loser_checkout_cleanup(repo: &Path, checkout: &Path, branch: &str) -> LoserCleanup {
    let live = match crate::worktree::list_existing_worktrees(repo) {
        Ok(live) => live,
        Err(_) => return LoserCleanup::KeptLive,
    };
    match crate::worktree::identify_worktree_at_path(&live, checkout, branch) {
        crate::worktree::WorktreeIdentity::Missing => {
            match cleanup_failed_worktree_add(repo, checkout) {
                Ok(()) => LoserCleanup::Removed,
                Err(error) => LoserCleanup::RemoveFailed(error),
            }
        }
        crate::worktree::WorktreeIdentity::Match
        | crate::worktree::WorktreeIdentity::Locked
        | crate::worktree::WorktreeIdentity::BranchMismatch { .. } => LoserCleanup::KeptLive,
    }
}

/// Concurrency observed inside the creation serial, per registry: the test
/// that pins the serial reads its own registry's probe, so worktree creates
/// on other registries in parallel tests cannot move its maximum.
/// Test-only: production takes the lock and never looks back.
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct WorktreeCreationProbe {
    in_flight: std::sync::atomic::AtomicUsize,
    max_seen: std::sync::atomic::AtomicUsize,
}

#[cfg(test)]
pub(crate) struct WorktreeProbeGuard {
    probe: std::sync::Arc<WorktreeCreationProbe>,
}

#[cfg(test)]
impl WorktreeCreationProbe {
    pub(crate) fn enter(self: &std::sync::Arc<Self>) -> WorktreeProbeGuard {
        let in_flight = self
            .in_flight
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            + 1;
        self.max_seen
            .fetch_max(in_flight, std::sync::atomic::Ordering::SeqCst);
        WorktreeProbeGuard {
            probe: std::sync::Arc::clone(self),
        }
    }

    pub(crate) fn reset(&self) {
        self.in_flight.store(0, std::sync::atomic::Ordering::SeqCst);
        self.max_seen.store(0, std::sync::atomic::Ordering::SeqCst);
    }

    pub(crate) fn max_seen(&self) -> usize {
        self.max_seen.load(std::sync::atomic::Ordering::SeqCst)
    }
}

#[cfg(test)]
impl Drop for WorktreeProbeGuard {
    fn drop(&mut self) {
        self.probe
            .in_flight
            .fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

fn worktree_branch_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0)
        ^ u64::from(std::process::id())
}

fn workspace_unavailable(workspace_id: &str, reason: &str) -> WireError {
    WireError::new(
        ErrorCode::WorkspaceUnavailable,
        format!("Workspace '{workspace_id}' is unavailable: {reason}."),
    )
}

fn workspace_journal_error(workspace_id: &str, error: crate::journal::JournalError) -> WireError {
    let mut wire = WireError::from(error);
    wire.message = format!(
        "Workspace '{workspace_id}' could not be read from the journal: {}",
        wire.message
    );
    wire
}
pub(super) fn workspace_spawn_error(
    workspace_id: Option<&str>,
    path: &std::path::Path,
    error: impl std::fmt::Display,
) -> WireError {
    let detail = error.to_string();
    workspace_directory_error(workspace_id, path, &detail)
        .unwrap_or_else(|| pty_wire_error("Could not start the terminal shell.", detail))
}

pub(super) fn map_workspace_spawn_wire_error(
    workspace_id: Option<&str>,
    path: &std::path::Path,
    error: WireError,
) -> WireError {
    workspace_directory_error(workspace_id, path, &error.message).unwrap_or(error)
}

fn workspace_directory_error(
    workspace_id: Option<&str>,
    path: &std::path::Path,
    detail: &str,
) -> Option<WireError> {
    let workspace_id = workspace_id?;
    let code = extract_os_error_code(detail)?;
    if !matches!(code, 2 | 3 | 267) {
        return None;
    }
    // The path is intentionally included only in the user-facing error. Do
    // not put this personal location in daemon logs or diagnostics.
    let plain_path = crate::workspace::plain_path(path.to_string_lossy().as_ref());
    eprintln!("workspace working directory became unavailable during spawn (OS error {code})");
    Some(WireError::new(
        ErrorCode::WorkspaceUnavailable,
        format!(
            "Workspace '{workspace_id}' at '{plain_path}' became unavailable while starting the session (OS error {code}: {}).",
            os_error_description(code)
        ),
    ))
}
