//! What the vault does when one half of a two-place save succeeds and the
//! other does not: the OS credential store here, the metadata file there.
//!
//! Every case in this file is a state the app can be left in, so each one ends
//! with what the answer said AND what the two stores actually hold. A sentence
//! that does not match the machine is worse than no sentence.

use std::fs;
use std::sync::Arc;

use super::secrets::fake::{Act, InMemory};
use super::{metadata, Refusal, Vault};

/// A value that is obviously a test's.
const SECRET: &str = "SENTINEL-PW-7f3a";

fn origins() -> Vec<String> {
    vec!["https://mail.example.test".to_owned()]
}

/// A folder whose book can never be written, on any OS: the read answers
/// an empty book while the write fails at staging, so both tests exercise
/// the same compensation. The shape differs per platform — a file in a
/// folder's place reads NotFound-as-empty on Windows but ENOTDIR on Unix,
/// which tested two different failures — so Windows keeps that shape and
/// Unix uses a read-only directory instead.
#[cfg(windows)]
fn a_dir_whose_book_cannot_be_written(dir: &std::path::Path) -> std::path::PathBuf {
    let blocked = dir.join("not-a-folder");
    fs::write(&blocked, b"this is a file where a folder belongs").expect("the blocker");
    blocked
}

#[cfg(not(windows))]
fn a_dir_whose_book_cannot_be_written(dir: &std::path::Path) -> std::path::PathBuf {
    let blocked = dir.join("read-only-book");
    fs::create_dir(&blocked).expect("the folder");
    let mut permissions = fs::metadata(&blocked).expect("metadata").permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&blocked, permissions).expect("read-only");
    blocked
}

#[test]
fn a_save_whose_book_cannot_be_written_takes_the_password_back() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(InMemory::empty());
    let vault = Vault::new(
        a_dir_whose_book_cannot_be_written(dir.path()),
        Box::new(Arc::clone(&store)),
    );

    let refused = vault
        .create("Work mail", &origins(), "person@example.test", SECRET)
        .expect_err("the book could not be written");

    assert!(matches!(refused, Refusal::Failed(_)), "{refused:?}");
    assert_eq!(
        store.held_ids(),
        Vec::<String>::new(),
        "nothing left in the credential store that no row names"
    );
}

#[test]
fn a_save_that_cannot_be_unwound_says_the_password_is_still_there() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(InMemory::empty());
    // The store takes the password and then refuses to give it back, which is
    // the state where a secret sits in the machine under an id no list shows.
    store.fail_after(Act::Delete, 0);
    let vault = Vault::new(
        a_dir_whose_book_cannot_be_written(dir.path()),
        Box::new(Arc::clone(&store)),
    );

    let refused = vault
        .create("Work mail", &origins(), "person@example.test", SECRET)
        .expect_err("the book could not be written");

    assert!(
        refused.sentence().contains("could not be removed"),
        "the answer names what is left behind: {}",
        refused.sentence()
    );
    assert!(
        refused.sentence().contains("still filed under"),
        "the answer says where it is: {}",
        refused.sentence()
    );
    assert_eq!(
        store.held_ids().len(),
        1,
        "the password really is still filed, which the sentence says"
    );
}

/// The one act of a save that can be undone on a machine whose book is
/// read-only: a file nobody can rename over. Windows spells that refusal as
/// an access error; the test is written for the platform that has one.
#[cfg(windows)]
fn make_the_book_unwritable(dir: &std::path::Path) {
    let file = metadata::path(dir);
    let mut permissions = fs::metadata(&file)
        .expect("the book is there")
        .permissions();
    permissions.set_readonly(true);
    fs::set_permissions(&file, permissions).expect("the book is made read-only");
}

#[cfg(windows)]
#[test]
fn a_change_that_cannot_be_written_puts_the_stored_password_back() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(InMemory::empty());
    let vault = Vault::new(dir.path().to_path_buf(), Box::new(Arc::clone(&store)));
    let listed = vault
        .create("Work mail", &origins(), "person@example.test", SECRET)
        .expect("saved");
    make_the_book_unwritable(dir.path());

    let refused = vault
        .update(
            &listed.id,
            "Work mail",
            &origins(),
            "person@example.test",
            Some("a new secret"),
        )
        .expect_err("the book could not be written");

    assert!(
        !refused.sentence().contains("new secret"),
        "the answer never quotes the password: {}",
        refused.sentence()
    );
    assert_eq!(
        vault.password_for(&listed.id).ok().flatten(),
        Some(SECRET.to_owned()),
        "the password the row names is the one the store holds again"
    );
}

#[cfg(windows)]
#[test]
fn a_change_that_cannot_be_unwound_says_the_password_has_moved() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(InMemory::empty());
    let vault = Vault::new(dir.path().to_path_buf(), Box::new(Arc::clone(&store)));
    let listed = vault
        .create("Work mail", &origins(), "person@example.test", SECRET)
        .expect("saved");
    make_the_book_unwritable(dir.path());
    // The store takes the new password and refuses the one that would put the
    // old back: one `set` to save it, one to change it, and the third is the
    // one that fails.
    store.fail_after(Act::Set, 2);

    let refused = vault
        .update(
            &listed.id,
            "Work mail",
            &origins(),
            "person@example.test",
            Some("a new secret"),
        )
        .expect_err("the book could not be written");

    assert!(
        refused.sentence().contains("holds the new password"),
        "the answer says which password is now in the store: {}",
        refused.sentence()
    );
    assert!(
        !refused.sentence().contains("a new secret"),
        "without quoting it: {}",
        refused.sentence()
    );
    assert_eq!(
        vault.password_for(&listed.id).ok().flatten(),
        Some("a new secret".to_owned()),
        "which is what the sentence claims"
    );
    assert_eq!(
        vault.list().expect("listed")[0].label,
        "Work mail",
        "and the row is the one it was before the failed change"
    );
}

#[test]
fn a_change_that_cannot_read_the_stored_password_changes_nothing() {
    // The old password is read so it can be put back, so a store that will
    // not read it must be refused before the new one is written.
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(InMemory::empty());
    let vault = Vault::new(dir.path().to_path_buf(), Box::new(Arc::clone(&store)));
    let listed = vault
        .create("Work mail", &origins(), "person@example.test", SECRET)
        .expect("saved");
    store.refuse(Act::Get);

    let refused = vault
        .update(
            &listed.id,
            "Work mail",
            &origins(),
            "person@example.test",
            Some("a new secret"),
        )
        .expect_err("the store would not read");

    assert!(matches!(refused, Refusal::Failed(_)), "{refused:?}");
    assert_eq!(
        store.held(&listed.id),
        Some(SECRET.to_owned()),
        "the stored password was never written over"
    );
}

#[cfg(windows)]
#[test]
fn a_delete_the_book_cannot_replace_says_the_password_is_already_gone() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(InMemory::empty());
    let vault = Vault::new(dir.path().to_path_buf(), Box::new(store));
    let listed = vault
        .create("Work mail", &origins(), "person@example.test", SECRET)
        .expect("saved");
    make_the_book_unwritable(dir.path());

    let refused = vault
        .delete(&listed.id)
        .expect_err("the row could not be written");

    assert!(
        refused.sentence().contains("The password is gone"),
        "the answer says what did happen: {}",
        refused.sentence()
    );
    assert_eq!(
        vault.password_for(&listed.id).ok().flatten(),
        None,
        "and the password really is gone"
    );
}
