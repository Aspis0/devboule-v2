//! Read a JSON config file's bytes before any store parses them.
//!
//! One responsibility: decide what the bytes on disk mean. A leading UTF-8
//! BOM (Windows Notepad) is stripped; a missing file is absence, an existing
//! but 0-byte/whitespace-only file is blank — the caller decides whether
//! blank is a first run or damage, because for access config it is damage.
//! Anything else is returned for the caller to parse. Oversize files, UTF-16
//! files and I/O failures stay errors with reasons that name the file.

use std::io;
use std::path::Path;

/// The three bytes Windows Notepad prepends to a UTF-8 file.
const UTF8_BOM: &[u8; 3] = b"\xef\xbb\xbf";

/// What the file at `path` turned out to be, before any store parses it.
///
/// `Absent` is a missing file — the first run. `Blank` is a file that exists
/// but carries no JSON (0 bytes, whitespace only, or a lone BOM): for most
/// stores that is also a first run, but the tool policy decides access, so
/// blank there is an unknown policy and fails closed. `Present` holds the
/// BOM-stripped bytes to parse.
#[derive(Debug)]
pub(crate) enum ConfigFile {
    Absent,
    Blank,
    Present(Vec<u8>),
}

/// Strip one leading UTF-8 BOM, if present. Only one: a file starting with
/// two BOMs keeps the second, and the parse then refuses it as damage.
pub(crate) fn strip_utf8_bom(bytes: &[u8]) -> &[u8] {
    bytes.strip_prefix(UTF8_BOM.as_slice()).unwrap_or(bytes)
}

/// Whether `bytes` carry no JSON after the BOM is stripped: empty or
/// whitespace only. Shared by the file reader and the device loader so the
/// two cannot disagree on what "blank" means.
pub(crate) fn is_blank(bytes: &[u8]) -> bool {
    strip_utf8_bom(bytes)
        .iter()
        .all(|byte| byte.is_ascii_whitespace())
}

/// Read `path` for a JSON store capped at `max_bytes`.
///
/// `Absent` is a missing file. `Blank` is an existing file with no JSON in
/// it; the file is left alone. `Present` holds the BOM-stripped bytes.
///
/// The cap is checked against the metadata first (so an oversized file is
/// refused without being read) and again against the bytes actually read, so
/// a file grown between the two cannot smuggle an unbounded read in. A file
/// starting with a UTF-16 BOM is refused as UTF-16, never decoded: the daemon
/// reads UTF-8 only, and guessing an encoding would invent a policy.
pub(crate) fn read_config_file(path: &Path, max_bytes: u64) -> io::Result<ConfigFile> {
    let metadata = match std::fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(ConfigFile::Absent),
        Err(error) => return Err(error),
    };
    if metadata.len() > max_bytes {
        return Err(too_large(path, metadata.len(), max_bytes));
    }
    let bytes = std::fs::read(path)?;
    if bytes.len() as u64 > max_bytes {
        return Err(too_large(path, bytes.len() as u64, max_bytes));
    }
    if is_blank(&bytes) {
        return Ok(ConfigFile::Blank);
    }
    if has_utf16_bom(&bytes) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!(
                "{} is UTF-16 encoded; save it as UTF-8 (without a BOM, or with a UTF-8 BOM) to load it",
                path.display()
            ),
        ));
    }
    Ok(ConfigFile::Present(strip_utf8_bom(&bytes).to_vec()))
}

/// Whether `bytes` start with a UTF-16 BOM, little- or big-endian: the two
/// encodings older Windows editors and PowerShell redirections produce.
fn has_utf16_bom(bytes: &[u8]) -> bool {
    bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF])
}

fn too_large(path: &Path, len: u64, max_bytes: u64) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!(
            "{path} is {len} bytes, over the {max_bytes}-byte cap",
            path = path.display()
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_single_leading_bom_is_stripped_and_a_second_is_kept() {
        assert_eq!(strip_utf8_bom(b"\xef\xbb\xbf{}"), b"{}");
        assert_eq!(strip_utf8_bom(b"{}"), b"{}");
        assert_eq!(
            strip_utf8_bom(b"\xef\xbb\xbf\xef\xbb\xbf{}"),
            b"\xef\xbb\xbf{}"
        );
    }

    #[test]
    fn blank_means_empty_or_whitespace_with_or_without_a_bom() {
        assert!(is_blank(b""));
        assert!(is_blank(b"  \r\n\t "));
        assert!(is_blank(b"\xef\xbb\xbf"));
        assert!(is_blank(b"\xef\xbb\xbf  \n "));
        assert!(!is_blank(b"{}"));
        assert!(!is_blank(b"\xef\xbb\xbf{}"));
    }

    #[test]
    fn a_missing_file_is_absent_and_a_blank_file_is_blank() {
        let dir = crate::test_dirs::test_temp_dir("devboule-config-read");
        let missing = dir.join("missing.json");
        assert!(
            matches!(
                read_config_file(&missing, 1024).expect("missing"),
                ConfigFile::Absent
            ),
            "a missing file is a first run"
        );

        for (tag, bytes) in [
            ("empty", b"".as_slice()),
            ("blank", b"  \n ".as_slice()),
            ("bom-only", b"\xef\xbb\xbf".as_slice()),
        ] {
            let path = dir.join(format!("{tag}.json"));
            std::fs::write(&path, bytes).expect("seed");
            assert!(
                matches!(
                    read_config_file(&path, 1024).expect("read"),
                    ConfigFile::Blank
                ),
                "{tag}: an existing file with no JSON is blank, not absent"
            );
            assert!(path.is_file(), "{tag}: a blank file is left where it is");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_bom_prefixed_document_is_returned_stripped() {
        let dir = crate::test_dirs::test_temp_dir("devboule-config-read-bom");
        let path = dir.join("doc.json");
        std::fs::write(&path, b"\xef\xbb\xbf{\"a\":1}").expect("seed");
        match read_config_file(&path, 1024).expect("present") {
            ConfigFile::Present(bytes) => assert_eq!(bytes, b"{\"a\":1}"),
            other => panic!(
                "a BOM document is present, got {}",
                config_file_name(&other)
            ),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn config_file_name(file: &ConfigFile) -> &'static str {
        match file {
            ConfigFile::Absent => "absent",
            ConfigFile::Blank => "blank",
            ConfigFile::Present(_) => "present",
        }
    }

    #[test]
    fn an_oversized_file_is_an_error_not_absence() {
        let dir = crate::test_dirs::test_temp_dir("devboule-config-read-cap");
        let path = dir.join("big.json");
        std::fs::write(&path, vec![b' '; 11]).expect("seed");
        let error = read_config_file(&path, 10).expect_err("over the cap");
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        assert!(path.is_file(), "a refused file is left where it is");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_utf16_file_is_refused_as_utf16_never_decoded() {
        let dir = crate::test_dirs::test_temp_dir("devboule-config-read-utf16");
        for (tag, bom) in [("le", [0xFF, 0xFE]), ("be", [0xFE, 0xFF])] {
            let path = dir.join(format!("{tag}.json"));
            let mut bytes = bom.to_vec();
            bytes.extend(b"{}");
            std::fs::write(&path, &bytes).expect("seed");
            let error = read_config_file(&path, 1024).expect_err("{tag}: UTF-16 refuses");
            assert!(
                error.to_string().contains("UTF-16"),
                "{tag}: the reason names the encoding: {error}"
            );
            assert!(path.is_file(), "{tag}: the file is kept");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
