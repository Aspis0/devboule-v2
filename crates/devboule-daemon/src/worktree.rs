//! Git worktree helpers.
//!
//! Derived from herdr `src/worktree.rs` at commit
//! `d79fd746a96ddb5642939c9727baefce642d78e6` (Apache-2.0). This file has been
//! modified: process runners go through [`crate::git`] (Job Object, concurrent
//! stdout/stderr drain, timeout, CREATE_NO_WINDOW) instead of a bare
//! `Command`; tilde expansion is omitted because Devboule takes canonical
//! absolute paths; git argv paths are passed in display form so a Windows
//! verbatim `\\?\` prefix does not reach git; `git_common_worktrees_dir` is
//! split so parsing stdout stays pure; `ExistingWorktree` records a `locked`
//! porcelain line; checkout directory names include an FNV-1a of the exact
//! branch so slug collisions cannot share a folder.

use std::path::{Path, PathBuf};

use crate::git::{run_git_args, GitOutput, GitRunError};
use crate::workspace::display_path;

const DEFAULT_WORKTREE_PREFIX: &str = "worktree";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorktreeCommand {
    pub program: String,
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExistingWorktree {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub is_bare: bool,
    pub is_detached: bool,
    pub is_prunable: bool,
    pub is_locked: bool,
}

pub(crate) fn generated_branch_slug(seed: u64) -> String {
    let adjectives = [
        "brave", "calm", "clear", "green", "lucky", "quiet", "rapid", "silver",
    ];
    let nouns = [
        "river", "cloud", "field", "forest", "harbor", "meadow", "stone", "valley",
    ];
    let adjective = adjectives[(seed as usize) % adjectives.len()];
    let noun = nouns[((seed / adjectives.len() as u64) as usize) % nouns.len()];
    let suffix = seed & 0xffff;
    format!("{DEFAULT_WORKTREE_PREFIX}/{adjective}-{noun}-{suffix:04x}")
}

pub(crate) fn branch_to_path_slug(branch: &str) -> String {
    let mut slug = String::new();
    let mut last_was_dash = false;

    for ch in branch.chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
            last_was_dash = false;
        } else if !last_was_dash {
            slug.push('-');
            last_was_dash = true;
        }
    }

    let trimmed = slug.trim_matches('-').to_string();
    if trimmed.is_empty() {
        DEFAULT_WORKTREE_PREFIX.to_string()
    } else {
        trimmed
    }
}

