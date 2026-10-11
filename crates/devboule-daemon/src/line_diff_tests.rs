//! The line diff of two texts and of a provider's unified diff, and the counts
//! the rows add up to.

use devboule_protocol::{WorkspaceGitDiffLine, WorkspaceGitDiffLineKind};

use super::{changed_counts, replace_lines, unified_lines};

fn line(kind: WorkspaceGitDiffLineKind, text: &str) -> WorkspaceGitDiffLine {
    WorkspaceGitDiffLine {
        kind,
        text: text.to_string(),
    }
}

#[test]
fn a_replaced_line_is_removed_then_added() {
    assert_eq!(
        replace_lines("one\n", "two\n"),
        vec![
            line(WorkspaceGitDiffLineKind::Remove, "one"),
            line(WorkspaceGitDiffLineKind::Add, "two"),
        ]
    );
}

#[test]
fn unchanged_text_has_no_rows_and_counts_nothing() {
    assert!(replace_lines("a\nb\n", "a\nb\n").is_empty());
    assert!(replace_lines("", "").is_empty());
    // The trailing newline is not a line: a file that only gained its last
    // newline did not change.
    assert!(replace_lines("a\nb", "a\nb\n").is_empty());
}

#[test]
fn a_new_file_is_all_added_and_a_delete_is_all_removed() {
    assert_eq!(
        changed_counts(&replace_lines("", "one\ntwo\nthree\n")),
        (3, 0)
    );
    assert_eq!(
        changed_counts(&replace_lines("one\ntwo\nthree\n", "")),
        (0, 3)
    );
}

#[test]
fn an_insertion_counts_only_the_new_line() {
    assert_eq!(
        replace_lines("a\nc\n", "a\nb\nc\n"),
        vec![line(WorkspaceGitDiffLineKind::Add, "b")]
    );
    assert_eq!(
        changed_counts(&replace_lines("a\nb\nc\n", "a\nc\n")),
        (0, 1)
    );
}

#[test]
fn the_common_head_and_tail_are_not_part_of_the_change() {
    // Two thousand shared lines would overflow the cell ceiling; the trim
    // leaves one line in the middle, so the change stays exact.
    let old: String = (0..2_000).map(|index| format!("line {index}\n")).collect();
    let new = format!("{old}added\n");
    assert_eq!(
        replace_lines(&old, &new),
        vec![line(WorkspaceGitDiffLineKind::Add, "added")]
    );
}

#[test]
fn a_pair_past_the_cell_ceiling_counts_its_middle_as_wholly_changed() {
    let old: String = (0..2_000).map(|index| format!("old {index}\n")).collect();
    let new: String = (0..2_000).map(|index| format!("new {index}\n")).collect();
    assert_eq!(changed_counts(&replace_lines(&old, &new)), (2_000, 2_000));
}

#[test]
fn a_providers_own_diff_keeps_its_hunk_lines_and_headers() {
    let diff = "--- a/src/lib.rs\n+++ b/src/lib.rs\n@@ -1,2 +1,2 @@\n kept\n-old\n+new\n";
    assert_eq!(
        unified_lines(diff),
        vec![
            line(WorkspaceGitDiffLineKind::Header, "@@ -1,2 +1,2 @@"),
            line(WorkspaceGitDiffLineKind::Context, "kept"),
            line(WorkspaceGitDiffLineKind::Remove, "old"),
            line(WorkspaceGitDiffLineKind::Add, "new"),
        ]
    );
    assert!(unified_lines("").is_empty());
    // Nothing before the first hunk is content, whatever it starts with.
    assert!(unified_lines("--- a\n+++ b\n").is_empty());
}

#[test]
fn a_hunkless_add_or_delete_is_its_bare_lines() {
    assert_eq!(
        unified_lines("+hello\n"),
        vec![line(WorkspaceGitDiffLineKind::Add, "hello")]
    );
    assert_eq!(changed_counts(&unified_lines("-x\n-y\n")), (0, 2));
}

#[test]
fn a_second_file_in_one_unified_diff_is_not_content_until_its_hunk() {
    let diff = "@@ -1 +1 @@\n-a\n+b\ndiff --git a/y b/y\n--- a/y\n+++ b/y\n@@ -1 +1 @@\n-c\n+d\n";
    assert_eq!(changed_counts(&unified_lines(diff)), (2, 2));
}

#[test]
fn the_no_newline_marker_is_not_content() {
    let diff = "@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new\n";
    assert_eq!(changed_counts(&unified_lines(diff)), (1, 1));
}
