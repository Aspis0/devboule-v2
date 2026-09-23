//! The Files panel's three write acts — rename, duplicate, delete — over
//! one workspace folder.
//! Orchestration only, the writing twin of [`crate::workspace_file_read`]:
//! resolve the folder from a workspace id — never a request field — confine
//! the requested path to it with the same two-layer rule the reads use
//! (lexical components plus a walk that refuses links and junctions, both
//! through [`crate::workspace_git_support`], so the sentences are shared and
//! not copied), refuse the repository's metadata with the listing's own
//! guard, and only then act. Every refusal in this module is static or the
//! registry's own: no path reaches a sentence.
//!
//! Two races are part of this module's contract and both are fought where
//! they occur: the rename destination is stat'ed and then **claimed
//! exclusively** ([`claim_then_move`]) — a name created in between is
//! refused, never replaced — and `git mv` keeps its own window, which
//! [`rename_on_disk`] measures and states instead of assuming away. The
//! delete is vouched the same way **before** anything is touched, and
//! because it is the one act that loses data its act is anchored where the
//! platform allows — the open and the disposition on a handle for a file,
//! a handle's own location verdict before the recursive removal for a
//! folder, with the window that remains declared there. That act lives in
//! its own file, [`delete`], split by responsibility. The confirmation is
//! not on this wire at all: it is the Files screen's own gate, and a peer
//! holding the admin capability acts under it as behind every
//! administrative door — an open product question, `DECISIONS-write.md`.

use std::fs::File;
use std::path::{Component, Path, PathBuf};

use devboule_protocol::{DaemonMessage, WorkspaceFileMutation};

use crate::workspace_files::{names_git_metadata, DOES_NOT_EXIST, NOT_PART_OF_THE_TREE};
use crate::workspace_git_support::{
    confined, crosses_a_link, exit_error, git, probe, run_error, walk, Walked,
    OUTSIDE_THE_WORKSPACE,
};
use crate::ServerState;

/// The workspace's own folder: the id resolved it, so there is no parent to
/// act from and no second spelling to give it (Paseo refuses the same three
/// spellings, `service.ts:640-697-752`). Pathless, like every sentence here.
const THE_ROOT: &str = "the workspace's own folder cannot be renamed, duplicated or deleted";
const NAME_EMPTY: &str = "the new name is empty";
const NAME_SEPARATOR: &str = "the new name must not contain a path separator";
const NAME_DOT: &str = "the new name must not be `.` or `..`";
/// Anything that does not parse as one ordinary name below the root — a
/// drive-relative spelling like `C:x` on Windows — says so in its own words:
/// the separator sentence above would be a different (false) reason.
const NAME_INVALID: &str = "the new name must be a single name below the folder";
/// Windows would silently rewrite such a spelling (Win32 drops trailing dots
/// and spaces on create), and the reply would then claim a name the tree
/// never shows — so it is refused at the same door as the rest.
const NAME_TRAILING: &str = "the new name must not end in a dot or a space";
const NAME_TAKEN: &str = "an entry with that new name already exists";
const RENAME_FAILED: &str = "the entry could not be renamed";
const COPY_FAILED: &str = "the entry could not be duplicated";
/// The one rule this tree keeps everywhere: a link is never followed and
/// never recreated. A folder copy that meets one is removed whole, so the
/// sentence is true — nothing was copied.
const COPY_MEETS_A_LINK: &str = "a folder being duplicated contains a link; nothing was copied";

/// Resolve `workspace_id` through the registry — never a request field.
pub(crate) fn reply_rename(
    state: &ServerState,
    id: u64,
    workspace_id: &str,
    path: &str,
    name: &str,
) -> DaemonMessage {
    let change = match state.sessions.workspace_cwd(workspace_id) {
        Ok(root) => match renamed(&root, path, name) {
            Ok(new_path) => answered(new_path),
            Err(sentence) => refused(sentence),
        },
        // The sentence comes from the registry and names no path (see
        // `workspace_cwd`): it is safe to echo on this frame.
        Err(error) => refused(error.message),
    };
    DaemonMessage::WorkspaceFileRenamed { id, change }
}

