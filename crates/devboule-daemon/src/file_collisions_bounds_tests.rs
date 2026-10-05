//! The sweep's bounds and its answers with no repository behind them, against
//! a runner that answers from a table: the worktree cap, the answer for a
//! checkout whose `HEAD` does not resolve, and what a runner failure is.

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use super::{scan, MAX_OTHER_WORKTREES};
use crate::git::{GitOutput, GitRunError};

/// A commit-shaped name. `rev-parse` prints one; nothing here cares which.
const SHA: &str = "1111111111111111111111111111111111111111";

fn answered(stdout: &str) -> GitOutput {
    GitOutput {
        success: true,
        code: Some(0),
        stdout: stdout.to_string(),
        stderr: String::new(),
    }
}

/// Empty folders under one temp dir, removed with it. The sweep refuses to
/// read a worktree whose folder is gone, so even a table-driven case needs
/// real folders to name.
struct Checkouts {
    dir: PathBuf,
}

impl Checkouts {
    fn new(count: usize) -> Self {
        let dir = crate::test_dirs::test_temp_dir("devboule-collisions-bounds");
        for index in 0..count {
            std::fs::create_dir(dir.join(format!("checkout-{index}"))).expect("checkout folder");
        }
        Self { dir }
    }

    fn folder(&self, index: usize) -> PathBuf {
        self.dir.join(format!("checkout-{index}"))
    }
}

impl Drop for Checkouts {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// A `worktree list --porcelain` document: the caller's own checkout first,
/// then one entry per other folder, each on its own branch.
fn porcelain(caller: &Path, others: &[(PathBuf, String)]) -> String {
    let mut text = format!("worktree {}\nbranch refs/heads/main\n\n", caller.display());
    for (folder, branch) in others {
        text.push_str(&format!(
            "worktree {}\nbranch refs/heads/{branch}\n\n",
            folder.display()
        ));
    }
    text
}

#[test]
fn the_worktree_cap_stops_the_sweep_and_says_so() {
    let folders = Checkouts::new(MAX_OTHER_WORKTREES + 2);
    let caller = folders.folder(0);
    let others: Vec<(PathBuf, String)> = (1..=MAX_OTHER_WORKTREES + 1)
        .map(|index| (folders.folder(index), format!("branch-{index}")))
        .collect();
    let listed = porcelain(&caller, &others);
    let calls = RefCell::new(Vec::<Vec<String>>::new());
    let run = |_root: &Path, arguments: &[&str]| -> Result<GitOutput, GitRunError> {
        calls.borrow_mut().push(
            arguments
                .iter()
                .map(|argument| (*argument).to_string())
                .collect(),
        );
        let stdout = match arguments {
            ["worktree", "list", ..] => listed.clone(),
            ["rev-parse", "HEAD"] | ["merge-base", ..] => SHA.to_string(),
            _ => String::new(),
        };
        Ok(answered(&stdout))
    };

    let sweep = scan(&caller, "src/a.rs", &[], &run).expect("sweep");
    assert_eq!(
        sweep.worktrees.len(),
        MAX_OTHER_WORKTREES,
        "one checkout past the cap was read anyway"
    );
    assert!(sweep.capped, "a stopped sweep must say it stopped");
    // One listing, the caller's own head, then four reads per checkout: the
    // bound is on the commands too, not only on the rows.
    assert_eq!(calls.borrow().len(), 2 + MAX_OTHER_WORKTREES * 4);
    assert!(sweep
        .worktrees
        .iter()
        .all(|row| !row.committed_change && !row.dirty_change));
}

#[test]
fn an_unresolved_head_is_not_a_commit_sha() {
    let folders = Checkouts::new(2);
    let caller = folders.folder(0);
    let other = folders.folder(1);
    let listed = porcelain(&caller, &[(other, "unborn".to_string())]);
    let run = |_root: &Path, arguments: &[&str]| -> Result<GitOutput, GitRunError> {
        Ok(match arguments {
            ["worktree", "list", ..] => answered(&listed),
            // Measured on an unborn branch: git prints the word it was asked
            // for, on stdout, and exits 0.
            ["rev-parse", "HEAD"] => answered("HEAD\n"),
            ["--literal-pathspecs", "--no-optional-locks", "status", ..] => {
                answered("?? src/a.rs\0")
            }
            _ => return Err(GitRunError::SpawnFailed),
        })
    };

    let sweep = scan(&caller, "src/a.rs", &[], &run).expect("sweep");
    let row = &sweep.worktrees[0];
    assert_eq!(
        row.base_sha, None,
        "an unresolved HEAD is not a branch point"
    );
    assert!(!row.committed_change);
    assert!(
        row.dirty_change,
        "an unborn checkout still has a working tree to read"
    );
}

#[test]
fn a_runner_that_never_answered_is_not_an_answer() {
    let folders = Checkouts::new(2);
    let caller = folders.folder(0);
    let run = |_root: &Path, _arguments: &[&str]| Err(GitRunError::TimedOut);
    let error = scan(&caller, "src/a.rs", &[], &run).expect_err("a timeout is not an answer");
    assert!(error.contains("git worktree list"), "{error}");
    assert!(!error.contains('/'), "the sentence names no path: {error}");
}
