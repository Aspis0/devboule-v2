//! What the install proves without the internet: hash before unzip, safe
//! paths, markers, and the old version surviving a bad new one.

use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use super::super::cft_fetch::Https;
use super::super::cft_manifest::{Archive, Pin, Platform};
use super::*;

const TEST_VERSION: &str = "9.9.9-test";

/// A source that writes bytes already on disk, so no test needs the network.
struct Bytes {
    body: Vec<u8>,
}

impl ArchiveSource for Bytes {
    fn fetch(&self, _url: &str, into: &Path) -> Result<(), String> {
        std::fs::write(into, &self.body).map_err(|error| error.to_string())
    }
}

/// A source that copies an archive already on disk, so an ignored test can
/// use the pinned zip CI downloaded without fetching it twice.
struct FileAt(PathBuf);

impl ArchiveSource for FileAt {
    fn fetch(&self, _url: &str, into: &Path) -> Result<(), String> {
        std::fs::copy(&self.0, into)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

fn test_zip(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, body) in files {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .expect("a test entry starts");
        writer.write_all(body).expect("a test entry writes");
    }
    writer.finish().expect("the test zip finishes").into_inner()
}

fn digest_of(body: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(body);
    hash.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// A pin whose URL never leaves the test: leaked once, because the manifest
/// holds `&'static str` and a loopback port is only known at runtime.
fn test_pin(url: &str, sha256: &str) -> Pin {
    let url: &'static str = Box::leak(url.to_owned().into_boxed_str());
    let sha256: &'static str = Box::leak(sha256.to_owned().into_boxed_str());
    Pin {
        version: TEST_VERSION,
        win64: Archive { url, sha256 },
        mac_arm64: Archive { url, sha256 },
    }
}

fn win_platform() -> Platform {
    Platform::Win64
}

#[test]
fn a_verified_archive_unpacks_and_marks() {
    let app_data = tempfile::tempdir().expect("an app-data dir");
    let body = test_zip(&[("chrome-win64/chrome.exe", b"browser")]);
    let pin = test_pin(
        "https://example.invalid/chrome-win64.zip",
        &digest_of(&body),
    );
    // Through the async wrapper, so the blocking worker path is proved too.
    let done = tauri::async_runtime::block_on(install(
        pin,
        win_platform(),
        app_data.path(),
        Bytes { body },
    ))
    .expect("the reviewed archive installs");
    assert_eq!(done.version, TEST_VERSION);
    assert!(done.executable(win_platform()).is_file());
    assert!(done.root.join(pin.marker(win_platform())).is_file());
    assert!(installed(pin, win_platform(), app_data.path()).is_some());
}

#[test]
fn a_digest_mismatch_deletes_the_download_and_installs_nothing() {
    let app_data = tempfile::tempdir().expect("an app-data dir");
    let body = test_zip(&[("chrome-win64/chrome.exe", b"browser")]);
    let pin = test_pin("https://example.invalid/chrome-win64.zip", &"0".repeat(64));
    let refused = install_blocking(pin, win_platform(), app_data.path(), &Bytes { body });
    let Err(InstallError::DigestMismatch { .. }) = refused else {
        panic!("a wrong digest must refuse: {refused:?}");
    };
    let home = app_data.path().join("chrome-for-testing");
    assert!(
        part_files(&home).is_empty(),
        "no partial download may be left for a later call"
    );
    assert!(!pin.install_dir(app_data.path()).exists());
}

/// Temp downloads, whatever suffix the run gave them.
fn part_files(home: &Path) -> Vec<std::path::PathBuf> {
    std::fs::read_dir(home)
        .map(|entries| {
            entries
                .filter_map(|entry| entry.ok().map(|entry| entry.path()))
                .filter(|path| path.extension().is_some_and(|ext| ext == "part"))
                .collect()
        })
        .unwrap_or_default()
}

#[test]
fn a_marker_from_another_digest_is_not_an_install() {
    let app_data = tempfile::tempdir().expect("an app-data dir");
    let body = test_zip(&[("chrome-win64/chrome.exe", b"browser")]);
    let pin = test_pin(
        "https://example.invalid/chrome-win64.zip",
        &digest_of(&body),
    );
    install_blocking(pin, win_platform(), app_data.path(), &Bytes { body })
        .expect("the reviewed archive installs");
    std::fs::write(
        pin.install_dir(app_data.path())
            .join(pin.marker(win_platform())),
        format!("{}\n", "1".repeat(64)),
    )
    .expect("the marker is tampered");
    assert!(installed(pin, win_platform(), app_data.path()).is_none());
}

#[test]
fn a_failed_replacement_keeps_the_working_version() {
    let app_data = tempfile::tempdir().expect("an app-data dir");
    let good = test_zip(&[("chrome-win64/chrome.exe", b"old-browser")]);
    let pin = test_pin(
        "https://example.invalid/chrome-win64.zip",
        &digest_of(&good),
    );
    install_blocking(pin, win_platform(), app_data.path(), &Bytes { body: good })
        .expect("the first version installs");
    let exe = pin.install_dir(app_data.path()).join("chrome.exe");
    assert_eq!(std::fs::read(&exe).expect("the old exe"), b"old-browser");

    let bad = test_zip(&[("chrome-win64/chrome.exe", b"new-browser")]);
    let bad_pin = test_pin("https://example.invalid/chrome-win64.zip", &"2".repeat(64));
    let refused = install_blocking(
        bad_pin,
        win_platform(),
        app_data.path(),
        &Bytes { body: bad },
    );
    assert!(matches!(refused, Err(InstallError::DigestMismatch { .. })));
    assert_eq!(
        std::fs::read(&exe).expect("the old exe survives"),
        b"old-browser"
    );
}

/// The download path itself, against loopback: a tiny HTTP server serves the
/// zip, and the real [`Https`] client fetches it. No internet is touched.
#[test]
fn the_https_client_downloads_from_loopback() {
    let body = test_zip(&[("chrome-win64/chrome.exe", b"loopback-browser")]);
    let digest = digest_of(&body);
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    let port = listener.local_addr().expect("the port").port();
    let server = std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut request = [0u8; 4096];
            let _ = std::io::Read::read(&mut stream, &mut request);
            let header = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(header.as_bytes());
            let _ = stream.write_all(&body);
        }
    });
    let url = format!("http://127.0.0.1:{port}/chrome-win64.zip");
    let pin = test_pin(&url, &digest);
    let app_data = tempfile::tempdir().expect("an app-data dir");
    install_blocking(pin, win_platform(), app_data.path(), &Https)
        .expect("loopback downloads install");
    assert!(pin
        .install_dir(app_data.path())
        .join("chrome.exe")
        .is_file());
    let _ = server.join();
}