/// Resolve `workspace_id` through the registry — never a request field.
pub(crate) fn reply_duplicate(
    state: &ServerState,
    id: u64,
    workspace_id: &str,
    path: &str,
) -> DaemonMessage {
    let change = match state.sessions.workspace_cwd(workspace_id) {
        Ok(root) => match duplicated(&root, path) {
            Ok(new_path) => answered(new_path),
            Err(sentence) => refused(sentence),
        },
        Err(error) => refused(error.message),
    };
    DaemonMessage::WorkspaceFileDuplicated { id, change }
}

/// Resolve `workspace_id` through the registry — never a request field.
pub(crate) fn reply_delete(
    state: &ServerState,
    id: u64,
    workspace_id: &str,
    path: &str,
) -> DaemonMessage {
    let change = match state.sessions.workspace_cwd(workspace_id) {
        Ok(root) => match delete::deleted(&root, path) {
            Ok(()) => erased(),
            Err(sentence) => refused(sentence),
        },
        Err(error) => refused(error.message),
    };
    DaemonMessage::WorkspaceFileDeleted { id, change }
}

/// One write succeeded: the entry's new spelling, `/`-joined the way the
/// listing builds its entries.
fn answered(new_path: String) -> WorkspaceFileMutation {
    WorkspaceFileMutation {
        new_path: Some(new_path),
        error: None,
    }
}

/// One write refused: the sentence says what stopped it, and the reply
/// claims no path — a refusal says nothing about where anything is.
fn refused(sentence: impl Into<String>) -> WorkspaceFileMutation {
    WorkspaceFileMutation {
        new_path: None,
        error: Some(sentence.into()),
    }
}

/// The delete's own success shape: no new spelling to name and nothing
/// left to say it about — both fields `None`, the one reply of this module
/// that speaks entirely through its emptiness.
fn erased() -> WorkspaceFileMutation {
    WorkspaceFileMutation {
        new_path: None,
        error: None,
    }
}

/// The entry a write acts on, vouched the way the reads are: the workspace's
/// own folder (in any spelling, `""` included) refused first, then
/// confinement, then the listing's `.git` guard, then the walk.
fn entry_at(root: &Path, requested: &str) -> Result<PathBuf, String> {
    vouched_entry(root, requested).map(|(path, _metadata)| path)
}

/// The same vouching with the walk's own stat kept: the delete reads its
/// folder-vs-file branch off the stat that vouched the entry, rather than
/// stat'ing the name a second time and trusting two lookups more than one.
fn vouched_entry(root: &Path, requested: &str) -> Result<(PathBuf, std::fs::Metadata), String> {
    // The root itself — no parent to act from. Checked ahead of the
    // confinement, which would call the empty spelling an escape.
    if Path::new(requested)
        .components()
        .all(|component| matches!(component, Component::CurDir))
    {
        return Err(THE_ROOT.to_string());
    }
    let Some(target) = confined(root, requested) else {
        return Err(OUTSIDE_THE_WORKSPACE.to_string());
    };
    if names_git_metadata(requested) {
        return Err(NOT_PART_OF_THE_TREE.to_string());
    }
    match walk(root, requested) {
        Walked::Link(sentence) => Err(sentence.to_string()),
        Walked::Missing => Err(DOES_NOT_EXIST.to_string()),
        Walked::Inside(metadata) => Ok((target, metadata)),
    }
}

/// The new name, judged the way Paseo judges a created name
/// (`service.ts:599-605`) — trimmed first, and the trimmed spelling is the
/// one acted on, because validating one string and storing another would
/// keep a name the rule never judged. One name: not empty, not `.`/`..`, no
/// separator, never Win32's silent rewrites, and never the repository's own
/// metadata folder — the guard the listing runs, on the other end of the
/// rename.
fn usable_name(name: &str) -> Result<String, String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(NAME_EMPTY.to_string());
    }
    if trimmed == "." || trimmed == ".." {
        return Err(NAME_DOT.to_string());
    }
    if trimmed.contains(['/', '\\']) {
        return Err(NAME_SEPARATOR.to_string());
    }
    // Before the trailing-dot rule: `.git.` and `.git ` end in the very
    // characters Win32 drops, and the honest sentence for them is the
    // listing's exclusion, not the spelling rule.
    if names_git_metadata(trimmed) {
        return Err(NOT_PART_OF_THE_TREE.to_string());
    }
    if trimmed.ends_with(['.', ' ']) {
        return Err(NAME_TRAILING.to_string());
    }
    // Exactly one normal component: the checks above caught every spelling
    // a plain `contains` sees; this catches what a Windows path parses as a
    // drive prefix (`C:x`), which has no separator in it at all.
    let mut components = Path::new(trimmed).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) => {}
        _ => return Err(NAME_INVALID.to_string()),
    }
    Ok(trimmed.to_string())
}

