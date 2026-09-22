//! The plain folders both test modules of [`super`] drive their cases
//! through: a unique directory under the temp dir, with helpers to make the
//! entries a listing reads. No repository — git has nothing to do with these
//! tests, and a repo fixture would suggest otherwise. Test code, in its own
//! file and `#[cfg(test)]`-only.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// A unique path under `temp_dir`, for a directory the test wants *outside*
/// the workspace (the escape cases).
pub(super) fn unique_directory(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "devboule-workspace-files-{label}-{}-{stamp}",
        std::process::id()
    ))
}

pub(super) struct Folder {
    pub(super) root: PathBuf,
}

/// No absolute path in a sentence that leaves this machine: `error` is not
/// redacted on its way out (the debt on `WorkspaceDirectory`), so the
/// message is the guard. Shared by both test modules of [`super`].
pub(super) fn assert_no_path(message: &str) {
    let pathish = message.contains('\\') || message.contains('/') || message.contains(':');
    assert!(!pathish, "a path leaked into `error`: {message}");
}

impl Folder {
    pub(super) fn new(label: &str) -> Self {
        let root = unique_directory(label);
        std::fs::create_dir(&root).expect("test directory");
        Self { root }
    }

    pub(super) fn file(&self, relative: &str, contents: &str) {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent directory");
        }
        std::fs::write(path, contents).expect("write");
    }

    pub(super) fn dir(&self, relative: &str) -> PathBuf {
        let path = self.root.join(relative);
        std::fs::create_dir_all(&path).expect("directory");
        path
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
