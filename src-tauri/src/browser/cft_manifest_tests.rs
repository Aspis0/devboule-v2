//! What the pin promises: the reviewed version, addresses and digests.

use std::path::{Path, PathBuf};

use super::*;

#[test]
fn the_pin_is_the_reviewed_stable() {
    assert_eq!(PINNED.version, "154.0.8037.92");
}

#[test]
fn both_platforms_point_at_the_official_bucket() {
    for archive in [PINNED.win64, PINNED.mac_arm64] {
        assert!(
            archive.url.starts_with(
                "https://storage.googleapis.com/chrome-for-testing-public/154.0.8037.92/"
            ),
            "unexpected origin: {}",
            archive.url
        );
    }
    assert!(PINNED.win64.url.ends_with("/win64/chrome-win64.zip"));
    assert!(PINNED
        .mac_arm64
        .url
        .ends_with("/mac-arm64/chrome-mac-arm64.zip"));
}

#[test]
fn both_digests_are_sha256_hex() {
    for digest in [PINNED.win64.sha256, PINNED.mac_arm64.sha256] {
        assert_eq!(digest.len(), 64, "not a SHA-256: {digest}");
        assert!(
            digest.chars().all(|c| c.is_ascii_hexdigit()),
            "not hex: {digest}"
        );
    }
    assert_ne!(PINNED.win64.sha256, PINNED.mac_arm64.sha256);
}

#[test]
fn the_platform_selects_its_own_archive() {
    assert_eq!(PINNED.archive(Platform::Win64).sha256, PINNED.win64.sha256);
    assert_eq!(
        PINNED.archive(Platform::MacArm64).sha256,
        PINNED.mac_arm64.sha256
    );
}

#[test]
fn the_install_dir_is_versioned_under_app_data() {
    let root = Path::new("/data");
    assert_eq!(
        PINNED.install_dir(root),
        PathBuf::from("/data/chrome-for-testing/154.0.8037.92")
    );
}

#[test]
fn each_platform_has_its_own_marker() {
    assert_ne!(
        PINNED.marker(Platform::Win64),
        PINNED.marker(Platform::MacArm64)
    );
    assert!(PINNED.marker(Platform::Win64).ends_with(".sha256"));
}

#[test]
fn the_executable_is_relative_to_the_version_dir() {
    assert_eq!(Executable::of(Platform::Win64), PathBuf::from("chrome.exe"));
    assert_eq!(
        Executable::of(Platform::MacArm64),
        PathBuf::from("Google Chrome for Testing.app")
            .join("Contents")
            .join("MacOS")
            .join("Google Chrome for Testing")
    );
}

#[test]
fn this_machine_reports_its_platform() {
    // Windows x86_64 here; elsewhere the same call names the missing build.
    assert!(Platform::here().is_ok());
}
