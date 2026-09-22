//! `diff_of` over real repositories — the replies that come back **with**
//! lines (`status: ok`), which is what `composed` builds. The replies that
//! carry no lines on purpose (`binary`, `too_large`, `error`) live in
//! `workspace_git_diff_without_lines_tests.rs`, and the pure parsing rules
//! in `workspace_git_diff_parse_tests.rs`.

use devboule_protocol::{WorkspaceGitDiffLineKind, WorkspaceGitDiffStatus};

use super::diff_of;
use super::fixture::Repo;

/// The whole shape of a modified file: hunk header as a `header` line, the
/// content lines with their markers already stripped, and the two counts
/// taken from those very lines.
#[test]
fn a_modified_file_comes_back_line_by_line() {
    let repo = Repo::new("modified");
    repo.write("file.txt", "one\ntwo\nthree\n");
    repo.commit("initial");
    repo.write("file.txt", "one\nTWO\nthree\nfour\n");

    let diff = diff_of(&repo.root, "file.txt");

    assert_eq!(diff.error, None, "{:?}", diff.error);
    assert_eq!(diff.status, WorkspaceGitDiffStatus::Ok);
    assert_eq!(diff.path, "file.txt", "the request is echoed verbatim");
    assert!(!diff.is_new && !diff.is_deleted);
    let rendered: Vec<(&str, &str)> = diff
        .lines
        .iter()
        .map(|line| match line.kind {
            WorkspaceGitDiffLineKind::Header => ("header", line.text.as_str()),
            WorkspaceGitDiffLineKind::Add => ("add", line.text.as_str()),
            WorkspaceGitDiffLineKind::Remove => ("remove", line.text.as_str()),
            WorkspaceGitDiffLineKind::Context => ("context", line.text.as_str()),
        })
        .collect();
    assert_eq!(
        rendered,
        [
            ("header", "@@ -1,3 +1,4 @@"),
            ("context", "one"),
            ("remove", "two"),
            ("add", "TWO"),
            ("context", "three"),
            ("add", "four"),
        ],
        "{rendered:?}"
    );
    assert_eq!((diff.additions, diff.deletions), (2, 1));
}

/// An untracked file has no `git diff` to ask for; its lines are the file's
/// own, every one an addition — and `is_new` says why.
#[test]
fn an_untracked_file_is_synthesized_as_all_additions() {
    let repo = Repo::new("untracked");
    repo.write("tracked.txt", "a\n");
    repo.commit("initial");
    repo.write("fresh.txt", "1\n2\n3\n");

    let diff = diff_of(&repo.root, "fresh.txt");

    assert_eq!(diff.error, None, "{:?}", diff.error);
    assert_eq!(diff.status, WorkspaceGitDiffStatus::Ok);
    assert!(diff.is_new, "an untracked path is not in HEAD");
    assert!(!diff.is_deleted);
    assert_eq!((diff.additions, diff.deletions), (3, 0));
    assert!(diff
        .lines
        .iter()
        .all(|line| line.kind == WorkspaceGitDiffLineKind::Add));
    assert_eq!(
        diff.lines
            .iter()
            .map(|line| line.text.as_str())
            .collect::<Vec<_>>(),
        ["1", "2", "3"]
    );
}

/// Mutant `m:c` — never set `is_deleted`. git states it twice in the
/// preamble (`deleted file mode` and `+++ /dev/null`); without the flag the
/// panel cannot tell a deletion from a modification.
#[test]
fn a_deleted_file_is_deleted_and_carries_its_removed_lines() {
    let repo = Repo::new("deleted");
    repo.write("gone.txt", "a\nb\nc\n");
    repo.commit("initial");
    std::fs::remove_file(repo.root.join("gone.txt")).expect("delete");

    let diff = diff_of(&repo.root, "gone.txt");

    assert_eq!(diff.error, None, "{:?}", diff.error);
    assert_eq!(diff.status, WorkspaceGitDiffStatus::Ok);
    assert!(diff.is_deleted, "git printed `deleted file mode` for it");
    assert!(!diff.is_new);
    assert_eq!((diff.additions, diff.deletions), (0, 3));
    assert!(diff.lines.iter().all(|line| matches!(
        line.kind,
        WorkspaceGitDiffLineKind::Header | WorkspaceGitDiffLineKind::Remove
    )));
}

/// `1 A.` from a real `git add`: staged but never committed, so it is not
/// in `HEAD` and every one of its lines is new.
#[test]
fn a_staged_addition_is_new() {
    let repo = Repo::new("staged");
    repo.write("base.txt", "a\n");
    repo.commit("initial");
    repo.write("added.txt", "x\ny\n");
    repo.run(&["add", "added.txt"]);

    let diff = diff_of(&repo.root, "added.txt");

    assert_eq!(diff.error, None, "{:?}", diff.error);
    assert_eq!(diff.status, WorkspaceGitDiffStatus::Ok);
    assert!(diff.is_new && !diff.is_deleted);
    assert_eq!((diff.additions, diff.deletions), (2, 0));
}

