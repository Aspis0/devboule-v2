//! The real OS credential store, round-tripped once by hand.
//!
//! It is `#[ignore]`d because it writes to this machine's credential store and
//! depends on a store the build runner may not have unlocked; the rest of the
//! vault runs against the in-memory fake. Run it with
//! `cargo test -p devboule --lib browser::credentials -- --ignored`.

use super::{Keyring, SecretStore};

/// A value that is obviously a test's, so a store left holding one can only be
/// this test's own.
const SENTINEL: &str = "SENTINEL-PW-7f3a";
/// An id that names this test and nothing else.
const ID: &str = "vault-v1-selftest-0000";

#[test]
#[ignore = "writes to this machine's real credential store"]
fn a_password_survives_the_platform_store_and_leaves_nothing_behind() {
    let store = Keyring;
    let _ = store.delete(ID);
    assert_eq!(
        store.get(ID).expect("the store read"),
        None,
        "a store that has never been written reads as empty"
    );

    store.set(ID, SENTINEL).expect("the store saved a password");
    assert_eq!(
        store.get(ID).expect("the store read"),
        Some(SENTINEL.to_owned())
    );

    store.delete(ID).expect("the store removed the password");
    assert_eq!(
        store.get(ID).expect("the store read"),
        None,
        "a removed password must not come back"
    );
    // Removing what is not there is a removal, so a second delete is fine.
    store.delete(ID).expect("removing twice is not a failure");
}
