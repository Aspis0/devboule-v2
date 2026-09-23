//! `status_of` over real repositories built in `%TEMP%` — the cases that need
//! a process; the pure parsing rules live in `workspace_git_status_parse_tests.rs`.

use std::path::PathBuf;
use std::process::Command;

use devboule_protocol::{
    WorkspaceGitFileStatus, WorkspaceGitRow, WorkspaceGitStatus, WorkspaceGitTotals,
};

use super::status_of;

fn unique_directory(label: &str) -> PathBuf {
    crate::test_dirs::test_temp_dir(&format!("devboule-workspace-git-{label}"))
}

/// A repository under `temp_dir`, pinned against the machine it runs on:
/// line-ending conversion changes what a diff counts, and commit signing can
/// fail on someone else's key. Hard-requires git — a silent skip would let
/// every case below pass without running one.
struct Repo {
    root: PathBuf,
}

impl Repo {
    fn new(label: &str) -> Self {
        let root = unique_directory(label);
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

    fn run(&self, arguments: &[&str]) {
        let output = self.git(arguments).expect("git could not be spawned");
        assert!(
            output.status.success(),
            "git {arguments:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    fn write(&self, relative: &str, contents: impl AsRef<[u8]>) {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent directory");
        }
        std::fs::write(path, contents).expect("write");
    }

    fn commit(&self, message: &str) {
        self.run(&["add", "-A"]);
        self.run(&["commit", "--quiet", "--message", message]);
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

fn row<'a>(status: &'a WorkspaceGitStatus, path: &str) -> &'a WorkspaceGitRow {
    status
        .rows
        .iter()
        .find(|row| row.path == path)
        .unwrap_or_else(|| panic!("no row for `{path}` in {:?}", status.rows))
}

/// No absolute path in a sentence that leaves this machine: `error` is not
/// redacted on its way out (see `WorkspaceGitStatus`), so the message is the guard.
fn assert_no_path(message: &str) {
    let pathish = message.contains('\\') || message.contains('/') || message.contains(':');
    assert!(!pathish, "a path leaked into `error`: {message}");
}

/// Mutant `m:b` dies here end to end: a committed tree has only the two
/// `# branch.*` records, so any row is fabricated.
#[test]
fn a_clean_repository_answers_with_a_branch_and_no_rows() {
    let repo = Repo::new("clean");
    repo.write("file.txt", "one\ntwo\n");
    repo.commit("initial");

    let status = status_of(&repo.root);

    assert_eq!(status.error, None, "{:?}", status.error);
    assert!(status.is_git);
    assert!(!status.dirty, "dirty only comes from rows");
    assert!(status.rows.is_empty(), "invented rows: {:?}", status.rows);
    assert!(status.branch.is_some(), "branch.head is always printed");
    assert_eq!(status.totals, WorkspaceGitTotals::default());
}

/// `1 .M` with one line appended: the numstat merge, not just the record.
#[test]
fn a_modified_file_is_one_row_with_gits_line_counts() {
    let repo = Repo::new("modified");
    repo.write("file.txt", "one\ntwo\nthree\n");
    repo.commit("initial");
    repo.write("file.txt", "one\ntwo\nthree\nfour\n");

    let status = status_of(&repo.root);

    assert_eq!(status.error, None, "{:?}", status.error);
    assert!(status.dirty);
    assert_eq!(status.rows.len(), 1, "{:?}", status.rows);
    let changed = row(&status, "file.txt");
    assert_eq!(changed.status, WorkspaceGitFileStatus::Modified);
    assert_eq!((changed.additions, changed.deletions), (1, 0));
    assert!(!changed.capped);
    assert_eq!(
        (status.totals.additions, status.totals.deletions),
        (1, 0),
        "totals are the sum of the rows"
    );
}

