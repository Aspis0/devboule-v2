//! The no-follow rules of `test_dirs`, on a real link each platform makes
//! without privileges: a symlink on unix, a directory junction on Windows.
//! A planted link must be refused by name, skipped by the sweep, and taken
//! as a link by the Windows fallback — the target on the far side is never
//! deleted through, and never re-granted.

use std::path::Path;

use crate::test_dirs::{is_link, refuse_if_link, sweep_runs_under, test_temp_dir};

/// The entry's own metadata, the only kind both rules read.
fn metadata(path: &Path) -> std::fs::Metadata {
    std::fs::symlink_metadata(path).expect("the entry's own metadata")
}

/// A link at `link` pointing at `target`: a symlink on unix, and on Windows
/// a directory junction, because `mklink /J` needs no privilege while a
/// symlink needs an unchecked developer mode.
fn plant_a_link(target: &Path, link: &Path) {
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, link).expect("the symlink is made");

    #[cfg(windows)]
    {
        let made = std::process::Command::new("cmd")
            .args(["/c", "mklink", "/J"])
            .arg(link)
            .arg(target)
            .output()
            .expect("mklink runs");
        assert!(
            made.status.success(),
            "mklink: {}",
            String::from_utf8_lossy(&made.stderr)
        );
    }
}

#[test]
fn an_ordinary_directory_and_file_are_not_links() {
    let dir = test_temp_dir("devboule-not-a-link");
    assert!(!is_link(&metadata(&dir)));
    let file = dir.join("plain.txt");
    std::fs::write(&file, b"plain").expect("a file");
    assert!(!is_link(&metadata(&file)));
}

/// The refusal reads the entry's own metadata and stops on a link, so a
/// root planted over the helper's is never used — and never swept through.
#[test]
fn a_planted_link_is_refused_by_name() {
    let base = test_temp_dir("devboule-planted-root");
    let target = base.join("target");
    std::fs::create_dir(&target).expect("a real directory");
    let planted = base.join("planted");
    plant_a_link(&target, &planted);

    assert!(is_link(&metadata(&planted)), "the entry reads as a link");
    let caught = std::panic::catch_unwind(|| refuse_if_link(&planted));
    let payload = caught.expect_err("a planted link must stop the helper");
    let message = payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or_default();
    assert!(
        message.contains("refusing to use it or sweep through it"),
        "{message}"
    );
    assert!(target.is_dir(), "the refusal changed nothing");
}

/// In the sweep: a link standing where a run root would be is left alone
/// whatever its name says, and nothing is deleted on the far side. The name
/// is a pid that is not this process's, so the entry would be a candidate
/// if the link check were missing — and under a live pid the sweep skips it
/// anyway, which keeps the assertion true either way liveness reads.
#[test]
fn the_sweep_skips_a_link_and_never_deletes_through_it() {
    let root = test_temp_dir("devboule-sweep-planted");
    let target = root.join("target");
    std::fs::create_dir(&target).expect("a real directory");
    let keeper = target.join("keep.txt");
    std::fs::write(&keeper, b"kept").expect("a file to protect");
    let planted = root.join("4194304");
    plant_a_link(&target, &planted);

    sweep_runs_under(root.as_path(), std::process::id());

    assert!(
        is_link(&metadata(&planted)),
        "the planted link is still there"
    );
    assert!(keeper.exists(), "nothing was deleted through the link");
}

/// The Windows fallback takes a junction as a link, before any ACL work:
/// the junction itself goes, its target keeps its files.
#[cfg(windows)]
#[test]
fn the_fallback_removes_a_junction_and_leaves_its_target() {
    let base = test_temp_dir("devboule-junction");
    let target = base.join("target");
    std::fs::create_dir(&target).expect("a real directory");
    let keeper = target.join("keep.txt");
    std::fs::write(&keeper, b"kept").expect("a file to protect");
    let planted = base.join("junction");
    plant_a_link(&target, &planted);

    crate::test_dirs::force_remove(&planted);

    assert!(
        std::fs::symlink_metadata(&planted).is_err(),
        "the junction itself is gone"
    );
    assert!(keeper.exists(), "the target keeps its files");
}
