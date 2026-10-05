//! Workspace and project lifecycle: the journal reads behind `projects_list`
//! and `workspaces_list`, the local-versus-worktree split of `workspace_create`,
//! the workspace creation gate, and the working directory a session command is
//! started in.

use super::*;

/// Per-workspace create/delete bookkeeping under one short mutex: which
/// workspaces have a create or resume in flight, and which carry the
/// archiving mark. A create parks by being counted, never by holding the
/// lock, so no delete or archive ever waits out a spawn. A marked
/// workspace never has a count: a create is counted only unmarked, and a
/// mark is taken only at count zero, in the same critical section. A
/// count lives as long as its create's scope, so a create that never
/// returns keeps that workspace's delete and archive refusing.
#[derive(Default)]
pub(crate) struct WorkspaceCreationGate {
    creating: std::collections::HashMap<String, usize>,
    archiving: std::collections::HashSet<String>,
}

pub(crate) struct WorkspaceCreationGuard<'a> {
    registry: &'a super::SessionRegistry,
    workspace_id: String,
}

impl Drop for WorkspaceCreationGuard<'_> {
    fn drop(&mut self) {
        let mut gate = self
            .registry
            .workspace_creation_gate
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        let creating = gate.creating.get_mut(&self.workspace_id);
        debug_assert!(
            creating.is_some(),
            "the count for a live guard cannot be absent"
        );
        if let Some(creating) = creating {
            *creating -= 1;
            if *creating == 0 {
                gate.creating.remove(&self.workspace_id);
            }
        }
    }
}

pub(crate) struct WorkspaceArchivingGuard<'a> {
    registry: &'a super::SessionRegistry,
    workspace_id: String,
}

/// The delete's own hold on the archiving mark. `owned` records whether THIS
/// call inserted the mark: the MCP archive flow holds it across its own
/// delete, and that holder's guard must stay the one that removes it.
struct WorkspaceDeleteReservation<'a> {
    registry: &'a super::SessionRegistry,
    workspace_id: String,
    owned: bool,
}

impl Drop for WorkspaceDeleteReservation<'_> {
    fn drop(&mut self) {
        if !self.owned {
            return;
        }
        self.registry
            .workspace_creation_gate
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .archiving
            .remove(&self.workspace_id);
    }
}

impl Drop for WorkspaceArchivingGuard<'_> {
    fn drop(&mut self) {
        self.registry
            .workspace_creation_gate
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .archiving
            .remove(&self.workspace_id);
    }
}

/// The refusal both the delete and the archive road give when a session is
/// starting into the workspace they target. The archive audit keys on this
/// sentence to keep a retryable refusal out of the denial outcomes.
pub(crate) const SESSION_STARTING_MESSAGE: &str =
    "A session is starting in this workspace; try again.";

fn a_session_is_starting() -> WireError {
    WireError::new(
        ErrorCode::InvalidRequest,
        SESSION_STARTING_MESSAGE.to_string(),
    )
}

#[cfg(test)]
pub(super) type CreateGateCheckpointHook = Arc<dyn Fn(&super::SessionRegistry) + Send + Sync>;

/// The stored path in the spelling a child process receives: see
/// `plain_path` for what stays verbatim.
pub(super) fn plain_cwd(path: &Path) -> PathBuf {
    crate::verbatim_path::plain_path(&path.to_string_lossy()).into()
}

/// What the filesystem says about a folder a delete is asked to judge.
/// Only `NotFound` **on a present volume** is `Vanished`: on Windows an
/// unassigned or deleted drive letter answers `NotFound` exactly like a
/// deleted folder does, so the volume root — `X:\` or `\\server\share\` —
/// must be there for that answer to be trusted; on Unix an unplugged disk or
/// a dropped share answers the same, so the path's mount point must be there
/// (`session_workspace_volume.rs`). Every other metadata error is
/// `Unavailable`, and a delete never acts on cannot-tell.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum FolderPresence {
    Present,
    Vanished,
    Unavailable,
}

/// The volume root of `path` — `X:\`, `\\?\C:\`, `\\server\share\`: the
/// path's disk/UNC prefix plus its root separator, which is itself statable.
/// `None` for a path with no such prefix or no root separator (a relative
/// path; a stored workspace path always has both), and the caller then
/// answers `Unavailable`.
#[cfg(windows)]
pub(super) fn volume_root(path: &Path) -> Option<PathBuf> {
    use std::path::{Component, Prefix};
    let mut components = path.components();
    let prefix = match components.next()? {
        Component::Prefix(prefix) => prefix,
        _ => return None,
    };
    match components.next()? {
        Component::RootDir => {}
        _ => return None,
    }
    match prefix.kind() {
        Prefix::Disk(_) | Prefix::VerbatimDisk(_) | Prefix::UNC(..) | Prefix::VerbatimUNC(..) => {
            Some(PathBuf::from(prefix.as_os_str()).join(std::path::MAIN_SEPARATOR.to_string()))
        }
        _ => None,
    }
}

