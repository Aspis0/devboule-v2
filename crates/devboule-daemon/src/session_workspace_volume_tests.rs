//! The mount rule over a described filesystem: each test lists the paths that
//! exist, so the outage is modelled with the root present and nothing on the
//! real disk decides the answer.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use super::{volume_is_present, Entry};

const ROOT_DEV: u64 = 1;
const DISK_DEV: u64 = 2;

#[derive(Default)]
struct Described {
    entries: HashMap<PathBuf, Entry>,
    unreadable: Vec<PathBuf>,
    empty: Vec<PathBuf>,
}

impl Described {
    /// The root filesystem with the directories every Unix box has.
    fn with(dirs: &[&str]) -> Self {
        let mut described = Self::default();
        described.dir("/", ROOT_DEV);
        for dir in dirs {
            described.dir(dir, ROOT_DEV);
        }
        described
    }

    fn dir(&mut self, path: &str, dev: u64) -> &mut Self {
        self.entries
            .insert(PathBuf::from(path), Entry { is_dir: true, dev });
        self
    }

    fn link(&mut self, path: &str) -> &mut Self {
        self.entries.insert(
            PathBuf::from(path),
            Entry {
                is_dir: false,
                dev: ROOT_DEV,
            },
        );
        self
    }

    fn empty_dir(&mut self, path: &str, dev: u64) -> &mut Self {
        self.dir(path, dev);
        self.empty.push(PathBuf::from(path));
        self
    }

    fn present(&self, missing: &str) -> bool {
        let stat = |path: &Path| {
            if self.unreadable.iter().any(|denied| denied == path) {
                return Err(io::Error::new(io::ErrorKind::PermissionDenied, "denied"));
            }
            self.entries
                .get(path)
                .copied()
                .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "absent"))
        };
        let looks_empty = |path: &Path| self.empty.iter().any(|empty| empty == path);
        volume_is_present(Path::new(missing), &stat, &looks_empty)
    }
}

#[test]
fn an_unplugged_volume_is_not_a_vanished_folder() {
    let described = Described::with(&["/Volumes", "/Users", "/Users/me"]);
    assert!(!described.present("/Volumes/WorkDisk/project"));
    assert!(!described.present("/Volumes/WorkDisk/project.worktrees/feature"));
}

#[test]
fn a_deleted_folder_in_a_present_home_is_vanished() {
    let described = Described::with(&["/Users", "/Users/me", "/Users/me/code"]);
    assert!(described.present("/Users/me/code/old-project"));
    assert!(described.present("/Users/me/code/old-project/sub/dir"));
}

#[test]
fn a_deleted_folder_on_a_mounted_volume_is_vanished() {
    let mut described = Described::with(&["/Volumes"]);
    described.dir("/Volumes/WorkDisk", DISK_DEV);
    assert!(described.present("/Volumes/WorkDisk/project"));
}

#[test]
fn an_empty_leftover_mount_point_is_an_unmounted_volume() {
    let mut described = Described::with(&["/Volumes"]);
    described.empty_dir("/Volumes/WorkDisk", ROOT_DEV);
    assert!(!described.present("/Volumes/WorkDisk/project"));
}

#[test]
fn an_empty_but_mounted_volume_still_judges_a_missing_folder_gone() {
    let mut described = Described::with(&["/Volumes"]);
    described.empty_dir("/Volumes/Fresh", DISK_DEV);
    assert!(described.present("/Volumes/Fresh/project"));
}

#[test]
fn a_plain_folder_with_content_under_a_mount_parent_is_a_real_folder() {
    let mut described = Described::with(&["/mnt"]);
    described.dir("/mnt/projects", ROOT_DEV);
    assert!(described.present("/mnt/projects/old-app"));
}

#[test]
fn a_dropped_network_share_is_not_a_vanished_folder() {
    let described = Described::with(&["/mnt"]);
    assert!(!described.present("/mnt/share/project"));
}

#[test]
fn a_per_user_media_layout_needs_the_disk_mounted_under_the_user_folder() {
    let mut described = Described::with(&["/run", "/run/media"]);
    described.dir("/run/media/alice", ROOT_DEV);
    assert!(!described.present("/run/media/alice/Disk/project"));

    described.dir("/run/media/alice/Disk", DISK_DEV);
    assert!(described.present("/run/media/alice/Disk/project"));
}

#[test]
fn a_disk_mounted_directly_under_media_is_the_mount_point() {
    let mut described = Described::with(&["/media"]);
    described.dir("/media/Disk", DISK_DEV);
    assert!(described.present("/media/Disk/project"));
    assert!(!described.present("/media/Other/project"));
}

#[test]
fn a_root_that_is_a_link_is_never_followed_into_the_missing_part() {
    let mut described = Described::with(&["/Users", "/Users/me", "/Volumes"]);
    described.link("/Users/me/work");
    assert!(
        !described.present("/Users/me/work/project"),
        "a link to an unplugged disk must refuse"
    );
}

#[test]
fn what_cannot_be_read_or_placed_is_never_vanished() {
    let mut described = Described::with(&["/Users", "/Users/me"]);
    described.unreadable.push(PathBuf::from("/Users/me"));
    assert!(!described.present("/Users/me/project"), "denied ancestor");
    assert!(!described.present("relative/project"), "relative path");
    assert!(
        !Described::default().present("/Users/me/project"),
        "no existing ancestor at all"
    );
    let mut file = Described::with(&["/Users"]);
    file.entries.insert(
        PathBuf::from("/Users/me"),
        Entry {
            is_dir: false,
            dev: ROOT_DEV,
        },
    );
    assert!(!file.present("/Users/me/project"), "a file as the ancestor");
}
