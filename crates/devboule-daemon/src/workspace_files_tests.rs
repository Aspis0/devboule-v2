//! `directory_of` over plain folders — the answers that come back listed
//! and the refusals that come back with no entries. The rules of one
//! listing (`.git`, order, cap, skipped entries) are in
//! `workspace_files_read_tests.rs`.

use devboule_protocol::{WorkspaceDirectory, WorkspaceFileKind};

use super::directory_of;
use super::fixture::{assert_no_path, unique_directory, Folder};

/// The folder itself: the empty request is the top level, every ordinary
/// entry is there (folders first — the listing's own rule), `.git` is not,
/// and the echo of the request is the empty string rather than a path this
/// side made up.
#[test]
fn the_folder_itself_lists_every_ordinary_entry_and_never_git() {
    let folder = Folder::new("root");
    folder.dir(".git");
    folder.file(".git/config", "[core]\n");
    folder.dir("src");
    folder.file("README.md", "# t\n");

    let listing = directory_of(&folder.root, "");

    assert_eq!(listing.error, None, "{:?}", listing.error);
    assert_eq!(listing.path, "", "the empty request is echoed verbatim");
    assert!(!listing.capped);
    assert_eq!(listing.skipped, 0, "nothing here failed the survival test");
    let rendered: Vec<(&str, bool)> = listing
        .entries
        .iter()
        .map(|entry| (entry.name.as_str(), entry.kind == WorkspaceFileKind::Dir))
        .collect();
    assert_eq!(
        rendered,
        [("src", true), ("README.md", false)],
        "{rendered:?}"
    );
    assert!(
        !listing.entries.iter().any(|entry| entry.name == ".git"),
        ".git must not be listed: {:?}",
        listing.entries
    );
}

/// One directory per request: a nested folder answers for itself, its
/// entries carry paths under it, and the two dirty spellings of the same
/// folder (`sub/`, `./sub`) produce the same child paths a clean one does —
/// a key the panel expands by must not depend on how the request was typed.
#[test]
fn a_nested_folder_lists_its_entries_under_its_own_wire_path() {
    let folder = Folder::new("nested");
    folder.file("sub/child.txt", "c");
    folder.file("sub/sibling.txt", "s");

    for requested in ["sub", "sub/", "./sub"] {
        let listing = directory_of(&folder.root, requested);
        assert_eq!(listing.error, None, "{requested}: {:?}", listing.error);
        assert_eq!(listing.path, requested, "the echo stays verbatim");
        let paths: Vec<&str> = listing
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect();
        assert_eq!(paths, ["sub/child.txt", "sub/sibling.txt"], "{requested}");
    }
}

/// Mutant `m:a` — drop the confinement of `path`. A real file outside the
/// workspace is named by both escapes, and only the component rules stop
/// them; an absolute path is matched by component, because `is_absolute()`
/// calls a bare `/x` relative on Windows.
#[test]
fn paths_that_leave_the_workspace_are_refused() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let outside_name = format!("devboule-files-outside-{}-{stamp}", std::process::id());
    let folder = Folder::new("escape");
    let parent = folder.root.parent().expect("temp parent").to_path_buf();
    let outside = parent.join(&outside_name);
    std::fs::create_dir(&outside).expect("outside dir");
    folder.file("in.txt", "a\n");

    for requested in [format!("../{outside_name}"), "/absolute/escape".to_string()] {
        let listing = directory_of(&folder.root, &requested);
        assert!(
            listing.entries.is_empty(),
            "{requested} was not refused: {listing:?}"
        );
        assert_eq!(
            listing.path, requested,
            "a refusal echoes the rejected text verbatim"
        );
        let error = listing.error.as_deref().expect("the refusal says why");
        assert!(error.contains("outside the workspace folder"), "{error}");
        assert_no_path(error);
    }
    let _ = std::fs::remove_dir_all(&outside);
}

/// Two different facts, two different sentences: a component nothing ever
/// statted does not exist, and an existing file is not a folder. Collapsed
/// into one, the panel could not tell a typo from a wrong click.
#[test]
fn a_missing_path_and_a_file_path_are_refused_with_their_own_sentences() {
    let folder = Folder::new("kinds");
    folder.file("real.txt", "a\n");

    let missing = directory_of(&folder.root, "never-existed");
    assert!(missing.entries.is_empty(), "{missing:?}");
    let sentence = missing.error.as_deref().expect("the refusal says why");
    assert!(sentence.contains("does not exist"), "{sentence}");
    assert_no_path(sentence);

    let file = directory_of(&folder.root, "real.txt");
    assert!(file.entries.is_empty(), "{file:?}");
    let sentence = file.error.as_deref().expect("the refusal says why");
    assert!(sentence.contains("not a folder"), "{sentence}");
    assert_no_path(sentence);
}

