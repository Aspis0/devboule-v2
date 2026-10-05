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

/// Create a directory (parents included) that only its owner can enter.
/// Re-applied every time: a re-created directory keeps whatever mode it
/// already had.
pub fn ensure_private_dir(path: &Path) -> io::Result<()> {
    std::fs::create_dir_all(path)?;
    narrow_to_owner(path, 0o700)
}

/// Set `mode` on a sensitive path this user owns. Refuses links — chmod
/// would narrow the target while the name keeps pointing at it — and paths
/// owned by anyone else, naming the rule that fired.
pub fn narrow_to_owner(path: &Path, mode: u32) -> io::Result<()> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "refusing to narrow a symlink, not its target: {}",
                path.display()
            ),
        ));
    }
    use std::os::unix::fs::MetadataExt;
    if metadata.uid() != current_uid() {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            format!(
                "refusing to narrow {} owned by uid {}, running as {}",
                path.display(),
                metadata.uid(),
                current_uid()
            ),
        ));
    }
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
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

    fn temp() -> std::path::PathBuf {
        crate::test_dirs::test_temp_dir("devboule-modes")
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
        std::fs::create_dir(&dir).expect("dir");
        let path = dir.join("secret.bin");
        let mut file = create_private_file(&path).expect("create");
        file.write_all(b"x").expect("write");
        drop(file);
        assert_eq!(mode_of(&path), 0o600);
    }

    #[test]
    fn create_refuses_a_name_that_is_taken() {
        let dir = temp();
        std::fs::create_dir(&dir).expect("dir");
        let path = dir.join("taken.bin");
        create_private_file(&path).expect("first");
        let error = create_private_file(&path).expect_err("second");
        assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    }

    #[test]
    fn created_dir_is_owner_only() {
        let dir = temp().join("nested").join("private");
        ensure_private_dir(&dir).expect("create");
        assert_eq!(mode_of(&dir), 0o700);
    }

    #[test]
    fn narrow_sets_the_mode_on_an_owned_file() {
        let dir = temp();
        std::fs::create_dir(&dir).expect("dir");
        let path = dir.join("wide.txt");
        std::fs::write(&path, b"wide").expect("write");
        narrow_to_owner(&path, 0o600).expect("narrow");
        assert_eq!(mode_of(&path), 0o600);
    }

    #[test]
    fn narrow_refuses_a_symlink_and_leaves_the_target() {
        let dir = temp();
        std::fs::create_dir(&dir).expect("dir");
        let target = dir.join("target.txt");
        std::fs::write(&target, b"target").expect("write");
        let before = mode_of(&target);
        let link = dir.join("link.txt");
        std::os::unix::fs::symlink(&target, &link).expect("link");
        let error = narrow_to_owner(&link, 0o600).expect_err("link refused");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(mode_of(&target), before, "target untouched");
    }
}
