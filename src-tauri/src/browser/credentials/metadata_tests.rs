//! The document's own guarantees: a book that survives a round trip, an
//! absent file that is not an error, and a write that leaves the old book
//! whole when it fails.

use super::{path, read, write, Book, SavedLogin};
use std::fs;

fn login(id: &str) -> SavedLogin {
    SavedLogin {
        id: id.to_owned(),
        label: format!("Login {id}"),
        origins: vec!["https://example.test".to_owned()],
        username: "person@example.test".to_owned(),
    }
}

fn book() -> Book {
    Book {
        logins: vec![login("one"), login("two")],
    }
}

#[test]
fn a_book_survives_a_round_trip_through_the_file() {
    let temp = tempfile::tempdir().expect("tempdir");

    write(temp.path(), &book()).expect("written");

    let raw = fs::read_to_string(path(temp.path())).expect("the file is there");
    assert!(
        raw.contains('\n'),
        "the stored book is readable by a person"
    );
    assert_eq!(read(temp.path()).expect("read back"), book());
}

#[test]
fn a_machine_that_has_saved_nothing_has_an_empty_book_rather_than_a_failure() {
    let temp = tempfile::tempdir().expect("tempdir");

    assert_eq!(read(temp.path()).expect("read"), Book::default());
    assert!(
        !path(temp.path()).exists(),
        "reading must not create the file"
    );
}

#[test]
fn a_file_that_is_not_a_book_is_a_failure_and_stays_on_disk() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::write(path(temp.path()), b"{ not a book").expect("garbage");

    let why = read(temp.path()).expect_err("a corrupt book is not an empty one");

    assert!(
        why.contains("not readable"),
        "the refusal says what is wrong: {why}"
    );
    assert_eq!(
        fs::read(path(temp.path())).expect("still there"),
        b"{ not a book",
        "the evidence must survive a read that refused it"
    );
}

#[test]
fn a_write_replaces_the_previous_book_and_leaves_no_file_beside_it() {
    let temp = tempfile::tempdir().expect("tempdir");
    write(temp.path(), &book()).expect("first write");

    let fewer = Book {
        logins: vec![login("two")],
    };
    write(temp.path(), &fewer).expect("second write");

    assert_eq!(read(temp.path()).expect("read"), fewer);
    let left = fs::read_dir(temp.path())
        .expect("the folder")
        .map(|entry| entry.expect("an entry").file_name())
        .collect::<Vec<_>>();
    assert_eq!(left, vec![std::ffi::OsString::from("saved-logins.json")]);
}

#[test]
fn a_write_over_a_path_that_is_a_directory_leaves_the_old_error_visible() {
    let temp = tempfile::tempdir().expect("tempdir");
    fs::create_dir(path(temp.path())).expect("a folder where the file goes");

    let why = write(temp.path(), &book()).expect_err("a folder is not a book");

    assert!(
        why.contains("could not"),
        "the refusal names the act: {why}"
    );
}

#[test]
fn a_login_is_found_by_its_own_id_and_by_no_other() {
    let book = book();

    assert_eq!(book.find("two"), Some(&login("two")));
    assert_eq!(book.find("tw"), None);
    assert_eq!(book.find(""), None);
}
