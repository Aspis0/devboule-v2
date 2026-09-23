//! `deleted` over real repositories in `%TEMP%`: the act that loses data,
//! and every refusal — each pinned to its exact sentence, because the
//! mutations these tests must kill die on the sentence, not on "some error
//! came back". The walk vouches **before** anything is touched, so every
//! case here also asserts what survived the act: a refusal leaves the tree
//! exactly as it found it.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use super::{deleted, DOES_NOT_EXIST, NOT_PART_OF_THE_TREE, THE_ROOT};

/// A repository under `temp_dir`, the fixture the sibling mutation tests
/// use: a real checkout, so the `.git` the guard refuses is real. Hard-
/// requires git — a silent skip would let every case below pass without
/// running one.
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
            "devboule-file-delete-{label}-{}-{stamp}",
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

    fn run(&self, arguments: &[&str]) {
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

    fn write(&self, relative: &str, contents: impl AsRef<[u8]>) {
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

/// A unique path under `temp_dir`, for the link targets of the escape cases.
fn unique_directory(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "devboule-file-delete-{label}-{}-{stamp}",
        std::process::id()
    ))
}

#[test]
fn a_file_is_deleted_and_the_reply_names_nothing() {
    let repo = Repo::new("file");
    repo.write("old-notes.txt", "gone\n");

    deleted(&repo.root, "old-notes.txt").expect("delete");

    assert!(
        !repo.root.join("old-notes.txt").exists(),
        "the file is gone"
    );
}

/// Mutant `m:recursive` — delete only the folder's own shell: any of these
/// children surviving kills this case. The folder goes **whole**, and the
/// deletion is the act itself — no `git rm` is consulted (a tracked file's
/// deletion is the user's to stage, as `git status` already shows it).
#[test]
fn a_folder_is_deleted_whole_with_its_children() {
    let repo = Repo::new("recursive");
    repo.write("src/lib.rs", "//\n");
    repo.write("src/deep/mod.rs", "\n");
    repo.write("src/notes.txt", "tracked or not\n");
    repo.run(&["add", "src/notes.txt"]);

    deleted(&repo.root, "src").expect("delete folder");

    assert!(!repo.root.join("src").exists(), "the folder itself is gone");
    assert!(
        repo.root.join("src/lib.rs").metadata().is_err(),
        "the nested file is gone with it"
    );
    assert!(
        repo.root.join("src/deep/mod.rs").metadata().is_err(),
        "the deep child is gone with it"
    );
    assert!(
        repo.root.join("src/notes.txt").metadata().is_err(),
        "the tracked child is gone too — deletion is the act, no index to please"
    );
}

/// Mutant `m:root` — drop the root guard: the workspace's own folder falls
/// through to the confinement, which answers "outside" about a path that is
/// inside — and the one act that could empty the whole checkout answers
/// with the wrong words. This case dies on the sentence.
#[test]
fn the_workspaces_own_folder_is_never_deleted() {
    let repo = Repo::new("root");
    repo.write("a.txt", "a\n");

    for requested in ["", ".", "./"] {
        assert_eq!(
            deleted(&repo.root, requested).expect_err("refused"),
            THE_ROOT,
            "delete {requested:?}"
        );
    }
    assert!(repo.root.is_dir(), "the folder itself still stands");
    assert!(
        repo.root.join("a.txt").is_file(),
        "nothing inside was touched"
    );
}

/// Mutant `m:git` — drop the metadata guard: `.GIT` resolves to the real
/// `.git` on NTFS and the deletion would empty the repository's own
/// metadata — the one deletion this tree never performs, in any spelling
/// Win32 resolves.
#[test]
fn the_repository_metadata_folder_is_never_deleted_in_any_spelling() {
    let repo = Repo::new("git-guard");
    repo.write("a.txt", "a\n");
    assert!(
        repo.root.join(".git/config").is_file(),
        "the fixture's .git must be real for this case to mean anything"
    );

    for requested in [".git", ".GIT", ".git.", ".git ", ".git/config"] {
        let error = deleted(&repo.root, requested).expect_err("refused");
        assert_eq!(error, NOT_PART_OF_THE_TREE, "{requested:?}");
    }
    assert!(
        repo.root.join(".git/config").is_file(),
        "the repository's metadata survived every spelling"
    );
    assert!(
        repo.root.join("a.txt").is_file(),
        "nothing else was touched"
    );
}

