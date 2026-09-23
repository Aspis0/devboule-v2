//! `renamed` and `duplicated` over real repositories in `%TEMP%`: the two
//! acts that succeed, and every refusal — each pinned to its exact sentence,
//! because the mutations these tests must kill die on the sentence, not on
//! "some error came back". None of the refusals carries the workspace root.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use super::{
    claim_then_move, duplicated, renamed, NAME_DOT, NAME_EMPTY, NAME_INVALID, NAME_SEPARATOR,
    NAME_TAKEN, NAME_TRAILING, THE_ROOT,
};

/// A repository under `temp_dir`, the fixture the sibling read tests use:
/// a real checkout, so the `.git` the guard refuses and the index the
/// `git mv` test stages into are real. Hard-requires git — a silent skip
/// would let every case below pass without running one.
struct Repo {
    root: PathBuf,
}

impl Repo {
    fn new(label: &str) -> Self {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "devboule-file-mutations-{label}-{}-{stamp}",
            std::process::id()
        ));
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

    fn stdout(&self, arguments: &[&str]) -> String {
        let output = self.git(arguments).expect("git could not be spawned");
        assert!(
            output.status.success(),
            "git {arguments:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).expect("utf-8")
    }

    fn write(&self, relative: &str, contents: impl AsRef<[u8]>) {
        let path = self.root.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("parent directory");
        }
        std::fs::write(path, contents).expect("write");
    }

    fn names_of(&self, relative: &str) -> Vec<String> {
        std::fs::read_dir(self.root.join(relative))
            .expect("read_dir")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect()
    }
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// A unique path under `temp_dir`, for the link targets of the escape cases.
fn unique_directory(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "devboule-file-mutations-{label}-{}-{stamp}",
        std::process::id()
    ))
}

#[test]
fn a_rename_moves_the_entry_and_the_reply_names_the_new_spelling() {
    let repo = Repo::new("rename");
    repo.write("docs/guide.txt", "text\n");

    let new_path = renamed(&repo.root, "docs/guide.txt", " guide.md ").expect("rename");

    assert_eq!(new_path, "docs/guide.md", "trimmed and /-joined");
    assert!(
        !repo.root.join("docs/guide.txt").exists(),
        "old spelling gone"
    );
    let moved = std::fs::read_to_string(repo.root.join("docs/guide.md")).expect("read back");
    assert_eq!(moved, "text\n", "the bytes themselves moved");
}

/// Mutant `m:separator` — drop the separator check: the new name stops
/// being one name, and this case dies on the sentence (a name with a
/// separator falls to the single-name rule below, which says different
/// words).
#[test]
fn a_new_name_with_a_path_separator_is_refused_with_the_separator_sentence() {
    let repo = Repo::new("separator");
    repo.write("a.txt", "a\n");

    for name in ["sub/b.txt", "sub\\b.txt"] {
        let error = renamed(&repo.root, "a.txt", name).expect_err("refused");
        assert_eq!(error, NAME_SEPARATOR, "{name}");
        assert!(repo.root.join("a.txt").exists(), "{name}: nothing moved");
        assert!(
            !repo.root.join("sub").exists(),
            "{name}: no folder was created"
        );
    }
}

#[test]
fn an_empty_name_and_a_parent_spelling_are_refused() {
    let repo = Repo::new("bad-name");
    repo.write("a.txt", "a\n");

    for (name, sentence) in [
        ("", NAME_EMPTY),
        ("   ", NAME_EMPTY),
        (".", NAME_DOT),
        ("..", NAME_DOT),
        // Win32 would drop the trailing dot on create and the reply would
        // claim a name the tree never shows — refused at the same door.
        ("trailing.", NAME_TRAILING),
    ] {
        let error = renamed(&repo.root, "a.txt", name).expect_err("refused");
        assert_eq!(error, sentence, "{name:?}");
        assert!(repo.root.join("a.txt").exists(), "{name:?}: nothing moved");
    }
}

