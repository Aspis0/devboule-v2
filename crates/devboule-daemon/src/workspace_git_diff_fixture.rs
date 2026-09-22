//! The repository fixture both test modules of [`super`] drive their cases
//! through. Test code, in its own file and `#[cfg(test)]`-only: one fixture
//! for one module tree, so the answers file and the no-lines file cannot
//! drift into two shapes of the same repository.

use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

pub(super) fn unique_directory(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "devboule-workspace-diff-{label}-{}-{stamp}",
        std::process::id()
    ))
}

/// A repository under `temp_dir`, pinned against the machine it runs on:
/// line-ending conversion changes what a diff counts, and commit signing can
/// fail on someone else's key. Hard-requires git — a silent skip would let
/// every case below pass without running one. Mirrors the fixture of
/// `workspace_git_status_tests.rs`: two slices, one shape each, neither
/// reaching into the other's scaffolding.
pub(super) struct Repo {
    pub root: PathBuf,
}

impl Repo {
    pub(super) fn new(label: &str) -> Self {
        let root = unique_directory(label);
        std::fs::create_dir(&root).expect("test directory");
        let repo = Self { root };
        repo.run(&["init", "--quiet"]);
        repo.run(&["config", "user.email", "test@devboule.local"]);
        repo.run(&["config", "user.name", "devboule test"]);
        repo.run(&["config", "core.autocrlf", "false"]);
        repo.run(&["config", "commit.gpgsign", "false"]);
        repo
    }

    fn git(&self, arguments: &[&str]) -> std::io::Result<std::process::Output> {
        Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(arguments)
            .output()
    }

    pub(super) fn run(&self, arguments: &[&str]) {
        let output = self.git(arguments).expect("git could not be spawned");
        assert!(
            output.status.success(),
            "git {arguments:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    pub(super) fn write(&self, relative: &str, contents: impl AsRef<[u8]>) {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent directory");
        }
        std::fs::write(path, contents).expect("write");
    }

    pub(super) fn commit(&self, message: &str) {
        self.run(&["add", "-A"]);
        self.run(&["commit", "--quiet", "--message", message]);
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