pub(crate) fn canonical_or_original(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

#[allow(dead_code)] // herdr API; pinned by default_checkout_path_appends_repo_and_branch_slug
pub(crate) fn default_checkout_path(root: &Path, repo_name: &str, branch: &str) -> PathBuf {
    root.join(repo_name).join(branch_to_path_slug(branch))
}

/// Directory name for a checkout. `branch_to_path_slug` collapses `feature/a`
/// and `feature.a` to the same folder; the FNV-1a suffix of the exact branch
/// name makes two distinct branches never share a directory.
pub(crate) fn unique_checkout_dir_name(branch: &str) -> String {
    format!(
        "{}-{:08x}",
        branch_to_path_slug(branch),
        fnv1a32(branch.as_bytes())
    )
}

fn fnv1a32(bytes: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for byte in bytes {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// Sibling of the project folder, never inside it (so it does not appear in
/// the repo's own `git status`). `C:\code\foo` → `C:\code\foo.worktrees`.
pub(crate) fn worktree_root_beside_project(project_path: &Path) -> Option<PathBuf> {
    let parent = project_path.parent()?;
    let name = project_path.file_name()?;
    if name.is_empty() {
        return None;
    }
    Some(parent.join(format!("{}.worktrees", name.to_string_lossy())))
}

pub(crate) fn checkout_path_for_branch(project_path: &Path, branch: &str) -> Option<PathBuf> {
    Some(worktree_root_beside_project(project_path)?.join(unique_checkout_dir_name(branch)))
}

pub(crate) fn path_is_within(child: &Path, parent: &Path) -> bool {
    let child = canonical_or_original(child);
    let parent = canonical_or_original(parent);
    #[cfg(windows)]
    {
        let child = child
            .to_string_lossy()
            .replace('/', "\\")
            .to_ascii_lowercase();
        let parent = parent
            .to_string_lossy()
            .replace('/', "\\")
            .to_ascii_lowercase();
        let parent = parent.trim_end_matches('\\');
        child == parent || child.starts_with(&format!("{parent}\\"))
    }
    #[cfg(not(windows))]
    {
        child == parent || child.starts_with(&parent)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorktreeIdentity {
    Match,
    Missing,
    BranchMismatch { observed: Option<String> },
    Locked,
}

/// Keep the porcelain branch: a path match is not enough to prove this is
/// the workspace's worktree.
pub(crate) fn identify_worktree_at_path(
    entries: &[ExistingWorktree],
    path: &Path,
    expected_branch: &str,
) -> WorktreeIdentity {
    let expected = canonical_or_original(path);
    let Some(entry) = entries
        .iter()
        .find(|entry| canonical_or_original(&entry.path) == expected)
    else {
        return WorktreeIdentity::Missing;
    };
    if entry.is_locked {
        return WorktreeIdentity::Locked;
    }
    match entry.branch.as_deref() {
        Some(observed) if observed == expected_branch => WorktreeIdentity::Match,
        observed => WorktreeIdentity::BranchMismatch {
            observed: observed.map(str::to_string),
        },
    }
}

fn git_path_arg(path: &Path) -> String {
    display_path(&path.to_string_lossy())
}

pub(crate) fn build_worktree_remove_command(
    repo_root: &Path,
    path: &Path,
    force: bool,
) -> WorktreeCommand {
    let mut args = vec![
        "-C".to_string(),
        git_path_arg(repo_root),
        "worktree".to_string(),
        "remove".to_string(),
    ];
    if force {
        args.push("--force".to_string());
    }
    args.push(git_path_arg(path));

    WorktreeCommand {
        program: "git".to_string(),
        args,
    }
}

pub(crate) fn is_dirty_worktree_remove_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("contains modified or untracked files")
        && lower.contains("use --force to delete it")
}

pub(crate) fn is_not_working_tree_remove_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("is not a working tree") || lower.contains("is not a worktree")
}

pub(crate) fn worktree_dirty_remove_message(path: &Path) -> String {
    format!(
        "fatal: '{}' contains modified or untracked files, use --force to delete it",
        git_path_arg(path)
    )
}

pub(crate) fn build_worktree_add_new_branch_command(
    repo_root: &Path,
    path: &Path,
    branch: &str,
    base: &str,
) -> WorktreeCommand {
    WorktreeCommand {
        program: "git".to_string(),
        args: vec![
            "-C".to_string(),
            git_path_arg(repo_root),
            "worktree".to_string(),
            "add".to_string(),
            "-b".to_string(),
            branch.to_string(),
            git_path_arg(path),
            base.to_string(),
        ],
    }
}

pub(crate) fn build_worktree_add_existing_branch_command(
    repo_root: &Path,
    path: &Path,
    branch: &str,
) -> WorktreeCommand {
    WorktreeCommand {
        program: "git".to_string(),
        args: vec![
            "-C".to_string(),
            git_path_arg(repo_root),
            "worktree".to_string(),
            "add".to_string(),
            git_path_arg(path),
            branch.to_string(),
        ],
    }
}

pub(crate) fn leftover_worktree_checkout_matches_repo(path: &Path, worktrees_dir: &Path) -> bool {
    let git_file = path.join(".git");
    let Ok(content) = std::fs::read_to_string(&git_file) else {
        return false;
    };
    let Some(gitdir) = content.trim().strip_prefix("gitdir:") else {
        return false;
    };
    let gitdir = PathBuf::from(gitdir.trim());
    let gitdir = if gitdir.is_absolute() {
        gitdir
    } else {
        path.join(gitdir)
    };
    canonical_or_original(&gitdir).starts_with(canonical_or_original(worktrees_dir))
}

pub(crate) fn git_common_worktrees_dir_from_stdout(
    repo_root: &Path,
    stdout: &str,
) -> Option<PathBuf> {
    let common_dir = stdout.trim();
    if common_dir.is_empty() {
        return None;
    }
    let common_dir = PathBuf::from(common_dir);
    let common_dir = if common_dir.is_absolute() {
        common_dir
    } else {
        repo_root.join(common_dir)
    };
    Some(common_dir.join("worktrees"))
}

pub(crate) fn parse_worktree_list_porcelain(output: &str) -> Vec<ExistingWorktree> {
    let mut entries = Vec::new();
    let mut path: Option<PathBuf> = None;
    let mut branch = None;
    let mut is_bare = false;
    let mut is_detached = false;
    let mut is_prunable = false;
    let mut is_locked = false;

    let finish = |entries: &mut Vec<ExistingWorktree>,
                  path: &mut Option<PathBuf>,
                  branch: &mut Option<String>,
                  is_bare: &mut bool,
                  is_detached: &mut bool,
                  is_prunable: &mut bool,
                  is_locked: &mut bool| {
        if let Some(path) = path.take() {
            entries.push(ExistingWorktree {
                path,
                branch: branch.take(),
                is_bare: *is_bare,
                is_detached: *is_detached,
                is_prunable: *is_prunable,
                is_locked: *is_locked,
            });
        }
        *is_bare = false;
        *is_detached = false;
        *is_prunable = false;
        *is_locked = false;
    };

    for line in output.lines() {
        if line.trim().is_empty() {
            finish(
                &mut entries,
                &mut path,
                &mut branch,
                &mut is_bare,
                &mut is_detached,
                &mut is_prunable,
                &mut is_locked,
            );
            continue;
        }
        if let Some(value) = line.strip_prefix("worktree ") {
            path = Some(PathBuf::from(value));
        } else if let Some(value) = line.strip_prefix("branch ") {
            branch = Some(
                value
                    .strip_prefix("refs/heads/")
                    .unwrap_or(value)
                    .to_string(),
            );
        } else if line == "detached" {
            is_detached = true;
        } else if line == "bare" {
            is_bare = true;
        } else if line.starts_with("prunable") {
            is_prunable = true;
        } else if line.starts_with("locked") {
            is_locked = true;
        }
    }

    finish(
        &mut entries,
        &mut path,
        &mut branch,
        &mut is_bare,
        &mut is_detached,
        &mut is_prunable,
        &mut is_locked,
    );
    entries
}

pub(crate) fn run_worktree_command(command: &WorktreeCommand) -> Result<(), String> {
    let output = run_git_command(command)?;
    if output.success {
        return Ok(());
    }
    Err(output.error_message(&command.program))
}

fn run_git_command(command: &WorktreeCommand) -> Result<GitOutput, String> {
    run_git_args(&command.args).map_err(|error| match error {
        GitRunError::NotFound => "git is not available".to_string(),
        GitRunError::TimedOut => "git timed out".to_string(),
        GitRunError::SpawnFailed => "git could not be started".to_string(),
    })
}

pub(crate) fn local_branch_exists(repo_root: &Path, branch: &str) -> Result<bool, String> {
    let command = WorktreeCommand {
        program: "git".to_string(),
        args: vec![
            "-C".to_string(),
            git_path_arg(repo_root),
            "show-ref".to_string(),
            "--verify".to_string(),
            "--quiet".to_string(),
            format!("refs/heads/{branch}"),
        ],
    };
    let output = run_git_command(&command)?;
    if output.success {
        return Ok(true);
    }
    if output.code == Some(1) {
        return Ok(false);
    }
    Err(output.error_message(&command.program))
}

pub(crate) fn checkout_has_dirty_files(path: &Path) -> Result<bool, String> {
    let command = WorktreeCommand {
        program: "git".to_string(),
        args: vec![
            "-C".to_string(),
            git_path_arg(path),
            "status".to_string(),
            "--porcelain".to_string(),
            "--untracked-files=all".to_string(),
        ],
    };
    let output = run_git_command(&command)?;
    if output.success {
        return Ok(!output.stdout.trim().is_empty());
    }
    Err(output.error_message(&command.program))
}

pub(crate) fn list_existing_worktrees(repo_root: &Path) -> Result<Vec<ExistingWorktree>, String> {
    let command = WorktreeCommand {
        program: "git".to_string(),
        args: vec![
            "-C".to_string(),
            git_path_arg(repo_root),
            "worktree".to_string(),
            "list".to_string(),
            "--porcelain".to_string(),
        ],
    };
    let output = run_git_command(&command)?;
    if output.success {
        return Ok(parse_worktree_list_porcelain(&output.stdout));
    }
    Err(output.error_message(&command.program))
}

pub(crate) fn git_common_worktrees_dir(repo_root: &Path) -> Option<PathBuf> {
    let command = WorktreeCommand {
        program: "git".to_string(),
        args: vec![
            "-C".to_string(),
            git_path_arg(repo_root),
            "rev-parse".to_string(),
            "--git-common-dir".to_string(),
        ],
    };
    let output = run_git_command(&command).ok()?;
    if !output.success {
        return None;
    }
    git_common_worktrees_dir_from_stdout(repo_root, &output.stdout)
}

pub(crate) fn worktree_list_contains_path(repo_root: &Path, path: &Path) -> Result<bool, String> {
    let expected = canonical_or_original(path);
    Ok(list_existing_worktrees(repo_root)?
        .into_iter()
        .any(|entry| canonical_or_original(&entry.path) == expected))
}

pub(crate) fn run_worktree_add_command(
    repo_root: &Path,
    path: &Path,
    branch: &str,
    base: &str,
) -> Result<(), String> {
    let command = if local_branch_exists(repo_root, branch)? {
        build_worktree_add_existing_branch_command(repo_root, path, branch)
    } else {
        build_worktree_add_new_branch_command(repo_root, path, branch, base)
    };
    run_worktree_command(&command)
}

pub(crate) fn run_worktree_remove_command_with_recovery(
    command: &WorktreeCommand,
    repo_root: &Path,
    path: &Path,
    force: bool,
) -> Result<(), String> {
    match run_worktree_command(command) {
        Ok(()) => Ok(()),
        Err(err) if force && is_not_working_tree_remove_error(&err) => {
            if worktree_list_contains_path(repo_root, path)? {
                return Err(err);
            }
            if path.exists() {
                let Some(worktrees_dir) = git_common_worktrees_dir(repo_root) else {
                    return Err(err);
                };
                if !leftover_worktree_checkout_matches_repo(path, &worktrees_dir) {
                    return Err(err);
                }
                std::fs::remove_dir_all(path).map_err(|remove_err| {
                    format!(
                        "{err}; failed to remove leftover checkout {}: {remove_err}",
                        path.display()
                    )
                })?;
            }
            Ok(())
        }
        Err(err) => Err(err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_branch_slug_is_worktree_namespaced_and_stable() {
        assert_eq!(generated_branch_slug(0), "worktree/brave-river-0000");
        assert_eq!(generated_branch_slug(9), "worktree/calm-cloud-0009");
    }

    #[test]
    fn parses_git_worktree_list_porcelain() {
        let output = "\
worktree /repo/main
HEAD abc
branch refs/heads/main

worktree /repo/issue
HEAD def
branch refs/heads/worktree/issue

worktree /repo/detached
HEAD fed
detached
prunable stale

";

        assert_eq!(
            parse_worktree_list_porcelain(output),
            vec![
                ExistingWorktree {
                    path: PathBuf::from("/repo/main"),
                    branch: Some("main".into()),
                    is_bare: false,
                    is_detached: false,
                    is_prunable: false,
                    is_locked: false,
                },
                ExistingWorktree {
                    path: PathBuf::from("/repo/issue"),
                    branch: Some("worktree/issue".into()),
                    is_bare: false,
                    is_detached: false,
                    is_prunable: false,
                    is_locked: false,
                },
                ExistingWorktree {
                    path: PathBuf::from("/repo/detached"),
                    branch: None,
                    is_bare: false,
                    is_detached: true,
                    is_prunable: true,
                    is_locked: false,
                },
            ]
        );
    }

    #[test]
    fn parses_locked_prunable_detached_and_bare_porcelain_worktrees() {
        let output = "\
worktree /repo/main
bare

worktree /repo/feature
HEAD abc
branch refs/heads/feature
locked why

worktree /repo/detached
HEAD def
detached

worktree /repo/stale
HEAD fed
branch refs/heads/stale
prunable gitdir file points to non-existent location

";
        let entries = parse_worktree_list_porcelain(output);
        assert_eq!(entries.len(), 4);
        assert!(entries[0].is_bare);
        assert_eq!(entries[0].path, PathBuf::from("/repo/main"));
        assert!(entries[1].is_locked);
        assert_eq!(entries[1].branch.as_deref(), Some("feature"));
        assert!(entries[2].is_detached);
        assert!(entries[3].is_prunable);
        assert!(!entries[3].is_locked);
    }

    #[test]
    fn branch_to_path_slug_makes_branch_safe_folder_name() {
        assert_eq!(
            branch_to_path_slug("worktree/brave-river"),
            "worktree-brave-river"
        );
        assert_eq!(
            branch_to_path_slug("issue/137 Worktree Spaces"),
            "issue-137-worktree-spaces"
        );
        assert_eq!(branch_to_path_slug("///"), "worktree");
    }

    #[test]
    fn distinct_branches_that_slug_alike_do_not_share_a_checkout_directory() {
        assert_eq!(
            branch_to_path_slug("feature/a"),
            branch_to_path_slug("feature.a")
        );
        assert_ne!(
            unique_checkout_dir_name("feature/a"),
            unique_checkout_dir_name("feature.a")
        );
        let project = Path::new(r"C:\code\project");
        assert_ne!(
            checkout_path_for_branch(project, "feature/a"),
            checkout_path_for_branch(project, "feature.a")
        );
        let root = worktree_root_beside_project(project).expect("root");
        assert_eq!(root, PathBuf::from(r"C:\code\project.worktrees"));
        assert!(
            !root.starts_with(project),
            "worktree root must be a sibling, not inside the project"
        );
    }

    #[test]
    fn identify_worktree_at_path_keeps_the_porcelain_branch() {
        let entries = parse_worktree_list_porcelain(
            "\
worktree /w/feature-a
HEAD abc
branch refs/heads/feature/a

worktree /w/locked
HEAD def
branch refs/heads/locked
locked hold

",
        );
        assert_eq!(
            identify_worktree_at_path(&entries, Path::new("/w/feature-a"), "feature/a"),
            WorktreeIdentity::Match
        );
        assert_eq!(
            identify_worktree_at_path(&entries, Path::new("/w/feature-a"), "feature.a"),
            WorktreeIdentity::BranchMismatch {
                observed: Some("feature/a".to_string())
            }
        );
        assert_eq!(
            identify_worktree_at_path(&entries, Path::new("/w/missing"), "feature/a"),
            WorktreeIdentity::Missing
        );
        assert_eq!(
            identify_worktree_at_path(&entries, Path::new("/w/locked"), "locked"),
            WorktreeIdentity::Locked
        );
    }

    #[test]
    fn default_checkout_path_appends_repo_and_branch_slug() {
        assert_eq!(
            default_checkout_path(
                Path::new("/home/me/.herdr/worktrees"),
                "herdr",
                "worktree/brave-river",
            ),
            PathBuf::from("/home/me/.herdr/worktrees/herdr/worktree-brave-river")
        );
    }

    #[test]
    fn worktree_remove_command_preserves_branch_by_not_deleting_it() {
        let command = build_worktree_remove_command(
            Path::new("/repo/herdr"),
            Path::new("/w/herdr/issue-137"),
            false,
        );
        assert_eq!(command.program, "git");
        assert_eq!(
            command.args,
            vec![
                "-C",
                "/repo/herdr",
                "worktree",
                "remove",
                "/w/herdr/issue-137"
            ]
        );
        assert!(
            !command
                .args
                .iter()
                .any(|arg| arg == "-d" || arg == "-D" || arg == "--delete"),
            "removing a worktree must not delete the branch: {:?}",
            command.args
        );
    }

    #[test]
    fn forced_worktree_remove_command_uses_git_force_flag() {
        let command = build_worktree_remove_command(
            Path::new("/repo/herdr"),
            Path::new("/w/herdr/issue-137"),
            true,
        );
        assert_eq!(
            command.args,
            vec![
                "-C",
                "/repo/herdr",
                "worktree",
                "remove",
                "--force",
                "/w/herdr/issue-137"
            ]
        );
        assert!(
            !command
                .args
                .iter()
                .any(|arg| arg == "-d" || arg == "-D" || arg == "--delete"),
            "force still must not delete the branch: {:?}",
            command.args
        );
    }

    #[test]
    fn dirty_remove_error_detection_matches_git_force_hint() {
        assert!(is_dirty_worktree_remove_error(
            "fatal: '/w/herdr' contains modified or untracked files, use --force to delete it"
        ));
        assert!(!is_dirty_worktree_remove_error(
            "fatal: '/w/herdr' is a missing but already registered worktree"
        ));
        assert!(!is_dirty_worktree_remove_error(
            "fatal: '/w/herdr' contains a locked worktree, use --force only if you know why"
        ));
        assert!(is_not_working_tree_remove_error(
            "fatal: '/w/herdr' is not a working tree"
        ));
    }

    #[test]
    fn worktree_add_command_creates_new_branch_from_base() {
        let command = build_worktree_add_new_branch_command(
            Path::new("/repo/herdr"),
            Path::new("/w/herdr/worktree-brave-river"),
            "worktree/brave-river",
            "HEAD",
        );
        assert_eq!(command.program, "git");
        assert_eq!(
            command.args,
            vec![
                "-C",
                "/repo/herdr",
                "worktree",
                "add",
                "-b",
                "worktree/brave-river",
                "/w/herdr/worktree-brave-river",
                "HEAD"
            ]
        );
    }

    #[test]
    fn worktree_add_command_checks_out_existing_branch() {
        let command = build_worktree_add_existing_branch_command(
            Path::new("/repo/herdr"),
            Path::new("/w/herdr/worktree-brave-river"),
            "worktree/brave-river",
        );
        assert_eq!(command.program, "git");
        assert_eq!(
            command.args,
            vec![
                "-C",
                "/repo/herdr",
                "worktree",
                "add",
                "/w/herdr/worktree-brave-river",
                "worktree/brave-river"
            ]
        );
    }

    #[test]
    fn git_common_worktrees_dir_from_stdout_joins_relative_and_absolute() {
        assert_eq!(
            git_common_worktrees_dir_from_stdout(Path::new("/repo"), ".git\n"),
            Some(PathBuf::from("/repo/.git/worktrees"))
        );
        assert_eq!(
            git_common_worktrees_dir_from_stdout(Path::new("/repo"), "/abs/git\n"),
            Some(PathBuf::from("/abs/git/worktrees"))
        );
        assert_eq!(
            git_common_worktrees_dir_from_stdout(Path::new("/repo"), "  \n"),
            None
        );
    }

    #[test]
    fn leftover_checkout_matches_only_this_repo_worktrees_dir() {
        let root = std::env::temp_dir().join(format!(
            "devboule-leftover-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        ));
        let checkout = root.join("checkout");
        let worktrees = root.join("git").join("worktrees");
        std::fs::create_dir_all(&checkout).expect("checkout");
        std::fs::create_dir_all(worktrees.join("wt")).expect("worktrees");
        std::fs::write(
            checkout.join(".git"),
            format!("gitdir: {}\n", worktrees.join("wt").display()),
        )
        .expect("gitfile");
        assert!(leftover_worktree_checkout_matches_repo(
            &checkout, &worktrees
        ));
        assert!(!leftover_worktree_checkout_matches_repo(
            &checkout,
            &root.join("other")
        ));
        let _ = std::fs::remove_dir_all(root);
    }

    fn unique_temp_path(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir().join(format!("devboule-{name}-{}-{nanos}", std::process::id()))
    }

    fn run_git(repo: &Path, args: &[&str]) {
        let args = {
            let mut full = vec!["-C".to_string(), git_path_arg(repo)];
            full.extend(args.iter().map(|arg| (*arg).to_string()));
            full
        };
        let output = run_git_args(&args).expect("git");
        assert!(
            output.success,
            "git command failed: git {} stderr={}",
            args.join(" "),
            output.stderr
        );
    }

    fn create_committed_repo(name: &str) -> PathBuf {
        let repo = unique_temp_path(name);
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "--quiet"]);
        run_git(&repo, &["config", "user.email", "devboule@example.invalid"]);
        run_git(&repo, &["config", "user.name", "Devboule Test"]);
        std::fs::write(repo.join("README.md"), "test\n").unwrap();
        run_git(&repo, &["add", "README.md"]);
        run_git(&repo, &["commit", "--quiet", "-m", "initial"]);
        repo
    }

    #[test]
    #[ignore = "spawns git to create a real worktree checkout"]
    fn checkout_dirty_detection_reports_clean_and_dirty_worktrees() {
        let repo = create_committed_repo("worktree-dirty-detection-repo");
        let checkout = unique_temp_path("worktree-dirty-detection-checkout");
        let add = build_worktree_add_new_branch_command(
            &repo,
            &checkout,
            "worktree/dirty-detection",
            "HEAD",
        );
        run_worktree_command(&add).unwrap();

        assert_eq!(checkout_has_dirty_files(&checkout), Ok(false));
        std::fs::write(checkout.join("README.md"), "dirty\n").unwrap();
        assert_eq!(checkout_has_dirty_files(&checkout), Ok(true));

        let remove = build_worktree_remove_command(&repo, &checkout, true);
        run_worktree_command(&remove).unwrap();
        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    #[ignore = "spawns git to add and remove a real worktree"]
    fn run_worktree_add_and_remove_create_and_delete_checkout() {
        let repo = create_committed_repo("worktree-run-repo");
        let checkout = unique_temp_path("worktree-run-checkout");
        let branch = "worktree/test-create-remove";

        let add = build_worktree_add_new_branch_command(&repo, &checkout, branch, "HEAD");
        run_worktree_command(&add).unwrap();
        assert!(checkout.join("README.md").exists());
        let shown = run_git_args(&[
            "-C".to_string(),
            git_path_arg(&checkout),
            "branch".to_string(),
            "--show-current".to_string(),
        ])
        .expect("git branch --show-current");
        assert!(shown.success);
        assert_eq!(shown.stdout.trim(), branch);

        let remove = build_worktree_remove_command(&repo, &checkout, false);
        run_worktree_command(&remove).unwrap();
        assert!(!checkout.exists());

        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    #[ignore = "spawns git; covers the only remove_dir_all recovery path"]
    fn forced_worktree_remove_recovers_leftover_unregistered_checkout() {
        let repo = create_committed_repo("worktree-recovery-repo");
        let checkout = unique_temp_path("worktree-recovery-checkout");
        let branch = "worktree/recovery";

        let add = build_worktree_add_new_branch_command(&repo, &checkout, branch, "HEAD");
        run_worktree_command(&add).unwrap();
        let remove = build_worktree_remove_command(&repo, &checkout, true);
        run_worktree_command(&remove).unwrap();
        std::fs::create_dir_all(&checkout).unwrap();
        let stale_admin_dir = git_common_worktrees_dir(&repo).unwrap().join("stale");
        std::fs::write(
            checkout.join(".git"),
            format!("gitdir: {}\n", stale_admin_dir.display()),
        )
        .unwrap();
        std::fs::write(checkout.join("leftover"), "leftover\n").unwrap();

        run_worktree_remove_command_with_recovery(&remove, &repo, &checkout, true).unwrap();

        assert!(!checkout.exists());
        let _ = std::fs::remove_dir_all(repo);
    }

    #[test]
    #[ignore = "spawns git; refuses remove_dir_all on an unrelated replacement directory"]
    fn forced_worktree_remove_recovery_keeps_unrelated_replacement_directory() {
        let repo = create_committed_repo("worktree-recovery-unrelated-repo");
        let checkout = unique_temp_path("worktree-recovery-unrelated-checkout");
        let branch = "worktree/recovery-unrelated";

        let add = build_worktree_add_new_branch_command(&repo, &checkout, branch, "HEAD");
        run_worktree_command(&add).unwrap();
        let remove = build_worktree_remove_command(&repo, &checkout, true);
        run_worktree_command(&remove).unwrap();
        std::fs::create_dir_all(&checkout).unwrap();
        std::fs::write(checkout.join("unrelated"), "do not delete\n").unwrap();

        let err = run_worktree_remove_command_with_recovery(&remove, &repo, &checkout, true)
            .expect_err("unrelated replacement directory should not be removed");

        assert!(is_not_working_tree_remove_error(&err));
        assert!(checkout.join("unrelated").exists());
        let _ = std::fs::remove_dir_all(checkout);
        let _ = std::fs::remove_dir_all(repo);
    }
}
