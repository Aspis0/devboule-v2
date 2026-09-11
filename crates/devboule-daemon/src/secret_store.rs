//! Where the Noise static private key lives.
//!
//! Two stores, both same-user protected: the OS credential store (Windows
//! Credential Manager / Keychain / Secret Service) and a file in the runtime
//! directory with the same current-user-only DACL the named pipe uses
//! (`DESIGN-remote-agents.md` §8 R5: same-user code execution is outside the
//! trust boundary). The file store exists because the integration tests and
//! CI cannot depend on an unlocked credential store, and because a headless
//! daemon must still be able to start.
//!
//! The store is selected once, at startup, and the choice is reported in
//! `Status.secret_store`: `keyring` or `file`.
//!
//! Wired by S4 (`status_body` and the daemon start-up path). Until then the
//! module has no non-test caller; remove this allowance in the step that
//! wires it.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::paths::RuntimePaths;

/// One service name for every devboule daemon of this user; the runtime-dir
/// hash in the username is what tells two daemons apart.
pub const KEYRING_SERVICE: &str = "devboule";
/// Environment override. `file` is the only honoured value; anything else
/// (including a typo or a stale `memory`) falls back to the normal probe.
pub const SECRET_STORE_ENV: &str = "DEVBOULE_SECRET_STORE";

#[derive(Debug)]
pub enum SecretStoreError {
    /// No platform entry for this name. `get` maps this to `Ok(None)`.
    NoEntry,
    PlatformFailure(String),
    NoStorageAccess(String),
    Unavailable(String),
}

impl std::fmt::Display for SecretStoreError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoEntry => write!(formatter, "no stored secret under that name"),
            Self::PlatformFailure(message) => {
                write!(formatter, "credential store failure: {message}")
            }
            Self::NoStorageAccess(message) => {
                write!(formatter, "credential store is not accessible: {message}")
            }
            Self::Unavailable(message) => write!(formatter, "secret store unavailable: {message}"),
        }
    }
}

impl std::error::Error for SecretStoreError {}

pub trait SecretStore: Send + Sync {
    /// `Ok(None)` means "no entry", which callers must treat as missing, not
    /// as an empty secret.
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>, SecretStoreError>;
    fn set(&self, name: &str, bytes: &[u8]) -> Result<(), SecretStoreError>;
    fn delete(&self, name: &str) -> Result<(), SecretStoreError>;
}

fn map_keyring(error: keyring::Error) -> SecretStoreError {
    match error {
        keyring::Error::NoEntry => SecretStoreError::NoEntry,
        keyring::Error::PlatformFailure(detail) => {
            SecretStoreError::PlatformFailure(detail.to_string())
        }
        keyring::Error::NoStorageAccess(detail) => {
            SecretStoreError::NoStorageAccess(detail.to_string())
        }
        other => SecretStoreError::Unavailable(other.to_string()),
    }
}

/// The OS credential store. The username carries the runtime-dir hash so two
/// daemons of one user do not share an entry; Windows Credential Manager
/// already scopes entries per user, so two users never do.
pub struct KeyringStore {
    suffix: String,
}

impl KeyringStore {
    pub fn new(runtime_dir: &Path) -> Self {
        Self {
            suffix: crate::paths::runtime_dir_hash(runtime_dir),
        }
    }

    pub fn username(&self, name: &str) -> String {
        format!("{name}-{}", self.suffix)
    }

    fn entry(&self, name: &str) -> Result<keyring::Entry, SecretStoreError> {
        keyring::Entry::new(KEYRING_SERVICE, &self.username(name)).map_err(map_keyring)
    }

    /// Probe the store by reading the entry this identity will use. A missing
    /// entry proves the store works; `PlatformFailure`/`NoStorageAccess`
    /// prove it does not, which is what selects the file store.
    pub fn available(runtime_dir: &Path) -> bool {
        let store = Self::new(runtime_dir);
        match store.entry(crate::device_identity::NOISE_STATIC_SECRET_NAME) {
            Ok(entry) => match entry.get_secret() {
                Ok(_) | Err(keyring::Error::NoEntry) => true,
                Err(_) => false,
            },
            Err(_) => false,
        }
    }
}

impl SecretStore for KeyringStore {
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>, SecretStoreError> {
        match self.entry(name)?.get_secret() {
            Ok(bytes) => Ok(Some(bytes)),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(map_keyring(error)),
        }
    }

    fn set(&self, name: &str, bytes: &[u8]) -> Result<(), SecretStoreError> {
        self.entry(name)?.set_secret(bytes).map_err(map_keyring)
    }

    fn delete(&self, name: &str) -> Result<(), SecretStoreError> {
        match self.entry(name)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(map_keyring(error)),
        }
    }
}

/// `<runtime dir>/secrets/<name>.bin`, created current-user-only. The name is
/// a fixed constant in this crate; the validation is here so a future caller
/// cannot turn it into a path.
pub struct FileStore {
    dir: PathBuf,
}

impl FileStore {
    pub fn new(runtime_dir: &Path) -> Self {
        Self {
            dir: runtime_dir.join("secrets"),
        }
    }

    pub fn path_for(&self, name: &str) -> Result<PathBuf, SecretStoreError> {
        if name.is_empty()
            || !name
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(SecretStoreError::Unavailable(format!(
                "secret name {name:?} is not a plain file name"
            )));
        }
        Ok(self.dir.join(format!("{name}.bin")))
    }
}

