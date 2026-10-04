//! What each of the four commands can answer.
//!
//! Every answer is the vault's metadata, and the assertion is on the JSON the
//! IPC layer would serialise: a sentinel password must appear in none of them,
//! including the ones that just wrote it to the OS store.

use serde_json::Value;

use super::{create, delete, list, update, Vault};
use crate::browser::credentials::secrets::fake::InMemory;

/// A value that is obviously a test's.
const SECRET: &str = "SENTINEL-PW-7f3a";
const ORIGIN: &[&str] = &["https://mail.example.test"];

struct Over {
    dir: tempfile::TempDir,
    vault: Vault,
}

impl Over {
    fn new() -> Self {
        let dir = tempfile::tempdir().expect("tempdir");
        let vault = Vault::new(dir.path().to_path_buf(), Box::new(InMemory::empty()));
        Over { dir, vault }
    }

    /// The folder the commands resolve, which is what the metadata is in.
    fn folder(&self) -> &std::path::Path {
        self.dir.path()
    }
}

fn origins() -> Vec<String> {
    ORIGIN.iter().map(|one| (*one).to_owned()).collect()
}

fn json_of<T: serde::Serialize>(answer: &T) -> String {
    serde_json::to_string(answer).expect("the answer serialises")
}

#[test]
fn no_command_answers_with_a_password() {
    let over = Over::new();

    let saved: Value = serde_json::to_value(
        create(
            &over.vault,
            "Work mail",
            &origins(),
            "person@example.test",
            SECRET,
        )
        .expect("the login was saved"),
    )
    .expect("json");
    let id = saved["id"].as_str().expect("an id").to_owned();
    let listed: Value =
        serde_json::to_value(list(&over.vault).expect("the list was read")).expect("json");
    let changed: Value = serde_json::to_value(
        update(
            &over.vault,
            &id,
            "Personal mail",
            &origins(),
            "person@example.test",
            Some("another-secret"),
        )
        .expect("the change was saved"),
    )
    .expect("json");
    delete(&over.vault, &id).expect("the entry was removed");
    // A delete answers nothing, which is what the IPC sends for a `()`.
    let removed = Value::Null;

    for (command, answer) in [
        ("create", &saved),
        ("list", &listed),
        ("update", &changed),
        ("delete", &removed),
    ] {
        assert!(!json_of(answer).contains(SECRET), "{command}: {answer}");
        assert!(!json_of(answer).contains("password"), "{command}: {answer}");
    }
    // The one that kept the password reads back the entry the person changed,
    // and the list is the metadata of both calls.
    assert_eq!(changed["label"], "Personal mail");
    assert_eq!(listed[0]["username"], "person@example.test");
}

#[test]
fn a_change_without_a_password_leaves_the_stored_one_alone() {
    let over = Over::new();
    let saved = create(
        &over.vault,
        "Work mail",
        &origins(),
        "person@example.test",
        SECRET,
    )
    .expect("saved");

    update(
        &over.vault,
        &saved.id,
        "Personal mail",
        &origins(),
        "person@example.test",
        None,
    )
    .expect("changed");

    assert_eq!(
        over.vault.password_for(&saved.id).ok().flatten(),
        Some(SECRET.to_owned()),
        "the form's empty password field means keep the one stored"
    );
}

#[test]
fn what_the_commands_write_is_what_the_next_call_reads() {
    let over = Over::new();

    create(
        &over.vault,
        "Work mail",
        &origins(),
        "person@example.test",
        SECRET,
    )
    .expect("saved");

    let listed = list(&over.vault).expect("listed");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].origins, origins());

    delete(&over.vault, &listed[0].id).expect("deleted");
    assert!(list(&over.vault).expect("listed").is_empty());
    assert!(
        over.folder().join("saved-logins.json").exists(),
        "the file is where the commands put it"
    );
}
