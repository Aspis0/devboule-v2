//! What is stored outside the OS credential store: the metadata of each saved
//! login, and the one file it all lives in.
//!
//! No password is here. This document is what the Settings list renders and
//! what the fill path reads to decide which entry a field belongs to, so it
//! may be read freely; the password is only ever in the OS store, keyed by
//! this record's id.

use std::fs;
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The file name inside the app's local data directory.
const FILE_NAME: &str = "saved-logins.json";

/// One saved login as the list shows it. The password is not a field here and
/// cannot become one: this record is what the app writes to disk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedLogin {
    /// Opaque and random: it names the OS store item and nothing else, so an
    /// agent that ever saw one could not ask a site for it by name.
    pub id: String,
    pub label: String,
    /// Exact origins, each canonical, as [`super::origin`] spells them.
    pub origins: Vec<String>,
    pub username: String,
}

/// The metadata of every saved login, in the one file the app owns.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Book {
    #[serde(default)]
    pub logins: Vec<SavedLogin>,
}

impl Book {
    pub fn find(&self, id: &str) -> Option<&SavedLogin> {
        self.logins.iter().find(|login| login.id == id)
    }
}

/// The path this app keeps the document at.
pub fn path(local_data_dir: &Path) -> PathBuf {
    local_data_dir.join(FILE_NAME)
}

/// The stored document, or an empty one when there is none yet. A read failure
/// is a failure, never an empty book: a corrupt file that read as "no saved
/// logins" would be overwritten by the next save.
pub fn read(local_data_dir: &Path) -> Result<Book, String> {
    let file = path(local_data_dir);
    let raw = match fs::read_to_string(&file) {
        Ok(raw) => raw,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Book::default()),
        Err(error) => return Err(format!("The saved logins could not be read: {error}.")),
    };
    serde_json::from_str(&raw)
        .map_err(|error| format!("The saved logins file is not readable: {error}."))
}

/// Replace the document with `book`, or leave the old one whole.
pub fn write(local_data_dir: &Path, book: &Book) -> Result<(), String> {
    let file = path(local_data_dir);
    let raw = serde_json::to_vec_pretty(book)
        .map_err(|error| format!("The saved logins could not be serialized: {error}."))?;
    fs::create_dir_all(local_data_dir)
        .map_err(|error| format!("The app data folder could not be created: {error}."))?;
    // Written beside the target and renamed over it, so a crash mid-write
    // leaves the previous book rather than half of one.
    let mut temp = tempfile::NamedTempFile::new_in(local_data_dir)
        .map_err(|error| format!("The saved logins could not be staged: {error}."))?;
    temp.write_all(&raw)
        .map_err(|error| format!("The saved logins could not be written: {error}."))?;
    temp.as_file()
        .sync_all()
        .map_err(|error| format!("The saved logins could not be flushed: {error}."))?;
    temp.persist(&file)
        .map_err(|error| format!("The saved logins could not be replaced: {}.", error.error))?;
    Ok(())
}
#[cfg(test)]
#[path = "metadata_tests.rs"]
mod tests;
