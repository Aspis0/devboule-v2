//! One path's diff: each selected edit's rows in order, labelled when a file has
//! more than one, and marked truncated when any recorded text was cut.

use devboule_protocol::{AgentEditText, WorkspaceGitDiffLine, WorkspaceGitDiffLineKind};
use serde_json::json;

use super::{lines_for, MAX_DIFF_REPLY_BYTES};
use crate::agent_edit_records::AgentEditRecord;

fn record(text: Option<AgentEditText>, truncated: bool) -> AgentEditRecord {
    AgentEditRecord {
        session_id: "s.a".to_string(),
        agent: "agent s.a".to_string(),
        tool_call_id: "call-1".to_string(),
        at_ms: 100,
        path: "src/lib.rs".to_string(),
        text,
        added: 0,
        removed: 0,
        truncated,
    }
}

fn replace(old: &str, new: &str) -> Option<AgentEditText> {
    Some(AgentEditText::Replace {
        old: old.to_string(),
        new: new.to_string(),
    })
}

fn line(kind: WorkspaceGitDiffLineKind, text: &str) -> WorkspaceGitDiffLine {
    WorkspaceGitDiffLine {
        kind,
        text: text.to_string(),
    }
}

#[test]
fn one_replace_is_its_line_diff_with_no_label() {
    let record = record(replace("one\n", "two\n"), false);
    let (lines, truncated) = lines_for(&[&record]);
    assert!(!truncated);
    assert_eq!(
        lines,
        vec![
            line(WorkspaceGitDiffLineKind::Remove, "one"),
            line(WorkspaceGitDiffLineKind::Add, "two"),
        ]
    );
}

#[test]
fn a_provider_diff_is_read_as_it_was_sent() {
    let record = record(
        Some(AgentEditText::Diff {
            text: "@@ -1 +1 @@\n-old\n+new\n".to_string(),
        }),
        false,
    );
    let (lines, _) = lines_for(&[&record]);
    assert_eq!(
        lines,
        vec![
            line(WorkspaceGitDiffLineKind::Header, "@@ -1 +1 @@"),
            line(WorkspaceGitDiffLineKind::Remove, "old"),
            line(WorkspaceGitDiffLineKind::Add, "new"),
        ]
    );
}

#[test]
fn two_edits_of_one_file_are_labelled_in_order() {
    let first = record(replace("a\n", "b\n"), false);
    let second = record(replace("c\n", "d\n"), false);
    let (lines, _) = lines_for(&[&first, &second]);
    let headers: Vec<&str> = lines
        .iter()
        .filter(|line| line.kind == WorkspaceGitDiffLineKind::Header)
        .map(|line| line.text.as_str())
        .collect();
    assert_eq!(headers, ["Edit 1 of 2", "Edit 2 of 2"]);
    assert_eq!(lines.len(), 6);
}

#[test]
fn a_cut_text_marks_the_whole_diff_truncated() {
    let kept = record(replace("a\n", "b\n"), false);
    let cut = record(replace("c\n", "d\n"), true);
    let (_, truncated) = lines_for(&[&kept, &cut]);
    assert!(truncated);
}

#[test]
fn an_edit_with_no_text_contributes_no_rows() {
    let empty = record(None, false);
    let (lines, truncated) = lines_for(&[&empty]);
    assert!(lines.is_empty());
    assert!(!truncated);
}

/// The mixed fixture as the adapter records it: two object-kind adds whose
/// text is the new file body, one object-kind update carrying a patch.
fn codex_add_records() -> Vec<AgentEditRecord> {
    let changes = json!([
        {"kind": {"type": "add"}, "path": "alpha.txt", "diff": "Alpha live check.\n"},
        {"kind": {"type": "add"}, "path": "beta.txt", "diff": "Beta live check.\n"},
        {"kind": {"type": "update"}, "path": "notes.txt",
         "diff": "@@ -1 +1 @@\n-vecchio\n+nuovo\n"},
    ]);
    crate::agent_edit::codex_file_change_edits(Some(&changes), None)
        .expect("changes")
        .into_iter()
        .enumerate()
        .map(|(index, edit)| AgentEditRecord {
            session_id: "s.a".to_string(),
            agent: "agent s.a".to_string(),
            tool_call_id: "call-1".to_string(),
            at_ms: 100 + index as u64,
            path: edit.path,
            text: edit.text,
            added: edit.added,
            removed: edit.removed,
            truncated: edit.truncated,
        })
        .collect()
}

#[test]
fn codex_add_content_rows_read_as_added_lines_and_the_update_as_a_patch() {
    let records = codex_add_records();
    let of = |path: &str| {
        records
            .iter()
            .filter(|record| record.path == path)
            .collect::<Vec<_>>()
    };
    let (alpha, _) = lines_for(&of("alpha.txt"));
    assert_eq!(
        alpha,
        vec![line(WorkspaceGitDiffLineKind::Add, "Alpha live check.")]
    );
    let (beta, _) = lines_for(&of("beta.txt"));
    assert_eq!(
        beta,
        vec![line(WorkspaceGitDiffLineKind::Add, "Beta live check.")]
    );
    let (notes, _) = lines_for(&of("notes.txt"));
    assert_eq!(
        notes,
        vec![
            line(WorkspaceGitDiffLineKind::Header, "@@ -1 +1 @@"),
            line(WorkspaceGitDiffLineKind::Remove, "vecchio"),
            line(WorkspaceGitDiffLineKind::Add, "nuovo"),
        ]
    );
}

#[test]
fn the_diff_reply_stops_at_its_byte_budget() {
    let body = "+abcdefghij\n".repeat(30_000);
    let record = record(Some(AgentEditText::Diff { text: body }), false);
    let (lines, truncated) = lines_for(&[&record]);
    assert!(truncated);
    let encoded = serde_json::to_vec(&lines).expect("encode");
    assert!(encoded.len() <= MAX_DIFF_REPLY_BYTES);
}
