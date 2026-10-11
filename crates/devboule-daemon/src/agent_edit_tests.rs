//! The edit readers and the per-call budget: one provider frame in, one edit
//! list out.

use devboule_protocol::{AgentEditKind, AgentEditText};
use serde_json::json;

use super::{acp_diff_edits, claude_tool_edits, codex_file_change_edits, MAX_EDIT_CALL_BYTES};

#[test]
fn claude_edit_and_multiedit_keep_their_texts() {
    let edit = claude_tool_edits(
        "Edit",
        &json!({"file_path": "src/lib.rs", "old_string": "one", "new_string": "two"}),
        None,
    )
    .expect("edit");
    assert_eq!(edit.len(), 1);
    assert_eq!(edit[0].path, "src/lib.rs");
    assert_eq!(edit[0].kind, AgentEditKind::Edit);
    assert_eq!(
        edit[0].text,
        Some(AgentEditText::Replace {
            old: "one".to_string(),
            new: "two".to_string(),
        })
    );

    let multi = claude_tool_edits(
        "MultiEdit",
        &json!({
            "file_path": "src/lib.rs",
            "edits": [
                {"old_string": "a", "new_string": "b"},
                {"old_string": "c", "new_string": "d"},
            ]
        }),
        None,
    )
    .expect("multiedit");
    assert_eq!(multi.len(), 2);
    assert!(multi.iter().all(|edit| edit.path == "src/lib.rs"));
    assert_eq!(
        multi[1].text,
        Some(AgentEditText::Replace {
            old: "c".to_string(),
            new: "d".to_string(),
        })
    );
}

#[test]
fn claude_write_carries_only_the_new_text() {
    let edits = claude_tool_edits(
        "Write",
        &json!({"file_path": "notes.txt", "content": "hello\n"}),
        None,
    )
    .expect("write");
    assert_eq!(edits[0].kind, AgentEditKind::Write);
    assert_eq!(
        edits[0].text,
        Some(AgentEditText::Replace {
            old: String::new(),
            new: "hello\n".to_string(),
        })
    );
}

#[test]
fn claude_read_carries_no_edits() {
    assert!(
        claude_tool_edits("Read", &json!({"file_path": "src/lib.rs"}), None).is_none(),
        "a read changes no file"
    );
}

#[test]
fn codex_file_change_reads_every_change_and_skips_the_pathless_one() {
    let edits = codex_file_change_edits(
        Some(&json!([
            {"kind": "update", "path": "src/lib.rs", "diff": "@@ -1 +1 @@\n-old\n+new\n"},
            {"kind": "delete"},
            {"kind": "add", "path": "src/new.rs", "diff": "+hello\n"},
        ])),
        None,
    )
    .expect("changes");
    assert_eq!(edits.len(), 2);
    assert_eq!(
        edits[0].text,
        Some(AgentEditText::Diff {
            text: "@@ -1 +1 @@\n-old\n+new\n".to_string(),
        })
    );
    assert_eq!(edits[1].path, "src/new.rs");
    assert_eq!(edits[1].kind, AgentEditKind::Write);
}

#[test]
fn codex_change_without_a_diff_keeps_its_path_and_kind() {
    let edits =
        codex_file_change_edits(Some(&json!([{"kind": "delete", "path": "gone.txt"}])), None)
            .expect("change");
    assert_eq!(edits[0].kind, AgentEditKind::Delete);
    assert_eq!(edits[0].text, None);
}

#[test]
fn acp_diff_elements_become_replace_edits() {
    let edits = acp_diff_edits(
        Some(&json!([
            {"type": "content", "content": {"type": "text", "text": "note"}},
            {"type": "diff", "path": "src/lib.rs", "oldText": "one", "newText": "two"},
            {"type": "diff", "path": "gone.txt"},
        ])),
        Some("edit"),
        None,
    )
    .expect("diffs");
    assert_eq!(edits.len(), 2);
    assert_eq!(
        edits[0].text,
        Some(AgentEditText::Replace {
            old: "one".to_string(),
            new: "two".to_string(),
        })
    );
    // Neither side sent: the element names the file and carries no text.
    assert_eq!(edits[1].text, None);
}

#[test]
fn acp_diff_kind_follows_the_calls_own_kind() {
    let edits = acp_diff_edits(
        Some(&json!([{"type": "diff", "path": "gone.txt", "oldText": "bye"}])),
        Some("delete"),
        None,
    )
    .expect("diff");
    assert_eq!(edits[0].kind, AgentEditKind::Delete);
}

#[test]
fn a_text_over_the_call_budget_is_cut_and_flagged() {
    let content = "x".repeat(MAX_EDIT_CALL_BYTES + 10);
    let edits = claude_tool_edits(
        "Write",
        &json!({"file_path": "big.txt", "content": content}),
        None,
    )
    .expect("write");
    let Some(AgentEditText::Replace { new, .. }) = &edits[0].text else {
        panic!("expected a replace, got {:?}", edits[0].text);
    };
    assert_eq!(new.len(), MAX_EDIT_CALL_BYTES);
    assert!(edits[0].truncated);
}

