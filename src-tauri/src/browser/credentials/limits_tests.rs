//! What a save may carry: the byte caps, and the origin count measured after
//! the spellings are canonicalised.
//!
//! These are the bounds that keep one Settings form from handing a megabyte of
//! text to the credential store, and they are checked before the store is
//! touched at all — a refusal that had already saved a password would be a
//! password nobody can reach.

use std::sync::Arc;

use super::secrets::fake::InMemory;
use super::{
    checked, Vault, MAX_ORIGINS, MAX_ORIGIN_BYTES, MAX_PASSWORD_BYTES, MAX_SUBMITTED_ORIGINS,
    MAX_USERNAME_BYTES,
};

const SECRET: &str = "SENTINEL-PW-7f3a";

fn one_site() -> Vec<String> {
    vec!["https://mail.example.test".to_owned()]
}

/// A vault over a fresh folder whose store the test also holds, which is how a
/// refusal is checked for having left the credential store alone.
fn vault() -> (tempfile::TempDir, Arc<InMemory>, Vault) {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(InMemory::empty());
    let vault = Vault::new(dir.path().to_path_buf(), Box::new(Arc::clone(&store)));
    (dir, store, vault)
}

#[test]
fn a_username_longer_than_the_cap_is_refused() {
    let (_dir, _store, vault) = vault();

    let refused = vault
        .create(
            "Work mail",
            &one_site(),
            &"u".repeat(MAX_USERNAME_BYTES + 1),
            SECRET,
        )
        .expect_err("the username is over the cap");

    assert!(refused.sentence().contains(&MAX_USERNAME_BYTES.to_string()));
}

#[test]
fn a_password_longer_than_the_cap_is_refused() {
    let (_dir, store, vault) = vault();

    let refused = vault
        .create(
            "Work mail",
            &one_site(),
            "",
            &"p".repeat(MAX_PASSWORD_BYTES + 1),
        )
        .expect_err("the password is over the cap");

    assert!(refused.sentence().contains(&MAX_PASSWORD_BYTES.to_string()));
    assert_eq!(
        store.held_ids(),
        Vec::<String>::new(),
        "nothing reached the credential store"
    );
}

#[test]
fn a_name_longer_than_the_cap_is_refused() {
    let (_dir, _store, vault) = vault();

    let refused = vault
        .create(&"n".repeat(4096), &one_site(), "", SECRET)
        .expect_err("the name is over the cap");

    assert!(refused.sentence().contains("longest this app stores"));
}

#[test]
fn an_origin_longer_than_the_cap_is_refused_before_it_is_parsed() {
    let (_dir, _store, vault) = vault();

    let refused = vault
        .create(
            "Work mail",
            &[format!(
                "https://{}.example.test",
                "a".repeat(MAX_ORIGIN_BYTES)
            )],
            "",
            SECRET,
        )
        .expect_err("the origin is over the cap");

    assert!(
        refused.sentence().contains(&MAX_ORIGIN_BYTES.to_string()),
        "the refusal is about the size, not about the spelling: {}",
        refused.sentence()
    );
}

#[test]
fn a_capped_field_is_accepted_at_exactly_its_cap() {
    let (_dir, _store, vault) = vault();

    let saved = vault
        .create(
            "Work mail",
            &one_site(),
            &"u".repeat(MAX_USERNAME_BYTES),
            &"p".repeat(MAX_PASSWORD_BYTES),
        )
        .expect("a field at its cap is a field");

    assert_eq!(saved.username.len(), MAX_USERNAME_BYTES);
}

#[test]
fn the_command_boundary_answers_before_the_work_is_handed_over() {
    // What the Tauri command asks before it copies the request onto a thread of
    // its own: the same caps, refused there rather than after the copy.
    let refused = checked(
        "Work mail",
        &one_site(),
        &"u".repeat(MAX_USERNAME_BYTES + 1),
        Some(SECRET),
    )
    .expect_err("the username is over the cap");

    assert!(
        refused.sentence().contains(&MAX_USERNAME_BYTES.to_string()),
        "{}",
        refused.sentence()
    );
    assert!(
        checked("Work mail", &one_site(), "person", Some(SECRET)).is_ok(),
        "and a request within every cap passes"
    );
}

#[test]
fn the_site_count_is_measured_after_the_spellings_are_one() {
    let (_dir, _store, vault) = vault();
    // Nine spellings of one site are one site, so the cap is about sites and
    // not about how many ways the person typed them.
    let spellings: Vec<String> = (0..9)
        .map(|at| {
            if at % 2 == 0 {
                "https://mail.example.test".to_owned()
            } else {
                "https://mail.example.test:443/".to_owned()
            }
        })
        .collect();

    let saved = vault
        .create("Work mail", &spellings, "", SECRET)
        .expect("one site is one site");

    assert_eq!(saved.origins, vec!["https://mail.example.test".to_owned()]);
}

#[test]
fn a_ninth_distinct_site_is_refused() {
    let (_dir, _store, vault) = vault();
    let sites: Vec<String> = (0..=MAX_ORIGINS)
        .map(|at| format!("https://site{at}.example.test"))
        .collect();

    let refused = vault
        .create("Work mail", &sites, "", SECRET)
        .expect_err("nine sites are nine sites");

    assert!(refused.sentence().contains(&MAX_ORIGINS.to_string()));
}

#[test]
fn more_addresses_than_the_app_will_read_are_refused_before_any_is_parsed() {
    let (_dir, store, vault) = vault();
    // One site spelled seventeen times is one site, and it is also seventeen
    // strings to read before the count can say so. The raw count is what stops
    // a long list of near-duplicates from costing a person a saved login.
    let spellings: Vec<String> = (0..MAX_SUBMITTED_ORIGINS + 1)
        .map(|_| "https://mail.example.test".to_owned())
        .collect();

    let refused = vault
        .create("Work mail", &spellings, "", SECRET)
        .expect_err("seventeen addresses is more than this app reads");

    assert!(
        refused
            .sentence()
            .contains(&MAX_SUBMITTED_ORIGINS.to_string()),
        "the refusal names the raw bound, not the distinct one: {}",
        refused.sentence()
    );
    assert_eq!(
        store.held_ids(),
        Vec::<String>::new(),
        "and nothing was written before it was refused"
    );
}

#[test]
fn exactly_the_raw_bound_of_one_site_is_accepted() {
    let (_dir, _store, vault) = vault();
    let spellings: Vec<String> = (0..MAX_SUBMITTED_ORIGINS)
        .map(|_| "https://mail.example.test:443/".to_owned())
        .collect();

    let saved = vault
        .create("Work mail", &spellings, "", SECRET)
        .expect("sixteen spellings of one site is one site");

    assert_eq!(saved.origins, vec!["https://mail.example.test".to_owned()]);
}