/// Rename one entry and hand back its new spelling. Private on purpose:
/// callers outside this module arrive through [`reply_rename`], and the test
/// module is a child.
fn renamed(root: &Path, requested: &str, name: &str) -> Result<String, String> {
    let source = entry_at(root, requested)?;
    let new_name = usable_name(name)?;
    let destination = source
        .parent()
        .expect("a confined entry always has a parent")
        .join(&new_name);
    let landing = match std::fs::symlink_metadata(&destination) {
        Ok(_) if case_only_of(&source, &destination, &new_name) => Landing::CaseOnly,
        Ok(_) => return Err(NAME_TAKEN.to_string()),
        Err(_) => Landing::Free,
    };
    rename_on_disk(root, requested, &source, &destination, &new_name, landing)
}

/// Where a rename lands, read off the destination's own stat — that stat is
/// a check, not a promise: [`claim_then_move`] is what keeps the promise.
enum Landing {
    /// Nothing there when last looked: the act must claim the name itself,
    /// because `fs::rename` replaces whatever appeared in the meantime.
    Free,
    /// The same entry under another case — the one existing name a rename
    /// may take; nothing but the entry itself stands behind it.
    CaseOnly,
}

/// Whether an existing destination is the entry being renamed, spelled with
/// a different case — the one name a rename may land on. Both halves are
/// proved: the two spellings are equal ignoring case, and one canonical path
/// stands behind them (on a case-sensitive filesystem two spellings are two
/// entries and this never fires — `fs::rename` would otherwise overwrite).
fn case_only_of(source: &Path, destination: &Path, new_name: &str) -> bool {
    let source_name = source.file_name().unwrap_or_default();
    let source_name = source_name.to_string_lossy();
    let case_only = new_name != source_name && new_name.eq_ignore_ascii_case(&source_name);
    case_only
        && std::fs::canonicalize(source)
            .ok()
            .zip(std::fs::canonicalize(destination).ok())
            .is_some_and(|(left, right)| left == right)
}

/// The act itself. An entry git tracks is renamed through `git mv`, so the
/// act arrives in the Changes panel staged — one rename there, not a
/// deletion beside a stranger (parity with Paseo `service.ts:730`,
/// `DECISIONS-write.md` §3). A workspace that is not a repository root (no
/// repository, a folder inside one, or a probe git did not answer) renames on
/// the filesystem alone: there the Changes panel's own views have the same
/// boundary, so there is nothing staged for.
///
/// **When `git mv` fails — measured on this machine, not assumed**: with the
/// destination already existing git dies `destination exists` **before
/// moving anything**, and with `index.lock` held it dies before moving
/// anything either — both common refusals leave the disk untouched, so the
/// sentence this road returns (`exit_error`: operation + code, never git's
/// stderr) is the truth, and the loser of a lock race against the Changes
/// panel's own 5 s poll is simply whoever arrived second (the per-workspace
/// write mutex belongs to the git-write slice). What git promises no
/// atomicity for is the gap between its move and its index commit: a crash
/// there would leave the file moved on disk with the index on the old
/// spelling — a deletion beside an untracked name, repaired by `git add` of
/// the two paths. That half state is **not reproduced by any test** (said,
/// not hidden); the panel re-lists the parent on refusals as well as on
/// successes (`useWorkspaceFileActions`), so whatever the disk holds is
/// re-read instead of staying on screen.
fn rename_on_disk(
    root: &Path,
    requested: &str,
    source: &Path,
    destination: &Path,
    new_name: &str,
    landing: Landing,
) -> Result<String, String> {
    let mut components: Vec<Component> = Path::new(requested).components().collect();
    components.pop();
    let parent = wire_join(components.into_iter());
    let new_path = if parent.is_empty() {
        new_name.to_string()
    } else {
        format!("{parent}/{new_name}")
    };
    let old_path = wire_join(Path::new(requested).components());
    if probe(root).refusal().is_none() {
        let listed = match git(root, &["ls-files", "-z", "--", &old_path]) {
            Ok(output) if output.success => output,
            Ok(output) => return Err(exit_error("git ls-files", &output)),
            Err(error) => return Err(run_error(error, "git ls-files")),
        };
        if !listed.stdout.is_empty() {
            return match git(root, &["mv", "--", &old_path, &new_path]) {
                Ok(output) if output.success => Ok(new_path),
                Ok(output) => Err(exit_error("git mv", &output)),
                Err(error) => Err(run_error(error, "git mv")),
            };
        }
    }
    let moved = match landing {
        // The destination resolves to the source itself: there is nothing
        // it could stand in for but the entry being renamed.
        Landing::CaseOnly => {
            std::fs::rename(source, destination).map_err(|_| RENAME_FAILED.to_string())
        }
        Landing::Free => claim_then_move(source, destination),
    };
    moved.map(|()| new_path)
}

