//! The pure half: every rule here is driven from a literal string or a plain
//! temp file — no `git` process and no repository. The cases that need a real
//! repository live in `workspace_git_status_tests.rs`.

use std::collections::HashMap;
use std::path::PathBuf;

use devboule_protocol::WorkspaceGitFileStatus;

use crate::git::GIT_STDOUT_MAX_BYTES;

use super::{
    build_row, count_untracked_lines, merge_numstat, parse_status, stdout_truncated,
    UNTRACKED_COUNT_MAX_BYTES,
};

fn unique_path(label: &str) -> PathBuf {
    crate::test_dirs::test_temp_dir(&format!("devboule-workspace-parse-{label}"))
}

const HEADERS: &str = "# branch.oid 834fbbf\0# branch.head main\0";

/// Mutant `m:b` — invent rows when porcelain is empty. A committed tree is
/// exactly this: two header records and nothing else.
#[test]
fn a_header_only_status_yields_the_branch_and_no_rows() {
    let (branch, rows) = parse_status(HEADERS);
    assert_eq!(branch.as_deref(), Some("main"));
    assert!(rows.is_empty(), "invented rows: {rows:?}");
}

/// Mutant `m:c` — skip the untracked records. `git diff --numstat` never
/// lists them, so this record is their only way into the reply.
#[test]
fn an_untracked_record_is_a_row() {
    let (branch, rows) = parse_status(&format!("{HEADERS}? new.txt\0"));
    assert_eq!(branch.as_deref(), Some("main"));
    assert_eq!(
        rows,
        vec![(
            "new.txt".to_string(),
            WorkspaceGitFileStatus::Untracked,
            None
        )]
    );
}

/// All six wire words: `?` and `u` decide by record kind, the rest by the
/// `XY` pair — deletion checked before addition, so `AD` reads `deleted`.
#[test]
fn the_six_status_words_come_from_the_record_kind_and_the_xy_pair() {
    let records = [
        ("1 .M N... 100644 100644 100644 a a m.txt", "modified"),
        ("1 M. N... 100644 100644 100644 a a s.txt", "modified"),
        ("1 A. N... 000000 100644 100644 a a added.txt", "added"),
        ("1 .D N... 100644 100644 000000 a a gone.txt", "deleted"),
        ("1 AD N... 100644 000000 000000 a a both.txt", "deleted"),
        (
            "u UU N... 100644 100644 100644 100644 a a b c conflict.txt",
            "conflicted",
        ),
        ("? fresh.txt", "untracked"),
    ];
    for (record, expected) in records {
        let (_, rows) = parse_status(&format!("{HEADERS}{record}\0"));
        assert_eq!(rows.len(), 1, "{record} -> {rows:?}");
        let spelled = serde_json::to_string(&rows[0].1).expect("serialize");
        assert_eq!(
            spelled,
            format!("\"{expected}\""),
            "wrong word for {record}"
        );
    }
}

/// A `2` record, and the bare original path `-z` writes after it: the source
/// must not become a second row, and it must land in `renamed_from` — the
/// field the panel's renamed row acts with. Mutant `e:2` — consume that
/// token without capturing it (its shape before this fix): the row answers
/// `None` and this equality fails.
#[test]
fn a_rename_keeps_one_row_and_carries_its_original_path() {
    let stdout = format!("{HEADERS}2 R. N... 100644 100644 100644 a b R100 new.txt\0old.txt\0");
    let (_, rows) = parse_status(&stdout);
    assert_eq!(
        rows,
        vec![(
            "new.txt".to_string(),
            WorkspaceGitFileStatus::Renamed,
            Some("old.txt".to_string()),
        )],
        "{rows:?}"
    );
}

