//! `diff_of` over real repositories — every reply that comes back **without**
//! lines, which is exactly what `without_lines` builds: `binary` (git or the
//! NUL byte called the file binary), `too_large` (a cap that declares
//! itself), and `error` (a refusal). The replies that carry lines live in
//! `workspace_git_diff_tests.rs`, the pure parsing rules in
//! `workspace_git_diff_parse_tests.rs`.

use devboule_protocol::WorkspaceGitDiffStatus;

use super::fixture::{unique_directory, Repo};
use super::{diff_of, DIFF_FILE_MAX_BYTES};

/// No absolute path in a sentence that leaves this machine: `error` is not
/// redacted on its way out (the debt on `WorkspaceGitFileDiff`), so the
/// message is the guard.
fn assert_no_path(message: &str) {
    let pathish = message.contains('\\') || message.contains('/') || message.contains(':');
    assert!(!pathish, "a path leaked into `error`: {message}");
}

/// Mutant `m:d` — serve binaries as text. git's own sentence for one is
/// `Binary files … differ`: no hunk follows it, and an `ok` with no lines
/// would read as "this change has no lines".
#[test]
fn a_binary_change_is_reported_as_binary_without_lines() {
    let repo = Repo::new("binary");
    repo.write("blob.dat", b"\x00\x01\x02first\x00");
    repo.commit("initial");
    repo.write("blob.dat", b"\x00\x01\x03second\x00");

    let diff = diff_of(&repo.root, "blob.dat");

    assert_eq!(diff.status, WorkspaceGitDiffStatus::Binary, "{diff:?}");
    assert!(diff.lines.is_empty(), "{:?}", diff.lines);
    assert_eq!(diff.error, None, "binary is an answer, not a failure");
    assert_eq!((diff.additions, diff.deletions), (0, 0));
}

/// The same word for an untracked binary, decided before any synthesis:
/// git would call this file binary too, so its bytes never become lines.
#[test]
fn an_untracked_binary_is_binary_before_any_synthesis() {
    let repo = Repo::new("binary-untracked");
    repo.write("tracked.txt", "a\n");
    repo.commit("initial");
    repo.write("fresh.dat", b"\x00binary\x00payload");

    let diff = diff_of(&repo.root, "fresh.dat");

    assert_eq!(diff.status, WorkspaceGitDiffStatus::Binary, "{diff:?}");
    assert!(diff.lines.is_empty(), "{:?}", diff.lines);
    assert_eq!(diff.error, None);
}

/// Mutant `m:b` — drop the per-file cap (or apply it after the lines are
/// already out). The fixture is a modified file over [`DIFF_FILE_MAX_BYTES`]
/// whose *diff* is a couple of lines: only the `stat` can refuse it, and
/// what comes back must be `too_large` with no lines, never the small diff
/// that would have hidden the cap's absence.
#[test]
fn a_file_over_the_per_file_cap_is_refused_before_its_diff_is_read() {
    let repo = Repo::new("over-cap");
    let mut rows = String::new();
    let mut index = 0usize;
    while rows.len() as u64 <= DIFF_FILE_MAX_BYTES {
        rows.push_str(&format!("row {index:08}\n"));
        index += 1;
    }
    assert!(
        rows.len() as u64 > DIFF_FILE_MAX_BYTES,
        "the fixture must exceed the cap"
    );
    repo.write("big.txt", &rows);
    repo.commit("initial");
    repo.write("big.txt", format!("tail\n{rows}"));

    let diff = diff_of(&repo.root, "big.txt");

    assert_eq!(diff.status, WorkspaceGitDiffStatus::TooLarge, "{diff:?}");
    assert!(diff.lines.is_empty(), "a cap cut must not leak lines");
    let error = diff.error.as_deref().expect("the cap is named");
    assert!(error.contains("per-file cap"), "{error}");
    assert_no_path(error);
}

/// The other cap: a file under [`DIFF_FILE_MAX_BYTES`] whose hunks run past
/// the shared stdout accumulator. Git exits 0 while the reader has stopped
/// reading — only the length says so, and the lines are withheld whole
/// rather than cut short.
#[test]
fn a_diff_past_the_reply_cap_is_withheld_rather_than_cut_short() {
    let repo = Repo::new("diff-cap");
    let mut before = String::new();
    let mut after = String::new();
    for index in 0..700 {
        before.push_str(&format!("original line {index:04} padding padding\n"));
        after.push_str(&format!("changed line {index:04} padding padding\n"));
    }
    repo.write("long.txt", &before);
    repo.commit("initial");
    repo.write("long.txt", &after);
    assert!(
        (repo.root.join("long.txt").metadata().expect("stat").len() as usize)
            < DIFF_FILE_MAX_BYTES as usize,
        "the per-file cap must not be what fires here"
    );

    let diff = diff_of(&repo.root, "long.txt");

    assert_eq!(diff.status, WorkspaceGitDiffStatus::TooLarge, "{diff:?}");
    assert!(
        diff.lines.is_empty(),
        "a cut diff must not pass for a whole one"
    );
    let error = diff.error.as_deref().expect("the cap is reported");
    assert!(error.contains("reply cap"), "{error}");
    assert_no_path(error);
}

