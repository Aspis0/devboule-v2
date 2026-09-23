//! `stage` over real repositories built in `%TEMP%` — the cases that need
//! a process: the selection laws (cap, root, `.git`, confinement) are
//! proved here against git itself, each with the mutation that kills it,
//! because a refusal that merely *looks* right still loses an index if
//! spawn order or pathspec spelling is wrong.

use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use super::{commit, discard, stage, unstage, SELECTION_MAX, THE_ROOT};
use crate::workspace_files::NOT_PART_OF_THE_TREE;
use crate::workspace_git_support::{write_failure, INDEX_LOCKED, OUTSIDE_THE_WORKSPACE};

fn unique_directory(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "devboule-git-write-{label}-{}-{stamp}",
        std::process::id()
    ))
}

/// A repository under `temp_dir`, pinned the way the status tests pin
/// theirs: autocrlf off (line-ending conversion changes what `add`
/// stages), signing off (no key on someone else's machine). Hard-requires
/// git — a silent skip would let every case below pass without one.
struct Repo {
    root: PathBuf,
}

impl Repo {
    fn new(label: &str) -> Self {
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

    /// `git diff --cached --name-only`: the index's own answer to "what is
    /// staged", read straight — the assertion the brief names.
    fn staged_names(&self) -> String {
        let output = self
            .git(&["diff", "--cached", "--name-only"])
            .expect("git diff");
        String::from_utf8_lossy(&output.stdout).into_owned()
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// No absolute path in a sentence that leaves this machine: `error` is not
/// redacted on its way out, so the message is the guard.
fn assert_no_path(message: &str) {
    let pathish = message.contains('\\') || message.contains('/') || message.contains(':');
    assert!(!pathish, "a path leaked into the sentence: {message}");
}

/// Mutant `m:1` — confine after spawning (or not at all): `../x` reaches
/// `git add` and stages outside the checkout, and this refusal never
/// comes back.
#[test]
fn a_path_that_climbs_out_is_refused_and_nothing_is_staged() {
    let repo = Repo::new("outside");
    repo.write("a.txt", "one\n");
    repo.commit("initial");

    let error = stage(&repo.root, &["../escape.txt".to_string()]).expect_err("must refuse");

    assert_eq!(error, OUTSIDE_THE_WORKSPACE);
    assert_no_path(&error);
    assert!(repo.staged_names().is_empty(), "a refused path was staged");
}

/// Mutant `m:2` — drop the listing's `.git` guard from the selection
/// loop: `.git/config` reaches `git add`, which stages the repository's
/// own metadata (measured: `git add -- .git/config` succeeds), and the
/// refusal — checked before any spawn, so not even the probe runs for it —
/// never comes back.
#[test]
fn git_metadata_in_the_selection_is_refused_in_any_spelling() {
    let repo = Repo::new("git-metadata");
    repo.write("a.txt", "one\n");
    repo.commit("initial");

    for requested in [".git/config".to_string(), ".GIT/config".to_string()] {
        let error = stage(&repo.root, std::slice::from_ref(&requested)).expect_err("must refuse");
        assert_eq!(error, NOT_PART_OF_THE_TREE, "spelling: {requested}");
        assert_no_path(&error);
        assert!(
            repo.staged_names().is_empty(),
            "a refused path was staged: {requested}"
        );
    }
}

/// Mutant `m:3` — drop the cap: 501 paths are spawned through
/// `status --porcelain` inside the discard's classification until the
/// 16 KiB accumulator cuts the reply and the act proceeds on half a
/// selection. The refusal is spelling-only and costs no spawn.
#[test]
fn a_selection_past_the_cap_is_refused_before_anything_runs() {
    let repo = Repo::new("cap");
    repo.write("a.txt", "one\n");
    repo.commit("initial");
    let flood: Vec<String> = (0..SELECTION_MAX + 1)
        .map(|index| format!("file-{index:04}.txt"))
        .collect();

    let error = stage(&repo.root, &flood).expect_err("must refuse");

    assert_eq!(error, "too many paths in one request");
    assert_no_path(&error);
    assert!(repo.staged_names().is_empty(), "an over-cap selection ran");
}

/// Mutant `m:4` — let `""`/`"."` through: the workspace's whole tree goes
/// into the index from one frame — the `add -A` this design refuses —
/// while the owner believes only their own row was staged.
#[test]
fn the_workspaces_own_folder_is_refused_in_every_spelling() {
    let repo = Repo::new("root");
    repo.write("a.txt", "one\n");
    repo.commit("initial");
    repo.write("a.txt", "two\n");

    for requested in ["".to_string(), ".".to_string(), "./.".to_string()] {
        let error = stage(&repo.root, std::slice::from_ref(&requested)).expect_err("must refuse");
        assert_eq!(error, THE_ROOT, "spelling: {requested:?}");
        assert_no_path(&error);
        assert!(
            repo.staged_names().is_empty(),
            "the whole tree was staged from {requested:?}"
        );
    }
}

/// The happy path, all three shapes `git add` must cover for the panel's
/// rows: a modified file, a new file, and a tracked file's deletion. The
/// assertion is git's own `diff --cached --name-only`, not our belief.
#[test]
fn stage_names_a_modified_a_new_and_a_deleted_file_in_the_index() {
    let repo = Repo::new("shapes");
    repo.write("modified.txt", "one\n");
    repo.write("deleted.txt", "gone soon\n");
    repo.commit("initial");
    repo.write("modified.txt", "two\n");
    repo.write("fresh.txt", "new\n");
    std::fs::remove_file(repo.root.join("deleted.txt")).expect("delete");

    stage(
        &repo.root,
        &[
            "modified.txt".to_string(),
            "fresh.txt".to_string(),
            "deleted.txt".to_string(),
        ],
    )
    .expect("stage must succeed");

    let staged = repo.staged_names();
    for expected in ["modified.txt", "fresh.txt", "deleted.txt"] {
        assert!(
            staged.contains(expected),
            "`{expected}` not staged: {staged}"
        );
    }
}

/// Mutant `m:5` (the brief's `--` proof) — drop `--` from the argv: git
/// reads `-f` as its own force flag, prints "Nothing specified, nothing
/// added" **and exits 0** (measured), so the success branch passes while
/// the index stays empty — only this assertion kills it. The literal flag
/// does not cover this case: measured, `git add -- -f` stages the file
/// with or without `--literal-pathspecs`; `--` is what makes `-f` a path.
#[test]
fn a_file_named_dash_f_is_a_file_not_an_option() {
    let repo = Repo::new("dash-f");
    repo.write("a.txt", "one\n");
    repo.commit("initial");
    repo.write("-f", "the file is the argument\n");

    stage(&repo.root, &["-f".to_string()]).expect("stage must succeed");

    let staged = repo.staged_names();
    assert!(staged.contains("-f"), "`-f` was not staged: {staged}");
}

/// Mutant `m:6` (the literal-flag proof) — drop
/// `--literal-pathspecs`: `a[1].txt` is a glob that matches the
/// **neighbour** `a1.txt` too (measured on git 2.54.0: both end up
/// staged), so a row's stage reaches a file the owner never selected.
/// With the flag, the glob is a name and stages only itself.
#[test]
fn a_glob_spelling_stages_only_itself_and_never_its_neighbour() {
    let repo = Repo::new("glob");
    repo.write("a1.txt", "one\n");
    repo.write("a[1].txt", "two\n");
    repo.commit("initial");
    repo.write("a1.txt", "changed neighbour\n");
    repo.write("a[1].txt", "changed literal\n");

    stage(&repo.root, &["a[1].txt".to_string()]).expect("stage must succeed");

    let staged = repo.staged_names();
    assert!(
        staged.contains("a[1].txt"),
        "the requested file was not staged: {staged}"
    );
    assert!(
        !staged.contains("a1.txt"),
        "the glob staged its neighbour: {staged}"
    );
    assert!(
        output_of(repo.git(&["diff", "--name-only"]).expect("diff")).contains("a1.txt"),
        "the neighbour must still be modified-unstaged, untouched by the act"
    );
}

/// Not a repository → the probe's own sentence, before any `add`. The
/// mutation that kills it: classify `NotRepository` as `Ready`, and this
/// folder answers git's "not a git repository" exit instead — a sentence
/// no longer ours, and `is_git` reasoning upstream would believe it.
#[test]
fn a_folder_without_a_repository_is_refused_with_the_probes_sentence() {
    let folder = unique_directory("not-a-repo");
    std::fs::create_dir(&folder).expect("test directory");
    std::fs::write(folder.join("a.txt"), "one\n").expect("write");

    let error = stage(&folder, &["a.txt".to_string()]).expect_err("must refuse");
    let _ = std::fs::remove_dir_all(&folder);

    assert_eq!(error, "this workspace folder is not a git repository");
    assert_no_path(&error);
}

/// `index.lock` held — by the owner's own `git add` in a terminal, or a
/// stale file: git dies 128 with its absolute path in stderr (measured),
/// and the reply is the shared **static** sentence: no path, no stderr,
/// no machine spelling. Mutant `m:7` — hand `output.stderr` to the
/// sentence instead of this match, and the measured stderr above leaks
/// its full `C:\…` path through `assert_no_path`.
#[test]
fn a_held_index_lock_is_the_static_sentence_never_gits_stderr() {
    let repo = Repo::new("locked");
    repo.write("a.txt", "one\n");
    repo.commit("initial");
    repo.write("a.txt", "two\n");
    std::fs::write(repo.root.join(".git/index.lock"), []).expect("plant the lock");

    let error = stage(&repo.root, &["a.txt".to_string()]).expect_err("must fail");
    assert_eq!(error, INDEX_LOCKED);
    assert_no_path(&error);
    assert!(
        !error.to_lowercase().contains("index.lock"),
        "git's own wording leaked: {error}"
    );

    // The arbiter is external: clearing it (the owner's terminal finished)
    // makes the very same act succeed — the refusal named a lock, not a
    // broken act.
    std::fs::remove_file(repo.root.join(".git/index.lock")).expect("release the lock");
    stage(&repo.root, &["a.txt".to_string()]).expect("the same act after release");
    assert!(repo.staged_names().contains("a.txt"));
}

/// `write_failure`'s own rule, over a failure git did not attribute to
/// the lock: the sentence is the operation and the exit code, and stderr
/// — which the hook above filled with a path — stays dropped. This is the
/// shape the commit-with-failing-hook case reuses.
#[test]
fn a_failure_that_is_not_the_lock_is_operation_and_code_without_stderr() {
    let output = crate::git::GitOutput {
        success: false,
        code: Some(7),
        stdout: String::new(),
        stderr: "hook exploded in /home/someone/secret/file.rs".to_string(),
    };

    let error = write_failure("git commit", &output);

    assert_eq!(error, "git commit exited with code 7");
    assert_no_path(&error);
    assert!(!error.contains("hook exploded"), "stderr leaked: {error}");
}

/// The unstage's own truth (the brief's "cached vuoto e file ancora
/// modificato"): the index entry goes back to `HEAD`, the worktree keeps
/// its bytes. Mutant `u:1` — drop the reset entirely: the path stays
/// staged and `diff --cached` keeps naming it.
#[test]
fn unstage_empties_the_cached_side_and_keeps_the_worktree_bytes() {
    let repo = Repo::new("unstage");
    repo.write("a.txt", "one\n");
    repo.commit("initial");
    repo.write("a.txt", "staged-only\n");
    repo.run(&["add", "a.txt"]);

    unstage(&repo.root, &["a.txt".to_string()]).expect("unstage must succeed");

    assert!(
        repo.staged_names().is_empty(),
        "the cached side is not empty: {}",
        repo.staged_names()
    );
    let worktree = std::fs::read_to_string(repo.root.join("a.txt")).expect("read back");
    assert_eq!(worktree, "staged-only\n", "the worktree lost its bytes");
    let status = repo.git(&["status", "--porcelain"]).expect("status");
    assert!(
        output_of(status).contains("a.txt"),
        "the file must still read as modified"
    );
}

/// The brief's mandated "HEAD non nato" case: `git init`, stage, no
/// commit — unstage works (measured on git 2.54.0: `reset -q HEAD -- p`
/// itself exits 0 here and unstages, so this case passes with or without
/// the fallback; the mutant that kills is the broken-HEAD case below, and
/// that pairing is declared in the coder report).
#[test]
fn unstage_works_before_the_first_commit() {
    let repo = Repo::new("unborn");
    repo.write("new.txt", "brand new\n");
    repo.run(&["add", "new.txt"]);

    unstage(&repo.root, &["new.txt".to_string()]).expect("unstage must succeed on unborn HEAD");

    let status = output_of(repo.git(&["status", "--porcelain"]).expect("status"));
    assert!(status.contains("?? new.txt"), "not untracked: {status}");
    assert!(!status.contains("A  new.txt"), "still staged: {status}");
    assert_eq!(
        std::fs::read_to_string(repo.root.join("new.txt")).expect("read back"),
        "brand new\n",
        "the worktree lost its bytes"
    );
}

/// Mutant `u:2` — drop the `rm --cached` fallback: an `HEAD` that does
/// not resolve makes `git reset HEAD -- p` die 128 (measured, stderr
/// `Could not parse object 'HEAD'`), and without the fallback the index
/// keeps the path — this assertion (`ls-files` reads only the index, so
/// it works where `status` dies `bad object HEAD`, measured) fails.
#[test]
fn unstage_falls_back_when_head_does_not_resolve() {
    let repo = Repo::new("broken-head");
    repo.write("f.txt", "one\n");
    repo.commit("initial");
    let branch = output_of(repo.git(&["symbolic-ref", "--short", "HEAD"]).expect("ref"))
        .trim()
        .to_string();
    std::fs::write(
        repo.root.join(".git/refs/heads").join(&branch),
        "0000000000000000000000000000000000000001\n",
    )
    .expect("break HEAD");
    repo.write("f.txt", "staged over the break\n");
    repo.run(&["add", "f.txt"]);

    unstage(&repo.root, &["f.txt".to_string()])
        .expect("the fallback must complete the act when reset cannot");

    let indexed = output_of(
        repo.git(&["ls-files", "--", "f.txt"])
            .expect("ls-files reads the index alone"),
    );
    assert!(indexed.trim().is_empty(), "still staged: {indexed}");
    assert_eq!(
        std::fs::read_to_string(repo.root.join("f.txt")).expect("read back"),
        "staged over the break\n",
        "the worktree lost its bytes"
    );
}

/// Discard's three measured truths in one act (Paseo
/// `checkout-git.test.ts:3827`): the tracked selection returns to `HEAD`
/// — staged change included: discard discards, it does not merely
/// unstage (the brief's "staged-only diventa unstaged col contenuto che
/// resta" is this sequence's state after the reset step only, and the
/// unstage truth above; through the full sequence the content returns to
/// `HEAD`, measured twice on git 2.54.0 — declared in the coder report) —
/// the untracked selection disappears, and an unselected file keeps its
/// own bytes. Mutant `d:1` — drop the `clean` step: `fresh.txt` survives.
/// Mutant `d:2` — drop the `checkout` step: `a.txt` keeps its change.
#[test]
fn discard_returns_the_tracked_to_head_deletes_the_untracked_and_leaves_the_unselected_alone() {
    let repo = Repo::new("discard");
    repo.write("a.txt", "committed\n");
    repo.write("b.txt", "committed too\n");
    repo.commit("initial");
    repo.write("a.txt", "discarded\n");
    repo.run(&["add", "a.txt"]); // staged-only change on a.txt
    repo.write("fresh.txt", "untracked\n");
    repo.write("b.txt", "unselected keeps me\n");

    discard(&repo.root, &["a.txt".to_string(), "fresh.txt".to_string()])
        .expect("discard must succeed");

    assert_eq!(
        std::fs::read_to_string(repo.root.join("a.txt")).expect("read back"),
        "committed\n",
        "the tracked selection did not return to HEAD"
    );
    assert!(
        !repo.root.join("fresh.txt").exists(),
        "the untracked selection survived"
    );
    assert_eq!(
        std::fs::read_to_string(repo.root.join("b.txt")).expect("read back"),
        "unselected keeps me\n",
        "an unselected file was touched"
    );
    let status = output_of(repo.git(&["status", "--porcelain"]).expect("status"));
    assert_eq!(
        status.trim_end(),
        " M b.txt",
        "the selection left residue: {status}"
    );
}

/// Paseo's second truth (`checkout-git.test.ts:3865`): a **staged new
/// file** in a repository with no commit yet is deleted by a discard —
/// unstage (reset succeeds on this git even unborn, measured) turns it
/// untracked, and `clean -fd` removes it. Mutant `u:1` again: without
/// the reset the file reads `A ` (tracked), `checkout` cannot restore a
/// path no `HEAD` knows, and the file survives.
#[test]
fn discard_removes_a_staged_new_file_on_a_repository_without_a_commit() {
    let repo = Repo::new("discard-unborn");
    repo.write("new.txt", "staged new\n");
    repo.run(&["add", "new.txt"]);

    discard(&repo.root, &["new.txt".to_string()]).expect("discard must succeed");

    assert!(
        !repo.root.join("new.txt").exists(),
        "the staged new file survived the discard"
    );
    let status = output_of(repo.git(&["status", "--porcelain"]).expect("status"));
    assert_eq!(status.trim_end(), "", "residue: {status}");
}

/// Mutant `m:2` again through this act's own door: the refusal happens
/// before any spawn, and the proof is that an index entry the act never
/// touched is still there afterwards.
#[test]
fn discard_refuses_git_metadata_before_touching_the_index() {
    let repo = Repo::new("discard-git-metadata");
    repo.write("a.txt", "one\n");
    repo.commit("initial");
    repo.write("a.txt", "staged and precious\n");
    repo.run(&["add", "a.txt"]);

    let error = discard(&repo.root, &[".git/config".to_string()]).expect_err("must refuse");

    assert_eq!(error, NOT_PART_OF_THE_TREE);
    assert_no_path(&error);
    assert!(
        repo.staged_names().contains("a.txt"),
        "a refused selection changed the index"
    );
}

/// The commit is **staged only**: what the owner staged enters history,
/// what they did not stays exactly as it was — the divergence from
/// Paseo's `commitChanges` (`addAll` default `true`) that the whole
/// explicit stage exists for. Mutant `c:1` — reintroduce `add -A`
/// before the commit: `b.txt` lands in the commit and this assertion
/// fails.
#[test]
fn commit_takes_only_what_is_staged_and_generates_nothing() {
    let repo = Repo::new("commit-staged");
    repo.write("a.txt", "one\n");
    repo.write("b.txt", "one\n");
    repo.commit("initial");
    repo.write("a.txt", "staged change\n");
    repo.run(&["add", "a.txt"]);
    repo.write("b.txt", "unstaged change\n");
    repo.write("fresh.txt", "untracked\n");

    commit(&repo.root, "only the staged half").expect("commit must succeed");

    let committed = output_of(
        repo.git(&["show", "--stat", "--format=", "--name-only", "HEAD"])
            .expect("show"),
    );
    assert!(
        committed.contains("a.txt"),
        "the staged file missed: {committed}"
    );
    assert!(
        !committed.contains("b.txt"),
        "an unstaged file entered the commit: {committed}"
    );
    assert!(
        !committed.contains("fresh.txt"),
        "an untracked file entered the commit: {committed}"
    );
    let status = output_of(repo.git(&["status", "--porcelain"]).expect("status"));
    assert!(
        status.contains("b.txt"),
        "the unstaged half vanished: {status}"
    );
    assert!(
        status.contains("fresh.txt"),
        "the untracked half vanished: {status}"
    );
    assert_eq!(
        std::fs::read_to_string(repo.root.join("a.txt")).expect("read back"),
        "staged change\n"
    );
}

/// An empty message is refused **before the probe runs** — spelling-only,
/// no spawn — and nothing was committed. Mutant `c:2` — drop the trim
/// check: `"   "` reaches `git commit -m`, git's cleanup turns it into
/// an empty message and dies 1 — a machine's failure sentence instead of
/// ours, and this equality fails.
#[test]
fn commit_without_a_message_is_refused_before_anything_spawns() {
    let repo = Repo::new("commit-empty");
    repo.write("a.txt", "one\n");
    repo.commit("initial");
    repo.write("a.txt", "staged\n");
    repo.run(&["add", "a.txt"]);
    let head_before = output_of(repo.git(&["rev-parse", "HEAD"]).expect("head"));

    for message in ["", "   ", "\t\n"] {
        let error = commit(&repo.root, message).expect_err("must refuse");
        assert_eq!(error, "the commit message is empty", "message: {message:?}");
        assert_no_path(&error);
    }

    let head_after = output_of(repo.git(&["rev-parse", "HEAD"]).expect("head"));
    assert_eq!(head_before, head_after, "a refused commit moved HEAD");
}

/// A hook that dies answers as operation plus exit code, never its
/// stderr: measured on this machine a `pre-commit` exiting **7** is
/// reported by `git commit` as exit **1**, and the hook's stderr — which
/// this fixture fills with an absolute path — stays dropped. Mutant
/// `m:7` again, from the live road this time: hand stderr to the
/// sentence and `assert_no_path` dies on the planted `/home/...` path.
#[test]
fn a_failing_hook_answers_with_the_exit_code_and_never_its_stderr() {
    let repo = Repo::new("commit-hook");
    repo.write("a.txt", "one\n");
    repo.commit("initial");
    repo.write("a.txt", "staged\n");
    repo.run(&["add", "a.txt"]);
    let hook = "#!/bin/sh\necho \"hook exploded in /home/someone/secret/file.rs\" >&2\nexit 7\n";
    std::fs::write(repo.root.join(".git/hooks/pre-commit"), hook).expect("hook");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            repo.root.join(".git/hooks/pre-commit"),
            std::fs::Permissions::from_mode(0o755),
        )
        .expect("chmod");
    }
    let head_before = output_of(repo.git(&["rev-parse", "HEAD"]).expect("head"));

    let error = commit(&repo.root, "hooked").expect_err("the hook must fail the commit");

    assert_eq!(error, "git commit exited with code 1");
    assert_no_path(&error);
    assert!(!error.contains("hook exploded"), "stderr leaked: {error}");
    let head_after = output_of(repo.git(&["rev-parse", "HEAD"]).expect("head"));
    assert_eq!(head_before, head_after, "the failed commit moved HEAD");
}

/// The commit refuses a folder git cannot serve — same probe sentence as
/// the rest of the module, before anything spawns.
#[test]
fn commit_in_a_folder_without_a_repository_is_refused() {
    let folder = unique_directory("commit-not-a-repo");
    std::fs::create_dir(&folder).expect("test directory");
    std::fs::write(folder.join("a.txt"), "one\n").expect("write");

    let error = commit(&folder, "anything").expect_err("must refuse");
    let _ = std::fs::remove_dir_all(&folder);

    assert_eq!(error, "this workspace folder is not a git repository");
    assert_no_path(&error);
}

/// stdout of one fixture git call as an owned string (the command must
/// have succeeded — these are assertions' reads of git's own answer).
fn output_of(output: std::process::Output) -> String {
    assert!(
        output.status.success(),
        "fixture git failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}
