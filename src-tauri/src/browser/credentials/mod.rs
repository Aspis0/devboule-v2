//! The saved logins of this machine: what a person saved, where each one's
//! password is, and the two reads the fill path asks for.
//!
//! The metadata says which exact origins an entry may be used on and which id
//! its password is filed under in the OS credential store; the two are kept
//! apart so that reading the list, which happens on every visit to Settings,
//! never reads a secret. Nothing the Tauri layer can reach hands a password
//! back: the one function that reads one is crate-internal, and the fill path
//! that will use it lands with slice V2.
//!
//! Every change is a read, a change to the credential store and a change to
//! one file, and the three of them are held together by [`TRANSACTION`]. Two
//! saves that overlap would otherwise each read the same book and each write
//! their own version of it: the file would hold one row and the store would
//! hold two passwords, one of which nothing can name.

pub(crate) mod commands;
mod metadata;
mod secrets;

// `Origin` is compared and logged by the fill path; until that path lands,
// only the tests read an origin beyond its canonical spelling.
#[cfg_attr(not(test), allow(dead_code))]
mod origin;

use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

use secrets::{Keyring, SecretStore};

use metadata::{Book, SavedLogin};

/// The longest name an entry carries, the longest username, the longest
/// password and the longest site address, in bytes.
///
/// These are read on the caller's thread before anything is handed to the
/// credential store or written to the folder, so one oversized field costs one
/// comparison rather than a copy of itself in two more places.
const MAX_LABEL: usize = 80;
const MAX_USERNAME_BYTES: usize = 256;
const MAX_PASSWORD_BYTES: usize = 1024;
const MAX_ORIGIN_BYTES: usize = 2048;
/// The most sites one entry may list, counted after two spellings of one site
/// have become one.
const MAX_ORIGINS: usize = 8;
/// The most site addresses one entry may be GIVEN, counted before anything is
/// read. Nine sites is refused on its own account; this is what refuses a long
/// list of near-duplicates that would otherwise be read one by one to arrive at
/// one site.
const MAX_SUBMITTED_ORIGINS: usize = 16;
/// How many times a fresh id is drawn before the vault gives up on one that
/// keeps colliding. It is drawn from 16 bytes of OS entropy, so a collision
/// means the entropy source is not the one this asked for.
const ID_ATTEMPTS: usize = 4;

/// The one lock every change in this process takes, held across the whole
/// read-modify-write and across any compensation for a half-finished one.
///
/// A poisoned lock is taken anyway: the save that panicked left the folder and
/// the store in some state, and refusing every later save would make that state
/// permanent. Every operation is written so that the state it leaves is one the
/// person can see and delete.
fn transaction() -> MutexGuard<'static, ()> {
    static TRANSACTION: OnceLock<Mutex<()>> = OnceLock::new();
    TRANSACTION
        .get_or_init(|| Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Why the vault refused: what the person typed, or what this machine's own
/// store did. The Settings form shows either sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    Asked(String),
    Failed(String),
}

impl Refusal {
    pub fn sentence(&self) -> &str {
        match self {
            Refusal::Asked(why) | Refusal::Failed(why) => why,
        }
    }
}

/// Whether a request is one this vault could carry, checked with no folder,
/// no store and no lock behind it.
///
/// The Tauri commands ask this before their work goes to a thread of its own,
/// so an oversized field is one comparison rather than a copy of itself across
/// a thread boundary and into a credential store. What passes here is checked
/// again by the change that follows: this is the early answer, not the one
/// that saves anything.
pub(crate) fn checked(
    label: &str,
    origins: &[String],
    username: &str,
    password: Option<&str>,
) -> Result<(), Refusal> {
    label_of(label)?;
    username_of(username)?;
    origins_of(origins)?;
    if let Some(password) = password {
        password_of(password)?;
    }
    Ok(())
}

/// Every saved login, over one directory's metadata and one secret store.
pub struct Vault {
    dir: PathBuf,
    secrets: Box<dyn SecretStore>,
}

impl Vault {
    /// The vault of this machine's own directory, over the OS credential store.
    pub fn in_dir(dir: PathBuf) -> Self {
        Vault::new(dir, Box::new(Keyring))
    }

    /// The vault over a store of the caller's choosing, which is how a test
    /// runs the whole vault without this machine's credential store.
    pub fn new(dir: PathBuf, secrets: Box<dyn SecretStore>) -> Self {
        Vault { dir, secrets }
    }

    /// Every entry, as the Settings list shows it. Metadata only.
    pub fn list(&self) -> Result<Vec<SavedLogin>, Refusal> {
        Ok(self.book()?.logins)
    }

