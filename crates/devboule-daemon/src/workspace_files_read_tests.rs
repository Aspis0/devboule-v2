//! The rules of one listing, on plain folders with no repository in them:
//! which entries survive (`.git` never; a link or an entry that will not
//! stat is skipped, not fatal), in which order (folders first, then byte
//! order), and how many one reply carries (the cap that declares itself).

use devboule_protocol::{WorkspaceFileEntry, WorkspaceFileKind};

use super::fixture::Folder;
use super::read::{ordered, read, MAX_ENTRIES};

/// Mutants `m:b` and `m:r1` — drop the `.git` exclusion, or make it a
/// byte-compare again. The folders and files around it pin the whole
/// ordering too: `crates` is a folder and sorts before every file, and
/// `Zeta.c` precedes `alpha.txt` — byte order, where a locale collation
/// (`localeCompare`) would put `alpha` first.
///
/// The metadata folder is created as **`.GIT`**, not `.git`: NTFS matches
/// names case-insensitively, so the two spellings are the same directory and
/// this fixture is the listing-side half of the spelling-blind rule. Before
/// that rule this test went **red** on this machine — `.GIT` was listed —
/// (log `fix-slice4-r1-red.txt`).
#[test]
fn git_is_excluded_and_every_other_entry_lists_folders_first_in_byte_order() {
    let folder = Folder::new("rules");
    folder.dir(".GIT");
    folder.file(".GIT/config", "[core]\n");
    folder.dir("crates");
    folder.file("Zeta.c", "z");
    folder.file("alpha.txt", "a");
    folder.file("beta.rs", "b");

    let listed = read(&folder.root, "").expect("an ordinary folder lists");

    let rendered: Vec<(&str, bool)> = listed
        .entries
        .iter()
        .map(|entry| (entry.name.as_str(), entry.kind == WorkspaceFileKind::Dir))
        .collect();
    assert_eq!(
        rendered,
        [
            ("crates", true),
            ("Zeta.c", false),
            ("alpha.txt", false),
            ("beta.rs", false),
        ],
        "{rendered:?}"
    );
    assert!(
        !listed
            .entries
            .iter()
            .any(|entry| entry.name.eq_ignore_ascii_case(".git")),
        ".git (in any ASCII case) must not be listed: {:?}",
        listed.entries
    );
    assert!(!listed.capped, "{:?}", listed.capped);
    // `.git` is the tree's declared policy exclusion, not a hidden entry:
    // it must stay out of `skipped` too (R2's carve-out).
    assert_eq!(listed.skipped, 0, "{:?}", listed.skipped);
    // Sizes: a folder has none to show, a file has the one stat reported —
    // and a file's own path rides under the prefix it was asked with.
    for entry in &listed.entries {
        assert_eq!(
            entry.size.is_some(),
            entry.kind == WorkspaceFileKind::File,
            "{entry:?}"
        );
    }
    let prefixed = read(&folder.root, "crates").expect("a prefixed listing");
    assert!(
        prefixed
            .entries
            .iter()
            .all(|entry| entry.path.starts_with("crates/")),
        "entries keep their folder's wire spelling: {:?}",
        prefixed.entries
    );
}

/// Mutant `m:d` — order with `localeCompare` (or not at all). The comparator
/// itself is pure, so the rule is proven from four strings: folders first
/// whatever their name, and names in byte order — `Beta` and `Zeta.c` before
/// `alpha`, which is exactly what a locale collation would flip.
#[test]
fn the_order_is_folders_first_then_byte_order_of_names() {
    let entry = |name: &str, kind| WorkspaceFileEntry {
        path: name.to_string(),
        name: name.to_string(),
        kind,
        size: None,
    };
    let mut entries = [
        entry("alpha", WorkspaceFileKind::File),
        entry("Zeta.c", WorkspaceFileKind::File),
        entry("src", WorkspaceFileKind::Dir),
        entry("Beta", WorkspaceFileKind::File),
        entry("_assets", WorkspaceFileKind::Dir),
    ];

    entries.sort_by(ordered);

    let rendered: Vec<&str> = entries.iter().map(|entry| entry.name.as_str()).collect();
    assert_eq!(rendered, ["_assets", "src", "Beta", "Zeta.c", "alpha"]);
}

/// Mutant `m:c` — drop the cap (or apply it after the entries are already
/// out). The fixture is a folder with [`MAX_ENTRIES`] + 3 files: only the
/// counting loop can refuse it, and what comes back must be exactly the cap
/// with `capped` saying the list is partial — never `MAX_ENTRIES + 3`, and
/// never a silent cut.
#[test]
fn past_the_cap_the_list_stops_at_the_cap_and_declares_itself() {
    let folder = Folder::new("cap");
    for index in 0..(MAX_ENTRIES + 3) {
        folder.file(&format!("f{index:05}.txt"), "x");
    }

    let listed = read(&folder.root, "").expect("the folder lists");

    assert!(listed.capped, "a cut list must say so: {:?}", listed.capped);
    assert_eq!(
        listed.skipped, 0,
        "entries past the cap belong to `capped`, never to `skipped`"
    );
    assert_eq!(
        listed.entries.len(),
        MAX_ENTRIES,
        "the cap is a stop, not a suggestion"
    );

    let under = Folder::new("under-cap");
    for index in 0..3 {
        under.file(&format!("f{index}.txt"), "x");
    }
    let short = read(&under.root, "").expect("the folder lists");
    assert!(!short.capped, "an uncapped list must not claim a cut");
}

/// Mutant `m:e` — make an entry that fails the survival test fail the whole
/// listing instead of being skipped. The two link fixtures are the measured
/// cases: a junction (`mklink /J`, no privilege needed) and a symlink whose
/// target has vanished — the brief's "target sparito" — plus the ordinary
/// entries around them, which must still come back in one answer.
#[test]
#[cfg(windows)]
fn an_entry_that_does_not_survive_is_skipped_and_the_list_stands() {
    let folder = Folder::new("skip");
    folder.file("real.txt", "r");
    folder.dir("ordinary");
    let outside = super::fixture::unique_directory("skip-target");
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
    std::os::windows::fs::symlink_file(folder.root.join("never-existed.bin"), &dangling)
        .expect("symlink");

    let listed = read(&folder.root, "").expect("one bad entry must not fail the listing");

    let rendered: Vec<&str> = listed
        .entries
        .iter()
        .map(|entry| entry.name.as_str())
        .collect();
    assert_eq!(rendered, ["ordinary", "real.txt"], "{rendered:?}");
    assert!(!listed.capped);
    // R2: the two links that did not survive are counted — a folder with a
    // link inside declares it instead of looking complete.
    assert_eq!(listed.skipped, 2, "{:?}", listed.skipped);
    // The links go before `Folder`'s cleanup: `remove_dir_all` must never
    // be asked to decide what a junction points at.
    let _ = std::fs::remove_file(&dangling);
    let _ = std::fs::remove_dir(&junction);
    let _ = std::fs::remove_dir_all(&outside);
    let _ = std::fs::remove_dir_all(&folder.root);
}
