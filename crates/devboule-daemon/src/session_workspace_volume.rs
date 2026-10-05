//! Whether the volume a missing workspace path lived on is still mounted.
//!
//! `NotFound` under a mount-parent directory (`/Volumes`, `/mnt`, `/media`)
//! is usually an unplugged disk or a dropped share, not a deleted folder, and
//! a delete must not detach the row for the first. The rule is conservative:
//! the folder counts as vanished only when its mount point is provably there,
//! so an outage can never read as a deletion. Unix only; Windows derives the
//! drive or share root instead.

use std::io;
use std::path::{Path, PathBuf};

/// The two facts the rule reads from a path's own metadata (no link
/// followed): whether it is a directory, and which device holds it.
#[derive(Clone, Copy)]
pub(super) struct Entry {
    pub(super) is_dir: bool,
    pub(super) dev: u64,
}

impl Entry {
    pub(super) fn of(metadata: &std::fs::Metadata) -> Self {
        use std::os::unix::fs::MetadataExt;
        Self {
            is_dir: metadata.is_dir(),
            dev: metadata.dev(),
        }
    }
}

type Stat<'a> = &'a dyn Fn(&Path) -> io::Result<Entry>;
type LooksEmpty<'a> = &'a dyn Fn(&Path) -> bool;

/// Directories whose children are mount points.
const MOUNT_ROOTS: [&str; 4] = ["/Volumes", "/mnt", "/media", "/run/media"];

/// Of those, the ones that distros also fill with per-user folders
/// (`/media/<user>/<disk>`), where the mount point is one level deeper.
const USER_LAYOUT_ROOTS: [&str; 2] = ["/media", "/run/media"];

/// `true` only when `missing` (a path that answered `NotFound`) provably sat
/// on a volume that is mounted right now, so its absence is a deletion.
///
/// Every doubt answers `false`: a relative path, no readable ancestor, a
/// link or a file where a directory must be, a mount point that is missing,
/// or one that is an empty directory on the same device as its parent (the
/// leftover of an unmounted volume).
pub(super) fn volume_is_present(
    missing: &Path,
    stat: Stat<'_>,
    looks_empty: LooksEmpty<'_>,
) -> bool {
    if !missing.is_absolute() {
        return false;
    }
    // A link is never followed into the missing part: a link that exists is
    // not a directory here, so a root that is a link to an unplugged disk
    // refuses instead of reading as a deleted folder.
    if !nearest_existing_ancestor(missing, stat).is_some_and(|ancestor| ancestor.is_dir) {
        return false;
    }
    mount_points(missing, stat)
        .iter()
        .all(|point| is_mounted_or_populated(point, stat, looks_empty))
}

fn nearest_existing_ancestor(missing: &Path, stat: Stat<'_>) -> Option<Entry> {
    for ancestor in missing.ancestors().skip(1) {
        match stat(ancestor) {
            Ok(entry) => return Some(entry),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(_) => return None,
        }
    }
    None
}

/// The mount points a path would sit on, by where it lives; empty when it is
/// not under any mount-parent directory.
fn mount_points(path: &Path, stat: Stat<'_>) -> Vec<PathBuf> {
    for root in MOUNT_ROOTS {
        let Ok(rest) = path.strip_prefix(root) else {
            continue;
        };
        let mut parts = rest.components();
        let Some(first) = parts.next() else {
            return Vec::new();
        };
        let child = Path::new(root).join(first);
        let mut points = vec![child.clone()];
        if USER_LAYOUT_ROOTS.contains(&root) && is_user_folder(&child, Path::new(root), stat) {
            if let Some(second) = parts.next() {
                points.push(child.join(second));
            }
        }
        return points;
    }
    Vec::new()
}

/// A child of `/media` that lives on the same device as `/media` is a plain
/// per-user folder, not itself a mount.
fn is_user_folder(child: &Path, root: &Path, stat: Stat<'_>) -> bool {
    match (stat(child), stat(root)) {
        (Ok(child), Ok(root)) => child.is_dir && child.dev == root.dev,
        _ => false,
    }
}

/// A mount point is there when it is a directory on another device than its
/// parent (a mounted filesystem), or a directory with content: an empty one
/// on the parent's own device is an unmounted volume's leftover.
fn is_mounted_or_populated(point: &Path, stat: Stat<'_>, looks_empty: LooksEmpty<'_>) -> bool {
    let Ok(entry) = stat(point) else {
        return false;
    };
    if !entry.is_dir {
        return false;
    }
    let Some(parent) = point.parent().map(stat) else {
        return false;
    };
    match parent {
        Ok(parent) if parent.dev != entry.dev => true,
        Ok(_) => !looks_empty(point),
        Err(_) => false,
    }
}

#[cfg(test)]
#[path = "session_workspace_volume_tests.rs"]
mod tests;
