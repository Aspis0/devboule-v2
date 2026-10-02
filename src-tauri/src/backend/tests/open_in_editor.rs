//! Spawn-time containment for one workspace file: the canonical check that
//! runs immediately before the launch, over real folders in the system temp
//! dir. The sentences are pinned by value so a rewording is a reviewable
//! change, not a silent one.

use std::path::Path;

use devboule_protocol::ErrorCode;
use tempfile::TempDir;

use crate::backend::open_in_editor::{file_stat_error, trusted_program, validated_file};

const MISSING: &str = "the file no longer exists in the workspace";
const ACCESS_DENIED: &str = "this machine denied access to this file";
const ROOT_GONE: &str = "the workspace folder is not on this machine";
const OUTSIDE: &str = "the file resolved outside the workspace folder";
const NOT_A_FILE: &str = "the target is not a regular file";

fn make_file(path: &Path) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("parent directory");
    }
    std::fs::write(path, b"inside\n").expect("file");
}

#[test]
fn a_relative_file_under_the_root_resolves_to_its_canonical_identity() {
    let dir = TempDir::new().expect("temp");
    let root = dir.path().join("repo");
    make_file(&root.join("docs/SETUP.md"));

    let validated = validated_file(&root, "docs/SETUP.md").expect("a confined file validates");

    assert_eq!(
        validated.root,
        std::fs::canonicalize(&root).expect("canonical root")
    );
    assert_eq!(
        validated.file,
        std::fs::canonicalize(root.join("docs/SETUP.md")).expect("canonical file")
    );
}

#[test]
fn traversal_out_of_the_root_is_refused() {
    let dir = TempDir::new().expect("temp");
    let root = dir.path().join("repo");
    make_file(&root.join("docs/SETUP.md"));
    make_file(&dir.path().join("outside.txt"));

    let error = validated_file(&root, "../outside.txt").expect_err("an escape must never validate");

    assert_eq!(error.code, ErrorCode::Io);
    assert_eq!(error.message, OUTSIDE);
}

#[test]
fn an_absolute_path_outside_the_root_is_refused() {
    let dir = TempDir::new().expect("temp");
    let root = dir.path().join("repo");
    make_file(&root.join("inside.txt"));
    let outside = dir.path().join("outside.txt");
    make_file(&outside);
    let absolute = outside.to_string_lossy().into_owned();

    let error = validated_file(&root, &absolute).expect_err("an escape must never validate");

    assert_eq!(error.code, ErrorCode::Io);
    assert_eq!(error.message, OUTSIDE);
}

#[test]
fn a_file_the_disk_no_longer_has_says_missing() {
    let dir = TempDir::new().expect("temp");
    let root = dir.path().join("repo");
    make_file(&root.join("present.txt"));

    let error = validated_file(&root, "docs/gone.md").expect_err("nothing to open");

    assert_eq!(error.code, ErrorCode::Io);
    assert_eq!(error.message, MISSING);
}

#[test]
fn permission_denied_and_missing_are_different_sentences() {
    let denied = file_stat_error(std::io::Error::from(std::io::ErrorKind::PermissionDenied));
    let missing = file_stat_error(std::io::Error::from(std::io::ErrorKind::NotFound));

    assert_eq!(denied.code, ErrorCode::Io);
    assert_eq!(denied.message, ACCESS_DENIED);
    assert_eq!(missing.code, ErrorCode::Io);
    assert_eq!(missing.message, MISSING);
    assert_ne!(denied.message, missing.message);
}

#[test]
fn a_directory_is_not_a_file_this_action_opens() {
    let dir = TempDir::new().expect("temp");
    let root = dir.path().join("repo");
    make_file(&root.join("docs/SETUP.md"));

    let error = validated_file(&root, "docs").expect_err("a folder is not a file");

    assert_eq!(error.code, ErrorCode::Io);
    assert_eq!(error.message, NOT_A_FILE);
}

