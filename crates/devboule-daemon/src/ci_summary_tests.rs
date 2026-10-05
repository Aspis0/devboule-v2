//! The verdict: how checks add up to a state, what a failed job contributes,
//! and the bounds and redaction that keep a log from travelling whole.

use serde_json::json;

use super::{
    build, overall, parse_check_runs, Cause, CheckRun, CiState, MAX_EXCERPT_LINES, MAX_LINE_CHARS,
};
use crate::ci_test_support::{check_run, check_runs};

fn runs(items: &[serde_json::Value]) -> Vec<CheckRun> {
    parse_check_runs(&serde_json::from_str(&check_runs(items)).expect("json"))
}

#[test]
fn checks_add_up_to_one_state() {
    assert_eq!(
        overall(&runs(&[])),
        CiState::Queued,
        "nothing registered yet"
    );
    assert_eq!(
        overall(&runs(&[check_run(1, "build", "queued", None)])),
        CiState::Queued
    );
    assert_eq!(
        overall(&runs(&[
            check_run(1, "build", "completed", Some("failure")),
            check_run(2, "test", "in_progress", None),
        ])),
        CiState::Running,
        "a failure does not end the watch while other checks run"
    );
    assert_eq!(
        overall(&runs(&[
            check_run(1, "build", "completed", Some("success")),
            check_run(2, "lint", "completed", Some("skipped")),
        ])),
        CiState::Passed
    );
    assert_eq!(
        overall(&runs(&[
            check_run(1, "build", "completed", Some("success")),
            check_run(2, "test", "completed", Some("timed_out")),
        ])),
        CiState::Failed
    );
}

#[test]
fn the_run_id_is_read_from_the_job_url() {
    let parsed = runs(&[check_run(77, "build", "completed", Some("success"))]);
    assert_eq!(parsed[0].run_id, Some(900));
    assert_eq!(parsed[0].id, 77);
}

#[test]
fn ci_summary_redacts_and_caps_logs() {
    let secret = "ghp_abcdefghijklmnopqrstuvwxyz0123456789";
    let stamp = "2025-01-01T10:00:00.1234567Z";
    let mut log = String::new();
    log.push_str(&format!("{stamp} ##[group]Run cargo test\n"));
    log.push_str(&format!("{stamp} error: token {secret} leaked\n"));
    log.push_str(&format!(
        "{stamp} -----BEGIN RSA PRIVATE KEY----- error: leaked\n"
    ));
    log.push_str(&format!("{stamp} password=hunter2hunter2 error: bad\n"));
    log.push_str(&format!("{stamp} error[E0308]: {}\n", "x".repeat(2000)));
    for index in 0..30 {
        log.push_str(&format!(
            "{stamp} \u{1b}[31mthread main panicked at src/lib.rs:{index}\u{1b}[0m\n"
        ));
    }
    log.push_str("this is an ordinary line that matches nothing\n");
    let failing = runs(&[check_run(5, "test", "completed", Some("failure"))]);
    let verdict = build(&failing, &mut |_| Some(log.clone()));
    let job = &verdict.jobs[0];

    assert_eq!(job.excerpt.len(), MAX_EXCERPT_LINES, "ten lines at most");
    assert!(
        job.more_lines >= 20,
        "the rest is counted, not shown: {}",
        job.more_lines
    );
    for line in &job.excerpt {
        assert!(
            line.chars().count() <= MAX_LINE_CHARS + 1,
            "each line is capped: {line}"
        );
        assert!(!line.contains('\u{1b}'), "terminal escapes are stripped");
    }
    let text = verdict.render("CI failed");
    for leaked in [secret, "hunter2hunter2", "PRIVATE KEY"] {
        assert!(
            !text.contains(leaked),
            "{leaked} must not reach the summary:\n{text}"
        );
    }
    assert!(text.contains("[redacted-secret]"));
    assert!(
        text.contains("[truncated]"),
        "a truncation marker says lines were left out"
    );
    assert!(text.contains("run 900, job 5"), "ids are named: {text}");
    assert!(text.contains("https://github.com/acme/widgets/actions/runs/900/job/5"));
    assert!(
        !text.contains("ordinary line"),
        "only matched lines are quoted, never the whole log"
    );
    assert!(text.chars().count() < 6100, "the whole summary is bounded");
}

#[test]
fn a_cancelled_or_unstarted_job_is_infra_and_everything_else_is_code() {
    let mut unstarted = check_run(3, "deploy", "completed", Some("failure"));
    unstarted["output"]["summary"] =
        json!("The job was not acquired by Runner of type hosted even after multiple attempts");
    let items = [
        check_run(1, "build", "completed", Some("cancelled")),
        check_run(2, "test", "completed", Some("failure")),
        unstarted,
        check_run(4, "lint", "completed", Some("success")),
    ];
    let verdict = build(&runs(&items), &mut |_| Some("error: boom".to_string()));
    let causes: Vec<_> = verdict.jobs.iter().map(|job| job.cause.clone()).collect();
    assert_eq!(
        causes,
        vec![
            Some(Cause::Infra("the job was cancelled")),
            Some(Cause::Code),
            Some(Cause::Infra("the job was not acquired by a runner")),
            None,
        ]
    );
    let text = verdict.render("CI failed");
    assert!(text.contains("[INFRA: the job was cancelled]"), "{text}");
    assert!(text.contains("[CODE]"), "{text}");
    assert!(
        !verdict.only_infra(),
        "one code failure makes the verdict code"
    );

    let only = build(
        &runs(&[check_run(1, "build", "completed", Some("cancelled"))]),
        &mut |_| None,
    );
    assert!(only.only_infra());
}

#[test]
fn a_green_job_is_listed_without_its_log_being_read() {
    let items = [check_run(1, "build", "completed", Some("success"))];
    let verdict = build(&runs(&items), &mut |_| {
        panic!("a passing job's log is never fetched")
    });
    assert_eq!(verdict.state, CiState::Passed);
    assert_eq!(verdict.jobs[0].cause, None);
    assert!(verdict.render("CI passed").contains("- build: success"));
}
