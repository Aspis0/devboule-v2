//! The sweep against real repositories: which other checkout committed the
//! path, which one has it uncommitted, and which paths are in the path of
//! neither.

use std::path::Path;
use std::time::{Duration, Instant};

use super::fixture::Repo;
use super::{scan, KnownCheckout};
use crate::workspace_files::NOT_PART_OF_THE_TREE;
use crate::workspace_git_support::{git_within, OUTSIDE_THE_WORKSPACE};

/// The sweep's git runner, exactly as the tool supplies it.
fn runner() -> impl Fn(&Path, &[&str]) -> Result<crate::git::GitOutput, crate::git::GitRunError> {
    |root, arguments| git_within(root, arguments, super::COMMAND_TIMEOUT)
}

/// The bound the tool hands the sweep. These repositories answer in
/// milliseconds.
fn plenty_of_time() -> Instant {
    Instant::now() + Duration::from_secs(60)
}

#[test]
fn collision_reports_committed_and_dirty_paths() {
    let repo = Repo::new("sweep");
    repo.write("src/shared.rs", "one\n");
    repo.write("src/other.rs", "one\n");
    repo.write("src/quiet.rs", "one\n");
    repo.commit("first");
    // The branch point: everything the other checkout inherits is this commit
    // and nothing after it, so every base in the answer must be it.
    let branch_point = repo.head(&repo.root);
    let other = repo.add_worktree("feature", "feature-collision");

    repo.write_in(&other, "src/shared.rs", "committed on the other branch\n");
    repo.commit_in(&other, "second");
    // Left uncommitted in that checkout, never in this one.
    repo.write_in(&other, "src/other.rs", "still being written\n");
    // And one the other branch never heard of.
    repo.write("src/third.rs", "one\n");
    repo.commit("third");

    let known = vec![KnownCheckout {
        path: other.clone(),
        workspace_id: "ws-other".to_string(),
    }];
    let run = runner();

    let committed =
        scan(&repo.root, "src/shared.rs", &known, plenty_of_time(), &run).expect("sweep");
    assert!(!committed.capped, "two checkouts fit the cap");
    assert_eq!(committed.worktrees.len(), 1);
    let row = &committed.worktrees[0];
    assert_eq!(row.branch.as_deref(), Some("feature-collision"));
    assert_eq!(row.workspace_id.as_deref(), Some("ws-other"));
    assert_eq!(row.base_sha.as_deref(), Some(branch_point.as_str()));
    assert_eq!(
        row.committed_change,
        Some(true),
        "the other branch committed this file"
    );
    assert!(!row.dirty_change, "and committed it, so it is not dirty");

    let dirty = scan(&repo.root, "src/other.rs", &known, plenty_of_time(), &run).expect("sweep");
    let row = &dirty.worktrees[0];
    assert_eq!(
        row.committed_change,
        Some(false),
        "the other branch never committed this file"
    );
    assert!(row.dirty_change, "but has it open in its working tree");

    let untouched =
        scan(&repo.root, "src/quiet.rs", &known, plenty_of_time(), &run).expect("sweep");
    let row = &untouched.worktrees[0];
    assert_eq!(row.committed_change, Some(false));
    assert!(!row.dirty_change);
    assert_eq!(
        row.base_sha.as_deref(),
        Some(branch_point.as_str()),
        "a quiet file still has a branch point: the sweep answers per worktree, not per file"
    );
}

#[test]
fn collision_reports_the_caller_own_checkout_as_nobody() {
    let repo = Repo::new("self");
    repo.write("seed.txt", "one\n");
    repo.commit("first");
    let other = repo.add_worktree("other", "other-branch");
    repo.write_in(&other, "seed.txt", "two\n");
    repo.commit_in(&other, "second");
    // Registered, but the sweep asks about the repository this session sits
    // in, so its own row is never one of the answers.
    let known = vec![
        KnownCheckout {
            path: repo.root.clone(),
            workspace_id: "ws-caller".to_string(),
        },
        KnownCheckout {
            path: other.clone(),
            workspace_id: "ws-other".to_string(),
        },
    ];
    let run = runner();
    let sweep = scan(&repo.root, "seed.txt", &known, plenty_of_time(), &run).expect("sweep");
    assert_eq!(sweep.worktrees.len(), 1);
    assert_eq!(sweep.worktrees[0].workspace_id.as_deref(), Some("ws-other"));
}

#[test]
fn collision_rejects_path_outside_repo() {
    let repo = Repo::new("outside");
    repo.write("src/kept.rs", "one\n");
    repo.commit("first");
    let root = repo.root.clone();
    let outside = repo.dir.join("elsewhere");
    std::fs::create_dir(&outside).expect("outside folder");
    std::fs::write(outside.join("secret.rs"), "not yours\n").expect("outside file");

    let absolute = outside.join("secret.rs");
    assert_eq!(
        super::confine_subject(&root, absolute.to_str().expect("absolute path")),
        Err(OUTSIDE_THE_WORKSPACE),
        "an absolute path into another folder is not a subject"
    );
    assert_eq!(
        super::confine_subject(&root, "../elsewhere/secret.rs"),
        Err(OUTSIDE_THE_WORKSPACE),
        "a parent hop out of the folder is not a subject"
    );
    assert_eq!(
        super::confine_subject(&root, ".git/config"),
        Err(NOT_PART_OF_THE_TREE),
        "the repository's own metadata is not a subject"
    );

    let escape = root.join("escape");
    #[cfg(windows)]
    let linked = std::process::Command::new("cmd")
        .args([
            "/c",
            "mklink",
            "/J",
            &escape.to_string_lossy(),
            &outside.to_string_lossy(),
        ])
        .output()
        .expect("mklink runs")
        .status
        .success();
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&outside, &escape).is_ok();
    assert!(linked, "could not create the link this test is about");
    let crossed = super::confine_subject(&root, "escape/secret.rs")
        .expect_err("a path reached through a link out of the folder is not a subject");
    assert!(crossed.contains("link"), "{crossed}");

    // The refusals are the only answers here: the folder itself and a file
    // inside it are both legitimate subjects, so the rule is not a blanket one.
    assert_eq!(super::confine_subject(&root, ".").as_deref(), Ok("."));
    assert_eq!(
        super::confine_subject(&root, "src/kept.rs").as_deref(),
        Ok("src/kept.rs")
    );
    assert_eq!(
        super::confine_subject(&root, ".\\src\\kept.rs").as_deref(),
        Ok("./src/kept.rs"),
        "the answer is the one spelling both consumers match on"
    );
    assert!(super::confine_subject(&root, "src/never-written.rs").is_ok());
}