/// A spelling that parses as something other than one ordinary name: on
/// Windows `C:x` is a drive prefix, with no separator in it at all — the
/// rule's own sentence, not the separator's.
#[test]
#[cfg(windows)]
fn a_name_that_parses_as_a_drive_path_is_refused_as_not_one_name() {
    let repo = Repo::new("drive-name");
    repo.write("a.txt", "a\n");

    let error = renamed(&repo.root, "a.txt", "C:x").expect_err("refused");

    assert_eq!(error, NAME_INVALID);
    assert!(repo.root.join("a.txt").exists(), "nothing moved");
    assert!(!repo.root.join("x").exists(), "nothing created");
}

/// Mutant `m:root` — drop the root guard: the workspace's own folder falls
/// through to the confinement, which answers "outside" about a path that is
/// inside — a false sentence about an internal path, and the exact words
/// this case pins. The same refusal covers all three acts (the delete's
/// own root case is in `workspace_file_delete_tests.rs`).
#[test]
fn the_workspaces_own_folder_is_never_renamed_or_duplicated() {
    let repo = Repo::new("root");
    repo.write("a.txt", "a\n");

    for requested in ["", ".", "./"] {
        assert_eq!(
            renamed(&repo.root, requested, "elsewhere").expect_err("refused"),
            THE_ROOT,
            "rename {requested:?}"
        );
        assert_eq!(
            duplicated(&repo.root, requested).expect_err("refused"),
            THE_ROOT,
            "duplicate {requested:?}"
        );
    }
    assert!(repo.root.is_dir(), "the folder itself still stands");
}

#[test]
fn a_new_name_that_is_taken_is_refused_and_nothing_moves() {
    let repo = Repo::new("collision");
    repo.write("a.txt", "a\n");
    repo.write("b.txt", "b\n");

    let error = renamed(&repo.root, "a.txt", "b.txt").expect_err("refused");

    assert_eq!(error, NAME_TAKEN);
    assert_eq!(
        std::fs::read_to_string(repo.root.join("a.txt")).unwrap(),
        "a\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.root.join("b.txt")).unwrap(),
        "b\n"
    );
}

/// The one existing name a rename may land on, measured like the house
/// measures: the entry spelled with a different case, proven to be the same
/// file behind the two spellings.
#[test]
#[cfg(windows)]
fn a_rename_that_changes_only_the_case_is_allowed() {
    let repo = Repo::new("case-only");
    repo.write("cased.txt", "same\n");

    let new_path = renamed(&repo.root, "cased.txt", "Cased.txt").expect("case-only rename");

    assert_eq!(new_path, "Cased.txt");
    let names = repo.names_of("");
    assert!(names.contains(&"Cased.txt".to_string()), "{names:?}");
    assert!(!names.contains(&"cased.txt".to_string()), "{names:?}");
    assert_eq!(
        std::fs::read_to_string(repo.root.join("Cased.txt")).unwrap(),
        "same\n"
    );
}

/// The listing's own spelling-blind guard on the other end of the act: the
/// metadata folder cannot be renamed, and cannot be the destination's name
/// either — in any spelling Win32 resolves.
#[test]
fn the_repository_metadata_folder_is_refused_in_every_spelling() {
    let repo = Repo::new("git-guard");
    repo.write("a.txt", "a\n");
    assert!(
        repo.root.join(".git/config").is_file(),
        "the fixture's .git must be real for this case to mean anything"
    );

    for source in [".git", ".GIT", ".git/config"] {
        let error = renamed(&repo.root, source, "elsewhere").expect_err("refused");
        assert!(error.contains("not part of the tree"), "{source}: {error}");
        let duplicated_error = duplicated(&repo.root, source).expect_err("refused");
        assert!(
            duplicated_error.contains("not part of the tree"),
            "{source}: {duplicated_error}"
        );
    }
    for destination in [".git", ".GIT.", ".git "] {
        let error = renamed(&repo.root, "a.txt", destination).expect_err("refused");
        assert!(
            error.contains("not part of the tree"),
            "renaming TO {destination}: {error}"
        );
        assert!(repo.root.join("a.txt").exists(), "{destination}");
    }
    assert!(repo.root.join(".git/config").is_file(), "nothing moved");
}

