//! `copy_of` and the folder's lifecycle over real repositories in `%TEMP%`:
//! the copy that must exist with the source's bytes, every refusal the read
//! makes (plus the extension gate this module adds), the folder's "at most
//! one copy" invariant, and the two deletes — unstage's and the start
//! sweep's. The sentences are pinned to the words the rest of the panel
//! shows, and none of the refusals carries the workspace root.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use devboule_protocol::WorkspaceFilePreviewStatus;

use super::{clear, copy_of, previews_of, sweep, NOT_SHOWABLE, OUTSIDE_THE_WORKSPACE};

/// A repository under `temp_dir`, the fixture `workspace_file_read_tests`
/// uses: the `.git` the guard refuses here is the real metadata folder, not
/// one a test pretended with. Hard-requires git — a silent skip would let
/// the `.git` cases below pass without running anything.
struct Repo {
    root: PathBuf,
}

impl Repo {
    fn new(label: &str) -> Self {
        let root = unique_path(&format!("devboule-preview-{label}"));
        std::fs::create_dir(&root).expect("test directory");
        let repo = Self { root };
        let output = Command::new("git")
            .arg("-C")
            .arg(&repo.root)
            .args(["init", "--quiet"])
            .output()
            .expect("git could not be spawned");
        assert!(
            output.status.success(),
            "git init failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        repo
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

/// A unique path under `temp_dir`: the label keeps the run readable, the
/// stamp and pid keep two parallel tests apart.
fn unique_path(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!("{label}-{}-{stamp}", std::process::id()))
}

/// The staging folder of one test — itself under `temp_dir`, outside every
/// fixture repo, so nothing here can pass by reading a workspace path.
fn fresh_previews(label: &str) -> PathBuf {
    unique_path(&format!("devboule-previews-{label}"))
}

/// A directory outside the workspace, for the link targets of the escape
/// cases (the same shape the read's fixture uses).
fn unique_directory(label: &str) -> PathBuf {
    unique_path(&format!("devboule-preview-target-{label}"))
}

fn mtime_ms(path: &Path) -> i64 {
    let time = std::fs::metadata(path)
        .expect("stat")
        .modified()
        .expect("mtime");
    match time.duration_since(UNIX_EPOCH) {
        Ok(after) => after.as_millis() as i64,
        Err(before) => -(before.duration().as_millis() as i64),
    }
}

/// The stage's success half: the copy rests in the previews folder (never in
/// the workspace), carries the source's bytes, and answers with the
/// source's own stat. Staging the same unchanged file twice lands on the
/// same path — the name comes from the file's identity, so the webview's
/// asset URL is stable for an unchanged file and changes when it is edited.
#[test]
fn a_staged_copy_rests_in_the_folder_with_the_sources_bytes_and_stat() {
    let repo = Repo::new("stage-ok");
    let previews = fresh_previews("stage-ok");
    let bytes = [0x89u8, b'P', b'N', b'G', 0x00, 0xFF, 0x0D, 0x0A];
    repo.write("shot.png", bytes);

    let staged = copy_of(&repo.root, "shot.png", &previews);

    assert_eq!(staged.status, WorkspaceFilePreviewStatus::Ok);
    assert_eq!(staged.error, None, "a success carries no sentence");
    let destination = PathBuf::from(staged.path.as_ref().expect("a success names its copy"));
    assert!(destination.starts_with(&previews), "{destination:?}");
    assert!(destination.is_absolute(), "{destination:?}");
    assert_eq!(destination.extension().expect("png"), "png");
    assert_eq!(
        std::fs::read(&destination).expect("the copy exists"),
        bytes.as_slice(),
        "the copy must be the source's bytes, not a placeholder"
    );
    assert_eq!(staged.size, Some(bytes.len() as u64));
    assert_eq!(
        staged.modified_at,
        Some(mtime_ms(&repo.root.join("shot.png")))
    );

    let again = copy_of(&repo.root, "shot.png", &previews);
    assert_eq!(
        again.path, staged.path,
        "the same unchanged file must keep the same path, or the asset URL churns"
    );

    let _ = std::fs::remove_dir_all(&previews);
}

/// Every refusal the read makes is a refusal here too, each pinned to the
/// sentence the rest of the panel shows — and no refusal creates the
/// staging folder, because a file that was never copied must leave no room
/// for the webview to find one in. The `.GIT` spellings are why the guard
/// sits before the extension gate: without it, `.git/config` would still be
/// refused (no shown extension) but with the wrong sentence, and this
/// assertion is what notices.
#[test]
fn every_refusal_the_read_makes_refuses_a_stage_and_copies_nothing() {
    let repo = Repo::new("refusals");
    repo.write("in.png", "x");
    let previews = fresh_previews("refusals");

    let folder_spellings: [(&str, &str); 3] = [
        ("", "not a file"),
        (".", "not a file"),
        ("./", "not a file"),
    ];
    let escapes: [(&str, &str); 2] = [
        ("../escape.png", "outside the workspace folder"),
        ("/absolute/escape.png", "outside the workspace folder"),
    ];
    let metadata: [(&str, &str); 6] = [
        (".git", "not part of the tree"),
        (".GIT", "not part of the tree"),
        (".git.", "not part of the tree"),
        (".git ", "not part of the tree"),
        (".git/config", "not part of the tree"),
        (".GIT/config", "not part of the tree"),
    ];
    let missing: [(&str, &str); 1] = [("never-existed.png", "does not exist")];

    for (requested, fragment) in folder_spellings
        .into_iter()
        .chain(escapes)
        .chain(metadata)
        .chain(missing)
    {
        let staged = copy_of(&repo.root, requested, &previews);
        assert_eq!(
            staged.status,
            WorkspaceFilePreviewStatus::Refused,
            "{requested:?}"
        );
        assert_eq!(staged.path, None, "{requested:?}");
        assert_eq!(staged.size, None, "a refusal claims nothing: {requested:?}");
        let error = staged.error.as_deref().expect("the refusal says why");
        assert!(error.contains(fragment), "{requested:?}: {error}");
        assert_no_root(error, &repo.root);
    }
    assert!(
        !previews.exists(),
        "a refusal must never create the folder the webview reads"
    );
}

/// A folder that only *looks* like a shown file: the walk classifies the
/// spelling-free truth (it is a directory), and `std::fs::copy` of a
/// directory is not a preview.
#[test]
fn a_folder_whose_name_ends_in_a_shown_extension_is_not_a_file() {
    let repo = Repo::new("folder");
    repo.write("album.png/inner.png", "x");
    let previews = fresh_previews("folder");

    let staged = copy_of(&repo.root, "album.png", &previews);

    assert_eq!(staged.status, WorkspaceFilePreviewStatus::Refused);
    let error = staged.error.as_deref().expect("the refusal says why");
    assert!(error.contains("not a file"), "{error}");
    assert!(!previews.exists());
}

/// The gate this module adds beyond the read: only the extensions the panel
/// draws as media reach the folder the webview can read without this
/// module's confinement — so a secrets file the read would happily answer
/// under its 128 KiB cap cannot be staged into a path the scope lets the
/// renderer fetch whole, and `svg` (which the read answers as text) stays
/// out with the text formats.
#[test]
fn a_file_the_panel_never_draws_is_not_staged() {
    let repo = Repo::new("gate");
    repo.write("secrets.txt", "token=1\n");
    repo.write("logo.svg", "<svg/>");
    let previews = fresh_previews("gate");

    for requested in ["secrets.txt", "logo.svg"] {
        let staged = copy_of(&repo.root, requested, &previews);
        assert_eq!(
            staged.status,
            WorkspaceFilePreviewStatus::Refused,
            "{requested}"
        );
        let error = staged.error.as_deref().expect("the refusal says why");
        assert!(error.contains(NOT_SHOWABLE), "{requested}: {error}");
        assert_no_root(error, &repo.root);
    }
    assert!(!previews.exists());

    // The other half of the gate: an upper-case spelling is still a shown
    // file, and the copy keeps the lower-cased extension the asset
    // protocol's type sniff and the element both read.
    repo.write("PHOTO.JPG", [0xFFu8, 0xD8, 0xFF]);
    let staged = copy_of(&repo.root, "PHOTO.JPG", &previews);
    assert_eq!(staged.status, WorkspaceFilePreviewStatus::Ok);
    assert!(
        staged
            .path
            .as_deref()
            .expect("a success names its copy")
            .ends_with(".jpg"),
        "{staged:?}"
    );
    let _ = std::fs::remove_dir_all(&previews);
}

/// Mutant target for the confinement: drop `confined` from the stage and
/// `..` walks out with the walk's "does not exist" instead of the escape's
/// sentence — and a reachable link's target would be one `copy` away. Both
/// link kinds get the SAME sentences the rest of the panel shows (shared in
/// `workspace_git_support`, never copied).
#[test]
#[cfg(windows)]
fn a_path_through_or_to_a_link_is_refused_with_the_walk_sentences() {
    let repo = Repo::new("links");
    repo.write("in.png", "x");
    let outside = unique_directory("links-target");
    std::fs::create_dir(&outside).expect("outside dir");
    std::fs::write(outside.join("present.png"), "outside reached").expect("outside file");
    let junction = repo.root.join("dirlink");
    let created = Command::new("cmd")
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
    let dangling = repo.root.join("vanished.png");
    std::os::windows::fs::symlink_file(outside.join("present.png"), &dangling).expect("symlink");
    let previews = fresh_previews("links");

    for (requested, fragment) in [
        ("dirlink", "symbolic link"),
        ("dirlink/present.png", "crosses a link"),
        ("vanished.png", "symbolic link"),
    ] {
        let staged = copy_of(&repo.root, requested, &previews);
        assert_eq!(
            staged.status,
            WorkspaceFilePreviewStatus::Refused,
            "{requested}"
        );
        let error = staged.error.as_deref().expect("the refusal says why");
        assert!(error.contains(fragment), "{requested}: {error}");
        assert_no_root(error, &repo.root);
    }
    assert!(!previews.exists(), "a refused link is never staged");

    let _ = std::fs::remove_file(&dangling);
    let _ = std::fs::remove_dir(&junction);
    let _ = std::fs::remove_dir_all(&outside);
}

/// The folder's whole invariant: a second stage replaces the first, never
/// joins it — the panel's URL must be the only readable copy, or unstage
/// would revoke the file under the panel while an older one stayed
/// reachable through the scope.
#[test]
fn staging_a_second_file_replaces_the_first() {
    let repo = Repo::new("replace");
    repo.write("one.png", "first");
    repo.write("two.png", "second");
    let previews = fresh_previews("replace");

    let first = copy_of(&repo.root, "one.png", &previews);
    let second = copy_of(&repo.root, "two.png", &previews);
    assert_eq!(first.status, WorkspaceFilePreviewStatus::Ok);
    assert_eq!(second.status, WorkspaceFilePreviewStatus::Ok);
    assert_ne!(first.path, second.path, "different files, different copies");

    let entries: Vec<PathBuf> = std::fs::read_dir(&previews)
        .expect("the folder exists")
        .map(|entry| entry.expect("entry").path())
        .collect();
    assert_eq!(entries.len(), 1, "{entries:?}");
    assert_eq!(
        std::fs::read(&entries[0]).expect("the copy"),
        b"second".as_slice(),
        "the folder must hold the staged file, only"
    );

    let _ = std::fs::remove_dir_all(&previews);
}

/// The revoke: unstage deletes the copy (and the folder with it), and a
/// second revoke of nothing is still a success — the panel unstage fires on
/// a selection change and on close, and neither may turn "already revoked"
/// into an error. Mutant target: a `clear` that stops deleting leaves the
/// copy readable through the scope, and this test dies.
#[test]
fn unstage_deletes_the_copy_and_revoking_nothing_is_a_success() {
    let repo = Repo::new("revoke");
    repo.write("shot.png", "pixels");
    let previews = fresh_previews("revoke");
    let staged = copy_of(&repo.root, "shot.png", &previews);
    let destination = PathBuf::from(staged.path.expect("a success names its copy"));
    assert!(destination.is_file());

    assert!(clear(&previews));
    assert!(!destination.exists(), "the copy must be gone: revoked");
    assert!(!previews.exists(), "the folder goes with it");
    assert!(clear(&previews), "revoking twice is not a failure");
}

/// The start sweep: whatever a killed process left in the runtime dir's
/// folder dies at the next start, before any request can stage again — the
/// residual an app crash leaves is bounded by the restart, never by the
/// user's next click.
#[test]
fn the_start_sweep_removes_what_a_killed_process_left() {
    let runtime = unique_path("devboule-preview-runtime");
    let previews = previews_of(&runtime);
    std::fs::create_dir_all(&previews).expect("folder");
    std::fs::write(previews.join("ab12.png"), "left behind").expect("copy");

    sweep(&runtime);

    assert!(
        !previews.exists(),
        "the sweep takes the folder, copy and all"
    );
    let _ = std::fs::remove_dir_all(&runtime);
}

/// The two things a refusal sentence must never carry: this run's workspace
/// root, and any path-shaped text at all (`/`, `\`, `:` — which catches a
/// leaked *relative* path too). The sentences under test are static
/// constants, which is the point: this fails the day one is composed from
/// the path instead.
fn assert_no_root(error: &str, root: &Path) {
    let root = root.to_string_lossy().into_owned();
    assert!(!error.contains(root.as_str()), "the root leaked: {error}");
    assert!(
        !error.contains('/') && !error.contains('\\') && !error.contains(':'),
        "a path leaked: {error}"
    );
}

/// Kept honest with the escape list above: the shared sentence this module
/// imports must be the words the assertion claims, so a rename in
/// `workspace_git_support` fails here instead of silently mismatching every
/// `{requested}: {error}` above.
#[test]
fn the_shared_escape_sentence_is_the_one_this_test_pins() {
    assert!(OUTSIDE_THE_WORKSPACE.contains("outside the workspace folder"));
    assert!(!NOT_SHOWABLE.contains('/') && !NOT_SHOWABLE.contains('\\'));
}
