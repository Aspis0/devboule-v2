//! The pure half of the log read's parser: every rule here is driven from a
//! literal string — no `git` process and no repository. The cases that need
//! a real repository live in `workspace_git_log_tests.rs`.

use super::{parse_commit_records, Commit};

/// One record's bytes, spelled the way git emits them: the `\x1e`-separated
/// header line — sha, short sha, author, ISO date, subject — and nothing
/// else; the frame carries no files array.
fn record(sha: &str, short: &str, author: &str, date: &str, subject: &str) -> String {
    format!("\u{1e}{sha}\0{short}\0{author}\0{date}\0{subject}\n")
}

fn commit_of(records: &[Commit], index: usize) -> &Commit {
    records
        .get(index)
        .unwrap_or_else(|| panic!("no commit at index {index} in {records:?}"))
}

/// The happy path: one commit, every field — the shape the reply carries,
/// pinned field by field.
#[test]
fn one_commit_parses_into_its_entry() {
    let records = parse_commit_records(&record(
        "834fbbf2c1d2e3f4a5b6c7d8e9f0a1b2c3d4e5f6",
        "834fbbf",
        "Test User",
        "2026-09-27T23:37:04-04:00",
        "Add foo",
    ));

    assert_eq!(records.len(), 1, "{records:?}");
    let commit = commit_of(&records, 0);
    assert_eq!(commit.sha, "834fbbf2c1d2e3f4a5b6c7d8e9f0a1b2c3d4e5f6");
    assert_eq!(commit.short_sha, "834fbbf");
    assert_eq!(commit.author_name, "Test User");
    assert_eq!(commit.author_date, "2026-09-27T23:37:04-04:00");
    assert_eq!(commit.subject, "Add foo");
}

/// Git's own order — newest first — is the order the reply carries.
#[test]
fn the_records_keep_gits_newest_first_order() {
    let records = parse_commit_records(&format!(
        "{}{}",
        record(
            "a".repeat(40).as_str(),
            "aaaa",
            "Test User",
            "2026-01-02T03:04:05+00:00",
            "second"
        ),
        record(
            "b".repeat(40).as_str(),
            "bbbb",
            "Test User",
            "2026-01-01T03:04:05+00:00",
            "first"
        ),
    ));

    assert_eq!(records.len(), 2, "{records:?}");
    assert_eq!(commit_of(&records, 0).subject, "second");
    assert_eq!(commit_of(&records, 1).subject, "first");
}

/// A multi-line message contributes its first line only — git's `%s`
/// reflows the subject onto one line, and the body never reaches the
/// record. The parser carries whatever git's `%s` emitted.
#[test]
fn a_multi_line_message_contributes_its_first_line() {
    let records = parse_commit_records(&record(
        "a".repeat(40).as_str(),
        "short",
        "Test User",
        "2026-01-02T03:04:05+00:00",
        "subject line",
    ));

    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(commit_of(&records, 0).subject, "subject line");
}

/// A tab inside the subject travels whole: the field separator is NUL, so
/// no byte but a NUL can end a field early.
#[test]
fn a_tab_inside_the_subject_travels_whole() {
    let records = parse_commit_records(&record(
        "a".repeat(40).as_str(),
        "short",
        "Test User",
        "2026-01-02T03:04:05+00:00",
        "with\ttab",
    ));

    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(commit_of(&records, 0).subject, "with\ttab");
}

/// An empty subject (git's `--allow-empty-message`) is an answer, not a
/// parse failure: the record still carries its sha.
#[test]
fn an_empty_subject_is_an_answer() {
    let records = parse_commit_records(&record(
        "a".repeat(40).as_str(),
        "short",
        "Test User",
        "2026-01-02T03:04:05+00:00",
        "",
    ));

    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(commit_of(&records, 0).subject, "");
    assert_eq!(commit_of(&records, 0).sha.len(), 40);
}

/// A record git could not give five fields, or whose sha is empty, is
/// skipped rather than guessed at — the same rule Paseo's parser applies.
/// A record the accumulator cut short keeps fewer than five fields and
/// lands here.
#[test]
fn a_record_without_a_parseable_header_is_skipped() {
    let records = parse_commit_records(&format!(
        "\u{1e}only-three\0fields\0here\n\u{1e}{}0000000000000000000000000000000000000000\0short\0Test User\02026-01-02T03:04:05+00:00\0good\n",
        "a".repeat(40)
    ));

    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(commit_of(&records, 0).subject, "good");
}

/// An empty run — a revision with no commits — parses to no records.
#[test]
fn an_empty_run_parses_to_no_records() {
    assert!(parse_commit_records("").is_empty());
}

/// A subject carrying a non-UTF-8 byte arrives as the replacement
/// character: the shared git runner lossy-converts every command's stdout,
/// and the parser carries whatever the runner handed it.
#[test]
fn a_non_utf8_subject_arrives_lossy_converted() {
    let records = parse_commit_records(&format!(
        "\u{1e}{}0000000000000000000000000000000000000000\0short\0Test User\02026-01-02T03:04:05+00:00\0caf\u{fffd} subject\n",
        "a".repeat(40)
    ));

    assert_eq!(records.len(), 1, "{records:?}");
    assert_eq!(commit_of(&records, 0).subject, "caf\u{fffd} subject");
}