/// The real bucket, by hand only: fetches the pinned win64 zip, hashes it,
/// and checks it against the manifest. Needs the network and ~200 MB.
#[test]
#[ignore = "needs the network: DEVBOULE_CFT_DOWNLOAD to run"]
fn the_pinned_win64_archive_still_hashes_to_the_manifest() {
    let root = tempfile::tempdir().expect("a scratch dir");
    let into = root.path().join("chrome-win64.zip");
    Https
        .fetch(super::super::cft_manifest::PINNED.win64.url, &into)
        .expect("the pinned archive downloads");
    let found = super::sha256_of(&into).expect("the download hashes");
    assert_eq!(found, super::super::cft_manifest::PINNED.win64.sha256);
}

#[test]
fn a_crashed_swap_is_repaired_before_a_failed_install() {
    let app_data = tempfile::tempdir().expect("an app-data dir");
    let good = test_zip(&[("chrome-win64/chrome.exe", b"old-browser")]);
    let pin = test_pin(
        "https://example.invalid/chrome-win64.zip",
        &digest_of(&good),
    );
    install_blocking(pin, win_platform(), app_data.path(), &Bytes { body: good })
        .expect("the first version installs");
    // The crash window: destination gone, old tree under `.previous`.
    let home = app_data.path().join("chrome-for-testing");
    let destination = pin.install_dir(app_data.path());
    std::fs::rename(&destination, home.join(format!("{TEST_VERSION}.previous")))
        .expect("the crash is staged");
    assert!(!destination.exists());

    let bad = test_zip(&[("chrome-win64/chrome.exe", b"new-browser")]);
    let bad_pin = test_pin("https://example.invalid/chrome-win64.zip", &"3".repeat(64));
    let refused = install_blocking(
        bad_pin,
        win_platform(),
        app_data.path(),
        &Bytes { body: bad },
    );
    assert!(matches!(refused, Err(InstallError::DigestMismatch { .. })));
    let exe = destination.join("chrome.exe");
    assert_eq!(
        std::fs::read(&exe).expect("the old tree is back"),
        b"old-browser"
    );
    assert!(installed(pin, win_platform(), app_data.path()).is_some());
}

