//! Put the pinned Chrome for Testing where the process manager can launch it.
//!
//! The order is the whole of this module: fetch, hash, compare, and only then
//! unpack. A digest that does not match the pin deletes the download and stops,
//! so an archive from anywhere but the reviewed URL never reaches a disk the
//! app will execute from. The unpack lands in a staging directory beside its
//! destination and is renamed into place in one step, so an install that dies
//! half way leaves the working version that was already there.

#![cfg_attr(not(test), allow(dead_code))]

use std::fs::File;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use sha2::{Digest, Sha256};

use super::cft_fetch::ArchiveSource;
use super::cft_lock::ResourceLock;
use super::cft_macos;
use super::cft_manifest::{Executable, Pin, Platform};
use super::cft_unpack::unpack;

/// Why an install did not happen, or did not count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallError {
    /// The archive did not hash to the pin. The download has been deleted.
    DigestMismatch { expected: String, found: String },
    /// The download, the unpack, or the rename failed. The text is the OS's own.
    Transfer(String),
    /// The archive carried an entry that would land outside the install.
    UnsafeEntry(String),
    /// The macOS signature or quarantine step refused the bundle.
    Untrusted(String),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallError::DigestMismatch { expected, found } => write!(
                f,
                "the downloaded browser is not the one that was reviewed: it \
                 hashes to {found} and the pin says {expected}"
            ),
            InstallError::Transfer(text) => write!(f, "{text}"),
            InstallError::UnsafeEntry(name) => write!(
                f,
                "the archive carries an entry that would land outside the \
                 install: {name}"
            ),
            InstallError::Untrusted(text) => {
                write!(f, "the macOS signature check refused the browser: {text}")
            }
        }
    }
}

/// A browser this app installed and verified, and the version directory it is
/// in. Constructed only by [`install`] and [`install_blocking`], so holding
/// one means the bytes behind it were hashed against the pin.
#[derive(Clone, Debug)]
pub struct Installed {
    pub version: &'static str,
    pub root: PathBuf,
}

impl Installed {
    /// The browser executable, which the process manager launches.
    pub fn executable(&self, platform: Platform) -> PathBuf {
        self.root.join(Executable::of(platform))
    }
}

/// Install the pinned build under `app_data`, from `source`.
///
/// Blocking work throughout: a 200 MB download, a hash over it and an unpack.
/// Runs on a blocking worker so the app's own runtime is not held up. Takes
/// the source by value because the worker is `'static`: a reference would
/// not live long enough to cross into it.
pub async fn install<S>(
    pin: Pin,
    platform: Platform,
    app_data: &Path,
    source: S,
) -> Result<Installed, InstallError>
where
    S: ArchiveSource + Send + Sync + 'static,
{
    // Pin is Copy, PathBuf is owned: move both into the worker.
    let app_data = app_data.to_owned();
    tauri::async_runtime::spawn_blocking(move || {
        install_blocking(pin, platform, &app_data, &source)
    })
    .await
    .map_err(|error| InstallError::Transfer(format!("the install worker ended: {error}")))?
}

/// The install itself, off the runtime. Split out so the blocking body is one
/// function a synchronous test can call without a worker of its own.
///
/// One installer per app and version at a time: the install lock serializes
/// concurrent installs, the download lands in a unique temp file, and every
/// temp is removed on every path out — including a failed hash, which is why
/// a partial download can never be unzipped by a later call.
pub fn install_blocking(
    pin: Pin,
    platform: Platform,
    app_data: &Path,
    source: &dyn ArchiveSource,
) -> Result<Installed, InstallError> {
    let archive_meta = pin.archive(platform);
    let home = app_data.join("chrome-for-testing");
    let fresh_home = !home.exists();
    std::fs::create_dir_all(&home)
        .map_err(|error| InstallError::Transfer(format!("{}: {error}", home.display())))?;
    restrict(&home, fresh_home);
    let _install_lock = ResourceLock::acquire(&home.join(format!(".install-{}.lock", pin.version)))
        .map_err(InstallError::Transfer)?;

    let destination = pin.install_dir(app_data);
    // A crash between the two renames of a replacement leaves the
    // destination missing and the old tree under `.previous`: put it back
    // before doing anything else, so a new install that then fails still
    // leaves the working version behind.
    restore_previous(&home, pin.version, &destination)?;

    let temp = home.join(format!(
        "{}.zip.{}.{}.part",
        pin.version,
        std::process::id(),
        TEMP_COUNTER.fetch_add(1, Ordering::SeqCst)
    ));
    let _temp_guard = RemoveOnDrop::file(&temp);
    source
        .fetch(archive_meta.url, &temp)
        .map_err(InstallError::Transfer)?;

    // Read, compare, and only then open: nothing downstream of this line sees
    // an archive that has not matched.
    let found = sha256_of(&temp).map_err(|error| InstallError::Transfer(error.to_string()))?;
    if !found.eq_ignore_ascii_case(archive_meta.sha256) {
        return Err(InstallError::DigestMismatch {
            expected: archive_meta.sha256.to_owned(),
            found,
        });
    }

    let staging = home.join(format!("{}.staging", pin.version));
    let _staging_guard = RemoveOnDrop::dir(&staging);
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)
        .map_err(|error| InstallError::Transfer(format!("{}: {error}", staging.display())))?;
    unpack(&temp, platform.archive_root(), &staging)?;

    // Apple's gate runs on the staged copy: a bundle this app does not trust
    // never reaches the directory the process manager launches out of. Off
    // macOS this is a no-op (SHA-256 alone is the trust).
    cft_macos::trust(&staging).map_err(InstallError::Untrusted)?;

    std::fs::write(
        staging.join(pin.marker(platform)),
        format!("{}\n", archive_meta.sha256),
    )
    .map_err(|error| InstallError::Transfer(format!("the marker: {error}")))?;

    swap_in(&staging, &destination, &home, pin.version)?;

    Ok(Installed {
        version: pin.version,
        root: destination,
    })
}

