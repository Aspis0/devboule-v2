//! The rules of one directory listing, over a directory
//! [`crate::workspace_files`] already vouched for: which entries survive,
//! in which order, and how many one reply carries. The refusal side (the
//! confinement, the walk, the folder's own kind) lives in
//! `workspace_files.rs`; this half only reads.

use std::cmp::Ordering;
use std::path::Path;

use devboule_protocol::{WorkspaceFileEntry, WorkspaceFileKind};

use crate::workspace_git_support::crosses_a_link;

/// Entries one reply will carry. Past it the folder is listed partially,
/// with `capped` saying so — never truncated in silence, and never an
/// unbounded read of a folder nobody chose.
pub(super) const MAX_ENTRIES: usize = 1000;

/// The folder itself could not be opened: the one sentence this half makes,
/// pathless like the rest of the frames.
pub(super) const LIST_FAILED: &str = "the folder could not be listed";

/// One directory's surviving entries (already ordered) plus what the reply
/// did **not** carry: `capped` for the entries the cap stopped, `skipped`
/// for the entries that failed the survival test.
pub(super) struct Listed {
    pub(super) entries: Vec<WorkspaceFileEntry>,
    pub(super) capped: bool,
    pub(super) skipped: u64,
}

/// Read the entries of `directory`, each entry's `path` built under
/// `prefix` (the wire spelling of this folder, empty at the top level).
///
/// Four rules decide what an entry is:
/// - `.git` is excluded (DECISIONS §6: git is the authority for status; the
///   tree lists everything else). That exclusion is **not** in `skipped` —
///   it is the tree's declared policy, not a hidden entry;
/// - an entry must stat **without following** and must be an ordinary entry.
///   A vanished or unreadable entry, a symlink and a junction all fail that
///   one test on one line, and all are **skipped** — a single bad entry never
///   fails the whole listing (Paseo's rule, `service.ts:164-174`), and a
///   link's target is never classified, which is why `kind` is only ever
///   `dir` or `file`. Every such skip is **counted** in `skipped`: a folder
///   with a link inside says so instead of looking complete;
/// - past [`MAX_ENTRIES`] the listing stops and `capped` declares the cut
///   (entries the cap dropped belong to `capped`, never to `skipped`);
/// - names are compared spelling-blind ([`is_git_metadata`]), because the
///   filesystem resolves spellings a byte-compare would not.
pub(super) fn read(directory: &Path, prefix: &str) -> Result<Listed, &'static str> {
    let stream = std::fs::read_dir(directory).map_err(|_| LIST_FAILED)?;
    let mut entries: Vec<WorkspaceFileEntry> = Vec::new();
    let mut capped = false;
    let mut skipped: u64 = 0;
    for item in stream {
        // An entry the stream itself could not hand over is the same class
        // as one that will not stat: skipped, never fatal — and counted.
        let Ok(entry) = item else {
            skipped += 1;
            continue;
        };
        let name = entry.file_name().to_string_lossy().into_owned();
        if is_git_metadata(&name) {
            // Policy exclusion (DECISIONS §6), not a hidden entry: it is
            // not part of `skipped`.
            continue;
        }
        let survived = entry
            .metadata()
            .ok()
            .filter(|metadata| !crosses_a_link(metadata));
        let Some(metadata) = survived else {
            skipped += 1;
            continue;
        };
        let is_dir = metadata.is_dir();
        entries.push(WorkspaceFileEntry {
            path: if prefix.is_empty() {
                name.clone()
            } else {
                format!("{prefix}/{name}")
            },
            name,
            kind: if is_dir {
                WorkspaceFileKind::Dir
            } else {
                WorkspaceFileKind::File
            },
            size: (!is_dir).then_some(metadata.len()),
        });
        if entries.len() > MAX_ENTRIES {
            // One entry past the cap is read only to learn the folder holds
            // more than the reply carries; it is not part of the answer.
            capped = true;
            entries.pop();
            break;
        }
    }
    entries.sort_by(ordered);
    Ok(Listed {
        entries,
        capped,
        skipped,
    })
}

/// Whether a name is the repository's own metadata folder — in **any**
/// spelling the filesystem resolves to it. NTFS matches names
/// case-insensitively and Win32's path resolution strips trailing dots and
/// spaces, so `.GIT`, `.git.` and `.git ` all open the real `.git` —
/// measured on this machine: before this rule the request guard *listed*
/// `.git/config` behind the spelling `.GIT` (log `fix-slice4-r1-red.txt`).
/// One function for both places the exclusion lives (the request guard and
/// this filter): two spellings of one exclusion would be two truths.
pub(super) fn is_git_metadata(name: &str) -> bool {
    name.trim_end_matches(['.', ' '])
        .eq_ignore_ascii_case(".git")
}

/// Folders first, then by name — and by name in **byte order** (`str::cmp`
/// is lexicographic on bytes: ASCII order in the ASCII range, code-point
/// order past it), deliberately not a locale collation such as
/// `localeCompare`. The same choice Paseo's diff-tree makes, repeated here:
/// one authority for order (the daemon sorts), and the panel renders the
/// order it is given instead of sorting a second time.
pub(super) fn ordered(a: &WorkspaceFileEntry, b: &WorkspaceFileEntry) -> Ordering {
    rank(a.kind)
        .cmp(&rank(b.kind))
        .then_with(|| a.name.cmp(&b.name))
}

fn rank(kind: WorkspaceFileKind) -> u8 {
    match kind {
        WorkspaceFileKind::Dir => 0,
        WorkspaceFileKind::File => 1,
    }
}
