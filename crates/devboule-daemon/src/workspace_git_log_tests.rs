//! `log_of` over real repositories built in `%TEMP%` — the cases that need
//! a process; the pure parsing rules live in
//! `workspace_git_log_parse_tests.rs`. The histories and the classification
//! rules are Paseo's own
//! (`packages/server/src/utils/checkout-git.commits.test.ts`), translated.

use std::io::Write;
use std::process::{Command, Stdio};

use super::log_of;
use crate::workspace_git_support::{INSIDE_A_REPOSITORY, NOT_A_REPOSITORY};

/// The committer line every fast-import stream in this file carries.
const COMMITTER: &str = "Test User <test@devboule.local>";

fn unique_directory(label: &str) -> std::path::PathBuf {
    crate::test_dirs::test_temp_dir(&format!("devboule-workspace-log-{label}"))
}

/// A repository under `temp_dir`, pinned against the machine it runs on:
/// line-ending conversion changes what a numstat counts, and commit signing
/// can fail on someone else's key. Hard-requires git — a silent skip would
/// let every case below pass without running one.
struct Repo {
    root: std::path::PathBuf,
}

impl Repo {
    fn new(label: &str) -> Self {
        let root = unique_directory(label);
        let repo = Self { root };
        repo.run(&["init", "--quiet", "-b", "main"]);
        repo.run(&["config", "user.email", "test@devboule.local"]);
        repo.run(&["config", "user.name", "Test User"]);
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

    fn run_output(&self, arguments: &[&str]) -> String {
        let output = self.git(arguments).expect("git could not be spawned");
        assert!(
            output.status.success(),
            "git {arguments:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    fn run_with_input(&self, arguments: &[&str], input: &[u8]) {
        let mut command = Command::new("git");
        command.arg("-C").arg(&self.root).args(arguments);
        command.stdin(Stdio::piped());
        command.stderr(Stdio::piped());
        let mut child = command.spawn().expect("git could not be spawned");
        child
            .stdin
            .take()
            .expect("git stdin")
            .write_all(input)
            .expect("git stdin write");
        let output = child.wait_with_output().expect("git wait");
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

    /// One commit on the current branch with `message` as its raw bytes —
    /// fast-import stores them verbatim, unlike `git commit -F`, which
    /// transcodes a non-UTF-8 message to UTF-8 on the way in.
    fn import_commit_with_message(&self, message: &[u8]) {
        let parent = self.run_output(&["rev-parse", "HEAD"]).trim().to_string();
        let branch = self
            .run_output(&["branch", "--show-current"])
            .trim()
            .to_string();
        let mut stream: Vec<u8> = format!(
            "commit refs/heads/{branch}\nmark :1\ncommitter {COMMITTER} 1700000000 +0000\ndata {}\n",
            message.len(),
        )
        .into_bytes();
        stream.extend_from_slice(message);
        stream.extend_from_slice(format!("\nfrom {parent}\n\n").as_bytes());
        self.run_with_input(&["fast-import", "--quiet"], &stream);
        self.run(&["reset", "--quiet", "--hard", &branch]);
    }

    /// A chain of `count` commits on `branch`, each setting `file` and
    /// committing with `subject(index)` — built in one `git fast-import`
    /// process.
    fn import_linear_history(
        &self,
        branch: &str,
        file: &str,
        subject: impl Fn(usize) -> String,
        count: usize,
    ) {
        let mut commands = String::new();
        let mut parent = self.run_output(&["rev-parse", "HEAD"]).trim().to_string();
        for index in 1..=count {
            let content = format!("{index}\n");
            let blob_mark = index * 2 - 1;
            let commit_mark = index * 2;
            let message = subject(index);
            commands.push_str(&format!(
                "blob\nmark :{blob_mark}\ndata {}\n{content}commit refs/heads/{branch}\nmark \
                 :{commit_mark}\ncommitter {COMMITTER} {} +0000\ndata {}\n{message}\nfrom \
                 {parent}\nM 100644 :{blob_mark} {file}\n\n",
                content.len(),
                1_700_000_000 + index,
                message.len(),
            ));
            parent = format!(":{commit_mark}");
        }
        self.run_with_input(&["fast-import", "--quiet"], commands.as_bytes());
        self.run(&["reset", "--quiet", "--hard", branch]);
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// No absolute path in a sentence that leaves this machine: `error` is not
/// redacted on its way out (see `WorkspaceGitLog`), so the message is the
/// guard.
fn assert_no_path(message: &str) {
    let pathish = message.contains('\\') || message.contains('/') || message.contains(':');
    assert!(!pathish, "a path leaked into `error`: {message}");
}

/// On the base branch itself: no base to split from, so the reply is the
/// branch's own recent history — every commit `is_on_base`, and
/// `is_on_remote` false because the repository has no remote.
#[test]
fn a_repository_on_its_default_branch_lists_its_own_recent_history() {
    let repo = Repo::new("on-default");
    repo.write("file.txt", "one\ntwo\n");
    repo.commit("initial");

    let log = log_of(&repo.root, None);

    assert_eq!(log.error, None, "{:?}", log.error);
    assert_eq!(log.base_ref, None, "on the base branch there is no split");
    assert_eq!(log.commits.len(), 1);
    assert_eq!(log.commits[0].subject, "initial");
    assert!(log.commits[0].is_on_base);
    assert!(
        !log.commits[0].is_on_remote,
        "no remote means nothing is pushed"
    );
}

/// A feature branch cut from `main`: the workspace's own commits first,
/// then the base branch's history back to the fork point — pushed commits
/// are on the remote, the local one is not.
#[test]
fn a_feature_branch_lists_its_own_commits_then_the_base_history() {
    let repo = Repo::new("feature");
    repo.write("file.txt", "base\n");
    repo.commit("initial");
    repo.run(&["checkout", "-qb", "feature"]);
    repo.write("foo.txt", "a\nb\nc\n");
    repo.commit("Add foo");
    // A bare remote holding the pushed feature branch, then one more local
    // commit.
    let remote = unique_directory("feature-remote");
    repo.run(&[
        "init",
        "--quiet",
        "--bare",
        "-b",
        "feature",
        remote.to_str().expect("path"),
    ]);
    repo.run(&["remote", "add", "origin", remote.to_str().expect("path")]);
    repo.run(&["push", "-q", "-u", "origin", "feature"]);
    repo.write("bar.txt", "x\n");
    repo.commit("Add bar");

    let log = log_of(&repo.root, Some("main"));

    assert_eq!(log.error, None, "{:?}", log.error);
    assert_eq!(log.base_ref.as_deref(), Some("main"));
    let subjects: Vec<&str> = log
        .commits
        .iter()
        .map(|commit| commit.subject.as_str())
        .collect();
    assert_eq!(subjects, ["Add bar", "Add foo", "initial"]);
    let flags: Vec<(bool, bool)> = log
        .commits
        .iter()
        .map(|commit| (commit.is_on_remote, commit.is_on_base))
        .collect();
    assert_eq!(
        flags,
        [(false, false), (true, false), (true, true)],
        "the local commit is on no remote; the fork point is base history"
    );
}

/// Mutant `m:a` end to end: the base list stops at ten commits however long
/// the base branch is.
#[test]
fn the_base_list_is_capped_at_ten_commits() {
    let repo = Repo::new("base-cap");
    repo.write("file.txt", "base\n");
    repo.commit("initial");
    repo.import_linear_history("main", "history.txt", |index| format!("Commit {index}"), 14);

    let log = log_of(&repo.root, None);

    assert_eq!(log.error, None, "{:?}", log.error);
    assert_eq!(log.base_ref, None);
    assert_eq!(log.commits.len(), 10, "the cap, not the branch length");
    assert!(log.commits.iter().all(|commit| commit.is_on_base));
    assert_eq!(log.commits[0].subject, "Commit 14");
    assert_eq!(log.commits[9].subject, "Commit 5");
}

/// When the base branch has advanced past the fork point, the base list
/// starts at the merge base — the base's newer commits are not the
/// workspace's history.
#[test]
fn the_base_list_starts_at_the_fork_point_when_the_base_has_advanced() {
    let repo = Repo::new("fork-point");
    repo.write("file.txt", "base\n");
    repo.commit("initial");
    repo.write("shared.txt", "shared\n");
    repo.commit("Shared base");
    repo.run(&["checkout", "-qb", "feature"]);
    repo.write("feature.txt", "feature\n");
    repo.commit("Feature work");
    repo.run(&["checkout", "-q", "main"]);
    repo.write("newer-base.txt", "newer\n");
    repo.commit("Newer base");
    repo.run(&["checkout", "-q", "feature"]);

    let log = log_of(&repo.root, Some("main"));

    let classified: Vec<(&str, bool)> = log
        .commits
        .iter()
        .map(|commit| (commit.subject.as_str(), commit.is_on_base))
        .collect();
    assert_eq!(
        classified,
        [
            ("Feature work", false),
            ("Shared base", true),
            ("initial", true),
        ],
        "the merge base starts the base list; the advanced base commit stays out"
    );
}

/// Local commits on the base branch after the fork are base history, not
/// workspace history — they are reachable from the base ref, so the
/// workspace list never carried them.
#[test]
fn local_base_commits_stay_out_of_the_workspace_history() {
    let repo = Repo::new("local-base");
    repo.write("file.txt", "base\n");
    repo.commit("initial");
    let remote = unique_directory("local-base-remote");
    repo.run(&[
        "init",
        "--quiet",
        "--bare",
        "-b",
        "main",
        remote.to_str().expect("path"),
    ]);
    repo.run(&["remote", "add", "origin", remote.to_str().expect("path")]);
    repo.run(&["push", "-q", "-u", "origin", "main"]);
    repo.write("local-base.txt", "base\n");
    repo.commit("Local base work");
    repo.run(&["checkout", "-qb", "feature"]);
    repo.write("feature.txt", "feature\n");
    repo.commit("Feature work");

    let log = log_of(&repo.root, Some("main"));

    let classified: Vec<(&str, bool)> = log
        .commits
        .iter()
        .map(|commit| (commit.subject.as_str(), commit.is_on_base))
        .collect();
    assert_eq!(
        classified,
        [
            ("Feature work", false),
            ("Local base work", true),
            ("initial", true),
        ],
        "the local base commit is base history the workspace list never carried"
    );
    // The combined limit: every workspace commit, then at most ten of
    // the base.
    assert_eq!(log.commits.len(), 3);
}

/// The combined shape: twenty-four workspace commits, then the ten
/// the cap allows of a fourteen-commit base.
#[test]
fn every_workspace_commit_precedes_the_capped_base_list() {
    let repo = Repo::new("combined");
    repo.write("file.txt", "base\n");
    repo.commit("initial");
    repo.import_linear_history(
        "main",
        "base-history.txt",
        |index| format!("Base {index}"),
        14,
    );
    repo.run(&["checkout", "-qb", "feature"]);
    repo.import_linear_history(
        "feature",
        "workspace-history.txt",
        |index| format!("Workspace {index}"),
        24,
    );

    let log = log_of(&repo.root, Some("main"));

    assert_eq!(log.error, None, "{:?}", log.error);
    assert_eq!(log.commits.len(), 34, "24 workspace + 10 base");
    assert!(!log.commits[0].is_on_base);
    assert_eq!(log.commits[0].subject, "Workspace 24");
    assert!(log.commits[..24].iter().all(|commit| !commit.is_on_base));
    assert!(log.commits[24..].iter().all(|commit| commit.is_on_base));
    assert_eq!(log.commits[24].subject, "Base 14");
    assert_eq!(log.commits[33].subject, "Base 5");
}

/// A multi-line message contributes its first line only.
#[test]
fn a_multi_line_message_contributes_its_first_line() {
    let repo = Repo::new("multi-line");
    repo.write("file.txt", "base\n");
    repo.commit("initial");
    repo.run(&["checkout", "-qb", "feature"]);
    repo.run(&[
        "commit",
        "--quiet",
        "--allow-empty",
        "-m",
        "subject line",
        "-m",
        "body paragraph",
    ]);

    let log = log_of(&repo.root, Some("main"));

    assert_eq!(log.error, None, "{:?}", log.error);
    assert_eq!(log.commits[0].subject, "subject line");
}

/// A tab inside the subject travels whole — the field separator is NUL.
#[test]
fn a_tab_inside_the_subject_travels_whole() {
    let repo = Repo::new("tab-subject");
    repo.write("file.txt", "base\n");
    repo.commit("initial");
    repo.run(&["checkout", "-qb", "feature"]);
    repo.run(&["commit", "--quiet", "--allow-empty", "-m", "with\ttab"]);

    let log = log_of(&repo.root, Some("main"));

    assert_eq!(log.error, None, "{:?}", log.error);
    assert_eq!(log.commits[0].subject, "with\ttab");
}

/// A subject with a non-UTF-8 byte arrives lossy-converted: the shared git
/// runner converts every command's stdout, and the parser carries whatever
/// the runner handed it. `git commit -F` cannot produce this case — it
/// transcodes a non-UTF-8 message to UTF-8 — so the commit is built by
/// fast-import, which stores the bytes verbatim.
#[test]
fn a_non_utf8_subject_arrives_lossy_converted() {
    let repo = Repo::new("non-utf8");
    repo.write("file.txt", "base\n");
    repo.commit("initial");
    repo.run(&["checkout", "-qb", "feature"]);
    repo.import_commit_with_message(b"caf\xe9 subject\n");

    let log = log_of(&repo.root, Some("main"));

    assert_eq!(log.error, None, "{:?}", log.error);
    assert_eq!(log.commits[0].subject, "caf\u{fffd} subject");
}

/// A detached HEAD outside a rebase has no branch to split from: the answer
/// is an empty history, not a refusal.
#[test]
fn a_detached_head_answers_with_an_empty_history() {
    let repo = Repo::new("detached");
    repo.write("file.txt", "base\n");
    repo.commit("initial");
    repo.run(&["checkout", "--quiet", "--detach", "HEAD"]);

    let log = log_of(&repo.root, Some("main"));

    assert_eq!(log.error, None, "{:?}", log.error);
    assert_eq!(log.base_ref, None);
    assert!(log.commits.is_empty());
}

/// A rebase keeps its branch: git records it in `.git/rebase-merge/head-name`,
/// and the read resolves the branch from there — the answer is the history
/// of the rebasing branch, not the detached-HEAD empty one.
#[test]
fn a_detached_head_inside_a_rebase_keeps_its_branch() {
    let repo = Repo::new("rebase");
    repo.write("file.txt", "base\n");
    repo.commit("initial");
    repo.run(&["checkout", "-qb", "feature"]);
    repo.write("file.txt", "feature\n");
    repo.commit("Feature work");
    repo.run(&["checkout", "-q", "main"]);
    repo.write("file.txt", "main work\n");
    repo.commit("Main work");
    repo.run(&["checkout", "-q", "feature"]);
    // The rebase stops on the conflict between the two edits of file.txt.
    let output = repo
        .git(&["rebase", "main"])
        .expect("git could not be spawned");
    assert!(
        !output.status.success(),
        "the rebase must stop on the conflict"
    );
    assert!(
        repo.root.join(".git/rebase-merge/head-name").exists(),
        "the rebase state file must exist"
    );

    let log = log_of(&repo.root, Some("main"));

    assert_eq!(log.error, None, "{:?}", log.error);
    // The branch was recovered: the split ran against `main` instead of
    // answering the detached-HEAD empty history.
    assert_eq!(log.base_ref.as_deref(), Some("main"));
    let subjects: Vec<&str> = log
        .commits
        .iter()
        .map(|commit| commit.subject.as_str())
        .collect();
    assert_eq!(subjects, ["Main work", "initial"]);
}

/// An oversized rebase state file is refused, never read whole: the file is
/// the checkout's to ship, so its size is not ours to trust. A detached HEAD
/// whose `rebase-merge/head-name` carries a mebibyte answers no branch at
/// all — the empty-history answer — instead of allocating the file whole.
#[test]
fn an_oversized_rebase_head_name_is_rejected_not_read() {
    let repo = Repo::new("rebase-oversized");
    repo.write("file.txt", "base\n");
    repo.commit("initial");
    repo.run(&["checkout", "--quiet", "--detach", "HEAD"]);
    let rebase_merge = repo.root.join(".git/rebase-merge");
    std::fs::create_dir_all(&rebase_merge).expect("rebase state dir");
    std::fs::write(rebase_merge.join("head-name"), vec![b'a'; 1024 * 1024])
        .expect("oversized head-name");

    assert_eq!(
        super::base::current_branch(&repo.root),
        None,
        "a mebibyte head-name must not become a branch"
    );
}

/// A repository with no commit yet has no branch: the answer is an empty
/// history, not a refusal.
#[test]
fn a_repository_with_no_commit_answers_with_an_empty_history() {
    let repo = Repo::new("no-commits");

    let log = log_of(&repo.root, None);

    assert_eq!(log.error, None, "{:?}", log.error);
    assert_eq!(log.base_ref, None);
    assert!(log.commits.is_empty());
}

/// A saved base that no longer exists falls back to the repository's
/// default branch — the worktree metadata outliving a renamed or deleted
/// base branch.
#[test]
fn a_saved_base_that_no_longer_exists_falls_back_to_the_default_branch() {
    let repo = Repo::new("deleted-base");
    repo.write("file.txt", "base\n");
    repo.commit("initial");
    repo.run(&["checkout", "-qb", "feature"]);
    repo.write("feature.txt", "feature\n");
    repo.commit("Feature work");

    let log = log_of(&repo.root, Some("deleted-base"));

    assert_eq!(log.error, None, "{:?}", log.error);
    assert_eq!(log.base_ref.as_deref(), Some("main"), "the default branch");
    let classified: Vec<(&str, bool)> = log
        .commits
        .iter()
        .map(|commit| (commit.subject.as_str(), commit.is_on_base))
        .collect();
    assert_eq!(classified, [("Feature work", false), ("initial", true)]);
}

/// A fully qualified base ref is verified as-is — a caller who named an
/// exact ref meant it.
#[test]
fn a_qualified_base_ref_resolves_as_is() {
    let repo = Repo::new("qualified");
    repo.write("file.txt", "base\n");
    repo.commit("initial");
    repo.run(&["checkout", "-qb", "feature"]);
    repo.write("feature.txt", "feature\n");
    repo.commit("Feature work");

    let log = log_of(&repo.root, Some("refs/heads/main"));

    assert_eq!(log.error, None, "{:?}", log.error);
    assert_eq!(log.base_ref.as_deref(), Some("refs/heads/main"));
    assert_eq!(log.commits.len(), 2);
}

/// A qualified base ref that does not exist is the caller's error: the
/// refusal answers with no commits and the sentence, never a fallback.
#[test]
fn a_qualified_base_ref_that_does_not_exist_is_a_refusal() {
    let repo = Repo::new("qualified-gone");
    repo.write("file.txt", "base\n");
    repo.commit("initial");
    repo.run(&["checkout", "-qb", "feature"]);
    repo.write("feature.txt", "feature\n");
    repo.commit("Feature work");

    let log = log_of(&repo.root, Some("refs/heads/gone"));

    assert_eq!(log.base_ref, None);
    assert!(log.commits.is_empty());
    let error = log.error.expect("a refusal carries its sentence");
    // The sentence names no ref: a git ref is not a filesystem path, but
    // the house rule is that `error` carries no path-like text, and the
    // helper that enforces it cannot tell a ref's `/` from a path's. The
    // ref came from the persisted workspace record, not the caller — the
    // frame carries only a workspace id.
    assert_eq!(error, "the base ref does not exist");
    assert_no_path(&error);
}

/// A history past the log read's own cap is flagged, not emptied: the
/// records parsed up to the last complete one are the newest commits, and
/// the sentence says the oldest are missing. The cap is sized to the two
/// commit limits git applies, so reaching it takes records averaging more
/// than the generous per-record size — this is the stress case, not a
/// realistic one.
#[test]
fn a_history_past_the_reply_cap_is_flagged_not_emptied() {
    let repo = Repo::new("truncated");
    repo.write("file.txt", "base\n");
    repo.commit("initial");
    repo.run(&["checkout", "-qb", "feature"]);
    // 200 commits (the workspace limit) with ~1,000-char subjects: ~1,085
    // bytes per record, ~217 KB in total — past the ~215 KB ceiling.
    repo.import_linear_history("feature", "history.txt", |_| "x".repeat(1_000), 200);

    let log = log_of(&repo.root, Some("main"));

    assert!(
        !log.commits.is_empty(),
        "a cut-short list keeps the newest commits"
    );
    assert!(
        log.commits.len() < 210,
        "the cap cut the list short (the full list would be 200 workspace + 10 base)"
    );
    let error = log.error.expect("a cut-short list is flagged");
    assert!(error.contains("reply cap"), "{error}");
    assert_no_path(&error);
}

/// A branch longer than the workspace limit ships its newest commits and
/// the truncation flag — the count cap discloses itself the way the byte
/// cap does. On `bc908373` the flag is absent (git stopped at the limit and
/// the daemon never consulted it) and this fails.
#[test]
fn a_branch_past_the_commit_limit_is_flagged() {
    let repo = Repo::new("count-cap");
    repo.write("file.txt", "base\n");
    repo.commit("initial");
    repo.run(&["checkout", "-qb", "feature"]);
    repo.import_linear_history(
        "feature",
        "history.txt",
        |index| format!("Workspace {index}"),
        250,
    );

    let log = log_of(&repo.root, Some("main"));

    // The limit's 200 workspace commits plus the base branch's one commit.
    assert_eq!(log.commits.len(), 201);
    assert!(log.commits[..200].iter().all(|commit| !commit.is_on_base));
    assert!(log.commits[200..].iter().all(|commit| commit.is_on_base));
    let error = log.error.expect("a cut-short list is flagged");
    assert!(error.contains("reply cap"), "{error}");
    assert_no_path(&error);
}

/// A shallow clone answers like any repository: git fills in the boundary,
/// and the read's git commands all work against it.
#[test]
fn a_shallow_clone_answers_like_any_repository() {
    let repo = Repo::new("shallow-source");
    repo.write("file.txt", "one\n");
    repo.commit("first");
    repo.write("file.txt", "two\n");
    repo.commit("second");
    repo.write("file.txt", "three\n");
    repo.commit("third");
    let clone = unique_directory("shallow-clone");
    // A local-path clone ignores `--depth`; the file:// transport honors it.
    let source = format!("file://{}", repo.root.to_str().expect("path"));
    repo.run(&[
        "clone",
        "--quiet",
        "--depth",
        "1",
        &source,
        clone.to_str().expect("path"),
    ]);

    let log = log_of(&clone, None);

    assert_eq!(log.error, None, "{:?}", log.error);
    assert_eq!(log.commits.len(), 1, "the depth is one");
    assert_eq!(log.commits[0].subject, "third");
    assert!(log.commits[0].is_on_base);
}

/// A folder that is not a repository is refused with the shared sentence —
/// the same one the status read answers with.
#[test]
fn a_folder_that_is_not_a_repository_is_refused() {
    let dir = unique_directory("not-a-repo");

    let log = log_of(&dir, None);

    assert_eq!(log.error.as_deref(), Some(NOT_A_REPOSITORY));
    assert_eq!(log.base_ref, None);
    assert!(log.commits.is_empty());
}

/// A realistic history fits under the log read's own cap: a hundred
/// commits with multi-file changes and long subjects — the payload that
/// the shared 16 KiB ceiling refused whole (measured at ~1,560 bytes per
/// commit with the files array, ~330 without). The boundary to pin: the
/// whole history comes back.
#[test]
fn a_realistic_history_fits_under_the_reply_cap() {
    let repo = Repo::new("realistic");
    repo.write("file.txt", "base\n");
    repo.commit("initial");
    repo.run(&["checkout", "-qb", "feature"]);
    // A hundred commits, each touching three files with a long subject —
    // the cost driver is the subject (the frame carries no files array),
    // and these are paragraph-length.
    let long_subject = ["a long subject line"; 12].join(" ");
    repo.import_linear_history(
        "feature",
        "history.txt",
        |index| format!("Workspace {index}: {long_subject}"),
        100,
    );

    let log = log_of(&repo.root, Some("main"));

    assert_eq!(log.error, None, "{:?}", log.error);
    assert_eq!(log.base_ref.as_deref(), Some("main"));
    // The whole workspace history plus the base branch's one commit.
    assert_eq!(log.commits.len(), 101);
    assert_eq!(
        log.commits[0].subject,
        format!("Workspace 100: {}", ["a long subject line"; 12].join(" "))
    );
    assert!(log.commits[..100].iter().all(|commit| !commit.is_on_base));
    assert!(log.commits[100..].iter().all(|commit| commit.is_on_base));
}

/// A merge commit is listed like any other: `--diff-merges=first-parent`
/// is carried in the argv (its diff effect is unobservable without diff
/// output, but the flag is carried for the commit-diff read that re-adds
/// it), and the merge's subject and position are pinned.
#[test]
fn a_merge_commit_is_listed_with_its_subject() {
    let repo = Repo::new("merge");
    repo.write("file.txt", "base\n");
    repo.commit("initial");
    repo.run(&["checkout", "-qb", "feature"]);
    repo.write("feature.txt", "feature\n");
    repo.commit("Add feature");
    repo.run(&["checkout", "-q", "main"]);
    repo.write("main.txt", "main\n");
    repo.commit("Advance main");
    repo.run(&["merge", "--no-ff", "feature", "--message", "Merge feature"]);

    let log = log_of(&repo.root, None);

    assert_eq!(log.error, None, "{:?}", log.error);
    let subjects: Vec<&str> = log
        .commits
        .iter()
        .map(|commit| commit.subject.as_str())
        .collect();
    assert_eq!(
        subjects,
        ["Merge feature", "Advance main", "Add feature", "initial"]
    );
}

/// The `origin > local` arm of the base resolution: when the remote base
/// is ahead of the local one, the comparison runs against `origin/<name>`.
#[test]
fn a_remote_base_ahead_of_the_local_one_is_the_comparison() {
    let repo = Repo::new("origin-ahead");
    repo.write("file.txt", "base\n");
    repo.commit("initial");
    let remote = unique_directory("origin-ahead-remote");
    repo.run(&[
        "init",
        "--quiet",
        "--bare",
        "-b",
        "main",
        remote.to_str().expect("path"),
    ]);
    repo.run(&["remote", "add", "origin", remote.to_str().expect("path")]);
    repo.run(&["push", "-q", "-u", "origin", "main"]);
    // Local main advances past the remote, then resets back: origin/main
    // is now the ahead one.
    repo.write("local-only.txt", "local\n");
    repo.commit("Local only");
    repo.run(&["push", "-q", "origin", "main"]);
    repo.run(&["reset", "--quiet", "--hard", "HEAD~1"]);
    repo.run(&["checkout", "-qb", "feature"]);
    repo.write("feature.txt", "feature\n");
    repo.commit("Feature work");

    let log = log_of(&repo.root, Some("main"));

    assert_eq!(log.error, None, "{:?}", log.error);
    assert_eq!(
        log.base_ref.as_deref(),
        Some("origin/main"),
        "the ahead remote base is the comparison"
    );
    let classified: Vec<(&str, bool)> = log
        .commits
        .iter()
        .map(|commit| (commit.subject.as_str(), commit.is_on_base))
        .collect();
    assert_eq!(classified, [("Feature work", false), ("initial", true)]);
}

/// A stored base branch whose name begins with `-` is refused before it
/// reaches any argv: git parses a leading `-` as its own option (measured:
/// `git log --output=x..HEAD` writes the log to a file named `x..HEAD` and
/// exits 0). The answer is a defined refusal — no commits, a sentence —
/// and no file is written.
#[test]
fn a_stored_base_branch_starting_with_a_dash_is_refused() {
    let repo = Repo::new("injection");
    repo.write("file.txt", "base\n");
    repo.commit("initial");
    // A branch named `--output=x`, built by fast-import (the CLI would
    // parse the name as an option too).
    let parent = repo.run_output(&["rev-parse", "HEAD"]).trim().to_string();
    let stream: Vec<u8> = format!(
        "blob\nmark :1\ndata 2\na\n\ncommit refs/heads/--output=x\nmark :2\ncommitter {COMMITTER} 1700000000 +0000\ndata 8\ninjected\nfrom {parent}\nM 100644 :1 file.txt\n\n"
    )
    .into_bytes();
    repo.run_with_input(&["fast-import", "--quiet"], &stream);
    repo.run(&["reset", "--quiet", "--hard", "main"]);

    let log = log_of(&repo.root, Some("--output=x"));

    assert_eq!(log.base_ref, None);
    assert!(log.commits.is_empty());
    let error = log.error.expect("a refusal carries its sentence");
    assert_eq!(error, "the stored base branch is not a valid git ref");
    assert_no_path(&error);
    // No file was written: the refusal happened before any git log ran.
    let entries: Vec<String> = std::fs::read_dir(&repo.root)
        .expect("repo root")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    assert!(
        !entries.iter().any(|name| name.contains("output")),
        "no output file was written: {entries:?}"
    );
}

/// A folder inside a repository but not its root is refused with the shared
/// sentence — the panel lists the repository's changes, not this folder's.
#[test]
fn a_folder_inside_a_repository_but_not_its_root_is_refused() {
    let repo = Repo::new("inside");
    repo.write("file.txt", "base\n");
    repo.commit("initial");
    let subdir = repo.root.join("sub");
    std::fs::create_dir(&subdir).expect("subdir");

    let log = log_of(&subdir, None);

    assert_eq!(log.error.as_deref(), Some(INSIDE_A_REPOSITORY));
    assert_eq!(log.base_ref, None);
    assert!(log.commits.is_empty());
}