#[test]
fn a_workspace_folder_this_machine_cannot_see_is_an_availability_error() {
    let dir = TempDir::new().expect("temp");
    let missing_root = dir.path().join("never-created");

    let error = validated_file(&missing_root, "anything.txt").expect_err("no root, no open");

    assert_eq!(error.code, ErrorCode::WorkspaceUnavailable);
    assert_eq!(error.message, ROOT_GONE);
}

/// Windows-only, like every link case in this crate's suites: a symlink
/// needs a privilege this machine may not grant, and the test says so and
/// returns instead of failing — the traversal and absolute cases above
/// still pin the containment rule itself.
#[test]
#[cfg(windows)]
fn a_symlinked_child_pointing_outside_is_refused_at_spawn_time() {
    let dir = TempDir::new().expect("temp");
    let root = dir.path().join("repo");
    make_file(&root.join("real.txt"));
    let outside = dir.path().join("outside.txt");
    make_file(&outside);
    let link = root.join("link.txt");
    if let Err(error) = std::os::windows::fs::symlink_file(&outside, &link) {
        eprintln!("skipping: this machine refuses to create a symlink ({error})");
        return;
    }

    let error = validated_file(&root, "link.txt").expect_err("the link escapes the root");

    assert_eq!(error.code, ErrorCode::Io);
    assert_eq!(error.message, OUTSIDE);
    let _ = std::fs::remove_file(&link);
}

/// Same skip rule as the child case above.
#[test]
#[cfg(windows)]
fn a_symlinked_root_resolves_a_file_inside_it() {
    let dir = TempDir::new().expect("temp");
    let real = dir.path().join("real-root");
    make_file(&real.join("inside.txt"));
    let linked = dir.path().join("link-root");
    if let Err(error) = std::os::windows::fs::symlink_dir(&real, &linked) {
        eprintln!("skipping: this machine refuses to create a symlink ({error})");
        return;
    }

    let validated = validated_file(&linked, "inside.txt").expect("a symlinked root is fine");

    assert_eq!(
        validated.file,
        std::fs::canonicalize(real.join("inside.txt")).expect("canonical file")
    );
    let _ = std::fs::remove_dir(&linked);
}

#[test]
fn an_executable_inside_the_workspace_is_refused() {
    let dir = TempDir::new().expect("temp");
    let root = dir.path().join("repo");
    make_file(&root.join("tools/idea64.exe"));
    let canonical_root = std::fs::canonicalize(&root).expect("canonical root");
    let program = std::fs::canonicalize(root.join("tools/idea64.exe")).expect("canonical program");

    let error =
        trusted_program(&program, &canonical_root).expect_err("a workspace program must refuse");

    assert_eq!(error.code, ErrorCode::Io);
    assert_eq!(error.message, "the chosen editor is inside this workspace");
}

/// The accepted model: an absolute PATH entry is the user's own
/// configuration, so a program under another workspace's root launches.
#[test]
fn an_executable_in_a_different_workspace_is_accepted() {
    let dir = TempDir::new().expect("temp");
    let root = dir.path().join("repo-b");
    std::fs::create_dir_all(&root).expect("root");
    let other = dir.path().join("repo-a");
    make_file(&other.join("bin/idea64.exe"));
    let canonical_root = std::fs::canonicalize(&root).expect("canonical root");
    let program = std::fs::canonicalize(other.join("bin/idea64.exe")).expect("canonical program");

    trusted_program(&program, &canonical_root).expect("another root is the user's own PATH");
}

#[test]
fn an_executable_outside_the_workspace_is_accepted() {
    let dir = TempDir::new().expect("temp");
    let root = dir.path().join("repo");
    std::fs::create_dir_all(&root).expect("root");
    let tools = dir.path().join("tools");
    make_file(&tools.join("idea64.exe"));
    let canonical_root = std::fs::canonicalize(&root).expect("canonical root");
    let program = std::fs::canonicalize(tools.join("idea64.exe")).expect("canonical program");

    trusted_program(&program, &canonical_root).expect("outside the workspace launches");
}