/// Directory modes this app owns, where the platform has modes. Best effort:
/// a staging tree the app cannot read is one it must not execute from, but a
/// chmod failure on a foreign filesystem must not fail an install either.
fn restrict(home: &Path, fresh: bool) {
    #[cfg(unix)]
    if fresh {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(home, std::fs::Permissions::from_mode(0o700));
    }
    #[cfg(not(unix))]
    let _ = (home, fresh);
}

/// Suffixes that make one download's temp file unique to it.
static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A temp file or directory removed when it goes out of scope, so every
/// error path below cleans up without a remove call of its own. After a
/// successful rename the path is gone and the removal is a silent no-op.
struct RemoveOnDrop {
    path: PathBuf,
    dir: bool,
}

impl RemoveOnDrop {
    fn file(path: &Path) -> Self {
        RemoveOnDrop {
            path: path.to_owned(),
            dir: false,
        }
    }

    fn dir(path: &Path) -> Self {
        RemoveOnDrop {
            path: path.to_owned(),
            dir: true,
        }
    }
}

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        if self.dir {
            let _ = std::fs::remove_dir_all(&self.path);
        } else {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Put back the previous install when a crashed replacement left the
/// destination missing. Returns whether anything was restored; a restore that
/// itself fails is reported, never discarded.
fn restore_previous(home: &Path, version: &str, destination: &Path) -> Result<bool, InstallError> {
    if destination.exists() {
        return Ok(false);
    }
    let backup = home.join(format!("{version}.previous"));
    if !backup.exists() {
        return Ok(false);
    }
    restore_previous_from(&backup, destination)?;
    Ok(true)
}

fn restore_previous_from(backup: &Path, destination: &Path) -> Result<(), InstallError> {
    std::fs::rename(backup, destination).map_err(|error| {
        InstallError::Transfer(format!("restoring the previous browser install: {error}"))
    })
}

/// Move the verified staging directory into place, keeping the working version
/// until the new one is ready. The new one is fully verified by now (hash,
/// unpack, trust, marker); the swap is the only moment the old one is
/// touched, and a failed swap puts the old one back.
fn swap_in(
    staging: &Path,
    destination: &Path,
    home: &Path,
    version: &str,
) -> Result<(), InstallError> {
    if !destination.exists() {
        std::fs::rename(staging, destination).map_err(|error| {
            InstallError::Transfer(format!("{}: {error}", destination.display()))
        })?;
        return Ok(());
    }
    let backup = home.join(format!("{version}.previous"));
    let _ = std::fs::remove_dir_all(&backup);
    std::fs::rename(destination, &backup)
        .map_err(|error| InstallError::Transfer(format!("{}: {error}", destination.display())))?;
    match std::fs::rename(staging, destination) {
        Ok(()) => {
            let _ = std::fs::remove_dir_all(&backup);
            Ok(())
        }
        Err(error) => {
            // The old tree is owed back its place; a restore that fails too
            // is reported with both errors, never discarded.
            if let Err(restore) = std::fs::rename(&backup, destination) {
                return Err(InstallError::Transfer(format!(
                    "{}: {error}; restoring the previous install then failed: {restore}",
                    destination.display()
                )));
            }
            Err(InstallError::Transfer(format!(
                "{}: {error}",
                destination.display()
            )))
        }
    }
}

/// The archive already installed and marked, if it is this version's.
///
/// The marker is a cache hint, not proof of the bytes: it records that a
/// verified install happened here and says nothing about who replaced the
/// tree since. The hint is worth taking only while the expected executable
/// is still there — an archive that no longer has its binary is not an
/// install, whatever the marker says.
pub fn installed(pin: Pin, platform: Platform, app_data: &Path) -> Option<Installed> {
    let root = pin.install_dir(app_data);
    let marker = root.join(pin.marker(platform));
    let marked = std::fs::read_to_string(marker).ok()?;
    if !marked
        .trim()
        .eq_ignore_ascii_case(pin.archive(platform).sha256)
    {
        return None;
    }
    // The executable sits inside the mac bundle, so one check covers both:
    // no binary, no install.
    if !root.join(Executable::of(platform)).is_file() {
        return None;
    }
    Some(Installed {
        version: pin.version,
        root,
    })
}

/// The SHA-256 of a file, read in blocks so a 200 MB archive is never in
/// memory at once.
fn sha256_of(path: &Path) -> Result<String, std::io::Error> {
    let mut file = File::open(path)?;
    let mut hash = Sha256::new();
    let mut block = vec![0u8; 64 * 1024];
    loop {
        let read = file.read(&mut block)?;
        if read == 0 {
            break;
        }
        hash.update(&block[..read]);
    }
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

#[cfg(test)]
#[path = "cft_install_tests.rs"]
mod tests;
