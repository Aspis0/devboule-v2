//! The per-workspace fold: one row per path, the sums of what each call
//! recorded, and the newest edit's agent.

use super::{fold, MAX_CALLS_PER_FILE, MAX_FILES, MAX_REPLY_BYTES};
use crate::agent_edit_records::AgentEditRecord;

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