/// Mutant `m:walk` — let the walk pass a link: the act would remove the
/// link itself (or reach at its target through it) where this tree refuses
/// the act outright — the divergence from Paseo's delete declared in
/// `DECISIONS-write.md` §6, kept as the walk's one rule.
#[test]
#[cfg(windows)]
fn a_link_is_refused_not_deleted_nor_followed() {
    let repo = Repo::new("links");
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
        let error = deleted(&repo.root, requested).expect_err("refused");
        let fragment = if requested == "dirlink" || requested == "vanished.txt" {
            "symbolic link"
        } else {
            "crosses a link"
        };
        assert!(error.contains(fragment), "{requested}: {error}");
        assert_no_root(&error, &repo.root);
    }
    assert!(
        outside.join("present.txt").is_file(),
        "the target was never reached"
    );
    assert!(junction.exists(), "the link itself was never removed");
    assert!(dangling.exists(), "the dangling link was never removed");
    let _ = std::fs::remove_file(&dangling);
    let _ = std::fs::remove_dir(&junction);
    let _ = std::fs::remove_dir_all(&outside);
}

/// One level deeper than the walk can see: a folder **being deleted** that
/// holds a link loses the link and never its target — `remove_dir_all`
/// unlinks without traversing, so the one rule holds inside the recursion.
/// The folder's ordinary children go with it.
#[test]
#[cfg(windows)]
fn a_deleted_folder_unlinks_a_child_link_and_never_its_target() {
    let repo = Repo::new("child-link");
    repo.write("tree/real.txt", "real\n");
    let outside = unique_directory("child-link-target");
    std::fs::create_dir(&outside).expect("outside dir");
    std::fs::write(outside.join("present.txt"), "outside reached\n").expect("outside file");
    // Two joins, not one: `join("tree/inner")` would hand `cmd` a slash it
    // parses as a switch.
    let link = repo.root.join("tree").join("inner");
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

    deleted(&repo.root, "tree").expect("delete folder with a child link");

    assert!(!repo.root.join("tree").exists(), "the folder is gone");
    assert!(
        outside.join("present.txt").is_file(),
        "the link's target was never reached"
    );
    let _ = std::fs::remove_dir_all(&outside);
}

/// The entry the walk vouched is the entry the act names: a spelling that
/// never had a stat is refused with the listing's own sentence, and
/// nothing anywhere was touched.
#[test]
fn a_missing_entry_is_refused_with_the_listings_own_sentence() {
    let repo = Repo::new("missing");
    repo.write("kept.txt", "kept\n");

    let error = deleted(&repo.root, "never-existed.txt").expect_err("refused");
    assert_eq!(error, DOES_NOT_EXIST);
    assert!(repo.root.join("kept.txt").is_file(), "nothing was touched");

    let error = deleted(&repo.root, "kept.txt/inner.txt").expect_err("a file is not a folder");
    assert_eq!(error, DOES_NOT_EXIST);
    assert_eq!(
        std::fs::read_to_string(repo.root.join("kept.txt")).unwrap(),
        "kept\n",
        "the file survived the nonsense spelling"
    );
}

/// The same double assertion the sibling mutation tests pin: no refusal of
/// this act ever carries this run's workspace root or any path-shaped text
/// (`/`, `\`, `:`) — every sentence is static or the walk's own.
fn assert_no_root(error: &str, root: &Path) {
    let root = root.to_string_lossy().into_owned();
    assert!(!error.contains(root.as_str()), "the root leaked: {error}");
    assert!(
        !error.contains('/') && !error.contains('\\') && !error.contains(':'),
        "a path leaked: {error}"
    );
}

/// Mutant `m:sentence` — build one of these sentences from the path: the
/// root leaks into an `error` and this dies. The list covers every refusal
/// class the delete composes around a confined path; the two link
/// refusals run [`assert_no_root`] in their own test above. Never in any
/// list, and declared rather than hidden: `DELETE_FAILED` — a bare
/// constant on an arm no test can force open (there is no deterministic
/// way to make a removal of a path this suite owns fail).
#[test]
fn no_refusal_sentence_carries_the_workspace_root() {
    let repo = Repo::new("pathless");

    let refusals = [
        deleted(&repo.root, "").expect_err("root"),
        deleted(&repo.root, ".").expect_err("root dotted"),
        deleted(&repo.root, "../escape.txt").expect_err("outside"),
        deleted(&repo.root, ".git/config").expect_err("git"),
        deleted(&repo.root, "never-existed.txt").expect_err("missing"),
    ];
    for error in refusals {
        assert_no_root(&error, &repo.root);
    }
}