/// Mutant `m:git-mv` — rename on the filesystem always: the index never
/// hears about the act, `git status` shows a deletion beside an untracked
/// file instead of a staged rename, and this case dies.
#[test]
fn a_tracked_file_is_renamed_through_git_so_the_act_lands_staged() {
    let repo = Repo::new("tracked");
    repo.write("tracked.txt", "one\n");
    repo.run(&["add", "tracked.txt"]);
    repo.run(&["commit", "--quiet", "-m", "init"]);

    let new_path = renamed(&repo.root, "tracked.txt", "renamed.txt").expect("rename");

    assert_eq!(new_path, "renamed.txt");
    let status = repo.stdout(&["status", "--porcelain"]);
    let staged_rename = status
        .lines()
        .find(|line| line.starts_with("R "))
        .unwrap_or_else(|| panic!("no staged rename in {status:?}"));
    assert!(
        staged_rename.contains("tracked.txt") && staged_rename.contains("renamed.txt"),
        "{staged_rename}"
    );
}

/// An untracked entry takes the filesystem road — nothing in the index to
/// stage, and the tree still shows one move.
#[test]
fn an_untracked_entry_renames_without_touching_the_index() {
    let repo = Repo::new("untracked");
    repo.write("loose.txt", "loose\n");

    renamed(&repo.root, "loose.txt", "moved.txt").expect("rename");

    assert!(!repo.root.join("loose.txt").exists());
    assert!(repo.root.join("moved.txt").is_file());
    let status = repo.stdout(&["status", "--porcelain"]);
    assert!(
        !status.contains('R'),
        "an untracked rename stages nothing: {status:?}"
    );
    assert!(status.contains("?? moved.txt"), "{status:?}");
}

/// Mutant `m:suffix` — take the first free name without the loop: the
/// second duplicate collides with the first copy (or overwrites it), and
/// this case dies on `copy 2`. The tampered first copy also proves the loop
/// stat's every candidate rather than trusting that the first name is free.
#[test]
fn a_second_duplicate_gets_the_numbered_name_and_never_overwrites() {
    let repo = Repo::new("copy-loop");
    repo.write("a.txt", "original\n");

    let first = duplicated(&repo.root, "a.txt").expect("first copy");
    assert_eq!(first, "a copy.txt");
    assert_eq!(
        std::fs::read_to_string(repo.root.join("a copy.txt")).unwrap(),
        "original\n"
    );
    repo.write("a copy.txt", "tampered\n");

    let second = duplicated(&repo.root, "a.txt").expect("second copy");
    assert_eq!(second, "a copy 2.txt");
    assert_eq!(
        std::fs::read_to_string(repo.root.join("a copy 2.txt")).unwrap(),
        "original\n",
        "the copy is the source's bytes, never the neighbor's"
    );
    assert_eq!(
        std::fs::read_to_string(repo.root.join("a copy.txt")).unwrap(),
        "tampered\n",
        "an existing entry is never overwritten"
    );
    assert_eq!(
        std::fs::read_to_string(repo.root.join("a.txt")).unwrap(),
        "original\n",
        "the source itself is untouched"
    );
}

/// A folder copy goes whole — nested children included — and a folder
/// rename carries its children with it: one entry moves, not a flattened
/// shell of it.
#[test]
fn a_folder_duplicates_whole_and_renames_with_its_children() {
    let repo = Repo::new("folder");
    repo.write("src/lib.rs", "//\n");
    repo.write("src/deep/mod.rs", "\n");

    let copied = duplicated(&repo.root, "src").expect("duplicate folder");
    assert_eq!(copied, "src copy");
    assert_eq!(
        std::fs::read_to_string(repo.root.join("src copy/lib.rs")).unwrap(),
        "//\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.root.join("src copy/deep/mod.rs")).unwrap(),
        "\n"
    );

    let moved = renamed(&repo.root, "src", "crates-src").expect("rename folder");
    assert_eq!(moved, "crates-src");
    assert!(!repo.root.join("src").exists());
    assert_eq!(
        std::fs::read_to_string(repo.root.join("crates-src/deep/mod.rs")).unwrap(),
        "\n"
    );
    assert!(
        !repo.root.join("crates-src copy").exists(),
        "the rename moved the folder, not a copy beside it"
    );
}

