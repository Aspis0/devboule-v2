//! The per-workspace fold: one row per path, the sums of what each call
//! recorded, and the newest edit's agent.

use devboule_protocol::{AgentEditText, SessionKind, WorkspaceIsolation};
use serde_json::json;

use super::{fold, MAX_CALLS_PER_FILE, MAX_FILES, MAX_REPLY_BYTES};
use crate::agent_edit_records::AgentEditRecord;
use crate::journal::{
    acp_envelope_record, new_session_record, now_ms, tmp_journal, Journal, ProjectRecord,
    WorkspaceRecord,
};

fn record(
    path: &str,
    session_id: &str,
    tool_call_id: &str,
    at_ms: u64,
    added: u64,
    removed: u64,
) -> AgentEditRecord {
    AgentEditRecord {
        session_id: session_id.to_string(),
        agent: format!("agent {session_id}"),
        tool_call_id: tool_call_id.to_string(),
        at_ms,
        path: path.to_string(),
        text: None,
        added,
        removed,
        truncated: false,
    }
}

#[test]
fn two_edits_of_one_file_are_one_row_that_sums_both() {
    let records = vec![
        record("src/lib.rs", "s.a", "call-1", 100, 1, 1),
        record("src/lib.rs", "s.b", "call-2", 200, 2, 0),
    ];
    let (files, totals, capped) = fold(&records);
    assert!(!capped);
    assert_eq!(files.len(), 1);
    let file = &files[0];
    assert_eq!((file.additions, file.deletions), (3, 1));
    assert_eq!(file.last_agent.as_deref(), Some("agent s.b"));
    assert_eq!(file.last_at_ms, 200);
    assert_eq!(file.calls.len(), 2);
    assert_eq!(file.calls[1].tool_call_id, "call-2");
    assert_eq!((totals.additions, totals.deletions), (3, 1));
}

#[test]
fn the_last_agent_is_the_newest_edit_whatever_order_it_arrived_in() {
    let records = vec![
        record("src/lib.rs", "s.b", "call-2", 200, 1, 0),
        record("src/lib.rs", "s.a", "call-1", 100, 1, 0),
    ];
    let (files, _, _) = fold(&records);
    assert_eq!(files[0].last_agent.as_deref(), Some("agent s.b"));
    assert_eq!(files[0].last_at_ms, 200);
}

#[test]
fn rows_are_sorted_by_path_and_the_totals_cover_them() {
    let records = vec![
        record("src/main.rs", "s.a", "call-1", 100, 4, 0),
        record("README.md", "s.a", "call-2", 110, 1, 2),
    ];
    let (files, totals, _) = fold(&records);
    let paths: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, ["README.md", "src/main.rs"]);
    assert_eq!((totals.additions, totals.deletions), (5, 2));
}

#[test]
fn files_past_the_cap_are_cut_and_the_reply_says_so() {
    let records: Vec<AgentEditRecord> = (0..=MAX_FILES)
        .map(|index| record(&format!("f{index:03}.txt"), "s.a", "call-1", 100, 1, 0))
        .collect();
    let (files, totals, capped) = fold(&records);
    assert!(capped);
    assert_eq!(files.len(), MAX_FILES);
    assert_eq!(totals.additions, MAX_FILES as u64);
}

#[cfg(windows)]
#[test]
fn two_spellings_of_one_file_on_windows_are_one_row() {
    let records = vec![
        record("Src/Lib.rs", "s.a", "call-1", 100, 1, 0),
        record("src/lib.rs", "s.b", "call-2", 200, 1, 1),
    ];
    let (files, _, _) = fold(&records);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].path, "src/lib.rs");
    assert_eq!((files[0].additions, files[0].deletions), (2, 1));
}

#[test]
fn a_file_past_the_call_cap_lists_its_first_calls_and_flags_its_row() {
    let records: Vec<AgentEditRecord> = (0..=MAX_CALLS_PER_FILE)
        .map(|index| record("src/lib.rs", "s.a", &format!("call-{index}"), 100, 1, 0))
        .collect();
    let (files, totals, _) = fold(&records);
    assert_eq!(files[0].calls.len(), MAX_CALLS_PER_FILE);
    assert!(files[0].truncated);
    assert_eq!(totals.additions, MAX_CALLS_PER_FILE as u64 + 1);
}