#[test]
fn control_bytes_are_budgeted_by_their_json_size() {
    let content = "\u{1}".repeat(MAX_EDIT_CALL_BYTES);
    let edits = claude_tool_edits(
        "Write",
        &json!({"file_path": "ctl.bin", "content": content}),
        None,
    )
    .expect("write");
    let Some(AgentEditText::Replace { new, .. }) = &edits[0].text else {
        panic!("expected a replace, got {:?}", edits[0].text);
    };
    assert_eq!(new.chars().count(), MAX_EDIT_CALL_BYTES / 6);
    let encoded = serde_json::to_string(new).expect("encode");
    assert!(encoded.len() <= MAX_EDIT_CALL_BYTES + 2);
    assert!(edits[0].truncated);
}

#[test]
fn a_multibyte_cut_lands_on_a_character_boundary() {
    let content = "é".repeat(MAX_EDIT_CALL_BYTES);
    let edits = claude_tool_edits(
        "Write",
        &json!({"file_path": "accent.txt", "content": content}),
        None,
    )
    .expect("write");
    let Some(AgentEditText::Replace { new, .. }) = &edits[0].text else {
        panic!("expected a replace, got {:?}", edits[0].text);
    };
    assert_eq!(new.len(), MAX_EDIT_CALL_BYTES);
    assert!(edits[0].truncated);
}

#[test]
fn the_budget_is_shared_by_every_edit_of_the_call() {
    let first = "y".repeat(200 * 1024);
    let second = "z".repeat(100 * 1024);
    let edits = claude_tool_edits(
        "MultiEdit",
        &json!({
            "file_path": "src/lib.rs",
            "edits": [
                {"old_string": first, "new_string": ""},
                {"old_string": "later", "new_string": second},
                {"old_string": "gone", "new_string": "text"},
            ]
        }),
        None,
    )
    .expect("multiedit");
    assert_eq!(edits.len(), 3, "every edit still names the file");
    assert!(!edits[0].truncated);
    assert!(edits[1].truncated);
    let Some(AgentEditText::Replace { new, .. }) = &edits[1].text else {
        panic!("expected a replace, got {:?}", edits[1].text);
    };
    assert_eq!(new.len(), MAX_EDIT_CALL_BYTES - 200 * 1024 - "later".len());
    assert!(edits[2].truncated);
    assert_eq!(
        edits[2].text,
        Some(AgentEditText::Replace {
            old: String::new(),
            new: String::new(),
        }),
        "an edit past the budget keeps no text"
    );
}

#[test]
fn a_path_inside_the_workspace_is_relative_with_forward_slashes() {
    let cwd = std::path::Path::new(crate::test_support::FIXTURE_ROOT);
    let inside = crate::test_support::fixture_path("src/lib.rs");
    let edits = claude_tool_edits(
        "Write",
        &json!({"file_path": inside.to_string_lossy(), "content": "x"}),
        Some(cwd),
    )
    .expect("write");
    assert_eq!(edits[0].path, "src/lib.rs");
}

#[test]
fn a_path_outside_the_workspace_keeps_the_providers_spelling() {
    let cwd = std::path::Path::new(crate::test_support::FIXTURE_ROOT);
    let outside = cwd
        .parent()
        .expect("the fixture root has a parent")
        .join("other.txt");
    let edits = claude_tool_edits(
        "Write",
        &json!({"file_path": outside.to_string_lossy(), "content": "x"}),
        Some(cwd),
    )
    .expect("write");
    assert_eq!(edits[0].path, outside.to_string_lossy());
}

#[test]
fn a_replace_counts_the_lines_it_removed_and_added() {
    let edit = claude_tool_edits(
        "Edit",
        &json!({"file_path": "src/lib.rs", "old_string": "a\nb\nc\n", "new_string": "a\nB\nc\n"}),
        None,
    )
    .expect("edit");
    assert_eq!((edit[0].added, edit[0].removed), (1, 1));
}

#[test]
fn a_write_counts_every_line_as_added() {
    let edits = claude_tool_edits(
        "Write",
        &json!({"file_path": "notes.txt", "content": "a\nb\n"}),
        None,
    )
    .expect("write");
    assert_eq!((edits[0].added, edits[0].removed), (2, 0));
}

#[test]
fn a_codex_diff_counts_its_hunk_lines() {
    let edits = codex_file_change_edits(
        Some(&json!([
            {"kind": "update", "path": "src/lib.rs", "diff": "@@ -1 +1 @@\n-old\n+new\n"},
            {"kind": "delete", "path": "gone.txt", "diff": "-x\n-y\n"},
        ])),
        None,
    )
    .expect("changes");
    assert_eq!((edits[0].added, edits[0].removed), (1, 1));
    assert_eq!((edits[1].added, edits[1].removed), (0, 2));
}

#[test]
fn an_acp_delete_counts_the_old_lines_it_removed() {
    let edits = acp_diff_edits(
        Some(&json!([{"type": "diff", "path": "gone.txt", "oldText": "bye\n"}])),
        Some("delete"),
        None,
    )
    .expect("diff");
    assert_eq!((edits[0].added, edits[0].removed), (0, 1));
}

#[test]
fn counts_come_from_the_whole_text_not_the_cut_one() {
    let content = "line\n".repeat(60_000);
    let edits = claude_tool_edits(
        "Write",
        &json!({"file_path": "big.txt", "content": content}),
        None,
    )
    .expect("write");
    assert!(edits[0].truncated);
    assert_eq!(edits[0].added, 60_000);
}