/// The filesystem road with the destination **claimed before it is taken**:
/// `fs::rename` replaces an existing destination on POSIX and on Windows
/// alike, so the stat in [`renamed`] alone would leave a window — anything
/// created between that stat and this act would be overwritten in silence,
/// and the two promises of this slice ("a taken name is refused", "no data
/// is lost") would be false. The claim is exclusive by construction:
/// `hard_link` for a file and `create_dir` for a folder both fail once the
/// name has appeared, and a failed claim is re-read — the name exists now,
/// so the honest answer is [`NAME_TAKEN`], never a replacement. Where the
/// platform cannot claim (a filesystem without hard links) the road falls
/// back to `fs::rename` after finding the destination absent an instant
/// ago: the window the claim closes is open only there, declared. The
/// case-only landing never comes here — its destination *is* the source.
fn claim_then_move(source: &Path, destination: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(source).map_err(|_| RENAME_FAILED.to_string())?;
    if metadata.is_dir() {
        if std::fs::create_dir(destination).is_err() {
            return Err(claim_lost(destination));
        }
        if std::fs::rename(source, destination).is_err() {
            // The claim is ours alone while it is empty: releasing it this
            // way — never `remove_dir_all`, nothing of another actor is ever
            // destroyed — buys one honest second chance, and a name someone
            // else took in the meantime is refused by name, not replaced.
            let released = std::fs::remove_dir(destination).is_ok();
            if !released || std::fs::rename(source, destination).is_err() {
                return Err(claim_lost(destination));
            }
        }
        return Ok(());
    }
    match std::fs::hard_link(source, destination) {
        Ok(()) => {}
        Err(_) => {
            if std::fs::symlink_metadata(destination).is_ok() {
                return Err(NAME_TAKEN.to_string());
            }
            // Declared: no hard links on this filesystem (or the source
            // vanished) — this is the one road where the window the claim
            // closes is still open.
            return std::fs::rename(source, destination).map_err(|_| RENAME_FAILED.to_string());
        }
    }
    // The name is ours exclusively; dropping the source's own name finishes
    // the rename — same directory, same file, nothing copied.
    std::fs::remove_file(source).map_err(|_| {
        // Roll back toward the source: a refused rename beats a claimed name
        // with the source still standing. Both surviving needs this removal
        // to fail too — no test can force it; declared.
        let _ = std::fs::remove_file(destination);
        RENAME_FAILED.to_string()
    })
}

/// The sentence for a claim that lost: the name exists now — it appeared
/// after the stat, or the release of a folder claim found it touched — so
/// the refusal names that fact; if the name still does not exist, nothing
/// moved and [`RENAME_FAILED`] says exactly that.
fn claim_lost(destination: &Path) -> String {
    if std::fs::symlink_metadata(destination).is_ok() {
        NAME_TAKEN.to_string()
    } else {
        RENAME_FAILED.to_string()
    }
}

/// Duplicate one entry and hand back the copy's spelling — the name the
/// daemon chose, never the caller's.
fn duplicated(root: &Path, requested: &str) -> Result<String, String> {
    let source = entry_at(root, requested)?;
    let destination = free_copy_name(&source);
    copy_entry(&source, &destination)?;
    let relative = destination.strip_prefix(root).unwrap_or(&destination);
    Ok(wire_join(relative.components()))
}