impl SecretStore for FileStore {
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>, SecretStoreError> {
        let path = self.path_for(name)?;
        match std::fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(SecretStoreError::Unavailable(error.to_string())),
        }
    }

    fn set(&self, name: &str, bytes: &[u8]) -> Result<(), SecretStoreError> {
        let path = self.path_for(name)?;
        std::fs::create_dir_all(&self.dir)
            .map_err(|error| SecretStoreError::Unavailable(error.to_string()))?;
        let mut file = open_private_file(&path)
            .map_err(|error| SecretStoreError::Unavailable(error.to_string()))?;
        use std::io::Write;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|error| SecretStoreError::Unavailable(error.to_string()))
    }

    fn delete(&self, name: &str) -> Result<(), SecretStoreError> {
        let path = self.path_for(name)?;
        match std::fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(SecretStoreError::Unavailable(error.to_string())),
        }
    }
}

#[cfg(windows)]
fn open_private_file(path: &Path) -> std::io::Result<std::fs::File> {
    crate::security::create_private_file(path)
}

#[cfg(not(windows))]
fn open_private_file(path: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretStoreKind {
    Keyring,
    File,
}

impl SecretStoreKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Keyring => "keyring",
            Self::File => "file",
        }
    }
}

/// Pick the store. `DEVBOULE_SECRET_STORE=file` forces the file store; the
/// default is the credential store when it initialises, the file store when
/// it does not. There is no third option and no in-memory store in a
/// production build: a key that vanishes at restart would be worse than no
/// remote listener.
pub fn select_secret_store(paths: &RuntimePaths) -> (Arc<dyn SecretStore>, SecretStoreKind) {
    let forced_file = std::env::var(SECRET_STORE_ENV)
        .map(|value| value.eq_ignore_ascii_case("file"))
        .unwrap_or(false);
    if !forced_file && KeyringStore::available(&paths.dir) {
        (
            Arc::new(KeyringStore::new(&paths.dir)),
            SecretStoreKind::Keyring,
        )
    } else {
        (Arc::new(FileStore::new(&paths.dir)), SecretStoreKind::File)
    }
}

/// Test-only store. It has no production switch: an in-memory key would be
/// lost at restart, and a daemon that reloaded with a new key would break
/// every peer that pinned the old one.
#[cfg(test)]
#[derive(Default)]
pub struct InMemoryStore {
    entries: std::sync::Mutex<std::collections::HashMap<String, Vec<u8>>>,
}

#[cfg(test)]
impl SecretStore for InMemoryStore {
    fn get(&self, name: &str) -> Result<Option<Vec<u8>>, SecretStoreError> {
        Ok(self
            .entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .get(name)
            .cloned())
    }

    fn set(&self, name: &str, bytes: &[u8]) -> Result<(), SecretStoreError> {
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .insert(name.to_string(), bytes.to_vec());
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<(), SecretStoreError> {
        self.entries
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .remove(name);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    const NAME: &str = "noise-static";

    fn tmp_dir() -> PathBuf {
        static COUNTER: AtomicU64 = AtomicU64::new(1);
        let dir = std::env::temp_dir().join(format!(
            "devboule secrets {}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    #[test]
    fn in_memory_round_trip_and_missing_entry() {
        let store = InMemoryStore::default();
        assert!(store.get(NAME).expect("get").is_none());
        store.set(NAME, b"0123456789").expect("set");
        assert_eq!(
            store.get(NAME).expect("get").as_deref(),
            Some(&b"0123456789"[..])
        );
        store.delete(NAME).expect("delete");
        assert!(store.get(NAME).expect("get").is_none());
        // Deleting twice is not an error.
        store.delete(NAME).expect("delete again");
    }

    #[test]
    fn file_store_round_trips_and_holds_the_envelope() {
        let dir = tmp_dir();
        let store = FileStore::new(&dir);
        assert!(store.get(NAME).expect("get").is_none());
        let bytes = crate::device_identity::encode_envelope(&[9u8; 32]);
        store.set(NAME, &bytes).expect("set");
        assert_eq!(
            store.get(NAME).expect("get").expect("entry").as_slice(),
            &bytes[..]
        );
        assert_eq!(
            store.path_for(NAME).expect("path"),
            dir.join("secrets").join("noise-static.bin")
        );
        store.delete(NAME).expect("delete");
        assert!(store.get(NAME).expect("get").is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_store_rejects_a_name_that_is_not_a_plain_file_name() {
        let dir = tmp_dir();
        let store = FileStore::new(&dir);
        assert!(store.path_for("../escape").is_err());
        assert!(store.path_for("").is_err());
        assert!(store.path_for("sub/dir").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(windows)]
    #[test]
    fn file_store_file_is_current_user_only() {
        let dir = tmp_dir();
        let store = FileStore::new(&dir);
        store.set(NAME, b"secret").expect("set");
        let path = store.path_for(NAME).expect("path");
        let sddl = crate::security::dacl_sddl_for_path(&path).expect("dacl");
        let sid = crate::security::current_user_sid().expect("sid");
        assert!(
            crate::security::dacl_is_current_user_only(&sddl, &sid),
            "secret file DACL must name only the current user: {sddl}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keyring_username_includes_the_runtime_dir_hash() {
        let store = KeyringStore::new(Path::new(r"C:\Users\Gualt\AppData\Local\Devboule"));
        assert_eq!(
            store.username(NAME),
            format!(
                "{NAME}-{}",
                crate::paths::runtime_dir_hash(Path::new(r"C:\Users\Gualt\AppData\Local\Devboule"))
            )
        );
        let other = KeyringStore::new(Path::new(r"C:\Users\Gualt\AppData\Local\Devboule Two"));
        assert_ne!(store.username(NAME), other.username(NAME));
    }
}
