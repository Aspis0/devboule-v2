//! The Chrome for Testing this app may run, pinned.
//!
//! The version and the digests below are the ones a human reviewed and wrote
//! into this file. The app never fetches a digest to compare an archive with:
//! Google's availability JSON is an unauthenticated response, and a digest
//! read from the same unauthenticated response as the archive it describes
//! checks only that the download arrived intact, never that it is the browser
//! it claims to be. So the digests live here, in the repository, and a change
//! to them is a reviewable diff.
//!
//! Only a pinned latest Stable entry is ever used, and nothing here checks for
//! a newer one: an update policy is a later slice's decision, not this file's.
//!
//! Adding a version means: download both archives by hand from the
//! [availability JSON](https://googlechromelabs.github.io/chrome-for-testing/last-known-good-versions-with-downloads.json),
//! compute each digest with two independent tools, and write both into [`PINNED`].

#![cfg_attr(not(test), allow(dead_code))]

use std::path::{Path, PathBuf};

/// A CfT build for one of the platforms this app can run it on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Platform {
    Win64,
    MacArm64,
}

impl Platform {
    /// The platform this build is running on, if it is one of the two with a
    /// pinned build. Anything else has no Chromium to install and says so.
    pub fn here() -> Result<Self, &'static str> {
        match (std::env::consts::OS, std::env::consts::ARCH) {
            ("windows", "x86_64") => Ok(Platform::Win64),
            ("macos", "aarch64") => Ok(Platform::MacArm64),
            (os, arch) => Err(match (os, arch) {
                ("macos", _) => "macOS Intel has no pinned Chrome for Testing build.",
                ("windows", _) => "32-bit Windows has no pinned Chrome for Testing build.",
                _ => "this platform has no pinned Chrome for Testing build.",
            }),
        }
    }

    /// The zip's single top directory. The install keeps the entries under it
    /// and drops the prefix, so this names what an install skips, not what it
    /// stores.
    pub fn archive_root(self) -> &'static str {
        match self {
            Platform::Win64 => "chrome-win64",
            Platform::MacArm64 => "chrome-mac-arm64",
        }
    }
}

/// One reviewed archive: where it came from and what it must hash to.
#[derive(Clone, Copy)]
pub struct Archive {
    pub url: &'static str,
    pub sha256: &'static str,
}

/// The pin. Version, and per platform the exact archive and the exact digest.
#[derive(Clone, Copy)]
pub struct Pin {
    pub version: &'static str,
    pub win64: Archive,
    pub mac_arm64: Archive,
}

/// Chrome for Testing Stable 154.0.8037.92.
///
/// Verified 2026-10-05: both zips downloaded from the availability JSON above
/// into `C:\tmp\cft` (outside the repo) and hashed with `certutil -hashfile
/// SHA256`, agreeing with the previous coder's `Get-FileHash` values written
/// here. Promote a new version by repeating that, never by copying a digest
/// out of a download.
pub const PINNED: Pin = Pin {
    version: "154.0.8037.92",
    win64: Archive {
        url: "https://storage.googleapis.com/chrome-for-testing-public/154.0.8037.92/win64/chrome-win64.zip",
        sha256: "b897ef3601c947ac0620c784556dec719ac602b0159ce105927acf645ee0f598",
    },
    mac_arm64: Archive {
        url: "https://storage.googleapis.com/chrome-for-testing-public/154.0.8037.92/mac-arm64/chrome-mac-arm64.zip",
        sha256: "b62e904b6571c5ff5108ed7812cf93ac6d1c4027f10ae47ac34d8e229ed88001",
    },
};

impl Pin {
    /// The archive for one platform.
    pub fn archive(self, platform: Platform) -> Archive {
        match platform {
            Platform::Win64 => self.win64,
            Platform::MacArm64 => self.mac_arm64,
        }
    }

    /// Where an install of this version lives under an app-data root:
    /// `<root>/chrome-for-testing/<version>`.
    pub fn install_dir(self, app_data: &Path) -> PathBuf {
        app_data.join("chrome-for-testing").join(self.version)
    }

    /// The file whose presence says this directory was verified before it was
    /// put where it is. Its contents are the digest of the archive it came
    /// from, so a directory from another version cannot be mistaken for this
    /// one's.
    pub fn marker(self, platform: Platform) -> &'static str {
        match platform {
            Platform::Win64 => "devboule-cft-verified-win64.sha256",
            Platform::MacArm64 => "devboule-cft-verified-mac-arm64.sha256",
        }
    }
}

/// The browser binary's path below an install root. Relative to the version
/// directory itself, because the unpack drops the zip's top directory: what
/// was `chrome-win64/chrome.exe` in the archive is `chrome.exe` on disk.
pub struct Executable;

impl Executable {
    pub fn of(platform: Platform) -> PathBuf {
        match platform {
            Platform::Win64 => PathBuf::from("chrome.exe"),
            Platform::MacArm64 => PathBuf::from("Google Chrome for Testing.app")
                .join("Contents")
                .join("MacOS")
                .join("Google Chrome for Testing"),
        }
    }
}

#[cfg(test)]
#[path = "cft_manifest_tests.rs"]
mod tests;