    /// Save a login: its password into the OS store, its metadata into the app
    /// data file.
    pub fn create(
        &self,
        label: &str,
        origins: &[String],
        username: &str,
        password: &str,
    ) -> Result<SavedLogin, Refusal> {
        let login = SavedLogin {
            id: String::new(),
            label: label_of(label)?,
            origins: origins_of(origins)?,
            username: username_of(username)?,
        };
        let password = password_of(password)?;
        let _one_at_a_time = transaction();
        let mut book = self.book()?;
        let login = SavedLogin {
            id: fresh_id(&book)?,
            ..login
        };
        // The password first: a store that refuses leaves nothing listed at
        // all, where a listed entry whose password never landed would be a row
        // that says "saved" over nothing.
        self.secrets
            .set(&login.id, password)
            .map_err(Refusal::Failed)?;
        book.logins.push(login.clone());
        if let Err(failed) = metadata::write(&self.dir, &book) {
            return Err(self.unwind_create(login.id, failed));
        }
        Ok(login)
    }

    /// Change an entry. No password keeps the one already stored, which is
    /// what the Settings form's empty password field means.
    pub fn update(
        &self,
        id: &str,
        label: &str,
        origins: &[String],
        username: &str,
        password: Option<&str>,
    ) -> Result<SavedLogin, Refusal> {
        let login = SavedLogin {
            id: id.to_owned(),
            label: label_of(label)?,
            origins: origins_of(origins)?,
            username: username_of(username)?,
        };
        let password = password.map(password_of).transpose()?;
        let _one_at_a_time = transaction();
        let mut book = self.book()?;
        let at = index_of(&book, id)?;
        // The password that is in the store now, read before the new one goes
        // over it: without it there is nothing to put back, and a failed save
        // would leave a password no row has ever named.
        let replaced = match password {
            None => None,
            Some(password) => Some(self.replace_password(id, password)?),
        };
        book.logins[at] = login.clone();
        if let Err(failed) = metadata::write(&self.dir, &book) {
            return Err(self.unwind_update(id, replaced, failed));
        }
        Ok(login)
    }

    /// Forget an entry: its password out of the OS store first, then its row.
    ///
    /// That order is the one that leaves the least to explain. A store that
    /// refuses leaves the entry listed and still working; a metadata file that
    /// cannot be written leaves a row the person can delete again, which the
    /// answer says outright.
    pub fn delete(&self, id: &str) -> Result<(), Refusal> {
        let _one_at_a_time = transaction();
        let mut book = self.book()?;
        let at = index_of(&book, id)?;
        self.secrets.delete(id).map_err(Refusal::Failed)?;
        book.logins.remove(at);
        metadata::write(&self.dir, &book).map_err(|failed| {
            Refusal::Failed(format!(
                "{failed} The password is gone from the credential store; delete the entry again to clear it from the list."
            ))
        })
    }

    /// Write a new password over the one that is there, and hand back what it
    /// replaced so a failed save can put it back. `None` means the store held
    /// no password for this entry.
    fn replace_password(&self, id: &str, password: &str) -> Result<Option<String>, Refusal> {
        let previous = self.secrets.get(id).map_err(Refusal::Failed)?;
        self.secrets.set(id, password).map_err(Refusal::Failed)?;
        Ok(previous)
    }

    /// The answer to a save whose book could not be written: the password is
    /// taken back out of the store. If that fails, the secret is still in the
    /// machine under an id no list names, and the answer says so rather than
    /// reporting a failure that cleaned up after itself.
    fn unwind_create(&self, id: String, failed: String) -> Refusal {
        match self.secrets.delete(&id) {
            Ok(()) => Refusal::Failed(failed),
            Err(kept) => Refusal::Failed(format!(
                "{failed} The password just saved could not be removed from this \
                 machine's credential store, so it is still filed under {id} with no \
                 entry in the list: {kept}"
            )),
        }
    }

    /// The answer to a change whose book could not be written. The row is still
    /// the one it was, so the password the store holds has to be the one it
    /// was too — unless putting it back fails, which is said outright.
    fn unwind_update(&self, id: &str, replaced: Option<Option<String>>, failed: String) -> Refusal {
        let Some(was) = replaced else {
            return Refusal::Failed(failed);
        };
        let put_back = match was {
            Some(password) => self.secrets.set(id, &password),
            None => self.secrets.delete(id),
        };
        match put_back {
            Ok(()) => Refusal::Failed(failed),
            Err(kept) => Refusal::Failed(format!(
                "{failed} This machine's credential store now holds the new password for \
                 this login, and the one it held could not be put back ({kept}); changing \
                 this entry again with a password sets it to what you type next."
            )),
        }
    }

