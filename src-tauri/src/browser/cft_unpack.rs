//! Safe ZIP extraction for a reviewed-but-untrusted archive.
//!
//! One phrase: every byte lands inside the staging root. Entry names are
//! checked by `enclosed_name`, and symlink targets — which `enclosed_name`
//! never sees — are resolved lexically against the staging root and rejected
//! when absolute or escaping. Links are created last: regular files are
//! written before any link exists, and the staging directory is freshly
//! recreated under the install lock, so no write ever passes through a link.

use std::fs::File;
use std::path::{Component, Path, PathBuf};

use super::cft_install::InstallError;

/// A symlink entry held back for the second pass.
struct PendingLink {
    path: PathBuf,
    link_to: String,
}

/// Unpack `archive` into `into`, keeping only the entries under `keep`.
pub(super) fn unpack(archive: &Path, keep: &str, into: &Path) -> Result<(), InstallError> {
    let file = File::open(archive).map_err(|error| InstallError::Transfer(format!("{error}")))?;
    let mut zip = zip::ZipArchive::new(file)
        .map_err(|error| InstallError::Transfer(format!("{archive:?}: {error}")))?;
    let mut links = Vec::new();
    for index in 0..zip.len() {
        let mut entry = zip
            .by_index(index)
            .map_err(|error| InstallError::Transfer(format!("{archive:?}: {error}")))?;
        let Some(relative) = entry.enclosed_name() else {
            // zip 9 decodes the name on demand and `enclosed_name` gives up
            // when that fails (non-UTF-8, non-CP437 bytes), so the message
            // falls back to the raw bytes: an undecodable name is refused
            // exactly like an unsafe one.
            let name = match entry.name() {
                Ok(name) => name.into_owned(),
                Err(_) => String::from_utf8_lossy(entry.name_raw()).into_owned(),
            };
            return Err(InstallError::UnsafeEntry(name));
        };
        let Ok(under) = relative.strip_prefix(keep) else {
            continue;
        };
        if under.as_os_str().is_empty() {
            continue;
        }
        let target = into.join(under);
        if entry.is_dir() {
            std::fs::create_dir_all(&target)
                .map_err(|error| InstallError::Transfer(format!("{target:?}: {error}")))?;
            continue;
        }
        if entry.is_symlink() {
            let mut link_to = String::new();
            std::io::Read::read_to_string(&mut entry, &mut link_to)
                .map_err(|error| InstallError::Transfer(format!("{target:?}: {error}")))?;
            links.push(PendingLink {
                path: target,
                link_to,
            });
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| InstallError::Transfer(format!("{parent:?}: {error}")))?;
        }
        let mut out = File::create(&target)
            .map_err(|error| InstallError::Transfer(format!("{target:?}: {error}")))?;
        std::io::copy(&mut entry, &mut out)
            .map_err(|error| InstallError::Transfer(format!("{target:?}: {error}")))?;
        // Zip creation drops the exec bit; the mac binaries need it back to
        // launch. Only the bit is restored, never a full mode.
        #[cfg(unix)]
        if entry.unix_mode().is_some_and(|mode| mode & 0o111 != 0) {
            use std::os::unix::fs::PermissionsExt as _;
            let permissions = std::fs::Permissions::from_mode(0o755);
            let _ = std::fs::set_permissions(&target, permissions);
        }
    }
    for link in &links {
        if !link_stays_inside(into, &link.path, &link.link_to) {
            return Err(InstallError::UnsafeEntry(link.path.display().to_string()));
        }
        // A link nested under another link's path would be created by walking
        // through that link, which is a write the extractor promised never
        // happens. No reviewed archive nests one; a name that does is refused.
        if links.iter().any(|other| {
            other.path != link.path
                && link
                    .path
                    .parent()
                    .is_some_and(|parent| parent.starts_with(&other.path))
        }) {
            return Err(InstallError::UnsafeEntry(link.path.display().to_string()));
        }
        if let Some(parent) = link.path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| InstallError::Transfer(format!("{parent:?}: {error}")))?;
        }
        create_link(&link.path, &link.link_to)?;
    }
    Ok(())
}

/// Whether a symlink target stays inside the staging root, resolved lexically
/// from the link's own directory. Absolute targets and climbs past the root
/// are refused; everything else — including the framework `Versions/Current`
/// style relative links the real mac archive carries — is a path under root.
fn link_stays_inside(root: &Path, link_path: &Path, target: &str) -> bool {
    let target = Path::new(target);
    if target.is_absolute() {
        return false;
    }
    let link_dir = link_path.parent().unwrap_or(root);
    let Ok(relative) = link_dir.strip_prefix(root) else {
        return false;
    };
    let mut depth = relative.components().count();
    for component in target.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => return false,
            Component::CurDir => {}
            Component::ParentDir => {
                if depth == 0 {
                    return false;
                }
                depth -= 1;
            }
            Component::Normal(_) => {
                depth += 1;
            }
        }
    }
    true
}

/// Recreate one validated link. Unix links cover files and directories alike;
/// Windows needs the kind up front, so the resolved target decides.
fn create_link(path: &Path, link_to: &str) -> Result<(), InstallError> {
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(link_to, path)
            .map_err(|error| InstallError::Transfer(format!("{path:?}: {error}")))
    }
    #[cfg(not(unix))]
    {
        // Windows stores a link target verbatim and its path resolver does
        // not take forward slashes there (measured: WinError 123 on open), so
        // an archive's link is re-created with backslashes. It stays relative:
        // an absolute target would point into the staging directory that is
        // about to be renamed.
        let target = link_to.replace('/', "\\");
        let resolved = path.parent().unwrap_or(Path::new(".")).join(&target);
        if resolved.is_dir() {
            std::os::windows::fs::symlink_dir(&target, path)
        } else {
            std::os::windows::fs::symlink_file(&target, path)
        }
        .map_err(|error| InstallError::Transfer(format!("{path:?}: {error}")))
    }
}

#[cfg(test)]
#[path = "cft_unpack_tests.rs"]
mod tests;