#[test]
fn a_failed_restore_is_reported_never_discarded() {
    let dir = tempfile::tempdir().expect("a scratch dir");
    let missing = dir.path().join("no-such-backup");
    let destination = dir.path().join("no-such-destination");
    let refused = super::restore_previous_from(&missing, &destination);
    let Err(InstallError::Transfer(text)) = refused else {
        panic!("a failed restore must report: {refused:?}");
    };
    assert!(
        text.contains("restoring the previous"),
        "the report must name the restore: {text}"
    );
}

#[test]
fn a_missing_executable_invalidates_the_marker_hint() {
    let app_data = tempfile::tempdir().expect("an app-data dir");
    let body = test_zip(&[("chrome-win64/chrome.exe", b"browser")]);
    let pin = test_pin(
        "https://example.invalid/chrome-win64.zip",
        &digest_of(&body),
    );
    install_blocking(pin, win_platform(), app_data.path(), &Bytes { body })
        .expect("the reviewed archive installs");
    assert!(installed(pin, win_platform(), app_data.path()).is_some());
    std::fs::remove_file(pin.install_dir(app_data.path()).join("chrome.exe"))
        .expect("the binary is tampered away");
    assert!(
        installed(pin, win_platform(), app_data.path()).is_none(),
        "a marker without its binary is only a hint"
    );
}

/// The pinned mac archive through the whole install: top-directory strip,
/// framework symlinks and exec bits. CI hands it the archive it already
/// downloaded and a destination under the runner's temp.
#[test]
#[ignore = "needs the pinned archive: DEVBOULE_CFT_ARCHIVE and DEVBOULE_CFT_APP_DATA"]
fn the_pinned_mac_archive_installs_with_its_framework_links() {
    let archive = std::env::var_os("DEVBOULE_CFT_ARCHIVE").expect("DEVBOULE_CFT_ARCHIVE");
    let app_data = std::env::var_os("DEVBOULE_CFT_APP_DATA").expect("DEVBOULE_CFT_APP_DATA");
    let app_data = PathBuf::from(app_data);
    let pin = super::super::cft_manifest::PINNED;
    let done = install_blocking(pin, Platform::MacArm64, &app_data, &FileAt(archive.into()))
        .expect("the pinned mac archive installs");
    assert_eq!(done.version, pin.version);
    assert!(done.executable(Platform::MacArm64).is_file());
    let framework = done
        .root
        .join("Google Chrome for Testing.app/Contents/Frameworks/Google Chrome for Testing Framework.framework");
    let link = framework.join("Versions/Current");
    assert!(
        std::fs::symlink_metadata(&link)
            .expect("the framework link is on disk")
            .file_type()
            .is_symlink(),
        "the archive's link must stay a link"
    );
    assert!(
        link.join("Google Chrome for Testing Framework").is_file(),
        "the link resolves to the framework's own binary"
    );
    assert!(installed(pin, Platform::MacArm64, &app_data).is_some());
}

/// A crash between the swap's two renames is repaired by `installed()`
/// itself: no new install runs, the old tree is back and counts.
#[test]
fn installed_repairs_a_crashed_swap_without_a_new_install() {
    let app_data = tempfile::tempdir().expect("an app-data dir");
    let good = test_zip(&[("chrome-win64/chrome.exe", b"old-browser")]);
    let pin = test_pin(
        "https://example.invalid/chrome-win64.zip",
        &digest_of(&good),
    );
    install_blocking(pin, win_platform(), app_data.path(), &Bytes { body: good })
        .expect("the first version installs");
    let home = app_data.path().join("chrome-for-testing");
    let destination = pin.install_dir(app_data.path());
    std::fs::rename(&destination, home.join(format!("{TEST_VERSION}.previous")))
        .expect("the crash is staged");
    assert!(!destination.exists());

    assert!(installed(pin, win_platform(), app_data.path()).is_some());
    assert_eq!(
        std::fs::read(destination.join("chrome.exe")).expect("the old tree is back"),
        b"old-browser"
    );
}
