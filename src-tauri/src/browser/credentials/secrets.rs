//! Where a password actually lives: the OS credential store, behind one trait.
//!
//! This is the only file in the vault that names a credential store. Nothing
//! above it knows whether the bytes are in the Windows Credential Manager, the
//! macOS keychain or a test's memory, which is what lets another platform be a
//! dependency rather than a rewrite.

/// One secret, named by the id of the entry that owns it.
///
/// `get` answering `Ok(None)` means the store has no such item, which is a
/// fact about the store and not a failure: an entry whose password was removed
/// behind the app's back is still an entry the list can show.
pub trait SecretStore: Send + Sync {
    fn set(&self, id: &str, password: &str) -> Result<(), String>;
    fn get(&self, id: &str) -> Result<Option<String>, String>;
    /// Removing an item that is not there is a removal, not a failure.
    fn delete(&self, id: &str) -> Result<(), String>;
}

/// The service every saved login of this app is filed under, whatever the
/// platform calls it.
pub const SERVICE: &str = "devboule.saved-login";

/// The OS credential store. `keyring`'s `v1` feature is what picks the native
/// store per platform; the entry's username is the entry's opaque id, so the
/// store itself holds no site, no label and no username.
pub struct Keyring;

impl SecretStore for Keyring {
    fn set(&self, id: &str, password: &str) -> Result<(), String> {
        entry(id)?.set_password(password).map_err(why_writing)
    }

    fn get(&self, id: &str) -> Result<Option<String>, String> {
        match entry(id)?.get_password() {
            Ok(password) => Ok(Some(password)),
            // An item that is not there is a fact about the store: the row is
            // listed either way, and a missing password is not a failure of
            // the machine.
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(error) => Err(why(error)),
        }
    }

    fn delete(&self, id: &str) -> Result<(), String> {
        match entry(id)?.delete_credential() {
            // Removing what is not there is the removal that was asked for.
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(error) => Err(why(error)),
        }
    }
}

fn entry(id: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new(SERVICE, id).map_err(|error| {
        format!(
            "This machine's credential store would not open an item for this login: {}.",
            why(error)
        )
    })
}

/// What the store said while reading or removing, in a sentence the Settings
/// page can show.
fn why(error: keyring::Error) -> String {
    match error {
        keyring::Error::NoEntry => {
            "This machine's credential store has no such saved password.".to_owned()
        }
        keyring::Error::NoStorageAccess(detail) => {
            format!("This machine's credential store is not available: {detail}.")
        }
        other => format!("This machine's credential store refused: {other}."),
    }
}

/// The same, for a write. "No such entry" is what a store says when it was
/// asked to create one and could not — it is never what a save means, and
/// repeating it here would tell a person their password is missing just as
/// they typed it.
fn why_writing(error: keyring::Error) -> String {
    if let keyring::Error::NoEntry = error {
        return "This machine's credential store would not create an item for this \
                password."
            .to_owned();
    }
    why(error)
}

/// A store that keeps its secrets in memory, so a test can drive the vault
/// without this machine's credential store — and can be made to fail on
/// purpose, which is how the partial-failure cases are proved.
#[cfg(test)]
pub(crate) mod fake {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use super::SecretStore;

    /// Which act an [`InMemory`] refuses.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
    pub enum Act {
        Set,
        Get,
        Delete,
    }

    pub struct InMemory {
        held: Mutex<HashMap<String, String>>,
        refusing: Mutex<Option<(Act, u32)>>,
        calls: Mutex<HashMap<Act, u32>>,
    }

    impl InMemory {
        pub fn empty() -> Self {
            InMemory {
                held: Mutex::new(HashMap::new()),
                refusing: Mutex::new(None),
                calls: Mutex::new(HashMap::new()),
            }
        }

        /// Make this store fail every call of one act, as an OS store does when
        /// the machine's own secrets are locked or its quota is spent. The flag
        /// is set on a store an entry is already saved in, so the refusal is
        /// the machine's and not a second, empty store's.
        pub fn refuse(&self, act: Act) {
            self.fail_after(act, 0);
        }

        /// Fail every call of one act after `allowed` of them have gone
        /// through, which is the shape of a store that takes a write and
        /// refuses the one that would put it back.
        pub fn fail_after(&self, act: Act, allowed: u32) {
            *self.refusing.lock().expect("fake store poisoned") = Some((act, allowed));
        }

        /// What this store holds under `id`, for a test that looks at the
        /// machine rather than at an answer.
        pub fn held(&self, id: &str) -> Option<String> {
            self.held
                .lock()
                .expect("fake store poisoned")
                .get(id)
                .cloned()
        }

        /// Every id this store holds a password under, which is what "the list
        /// and the store agree" is measured against.
        pub fn held_ids(&self) -> Vec<String> {
            let mut ids: Vec<String> = self
                .held
                .lock()
                .expect("fake store poisoned")
                .keys()
                .cloned()
                .collect();
            ids.sort();
            ids
        }

        fn refuses(&self, act: Act) -> bool {
            let mut calls = self.calls.lock().expect("fake store poisoned");
            let seen = calls.entry(act).or_insert(0);
            *seen += 1;
            let taken = *seen;
            drop(calls);
            match *self.refusing.lock().expect("fake store poisoned") {
                Some((refused, allowed)) => refused == act && taken > allowed,
                None => false,
            }
        }
    }

    impl SecretStore for InMemory {
        fn set(&self, id: &str, password: &str) -> Result<(), String> {
            if self.refuses(Act::Set) {
                return Err("the fake store refuses to save".to_owned());
            }
            self.held
                .lock()
                .expect("fake store poisoned")
                .insert(id.to_owned(), password.to_owned());
            Ok(())
        }

        fn get(&self, id: &str) -> Result<Option<String>, String> {
            if self.refuses(Act::Get) {
                return Err("the fake store refuses to read".to_owned());
            }
            Ok(self
                .held
                .lock()
                .expect("fake store poisoned")
                .get(id)
                .cloned())
        }

        fn delete(&self, id: &str) -> Result<(), String> {
            if self.refuses(Act::Delete) {
                return Err("the fake store refuses to remove".to_owned());
            }
            self.held.lock().expect("fake store poisoned").remove(id);
            Ok(())
        }
    }

    /// A store shared by two vaults over one folder: what proves that a
    /// refusal comes from the machine's own store and not from an empty one.
    impl SecretStore for std::sync::Arc<InMemory> {
        fn set(&self, id: &str, password: &str) -> Result<(), String> {
            (**self).set(id, password)
        }

        fn get(&self, id: &str) -> Result<Option<String>, String> {
            (**self).get(id)
        }

        fn delete(&self, id: &str) -> Result<(), String> {
            (**self).delete(id)
        }
    }
}

#[cfg(test)]
#[path = "secrets_tests.rs"]
mod tests;
