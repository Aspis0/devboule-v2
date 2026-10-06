//! The sweep's bounds and its answers when there is no repository to read —
//! the worktree cap, its own deadline, an unborn `HEAD`, a documented "no
//! answer" exit — against a runner that answers from a table, so none of them
//! needs a repository of its own.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use super::{scan, MAX_OTHER_WORKTREES, SCAN_DEADLINE};
use crate::git::{GitOutput, GitRunError};

/// A commit-shaped name. `rev-parse` prints one; nothing here cares which.
const SHA: &str = "1111111111111111111111111111111111111111";
const OTHER_SHA: &str = "2222222222222222222222222222222222222222";

fn answered(stdout: &str) -> GitOutput {
    GitOutput {
        success: true,
        code: Some(0),
        stdout: stdout.to_string(),
        stderr: String::new(),
    }
}

fn exited(code: i32) -> GitOutput {
    GitOutput {
        success: false,
        code: Some(code),
        stdout: String::new(),
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

fn plenty_of_time() -> Instant {
    Instant::now() + Duration::from_secs(60)
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

    let sweep = scan(&caller, "src/a.rs", &[], plenty_of_time(), &run).expect("sweep");
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
        .all(|row| { row.committed_change == Some(false) && !row.dirty_change }));
}

#[test]
fn a_sweep_that_has_already_given_up_claims_no_checkout_and_no_worktree() {
    let folders = Checkouts::new(2);
    let caller = folders.folder(0);
    let calls = RefCell::new(0usize);
    let run = |_root: &Path, _arguments: &[&str]| -> Result<GitOutput, GitRunError> {
        *calls.borrow_mut() += 1;
        Ok(answered(""))
    };
    // A deadline already passed: the sweep must not spend a single command on
    // a call its caller has stopped waiting for.
    let sweep = scan(
        &caller,
        "src/a.rs",
        &[],
        Instant::now() - Duration::from_secs(1),
        &run,
    )
    .expect("sweep");
    assert!(sweep.worktrees.is_empty(), "{:?}", sweep.worktrees);
    assert!(sweep.capped, "a sweep that gave up must say so");
    assert_eq!(*calls.borrow(), 0, "the listing ran after the deadline");
}

/// An unborn branch prints the word git was asked for, on stdout, and exits
/// **0** — so the shape is the only thing that separates it from a commit.
/// Whichever checkout is unborn, there is no branch point, and the row says
/// "unknown" rather than "no change".
#[test]
fn an_unborn_head_leaves_the_committed_change_unknown() {
    for unborn_is_caller in [true, false] {
        let folders = Checkouts::new(2);
        let caller = folders.folder(0);
        let other = folders.folder(1);
        let listed = porcelain(&caller, &[(other.clone(), "side".to_string())]);
        let run = |root: &Path, arguments: &[&str]| -> Result<GitOutput, GitRunError> {
            Ok(match arguments {
                ["worktree", "list", ..] => answered(&listed),
                ["rev-parse", "HEAD"] => {
                    let unborn = if unborn_is_caller {
                        root == caller
                    } else {
                        root == other
                    };
                    if unborn {
                        answered("HEAD\n")
                    } else {
                        answered(&format!("{SHA}\n"))
                    }
                }
                ["--literal-pathspecs", "--no-optional-locks", "status", ..] => {
                    answered("?? src/a.rs\0")
                }
                _ => return Err(GitRunError::SpawnFailed),
            })
        };

        let sweep = scan(&caller, "src/a.rs", &[], plenty_of_time(), &run).expect("sweep");
        let row = &sweep.worktrees[0];
        assert_eq!(row.base_sha, None, "unborn_is_caller={unborn_is_caller}");
        assert_eq!(
            row.committed_change, None,
            "no branch point is unknown, not 'no change' (unborn_is_caller={unborn_is_caller})"
        );
        assert!(
            row.dirty_change,
            "an unborn checkout still has a working tree to read"
        );
    }
}

/// What `merge-base` does in the case under test.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MergeBase {
    /// The two histories share an ancestor.
    Shared,
    /// `merge-base`'s documented "no answer": exit 1, nothing on stdout.
    NoAncestor,
    /// A repository that will not answer: exit 128.
    Broken,
}

