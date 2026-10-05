//! What the extractor promises directly: bytes stay under the staging root,
//! a link keeps its type and its target, and no path is written through a
//! link. The real mac-arm64 framework links are the shape these fixtures use.

use std::io::Write;
use std::path::PathBuf;

use super::super::cft_install::InstallError;

/// One archive on disk plus a fresh staging directory to unpack it into.
/// `_dir` owns both and lives as long as the fixture.
struct Fixture {
    _dir: tempfile::TempDir,
    archive: PathBuf,
    staging: PathBuf,
}

impl Fixture {
    fn with_entries(files: &[(&str, &[u8])], links: &[(&str, &str)]) -> Self {
        let dir = tempfile::tempdir().expect("a scratch dir");
        let archive = dir.path().join("chrome-win64.zip");
        let mut writer =
            zip::ZipWriter::new(std::fs::File::create(&archive).expect("the archive file"));
        for (name, body) in files {
            writer
                .start_file(*name, zip::write::SimpleFileOptions::default())
                .expect("the entry starts");
            writer.write_all(body).expect("the entry writes");
        }
        for (name, target) in links {
            writer
                .add_symlink(*name, *target, zip::write::SimpleFileOptions::default())
                .expect("the link is added");
        }
        writer.finish().expect("the zip finishes");
        let staging = dir.path().join("staging");
        Fixture {
            _dir: dir,
            archive,
            staging,
        }
    }

    fn unpack(&self) -> Result<(), InstallError> {
        std::fs::create_dir_all(&self.staging).expect("the staging dir");
        super::unpack(&self.archive, "chrome-win64", &self.staging)
    }

    fn at(&self, relative: &str) -> PathBuf {
        self.staging.join(relative)
    }
}

#[test]
fn an_entry_outside_the_keep_prefix_is_refused() {
    let fixture = Fixture::with_entries(&[("../evil.exe", b"escape")], &[]);
    let refused = fixture.unpack();
    assert!(
        matches!(refused, Err(InstallError::UnsafeEntry(_))),
        "a climbing entry must refuse: {refused:?}"
    );
}

#[test]
fn an_entry_for_another_platform_is_skipped() {
    let fixture = Fixture::with_entries(
        &[
            ("chrome-mac-arm64/chrome.bin", b"mac"),
            ("chrome-win64/chrome.exe", b"win"),
        ],
        &[],
    );
    fixture.unpack().expect("the win64 archive unpacks");
    assert!(fixture.at("chrome.exe").is_file());
    assert!(!fixture.staging.join("chrome-mac-arm64").exists());
}

#[test]
fn a_relative_link_is_recreated_and_resolves() {
    let fixture = Fixture::with_entries(
        &[("chrome-win64/Frameworks/Versions/A/blob.bin", b"blob")],
        &[("chrome-win64/Frameworks/Current", "Versions/A")],
    );
    fixture.unpack().expect("a relative link installs");
    let link = fixture.at("Frameworks/Current");
    assert!(
        std::fs::symlink_metadata(&link)
            .expect("the link is on disk")
            .file_type()
            .is_symlink(),
        "the archive's link must stay a link"
    );
    assert_eq!(
        std::fs::read(link.join("blob.bin")).expect("the link resolves"),
        b"blob"
    );
}

#[test]
fn an_absolute_link_target_is_refused() {
    let fixture = Fixture::with_entries(
        &[("chrome-win64/chrome.exe", b"browser")],
        &[("chrome-win64/Frameworks/Current", "/etc/passwd")],
    );
    let refused = fixture.unpack();
    assert!(
        matches!(refused, Err(InstallError::UnsafeEntry(_))),
        "an absolute link must refuse: {refused:?}"
    );
}

#[test]
fn an_escaping_link_target_is_refused() {
    let fixture = Fixture::with_entries(
        &[("chrome-win64/chrome.exe", b"browser")],
        &[("chrome-win64/Frameworks/Current", "../../outside")],
    );
    let refused = fixture.unpack();
    assert!(
        matches!(refused, Err(InstallError::UnsafeEntry(_))),
        "a climbing link must refuse: {refused:?}"
    );
}

#[test]
fn a_link_inside_another_links_path_is_refused() {
    let fixture = Fixture::with_entries(
        &[],
        &[
            ("chrome-win64/Frameworks", "Versions"),
            ("chrome-win64/Frameworks/Current", "A"),
        ],
    );
    let refused = fixture.unpack();
    assert!(
        matches!(refused, Err(InstallError::UnsafeEntry(_))),
        "a link created through another link must refuse: {refused:?}"
    );
}

#[cfg(unix)]
#[test]
fn an_executable_entry_comes_back_executable() {
    use std::os::unix::fs::PermissionsExt as _;

    let dir = tempfile::tempdir().expect("a scratch dir");
    let archive = dir.path().join("chrome-win64.zip");
    let mut writer =
        zip::ZipWriter::new(std::fs::File::create(&archive).expect("the archive file"));
    writer
        .start_file(
            "chrome-win64/chrome",
            zip::write::SimpleFileOptions::default().unix_permissions(0o755),
        )
        .expect("the entry starts");
    writer.write_all(b"bin").expect("the entry writes");
    writer.finish().expect("the zip finishes");
    let staging = dir.path().join("staging");
    std::fs::create_dir_all(&staging).expect("the staging dir");
    super::unpack(&archive, "chrome-win64", &staging).expect("the archive unpacks");
    let mode = std::fs::metadata(staging.join("chrome"))
        .expect("the binary")
        .permissions()
        .mode();
    assert_ne!(mode & 0o111, 0, "an executable entry must stay executable");
}
