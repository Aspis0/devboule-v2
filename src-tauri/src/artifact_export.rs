//! Writes the standalone artifact document to a path the user chose in the OS
//! save dialog.
//!
//! The path is never invented here and never defaulted: the dialog already
//! returned a concrete, user-chosen location, and this module only commits the
//! bytes it is handed. The frontend owns the decision of *what* document to
//! write (`artifactExport.ts` builds it once for both the clipboard and the
//! file), so the two commands cannot drift; the backend owns nothing but the
//! write.

use std::fs;
use std::path::Path;

use devboule_protocol::ErrorCode;

use crate::backend::error::CommandError;

/// A failed write names the operation and the path. A bare `os error 5` tells
/// the caller neither what failed nor where; on Windows the path is the only
/// part of the message a person can act on.
fn write_error(path: &Path, error: impl std::fmt::Display) -> CommandError {
    CommandError::new(
        ErrorCode::Io,
        format!("writing `{}` failed: {error}", path.display()),
    )
}

fn write_artifact_file_inner(path: &str, contents: &str) -> Result<String, CommandError> {
    if path.trim().is_empty() {
        return Err(CommandError::new(
            ErrorCode::InvalidRequest,
            "the artifact path is empty: the save dialog must return a concrete path.",
        ));
    }
    // `contents.as_bytes()` is the document's UTF-8 encoding. The clipboard
    // receives the same `String`, and UTF-8 is the only encoding either one
    // uses, so the file and the clipboard hold the same bytes by construction.
    fs::write(path, contents.as_bytes()).map_err(|error| write_error(Path::new(path), error))?;
    // Return what was written so the caller can show the location that was
    // actually committed instead of re-deriving it from the dialog's answer.
    Ok(path.to_string())
}

/// Writes `contents` to the user-chosen `path` as UTF-8 bytes and returns that
/// path. An empty path is rejected before it reaches the filesystem; any other
/// failure (missing parent directory, permissions, a directory at the target)
/// comes back as an `io` error naming the operation and the path.
#[tauri::command]
pub fn artifact_write_file(path: String, contents: String) -> Result<String, CommandError> {
    write_artifact_file_inner(&path, &contents)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_the_exact_utf8_bytes_and_returns_the_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("index.html");
        let contents = "<!DOCTYPE html>\n<html><body><h1>Città</h1></body></html>\n";

        let returned =
            write_artifact_file_inner(&path.to_string_lossy(), contents).expect("artifact write");

        assert_eq!(returned, path.to_string_lossy());
        let on_disk = fs::read(&path).expect("written file");
        assert_eq!(on_disk, contents.as_bytes());
    }

    #[test]
    fn replaces_an_existing_file_instead_of_appending() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("index.html");
        fs::write(&path, "the previous export").expect("seed file");

        write_artifact_file_inner(&path.to_string_lossy(), "the new export")
            .expect("artifact write");

        assert_eq!(
            fs::read_to_string(&path).expect("written file"),
            "the new export"
        );
    }

    #[test]
    fn rejects_an_empty_path_before_any_filesystem_call() {
        for path in ["", "   "] {
            let error = write_artifact_file_inner(path, "<html></html>").expect_err("empty path");
            assert_eq!(error.code, ErrorCode::InvalidRequest);
            assert!(
                error.message.contains("empty"),
                "the error must say the path is empty: {}",
                error.message
            );
        }
    }

    #[test]
    fn a_missing_parent_directory_names_the_operation_and_the_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("missing").join("index.html");

        let error =
            write_artifact_file_inner(&path.to_string_lossy(), "<html></html>").expect_err("write");

        assert_eq!(error.code, ErrorCode::Io);
        assert!(
            error.message.contains("writing"),
            "the error must name the operation: {}",
            error.message
        );
        assert!(
            error.message.contains(&path.to_string_lossy().to_string()),
            "the error must name the path: {}",
            error.message
        );
    }
}
