//! The repository the collision sweep's tests drive their cases through: a
//! real git repository with real worktrees, pinned against the machine it
//! runs on (line-ending conversion changes what a diff counts, commit
//! signing can fail on someone else's key). Hard-requires git — a silent skip
//! would let every case pass without reading one.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The folder holding the repository and every checkout of it, so one
/// `Drop` removes the whole fixture.
pub(super) struct Repo {
    pub dir: PathBuf,
    pub root: PathBuf,
}

impl Repo {
    pub(super) fn new(label: &str) -> Self {
        let dir = crate::test_dirs::test_temp_dir(&format!("devboule-collisions-{label}"));
        let root = dir.join("repo");
        std::fs::create_dir_all(&root).expect("repository folder");
        let repo = Self { dir, root };
        repo.git_in(&repo.root, &["init", "--quiet"]);
        for setting in [
            ["config", "user.email", "test@devboule.local"].as_slice(),
            ["config", "user.name", "devboule test"].as_slice(),
            ["config", "core.autocrlf", "false"].as_slice(),
            ["config", "commit.gpgsign", "false"].as_slice(),
        ] {
            repo.git_in(&repo.root, setting);
        }
        repo
    }

    fn git_in(&self, root: &Path, arguments: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(arguments)
            .output()
            .expect("git could not be spawned");
        assert!(
            output.status.success(),
            "git {arguments:?} in {} failed: {}",
            root.display(),
            String::from_utf8_lossy(&output.stderr)
        );
    }

    /// Write a tracked file inside `root`.
    pub(super) fn write_in(&self, root: &Path, relative: &str, contents: &str) {
        let path = root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent directory");
        }
        std::fs::write(path, contents).expect("write");
    }

    pub(super) fn write(&self, relative: &str, contents: &str) {
        self.write_in(&self.root, relative, contents);
    }

    pub(super) fn commit_in(&self, root: &Path, message: &str) {
        self.git_in(root, &["add", "-A"]);
        self.git_in(root, &["commit", "--quiet", "--message", message]);
    }

    pub(super) fn commit(&self, message: &str) {
        self.commit_in(&self.root, message);
    }

    /// A second checkout of this repository on a new branch, beside the
    /// repository folder rather than inside it (a checkout inside another
    /// would show up in the repository's own status).
    pub(super) fn add_worktree(&self, label: &str, branch: &str) -> PathBuf {
        let path = self.dir.join(format!("worktree-{label}"));
        self.git_in(
            &self.root,
            &[
                "worktree",
                "add",
                "--quiet",
                "-b",
                branch,
                &path.to_string_lossy(),
            ],
        );
        path
    }

    /// This checkout's `HEAD`, trimmed.
    pub(super) fn head(&self, root: &Path) -> String {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args(["rev-parse", "HEAD"])
            .output()
            .expect("git could not be spawned");
        assert!(output.status.success(), "rev-parse HEAD failed");
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