/// Mutant `m:walk` — a link as the source is refused with the walk's own
/// sentence (the shared one, never a copy), and neither act leaves a trace:
/// nothing renamed, nothing copied through the link.
#[test]
#[cfg(windows)]
fn a_path_through_or_to_a_link_is_refused_with_the_walk_sentence() {
    let repo = Repo::new("links");
    repo.write("in.txt", "a\n");
    let outside = unique_directory("links-target");
    std::fs::create_dir(&outside).expect("outside dir");
    std::fs::write(outside.join("present.txt"), "outside reached\n").expect("outside file");
    let junction = repo.root.join("dirlink");
    let created = std::process::Command::new("cmd")
        .args(["/C", "mklink", "/J"])
        .arg(&junction)
        .arg(&outside)
        .output()
        .expect("mklink");
    assert!(
        created.status.success(),
        "mklink /J failed: {}",
        String::from_utf8_lossy(&created.stderr)
    );
    let dangling = repo.root.join("vanished.txt");
    std::os::windows::fs::symlink_file(outside.join("present.txt"), &dangling).expect("symlink");

    for requested in ["dirlink", "dirlink/present.txt", "vanished.txt"] {
        let fragment = if requested == "dirlink" || requested == "vanished.txt" {
            "symbolic link"
        } else {
            "crosses a link"
        };
        for error in [
            renamed(&repo.root, requested, "elsewhere").expect_err("refused"),
            duplicated(&repo.root, requested).expect_err("refused"),
        ] {
            assert!(error.contains(fragment), "{requested}: {error}");
            assert_no_root(&error, &repo.root);
            assert!(
                !error.contains("outside reached"),
                "{requested} read through the link: {error}"
            );
        }
    }
    assert!(
        outside.join("present.txt").is_file(),
        "the target still stands"
    );
    let _ = std::fs::remove_file(&dangling);
    let _ = std::fs::remove_dir(&junction);
    let _ = std::fs::remove_dir_all(&outside);
}

/// The copy half of the same rule: a link **inside** a folder being
/// duplicated is refused too — nothing is ever copied *through* a link —
/// and the partial copy is removed whole, so the sentence ("nothing was
/// copied") is the truth and not a hope. The source stays untouched.
#[test]
#[cfg(windows)]
fn a_folder_copy_that_meets_a_child_link_is_refused_and_leaves_nothing() {
    let repo = Repo::new("copy-child-link");
    repo.write("tree/real.txt", "real\n");
    let outside = unique_directory("copy-child-target");
    std::fs::create_dir(&outside).expect("outside dir");
    std::fs::write(outside.join("present.txt"), "outside reached\n").expect("outside file");
    let dangling = repo.root.join("tree/inner.txt");
    std::os::windows::fs::symlink_file(outside.join("present.txt"), &dangling).expect("symlink");

    let error = duplicated(&repo.root, "tree").expect_err("refused");

    assert!(error.contains("nothing was copied"), "{error}");
    assert_no_root(&error, &repo.root);
    assert!(
        !repo.root.join("tree copy").exists(),
        "the partial copy was removed whole"
    );
    assert_eq!(
        std::fs::read_to_string(repo.root.join("tree/real.txt")).unwrap(),
        "real\n",
        "the source folder is untouched"
    );
    let _ = std::fs::remove_file(&dangling);
    let _ = std::fs::remove_dir_all(&outside);
}

/// Mutant `m:claim` — take the destination with `fs::rename` instead of
/// claiming it: this newcomer stands exactly where the stat in `renamed`
/// has already looked away, and a plain rename would replace its bytes in
/// silence — the hole the claim closes, and the sentence that must come
/// back instead. Drives the claim road directly, which *is* the window
/// made callable: no thread, no timing, both halves (file and folder).
#[test]
fn a_destination_created_after_the_stat_is_refused_not_replaced() {
    let repo = Repo::new("claim-race");
    repo.write("src.txt", "source\n");
    repo.write("dst.txt", "concurrent\n");

    let error = claim_then_move(&repo.root.join("src.txt"), &repo.root.join("dst.txt"))
        .expect_err("the claim must lose honestly");

    assert_eq!(error, NAME_TAKEN);
    assert_eq!(
        std::fs::read_to_string(repo.root.join("dst.txt")).unwrap(),
        "concurrent\n",
        "the newcomer's bytes are intact"
    );
    assert_eq!(
        std::fs::read_to_string(repo.root.join("src.txt")).unwrap(),
        "source\n",
        "the source never moved"
    );

    // The folder half of the same claim: `create_dir` names the loser the
    // same way, and neither tree lost a child.
    repo.write("tree/inner.txt", "tree\n");
    repo.write("raced/inner.txt", "raced\n");
    let error = claim_then_move(&repo.root.join("tree"), &repo.root.join("raced"))
        .expect_err("the folder claim must lose honestly");
    assert_eq!(error, NAME_TAKEN);
    assert_eq!(
        std::fs::read_to_string(repo.root.join("tree/inner.txt")).unwrap(),
        "tree\n"
    );
    assert_eq!(
        std::fs::read_to_string(repo.root.join("raced/inner.txt")).unwrap(),
        "raced\n"
    );
}