/// Test-only replacement for the metadata probe the presence decision rests
/// on: a test cannot fabricate an unassigned drive letter portably. The
/// boxed fn answers for the folder **and** for its volume root — the
/// decision asks it for both.
#[cfg(test)]
pub(crate) type PresenceMetadataFn =
    Box<dyn Fn(&Path) -> std::io::Result<std::fs::Metadata> + Send + Sync>;

#[cfg(test)]
pub(crate) struct PresenceProbe {
    pub(crate) metadata: PresenceMetadataFn,
}

impl super::SessionRegistry {
    pub(crate) fn workspace_creation_guard(
        &self,
        workspace_id: Option<&str>,
    ) -> Result<Option<WorkspaceCreationGuard<'_>>, WireError> {
        let Some(workspace_id) = workspace_id else {
            return Ok(None);
        };
        let mut gate = self
            .workspace_creation_gate
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if gate.archiving.contains(workspace_id) {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "Workspace is being archived.",
            ));
        }
        *gate.creating.entry(workspace_id.to_string()).or_insert(0) += 1;
        Ok(Some(WorkspaceCreationGuard {
            registry: self,
            workspace_id: workspace_id.to_string(),
        }))
    }

    pub(crate) fn mark_workspace_archiving(
        &self,
        workspace_id: &str,
    ) -> Result<WorkspaceArchivingGuard<'_>, WireError> {
        let mut gate = self
            .workspace_creation_gate
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if gate.creating.get(workspace_id).copied().unwrap_or(0) > 0 {
            return Err(a_session_is_starting());
        }
        if !gate.archiving.insert(workspace_id.to_string()) {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "Workspace is already being archived.",
            ));
        }
        Ok(WorkspaceArchivingGuard {
            registry: self,
            workspace_id: workspace_id.to_string(),
        })
    }

    /// The queued delete's reservation: the mark blocks new session creation
    /// into this workspace (`workspace_creation_guard` refuses it), and a
    /// create already starting here refuses the delete instead of being
    /// waited out. Insert-if-absent: when the archive flow already holds
    /// the mark, its guard stays the owner and this one removes nothing.
    fn reserve_workspace_for_delete(
        &self,
        workspace_id: &str,
    ) -> Result<WorkspaceDeleteReservation<'_>, WireError> {
        let mut gate = self
            .workspace_creation_gate
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if gate.creating.get(workspace_id).copied().unwrap_or(0) > 0 {
            return Err(a_session_is_starting());
        }
        let owned = gate.archiving.insert(workspace_id.to_string());
        Ok(WorkspaceDeleteReservation {
            registry: self,
            workspace_id: workspace_id.to_string(),
            owned,
        })
    }

    /// Test-only face of the archiving mark, so the mark-ownership tests
    /// observe release and non-removal directly.
    #[cfg(test)]
    pub(crate) fn workspace_is_marked_archiving(&self, workspace_id: &str) -> bool {
        self.workspace_creation_gate
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .archiving
            .contains(workspace_id)
    }

    /// Test-only window on the delete's own reservation: `probe` runs while
    /// the delete's mark is held, the way a mid-flight delete's concurrent
    /// archive would see it. The reservation type never leaves this module.
    #[cfg(test)]
    pub(crate) fn hold_workspace_delete_reservation(
        &self,
        workspace_id: &str,
        probe: impl FnOnce(),
    ) {
        let _reservation = self
            .reserve_workspace_for_delete(workspace_id)
            .expect("the delete reservation must hold when no create is starting");
        probe();
    }

    /// Arm a callback the create road runs at its birth row and again once
    /// the session is registered: the two ends its workspace count must span.
    #[cfg(test)]
    pub(super) fn set_create_gate_checkpoint_hook(&self, hook: CreateGateCheckpointHook) {
        *self
            .create_gate_checkpoint_hook
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(hook);
    }

    #[cfg(test)]
    pub(super) fn fire_create_gate_checkpoint_hook(&self) {
        let hook = self
            .create_gate_checkpoint_hook
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone();
        if let Some(hook) = hook {
            hook(self);
        }
    }

    /// Arm the presence seam for this registry (test-only).
    #[cfg(test)]
    pub(crate) fn set_presence_probe_for_test(&self, probe: PresenceProbe) {
        *self
            .presence_probe
            .lock()
            .unwrap_or_else(|error| error.into_inner()) = Some(probe);
    }

    /// The folder's own metadata, through the presence seam when one is
    /// armed.
    fn presence_metadata(&self, path: &Path) -> std::io::Result<std::fs::Metadata> {
        #[cfg(test)]
        {
            let probe = self
                .presence_probe
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Some(probe) = probe.as_ref() {
                return (probe.metadata)(path);
            }
        }
        std::fs::symlink_metadata(path)
    }

    /// The presence decision both delete gates share: see
    /// [`FolderPresence`] for the rule.
    fn folder_presence(&self, path: &Path) -> FolderPresence {
        match self.presence_metadata(path) {
            Ok(_) => FolderPresence::Present,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if self.volume_is_present(path) {
                    FolderPresence::Vanished
                } else {
                    FolderPresence::Unavailable
                }
            }
            Err(_) => FolderPresence::Unavailable,
        }
    }

    #[cfg(windows)]
    fn volume_is_present(&self, path: &Path) -> bool {
        volume_root(path).is_some_and(|root| self.presence_metadata(&root).is_ok())
    }

    #[cfg(unix)]
    fn volume_is_present(&self, path: &Path) -> bool {
        use crate::session::session_workspace_volume::{volume_is_present, Entry};

        volume_is_present(
            path,
            &|probed| {
                self.presence_metadata(probed)
                    .map(|metadata| Entry::of(&metadata))
            },
            &|dir| {
                std::fs::read_dir(dir)
                    .map(|mut entries| entries.next().is_none())
                    .unwrap_or(true)
            },
        )
    }

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

    /// Rename a workspace (`WorkspaceSetTitle`): a rename changes a label,
    /// never an identity — the id and the sessions on the row are untouched.
    pub fn workspace_set_title(
        &self,
        workspace_id: &str,
        title: &str,
    ) -> Result<Workspace, WireError> {
        let title = validate_workspace_title(title)
            .map_err(|message| WireError::new(ErrorCode::InvalidRequest, message))?;
        self.journal
            .as_ref()
            .ok_or_else(journal_unavailable)?
            .workspace_set_title(workspace_id, &title)
            .map(|record| record.to_workspace())
            .map_err(WireError::from)
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
        let workspace = match title.map(str::trim).filter(|title| !title.is_empty()) {
            Some(title) => {
                record.title = title.to_string();
                journal.workspace_create(record)
            }
            None => journal.workspace_create_auto_titled(record),
        }
        .map_err(WireError::from)?;
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
                    crate::verbatim_path::plain_path(&root.to_string_lossy())
                ),
            )
        })?;
        self.note_worktree_add_start(&checkout);
        if let Err(error) =
            crate::worktree::run_worktree_add_command(&project_path, &checkout, &branch, "HEAD")
        {
            // The add's failure text is git's stderr; the repository-level
            // dubious-ownership refusal is answered with the static sentence,
            // so the repository's absolute path never travels on a create
            // failure.
            let error = match error {
                crate::worktree::WorktreeAddError::Failed(message) => {
                    crate::worktree::WorktreeAddError::Failed(worktree_error_reason(message))
                }
                other => other,
            };
            let checkout_path = crate::verbatim_path::plain_path(&checkout.to_string_lossy());
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
                            crate::verbatim_path::plain_path(&checkout.to_string_lossy())
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
        #[cfg(test)]
        self.workspace_delete_calls
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
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
        // At execution time on the worker, not at dispatch: the queue can
        // delay this job arbitrarily, so the sessions live NOW are the ones
        // that decide. The reservation taken first closes the gap the check
        // leaves — no session can be created into this workspace between the
        // check and the removal — and refuses at once when one is starting.
        let _reservation = self.reserve_workspace_for_delete(workspace_id)?;
        let live = self.live_sessions_in_workspace(workspace_id)?;
        if !live.is_empty() {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "sessions or terminals are still running in this workspace; close them first",
            ));
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
        match self.folder_presence(&repo) {
            FolderPresence::Vanished => {
                return self.detach_worktree_row(
                    journal,
                    workspace_id,
                    &checkout,
                    "its project folder is gone",
                );
            }
            // Not "gone" but not provably there either — a drive or share
            // that is unreachable right now, a denied path: refuse, keep the
            // row, let the owner retry when the volume returns.
            FolderPresence::Unavailable => {
                return Err(WireError::new(
                    ErrorCode::WorkspaceUnavailable,
                    "the folder's drive or share is not reachable right now",
                ));
            }
            FolderPresence::Present => {}
        }
        let Some(root) = crate::worktree::worktree_root_beside_project(&repo) else {
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                "Project has no parent directory for a sibling worktree.",
            ));
        };
        if !crate::worktree::path_is_within(&checkout, &root) {
            let path = crate::verbatim_path::plain_path(&checkout.to_string_lossy());
            let root = crate::verbatim_path::plain_path(&root.to_string_lossy());
            return Err(WireError::new(
                ErrorCode::InvalidRequest,
                format!("Checkout '{path}' is not inside worktree root '{root}'."),
            )
            .with_details(ErrorDetails::WorktreeNotConfined { path, root }));
        }
        let expected_branch = workspace.branch.as_deref().unwrap_or("");
        // The same presence rule the project gate runs, on the checkout:
        // only `NotFound` on a present volume means gone. An unavailable
        // checkout — unreachable volume, denied path — refuses, pathless;
        // a checkout that is a **file** is present, and the listing and
        // removal below refuse on their own terms.
        match self.folder_presence(&checkout) {
            FolderPresence::Unavailable => {
                return Err(WireError::new(
                    ErrorCode::WorkspaceUnavailable,
                    "the folder's drive or share is not reachable right now",
                ));
            }
            FolderPresence::Present => {}
            FolderPresence::Vanished => {
                // The listing is a precondition, on purpose: the lock check
                // below is only as good as it, so a vanished checkout in a
                // repository whose `git worktree list` fails stays refused
                // and the row keeps the workspace findable until git can
                // answer. Deliberate — see the report.
                let entries = match crate::worktree::list_existing_worktrees(&repo) {
                    Ok(entries) => entries,
                    Err(error) => {
                        let reason = worktree_error_reason(error);
                        return Err(WireError::new(
                            ErrorCode::WorkspaceUnavailable,
                            format!("Could not list worktrees for '{workspace_id}': {reason}"),
                        ));
                    }
                };
                // A locked registration survives every prune: detaching the
                // row would orphan it behind no app surface, so the lock
                // keeps its refusal. The comparison cannot canonicalize —
                // the folder is gone, and the journal's verbatim spelling
                // and git's plain one only agree through canonicalization —
                // so the plain keys are compared.
                let checkout_key = vanished_path_key(&checkout);
                if entries
                    .iter()
                    .any(|entry| entry.is_locked && vanished_path_key(&entry.path) == checkout_key)
                {
                    let path = crate::verbatim_path::plain_path(&checkout.to_string_lossy());
                    return Err(locked_worktree_error(&path));
                }
                // The prune rewrites the admin side of the repository, like
                // the add and the remove: it runs under the same creation
                // serial.
                let _serial = self
                    .worktree_creation
                    .lock()
                    .unwrap_or_else(|error| error.into_inner());
                self.detach_worktree_row(
                    journal,
                    workspace_id,
                    &checkout,
                    "its checkout folder is gone",
                )?;
                let _ = crate::worktree::run_worktree_command(
                    &crate::worktree::build_worktree_prune_command(&repo),
                );
                return Ok(());
            }
        }
        match crate::worktree::list_existing_worktrees(&repo) {
            Ok(entries) => {
                match crate::worktree::identify_worktree_at_path(
                    &entries,
                    &checkout,
                    expected_branch,
                ) {
                    crate::worktree::WorktreeIdentity::Locked => {
                        let path = crate::verbatim_path::plain_path(&checkout.to_string_lossy());
                        return Err(locked_worktree_error(&path));
                    }
                    crate::worktree::WorktreeIdentity::BranchMismatch { observed } => {
                        let path = crate::verbatim_path::plain_path(&checkout.to_string_lossy());
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
                let reason = worktree_error_reason(error);
                return Err(WireError::new(
                    ErrorCode::WorkspaceUnavailable,
                    format!("Could not list worktrees for '{workspace_id}': {reason}"),
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
                    path: crate::verbatim_path::plain_path(&checkout.to_string_lossy()),
                    force_required: true,
                }));
            }
        }
        let command = crate::worktree::build_worktree_remove_command(&repo, &checkout, force);
        // The serial `create_worktree_workspace` holds, held here too: an
        // add and this remove both rewrite the admin side of one repository,
        // and since the git arms left the loop they run on different threads
        // and different queue keys. Held only across the remove: a failed
        // remove must leave the row in place, so the failure stays retryable
        // (with force, and at all). The shutdown race with the journal
        // writer is closed on the other side, by the bounded write drain
        // before `flush_journal`.
        {
            let _serial = self
                .worktree_creation
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            if let Err(error) = crate::worktree::run_worktree_remove_command_with_recovery(
                &command, &repo, &checkout, force,
            ) {
                if crate::worktree::is_submodule_worktree_remove_error(&error) {
                    return Err(WireError::new(
                        ErrorCode::InvalidRequest,
                        crate::worktree::worktree_submodule_remove_message(),
                    )
                    // The details are the client's force hint, not a
                    // diagnosis: a submodule worktree need not be dirty.
                    .with_details(ErrorDetails::WorktreeDirty {
                        path: crate::verbatim_path::plain_path(&checkout.to_string_lossy()),
                        force_required: true,
                    }));
                }
                if crate::worktree::is_dirty_worktree_remove_error(&error) {
                    return Err(WireError::new(
                        ErrorCode::InvalidRequest,
                        crate::worktree::worktree_dirty_remove_message(&checkout),
                    )
                    .with_details(ErrorDetails::WorktreeDirty {
                        path: crate::verbatim_path::plain_path(&checkout.to_string_lossy()),
                        force_required: true,
                    }));
                }
                let reason = worktree_error_reason(error);
                return Err(WireError::new(
                    ErrorCode::WorkspaceUnavailable,
                    format!("Could not remove worktree '{workspace_id}': {reason}"),
                ));
            }
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
            .then(|| crate::verbatim_path::plain_path(&checkout.to_string_lossy()));
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

    /// The workspace row's own recorded base branch — the branch a worktree
    /// workspace was cut from; `None` for a Local workspace. The git-log
    /// read takes its comparison base from the row rather than
    /// re-deriving it from the checkout, falling back to the
    /// repository's default branch when the row records none.
    pub(crate) fn workspace_branch(&self, workspace_id: &str) -> Result<Option<String>, WireError> {
        let journal = self.journal.as_ref().ok_or_else(|| {
            workspace_journal_error(
                workspace_id,
                crate::journal::JournalError::Unavailable("journal is not open".to_string()),
            )
        })?;
        journal
            .workspace_get_for_session(workspace_id)
            .map(|record| record.and_then(|record| record.branch))
            .map_err(|error| workspace_journal_error(workspace_id, error))
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
    let checkout_path = crate::verbatim_path::plain_path(&checkout.to_string_lossy());
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

/// A comparable key for a recorded worktree path whose folder may be gone:
/// canonicalization needs the folder, so the plain spelling stands in, and
/// on Windows it compares the way `crate::worktree::path_is_within` compares.
/// The fold is ASCII-only and a miss is the safe direction: the lock refusal
/// then does not fire and the ghost can return, but nothing is ever refused
/// or detached by mistake. Off Windows the plain spelling compares as-is —
/// case-sensitive is correct there, and no separator translation applies.
fn vanished_path_key(path: &Path) -> String {
    let plain = crate::verbatim_path::plain_path(&path.to_string_lossy());
    #[cfg(windows)]
    {
        plain.replace('/', "\\").to_ascii_lowercase()
    }
    #[cfg(not(windows))]
    {
        plain
    }
}

/// The worktree family's failures carry git's stderr; the repository-level
/// dubious-ownership refusal is replaced with the shared static sentence, so
/// the repository's absolute path never travels on a create or delete
/// failure.
fn worktree_error_reason(error: String) -> String {
    if crate::workspace_git_support::is_dubious_ownership_message(&error) {
        crate::workspace_git_support::DUBIOUS_OWNERSHIP.to_string()
    } else {
        error
    }
}

/// The locked worktree's refusal — the vanished-checkout road and the normal
/// removal road answer with the same sentence and the same details.
fn locked_worktree_error(path: &str) -> WireError {
    WireError::new(
        ErrorCode::InvalidRequest,
        format!(
            "Worktree '{path}' is locked. Unlock it before removing; --force does not override a lock."
        ),
    )
    .with_details(ErrorDetails::WorktreeLocked {
        path: path.to_string(),
    })
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
    let plain_path = crate::verbatim_path::plain_path(path.to_string_lossy().as_ref());
    eprintln!("workspace working directory became unavailable during spawn (OS error {code})");
    Some(WireError::new(
        ErrorCode::WorkspaceUnavailable,
        format!(
            "Workspace '{workspace_id}' at '{plain_path}' became unavailable while starting the session (OS error {code}: {}).",
            os_error_description(code)
        ),
    ))
}