/// Mutant `m:c` end to end: an untracked file is in neither numstat, so only
/// this path puts it in the reply.
#[test]
fn an_untracked_file_is_listed_with_its_own_line_count() {
    let repo = Repo::new("untracked");
    repo.write("tracked.txt", "a\n");
    repo.commit("initial");
    repo.write("fresh.txt", "1\n2\n3\n");

    let status = status_of(&repo.root);

    assert_eq!(status.error, None, "{:?}", status.error);
    assert!(status.dirty);
    assert_eq!(status.rows.len(), 1, "skipped: {:?}", status.rows);
    let fresh = row(&status, "fresh.txt");
    assert_eq!(fresh.status, WorkspaceGitFileStatus::Untracked);
    assert_eq!((fresh.additions, fresh.deletions), (3, 0));
    assert!(!fresh.capped, "a file under the cap is counted exactly");
    assert_eq!(status.totals.additions, 3);
}

/// `1 .D`: the deletion half of `git diff --numstat`, keyed on the row's path.
#[test]
fn a_deleted_file_is_a_deleted_row_with_the_lines_it_lost() {
    let repo = Repo::new("deleted");
    repo.write("gone.txt", "a\nb\nc\n");
    repo.commit("initial");
    std::fs::remove_file(repo.root.join("gone.txt")).expect("delete");

    let status = status_of(&repo.root);

    assert_eq!(status.error, None, "{:?}", status.error);
    assert_eq!(status.rows.len(), 1, "{:?}", status.rows);
    let gone = row(&status, "gone.txt");
    assert_eq!(gone.status, WorkspaceGitFileStatus::Deleted);
    assert_eq!((gone.additions, gone.deletions), (0, 3));
}

/// Mutant `m:a` — answer `is_git: true` for a folder with no `.git`.
/// `GitRepositoryStatus` exists to keep "not a repository" apart from "could
/// not tell", so `error` stays `null`: this is an answer, not a refusal.
#[test]
fn a_folder_without_a_repository_is_not_git_and_is_not_an_error() {
    let folder = unique_directory("not-a-repo");

    let status = status_of(&folder);
    let _ = std::fs::remove_dir_all(&folder);

    assert!(!status.is_git, "no .git answered is_git:true");
    assert_eq!(status.error, None, "{:?}", status.error);
    assert!(!status.dirty);
    assert!(status.rows.is_empty());
    assert_eq!(status.branch, None);
    assert_eq!(status.totals, WorkspaceGitTotals::default());
}

/// A path that is not there is refused before any subprocess is spawned, and
/// says so with no path in the sentence.
#[test]
fn a_folder_that_does_not_exist_is_an_error_and_never_a_panic() {
    let missing = unique_directory("missing");
    // The helper pre-creates the dir; this case needs the path absent.
    let _ = std::fs::remove_dir(&missing);

    let status = status_of(&missing);
    let _ = std::fs::remove_dir_all(&missing);

    assert!(!status.is_git);
    let error = status.error.as_deref().expect("an error");
    assert!(error.contains("not a directory"), "{error}");
    assert_no_path(error);
    assert!(status.rows.is_empty());
    assert_eq!(status.branch, None);
}

/// Mutant `m:g` — treat `InsideRepository` as a root. Read from a
/// subdirectory, `git status` answers for the whole repository with paths
/// relative to that subdirectory: other checkouts' files would land in the
/// panel and every untracked count would be taken against a path that is not
/// there. The refusal carries no path either.
#[test]
fn a_subfolder_of_a_repository_is_refused_with_a_caveat_and_no_rows() {
    let repo = Repo::new("subfolder");
    repo.write("file.txt", "one\n");
    repo.commit("initial");
    let nested = repo.root.join("nested");
    std::fs::create_dir(&nested).expect("nested");

    let status = status_of(&nested);

    assert!(status.is_git, "it is inside a repository: {status:?}");
    assert!(!status.dirty, "nothing was read");
    assert!(
        status.rows.is_empty(),
        "the whole repository leaked: {:?}",
        status.rows
    );
    assert_eq!(status.branch, None);
    assert_eq!(status.totals, WorkspaceGitTotals::default());
    let error = status.error.as_deref().expect("the caveat says why");
    assert!(error.contains("inside a git repository"), "{error}");
    assert_no_path(error);
}