#[test]
fn only_a_documented_no_answer_reads_as_no() {
    let folders = Checkouts::new(2);
    let caller = folders.folder(0);
    let other = folders.folder(1);
    let listed = porcelain(&caller, &[(other.clone(), "side".to_string())]);
    let merge_base = RefCell::new(MergeBase::Shared);
    let run = |_root: &Path, arguments: &[&str]| -> Result<GitOutput, GitRunError> {
        Ok(match arguments {
            ["worktree", "list", ..] => answered(&listed),
            ["rev-parse", "HEAD"] => answered(&format!("{SHA}\n")),
            ["merge-base", ..] => match *merge_base.borrow() {
                MergeBase::Shared => answered(&format!("{OTHER_SHA}\n")),
                MergeBase::NoAncestor => exited(1),
                MergeBase::Broken => exited(128),
            },
            _ => answered(""),
        })
    };

    let shared = scan(&caller, "src/a.rs", &[], plenty_of_time(), &run).expect("sweep");
    let row = &shared.worktrees[0];
    assert_eq!(row.base_sha.as_deref(), Some(OTHER_SHA));
    assert_eq!(
        row.committed_change,
        Some(false),
        "an empty diff is a false"
    );
    assert!(!row.dirty_change);

    // Exit 1 is merge-base's documented "the histories share no ancestor".
    *merge_base.borrow_mut() = MergeBase::NoAncestor;
    let unrelated = scan(&caller, "src/a.rs", &[], plenty_of_time(), &run).expect("sweep");
    let row = &unrelated.worktrees[0];
    assert_eq!(row.base_sha, None);
    assert_eq!(
        row.committed_change, None,
        "no base means nothing to compare"
    );
    assert!(!row.dirty_change);

    // Exit 128 is a repository that would not answer, and it is surfaced
    // rather than read as "no branch point".
    *merge_base.borrow_mut() = MergeBase::Broken;
    let failure = scan(&caller, "src/a.rs", &[], plenty_of_time(), &run)
        .expect_err("a broken repository is not a 'no answer'");
    assert!(failure.message.contains("git merge-base"), "{failure:?}");
    assert!(
        failure.message.contains("128"),
        "the operation and its exit code: {failure:?}"
    );
    assert!(
        !failure.retryable,
        "a repository that will not answer does not answer on a retry: {failure:?}"
    );
}

#[test]
fn a_runner_that_never_answered_is_not_an_answer() {
    let folders = Checkouts::new(2);
    let caller = folders.folder(0);
    let run = |_root: &Path, _arguments: &[&str]| Err(GitRunError::TimedOut);
    let failure = scan(&caller, "src/a.rs", &[], plenty_of_time(), &run)
        .expect_err("a timeout is not an answer");
    assert!(failure.message.contains("git worktree list"), "{failure:?}");
    assert!(failure.retryable, "a timeout is worth one more call");
    assert!(!failure.message.contains('/'), "the sentence names no path");

    let run = |_root: &Path, _arguments: &[&str]| Err(GitRunError::NotFound);
    let missing = scan(&caller, "src/a.rs", &[], plenty_of_time(), &run)
        .expect_err("git that is not installed is not an answer");
    assert!(!missing.retryable, "no git stays no git");
    assert_eq!(missing.message, "git worktree list: git is not installed");
}

/// The sweep's own bound must sit under any wait its caller would allow: the
/// tool hands [`SCAN_DEADLINE`] + [`COMMAND_TIMEOUT`] to the queue as that
/// wait, so the sum is the caller's whole patience.
#[test]
fn the_sweep_is_over_before_its_caller_stops_waiting() {
    assert!(
        SCAN_DEADLINE + super::COMMAND_TIMEOUT <= Duration::from_secs(30),
        "the sweep's deadline plus the one command running when it trips is {:?}",
        SCAN_DEADLINE + super::COMMAND_TIMEOUT
    );
}
