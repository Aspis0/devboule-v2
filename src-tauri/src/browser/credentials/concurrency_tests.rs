//! One process-wide lock around the vault's read-modify-write.
//!
//! Every command builds its own `Vault` over the same folder, so nothing but a
//! lock shared by all of them keeps two saves from reading the same book and
//! each writing their own version of it. Atomic replacement stops a torn file;
//! it does not stop the second writer from losing the first one's row.

use std::sync::Arc;

use super::secrets::fake::InMemory;
use super::Vault;

const SECRET: &str = "SENTINEL-PW-7f3a";

/// The origins eight saves can each own, so a lost row is a lost row and not a
/// collision on the site list.
fn origin(at: usize) -> Vec<String> {
    vec![format!("https://site{at}.example.test")]
}

#[test]
fn saves_that_overlap_keep_every_row() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(InMemory::empty());
    let labels: Vec<String> = (0..8).map(|at| format!("Login {at}")).collect();

    std::thread::scope(|threads| {
        for (at, label) in labels.iter().enumerate() {
            let vault = Vault::new(dir.path().to_path_buf(), Box::new(Arc::clone(&store)));
            let site = origin(at);
            threads.spawn(move || vault.create(label, &site, "", SECRET));
        }
    });

    let vault = Vault::new(dir.path().to_path_buf(), Box::new(Arc::clone(&store)));
    let listed = vault.list().expect("listed");

    assert_eq!(
        listed.len(),
        labels.len(),
        "every overlapping save is listed: {:?}",
        listed.iter().map(|login| &login.label).collect::<Vec<_>>()
    );
    for login in &listed {
        assert_eq!(
            vault.password_for(&login.id).ok().flatten(),
            Some(SECRET.to_owned()),
            "every listed row still has the password it was saved with"
        );
    }
}

#[test]
fn a_delete_that_overlaps_a_save_leaves_no_row_behind() {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Arc::new(InMemory::empty());
    Vault::new(dir.path().to_path_buf(), Box::new(Arc::clone(&store)))
        .create("Kept", &origin(0), "", SECRET)
        .expect("saved");
    let doomed = Vault::new(dir.path().to_path_buf(), Box::new(Arc::clone(&store)))
        .create("Doomed", &origin(1), "", SECRET)
        .expect("saved");

    std::thread::scope(|threads| {
        let vault = Vault::new(dir.path().to_path_buf(), Box::new(Arc::clone(&store)));
        threads.spawn(move || vault.delete(&doomed.id));
        for at in 2..5 {
            let vault = Vault::new(dir.path().to_path_buf(), Box::new(Arc::clone(&store)));
            let site = origin(at);
            threads.spawn(move || vault.create(&format!("Login {at}"), &site, "", SECRET));
        }
    });

    let vault = Vault::new(dir.path().to_path_buf(), Box::new(Arc::clone(&store)));
    let listed = vault.list().expect("listed");
    let labels: Vec<&str> = listed.iter().map(|login| login.label.as_str()).collect();

    assert_eq!(
        labels.len(),
        4,
        "the delete and every save took effect exactly once: {labels:?}"
    );
    assert!(
        !labels.contains(&"Doomed"),
        "the deleted row is gone: {labels:?}"
    );
    assert!(
        labels.contains(&"Kept"),
        "and nothing else went with it: {labels:?}"
    );
    let mut in_the_book: Vec<String> = listed.iter().map(|login| login.id.clone()).collect();
    in_the_book.sort();
    assert_eq!(
        store.held_ids(),
        in_the_book,
        "the store holds exactly what the book lists"
    );
}
