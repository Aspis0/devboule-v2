//! The vault's own behaviour: what a save lists, what a change keeps, and what
//! a half-finished delete says.

use std::sync::Arc;

use super::origin;
use super::secrets::fake::{Act, InMemory};
use super::{metadata, Refusal, Vault};

/// A value that is obviously a test's.
const SECRET: &str = "SENTINEL-PW-7f3a";

/// A vault over a fresh folder and a store in memory.
fn vault() -> (tempfile::TempDir, Vault) {
    let dir = tempfile::tempdir().expect("tempdir");
    let vault = Vault::new(dir.path().to_path_buf(), Box::new(InMemory::empty()));
    (dir, vault)
}

/// One saved login, saved.
fn saved(vault: &Vault) -> super::SavedLogin {
    vault
        .create(
            "Work mail",
            &["https://mail.example.test".to_owned()],
            "person@example.test",
            SECRET,
        )
        .expect("the login was saved")
}

#[test]
fn a_saved_login_is_listed_as_its_metadata_and_nothing_else() {
    let (_dir, vault) = vault();

    let listed = saved(&vault);

    assert_eq!(
        listed.origins,
        vec!["https://mail.example.test".to_owned()],
        "the origin is stored as it will be compared"
    );
    assert!(
        !format!("{listed:?}").contains(SECRET),
        "no password in metadata"
    );
    let json = serde_json::to_string(&vault.list().expect("listed")).expect("json");
    assert!(!json.contains(SECRET), "no password in the list: {json}");
    assert!(
        json.contains("\"username\""),
        "the username is shown: {json}"
    );
}

#[test]
fn a_password_is_read_only_by_the_id_of_the_entry_that_saved_it() {
    let (_dir, vault) = vault();
    let listed = saved(&vault);

    assert_eq!(
        vault.password_for(&listed.id).expect("the store read"),
        Some(SECRET.to_owned())
    );
    assert_eq!(
        vault
            .password_for("0000000000000000")
            .expect("the store read"),
        None,
        "an id no entry holds names no password"
    );
}

#[test]
fn a_change_without_a_password_keeps_the_one_already_stored() {
    let (_dir, vault) = vault();
    let listed = saved(&vault);

    let changed = vault
        .update(
            &listed.id,
            "Personal mail",
            &["https://mail.example.test:8443".to_owned()],
            "person@example.test",
            None,
        )
        .expect("the change was saved");

    assert_eq!(changed.label, "Personal mail");
    assert_eq!(
        vault.password_for(&listed.id).expect("the store read"),
        Some(SECRET.to_owned()),
        "an empty password field in the form means keep the one stored"
    );
}

#[test]
fn a_change_with_a_password_replaces_it() {
    let (_dir, vault) = vault();
    let listed = saved(&vault);

    vault
        .update(
            &listed.id,
            "Work mail",
            &listed_origins(),
            "",
            Some("another-secret"),
        )
        .expect("the change was saved");

    assert_eq!(
        vault.password_for(&listed.id).expect("the store read"),
        Some("another-secret".to_owned())
    );
}

fn listed_origins() -> Vec<String> {
    vec!["https://mail.example.test".to_owned()]
}

#[test]
fn an_address_that_is_not_an_exact_origin_leaves_nothing_stored() {
    let (dir, vault) = vault();

    let refused = vault
        .create(
            "Work mail",
            &["https://mail.example.test/inbox".to_owned()],
            "",
            SECRET,
        )
        .expect_err("a path is not an origin");

    assert_eq!(
        refused,
        Refusal::Asked(
            "\"https://mail.example.test/inbox\" is not a site address this app can save: an origin has no path after the host."
                .to_owned()
        )
    );
    assert!(vault.list().expect("listed").is_empty(), "nothing listed");
    assert!(
        !metadata::path(dir.path()).exists(),
        "and no file left behind"
    );
}

#[test]
fn a_store_that_refuses_leaves_no_entry_listed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = InMemory::empty();
    store.refuse(Act::Set);
    let vault = Vault::new(dir.path().to_path_buf(), Box::new(store));

    let refused = vault
        .create("Work mail", &listed_origins(), "", SECRET)
        .expect_err("the store refused the password");

    assert!(matches!(refused, Refusal::Failed(_)), "{refused:?}");
    assert!(vault.list().expect("listed").is_empty());
}

#[test]
fn a_delete_the_store_refuses_leaves_the_entry_listed_and_working() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(InMemory::empty());
    let listed = saved(&Vault::new(
        dir.path().to_path_buf(),
        Box::new(Arc::clone(&store)),
    ));
    // The machine's own store goes unavailable under an entry that is already
    // in it, which is not a second, empty store refusing.
    store.refuse(Act::Delete);
    let vault = Vault::new(dir.path().to_path_buf(), Box::new(store));

    let refused = vault
        .delete(&listed.id)
        .expect_err("the store refused the removal");

    assert!(matches!(refused, Refusal::Failed(_)), "{refused:?}");
    assert_eq!(
        vault.list().expect("listed").len(),
        1,
        "an entry whose password is still there is still an entry"
    );
    assert_eq!(
        vault.password_for(&listed.id).ok().flatten(),
        Some(SECRET.to_owned())
    );
}

#[test]
fn an_entry_that_is_not_there_is_refused_by_id() {
    let (_dir, vault) = vault();

    let refused = vault.delete("0000000000000000").expect_err("no such entry");

    assert_eq!(
        refused,
        Refusal::Asked("That saved login is not in the list.".to_owned())
    );
}

#[test]
fn only_the_entries_of_the_origin_asked_about_are_offered() {
    let (_dir, vault) = vault();
    let wanted = saved(&vault);
    vault
        .create(
            "Bank",
            &["https://bank.example.test".to_owned()],
            "person@example.test",
            SECRET,
        )
        .expect("saved");

    let for_mail = vault
        .lookup_for_origin(&origin::canonical("https://mail.example.test").expect("canonical"))
        .expect("the lookup read the book");

    assert_eq!(
        for_mail.iter().map(|login| &login.id).collect::<Vec<_>>(),
        vec![&wanted.id],
        "a password is offered only to the origin the person named"
    );
}