/// Mutant `m:a` — skip the confinement of `path`. The first case walks out
/// to a real file that exists outside the repository, and only this check
/// stops it; an absolute path is refused by the same rule (matched by
/// component, because `is_absolute()` calls a bare `/x` relative on Windows).
#[test]
fn paths_that_leave_the_workspace_are_refused() {
    let repo = Repo::new("escape");
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let outside_name = format!("devboule-diff-outside-{}-{stamp}.txt", std::process::id());
    let parent = repo.root.parent().expect("temp parent").to_path_buf();
    let outside = parent.join(&outside_name);
    std::fs::write(&outside, "secret\n").expect("outside file");
    repo.write("in.txt", "a\n");
    repo.commit("initial");

    for requested in [
        format!("../{outside_name}"),
        "/absolute/escape.txt".to_string(),
    ] {
        let diff = diff_of(&repo.root, &requested);
        assert_eq!(
            diff.status,
            WorkspaceGitDiffStatus::Error,
            "{requested} was not refused: {diff:?}"
        );
        assert_eq!(
            diff.path, requested,
            "a refusal echoes the rejected text verbatim"
        );
        assert!(diff.lines.is_empty(), "{:?}", diff.lines);
        let error = diff.error.as_deref().expect("the refusal says why");
        assert!(error.contains("outside the workspace folder"), "{error}");
        assert_no_path(error);
    }
    let _ = std::fs::remove_file(&outside);
}

/// A path this repository does not track — a name it never had — is
/// refused, not answered with an empty diff that would read as "unchanged":
/// `git ls-files` carries no such path, the same verdict an ignored file
/// and a path behind a junction get (the two cases have their own tests).
#[test]
fn a_path_that_git_does_not_track_is_refused() {
    let repo = Repo::new("not-a-file");
    repo.write("real.txt", "a\n");
    repo.commit("initial");

    let diff = diff_of(&repo.root, "never-existed.txt");

    assert_eq!(diff.status, WorkspaceGitDiffStatus::Error, "{diff:?}");
    let error = diff.error.as_deref().expect("the refusal says why");
    assert!(
        error.contains("does not track the requested path"),
        "{error}"
    );
    assert_no_path(error);
    assert!(diff.lines.is_empty());
}

/// An ignored file exists inside the workspace and is still refused: it is
/// not a change of the workspace — git lists no record for it and tracks no
/// diff to give (both halves measured: status empty, `ls-files` empty). The
/// panel never offers the row either: the slice-1 status does not list
/// ignored files.
#[test]
fn an_ignored_file_is_refused_because_git_tracks_no_diff_for_it() {
    let repo = Repo::new("ignored");
    repo.write("app.log", "secret-content\n");
    repo.write(".gitignore", "*.log\n");
    repo.commit("initial");

    let diff = diff_of(&repo.root, "app.log");

    assert_eq!(diff.status, WorkspaceGitDiffStatus::Error, "{diff:?}");
    assert!(diff.lines.is_empty(), "{:?}", diff.lines);
    let error = diff.error.as_deref().expect("the refusal says why");
    assert!(
        error.contains("does not track the requested path"),
        "{error}"
    );
    assert_no_path(error);
    assert!(
        !format!("{diff:?}").contains("secret-content"),
        "the file exists but its content must not travel: {diff:?}"
    );
}

/// Mutant `m:f` — drop the symlink refusal. A link inside the checkout that
/// points outside it used to be opened by the synthesis (git itself refuses
/// to diff through it — measured: `git diff` on the link's path is empty),
/// so the target's content came back as `+` lines of a file "of the
/// workspace". Now the final component is stat'ed without following and the
/// link itself is refused, before any process or open. Windows-only: the
/// symlink API and the gate both live here (measured creatable in this
/// environment, so the test runs and is not `#[ignore]`).
#[test]
#[cfg(windows)]
fn a_symlink_is_refused_and_its_target_is_never_read() {
    let repo = Repo::new("symlink");
    repo.write("tracked.txt", "a\n");
    repo.commit("initial");
    let target = unique_directory("symlink-target").join("target.txt");
    std::fs::write(&target, "TOP SECRET OUTSIDE CONTENT\n").expect("target");
    let link = repo.root.join("link.txt");
    std::os::windows::fs::symlink_file(&target, &link).expect("symlink");

    let diff = diff_of(&repo.root, "link.txt");

    assert_eq!(diff.status, WorkspaceGitDiffStatus::Error, "{diff:?}");
    assert!(diff.lines.is_empty(), "{:?}", diff.lines);
    let error = diff.error.as_deref().expect("the refusal says why");
    assert!(error.contains("symbolic link"), "{error}");
    assert_no_path(error);
    assert!(
        !format!("{diff:?}").contains("TOP SECRET"),
        "the target's content leaked: {diff:?}"
    );
    let _ = std::fs::remove_file(&link);
    let _ = std::fs::remove_file(&target);
}