/// The angle the copy loop's extension filter closes, pinned on the fact
/// the declaration rests on: a name ending in a dot exists on Windows only
/// when created verbatim (`\\?\`), and the tree cannot see such a name at
/// all — the walk stats plain paths, and Win32 strips those dots — so no
/// reachable source can ever feed the copy loop a trailing-dot spelling.
/// If the walk ever switched to verbatim paths, this test dies and the
/// filter in `free_copy_name` becomes the load-bearing half.
#[test]
#[cfg(windows)]
fn a_name_ending_in_a_dot_exists_but_is_invisible_to_the_tree() {
    let repo = Repo::new("trailing-source");
    let verbatim = format!(r"\\?\{}", repo.root.join("a.").display());
    std::fs::write(&verbatim, "dot\n").expect("verbatim create");
    assert!(
        std::fs::metadata(&verbatim).is_ok(),
        "the name exists verbatim"
    );

    let error = duplicated(&repo.root, "a.").expect_err("the tree stats plain paths");

    assert!(error.contains("does not exist"), "{error}");
    let copies: Vec<String> = repo
        .names_of("")
        .into_iter()
        .filter(|name| name.contains("copy"))
        .collect();
    assert!(copies.is_empty(), "the copy loop never ran: {copies:?}");
}

/// The two things a refusal sentence must never carry: this run's workspace
/// root, and any path-shaped text at all (`/`, `\`, `:`) — the same double
/// assertion `workspace_file_read_tests.rs` pins for the reads.
fn assert_no_root(error: &str, root: &Path) {
    let root = root.to_string_lossy().into_owned();
    assert!(!error.contains(root.as_str()), "the root leaked: {error}");
    assert!(
        !error.contains('/') && !error.contains('\\') && !error.contains(':'),
        "a path leaked: {error}"
    );
}

/// Mutant `m:sentence` — build one of these sentences from the path instead
/// of a constant: the root leaks into an `error` and this dies. The list
/// covers every refusal class the two acts compose around a confined path;
/// the two link refusals run [`assert_no_root`] in their own test above, the
/// child-link refusal in its own, and `NAME_TAKEN` is in this list (the
/// collision row). Never in any list, and declared rather than hidden:
/// `RENAME_FAILED` and `COPY_FAILED` — bare constants on arms no test can
/// force open (there is no deterministic way to make a rename or a copy
/// fail on a file this suite owns) — and `NAME_TRAILING`/`NAME_INVALID`,
/// whose own tests assert their exact sentences but compose no path, so
/// there is nothing root-shaped for this one to find in them.
#[test]
fn no_refusal_sentence_carries_the_workspace_root() {
    let repo = Repo::new("pathless");
    repo.write("src/lib.rs", "//\n");
    repo.write("taken.txt", "t\n");

    let refusals = [
        renamed(&repo.root, "", "other").expect_err("root"),
        renamed(&repo.root, "../escape.txt", "other").expect_err("outside"),
        renamed(&repo.root, ".git/config", "other").expect_err("git"),
        renamed(&repo.root, "never-existed.txt", "other").expect_err("missing"),
        renamed(&repo.root, "src/lib.rs", "sub/name").expect_err("separator"),
        renamed(&repo.root, "taken.txt", "src").expect_err("collision"),
        duplicated(&repo.root, "never-existed.txt").expect_err("missing"),
        duplicated(&repo.root, "../escape.txt").expect_err("outside"),
    ];
    for error in refusals {
        assert_no_root(&error, &repo.root);
    }
}