/// `.git` is excluded from every listing, so naming it is refused too: one
/// exclusion, one meaning, no second way in through the request field.
///
/// The spellings are the ones Win32 resolves to the *same* directory:
/// NTFS matches names case-insensitively and the Win32 path resolution
/// strips trailing dots and spaces — so `.GIT`, `.git.`, `.GIT.` and
/// `.git ` all open the very `.git` below the root. Before the spelling-blind
/// rule this loop went **red** on this machine (log
/// `fix-slice4-r1-red.txt`): the variant was listed instead of refused.
#[test]
fn the_git_folder_is_refused_when_named_directly() {
    let folder = Folder::new("git-direct");
    folder.dir(".git");
    folder.file(".git/config", "[core]\n");
    folder.file("sub/keep.txt", "k\n");
    folder.dir("sub/.git");

    for requested in [
        ".git", ".GIT", ".git.", ".GIT.", ".git ", "sub/.git", "sub/.GIT",
    ] {
        let listing = directory_of(&folder.root, requested);
        assert!(listing.entries.is_empty(), "{requested}: {listing:?}");
        let sentence = listing.error.as_deref().expect("the refusal says why");
        assert!(
            sentence.contains("not part of the tree"),
            "{requested}: {sentence}"
        );
        assert_no_path(sentence);
    }
}

/// A workspace whose folder vanished after the registry had cached it: the
/// shared sentence of the probe's own refusal, with no path in it.
#[test]
fn a_workspace_folder_that_does_not_exist_is_refused_without_a_path() {
    let missing = unique_directory("missing-root");
    // The helper pre-creates the dir; this case needs the path absent.
    let _ = std::fs::remove_dir(&missing);

    let listing = directory_of(&missing, "");
    let _ = std::fs::remove_dir_all(&missing);

    assert!(listing.entries.is_empty(), "{listing:?}");
    let sentence = listing.error.as_deref().expect("the refusal says why");
    assert!(sentence.contains("not a directory"), "{sentence}");
    assert_no_path(sentence);
}

/// Mutant `m:f` — drop the walk's link check. Both link kinds get the SAME
/// sentences the diff shows (they are shared in `workspace_git_support`, not
/// copied): the final component — the junction, or the symlink file — is the
/// link itself and says so; a path behind the junction is refused mid-walk,
/// whether the target outside exists or not.
#[test]
#[cfg(windows)]
fn a_path_through_or_to_a_link_is_refused_with_the_shared_sentences() {
    let folder = Folder::new("links");
    folder.file("in.txt", "a\n");
    folder.file("sub/child.txt", "c\n");
    let outside = unique_directory("links-target");
    std::fs::write(outside.join("present.txt"), "outside reached\n").expect("outside file");
    let junction = folder.root.join("dirlink");
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
    let dangling = folder.root.join("vanished.txt");
    std::os::windows::fs::symlink_file(outside.join("present.txt"), &dangling).expect("symlink");

    for (requested, fragment) in [
        ("dirlink", "symbolic link"),
        ("dirlink/present.txt", "crosses a link"),
        ("vanished.txt", "symbolic link"),
    ] {
        let listing = directory_of(&folder.root, requested);
        assert!(
            listing.entries.is_empty(),
            "{requested} was not refused: {listing:?}"
        );
        let sentence = listing.error.as_deref().expect("the refusal says why");
        assert!(sentence.contains(fragment), "{requested}: {sentence}");
        assert_no_path(sentence);
        assert!(
            !format!("{listing:?}").contains("outside reached"),
            "{requested} read through the link: {listing:?}"
        );
    }
    let _ = std::fs::remove_file(&dangling);
    let _ = std::fs::remove_dir(&junction);
    let _ = std::fs::remove_dir_all(&outside);
}

/// The refusals of this frame and the entries of a listed one, side by side:
/// a refusal carries no entries and never a `capped` flag — there is nothing
/// partial about an answer that did not happen.
#[test]
fn a_refusal_never_carries_entries_or_a_cap_flag() {
    let folder = Folder::new("shape");
    folder.file("real.txt", "a\n");

    let refusals: Vec<WorkspaceDirectory> = [
        directory_of(&folder.root, "../escape"),
        directory_of(&folder.root, "never"),
        directory_of(&folder.root, "real.txt"),
        directory_of(&folder.root, ".git"),
    ]
    .into_iter()
    .collect();

    for listing in refusals {
        assert!(listing.error.is_some(), "{listing:?}");
        assert!(listing.entries.is_empty(), "{listing:?}");
        assert!(!listing.capped, "{listing:?}");
        // A refusal claims nothing about a folder it never listed — not
        // even how much it "skipped".
        assert_eq!(listing.skipped, 0, "{listing:?}");
    }
}