/// The first free `… copy` / `… copy 2` / `… copy 3` name beside the
/// original — Paseo's loop (`service.ts:651-664`), which never overwrites:
/// a name is taken only after its own stat says something is there, and the
/// creation below is exclusive anyway.
fn free_copy_name(source: &Path) -> PathBuf {
    let parent = source
        .parent()
        .expect("a confined entry always has a parent");
    let name = source
        .file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let path = Path::new(&name);
    let stem = path
        .file_stem()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned();
    let extension = match path.extension() {
        // `Path::extension` answers `Some("")` for a name that ends in a dot,
        // and appending that bare dot would spell a candidate Win32 rewrites
        // on create — the `NAME_TRAILING` class this slice refuses for typed
        // names. The generated name obeys the same rules: no extension, no
        // dot. Reachable only where the tree can *see* a dot-terminated
        // name, and on Windows it cannot (the walk stats plain paths, which
        // strip those dots — pinned by its own test), so this guard is
        // structure, not a claim any test here proves.
        Some(extension) if !extension.is_empty() => format!(".{}", extension.to_string_lossy()),
        _ => String::new(),
    };
    let mut attempt = 1usize;
    loop {
        let candidate = if attempt == 1 {
            format!("{stem} copy{extension}")
        } else {
            format!("{stem} copy {attempt}{extension}")
        };
        match std::fs::symlink_metadata(parent.join(&candidate)) {
            Ok(_) => attempt += 1,
            Err(_) => return parent.join(candidate),
        }
    }
}

/// One entry copied, exclusively at every name: `create_dir` refuses an
/// existing folder and `File::create_new` an existing file, so no copy ever
/// overwrites — and a folder that fails half-way is removed whole, so a
/// refusal never leaves a partial copy beside the original. A link is
/// refused, not followed: the same single rule the walk keeps for reads.
fn copy_entry(source: &Path, destination: &Path) -> Result<(), String> {
    let metadata = std::fs::symlink_metadata(source).map_err(|_| COPY_FAILED.to_string())?;
    if crosses_a_link(&metadata) {
        return Err(COPY_MEETS_A_LINK.to_string());
    }
    if metadata.is_dir() {
        std::fs::create_dir(destination).map_err(|_| COPY_FAILED.to_string())?;
        if let Err(sentence) = copy_children(source, destination) {
            let _ = std::fs::remove_dir_all(destination);
            return Err(sentence);
        }
        return Ok(());
    }
    if !metadata.is_file() {
        return Err(COPY_FAILED.to_string());
    }
    copy_file(source, destination)
}

fn copy_children(source: &Path, destination: &Path) -> Result<(), String> {
    let stream = std::fs::read_dir(source).map_err(|_| COPY_FAILED.to_string())?;
    for item in stream {
        let entry = item.map_err(|_| COPY_FAILED.to_string())?;
        copy_entry(&entry.path(), &destination.join(entry.file_name()))?;
    }
    Ok(())
}

fn copy_file(source: &Path, destination: &Path) -> Result<(), String> {
    let mut from = File::open(source).map_err(|_| COPY_FAILED.to_string())?;
    let mut to = File::create_new(destination).map_err(|_| COPY_FAILED.to_string())?;
    std::io::copy(&mut from, &mut to)
        .map(|_bytes_copied| ())
        .map_err(|_| {
            // We created this name; a half-written copy must not survive it.
            let _ = std::fs::remove_file(destination);
            COPY_FAILED.to_string()
        })
}

/// The wire spelling of a path below the root: its normal components
/// rejoined with `/`, the same rule the listing builds entry paths with.
fn wire_join<'a>(components: impl Iterator<Item = Component<'a>>) -> String {
    components
        .filter_map(|component| match component {
            Component::Normal(name) => Some(name.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

/// The delete's own file: the vouching stays here (it is the door every
/// act walks through), the removal roads and their platform anchoring go
/// with the act, and the delete's tests are its children.
#[path = "workspace_file_delete.rs"]
mod delete;

#[cfg(test)]
#[path = "workspace_file_mutations_tests.rs"]
mod tests;
