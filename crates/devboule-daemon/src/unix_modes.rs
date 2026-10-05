//! Owner-only paths on Unix: the mode-bits half of what `security.rs` does
//! with DACLs on Windows. Narrow at creation (a mode never requested cannot
//! be broadened by the umask); narrow existing sensitive paths only for
//! their owner, never through a link.

use std::fs::File;
use std::fs::OpenOptions;
use std::io;
use std::path::Path;

/// Create a file no other account can read. `create_new` refuses a name
/// that is already taken — including a planted link — so the open never
/// follows one.
pub fn create_private_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    use std::os::unix::fs::OpenOptionsExt;
    options.mode(0o600);
    options.open(path)
}

/// Create a directory (parents included) that only its owner can enter,
/// refusing a link, a non-directory, or another owner's dir instead of
/// chmodding it: narrowing a foreign or redirected path would alter
/// permissions on a tree this daemon must not touch.
pub fn ensure_private_dir(path: &Path) -> io::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("refusing a symlinked dir: {}", path.display()),
            ));
        }
        Ok(metadata) if !metadata.is_dir() => {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("private dir is not a directory: {}", path.display()),
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            std::fs::create_dir_all(path)?;
        }
        Err(error) => return Err(error),
    }
    // Re-verified inside: a link swapped in after the check above still
    // fails the descriptor open, and foreign owners are refused there.
    narrow_to_owner(path, 0o700)
}

/// Set `mode` on a sensitive path this user owns. Opens the path with
/// `O_NOFOLLOW` and checks/chmods the descriptor, never the name twice:
/// a link swapped in between cannot redirect the chmod, and refusing
/// covers links, other owners, and missing files alike.
pub fn narrow_to_owner(path: &Path, mode: u32) -> io::Result<()> {
    let file = open_no_follow(path)?;
    narrow_open_file(&file, mode)
}

/// `narrow_to_owner` for a descriptor the caller already holds (a lock
/// file just taken, a temp just created): the inode is pinned, so there
/// is no name left to race on.
pub fn narrow_open_file(file: &File, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::MetadataExt;
    let uid = fstat_uid(file)?;
    if uid != current_uid() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "refusing to narrow a file owned by uid {uid}, running as {}",
                current_uid()
            ),
        ));
    }
    use std::os::unix::io::AsRawFd;
    // SAFETY: fchmod takes the open descriptor and a mode; nothing else.
    if unsafe { libc::fchmod(file.as_raw_fd(), mode as libc::mode_t) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Open for metadata/chmod only, refusing to follow a trailing link.
fn open_no_follow(path: &Path) -> io::Result<File> {
    use std::ffi::CString;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::io::FromRawFd;
    let name = CString::new(path.as_os_str().as_bytes()).map_err(|_| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("path holds an interior NUL: {}", path.display()),
        )
    })?;
    // SAFETY: O_RDONLY opens for metadata only; O_NOFOLLOW makes a trailing
    // link fail instead of resolving; O_NONBLOCK keeps a FIFO swapped in for
    // the path from blocking the open. On success the fd is ours to own.
    let fd = unsafe {
        libc::open(
            name.as_ptr(),
            libc::O_RDONLY | libc::O_NOFOLLOW | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_fd(fd) })
}

/// Owner of the open description just opened: what `fchmod` below acts on.
fn fstat_uid(file: &File) -> io::Result<u32> {
    use std::os::unix::io::AsRawFd;
    let mut status: libc::stat = unsafe { std::mem::zeroed() };
    // SAFETY: fstat fills a live struct of exactly this type.
    if unsafe { libc::fstat(file.as_raw_fd(), &mut status) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(status.st_uid)
}

fn current_uid() -> u32 {
    // SAFETY: getuid takes no arguments and cannot fail.
    unsafe { libc::getuid() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::os::unix::fs::PermissionsExt;

    fn temp() -> TempDir {
        TempDir(crate::test_dirs::test_temp_dir("devboule-modes"))
    }

    /// `test_temp_dir` names are unique per call but never cleaned: drop
    /// removes the whole tree so repeated runs do not accumulate.
    struct TempDir(std::path::PathBuf);

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn mode_of(path: &Path) -> u32 {
        std::fs::metadata(path)
            .expect("metadata")
            .permissions()
            .mode()
            & 0o777
    }

    #[test]
    fn created_file_is_owner_only() {
        let dir = temp();
        let path = dir.0.join("secret.bin");
        let mut file = create_private_file(&path).expect("create");
        file.write_all(b"x").expect("write");
        drop(file);
        assert_eq!(mode_of(&path), 0o600);
    }

    #[test]
    fn create_refuses_a_name_that_is_taken() {
        let dir = temp();
        let path = dir.0.join("taken.bin");
        create_private_file(&path).expect("first");
        let error = create_private_file(&path).expect_err("second");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn created_dir_is_owner_only() {
        let base = temp();
        let dir = base.0.join("nested").join("private");
        ensure_private_dir(&dir).expect("create");
        assert_eq!(mode_of(&dir), 0o700);
    }

    #[test]
    fn narrow_sets_the_mode_on_an_owned_file() {
        let dir = temp();
        let path = dir.0.join("wide.txt");
        std::fs::write(&path, b"wide").expect("write");
        narrow_to_owner(&path, 0o600).expect("narrow");
        assert_eq!(mode_of(&path), 0o600);
    }

    #[test]
    fn narrow_refuses_a_symlink_and_leaves_the_target() {
        let dir = temp();
        let target = dir.0.join("target.txt");
        std::fs::write(&target, b"target").expect("write");
        let before = mode_of(&target);
        let link = dir.0.join("link.txt");
        std::os::unix::fs::symlink(&target, &link).expect("link");
        let error = narrow_to_owner(&link, 0o600).expect_err("link refused");
        // O_NOFOLLOW fails the open itself: nothing was ever chmodded.
        assert_eq!(error.kind(), io::ErrorKind::FilesystemLoop);
        assert_eq!(mode_of(&target), before, "target untouched");
    }
}