/// Mutant `m:f` — drop the walk's link check (`crosses_a_link` gate). A
/// path behind a directory junction used to be answered by git itself
/// (`? dirlink/present.txt`, measured — git walks through a reachable
/// junction) and the synthesis then read the target outside the workspace as
/// `+` lines. Now every component is stat'ed without following: both the
/// path whose target exists and the one whose target does not get the same
/// refusal — no bit about what is outside. Windows-only: junctions come
/// from `mklink /J`, which needs no privilege (measured).
#[test]
#[cfg(windows)]
fn a_path_through_a_directory_junction_is_refused_the_same_way_whether_the_target_exists_or_not() {
    let repo = Repo::new("junction");
    repo.write("in.txt", "a\n");
    repo.commit("initial");
    let outside = unique_directory("junction-target");
    std::fs::write(outside.join("present.txt"), "outside reached\n").expect("outside file");
    let link = repo.root.join("dirlink");
    let created = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&link)
        .arg(&outside)
        .output()
        .expect("mklink");
    assert!(
        created.status.success(),
        "mklink /J failed: {}",
        String::from_utf8_lossy(&created.stderr)
    );

    let reached = diff_of(&repo.root, "dirlink/present.txt");
    let missing = diff_of(&repo.root, "dirlink/absent.txt");

    assert_eq!(
        reached.status,
        WorkspaceGitDiffStatus::Error,
        "the junction walked into the target outside: {reached:?}"
    );
    assert_eq!(missing.status, WorkspaceGitDiffStatus::Error, "{missing:?}");
    assert_eq!(
        reached.error, missing.error,
        "the refusal must not depend on whether the target outside exists — that would be an \
         existence oracle"
    );
    for diff in [&reached, &missing] {
        assert!(diff.lines.is_empty(), "{:?}", diff.lines);
        let error = diff.error.as_deref().expect("the refusal says why");
        assert!(error.contains("crosses a link"), "{error}");
        assert_no_path(error);
    }
    let _ = std::fs::remove_dir(&link);
    let _ = std::fs::remove_dir_all(&outside);
}

/// A folder is not a file: `git diff` on a directory would hand back a
/// subtree, which is not what this frame promises.
#[test]
fn a_folder_path_is_refused() {
    let repo = Repo::new("folder-path");
    repo.write("sub/inner.txt", "a\n");
    repo.commit("initial");

    let diff = diff_of(&repo.root, "sub");

    assert_eq!(diff.status, WorkspaceGitDiffStatus::Error, "{diff:?}");
    let error = diff.error.as_deref().expect("the refusal says why");
    assert!(error.contains("folder, not a file"), "{error}");
    assert_no_path(error);
}

/// A workspace whose folder vanished is refused by the shared probe with
/// the same sentence the status list uses — and with no path in it.
#[test]
fn a_folder_that_does_not_exist_is_refused_without_a_path_in_the_sentence() {
    let missing = unique_directory("missing-root");
    // The helper pre-creates the dir; this case needs the path absent.
    let _ = std::fs::remove_dir(&missing);

    let diff = diff_of(&missing, "any.txt");
    let _ = std::fs::remove_dir_all(&missing);

    assert_eq!(diff.status, WorkspaceGitDiffStatus::Error, "{diff:?}");
    let error = diff.error.as_deref().expect("the refusal says why");
    assert!(error.contains("not a directory"), "{error}");
    assert_no_path(error);
    assert!(diff.lines.is_empty());
}

/// The shared probe's two other answers reach this frame with the same
/// sentences the status list shows: a workspace inside a monorepo is not
/// its root (the slice-1 review's trap), and a folder with no `.git` is
/// named as such — never answered with an `ok` diff of the wrong tree.
#[test]
fn the_probes_own_refusals_reach_the_diff_with_the_same_sentences() {
    let repo = Repo::new("inside");
    repo.write("file.txt", "a\n");
    repo.commit("initial");
    let nested = repo.root.join("nested");
    std::fs::create_dir(&nested).expect("nested");

    let inside = diff_of(&nested, "file.txt");
    assert_eq!(inside.status, WorkspaceGitDiffStatus::Error, "{inside:?}");
    assert!(inside.lines.is_empty(), "{:?}", inside.lines);
    let sentence = inside.error.as_deref().expect("the caveat says why");
    assert!(sentence.contains("inside a git repository"), "{sentence}");
    assert_no_path(sentence);

    let plain = unique_directory("not-a-repo");
    let not_a_repo = diff_of(&plain, "file.txt");
    let _ = std::fs::remove_dir_all(&plain);
    assert_eq!(
        not_a_repo.status,
        WorkspaceGitDiffStatus::Error,
        "{not_a_repo:?}"
    );
    assert!(not_a_repo.lines.is_empty(), "{:?}", not_a_repo.lines);
    let sentence = not_a_repo.error.as_deref().expect("the refusal says why");
    assert!(sentence.contains("not a git repository"), "{sentence}");
    assert_no_path(sentence);
}