    /// The entries that may be used on `origin`, which a fill path asks for
    /// with the origin of the frame the field lives in. Metadata only.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn lookup_for_origin(
        &self,
        origin: &origin::Origin,
    ) -> Result<Vec<SavedLogin>, Refusal> {
        let wanted = origin.as_str();
        Ok(self
            .book()?
            .logins
            .into_iter()
            .filter(|login| login.origins.iter().any(|allowed| allowed == wanted))
            .collect())
    }

    /// The password of one entry, for the fill that types it. `None` when the
    /// store has no item under that id, which is a fact and not a failure.
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) fn password_for(&self, id: &str) -> Result<Option<String>, Refusal> {
        self.secrets.get(id).map_err(Refusal::Failed)
    }

    fn book(&self) -> Result<Book, Refusal> {
        metadata::read(&self.dir).map_err(Refusal::Failed)
    }
}

/// An id no entry of `book` already holds, drawn from the OS entropy source.
fn fresh_id(book: &Book) -> Result<String, Refusal> {
    for _ in 0..ID_ATTEMPTS {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).map_err(|error| {
            Refusal::Failed(format!(
                "This machine would not draw random bytes: {error}."
            ))
        })?;
        let id: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
        if book.find(&id).is_none() {
            return Ok(id);
        }
    }
    Err(Refusal::Failed(
        "This machine could not draw an unused saved-login id.".to_owned(),
    ))
}

fn index_of(book: &Book, id: &str) -> Result<usize, Refusal> {
    book.logins
        .iter()
        .position(|login| login.id == id)
        .ok_or_else(|| Refusal::Asked("That saved login is not in the list.".to_owned()))
}

fn label_of(label: &str) -> Result<String, Refusal> {
    let label = label.trim();
    if label.is_empty() {
        return Err(Refusal::Asked("Give this login a name.".to_owned()));
    }
    if label.len() > MAX_LABEL {
        return Err(Refusal::Asked(format!(
            "A name of {MAX_LABEL} bytes is the longest this app stores."
        )));
    }
    Ok(label.to_owned())
}

fn username_of(username: &str) -> Result<String, Refusal> {
    let username = username.trim();
    if username.len() > MAX_USERNAME_BYTES {
        return Err(Refusal::Asked(format!(
            "A username of {MAX_USERNAME_BYTES} bytes is the longest this app stores."
        )));
    }
    Ok(username.to_owned())
}

/// The sites one entry may be used on, each canonical, with two spellings of one
/// site collapsed before the count is taken: the cap is about sites a password
/// may go to, not about how many ways the person typed them.
fn origins_of(origins: &[String]) -> Result<Vec<String>, Refusal> {
    if origins.is_empty() {
        return Err(Refusal::Asked(
            "Name at least one site this login may be used on.".to_owned(),
        ));
    }
    if origins.len() > MAX_SUBMITTED_ORIGINS {
        return Err(Refusal::Asked(format!(
            "A saved login is given at most {MAX_SUBMITTED_ORIGINS} site addresses, and \
             keeps at most {MAX_ORIGINS} of them."
        )));
    }
    let mut canonical: Vec<String> = Vec::new();
    for asked in origins {
        if asked.len() > MAX_ORIGIN_BYTES {
            return Err(Refusal::Asked(format!(
                "A site address of {MAX_ORIGIN_BYTES} bytes is the longest this app reads."
            )));
        }
        let one = origin::canonical(asked)
            .map_err(Refusal::Asked)?
            .to_string();
        if !canonical.contains(&one) {
            canonical.push(one);
        }
    }
    if canonical.len() > MAX_ORIGINS {
        return Err(Refusal::Asked(format!(
            "A saved login lists at most {MAX_ORIGINS} sites."
        )));
    }
    Ok(canonical)
}

fn password_of(password: &str) -> Result<&str, Refusal> {
    if password.is_empty() {
        return Err(Refusal::Asked("Type a password to save.".to_owned()));
    }
    if password.len() > MAX_PASSWORD_BYTES {
        return Err(Refusal::Asked(format!(
            "A password of {MAX_PASSWORD_BYTES} bytes is the longest this app stores."
        )));
    }
    Ok(password)
}

#[cfg(test)]
#[path = "compensation_tests.rs"]
mod compensation_tests;
#[cfg(test)]
#[path = "concurrency_tests.rs"]
mod concurrency_tests;
#[cfg(test)]
#[path = "limits_tests.rs"]
mod limits_tests;
#[cfg(test)]
#[path = "vault_tests.rs"]
mod tests;