/// A repository with no commit yet has no `HEAD` (git exits 128, measured):
/// the one declared fallback to `git diff --cached` has to answer for the
/// file staged in its index.
#[test]
fn a_repository_with_no_commit_reports_its_staged_file() {
    let repo = Repo::new("initial");
    repo.write("first.txt", "a\nb\n");
    repo.run(&["add", "-A"]);

    let diff = diff_of(&repo.root, "first.txt");

    assert_eq!(diff.error, None, "{:?}", diff.error);
    assert_eq!(diff.status, WorkspaceGitDiffStatus::Ok);
    assert!(
        diff.is_new,
        "nothing exists in a tree-less index to compare"
    );
    assert_eq!((diff.additions, diff.deletions), (2, 0));
}

/// An unchanged tracked file: no status record, and `git ls-files` confirms
/// the path is tracked — an empty diff is git's own answer, and `ok` with no
/// lines is how it travels.
#[test]
fn an_unchanged_file_is_an_empty_ok_diff() {
    let repo = Repo::new("unchanged");
    repo.write("steady.txt", "a\nb\n");
    repo.commit("initial");

    let diff = diff_of(&repo.root, "steady.txt");

    assert_eq!(diff.status, WorkspaceGitDiffStatus::Ok, "{diff:?}");
    assert_eq!(diff.error, None);
    assert!(diff.lines.is_empty());
    assert_eq!((diff.additions, diff.deletions), (0, 0));
    assert!(!diff.is_new && !diff.is_deleted);
}

/// `assume-unchanged` hides the local edit from `git status` (measured), so
/// no record reaches the classifier; `git ls-files` still tracks the path,
/// and `ok` with no lines is git's own answer to a path git insists has no
/// delta — git is the authority for status (DECISIONS §6), and this is that
/// authority speaking, not a missing lookup.
#[test]
fn an_assume_unchanged_file_stays_an_ok_empty_diff_because_git_tracks_it() {
    let repo = Repo::new("assume");
    repo.write("file.txt", "base\n");
    repo.commit("initial");
    repo.run(&["update-index", "--assume-unchanged", "file.txt"]);
    repo.write("file.txt", "base\nchanged\n");

    let diff = diff_of(&repo.root, "file.txt");

    assert_eq!(diff.status, WorkspaceGitDiffStatus::Ok, "{diff:?}");
    assert!(
        diff.lines.is_empty(),
        "git hid the edit from its own status, so there is no diff to give: {:?}",
        diff.lines
    );
    assert_eq!(diff.error, None);
}

/// Mutant `m:h` — drop the `:(literal)` pathspec magic. Measured on git
/// 2.54: without it the pattern `a[1].txt` also answers `a1.txt` (BOTH
/// files come back), and the second file's preamble lines fall inside the
/// first file's hunks — where `--- a/…` parses as a removal. The reply must
/// name this file alone. (`*` and `?` cannot exist in a Windows file name,
/// so the bracket pair is the fixture that carries the rule — the same
/// Windows limit that moved slice 1's rename killer into a literal.)
#[test]
fn a_path_with_glob_characters_is_answered_for_that_file_alone() {
    let repo = Repo::new("literal");
    repo.write("a[1].txt", "bracket\n");
    repo.write("a1.txt", "plain\n");
    repo.commit("initial");
    repo.write("a[1].txt", "bracket\ntwo\n");
    repo.write("a1.txt", "plain\ntwo\n");

    let diff = diff_of(&repo.root, "a[1].txt");

    assert_eq!(diff.error, None, "{:?}", diff.error);
    assert_eq!(diff.status, WorkspaceGitDiffStatus::Ok);
    assert_eq!(
        (diff.additions, diff.deletions),
        (1, 0),
        "another file's lines counted here: {:?}",
        diff.lines
    );
    let rendered: Vec<(&str, &str)> = diff
        .lines
        .iter()
        .map(|line| match line.kind {
            WorkspaceGitDiffLineKind::Header => ("header", line.text.as_str()),
            WorkspaceGitDiffLineKind::Add => ("add", line.text.as_str()),
            WorkspaceGitDiffLineKind::Remove => ("remove", line.text.as_str()),
            WorkspaceGitDiffLineKind::Context => ("context", line.text.as_str()),
        })
        .collect();
    assert_eq!(
        rendered,
        [
            ("header", "@@ -1 +1,2 @@"),
            ("context", "bracket"),
            ("add", "two"),
        ],
        "{rendered:?}"
    );
    let quoted = format!("{:?}", diff.lines);
    assert!(
        !quoted.contains("plain") && !quoted.contains("a1.txt"),
        "the glob answered another file: {quoted}"
    );
}