/// `1 A.` and `2 R.` from a real `git mv`, with the counts keyed on the new
/// path — and the original path never becoming a second row.
#[test]
fn a_staged_addition_and_a_rename_keep_their_own_words() {
    let repo = Repo::new("staged");
    repo.write("old.txt", "one\ntwo\n");
    repo.commit("initial");
    repo.write("added.txt", "x\ny\n");
    repo.run(&["add", "added.txt"]);
    repo.run(&["mv", "old.txt", "renamed.txt"]);

    let status = status_of(&repo.root);

    assert_eq!(status.error, None, "{:?}", status.error);
    assert_eq!(status.rows.len(), 2, "{:?}", status.rows);
    let added = row(&status, "added.txt");
    assert_eq!(added.status, WorkspaceGitFileStatus::Added);
    assert_eq!((added.additions, added.deletions), (2, 0));
    assert_eq!(
        row(&status, "renamed.txt").status,
        WorkspaceGitFileStatus::Renamed
    );
    assert!(
        !status.rows.iter().any(|row| row.path == "old.txt"),
        "the rename's source became a row: {:?}",
        status.rows
    );
}

/// `u UU` from a real conflicting merge: no line delta to report, and the
/// zero standing for one is flagged rather than exact.
#[test]
fn a_conflict_is_conflicted_and_carries_no_invented_line_counts() {
    let repo = Repo::new("conflict");
    repo.write("c.txt", "base\n");
    repo.commit("initial");
    repo.run(&["checkout", "--quiet", "-b", "other"]);
    repo.write("c.txt", "other\n");
    repo.commit("other");
    repo.run(&["checkout", "--quiet", "-"]);
    repo.write("c.txt", "mainline\n");
    repo.commit("main");
    let merge = repo.git(&["merge", "other"]).expect("merge");
    assert!(
        !merge.status.success(),
        "the fixture must conflict: {}",
        String::from_utf8_lossy(&merge.stdout)
    );

    let status = status_of(&repo.root);

    assert_eq!(status.error, None, "{:?}", status.error);
    assert_eq!(status.rows.len(), 1, "{:?}", status.rows);
    let conflicted = row(&status, "c.txt");
    assert_eq!(conflicted.status, WorkspaceGitFileStatus::Conflicted);
    assert_eq!(
        (conflicted.additions, conflicted.deletions),
        (0, 0),
        "an unmerged path has no line delta"
    );
    assert!(conflicted.capped, "zero here is not an exact count");
}

/// `status` records are ~120 bytes each, so a couple of hundred changed files
/// pass the 16 KiB stdout accumulator. A list the daemon knows to be partial
/// is withheld, never handed over as a whole one.
#[test]
fn status_beyond_the_reply_cap_is_withheld_instead_of_cut_short() {
    let repo = Repo::new("flood");
    for index in 0..200 {
        repo.write(&format!("file-{index:03}.txt"), "a\n");
    }
    repo.commit("initial");
    for index in 0..200 {
        repo.write(&format!("file-{index:03}.txt"), "a\nb\n");
    }

    let status = status_of(&repo.root);

    assert!(status.is_git);
    assert!(status.dirty, "dirty even when the list is withheld");
    assert!(
        status.rows.is_empty(),
        "a partial list passed for a whole one: {} rows",
        status.rows.len()
    );
    let error = status.error.as_deref().expect("the cap is reported");
    assert!(error.contains("reply cap"), "{error}");
}

/// A repository with no commit yet has no `HEAD`, so one `git diff HEAD` would
/// fail: the two-call construction has to report an initial tree's staged file.
#[test]
fn a_repository_with_no_commit_still_reports_its_staged_files() {
    let repo = Repo::new("initial");
    repo.write("first.txt", "a\nb\n");
    repo.run(&["add", "-A"]);

    let status = status_of(&repo.root);

    assert_eq!(status.error, None, "{:?}", status.error);
    assert!(status.is_git, "{:?}", status.error);
    assert!(status.dirty);
    let first = row(&status, "first.txt");
    assert_eq!(first.status, WorkspaceGitFileStatus::Added);
    assert_eq!((first.additions, first.deletions), (2, 0));
}
