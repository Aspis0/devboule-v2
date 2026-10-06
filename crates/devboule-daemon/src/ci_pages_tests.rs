//! Joining the pages of a commit's check runs: whole or refused, never a
//! verdict off part of the list.

use serde_json::{json, Value};

use super::join_check_run_pages;
use crate::ci_summary::{overall, CiState};
use crate::ci_test_support::{check_run, check_run_pages};

fn pages(text: &str) -> Vec<Value> {
    serde_json::from_str(text).expect("a page list")
}

#[test]
fn a_failed_check_on_a_later_page_is_not_lost() {
    let first: Vec<Value> = (1..=100)
        .map(|id| check_run(id, "build", "completed", Some("success")))
        .collect();
    let second = [check_run(101, "deploy", "completed", Some("failure"))];
    let joined =
        join_check_run_pages(&pages(&check_run_pages(&[&first, &second]))).expect("whole list");
    assert_eq!(joined.len(), 101);
    assert_eq!(
        overall(&joined),
        CiState::Failed,
        "page one alone would have read as passed"
    );
}

#[test]
fn a_page_that_never_arrived_refuses_the_list() {
    let first = [check_run(1, "build", "completed", Some("success"))];
    let text = json!([{"total_count": 3, "check_runs": first}]).to_string();
    let refused = join_check_run_pages(&pages(&text)).expect_err("two checks are missing");
    assert!(refused.retryable, "the watch asks again, it does not judge");
}

#[test]
fn pages_that_are_not_check_run_pages_refuse_the_list() {
    for text in [
        "[]",
        r#"[{"total_count": 1}]"#,
        r#"[{"total_count": 1, "check_runs": [{"name": "no id"}]}]"#,
        r#"["not a page"]"#,
    ] {
        let refused = join_check_run_pages(&pages(text)).expect_err(text);
        assert!(refused.retryable, "{text}");
    }
}

#[test]
fn an_empty_commit_is_an_empty_list_not_an_error() {
    let joined = join_check_run_pages(&pages(&check_run_pages(&[&[]]))).expect("one empty page");
    assert!(joined.is_empty());
}