/// `-z` writes a rename in numstat as `additions<tab>deletions<tab>` then the
/// old and the new path; `status` reports the new one, so that is the key.
#[test]
fn a_rename_numstat_keys_on_the_new_path() {
    let mut counts = HashMap::new();
    merge_numstat("1\t2\t\0old.txt\0new.txt\0", &mut counts);
    assert_eq!(counts.len(), 1, "the source of a rename gets no row either");
    let counted = counts.get("new.txt").expect("keyed on the new path");
    assert_eq!((counted.additions, counted.deletions), (1, 2));
    assert!(!counted.inexact, "a counted delta is exact");
}

/// git prints two dashes for a binary: zero is the floor of any count, and
/// `inexact` is what keeps it from reading as "this change has no lines".
#[test]
fn a_binary_numstat_is_inexact_rather_than_zero_exact_lines() {
    let mut counts = HashMap::new();
    merge_numstat("-\t-\tblob.dat\0", &mut counts);
    let counted = counts.get("blob.dat").expect("the path is keyed");
    assert_eq!((counted.additions, counted.deletions), (0, 0));
    assert!(counted.inexact, "git gave no line count at all");
}

/// The worktree dump and the index dump land on the same path: a file staged
/// and then edited again has both halves.
#[test]
fn two_numstat_dumps_accumulate_on_one_path() {
    let mut counts = HashMap::new();
    merge_numstat("1\t0\tfile.txt\0", &mut counts);
    merge_numstat("2\t0\tfile.txt\0", &mut counts);
    let counted = counts.get("file.txt").expect("accumulated");
    assert_eq!((counted.additions, counted.deletions), (3, 0));
    assert!(!counted.inexact);
}

/// Mutant `m:d` — drop the line-count cap. The size pre-check is what fires
/// first for a static file over the cap, and `lines == 0` is its signature:
/// the read loop would have counted something. Without the cap the reader
/// walks to EOF, answers exactly and never flags; all three assertions die.
/// (The loop's own guard covers a file that *grows* while being read; a
/// racing writer is not something this test can hold still — declared.)
#[test]
fn the_untracked_line_count_stops_at_the_cap_and_says_so() {
    let file = unique_path("cap").join("untracked.txt");
    let mut contents = vec![0u8; UNTRACKED_COUNT_MAX_BYTES * 2];
    for pair in contents.chunks_mut(2) {
        pair[0] = b'a';
        pair[1] = b'\n';
    }
    std::fs::write(&file, &contents).expect("write");
    let (lines, capped) = count_untracked_lines(&file);
    let whole_file = contents.iter().filter(|byte| **byte == b'\n').count() as u64;
    let _ = std::fs::remove_file(&file);

    assert!(capped, "a count that stopped at the cap must say so");
    assert_eq!(lines, 0, "the size check must refuse before reading");
    assert!(lines < whole_file, "the capped count is a floor: {lines}");
}

/// The other three answers of the same reader: a file under the cap is
/// counted exactly, a NUL byte means git would call it binary, and a file
/// that cannot be opened has no count to give.
#[test]
fn the_line_count_answers_exactly_under_the_cap_and_refuses_to_invent_one() {
    let file = unique_path("exact").join("untracked.txt");
    std::fs::write(&file, b"a\nb\nc\n").expect("write");
    assert_eq!(count_untracked_lines(&file), (3, false));
    std::fs::write(&file, b"\0binary\0bytes\0").expect("rewrite");
    assert_eq!(count_untracked_lines(&file), (0, true));
    let _ = std::fs::remove_file(&file);
    assert_eq!(
        count_untracked_lines(&file),
        (0, true),
        "gone is not zero lines"
    );
}

/// A rename's original path, bare, must not become a row — and it must not
/// become one even when it *looks* like an entry. Windows forbids `?` in a
/// file name, so this record is spelled rather than produced by a real
/// `git mv`. **Mutant `m:e`: delete the `records.next()` skip in
/// `parse_status`** — `? trap.txt` then parses as `Untracked`.
#[test]
fn the_original_path_of_a_rename_never_becomes_a_row() {
    let stdout = format!("{HEADERS}2 R. N... 100644 100644 100644 a b R100 new.txt\0? trap.txt\0");
    let (_, rows) = parse_status(&stdout);
    assert_eq!(
        rows,
        vec![(
            "new.txt".to_string(),
            WorkspaceGitFileStatus::Renamed,
            // The source is captured even when it spells like an entry: one
            // token, one job — consumed by position, remembered as identity.
            Some("? trap.txt".to_string()),
        )],
        "the orphan record leaked in as a row: {rows:?}"
    );
}

