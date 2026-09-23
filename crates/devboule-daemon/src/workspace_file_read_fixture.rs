//! The repository fixture both test modules of [`super`] drive their cases
//! through. Test code, in its own file and `#[cfg(test)]`-only: one
//! fixture for one module tree, so the refusals file and the window file
//! cannot drift into two shapes of the same repository.

use std::path::PathBuf;
use std::process::Command;
use std::time::UNIX_EPOCH;

/// A repository under `temp_dir`, the fixture this slice's brief names: the
/// panel reads files of a real checkout, and the `.git` the guard refuses
/// here is the real metadata folder — `config` included — rather than one a
/// test pretended with. Hard-requires git — a silent skip would let the
/// `.git` cases below pass without running anything.
pub(super) struct Repo {
    pub root: PathBuf,
}

impl Repo {
    pub(super) fn new(label: &str) -> Self {
        let root = crate::test_dirs::test_temp_dir(&format!("devboule-file-read-{label}"));
        let repo = Self { root };
        repo.run(&["init", "--quiet"]);
        repo
    }

    pub(super) fn run(&self, arguments: &[&str]) {
        let output = Command::new("git")
            .arg("-C")
            .arg(&self.root)
            .args(arguments)
            .output()
            .expect("git could not be spawned");
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
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A fresh directory *outside* the workspace (the link targets of the
/// escape cases): the shared helper **creates** it before this returns —
/// do not `create_dir` it again.
pub(super) fn unique_directory(label: &str) -> PathBuf {
    crate::test_dirs::test_temp_dir(&format!("devboule-file-read-{label}"))
}

/// The stat's own mtime, spelled the way the module spells it — the unit
/// (milliseconds) and the epoch are what the wire pins, so the test
/// computes them from the file rather than trusting a hard-coded number.
pub(super) fn mtime_ms(path: &std::path::Path) -> i64 {
    let time = std::fs::metadata(path)
        .expect("stat")
        .modified()
        .expect("mtime");
    match time.duration_since(UNIX_EPOCH) {
        Ok(after) => after.as_millis() as i64,
        Err(before) => -(before.duration().as_millis() as i64),
    }
}