#[test]
fn the_reply_stops_adding_rows_past_its_byte_budget() {
    let long = "p".repeat(4_000);
    let records: Vec<AgentEditRecord> = (0..MAX_FILES)
        .map(|index| record(&format!("{long}{index:03}"), "s.a", "call-1", 100, 1, 0))
        .collect();
    let (files, _, capped) = fold(&records);
    assert!(capped);
    assert!(files.len() < MAX_FILES);
    let encoded = serde_json::to_vec(&files).expect("encode");
    assert!(encoded.len() <= MAX_REPLY_BYTES);
}

/// A Codex session of a workspace rooted at the fixture root.
fn codex_session(journal: &Journal, session_id: &str, workspace_id: &str) {
    journal
        .project_add(ProjectRecord {
            id: "p.1".to_string(),
            name: "Project".to_string(),
            path: crate::test_support::FIXTURE_ROOT.to_string(),
            git_state: "not_repository".to_string(),
            created_at_ms: now_ms(),
            updated_at_ms: now_ms(),
        })
        .expect("project row");
    journal
        .workspace_create(WorkspaceRecord {
            id: workspace_id.to_string(),
            project_id: "p.1".to_string(),
            title: "Workspace".to_string(),
            isolation: WorkspaceIsolation::Local,
            path: crate::test_support::FIXTURE_ROOT.to_string(),
            branch: None,
            created_at_ms: now_ms(),
            updated_at_ms: now_ms(),
        })
        .expect("workspace row");
    journal
        .upsert_blocking(new_session_record(
            session_id,
            "S-1-5-21-1",
            Some(workspace_id.to_string()),
            SessionKind::Codex,
            "Codex",
        ))
        .expect("session row");
}

/// One `fileChange` frame journaled for replay through the Codex reader.
fn codex_file_change_frame(
    journal: &Journal,
    session_id: &str,
    seq: u64,
    method: &str,
    status: &str,
    changes: &serde_json::Value,
) {
    let envelope = json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": {
            "threadId": "th",
            "turnId": "tu",
            "item": {
                "type": "fileChange",
                "id": "fc.1",
                "status": status,
                "changes": changes,
            },
        },
    });
    journal
        .append_blocking(
            acp_envelope_record(session_id.to_string(), 1, seq, &envelope).expect("row"),
        )
        .expect("append");
}

#[test]
fn codex_add_changes_fold_from_the_journalled_replay() {
    let (dir, path) = tmp_journal();
    let journal = Journal::open(&path).expect("open");
    codex_session(&journal, "s.codex.1", "ws.1");
    let changes = json!([
        {"kind": {"type": "add"}, "path": "alpha.txt", "diff": "Alpha live check.\n"},
        {"kind": {"type": "add"}, "path": "beta.txt", "diff": "Beta live check.\n"},
        {"kind": {"type": "update"}, "path": "notes.txt",
         "diff": "@@ -1 +1 @@\n-vecchio\n+nuovo\n"},
    ]);
    codex_file_change_frame(
        &journal,
        "s.codex.1",
        1,
        "item/started",
        "inProgress",
        &changes,
    );
    codex_file_change_frame(
        &journal,
        "s.codex.1",
        2,
        "item/completed",
        "completed",
        &changes,
    );

    let found = crate::agent_edit_records::read(&journal, "ws.1", now_ms());
    journal.shutdown();
    let _ = std::fs::remove_dir_all(&dir);

    assert!(found.error.is_none());
    let (files, totals, capped) = fold(&found.records);
    assert!(!capped);
    let paths: Vec<&str> = files.iter().map(|file| file.path.as_str()).collect();
    assert_eq!(paths, ["alpha.txt", "beta.txt", "notes.txt"]);
    assert_eq!((files[0].additions, files[0].deletions), (1, 0));
    assert_eq!((files[1].additions, files[1].deletions), (1, 0));
    assert_eq!((files[2].additions, files[2].deletions), (1, 1));
    assert_eq!((totals.additions, totals.deletions), (3, 1));
    let alpha = found
        .records
        .iter()
        .find(|record| record.path == "alpha.txt")
        .expect("alpha is replayed");
    assert_eq!(
        alpha.text,
        Some(AgentEditText::Replace {
            old: String::new(),
            new: "Alpha live check.\n".to_string(),
        })
    );
}