/// Both dumps go through the shared accumulator, which cuts at
/// `GIT_STDOUT_MAX_BYTES + 1` and exits 0: only the length can say it happened.
#[test]
fn a_dump_past_the_shared_cap_is_recognised_by_length_alone() {
    assert!(!stdout_truncated("short output"));
    assert!(!stdout_truncated(&"x".repeat(GIT_STDOUT_MAX_BYTES)));
    assert!(stdout_truncated(&"x".repeat(GIT_STDOUT_MAX_BYTES + 1)));
}

/// Mutant `m:f` — drop `degraded` from the row. A staged-only file whose
/// numstat round failed would then read `(0, 0, capped: false)`: an exact
/// zero that is really "we do not know".
#[test]
fn a_degraded_count_round_marks_its_numbers_a_floor_rather_than_exact_zeros() {
    let root = unique_path("degraded");
    let empty = HashMap::new();

    let missing = build_row(
        &root,
        "staged.txt",
        WorkspaceGitFileStatus::Added,
        &empty,
        true,
    );
    assert_eq!((missing.additions, missing.deletions), (0, 0));
    assert!(
        missing.capped,
        "a zero from a failed round must not read as exact"
    );

    let healthy = build_row(
        &root,
        "staged.txt",
        WorkspaceGitFileStatus::Added,
        &empty,
        false,
    );
    assert!(!healthy.capped, "a healthy round's mode-only zero is exact");

    let mut counts = HashMap::new();
    merge_numstat("3\t1\tcounted.txt\0", &mut counts);
    let partial = build_row(
        &root,
        "counted.txt",
        WorkspaceGitFileStatus::Modified,
        &counts,
        true,
    );
    assert_eq!((partial.additions, partial.deletions), (3, 1));
    assert!(partial.capped, "half of a degraded round is still a floor");
    let _ = std::fs::remove_dir_all(&root);
}

/// `build_row` picks where a number comes from: untracked counts its own
/// file, a conflicted path has no delta to give, a counted path takes the
/// map, and a path git printed no line for is exactly zero.
#[test]
fn a_row_takes_its_counts_from_the_status_it_carries() {
    let root = unique_path("rows-root");
    std::fs::write(root.join("fresh.txt"), "x\ny\n").expect("write");
    let mut counts = HashMap::new();
    merge_numstat("7\t2\tcounted.txt\0", &mut counts);

    let untracked = build_row(
        &root,
        "fresh.txt",
        WorkspaceGitFileStatus::Untracked,
        &counts,
        false,
    );
    assert_eq!((untracked.additions, untracked.deletions), (2, 0));
    assert!(!untracked.capped);

    let conflicted = build_row(
        &root,
        "conflict.txt",
        WorkspaceGitFileStatus::Conflicted,
        &counts,
        false,
    );
    assert_eq!((conflicted.additions, conflicted.deletions), (0, 0));
    assert!(conflicted.capped, "an unmerged path has no exact count");

    let counted = build_row(
        &root,
        "counted.txt",
        WorkspaceGitFileStatus::Modified,
        &counts,
        false,
    );
    assert_eq!((counted.additions, counted.deletions), (7, 2));
    assert!(!counted.capped);

    let mode_only = build_row(
        &root,
        "mode.txt",
        WorkspaceGitFileStatus::Modified,
        &counts,
        false,
    );
    assert_eq!((mode_only.additions, mode_only.deletions), (0, 0));
    assert!(!mode_only.capped, "no numstat line is git saying no delta");
    let _ = std::fs::remove_dir_all(&root);
}
